# 变更记录

按时间倒序，最新版本在最前面。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 精简版。

标签约定：
- **新增** —— 新特性
- **变更** —— 已有功能的行为变化
- **修复** —— Bug fix
- **文档** —— 只改文档
- **构建** —— 构建/发布流程调整

---

## v0.4.0 — UI 风格化 + 简约视图 + 无 GPU 启动性能修复

### 修复

- **无 GPU 机器启动白屏十几秒** —— 这是本版最重要的修复。WebView2（Chromium 内核）在没有 GPU 的机器上，会先尝试初始化 GPU 进程、等待超时、再 fallback 到软件渲染，导致首次渲染白屏 8-9 秒 + 鼠标卡顿。
  - 修复：启动时设置环境变量 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--disable-gpu --disable-gpu-compositing"`，让 WebView2 直接走软件渲染，跳过 GPU 初始化超时。实测首帧绘制从 8 秒降到 16 毫秒。
  - 参考微软官方 [Disable hardware acceleration](https://learn.microsoft.com/en-us/answers/questions/1227551/)。
- **窗口启动白闪** —— `tauri.conf.json` 窗口加 `"backgroundColor": "#0E0F13"`，webview 内容画上前的白底改成跟 UI 一致的深色，消除白闪。

### 变更

- **UI 风格化：向"简约干练"靠拢**
  - Sidebar 从 220px 宽导航收窄到 68px 图标模式（hover 弹出 tooltip）
  - 黑猫吉祥物 logo（内联 SVG，紫色眼睛呼应主色）
  - 卡片圆角加大（统一 `--radius-*` 体系，卡片 16-22px），hover 上抬 + 阴影
  - 时间范围/图标按钮改胶囊状
  - 去掉各处紫色辉光（顶部高光条、hero 渐变底、CTA 发光），风格更冷静
  - 亮色主题改暖灰米调（`--card` #e6e9ef → #e0ddd5），护眼不刺眼
- **echarts 加载优化** —— 从 `<script defer>` 改成 app.js 里动态异步注入（`loadEchartsAsync`），推迟到首帧绘制之后。图表函数加 echarts 未就绪的容错（先出数据/骨架，图表随后补上）。
- **IPC 命令标记 `(async)`** —— `get_data` / `export_csv` / `clear_history` 改 `#[tauri::command(async)]`，避免同步命令在主线程执行阻塞 UI。

### 新增

- **简约视图（glance）** —— 新的默认首页。欢迎大卡（黑猫 + 问候语 + 历史库状态）+ 4 张 Bento KPI 大卡 + 一张今日趋势 + "查看完整仪表盘"入口。定位是"打开一眼扫完"，深挖走完整的 6 视图。
- 左侧导航从 5 项增至 6 项（简约 / 总览 / 趋势 / 工具与工作区 / 账号历史 / 明细）。

### 性能说明（诚实）

- 修复后启动仍有约 **4-5 秒**，几乎全是 **WebView2 runtime 进程冷启动**（Chromium 引擎固有，压不掉）。渲染本身已经流畅（16ms）。
- 追求秒开需换非 Chromium 的 GUI 框架（egui / Slint 等），后端 Rust 数据层可复用。此为后续探索方向。

---

## v0.3.0 — 本地持久化历史库 + Kiro 升级适配指南

### 新增

- **本地持久化历史库**（SQLite，位置见 `HistoryStats.db_path`，Windows 默认 `%APPDATA%\kiro-usage-dashboard\history.db`）
  - 三张表：`turns` / `v1_sessions` / `quota_snapshots`，各自带主键做去重（`execution_id` / `workspace_full+session_id` / `uid+ts_secs`）
  - 启动时把当前 Kiro 扫到的数据用 `INSERT OR IGNORE` upsert 到本地库
  - Dashboard 展示的是历史库读全量后的结果 —— Kiro 数据被清除（切账号覆盖 / 日志滚动 3-7 天 / v1→v2 迁移）时，本地历史仍在
  - 未来 Kiro 升级导致 scanner 失效时，历史库里的老记录依然能正常展示
- **新 IPC 命令 `clear_history`** —— 清空三张表 + VACUUM，返回清除前的统计。前端 sidebar 底部"清除历史库"按钮触发，有二次确认对话框（列出即将删除的条数、起始日期，明确后果）
- **新文档 `docs/upgrade-guide.md`** —— Kiro 版本升级适配指南：失效症状对照表 / 诊断三步 / 改动位点速查表 / 验证流程 / 升级前 snapshot
- 页脚新增历史库状态："历史库: X turn · Y v1 · Z quota（起始 YYYY-MM-DD）本次 +N"，鼠标悬停显示 db 文件路径和大小

### 变更

- `DataResponse` 新增 `history_stats` 字段（`HistoryStats` struct）
- `get_data` 流程改为「扫当前 → upsert 到历史 → 从历史读全量 → 用 `aggregate_accounts_from_snapshots` 聚合 accounts」
- `scanner/quota_history.rs` 里 snapshot→Account 聚合逻辑独立成 `pub fn aggregate_accounts_from_snapshots`（history_store 从 SQLite 读快照后复用同一份聚合逻辑，保证一致性）
- `export_csv` 改为从历史库读，导出的是**累积完整历史**（含 Kiro 已清但本地保留的记录）
- Footer 显示修 bug：`v2: X 文件/undefined turn` 中 `undefined` 来自不存在的 `s.turns` 字段，改成 `v2: X 文件 (Y ms)`

### 构建

- 新依赖模块 `history_store.rs`（约 350 行）
- rusqlite 已有依赖 (v0.32 with `bundled` feature)，零新增外部依赖
- Release exe 从 5.03 MB 增至 5.08 MB (+45 KB)

---

## v0.2.0 — UI 重构 + 关键 Bug 修复

### 修复

- **Tauri v2 前端拿不到 invoke** —— `tauri.conf.json` 打开 `app.withGlobalTauri: true`，前端 `invokeGetData` 加 `__TAURI_INTERNALS__` 兜底路径。修复症状：exe 打开后 KPI 全 0、图表空白、状态圆点红、"服务时间 1970/1/1"。
- **趋势图 legend 与 Y 轴刻度打架** —— 删掉图内 y 轴 name（`credits / turns / h`，本来面板标题已经说明），`grid.top` 从 40 加大到 48，legend 字色从 `--fg-dim` 改成 `--fg` 清晰。

### 变更

- **UI 布局重构：单页 → 左侧导航 + 5 视图**
  - `.sidebar`（220px 固定，含 brand + 5 项 nav + 版本号）
  - `.main-area`（自适应，含顶栏 + 视图容器 + 页脚）
  - 5 个视图各自 `<section class="view" data-view="xxx">`，同时只显示一个
  - 视图列表：
    - **总览** —— KPI 卡 + 精简趋势速览
    - **趋势** —— 完整趋势图（含粒度/叠加控件） + 24×7 热力图
    - **工具与工作区** —— 工具 Treemap + Workspace 环形 + Top Sessions
    - **账号历史** —— 多账号 quota 折线 + 账号明细表
    - **明细** —— v2 Turn 表 + v1 Sessions 表（Tab 切换）
  - 顶栏简化：视图标题 + 全局范围切换 + 状态圆点 + 刷新 + 主题
  - 支持 URL hash 路由（`#overview` / `#trends` / ...），可用 window `hashchange` 同步
  - 响应式：窗口宽 < 900px 时 sidebar 自动折叠到顶部只显示图标

- **亮色主题：从"刺眼白"改成"柔和灰调"**
  - `--card` 从 `#ffffff` 拉到 `#e6e9ef`（占屏幕大半的卡片背景，是刺眼的关键色）
  - `--bg` 从 `#f7f8fa` 拉到 `#d9dde5`（灰蓝底）
  - `--fg` 从 `#14161c`（近纯黑）到 `#232935`（深灰蓝）
  - 增加亮色下的柔光背景叠加 + 卡片细阴影，跟暗色主题对称

- **明细 v2 / v1 拆分成 Tab** —— 之前 v2 明细表和 v1 sessions 表堆在一起滚动，现在都在"明细"视图内用横向 Tab 切换。

### 新增

- `renderTrendPreview()` —— 总览视图的精简趋势图。固定按日粒度、不含叠加、无 dataZoom，独立 chart id `chart-trend-preview` 避免和"趋势"视图的完整图 id 冲突。
- `VIEW_TITLES` / `VALID_VIEWS` / `switchView(name)` / `initRouter()` / `switchDetailTab(tab)` —— 5 个视图/tab 切换与路由函数。
- `data-view-link` 属性支持 —— 总览页"查看完整趋势 →"这类内部跳转能触发 `switchView` 而不是浏览器 hash 跳转。

### 构建

- `dist/kiro-usage-dashboard.exe` —— release 版从 target 拷到 `dist/` 目录做稳定分发位置，`.gitignore` 里 `dist/` 已忽略（不进仓库，走 GitHub Release）。
- 前端资源（`ui/*.html|.css|.js|.min.js`）都被 Tauri 内嵌到 exe 里，改前端后**必须**重新 `cargo build --release`。

---

## v0.1.0 — 首次公开发布

### 新增

- **Python 原型** (`prototype-python/`)：`kiro_dashboard.py` 本地 HTTP 服务器 + 数据扫描（约 966 行，纯 stdlib，Python 3.9+）。`kiro_dashboard.cmd` 双击启动脚本，自动探测 miniconda / anaconda / py launcher / PATH。CLI 版 `kiro_stats.py` 走批处理。
- **Rust + Tauri v2 骨架** (`src-tauri/`)
  - `models.rs` —— 8 个 Serialize struct，字段与 Python `/api/data` JSON 严格一致
  - `util.rs` —— Kiro base64 变体解码（含单元测试）+ 时区偏移 + 路径查找
  - `quota_snapshot.rs` —— rusqlite URI 模式（`file://...?mode=ro&immutable=1`）读 `state.vscdb`
  - `scanner/v2_turns.rs` —— 增量扫 `messages.jsonl` 提取 `usage_summary`
  - `scanner/v1_sessions.rs` —— 扫 `workspace-sessions/`，解码 Kiro base64 变体目录名
  - `scanner/quota_history.rs` —— `regex::bytes` 匹配 `.log` 提取多账号 quota 时间序列
  - `main.rs` —— Tauri 入口 + `get_data` / `export_csv` 两个 IPC 命令
- **前端** (`ui/`)
  - `index.html` + `style.css` + `app.js` + `echarts.min.js`（本地内嵌避免 CDN 拉不到）
  - `app.js` 检测到 Tauri 环境时走 `invoke`，否则 fallback 到 `fetch('/api/data')`（保留和 Python 版兼容）
- **文档**
  - `README.md` —— 项目总览、两版本对照、快速开始
  - `docs/data-sources.md` —— 完整 Kiro 数据源字段参考（含 base64 变体解码坑）
- **图标**：`src-tauri/logo.png`（1024×1024 紫→蓝渐变 + K，placeholder）+ `icons/`（32×32 / 128×128 / @2x / icon.ico 多尺寸 / icon.png）
- **License**：MIT

### 数据源

覆盖 4 个本地路径：
- `~/.kiro/sessions/**/messages.jsonl` —— 每 turn 的 `usage_summary`（当前账号）
- `%APPDATA%/Kiro/User/globalStorage/kiro.kiroagent/workspace-sessions/` —— v1 旧格式历史（跨所有 workspace）
- `%APPDATA%/Kiro/logs/**/*.log` —— quota 快照时间序列（跨多账号）
- `%APPDATA%/Kiro/User/globalStorage/state.vscdb` —— 当前账号本月配额

### 已知边界

- **估算累计**只覆盖当前本地 sessions 归属账号，Kiro 切换账号时旧账号数据会被覆盖
- **实际扣费历史**依赖 Kiro 本地日志保留（通常 3-7 天），更早的账单快照本地已丢失
- **v1 sessions** 没有 credits 信息（v1 时代 Kiro 未追踪 credits）
