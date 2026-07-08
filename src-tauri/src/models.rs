// 与前端 JSON 完全对齐的数据结构。
// 字段名和 Python 版 `kiro_dashboard.py` 里的 /api/data 响应保持一致，
// 前端 (../ui/app.js) 不需要为字段命名做任何变更。

use serde::Serialize;

/// 一次 turn（v2 时代的 `usage_summary` 事件）。
/// 字段名故意用短名（`t`/`c`/`e`/...）以减少 JSON 体积，跟 Python 版一致。
#[derive(Serialize, Clone, Debug)]
pub struct Turn {
    /// timestamp, UTC 毫秒
    pub t: i64,
    /// est credits（`promptTurnSummaries[0].usage`）
    pub c: f64,
    /// elapsed, 毫秒
    pub e: i64,
    /// status: `success` / `aborted` / `failed` / `unknown`
    pub s: String,
    /// workspace basename（末段目录名）
    pub ws: String,
    /// 外层 session id（`.kiro/sessions/<sid>/...`）
    pub sid: String,
    /// agent session id（`.kiro/sessions/<sid>/<aid>/...`）
    pub aid: String,
    /// execution id (uuid)
    pub eid: String,
    pub title: String,
    pub model: String,
    pub tools: Vec<String>,
}

/// v1 时代的旧格式会话。没有 credits（v1 时期 Kiro 还没引入用量追踪），
/// 只有会话元信息和从 history 数出来的 turn 数近似。
#[derive(Serialize, Clone, Debug)]
pub struct V1Session {
    /// 恒等 "v1"，用于前端区分数据来源
    pub source: &'static str,
    pub session_id: String,
    pub title: String,
    /// workspace 末段目录名
    pub workspace: String,
    /// workspace 完整路径（用于 filter dropdown 的 value）
    pub workspace_full: String,
    /// UTC 毫秒时间戳，优先来自 sessions.json 索引的 dateCreated
    pub t: i64,
    /// history 里 role=user 消息数量或 executionId 去重数量，取较大者
    pub turn_count: usize,
    pub model: String,
}

impl V1Session {
    pub fn new(
        session_id: String,
        title: String,
        workspace: String,
        workspace_full: String,
        t: i64,
        turn_count: usize,
        model: String,
    ) -> Self {
        Self {
            source: "v1",
            session_id,
            title,
            workspace,
            workspace_full,
            t,
            turn_count,
            model,
        }
    }
}

/// 一次 quota 快照（从 Kiro 日志里挖出的 currentUsage 时间点）。
#[derive(Serialize, Clone, Debug)]
pub struct QuotaSnapshot {
    /// UTC 毫秒
    pub t: i64,
    /// 从上下文推断的 userId（可能为 None 表示 log 段没写 userId）
    pub uid: Option<String>,
    /// currentUsageWithPrecision
    pub current: f64,
    /// usageLimit
    pub limit: i64,
}

/// 按账号分组的 quota 时间序列 + 统计。
#[derive(Serialize, Clone, Debug)]
pub struct Account {
    /// userId；找不到 userId 的快照全部归到 "(unknown)"
    pub uid: String,
    pub first_seen: i64,
    pub last_seen: i64,
    /// 历史 currentUsage 峰值
    pub peak: f64,
    /// 最新一次 currentUsage
    pub latest: f64,
    /// 最新一次 usageLimit
    pub latest_limit: i64,
    /// currentUsage 断崖式下跌次数（≈ 账号切换/月度重置次数）
    pub resets: u32,
    pub snapshots: Vec<QuotaSnapshot>,
}

/// state.vscdb 里读的当前账号配额（对应 IDE 右下角显示的数字）。
#[derive(Serialize, Clone, Debug, Default)]
pub struct Quota {
    /// 数据源: "resourceNotifications" 或 "usageBreakdownList"
    pub source: String,
    pub current: Option<f64>,
    pub limit: Option<i64>,
    pub percentage: Option<f64>,
    pub overage_cap: Option<i64>,
    pub overage_rate: Option<f64>,
    pub reset_date: Option<String>,
    pub subscription: Option<String>,
}

/// 扫描器返回的统计信息（用于 footer 显示 "v2: XX 文件/XX turn"）。
#[derive(Serialize, Clone, Debug, Default)]
pub struct ScanStats {
    pub files: usize,
    pub reparsed: usize,
    pub reused: usize,
    pub took_ms: u64,
}

/// `get_data` IPC 命令的完整响应。JSON schema 与 Python 版 `/api/data` 一致。
#[derive(Serialize, Debug)]
pub struct DataResponse {
    pub turns: Vec<Turn>,
    pub v1_sessions: Vec<V1Session>,
    pub accounts: Vec<Account>,
    pub quota: Option<Quota>,
    pub server_ts: i64,
    pub server_tz_offset_min: i32,
    pub scan: ScanStats,
    pub scan_v1: ScanStats,
    pub scan_accounts: ScanStats,
}
