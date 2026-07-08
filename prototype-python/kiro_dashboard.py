# -*- coding: utf-8 -*-
"""Kiro 用量 Dashboard 后端。

本文件是一个自包含的本地 HTTP 服务器，从 Kiro 的本地会话数据里抽出
每次 turn 的 Credits/耗时/工具调用，通过 JSON API 供前端 (static/) 渲染。

主要端点：
    GET /                → static/index.html
    GET /app.js          → static/app.js
    GET /style.css       → static/style.css
    GET /api/data        → JSON，包含 quota + 全部 turns（前端负责聚合）
    GET /api/export.csv  → 所有 turns 的 CSV
    GET /api/health      → {"status":"ok"}

设计原则：
- 零第三方依赖，只用 Python stdlib。
- 增量扫描：只重读 mtime 变化的 messages.jsonl，冷启后热请求 < 50ms。
- 只监听 127.0.0.1（默认），不对外暴露；host/port 可配置。
- 数据完全由前端聚合渲染，本模块只做"读文件 + 转 JSON"。
"""

from __future__ import annotations

import argparse
import base64
import csv
import glob
import io
import json
import os
import re
import socket
import sqlite3
import sys
import threading
import time
import webbrowser
from collections import Counter, defaultdict
from datetime import datetime, timezone
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Dict, Iterable, List, Optional, Tuple
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
STATIC_DIR = os.path.join(HERE, "static")

# ---------------------------------------------------------------------------
# 数据源扫描（带缓存）
# ---------------------------------------------------------------------------


class TurnCache:
    """把每个 messages.jsonl 的 usage_summary 结果按文件 mtime 缓存，
    只重扫变化的文件，避免每次 API 都全量重解析。"""

    def __init__(self, sessions_root: str) -> None:
        self.sessions_root = sessions_root
        # {msg_path: (mtime, [turn_dict, ...])}
        self._cache: Dict[str, Tuple[float, List[dict]]] = {}
        # {agent_dir: (mtime, meta_dict)}   # session.json 的缓存
        self._meta_cache: Dict[str, Tuple[float, dict]] = {}
        self._lock = threading.Lock()

    # ---- 单文件解析 ----

    @staticmethod
    def _parse_msg_file(msg_path: str, meta: dict) -> List[dict]:
        """从一个 messages.jsonl 里抽出所有 usage_summary，返回 turn 字典列表。"""
        session_id = os.path.basename(os.path.dirname(os.path.dirname(msg_path)))
        agent_id = os.path.basename(os.path.dirname(msg_path))
        workspace = _guess_workspace(meta)
        title = meta.get("title") or ""
        model = meta.get("modelId") or ""

        turns: List[dict] = []
        try:
            with open(msg_path, "r", encoding="utf-8") as f:
                for line in f:
                    # 快速子串预筛，避免每行都 json.loads
                    if '"type":"usage_summary"' not in line:
                        continue
                    try:
                        ev = json.loads(line)
                    except Exception:
                        continue
                    p = ev.get("payload", {})
                    if p.get("type") != "usage_summary":
                        continue

                    ts_ms = _iso_to_ms(ev.get("timestamp", ""))
                    if ts_ms is None:
                        continue

                    summaries = p.get("promptTurnSummaries") or []
                    credits = 0.0
                    tools: List[str] = []
                    if summaries:
                        credits = float(summaries[0].get("usage") or 0.0)
                        tools = list(summaries[0].get("usedTools") or [])

                    turns.append({
                        "t": ts_ms,                                    # ms UTC
                        "c": credits,                                  # est. credits
                        "e": int(p.get("elapsedTime") or 0),           # elapsed ms
                        "s": p.get("status") or "unknown",             # success/aborted/failed
                        "ws": workspace,
                        "sid": session_id,
                        "aid": agent_id,
                        "eid": p.get("executionId") or "",
                        "title": title,
                        "model": model,
                        "tools": tools,
                    })
        except FileNotFoundError:
            pass
        except Exception as exc:
            print(f"[warn] 解析 {msg_path} 失败: {exc}", file=sys.stderr)
        return turns

    def _load_meta(self, agent_dir: str) -> dict:
        """读 session.json，带 mtime 缓存。"""
        meta_path = os.path.join(agent_dir, "session.json")
        try:
            mtime = os.path.getmtime(meta_path)
        except OSError:
            return {}
        cached = self._meta_cache.get(agent_dir)
        if cached and cached[0] == mtime:
            return cached[1]
        try:
            with open(meta_path, "r", encoding="utf-8") as f:
                meta = json.load(f)
        except Exception:
            meta = {}
        self._meta_cache[agent_dir] = (mtime, meta)
        return meta

    # ---- 对外接口 ----

    def scan(self) -> Tuple[List[dict], Dict[str, int]]:
        """全量扫描，返回 (turns, stats)。第二次调用时增量。"""
        t0 = time.perf_counter()
        pattern = os.path.join(self.sessions_root, "*", "*", "messages.jsonl")
        files = list(glob.iglob(pattern))
        reused = 0
        reparsed = 0

        with self._lock:
            active_paths = set()
            all_turns: List[dict] = []
            for msg_path in files:
                active_paths.add(msg_path)
                try:
                    mtime = os.path.getmtime(msg_path)
                except OSError:
                    continue
                cached = self._cache.get(msg_path)
                if cached and cached[0] == mtime:
                    all_turns.extend(cached[1])
                    reused += 1
                    continue
                # 需要重扫
                agent_dir = os.path.dirname(msg_path)
                meta = self._load_meta(agent_dir)
                turns = self._parse_msg_file(msg_path, meta)
                self._cache[msg_path] = (mtime, turns)
                all_turns.extend(turns)
                reparsed += 1

            # 清理已删除的文件
            for stale in list(self._cache.keys()):
                if stale not in active_paths:
                    del self._cache[stale]

        all_turns.sort(key=lambda t: t["t"])
        took_ms = int((time.perf_counter() - t0) * 1000)
        stats = {
            "files": len(files),
            "reparsed": reparsed,
            "reused": reused,
            "turns": len(all_turns),
            "took_ms": took_ms,
        }
        return all_turns, stats


def _iso_to_ms(iso: str) -> Optional[int]:
    """'2026-07-01T02:17:07.966Z' → 1719800227966（UTC 毫秒）。"""
    if not iso:
        return None
    if iso.endswith("Z"):
        iso = iso[:-1] + "+00:00"
    try:
        dt = datetime.fromisoformat(iso)
    except ValueError:
        return None
    return int(dt.timestamp() * 1000)


def _guess_workspace(meta: dict) -> str:
    paths = meta.get("workspacePaths") or []
    if not paths:
        return "(no-workspace)"
    p = str(paths[0]).rstrip("/\\")
    return os.path.basename(p) or p


# ---------------------------------------------------------------------------
# 配额（state.vscdb）
# ---------------------------------------------------------------------------


def load_quota(state_db_path: Optional[str]) -> Optional[dict]:
    """从 Kiro 的 state.vscdb 读本月配额进度。数据库被 Kiro 独占时返回 None。"""
    if state_db_path is None:
        state_db_path = os.path.join(
            os.environ.get("APPDATA", ""),
            "Kiro", "User", "globalStorage", "state.vscdb",
        )
    if not os.path.exists(state_db_path):
        return None
    try:
        con = sqlite3.connect(f"file:{state_db_path}?mode=ro&immutable=1", uri=True)
        row = con.execute(
            "SELECT value FROM ItemTable WHERE key='kiro.kiroAgent'"
        ).fetchone()
        con.close()
    except Exception:
        return None
    if not row:
        return None
    try:
        j = json.loads(row[0])
    except Exception:
        return None

    sub_title = (j.get("subscriptionInfo") or {}).get("subscriptionTitle")

    ns = j.get("kiro.resourceNotifications.usageState") or {}
    breakdowns = ns.get("usageBreakdowns") or []
    if breakdowns:
        b = breakdowns[0]
        return {
            "source": "resourceNotifications",
            "current": b.get("currentUsage"),
            "limit": b.get("usageLimit"),
            "percentage": b.get("percentageUsed"),
            "overage_cap": b.get("overageCap"),
            "overage_rate": b.get("overageRate"),
            "reset_date": b.get("resetDate"),
            "subscription": sub_title,
        }

    ubl = j.get("usageBreakdownList") or []
    if ubl:
        b = ubl[0]
        return {
            "source": "usageBreakdownList",
            "current": b.get("currentUsage"),
            "limit": b.get("usageLimit"),
            "percentage": None,
            "overage_cap": b.get("overageCap"),
            "overage_rate": b.get("overageRate"),
            "reset_date": None,
            "subscription": sub_title,
        }
    return None


# ---------------------------------------------------------------------------
# v1 sessions 扫描（Kiro 数据格式 v1 时代的历史，跨所有 workspace）
# ---------------------------------------------------------------------------


def _decode_kiro_ws_name(name: str) -> str:
    """Kiro 的 workspace-sessions 目录名编码规则（实测验证）：
      - 使用标准 base64 alphabet (A-Za-z0-9+/)
      - 但把 62 的 '+' 换成 '_'（避免 Windows/URL 敏感字符）
      - 末尾 padding '=' 也换成 '_'
    """
    stripped = name.rstrip("_")
    n_pad = len(name) - len(stripped)
    # 中间的 '_' 换回 '+'，末尾单独补 '=' padding
    body = stripped.replace("_", "+")
    padded = body + "=" * n_pad
    while len(padded) % 4:
        padded += "="
    try:
        return base64.b64decode(padded).decode("utf-8", errors="replace")
    except Exception:
        return name  # 解码失败就用原名


def _workspace_basename(full_path: str) -> str:
    """项目路径 → 项目名（末段目录）。"""
    if not full_path:
        return "(unknown)"
    p = full_path.rstrip("/\\")
    return os.path.basename(p) or p


class V1SessionCache:
    """扫 `%APPDATA%\\Kiro\\User\\globalStorage\\kiro.kiroagent\\workspace-sessions\\`。

    v1 时期的每个 session 是一个独立的 JSON 文件（含 history 数组）。
    这里提取轻量元信息 + turn 数（history 里 role=user 的消息数量近似）。
    v1 session 没有 usage_summary，所以拿不到 credits。
    """

    def __init__(self, ws_sessions_root: str) -> None:
        self.root = ws_sessions_root
        # {file_path: (mtime, session_dict)}
        self._cache: Dict[str, Tuple[float, Optional[dict]]] = {}
        self._lock = threading.Lock()

    @staticmethod
    def _parse_session(fp: str, workspace_full: str,
                       idx_info: Optional[dict]) -> Optional[dict]:
        try:
            with open(fp, "r", encoding="utf-8") as f:
                data = json.load(f)
        except Exception:
            return None

        sid = data.get("sessionId") or os.path.splitext(os.path.basename(fp))[0]
        history = data.get("history") or []

        # turn 数：以 executionId 去重优先，否则用 role=user 的消息数
        exec_ids = set()
        user_msgs = 0
        for h in history:
            eid = h.get("executionId")
            if eid:
                exec_ids.add(eid)
            msg = h.get("message") or {}
            if msg.get("role") == "user":
                user_msgs += 1
        turn_count = max(len(exec_ids), user_msgs)

        # 模型：config.models 或 selectedModel
        model = ""
        cfg_models = (data.get("config") or {}).get("models") or []
        if cfg_models:
            model = cfg_models[0].get("title") or cfg_models[0].get("model") or ""
        sel = data.get("selectedModel") or {}
        if isinstance(sel, dict) and sel.get("title"):
            model = sel["title"]

        # 时间戳：优先 sessions.json 索引里的 dateCreated（毫秒）
        ts_ms = None
        if idx_info:
            dc = idx_info.get("dateCreated")
            if dc:
                try:
                    ts_ms = int(dc)
                except Exception:
                    pass
        if ts_ms is None:
            try:
                ts_ms = int(os.path.getmtime(fp) * 1000)
            except OSError:
                return None

        title = data.get("title") or (idx_info.get("title") if idx_info else "") or ""

        return {
            "source": "v1",
            "session_id": sid,
            "title": title,
            "workspace": _workspace_basename(workspace_full),
            "workspace_full": workspace_full,
            "t": ts_ms,
            "turn_count": turn_count,
            "model": model,
        }

    def scan(self) -> Tuple[List[dict], Dict[str, int]]:
        """返回 (v1_sessions, stats)。增量按文件 mtime。"""
        t0 = time.perf_counter()
        results: List[dict] = []
        files_seen = 0
        reparsed = 0
        reused = 0

        if not os.path.isdir(self.root):
            return results, {"files": 0, "reparsed": 0, "reused": 0, "took_ms": 0}

        with self._lock:
            active_paths = set()
            # 每个 workspace 目录
            for ws_entry in os.listdir(self.root):
                ws_dir = os.path.join(self.root, ws_entry)
                if not os.path.isdir(ws_dir):
                    continue
                workspace_full = _decode_kiro_ws_name(ws_entry)

                # 读该 workspace 的 sessions.json 索引（可能没有）
                idx_map: Dict[str, dict] = {}
                idx_path = os.path.join(ws_dir, "sessions.json")
                if os.path.exists(idx_path):
                    try:
                        with open(idx_path, "r", encoding="utf-8") as f:
                            idx = json.load(f)
                        if isinstance(idx, list):
                            for item in idx:
                                sid = (item or {}).get("sessionId")
                                if sid:
                                    idx_map[sid] = item
                    except Exception:
                        pass

                # 每个 UUID.json
                try:
                    entries = os.listdir(ws_dir)
                except OSError:
                    continue
                for fn in entries:
                    # 只处理 UUID.json，跳过 sessions.json 索引和 ._migration 标记
                    if fn == "sessions.json":
                        continue
                    if fn.startswith("._migration-"):
                        continue
                    if not fn.endswith(".json"):
                        continue
                    fp = os.path.join(ws_dir, fn)
                    active_paths.add(fp)
                    files_seen += 1

                    try:
                        mtime = os.path.getmtime(fp)
                    except OSError:
                        continue

                    cached = self._cache.get(fp)
                    if cached and cached[0] == mtime:
                        if cached[1] is not None:
                            results.append(cached[1])
                        reused += 1
                        continue

                    sid_prefix = fn[:-5]  # 去 .json
                    parsed = self._parse_session(fp, workspace_full, idx_map.get(sid_prefix))
                    self._cache[fp] = (mtime, parsed)
                    if parsed is not None:
                        results.append(parsed)
                    reparsed += 1

            # 清缓存
            for stale in list(self._cache.keys()):
                if stale not in active_paths:
                    del self._cache[stale]

        results.sort(key=lambda x: x["t"])
        return results, {
            "files": files_seen,
            "reparsed": reparsed,
            "reused": reused,
            "took_ms": int((time.perf_counter() - t0) * 1000),
        }


# ---------------------------------------------------------------------------
# 多账号 billed 时间序列（从 Kiro 运行日志）
# ---------------------------------------------------------------------------


class QuotaHistoryCache:
    """从 %APPDATA%\\Kiro\\logs\\ 里提取每个账号的 currentUsage 时间序列。

    数据源：
      - q-client.log     每次拉 profile 时的完整 JSON 响应（含 userId + usageBreakdown）
      - Kiro Logs.*.log  resource-notifications 事件（含 currentUsage）

    时间戳来源优先级：
      1. 行内 "YYYY-MM-DD HH:MM:SS.mmm [xxx]" 格式（Kiro Logs 里有）
      2. 目录名 <YYYYMMDDTHHMMSS> 的启动时间
      3. 文件 mtime
    """

    _combo_pat = re.compile(
        rb'"currentUsageWithPrecision":\s*([\d.]+)'
        rb'[\s\S]{0,600}?"usageLimit":\s*(\d+)'
    )
    _uid_pat = re.compile(rb'"userId"\s*:\s*"(d-[0-9a-f]+\.[0-9a-f-]+)"')
    _ts_line_pat = re.compile(
        rb'(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(?:\[|\|)'
    )
    _dir_ts_pat = re.compile(r'^(\d{8})T(\d{6})')

    def __init__(self, logs_root: str) -> None:
        self.root = logs_root
        # {log_path: (mtime, [snapshot, ...])}
        self._cache: Dict[str, Tuple[float, List[dict]]] = {}
        self._lock = threading.Lock()

    def _parse_one_log(self, path: str) -> List[dict]:
        """解析一个 log 文件，返回其中所有 quota 快照。"""
        try:
            with open(path, "rb") as f:
                data = f.read()
        except Exception:
            return []

        # 目录启动时间兜底
        dir_ts = None
        for p in os.path.dirname(path).split(os.sep):
            m = self._dir_ts_pat.match(p)
            if m:
                try:
                    dir_ts = datetime.strptime(m.group(1) + m.group(2),
                                                 "%Y%m%d%H%M%S")
                except ValueError:
                    pass
                break
        try:
            mtime_dt = datetime.fromtimestamp(os.path.getmtime(path))
        except OSError:
            mtime_dt = None

        snaps: List[dict] = []
        for m in self._combo_pat.finditer(data):
            cur = float(m.group(1))
            lim = int(m.group(2))
            # 前向找最近的 userId 和 timestamp
            back = data[max(0, m.start() - 5000):m.start()]

            uid_matches = self._uid_pat.findall(back)
            uid = uid_matches[-1].decode() if uid_matches else None

            ts_matches = self._ts_line_pat.findall(back)
            ts: Optional[datetime] = None
            if ts_matches:
                d, t = ts_matches[-1]
                try:
                    ts = datetime.strptime(
                        d.decode() + " " + t.decode()[:15],
                        "%Y-%m-%d %H:%M:%S.%f",
                    )
                except ValueError:
                    try:
                        ts = datetime.strptime(
                            d.decode() + " " + t.decode()[:8],
                            "%Y-%m-%d %H:%M:%S",
                        )
                    except ValueError:
                        pass
            if ts is None:
                ts = dir_ts or mtime_dt
            if ts is None:
                continue

            # 转 UTC 毫秒（这里 ts 是本地时间的 naive datetime，我们把它当作本地时钟）
            ts_ms = int(ts.timestamp() * 1000)
            snaps.append({
                "t": ts_ms,
                "uid": uid,
                "current": cur,
                "limit": lim,
            })
        return snaps

    def scan(self) -> Tuple[List[dict], Dict[str, int]]:
        """扫描所有 log 文件，返回 (账号列表, stats)。

        账号列表：每个 dict 含 uid / first_seen / last_seen / peak / latest / resets / snapshots。
        """
        t0 = time.perf_counter()
        all_snaps: List[dict] = []
        files_seen = 0
        reparsed = 0
        reused = 0

        if not os.path.isdir(self.root):
            return [], {"files": 0, "reparsed": 0, "reused": 0, "took_ms": 0}

        with self._lock:
            active = set()
            for dp, _, fns in os.walk(self.root):
                for fn in fns:
                    if not fn.endswith(".log"):
                        continue
                    fp = os.path.join(dp, fn)
                    active.add(fp)
                    files_seen += 1
                    try:
                        mtime = os.path.getmtime(fp)
                    except OSError:
                        continue
                    cached = self._cache.get(fp)
                    if cached and cached[0] == mtime:
                        all_snaps.extend(cached[1])
                        reused += 1
                        continue
                    snaps = self._parse_one_log(fp)
                    self._cache[fp] = (mtime, snaps)
                    all_snaps.extend(snaps)
                    reparsed += 1

            for stale in list(self._cache.keys()):
                if stale not in active:
                    del self._cache[stale]

        # 按时间排序，同时用"最近的 uid"填补空 uid
        all_snaps.sort(key=lambda x: x["t"])
        last_uid = None
        for s in all_snaps:
            if s["uid"]:
                last_uid = s["uid"]
            elif last_uid:
                s["uid"] = last_uid

        # 按 uid 分组、去重、算峰值
        by_uid: Dict[str, List[dict]] = defaultdict(list)
        for s in all_snaps:
            uid = s["uid"] or "(unknown)"
            by_uid[uid].append(s)

        accounts: List[dict] = []
        for uid, snaps in by_uid.items():
            snaps.sort(key=lambda x: x["t"])
            # 同秒同值去重
            dedup = []
            last_key = None
            for s in snaps:
                key = (s["t"] // 1000, round(s["current"], 2), s["limit"])
                if key != last_key:
                    dedup.append(s)
                    last_key = key

            # 数归零/重置次数
            resets = 0
            prev = None
            for s in dedup:
                if prev and s["current"] < prev["current"] - 30 \
                        and s["current"] < prev["current"] * 0.7:
                    resets += 1
                prev = s

            accounts.append({
                "uid": uid,
                "first_seen": dedup[0]["t"] if dedup else 0,
                "last_seen": dedup[-1]["t"] if dedup else 0,
                "peak": max((s["current"] for s in dedup), default=0.0),
                "latest": dedup[-1]["current"] if dedup else 0.0,
                "latest_limit": dedup[-1]["limit"] if dedup else 0,
                "resets": resets,
                "snapshots": dedup,  # 每条 {t, current, limit}
            })

        # 按峰值降序
        accounts.sort(key=lambda a: -a["peak"])

        return accounts, {
            "files": files_seen,
            "reparsed": reparsed,
            "reused": reused,
            "took_ms": int((time.perf_counter() - t0) * 1000),
        }


# ---------------------------------------------------------------------------
# HTTP 处理
# ---------------------------------------------------------------------------


_MIME = {
    ".html": "text/html; charset=utf-8",
    ".js":   "application/javascript; charset=utf-8",
    ".css":  "text/css; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".svg":  "image/svg+xml",
    ".ico":  "image/x-icon",
    ".map":  "application/json; charset=utf-8",
}


class DashboardHandler(BaseHTTPRequestHandler):
    """请求路由。server.cache / server.state_db 由主流程注入。"""

    # 关掉 stdout 请求日志，避免命令行刷屏；错误仍写 stderr。
    def log_message(self, fmt: str, *args: Any) -> None:  # noqa: N802
        return

    # ---- 工具方法 ----

    def _send_bytes(self, status: int, body: bytes, ctype: str,
                    extra_headers: Optional[Dict[str, str]] = None) -> None:
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        # 允许 devtools/前端跨端口调试
        self.send_header("Access-Control-Allow-Origin", "*")
        if extra_headers:
            for k, v in extra_headers.items():
                self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _send_json(self, obj: Any, status: int = 200) -> None:
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self._send_bytes(status, body, _MIME[".json"])

    def _send_text(self, status: int, msg: str) -> None:
        self._send_bytes(status, msg.encode("utf-8"), "text/plain; charset=utf-8")

    def _send_static(self, rel: str) -> None:
        """把 static/ 下的资源返回。rel 是相对路径（前端 URL 段），做基本越界防护。"""
        # 归一化 + 越界检查
        safe = os.path.normpath(rel).lstrip("/\\")
        if ".." in safe.split(os.sep):
            return self._send_text(400, "bad path")
        full = os.path.join(STATIC_DIR, safe)
        if not os.path.isfile(full):
            return self._send_text(404, f"not found: {safe}")
        ext = os.path.splitext(full)[1].lower()
        ctype = _MIME.get(ext, "application/octet-stream")
        try:
            with open(full, "rb") as f:
                data = f.read()
        except OSError as e:
            return self._send_text(500, f"read error: {e}")
        self._send_bytes(200, data, ctype)

    # ---- 路由 ----

    def do_GET(self) -> None:  # noqa: N802
        url = urlparse(self.path)
        path = url.path

        try:
            if path == "/" or path == "/index.html":
                return self._send_static("index.html")

            if path == "/api/health":
                return self._send_json({"status": "ok", "ts": int(time.time() * 1000)})

            if path == "/api/data":
                return self._handle_data()

            if path == "/api/export.csv":
                return self._handle_export_csv()

            # 静态资源：/app.js /style.css /favicon.ico 等
            if path.startswith("/") and "." in os.path.basename(path):
                return self._send_static(path[1:])

            return self._send_text(404, f"not found: {path}")
        except Exception as exc:
            print(f"[error] {path}: {exc}", file=sys.stderr)
            return self._send_text(500, f"internal error: {exc}")

    # ---- 具体 API ----

    def _handle_data(self) -> None:
        cache: TurnCache = self.server.cache  # type: ignore[attr-defined]
        state_db: Optional[str] = self.server.state_db  # type: ignore[attr-defined]
        v1_cache: Optional[V1SessionCache] = getattr(self.server, "v1_cache", None)
        quota_cache: Optional[QuotaHistoryCache] = getattr(self.server, "quota_cache", None)

        turns, stats = cache.scan()
        quota = load_quota(state_db)

        v1_sessions: List[dict] = []
        v1_stats: Dict[str, int] = {}
        if v1_cache is not None:
            v1_sessions, v1_stats = v1_cache.scan()

        accounts: List[dict] = []
        acc_stats: Dict[str, int] = {}
        if quota_cache is not None:
            accounts, acc_stats = quota_cache.scan()

        payload = {
            "quota": quota,
            "turns": turns,
            "v1_sessions": v1_sessions,
            "accounts": accounts,
            "server_ts": int(time.time() * 1000),
            "server_tz_offset_min": _local_tz_offset_min(),
            "scan": stats,
            "scan_v1": v1_stats,
            "scan_accounts": acc_stats,
        }
        self._send_json(payload)

    def _handle_export_csv(self) -> None:
        cache: TurnCache = self.server.cache  # type: ignore[attr-defined]
        turns, _ = cache.scan()
        buf = io.StringIO()
        w = csv.writer(buf)
        w.writerow([
            "ts_utc_ms", "ts_local", "workspace", "session_id",
            "agent_session_id", "execution_id",
            "credits", "elapsed_ms", "status", "tool_count",
            "model", "title", "tools",
        ])
        tz_offset = _local_tz_offset_min() * 60  # 秒
        for t in turns:
            local_dt = datetime.fromtimestamp(t["t"] / 1000 + tz_offset, tz=timezone.utc)
            w.writerow([
                t["t"],
                local_dt.strftime("%Y-%m-%d %H:%M:%S"),
                t["ws"],
                t["sid"],
                t["aid"],
                t["eid"],
                f"{t['c']:.6f}",
                t["e"],
                t["s"],
                len(t["tools"]),
                t["model"],
                t["title"],
                "|".join(t["tools"]),
            ])
        # Excel 打开中文 CSV 需要 BOM
        body = ("\ufeff" + buf.getvalue()).encode("utf-8")
        self._send_bytes(200, body, "text/csv; charset=utf-8", extra_headers={
            "Content-Disposition": 'attachment; filename="kiro_usage.csv"'
        })


def _local_tz_offset_min() -> int:
    """本地时区相对 UTC 的偏移，单位分钟。北京时间 = +480。"""
    now = datetime.now().astimezone()
    return int((now.utcoffset() or timezone.utc.utcoffset(now)).total_seconds() // 60)


# ---------------------------------------------------------------------------
# 服务器封装
# ---------------------------------------------------------------------------


class Server(ThreadingHTTPServer):
    """给 handler 用的自定义属性容器。"""

    cache: TurnCache
    state_db: Optional[str]
    v1_cache: Optional["V1SessionCache"] = None
    quota_cache: Optional["QuotaHistoryCache"] = None


def _find_free_port(host: str, prefer: int) -> int:
    """尝试 prefer，不行就往上找 20 个端口，都不行抛错。"""
    for port in [prefer] + [prefer + i for i in range(1, 21)]:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                s.bind((host, port))
                return port
        except OSError:
            continue
    raise RuntimeError(f"端口 {prefer}..{prefer + 20} 全部被占用")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Kiro 用量 Dashboard（本地 Web UI）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例：
  python kiro_dashboard.py                      # 默认 127.0.0.1:8765，自动开浏览器
  python kiro_dashboard.py --port 9000          # 换端口
  python kiro_dashboard.py --host 0.0.0.0       # 允许局域网访问（注意隐私）
  python kiro_dashboard.py --no-browser         # 不自动开浏览器
  python kiro_dashboard.py --auto-port          # 端口占用时自动找下一个可用端口
""",
    )
    p.add_argument("--host", default=os.environ.get("KIRO_DASHBOARD_HOST", "127.0.0.1"),
                   help="监听地址（默认 127.0.0.1，仅本机可访问）")
    p.add_argument("--port", type=int,
                   default=int(os.environ.get("KIRO_DASHBOARD_PORT", "8765")),
                   help="监听端口（默认 8765，可用环境变量 KIRO_DASHBOARD_PORT 覆盖）")
    p.add_argument("--auto-port", action="store_true",
                   help="端口被占用时自动尝试下一个可用端口")
    p.add_argument("--sessions-root",
                   default=os.path.join(os.path.expanduser("~"), ".kiro", "sessions"),
                   help="Kiro v2 sessions 根目录（含 messages.jsonl）")
    p.add_argument("--v1-sessions-root", default=None,
                   help="Kiro v1 sessions 根目录（默认自动定位到 workspace-sessions/）")
    p.add_argument("--logs-root", default=None,
                   help="Kiro 日志根目录（默认自动定位到 %%APPDATA%%/Kiro/logs）")
    p.add_argument("--state-db", default=None,
                   help="Kiro state.vscdb 路径（默认自动定位）")
    p.add_argument("--no-browser", action="store_true",
                   help="启动后不自动打开浏览器")
    p.add_argument("--open-path", default="/",
                   help="自动打开时的路径（默认 /）")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)

    if not os.path.isdir(STATIC_DIR):
        print(f"[fatal] 缺少静态资源目录: {STATIC_DIR}", file=sys.stderr)
        return 2

    # 端口选择
    port = args.port
    if args.auto_port:
        port = _find_free_port(args.host, args.port)

    # v2 sessions（.kiro/sessions 下的 messages.jsonl）
    cache = TurnCache(args.sessions_root)
    # v1 sessions（历史，跨 workspace）
    v1_root = args.v1_sessions_root or os.path.join(
        os.environ.get("APPDATA", ""), "Kiro", "User", "globalStorage",
        "kiro.kiroagent", "workspace-sessions",
    )
    v1_cache = V1SessionCache(v1_root)
    # 多账号 quota 时间序列（从 Kiro 运行日志）
    logs_root = args.logs_root or os.path.join(
        os.environ.get("APPDATA", ""), "Kiro", "logs",
    )
    quota_cache = QuotaHistoryCache(logs_root)

    # 首次预热扫描一次
    print("[info] 正在扫描 Kiro 会话数据 ...")
    turns, stats = cache.scan()
    print(f"[info] v2 sessions: {stats['files']} 个 messages.jsonl，"
          f"{stats['turns']} 个 turn，用时 {stats['took_ms']} ms")

    v1_sessions, v1_stats = v1_cache.scan()
    print(f"[info] v1 sessions: {v1_stats.get('files', 0)} 个 session，"
          f"用时 {v1_stats.get('took_ms', 0)} ms   root={v1_root}")

    accounts, acc_stats = quota_cache.scan()
    print(f"[info] quota 快照: {acc_stats.get('files', 0)} 个 log，"
          f"识别到 {len(accounts)} 个账号，用时 {acc_stats.get('took_ms', 0)} ms")

    server = Server((args.host, port), DashboardHandler)
    server.cache = cache
    server.state_db = args.state_db
    server.v1_cache = v1_cache
    server.quota_cache = quota_cache

    url = f"http://{args.host}:{port}{args.open_path}"
    print("=" * 60)
    print(f"  Kiro Dashboard 已启动: {url}")
    print(f"  sessions: {args.sessions_root}")
    print(f"  Ctrl+C 停止")
    print("=" * 60)

    if not args.no_browser:
        # 避免主线程被 webbrowser 拖住，异步开
        threading.Thread(target=lambda: (time.sleep(0.3), webbrowser.open(url)),
                         daemon=True).start()

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[info] 收到中断信号，正在关闭 ...")
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
