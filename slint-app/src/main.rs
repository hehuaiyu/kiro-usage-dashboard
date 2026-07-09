// Kiro Usage Dashboard —— Slint 版
//
// 纯 Rust CPU 软件渲染 (renderer-software), 在无 GPU 机器上也能秒开。
// 数据层复用 kiro-core (跟 Tauri 版同一套扫描/持久化/聚合)。
//
// 当前进度 (增量): 左侧导航 + 简约视图 (KPI + 按日柱状图)。明细/趋势/账号视图逐步加。

slint::include_modules!();

use kiro_core::history_store::{default_history_db, HistoryStore};
use kiro_core::models::Turn;
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
    let all_turns = match HistoryStore::open(default_history_db()) {
        Ok(history) => {
            let _ = history.upsert_turns(&turns);
            let _ = history.upsert_v1_sessions(&v1s);
            let snaps: Vec<_> = accts
                .iter()
                .flat_map(|a| a.snapshots.iter().cloned())
                .collect();
            let _ = history.upsert_quota_snapshots(&snaps);
            history.load_all_turns().unwrap_or(turns)
        }
        Err(e) => {
            diag(&format!("历史库打开失败, 用当前扫描兜底: {}", e));
            turns
        }
    };
    LoadedData {
        turns: all_turns,
        v1_count,
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

    // KPI
    let total_credits: f64 = data.turns.iter().map(|t| t.c).sum();
    let total_elapsed: i64 = data.turns.iter().map(|t| t.e).sum();
    let priced = data.turns.iter().filter(|t| t.c > 0.0).count();
    // v2 session 按 agent_session_id 去重
    let v2_sessions: std::collections::BTreeSet<&str> =
        data.turns.iter().map(|t| t.aid.as_str()).collect();
    let total_sessions = data.v1_count + v2_sessions.len();

    // 柱状图: 最近 30 天
    let daily = aggregate_daily(&data.turns, 30);
    let max_v = daily.iter().map(|(_, c)| *c).fold(0.0_f64, f64::max).max(0.0001);
    let bars: Vec<BarItem> = daily
        .iter()
        .map(|(label, c)| BarItem {
            label: label.clone().into(),
            value: fmt_credits(*c).into(),
            ratio: (*c / max_v) as f32,
        })
        .collect();

    let app = AppWindow::new()?;
    diag(&format!(
        "AppWindow::new 完成, 从 main 累计 {}ms",
        t0.elapsed().as_millis()
    ));

    app.set_kpi_credits(fmt_credits(total_credits).into());
    app.set_kpi_turns(data.turns.len().to_string().into());
    app.set_kpi_turns_hint(format!("含计费 {}", priced).into());
    app.set_kpi_elapsed(fmt_duration(total_elapsed).into());
    app.set_kpi_sessions(total_sessions.to_string().into());
    app.set_kpi_sessions_hint(format!("v1 {} · v2 {}", data.v1_count, v2_sessions.len()).into());
    app.set_chart_sub(format!("近 {} 天", daily.len()).into());
    app.set_footer(
        format!(
            "启动 {}ms · {} turns · 数据源 kiro-core",
            t0.elapsed().as_millis(),
            data.turns.len()
        )
        .into(),
    );

    let bars_model = Rc::new(slint::VecModel::from(bars));
    app.set_bars(bars_model.into());

    diag(&format!(
        "数据已绑定, 即将 run, 累计 {}ms",
        t0.elapsed().as_millis()
    ));
    app.run()
}
