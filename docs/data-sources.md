# Kiro IDE 本地数据源探索总结

Kiro（AWS 的 AI IDE）在本机会把每次会话的对话历史、每次 turn 的 `Est. Credits Used`、每次拉取账户配额的响应都落盘到本地。本文档汇总所有能挖出用量信息的位置、每个数据源的字段结构、以及踩过的坑。

> 本文档以 Windows 平台为准；macOS / Linux 只是根目录不同，文件格式一致。以下路径全部用环境变量表示。

## 一、数据源全景

| 数据源 | 位置 | 内容 | 覆盖范围 |
|---|---|---|---|
| **v2 sessions** | `~/.kiro/sessions/<sessionId>/<agentSessionId>/messages.jsonl` | 每 turn 的 `usage_summary` 事件（含 credits + 耗时 + 工具调用） | 当前本地 sessions 归属账号，切账号会覆盖 |
| **v1 sessions** | `%APPDATA%\Kiro\User\globalStorage\kiro.kiroagent\workspace-sessions\<encoded_ws>\<uuid>.json` | 旧格式会话（Kiro 数据格式 v1 时代），含 `history` 数组但**无** `usage_summary` | 跨所有 workspace，跨账号（历史） |
| **quota 历史** | `%APPDATA%\Kiro\logs\<YYYYMMDDTHHMMSS>\window*\exthost\kiro.kiroAgent\*.log` | 每次 GetUsageLimits API 调用的完整响应（`currentUsage`、`userId`） | 3~4 天保留期，含多个账号 |
| **quota 快照** | `%APPDATA%\Kiro\User\globalStorage\state.vscdb` | SQLite 库，`kiro.kiroAgent` 键存**当前账号**最新一次配额同步结果 | 仅当前登录账号，仅"当前值"不带历史 |

## 二、v2 sessions（每 turn 精确 credits，主数据源）

### 位置

```
~/.kiro/sessions/
├── <sessionId>/                    # 一层：账号级 hash（切账号会换）
│   └── <agentSessionId>/           # 二层：agent session（每次新对话一个）
│       ├── messages.jsonl          # 事件流（append-only）
│       └── session.json            # 会话元信息
```

`<sessionId>` 一层看似是账号 hash（同一账号下不同 agentSession 共用），但同一账号一直保持不变。切账号时这个 hash 会**被替换**，旧账号 sessions 数据丢失。

### session.json 字段

```json
{
  "schemaVersion": "1.0.0",
  "id": "<agentSessionId>",
  "title": "会话标题（第一条用户消息前 40 字）",
  "agentMode": "vibe",
  "workspacePaths": ["d:\\path\\to\\project"],
  "createdAt": "2026-07-01T02:17:07.966Z",
  "lastModifiedAt": "...",
  "modelId": "claude-opus-4.8"
}
```

**没有 email / userId 字段**——从 session.json 无法追溯是哪个账号。

### messages.jsonl 事件流

每行一个 JSON 对象，字段：`id` / `timestamp` (ISO UTC) / `payload`。`payload.type` 是事件类型。

事件类型分布（示例，本机 316 turn 数据）：

| type | 数量 | 说明 |
|---|---|---|
| `assistant` | 7631 | 助手一次消息（细粒度） |
| `tool_call` / `tool_result` | 7398/7398 | 每次工具调用及结果 |
| `session_metadata` | 2602 | contextUsage 等元信息 |
| `user` | 658 | 用户提问 |
| `turn_start` / `turn_end` | 658/656 | turn 开始/结束标记 |
| **`usage_summary`** | **311** | **★ 每 turn 的用量总结（credits + elapsed）** |
| `session_event` | 311 | session_pause / session_resume 之类 |
| 其它 | 少量 | tombstone / interaction_resolved / steering_inclusion |

### usage_summary 事件结构（关键）

```json
{
  "id": "<executionId>-usage",
  "timestamp": "2026-07-01T02:17:07.966Z",
  "payload": {
    "type": "usage_summary",
    "executionId": "<uuid>",
    "status": "success",              // success / aborted / failed
    "elapsedTime": 810106,            // 毫秒
    "promptTurnSummaries": [
      {
        "unit": "credit",
        "unitPlural": "credits",
        "usage": 18.50065975379768,   // ← 就是 UI 显示的 "Est. Credits Used: 18.50"
        "usedTools": ["execute_pwsh", "read_file", ...]
      }
    ]
  }
}
```

- `promptTurnSummaries` 数组通常长度 0 或 1
- `usage` 是**未折算**的估算值，不是最终扣费
- `aborted` 状态的 turn 里 `promptTurnSummaries` 可能为空（credits 记为 0）

### 关键结论

- **每 turn 的 `Est. Credits Used`** = `payload.promptTurnSummaries[0].usage`
- **每 turn 的 `Elapsed time`** = `payload.elapsedTime`（毫秒）
- **workspace** = `session.json.workspacePaths[0]`
- **模型** = `session.json.modelId`
- **账号信息** = messages.jsonl 里不含（要外部数据源关联）

## 三、v1 sessions（旧格式历史，跨所有 workspace）

### 位置

```
%APPDATA%\Kiro\User\globalStorage\kiro.kiroagent\
├── workspace-sessions/
│   ├── <encoded_workspace_1>/
│   │   ├── <uuid>.json              # 每个 session 一个文件
│   │   ├── sessions.json            # 索引：[{sessionId, title, dateCreated, workspaceDirectory}, ...]
│   │   └── ._migration-<uuid>.json  # 迁移标记（v1→v2）
│   ├── <encoded_workspace_2>/
│   └── ...
├── config.json                      # 全局配置
├── profile.json                     # 当前 profile ARN
├── default/                         # 默认 profile 数据
└── <hash>/<hash>/                   # snapshot 内容缓存（每 turn 快照）
```

### workspace 目录名编码（重要坑）

目录名是 workspace 路径的 base64 编码，但用了 **Kiro 自定义变体**：

- alphabet 基于**标准 base64**（`A-Za-z0-9+/`），但把 `+`（值 62）换成 `_`
- 末尾 padding `=` 也换成 `_`
- 中间的 `_` 是 `+`（不是 `/`，跟 URL-safe base64 不同）

Python 参考实现：

```python
def decode_kiro_ws_name(name):
    stripped = name.rstrip('_')                    # 剥离末尾 padding
    n_pad = len(name) - len(stripped)
    body = stripped.replace('_', '+')              # 中间 _ 换回 +
    padded = body + '=' * n_pad
    while len(padded) % 4:
        padded += '='
    return base64.b64decode(padded).decode('utf-8', errors='replace')
```

验证例：`ZTpca2lyb_i0puWPtw__` → `e:\kiro账号`。

**关键**：Kiro 直到目前版本仍在用这种变体，不是 URL-safe。**用 `base64.urlsafe_b64decode` 会把中间 `_` 当成 `/`（值 63），解出乱码或崩溃**。

### 每个 UUID.json（v1 session 内容）

```json
{
  "sessionId": "<uuid>",
  "title": "第一条用户消息的前几十字",
  "history": [
    {
      "message": { "role": "user", "content": "...", "id": "..." },
      "editorState": { ... },
      "contextItems": []
    },
    {
      "message": { "role": "assistant", "content": "...", "id": "..." },
      "executionId": "<uuid>",
      ...
    },
    // ... 交替
  ],
  "config": { "models": [...], "contextProviders": [...] },
  "workspacePath": "d:\\path\\to\\project",
  "workspaceDirectory": "d:\\path\\to\\project",
  "selectedModel": { "title": "Agent", ... },
  ...
}
```

**重点**：`history` 数组是消息级细粒度，**没有 `usage_summary` 事件**——v1 时代 Kiro 还没引入 credits 追踪。所以 v1 session 只能挖出：

- **turn 数** ≈ `history` 里 `role === 'user'` 的消息数，或 `executionId` 去重数
- **时间戳**：优先用 `sessions.json` 索引里的 `dateCreated`（毫秒时间戳字符串），否则用文件 mtime
- **模型** 从 `config.models[0].title` 或 `selectedModel.title` 读

### `._migration-<uuid>.json`（迁移标记）

```json
{
  "migratedAt": "2026-07-01T01:39:26.625Z",
  "v2SessionId": "<uuid>",
  "workspaceHash": "<hash>",
  "v1WorkspaceDirectory": "d:\\path\\to\\your-project",
  "markerVersion": 2
}
```

这些是"该 session 已迁移到 v2"的标记，**不含实际内容**。扫描时应**跳过**（否则会漏掉真正的 UUID.json 或者算重）。

判断：文件名以 `._migration-` 开头就跳。

### 关键结论

- **v1 session 拿不到 credits**（v1 时代没记）
- **能拿 turn 数、workspace、时间、模型**——用来统计"跨所有项目/账号历史活跃度"
- **workspace 目录名解码必须用 Kiro 自定义变体**，别踩 URL-safe 坑

## 四、quota 历史（多账号时间序列，从 Kiro 运行日志）

### 位置

```
%APPDATA%\Kiro\logs\
├── <YYYYMMDDTHHMMSS>/               # 每次 Kiro 启动一个目录
│   ├── window1/exthost/kiro.kiroAgent/
│   │   ├── Kiro Logs.log            # VS Code 标准日志 (有行头时间戳)
│   │   ├── Kiro Logs.N.log          # 分卷 (超过大小自动切)
│   │   └── q-client.log             # ★ Kiro API 客户端日志 (无行头时间戳，纯 JSON payload)
│   ├── window2/...
│   └── ...
```

### 日志保留期

**只保留最近约 10 次启动的日志目录**（VS Code 默认行为，Kiro 继承）。老于此的日志目录会被自动删除——**这是最大的信息损失来源**。

如果用户想统计 30 天前的账单历史，本地日志已经没了，只能：
1. 登录当时的账号到 Kiro/AWS Q 后台查
2. 用第三方工具（如 Kiro Account Manager）代理 API 时自留一份记录

### 提取 quota 快照的正则

在 log 文件里搜：

```
"currentUsageWithPrecision":\s*([\d.]+)[\s\S]{0,500}?"usageLimit":\s*(\d+)
```

匹配的一次 API 响应示例（`q-client.log` 里）：

```json
{
  ...
  "userInfo": {
    "email": "***SensitiveInformation***",     // 有时被 Kiro 遮蔽为敏感字段
    "userId": "d-XXXXXXXXXX.XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"
  },
  "usageBreakdownList": [{
    "currentUsage": 1234,
    "currentUsageWithPrecision": 1234.56,      // ★ 实际扣费
    "usageLimit": 5000,
    "unit": "INVOCATIONS",
    "resourceType": "CREDIT",
    "overageCap": 10000,
    "overageRate": 0.04,
    "nextDateReset": "2026-08-01T00:00:00.000Z",  // ⚠ 别把这个日期当行时间戳
    ...
  }],
  "subscriptionInfo": { "subscriptionTitle": "KIRO FREE", ... }
}
```

### 时间戳来源优先级

1. **行内时间戳**（`Kiro Logs.log` 有）：正则 `^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}\.\d+)\s+\[`
   - **必须要求后面跟 `[` 或 `|`**——否则会误匹配 payload 里的 `nextDateReset: "2026-08-01T00:00:00.000Z"`（这是月度重置日，不是日志时间）
2. **目录名启动时间**（`Kiro Logs.log` 不带行头的情况，或 `q-client.log`）：目录名格式 `YYYYMMDDTHHMMSS`
3. **文件 mtime**：兜底

### 账号识别

在快照附近向前搜 `"userId":\s*"(d-[0-9a-f]+\.[0-9a-f-]+)"`。同一次 API 响应里 `userInfo.userId` 和 `usageBreakdownList` 相隔几百字节。

**注意**：早期 log 可能不带 userId（Kiro 早版本）。此时用"最近见过的 userId"填补，即按时间顺序处理，见到新 userId 就更新"当前活跃账号"。

### 归零/账号切换识别

`currentUsage` 时间序列上如果出现**断崖式下跌**（例如前一秒还是 5000 左右，下一秒变成 0 或者一个很小的数），大概率是：

- 服务端切换了账号（多账号场景）
- 月度重置（正常账单周期）
- Kiro 后端调整了账户配额策略

**跨账号计费峰值和** = 每个账号在时间轴上的最高 `currentUsage` 之和，反映"这台机器上所有账号累计付过多少费"。

## 五、quota 当前快照（`state.vscdb`）

### 位置

```
%APPDATA%\Kiro\User\globalStorage\state.vscdb
```

SQLite 数据库，`ItemTable` 表存 key/value 键值对。

### 关键 key: `kiro.kiroAgent`

```json
{
  "daysUntilReset": 30,
  "nextDateReset": 1782864000,
  "subscriptionInfo": {
    "subscriptionTitle": "KIRO FREE",
    "type": "Q_DEVELOPER_STANDALONE_FREE",
    ...
  },
  "usageBreakdownList": [ /* 初始订阅快照，可能是 0 */ ],
  "userInfo": {
    "email": "<current-account-email>",
    "userId": "d-XXXXXXXXXX.XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"
  },
  "kiro.resourceNotifications.usageState": {
    "usageBreakdowns": [{
      "currency": { "code": "USD", "symbol": "$" },
      "currentUsage": 295.38,
      "usageLimit": 10000,
      "percentageUsed": 2.95,
      "overageCap": 2500,
      "overageRate": 0.04,
      "resetDate": "2026-08-01T00:00:00.000Z",
      "resourceType": "CREDIT",
      "unit": "INVOCATIONS"
    }],
    "timestamp": 1783480365829
  },
  "lastSelectedModelId": "claude-opus-4.7",
  "lastSelectedEffortLevel": "max"
}
```

**读取顺序**：优先读 `kiro.resourceNotifications.usageState.usageBreakdowns[0]`（这就是 IDE 右下角显示的数字），fallback 到 `usageBreakdownList[0]`（订阅初始值，可能不准）。

### 只读打开 SQLite（避免与 Kiro 抢锁）

```
file:<db_path>?mode=ro&immutable=1
```

即使 Kiro 正在运行且持有独占锁，加了 `immutable=1` 也能读。SQLite 会认为这是一份不会变化的快照。

### 关键结论

- 这个库只有**当前登录账号**的**最新一次**同步值
- 切账号后旧账号的 quota 会被覆盖
- 想拿账号历史必须走"quota 历史（日志）"路径

## 六、常见陷阱汇总

| 坑 | 症状 | 处理 |
|---|---|---|
| Kiro base64 变体被当 URL-safe 解 | 中文 workspace 名解出乱码或 `???` | 用文档四的 `decode_kiro_ws_name` |
| 正则匹配到 payload 里的 ISO 时间戳 | 大部分 quota 快照时间显示为 `2026-08-01 00:00:00`（`nextDateReset` 值） | 时间戳正则强制后面跟 `[` 或 `|` |
| 把 `._migration-*.json` 当 session 内容读 | v1 session 数虚高 / 部分 workspace 显示 0 turn | 文件名以 `._migration-` 开头则跳过 |
| SQLite 被 Kiro 独占 | 读 `state.vscdb` 抛 database is locked | 用 `?mode=ro&immutable=1` URI |
| `usage_summary` 里 `promptTurnSummaries` 长度 0 | credits 累加不对 | 空则记为 0，或按 `status != 'success'` 单独统计 |
| session.json 无 userId 字段 | 无法知道每个 session 属于哪个账号 | 只能通过 quota 日志的时间关联反推 |
| 估算累计 ≠ 实际扣费 | 两个数字差数量级（估算大） | 是正常的，见文档二说明 |

## 七、字段与数据源快查表

| 想统计什么 | 数据源 | 字段 |
|---|---|---|
| 每 turn 的 Est. Credits | v2 sessions | `messages.jsonl` → `payload.promptTurnSummaries[0].usage` |
| 每 turn 的耗时 | v2 sessions | `messages.jsonl` → `payload.elapsedTime` (ms) |
| 每 turn 用了哪些工具 | v2 sessions | `messages.jsonl` → `payload.promptTurnSummaries[0].usedTools[]` |
| 会话标题 & workspace | v2 sessions | `session.json` → `title`, `workspacePaths[0]` |
| 使用的模型 | v2 sessions | `session.json` → `modelId` |
| 历史会话数（含旧格式） | v1 sessions | `workspace-sessions/<ws>/<uuid>.json` |
| 每个 workspace 的会话数 | v1 + v2 合并 | workspace 目录名解码 + `workspacePaths[0]` 合并去重 |
| 服务端实际扣费历史 | quota 历史 | `logs/**/q-client.log` 里 `currentUsageWithPrecision` |
| 多账号切换轨迹 | quota 历史 | `logs/**/q-client.log` 里 `userId` + `currentUsage` 时间序列 |
| 本月配额进度 | state.vscdb | `kiro.resourceNotifications.usageState.usageBreakdowns[0]` |

## 八、可复用的正则片段

```python
# usage_summary 事件的完整 JSON 行匹配（快速预筛，避免每行 json.loads）
'"type":"usage_summary"' in line

# quota 快照
r'"currentUsageWithPrecision":\s*([\d.]+)[\s\S]{0,500}?"usageLimit":\s*(\d+)'

# 日志行头时间戳（必须带 [ 或 | 后缀，避免误匹配 payload）
r'(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(?:\[|\|)'

# userId
r'"userId"\s*:\s*"(d-[0-9a-f]+\.[0-9a-f-]+)"'

# Kiro workspace 目录名的时间戳（用于 quota 日志目录）
r'^(\d{8})T(\d{6})'
```
