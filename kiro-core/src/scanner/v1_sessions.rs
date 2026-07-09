// v1 sessions 扫描器：遍历
// `<APPDATA>/Kiro/User/globalStorage/kiro.kiroagent/workspace-sessions/<encoded_ws>/*.json`
// 每个 UUID.json 是一个 v1 时代的完整会话。v1 时期 Kiro 还没引入 credits 追踪，
// 所以这里只能挖出 turn 数（从 history 数出）+ 会话元信息。
//
// 关键处理：
//   - 跳过 `sessions.json`（索引）和 `._migration-*.json`（迁移标记，不含内容）
//   - workspace 目录名用 Kiro base64 变体解码（见 util::decode_kiro_ws_name）
//   - turn 数取 executionId 去重数 与 role=user 消息数 的较大者
//   - 时间戳优先来自 sessions.json 索引里的 dateCreated，否则用文件 mtime
//
// 对应 Python 原型的 `V1SessionCache` 类。

use crate::models::{ScanStats, V1Session};
use crate::util;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct V1SessionCache {
    root: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    /// {UUID.json 路径: (mtime_secs, 解析结果)}
    /// 值可能是 None——parse 失败也缓存，下次不再重试同样坏的文件
    cache: HashMap<PathBuf, (u64, Option<V1Session>)>,
}

impl V1SessionCache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            inner: Mutex::new(Inner {
                cache: HashMap::new(),
            }),
        }
    }

    /// 扫描全部 workspace-sessions，返回 (按时间升序的 v1 会话, 统计)。
    pub fn scan(&self) -> (Vec<V1Session>, ScanStats) {
        let start = Instant::now();

        if !self.root.is_dir() {
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
        let mut results: Vec<V1Session> = Vec::new();
        let mut files_seen = 0usize;
        let mut reparsed = 0usize;
        let mut reused = 0usize;
        let mut active: HashSet<PathBuf> = HashSet::new();

        // 一层: 每个 encoded workspace 目录
        let ws_iter = match std::fs::read_dir(&self.root) {
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

        for ws_entry in ws_iter.flatten() {
            let ws_dir = ws_entry.path();
            if !ws_dir.is_dir() {
                continue;
            }

            let ws_encoded = match ws_dir.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => continue,
            };
            let workspace_full = util::decode_kiro_ws_name(&ws_encoded);
            let workspace_base = util::basename(&workspace_full);

            // sessions.json 索引（可能没有；有则拿 dateCreated / title）
            let idx_map = Self::load_sessions_index(&ws_dir);

            // 二层: 每个 <uuid>.json
            let entries = match std::fs::read_dir(&ws_dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let fp = entry.path();
                let fname = match fp.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                // 跳过索引和迁移标记
                if fname == "sessions.json" {
                    continue;
                }
                if fname.starts_with("._migration-") {
                    continue;
                }
                if !fname.ends_with(".json") {
                    continue;
                }
                if fname.len() <= 5 {
                    continue; // 就是 ".json" 之类
                }

                files_seen += 1;
                active.insert(fp.clone());

                let mtime = match util::file_mtime_secs(&fp) {
                    Some(m) => m,
                    None => continue,
                };

                // 增量：mtime 不变直接复用（包括 None 缓存）
                if let Some((cached_mtime, cached_opt)) = inner.cache.get(&fp) {
                    if *cached_mtime == mtime {
                        if let Some(s) = cached_opt.clone() {
                            results.push(s);
                        }
                        reused += 1;
                        continue;
                    }
                }

                // 需重解析
                let sid_prefix = &fname[..fname.len() - 5]; // 去 ".json"
                let idx_info = idx_map.get(sid_prefix);
                let parsed = Self::parse_session(&fp, &workspace_base, &workspace_full, idx_info);
                if let Some(ref s) = parsed {
                    results.push(s.clone());
                }
                inner.cache.insert(fp, (mtime, parsed));
                reparsed += 1;
            }
        }

        // 清 stale
        inner.cache.retain(|k, _| active.contains(k));

        results.sort_by_key(|s| s.t);

        let stats = ScanStats {
            files: files_seen,
            reparsed,
            reused,
            took_ms: start.elapsed().as_millis() as u64,
        };
        (results, stats)
    }

    /// 读 `<ws>/sessions.json` 索引，返回 sessionId → 索引 item 的映射。
    fn load_sessions_index(ws_dir: &Path) -> HashMap<String, Value> {
        let idx_path = ws_dir.join("sessions.json");
        if !idx_path.is_file() {
            return HashMap::new();
        }
        let content = match std::fs::read_to_string(&idx_path) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };
        let j: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return HashMap::new(),
        };
        let mut map = HashMap::new();
        if let Some(arr) = j.as_array() {
            for item in arr {
                if let Some(sid) = item.get("sessionId").and_then(|v| v.as_str()) {
                    map.insert(sid.to_string(), item.clone());
                }
            }
        }
        map
    }

    /// 解析一个 v1 UUID.json，转成 V1Session。
    fn parse_session(
        fp: &Path,
        workspace_base: &str,
        workspace_full: &str,
        idx_info: Option<&Value>,
    ) -> Option<V1Session> {
        let content = std::fs::read_to_string(fp).ok()?;
        let data: Value = serde_json::from_str(&content).ok()?;

        // sessionId：优先文件内容，否则从文件名（去 .json）
        let sid = data
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                fp.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });

        // turn 数：executionId 去重数 vs role=user 消息数，取较大者
        let (exec_count, user_count) = match data.get("history").and_then(|v| v.as_array()) {
            Some(arr) => {
                let mut exec_ids: HashSet<String> = HashSet::new();
                let mut user_msgs = 0usize;
                for h in arr {
                    if let Some(eid) = h.get("executionId").and_then(|v| v.as_str()) {
                        exec_ids.insert(eid.to_string());
                    }
                    if h.get("message")
                        .and_then(|m| m.get("role"))
                        .and_then(|v| v.as_str())
                        == Some("user")
                    {
                        user_msgs += 1;
                    }
                }
                (exec_ids.len(), user_msgs)
            }
            None => (0, 0),
        };
        let turn_count = exec_count.max(user_count);

        // model：优先 selectedModel.title，其次 config.models[0].title / .model
        let mut model = String::new();
        if let Some(models_arr) = data
            .get("config")
            .and_then(|c| c.get("models"))
            .and_then(|m| m.as_array())
        {
            if let Some(first) = models_arr.first() {
                if let Some(t) = first.get("title").and_then(|v| v.as_str()) {
                    model = t.to_string();
                } else if let Some(m) = first.get("model").and_then(|v| v.as_str()) {
                    model = m.to_string();
                }
            }
        }
        if let Some(sel_title) = data
            .get("selectedModel")
            .and_then(|s| s.get("title"))
            .and_then(|v| v.as_str())
        {
            model = sel_title.to_string(); // selectedModel 更权威，覆盖
        }

        // 时间戳：优先 sessions.json 索引里的 dateCreated
        //         （字符串形式 "1720000000000" 或数字），fallback 文件 mtime
        let ts_ms = idx_info
            .and_then(|i| i.get("dateCreated"))
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .or_else(|| v.as_i64())
            })
            .or_else(|| util::file_mtime_ms(fp))?;

        // title：优先文件里 title，其次索引
        let title = data
            .get("title")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                idx_info
                    .and_then(|i| i.get("title"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();

        Some(V1Session::new(
            sid,
            title,
            workspace_base.to_string(),
            workspace_full.to_string(),
            ts_ms,
            turn_count,
            model,
        ))
    }
}
