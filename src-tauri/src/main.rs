// Kiro Usage Dashboard - Tauri v2 桌面应用入口。
//
// 前端 (../ui) 通过 window.__TAURI__.core.invoke() 调 IPC 命令：
//   - get_data      → 返回 DataResponse (含 turns, v1_sessions, accounts, quota, ...)
//   - export_csv    → 返回 CSV 字符串（前端用 Blob 触发下载）
//
// 数据结构与 Python 版 /api/data 完全一致，前端可 1:1 复用。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history_store;
mod models;
mod quota_snapshot;
mod scanner;
mod util;

use history_store::HistoryStore;
use models::{DataResponse, HistoryStats, QuotaSnapshot};
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
    history: Arc<HistoryStore>,
}

/// 从当前扫描的 Account 列表里抽出所有 QuotaSnapshot。
/// 用于把当前扫描的快照 upsert 到历史库。
fn flatten_snapshots(accounts: &[models::Account]) -> Vec<QuotaSnapshot> {
    accounts
        .iter()
        .flat_map(|a| a.snapshots.iter().cloned())
        .collect()
}



// ---------------------------------------------------------------------------
// IPC 命令
// ---------------------------------------------------------------------------

/// 前端主数据接口。等价于 Python 版的 `GET /api/data`。
///
/// v0.3 变更：
///   1) 先扫当前 Kiro 数据
///   2) upsert 到本地历史库（INSERT OR IGNORE，不覆盖）
///   3) **从历史库读全量**作为返回值（Kiro 那边如果清理了，历史仍在）
///   4) accounts 从历史库的原始 snapshots 重新聚合
///
/// v0.4.1 关键修复：标记 `(async)` 让命令在独立线程执行，而不是主线程。
/// 同步命令默认在主线程跑，里面的 scan() 扫十几秒会冻结整个 UI（白屏 + 鼠标卡顿）。
/// 参考 Tauri Discussion #3561 / #4191。
#[tauri::command(async)]
fn get_data(state: State<'_, AppState>) -> Result<DataResponse, String> {
    // 1) 扫当前 Kiro 数据
    let (curr_turns, scan) = state.v2.scan();
    let (curr_v1, scan_v1) = state.v1.scan();
    let (curr_accounts, scan_accounts) = state.quota.scan();
    let quota = quota_snapshot::load(&state.state_db_path);

    // 2) upsert 到历史库
    let ins_t = state.history.upsert_turns(&curr_turns).unwrap_or(0);
    let ins_v = state.history.upsert_v1_sessions(&curr_v1).unwrap_or(0);
    let curr_snaps = flatten_snapshots(&curr_accounts);
    let ins_q = state.history.upsert_quota_snapshots(&curr_snaps).unwrap_or(0);

    // 3) 从历史库读全量（Kiro 数据即便被清，这里仍能拿到累积历史）
    let all_turns = state.history.load_all_turns().unwrap_or_default();
    let all_v1 = state.history.load_all_v1_sessions().unwrap_or_default();
    let all_snaps = state.history.load_all_quota_snapshots().unwrap_or_default();
    let accounts = scanner::quota_history::aggregate_accounts_from_snapshots(all_snaps);

    // 4) history stats
    let mut history_stats = state.history.stats();
    history_stats.last_upserted = ins_t + ins_v + ins_q;

    Ok(DataResponse {
        turns: all_turns,
        v1_sessions: all_v1,
        accounts,
        quota,
        server_ts: util::now_ms(),
        server_tz_offset_min: util::local_tz_offset_min(),
        scan,
        scan_v1,
        scan_accounts,
        history_stats,
    })
}

/// 清空本地历史库。返回清除前的统计（供前端 toast "已清除 X 条"）。
/// (async) 同理，SQLite DELETE + VACUUM 也别占主线程。
#[tauri::command(async)]
fn clear_history(state: State<'_, AppState>) -> Result<HistoryStats, String> {
    state.history.clear_all()
}

/// 前端"导出 CSV"按钮走这个命令，直接拿字符串然后前端 Blob 触发下载。
/// v0.3：从历史库读，包含累积的完整历史（不只是当前 Kiro 目录的数据）。
/// (async) 让读库 + 拼字符串在独立线程，不阻塞 UI。
#[tauri::command(async)]
fn export_csv(state: State<'_, AppState>) -> Result<String, String> {
    let turns = state.history.load_all_turns().unwrap_or_default();
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
    // 【关键】无 GPU / 显卡驱动异常的机器上, WebView2 (Chromium 内核) 会先尝试初始化 GPU 进程,
    // 等待超时后才 fallback 到软件渲染 —— 这个超时导致首次渲染白屏 8-9 秒。
    // 显式禁用 GPU, 让它一开始就走软件渲染, 跳过超时等待。
    // 参考微软官方: SetEnvironmentVariable("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "--disable-gpu")
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--disable-gpu --disable-gpu-compositing",
    );

    // 数据源路径全部自动探测（跨平台，Windows/macOS/Linux 用 dirs crate）
    let sessions_root = util::default_sessions_root();
    let v1_root = util::default_v1_sessions_root();
    let logs_root = util::default_logs_root();
    let state_db_path = util::default_state_db();
    let history_db_path = history_store::default_history_db();

    // 3 个 Kiro 数据扫描器
    let v2 = Arc::new(TurnCache::new(sessions_root.clone()));
    let v1 = Arc::new(V1SessionCache::new(v1_root.clone()));
    let quota = Arc::new(QuotaHistoryCache::new(logs_root.clone()));

    // 本地持久化历史库（打开失败直接 panic —— 没有持久化, 工具核心价值就没了）
    let history = Arc::new(
        HistoryStore::open(history_db_path.clone())
            .unwrap_or_else(|e| panic!("[kiro-usage-dashboard] 历史库打开失败: {}", e)),
    );

    // 打印诊断信息（release 模式看不到，dev 模式能看）
    eprintln!("[kiro-usage-dashboard] 数据源目录：");
    eprintln!("  v2 sessions:   {}", sessions_root.display());
    eprintln!("  v1 sessions:   {}", v1_root.display());
    eprintln!("  logs (quota):  {}", logs_root.display());
    eprintln!("  state.vscdb:   {}", state_db_path.display());
    eprintln!("  history db:    {}", history_db_path.display());

    // v0.4.1: 预扫不再阻塞 main() —— 之前十几秒白屏就是这一坨在挡窗口。
    // 现在把 Arc clone 一份, 到 tauri::setup 里的后台线程去扫,
    // 窗口秒开; 首次 get_data 若刚好赶上扫完则直接读历史库, 否则退化到当前扫的空数据 (前端 15s 自动刷新会兜住)
    let v2_pre = v2.clone();
    let v1_pre = v1.clone();
    let quota_pre = quota.clone();
    let history_pre = history.clone();

    let app_state = AppState {
        v2,
        v1,
        quota,
        state_db_path,
        history,
    };

    tauri::Builder::default()
        .setup(move |app| {
            app.manage(app_state);

            // 后台预扫线程：不阻塞窗口显示，扫完后前端下次 15s 自动刷新就能看到
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let (turns0, s0) = v2_pre.scan();
                let (v1_0, s1) = v1_pre.scan();
                let (accts0, sa) = quota_pre.scan();
                let snaps0 = flatten_snapshots(&accts0);
                let ins_t = history_pre.upsert_turns(&turns0).unwrap_or(0);
                let ins_v = history_pre.upsert_v1_sessions(&v1_0).unwrap_or(0);
                let ins_q = history_pre.upsert_quota_snapshots(&snaps0).unwrap_or(0);
                let hstats = history_pre.stats();
                eprintln!(
                    "[background-warmup] 后台预扫完成 (总耗 {}ms): v2 turn={} ({}ms), v1 session={} ({}ms), account={} ({}ms)",
                    t0.elapsed().as_millis(),
                    turns0.len(), s0.took_ms,
                    v1_0.len(), s1.took_ms,
                    accts0.len(), sa.took_ms,
                );
                eprintln!(
                    "[history] upsert 新增: turn={} v1={} quota={} | 历史库累计: turn={} v1={} quota={} (db {:.1} KB)",
                    ins_t, ins_v, ins_q,
                    hstats.turns_count, hstats.v1_sessions_count, hstats.quota_snapshots_count,
                    hstats.db_size_bytes as f64 / 1024.0,
                );
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_data, export_csv, clear_history])
        .run(tauri::generate_context!())
        .expect("Tauri 启动失败");
}
