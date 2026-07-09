// 本地持久化历史库（SQLite）。
//
// 目的：Kiro 原始数据（sessions/logs/state.vscdb）会因为切账号、日志滚动
// (3-7 天)、v1→v2 迁移、未来版本更新等原因被覆盖或清除。工具启动时先
// snapshot 一份到自己的库，之后 Kiro 那边怎么变，本地库存都在。
//
// 存储位置：`%APPDATA%/kiro-usage-dashboard/history.db` (Windows)
// / `~/Library/Application Support/kiro-usage-dashboard/` (macOS)
// / `~/.local/share/kiro-usage-dashboard/` (Linux)
//
// 表结构：
//   turns             主键 execution_id (turn 全局唯一)
//   v1_sessions       主键 session_key = workspace_full + '::' + session_id
//   quota_snapshots   主键 snap_key    = uid + '::' + ts_secs
//   meta              schema_version 等
//
// 合并策略：INSERT OR IGNORE（保守 —— 不覆盖已有记录，避免误覆盖真实历史）。
//
// 并发：内部 parking_lot::Mutex<Connection>。tauri 命令是多线程调度的，
// 必须保护 Connection（rusqlite Connection 非 Sync）。

use crate::models::{HistoryStats, QuotaSnapshot, Turn, V1Session};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OpenFlags};
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i32 = 1;

/// SQL DDL：所有表在 open 时保证存在，允许多次运行。
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS turns (
    execution_id     TEXT PRIMARY KEY,
    ts_ms            INTEGER NOT NULL,
    agent_session_id TEXT NOT NULL,
    session_id       TEXT NOT NULL,
    workspace        TEXT NOT NULL,
    credits          REAL NOT NULL,
    elapsed_ms       INTEGER NOT NULL,
    status           TEXT NOT NULL,
    model            TEXT NOT NULL,
    title            TEXT NOT NULL,
    tools_json       TEXT NOT NULL,
    first_seen_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turns_ts ON turns(ts_ms);
CREATE INDEX IF NOT EXISTS idx_turns_aid ON turns(agent_session_id);

CREATE TABLE IF NOT EXISTS v1_sessions (
    session_key    TEXT PRIMARY KEY,
    ts_ms          INTEGER NOT NULL,
    session_id     TEXT NOT NULL,
    title          TEXT NOT NULL,
    workspace      TEXT NOT NULL,
    workspace_full TEXT NOT NULL,
    turn_count     INTEGER NOT NULL,
    model          TEXT NOT NULL,
    first_seen_ms  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_v1_ts ON v1_sessions(ts_ms);

CREATE TABLE IF NOT EXISTS quota_snapshots (
    snap_key       TEXT PRIMARY KEY,
    ts_ms          INTEGER NOT NULL,
    uid            TEXT,
    current_usage  REAL NOT NULL,
    usage_limit    INTEGER NOT NULL,
    first_seen_ms  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_quota_uid ON quota_snapshots(uid);
CREATE INDEX IF NOT EXISTS idx_quota_ts  ON quota_snapshots(ts_ms);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// HistoryStore
// ---------------------------------------------------------------------------

pub struct HistoryStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl HistoryStore {
    /// 打开/创建持久化数据库。父目录不存在会自动创建。
    ///
    /// 失败场景：目录创建失败（权限）、磁盘满、SQLite 打开失败。
    /// 调用方（`main.rs`）通常应该 panic：没有持久化，工具的核心价值就没了。
    pub fn open(db_path: PathBuf) -> Result<Self, String> {
        // 保证父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建历史库目录失败 {}: {}", parent.display(), e))?;
        }

        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| format!("打开历史库失败 {}: {}", db_path.display(), e))?;

        // 打开 WAL 以获得更好的写并发（多 IPC 命令可能并发触发 upsert）
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");

        // 建表（幂等）
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| format!("初始化 schema 失败: {}", e))?;

        // 写 schema_version
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )
        .map_err(|e| format!("写 meta.schema_version 失败: {}", e))?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path,
        })
    }

    /// 数据库文件路径（供 UI 显示）。
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    // -----------------------------------------------------------------------
    // upsert：把当前扫描到的数据合并进历史库（INSERT OR IGNORE, 不覆盖已有）
    // -----------------------------------------------------------------------

    /// 追加 turns。返回新插入的行数（已存在的 execution_id 视为忽略）。
    pub fn upsert_turns(&self, turns: &[Turn]) -> Result<usize, String> {
        if turns.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let now = crate::util::now_ms();
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin tx (turns): {}", e))?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR IGNORE INTO turns
                     (execution_id, ts_ms, agent_session_id, session_id, workspace,
                      credits, elapsed_ms, status, model, title, tools_json, first_seen_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )
                .map_err(|e| format!("prepare (turns): {}", e))?;
            for t in turns {
                if t.eid.is_empty() {
                    // 没有 execution_id 无法去重，跳过（scanner 应保证 eid 非空）
                    continue;
                }
                let tools_json = serde_json::to_string(&t.tools).unwrap_or_else(|_| "[]".into());
                let changes = stmt
                    .execute(params![
                        t.eid,
                        t.t,
                        t.aid,
                        t.sid,
                        t.ws,
                        t.c,
                        t.e,
                        t.s,
                        t.model,
                        t.title,
                        tools_json,
                        now,
                    ])
                    .map_err(|e| format!("execute (turns): {}", e))?;
                inserted += changes;
            }
        }
        tx.commit()
            .map_err(|e| format!("commit tx (turns): {}", e))?;
        Ok(inserted)
    }

    /// 追加 v1 sessions。返回新插入行数。
    pub fn upsert_v1_sessions(&self, sessions: &[V1Session]) -> Result<usize, String> {
        if sessions.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let now = crate::util::now_ms();
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin tx (v1): {}", e))?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR IGNORE INTO v1_sessions
                     (session_key, ts_ms, session_id, title, workspace,
                      workspace_full, turn_count, model, first_seen_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| format!("prepare (v1): {}", e))?;
            for s in sessions {
                // 去重键 = workspace_full + '::' + session_id
                // 空 workspace_full 时用 "(no-workspace)" 兜底，避免空串导致 primary key 冲突
                let ws = if s.workspace_full.is_empty() {
                    "(no-workspace)"
                } else {
                    s.workspace_full.as_str()
                };
                let key = format!("{}::{}", ws, s.session_id);
                let changes = stmt
                    .execute(params![
                        key,
                        s.t,
                        s.session_id,
                        s.title,
                        s.workspace,
                        s.workspace_full,
                        s.turn_count as i64,
                        s.model,
                        now,
                    ])
                    .map_err(|e| format!("execute (v1): {}", e))?;
                inserted += changes;
            }
        }
        tx.commit()
            .map_err(|e| format!("commit tx (v1): {}", e))?;
        Ok(inserted)
    }

    /// 追加 quota snapshots。返回新插入行数。
    ///
    /// 去重键 = uid + '::' + (ts_ms / 1000)。同秒同账号视为重复。
    pub fn upsert_quota_snapshots(&self, snaps: &[QuotaSnapshot]) -> Result<usize, String> {
        if snaps.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let now = crate::util::now_ms();
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin tx (quota): {}", e))?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR IGNORE INTO quota_snapshots
                     (snap_key, ts_ms, uid, current_usage, usage_limit, first_seen_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| format!("prepare (quota): {}", e))?;
            for s in snaps {
                let uid_val = s.uid.clone().unwrap_or_else(|| "(unknown)".to_string());
                let key = format!("{}::{}", uid_val, s.t / 1000);
                let changes = stmt
                    .execute(params![
                        key,
                        s.t,
                        s.uid,
                        s.current,
                        s.limit,
                        now,
                    ])
                    .map_err(|e| format!("execute (quota): {}", e))?;
                inserted += changes;
            }
        }
        tx.commit()
            .map_err(|e| format!("commit tx (quota): {}", e))?;
        Ok(inserted)
    }

    // -----------------------------------------------------------------------
    // load：从历史库读全量（用于合并展示 / 前端渲染）
    // -----------------------------------------------------------------------

    pub fn load_all_turns(&self) -> Result<Vec<Turn>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT execution_id, ts_ms, agent_session_id, session_id, workspace,
                        credits, elapsed_ms, status, model, title, tools_json
                 FROM turns
                 ORDER BY ts_ms ASC",
            )
            .map_err(|e| format!("prepare (load turns): {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                let tools_json: String = row.get(10)?;
                let tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();
                Ok(Turn {
                    eid: row.get(0)?,
                    t: row.get(1)?,
                    aid: row.get(2)?,
                    sid: row.get(3)?,
                    ws: row.get(4)?,
                    c: row.get(5)?,
                    e: row.get(6)?,
                    s: row.get(7)?,
                    model: row.get(8)?,
                    title: row.get(9)?,
                    tools,
                })
            })
            .map_err(|e| format!("query (load turns): {}", e))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("row (load turns): {}", e))?);
        }
        Ok(out)
    }

    pub fn load_all_v1_sessions(&self) -> Result<Vec<V1Session>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, title, workspace, workspace_full, ts_ms, turn_count, model
                 FROM v1_sessions
                 ORDER BY ts_ms ASC",
            )
            .map_err(|e| format!("prepare (load v1): {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                let turn_count: i64 = row.get(5)?;
                Ok(V1Session::new(
                    row.get(0)?, // session_id
                    row.get(1)?, // title
                    row.get(2)?, // workspace
                    row.get(3)?, // workspace_full
                    row.get(4)?, // t
                    turn_count.max(0) as usize,
                    row.get(6)?, // model
                ))
            })
            .map_err(|e| format!("query (load v1): {}", e))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("row (load v1): {}", e))?);
        }
        Ok(out)
    }

    pub fn load_all_quota_snapshots(&self) -> Result<Vec<QuotaSnapshot>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT ts_ms, uid, current_usage, usage_limit
                 FROM quota_snapshots
                 ORDER BY ts_ms ASC",
            )
            .map_err(|e| format!("prepare (load quota): {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                let uid: Option<String> = row.get(1)?;
                Ok(QuotaSnapshot {
                    t: row.get(0)?,
                    uid,
                    current: row.get(2)?,
                    limit: row.get(3)?,
                })
            })
            .map_err(|e| format!("query (load quota): {}", e))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("row (load quota): {}", e))?);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // stats / clear
    // -----------------------------------------------------------------------

    /// 统计信息（供前端显示）。
    pub fn stats(&self) -> HistoryStats {
        let conn = self.conn.lock();

        let turns_count = count_rows(&conn, "turns").unwrap_or(0);
        let v1_count = count_rows(&conn, "v1_sessions").unwrap_or(0);
        let quota_count = count_rows(&conn, "quota_snapshots").unwrap_or(0);

        // 三张表 ts_ms 的 min / max
        let earliest = min_ts_across(&conn);
        let latest = max_ts_across(&conn);

        let size = std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);

        HistoryStats {
            turns_count,
            v1_sessions_count: v1_count,
            quota_snapshots_count: quota_count,
            earliest_ts: earliest,
            latest_ts: latest,
            db_path: self.db_path.to_string_lossy().to_string(),
            db_size_bytes: size,
            last_upserted: 0,
        }
    }

    /// 清空历史库三张表。返回清除前的统计（用于前端 toast "已清除 X 条"）。
    pub fn clear_all(&self) -> Result<HistoryStats, String> {
        let before = self.stats();
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin tx (clear): {}", e))?;
        tx.execute("DELETE FROM turns", [])
            .map_err(|e| format!("delete turns: {}", e))?;
        tx.execute("DELETE FROM v1_sessions", [])
            .map_err(|e| format!("delete v1_sessions: {}", e))?;
        tx.execute("DELETE FROM quota_snapshots", [])
            .map_err(|e| format!("delete quota_snapshots: {}", e))?;
        tx.commit()
            .map_err(|e| format!("commit tx (clear): {}", e))?;

        // VACUUM 缩文件（在 tx 外执行；VACUUM 不能在事务内）
        let _ = conn.execute_batch("VACUUM");
        Ok(before)
    }
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn count_rows(conn: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {}", table);
    conn.query_row(&sql, [], |row| {
        let n: i64 = row.get(0)?;
        Ok(n as usize)
    })
    .map_err(|e| format!("count({}): {}", table, e))
}

/// 三张表 ts_ms 的最小值。空库返回 None。
fn min_ts_across(conn: &Connection) -> Option<i64> {
    let sql = "SELECT MIN(m) FROM (
                 SELECT MIN(ts_ms) AS m FROM turns
                 UNION ALL SELECT MIN(ts_ms) FROM v1_sessions
                 UNION ALL SELECT MIN(ts_ms) FROM quota_snapshots
               )";
    conn.query_row(sql, [], |row| row.get::<_, Option<i64>>(0))
        .ok()
        .flatten()
}

fn max_ts_across(conn: &Connection) -> Option<i64> {
    let sql = "SELECT MAX(m) FROM (
                 SELECT MAX(ts_ms) AS m FROM turns
                 UNION ALL SELECT MAX(ts_ms) FROM v1_sessions
                 UNION ALL SELECT MAX(ts_ms) FROM quota_snapshots
               )";
    conn.query_row(sql, [], |row| row.get::<_, Option<i64>>(0))
        .ok()
        .flatten()
}

// ---------------------------------------------------------------------------
// 数据目录定位
// ---------------------------------------------------------------------------

/// 历史库默认位置：
///   Windows: `%APPDATA%/kiro-usage-dashboard/history.db`
///   macOS:   `~/Library/Application Support/kiro-usage-dashboard/history.db`
///   Linux:   `~/.local/share/kiro-usage-dashboard/history.db`
pub fn default_history_db() -> PathBuf {
    let base = dirs::data_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kiro-usage-dashboard").join("history.db")
}
