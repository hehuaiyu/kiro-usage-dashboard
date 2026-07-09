# 变更记录

按时间倒序，最新版本在最前面。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 精简版。

标签约定：
- **新增** —— 新特性
- **变更** —— 已有功能的行为变化
- **修复** —— Bug fix
- **文档** —— 只改文档
- **构建** —— 构建/发布流程调整

---

## v0.2.0 — UI 重构 + 关键 Bug 修复

### 修复

- **Tauri v2 前端拿不到 invoke** —— `tauri.conf.json` 打开 `app.withGlobalTauri: true`，前端 `invokeGetData` 加 `__TAURI_INTERNALS__` 兜底路径。修复症状：exe 打开后 KPI 全 0、图表空白、状态圆点红、"服务时间 1970/1/1"。详见 [`troubleshooting.md` — Tauri v2 前端拿不到 window.__TAURI__](./troubleshooting.md#tauri-v2-前端拿不到-window__tauri__v02-关键坑)。
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
  - `docs/design-rust-tauri.md` —— Rust 迁移架构设计
  - `docs/troubleshooting.md` —— 代理/编码/编译踩坑
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
