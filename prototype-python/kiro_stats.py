# -*- coding: utf-8 -*-
"""Kiro 用量统计工具。

从 Kiro 的本地会话数据里抽取每次 turn 的额度（Credits）和耗时（Elapsed），
按天 / 周 / 月 / session / workspace 聚合并展示。

数据源（全部本地文件，只读访问）：
- ~/.kiro/sessions/<sessionId>/<agentSessionId>/messages.jsonl
    每 turn 结束时会有一条 payload.type == "usage_summary" 的事件，字段包括：
      promptTurnSummaries[0].usage   → Credits（对应 UI 显示的 "Est. Credits Used"）
      elapsedTime                    → 耗时（毫秒，对应 "Elapsed time"）
      status                         → success / aborted / failed
      executionId                    → turn 唯一 id
- ~/.kiro/sessions/<sessionId>/<agentSessionId>/session.json
    workspacePaths / title / modelId / createdAt
- %APPDATA%/Kiro/User/globalStorage/state.vscdb
    ItemTable.kiro.kiroAgent → 本月配额、订阅信息

用法示例（cmd / PowerShell 都行）：
    python kiro_stats.py                              # 默认：本月配额概览 + 最近 7 天日报
    python kiro_stats.py --by session --top 20        # 按 session 聚合，显示消耗最大的前 20
    python kiro_stats.py --by workspace               # 按项目聚合
    python kiro_stats.py --by month                   # 按月聚合
    python kiro_stats.py --detail --top 30            # 展开每一次 turn 明细
    python kiro_stats.py --csv out.csv                # 把所有 turn 导出为 CSV
    python kiro_stats.py --from 2026-06-01 --to 2026-06-30
"""

from __future__ import annotations

import argparse
import csv
import glob
import json
import os
import re
import sqlite3
import sys
from dataclasses import dataclass, asdict
from datetime import datetime, timedelta, timezone
from typing import Dict, Iterable, List, Optional, Tuple

# ---------------------------------------------------------------------------
# 数据模型
# ---------------------------------------------------------------------------


@dataclass
class Turn:
    """一次 turn（一次用户提问 → 一次 Kiro 响应结束）的用量记录。"""

    session_id: str  # 会话文件夹名（外层）
    agent_session_id: str  # 内层 agent session id
    execution_id: str  # turn 唯一 id
    ts_utc: datetime  # usage_summary 事件的时间戳（UTC）
    ts_local: datetime  # 转为本地时区的时间戳
    credits: float  # 消耗的 credits，aborted 且无 usage 记录时为 0
    elapsed_ms: int  # 耗时毫秒
    status: str  # success / aborted / failed
    tool_count: int  # 这一 turn 调用工具的次数
    workspace: str  # workspacePaths[0] 的最后一段目录名，多项目区分用
    title: str  # session.json 里的 title
    model: str  # modelId


# ---------------------------------------------------------------------------
# 加载
# ---------------------------------------------------------------------------


def _parse_ts(s: str) -> datetime:
    """把 '2026-07-01T02:17:07.966Z' 解析成 UTC datetime。"""
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    return datetime.fromisoformat(s)


def _guess_workspace(session_meta: dict) -> str:
    """从 session.json 的 workspacePaths 推断项目名。"""
    paths = session_meta.get("workspacePaths") or []
    if not paths:
        return "(no-workspace)"
    p = paths[0].rstrip("/\\")
    return os.path.basename(p) or p


def load_turns(sessions_root: str, local_tz: timezone) -> List[Turn]:
    """扫描 sessions 目录，抽出所有 usage_summary 事件，配合 session.json 组装成 Turn 列表。"""
    turns: List[Turn] = []

    # 匹配两层结构：<sessionId>/<agentSessionId>/messages.jsonl
    pattern = os.path.join(sessions_root, "*", "*", "messages.jsonl")
    for msg_path in glob.iglob(pattern):
        agent_dir = os.path.dirname(msg_path)
        session_dir = os.path.dirname(agent_dir)
        session_id = os.path.basename(session_dir)
        agent_session_id = os.path.basename(agent_dir)

        # 读 session.json，拿 workspace/title/model
        meta_path = os.path.join(agent_dir, "session.json")
        title = ""
        model = ""
        workspace = "(unknown)"
        if os.path.exists(meta_path):
            try:
                with open(meta_path, "r", encoding="utf-8") as f:
                    meta = json.load(f)
                title = meta.get("title", "") or ""
                model = meta.get("modelId", "") or ""
                workspace = _guess_workspace(meta)
            except Exception:
                pass

        # 遍历 messages.jsonl，只挑 usage_summary
        try:
            with open(msg_path, "r", encoding="utf-8") as f:
                # 先做子串预筛，避免每行都 json.loads
                for line in f:
                    if '"type":"usage_summary"' not in line:
                        continue
                    try:
                        ev = json.loads(line)
                    except Exception:
                        continue
                    payload = ev.get("payload", {})
                    if payload.get("type") != "usage_summary":
                        continue

                    ts_utc = _parse_ts(ev["timestamp"])
                    ts_local = ts_utc.astimezone(local_tz)

                    # credits 可能为空（aborted 且没算钱的情况）
                    summaries = payload.get("promptTurnSummaries") or []
                    credits = 0.0
                    tool_count = 0
                    if summaries:
                        credits = float(summaries[0].get("usage") or 0.0)
                        tool_count = len(summaries[0].get("usedTools") or [])

                    turns.append(Turn(
                        session_id=session_id,
                        agent_session_id=agent_session_id,
                        execution_id=payload.get("executionId", ""),
                        ts_utc=ts_utc,
                        ts_local=ts_local,
                        credits=credits,
                        elapsed_ms=int(payload.get("elapsedTime") or 0),
                        status=payload.get("status", "unknown"),
                        tool_count=tool_count,
                        workspace=workspace,
                        title=title,
                        model=model,
                    ))
        except Exception as e:
            print(f"[warn] 读取 {msg_path} 失败: {e}", file=sys.stderr)

    turns.sort(key=lambda t: t.ts_utc)
    return turns


def load_quota(state_db_path: Optional[str]) -> Optional[dict]:
    """从 state.vscdb 里读出本月配额进度。文件锁定或字段缺失时返回 None。"""
    if state_db_path is None:
        state_db_path = os.path.join(
            os.environ.get("APPDATA", ""), "Kiro", "User",
            "globalStorage", "state.vscdb",
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

    # 优先读通知状态里的 usageBreakdowns（数值就是 UI 上显示的月度进度）
    ns = j.get("kiro.resourceNotifications.usageState") or {}
    breakdowns = ns.get("usageBreakdowns") or []
    if breakdowns:
        b = breakdowns[0]
        return {
            "source": "resourceNotifications",
            "current_usage": b.get("currentUsage"),
            "usage_limit": b.get("usageLimit"),
            "percentage": b.get("percentageUsed"),
            "overage_cap": b.get("overageCap"),
            "overage_rate": b.get("overageRate"),
            "reset_date": b.get("resetDate"),
            "subscription": (j.get("subscriptionInfo") or {}).get("subscriptionTitle"),
        }

    # 兜底：读 usageBreakdownList（订阅初始快照，可能是 0）
    ubl = j.get("usageBreakdownList") or []
    if ubl:
        b = ubl[0]
        return {
            "source": "usageBreakdownList",
            "current_usage": b.get("currentUsage"),
            "usage_limit": b.get("usageLimit"),
            "percentage": None,
            "overage_cap": b.get("overageCap"),
            "overage_rate": b.get("overageRate"),
            "reset_date": None,
            "subscription": (j.get("subscriptionInfo") or {}).get("subscriptionTitle"),
        }
    return None


# ---------------------------------------------------------------------------
# 聚合
# ---------------------------------------------------------------------------

BY_CHOICES = ("day", "week", "month", "session", "workspace", "model")


def _bucket_key(t: Turn, by: str) -> str:
    if by == "day":
        return t.ts_local.strftime("%Y-%m-%d")
    if by == "week":
        # ISO 周：2026-W27
        y, w, _ = t.ts_local.isocalendar()
        return f"{y}-W{w:02d}"
    if by == "month":
        return t.ts_local.strftime("%Y-%m")
    if by == "session":
        # 用 agent_session_id + 标题，方便识别
        short = t.agent_session_id[:8]
        title = t.title[:30] if t.title else "(untitled)"
        return f"{short}  {title}"
    if by == "workspace":
        return t.workspace or "(no-workspace)"
    if by == "model":
        return t.model or "(unknown-model)"
    raise ValueError(by)


@dataclass
class Bucket:
    key: str
    turns: int
    turns_success: int
    credits: float
    elapsed_ms: int
    first_ts: datetime
    last_ts: datetime


def aggregate(turns: Iterable[Turn], by: str) -> List[Bucket]:
    """按指定维度聚合 turns。"""
    buckets: Dict[str, Bucket] = {}
    for t in turns:
        k = _bucket_key(t, by)
        b = buckets.get(k)
        if b is None:
            b = Bucket(
                key=k, turns=0, turns_success=0, credits=0.0,
                elapsed_ms=0, first_ts=t.ts_local, last_ts=t.ts_local,
            )
            buckets[k] = b
        b.turns += 1
        if t.status == "success":
            b.turns_success += 1
        b.credits += t.credits
        b.elapsed_ms += t.elapsed_ms
        if t.ts_local < b.first_ts:
            b.first_ts = t.ts_local
        if t.ts_local > b.last_ts:
            b.last_ts = t.ts_local

    # 时间维度按 key 升序，其它维度按 credits 降序（更符合"消耗大头在前"的直觉）
    if by in ("day", "week", "month"):
        return sorted(buckets.values(), key=lambda b: b.key)
    return sorted(buckets.values(), key=lambda b: b.credits, reverse=True)


# ---------------------------------------------------------------------------
# 展示
# ---------------------------------------------------------------------------


def fmt_duration(ms: int) -> str:
    s = int(ms // 1000)
    h, rem = divmod(s, 3600)
    m, s = divmod(rem, 60)
    if h:
        return f"{h}h{m:02d}m"
    if m:
        return f"{m}m{s:02d}s"
    return f"{s}s"


def _render_table(headers: List[str], rows: List[List[str]], aligns: List[str]) -> str:
    """一个简单的等宽表格渲染器，避免依赖第三方库。aligns[i] 是 'l'/'r'。"""
    if not rows:
        return "  (无数据)"
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    def _fmt_row(cells: List[str]) -> str:
        parts = []
        for i, c in enumerate(cells):
            if aligns[i] == "r":
                parts.append(c.rjust(widths[i]))
            else:
                parts.append(c.ljust(widths[i]))
        return "  " + "  ".join(parts)

    sep = "  " + "  ".join("-" * w for w in widths)
    out = [_fmt_row(headers), sep]
    for row in rows:
        out.append(_fmt_row(row))
    return "\n".join(out)


def render_overview(turns: List[Turn], quota: Optional[dict]) -> str:
    """总览：本月配额 + 全量累计 + 时间跨度。"""
    lines = ["=" * 60, "Kiro 用量总览", "=" * 60]

    if quota:
        cur = quota.get("current_usage")
        lim = quota.get("usage_limit")
        pct = quota.get("percentage")
        sub = quota.get("subscription") or "(未知订阅)"
        reset = quota.get("reset_date") or "(未知重置日)"
        rate = quota.get("overage_rate")
        cap = quota.get("overage_cap")
        lines.append(f"订阅: {sub}")
        if cur is not None and lim is not None:
            pct_s = f"{pct:.2f}%" if pct is not None else "N/A"
            lines.append(f"本月已用: {cur:.2f} / {lim} credits  ({pct_s})")
        if cap is not None:
            lines.append(
                f"超额上限: {cap} credits    超额单价: {rate} USD/credit"
            )
        lines.append(f"下次重置: {reset}")
        if quota.get("source") == "usageBreakdownList":
            lines.append("(提示: 数据源为初始订阅快照，可能不反映真实使用；请打开 Kiro 让它同步刷新)")
    else:
        lines.append("本月配额信息: 未读取到（state.vscdb 缺失或被锁定）")

    lines.append("-" * 60)

    if not turns:
        lines.append("本地没有找到任何 usage_summary 记录。")
        return "\n".join(lines)

    total_credits = sum(t.credits for t in turns)
    total_ms = sum(t.elapsed_ms for t in turns)
    first = turns[0].ts_local
    last = turns[-1].ts_local
    span_days = (last.date() - first.date()).days + 1

    # 有 credits 记录的 turn 才计入 credits 平均，避免被 aborted 的 0 拉低
    priced = [t for t in turns if t.credits > 0]

    lines.append(f"数据范围: {first:%Y-%m-%d %H:%M} ~ {last:%Y-%m-%d %H:%M}  ({span_days} 天)")
    lines.append(f"累计 turn: {len(turns)}  (含 credits 的 turn: {len(priced)})")
    lines.append(f"累计 credits: {total_credits:.2f}")
    lines.append(f"累计耗时:  {fmt_duration(total_ms)}")
    if priced:
        avg_c = total_credits / len(priced)
        avg_ms = sum(t.elapsed_ms for t in priced) / len(priced)
        lines.append(f"平均每 turn: {avg_c:.2f} credits, {fmt_duration(int(avg_ms))}")
    if span_days > 0:
        lines.append(f"日均 credits: {total_credits / span_days:.2f}")

    return "\n".join(lines)


def render_buckets(buckets: List[Bucket], by: str, top: Optional[int]) -> str:
    """按指定维度渲染聚合结果。"""
    lines = [f"\n=== 按 {by} 聚合 ==="]

    key_header = {
        "day": "日期",
        "week": "周",
        "month": "月",
        "session": "Session (id 前缀 + title)",
        "workspace": "Workspace",
        "model": "Model",
    }[by]

    headers = [key_header, "turns", "credits", "耗时", "首次", "最后"]
    aligns = ["l", "r", "r", "r", "l", "l"]

    show = buckets if top is None else buckets[:top]
    rows: List[List[str]] = []
    for b in show:
        rows.append([
            b.key,
            f"{b.turns} ({b.turns_success}ok)",
            f"{b.credits:.2f}",
            fmt_duration(b.elapsed_ms),
            b.first_ts.strftime("%Y-%m-%d %H:%M"),
            b.last_ts.strftime("%Y-%m-%d %H:%M"),
        ])

    lines.append(_render_table(headers, rows, aligns))
    if top is not None and len(buckets) > top:
        lines.append(f"  (仅显示前 {top} 行，共 {len(buckets)} 行；用 --top 调整或 --csv 导出全部)")
    return "\n".join(lines)


def render_detail(turns: List[Turn], top: Optional[int]) -> str:
    """展开每 turn 明细。按时间倒序（最近的在前），方便审最新。"""
    lines = ["\n=== 每 turn 明细（按时间倒序）==="]
    headers = ["时间", "workspace", "credits", "耗时", "status", "tools", "title"]
    aligns = ["l", "l", "r", "r", "l", "r", "l"]

    ordered = sorted(turns, key=lambda t: t.ts_local, reverse=True)
    show = ordered if top is None else ordered[:top]
    rows: List[List[str]] = []
    for t in show:
        title = (t.title or "")[:40]
        rows.append([
            t.ts_local.strftime("%m-%d %H:%M"),
            t.workspace[:15],
            f"{t.credits:.2f}",
            fmt_duration(t.elapsed_ms),
            t.status,
            str(t.tool_count),
            title,
        ])
    lines.append(_render_table(headers, rows, aligns))
    if top is not None and len(ordered) > top:
        lines.append(f"  (仅显示最近 {top} 条，共 {len(ordered)} 条；用 --top 调整或 --csv 导出全部)")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CSV 导出
# ---------------------------------------------------------------------------


def write_csv(turns: List[Turn], path: str) -> None:
    """把所有 turn 导出为 CSV（明细粒度，方便 Excel 二次分析）。"""
    fields = [
        "ts_local", "ts_utc", "workspace", "session_id", "agent_session_id",
        "execution_id", "credits", "elapsed_ms", "elapsed_human",
        "status", "tool_count", "model", "title",
    ]
    with open(path, "w", encoding="utf-8-sig", newline="") as f:
        w = csv.writer(f)
        w.writerow(fields)
        for t in turns:
            w.writerow([
                t.ts_local.strftime("%Y-%m-%d %H:%M:%S"),
                t.ts_utc.strftime("%Y-%m-%d %H:%M:%S"),
                t.workspace,
                t.session_id,
                t.agent_session_id,
                t.execution_id,
                f"{t.credits:.6f}",
                t.elapsed_ms,
                fmt_duration(t.elapsed_ms),
                t.status,
                t.tool_count,
                t.model,
                t.title,
            ])


# ---------------------------------------------------------------------------
# 过滤
# ---------------------------------------------------------------------------


def _parse_date(s: str) -> datetime:
    """把 '2026-07-01' 或 '2026-07-01 12:00' 解析为 naive datetime。"""
    s = s.strip()
    for fmt in ("%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d"):
        try:
            return datetime.strptime(s, fmt)
        except ValueError:
            continue
    raise argparse.ArgumentTypeError(f"日期格式无法识别: {s}")


def filter_turns(
    turns: List[Turn],
    dt_from: Optional[datetime],
    dt_to: Optional[datetime],
    workspace: Optional[str],
    status: Optional[str],
) -> List[Turn]:
    def ok(t: Turn) -> bool:
        # 用本地时间比较，与用户直觉一致
        d = t.ts_local.replace(tzinfo=None)
        if dt_from and d < dt_from:
            return False
        if dt_to and d > dt_to:
            return False
        if workspace and workspace.lower() not in t.workspace.lower():
            return False
        if status and t.status != status:
            return False
        return True
    return [t for t in turns if ok(t)]


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="统计 Kiro 每次运行的额度（Credits）与耗时（Elapsed）。",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--sessions-root",
        default=os.path.join(os.path.expanduser("~"), ".kiro", "sessions"),
        help="Kiro sessions 根目录（默认 ~/.kiro/sessions）",
    )
    parser.add_argument(
        "--state-db", default=None,
        help="Kiro state.vscdb 路径（默认自动定位到 %%APPDATA%%/Kiro/User/globalStorage/state.vscdb）",
    )
    parser.add_argument(
        "--by", choices=BY_CHOICES, default=None,
        help="聚合维度。不传时默认输出总览 + 最近 7 天日报",
    )
    parser.add_argument("--from", dest="dt_from", type=_parse_date, default=None,
                        help="起始时间（含），格式 YYYY-MM-DD 或 YYYY-MM-DD HH:MM")
    parser.add_argument("--to", dest="dt_to", type=_parse_date, default=None,
                        help="结束时间（含）")
    parser.add_argument("--workspace", default=None,
                        help="按 workspace 名过滤（子串匹配）")
    parser.add_argument("--status", default=None,
                        choices=["success", "aborted", "failed"],
                        help="按 turn 状态过滤")
    parser.add_argument("--detail", action="store_true",
                        help="展开每 turn 明细（按时间倒序）")
    parser.add_argument("--top", type=int, default=None,
                        help="最多展示多少行，默认全部")
    parser.add_argument("--csv", default=None,
                        help="把所有 turn 导出为 CSV 到指定路径")
    args = parser.parse_args(argv)

    # 本地时区：跟系统一致
    local_tz = datetime.now().astimezone().tzinfo or timezone.utc

    turns_all = load_turns(args.sessions_root, local_tz)
    quota = load_quota(args.state_db)

    turns = filter_turns(turns_all, args.dt_from, args.dt_to,
                         args.workspace, args.status)

    # CSV 导出用过滤后的结果
    if args.csv:
        write_csv(turns, args.csv)
        print(f"[csv] 已导出 {len(turns)} 条 turn 到 {args.csv}")

    print(render_overview(turns, quota))

    if args.by:
        print(render_buckets(aggregate(turns, args.by), args.by, args.top))
    else:
        # 默认视图：最近 7 天日报
        if turns:
            cutoff = turns[-1].ts_local - timedelta(days=6)
            recent = [t for t in turns if t.ts_local >= cutoff]
            print(render_buckets(aggregate(recent, "day"), "day", None))
            print("\n提示: 更多用法")
            print("  按周/月:      python kiro_stats.py --by week    /  --by month")
            print("  按项目/模型:  python kiro_stats.py --by workspace  / --by model")
            print("  按 session:   python kiro_stats.py --by session --top 20")
            print("  每次明细:     python kiro_stats.py --detail --top 30")
            print("  导 CSV:       python kiro_stats.py --csv kiro_usage.csv")

    if args.detail:
        print(render_detail(turns, args.top))

    return 0


if __name__ == "__main__":
    sys.exit(main())
