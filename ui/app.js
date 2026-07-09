/* ============================================================
 * Kiro Usage Dashboard —— 前端主逻辑
 *
 * 分模块：
 *   1. 状态与常量
 *   2. 工具函数（时间、格式化、CSS 变量读取）
 *   3. 数据获取与实时刷新
 *   4. 聚合（按粒度分桶、时间范围过滤）
 *   5. 各图表渲染（KPI、趋势、热力图、Treemap、环形、Top Sessions）
 *   6. 明细表（排序、搜索、分页、导出）
 *   7. 主题、交互绑定
 *   8. 启动
 * ============================================================ */

'use strict';

// ============================================================
// 1. 状态与常量
// ============================================================

const REFRESH_INTERVAL_MS = 15000;   // 后台静默刷新间隔
const PAGE_SIZE = 50;                // 明细表每页条数

const state = {
  // 后端返回
  turns: [],                    // v2 usage_summary，含 credits（v0.3+ 来自历史库合并）
  v1Sessions: [],               // v1 sessions 元数据（无 credits）
  accounts: [],                 // 多账号 quota 时间序列
  quota: null,
  serverTzOffsetMin: -new Date().getTimezoneOffset(),  // 兜底：用浏览器本地
  lastServerTs: 0,
  scanStats: null,
  historyStats: null,           // v0.3+: 本地持久化历史库统计
  lastFetchOk: 0,

  // 导航
  view: 'overview',       // overview / trends / tools / accounts / sessions
  detailTab: 'v2',        // v2 / v1  (明细视图内的 tab)

  // UI 状态
  gran: 'day',            // hour / day / week / month
  range: '30d',           // today / week / month / 30d / all
  toolMetric: 'credits',  // credits / count
  showTurns: true,        // 主图叠加 turn 数
  showElapsed: false,     // 主图叠加耗时

  // 明细表 (v2)
  detailSearch: '',
  detailStatus: '',
  detailWorkspace: '',
  detailSort: { field: 't', dir: 'desc' },
  detailPage: 1,

  // v1 sessions 表
  v1Search: '',
  v1WorkspaceFilter: '',
  v1Sort: { field: 't', dir: 'desc' },
  v1Page: 1,

  // 点击图表联动过滤
  clickFilter: null,      // {kind: 'timeSlot'|'workspace'|'tool'|'session', start, end, label, key}

  // 图表实例
  charts: {
    trend: null, heatmap: null, tools: null, workspace: null, accounts: null,
  },
};

// ECharts 实例句柄
function chartOf(el, name) {
  // echarts 异步加载中或容器不存在时返回 null，调用方跳过绘图
  if (typeof echarts === 'undefined' || !el) return null;
  if (!state.charts[name]) {
    state.charts[name] = echarts.init(el, null, { renderer: 'canvas' });
  }
  return state.charts[name];
}

// ============================================================
// 2. 工具函数
// ============================================================

// 读取 CSS 变量的运行时值（供 ECharts 用，保证主题一致）
function css(varName) {
  return getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
}

// 服务端返回的时间戳全是 UTC ms，把它加上时区偏移当作"本地墙钟时间"
function toLocalDate(tsMs) {
  return new Date(tsMs + state.serverTzOffsetMin * 60000);
}

function fmt2(n) { return String(n).padStart(2, '0'); }

// 格式化：credits 数字，>=100 保留 1 位，否则 2 位
function fmtCredits(c) {
  if (c == null || isNaN(c)) return '-';
  if (c >= 1000) return c.toFixed(0);
  if (c >= 100) return c.toFixed(1);
  return c.toFixed(2);
}

// 毫秒 → 人类可读时长
function fmtDuration(ms) {
  if (!ms || ms < 0) return '0s';
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h${fmt2(m)}m`;
  if (m > 0) return `${m}m${fmt2(sec)}s`;
  return `${sec}s`;
}

// "多久前"
function fmtAgo(tsMs) {
  const diff = Math.floor((Date.now() - tsMs) / 1000);
  if (diff < 5) return '刚刚';
  if (diff < 60) return `${diff}s 前`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m 前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h 前`;
  return `${Math.floor(diff / 86400)}d 前`;
}

// 本地时间格式化
function fmtLocalDT(tsMs) {
  const d = toLocalDate(tsMs);
  return `${d.getUTCFullYear()}-${fmt2(d.getUTCMonth() + 1)}-${fmt2(d.getUTCDate())} ${fmt2(d.getUTCHours())}:${fmt2(d.getUTCMinutes())}`;
}
function fmtLocalDate(tsMs) {
  const d = toLocalDate(tsMs);
  return `${d.getUTCFullYear()}-${fmt2(d.getUTCMonth() + 1)}-${fmt2(d.getUTCDate())}`;
}

// 一个 turn 的本地"分桶键"（视 gran 而定）
function bucketKey(tsMs, gran) {
  const d = toLocalDate(tsMs);
  const y = d.getUTCFullYear(), mo = d.getUTCMonth() + 1, da = d.getUTCDate();
  if (gran === 'hour') return `${y}-${fmt2(mo)}-${fmt2(da)} ${fmt2(d.getUTCHours())}:00`;
  if (gran === 'day')  return `${y}-${fmt2(mo)}-${fmt2(da)}`;
  if (gran === 'month') return `${y}-${fmt2(mo)}`;
  if (gran === 'week') {
    // ISO 周：以周一为一周开始。计算这一天所在周的周一日期作为 key
    const dow = d.getUTCDay() || 7; // 周一=1 ... 周日=7
    const monday = new Date(d.getTime() - (dow - 1) * 86400000);
    return `${monday.getUTCFullYear()}-${fmt2(monday.getUTCMonth() + 1)}-${fmt2(monday.getUTCDate())}`;
  }
  return '';
}

// 生成 [start, end] 之间的所有桶键（含空桶），保证图表 x 轴连续
function generateBucketKeys(startMs, endMs, gran) {
  const keys = [];
  const s = toLocalDate(startMs);
  const e = toLocalDate(endMs);

  if (gran === 'hour') {
    let cur = new Date(Date.UTC(s.getUTCFullYear(), s.getUTCMonth(), s.getUTCDate(), s.getUTCHours()));
    const end = new Date(Date.UTC(e.getUTCFullYear(), e.getUTCMonth(), e.getUTCDate(), e.getUTCHours()));
    while (cur <= end) {
      keys.push(`${cur.getUTCFullYear()}-${fmt2(cur.getUTCMonth() + 1)}-${fmt2(cur.getUTCDate())} ${fmt2(cur.getUTCHours())}:00`);
      cur = new Date(cur.getTime() + 3600000);
    }
  } else if (gran === 'day') {
    let cur = new Date(Date.UTC(s.getUTCFullYear(), s.getUTCMonth(), s.getUTCDate()));
    const end = new Date(Date.UTC(e.getUTCFullYear(), e.getUTCMonth(), e.getUTCDate()));
    while (cur <= end) {
      keys.push(`${cur.getUTCFullYear()}-${fmt2(cur.getUTCMonth() + 1)}-${fmt2(cur.getUTCDate())}`);
      cur = new Date(cur.getTime() + 86400000);
    }
  } else if (gran === 'week') {
    const sDow = s.getUTCDay() || 7;
    let cur = new Date(Date.UTC(s.getUTCFullYear(), s.getUTCMonth(), s.getUTCDate() - (sDow - 1)));
    const eDow = e.getUTCDay() || 7;
    const end = new Date(Date.UTC(e.getUTCFullYear(), e.getUTCMonth(), e.getUTCDate() - (eDow - 1)));
    while (cur <= end) {
      keys.push(`${cur.getUTCFullYear()}-${fmt2(cur.getUTCMonth() + 1)}-${fmt2(cur.getUTCDate())}`);
      cur = new Date(cur.getTime() + 7 * 86400000);
    }
  } else if (gran === 'month') {
    let y = s.getUTCFullYear(), m = s.getUTCMonth();
    const ey = e.getUTCFullYear(), em = e.getUTCMonth();
    while (y < ey || (y === ey && m <= em)) {
      keys.push(`${y}-${fmt2(m + 1)}`);
      m++;
      if (m > 11) { m = 0; y++; }
    }
  }
  return keys;
}

// 计算当前时间范围 [start, end]（毫秒时间戳，UTC 但基于本地日历）
function computeRange() {
  const nowLocal = toLocalDate(Date.now());
  const yy = nowLocal.getUTCFullYear(), mm = nowLocal.getUTCMonth(), dd = nowLocal.getUTCDate();
  const startOfLocalDay = Date.UTC(yy, mm, dd) - state.serverTzOffsetMin * 60000;
  const now = Date.now();

  switch (state.range) {
    case 'today':
      return { start: startOfLocalDay, end: now };
    case 'week': {
      const dow = nowLocal.getUTCDay() || 7;
      return { start: startOfLocalDay - (dow - 1) * 86400000, end: now };
    }
    case 'month':
      return { start: Date.UTC(yy, mm, 1) - state.serverTzOffsetMin * 60000, end: now };
    case '30d':
      return { start: now - 30 * 86400000, end: now };
    case 'all':
    default: {
      if (state.turns.length === 0) return { start: now - 86400000, end: now };
      return { start: state.turns[0].t, end: now };
    }
  }
}

// ============================================================
// 3. 数据获取与实时刷新
// ============================================================

// 从 Rust 后端拿数据。Tauri 环境走 IPC；如果检测到不在 Tauri 里
// （比如直接用浏览器打开 index.html 调试），回退到 HTTP fetch，
// 这样 prototype-python 里的静态服务器也能复用同一份前端。
async function invokeGetData() {
  // Tauri v2 常规路径：需要 tauri.conf.json 里 app.withGlobalTauri: true
  const tauri = window.__TAURI__;
  if (tauri && tauri.core && typeof tauri.core.invoke === 'function') {
    return await tauri.core.invoke('get_data');
  }
  // 兜底：Tauri v2 内部 API，即使 withGlobalTauri 没打开也可用
  const internals = window.__TAURI_INTERNALS__;
  if (internals && typeof internals.invoke === 'function') {
    return await internals.invoke('get_data');
  }
  // 最后兜底：Python 版 HTTP 后端（浏览器里直接开 index.html 调试用）
  const resp = await fetch('/api/data', { cache: 'no-store' });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  return await resp.json();
}

// v0.3+: 清除本地历史库。仅在 Tauri 环境下可用（Python 版没有持久化）。
async function invokeClearHistory() {
  const tauri = window.__TAURI__;
  if (tauri && tauri.core && typeof tauri.core.invoke === 'function') {
    return await tauri.core.invoke('clear_history');
  }
  const internals = window.__TAURI_INTERNALS__;
  if (internals && typeof internals.invoke === 'function') {
    return await internals.invoke('clear_history');
  }
  throw new Error('清除历史仅在 Tauri exe 环境下可用（Python 原型没有持久化历史库）。');
}

async function fetchData(silent = false) {
  try {
    const j = await invokeGetData();
    const prevCount = state.turns.length;
    state.turns = (j.turns || []).sort((a, b) => a.t - b.t);
    state.v1Sessions = (j.v1_sessions || []).sort((a, b) => a.t - b.t);
    state.accounts = j.accounts || [];
    state.quota = j.quota || null;
    state.lastServerTs = j.server_ts || Date.now();
    state.serverTzOffsetMin = j.server_tz_offset_min ?? state.serverTzOffsetMin;
    state.scanStats = j.scan || null;
    state.scanV1Stats = j.scan_v1 || null;
    state.scanAccountsStats = j.scan_accounts || null;
    state.historyStats = j.history_stats || null;
    state.lastFetchOk = Date.now();

    setLiveStatus('live');

    // 首次或 turn 数量变化 → 全量重绘；仅 mtime 变时依然重绘（可能是 credits 更新）
    const changed = !silent || prevCount !== state.turns.length ||
                    (state.scanStats && state.scanStats.reparsed > 0);
    if (changed) render();
    else updateFooter();

    // v0.4.2: 首次数据到手, 隐藏启动遮罩
    hideSplash();
  } catch (e) {
    console.error('[fetch]', e);
    setLiveStatus('error');
  }
}

let refreshTimer = null;
function startAutoRefresh() {
  stopAutoRefresh();
  refreshTimer = setInterval(() => fetchData(true), REFRESH_INTERVAL_MS);
}
function stopAutoRefresh() {
  if (refreshTimer) clearInterval(refreshTimer);
  refreshTimer = null;
}

// 每秒更新一次 "刚刚 / xx 秒前" 文本
setInterval(() => {
  const el = document.getElementById('live-text');
  if (!state.lastFetchOk) return;
  el.textContent = fmtAgo(state.lastFetchOk);
  // 如果超过 3 倍刷新间隔没成功，视为陈旧
  const stale = Date.now() - state.lastFetchOk > REFRESH_INTERVAL_MS * 3;
  const indicator = document.getElementById('live-indicator');
  if (stale && !indicator.classList.contains('error')) {
    indicator.classList.add('stale');
  } else {
    indicator.classList.remove('stale');
  }
}, 1000);

function setLiveStatus(status) {
  const el = document.getElementById('live-indicator');
  el.classList.remove('stale', 'error');
  if (status === 'error') el.classList.add('error');
}

// ============================================================
// 4. 过滤与聚合
// ============================================================

// 应用时间范围 + 联动过滤，返回过滤后的 turn 列表
function filteredTurns() {
  const { start, end } = computeRange();
  let arr = state.turns.filter(t => t.t >= start && t.t <= end);

  const cf = state.clickFilter;
  if (cf) {
    if (cf.kind === 'timeSlot') {
      arr = arr.filter(t => t.t >= cf.start && t.t < cf.end);
    } else if (cf.kind === 'workspace') {
      arr = arr.filter(t => t.ws === cf.key);
    } else if (cf.kind === 'tool') {
      arr = arr.filter(t => (t.tools || []).includes(cf.key));
    } else if (cf.kind === 'session') {
      arr = arr.filter(t => t.aid === cf.key);
    }
  }
  return arr;
}

// 按 gran 聚合
function aggregateByGran(turns, gran, range) {
  const map = new Map();
  for (const t of turns) {
    const k = bucketKey(t.t, gran);
    let b = map.get(k);
    if (!b) {
      b = { key: k, credits: 0, turns: 0, turnsSuccess: 0, elapsed: 0, ts: t.t };
      map.set(k, b);
    }
    b.credits += t.c;
    b.turns += 1;
    if (t.s === 'success') b.turnsSuccess += 1;
    b.elapsed += t.e;
    if (t.t < b.ts) b.ts = t.t;
  }

  // 生成完整的桶序列（补空桶），限制条数上限
  let keys = generateBucketKeys(range.start, range.end, gran);
  const MAX_BARS = { hour: 168, day: 180, week: 104, month: 60 }[gran] || 200;
  if (keys.length > MAX_BARS) keys = keys.slice(keys.length - MAX_BARS);

  return keys.map(k => map.get(k) || { key: k, credits: 0, turns: 0, turnsSuccess: 0, elapsed: 0 });
}

// ============================================================
// 5. 图表渲染
// ============================================================

function renderKPI(turns) {
  const sumC = turns.reduce((s, t) => s + t.c, 0);
  const sumE = turns.reduce((s, t) => s + t.e, 0);
  const priced = turns.filter(t => t.c > 0);

  document.getElementById('kpi-est').textContent = fmtCredits(sumC);
  document.getElementById('kpi-turns').textContent = String(turns.length);
  document.getElementById('kpi-turns-hint').textContent = `含计费 turn: ${priced.length}`;
  document.getElementById('kpi-elapsed').textContent = fmtDuration(sumE);
  document.getElementById('kpi-elapsed-hint').textContent =
    priced.length ? `平均 ${fmtDuration(Math.round(sumE / priced.length))} / turn` : '-';

  // 跨账号计费峰值和
  const accs = state.accounts || [];
  const crossPeak = accs.reduce((s, a) => s + (a.peak || 0), 0);
  document.getElementById('kpi-cross-peak').textContent = fmtCredits(crossPeak);
  document.getElementById('kpi-cross-peak-hint').textContent =
    accs.length ? `${accs.length} 个账号` : '暂无账号数据';

  // 所有 Session (v1 + v2)
  const v1Count = (state.v1Sessions || []).length;
  const v2SessSet = new Set();
  for (const t of state.turns) if (t.aid) v2SessSet.add(t.aid);
  const totalSess = v1Count + v2SessSet.size;
  document.getElementById('kpi-all-sessions').textContent = String(totalSess);
  document.getElementById('kpi-all-sessions-hint').textContent =
    `v1: ${v1Count} · v2: ${v2SessSet.size}`;

}

// -------- 主趋势图 --------
function renderTrend(turns) {
  if (typeof echarts === 'undefined') return;  // echarts 异步加载中, ready 后会重绘
  const range = computeRange();
  const buckets = aggregateByGran(turns, state.gran, range);
  const useArea = state.gran === 'week' || state.gran === 'month';

  const xData = buckets.map(b => b.key);
  const credits = buckets.map(b => +b.credits.toFixed(2));
  const turnCounts = buckets.map(b => b.turns);
  const elapsedHours = buckets.map(b => +(b.elapsed / 3600000).toFixed(2));

  const accent = css('--accent') || '#8b5cf6';
  const accent2 = css('--accent-2') || '#3b82f6';
  const fgDim = css('--fg-dim') || '#a1a5b3';
  const fgMute = css('--fg-mute') || '#6b7080';
  const border = css('--border-strong') || '#2e3341';

  const gradient = new echarts.graphic.LinearGradient(0, 0, 0, 1, [
    { offset: 0, color: accent },
    { offset: 1, color: accent2 },
  ]);

  const series = [{
    name: 'Credits',
    type: useArea ? 'line' : 'bar',
    yAxisIndex: 0,
    data: credits,
    smooth: useArea,
    symbol: useArea ? 'circle' : 'none',
    symbolSize: 6,
    itemStyle: { color: gradient, borderRadius: [4, 4, 0, 0] },
    lineStyle: useArea ? { color: accent, width: 2 } : undefined,
    areaStyle: useArea ? {
      color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
        { offset: 0, color: accent + '80' },
        { offset: 1, color: accent + '05' },
      ]),
    } : undefined,
    emphasis: { itemStyle: { color: css('--bar-grad-hi') || '#a78bfa' } },
    barMaxWidth: 32,
  }];

  // 三个 y 轴不显示 name（原来的 credits/turns/h 会和顶部 legend、tick 挤成一坨），
  // 用 legend + tooltip 传达维度即可。
  const yAxes = [{
    type: 'value',
    axisLine: { show: false },
    axisTick: { show: false },
    splitLine: { lineStyle: { color: border, opacity: 0.35, type: 'dashed' } },
    axisLabel: { color: fgDim, fontSize: 11 },
  }];

  const legendData = ['Credits'];

  if (state.showTurns) {
    series.push({
      name: 'Turn 数',
      type: 'line',
      yAxisIndex: 1,
      data: turnCounts,
      smooth: true,
      symbol: 'circle', symbolSize: 5,
      lineStyle: { width: 2, color: '#22d3ee' },
      itemStyle: { color: '#22d3ee' },
    });
    yAxes.push({
      type: 'value', position: 'right',
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { color: fgDim, fontSize: 11 },
    });
    legendData.push('Turn 数');
  }

  if (state.showElapsed) {
    const yIdx = yAxes.length;
    series.push({
      name: '耗时 (h)',
      type: 'line',
      yAxisIndex: yIdx,
      data: elapsedHours,
      smooth: true,
      symbol: 'circle', symbolSize: 5,
      lineStyle: { width: 2, color: '#f59e0b' },
      itemStyle: { color: '#f59e0b' },
    });
    yAxes.push({
      type: 'value', position: 'right', offset: 44,
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { color: fgDim, fontSize: 11 },
    });
    legendData.push('耗时 (h)');
  }

  const option = {
    animation: true,
    animationDuration: 400,
    // grid.top 给 legend 留够空间；不显示 y 轴 name 后可以更紧凑
    grid: { top: 48, right: 56 + (state.showElapsed ? 44 : 0), bottom: 40, left: 52 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: css('--card') || '#12141c',
      borderColor: border,
      textStyle: { color: css('--fg') || '#e5e7eb' },
      axisPointer: { type: useArea ? 'line' : 'shadow',
                     shadowStyle: { color: 'rgba(139,92,246,0.08)' } },
    },
    legend: {
      data: legendData,
      textStyle: { color: css('--fg') || '#e5e7eb', fontSize: 12 },
      top: 10, right: 16,
      icon: 'circle',
      itemGap: 16,
    },
    xAxis: {
      type: 'category',
      data: xData,
      axisLine: { lineStyle: { color: border } },
      axisTick: { show: false },
      axisLabel: {
        color: fgDim, fontSize: 11,
        hideOverlap: true,
        formatter: (v) => {
          if (state.gran === 'hour') return v.slice(5, 13);
          if (state.gran === 'day') return v.slice(5);
          if (state.gran === 'week') return v.slice(5) + ' 周';
          return v;
        },
      },
    },
    yAxis: yAxes,
    series,
    dataZoom: buckets.length > 30 ? [
      { type: 'inside', start: Math.max(0, 100 - Math.min(100, 3000 / buckets.length)), end: 100 },
      { type: 'slider', bottom: 8, height: 16,
        borderColor: 'transparent',
        backgroundColor: 'transparent',
        fillerColor: accent + '30',
        handleStyle: { color: accent },
        textStyle: { color: fgMute, fontSize: 10 },
      },
    ] : undefined,
  };

  const chart = chartOf(document.getElementById('chart-trend'), 'trend');
  chart.setOption(option, true);

  // 点击柱状/折线 → 过滤明细表
  chart.off('click');
  chart.on('click', (params) => {
    if (params.componentType !== 'series') return;
    const key = xData[params.dataIndex];
    if (!key) return;
    const [start, end] = bucketRange(key, state.gran);
    applyClickFilter({
      kind: 'timeSlot', start, end,
      label: `${labelOfGran(state.gran)}: ${key}`,
    });
  });

  // 副标题
  const subMap = { hour: '按小时', day: '按天', week: '按周', month: '按月' };
  const rangeMap = { today: '今日', week: '本周', month: '本月', '30d': '近 30 天', all: '全部' };
  document.getElementById('trend-sub').textContent =
    `${subMap[state.gran]} · ${rangeMap[state.range]} · ${buckets.length} 桶`;
}

// 一个桶键对应的时间区间 [start, end)
function bucketRange(key, gran) {
  const parse = (s) => {
    const [dp, tp] = s.split(' ');
    const [y, m, d] = dp.split('-').map(Number);
    let hh = 0;
    if (tp) hh = parseInt(tp.split(':')[0], 10);
    // 转成毫秒时间戳（"本地墙钟" → UTC ms）
    return Date.UTC(y, m - 1, d, hh) - state.serverTzOffsetMin * 60000;
  };
  if (gran === 'hour') { const s = parse(key); return [s, s + 3600000]; }
  if (gran === 'day')  { const s = parse(key); return [s, s + 86400000]; }
  if (gran === 'week') { const s = parse(key); return [s, s + 7 * 86400000]; }
  if (gran === 'month') {
    const [y, m] = key.split('-').map(Number);
    const s = Date.UTC(y, m - 1, 1) - state.serverTzOffsetMin * 60000;
    const e = Date.UTC(m === 12 ? y + 1 : y, m % 12, 1) - state.serverTzOffsetMin * 60000;
    return [s, e];
  }
  return [0, 0];
}
function labelOfGran(g) {
  return { hour: '小时', day: '日', week: '周', month: '月' }[g] || g;
}

// -------- 24×7 热力图 --------
function renderHeatmap(turns) {
  if (typeof echarts === 'undefined') return;  // echarts 异步加载中, ready 后会重绘
  // 累加：credits[weekday][hour]，weekday 0=周一 ... 6=周日
  const grid = Array.from({ length: 7 }, () => new Array(24).fill(0));
  for (const t of turns) {
    const d = toLocalDate(t.t);
    const dow = (d.getUTCDay() || 7) - 1;   // 0..6, Mon..Sun
    const hr = d.getUTCHours();
    grid[dow][hr] += t.c;
  }

  const data = [];
  let maxVal = 0;
  for (let d = 0; d < 7; d++) {
    for (let h = 0; h < 24; h++) {
      const v = +grid[d][h].toFixed(2);
      if (v > maxVal) maxVal = v;
      data.push([h, d, v]);
    }
  }

  const border = css('--border-strong') || '#2e3341';
  const fgDim = css('--fg-dim') || '#a1a5b3';
  const bg = css('--card') || '#12141c';

  const option = {
    tooltip: {
      backgroundColor: bg,
      borderColor: border,
      textStyle: { color: css('--fg') },
      formatter: (p) => {
        const days = ['周一','周二','周三','周四','周五','周六','周日'];
        return `${days[p.data[1]]} ${fmt2(p.data[0])}:00<br/><b>${fmtCredits(p.data[2])}</b> credits`;
      },
    },
    grid: { top: 20, right: 60, bottom: 30, left: 60 },
    xAxis: {
      type: 'category',
      data: Array.from({ length: 24 }, (_, i) => fmt2(i)),
      axisLine: { show: false }, axisTick: { show: false },
      axisLabel: { color: fgDim, fontSize: 10,
                   formatter: (v, i) => i % 3 === 0 ? v : '' },
      splitArea: { show: false },
    },
    yAxis: {
      type: 'category',
      data: ['周一','周二','周三','周四','周五','周六','周日'],
      axisLine: { show: false }, axisTick: { show: false },
      axisLabel: { color: fgDim, fontSize: 11 },
      splitArea: { show: false },
    },
    visualMap: {
      min: 0, max: Math.max(maxVal, 1),
      show: false,
      inRange: {
        color: ['rgba(139,92,246,0.05)', 'rgba(139,92,246,0.35)',
                'rgba(139,92,246,0.7)', 'rgba(167,139,250,0.95)'],
      },
    },
    series: [{
      type: 'heatmap', data,
      itemStyle: { borderRadius: 3, borderColor: bg, borderWidth: 2 },
      emphasis: { itemStyle: { borderColor: css('--accent'), borderWidth: 2 } },
    }],
  };
  const chart = chartOf(document.getElementById('chart-heatmap'), 'heatmap');
  chart.setOption(option, true);

  // 点击格子 → 按（周几+小时）过滤当前 range 内符合的所有 turn
  chart.off('click');
  chart.on('click', (p) => {
    const hr = p.data[0], dow = p.data[1];
    const days = ['周一','周二','周三','周四','周五','周六','周日'];
    // 用非常规过滤：kind='dowHour' 我们暂不支持时间段联动，
    // 改为按 dow+hour 直接筛 turns
    applyClickFilter({
      kind: 'dowHour', dow, hr,
      label: `${days[dow]} ${fmt2(hr)}:00`,
    });
  });
}

// -------- 工具 Treemap --------
function renderTools(turns) {
  if (typeof echarts === 'undefined') return;  // echarts 异步加载中, ready 后会重绘
  // 每 turn 的 credits 均摊到它调用的工具
  const map = new Map();
  for (const t of turns) {
    const tools = t.tools || [];
    if (tools.length === 0) continue;
    const share = state.toolMetric === 'credits' ? t.c / tools.length : 1;
    for (const tool of tools) {
      const cur = map.get(tool) || { name: tool, credits: 0, count: 0 };
      cur.credits += (state.toolMetric === 'credits' ? share : 0);
      cur.count += (state.toolMetric === 'count' ? 1 : (t.c > 0 ? share > 0 ? 1 : 0 : 0));
      // count 维度另算
      map.set(tool, cur);
    }
  }
  // 单独统计 count
  if (state.toolMetric === 'count') {
    for (const [, v] of map) v.count = 0;
    for (const t of turns) for (const tool of (t.tools || [])) {
      const cur = map.get(tool);
      if (cur) cur.count += 1;
    }
  }

  const data = Array.from(map.values()).map(v => ({
    name: v.name,
    value: state.toolMetric === 'credits' ? +v.credits.toFixed(2) : v.count,
  })).filter(x => x.value > 0).sort((a, b) => b.value - a.value);

  const border = css('--border-strong');
  const fg = css('--fg');
  const bg = css('--card');

  const option = {
    tooltip: {
      backgroundColor: bg,
      borderColor: border,
      textStyle: { color: fg },
      formatter: (p) => {
        const unit = state.toolMetric === 'credits' ? 'credits' : '次';
        return `<b>${p.name}</b><br/>${fmtCredits(p.value)} ${unit}`;
      },
    },
    series: [{
      type: 'treemap',
      data,
      roam: false,
      breadcrumb: { show: false },
      label: {
        show: true,
        formatter: (p) => `{n|${p.name}}\n{v|${fmtCredits(p.value)}}`,
        rich: {
          n: { color: '#fff', fontSize: 12, fontWeight: 500 },
          v: { color: 'rgba(255,255,255,0.7)', fontSize: 10, padding: [4,0,0,0] },
        },
      },
      itemStyle: {
        borderColor: bg, borderWidth: 2, gapWidth: 2,
      },
      levels: [{
        colorSaturation: [0.4, 0.9],
        itemStyle: { borderColor: bg, gapWidth: 2 },
      }],
      color: [
        '#8b5cf6', '#3b82f6', '#22d3ee', '#10b981', '#f59e0b',
        '#ef4444', '#ec4899', '#a78bfa', '#60a5fa', '#34d399',
      ],
    }],
  };
  const chart = chartOf(document.getElementById('chart-tools'), 'tools');
  chart.setOption(option, true);

  chart.off('click');
  chart.on('click', (p) => {
    if (!p.name) return;
    applyClickFilter({ kind: 'tool', key: p.name, label: `工具: ${p.name}` });
  });
}

// -------- Top Sessions 表 --------
function renderTopSessions(turns) {
  const map = new Map();
  for (const t of turns) {
    const key = t.aid;
    let s = map.get(key);
    if (!s) {
      s = { aid: key, title: t.title, turns: 0, credits: 0, elapsed: 0, last: 0 };
      map.set(key, s);
    }
    s.turns += 1;
    s.credits += t.c;
    s.elapsed += t.e;
    if (t.t > s.last) s.last = t.t;
    if (!s.title && t.title) s.title = t.title;
  }
  const rows = Array.from(map.values()).sort((a, b) => b.credits - a.credits).slice(0, 15);

  const tbody = document.querySelector('#top-sessions-table tbody');
  tbody.innerHTML = rows.map((s, i) => `
    <tr class="clickable" data-aid="${s.aid}">
      <td>${i + 1}</td>
      <td title="${escapeHtml(s.title || '(untitled)')} — ${s.aid}">
        <div style="max-width:280px;overflow:hidden;text-overflow:ellipsis">
          ${escapeHtml(s.title || '(untitled)')}
        </div>
        <div style="color:var(--fg-mute);font-size:11px">${s.aid.slice(0, 8)}</div>
      </td>
      <td class="num">${s.turns}</td>
      <td class="num">${fmtCredits(s.credits)}</td>
      <td class="num">${fmtDuration(s.elapsed)}</td>
      <td>${fmtLocalDT(s.last)}</td>
    </tr>
  `).join('') || '<tr><td colspan="6" style="text-align:center;color:var(--fg-mute);padding:24px">当前范围无数据</td></tr>';

  tbody.querySelectorAll('tr.clickable').forEach(tr => {
    tr.addEventListener('click', () => {
      const aid = tr.dataset.aid;
      const sess = rows.find(x => x.aid === aid);
      applyClickFilter({ kind: 'session', key: aid,
        label: `Session: ${sess.title || aid.slice(0, 8)}` });
    });
  });
}

// -------- Workspace 环形 --------
// 用"session 数"作为占比维度，混合 v1 + v2（credits 只有 v2 有，不适合跨源统计）。
function renderWorkspace(turns) {
  if (typeof echarts === 'undefined') return;  // echarts 异步加载中, ready 后会重绘
  const map = new Map();  // ws -> session_count
  // v2：按 aid 去重
  const v2Seen = new Map(); // ws -> Set(aid)
  for (const t of turns) {
    const k = t.ws || '(no-workspace)';
    if (!v2Seen.has(k)) v2Seen.set(k, new Set());
    v2Seen.get(k).add(t.aid);
  }
  for (const [k, aids] of v2Seen) map.set(k, aids.size);
  // v1：直接加 session 数
  for (const s of (state.v1Sessions || [])) {
    const k = s.workspace || '(no-workspace)';
    map.set(k, (map.get(k) || 0) + 1);
  }
  const data = Array.from(map.entries())
    .map(([name, value]) => ({ name, value }))
    .filter(x => x.value > 0)
    .sort((a, b) => b.value - a.value);

  const bg = css('--card');
  const fg = css('--fg');
  const fgDim = css('--fg-dim');
  const border = css('--border-strong');

  const option = {
    tooltip: {
      trigger: 'item',
      backgroundColor: bg, borderColor: border,
      textStyle: { color: fg },
      formatter: (p) => `<b>${p.name}</b><br/>${p.value} sessions (${p.percent}%)`,
    },
    legend: {
      orient: 'vertical', right: 8, top: 'middle',
      textStyle: { color: fgDim, fontSize: 12 },
      itemWidth: 10, itemHeight: 10,
      formatter: (n) => n.length > 14 ? n.slice(0, 13) + '…' : n,
    },
    series: [{
      type: 'pie',
      radius: ['48%', '76%'],
      center: ['38%', '50%'],
      avoidLabelOverlap: true,
      itemStyle: {
        borderColor: bg, borderWidth: 3, borderRadius: 4,
      },
      label: { show: false },
      emphasis: {
        label: {
          show: true,
          formatter: '{b}\n{d}%',
          color: fg, fontSize: 12, fontWeight: 600,
        },
      },
      data,
      color: [
        '#8b5cf6', '#3b82f6', '#22d3ee', '#10b981', '#f59e0b',
        '#ef4444', '#ec4899', '#a78bfa', '#60a5fa',
      ],
    }],
  };
  const chart = chartOf(document.getElementById('chart-workspace'), 'workspace');
  chart.setOption(option, true);

  chart.off('click');
  chart.on('click', (p) => {
    applyClickFilter({ kind: 'workspace', key: p.name, label: `Workspace: ${p.name}` });
  });
}

// ============================================================
// 6. 明细表
// ============================================================

function renderDetail(turnsInRange) {
  // 明细表的过滤基线是"当前 range + click filter"后的结果
  let arr = turnsInRange;

  // 额外应用搜索/status/workspace 筛选
  const q = state.detailSearch.trim().toLowerCase();
  if (q) arr = arr.filter(t =>
    (t.title || '').toLowerCase().includes(q) ||
    (t.ws || '').toLowerCase().includes(q) ||
    (t.model || '').toLowerCase().includes(q));
  if (state.detailStatus) arr = arr.filter(t => t.s === state.detailStatus);
  if (state.detailWorkspace) arr = arr.filter(t => t.ws === state.detailWorkspace);

  // 排序
  const { field, dir } = state.detailSort;
  const factor = dir === 'asc' ? 1 : -1;
  arr = arr.slice().sort((a, b) => {
    let va = a[field], vb = b[field];
    if (field === 'tools') { va = (a.tools || []).length; vb = (b.tools || []).length; }
    if (typeof va === 'string' && typeof vb === 'string') return va.localeCompare(vb) * factor;
    return (va > vb ? 1 : va < vb ? -1 : 0) * factor;
  });

  // 分页
  const total = arr.length;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  if (state.detailPage > totalPages) state.detailPage = totalPages;
  const start = (state.detailPage - 1) * PAGE_SIZE;
  const pageItems = arr.slice(start, start + PAGE_SIZE);

  document.getElementById('detail-sub').textContent = `${total} 条 · 第 ${state.detailPage}/${totalPages} 页`;

  const tbody = document.querySelector('#detail-table tbody');
  tbody.innerHTML = pageItems.map(t => `
    <tr>
      <td>${fmtLocalDT(t.t)}</td>
      <td title="${escapeHtml(t.ws)}">${escapeHtml(t.ws)}</td>
      <td class="num">${fmtCredits(t.c)}</td>
      <td class="num">${fmtDuration(t.e)}</td>
      <td><span class="badge badge-${t.s || 'unknown'}">${t.s}</span></td>
      <td class="num">${(t.tools || []).length}</td>
      <td title="${escapeHtml(t.model)}">${escapeHtml((t.model || '').replace('claude-', ''))}</td>
      <td title="${escapeHtml(t.title)}">${escapeHtml((t.title || '').slice(0, 60))}</td>
    </tr>
  `).join('') || `<tr><td colspan="8" style="text-align:center;color:var(--fg-mute);padding:24px">无数据</td></tr>`;

  // 分页控件
  renderPager(totalPages);

  // 表头排序标记
  document.querySelectorAll('#detail-table thead th[data-sort]').forEach(th => {
    th.classList.remove('sort-asc', 'sort-desc');
    if (th.dataset.sort === field) th.classList.add('sort-' + dir);
  });

  // 记住当前的 arr 供 CSV 导出
  state._detailFiltered = arr;
}

function renderPager(totalPages) {
  const pager = document.getElementById('detail-pager');
  if (totalPages <= 1) { pager.innerHTML = ''; return; }
  const cur = state.detailPage;
  const btn = (label, page, opts = {}) => {
    const dis = opts.disabled ? 'disabled' : '';
    const active = opts.active ? 'active' : '';
    return `<button ${dis} class="${active}" data-page="${page}">${label}</button>`;
  };
  let html = '';
  html += btn('«', 1, { disabled: cur === 1 });
  html += btn('‹', Math.max(1, cur - 1), { disabled: cur === 1 });
  // 中间连续 5 个页码
  const span = 2;
  const from = Math.max(1, cur - span);
  const to = Math.min(totalPages, cur + span);
  if (from > 1) html += `<span class="info">…</span>`;
  for (let i = from; i <= to; i++) html += btn(String(i), i, { active: i === cur });
  if (to < totalPages) html += `<span class="info">…</span>`;
  html += btn('›', Math.min(totalPages, cur + 1), { disabled: cur === totalPages });
  html += btn('»', totalPages, { disabled: cur === totalPages });
  html += `<span class="info">共 ${totalPages} 页</span>`;
  pager.innerHTML = html;

  pager.querySelectorAll('button').forEach(b => {
    b.addEventListener('click', () => {
      state.detailPage = +b.dataset.page;
      render();
    });
  });
}

// ============================================================
//  账号切换历史面板
// ============================================================

function renderAccounts() {
  const accs = state.accounts || [];
  document.getElementById('accounts-sub').textContent = `${accs.length} 个账号`;

  // ---- ECharts 折线：每个账号一条线 ----
  // echarts 异步加载中时整块跳过（表格在下方照常渲染），ready 后 render() 会重绘
  if (typeof echarts !== 'undefined') {
  const border = css('--border-strong');
  const fg = css('--fg');
  const fgDim = css('--fg-dim');
  const fgMute = css('--fg-mute');
  const bg = css('--card');

  const palette = ['#8b5cf6', '#22d3ee', '#f59e0b', '#10b981', '#ef4444', '#ec4899', '#60a5fa'];

  const legendData = [];
  const series = [];
  // 找全局时间范围
  let tMin = Infinity, tMax = -Infinity;
  for (const a of accs) {
    for (const s of (a.snapshots || [])) {
      if (s.t < tMin) tMin = s.t;
      if (s.t > tMax) tMax = s.t;
    }
  }

  accs.forEach((a, i) => {
    const label = a.uid && a.uid !== '(unknown)' ? a.uid.slice(0, 20) + '…' : '(账号 #' + (i + 1) + ')';
    legendData.push(label);
    const color = palette[i % palette.length];
    series.push({
      name: label,
      type: 'line',
      showSymbol: (a.snapshots || []).length < 100,
      symbol: 'circle', symbolSize: 5,
      smooth: false,
      step: 'end',   // 阶梯式：反映"扣费是离散事件"
      lineStyle: { color, width: 2 },
      itemStyle: { color },
      areaStyle: {
        color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
          { offset: 0, color: color + '55' },
          { offset: 1, color: color + '00' },
        ]),
      },
      data: (a.snapshots || []).map(s => [s.t, +s.current.toFixed(2)]),
    });
  });

  const option = {
    animation: true,
    grid: { top: 40, right: 20, bottom: 50, left: 60 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: bg, borderColor: border, textStyle: { color: fg },
      formatter: (params) => {
        const t = new Date(params[0].value[0]);
        let lines = [`<b>${t.toLocaleString('zh-CN', { hour12: false })}</b>`];
        for (const p of params) {
          lines.push(`${p.marker} ${p.seriesName}: ${fmtCredits(p.value[1])}`);
        }
        return lines.join('<br/>');
      },
    },
    legend: {
      data: legendData, top: 4, textStyle: { color: fgDim, fontSize: 11 },
      icon: 'circle',
    },
    xAxis: {
      type: 'time',
      axisLine: { lineStyle: { color: border } },
      axisTick: { show: false },
      axisLabel: { color: fgDim, fontSize: 10 },
    },
    yAxis: {
      type: 'value', name: 'currentUsage',
      nameTextStyle: { color: fgMute, fontSize: 11 },
      axisLine: { show: false }, axisTick: { show: false },
      splitLine: { lineStyle: { color: border, opacity: 0.3, type: 'dashed' } },
      axisLabel: { color: fgDim, fontSize: 11 },
    },
    series,
  };

  if (accs.length && isFinite(tMin)) {
    const chart = chartOf(document.getElementById('chart-accounts'), 'accounts');
    if (chart) chart.setOption(option, true);
  } else {
    document.getElementById('chart-accounts').innerHTML =
      '<div style="height:100%;display:grid;place-items:center;color:var(--fg-mute)">暂无账号 quota 快照数据</div>';
  }
  }  // end if (echarts ready)

  // ---- 表格 ----
  const tbody = document.querySelector('#accounts-table tbody');
  tbody.innerHTML = accs.map((a, i) => {
    const first = a.first_seen ? new Date(a.first_seen).toLocaleString('zh-CN', { hour12: false }) : '-';
    const last = a.last_seen ? new Date(a.last_seen).toLocaleString('zh-CN', { hour12: false }) : '-';
    const uid = a.uid || '(unknown)';
    return `
      <tr>
        <td>${i + 1}</td>
        <td title="${escapeHtml(uid)}">
          <div style="max-width:280px;overflow:hidden;text-overflow:ellipsis;font-family:var(--font-mono, monospace);font-size:12px">
            ${escapeHtml(uid)}
          </div>
        </td>
        <td style="font-size:12px">
          ${first}<br/><span style="color:var(--fg-mute)">→ ${last}</span>
        </td>
        <td class="num"><b>${fmtCredits(a.peak)}</b></td>
        <td class="num">${fmtCredits(a.latest)}</td>
        <td class="num">${a.latest_limit || '-'}</td>
        <td class="num">${a.resets}</td>
        <td class="num">${(a.snapshots || []).length}</td>
      </tr>
    `;
  }).join('') || '<tr><td colspan="8" style="text-align:center;color:var(--fg-mute);padding:24px">暂无账号数据</td></tr>';
}

// ============================================================
//  v1 sessions 表
// ============================================================

function renderV1Sessions() {
  const all = state.v1Sessions || [];
  let arr = all.slice();

  // 过滤
  const q = state.v1Search.trim().toLowerCase();
  if (q) arr = arr.filter(s =>
    (s.title || '').toLowerCase().includes(q) ||
    (s.workspace || '').toLowerCase().includes(q) ||
    (s.workspace_full || '').toLowerCase().includes(q) ||
    (s.model || '').toLowerCase().includes(q));
  if (state.v1WorkspaceFilter) arr = arr.filter(s => s.workspace_full === state.v1WorkspaceFilter);

  // 排序
  const { field, dir } = state.v1Sort;
  const factor = dir === 'asc' ? 1 : -1;
  arr.sort((a, b) => {
    const va = a[field], vb = b[field];
    if (typeof va === 'string' && typeof vb === 'string') return va.localeCompare(vb) * factor;
    return (va > vb ? 1 : va < vb ? -1 : 0) * factor;
  });

  const total = arr.length;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  if (state.v1Page > totalPages) state.v1Page = totalPages;
  const start = (state.v1Page - 1) * PAGE_SIZE;
  const pageItems = arr.slice(start, start + PAGE_SIZE);

  document.getElementById('v1-sub').textContent = `${total} 条 · 第 ${state.v1Page}/${totalPages} 页`;

  const tbody = document.querySelector('#v1-table tbody');
  tbody.innerHTML = pageItems.map(s => `
    <tr>
      <td>${fmtLocalDT(s.t)}</td>
      <td title="${escapeHtml(s.workspace_full || s.workspace)}">${escapeHtml(s.workspace)}</td>
      <td class="num">${s.turn_count}</td>
      <td title="${escapeHtml(s.model)}">${escapeHtml(s.model || '')}</td>
      <td title="${escapeHtml(s.title)}">${escapeHtml((s.title || '').slice(0, 80))}</td>
    </tr>
  `).join('') || '<tr><td colspan="5" style="text-align:center;color:var(--fg-mute);padding:24px">无数据</td></tr>';

  // 分页
  renderV1Pager(totalPages);

  // 排序标记
  document.querySelectorAll('#v1-table thead th[data-sort]').forEach(th => {
    th.classList.remove('sort-asc', 'sort-desc');
    if (th.dataset.sort === field) th.classList.add('sort-' + dir);
  });
}

function renderV1Pager(totalPages) {
  const pager = document.getElementById('v1-pager');
  if (totalPages <= 1) { pager.innerHTML = ''; return; }
  const cur = state.v1Page;
  const btn = (label, page, opts = {}) => {
    const dis = opts.disabled ? 'disabled' : '';
    const active = opts.active ? 'active' : '';
    return `<button ${dis} class="${active}" data-page="${page}">${label}</button>`;
  };
  let html = '';
  html += btn('«', 1, { disabled: cur === 1 });
  html += btn('‹', Math.max(1, cur - 1), { disabled: cur === 1 });
  const span = 2;
  const from = Math.max(1, cur - span);
  const to = Math.min(totalPages, cur + span);
  if (from > 1) html += `<span class="info">…</span>`;
  for (let i = from; i <= to; i++) html += btn(String(i), i, { active: i === cur });
  if (to < totalPages) html += `<span class="info">…</span>`;
  html += btn('›', Math.min(totalPages, cur + 1), { disabled: cur === totalPages });
  html += btn('»', totalPages, { disabled: cur === totalPages });
  html += `<span class="info">共 ${totalPages} 页</span>`;
  pager.innerHTML = html;

  pager.querySelectorAll('button').forEach(b => {
    b.addEventListener('click', () => {
      state.v1Page = +b.dataset.page;
      renderV1Sessions();
    });
  });
}

// 明细控件的下拉初始化（workspace 列表来自 turns 和 v1 sessions）
function refreshDetailFilters() {
  // v2 明细表
  const wsSel = document.getElementById('detail-workspace');
  const wsSet = new Set();
  for (const t of state.turns) if (t.ws) wsSet.add(t.ws);
  const cur = wsSel.value;
  wsSel.innerHTML = '<option value="">全部 workspace</option>' +
    Array.from(wsSet).sort().map(w => `<option value="${escapeAttr(w)}">${escapeHtml(w)}</option>`).join('');
  if (cur && wsSet.has(cur)) wsSel.value = cur;

  // v1 sessions 表：workspace 下拉用完整路径当 value
  const v1Sel = document.getElementById('v1-workspace-filter');
  if (v1Sel) {
    const wsMap = new Map(); // full → basename
    for (const s of (state.v1Sessions || [])) {
      if (s.workspace_full) wsMap.set(s.workspace_full, s.workspace);
    }
    const curV1 = v1Sel.value;
    const opts = Array.from(wsMap.entries())
      .sort((a, b) => a[1].localeCompare(b[1]))
      .map(([full, name]) => `<option value="${escapeAttr(full)}">${escapeHtml(name)}</option>`)
      .join('');
    v1Sel.innerHTML = '<option value="">全部 workspace</option>' + opts;
    if (curV1 && wsMap.has(curV1)) v1Sel.value = curV1;
  }
}

// CSV 导出（前端生成，反映当前筛选）
function exportCsv() {
  const arr = state._detailFiltered || state.turns;
  const cols = [
    ['ts_local', t => fmtLocalDT(t.t)],
    ['ts_utc_ms', t => t.t],
    ['workspace', t => t.ws],
    ['session_id', t => t.sid],
    ['agent_session_id', t => t.aid],
    ['execution_id', t => t.eid],
    ['credits', t => t.c.toFixed(6)],
    ['elapsed_ms', t => t.e],
    ['elapsed_human', t => fmtDuration(t.e)],
    ['status', t => t.s],
    ['tool_count', t => (t.tools || []).length],
    ['model', t => t.model],
    ['title', t => t.title],
    ['tools', t => (t.tools || []).join('|')],
  ];
  const esc = (v) => {
    v = (v == null) ? '' : String(v);
    if (/[",\r\n]/.test(v)) return '"' + v.replace(/"/g, '""') + '"';
    return v;
  };
  const lines = [cols.map(c => c[0]).join(',')];
  for (const t of arr) lines.push(cols.map(c => esc(c[1](t))).join(','));
  const blob = new Blob(['\ufeff' + lines.join('\r\n')], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `kiro_usage_${new Date().toISOString().slice(0, 10)}.csv`;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ============================================================
// 7. 主题、交互、联动
// ============================================================

function applyClickFilter(f) {
  state.clickFilter = f;
  state.detailPage = 1;
  const bar = document.getElementById('filter-bar');
  document.getElementById('filter-value').textContent = f.label;
  bar.classList.remove('hidden');
  render();
}

function clearClickFilter() {
  state.clickFilter = null;
  document.getElementById('filter-bar').classList.add('hidden');
  render();
}

// 应用主题：读 localStorage，重新渲染让 ECharts 拾取新颜色
function applyTheme(theme) {
  document.documentElement.setAttribute('data-theme', theme);
  localStorage.setItem('kiroDashTheme', theme);
  // ECharts 图表需要 setOption 重绘才能拿到新的 CSS 变量
  setTimeout(() => render(), 50);
}

function initTheme() {
  const saved = localStorage.getItem('kiroDashTheme') || 'dark';
  applyTheme(saved);
}

// ============================================================
//  视图路由（Sidebar 导航）
// ============================================================

const VIEW_TITLES = {
  glance: '简约',
  overview: '总览',
  trends: '趋势',
  tools: '工具与工作区',
  accounts: '账号历史',
  sessions: '明细',
};

const VALID_VIEWS = new Set(Object.keys(VIEW_TITLES));

// 切换到某个视图。会更新 DOM 类名、hash、标题，然后渲染该视图。
function switchView(name, opts = {}) {
  if (!VALID_VIEWS.has(name)) name = 'overview';
  const changed = state.view !== name;
  state.view = name;

  // section 显示切换
  document.querySelectorAll('.view').forEach(sec => {
    sec.classList.toggle('active', sec.dataset.view === name);
  });
  // sidebar 高亮
  document.querySelectorAll('.nav-item[data-view]').forEach(a => {
    a.classList.toggle('active', a.dataset.view === name);
  });
  // 顶栏标题
  const titleEl = document.getElementById('topbar-title');
  if (titleEl) titleEl.textContent = VIEW_TITLES[name] || '';

  // hash 同步（避免死循环 —— 只在真变了时写）
  if (!opts.skipHash) {
    const target = '#' + name;
    if (location.hash !== target) location.hash = target;
  }

  // 渲染当前视图（DOM 可见后 chart 才能拿到尺寸）
  render();

  // 视图切换后，同一 chart 实例可能因为之前隐藏导致尺寸未知 —— 强制 resize
  if (changed) {
    setTimeout(() => {
      Object.values(state.charts).forEach(c => c && c.resize());
    }, 60);
  }
}

// 初始化路由：读 hash + 绑定 nav 点击 + hashchange
function initRouter() {
  const initial = (location.hash || '').slice(1);
  // v0.4: 默认首页从 overview 改为 glance (简约视图)
  const start = VALID_VIEWS.has(initial) ? initial : 'glance';

  document.querySelectorAll('.nav-item[data-view]').forEach(a => {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      switchView(a.dataset.view);
    });
  });
  // "查看完整趋势 →" 之类的内部跳转
  document.querySelectorAll('[data-view-link]').forEach(a => {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      switchView(a.dataset.viewLink);
    });
  });
  window.addEventListener('hashchange', () => {
    const v = (location.hash || '').slice(1);
    if (VALID_VIEWS.has(v) && v !== state.view) switchView(v, { skipHash: true });
  });

  switchView(start);
}

// 明细视图内 v2/v1 tab 切换
function switchDetailTab(tab) {
  if (tab !== 'v2' && tab !== 'v1') tab = 'v2';
  state.detailTab = tab;
  document.querySelectorAll('.tab-btn').forEach(b => {
    b.classList.toggle('active', b.dataset.tab === tab);
  });
  document.querySelectorAll('.tab-panel').forEach(p => {
    p.classList.toggle('hidden', p.dataset.tabPanel !== tab);
  });
  // 切 tab 后可能有分页/搜索状态变化，重新渲染当前视图
  if (state.view === 'sessions') render();
}

// ============================================================
// 主渲染入口
// ============================================================

// 只渲染当前视图（每个视图各自独立初始化图表 / 表格）。
// KPI 和 footer 是全局，任何视图下都刷新（数据便宜）。
function render() {
  const rangeArr = filteredTurns();

  // KPI 只在总览视图显示，但计算便宜。footer 是全局状态。
  renderKPI(rangeArr);
  updateFooter();

  const view = state.view;

  if (view === 'glance') {
    renderGlance(rangeArr);
  } else if (view === 'overview') {
    renderTrendPreview(rangeArr);
  } else if (view === 'trends') {
    renderTrend(rangeArr);
    renderHeatmap(rangeArr);
  } else if (view === 'tools') {
    renderTools(rangeArr);
    renderWorkspace(rangeArr);
    renderTopSessions(rangeArr);
  } else if (view === 'accounts') {
    renderAccounts();
  } else if (view === 'sessions') {
    // dowHour 联动过滤只影响 v2 明细表
    let arrForDetail = rangeArr;
    if (state.clickFilter && state.clickFilter.kind === 'dowHour') {
      const { dow, hr } = state.clickFilter;
      arrForDetail = rangeArr.filter(t => {
        const d = toLocalDate(t.t);
        return ((d.getUTCDay() || 7) - 1) === dow && d.getUTCHours() === hr;
      });
    }
    refreshDetailFilters();
    if (state.detailTab === 'v2') renderDetail(arrForDetail);
    else renderV1Sessions();
  }
}

// v0.4: 简约视图渲染。核心 = 4 张 Bento KPI + 欢迎大卡文案 + 一张主图。
// 定位是"打开就一眼扫完"，所以布局精简，只放最关键的数据。
function renderGlance(turns) {
  // ---- 1) Bento KPI 4 张 ----
  const sumC = turns.reduce((s, t) => s + t.c, 0);
  const sumE = turns.reduce((s, t) => s + t.e, 0);
  const priced = turns.filter(t => t.c > 0);

  const setText = (id, val) => {
    const el = document.getElementById(id);
    if (el) el.textContent = val;
  };

  setText('bento-est', fmtCredits(sumC));
  setText('bento-est-hint',
    priced.length ? `含计费 turn ${priced.length}` : 'Est. Credits Used');

  setText('bento-turns', turns.length.toLocaleString('zh-CN'));
  setText('bento-turns-hint',
    priced.length ? `${priced.length} 条含 credits` : '当前范围无计费 turn');

  setText('bento-elapsed', fmtDuration(sumE));
  setText('bento-elapsed-hint',
    priced.length ? `平均 ${fmtDuration(Math.round(sumE / priced.length))} / turn` : '-');

  // v2 sessions 按 agent_session_id 去重
  const v2SessSet = new Set();
  for (const t of state.turns) if (t.aid) v2SessSet.add(t.aid);
  const v1Count = (state.v1Sessions || []).length;
  const totalSess = v1Count + v2SessSet.size;
  setText('bento-sessions', totalSess.toLocaleString('zh-CN'));
  setText('bento-sessions-hint', `v1 ${v1Count} · v2 ${v2SessSet.size}`);

  // ---- 2) Hero 欢迎卡文案（用问候语 + 历史库状态） ----
  const hour = new Date().getHours();
  const greeting = hour < 6 ? '深夜好'
                 : hour < 12 ? '早上好'
                 : hour < 18 ? '下午好'
                 : '晚上好';
  setText('glance-hero-title', `${greeting} 👋`);

  const hs = state.historyStats;
  const subEl = document.getElementById('glance-hero-sub');
  if (subEl) {
    if (hs && hs.turns_count > 0) {
      const startDate = hs.earliest_ts ? fmtLocalDate(hs.earliest_ts) : '-';
      subEl.textContent =
        `本地历史库已累计 ${hs.turns_count.toLocaleString('zh-CN')} 条 turn，` +
        `起始 ${startDate}。Kiro 那边即使清了原始数据，你的历史都在这。`;
    } else {
      subEl.textContent =
        '本地历史库准备就绪。启动 Kiro 使用后，工具会自动把用量记录 snapshot 下来。';
    }
  }

  // ---- 3) chart-glance: 按日柱状图（当前时间范围） ----
  // echarts 异步加载中时先跳过图表（KPI/hero 上面已渲染），ready 后重绘
  if (typeof echarts === 'undefined') { setText('glance-chart-sub', '图表加载中…'); return; }
  const range = computeRange();
  const buckets = aggregateByGran(turns, 'day', range);
  const xData = buckets.map(b => b.key.slice(5));   // MM-DD
  const credits = buckets.map(b => +b.credits.toFixed(2));

  const accent = css('--accent') || '#8b5cf6';
  const accent2 = css('--accent-2') || '#3b82f6';
  const fgDim = css('--fg-dim') || '#a1a5b3';
  const border = css('--border-strong') || '#2e3341';

  const gradient = new echarts.graphic.LinearGradient(0, 0, 0, 1, [
    { offset: 0, color: accent },
    { offset: 1, color: accent2 },
  ]);

  const option = {
    animation: true,
    animationDuration: 500,
    grid: { top: 20, right: 20, bottom: 34, left: 50 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: css('--card') || '#12141c',
      borderColor: border,
      textStyle: { color: css('--fg') || '#e5e7eb' },
      axisPointer: { type: 'shadow', shadowStyle: { color: 'rgba(139,92,246,0.08)' } },
    },
    xAxis: {
      type: 'category',
      data: xData,
      axisLine: { lineStyle: { color: border } },
      axisTick: { show: false },
      axisLabel: { color: fgDim, fontSize: 11, hideOverlap: true },
    },
    yAxis: {
      type: 'value',
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: border, opacity: 0.3, type: 'dashed' } },
      axisLabel: { color: fgDim, fontSize: 11 },
    },
    series: [{
      name: 'Credits',
      type: 'bar',
      data: credits,
      itemStyle: { color: gradient, borderRadius: [6, 6, 0, 0] },
      emphasis: { itemStyle: { color: css('--bar-grad-hi') || '#a78bfa' } },
      barMaxWidth: 28,
    }],
  };

  const chart = chartOf(document.getElementById('chart-glance'), 'glance');
  chart.setOption(option, true);

  const rangeLabel = { today: '今日', week: '本周', month: '本月', '30d': '近 30 天', all: '全部' }[state.range] || state.range;
  setText('glance-chart-sub', `${rangeLabel} · 按日 · ${buckets.length} 天`);
}

// 总览视图的精简趋势图：按日聚合当前 range，只画 credits 一条柱，无叠加、无 dataZoom。
function renderTrendPreview(turns) {
  if (typeof echarts === 'undefined') return;  // echarts 异步加载中, ready 后会重绘
  const range = computeRange();
  const buckets = aggregateByGran(turns, 'day', range);
  const xData = buckets.map(b => b.key.slice(5)); // 只显示 MM-DD
  const credits = buckets.map(b => +b.credits.toFixed(2));

  const accent = css('--accent') || '#8b5cf6';
  const accent2 = css('--accent-2') || '#3b82f6';
  const fgDim = css('--fg-dim') || '#a1a5b3';
  const fgMute = css('--fg-mute') || '#6b7080';
  const border = css('--border-strong') || '#2e3341';

  const gradient = new echarts.graphic.LinearGradient(0, 0, 0, 1, [
    { offset: 0, color: accent },
    { offset: 1, color: accent2 },
  ]);

  const option = {
    animation: true,
    animationDuration: 400,
    grid: { top: 20, right: 20, bottom: 34, left: 50 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: css('--card') || '#12141c',
      borderColor: border,
      textStyle: { color: css('--fg') || '#e5e7eb' },
      axisPointer: { type: 'shadow', shadowStyle: { color: 'rgba(139,92,246,0.08)' } },
    },
    xAxis: {
      type: 'category',
      data: xData,
      axisLine: { lineStyle: { color: border } },
      axisTick: { show: false },
      axisLabel: { color: fgDim, fontSize: 11, hideOverlap: true },
    },
    yAxis: {
      type: 'value',
      nameTextStyle: { color: fgMute, fontSize: 11 },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: border, opacity: 0.3, type: 'dashed' } },
      axisLabel: { color: fgDim, fontSize: 11 },
    },
    series: [{
      name: 'Credits',
      type: 'bar',
      data: credits,
      itemStyle: { color: gradient, borderRadius: [3, 3, 0, 0] },
      emphasis: { itemStyle: { color: css('--bar-grad-hi') || '#a78bfa' } },
      barMaxWidth: 24,
    }],
  };

  const chart = chartOf(document.getElementById('chart-trend-preview'), 'trendPreview');
  chart.setOption(option, true);

  const rangeLabel = { today: '今日', week: '本周', month: '本月', '30d': '近 30 天', all: '全部' }[state.range];
  const sub = document.getElementById('trend-preview-sub');
  if (sub) sub.textContent = `按日 · ${rangeLabel} · ${buckets.length} 天`;
}

function updateFooter() {
  const s = state.scanStats;
  const s1 = state.scanV1Stats;
  const sa = state.scanAccountsStats;
  const foot1 = document.getElementById('footer-scan');
  const parts = [];
  if (s) parts.push(`v2: ${s.files} 文件 (${s.took_ms}ms)`);
  if (s1) parts.push(`v1: ${s1.files} sessions (${s1.took_ms}ms)`);
  if (sa) parts.push(`quota-log: ${sa.files} 文件/${state.accounts.length} 账号 (${sa.took_ms}ms)`);
  // v0.4.1: 首次数据还没到时显示"扫描中...", 而非空字符串或 "1970/1/1"
  foot1.textContent = parts.length ? parts.join('  ·  ') : '扫描中...';
  const srvEl = document.getElementById('footer-server');
  srvEl.textContent = state.lastServerTs
    ? `服务时间 ${new Date(state.lastServerTs).toLocaleString('zh-CN', { hour12: false })}`
    : '服务时间 加载中...';

  // v0.3+: 历史库状态
  const hs = state.historyStats;
  const foothist = document.getElementById('footer-history');
  if (hs && foothist) {
    const startDate = hs.earliest_ts ? fmtLocalDate(hs.earliest_ts) : null;
    const nfmt = (n) => Number(n || 0).toLocaleString('zh-CN');
    let text = `历史库: ${nfmt(hs.turns_count)} turn · ${nfmt(hs.v1_sessions_count)} v1 · ${nfmt(hs.quota_snapshots_count)} quota`;
    if (startDate) text += `（起始 ${startDate}）`;
    if (hs.last_upserted > 0) text += `  本次 +${hs.last_upserted}`;
    foothist.textContent = text;
    foothist.title = `历史库文件: ${hs.db_path}\n大小: ${(hs.db_size_bytes / 1024).toFixed(1)} KB`;
  }
}

// ============================================================
// 辅助
// ============================================================

function escapeHtml(s) {
  if (s == null) return '';
  return String(s).replace(/[&<>"']/g, c =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}
function escapeAttr(s) { return escapeHtml(s); }

// ============================================================
// 8. 启动 & 事件绑定
// ============================================================

function bindEvents() {
  // 粒度切换
  document.querySelectorAll('.seg-btn[data-gran]').forEach(b => {
    b.addEventListener('click', () => {
      state.gran = b.dataset.gran;
      document.querySelectorAll('.seg-btn[data-gran]').forEach(x =>
        x.classList.toggle('active', x === b));
      render();
    });
  });

  // 范围切换
  document.querySelectorAll('.seg-btn[data-range]').forEach(b => {
    b.addEventListener('click', () => {
      state.range = b.dataset.range;
      state.detailPage = 1;
      document.querySelectorAll('.seg-btn[data-range]').forEach(x =>
        x.classList.toggle('active', x === b));
      render();
    });
  });

  // 工具排序维度
  document.querySelectorAll('.seg-btn[data-tool-metric]').forEach(b => {
    b.addEventListener('click', () => {
      state.toolMetric = b.dataset.toolMetric;
      document.querySelectorAll('.seg-btn[data-tool-metric]').forEach(x =>
        x.classList.toggle('active', x === b));
      renderTools(filteredTurns());
    });
  });

  // 主图叠加控制
  document.getElementById('trend-show-turns').addEventListener('change', e => {
    state.showTurns = e.target.checked; renderTrend(filteredTurns());
  });
  document.getElementById('trend-show-elapsed').addEventListener('change', e => {
    state.showElapsed = e.target.checked; renderTrend(filteredTurns());
  });

  // 手动刷新
  document.getElementById('btn-refresh').addEventListener('click', () => fetchData(false));

  // 主题
  document.getElementById('btn-theme').addEventListener('click', () => {
    const cur = document.documentElement.getAttribute('data-theme') || 'dark';
    applyTheme(cur === 'dark' ? 'light' : 'dark');
  });

  // 明细搜索 / 状态 / workspace
  document.getElementById('detail-search').addEventListener('input', e => {
    state.detailSearch = e.target.value; state.detailPage = 1;
    renderDetail(filteredTurnsForDetail());
  });
  document.getElementById('detail-status').addEventListener('change', e => {
    state.detailStatus = e.target.value; state.detailPage = 1;
    renderDetail(filteredTurnsForDetail());
  });
  document.getElementById('detail-workspace').addEventListener('change', e => {
    state.detailWorkspace = e.target.value; state.detailPage = 1;
    renderDetail(filteredTurnsForDetail());
  });

  // 明细表头排序
  document.querySelectorAll('#detail-table thead th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const f = th.dataset.sort;
      if (state.detailSort.field === f) {
        state.detailSort.dir = state.detailSort.dir === 'asc' ? 'desc' : 'asc';
      } else {
        state.detailSort.field = f;
        state.detailSort.dir = (f === 't' || f === 'c' || f === 'e' || f === 'tools') ? 'desc' : 'asc';
      }
      renderDetail(filteredTurnsForDetail());
    });
  });

  // CSV 导出
  document.getElementById('btn-export-csv').addEventListener('click', exportCsv);

  // 清除筛选
  document.getElementById('filter-clear').addEventListener('click', clearClickFilter);

  // v1 sessions 表控件
  document.getElementById('v1-search').addEventListener('input', e => {
    state.v1Search = e.target.value; state.v1Page = 1; renderV1Sessions();
  });
  document.getElementById('v1-workspace-filter').addEventListener('change', e => {
    state.v1WorkspaceFilter = e.target.value; state.v1Page = 1; renderV1Sessions();
  });
  document.querySelectorAll('#v1-table thead th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const f = th.dataset.sort;
      if (state.v1Sort.field === f) {
        state.v1Sort.dir = state.v1Sort.dir === 'asc' ? 'desc' : 'asc';
      } else {
        state.v1Sort.field = f;
        state.v1Sort.dir = (f === 't' || f === 'turn_count') ? 'desc' : 'asc';
      }
      renderV1Sessions();
    });
  });

  // 明细视图内 v2/v1 tab 切换
  document.querySelectorAll('.tab-btn[data-tab]').forEach(b => {
    b.addEventListener('click', () => switchDetailTab(b.dataset.tab));
  });

  // v0.3+: 清除历史库按钮（二次确认）
  const btnClear = document.getElementById('btn-clear-history');
  if (btnClear) {
    btnClear.addEventListener('click', async () => {
      const hs = state.historyStats;
      if (!hs) {
        alert('历史库状态未加载完成，请稍后再试。');
        return;
      }
      const total = (hs.turns_count || 0) + (hs.v1_sessions_count || 0) + (hs.quota_snapshots_count || 0);
      if (total === 0) {
        alert('历史库当前为空，无需清除。');
        return;
      }
      const startDate = hs.earliest_ts ? fmtLocalDate(hs.earliest_ts) : '-';
      const ok = confirm(
        `确定要清除本地历史库吗？\n\n` +
        `将删除累计以下记录（起始 ${startDate}）：\n` +
        `  · ${hs.turns_count} 条 turn\n` +
        `  · ${hs.v1_sessions_count} 条 v1 session\n` +
        `  · ${hs.quota_snapshots_count} 条 quota 快照\n\n` +
        `注意：Kiro 原始数据不受影响；但 Kiro 那边若已丢失过部分历史（切账号覆盖、日志过期），\n` +
        `本地这些记录一旦清除将无法从 Kiro 重新恢复。\n\n` +
        `确定继续？`
      );
      if (!ok) return;
      try {
        const before = await invokeClearHistory();
        const cleared = (before.turns_count || 0) + (before.v1_sessions_count || 0) + (before.quota_snapshots_count || 0);
        await fetchData(false);
        alert(`已清除 ${cleared} 条历史记录。\n\n下次 Kiro 使用时工具会开始重新积累历史。`);
      } catch (e) {
        alert('清除失败: ' + (e?.message || e));
      }
    });
  }

  // 窗口 resize：所有 chart 调用 resize
  window.addEventListener('resize', () => {
    Object.values(state.charts).forEach(c => c && c.resize());
  });
}

// 明细专用的过滤（重跑 render 中的 dowHour 分支）
function filteredTurnsForDetail() {
  const rangeArr = filteredTurns();
  if (state.clickFilter && state.clickFilter.kind === 'dowHour') {
    const { dow, hr } = state.clickFilter;
    return rangeArr.filter(t => {
      const d = toLocalDate(t.t);
      return ((d.getUTCDay() || 7) - 1) === dow && d.getUTCHours() === hr;
    });
  }
  return rangeArr;
}

// v0.4.1: echarts 改为异步加载 —— 这是解决启动白屏的关键。
// 之前 echarts.min.js (1MB) 用 <script defer> 且排在 app.js 前, 导致 app.js 必须
// 等 1MB 脚本下载+parse 完 (实测占启动 9 秒) 才执行 → 骨架和数据都卡住不显示。
// 现在 app.js 立即执行渲染骨架+数据, echarts 在后台异步加载, 加载完再补绘图表。
let echartsLoading = false;
function loadEchartsAsync() {
  if (typeof echarts !== 'undefined' || echartsLoading) return;
  echartsLoading = true;
  const s = document.createElement('script');
  s.src = 'echarts.min.js';
  s.onload = () => {
    render();   // echarts ready, 重新渲染当前视图 (这次图表会画出来)
  };
  s.onerror = () => {
    echartsLoading = false;
    const le = document.getElementById('load-error');
    if (le) le.classList.remove('hidden');
    console.error('echarts.min.js 加载失败');
  };
  document.body.appendChild(s);
}

// v0.4.2: 隐藏启动遮罩 (淡出后 display:none)。幂等, 多次调用无害。
let splashHidden = false;
function hideSplash() {
  if (splashHidden) return;
  const el = document.getElementById('splash');
  if (!el) return;
  splashHidden = true;
  el.classList.add('hiding');
  setTimeout(() => el.classList.add('gone'), 500);
}

async function boot() {
  initTheme();
  bindEvents();

  // 兜底: 即使数据一直没来 (后端出错), 也在 6 秒后强制撤掉遮罩, 别把用户永久卡在加载页
  setTimeout(hideSplash, 6000);

  // 立即渲染骨架 + 数据 (不依赖 echarts; 图表函数检测到 echarts 未加载会自动跳过)
  initRouter();
  fetchData(false).catch(() => { /* 错误已在 fetchData 内 setLiveStatus */ });
  startAutoRefresh();

  // echarts 加载推迟到首帧绘制之后：先让骨架/数据画出来，再让主线程去 parse echarts (1MB)，
  // 避免 parse 阻塞首屏。双 rAF 确保首帧已提交。
  requestAnimationFrame(() => requestAnimationFrame(() => loadEchartsAsync()));
}

// 等 DOM & ECharts 就绪
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
