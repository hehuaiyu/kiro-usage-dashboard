# Kiro Usage Dashboard

统计 [Kiro IDE](https://kiro.dev/) 在本地的**用量数据**——每次响应报的 `Est. Credits Used`、`Elapsed time`、跨账号的实际扣费历史、跨项目的会话足迹——用一个本地 Web dashboard 直观展示。

> Kiro 自带的 UI 只能看到"当前登录账号本月配额进度"这一个数字。这个工具挖出本地保存的所有历史用量，并支持多账号、多项目、多时间粒度分析。

## 特性

- **多维度趋势**：按小时 / 日 / 周 / 月切换，渐变柱状 + 折线双轴
- **24×7 小时热力图**：一眼看出你在哪个时段最费 credits
- **跨账号统计**：如果你切换过多个 Kiro 账号，全部账号的 quota 时间序列 + 峰值和
- **跨项目全景**：合并 v1 / v2 数据格式，覆盖所有用过 Kiro 的 workspace
- **工具调用分布**：Treemap 看哪些工具（`execute_pwsh` / `read_files` / ...）吃掉的 credits 最多
- **明细可搜可导 CSV**：每 turn 精确记录，方便二次分析
- **实时刷新**：默认 15 秒静默拉数据，Kiro 那边跑完 turn 这边立刻显示
- **本地持久化历史库** (v0.3+, Rust exe 独有)：启动时把当前 Kiro 数据 snapshot 到本地 SQLite，即便 Kiro 那边切账号/日志滚动/版本升级导致原始数据丢失，工具仍保留完整历史。用户可以随时"清除历史库"重置
- **暗/亮双主题**：一键切换，样式跟 Vercel Analytics / Linear 风格靠齐
- **零第三方依赖**：Python 版仅用 stdlib，只需 Python 3.9+
- **数据本地**：默认只监听 `127.0.0.1`，不做任何外部通信（除 ECharts CDN 拉图表库）

## 快速开始

项目提供 **两个可选版本**：

| | Python 原型 | Rust exe (Tauri) |
|---|---|---|
| 入口 | `prototype-python/kiro_dashboard.cmd` | `src-tauri/target/release/kiro-usage-dashboard.exe` |
| 依赖 | Python 3.9+（脚本会自动探测 miniconda / anaconda / py launcher / PATH） | 无依赖（通常 Win10/11 已自带 WebView2 Runtime；缺失时装 [Evergreen](https://developer.microsoft.com/microsoft-edge/webview2/) 一次） |
| 分发大小 | 全套代码 ~1.2 MB | 单文件 exe ~5 MB |
| 启动速度 | 冷启 1-2 s（Python interp 启动） | 冷启 < 500 ms |
| 用法场景 | 想直接看代码 / 想改逻辑 / 别人机器上跑（有 Python 即可） | 想快 / 想给不装 Python 的人用 / 想双击就跑 |

### 版本 A：Python 原型（无需编译，最快上手）

**双击启动**：

```
kiro-usage-dashboard/prototype-python/kiro_dashboard.cmd
```

自动打开浏览器到 <http://127.0.0.1:8765/>。

**命令行**：

```bash
cd kiro-usage-dashboard/prototype-python
python kiro_dashboard.py                    # 默认参数
python kiro_dashboard.py --port 9000        # 换端口
python kiro_dashboard.py --host 0.0.0.0     # 局域网访问（注意隐私）
python kiro_dashboard.py --no-browser       # 不自动开浏览器
python kiro_dashboard.py --auto-port        # 端口占用时自动往上找
```

详细用法看 [`prototype-python/README.md`](./prototype-python/README.md)。

### 版本 B：Rust exe（需一次性搭环境，之后双击 exe）

**前置**：

- 装 [Rust](https://rustup.rs)（`rustup-init.exe` 一键装）
- 装 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 并勾选 **Desktop development with C++** workload（`link.exe` 必需，约 3-5 GB）
- WebView2 Runtime（Win10/11 一般自带；缺失时从 [微软官网](https://developer.microsoft.com/microsoft-edge/webview2/) 装 Evergreen Runtime）

> **无 GPU / 虚拟机 / 远程桌面环境注意**：这类机器上 WebView2 首次渲染可能卡顿。本工具已默认禁用 GPU 加速（`--disable-gpu`）走软件渲染规避此问题。即便如此，首次启动仍有约 4-5 秒是 WebView2 (Chromium) runtime 冷启动，属框架固有开销。

**build + 运行**：

```powershell
cd src-tauri

# dev 模式（快速验证；带 console 显示日志）
cargo run

# release 模式（12 MB 优化 exe，无 console）
cargo build --release
.\target\release\kiro-usage-dashboard.exe

# 打包成 NSIS 安装器（需要先 cargo tauri icon <logo.png>）
cargo install tauri-cli --version "^2.0" --locked   # 一次性
cargo tauri icon path\to\your-logo.png              # 生成 icons/
cargo tauri build
```

产物：`src-tauri/target/release/kiro-usage-dashboard.exe` + `bundle/nsis/*.exe`（安装器）。

**首次 build 慢是正常的**——Cargo 会下载编译几百个依赖 crate，5-15 分钟；后续增量 build 秒级。

## 页面结构

**顶栏**（全局，各视图共用）
- 当前视图标题
- 时间范围切换（今日 / 本周 / 本月 / 30 天 / 全部）
- 实时状态指示器（脉动圆点 + "刚刚 / 5s 前"）
- 手动刷新按钮
- 主题切换（暗 ↔ 亮）

**左侧导航**（6 视图，窄图标栏 + hover tooltip，可用 URL hash 直达 `#glance` / `#overview` / `#trends` / ...）

| 视图 | 内容 |
|---|---|
| **简约**（默认首页） | 欢迎大卡（吉祥物 + 问候语 + 历史库状态）+ 4 张 Bento KPI 大卡 + 今日趋势 + "查看完整仪表盘"入口。一眼扫完 |
| **总览** | 指标说明（折叠）+ KPI 卡（估算累计 / 跨账号计费峰值和 / Turn 数 / 累计耗时 / Session 数）+ 精简趋势速览 |
| **趋势** | 完整趋势图（credits 柱状 + turn 数/耗时可选折线，粒度切换 时/日/周/月，超过 30 桶出 dataZoom）+ 24×7 小时热力图 |
| **工具与工作区** | 工具调用分布 Treemap（可切"按 credits / 按 turn 数"）+ Workspace 环形图（v1+v2 合并占比）+ Top Sessions 排行表 |
| **账号历史** | 多账号 quota 时间序列折线（跨账号叠加）+ 账号列表（uid / 时段 / 峰值 / 归零次数 / 快照数） |
| **明细** | Tab 切换 v2 Turn 明细表（搜索/状态/workspace 筛选/排序/分页/CSV 导出）与 v1 Sessions 表（旧格式历史） |

**响应式**：窗口宽 < 900px 时 sidebar 自动折叠到顶部横向，只显示图标。

## 目录结构

```
kiro-usage-dashboard/
├── README.md                      # 本文件
├── LICENSE                        # MIT
├── .gitignore
├── docs/
│   ├── data-sources.md            # 本地 Kiro 数据源位置和字段说明
│   ├── upgrade-guide.md           # Kiro 版本升级时的排查/适配指南
│   └── changelog.md               # 变更记录（版本时间线）
├── prototype-python/              # 版本 A：Python 原型
│   ├── kiro_dashboard.py          # HTTP 服务器 + 数据扫描 (~966 行)
│   ├── kiro_dashboard.cmd         # Windows 双击启动脚本
│   ├── kiro_stats.py              # CLI 版（跑批处理用）
│   ├── static/                    # 前端页面
│   │   ├── index.html
│   │   ├── style.css
│   │   └── app.js
│   └── README.md
├── src-tauri/                     # 版本 B：Rust + Tauri v2 后端
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   └── src/
│       ├── main.rs                # Tauri 入口 + IPC 命令 (get_data / export_csv / clear_history)
│       ├── models.rs              # DataResponse 等 struct
│       ├── util.rs                # base64 变体解码 / 路径查找 / 时区
│       ├── quota_snapshot.rs      # 读 state.vscdb (rusqlite)
│       ├── history_store.rs       # v0.3+ 本地持久化 SQLite (turns / v1_sessions / quota_snapshots)
│       └── scanner/
│           ├── v2_turns.rs        # 扫 messages.jsonl 里的 usage_summary
│           ├── v1_sessions.rs     # 扫 workspace-sessions 拿 v1 历史
│           └── quota_history.rs   # 扫 logs 拿多账号 quota 时间序列 + 独立聚合函数
└── ui/                            # 前端（与 Python 版共用同一份，微调后 Tauri 复用）
    ├── index.html
    ├── style.css
    ├── app.js                     # fetch → invoke 已适配 Tauri
    └── echarts.min.js             # 打包时随 exe 分发，完全离线
```

**Python 原型 vs Rust 版的关系**：数据源解析逻辑 1:1 对应，前端页面几乎完全共用（只有 `fetch('/api/data')` 在 Tauri 里被替换成 `invoke('get_data')`）。所有 KPI、图表、明细表在两个版本里表现完全一致。

## 数据来源

工具只读本地文件，不做任何网络请求（除前端从 CDN 加载 ECharts）。四个数据源：

| 数据源 | 提供什么 |
|---|---|
| `~/.kiro/sessions/**/messages.jsonl` | 每 turn 的 credits 和耗时（当前账号） |
| `%APPDATA%/Kiro/User/globalStorage/kiro.kiroagent/workspace-sessions/` | v1 旧格式历史会话（跨所有 workspace） |
| `%APPDATA%/Kiro/logs/**/*.log` | 每次拉配额时的服务端响应快照（含多账号 userId） |
| `%APPDATA%/Kiro/User/globalStorage/state.vscdb` | 当前账号的本月配额进度 |

详细字段结构和解析细节看 [docs/data-sources.md](./docs/data-sources.md)。

## 数据边界（诚实说明）

本地能拿到的数据有限制，请知悉：

- **首次运行工具之前的数据**：只能从 Kiro 当时还保留的原始文件里挖。Kiro 切账号会覆盖 v2 sessions、日志文件约 3-7 天滚动清理 —— 这些**首次运行前就已丢失的部分**工具救不回来
- **v1 sessions**：Kiro 旧数据格式的历史会话，**没有 credits 信息**（v1 时代还没引入 credits 追踪）—— 只能数 turn 数和会话数
- **服务器完整账单**：想看服务器视角的完整月度账单，只能登录对应账号到 Kiro / AWS Q Developer 后台查看

**但是**（v0.3+ Rust exe 版）：**只要工具跑过至少一次，那次扫到的数据就永久落在本地 SQLite 历史库里**。之后 Kiro 那边即使清账号、滚日志、大版本迁移，工具仍能展示历史。所以：

> **建议**：装完这个工具后，日常打开 Kiro 之前先启动一下 dashboard 让它 snapshot 当前数据。养成习惯后，Kiro 那边任何变化都不影响历史查看。

清除历史库：sidebar 底部的"清除历史库"按钮（有二次确认）。

## 两个版本各自的价值

**Python 原型不会被 Rust 版取代**——两者定位不同：

- **Python 原型** = 参考实现 + 快速改动。想加数据源、想改指标定义、想学习 Kiro 数据结构的人对着 Python 代码改一改立刻能看效果。数据源探索的 `docs/data-sources.md` 就是从这份原型摸出来的。

- **Rust exe** = 生产分发。目标用户只想双击一个 exe 看到自己的用量数字，不用管 Python、不用装环境。

## 常见问题

**Q：这工具会不会把我的用量数据上传到什么地方？**
A：不会。Python 服务默认只监听 `127.0.0.1`，Rust 版将来也一样。除了前端页面从 CDN 加载 ECharts 图表库这个可选行为外，工具本身不做任何外部通信。

**Q：Kiro 正在运行时能用吗？**
A：能。工具用 `?mode=ro&immutable=1` 只读打开 SQLite，不会跟 Kiro 抢锁；`messages.jsonl` 是 append-only 追加，读到的是快照。

**Q：我看到 KPI 卡里"估算累计"是 5000 但"跨账号计费峰值和"只有 3000，为什么？**
A：两个是不同层面的数字：估算是 Kiro 每次响应的**未折算原始估价**，实际扣费经过 Kiro 免费额度、缓存优惠、订阅折扣等折算，通常远小于估算。详见页面顶部"指标说明"面板。

**Q：ECharts 加载失败页面白屏怎么办？**
A：CDN 被墙。Python 版可以手动下载 `echarts.min.js` 放到 `prototype-python/static/`，再把 `index.html` 里的 CDN `<script>` 改成本地路径。Rust 版会默认内嵌 ECharts。

**Q：Kiro 更新了新版本，数据结构变了怎么办？**
A：本工具的数据源解析可能失效。请提 issue 或参考 [docs/data-sources.md](./docs/data-sources.md) 自行适配。

## 变更记录

详见 [docs/changelog.md](./docs/changelog.md)。最新版本可在 [GitHub Releases](https://github.com/hehuaiyu/kiro-usage-dashboard/releases) 下载编译好的 Windows exe。

## 贡献

欢迎 issue / PR。如果你的 Kiro 版本产生了新的日志字段或者你发现了没被本工具覆盖的数据源，欢迎补充到 `docs/data-sources.md`。

## License

[MIT](./LICENSE)

---

*本项目与 Kiro / AWS 无官方关联。Kiro 是 Amazon Web Services, Inc. 的产品。*
