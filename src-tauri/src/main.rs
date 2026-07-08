// Kiro Usage Dashboard - Tauri v2 桌面应用入口。
//
// 前端 (../ui) 通过 window.__TAURI__.core.invoke() 调 IPC 命令：
//   - get_data      → 返回 DataResponse (含 turns, v1_sessions, accounts, quota, ...)
//   - export_csv    → 返回 CSV 字符串（前端用 Blob 触发下载）
//
// 数据结构与 Python 版 /api/data 完全一致，前端可 1:1 复用。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod models;
mod quota_snapshot;
mod scanner;
mod util;

use models::DataResponse;
use scanner::{QuotaHistoryCache, TurnCache, V1SessionCache};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Manager, State};

/// Tauri 管理的全局状态，通过 `State<AppState>` 注入到命令。
struct AppState {
    v2: Arc<TurnCache>,
    v1: Arc<V1SessionCache>,
    quota: Arc<QuotaHistoryCache>,
    state_db_path: PathBuf,
}

// ---------------------------------------------------------------------------
// IPC 命令
// ---------------------------------------------------------------------------

/// 前端主数据接口。等价于 Python 版的 `GET /api/data`。
#[tauri::command]
fn get_data(state: State<'_, AppState>) -> Result<DataResponse, String> {
    let (turns, scan) = state.v2.scan();
    let (v1_sessions, scan_v1) = state.v1.scan();
    let (accounts, scan_accounts) = state.quota.scan();
    let quota = quota_snapshot::load(&state.state_db_path);

    Ok(DataResponse {
        turns,
        v1_sessions,
        accounts,
        quota,
        server_ts: util::now_ms(),
        server_tz_offset_min: util::local_tz_offset_min(),
        scan,
        scan_v1,
        scan_accounts,
    })
}

/// 前端"导出 CSV"按钮走这个命令，直接拿字符串然后前端 Blob 触发下载。
#[tauri::command]
fn export_csv(state: State<'_, AppState>) -> Result<String, String> {
    let (turns, _) = state.v2.scan();
    let tz_offset_min = util::local_tz_offset_min() as i64;

    let mut csv = String::new();
    csv.push_str(
        "ts_local,ts_utc_ms,workspace,session_id,agent_session_id,execution_id,\
         credits,elapsed_ms,elapsed_human,status,tool_count,model,title,tools\n",
    );

    for t in turns {
        // 把 UTC ms 加上本地时区偏移，格式化成"墙钟时间"字符串
        let local_dt = chrono::DateTime::from_timestamp_millis(t.t + tz_offset_min * 60_000)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        let elapsed_human = fmt_duration_ms(t.e);
        let tools_joined = t.tools.join("|");

        csv.push_str(&csv_esc(&local_dt));
        csv.push(',');
        csv.push_str(&t.t.to_string());
        csv.push(',');
        csv.push_str(&csv_esc(&t.ws));
        csv.push(',');
        csv.push_str(&csv_esc(&t.sid));
        csv.push(',');
        csv.push_str(&csv_esc(&t.aid));
        csv.push(',');
        csv.push_str(&csv_esc(&t.eid));
        csv.push(',');
        csv.push_str(&format!("{:.6}", t.c));
        csv.push(',');
        csv.push_str(&t.e.to_string());
        csv.push(',');
        csv.push_str(&csv_esc(&elapsed_human));
        csv.push(',');
        csv.push_str(&csv_esc(&t.s));
        csv.push(',');
        csv.push_str(&t.tools.len().to_string());
        csv.push(',');
        csv.push_str(&csv_esc(&t.model));
        csv.push(',');
        csv.push_str(&csv_esc(&t.title));
        csv.push(',');
        csv.push_str(&csv_esc(&tools_joined));
        csv.push('\n');
    }
    Ok(csv)
}

// ---------------------------------------------------------------------------
// CSV / 时长辅助
// ---------------------------------------------------------------------------

fn csv_esc(s: &str) -> String {
    if s.chars().any(|c| c == ',' || c == '"' || c == '\n' || c == '\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

fn fmt_duration_ms(ms: i64) -> String {
    if ms <= 0 {
        return "0s".to_string();
    }
    let secs = (ms / 1000) as u64;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h{:02}m", h, m)
    } else if m > 0 {
        format!("{}m{:02}s", m, s)
    } else {
        format!("{}s", s)
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() {
    // 数据源路径全部自动探测（跨平台，Windows/macOS/Linux 用 dirs crate）
    let sessions_root = util::default_sessions_root();
    let v1_root = util::default_v1_sessions_root();
    let logs_root = util::default_logs_root();
    let state_db_path = util::default_state_db();

    // 启动时先做一次预热扫描，让首次 get_data 就是热的
    let v2 = Arc::new(TurnCache::new(sessions_root.clone()));
    let v1 = Arc::new(V1SessionCache::new(v1_root.clone()));
    let quota = Arc::new(QuotaHistoryCache::new(logs_root.clone()));

    // 打印诊断信息（release 模式看不到，dev 模式能看）
    eprintln!("[kiro-usage-dashboard] 数据源目录：");
    eprintln!("  v2 sessions:   {}", sessions_root.display());
    eprintln!("  v1 sessions:   {}", v1_root.display());
    eprintln!("  logs (quota):  {}", logs_root.display());
    eprintln!("  state.vscdb:   {}", state_db_path.display());

    // 预扫（可选，能让首次 IPC 响应快）
    let (turns0, s0) = v2.scan();
    let (v1_0, s1) = v1.scan();
    let (accts0, sa) = quota.scan();
    eprintln!(
        "[kiro-usage-dashboard] 预热完成: v2 turn={} ({}ms), v1 session={} ({}ms), account={} ({}ms)",
        turns0.len(),
        s0.took_ms,
        v1_0.len(),
        s1.took_ms,
        accts0.len(),
        sa.took_ms,
    );

    let app_state = AppState {
        v2,
        v1,
        quota,
        state_db_path,
    };

    tauri::Builder::default()
        .setup(move |app| {
            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_data, export_csv])
        .run(tauri::generate_context!())
        .expect("Tauri 启动失败");
}
