// 读 Kiro 的 `state.vscdb`（SQLite）里 `kiro.kiroAgent` 键的当前账号配额快照。
//
// 用 URI 模式 + immutable=1 打开，避免与运行中的 Kiro 抢锁。
// 详见 docs/data-sources.md 第五节。

use crate::models::Quota;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

/// 从 state.vscdb 读当前账号配额；文件不存在或字段缺失时返回 None。
pub fn load(state_db_path: &Path) -> Option<Quota> {
    if !state_db_path.exists() {
        return None;
    }

    // SQLite URI: file:///<abs-path>?mode=ro&immutable=1
    // Windows 路径 `\` 换成 `/`，SQLite URI 语法要求 forward slash。
    let path_str = state_db_path.to_string_lossy().replace('\\', "/");
    let uri = format!(
        "file:///{}?mode=ro&immutable=1",
        path_str.trim_start_matches('/'),
    );

    let con = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(_) => return None, // 打不开就当没这数据
    };

    let value_str: String = con
        .query_row(
            "SELECT value FROM ItemTable WHERE key='kiro.kiroAgent'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()?;

    let j: Value = serde_json::from_str(&value_str).ok()?;

    let sub_title = j
        .get("subscriptionInfo")
        .and_then(|si| si.get("subscriptionTitle"))
        .and_then(|s| s.as_str())
        .map(String::from);

    // 优先: `kiro.resourceNotifications.usageState.usageBreakdowns[0]`
    //        这就是 IDE 右下角显示的数字（含 percentageUsed / resetDate）
    if let Some(breakdowns) = j
        .get("kiro.resourceNotifications.usageState")
        .and_then(|ns| ns.get("usageBreakdowns"))
        .and_then(|a| a.as_array())
    {
        if let Some(b) = breakdowns.first() {
            return Some(Quota {
                source: "resourceNotifications".to_string(),
                current: b.get("currentUsage").and_then(|v| v.as_f64()),
                limit: b.get("usageLimit").and_then(|v| v.as_i64()),
                percentage: b.get("percentageUsed").and_then(|v| v.as_f64()),
                overage_cap: b.get("overageCap").and_then(|v| v.as_i64()),
                overage_rate: b.get("overageRate").and_then(|v| v.as_f64()),
                reset_date: b
                    .get("resetDate")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                subscription: sub_title.clone(),
            });
        }
    }

    // 兜底: `usageBreakdownList[0]`（订阅创建时的初始快照，可能是 0）
    if let Some(list) = j
        .get("usageBreakdownList")
        .and_then(|a| a.as_array())
    {
        if let Some(b) = list.first() {
            return Some(Quota {
                source: "usageBreakdownList".to_string(),
                current: b.get("currentUsage").and_then(|v| v.as_f64()),
                limit: b.get("usageLimit").and_then(|v| v.as_i64()),
                percentage: None,
                overage_cap: b.get("overageCap").and_then(|v| v.as_i64()),
                overage_rate: b.get("overageRate").and_then(|v| v.as_f64()),
                reset_date: None,
                subscription: sub_title,
            });
        }
    }

    None
}
