//! Real-time web dashboard for Wireclaw.
//!
//! Serves a web UI at `http://127.0.0.1:8746` that shows live traffic,
//! request details, replay controls, and performance metrics.

use anyhow::Result;
use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
};
use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};
use tower_http::cors::CorsLayer;

use crate::db;
use crate::models::Exchange;
use crate::openapi;
use crate::perf;

/// Events sent over WebSocket to the dashboard.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum DashboardEvent {
    Exchange { exchange: Box<Exchange> },
}

/// Shared application state for the dashboard.
#[derive(Clone)]
pub struct DashboardState {
    pub pool: SqlitePool,
    pub session: String,
    pub tx: broadcast::Sender<DashboardEvent>,
}

/// Dashboard server.
pub struct DashboardServer {
    state: Arc<DashboardState>,
}

impl DashboardServer {
    pub fn new(pool: SqlitePool, session: String) -> Self {
        let (tx, _rx) = broadcast::channel::<DashboardEvent>(1024);
        let state = Arc::new(DashboardState { pool, session, tx });
        Self { state }
    }

    pub fn sender(&self) -> broadcast::Sender<DashboardEvent> {
        self.state.tx.clone()
    }

    pub async fn run(self, addr: &str) -> Result<()> {
        let app = Router::new()
            .route("/", get(index_handler))
            .route("/api/exchanges", get(api_exchanges))
            .route("/api/exchanges/{id}", get(api_exchange_detail))
            .route("/api/stats", get(api_stats))
            .route("/api/performance", get(api_performance))
            .route("/api/openapi", get(api_openapi))
            .route("/ws", get(ws_handler))
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone());

        let listener = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("[wireclaw] dashboard running at http://{addr}");
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// HTML dashboard — single-page app with embedded JS.
async fn index_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

/// API: List recent exchanges.
async fn api_exchanges(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    match db::list_exchanges(&state.pool, &state.session, 100).await {
        Ok(exchanges) => Json(json!({ "exchanges": exchanges })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// API: Get single exchange detail.
async fn api_exchange_detail(
    State(state): State<Arc<DashboardState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match db::get_exchange_by_request_id(&state.pool, &id).await {
        Ok(Some(exchange)) => (StatusCode::OK, Json(json!({ "exchange": exchange }))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// API: Session statistics.
async fn api_stats(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    match db::get_session_stats(&state.pool, &state.session).await {
        Ok(stats) => Json(json!({ "stats": stats })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// API: Performance metrics.
async fn api_performance(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    match perf::compute_metrics(&state.pool, &state.session).await {
        Ok(metrics) => Json(json!({ "metrics": metrics })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// API: Generate OpenAPI spec from captured traffic.
async fn api_openapi(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    match openapi::generate_from_session(&state.pool, &state.session).await {
        Ok(spec) => Json(json!({ "openapi": spec })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// WebSocket handler for real-time updates.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<DashboardState>) {
    let mut rx = state.tx.subscribe();
    let mut tick = interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            Ok(event) = rx.recv() => {
                let msg = serde_json::to_string(&event).unwrap_or_default();
                if socket
                    .send(axum::extract::ws::Message::Text(msg.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            _ = tick.tick() => {
                if socket
                    .send(axum::extract::ws::Message::Ping(vec![].into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

/// Embedded dashboard HTML — no external dependencies, works offline.
/// Features multiple themes: synthwave-84 (default), dark, light.
const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Wireclaw Dashboard</title>
<style>
/* ═══════════════════════════════════════════════════════════════
   THEME SYSTEM — Synthwave '84, Dark, Light
   ═══════════════════════════════════════════════════════════════ */

/* ─── Synthwave '84 (default) ─── deep purple, electric purple, magenta, cyan, yellow */
:root,
[data-theme="synthwave-84"] {
  --bg: #0a0014;
  --surface: #1a0b2e;
  --surface-2: #240046;
  --border: #3c096c;
  --text: #e0e0e0;
  --muted: #9d4edd;
  --accent: #00f0ff;
  --accent-2: #ff006e;
  --accent-3: #ffbe0b;
  --success: #00f0ff;
  --warn: #ffbe0b;
  --err: #ff006e;
  --font: 'Segoe UI', system-ui, -apple-system, sans-serif;
  --mono: 'Fira Code', 'Cascadia Code', 'SF Mono', Consolas, monospace;
  --glow-accent: 0 0 10px rgba(0,240,255,0.3);
  --glow-err: 0 0 10px rgba(255,0,110,0.3);
  --glow-warn: 0 0 10px rgba(255,190,11,0.3);
}

/* ─── Dark (GitHub-style) ─── */
[data-theme="dark"] {
  --bg: #0d1117;
  --surface: #161b22;
  --surface-2: #21262d;
  --border: #30363d;
  --text: #e6edf3;
  --muted: #8b949e;
  --accent: #58a6ff;
  --accent-2: #f78166;
  --accent-3: #d29922;
  --success: #3fb950;
  --warn: #d29922;
  --err: #f85149;
  --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
  --mono: 'SF Mono', Consolas, monospace;
  --glow-accent: none;
  --glow-err: none;
  --glow-warn: none;
}

/* ─── Light ─── */
[data-theme="light"] {
  --bg: #f6f8fa;
  --surface: #ffffff;
  --surface-2: #f3f4f6;
  --border: #d0d7de;
  --text: #1f2328;
  --muted: #656d76;
  --accent: #0969da;
  --accent-2: #cf222e;
  --accent-3: #9a6700;
  --success: #1a7f37;
  --warn: #9a6700;
  --err: #cf222e;
  --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
  --mono: 'SF Mono', Consolas, monospace;
  --glow-accent: none;
  --glow-err: none;
  --glow-warn: none;
}

/* ═══════════════════════════════════════════════════════════════
   BASE STYLES
   ═══════════════════════════════════════════════════════════════ */
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--text);font-family:var(--font);font-size:14px;line-height:1.5;min-height:100vh;transition:background .3s,color .3s}

/* ─── Header ─── */
header{background:var(--surface);border-bottom:1px solid var(--border);padding:16px 24px;display:flex;align-items:center;justify-content:space-between;position:relative;overflow:hidden}
header::before{content:'';position:absolute;top:0;left:0;right:0;height:2px;background:linear-gradient(90deg,var(--accent),var(--accent-2),var(--accent-3));opacity:.8}
header h1{font-size:20px;font-weight:700;display:flex;align-items:center;gap:10px;letter-spacing:.5px}
header h1 .logo-icon{font-size:22px;filter:drop-shadow(0 0 4px var(--accent))}
header h1 .logo-text{background:linear-gradient(90deg,var(--accent),var(--accent-2));-webkit-background-clip:text;-webkit-text-fill-color:transparent;background-clip:text}

/* ─── Status Badge ─── */
.status-badge{background:var(--success);color:var(--bg);padding:4px 12px;border-radius:12px;font-size:12px;font-weight:600;box-shadow:var(--glow-accent)}
.status-badge.offline{background:var(--muted);box-shadow:none}

/* ─── Theme Switcher ─── */
.theme-switcher{display:flex;gap:6px;align-items:center}
.theme-btn{background:transparent;border:1px solid var(--border);color:var(--muted);padding:4px 10px;border-radius:6px;cursor:pointer;font-size:12px;transition:all .2s}
.theme-btn:hover{border-color:var(--accent);color:var(--accent)}
.theme-btn.active{background:var(--accent);color:var(--bg);border-color:var(--accent);font-weight:600}

/* ─── Layout ─── */
.container{display:grid;grid-template-columns:1fr 400px;gap:16px;padding:16px;max-width:1500px;margin:0 auto}
@media(max-width:1000px){.container{grid-template-columns:1fr}}

/* ─── Cards ─── */
.card{background:var(--surface);border:1px solid var(--border);border-radius:10px;overflow:hidden;transition:box-shadow .3s}
.card:hover{box-shadow:0 4px 20px rgba(0,0,0,.15)}
.card-header{padding:12px 16px;border-bottom:1px solid var(--border);font-weight:600;font-size:14px;display:flex;justify-content:space-between;align-items:center;background:var(--surface-2)}
.card-body{padding:12px 16px;max-height:calc(100vh - 220px);overflow-y:auto}

/* ─── Metrics ─── */
.metrics{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:16px}
.metric-card{background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:18px;text-align:center;position:relative;overflow:hidden}
.metric-card::before{content:'';position:absolute;top:0;left:0;right:0;height:3px;background:linear-gradient(90deg,var(--accent),var(--accent-2))}
.metric-value{font-size:32px;font-weight:800;color:var(--accent);text-shadow:var(--glow-accent)}
.metric-label{font-size:11px;color:var(--muted);margin-top:6px;text-transform:uppercase;letter-spacing:1px}

/* ─── Exchange List ─── */
.exchange-list{list-style:none}
.exchange-item{padding:10px 12px;border-bottom:1px solid var(--border);cursor:pointer;transition:all .15s;display:flex;align-items:center;gap:12px;font-family:var(--mono);font-size:12px}
.exchange-item:hover{background:var(--surface-2)}
.exchange-item.active{background:var(--surface-2);border-left:3px solid var(--accent)}
.exchange-item .method{padding:2px 8px;border-radius:4px;font-size:11px;font-weight:700;font-family:var(--font);min-width:52px;text-align:center}
.exchange-item .method.get{background:rgba(0,240,255,.15);color:var(--accent)}
.exchange-item .method.post{background:rgba(255,190,11,.15);color:var(--warn)}
.exchange-item .method.put{background:rgba(157,78,221,.2);color:var(--muted)}
.exchange-item .method.delete{background:rgba(255,0,110,.15);color:var(--err)}
.exchange-item .method.patch{background:rgba(157,78,221,.15);color:var(--muted)}
.exchange-item .path{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text)}
.exchange-item .status{font-size:12px;color:var(--muted);min-width:36px;text-align:right}
.exchange-item .latency{font-size:11px;color:var(--muted);min-width:60px;text-align:right}
.exchange-item .latency.slow{color:var(--err);font-weight:700;text-shadow:var(--glow-err)}
.exchange-item .host{color:var(--muted);font-size:11px;min-width:120px;text-align:right;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}

/* ─── Detail Panel ─── */
.detail-panel .detail-row{padding:8px 0;border-bottom:1px solid var(--border);display:flex;gap:8px}
.detail-panel .detail-key{color:var(--muted);min-width:100px;font-size:12px;font-weight:600}
.detail-panel .detail-val{flex:1;font-family:var(--mono);font-size:12px;word-break:break-all}
.detail-panel pre{background:var(--bg);border:1px solid var(--border);border-radius:8px;padding:12px;overflow-x:auto;font-size:12px;margin-top:8px;font-family:var(--mono)}
.detail-panel .section-title{font-weight:700;margin:16px 0 8px;color:var(--accent);font-size:13px;text-transform:uppercase;letter-spacing:1px}

/* ─── Empty State ─── */
.empty-state{text-align:center;padding:40px;color:var(--muted)}
.empty-state .icon{font-size:48px;margin-bottom:12px;opacity:.7}

/* ─── Toolbar ─── */
.toolbar{display:flex;gap:8px;margin-bottom:12px;align-items:center}
.btn{background:var(--surface-2);border:1px solid var(--border);color:var(--text);padding:6px 14px;border-radius:6px;cursor:pointer;font-size:13px;transition:all .15s;font-family:var(--font)}
.btn:hover{background:var(--border);transform:translateY(-1px)}
.btn.primary{background:var(--accent);color:var(--bg);border-color:var(--accent);font-weight:600;box-shadow:var(--glow-accent)}
.btn.primary:hover{background:var(--accent);filter:brightness(1.1)}
.filter-input{background:var(--bg);border:1px solid var(--border);color:var(--text);padding:6px 12px;border-radius:6px;font-size:13px;flex:1;font-family:var(--font)}
.filter-input:focus{outline:none;border-color:var(--accent);box-shadow:var(--glow-accent)}

/* ─── Scrollbar ─── */
::-webkit-scrollbar{width:8px}
::-webkit-scrollbar-track{background:var(--bg)}
::-webkit-scrollbar-thumb{background:var(--border);border-radius:4px}
::-webkit-scrollbar-thumb:hover{background:var(--muted)}

/* ─── Animations ─── */
@keyframes pulse-glow {
  0%,100%{opacity:1}
  50%{opacity:.6}
}
.status-badge{animation:pulse-glow 2s ease-in-out infinite}
</style>
</head>
<body>
<header>
<h1><span class="logo-icon">🦞</span><span class="logo-text">Wireclaw</span></h1>
<div style="display:flex;gap:16px;align-items:center">
  <div class="theme-switcher">
    <button class="theme-btn active" data-theme="synthwave-84" title="Synthwave '84">🌆</button>
    <button class="theme-btn" data-theme="dark" title="Dark">🌙</button>
    <button class="theme-btn" data-theme="light" title="Light">☀️</button>
  </div>
  <span class="status-badge" id="conn-status">Live</span>
</div>
</header>
<div class="container">
<div class="left-col">
<div class="metrics">
<div class="metric-card"><div class="metric-value" id="metric-total">0</div><div class="metric-label">Total Requests</div></div>
<div class="metric-card"><div class="metric-value" id="metric-2xx">0</div><div class="metric-label">2xx Success</div></div>
<div class="metric-card"><div class="metric-value" id="metric-4xx">0</div><div class="metric-label">4xx Errors</div></div>
<div class="metric-card"><div class="metric-value" id="metric-latency">0</div><div class="metric-label">Avg Latency (ms)</div></div>
</div>
<div class="card">
<div class="card-header">
<span>Captured Traffic</span>
<div class="toolbar">
<input class="filter-input" id="filter-input" placeholder="Filter by path, host, method...">
<button class="btn primary" id="btn-export">Export OpenAPI</button>
</div>
</div>
<div class="card-body">
<ul class="exchange-list" id="exchange-list">
<li class="empty-state"><div class="icon">📡</div><div>Waiting for traffic...</div><div style="font-size:12px;margin-top:8px">Start the proxy with <code style="font-family:var(--mono);background:var(--bg);padding:2px 6px;border-radius:4px">wireclaw capture</code></div></li>
</ul>
</div>
</div>
</div>
<div class="right-col">
<div class="card">
<div class="card-header">Request Details</div>
<div class="card-body detail-panel" id="detail-panel">
<div class="empty-state"><div class="icon">📋</div><div>Select a request to view details</div></div>
</div>
</div>
</div>
</div>
<script>
/* ─── Theme Management ─── */
const THEMES = ['synthwave-84', 'dark', 'light'];
let currentTheme = localStorage.getItem('wireclaw-theme') || 'synthwave-84';

function applyTheme(theme) {
  document.documentElement.setAttribute('data-theme', theme);
  localStorage.setItem('wireclaw-theme', theme);
  document.querySelectorAll('.theme-btn').forEach(btn => {
    btn.classList.toggle('active', btn.dataset.theme === theme);
  });
}

document.querySelectorAll('.theme-btn').forEach(btn => {
  btn.addEventListener('click', () => applyTheme(btn.dataset.theme));
});

applyTheme(currentTheme);

/* ─── WebSocket + Data ─── */
const ws = new WebSocket('ws://' + location.host + '/ws');
let exchanges = [];
let selectedId = null;

ws.onopen = () => {
  document.getElementById('conn-status').textContent = 'Live';
  document.getElementById('conn-status').classList.remove('offline');
};
ws.onclose = () => {
  document.getElementById('conn-status').textContent = 'Offline';
  document.getElementById('conn-status').classList.add('offline');
};
ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.type === 'Exchange') {
        exchanges.unshift(msg.exchange);
        if (exchanges.length > 500) exchanges.pop();
        renderList();
        updateMetrics();
    } else if (msg.type === 'Stats') {
        document.getElementById('metric-total').textContent = msg.total;
    }
};

async function loadInitial() {
    try {
        const res = await fetch('/api/exchanges');
        const data = await res.json();
        if (data.exchanges) {
            exchanges = data.exchanges;
            renderList();
            updateMetrics();
        }
    } catch (e) { console.error('Failed to load exchanges', e); }
}

function renderList() {
    const list = document.getElementById('exchange-list');
    const filter = document.getElementById('filter-input').value.toLowerCase();
    const filtered = exchanges.filter(ex => {
        if (!filter) return true;
        const r = ex.request;
        return (r.path + r.host + r.method).toLowerCase().includes(filter);
    });
    if (filtered.length === 0) {
        list.innerHTML = '<li class="empty-state"><div class="icon">🔍</div><div>No matching requests</div></li>';
        return;
    }
    list.innerHTML = filtered.map(ex => {
        const r = ex.request;
        const resp = ex.response;
        const status = resp ? resp.status : '---';
        const latency = resp ? resp.latency_ms + 'ms' : '';
        const slow = resp && resp.latency_ms > 500 ? 'slow' : '';
        const methodClass = r.method.toLowerCase();
        return `<li class="exchange-item ${selectedId === r.id ? 'active' : ''}" data-id="${r.id}">
            <span class="method ${methodClass}">${r.method}</span>
            <span class="path">${escapeHtml(r.path)}</span>
            <span class="host">${escapeHtml(r.host)}</span>
            <span class="status">${status}</span>
            <span class="latency ${slow}">${latency}</span>
        </li>`;
    }).join('');
    list.querySelectorAll('.exchange-item').forEach(el => {
        el.addEventListener('click', () => { selectedId = el.dataset.id; renderList(); renderDetail(); });
    });
}

function renderDetail() {
    const panel = document.getElementById('detail-panel');
    if (!selectedId) { panel.innerHTML = '<div class="empty-state"><div class="icon">📋</div><div>Select a request to view details</div></div>'; return; }
    const ex = exchanges.find(e => e.request.id === selectedId);
    if (!ex) { panel.innerHTML = '<div class="empty-state">Not found</div>'; return; }
    const r = ex.request;
    const resp = ex.response;
    let html = '';
    html += '<div class="section-title">Request</div>';
    html += `<div class="detail-row"><span class="detail-key">Method</span><span class="detail-val">${r.method}</span></div>`;
    html += `<div class="detail-row"><span class="detail-key">URL</span><span class="detail-val">${escapeHtml(r.url)}</span></div>`;
    html += `<div class="detail-row"><span class="detail-key">Host</span><span class="detail-val">${escapeHtml(r.host)}</span></div>`;
    html += `<div class="detail-row"><span class="detail-key">Path</span><span class="detail-val">${escapeHtml(r.path)}</span></div>`;
    html += `<div class="detail-row"><span class="detail-key">Time</span><span class="detail-val">${r.timestamp}</span></div>`;
    html += '<div class="section-title">Request Headers</div><pre>' + escapeHtml(JSON.stringify(r.headers, null, 2)) + '</pre>';
    if (r.body) {
        html += '<div class="section-title">Request Body</div><pre>' + escapeHtml(formatBody(r.body)) + '</pre>';
    }
    if (resp) {
        html += '<div class="section-title">Response</div>';
        html += `<div class="detail-row"><span class="detail-key">Status</span><span class="detail-val">${resp.status} ${resp.status_text}</span></div>`;
        html += `<div class="detail-row"><span class="detail-key">Latency</span><span class="detail-val">${resp.latency_ms} ms</span></div>`;
        html += '<div class="section-title">Response Headers</div><pre>' + escapeHtml(JSON.stringify(resp.headers, null, 2)) + '</pre>';
        if (resp.body) {
            html += '<div class="section-title">Response Body</div><pre>' + escapeHtml(formatBody(resp.body)) + '</pre>';
        }
    }
    panel.innerHTML = html;
}

function updateMetrics() {
    const total = exchanges.length;
    const success = exchanges.filter(e => e.response && e.response.status >= 200 && e.response.status < 300).length;
    const clientErr = exchanges.filter(e => e.response && e.response.status >= 400 && e.response.status < 500).length;
    const latencies = exchanges.filter(e => e.response).map(e => e.response.latency_ms);
    const avg = latencies.length ? Math.round(latencies.reduce((a,b) => a+b, 0) / latencies.length) : 0;
    document.getElementById('metric-total').textContent = total;
    document.getElementById('metric-2xx').textContent = success;
    document.getElementById('metric-4xx').textContent = clientErr;
    document.getElementById('metric-latency').textContent = avg;
}

function escapeHtml(s) {
    if (!s) return '';
    const str = typeof s === 'string' ? s : JSON.stringify(s);
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function formatBody(body) {
    try {
        const s = typeof body === 'string' ? body : JSON.stringify(body);
        const obj = JSON.parse(s);
        return JSON.stringify(obj, null, 2);
    } catch { return typeof body === 'string' ? body : JSON.stringify(body); }
}

document.getElementById('filter-input').addEventListener('input', renderList);
document.getElementById('btn-export').addEventListener('click', async () => {
    try {
        const res = await fetch('/api/openapi');
        const data = await res.json();
        if (data.openapi) {
            const blob = new Blob([JSON.stringify(data.openapi, null, 2)], {type: 'application/json'});
            const a = document.createElement('a');
            a.href = URL.createObjectURL(blob);
            a.download = 'openapi.json';
            a.click();
        }
    } catch (e) { alert('Export failed: ' + e); }
});

loadInitial();

// Poll for new data every second (in case WebSocket misses or proxy is separate process)
setInterval(async () => {
    try {
        const res = await fetch('/api/exchanges');
        const data = await res.json();
        if (data.exchanges) {
            const existingIds = new Set(exchanges.map(e => e.request.id));
            const newExchanges = data.exchanges.filter(e => !existingIds.has(e.request.id));
            if (newExchanges.length > 0) {
                exchanges = [...newExchanges, ...exchanges].slice(0, 500);
                renderList();
                updateMetrics();
            }
        }
    } catch (e) { /* silently fail */ }
}, 1000);
</script>
</body>
</html>"#;

/// Helper to broadcast an exchange to all connected dashboard clients.
pub fn broadcast_exchange(tx: &broadcast::Sender<DashboardEvent>, exchange: &Exchange) {
    let event = DashboardEvent::Exchange {
        exchange: Box::new(exchange.clone()),
    };
    let _ = tx.send(event);
}

/// Run the dashboard as a standalone server.
pub async fn run_dashboard(pool: SqlitePool, session: String, addr: &str) -> Result<()> {
    let server = DashboardServer::new(pool, session);
    server.run(addr).await
}
