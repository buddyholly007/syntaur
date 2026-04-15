//! /coders page — web-based terminal with xterm.js.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

const EXTRA_STYLE: &str = r##"
/* Terminal chrome */
.term-container { background: #030712; border-radius: 0.5rem; overflow: hidden; border: 1px solid #1f2937; flex: 1; min-height: 0; display: flex; flex-direction: column; }
.tab-bar { display: flex; align-items: center; background: #111827; border-bottom: 1px solid #1f2937; padding: 0 0.5rem; height: 2.25rem; gap: 2px; overflow-x: auto; flex-shrink: 0; }
.tab { display: flex; align-items: center; gap: 0.375rem; padding: 0.25rem 0.75rem; font-size: 0.75rem; border-radius: 0.375rem 0.375rem 0 0; cursor: pointer; user-select: none; white-space: nowrap; color: #9ca3af; transition: all 0.15s; }
.tab:hover { color: #d1d5db; background: #1f2937; }
.tab.active { background: #030712; color: #fff; border-top: 2px solid #0ea5e9; }
.tab .close-btn { margin-left: 0.25rem; color: #4b5563; font-size: 0.625rem; line-height: 1; cursor: pointer; }
.tab .close-btn:hover { color: #ef4444; }
.tab .status-dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
.tab .status-dot.connected { background: #4ade80; }
.tab .status-dot.connecting { background: #fbbf24; }
.tab .status-dot.error { background: #ef4444; }
.add-tab { color: #6b7280; cursor: pointer; padding: 0 0.5rem; font-size: 1rem; }
.add-tab:hover { color: #d1d5db; }

/* Split panes */
.pane-container { display: flex; flex: 1; min-height: 0; min-width: 0; }
.pane-container.horizontal { flex-direction: column; }
.pane-container.vertical { flex-direction: row; }
.splitter { flex-shrink: 0; background: #1f2937; transition: background 0.15s; }
.splitter:hover { background: #0ea5e9; }
.splitter.horizontal { height: 4px; cursor: row-resize; }
.splitter.vertical { width: 4px; cursor: col-resize; }

/* Host sidebar */
.host-sidebar { min-width: 140px; max-width: 500px; flex-shrink: 0; background: #111827; border-right: none; display: flex; flex-direction: column; overflow-y: auto; overflow-x: hidden; }
.sidebar-resize { width: 5px; flex-shrink: 0; background: #1f2937; cursor: col-resize; transition: background 0.15s; }
.sidebar-resize:hover, .sidebar-resize.dragging { background: #0ea5e9; }
.host-item { display: flex; align-items: center; gap: 0.5rem; padding: 0.375rem 0.75rem; font-size: 0.8125rem; border-radius: 0.375rem; cursor: pointer; color: #d1d5db; }
.host-item:hover { background: #1f2937; }
.host-item.active { background: #1e3a5f; color: #fff; }
.sidebar-section { padding: 0.5rem 0.75rem; font-size: 0.6875rem; text-transform: uppercase; color: #6b7280; letter-spacing: 0.05em; }

/* Right panel — always visible AI chat + tabs */
.right-panel { min-width: 200px; max-width: 600px; flex-shrink: 0; background: #111827; display: flex; flex-direction: column; overflow: hidden; }
.right-resize { width: 5px; flex-shrink: 0; background: #1f2937; cursor: col-resize; transition: background 0.15s; }
.right-resize:hover, .right-resize.dragging { background: #0ea5e9; }
.context-tabs { display: flex; border-bottom: 1px solid #1f2937; }
.context-tab { flex: 1; text-align: center; padding: 0.5rem; font-size: 0.75rem; color: #6b7280; cursor: pointer; }
.context-tab:hover { color: #d1d5db; }
.context-tab.active { color: #0ea5e9; border-bottom: 2px solid #0ea5e9; }
.context-body { flex: 1; overflow-y: auto; padding: 0.75rem; display: flex; flex-direction: column; }
/* AI chat messages */
.ai-messages { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 0.5rem; padding-bottom: 0.5rem; }
.ai-msg { padding: 0.5rem 0.625rem; border-radius: 0.5rem; font-size: 0.8125rem; line-height: 1.4; max-width: 95%; word-wrap: break-word; }
.ai-msg.user { background: #1e3a5f; color: #e0f2fe; align-self: flex-end; }
.ai-msg.assistant { background: #1f2937; color: #d1d5db; align-self: flex-start; }
.ai-input-row { display: flex; gap: 0.375rem; padding-top: 0.5rem; border-top: 1px solid #1f2937; flex-shrink: 0; }

/* Connect dialog */
.connect-dialog { position: fixed; inset: 0; z-index: 50; display: flex; align-items: flex-start; justify-content: center; padding-top: 15vh; }
.connect-bg { position: absolute; inset: 0; background: rgba(0,0,0,0.6); backdrop-filter: blur(4px); }
.connect-box { position: relative; background: #1f2937; border: 1px solid #374151; border-radius: 0.75rem; width: 100%; max-width: 28rem; padding: 1.5rem; box-shadow: 0 25px 50px -12px rgba(0,0,0,0.5); }
.connect-box input, .connect-box select { width: 100%; padding: 0.5rem 0.75rem; background: #111827; border: 1px solid #374151; border-radius: 0.375rem; color: #f3f4f6; font-size: 0.875rem; outline: none; margin-top: 0.25rem; }
.connect-box input:focus, .connect-box select:focus { border-color: #0ea5e9; }
.connect-box label { font-size: 0.75rem; color: #9ca3af; }
.btn-primary { padding: 0.5rem 1rem; background: #0ea5e9; color: #fff; border-radius: 0.375rem; font-size: 0.875rem; cursor: pointer; border: none; }
.btn-primary:hover { background: #0284c7; }
.btn-secondary { padding: 0.5rem 1rem; background: #374151; color: #d1d5db; border-radius: 0.375rem; font-size: 0.875rem; cursor: pointer; border: none; }
.btn-secondary:hover { background: #4b5563; }

/* SFTP tree */
.sftp-item { display: flex; align-items: center; gap: 0.375rem; padding: 0.25rem 0.5rem; font-size: 0.75rem; cursor: pointer; border-radius: 0.25rem; }
.sftp-item:hover { background: #1f2937; }
.sftp-item.dir { color: #60a5fa; }
.sftp-item.file { color: #d1d5db; }

/* Snippet items */
.snippet-item { padding: 0.5rem; background: #1f2937; border-radius: 0.375rem; cursor: pointer; margin-bottom: 0.375rem; }
.snippet-item:hover { background: #374151; }
.snippet-item .name { font-size: 0.8125rem; color: #f3f4f6; }
.snippet-item .cmd { font-size: 0.6875rem; color: #6b7280; font-family: monospace; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

/* xterm overrides */
.xterm { padding: 4px; }
.xterm-viewport { scrollbar-width: thin; scrollbar-color: #374151 transparent; }
.xterm-viewport::-webkit-scrollbar { width: 6px; }
.xterm-viewport::-webkit-scrollbar-thumb { background: #374151; border-radius: 3px; }

@media (max-width: 767px) {
    .host-sidebar { min-width: 100px; }
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
    const defaults = [
        { name: 'This Machine', hostname: '127.0.0.1', port: 22, username: 'sean', auth_method: 'key', is_local: true, group_name: 'LAN', color: '#22c55e' },
        { name: 'openclawprod', hostname: '192.168.1.35', port: 22, username: 'sean', auth_method: 'key', group_name: 'LAN', color: '#0ea5e9' },
        { name: 'claudevm', hostname: '192.168.1.150', port: 22, username: 'sean', auth_method: 'key', group_name: 'LAN', color: '#a855f7' },
        { name: 'Gaming PC', hostname: '192.168.1.69', port: 22, username: 'sean', auth_method: 'key', group_name: 'LAN', color: '#f97316' },
        { name: 'Mac Mini', hostname: '192.168.1.58', port: 22, username: 'sean', auth_method: 'key', group_name: 'LAN', color: '#eab308' },
    ];
    for (const h of defaults) {
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
            background: '#030712',
            foreground: '#f3f4f6',
            cursor: '#0ea5e9',
            selectionBackground: 'rgba(14,165,233,0.3)',
            black: '#030712', red: '#ef4444', green: '#22c55e', yellow: '#eab308',
            blue: '#3b82f6', magenta: '#a855f7', cyan: '#06b6d4', white: '#f3f4f6',
            brightBlack: '#6b7280', brightRed: '#f87171', brightGreen: '#4ade80', brightYellow: '#facc15',
            brightBlue: '#60a5fa', brightMagenta: '#c084fc', brightCyan: '#22d3ee', brightWhite: '#ffffff',
        },
        cursorBlink: true,
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
function switchContextTab(panel) {
    S.contextPanel = panel;
    renderContext();
}

function renderContext() {
    const tabs = document.getElementById('context-tabs');
    const body = document.getElementById('context-body');
    if (!tabs || !body) return;

    let tabHtml = '';
    for (const [key, label] of [['ai','AI Chat'],['sftp','Files'],['health','Health']]) {
        tabHtml += `<div class="context-tab${S.contextPanel===key?' active':''}" onclick="switchContextTab('${key}')">${label}</div>`;
    }
    tabs.innerHTML = tabHtml;

    if (S.contextPanel === 'ai') {
        body.innerHTML = `
            <div class="ai-messages" id="ai-messages">
                <div class="ai-msg assistant">Ask me anything about the terminal — commands, errors, what to run next. I can see your terminal output for context.</div>
            </div>
            <div class="ai-input-row">
                <input id="ai-input" placeholder="Ask anything..." style="flex:1;padding:0.5rem 0.625rem;background:#030712;border:1px solid #374151;border-radius:0.375rem;color:#f3f4f6;font-size:0.8125rem;outline:none" onkeydown="if(event.key==='Enter')sendAiMsg()">
                <button class="btn-primary" style="padding:0.5rem 0.75rem;font-size:0.75rem" onclick="sendAiMsg()">Send</button>
            </div>`;
    } else if (S.contextPanel === 'sftp') {
        body.innerHTML = `
            <div style="display:flex;gap:0.375rem;margin-bottom:0.5rem">
                <input id="sftp-path" value="${esc(S.sftpPath)}" style="flex:1;padding:0.25rem 0.5rem;background:#030712;border:1px solid #374151;border-radius:0.25rem;color:#f3f4f6;font-size:0.75rem;outline:none" onkeydown="if(event.key==='Enter')browseSftp()">
                <button class="btn-secondary" style="padding:0.25rem 0.5rem;font-size:0.6875rem" onclick="browseSftp()">Go</button>
            </div>
            <div id="sftp-tree" style="font-size:0.75rem"></div>`;
        browseSftp();
    } else if (S.contextPanel === 'health') {
        body.innerHTML = '<div style="color:#6b7280;font-size:0.8125rem">Loading health metrics...</div>';
        loadHealth();
    }
}

// Init right panel on load — AI chat is default
setTimeout(() => { S.contextPanel = 'ai'; renderContext(); }, 100);

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

    msgs.innerHTML += `<div style="margin-top:0.5rem;color:#f3f4f6;font-size:0.8125rem"><strong>You:</strong> ${esc(text)}</div>`;
    msgs.innerHTML += `<div id="ai-response" style="margin-top:0.25rem;color:#9ca3af;font-size:0.8125rem">Thinking...</div>`;
    msgs.scrollTop = msgs.scrollHeight;

    try {
        const r = await apiFetch('/api/message', {
            method: 'POST',
            body: JSON.stringify({
                message: `Terminal context (last output):\n\`\`\`\n${termContext}\n\`\`\`\n\nUser question: ${text}`,
                agent: 'main',
            }),
        });
        const resp = document.getElementById('ai-response');
        if (resp) resp.innerHTML = `<strong>AI:</strong> ${esc(r.response || r.text || JSON.stringify(r))}`;
    } catch(e) {
        const resp = document.getElementById('ai-response');
        if (resp) resp.innerHTML = `<span style="color:#ef4444">Error: ${esc(e.message||e)}</span>`;
    }
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
"###;

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Coders",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };

    let body = html! {
        (top_bar_standard("Coders"))

        // Main layout: sidebar + terminal + context panel
        div style="display:flex; height:calc(100vh - 57px); overflow:hidden" {
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
