const state = {
  activeSessionId: null,
  turnsBySession: new Map(), // session_id → [{ts,input,output,cacheR,cacheC}]
  latestSnapshot: null,
  chart: null,
  trendCostChart: null,
  trendTokensChart: null,
  detailChart: null,
  detailSnapChart: null,
  detailSessionId: null,
  sessionsCache: null,
  sessionsSort: { key: 'last_seen_at', dir: 'desc' },
  hiddenCols: new Set(JSON.parse(localStorage.getItem('sessions-hidden-cols') || '[]')),
  sessionsRange: localStorage.getItem('sessions-range') || 'all',
  sessionsActivity: localStorage.getItem('sessions-activity') || 'all',
  sessionsModels: new Set(JSON.parse(localStorage.getItem('sessions-models') || '[]')),
  sessionsGroupByPath: localStorage.getItem('sessions-group-by-path') === '1',
  sessionsExpandedPaths: new Set(JSON.parse(localStorage.getItem('sessions-expanded-paths') || '[]')),
  quotaCaps: { five_hour: null, weekly: null },
  subagentsBySession: new Map(), // session_id → Map(task_id → row)
};

const $ = (s) => document.querySelector(s);
const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
function chartColors() {
  return { axis: cssVar('--muted') || '#8a93a6', grid: cssVar('--line') || '#242a36' };
}
const fmtInt = (n) => (n ?? 0).toLocaleString();
function projectName(dir) {
  if (!dir) return '';
  const parts = String(dir).split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : dir;
}
const fmtMoney = (n) => '$' + (n ?? 0).toFixed(2);
const fmtMoneyPrecise = (n) => n == null ? '—' : '$' + n.toFixed(4);
const fmtPct = (n) => n == null ? '—' : Math.round(n) + '%';
function fmtQuotaWithPct(tokens) {
  const base = fmtInt(tokens);
  const caps = state.quotaCaps || {};
  const parts = [];
  if (caps.five_hour && caps.five_hour > 0) parts.push(`${((tokens / caps.five_hour) * 100).toFixed(1)}% 5h`);
  if (caps.weekly    && caps.weekly    > 0) parts.push(`${((tokens / caps.weekly)    * 100).toFixed(1)}% weekly`);
  return parts.length ? `${base} <span class="muted-inline">(${parts.join(', ')})</span>` : base;
}
const fmtResets = (epoch) => {
  if (!epoch) return '—';
  const d = new Date(epoch * 1000);
  const dt = d - Date.now();
  if (dt <= 0) return 'now';
  const mins = Math.floor(dt / 60000);
  if (mins < 60) return `resets in ${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 48) return `resets in ${hrs}h ${mins % 60}m`;
  return 'resets ' + d.toLocaleDateString();
};

const VIEWS = ['live', 'sessions', 'trends', 'help', 'session'];

function parseRoute() {
  const h = location.hash.replace(/^#\/?/, '');
  if (!h) return { name: 'live' };
  const parts = h.split('/');
  if (parts[0] === 'session' && parts[1]) return { name: 'session', id: parts[1] };
  if (VIEWS.includes(parts[0])) return { name: parts[0] };
  return { name: 'live' };
}

function applyRoute() {
  const route = parseRoute();
  document.querySelectorAll('nav button').forEach(b => b.classList.toggle('active', b.dataset.view === route.name));
  document.querySelectorAll('.view').forEach(v => v.classList.toggle('hidden', v.id !== 'view-' + route.name));
  if (route.name === 'sessions') loadSessions();
  else if (route.name === 'trends') loadTrends();
  else if (route.name === 'live') renderLiveChart();
  else if (route.name === 'session') loadSessionDetail(route.id);
}
document.querySelectorAll('nav button').forEach(b => b.addEventListener('click', () => { location.hash = '#/' + b.dataset.view; }));
window.addEventListener('hashchange', applyRoute);

function connectWS() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/v1/live`);
  ws.onopen = () => { $('#conn').textContent = 'connected'; $('#conn').className = 'pill pill-on'; };
  ws.onclose = () => { $('#conn').textContent = 'disconnected'; $('#conn').className = 'pill pill-off'; setTimeout(connectWS, 2000); };
  ws.onmessage = (e) => handleEvent(JSON.parse(e.data));
}

function handleEvent(ev) {
  if (ev.kind === 'snapshot') {
    state.latestSnapshot = ev;
    state.activeSessionId = ev.session_id;
    renderLive();
  } else if (ev.kind === 'turn') {
    const { session_id, ts_ms, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens } = ev;
    const arr = state.turnsBySession.get(session_id) || [];
    arr.push({ ts: ts_ms / 1000, input: input_tokens, output: output_tokens, cacheC: cache_creation_input_tokens, cacheR: cache_read_input_tokens });
    state.turnsBySession.set(session_id, arr);
    if (session_id === state.activeSessionId || !state.activeSessionId) {
      state.activeSessionId = state.activeSessionId || session_id;
      renderLiveChart();
    }
    if (state.detailSessionId === session_id) renderDetailChart(arr);
  } else if (ev.kind === 'subagent_snapshot') {
    const { session_id, ts_ms, tasks } = ev;
    const map = state.subagentsBySession.get(session_id) || new Map();
    for (const t of tasks || []) {
      const prev = map.get(t.id) || {};
      map.set(t.id, {
        task_id: t.id,
        name: t.name ?? prev.name ?? null,
        task_type: t.type ?? prev.task_type ?? null,
        status: t.status ?? prev.status ?? null,
        description: t.description ?? prev.description ?? null,
        label: t.label ?? prev.label ?? null,
        start_time: t.startTime ?? prev.start_time ?? null,
        token_count: t.tokenCount ?? prev.token_count ?? null,
        cwd: t.cwd ?? prev.cwd ?? null,
        first_seen_at: prev.first_seen_at ?? ts_ms,
        last_seen_at: ts_ms,
      });
    }
    state.subagentsBySession.set(session_id, map);
    if (state.detailSessionId === session_id) renderSubagents(Array.from(map.values()));
  }
}

async function loadTurnsForActive() {
  if (!state.activeSessionId) return;
  const r = await fetch(`/v1/sessions/${state.activeSessionId}/turns`);
  const rows = await r.json();
  state.turnsBySession.set(state.activeSessionId, rows.map(t => ({
    ts: t.ts / 1000,
    input: t.input_tokens,
    output: t.output_tokens,
    cacheC: t.cache_creation_input_tokens,
    cacheR: t.cache_read_input_tokens,
  })));
  renderLiveChart();
}

function renderLive() {
  const s = state.latestSnapshot;
  if (!s) return;
  $('#live-session').textContent = (s.model && s.model.display_name) || '—';
  $('#live-project').textContent = (s.workspace && s.workspace.project_dir) || s.session_id;
  const pct = s.context_window && s.context_window.used_percentage;
  $('#live-ctx').textContent = fmtPct(pct);
  const bar = $('#live-ctx-bar');
  bar.style.width = Math.max(0, Math.min(100, pct || 0)) + '%';
  bar.classList.remove('warn', 'bad');
  if (pct >= 90) bar.classList.add('bad'); else if (pct >= 70) bar.classList.add('warn');
  const cost = s.cost && s.cost.total_cost_usd;
  $('#live-cost').textContent = fmtMoney(cost);
  const durMs = (s.cost && s.cost.total_duration_ms) || 0;
  const mins = Math.floor(durMs / 60000), secs = Math.floor((durMs % 60000) / 1000);
  $('#live-cost-sub').textContent = `${mins}m ${secs}s`;
  const f = s.rate_limits && s.rate_limits.five_hour;
  $('#live-5h').textContent = fmtPct(f && f.used_percentage);
  $('#live-5h-sub').textContent = f ? fmtResets(f.resets_at) : '—';
  const w = s.rate_limits && s.rate_limits.seven_day;
  $('#live-7d').textContent = fmtPct(w && w.used_percentage);
  $('#live-7d-sub').textContent = w ? fmtResets(w.resets_at) : '—';
  if (!state.turnsBySession.has(state.activeSessionId)) loadTurnsForActive();
}

function stackedTurnOpts(width) {
  const { axis, grid } = chartColors();
  return {
    width, height: 280,
    scales: { x: { time: true } },
    axes: [
      { stroke: axis, grid: { stroke: grid } },
      { stroke: axis, grid: { stroke: grid }, values: (u, vals) => vals.map(v => v >= 1000 ? (v/1000).toFixed(1)+'k' : v) },
    ],
    series: [
      {},
      { label: 'cache read',   stroke: '#6aa9ff', width: 2, fill: 'rgba(106,169,255,0.15)' },
      { label: 'cache create', stroke: '#ffc36a', width: 2, fill: 'rgba(255,195,106,0.15)' },
      { label: 'input',        stroke: '#8be28b', width: 2 },
      { label: 'output',       stroke: '#ff7b7b', width: 2 },
    ],
    legend: { show: false },
  };
}

function turnsToChartData(arr) {
  const xs = arr.map(t => t.ts);
  return [xs, arr.map(t => t.cacheR), arr.map(t => t.cacheC), arr.map(t => t.input), arr.map(t => t.output)];
}

function renderLiveChart() {
  const arr = state.turnsBySession.get(state.activeSessionId) || [];
  const el = document.getElementById('live-chart');
  if (!el) return;
  if (arr.length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No turns yet. Fire up a Claude Code session.</p>'; if (state.chart) { state.chart.destroy(); state.chart = null; } return; }
  if (state.chart) state.chart.destroy();
  state.chart = new uPlot(stackedTurnOpts(el.clientWidth), turnsToChartData(arr), el);
}

async function loadSessions() {
  const [sessRes, capsRes] = await Promise.all([
    fetch('/v1/sessions'),
    fetch('/v1/quota-caps'),
  ]);
  state.sessionsCache = await sessRes.json();
  if (capsRes.ok) state.quotaCaps = await capsRes.json();
  refreshModelFilterOptions();
  renderSessionsTable();
}

const RANGE_MS = { '24h': 86400e3, '7d': 7 * 86400e3, '30d': 30 * 86400e3 };
const ACTIVE_THRESHOLD_MS = 5 * 60 * 1000;

function refreshModelFilterOptions() {
  const menu = document.getElementById('sessions-model-menu');
  const btn = document.getElementById('sessions-model-btn');
  if (!menu || !btn) return;
  const models = Array.from(new Set((state.sessionsCache || []).map(s => s.model_id).filter(Boolean))).sort();
  // Prune selections that no longer exist in the current session set.
  for (const m of [...state.sessionsModels]) if (!models.includes(m)) state.sessionsModels.delete(m);
  menu.innerHTML = '';
  if (models.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'col-pick';
    empty.style.color = 'var(--muted)';
    empty.textContent = 'No models yet';
    menu.appendChild(empty);
  }
  for (const m of models) {
    const row = document.createElement('label');
    row.className = 'col-pick';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = state.sessionsModels.has(m);
    cb.addEventListener('change', () => {
      if (cb.checked) state.sessionsModels.add(m);
      else state.sessionsModels.delete(m);
      localStorage.setItem('sessions-models', JSON.stringify([...state.sessionsModels]));
      updateModelBtnLabel();
      renderSessionsTable();
    });
    row.appendChild(cb);
    row.appendChild(document.createTextNode(' ' + m));
    menu.appendChild(row);
  }
  updateModelBtnLabel();
}

function updateModelBtnLabel() {
  const btn = document.getElementById('sessions-model-btn');
  if (!btn) return;
  const n = state.sessionsModels.size;
  btn.textContent = (n === 0 ? 'All models' : `${n} model${n === 1 ? '' : 's'}`) + ' ▾';
}

function sessionCellsHtml(s) {
  return `
      <td><code>${s.session_id.slice(0,8)}</code></td>
      <td>${escapeHtml(projectName(s.project_dir) || '—')}</td>
      <td title="${escapeHtml(s.project_dir || '')}">${escapeHtml(s.project_dir || '—')}</td>
      <td>${escapeHtml(s.model_id || '—')}</td>
      <td class="num">${fmtInt(s.total_turns)}</td>
      <td class="num">${fmtInt(s.total_input_tokens)}</td>
      <td class="num">${fmtInt(s.total_output_tokens)}</td>
      <td class="num">${fmtInt(s.total_cache_read)}</td>
      <td class="num">${fmtInt(s.total_cache_write_5m)}</td>
      <td class="num">${fmtInt(s.total_cache_write_1h)}</td>
      <td class="num">${fmtQuotaWithPct(s.quota_tokens)}</td>
      <td class="num">${s.last_cost_usd != null ? fmtMoney(s.last_cost_usd) : '—'}</td>
      <td class="num">${fmtMoneyPrecise(s.estimated_cost_usd)}</td>
      <td>${new Date(s.last_seen_at).toLocaleString()}</td>`;
}

function aggregateGroup(sessions) {
  const sum = (k) => sessions.reduce((a, s) => a + (s[k] || 0), 0);
  const sumOrNull = (k) => {
    let any = false, total = 0;
    for (const s of sessions) if (s[k] != null) { any = true; total += s[k]; }
    return any ? total : null;
  };
  const models = new Set(sessions.map(s => s.model_id).filter(Boolean));
  return {
    session_count: sessions.length,
    project_dir: sessions[0].project_dir,
    model_id: models.size === 1 ? [...models][0] : (models.size > 1 ? 'mixed' : null),
    total_turns: sum('total_turns'),
    total_input_tokens: sum('total_input_tokens'),
    total_output_tokens: sum('total_output_tokens'),
    total_cache_read: sum('total_cache_read'),
    total_cache_write_5m: sum('total_cache_write_5m'),
    total_cache_write_1h: sum('total_cache_write_1h'),
    quota_tokens: sum('quota_tokens'),
    last_cost_usd: sumOrNull('last_cost_usd'),
    estimated_cost_usd: sumOrNull('estimated_cost_usd'),
    last_seen_at: sessions.reduce((m, s) => {
      const t = new Date(s.last_seen_at).getTime();
      return t > m ? t : m;
    }, 0),
  };
}

function groupCellsHtml(g, expanded) {
  const chev = expanded ? '▾' : '▸';
  const count = g.session_count === 1 ? '1 session' : `${g.session_count} sessions`;
  return `
      <td><span class="group-chev">${chev}</span> ${escapeHtml(count)}</td>
      <td>${escapeHtml(projectName(g.project_dir) || '—')}</td>
      <td title="${escapeHtml(g.project_dir || '')}">${escapeHtml(g.project_dir || '—')}</td>
      <td>${escapeHtml(g.model_id || '—')}</td>
      <td class="num">${fmtInt(g.total_turns)}</td>
      <td class="num">${fmtInt(g.total_input_tokens)}</td>
      <td class="num">${fmtInt(g.total_output_tokens)}</td>
      <td class="num">${fmtInt(g.total_cache_read)}</td>
      <td class="num">${fmtInt(g.total_cache_write_5m)}</td>
      <td class="num">${fmtInt(g.total_cache_write_1h)}</td>
      <td class="num">${fmtQuotaWithPct(g.quota_tokens)}</td>
      <td class="num">${g.last_cost_usd != null ? fmtMoney(g.last_cost_usd) : '—'}</td>
      <td class="num">${fmtMoneyPrecise(g.estimated_cost_usd)}</td>
      <td>${g.last_seen_at ? new Date(g.last_seen_at).toLocaleString() : '—'}</td>`;
}

function renderSessionsTable() {
  const all = state.sessionsCache || [];
  const cutoff = RANGE_MS[state.sessionsRange] ? Date.now() - RANGE_MS[state.sessionsRange] : null;
  const activeCutoff = Date.now() - ACTIVE_THRESHOLD_MS;
  const models = state.sessionsModels;
  const activity = state.sessionsActivity;
  const rows = all.filter(s => {
    const ts = new Date(s.last_seen_at).getTime();
    if (cutoff != null && ts < cutoff) return false;
    if (models.size > 0 && !models.has(s.model_id)) return false;
    if (activity === 'active' && ts < activeCutoff) return false;
    if (activity === 'inactive' && ts >= activeCutoff) return false;
    return true;
  });
  const { key, dir } = state.sessionsSort;
  const mult = dir === 'asc' ? 1 : -1;
  const sortVal = (row) => {
    if (key === 'project_name') return projectName(row.project_dir);
    if (key === 'last_seen_at' && typeof row[key] !== 'number') return new Date(row[key]).getTime();
    return row[key];
  };
  const cmp = (a, b) => {
    const av = sortVal(a), bv = sortVal(b);
    if (av == null && bv == null) return 0;
    if (av == null) return 1;
    if (bv == null) return -1;
    if (typeof av === 'number' && typeof bv === 'number') return (av - bv) * mult;
    return String(av).localeCompare(String(bv)) * mult;
  };
  document.querySelectorAll('#sessions-table th[data-sort]').forEach(th => {
    th.classList.toggle('sort-asc', th.dataset.sort === key && dir === 'asc');
    th.classList.toggle('sort-desc', th.dataset.sort === key && dir === 'desc');
  });
  const tbody = document.querySelector('#sessions-table tbody');
  tbody.innerHTML = '';

  if (state.sessionsGroupByPath) {
    const groups = new Map();
    for (const s of rows) {
      const k = s.project_dir || '';
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(s);
    }
    const entries = [...groups.entries()].map(([path, sessions]) => ({
      path,
      sessions: [...sessions].sort(cmp),
      agg: aggregateGroup(sessions),
    }));
    entries.sort((a, b) => cmp(a.agg, b.agg));
    for (const g of entries) {
      const expanded = state.sessionsExpandedPaths.has(g.path);
      const gr = document.createElement('tr');
      gr.className = 'group-row clickable';
      gr.innerHTML = groupCellsHtml(g.agg, expanded);
      gr.addEventListener('click', () => {
        if (expanded) state.sessionsExpandedPaths.delete(g.path);
        else state.sessionsExpandedPaths.add(g.path);
        localStorage.setItem('sessions-expanded-paths', JSON.stringify([...state.sessionsExpandedPaths]));
        renderSessionsTable();
      });
      tbody.appendChild(gr);
      if (expanded) {
        for (const s of g.sessions) {
          const tr = document.createElement('tr');
          tr.className = 'clickable group-child';
          tr.addEventListener('click', () => { location.hash = '#/session/' + s.session_id; });
          tr.innerHTML = sessionCellsHtml(s);
          tbody.appendChild(tr);
        }
      }
    }
  } else {
    const sorted = [...rows].sort(cmp);
    for (const s of sorted) {
      const tr = document.createElement('tr');
      tr.className = 'clickable';
      tr.addEventListener('click', () => { location.hash = '#/session/' + s.session_id; });
      tr.innerHTML = sessionCellsHtml(s);
      tbody.appendChild(tr);
    }
  }
  applyHiddenCols();
}

function applyHiddenCols() {
  const table = document.querySelector('#sessions-table');
  if (!table) return;
  const ths = table.querySelectorAll('thead th');
  const hiddenIdx = [];
  ths.forEach((th, i) => {
    const h = state.hiddenCols.has(th.dataset.sort);
    th.classList.toggle('col-hidden', h);
    if (h) hiddenIdx.push(i);
  });
  table.querySelectorAll('tbody tr').forEach(tr => {
    Array.from(tr.children).forEach((td, i) => {
      td.classList.toggle('col-hidden', hiddenIdx.includes(i));
    });
  });
}

function initColumnPicker() {
  const menu = document.getElementById('sessions-columns-menu');
  const btn = document.getElementById('sessions-columns-btn');
  if (!menu || !btn) return;
  const ths = document.querySelectorAll('#sessions-table thead th');
  ths.forEach(th => {
    const key = th.dataset.sort;
    const label = (th.firstChild && th.firstChild.nodeType === 3 ? th.firstChild.nodeValue : th.textContent).trim();
    const row = document.createElement('label');
    row.className = 'col-pick';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = !state.hiddenCols.has(key);
    cb.addEventListener('change', () => {
      if (cb.checked) state.hiddenCols.delete(key);
      else state.hiddenCols.add(key);
      localStorage.setItem('sessions-hidden-cols', JSON.stringify([...state.hiddenCols]));
      applyHiddenCols();
    });
    row.appendChild(cb);
    row.appendChild(document.createTextNode(' ' + label));
    menu.appendChild(row);
  });
  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    menu.classList.toggle('hidden');
  });
  document.addEventListener('click', (e) => {
    if (!menu.contains(e.target) && e.target !== btn) menu.classList.add('hidden');
  });
}
initColumnPicker();
applyHiddenCols();

function initSessionFilters() {
  const range = document.getElementById('sessions-range');
  if (range) {
    range.value = state.sessionsRange;
    range.addEventListener('change', () => {
      state.sessionsRange = range.value;
      localStorage.setItem('sessions-range', range.value);
      renderSessionsTable();
    });
  }
  const activity = document.getElementById('sessions-activity');
  if (activity) {
    activity.value = state.sessionsActivity;
    activity.addEventListener('change', () => {
      state.sessionsActivity = activity.value;
      localStorage.setItem('sessions-activity', activity.value);
      renderSessionsTable();
    });
  }
  const btn = document.getElementById('sessions-model-btn');
  const menu = document.getElementById('sessions-model-menu');
  if (btn && menu) {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      menu.classList.toggle('hidden');
    });
    document.addEventListener('click', (e) => {
      if (!menu.contains(e.target) && e.target !== btn) menu.classList.add('hidden');
    });
    updateModelBtnLabel();
  }
  const group = document.getElementById('sessions-group-by-path');
  if (group) {
    group.checked = state.sessionsGroupByPath;
    group.addEventListener('change', () => {
      state.sessionsGroupByPath = group.checked;
      localStorage.setItem('sessions-group-by-path', group.checked ? '1' : '0');
      renderSessionsTable();
    });
  }
}
initSessionFilters();

document.querySelectorAll('#sessions-table th[data-sort]').forEach(th => {
  th.addEventListener('click', () => {
    const key = th.dataset.sort;
    if (state.sessionsSort.key === key) {
      state.sessionsSort.dir = state.sessionsSort.dir === 'asc' ? 'desc' : 'asc';
    } else {
      state.sessionsSort.key = key;
      state.sessionsSort.dir = (key === 'project_name' || key === 'project_dir' || key === 'model_id' || key === 'session_id') ? 'asc' : 'desc';
    }
    renderSessionsTable();
  });
});

function makeColumnsResizable(tableSelector) {
  const table = document.querySelector(tableSelector);
  if (!table) return;
  let locked = false;
  const lockWidths = () => {
    if (locked) return;
    table.querySelectorAll('thead th').forEach(th => { th.style.width = th.offsetWidth + 'px'; });
    table.style.tableLayout = 'fixed';
    locked = true;
  };
  table.querySelectorAll('thead th').forEach(th => {
    const handle = document.createElement('span');
    handle.className = 'col-resizer';
    handle.addEventListener('click', e => e.stopPropagation());
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      lockWidths();
      const startX = e.clientX;
      const startW = th.offsetWidth;
      handle.classList.add('dragging');
      table.classList.add('resizing');
      const onMove = (ev) => {
        const w = Math.max(40, startW + ev.clientX - startX);
        th.style.width = w + 'px';
      };
      const onUp = () => {
        handle.classList.remove('dragging');
        table.classList.remove('resizing');
        window.removeEventListener('mousemove', onMove);
        window.removeEventListener('mouseup', onUp);
      };
      window.addEventListener('mousemove', onMove);
      window.addEventListener('mouseup', onUp);
    });
    th.appendChild(handle);
  });
}
makeColumnsResizable('#sessions-table');

async function loadTrends() {
  const r = await fetch('/v1/trends?window=day');
  const rows = await r.json();
  const days = rows.map(d => d.ts);
  const costs = rows.map(d => d.total_cost_usd || 0);
  const tokens = rows.map(d => d.total_tokens || 0);
  drawLine('trend-cost', [days, costs], 'cost', '#ffc36a', state, 'trendCostChart');
  drawLine('trend-tokens', [days, tokens], 'tokens', '#6aa9ff', state, 'trendTokensChart');
}

function drawLine(elId, data, label, color, store, key) {
  const el = document.getElementById(elId);
  if (!el) return;
  if (data[0].length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No data yet.</p>'; return; }
  const { axis, grid } = chartColors();
  const opts = {
    width: el.clientWidth, height: 240,
    scales: { x: { time: true } },
    axes: [{ stroke: axis, grid: { stroke: grid } }, { stroke: axis, grid: { stroke: grid } }],
    series: [{}, { label, stroke: color, width: 2, fill: color + '33' }],
    legend: { show: false },
  };
  if (store[key]) store[key].destroy();
  store[key] = new uPlot(opts, data, el);
}

async function loadSessionDetail(id) {
  state.detailSessionId = id;
  let meta = state.sessionsCache && state.sessionsCache.find(s => s.session_id === id);
  if (!meta) {
    const r = await fetch('/v1/sessions');
    state.sessionsCache = await r.json();
    meta = state.sessionsCache.find(s => s.session_id === id);
  }
  if (meta) {
    $('#detail-title').textContent = meta.project_dir || id;
    $('#detail-sub').textContent = `${meta.session_id} · ${meta.model_id || 'unknown model'} · last seen ${new Date(meta.last_seen_at).toLocaleString()}`;
    $('#detail-turns').textContent = fmtInt(meta.total_turns);
    const totalTok = (meta.total_input_tokens || 0) + (meta.total_output_tokens || 0) + (meta.total_cache_read || 0) + (meta.total_cache_creation || 0);
    $('#detail-tokens').textContent = fmtInt(totalTok);
    $('#detail-tokens-sub').textContent = `in ${fmtInt(meta.total_input_tokens)} · out ${fmtInt(meta.total_output_tokens)} · cR ${fmtInt(meta.total_cache_read)} · cC ${fmtInt(meta.total_cache_creation)}`;
    $('#detail-cost-snap').textContent = meta.last_cost_usd != null ? fmtMoney(meta.last_cost_usd) : '—';
    $('#detail-cost-est').textContent = fmtMoneyPrecise(meta.estimated_cost_usd);
  } else {
    $('#detail-title').textContent = id;
    $('#detail-sub').textContent = '—';
  }

  const [turnsR, snapsR, subsR] = await Promise.all([
    fetch(`/v1/sessions/${id}/turns`).then(r => r.json()),
    fetch(`/v1/sessions/${id}/snapshots`).then(r => r.json()),
    fetch(`/v1/sessions/${id}/subagents`).then(r => r.json()),
  ]);
  const turns = turnsR.map(t => ({
    ts: t.ts / 1000,
    input: t.input_tokens,
    output: t.output_tokens,
    cacheC: t.cache_creation_input_tokens,
    cacheR: t.cache_read_input_tokens,
  }));
  state.turnsBySession.set(id, turns);
  const subMap = new Map();
  for (const s of subsR) subMap.set(s.task_id, s);
  state.subagentsBySession.set(id, subMap);
  renderDetailChart(turns);
  renderDetailSnapshotChart(snapsR);
  renderSubagents(subsR);
}

function renderDetailChart(arr) {
  const el = document.getElementById('detail-chart');
  if (!el) return;
  if (arr.length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No turns recorded.</p>'; if (state.detailChart) { state.detailChart.destroy(); state.detailChart = null; } return; }
  if (state.detailChart) state.detailChart.destroy();
  state.detailChart = new uPlot(stackedTurnOpts(el.clientWidth), turnsToChartData(arr), el);
}

function renderDetailSnapshotChart(rows) {
  const el = document.getElementById('detail-snap-chart');
  if (!el) return;
  if (!rows || rows.length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No snapshots recorded (statusline never ran for this session).</p>'; if (state.detailSnapChart) { state.detailSnapChart.destroy(); state.detailSnapChart = null; } return; }
  const xs = rows.map(r => r.ts / 1000);
  const cost = rows.map(r => r.total_cost_usd);
  const ctx = rows.map(r => r.context_used_pct);
  const five = rows.map(r => r.five_hour_pct);
  const { axis, grid } = chartColors();
  const opts = {
    width: el.clientWidth, height: 240,
    scales: {
      x: { time: true },
      cost: {},
      pct: { range: [0, 100] },
    },
    axes: [
      { stroke: axis, grid: { stroke: grid } },
      { scale: 'cost', stroke: '#ffc36a', grid: { stroke: grid }, values: (u, vals) => vals.map(v => '$' + v.toFixed(2)) },
      { scale: 'pct', side: 1, stroke: '#6aa9ff', grid: { show: false }, values: (u, vals) => vals.map(v => v + '%') },
    ],
    series: [
      {},
      { label: 'cost', scale: 'cost', stroke: '#ffc36a', width: 2, fill: 'rgba(255,195,106,0.15)' },
      { label: 'context %', scale: 'pct', stroke: '#6aa9ff', width: 2 },
      { label: '5h %', scale: 'pct', stroke: '#ff7b7b', width: 2, dash: [4, 4] },
    ],
    legend: { show: false },
  };
  if (state.detailSnapChart) state.detailSnapChart.destroy();
  state.detailSnapChart = new uPlot(opts, [xs, cost, ctx, five], el);
}

function renderSubagents(rows) {
  const tbody = document.querySelector('#detail-subagents-table tbody');
  const empty = document.getElementById('detail-subagents-empty');
  if (!tbody) return;
  tbody.innerHTML = '';
  if (!rows || rows.length === 0) {
    empty.style.display = '';
    return;
  }
  empty.style.display = 'none';
  const sorted = [...rows].sort((a, b) => (a.first_seen_at || 0) - (b.first_seen_at || 0));
  for (const s of sorted) {
    const tr = document.createElement('tr');
    const elapsed = s.start_time ? fmtElapsed(Date.now() / 1000 - s.start_time) : '—';
    tr.innerHTML = `
      <td><code>${escapeHtml(String(s.task_id).slice(0, 8))}</code></td>
      <td>${escapeHtml(s.name || s.label || '—')}</td>
      <td>${escapeHtml(s.status || '—')}</td>
      <td class="num">${s.token_count != null ? fmtInt(s.token_count) : '—'}</td>
      <td class="num">${elapsed}</td>
      <td>${escapeHtml(s.cwd || '—')}</td>
      <td>${new Date(s.last_seen_at).toLocaleTimeString()}</td>`;
    tbody.appendChild(tr);
  }
}

function fmtElapsed(seconds) {
  if (!seconds || seconds < 0) return '—';
  const s = Math.floor(seconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function escapeHtml(s) { return String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }

window.addEventListener('resize', () => {
  renderLiveChart();
  if (state.detailSessionId) {
    const arr = state.turnsBySession.get(state.detailSessionId) || [];
    renderDetailChart(arr);
  }
});
function initThemeToggle() {
  const btn = document.getElementById('theme-toggle');
  if (!btn) return;
  const render = () => {
    const t = document.documentElement.getAttribute('data-theme') || 'dark';
    btn.textContent = t === 'dark' ? '☾' : '☀';
  };
  render();
  btn.addEventListener('click', () => {
    const cur = document.documentElement.getAttribute('data-theme') || 'dark';
    const next = cur === 'dark' ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', next);
    localStorage.setItem('theme', next);
    render();
    renderLiveChart();
    const route = parseRoute();
    if (route.name === 'session' && state.detailSessionId) loadSessionDetail(state.detailSessionId);
    if (route.name === 'trends') loadTrends();
  });
}
initThemeToggle();

connectWS();
applyRoute();

// Bootstrap: load sessions so we have something even before the first event.
fetch('/v1/sessions').then(r => r.json()).then(rows => {
  state.sessionsCache = rows;
  if (rows.length && !state.activeSessionId) {
    state.activeSessionId = rows[0].session_id;
    loadTurnsForActive();
  }
});
