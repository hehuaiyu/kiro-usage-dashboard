# Kiro Usage Dashboard

统计 Kiro（IDE 模式）每次运行输出的 `Est. Credits Used` 和 `Elapsed time`，
按小时/日/周/月多粒度趋势、24×7 热力图、Top Sessions、Workspace 分组、工具调用分布、明细表 + CSV 导出，
本地实时刷新（默认 15 秒），暗色 / 亮色主题一键切换。

数据完全从本地读取，只读，不会改动 Kiro 的任何文件：

- `~/.kiro/sessions/<sid>/<agent-sid>/messages.jsonl`：抽取 `usage_summary` 事件（每 turn 的 credits + 耗时 + 工具调用）
- `~/.kiro/sessions/<sid>/<agent-sid>/session.json`：workspace、title、模型
- `%APPDATA%/Kiro/User/globalStorage/state.vscdb`：本月配额进度（Kiro 后台实际扣费）

---

## 快速开始

### 双击启动（推荐）

在文件资源管理器里双击 `kiro_dashboard.cmd`，会自动打开浏览器到 <http://127.0.0.1:8765/>。

### 命令行启动

```cmd
:: 默认参数
python kiro_dashboard.py

:: 换端口
python kiro_dashboard.py --port 9000

:: 端口被占用时自动往上找
python kiro_dashboard.py --auto-port

:: 允许局域网访问（仅在受信任的网络里使用，会暴露你的用量数据）
python kiro_dashboard.py --host 0.0.0.0

:: 不自动开浏览器
python kiro_dashboard.py --no-browser
```

关闭：命令行窗口按 `Ctrl+C`，或直接关掉窗口。

### 命令行参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--host HOST` | `127.0.0.1` | 监听地址。默认只允许本机访问；也可以用环境变量 `KIRO_DASHBOARD_HOST` |
| `--port PORT` | `8765` | 监听端口。也可以用环境变量 `KIRO_DASHBOARD_PORT` |
| `--auto-port` | off | 端口被占用时自动尝试下一个可用端口（最多试 20 个） |
| `--no-browser` | off | 启动后不自动打开浏览器 |
| `--sessions-root PATH` | `~/.kiro/sessions` | Kiro 会话数据根目录 |
| `--state-db PATH` | 自动定位 | Kiro state.vscdb 路径 |

---

## 页面功能

顶部工具栏
- **时间粒度**：时 / 日 / 周 / 月 —— 影响主趋势图分桶。日以下用**渐变柱状图**，周/月用**渐变面积图**
- **时间范围**：今日 / 本周 / 本月 / 30 天 / 全部 —— 影响所有视图的数据范围
- **实时指示器**（绿点脉动）：每 15 秒静默拉一次数据；超过 3 倍间隔没成功变黄警告
- **手动刷新**、**主题切换**（暗/亮）

KPI 指标条
- **估算累计 (Est.)**：当前范围内每次 turn 的 `Est. Credits Used` 之和
- **本月实际扣费 (Billed)**：Kiro 后台报的实际计费值（跟 UI 里"账户配额"一致）
- **Turn 数**：含计费与未计费（aborted 有时不计费）
- **累计耗时**：所有 turn 的 `Elapsed time` 之和
- **本月配额进度**：环形进度 + 距重置天数

> 「估算」和「实际扣费」通常不相等：前者是 Kiro 每次报的原始估价，后者经过免费额度抵扣、缓存优惠、订阅折扣等折算。

主趋势图
- 主柱状/面积：credits
- 可选叠加：turn 数（右轴）、耗时（右轴 2）
- 桶数超过 30 自动出现 dataZoom 滑块
- **点击柱子** → 明细表自动过滤到该时段

24×7 小时热力图
- 横轴 24 小时、纵轴周一到周日、色深表示 credits 密度
- 一眼看出你几点最费钱
- 点击格子过滤明细表

工具调用分布 (Treemap)
- 每 turn 的 credits 均摊到它调用的工具（`按 credits`）
- 或统计每个工具的调用次数（`按 turn 数`）
- 点击方块过滤该工具的 turn

Top Sessions
- 按 credits 降序，最多 15 条
- 点击行过滤该 session 的所有 turn

Workspace 分组（环形图）
- 多项目 credits 占比
- 点击扇区过滤该 workspace

Turn 明细表
- 时间倒序，分页 50 条/页
- 表头点击排序
- 搜索 title / workspace / model
- 按 status / workspace 筛选
- **导出 CSV**：反映当前所有筛选条件

点击联动
- 主图/热力图/Treemap/Top Sessions/Workspace 任何一个点击都会在页面顶部出现"已筛选"提示条
- 点击 **清除** 按钮回到全量视图

---

## 数据字段说明

一条 turn = Kiro 每次 "用户提问 → 完成响应" 后打印的那句
`Est. Credits Used: 35.15  Elapsed time: 27m 52s`。

字段来自 messages.jsonl 里的 `usage_summary` 事件：

| 字段 | 来源 | 含义 |
|---|---|---|
| `credits` (`c`) | `promptTurnSummaries[0].usage` | UI 显示的 `Est. Credits Used` |
| `elapsed_ms` (`e`) | `elapsedTime` | UI 显示的 `Elapsed time` |
| `status` (`s`) | `status` | `success` / `aborted` / `failed` |
| `tools` | `promptTurnSummaries[0].usedTools` | 本次 turn 调过的工具列表 |
| `workspace` (`ws`) | `session.json` 的 `workspacePaths[0]` 目录名 | 归属项目 |
| `title`、`model` | 同上 | 会话标题、使用的模型 |

---

## 常见问题

**Q: 端口 8765 被占用怎么办？**
A: 加 `--auto-port` 自动找下一个可用端口，或用 `--port 9000` 指定别的。

**Q: 打开是空白 / 图表出不来？**
A: 检查浏览器控制台。最常见是 ECharts CDN 加载失败（离线环境）。工具当前依赖 jsdelivr / unpkg，如果长期离线用请告诉我加个 `--offline` 内嵌 echarts 的模式。

**Q: 配额显示"未读取到"？**
A: `state.vscdb` 被正在运行的 Kiro 独占了。dashboard 在打开 Kiro 时仍能读，但极偶尔碰到锁定时刻。刷新一下通常就好。

**Q: 数据能实时刷新到什么程度？**
A: Kiro 每次 turn 结束时会写 `messages.jsonl`，dashboard 默认 15 秒读一次，所以 Kiro 那边跑完一句你这边等 15 秒内能看到新数据。

**Q: 数据安全吗？会上传吗？**
A: 完全本地。服务只监听 `127.0.0.1`（本机），不主动做任何外部请求。除了 ECharts 的 CDN（前端页面从 CDN 拉 JS 文件），没有其它对外通信。

**Q: 想给团队用行不行？**
A: 加 `--host 0.0.0.0` 就能局域网访问。**但注意**：这会把你的用量、session 标题等信息暴露给能访问你 IP 的人。生产环境请加认证层。

**Q: 我改了 CSS/JS 想看效果？**
A: `Ctrl+C` 停服务，重新启动；浏览器 `Ctrl+F5` 强制刷新（脚本给静态资源加了 `Cache-Control: no-store`，普通刷新一般也行）。

---

## 目录结构

```
tools/kiro_stats/
├── kiro_stats.py         # 命令行版（跑批处理、集成到脚本用）
├── kiro_dashboard.py     # Dashboard 后端（HTTP 服务器）
├── kiro_dashboard.cmd    # Windows 双击启动脚本
├── static/
│   ├── index.html        # 前端页面
│   ├── style.css         # 样式（暗/亮双主题）
│   └── app.js            # 前端逻辑（数据获取 + 聚合 + ECharts 渲染）
└── README.md
```

---

## 未来演进

当前是 Python + Web 前端的原型。前端页面（`static/`）就是最终要嵌入 Tauri exe 的 UI，
后端 API 契约 (`/api/data` 的 JSON schema) 已经稳定，之后要打包成 exe 时：

- Rust 侧：把 `kiro_dashboard.py` 里的 `TurnCache.scan()` 和 `load_quota()` 1:1 翻译成 Rust（大概 200 行）
- 前端：`static/` 目录整个塞进 Tauri 的 `dist/`，不用改代码
- 打包：Tauri CLI 一键 build，产物是 10 MB 左右的单文件 exe

---

## 反馈

用得不顺就直接说，UI / 交互 / 图表类型都能调。
