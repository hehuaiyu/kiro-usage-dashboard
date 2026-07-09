// quota 历史扫描器：从 `%APPDATA%/Kiro/logs/**/*.log` 里挖每次拉配额的响应快照。
//
// 每条快照包含 `currentUsageWithPrecision` + `usageLimit` + 附近的 `userId`。
// 按账号分组之后，可以看到每个账号的 `currentUsage` 时间序列 + 归零/重置事件。
//
// 关键实现细节：
//   - 用 `regex::bytes` 直接匹配 &[u8]，避免整份文件 UTF-8 decode 的开销
//   - 时间戳三级兜底: 行内 [info] 前缀 > 目录名 YYYYMMDDTHHMMSS > 文件 mtime
//   - 行内正则强制后跟 `[` 或 `|`，避免误匹配 payload 里 `"nextDateReset": "2026-08-01T..."`
//   - 缓存按每个 .log 文件的 mtime 增量
//
// 对应 Python 原型的 `QuotaHistoryCache` 类。

use crate::models::{Account, QuotaSnapshot, ScanStats};
use crate::util;
use chrono::NaiveDateTime;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

// currentUsageWithPrecision + usageLimit（跨最多 600 字节的响应结构）
static COMBO_PAT: Lazy<BytesRegex> = Lazy::new(|| {
    BytesRegex::new(
        r#""currentUsageWithPrecision"\s*:\s*([\d.]+)[\s\S]{0,600}?"usageLimit"\s*:\s*(\d+)"#,
    )
    .expect("COMBO_PAT")
});

// Kiro userId 格式: d-XXXXXX.XXXX-XXXX-XXXX-XXXX
static UID_PAT: Lazy<BytesRegex> =
    Lazy::new(|| BytesRegex::new(r#""userId"\s*:\s*"(d-[0-9a-f]+\.[0-9a-f-]+)""#).expect("UID_PAT"));

// 日志行头时间戳: "YYYY-MM-DD HH:MM:SS.mmm [xxx]" 或 "... |"
// **关键**: 强制后跟 [ 或 |，避免误匹配 payload 里 nextDateReset 的 ISO 时间
static TS_LINE_PAT: Lazy<BytesRegex> = Lazy::new(|| {
    BytesRegex::new(r#"(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(?:\[|\|)"#)
        .expect("TS_LINE_PAT")
});

// 目录名: YYYYMMDDTHHMMSS(...更多字符)
static DIR_TS_PAT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d{8})T(\d{6})").expect("DIR_TS_PAT"));

pub struct QuotaHistoryCache {
    root: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    /// {log 文件路径: (mtime_secs, 解析出的快照)}
    cache: HashMap<PathBuf, (u64, Vec<QuotaSnapshot>)>,
}

impl QuotaHistoryCache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            inner: Mutex::new(Inner {
                cache: HashMap::new(),
            }),
        }
    }

    /// 扫描全部 .log，返回按账号分组的时间序列。
    pub fn scan(&self) -> (Vec<Account>, ScanStats) {
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
        let mut all_snaps: Vec<QuotaSnapshot> = Vec::new();
        let mut files_seen = 0usize;
        let mut reparsed = 0usize;
        let mut reused = 0usize;
        let mut active: HashSet<PathBuf> = HashSet::new();

        // 递归找所有 .log 文件
        for entry in WalkDir::new(&self.root).into_iter().filter_map(Result::ok) {
            let fp = entry.path();
            if !fp.is_file() {
                continue;
            }
            if fp.extension().and_then(|s| s.to_str()) != Some("log") {
                continue;
            }

            files_seen += 1;
            active.insert(fp.to_path_buf());

            let mtime = match util::file_mtime_secs(fp) {
                Some(m) => m,
                None => continue,
            };

            if let Some((cached_mtime, cached)) = inner.cache.get(fp) {
                if *cached_mtime == mtime {
                    all_snaps.extend(cached.iter().cloned());
                    reused += 1;
                    continue;
                }
            }

            let snaps = Self::parse_log_file(fp);
            all_snaps.extend(snaps.iter().cloned());
            inner.cache.insert(fp.to_path_buf(), (mtime, snaps));
            reparsed += 1;
        }

        inner.cache.retain(|k, _| active.contains(k));
        drop(inner); // 释放锁，后续处理不再需要

        let accounts = aggregate_accounts_from_snapshots(all_snaps);

        let stats = ScanStats {
            files: files_seen,
            reparsed,
            reused,
            took_ms: start.elapsed().as_millis() as u64,
        };
        (accounts, stats)
    }

    /// 解析一个 .log 文件，返回其中所有 quota 快照。
    fn parse_log_file(path: &Path) -> Vec<QuotaSnapshot> {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        // 目录启动时间兜底: 从 path 各段找 YYYYMMDDTHHMMSS
        let dir_ts: Option<NaiveDateTime> = path.ancestors().find_map(|anc| {
            let name = anc.file_name()?.to_str()?;
            let caps = DIR_TS_PAT.captures(name)?;
            let combined = format!("{}{}", caps.get(1)?.as_str(), caps.get(2)?.as_str());
            NaiveDateTime::parse_from_str(&combined, "%Y%m%d%H%M%S").ok()
        });

        // 文件 mtime 兜底
        let mtime_dt: Option<NaiveDateTime> = util::file_mtime_ms(path).and_then(|ms| {
            let secs = ms / 1000;
            let nsec = ((ms % 1000) as u32) * 1_000_000;
            chrono::DateTime::from_timestamp(secs, nsec).map(|dt| dt.naive_local())
        });

        let mut snaps: Vec<QuotaSnapshot> = Vec::new();

        for m in COMBO_PAT.captures_iter(&data) {
            let cur_m = match m.get(1) {
                Some(x) => x,
                None => continue,
            };
            let lim_m = match m.get(2) {
                Some(x) => x,
                None => continue,
            };
            let Ok(cur_str) = std::str::from_utf8(cur_m.as_bytes()) else { continue };
            let Ok(lim_str) = std::str::from_utf8(lim_m.as_bytes()) else { continue };
            let Ok(current) = cur_str.parse::<f64>() else { continue };
            let Ok(limit) = lim_str.parse::<i64>() else { continue };

            // 回溯 5000 字节找最近的 userId 和 timestamp
            let match_start = m.get(0).map(|x| x.start()).unwrap_or(0);
            let back_start = match_start.saturating_sub(5000);
            let back = &data[back_start..match_start];

            // 找 back 中最后一次 userId
            let uid: Option<String> = UID_PAT
                .captures_iter(back)
                .last()
                .and_then(|c| c.get(1).map(|m| m.as_bytes().to_vec()))
                .and_then(|bytes| String::from_utf8(bytes).ok());

            // 找 back 中最后一次日志行时间戳
            let ts_from_line: Option<NaiveDateTime> =
                TS_LINE_PAT.captures_iter(back).last().and_then(|c| {
                    let d = std::str::from_utf8(c.get(1)?.as_bytes()).ok()?;
                    let t = std::str::from_utf8(c.get(2)?.as_bytes()).ok()?;
                    let s = format!("{} {}", d, t);
                    NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
                        .ok()
                        .or_else(|| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S").ok())
                });

            let ts = ts_from_line.or(dir_ts).or(mtime_dt);
            let Some(naive) = ts else { continue };

            snaps.push(QuotaSnapshot {
                t: util::naive_local_to_ms(naive),
                uid,
                current,
                limit,
            });
        }

        snaps
    }
}

/// 把原始 quota snapshots 聚合成 Account 列表。
///
/// 步骤：
///   1) 按时间排序
///   2) 用"最近见过的 uid"填补空 uid（同一次 IDE 启动中，quota 响应通常带 uid，
///      随后同批次可能被截断但归属同一账号）
///   3) 按 uid 分组，每组按 (秒级时间戳, 值×100取整, limit) 去重
///   4) 统计断崖式下跌 (`resets`)、peak、latest 等
///   5) 按 peak 降序返回
///
/// 独立成 pub 函数是为了 history_store 从 SQLite 读出快照后能复用同一份聚合逻辑，
/// 保证 "扫当前 Kiro" 和 "读本地历史库" 给出的 Account[] 结构完全一致。
pub fn aggregate_accounts_from_snapshots(mut all_snaps: Vec<QuotaSnapshot>) -> Vec<Account> {
    // 1) 按时间排序
    all_snaps.sort_by_key(|s| s.t);

    // 2) 填补空 uid
    let mut last_uid: Option<String> = None;
    for s in all_snaps.iter_mut() {
        if let Some(uid) = &s.uid {
            last_uid = Some(uid.clone());
        } else if let Some(u) = &last_uid {
            s.uid = Some(u.clone());
        }
    }

    // 3) 按 uid 分组
    let mut by_uid: HashMap<String, Vec<QuotaSnapshot>> = HashMap::new();
    for s in all_snaps {
        let uid = s.uid.clone().unwrap_or_else(|| "(unknown)".to_string());
        by_uid.entry(uid).or_default().push(s);
    }

    // 4) 组装 Account
    let mut accounts: Vec<Account> = Vec::new();
    for (uid, mut snaps) in by_uid {
        snaps.sort_by_key(|s| s.t);

        // 同秒同值去重
        let mut dedup: Vec<QuotaSnapshot> = Vec::new();
        let mut last_key: Option<(i64, i64, i64)> = None;
        for s in snaps {
            let key = (s.t / 1000, (s.current * 100.0).round() as i64, s.limit);
            if last_key.as_ref() != Some(&key) {
                dedup.push(s);
                last_key = Some(key);
            }
        }
        if dedup.is_empty() {
            continue;
        }

        // 归零/重置次数：断崖式下跌（current < prev*0.7 且降幅 > 30）
        let mut resets = 0u32;
        let mut prev: Option<&QuotaSnapshot> = None;
        for s in &dedup {
            if let Some(p) = prev {
                if s.current < p.current - 30.0 && s.current < p.current * 0.7 {
                    resets += 1;
                }
            }
            prev = Some(s);
        }

        let first_seen = dedup.first().map(|s| s.t).unwrap_or(0);
        let last_seen = dedup.last().map(|s| s.t).unwrap_or(0);
        let peak = dedup.iter().map(|s| s.current).fold(0.0f64, f64::max);
        let latest = dedup.last().map(|s| s.current).unwrap_or(0.0);
        let latest_limit = dedup.last().map(|s| s.limit).unwrap_or(0);

        accounts.push(Account {
            uid,
            first_seen,
            last_seen,
            peak,
            latest,
            latest_limit,
            resets,
            snapshots: dedup,
        });
    }

    // 5) 按 peak 降序
    accounts.sort_by(|a, b| {
        b.peak
            .partial_cmp(&a.peak)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    accounts
}
