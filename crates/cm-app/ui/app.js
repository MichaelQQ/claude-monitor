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
};

const $ = (s) => document.querySelector(s);
const fmtInt = (n) => (n ?? 0).toLocaleString();
const fmtMoney = (n) => '$' + (n ?? 0).toFixed(2);
const fmtMoneyPrecise = (n) => n == null ? '—' : '$' + n.toFixed(4);
const fmtPct = (n) => n == null ? '—' : Math.round(n) + '%';
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

const VIEWS = ['live', 'sessions', 'trends', 'session'];

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
  return {
    width, height: 280,
    scales: { x: { time: true } },
    axes: [
      { stroke: '#8a93a6', grid: { stroke: '#242a36' } },
      { stroke: '#8a93a6', grid: { stroke: '#242a36' }, values: (u, vals) => vals.map(v => v >= 1000 ? (v/1000).toFixed(1)+'k' : v) },
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
  const r = await fetch('/v1/sessions');
  const rows = await r.json();
  state.sessionsCache = rows;
  const tbody = document.querySelector('#sessions-table tbody');
  tbody.innerHTML = '';
  for (const s of rows) {
    const tr = document.createElement('tr');
    tr.className = 'clickable';
    tr.addEventListener('click', () => { location.hash = '#/session/' + s.session_id; });
    tr.innerHTML = `
      <td><code>${s.session_id.slice(0,8)}</code></td>
      <td>${escapeHtml(s.project_dir || '—')}</td>
      <td>${escapeHtml(s.model_id || '—')}</td>
      <td class="num">${fmtInt(s.total_turns)}</td>
      <td class="num">${fmtInt(s.total_input_tokens)}</td>
      <td class="num">${fmtInt(s.total_output_tokens)}</td>
      <td class="num">${fmtInt(s.total_cache_read)}</td>
      <td class="num">${fmtInt(s.total_cache_creation)}</td>
      <td class="num">${s.last_cost_usd != null ? fmtMoney(s.last_cost_usd) : '—'}</td>
      <td class="num">${fmtMoneyPrecise(s.estimated_cost_usd)}</td>
      <td>${new Date(s.last_seen_at).toLocaleString()}</td>`;
    tbody.appendChild(tr);
  }
}

async function loadTrends() {
  const r = await fetch('/v1/sessions');
  const rows = await r.json();
  // Aggregate per UTC day.
  const byDay = new Map();
  for (const s of rows) {
    const day = Math.floor(s.last_seen_at / 86400000) * 86400;
    const cur = byDay.get(day) || { cost: 0, tokens: 0 };
    cur.cost += s.last_cost_usd || 0;
    cur.tokens += (s.total_input_tokens || 0) + (s.total_output_tokens || 0) + (s.total_cache_read || 0) + (s.total_cache_creation || 0);
    byDay.set(day, cur);
  }
  const days = [...byDay.keys()].sort();
  const costs = days.map(d => byDay.get(d).cost);
  const tokens = days.map(d => byDay.get(d).tokens);
  drawLine('trend-cost', [days, costs], 'cost', '#ffc36a', state, 'trendCostChart');
  drawLine('trend-tokens', [days, tokens], 'tokens', '#6aa9ff', state, 'trendTokensChart');
}

function drawLine(elId, data, label, color, store, key) {
  const el = document.getElementById(elId);
  if (!el) return;
  if (data[0].length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No data yet.</p>'; return; }
  const opts = {
    width: el.clientWidth, height: 240,
    scales: { x: { time: true } },
    axes: [{ stroke: '#8a93a6', grid: { stroke: '#242a36' } }, { stroke: '#8a93a6', grid: { stroke: '#242a36' } }],
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

  const [turnsR, snapsR] = await Promise.all([
    fetch(`/v1/sessions/${id}/turns`).then(r => r.json()),
    fetch(`/v1/sessions/${id}/snapshots`).then(r => r.json()),
  ]);
  const turns = turnsR.map(t => ({
    ts: t.ts / 1000,
    input: t.input_tokens,
    output: t.output_tokens,
    cacheC: t.cache_creation_input_tokens,
    cacheR: t.cache_read_input_tokens,
  }));
  state.turnsBySession.set(id, turns);
  renderDetailChart(turns);
  renderDetailSnapshotChart(snapsR);
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
  const opts = {
    width: el.clientWidth, height: 240,
    scales: {
      x: { time: true },
      cost: {},
      pct: { range: [0, 100] },
    },
    axes: [
      { stroke: '#8a93a6', grid: { stroke: '#242a36' } },
      { scale: 'cost', stroke: '#ffc36a', grid: { stroke: '#242a36' }, values: (u, vals) => vals.map(v => '$' + v.toFixed(2)) },
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

function escapeHtml(s) { return String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }

window.addEventListener('resize', () => {
  renderLiveChart();
  if (state.detailSessionId) {
    const arr = state.turnsBySession.get(state.detailSessionId) || [];
    renderDetailChart(arr);
  }
});
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
