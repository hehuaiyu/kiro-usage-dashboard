# Kiro 版本升级适配指南

这个工具靠**逆向解析** Kiro 的本地文件（`~/.kiro/sessions/`、`state.vscdb`、`logs/`）拿数据。Kiro 每次更新可能改字段名、变换目录结构、甚至删掉某个数据源——工具会**部分或完全失效**。

本文档告诉你：**怎么判断失效了 · 怎么定位 · 怎么修 · 怎么验**。

---

## 一、失效症状对照表

打开 dashboard 后如果看到下列任何一项，八成是 Kiro 变了：

| 症状 | 可能原因 | 首先看 |
|---|---|---|
| KPI 全 0，图表空白，右上圆点红 | v2 sessions 目录结构变 / IPC 失败 / 前端契约不合 | 启动 dev 模式看 stderr（下面第二节） |
| Turn 数正常，但 credits 全 0 | `usage_summary` 事件字段被改名或移除 | `messages.jsonl` 里搜 `usage_summary` |
| Turn 少了很多（比预期几百条只剩几十） | v2 sessions 存储位置或目录结构变了 | `~/.kiro/sessions/` 树 |
| "所有 Session" 明显不对 | v1 或 v2 会话解析漏了 | `%APPDATA%/Kiro/User/globalStorage/kiro.kiroagent/workspace-sessions/` |
| 账号切换历史无数据、账号数 0 | quota 响应字段改名或日志格式变 | `%APPDATA%/Kiro/logs/**/*.log` 抓 `currentUsageWithPrecision` |
| Dashboard 右下"本月配额进度"没有 | `state.vscdb` 里 kiro.kiroAgent 的 value schema 变了 | SQLite 里 SELECT |
| workspace 名字全是乱码 | Kiro 换了目录名编码方式 | `workspace-sessions/` 目录名对比 |
| 工具启动直接崩溃 | 数据源根目录路径变了 | 看 stderr 里"数据源目录"打印 |

---

## 二、诊断三步

### Step 1：跑 dev 版看日志

release exe 是没有 console 的（`windows_subsystem = "windows"`），看不到诊断输出。用 dev 版：

```powershell
cd src-tauri
cargo run     # 会保留 console 窗口，stderr 直接打印到终端
```

启动后看输出：

```
[kiro-usage-dashboard] 数据源目录：
  v2 sessions:   C:\Users\<你>\.kiro\sessions
  v1 sessions:   C:\Users\<你>\AppData\Roaming\Kiro\User\globalStorage\kiro.kiroagent\workspace-sessions
  logs (quota):  C:\Users\<你>\AppData\Roaming\Kiro\logs
  state.vscdb:   C:\Users\<你>\AppData\Roaming\Kiro\User\globalStorage\state.vscdb
  history db:    C:\Users\<你>\AppData\Roaming\kiro-usage-dashboard\history.db
[kiro-usage-dashboard] 预热完成: v2 turn=XXX, v1 session=XXX, account=XXX
```

**判断**：
- 数据源目录**不存在** → 说明 Kiro 换目录了，改 `src-tauri/src/util.rs` 里 `default_sessions_root()` 等
- 目录存在但**扫出来数量明显不对** → 走 Step 2 具体看文件内容

### Step 2：dump 一行原始数据看结构

**v2 turns**：找最新的 `messages.jsonl`，用 Python 或 PowerShell 抓一行看：

```powershell
# 找最新的 messages.jsonl
$latest = Get-ChildItem -Path "$env:USERPROFILE\.kiro\sessions" -Recurse -Filter "messages.jsonl" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
$latest.FullName

# 抓 usage_summary 事件（如果存在）
Get-Content $latest.FullName | Select-String '"usage_summary"' | Select-Object -First 1
```

对比字段名和现在 `scanner/v2_turns.rs` 里代码里期望的字段。**常见变化**：字段名从 `estCreditsUsed` 变成 `credits_used`、`elapsed_ms` 变成 `elapsed`、事件类型从 `usage_summary` 变成 `turn_completed` 等。

**v1 sessions**：workspace-sessions 目录里每个子目录是一次 workspace。目录名是 Kiro base64 变体编码（见 [`data-sources.md`](./data-sources.md)）。看看目录名的结构是否变了（比如新增了前缀、编码换了）。

**quota logs**：Kiro logs 里搜 `currentUsageWithPrecision`：

```powershell
Get-Content "$env:APPDATA\Kiro\logs\*\*.log" -Raw | Select-String -Pattern '"currentUsageWithPrecision"\s*:\s*[\d.]+' -AllMatches | Select-Object -First 3
```

**state.vscdb**：用命令行 sqlite3（Windows 可能没自带，先装或者用 [DB Browser for SQLite](https://sqlitebrowser.org/) GUI）：

```bash
sqlite3 "$env:APPDATA/Kiro/User/globalStorage/state.vscdb"
> .schema ItemTable
> SELECT key FROM ItemTable WHERE key LIKE '%kiro%';
> SELECT substr(value, 1, 500) FROM ItemTable WHERE key = 'kiro.kiroAgent';
```

看 value 是 JSON 还是别的，字段名对不对得上。

### Step 3：diff 新旧结构

拿到新 Kiro 的一份原始数据后，跟旧版对比：

- 用 `git log -p` 翻 workflow 里以前的 `scanner/*.rs` 提交，找期望的老字段名
- 或者对比 [Python 版原型](../prototype-python/kiro_dashboard.py) 里对应扫描函数中的正则/JSON key

---

## 三、改动位点速查表

按数据源分类，改对应文件即可：

| 数据源 | 现象 | Python 版位置 | Rust 版位置 |
|---|---|---|---|
| **v2 turns** (credits, elapsed, tools) | Turn 少 / credits 全 0 / status 全空 | `prototype-python/kiro_dashboard.py` 里 `TurnCache._parse_messages_jsonl` | `src-tauri/src/scanner/v2_turns.rs` |
| **v1 sessions** (旧格式历史) | v1 session 数 0 / workspace 乱码 | `prototype-python/kiro_dashboard.py` 里 `V1SessionCache._parse_session` | `src-tauri/src/scanner/v1_sessions.rs` |
| **quota history** (多账号时间序列) | 账号 0 个 / 峰值全 0 | `prototype-python/kiro_dashboard.py` 里 `QuotaHistoryCache._parse_log_file` | `src-tauri/src/scanner/quota_history.rs` |
| **state.vscdb** (本月配额) | Quota 卡显示无数据 | `prototype-python/kiro_dashboard.py` 里 `load_quota_from_state_db` | `src-tauri/src/quota_snapshot.rs` |
| **数据源根目录** (整个目录换位置) | 启动即报所有源"路径不存在" | `prototype-python/kiro_dashboard.py` 顶部路径常量 | `src-tauri/src/util.rs` 里 `default_*_root` |
| **workspace 目录名编码** | v1 workspace 全是乱码 | `prototype-python/kiro_dashboard.py` 里 `decode_kiro_ws_name` | `src-tauri/src/util.rs` 里 `decode_kiro_ws_name` |

**通用改法**：

- 字段改名 → 找到对应位置改字符串常量或正则；保留旧 key 做后兼容（`obj.get("credits_used") or obj.get("estCreditsUsed")`）
- 目录结构变 → 改 walk 起点或 glob 模式
- 编码换 → 参考 `data-sources.md` 里的编码规则，可能要重写 decode 函数

**Python 和 Rust 版建议同步改**：数据源逻辑相同，两个版本本来就是 1:1 对应（`docs/data-sources.md` 是唯一真源）。

---

## 四、验证流程

改完后按顺序跑：

### 4.1 单元测试

```bash
cd src-tauri
cargo test
```

现有测试覆盖：`util::decode_kiro_ws_name`（3 种编码变体）、`util::basename`（Windows/Unix 路径）、`util::iso_to_ms`。改字段名的时候可以加一个 mock JSON 单元测试。

### 4.2 用旧数据 regression

**关键**：改动前先备份一份 `~/.kiro/sessions/`（一份小样本就够，选一两个 workspace），改完先用这份跑一次确保**旧数据仍能正确解析**：

```powershell
# 备份
Copy-Item "$env:USERPROFILE\.kiro\sessions" -Destination "$env:TEMP\kiro-sessions-backup" -Recurse

# 改完代码后，替换 default_sessions_root() 让它指向备份 (或临时改环境变量)
```

跑 dashboard，对比 KPI 数字是否和改动前一致。

### 4.3 用新数据 forward

用最新的 Kiro 数据跑，确认前端展示不出错、KPI 数字合理。

### 4.4 前后端契约验证

新增字段？前端 `ui/app.js` 里对应的 `state.xxx = j.xxx || null` 有没有加？后端返回 struct 有没有 `#[derive(Serialize)]`？JSON key 名对得上不？

---

## 五、升级 Kiro 前的动作（重要）

**关键动作：升级前先跑本工具一次让持久化历史库 snapshot 当前所有数据。**

v0.3+ 版本每次启动会把当前 Kiro 数据合并进本地 SQLite 历史库（`%APPDATA%\kiro-usage-dashboard\history.db`）。**Kiro 升级如果导致解析失效或数据迁移丢失，历史库里的老记录仍能正常展示。**

步骤：

1. 关掉 Kiro
2. 运行 kiro-usage-dashboard.exe，等页脚显示"历史库：X 条 turn (起始 YYYY-MM-DD)"，确认数字合理
3. 关掉工具
4. 升级 Kiro
5. 再打开工具 —— 如果扫不到新数据但历史库仍有，说明 Kiro 变了结构，走上面第一~四节修 scanner
6. Scanner 修好后，新扫的数据会 append 到历史库，历史不丢

**清除历史库的场景**（sidebar 底部"清除历史库"按钮）：

- 换电脑 / 换账号，想从头开始
- 调试工具时想 reset 到干净状态
- 磁盘紧张（一般不会，历史库通常几百 KB 到几 MB）

**清除前会有二次确认弹框**。清除后原始 Kiro 数据不受影响（工具从不写 Kiro 目录），但工具本地累计的历史一去不复返。

---

## 六、如果解析完全崩掉

如果 Kiro 变化太大暂时修不好 scanner：

1. **优先保护历史库** — 别清除按钮点了；不要删 `%APPDATA%\kiro-usage-dashboard\history.db`
2. **Downgrade Kiro**（如果可能）到已知能解析的版本，跑一次工具让最近的数据也进历史库
3. 提 [issue](https://github.com/hehuaiyu/kiro-usage-dashboard/issues) 附上：
   - 新 Kiro 版本号
   - `messages.jsonl` 一行样本（脱敏后）
   - `stderr` 输出的启动日志
   - 期望但没显示的数据类型（turn / v1 session / quota history / state.vscdb）

---

## 七、可参考的历史迁移案例

工具本身经历过的 Kiro 版本变化，可以作为改动模板参考：

- **v1 → v2 sessions**：`~/.kiro/sessions/` 结构从旧的单文件 `history.json` 迁移到嵌套目录 `<sid>/<aid>/messages.jsonl`。v1 数据被移到 `workspace-sessions/` 且没有 credits 字段。本工具用**双源合并**处理（v1 补充 v0 时代的会话数，v2 提供 credits）
- **workspace 目录名编码**：Kiro 用了自定义 base64 变体（`+`→`_`，padding `=`→`_`），不是 URL-safe base64。见 `data-sources.md` "workspace 目录名编码" 一节。变体解码是维护重灾区，改前建议先跑单元测试

改动可以查这个工具的 [git 历史](https://github.com/hehuaiyu/kiro-usage-dashboard/commits/main)，特别是 `scanner/` 目录的提交。
