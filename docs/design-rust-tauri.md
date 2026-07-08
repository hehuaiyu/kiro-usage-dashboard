# Rust 版 (Tauri) 迁移方案

本文档描述如何把当前的 Python 原型 (`prototype-python/`) 迁移到 Rust + Tauri，产出单文件 exe。假设读者了解 Rust 基础和 Tauri 概念。

数据源的字段和解析细节参考 [data-sources.md](./data-sources.md)。

## 一、目标

- **单文件可分发**：产物是 ~12 MB 的 Windows exe，双击运行，无需装 Python / Node
- **启动更快**：冷启 < 500 ms（Python 版首次扫描约 1~2 s）
- **UI 零重写**：现有 `static/index.html + style.css + app.js` 原样复用
- **保持行为一致**：所有 KPI、图表、明细表、账号面板与 Python 版对齐

## 二、技术栈选择

选择 **Tauri v2**（Rust 后端 + WebView 前端）。

对比过的备选：

| 方案 | exe 大小 | 启动 | UI 重写代价 | 结论 |
|---|---|---|---|---|
| **Tauri 2**（选定） | ~12 MB | <500 ms | **零**（前端复用） | ✓ |
| Wails (Go) | ~14 MB | <500 ms | 零 | Go 生态桌面弱一些 |
| PySide6 + Nuitka | 40-80 MB | 1-2 s | 全部重写 | 产物过大 |
| Flet | 30-60 MB | 中 | 全部重写 | Flutter 不适合本项目 UI |
| egui / iced | 8-15 MB | <300 ms | 全部重写 | 图表能力弱 |
| Electron | 100+ MB | 2-3 s | 零 | 产物过大 |

**为什么 Tauri**：前端本来就是极简 HTML + JS + ECharts，Tauri 复用零成本；Rust 后端把 Python 逐个类翻译过去，代码量约 700 行；Windows 10+ 用 WebView2（系统自带），无额外依赖。

## 三、架构分层

```
┌──────────────────────────────────────────────────┐
│  WebView2 (系统内置)                             │
│  ├─ index.html                                    │
│  ├─ style.css                                     │
│  └─ app.js         ← 只改数据获取入口             │
│      │                                            │
│      ▼ Tauri IPC (invoke)                         │
├──────────────────────────────────────────────────┤
│  Rust 后端 (src-tauri/src/)                       │
│  ├─ scanner/                                      │
│  │   ├─ v2_turns.rs        ← 对应 TurnCache       │
│  │   ├─ v1_sessions.rs     ← 对应 V1SessionCache  │
│  │   └─ quota_history.rs   ← 对应 QuotaHistoryCache│
│  ├─ quota_snapshot.rs      ← 对应 load_quota       │
│  ├─ models.rs              ← Serde 数据结构        │
│  ├─ util.rs                ← base64 变体、路径查找 │
│  └─ main.rs                ← Tauri 入口 + IPC 命令 │
└──────────────────────────────────────────────────┘
```

**关键契约**：Rust 后端返回给前端的 JSON schema 与 Python 版**完全一致**（字段名、类型、嵌套结构都不变），保证前端不用改渲染逻辑。

## 四、目录结构

```
kiro-usage-dashboard/
├── prototype-python/            # Python 原型，独立存在
├── src-tauri/                   # Rust 版根
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── icons/                   # 应用图标（tauri icon 命令生成）
│   └── src/
│       ├── main.rs
│       ├── models.rs
│       ├── util.rs
│       ├── quota_snapshot.rs
│       └── scanner/
│           ├── mod.rs
│           ├── v2_turns.rs
│           ├── v1_sessions.rs
│           └── quota_history.rs
├── static/                      # 前端资源（打包时 Tauri 读这里）
│   ├── index.html
│   ├── style.css
│   ├── app.js
│   └── echarts.min.js           # 从 CDN 下载后内嵌，保证离线可用
├── package.json                 # Tauri 前端约定要求（不实际做 npm build）
└── docs/                        # 文档（本目录）
```

## 五、依赖选择 (`Cargo.toml`)

```toml
[package]
name = "kiro-usage-dashboard"
version = "0.1.0"
edition = "2021"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# 时间处理（带 tz 转换）
chrono = { version = "0.4", features = ["serde"] }

# 正则（在 log 里找 quota 快照、时间戳、userId）
regex = "1"
once_cell = "1"          # 用于全局编译的正则

# SQLite (bundled 特性把 sqlite 静态编进 exe，用户不用装)
rusqlite = { version = "0.31", features = ["bundled"] }

# 目录遍历
walkdir = "2"

# base64 (Kiro workspace 目录名解码)
base64 = "0.22"

# 跨平台定位 %APPDATA% / ~
dirs = "5"

# 并发原语（scanner 内部缓存用）
parking_lot = "0.12"

# 可选：文件变化推送
notify = { version = "6", optional = true }

[profile.release]
opt-level = "z"          # 优化体积
lto = true
codegen-units = 1
strip = true
```

## 六、Rust ↔ Python 翻译对照

| Python 原型 | Rust 版本 |
|---|---|
| `class TurnCache: self._cache: dict[str, tuple[float, list[dict]]]` | `struct TurnCache { cache: RwLock<HashMap<PathBuf, (u64, Vec<Turn>)>> }` |
| `class V1SessionCache` | `struct V1SessionCache { cache: RwLock<HashMap<PathBuf, (u64, Option<V1Session>)>> }` |
| `class QuotaHistoryCache` | `struct QuotaHistoryCache { cache: RwLock<HashMap<PathBuf, (u64, Vec<Snapshot>)>> }` |
| `os.path.getmtime(fp)` | `fs::metadata(&path)?.modified()?.duration_since(UNIX_EPOCH)?.as_secs()` |
| `glob.iglob("**/messages.jsonl")` | `WalkDir::new(root).into_iter().filter_map(...)` |
| `re.compile(r"...")` | `static PAT: Lazy<Regex> = Lazy::new(\|\| Regex::new(r"...").unwrap());` |
| `json.loads(line)` | `serde_json::from_str::<Value>(line)?` |
| `sqlite3.connect(f"file:{p}?mode=ro&immutable=1", uri=True)` | `Connection::open_with_flags(...)` + `SQLITE_OPEN_READ_ONLY \| SQLITE_OPEN_URI` |
| `base64.b64decode(padded)` | `base64::engine::general_purpose::STANDARD.decode(&padded)?` |
| `datetime.fromisoformat(...)` | `chrono::DateTime::parse_from_rfc3339(...)?` |
| `threading.Lock()` | `parking_lot::Mutex` / `RwLock` |
| `os.environ["APPDATA"]` | `std::env::var("APPDATA")` 或 `dirs::config_dir()` |
| `http.server.ThreadingHTTPServer` | 用不到（Tauri 用 IPC 代替 HTTP） |

## 七、数据模型（`models.rs`）

与前端 JSON 字段完全对齐：

```rust
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct Turn {
    pub t: i64,           // UTC ms
    pub c: f64,           // credits
    pub e: i64,           // elapsed ms
    pub s: String,        // status
    pub ws: String,       // workspace basename
    pub sid: String,      // outer session id
    pub aid: String,      // agent session id
    pub eid: String,      // execution id
    pub title: String,
    pub model: String,
    pub tools: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct V1Session {
    pub source: &'static str, // 常量 "v1"
    pub session_id: String,
    pub title: String,
    pub workspace: String,        // basename
    pub workspace_full: String,   // 完整路径
    pub t: i64,                   // UTC ms
    pub turn_count: usize,
    pub model: String,
}

#[derive(Serialize, Clone)]
pub struct QuotaSnapshot {
    pub t: i64,          // UTC ms
    pub uid: Option<String>,
    pub current: f64,
    pub limit: i64,
}

#[derive(Serialize, Clone)]
pub struct Account {
    pub uid: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub peak: f64,
    pub latest: f64,
    pub latest_limit: i64,
    pub resets: u32,
    pub snapshots: Vec<QuotaSnapshot>,
}

#[derive(Serialize, Clone)]
pub struct Quota {
    pub source: &'static str,
    pub current: Option<f64>,
    pub limit: Option<i64>,
    pub percentage: Option<f64>,
    pub overage_cap: Option<i64>,
    pub overage_rate: Option<f64>,
    pub reset_date: Option<String>,
    pub subscription: Option<String>,
}

#[derive(Serialize)]
pub struct ScanStats {
    pub files: usize,
    pub reparsed: usize,
    pub reused: usize,
    pub took_ms: u64,
}

#[derive(Serialize)]
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
```

## 八、IPC 命令签名（`main.rs`）

```rust
use tauri::{Manager, State};
use std::sync::Arc;
use parking_lot::RwLock;

struct AppState {
    v2: Arc<scanner::v2_turns::TurnCache>,
    v1: Arc<scanner::v1_sessions::V1SessionCache>,
    quota: Arc<scanner::quota_history::QuotaHistoryCache>,
    state_db_path: std::path::PathBuf,
}

#[tauri::command]
async fn get_data(state: State<'_, AppState>) -> Result<models::DataResponse, String> {
    let (turns, s_v2) = state.v2.scan().map_err(|e| e.to_string())?;
    let (v1_sessions, s_v1) = state.v1.scan().map_err(|e| e.to_string())?;
    let (accounts, s_acc) = state.quota.scan().map_err(|e| e.to_string())?;
    let quota = quota_snapshot::load(&state.state_db_path).ok();

    Ok(models::DataResponse {
        turns,
        v1_sessions,
        accounts,
        quota,
        server_ts: chrono::Utc::now().timestamp_millis(),
        server_tz_offset_min: util::local_tz_offset_min(),
        scan: s_v2,
        scan_v1: s_v1,
        scan_accounts: s_acc,
    })
}

#[tauri::command]
async fn export_csv(state: State<'_, AppState>) -> Result<String, String> {
    // 返回 CSV 字符串，前端用 Blob 触发下载
    ...
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState { /* 探测各根目录，初始化 3 个 cache */ };
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_data, export_csv])
        .run(tauri::generate_context!())
        .expect("Tauri 启动失败");
}
```

## 九、前端最小改动清单

需要改的**只有 3 处**：

1. **`app.js` 里 fetchData 内**：把 HTTP fetch 换成 Tauri invoke

```javascript
// 改前 (Python 版)
async function fetchData(silent = false) {
    const resp = await fetch('/api/data', { cache: 'no-store' });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const j = await resp.json();
    // ...
}

// 改后 (Tauri 版)
async function fetchData(silent = false) {
    const j = await window.__TAURI__.core.invoke('get_data');
    // ...
}
```

2. **CSV 导出**：从 `<a href="/api/export.csv">` 改成调用 IPC 拿字符串 + `Blob`

```javascript
async function exportCsv() {
    const csvString = await window.__TAURI__.core.invoke('export_csv');
    const blob = new Blob(['\ufeff' + csvString], { type: 'text/csv;charset=utf-8' });
    // ...触发下载
}
```

3. **ECharts 引入方式**：从 CDN `<script src="https://cdn.jsdelivr.net/...">` 改成本地 `<script src="echarts.min.js">`（把 echarts 下载后放 static/ 一起打包）

**其它一切保持不变**（HTML 结构、CSS、KPI、明细表、账号面板、v1 表、图表渲染逻辑）。

## 十、三个必须处理的坑

### 坑 1：Kiro workspace 目录名的 base64 变体

Kiro 用**标准 base64 alphabet 但 `+` 换成 `_`、末尾 `=` padding 也换成 `_`**。直接用 `base64::URL_SAFE` 会把中间的 `_` 当 `/`，解出乱码。

```rust
use base64::{Engine, engine::general_purpose::STANDARD};

pub fn decode_kiro_ws(name: &str) -> String {
    let stripped = name.trim_end_matches('_');
    let n_pad = name.len() - stripped.len();
    let body: String = stripped.replace('_', "+");
    let mut padded = body + &"=".repeat(n_pad);
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    STANDARD.decode(&padded)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_else(|_| name.to_string())
}
```

详见 [data-sources.md 三-2](./data-sources.md#workspace-目录名编码重要坑)。

### 坑 2：SQLite 被 Kiro 独占

Kiro 运行时会持有 `state.vscdb` 的独占锁。用 `?mode=ro&immutable=1` URI 打开可以绕过：

```rust
use rusqlite::{Connection, OpenFlags};

pub fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", path.display());
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}
```

如果这条路径仍锁定（极偶尔），返回 `Err` 并让 dashboard 显示"配额未读取到"，不要中断整个响应。

### 坑 3：quota 日志时间戳误匹配

`q-client.log` 里 payload 含 `"nextDateReset": "2026-08-01T00:00:00.000Z"`——这是月度重置日，不是日志时间。**时间戳正则必须强制后面跟 `[` 或 `|`**：

```rust
static TS_LINE_PAT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(?:\[|\|)")
        .unwrap()
});
```

否则大量快照会被打上重置日的时间戳，画出的图完全错乱。

## 十一、分步实施计划

| 阶段 | 内容 | 预估 |
|---|---|---|
| **P0 骨架** | `cargo tauri init` → 拷 static/ → 写 `get_data` 命令返回假数据 → 前端能显示 → 图标 | ~1 h |
| **P1 数据层** | 翻译 `TurnCache` + `V1SessionCache` + `QuotaHistoryCache` + `load_quota` | 3-4 h |
| **P2 打磨** | ECharts 内嵌离线 · CSV 导出 · 前端 fetch→invoke 迁移 · 错误处理 | ~1 h |
| **P3 分发** | `cargo tauri build` → 拿到 exe → 空机器验证运行 | 30 min |
| **合计** | | **约半天到一天** |

### P0 的具体步骤

```bash
# 前置：装 Rust toolchain + tauri CLI
cargo install create-tauri-app
cargo install tauri-cli --version "^2.0"

# 初始化项目结构（我们已经有目录了，只需 init src-tauri）
cd kiro-usage-dashboard
cargo tauri init \
  --app-name "Kiro Usage Dashboard" \
  --frontend-dist ../static \
  --dev-url ""

# 拷 echarts.min.js 到 static/（Python 版当前用 CDN）
curl -L https://cdn.jsdelivr.net/npm/echarts@5.5.1/dist/echarts.min.js -o static/echarts.min.js

# 生成图标
cargo tauri icon path/to/logo.png
```

### P1 的实施顺序（避免一次动太多）

1. `models.rs` 先定义所有 struct，让 `get_data` 能返回空的 `DataResponse`
2. `util.rs` 先做完 base64 解码、路径查找、tz offset
3. `scanner/v2_turns.rs`——把 messages.jsonl 里的 usage_summary 抽出来
4. `quota_snapshot.rs`——读 state.vscdb
5. `scanner/v1_sessions.rs`——扫 workspace-sessions
6. `scanner/quota_history.rs`——扫 logs 里的 quota 快照

**每完成一个模块跑一次 `cargo tauri dev`**，对比 Python 版的 API 响应 diff 一下（同数据字段值应该一致或极接近）。

## 十二、打包分发

```bash
# 开发
cargo tauri dev

# 出正式版
cargo tauri build
# → src-tauri/target/release/kiro-usage-dashboard.exe (~12 MB)
# → src-tauri/target/release/bundle/msi/Kiro Usage Dashboard_0.1.0_x64_en-US.msi
```

**单文件 exe** 就够用（Kiro 用户已经在跑 Kiro，不需要额外装东西）。MSI 是可选安装器，不做也行。

Windows 10 版本 < 1809 的机器需要 WebView2 Runtime，Tauri 会检测并提示。Windows 11 全部自带。

## 十三、可选增强（Rust 版比 Python 版更容易做）

1. **文件变化推送**：用 `notify` crate 监听 `messages.jsonl` 变化，Rust 主动 `emit` 事件到前端 → 前端不需要 15s 轮询
   ```rust
   let (tx, rx) = std::sync::mpsc::channel();
   let mut watcher = notify::recommended_watcher(tx)?;
   watcher.watch(&sessions_root, notify::RecursiveMode::Recursive)?;
   // 处理 rx，emit "data_changed" event
   ```
2. **系统托盘**：右下角常驻，点击展开 dashboard
3. **开机启动**（Tauri 内置 `autostart` plugin）
4. **额度阈值告警**：`currentUsage / usageLimit > 90%` 时弹 Windows 通知
5. **多平台**：`cargo tauri build --target aarch64-apple-darwin` 出 macOS 版；只需要把路径解析从硬编码 `%APPDATA%` 改成 `dirs::config_dir()`

## 十四、验收标准

Rust 版做完后，同一台机器上：

- [ ] `cargo tauri build` 出的 exe 双击运行，浏览器（内嵌 WebView）打开 dashboard
- [ ] 5 个 KPI 数字与 Python 版一致（估算累计 / 跨账号峰值和 / turn 数 / 耗时 / 总 session）
- [ ] 5 个 workspace 全部正确显示（包括含中文的）
- [ ] 3 个账号的 quota 时间序列折线图正确
- [ ] v1 session 表能显示 260 条左右（示例值，本机数据规模）
- [ ] 主题切换、时间粒度、CSV 导出都正常
- [ ] exe 拷贝到另一台没装 Kiro 的机器上运行——数据源缺失时 dashboard 显示"暂无数据"而不是崩溃

## 十五、后续改进入口

- 数据源新增（例如 Kiro 后续版本引入新的日志字段）：改 `scanner/` 对应模块 + `models.rs` 加字段 + 前端加渲染。
- UI 改版：只动 `static/` 目录，不用重新编译 Rust。
- 支持导入其它机器的数据：加 IPC 命令接受用户选文件，用同一 scanner 逻辑跑。
