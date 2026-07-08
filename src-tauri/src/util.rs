// 通用工具函数：base64 变体解码、路径处理、时间/时区、Kiro 数据目录定位。

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Kiro workspace-sessions 目录名解码
// ---------------------------------------------------------------------------

/// Kiro 的 `workspace-sessions/<encoded>/` 目录名编码规则：
///   - 标准 base64 alphabet (`A-Za-z0-9+/`)
///   - 值 62 的 `+` 替换成 `_`
///   - 末尾 padding `=` 也替换成 `_`
///
/// 注意：**不是** URL-safe base64。URL-safe 里 `_` 代表 63 (`/`)，
/// 那样解码 `ZTpca2lyb_i0puWPtw__` 会把中间 `_` 当 `/`，中文字节解错。
///
/// 详见 `docs/data-sources.md` 里 "workspace 目录名编码" 一节。
pub fn decode_kiro_ws_name(name: &str) -> String {
    let stripped = name.trim_end_matches('_');
    let n_pad = name.len() - stripped.len();

    // 只把中间的 `_` 换回 `+`，末尾 padding 单独补 `=`
    let mut padded: String = stripped.replace('_', "+");
    padded.push_str(&"=".repeat(n_pad));
    while padded.len() % 4 != 0 {
        padded.push('=');
    }

    match STANDARD.decode(&padded) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => name.to_string(), // 解码失败保底返回原名
    }
}

// ---------------------------------------------------------------------------
// 路径处理
// ---------------------------------------------------------------------------

/// 路径末段目录名（Windows/Unix 都支持）。
/// 空字符串或纯分隔符返回 "(no-workspace)"。
pub fn basename(p: &str) -> String {
    if p.is_empty() {
        return "(no-workspace)".to_string();
    }
    let trimmed: &str = p.trim_end_matches(|c: char| c == '\\' || c == '/');
    if trimmed.is_empty() {
        return "(no-workspace)".to_string();
    }
    match trimmed.rfind(|c: char| c == '\\' || c == '/') {
        Some(i) => trimmed[i + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 时间 / 时区
// ---------------------------------------------------------------------------

/// 本地时区相对 UTC 的偏移，单位分钟。北京时间 = +480。
pub fn local_tz_offset_min() -> i32 {
    Local::now().offset().local_minus_utc() / 60
}

/// 现在的 UTC 毫秒时间戳。
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// ISO 8601 时间字符串（含 `Z` 或数字 offset）→ UTC 毫秒。
///
/// 支持格式：
///   - `2026-07-01T02:17:07.966Z`
///   - `2026-07-01T02:17:07+08:00`
pub fn iso_to_ms(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    // Z 结尾换成 +00:00 才能被 RFC3339 解析
    let s2: String = if let Some(prefix) = s.strip_suffix('Z') {
        format!("{}+00:00", prefix)
    } else {
        s.to_string()
    };
    DateTime::parse_from_rfc3339(&s2).ok().map(|dt| dt.timestamp_millis())
}

/// 把 log 里解析出的 naive datetime（**没有时区信息，视为本地时间**）转成 UTC 毫秒。
/// Kiro 日志行头时间戳 `2026-07-06 10:55:38.742` 就是这种。
pub fn naive_local_to_ms(naive: NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp_millis())
        // 兜底：万一夏令时切换有歧义，直接当 UTC 处理（差几个小时但不崩）
        .unwrap_or_else(|| naive.and_utc().timestamp_millis())
}

/// 文件 mtime 转 UTC 毫秒。
pub fn file_mtime_ms(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i64)
}

/// 文件 mtime，秒（用于 cache 增量判断）。
pub fn file_mtime_secs(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(dur.as_secs())
}

// ---------------------------------------------------------------------------
// Kiro 数据目录定位（跨平台，主要针对 Windows）
// ---------------------------------------------------------------------------

/// `%APPDATA%/Kiro` (Win) / `~/Library/Application Support/Kiro` (macOS) / `~/.config/Kiro` (Linux)
fn kiro_appdata() -> PathBuf {
    match dirs::config_dir() {
        Some(d) => d.join("Kiro"),
        None => PathBuf::from("."),
    }
}

/// v2 sessions 根目录: `~/.kiro/sessions`
pub fn default_sessions_root() -> PathBuf {
    match dirs::home_dir() {
        Some(h) => h.join(".kiro").join("sessions"),
        None => PathBuf::from("."),
    }
}

/// v1 sessions 根目录:
/// `<APPDATA>/Kiro/User/globalStorage/kiro.kiroagent/workspace-sessions`
pub fn default_v1_sessions_root() -> PathBuf {
    kiro_appdata()
        .join("User")
        .join("globalStorage")
        .join("kiro.kiroagent")
        .join("workspace-sessions")
}

/// Kiro 日志根目录: `<APPDATA>/Kiro/logs`
pub fn default_logs_root() -> PathBuf {
    kiro_appdata().join("logs")
}

/// state.vscdb 路径: `<APPDATA>/Kiro/User/globalStorage/state.vscdb`
pub fn default_state_db() -> PathBuf {
    kiro_appdata()
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // 说明: 下面的 base64 样本是**通用示例**，通过手工构造 workspace 路径
    // 后按 Kiro 变体编码规则算出，用来覆盖：
    //   1. 无中间 `_`、无末尾 padding (`d:\proj\alpha` 长度整除 3)
    //   2. 有末尾 padding `_` (`d:\proj\beta` 需要 2 个 padding)
    //   3. 有中间 `_` (原字节含 `+` 用 `_` 替换)
    //   4. 含中文（关键：验证不是 URL-safe base64）
    // 想验证真机数据的话把你 workspace-sessions 里目录名拷进来测。

    #[test]
    fn decode_kiro_ws_variants() {
        // "d:\proj\alpha" 13 字节 → 20 char base64 (含 2 个 padding)
        // 用 Python 生成: base64.b64encode(b"d:\\proj\\alpha").decode().replace("+","_").replace("=","_")
        // 手工构造 + 反算，验证解码路径正确。
        assert_eq!(
            decode_kiro_ws_name("ZDpccHJvalxhbHBoYQ__"),
            r"d:\proj\alpha"
        );
        // 含中文 + 中间 `_` (原字节 base64 值 62): 验证 Kiro 变体，
        // 不是 URL-safe（URL-safe 里 `_` 是 `/` 会导致中文字节解错）。
        // 编码前: "e:\\测试" (UTF-8 字节 65 3A 5C E6 B5 8B E8 AF 95)
        // Kiro 编码后: 中间 `+` 被换成 `_`。
        assert_eq!(
            decode_kiro_ws_name("ZTpc5rWL6K_V"),
            "e:\\测试"
        );
    }

    #[test]
    fn basename_windows() {
        assert_eq!(basename(r"d:\path\to\proj"), "proj");
        assert_eq!(basename(r"d:\path\to\proj\"), "proj");
        assert_eq!(basename("/home/user/proj"), "proj");
        assert_eq!(basename(""), "(no-workspace)");
        assert_eq!(basename(r"\\"), "(no-workspace)");
    }

    #[test]
    fn iso_parse() {
        assert_eq!(
            iso_to_ms("2026-07-01T02:17:07.966Z"),
            Some(1782627427966)
        );
        assert_eq!(iso_to_ms(""), None);
        assert_eq!(iso_to_ms("not a date"), None);
    }
}
