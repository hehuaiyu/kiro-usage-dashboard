// v2 sessions 扫描器：递归 `~/.kiro/sessions/<sid>/<aid>/messages.jsonl`，
// 抽出 payload.type == "usage_summary" 的事件，配合 session.json 里的
// workspace/title/model 元信息组成 Turn 列表。
//
// 缓存策略：按每个 messages.jsonl 的 mtime 增量。mtime 不变直接复用；
// 变了才重解析。热请求 < 50ms（覆盖 30 个 session、300+ turn）。
//
// 对应 Python 原型的 `TurnCache` 类，逻辑 1:1。

use crate::models::{ScanStats, Turn};
use crate::util;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct TurnCache {
    sessions_root: PathBuf,
    inner: Mutex<Inner>,
}

/// 内部状态：两级缓存。
struct Inner {
    /// {messages.jsonl 路径: (mtime_secs, 解析出的 turn 列表)}
    turn_cache: HashMap<PathBuf, (u64, Vec<Turn>)>,
    /// {session.json 路径: (mtime_secs, 元信息)}
    meta_cache: HashMap<PathBuf, (u64, SessionMeta)>,
}

#[derive(Clone, Default)]
struct SessionMeta {
    /// workspace 末段目录名
    workspace: String,
    title: String,
    model: String,
}

impl TurnCache {
    pub fn new(sessions_root: PathBuf) -> Self {
        Self {
            sessions_root,
            inner: Mutex::new(Inner {
                turn_cache: HashMap::new(),
                meta_cache: HashMap::new(),
            }),
        }
    }

    /// 扫描全部 messages.jsonl，返回 (按时间升序的 turn 列表, 扫描统计)。
    pub fn scan(&self) -> (Vec<Turn>, ScanStats) {
        let start = Instant::now();

        if !self.sessions_root.is_dir() {
            return (
                Vec::new(),
                ScanStats {
                    files: 0,
                    reparsed: 0,
                    reused: 0,
                    took_ms: start.elapsed().as_millis() as u64,
                },
            );
        }

        let mut inner = self.inner.lock();
        let mut all_turns: Vec<Turn> = Vec::new();
        let mut files_seen = 0usize;
        let mut reparsed = 0usize;
        let mut reused = 0usize;
        let mut active: HashSet<PathBuf> = HashSet::new();

        // 一层：`<sessions_root>/<sessionId>/`
        let sids_iter = match std::fs::read_dir(&self.sessions_root) {
            Ok(rd) => rd,
            Err(_) => {
                return (
                    Vec::new(),
                    ScanStats {
                        files: 0,
                        reparsed: 0,
                        reused: 0,
                        took_ms: start.elapsed().as_millis() as u64,
                    },
                );
            }
        };

        for sid_entry in sids_iter.flatten() {
            let sid_path = sid_entry.path();
            if !sid_path.is_dir() {
                continue;
            }
            let sid = sid_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            // 二层：`<sessions_root>/<sessionId>/<agentSessionId>/`
            let aids_iter = match std::fs::read_dir(&sid_path) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for aid_entry in aids_iter.flatten() {
                let aid_path = aid_entry.path();
                if !aid_path.is_dir() {
                    continue;
                }
                let msg_path = aid_path.join("messages.jsonl");
                if !msg_path.is_file() {
                    continue;
                }

                files_seen += 1;
                active.insert(msg_path.clone());

                let mtime = match util::file_mtime_secs(&msg_path) {
                    Some(m) => m,
                    None => continue,
                };

                // 增量：mtime 不变 → 直接复用
                if let Some((cached_mtime, cached_turns)) = inner.turn_cache.get(&msg_path) {
                    if *cached_mtime == mtime {
                        all_turns.extend(cached_turns.iter().cloned());
                        reused += 1;
                        continue;
                    }
                }

                // 需要重新解析：先拿元信息（也带 mtime 缓存）
                let session_json = aid_path.join("session.json");
                let meta = Self::load_meta(&session_json, &mut inner.meta_cache);

                let aid = aid_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                let turns = Self::parse_messages(&msg_path, &meta, &sid, &aid);
                all_turns.extend(turns.iter().cloned());
                inner.turn_cache.insert(msg_path, (mtime, turns));
                reparsed += 1;
            }
        }

        // 清 stale：磁盘上已经不存在的文件从 cache 里剔除
        inner.turn_cache.retain(|k, _| active.contains(k));

        all_turns.sort_by_key(|t| t.t);

        let stats = ScanStats {
            files: files_seen,
            reparsed,
            reused,
            took_ms: start.elapsed().as_millis() as u64,
        };
        (all_turns, stats)
    }

    /// 读 session.json（带 mtime 缓存），拿 workspace/title/model。
    fn load_meta(
        session_json: &Path,
        cache: &mut HashMap<PathBuf, (u64, SessionMeta)>,
    ) -> SessionMeta {
        let mtime = match util::file_mtime_secs(session_json) {
            Some(m) => m,
            None => return SessionMeta::default(),
        };
        if let Some((cached_mtime, cached)) = cache.get(session_json) {
            if *cached_mtime == mtime {
                return cached.clone();
            }
        }
        let content = match std::fs::read_to_string(session_json) {
            Ok(s) => s,
            Err(_) => return SessionMeta::default(),
        };
        let j: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return SessionMeta::default(),
        };
        let workspace_full = j
            .get("workspacePaths")
            .and_then(|arr| arr.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let meta = SessionMeta {
            workspace: util::basename(workspace_full),
            title: j.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            model: j.get("modelId").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        };
        cache.insert(session_json.to_path_buf(), (mtime, meta.clone()));
        meta
    }

    /// 解析一个 messages.jsonl，抽 usage_summary 事件。
    fn parse_messages(msg_path: &Path, meta: &SessionMeta, sid: &str, aid: &str) -> Vec<Turn> {
        let file = match std::fs::File::open(msg_path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let reader = BufReader::new(file);
        let mut turns: Vec<Turn> = Vec::new();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            // 快速子串预筛（跟 Python 版一致），避免每行都 json.parse
            if !line.contains(r#""type":"usage_summary""#) {
                continue;
            }
            let ev: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let payload = match ev.get("payload") {
                Some(p) => p,
                None => continue,
            };
            // 严格 type 检查（防止子串预筛误命中其它字段里同样的字符串）
            if payload.get("type").and_then(|v| v.as_str()) != Some("usage_summary") {
                continue;
            }

            let ts_str = ev.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let t = match util::iso_to_ms(ts_str) {
                Some(v) => v,
                None => continue,
            };

            // promptTurnSummaries[0] 可能不存在（aborted 时）
            let (credits, tools) = payload
                .get("promptTurnSummaries")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .map(|summ| {
                    let c = summ.get("usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let tools_vec: Vec<String> = summ
                        .get("usedTools")
                        .and_then(|a| a.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    (c, tools_vec)
                })
                .unwrap_or((0.0, Vec::new()));

            let elapsed = payload
                .get("elapsedTime")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let status = payload
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let eid = payload
                .get("executionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            turns.push(Turn {
                t,
                c: credits,
                e: elapsed,
                s: status,
                ws: meta.workspace.clone(),
                sid: sid.to_string(),
                aid: aid.to_string(),
                eid,
                title: meta.title.clone(),
                model: meta.model.clone(),
                tools,
            });
        }
        turns
    }
}
