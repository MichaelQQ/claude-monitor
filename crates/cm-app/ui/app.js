const state = {
  activeSessionId: null,
  turnsBySession: new Map(), // session_id → [{ts,input,output,cacheR,cacheC}]
  latestSnapshot: null,
  chart: null,
  trendCostChart: null,
  trendTokensChart: null,
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

function setView(name) {
  document.querySelectorAll('nav button').forEach(b => b.classList.toggle('active', b.dataset.view === name));
  document.querySelectorAll('.view').forEach(v => v.classList.toggle('hidden', v.id !== 'view-' + name));
  if (name === 'sessions') loadSessions();
  if (name === 'trends') loadTrends();
  if (name === 'live') renderLiveChart();
}
document.querySelectorAll('nav button').forEach(b => b.addEventListener('click', () => setView(b.dataset.view)));

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

function renderLiveChart() {
  const arr = state.turnsBySession.get(state.activeSessionId) || [];
  const el = document.getElementById('live-chart');
  if (!el) return;
  if (arr.length === 0) { el.innerHTML = '<p style="color:var(--muted); padding:40px; text-align:center;">No turns yet. Fire up a Claude Code session.</p>'; if (state.chart) { state.chart.destroy(); state.chart = null; } return; }
  const xs = arr.map(t => t.ts);
  const cR = arr.map(t => t.cacheR);
  const cC = arr.map(t => t.cacheC);
  const inp = arr.map(t => t.input);
  const out = arr.map(t => t.output);
  const data = [xs, cR, cC, inp, out];
  const opts = {
    width: el.clientWidth, height: 280,
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
  if (state.chart) state.chart.destroy();
  state.chart = new uPlot(opts, data, el);
}

async function loadSessions() {
  const r = await fetch('/v1/sessions');
  const rows = await r.json();
  const tbody = document.querySelector('#sessions-table tbody');
  tbody.innerHTML = '';
  for (const s of rows) {
    const tr = document.createElement('tr');
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

function escapeHtml(s) { return String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }

window.addEventListener('resize', () => renderLiveChart());
connectWS();

// Bootstrap: load sessions so we have something even before the first event.
fetch('/v1/sessions').then(r => r.json()).then(rows => {
  if (rows.length && !state.activeSessionId) {
    state.activeSessionId = rows[0].session_id;
    loadTurnsForActive();
  }
});
