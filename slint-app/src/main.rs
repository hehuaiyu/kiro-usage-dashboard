// Kiro Usage Dashboard —— Slint 版
//
// 纯 Rust CPU 软件渲染 (renderer-software), 在无 GPU 机器上也能秒开。
// 数据层复用 kiro-core (跟 Tauri 版同一套扫描/持久化/聚合)。
//
// 当前进度 (增量): 左侧导航 + 简约视图 (KPI + 按日柱状图)。明细/趋势/账号视图逐步加。

slint::include_modules!();

use kiro_core::history_store::{default_history_db, HistoryStore};
use kiro_core::models::{Account, Turn};
use kiro_core::scanner::quota_history::aggregate_accounts_from_snapshots;
use kiro_core::scanner::{QuotaHistoryCache, TurnCache, V1SessionCache};
use kiro_core::util;
use std::rc::Rc;

fn diag(msg: &str) {
    eprintln!(
        "[slint {}] {}",
        chrono::Local::now().format("%H:%M:%S%.3f"),
        msg
    );
}

/// 完整数据 (跟 Tauri get_data 一致): 扫 3 源 → upsert 历史库 → 读全量。
struct LoadedData {
    turns: Vec<Turn>,
    v1_count: usize,
    accounts: Vec<Account>,
}

fn load_data() -> LoadedData {
    let v2 = TurnCache::new(util::default_sessions_root());
    let v1 = V1SessionCache::new(util::default_v1_sessions_root());
    let qc = QuotaHistoryCache::new(util::default_logs_root());
    let (turns, _) = v2.scan();
    let (v1s, _) = v1.scan();
    let (accts, _) = qc.scan();
    diag(&format!(
        "扫描完成: {} turns, {} v1, {} accounts",
        turns.len(),
        v1s.len(),
        accts.len()
    ));

    let v1_count = v1s.len();
    let (all_turns, accounts) = match HistoryStore::open(default_history_db()) {
        Ok(history) => {
            let _ = history.upsert_turns(&turns);
            let _ = history.upsert_v1_sessions(&v1s);
            let snaps: Vec<_> = accts
                .iter()
                .flat_map(|a| a.snapshots.iter().cloned())
                .collect();
            let _ = history.upsert_quota_snapshots(&snaps);
            let all_turns = history.load_all_turns().unwrap_or(turns);
            // 账号从历史库全量 snapshots 重新聚合 (跟 Tauri 版一致)
            let all_snaps = history.load_all_quota_snapshots().unwrap_or_default();
            let accounts = aggregate_accounts_from_snapshots(all_snaps);
            (all_turns, accounts)
        }
        Err(e) => {
            diag(&format!("历史库打开失败, 用当前扫描兜底: {}", e));
            (turns, accts)
        }
    };
    LoadedData {
        turns: all_turns,
        v1_count,
        accounts,
    }
}

fn fmt_credits(c: f64) -> String {
    if c >= 1000.0 {
        format!("{:.0}", c)
    } else if c >= 100.0 {
        format!("{:.1}", c)
    } else {
        format!("{:.2}", c)
    }
}

fn fmt_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0s".into();
    }
    let s = ms / 1000;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    if h > 0 {
        format!("{}h{:02}m", h, m)
    } else if m > 0 {
        format!("{}m", m)
    } else {
        format!("{}s", s % 60)
    }
}

fn fmt_local_dt(ts_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ts_ms).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".into(),
    }
}

/// 时间范围 (0今日 1本周 2本月 3近30天 4全部) → 起始 UTC 毫秒
fn range_start_ms(range: i32) -> i64 {
    use chrono::{Datelike, Duration, Local, Timelike};
    let now = Local::now();
    let start = match range {
        0 => now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|d| d.and_local_timezone(Local).unwrap()),
        1 => {
            // 本周一 0 点
            let days_from_mon = now.weekday().num_days_from_monday() as i64;
            (now - Duration::days(days_from_mon))
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|d| d.and_local_timezone(Local).unwrap())
        }
        2 => now
            .date_naive()
            .with_day(1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_local_timezone(Local).unwrap()),
        3 => Some(now - Duration::days(30)),
        _ => return 0, // 全部
    };
    start.map(|d| d.timestamp_millis()).unwrap_or(0)
}

fn range_label(range: i32) -> &'static str {
    match range {
        0 => "今日",
        1 => "本周",
        2 => "本月",
        3 => "近 30 天",
        _ => "全部",
    }
}

/// 按时间范围过滤数据并重算 KPI / 柱状图 / 热力图 / 明细, set 回 UI。
/// 账号视图是跨时间历史, 不在此函数 (在 main 一次性设置)。
fn refresh(app: &AppWindow, all: &[Turn], v1_count: usize, range: i32) {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    use std::collections::BTreeSet;

    let start = range_start_ms(range);
    let turns: Vec<Turn> = all.iter().filter(|t| t.t >= start).cloned().collect();

    // KPI
    let total_credits: f64 = turns.iter().map(|t| t.c).sum();
    let total_elapsed: i64 = turns.iter().map(|t| t.e).sum();
    let priced = turns.iter().filter(|t| t.c > 0.0).count();
    let v2_sessions: BTreeSet<&str> = turns.iter().map(|t| t.aid.as_str()).collect();
    let total_sessions = v1_count + v2_sessions.len();

    app.set_kpi_credits(fmt_credits(total_credits).into());
    app.set_kpi_turns(turns.len().to_string().into());
    app.set_kpi_turns_hint(format!("含计费 {}", priced).into());
    app.set_kpi_elapsed(fmt_duration(total_elapsed).into());
    app.set_kpi_sessions(total_sessions.to_string().into());
    app.set_kpi_sessions_hint(format!("v1 {} · v2 {}", v1_count, v2_sessions.len()).into());

    // 柱状图 (按日, 最近 30 天桶)
    let daily = aggregate_daily(&turns, 30);
    let max_v = daily
        .iter()
        .map(|(_, c)| *c)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let bars: Vec<BarItem> = daily
        .iter()
        .map(|(label, c)| BarItem {
            label: label.clone().into(),
            value: fmt_credits(*c).into(),
            ratio: (*c / max_v) as f32,
        })
        .collect();
    app.set_bars(Rc::new(slint::VecModel::from(bars)).into());
    app.set_chart_sub(format!("{} · {} 天", range_label(range), daily.len()).into());

    // 热力图 7×24
    let mut heat = [[0.0_f64; 24]; 7];
    for t in &turns {
        if let Some(d) = Local.timestamp_millis_opt(t.t).single() {
            heat[d.weekday().num_days_from_monday() as usize][d.hour() as usize] += t.c;
        }
    }
    let hmax = heat
        .iter()
        .flat_map(|r| r.iter())
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let heat_rows: Vec<slint::ModelRc<HeatCell>> = (0..7)
        .map(|d| {
            let cells: Vec<HeatCell> = (0..24)
                .map(|h| HeatCell {
                    ratio: (heat[d][h] / hmax) as f32,
                })
                .collect();
            slint::ModelRc::new(slint::VecModel::from(cells))
        })
        .collect();
    app.set_heat(slint::ModelRc::new(slint::VecModel::from(heat_rows)));

    // 明细表 (按范围, 最近在前)
    let detail: Vec<DetailRow> = turns
        .iter()
        .rev()
        .map(|t| DetailRow {
            time: fmt_local_dt(t.t).into(),
            credits: fmt_credits(t.c).into(),
            elapsed: fmt_duration(t.e).into(),
            status: t.s.clone().into(),
            workspace: t.ws.clone().into(),
            title: t.title.clone().into(),
        })
        .collect();
    let detail_count = detail.len();
    app.set_detail_rows(Rc::new(slint::VecModel::from(detail)).into());
    app.set_detail_sub(format!("{} · {} 条 turn", range_label(range), detail_count).into());
}

/// 按本地日期聚合 credits, 返回最近 N 天的 (MM-DD, credits)
fn aggregate_daily(turns: &[Turn], max_days: usize) -> Vec<(String, f64)> {
    use chrono::{Local, TimeZone};
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, f64> = BTreeMap::new();
    for t in turns {
        if let Some(d) = Local.timestamp_millis_opt(t.t).single() {
            *map.entry(d.format("%Y-%m-%d").to_string()).or_insert(0.0) += t.c;
        }
    }
    let mut v: Vec<(String, f64)> = map.into_iter().collect();
    if v.len() > max_days {
        v = v.split_off(v.len() - max_days);
    }
    // 标签只留 MM-DD
    v.into_iter()
        .map(|(k, c)| (k[5..].to_string(), c))
        .collect()
}

fn main() -> Result<(), slint::PlatformError> {
    let t0 = std::time::Instant::now();
    diag("slint main() 启动");

    let data = load_data();

    let app = AppWindow::new()?;
    diag(&format!(
        "AppWindow::new 完成, 从 main 累计 {}ms",
        t0.elapsed().as_millis()
    ));

    // 页脚 (一次性)
    app.set_footer(
        format!(
            "启动 {}ms · 全量 {} turns · 数据源 kiro-core",
            t0.elapsed().as_millis(),
            data.turns.len()
        )
        .into(),
    );

    // 账号历史 (跨时间的历史, 不随时间范围变, 一次性设置)
    let acct_rows: Vec<AccountRow> = data
        .accounts
        .iter()
        .map(|a| AccountRow {
            uid: a.uid.clone().into(),
            peak: fmt_credits(a.peak).into(),
            latest: fmt_credits(a.latest).into(),
            limit: if a.latest_limit > 0 {
                a.latest_limit.to_string()
            } else {
                "-".into()
            }
            .into(),
            resets: a.resets.to_string().into(),
        })
        .collect();
    let acct_count = acct_rows.len();
    app.set_account_rows(Rc::new(slint::VecModel::from(acct_rows)).into());
    app.set_account_sub(format!("{} 个账号 (每次归零 = 切账号/配额重置)", acct_count).into());

    // 交互: 时间范围切换 → refresh 重算 KPI/柱状/热力/明细
    let turns = Rc::new(data.turns);
    let v1_count = data.v1_count;
    {
        let turns = turns.clone();
        let weak = app.as_weak();
        app.on_range_changed(move |r| {
            if let Some(app) = weak.upgrade() {
                app.set_active_range(r);
                refresh(&app, &turns, v1_count, r);
            }
        });
    }
    // 初始渲染 (默认 30 天)
    refresh(&app, &turns, v1_count, 3);

    diag(&format!(
        "数据已绑定, 即将 run, 累计 {}ms",
        t0.elapsed().as_millis()
    ));
    app.run()
}
