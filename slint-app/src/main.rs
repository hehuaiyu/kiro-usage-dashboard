// Kiro Usage Dashboard —— Slint 试验版
//
// 目的: 验证 Slint 的纯 Rust CPU 软件渲染器 (renderer-software) 能否在
// 这台图形栈残缺的无 GPU 机器上跑起来 —— egui (glow/wgpu) 在此机全崩,
// 而 Slint 软件渲染不碰系统图形栈, 理论上能绕开。
//
// 第一版最小验证: 读 history.db 显示 4 个 KPI, 先用英文标签避免中文字体坑
// 干扰"能否跑"的判断。

slint::include_modules!();

fn diag(msg: &str) {
    use std::io::Write;
    eprintln!("[slint {}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    if let Some(dir) = dirs::data_dir().map(|d| d.join("kiro-usage-dashboard")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("slint-startup.log"))
        {
            let _ = writeln!(
                f,
                "[{}] {}",
                chrono::Local::now().format("%H:%M:%S%.3f"),
                msg
            );
        }
    }
}

/// 读 history.db 算 KPI: (credits 和, turn 数, 耗时 ms 和, 活跃天数)
fn load_kpi() -> (f64, usize, i64, usize) {
    let Some(path) = dirs::data_dir().map(|d| d.join("kiro-usage-dashboard").join("history.db"))
    else {
        return (0.0, 0, 0, 0);
    };
    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(e) => {
            diag(&format!("打开 history.db 失败: {}", e));
            return (0.0, 0, 0, 0);
        }
    };
    let mut credits = 0.0;
    let mut turns = 0usize;
    let mut elapsed = 0i64;
    let mut days = std::collections::BTreeSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT ts_ms, credits, elapsed_ms FROM turns") {
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, i64>(2)?))
        });
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (ts, c, e) = row;
                credits += c;
                turns += 1;
                elapsed += e;
                // 按天 (UTC 简化) 去重
                days.insert(ts / 86_400_000);
            }
        }
    }
    (credits, turns, elapsed, days.len())
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

fn main() -> Result<(), slint::PlatformError> {
    let t0 = std::time::Instant::now();
    diag("slint main() 启动");

    let (credits, turns, elapsed, days) = load_kpi();
    diag(&format!(
        "数据加载完成: credits={:.2} turns={} days={}",
        credits, turns, days
    ));

    diag("即将 AppWindow::new (Slint 软件渲染初始化)");
    let app = AppWindow::new()?;
    diag(&format!(
        "AppWindow::new 完成, 从 main 累计 {}ms",
        t0.elapsed().as_millis()
    ));

    app.set_credits(fmt_credits(credits).into());
    app.set_turns(turns.to_string().into());
    app.set_elapsed(fmt_duration(elapsed).into());
    app.set_days(days.to_string().into());
    app.set_footer(
        format!(
            "启动到窗口创建: {}ms  |  数据源: %APPDATA%\\kiro-usage-dashboard\\history.db",
            t0.elapsed().as_millis()
        )
        .into(),
    );

    // 首帧计时: 用 Timer 在第一次事件循环迭代后记录 (近似首帧)
    let t0_clone = t0;
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(0),
        move || {
            diag(&format!(
                "首个事件循环回调 (窗口应已显示), 从 main 累计 {}ms",
                t0_clone.elapsed().as_millis()
            ));
        },
    );

    diag("即将 app.run (进入事件循环)");
    app.run()
}
