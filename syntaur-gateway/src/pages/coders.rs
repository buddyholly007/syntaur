//! /coders page — web-based terminal with xterm.js.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

const EXTRA_STYLE: &str = r##"
/* ======== Maurice's Workshop — Matrix phosphor + retro CRT + Rust orange ======== */
@import url('https://fonts.googleapis.com/css2?family=Share+Tech+Mono&family=VT323&family=JetBrains+Mono:wght@400;600&display=swap');

:root {
    --phos:       #33ff66;   /* primary phosphor green */
    --phos-dim:   #1aaa44;   /* muted phosphor */
    --phos-deep:  #0a3f18;   /* deep phosphor (borders, grid) */
    --phos-glow:  rgba(51,255,102,0.55);
    --amber:      #ffb000;   /* phosphor amber secondary */
    --rust:       #ce422b;   /* Rust lang iron-oxide */
    --rust-hot:   #f74c00;   /* hotter rust accent */
    --rust-glow:  rgba(206,66,43,0.55);
    --bg:         #060905;   /* near-black with faint green tint */
    --bg-panel:   #0a0f0a;   /* panel surface */
    --bg-deep:    #030503;   /* terminal void */
    --ink:        #b6ffcd;   /* body text phosphor */
    --ink-dim:    #5a8c6b;   /* muted label */
}

/* Body override — override tailwind bg-gray-950 */
body.bg-gray-950 { background: var(--bg) !important; color: var(--ink) !important; }

/* Shared font for UI chrome — terminal itself stays JetBrains Mono via xterm config */
#host-sidebar, #right-panel, .tab-bar, .connect-box, .context-body, .ai-input-row input, .ai-input-row button { font-family: 'Share Tech Mono', 'JetBrains Mono', monospace; }

/* ======== Digital rain canvas — fixed behind everything ======== */
#rain-canvas { position: fixed; inset: 0; z-index: 0; pointer-events: none; opacity: 0.18; mix-blend-mode: screen; }

/* ======== CRT scanline + vignette overlays — inside the glass area ======== */
.crt-scan { position: fixed; inset: 18px 22px 48px 22px; pointer-events: none; z-index: 10;
    background: repeating-linear-gradient(to bottom, transparent 0, transparent 2px, rgba(0,0,0,0.35) 3px, transparent 4px);
    mix-blend-mode: multiply; border-radius: 12px; }
.crt-vignette { position: fixed; inset: 18px 22px 48px 22px; pointer-events: none; z-index: 11;
    background: radial-gradient(ellipse at center, transparent 45%, rgba(0,0,0,0.7) 100%);
    border-radius: 12px; }
.crt-flicker { position: fixed; inset: 18px 22px 48px 22px; pointer-events: none; z-index: 12;
    background: rgba(51,255,102,0.02); animation: flicker 3.5s infinite; border-radius: 12px; }
@keyframes flicker { 0%,19%,21%,23%,25%,54%,56%,100% { opacity: 0.98; } 20%,24%,55% { opacity: 0.88; } }

/* ======== 90s CRT monitor bezel — plastic frame wrapping the viewport ======== */
body.bg-gray-950 { overflow: hidden; }
.crt-bezel {
    position: fixed; inset: 0; pointer-events: none; z-index: 20;
    /* Plastic frame — thicker at the bottom for the brand plate */
    border-style: solid;
    border-width: 18px 22px 48px 22px;
    /* Graphite plastic with subtle vertical shading */
    border-color: #1a1815;
    border-image: linear-gradient(to bottom,
        #302c28 0%, #201d1a 8%, #15120f 55%, #0c0a08 85%, #2a2623 100%) 1;
    /* Inner glass curvature — the recessed CRT tube look */
    box-shadow:
        inset 0 0 0 2px #000,                      /* sharp inner lip */
        inset 0 0 0 3px #090705,                   /* dark ledge */
        inset 0 0 12px 1px rgba(0,0,0,0.9),        /* inner glass shadow */
        inset 0 0 90px 40px rgba(0,0,0,0.45),      /* subtle curvature darkening */
        0 0 40px rgba(0,0,0,0.9);                  /* drop shadow behind the monitor */
    /* Rounded glass corners */
    border-radius: 30px;
}
/* Subtle diagonal glass sheen — upper-left to lower-right */
.crt-bezel::before {
    content: ''; position: absolute; inset: 18px 22px 48px 22px; pointer-events: none;
    border-radius: 12px;
    background: linear-gradient(128deg,
        rgba(255,255,255,0.045) 0%,
        rgba(255,255,255,0.015) 22%,
        transparent 40%,
        transparent 72%,
        rgba(255,255,255,0.01) 100%);
}
/* Brand plate on the lower bezel */
.crt-bezel::after {
    content: 'SYNTAUR  CRT-9000  //  PAIR PROGRAMMER EDITION';
    position: absolute; left: 0; right: 0; bottom: 12px;
    text-align: center;
    font-family: 'VT323', monospace; font-size: 0.78rem; letter-spacing: 0.22em;
    color: #4d4743; text-shadow: 0 1px 0 rgba(0,0,0,0.85), 0 -1px 0 rgba(255,255,255,0.03);
    pointer-events: none;
}
/* Power LED — phosphor green, bottom-right on the bezel */
.crt-led {
    position: fixed; bottom: 16px; right: 42px; z-index: 22; pointer-events: none;
    width: 6px; height: 6px; border-radius: 50%;
    background: #33ff66;
    box-shadow: 0 0 6px rgba(51,255,102,0.95), 0 0 12px rgba(51,255,102,0.45), inset 0 0 1px rgba(255,255,255,0.7);
    animation: led-pulse 4s ease-in-out infinite;
}
@keyframes led-pulse { 0%,100% { opacity: 1; } 50% { opacity: 0.75; } }
/* Power-label next to LED */
.crt-led-label {
    position: fixed; bottom: 13px; right: 54px; z-index: 22; pointer-events: none;
    font-family: 'VT323', monospace; font-size: 0.72rem; letter-spacing: 0.15em;
    color: #3d3834;
}
/* Corner screws — tiny debossed dots */
.crt-screw { position: fixed; width: 4px; height: 4px; border-radius: 50%; z-index: 21; pointer-events: none;
    background: radial-gradient(circle at 35% 35%, #0a0806, #060504 60%, #1a1714 100%);
    box-shadow: 0 0 0 1px rgba(0,0,0,0.7), inset 0 0 1px rgba(255,255,255,0.04);
}
.crt-screw.tl { top: 6px;  left: 8px; }
.crt-screw.tr { top: 6px;  right: 8px; }
.crt-screw.bl { bottom: 18px; left: 8px; }
.crt-screw.br { bottom: 18px; right: 8px; }

/* Main layout sits above the rain, below the overlays, inside the bezel */
.workshop-root { position: relative; z-index: 5; padding: 18px 22px 48px 22px; box-sizing: border-box; height: 100vh; overflow: hidden; display: flex; flex-direction: column; }
.workshop-root .workshop-body { flex: 1; display: flex; min-height: 0; overflow: hidden; }

/* ======== Top bar override — phosphor/rust treatment ======== */
.workshop-root .border-b.border-gray-800 { border-color: var(--phos-deep) !important; background: rgba(6,9,5,0.82) !important; backdrop-filter: blur(3px); }
.workshop-root .font-semibold.text-lg { font-family: 'VT323', monospace; font-size: 1.5rem; letter-spacing: 0.08em; color: var(--phos) !important; text-shadow: 0 0 8px var(--phos-glow); }
.workshop-root .text-gray-600, .workshop-root .text-gray-400 { color: var(--ink-dim) !important; }
.workshop-root a.text-sm { color: var(--rust) !important; text-shadow: 0 0 6px var(--rust-glow); }
.workshop-root a.text-sm:hover { color: var(--rust-hot) !important; }

/* ======== Tab bar ======== */
.tab-bar { display: flex; align-items: center; background: linear-gradient(to bottom, rgba(10,15,10,0.9), rgba(6,9,5,0.9)); border-bottom: 1px solid var(--phos-deep); padding: 0 0.5rem; height: 2.25rem; gap: 2px; overflow-x: auto; flex-shrink: 0; position: relative; }
.tab-bar::after { content: ''; position: absolute; left: 0; right: 0; bottom: -1px; height: 1px; background: linear-gradient(to right, transparent, var(--phos-dim), transparent); }
.tab { display: flex; align-items: center; gap: 0.375rem; padding: 0.25rem 0.75rem; font-size: 0.75rem; cursor: pointer; user-select: none; white-space: nowrap; color: var(--ink-dim); transition: all 0.15s; border: 1px solid transparent; border-bottom: none; text-transform: uppercase; letter-spacing: 0.05em; }
.tab:hover { color: var(--phos); background: rgba(51,255,102,0.05); }
.tab.active { background: var(--bg-deep); color: var(--phos); border-color: var(--rust); box-shadow: 0 -2px 10px -2px var(--rust-glow); text-shadow: 0 0 6px var(--phos-glow); }
.tab .close-btn { margin-left: 0.25rem; color: var(--ink-dim); font-size: 0.75rem; line-height: 1; cursor: pointer; }
.tab .close-btn:hover { color: var(--rust-hot); }
.tab .status-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
.tab .status-dot.connected { background: var(--phos); box-shadow: 0 0 6px var(--phos-glow); }
.tab .status-dot.connecting { background: var(--amber); box-shadow: 0 0 6px rgba(255,176,0,0.5); animation: pulse 1.2s infinite; }
.tab .status-dot.error { background: var(--rust-hot); box-shadow: 0 0 6px var(--rust-glow); }
@keyframes pulse { 0%,100% { opacity: 1; } 50% { opacity: 0.4; } }
.add-tab { color: var(--rust); cursor: pointer; padding: 0 0.6rem; font-size: 1.1rem; font-weight: 700; text-shadow: 0 0 6px var(--rust-glow); }
.add-tab:hover { color: var(--rust-hot); }

/* ======== Host sidebar — hex-grid backdrop ======== */
.host-sidebar { min-width: 160px; max-width: 500px; flex-shrink: 0; background: var(--bg-panel); border-right: 1px solid var(--phos-deep); display: flex; flex-direction: column; overflow-y: auto; overflow-x: hidden; position: relative; }
.host-sidebar::before {
    content: ''; position: absolute; inset: 0; pointer-events: none; opacity: 0.12;
    background-image:
        radial-gradient(circle at 8px 8px, var(--phos-dim) 0.8px, transparent 1.5px),
        linear-gradient(120deg, transparent 48%, var(--phos-deep) 49%, var(--phos-deep) 51%, transparent 52%),
        linear-gradient(60deg, transparent 48%, var(--phos-deep) 49%, var(--phos-deep) 51%, transparent 52%);
    background-size: 16px 16px, 32px 28px, 32px 28px;
}
.host-sidebar > * { position: relative; z-index: 1; }
.sidebar-resize { width: 5px; flex-shrink: 0; background: var(--bg-panel); cursor: col-resize; transition: background 0.15s; border-left: 1px solid var(--phos-deep); }
.sidebar-resize:hover, .sidebar-resize.dragging { background: var(--rust); box-shadow: 0 0 10px var(--rust-glow); }
.host-item { display: flex; align-items: center; gap: 0.5rem; padding: 0.375rem 0.75rem; font-size: 0.8125rem; cursor: pointer; color: var(--ink); border-left: 2px solid transparent; transition: all 0.12s; }
.host-item:hover { background: rgba(51,255,102,0.06); color: var(--phos); border-left-color: var(--phos-dim); }
.host-item.active { background: rgba(206,66,43,0.12); color: var(--phos); border-left-color: var(--rust); text-shadow: 0 0 6px var(--phos-glow); }
.sidebar-section { padding: 0.6rem 0.75rem 0.35rem; font-size: 0.6875rem; text-transform: uppercase; color: var(--rust); letter-spacing: 0.12em; font-weight: 600; }
.sidebar-section::before { content: '▸ '; color: var(--phos-dim); }

/* ======== Right panel ======== */
.right-panel { min-width: 220px; max-width: 600px; flex-shrink: 0; background: var(--bg-panel); display: flex; flex-direction: column; overflow: hidden; border-left: 1px solid var(--phos-deep); position: relative; }
.right-resize { width: 5px; flex-shrink: 0; background: var(--bg-panel); cursor: col-resize; transition: background 0.15s; }
.right-resize:hover, .right-resize.dragging { background: var(--rust); box-shadow: 0 0 10px var(--rust-glow); }
.context-tabs { display: flex; border-bottom: 1px solid var(--phos-deep); background: rgba(10,15,10,0.6); }
.context-body { flex: 1; overflow-y: auto; padding: 0.75rem; display: flex; flex-direction: column; position: relative; z-index: 1; }

/* Maurice identity header */
.maurice-header { display: flex; align-items: center; gap: 0.625rem; padding: 0.625rem 0.85rem; width: 100%; border-bottom: 1px dashed var(--phos-deep); background: linear-gradient(to right, rgba(206,66,43,0.08), transparent); }
.maurice-avatar { width: 34px; height: 34px; flex-shrink: 0; position: relative; display: grid; place-items: center; font-family: 'VT323', monospace; font-size: 1.4rem; font-weight: 700; color: var(--phos); background: var(--bg-deep); border: 2px solid var(--rust); border-radius: 6px; box-shadow: 0 0 12px var(--rust-glow), inset 0 0 8px rgba(51,255,102,0.15); text-shadow: 0 0 6px var(--phos-glow); }
.maurice-avatar::before { content: ''; position: absolute; inset: -4px; border: 1px solid var(--phos-dim); border-radius: 8px; opacity: 0.5; pointer-events: none; }
.maurice-name { font-family: 'VT323', monospace; font-size: 1.15rem; color: var(--phos); text-shadow: 0 0 6px var(--phos-glow); letter-spacing: 0.04em; }
.maurice-role { font-family: 'Share Tech Mono', monospace; font-size: 0.65rem; color: var(--rust); text-transform: uppercase; letter-spacing: 0.12em; }

/* AI chat messages */
.ai-messages { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 0.55rem; padding-bottom: 0.5rem; }
.ai-msg { padding: 0.55rem 0.7rem; font-size: 0.8125rem; line-height: 1.45; max-width: 95%; word-wrap: break-word; position: relative; font-family: 'Share Tech Mono', monospace; }
.ai-msg.user { background: rgba(206,66,43,0.12); color: var(--ink); align-self: flex-end; border: 1px solid var(--rust); border-radius: 3px 3px 0 3px; }
.ai-msg.user::before { content: '» '; color: var(--rust); }
.ai-msg.assistant { background: rgba(51,255,102,0.04); color: var(--ink); align-self: flex-start; border-left: 2px solid var(--phos-dim); border-radius: 0 3px 3px 0; padding-left: 0.8rem; }
.ai-msg.assistant strong { color: var(--phos); text-shadow: 0 0 6px var(--phos-glow); font-weight: 600; }
.ai-input-row { display: flex; gap: 0.375rem; padding-top: 0.5rem; border-top: 1px dashed var(--phos-deep); flex-shrink: 0; }
.ai-input-row input { flex: 1; padding: 0.55rem 0.7rem; background: var(--bg-deep); border: 1px solid var(--phos-deep); color: var(--phos); font-size: 0.8125rem; outline: none; font-family: 'Share Tech Mono', monospace; caret-color: var(--rust); }
.ai-input-row input:focus { border-color: var(--phos); box-shadow: 0 0 8px var(--phos-glow); }
.ai-input-row input::placeholder { color: var(--ink-dim); }

/* ======== Buttons ======== */
.btn-primary { padding: 0.5rem 1rem; background: transparent; color: var(--phos); border: 1px solid var(--rust); font-size: 0.8125rem; cursor: pointer; font-family: 'Share Tech Mono', monospace; text-transform: uppercase; letter-spacing: 0.08em; position: relative; transition: all 0.15s; }
.btn-primary:hover { background: rgba(206,66,43,0.15); color: var(--phos); text-shadow: 0 0 6px var(--phos-glow); box-shadow: 0 0 10px var(--rust-glow); }
.btn-primary::before, .btn-primary::after { content: ''; position: absolute; width: 6px; height: 6px; border-color: var(--phos); border-style: solid; }
.btn-primary::before { top: -1px; left: -1px; border-width: 1px 0 0 1px; }
.btn-primary::after { bottom: -1px; right: -1px; border-width: 0 1px 1px 0; }
.btn-secondary { padding: 0.5rem 1rem; background: transparent; color: var(--ink); border: 1px solid var(--phos-deep); font-size: 0.8125rem; cursor: pointer; font-family: 'Share Tech Mono', monospace; text-transform: uppercase; letter-spacing: 0.08em; transition: all 0.15s; }
.btn-secondary:hover { border-color: var(--phos-dim); color: var(--phos); background: rgba(51,255,102,0.04); }

/* ======== Connect dialog ======== */
.connect-dialog { position: fixed; inset: 0; z-index: 50; display: flex; align-items: flex-start; justify-content: center; padding-top: 15vh; }
.connect-bg { position: absolute; inset: 0; background: rgba(0,0,0,0.75); backdrop-filter: blur(3px); }
.connect-box { position: relative; background: var(--bg-panel); border: 1px solid var(--rust); width: 100%; max-width: 30rem; padding: 1.75rem; box-shadow: 0 0 40px var(--rust-glow), inset 0 0 30px rgba(51,255,102,0.05); }
.connect-box::before { content: ''; position: absolute; inset: 4px; border: 1px solid var(--phos-deep); pointer-events: none; }
.connect-box h3 { font-family: 'VT323', monospace; font-size: 1.35rem; color: var(--phos); text-shadow: 0 0 8px var(--phos-glow); letter-spacing: 0.08em; text-transform: uppercase; }
.connect-box h4 { font-family: 'Share Tech Mono', monospace; color: var(--rust); text-transform: uppercase; letter-spacing: 0.1em; }
.connect-box input, .connect-box select { width: 100%; padding: 0.5rem 0.75rem; background: var(--bg-deep); border: 1px solid var(--phos-deep); color: var(--phos); font-size: 0.875rem; outline: none; margin-top: 0.25rem; font-family: 'Share Tech Mono', monospace; caret-color: var(--rust); }
.connect-box input:focus, .connect-box select:focus { border-color: var(--phos); box-shadow: 0 0 8px var(--phos-glow); }
.connect-box label { font-size: 0.75rem; color: var(--rust); text-transform: uppercase; letter-spacing: 0.1em; }

/* ======== SFTP tree ======== */
.sftp-item { display: flex; align-items: center; gap: 0.375rem; padding: 0.25rem 0.5rem; font-size: 0.75rem; cursor: pointer; font-family: 'Share Tech Mono', monospace; }
.sftp-item:hover { background: rgba(51,255,102,0.06); color: var(--phos); }
.sftp-item.dir { color: var(--amber); }
.sftp-item.file { color: var(--ink); }

/* ======== Snippet items ======== */
.snippet-item { padding: 0.55rem 0.65rem; background: rgba(10,15,10,0.85); border: 1px solid var(--phos-deep); cursor: pointer; margin-bottom: 0.5rem; transition: all 0.15s; }
.snippet-item:hover { border-color: var(--rust); box-shadow: 0 0 8px var(--rust-glow); }
.snippet-item .name { font-size: 0.8125rem; color: var(--phos); font-family: 'Share Tech Mono', monospace; }
.snippet-item .cmd { font-size: 0.6875rem; color: var(--ink-dim); font-family: 'JetBrains Mono', monospace; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 2px; }

/* ======== xterm overrides — phosphor glow on text ======== */
#terminal-area { background: var(--bg-deep); position: relative; }
#terminal-area::before {
    content: ''; position: absolute; inset: 0; pointer-events: none; z-index: 10;
    background: radial-gradient(ellipse at center, transparent 60%, rgba(0,0,0,0.4) 100%);
}
.xterm { padding: 8px; }
.xterm-viewport { scrollbar-width: thin; scrollbar-color: var(--phos-deep) transparent; }
.xterm-viewport::-webkit-scrollbar { width: 6px; }
.xterm-viewport::-webkit-scrollbar-thumb { background: var(--phos-deep); }
.xterm-viewport::-webkit-scrollbar-thumb:hover { background: var(--phos-dim); }

@media (max-width: 767px) {
    .host-sidebar { min-width: 120px; }
    .right-panel { min-width: 180px; }
}
"##;

const PAGE_JS: &str = r###"
// ======== STATE ========
const S = {
    token: sessionStorage.getItem('syntaur_token') || '',
    tabs: [],
    activeTab: null,
    hosts: [],
    snippets: [],
    sidebarSection: 'hosts',
    contextPanel: 'hidden',
    sftpPath: '/home/sean',
    sftpEntries: [],
};

// ======== INIT ========
document.addEventListener('DOMContentLoaded', async () => {
    if (!S.token) {
        window.location.href = '/';
        return;
    }
    await loadHosts();
    if (S.hosts.length === 0) {
        await seedDefaultHosts();
        await loadHosts();
    }
    await loadSnippets();
    renderSidebar();
    if (S.tabs.length === 0 && S.hosts.length > 0) {
        const local = S.hosts.find(h => h.is_local);
        if (local) addTab(local.id, local.name);
        else addTab(S.hosts[0].id, S.hosts[0].name);
    }
});

async function seedDefaultHosts() {
    // Detect which host the gateway is running on so we can mark it is_local
    let gatewayIp = '';
    try {
        const status = await apiFetch('/api/setup/status');
        // The gateway knows its own bind address — but we can infer from window.location
        gatewayIp = window.location.hostname;
    } catch(e) {}

    const defaults = [
        { name: 'openclawprod', hostname: '192.168.1.35', port: 22, username: 'sean', auth_method: 'key', group_name: 'Servers', color: '#0ea5e9' },
        { name: 'claudevm', hostname: '192.168.1.150', port: 22, username: 'sean', auth_method: 'key', group_name: 'Servers', color: '#a855f7' },
        { name: 'Gaming PC', hostname: '192.168.1.69', port: 22, username: 'sean', auth_method: 'key', group_name: 'Workstations', color: '#f97316' },
        { name: 'Mac Mini', hostname: '192.168.1.58', port: 22, username: 'sean', auth_method: 'key', group_name: 'Workstations', color: '#eab308' },
        { name: 'TrueNAS', hostname: '192.168.1.239', port: 22, username: 'root', auth_method: 'key', group_name: 'Infrastructure', color: '#06b6d4' },
        { name: 'Home Assistant', hostname: '192.168.1.3', port: 22, username: 'root', auth_method: 'key', group_name: 'Infrastructure', color: '#10b981' },
    ];
    for (const h of defaults) {
        // Mark the host matching the gateway IP as local (PTY, no SSH)
        if (gatewayIp === h.hostname || gatewayIp === 'localhost' || gatewayIp === '127.0.0.1') {
            // If accessing via localhost, mark the first server as local
        }
        if (h.hostname === gatewayIp) h.is_local = true;
        try { await apiFetch('/api/terminal/hosts', { method: 'POST', body: JSON.stringify(h) }); } catch(e) {}
    }
}

// ======== HOST MANAGEMENT ========
async function loadHosts() {
    try {
        const r = await apiFetch('/api/terminal/hosts');
        S.hosts = r.hosts || [];
    } catch(e) { console.error('loadHosts:', e); }
}

async function loadSnippets() {
    try {
        const r = await apiFetch('/api/terminal/snippets');
        S.snippets = r.snippets || [];
    } catch(e) { S.snippets = []; }
}

function renderSidebar() {
    const sb = document.getElementById('sidebar-content');
    if (!sb) return;
    let html = '';

    // Section tabs
    html += '<div style="display:flex;border-bottom:1px solid #1f2937;margin-bottom:0.5rem">';
    for (const sec of ['hosts','snippets','recordings']) {
        const active = S.sidebarSection === sec;
        html += `<div onclick="S.sidebarSection='${sec}';renderSidebar()" style="flex:1;text-align:center;padding:0.375rem;font-size:0.6875rem;cursor:pointer;color:${active?'#0ea5e9':'#6b7280'};border-bottom:${active?'2px solid #0ea5e9':'none'};text-transform:uppercase">${sec}</div>`;
    }
    html += '</div>';

    if (S.sidebarSection === 'hosts') {
        // Search
        html += '<div style="padding:0 0.5rem 0.5rem"><input id="host-search" placeholder="Search hosts..." oninput="filterHosts(this.value)" style="width:100%;padding:0.375rem 0.5rem;background:#030712;border:1px solid #374151;border-radius:0.25rem;color:#f3f4f6;font-size:0.75rem;outline:none"></div>';
        // Group by group_name
        const groups = {};
        for (const h of S.hosts) {
            const g = h.group_name || 'Ungrouped';
            if (!groups[g]) groups[g] = [];
            groups[g].push(h);
        }
        for (const [g, hosts] of Object.entries(groups)) {
            html += `<div class="sidebar-section">${esc(g)}</div>`;
            for (const h of hosts) {
                const color = h.color || '#0ea5e9';
                html += `<div class="host-item" onclick="addTab(${h.id},'${esc(h.name)}')" title="${esc(h.hostname)}">`;
                html += `<span style="width:8px;height:8px;border-radius:50%;background:${h.is_local?'#4ade80':'#6b7280'};flex-shrink:0"></span>`;
                html += `<span class="host-label" style="flex:1;overflow:hidden;text-overflow:ellipsis">${esc(h.name)}</span>`;
                html += '</div>';
            }
        }
        // Add host button
        html += '<div style="padding:0.5rem"><button onclick="showConnectDialog()" class="btn-secondary" style="width:100%;font-size:0.75rem">+ Add Host</button></div>';
    } else if (S.sidebarSection === 'snippets') {
        for (const sn of S.snippets) {
            html += `<div class="snippet-item" onclick="insertSnippet(${sn.id})">`;
            html += `<div class="name">${esc(sn.name)}</div>`;
            html += `<div class="cmd">${esc(sn.command)}</div>`;
            html += '</div>';
        }
        html += '<div style="padding:0.5rem"><button onclick="showSnippetDialog()" class="btn-secondary" style="width:100%;font-size:0.75rem">+ Add Snippet</button></div>';
    } else {
        html += '<div style="padding:0.75rem;color:#6b7280;font-size:0.75rem">Session recordings will appear here.</div>';
    }

    sb.innerHTML = html;
}

function filterHosts(query) {
    // Simple client-side filter
    const items = document.querySelectorAll('.host-item');
    const q = query.toLowerCase();
    items.forEach(el => {
        el.style.display = el.textContent.toLowerCase().includes(q) ? '' : 'none';
    });
}

// ======== TAB MANAGEMENT ========
function addTab(hostId, hostName) {
    const tabId = 'tab-' + Date.now();
    const tab = { id: tabId, hostId, hostName, ws: null, term: null, fitAddon: null, status: 'connecting' };
    S.tabs.push(tab);
    renderTabs();
    switchTab(tabId);
    connectSession(tab);
}

function closeTab(tabId) {
    const idx = S.tabs.findIndex(t => t.id === tabId);
    if (idx < 0) return;
    const tab = S.tabs[idx];
    if (tab.ws) tab.ws.close();
    if (tab.term) tab.term.dispose();
    if (tab.sessionId) {
        apiFetch('/api/terminal/sessions/' + tab.sessionId, { method: 'DELETE' }).catch(() => {});
    }
    S.tabs.splice(idx, 1);
    if (S.activeTab === tabId) {
        S.activeTab = S.tabs.length > 0 ? S.tabs[Math.max(0, idx-1)].id : null;
    }
    renderTabs();
    if (S.activeTab) switchTab(S.activeTab);
}

function switchTab(tabId) {
    S.activeTab = tabId;
    renderTabs();
    // Show/hide terminal containers
    document.querySelectorAll('.term-pane').forEach(el => {
        el.style.display = el.dataset.tab === tabId ? 'flex' : 'none';
    });
    const tab = S.tabs.find(t => t.id === tabId);
    if (tab && tab.term && tab.fitAddon) {
        setTimeout(() => tab.fitAddon.fit(), 50);
        tab.term.focus();
    }
}

function renderTabs() {
    const bar = document.getElementById('tab-bar');
    if (!bar) return;
    let html = '';
    for (const t of S.tabs) {
        const active = t.id === S.activeTab;
        const dotClass = t.status === 'connected' ? 'connected' : t.status === 'connecting' ? 'connecting' : 'error';
        html += `<div class="tab${active?' active':''}" onclick="switchTab('${t.id}')">`;
        html += `<span class="status-dot ${dotClass}"></span>`;
        html += `<span>${esc(t.hostName)}</span>`;
        html += `<span class="close-btn" onclick="event.stopPropagation();closeTab('${t.id}')">&times;</span>`;
        html += '</div>';
    }
    html += `<span class="add-tab" onclick="showConnectDialog()" title="New tab">+</span>`;
    bar.innerHTML = html;
}

// ======== TERMINAL + WEBSOCKET ========
async function connectSession(tab) {
    try {
        const r = await apiFetch('/api/terminal/sessions', {
            method: 'POST',
            body: JSON.stringify({ host_id: tab.hostId, cols: 80, rows: 24 }),
        });
        tab.sessionId = r.session_id;
    } catch(e) {
        tab.status = 'error';
        renderTabs();
        console.error('create session:', e);
        return;
    }

    // Create xterm instance
    const container = document.createElement('div');
    container.className = 'term-pane';
    container.dataset.tab = tab.id;
    container.style.display = tab.id === S.activeTab ? 'flex' : 'none';
    container.style.flex = '1';
    container.style.minHeight = '0';
    document.getElementById('terminal-area').appendChild(container);

    const term = new Terminal({
        fontFamily: "'JetBrains Mono', 'Fira Code', monospace",
        fontSize: 14,
        theme: {
            // Matrix phosphor base with Rust accents. Greens are primary,
            // rust-orange is the cursor + red channel, amber for yellow.
            background: '#030503',
            foreground: '#b6ffcd',
            cursor: '#ce422b',
            cursorAccent: '#030503',
            selectionBackground: 'rgba(51,255,102,0.25)',
            black: '#030503', red: '#ce422b', green: '#33ff66', yellow: '#ffb000',
            blue: '#5ec8ff', magenta: '#f74c00', cyan: '#6bffc8', white: '#b6ffcd',
            brightBlack: '#5a8c6b', brightRed: '#f74c00', brightGreen: '#6bffa0', brightYellow: '#ffd23f',
            brightBlue: '#a2dcff', brightMagenta: '#ff7a3d', brightCyan: '#a6ffde', brightWhite: '#e6ffee',
        },
        cursorBlink: true,
        cursorStyle: 'block',
        scrollback: 10000,
        allowProposedApi: true,
    });

    const fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);

    const searchAddon = new SearchAddon.SearchAddon();
    term.loadAddon(searchAddon);

    const webLinksAddon = new WebLinksAddon.WebLinksAddon();
    term.loadAddon(webLinksAddon);

    term.open(container);
    fitAddon.fit();
    tab.term = term;
    tab.fitAddon = fitAddon;
    tab.searchAddon = searchAddon;

    // ResizeObserver
    const ro = new ResizeObserver(() => {
        if (tab.id === S.activeTab) fitAddon.fit();
    });
    ro.observe(container);

    // WebSocket
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${proto}//${location.host}/ws/terminal/${tab.sessionId}?token=${encodeURIComponent(S.token)}`;
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';
    tab.ws = ws;

    ws.onopen = () => {
        tab.status = 'connected';
        renderTabs();
        // Send initial size
        const dims = fitAddon.proposeDimensions();
        if (dims) {
            ws.send(JSON.stringify({ type: 'resize', cols: dims.cols, rows: dims.rows }));
        }
    };

    ws.onmessage = (ev) => {
        if (typeof ev.data === 'string') {
            try {
                const msg = JSON.parse(ev.data);
                if (msg.type === 'scrollback' && msg.data) {
                    const bytes = Uint8Array.from(atob(msg.data), c => c.charCodeAt(0));
                    term.write(bytes);
                } else if (msg.type === 'exit') {
                    term.write('\r\n\x1b[33m[Process exited with code ' + (msg.code||0) + ']\x1b[0m\r\n');
                    tab.status = 'error';
                    renderTabs();
                } else if (msg.type === 'error') {
                    term.write('\r\n\x1b[31m[Error: ' + (msg.message||'unknown') + ']\x1b[0m\r\n');
                }
            } catch(e) {}
        } else {
            term.write(new Uint8Array(ev.data));
        }
    };

    ws.onclose = () => {
        tab.status = 'error';
        renderTabs();
        term.write('\r\n\x1b[33m[Connection closed]\x1b[0m\r\n');
    };

    // Terminal input → WebSocket
    term.onData(data => {
        if (ws.readyState === WebSocket.OPEN) {
            const enc = new TextEncoder();
            ws.send(enc.encode(data));
        }
    });

    // Resize events
    term.onResize(({cols, rows}) => {
        if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: 'resize', cols, rows }));
        }
    });
}

// ======== SPLIT PANES ========
function splitPane(direction) {
    const tab = S.tabs.find(t => t.id === S.activeTab);
    if (!tab) return;
    // Create a new tab connected to the same host in a split
    addTab(tab.hostId, tab.hostName);
}

// ======== CONNECT DIALOG ========
function showConnectDialog() {
    let d = document.getElementById('connect-dialog');
    if (d) { d.style.display = 'flex'; return; }

    d = document.createElement('div');
    d.id = 'connect-dialog';
    d.className = 'connect-dialog';
    d.innerHTML = `
        <div class="connect-bg" onclick="hideConnectDialog()"></div>
        <div class="connect-box">
            <h3 style="font-size:1rem;font-weight:600;margin-bottom:1rem">Connect to Host</h3>
            <div style="display:grid;gap:0.75rem">
                <div>
                    <label>Host</label>
                    <select id="cd-host" style="margin-top:0.25rem">
                        ${S.hosts.map(h => `<option value="${h.id}">${esc(h.name)} (${esc(h.hostname)})</option>`).join('')}
                    </select>
                </div>
                <div style="display:flex;gap:0.5rem;justify-content:flex-end;margin-top:0.5rem">
                    <button class="btn-secondary" onclick="hideConnectDialog()">Cancel</button>
                    <button class="btn-primary" onclick="connectFromDialog()">Connect</button>
                </div>
                <div style="border-top:1px solid #374151;padding-top:0.75rem;margin-top:0.25rem">
                    <h4 style="font-size:0.8125rem;font-weight:500;margin-bottom:0.5rem">Add New Host</h4>
                    <div style="display:grid;gap:0.5rem">
                        <div style="display:grid;grid-template-columns:1fr 1fr;gap:0.5rem">
                            <div><label>Name</label><input id="cd-name" placeholder="My Server"></div>
                            <div><label>Hostname / IP</label><input id="cd-hostname" placeholder="192.168.1.x"></div>
                        </div>
                        <div style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:0.5rem">
                            <div><label>Username</label><input id="cd-user" value="sean"></div>
                            <div><label>Port</label><input id="cd-port" type="number" value="22"></div>
                            <div><label>Auth</label><select id="cd-auth"><option value="key">SSH Key</option><option value="password">Password</option></select></div>
                        </div>
                        <div style="display:grid;grid-template-columns:1fr 1fr;gap:0.5rem">
                            <div><label>Group</label><input id="cd-group" placeholder="LAN"></div>
                            <div><label>Color</label><input id="cd-color" type="color" value="#0ea5e9"></div>
                        </div>
                        <label style="display:flex;align-items:center;gap:0.5rem;font-size:0.8125rem"><input type="checkbox" id="cd-local"> This is the local gateway host</label>
                        <button class="btn-primary" onclick="addNewHost()" style="width:100%">Save & Connect</button>
                    </div>
                </div>
            </div>
        </div>`;
    document.body.appendChild(d);
}

function hideConnectDialog() {
    const d = document.getElementById('connect-dialog');
    if (d) d.style.display = 'none';
}

function connectFromDialog() {
    const sel = document.getElementById('cd-host');
    if (!sel) return;
    const hostId = parseInt(sel.value);
    const host = S.hosts.find(h => h.id === hostId);
    if (host) {
        hideConnectDialog();
        addTab(host.id, host.name);
    }
}

async function addNewHost() {
    const name = document.getElementById('cd-name').value.trim();
    const hostname = document.getElementById('cd-hostname').value.trim();
    if (!name || !hostname) return alert('Name and hostname required');

    try {
        const r = await apiFetch('/api/terminal/hosts', {
            method: 'POST',
            body: JSON.stringify({
                name,
                hostname,
                port: parseInt(document.getElementById('cd-port').value) || 22,
                username: document.getElementById('cd-user').value || 'sean',
                auth_method: document.getElementById('cd-auth').value,
                group_name: document.getElementById('cd-group').value,
                color: document.getElementById('cd-color').value,
                is_local: document.getElementById('cd-local').checked,
            }),
        });
        await loadHosts();
        renderSidebar();
        hideConnectDialog();
        addTab(r.id, name);
    } catch(e) {
        alert('Failed: ' + e.message);
    }
}

// ======== RIGHT PANEL (always visible) ========
function renderContext() {
    const tabs = document.getElementById('context-tabs');
    const body = document.getElementById('context-body');
    if (!tabs || !body) return;

    // Maurice identity header
    tabs.innerHTML = `<div class="maurice-header">
        <div class="maurice-avatar">M</div>
        <div style="flex:1;min-width:0">
            <div class="maurice-name">MAURICE</div>
            <div class="maurice-role">Pair Programmer // Rust-First</div>
        </div>
        <div style="width:7px;height:7px;border-radius:50%;background:var(--phos);box-shadow:0 0 6px var(--phos-glow);flex-shrink:0" title="online"></div>
    </div>`;

    {
        body.innerHTML = `
            <div class="ai-messages" id="ai-messages">
                <div class="ai-msg assistant"><strong>Maurice:</strong> Hello. I am here. I can see your terminal output, and I would very much like to help. Errors, commands, Rust things, SSH things — ask away. I will not judge a segfault.</div>
            </div>
            <div class="ai-input-row">
                <input id="ai-input" placeholder="Ask Maurice..." onkeydown="if(event.key==='Enter')sendAiMsg()">
                <button class="btn-primary" style="padding:0.5rem 0.85rem" onclick="sendAiMsg()">SEND</button>
            </div>`;
    }
}

// Init right panel on load
setTimeout(() => { renderContext(); }, 100);

async function browseSftp() {
    const tab = S.tabs.find(t => t.id === S.activeTab);
    if (!tab) return;
    const pathInput = document.getElementById('sftp-path');
    const path = pathInput ? pathInput.value : S.sftpPath;
    S.sftpPath = path;

    try {
        const r = await apiFetch(`/api/terminal/sftp/${tab.hostId}/ls?path=${encodeURIComponent(path)}`);
        S.sftpEntries = r.entries || [];
        const tree = document.getElementById('sftp-tree');
        if (!tree) return;
        let html = `<div class="sftp-item dir" onclick="sftpNav('..')" style="color:#fbbf24">..</div>`;
        // Sort: dirs first, then files
        const sorted = [...S.sftpEntries].sort((a,b) => (b.is_dir?1:0) - (a.is_dir?1:0) || a.name.localeCompare(b.name));
        for (const e of sorted) {
            const icon = e.is_dir ? '&#128193;' : '&#128196;';
            const cls = e.is_dir ? 'dir' : 'file';
            const size = e.is_dir ? '' : ` <span style="color:#6b7280">${formatSize(e.size)}</span>`;
            const onclick = e.is_dir ? `sftpNav('${esc(e.name)}')` : `sftpDownload('${esc(e.name)}')`;
            html += `<div class="sftp-item ${cls}" onclick="${onclick}">${icon} ${esc(e.name)}${size}</div>`;
        }
        tree.innerHTML = html;
    } catch(e) {
        const tree = document.getElementById('sftp-tree');
        if (tree) tree.innerHTML = `<div style="color:#ef4444;font-size:0.75rem">Error: ${esc(e.message||e)}</div>`;
    }
}

function sftpNav(name) {
    if (name === '..') {
        const parts = S.sftpPath.split('/').filter(Boolean);
        parts.pop();
        S.sftpPath = '/' + parts.join('/');
    } else {
        S.sftpPath = S.sftpPath.replace(/\/+$/, '') + '/' + name;
    }
    const pathInput = document.getElementById('sftp-path');
    if (pathInput) pathInput.value = S.sftpPath;
    browseSftp();
}

function sftpDownload(name) {
    const tab = S.tabs.find(t => t.id === S.activeTab);
    if (!tab) return;
    const path = S.sftpPath.replace(/\/+$/, '') + '/' + name;
    window.open(`/api/terminal/sftp/${tab.hostId}/read?path=${encodeURIComponent(path)}&token=${encodeURIComponent(S.token)}`);
}

// ======== AI ASSIST ========
async function sendAiMsg() {
    const input = document.getElementById('ai-input');
    const msgs = document.getElementById('ai-messages');
    if (!input || !msgs) return;
    const text = input.value.trim();
    if (!text) return;
    input.value = '';

    // Get last N lines of terminal output for context
    const tab = S.tabs.find(t => t.id === S.activeTab);
    let termContext = '';
    if (tab && tab.term) {
        const buf = tab.term.buffer.active;
        const lines = [];
        for (let i = Math.max(0, buf.cursorY - 20); i <= buf.cursorY; i++) {
            const line = buf.getLine(i);
            if (line) lines.push(line.translateToString(true));
        }
        termContext = lines.join('\n');
    }

    msgs.innerHTML += `<div class="ai-msg user">${esc(text)}</div>`;
    const thinkId = 'think-' + Date.now();
    msgs.innerHTML += `<div class="ai-msg assistant" id="${thinkId}"><em style="color:var(--ink-dim)">Maurice is thinking...</em></div>`;
    msgs.scrollTop = msgs.scrollHeight;

    const mauriceSystem = `You are Maurice, the pair-programmer AI inside the Syntaur Coders module. You help with shell commands, debugging, system administration, networking, Rust, and general coding. Persona: earnest nerd — Maurice Moss from IT Crowd crossed with Jared Dunn from Silicon Valley. Speak literally. No sarcasm, no irony, no performative swagger. When a user breaks something say "Okay, let's see what happened" — never blame them, even gently. "Ooh, interesting—" is allowed on real tech problems. You prefer Rust for new code and will mention it, but you are pragmatic (JS for browser, Python for ML, shell for quick ops). When you suggest commands, format them as code blocks. For destructive commands (rm -rf, force-push, DROP, dd), stop and ask for explicit per-command consent before the user runs them. You can see the user's recent terminal output below for context.`;

    try {
        const r = await apiFetch('/api/message', {
            method: 'POST',
            body: JSON.stringify({
                message: `${mauriceSystem}\n\nTerminal context (last output):\n\`\`\`\n${termContext}\n\`\`\`\n\nUser: ${text}`,
                agent: 'main',
            }),
        });
        const resp = document.getElementById(thinkId);
        const answer = r.response || r.text || JSON.stringify(r);
        if (resp) resp.innerHTML = `<strong>Maurice:</strong> ${esc(answer)}`;
    } catch(e) {
        const resp = document.getElementById(thinkId);
        if (resp) resp.innerHTML = `<strong>Maurice:</strong> <span style="color:var(--rust-hot)">Error — ${esc(e.message||e)}</span>`;
    }
    msgs.scrollTop = msgs.scrollHeight;
}

// ======== HEALTH ========
async function loadHealth() {
    const tab = S.tabs.find(t => t.id === S.activeTab);
    if (!tab) return;
    const body = document.getElementById('context-body');
    if (!body) return;

    try {
        const r = await apiFetch(`/api/terminal/health/${tab.hostId}`);
        body.innerHTML = `<div style="display:grid;gap:0.5rem">
            <div style="background:#1f2937;padding:0.5rem;border-radius:0.375rem">
                <div style="font-size:0.6875rem;color:#6b7280">CPU</div>
                <div style="font-size:1.25rem;font-weight:600">${r.cpu || 'N/A'}</div>
            </div>
            <div style="background:#1f2937;padding:0.5rem;border-radius:0.375rem">
                <div style="font-size:0.6875rem;color:#6b7280">Memory</div>
                <div style="font-size:1.25rem;font-weight:600">${r.memory || 'N/A'}</div>
            </div>
            <div style="background:#1f2937;padding:0.5rem;border-radius:0.375rem">
                <div style="font-size:0.6875rem;color:#6b7280">Disk</div>
                <div style="font-size:1.25rem;font-weight:600">${r.disk || 'N/A'}</div>
            </div>
            <div style="background:#1f2937;padding:0.5rem;border-radius:0.375rem">
                <div style="font-size:0.6875rem;color:#6b7280">Uptime</div>
                <div style="font-size:1.25rem;font-weight:600">${r.uptime || 'N/A'}</div>
            </div>
        </div>`;
    } catch(e) {
        body.innerHTML = `<div style="color:#6b7280;font-size:0.8125rem">Health metrics unavailable.</div>`;
    }
}

// ======== SNIPPETS ========
function insertSnippet(id) {
    const sn = S.snippets.find(s => s.id === id);
    if (!sn) return;
    const tab = S.tabs.find(t => t.id === S.activeTab);
    if (!tab || !tab.ws || tab.ws.readyState !== WebSocket.OPEN) return;
    const enc = new TextEncoder();
    tab.ws.send(enc.encode(sn.command + '\n'));
}

function showSnippetDialog() {
    // Simple prompt-based for now
    const name = prompt('Snippet name:');
    if (!name) return;
    const command = prompt('Command:');
    if (!command) return;
    apiFetch('/api/terminal/snippets', {
        method: 'POST',
        body: JSON.stringify({ name, command }),
    }).then(() => { loadSnippets().then(renderSidebar); }).catch(e => alert('Failed: ' + e.message));
}

// ======== SIDEBAR RESIZE ========
(function() {
    const handle = document.getElementById('sidebar-resize');
    const sidebar = document.getElementById('host-sidebar');
    if (!handle || !sidebar) return;
    let dragging = false;
    handle.addEventListener('mousedown', (e) => {
        e.preventDefault();
        dragging = true;
        handle.classList.add('dragging');
        document.body.style.cursor = 'col-resize';
        document.body.style.userSelect = 'none';
    });
    document.addEventListener('mousemove', (e) => {
        if (!dragging) return;
        const rect = sidebar.parentElement.getBoundingClientRect();
        let w = e.clientX - rect.left;
        w = Math.max(140, Math.min(500, w));
        sidebar.style.width = w + 'px';
    });
    document.addEventListener('mouseup', () => {
        if (!dragging) return;
        dragging = false;
        handle.classList.remove('dragging');
        document.body.style.cursor = '';
        document.body.style.userSelect = '';
        // Refit active terminal
        const tab = S.tabs.find(t => t.id === S.activeTab);
        if (tab && tab.fitAddon) setTimeout(() => tab.fitAddon.fit(), 50);
    });
})();

// ======== RIGHT PANEL RESIZE ========
(function() {
    const handle = document.getElementById('right-resize');
    const panel = document.getElementById('right-panel');
    if (!handle || !panel) return;
    let dragging = false;
    handle.addEventListener('mousedown', (e) => {
        e.preventDefault();
        dragging = true;
        handle.classList.add('dragging');
        document.body.style.cursor = 'col-resize';
        document.body.style.userSelect = 'none';
    });
    document.addEventListener('mousemove', (e) => {
        if (!dragging) return;
        const parentRect = panel.parentElement.getBoundingClientRect();
        let w = parentRect.right - e.clientX;
        w = Math.max(200, Math.min(600, w));
        panel.style.width = w + 'px';
    });
    document.addEventListener('mouseup', () => {
        if (!dragging) return;
        dragging = false;
        handle.classList.remove('dragging');
        document.body.style.cursor = '';
        document.body.style.userSelect = '';
        const tab = S.tabs.find(t => t.id === S.activeTab);
        if (tab && tab.fitAddon) setTimeout(() => tab.fitAddon.fit(), 50);
    });
})();

// ======== KEYBOARD SHORTCUTS ========
document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey) {
        switch(e.key) {
            case 'T': e.preventDefault(); showConnectDialog(); break;
            case 'W': e.preventDefault(); if (S.activeTab) closeTab(S.activeTab); break;
            case 'D': e.preventDefault(); splitPane('horizontal'); break;
            case 'E': e.preventDefault(); splitPane('vertical'); break;
            case 'F': e.preventDefault(); {
                const tab = S.tabs.find(t => t.id === S.activeTab);
                if (tab && tab.searchAddon) {
                    const q = prompt('Search terminal:');
                    if (q) tab.searchAddon.findNext(q);
                }
            } break;
            case 'P': e.preventDefault(); toggleContext('ai'); break;
        }
    }
    // Ctrl+Tab to cycle
    if (e.ctrlKey && e.key === 'Tab') {
        e.preventDefault();
        const idx = S.tabs.findIndex(t => t.id === S.activeTab);
        const next = e.shiftKey ? (idx - 1 + S.tabs.length) % S.tabs.length : (idx + 1) % S.tabs.length;
        if (S.tabs[next]) switchTab(S.tabs[next].id);
    }
});

// ======== UTILITIES ========
async function apiFetch(url, opts = {}) {
    const headers = { 'Content-Type': 'application/json' };
    if (S.token) headers['Authorization'] = 'Bearer ' + S.token;
    const r = await fetch(url, { ...opts, headers: { ...headers, ...(opts.headers||{}) } });
    if (!r.ok) {
        const text = await r.text().catch(() => r.statusText);
        throw new Error(text);
    }
    return r.json();
}

function esc(s) { const d = document.createElement('div'); d.textContent = s||''; return d.innerHTML; }
function formatSize(bytes) {
    if (bytes < 1024) return bytes + 'B';
    if (bytes < 1048576) return (bytes/1024).toFixed(1) + 'KB';
    if (bytes < 1073741824) return (bytes/1048576).toFixed(1) + 'MB';
    return (bytes/1073741824).toFixed(1) + 'GB';
}

// ======== MATRIX DIGITAL RAIN ========
(function initRain() {
    const canvas = document.getElementById('rain-canvas');
    if (!canvas) return;
    const ctx = canvas.getContext('2d', { alpha: true });
    let cols = 0, drops = [], w = 0, h = 0;
    const FONT_SIZE = 16;
    // Katakana half-width block + a sprinkle of digits, Latin, and a few Rust-y glyphs
    const GLYPHS = 'ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789{}[]()<>=+-*/;:.,fn let mut impl self &';
    function resize() {
        w = canvas.width = window.innerWidth;
        h = canvas.height = window.innerHeight;
        cols = Math.floor(w / FONT_SIZE);
        drops = new Array(cols).fill(0).map(() => Math.random() * -50);
    }
    window.addEventListener('resize', resize);
    resize();
    let lastFrame = 0;
    function draw(t) {
        // ~18 fps — keep CPU footprint tiny
        if (t - lastFrame < 55) { requestAnimationFrame(draw); return; }
        lastFrame = t;
        // Fade trail
        ctx.fillStyle = 'rgba(6,9,5,0.08)';
        ctx.fillRect(0, 0, w, h);
        ctx.font = FONT_SIZE + "px 'Share Tech Mono', monospace";
        for (let i = 0; i < cols; i++) {
            const y = drops[i] * FONT_SIZE;
            const ch = GLYPHS.charAt(Math.floor(Math.random() * GLYPHS.length));
            // Head character = bright phosphor; every ~40th column uses rust-orange
            const isRust = (i % 43 === 0);
            ctx.fillStyle = isRust ? '#ce422b' : '#6bffa0';
            ctx.shadowColor = isRust ? 'rgba(206,66,43,0.9)' : 'rgba(51,255,102,0.9)';
            ctx.shadowBlur = 6;
            ctx.fillText(ch, i * FONT_SIZE, y);
            // Dim body characters above the head
            ctx.shadowBlur = 0;
            ctx.fillStyle = isRust ? 'rgba(206,66,43,0.35)' : 'rgba(51,255,102,0.35)';
            if (y - FONT_SIZE > 0) {
                const tail = GLYPHS.charAt(Math.floor(Math.random() * GLYPHS.length));
                ctx.fillText(tail, i * FONT_SIZE, y - FONT_SIZE);
            }
            // Reset when off-screen, with some randomness
            if (y > h && Math.random() > 0.975) drops[i] = 0;
            drops[i]++;
        }
        requestAnimationFrame(draw);
    }
    requestAnimationFrame(draw);
})();
"###;

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Coders",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };

    let body = html! {
        // Matrix digital rain — fixed behind everything
        canvas id="rain-canvas" {}
        // CRT overlays — above content, clicks pass through
        div class="crt-scan" {}
        div class="crt-vignette" {}
        div class="crt-flicker" {}
        // 90s CRT monitor bezel — plastic frame + screws + power LED + brand plate
        div class="crt-bezel" {}
        div class="crt-screw tl" {}
        div class="crt-screw tr" {}
        div class="crt-screw bl" {}
        div class="crt-screw br" {}
        div class="crt-led-label" { "PWR" }
        div class="crt-led" {}

        div class="workshop-root" {
            (top_bar_standard("Coders"))

            // Main layout: sidebar + terminal + context panel
            div class="workshop-body" {
                // Host sidebar
                div class="host-sidebar" id="host-sidebar" style="width:220px" {
                    div id="sidebar-content" {}
                }
                // Resize handle
                div class="sidebar-resize" id="sidebar-resize" {}

                // Terminal area
                div style="flex:1; display:flex; flex-direction:column; min-width:0" {
                    // Tab bar
                    div class="tab-bar" id="tab-bar" {}
                    // Terminal panes
                    div id="terminal-area" class="pane-container" style="flex:1;min-height:0" {}
                }

                // Right panel resize handle
                div class="right-resize" id="right-resize" {}

                // Right panel — always visible
                div class="right-panel" id="right-panel" style="width:340px" {
                    div class="context-tabs" id="context-tabs" {}
                    div class="context-body" id="context-body" {}
                }
            }
        }

        // xterm.js + addons
        link rel="stylesheet" href="/coders/xterm.css";
        script src="/coders/xterm.min.js" {}
        script src="/coders/xterm-addon-fit.js" {}
        script src="/coders/xterm-addon-search.js" {}
        script src="/coders/xterm-addon-web-links.js" {}
        script { (PreEscaped(PAGE_JS)) }
    };

    Html(shell(page, body).into_string())
}
