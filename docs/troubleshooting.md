# 常见问题与排障笔记

本文档记录使用、开发、推送本项目时踩过的环境坑，方便下次遇到快速排掉。

---

## Git 推送到 GitHub 失败

### 症状 1：`getaddrinfo() thread failed to start`

```
fatal: unable to access 'https://github.com/xxx/xxx.git/':
  getaddrinfo() thread failed to start
```

同时手动 `curl https://github.com/...` 也 fail、`ping github.com` 却通——只是 IP 看着是**保留网段**（如 `198.18.x.x`、`10.x.x.x`）。

### 根因

**本机的代理软件启用了"虚拟网卡 / TUN 模式"劫持了 DNS**。常见于：

- Clash for Windows / Clash Verge / Karing / v2rayN 等的 **TUN 模式** 或 **系统代理**
- 公司装的 SSL 拦截产品（Zscaler、Netskope、Fortinet 等）
- 一些 GitHub 加速器软件的"透明代理"模式

这些工具把 `github.com` 的 DNS 解析劫持到本地虚拟网卡地址（`198.18.0.0/15` 是保留网段，常被 CGNAT/虚拟网卡使用），Git 走 HTTPS 时 libcurl 处理这种伪 IP 就会挂在 `getaddrinfo` 线程创建这一步。

用 `nslookup github.com` 如果发现返回域名带公司后缀（如 `github.com.xxx.org`），基本能锁定是这类劫持。

### 处理方式（三选一）

**方式 A：关代理软件的 TUN / 虚拟网卡模式**
- Clash 系：把系统代理开关关掉，或改成 Rule 模式
- 公司代理软件：按 IT 说明关掉 SSL 拦截或走白名单

关完直接重推：
```bash
git push -u origin main
```

**方式 B：改走 SSH 协议（推荐做长期方案）**

SSH 用 22 端口，通常不被 HTTPS 代理劫持。

```bash
# 先测 SSH 通不通
ssh -T git@github.com

# 通了就改 remote 走 SSH
git remote set-url origin git@github.com:<用户名>/<仓库名>.git

# 推
git push -u origin main
```

前提：本机已经生成过 SSH key 并加到 GitHub 账号（Settings → SSH keys）。没有的话：
```bash
ssh-keygen -t ed25519 -C "<你的邮箱>"
type $HOME\.ssh\id_ed25519.pub   # Windows；把输出粘到 GitHub
```

**方式 C：给 git 指定代理**

如果你知道公司/软件的 HTTP 代理地址：
```bash
git config --global http.https://github.com.proxy http://<代理地址>:<端口>
```

---

## Windows cmd 脚本相关

### 症状：双击 `.cmd` 报 `'leExtensions' 不是内部或外部命令` 之类的莫名错误

### 根因

三个坑同时发作（本项目的 `kiro_dashboard.cmd` 已全部规避）：

1. **中文注释**：cmd 用 GBK 解析 UTF-8 中文字节时会产生特殊字符（如 `>` `|`），把命令行切断
2. **LF 换行符**：Windows cmd 严格要求 CRLF，LF-only 的 .cmd 会解析错乱
3. **嵌套 `if not defined X if not defined Y (...)` 单行嵌套 if 判断错乱**：在含 subroutine set 变量的场景下会错判

### 处理

编辑 `.cmd` 文件时：
- 注释用**纯 ASCII 英文**
- 换行符保存为 **CRLF**（VS Code 右下角切换）
- 不用嵌套单行 if，改用 `goto :label` 显式控制流

参考：`prototype-python/kiro_dashboard.cmd` 是修好后的干净版本。

### 症状：`python not found`（exit code 9009）

双击 `.cmd` 起的是纯 cmd 窗口，**没有激活 conda 环境**，所以 PATH 里没有 python。

### 处理

`kiro_dashboard.cmd` 已实现自动探测：优先 `%KIRO_PYTHON%` 环境变量 → 常见 miniconda / anaconda 路径 → `py` launcher → PATH 里的 `python`。

如果自动探测失败，手动指定：
```powershell
setx KIRO_PYTHON "C:\path\to\python.exe"
```
然后**新开一个终端**（`setx` 只在新会话生效）再双击 `.cmd`。

---

## Kiro 数据源相关

### 症状：workspace 名字显示成 `xxx???xx` 之类的乱码

### 根因

`workspace-sessions/` 目录名用了 **Kiro 自定义 base64 变体**（中间 `_` 是 `+` 的替代，末尾 `_` 是 `=` 的 padding），如果用标准 URL-safe base64 解码会把中间 `_` 当成 `/`，中文字节被错解。

### 处理

用文档 [`data-sources.md` 三节](./data-sources.md#workspace-目录名编码重要坑) 里的 `decode_kiro_ws_name` 实现。

### 症状：quota 快照的时间戳大量显示为月度重置日

大部分快照的时间戳变成了同一个 `2026-08-01T00:00:00` 之类。

### 根因

正则匹配 `currentUsage` 附近的时间戳时，误抓到了 payload 里的 `"nextDateReset": "2026-08-01T00:00:00.000Z"` 字段——那是月度重置日，不是日志时间戳。

### 处理

时间戳正则**强制后面跟 `[` 或 `|`**（区分日志行头和 payload 内 ISO 时间）：
```regex
(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(?:\[|\|)
```

### 症状：state.vscdb 读取 `database is locked`

Kiro 正在运行时会持独占锁。

### 处理

用 SQLite URI 模式 + immutable：
```
file:<path>?mode=ro&immutable=1
```

---

## 前端页面

### 症状：dashboard 打开是白屏 / 图表不显示

### 根因

ECharts 从 CDN 加载失败（内网 / 网络受限）。

### 处理

**方案 1：本地内嵌 echarts.min.js**
```powershell
curl.exe -L https://cdn.jsdelivr.net/npm/echarts@5.5.1/dist/echarts.min.js -o prototype-python\static\echarts.min.js
```
然后编辑 `prototype-python/static/index.html`，把两处 CDN `<script>` 标签的 `src` 改成 `/echarts.min.js`。

**方案 2：换 CDN**
把 `jsdelivr` 换成 `unpkg.com` 或国内镜像。

---

## 后续遇到新问题往这里加

格式建议：`症状 → 根因 → 处理`，每个问题独立一节。
