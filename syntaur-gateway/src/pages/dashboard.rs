//! /dashboard — migrated from static/dashboard.html. Structural markup and
//! embedded scripts live as raw-string consts below so their bytes
//! count as Rust and the file compiles type-checked through maud.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Dashboard",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! { (PreEscaped(BODY_HTML)) };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
  .card { @apply bg-gray-800 rounded-xl border border-gray-700 p-5; }
  .card-hover { @apply hover:border-gray-600 transition-colors cursor-pointer; }
  .badge { @apply inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium; }
  .badge-green { @apply bg-green-900/50 text-green-400; }
  .badge-yellow { @apply bg-yellow-900/50 text-yellow-400; }
  .badge-red { @apply bg-red-900/50 text-red-400; }
  .badge-blue { @apply bg-blue-900/50 text-blue-400; }
  .badge-gray { @apply bg-gray-700 text-gray-400; }
  .btn-primary { @apply bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-6 rounded-lg transition-colors; }
  .pulse { animation: pulse 2s infinite; }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:.6} }
  #chat-container { display: none; }
  #chat-container.open { display: flex; }
  .chat-md pre { white-space: pre-wrap; word-break: break-word; }
  .chat-md p + p { margin-top: 0.5rem; }
  @keyframes bounce { 0%,100%{transform:translateY(0)} 50%{transform:translateY(-4px)} }

  /* ═══════════════════════════════════════════════════════════════════
     Command-center dashboard — three-pane layout, calm-neutral aesthetic
     Left rail: agent + threads + modules | Main: chat | Right: today
     ═══════════════════════════════════════════════════════════════════ */
  :root {
    --dash-bg:        #0a0d12;    /* deepest — outside rails */
    --dash-rail:      #0f1319;    /* rail background */
    --dash-rail-2:    #141922;    /* rail hover / accent */
    --dash-line:      #1d232d;    /* subtle dividers */
    --dash-line-2:    #2a323e;    /* stronger divider */
    --dash-ink:       #e8eaed;    /* primary text */
    --dash-ink-dim:   #9aa3af;    /* secondary text */
    --dash-ink-mute:  #5d6673;    /* labels, metadata */
    --dash-ink-faint: #3b424d;    /* disabled */
    --dash-accent:    #7aa2ff;    /* calm blue accent (not neon sky) */
    --dash-accent-2:  #4d6fd0;
    --dash-success:   #7fbf8a;
    --dash-warn:      #f0b470;
    --dash-danger:    #d97a7a;
  }
  body.bg-gray-950 { background: var(--dash-bg) !important; color: var(--dash-ink); }

  /* Top-bar refinement — thinner, more neutral. */
  .border-b.border-gray-800.bg-gray-900\/50 {
    background: rgba(10,13,18,0.85) !important;
    border-color: var(--dash-line) !important;
    backdrop-filter: blur(8px);
  }

  /* Three-pane shell */
  .cc-shell {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr) 320px;
    height: calc(100vh - 45px);
    background: var(--dash-bg);
  }
  .cc-shell.focus-mode { grid-template-columns: 0 1fr 0; }
  .cc-shell.focus-mode .cc-left, .cc-shell.focus-mode .cc-right { opacity: 0; pointer-events: none; overflow: hidden; }
  @media (max-width: 1100px) {
    .cc-shell { grid-template-columns: 220px 1fr 280px; }
  }
  @media (max-width: 900px) {
    .cc-shell { grid-template-columns: 1fr; }
    .cc-left, .cc-right { display: none; }
  }

  /* ── LEFT RAIL ────────────────────────────────────────────────────── */
  .cc-left {
    background: var(--dash-rail);
    border-right: 1px solid var(--dash-line);
    display: flex; flex-direction: column;
    overflow: hidden;
    transition: opacity 0.2s, grid-template-columns 0.2s;
  }
  .cc-left-scroll { flex: 1; overflow-y: auto; padding: 12px 10px 8px; }
  .cc-left-scroll::-webkit-scrollbar { width: 5px; }
  .cc-left-scroll::-webkit-scrollbar-thumb { background: var(--dash-line-2); border-radius: 3px; }

  .cc-section-title {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--dash-ink-mute);
    padding: 14px 8px 6px;
  }

  /* Agent selector at the top of the left rail */
  .cc-agent-picker {
    margin: 8px 6px 6px;
    padding: 10px 12px;
    background: var(--dash-rail-2);
    border: 1px solid var(--dash-line);
    border-radius: 10px;
    display: flex; align-items: center; gap: 10px;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
    position: relative;
  }
  .cc-agent-picker:hover { border-color: var(--dash-line-2); }
  .cc-agent-avatar {
    width: 34px; height: 34px; border-radius: 9px;
    flex-shrink: 0; background: linear-gradient(135deg, var(--dash-accent), var(--dash-accent-2));
    display: grid; place-items: center;
    color: #0a0d12; font-weight: 700; font-size: 14px;
  }
  .cc-agent-identity { flex: 1; min-width: 0; }
  .cc-agent-name { font-size: 14px; font-weight: 600; color: var(--dash-ink); line-height: 1.2; }
  .cc-agent-status { font-size: 11px; color: var(--dash-ink-mute); margin-top: 2px; }
  .cc-agent-status .dot { display: inline-block; width: 6px; height: 6px; border-radius: 50%; background: var(--dash-success); margin-right: 4px; }

  /* Left-rail rows — recent threads + modules */
  .cc-row {
    display: flex; align-items: center; gap: 10px;
    padding: 7px 8px;
    font-size: 13px;
    color: var(--dash-ink-dim);
    border-radius: 7px;
    cursor: pointer;
    text-decoration: none;
    line-height: 1.3;
    transition: background 0.12s, color 0.12s;
  }
  .cc-row:hover { background: var(--dash-rail-2); color: var(--dash-ink); }
  .cc-row.active { background: var(--dash-rail-2); color: var(--dash-ink); }
  .cc-row-icon {
    flex-shrink: 0; width: 18px; height: 18px;
    display: grid; place-items: center;
    font-size: 13px;
  }
  .cc-row-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cc-row-badge {
    flex-shrink: 0; font-size: 10px; padding: 1px 7px;
    background: var(--dash-line-2); color: var(--dash-ink-dim);
    border-radius: 999px; font-weight: 500;
  }
  .cc-row-badge.active { background: rgba(122,162,255,0.2); color: var(--dash-accent); }
  .cc-row-sub { font-size: 11px; color: var(--dash-ink-mute); margin-top: 1px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .cc-row-thread { flex-direction: column; align-items: stretch; gap: 0; }
  .cc-row-thread .cc-row-label { font-size: 12.5px; color: var(--dash-ink-dim); }

  /* Footer pinned to bottom of left rail */
  .cc-left-footer {
    padding: 10px 12px;
    border-top: 1px solid var(--dash-line);
    font-size: 11px;
    color: var(--dash-ink-mute);
    display: flex; flex-direction: column; gap: 4px;
    background: rgba(10,13,18,0.6);
  }
  .cc-left-footer .footer-row { display: flex; align-items: center; gap: 6px; }
  .cc-left-footer .footer-row .dot { width: 6px; height: 6px; border-radius: 50%; background: var(--dash-success); flex-shrink: 0; }
  .cc-focus-toggle {
    margin-top: 2px;
    font-size: 10.5px; color: var(--dash-ink-mute);
    background: none; border: none; padding: 0; cursor: pointer;
    text-align: left;
  }
  .cc-focus-toggle:hover { color: var(--dash-ink-dim); }
  .cc-focus-toggle kbd {
    font-family: 'SF Mono', ui-monospace, monospace; font-size: 9px;
    background: var(--dash-line-2); color: var(--dash-ink-dim);
    padding: 1px 5px; border-radius: 3px; margin-left: 4px;
  }

  /* ── MAIN CANVAS ──────────────────────────────────────────────────── */
  .cc-main {
    display: flex; flex-direction: column;
    min-width: 0;
    background: var(--dash-bg);
    overflow: hidden;
  }
  .cc-persona-strip {
    padding: 10px 18px;
    border-bottom: 1px solid var(--dash-line);
    display: flex; align-items: center; gap: 10px;
    flex-shrink: 0;
  }
  .cc-persona-strip .cc-agent-avatar { width: 28px; height: 28px; border-radius: 7px; font-size: 12px; }
  .cc-persona-strip .name { font-size: 13.5px; font-weight: 600; color: var(--dash-ink); }
  .cc-persona-strip .whisper { font-size: 12px; color: var(--dash-ink-mute); margin-left: 6px; }
  .cc-persona-actions { margin-left: auto; display: flex; align-items: center; gap: 10px; font-size: 12px; color: var(--dash-ink-mute); }
  .cc-persona-actions button { background: none; border: none; cursor: pointer; color: var(--dash-ink-mute); padding: 4px 6px; border-radius: 5px; transition: all 0.12s; }
  .cc-persona-actions button:hover { color: var(--dash-ink); background: var(--dash-rail-2); }

  /* Hero empty-state: shown in chat area before any messages */
  .cc-hero {
    padding: 44px 28px 18px;
    max-width: 640px;
    margin: 0 auto;
    width: 100%;
  }
  .cc-hero-greeting {
    font-size: 26px;
    font-weight: 500;
    color: var(--dash-ink);
    letter-spacing: -0.01em;
    margin-bottom: 4px;
  }
  .cc-hero-greeting .accent { color: var(--dash-accent); }
  .cc-hero-subtitle {
    font-size: 14px;
    color: var(--dash-ink-mute);
    margin-bottom: 22px;
  }
  .cc-hero-prompts {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    margin-bottom: 12px;
  }
  @media (max-width: 700px) { .cc-hero-prompts { grid-template-columns: 1fr; } }
  .cc-hero-prompt {
    text-align: left;
    padding: 12px 14px;
    background: var(--dash-rail);
    border: 1px solid var(--dash-line);
    border-radius: 10px;
    font-size: 13px;
    color: var(--dash-ink-dim);
    cursor: pointer;
    line-height: 1.35;
    transition: all 0.12s;
  }
  .cc-hero-prompt:hover { border-color: var(--dash-line-2); color: var(--dash-ink); background: var(--dash-rail-2); }
  .cc-hero-prompt .ico { font-size: 14px; margin-right: 8px; opacity: 0.85; }

  /* Chat message area takes over once thread starts */
  .cc-chat-messages {
    flex: 1;
    overflow-y: auto;
    padding: 16px 28px;
    max-width: 760px;
    width: 100%;
    margin: 0 auto;
  }
  .cc-chat-messages.hidden-when-hero { display: none; }

  .cc-input-bar {
    padding: 12px 20px 16px;
    border-top: 1px solid var(--dash-line);
    background: var(--dash-bg);
    flex-shrink: 0;
  }
  .cc-input-inner {
    max-width: 760px;
    margin: 0 auto;
    display: flex; align-items: flex-end; gap: 8px;
    background: var(--dash-rail);
    border: 1px solid var(--dash-line-2);
    border-radius: 14px;
    padding: 8px 10px;
    transition: border-color 0.15s;
  }
  .cc-input-inner:focus-within { border-color: var(--dash-accent); }
  .cc-input-inner textarea {
    flex: 1;
    background: transparent !important;
    border: none !important;
    color: var(--dash-ink) !important;
    font-size: 14px;
    padding: 6px 4px;
    outline: none !important;
    resize: none;
    max-height: 180px;
  }
  .cc-input-inner textarea::placeholder { color: var(--dash-ink-mute); }
  .cc-input-btn {
    flex-shrink: 0;
    background: none; border: none;
    color: var(--dash-ink-mute);
    padding: 6px;
    cursor: pointer;
    border-radius: 7px;
    transition: all 0.12s;
  }
  .cc-input-btn:hover { color: var(--dash-ink); background: var(--dash-rail-2); }
  .cc-input-send {
    background: var(--dash-accent) !important;
    color: #0a0d12 !important;
  }
  .cc-input-send:hover { background: #9bbcff !important; }

  /* ── RIGHT RAIL ───────────────────────────────────────────────────── */
  .cc-right {
    background: var(--dash-rail);
    border-left: 1px solid var(--dash-line);
    display: flex; flex-direction: column;
    overflow: hidden;
    transition: opacity 0.2s;
  }
  .cc-right-scroll { flex: 1; overflow-y: auto; padding: 16px 14px 12px; }
  .cc-right-scroll::-webkit-scrollbar { width: 5px; }
  .cc-right-scroll::-webkit-scrollbar-thumb { background: var(--dash-line-2); border-radius: 3px; }
  .cc-block {
    background: var(--dash-bg);
    border: 1px solid var(--dash-line);
    border-radius: 10px;
    padding: 12px 14px;
    margin-bottom: 10px;
  }
  .cc-block-title {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--dash-ink-mute);
    display: flex; align-items: center; justify-content: space-between;
    margin-bottom: 8px;
  }
  .cc-block-title .count { color: var(--dash-ink-faint); font-weight: 500; }

  /* NEXT up */
  .cc-next { padding: 14px; }
  .cc-next.has-event { border-color: rgba(122,162,255,0.3); background: linear-gradient(135deg, rgba(122,162,255,0.06), transparent); }
  .cc-next-headline { font-size: 14px; font-weight: 500; color: var(--dash-ink); line-height: 1.3; }
  .cc-next-countdown { font-size: 22px; font-weight: 600; color: var(--dash-accent); margin-top: 4px; letter-spacing: -0.02em; }
  .cc-next-countdown.warn { color: var(--dash-warn); }
  .cc-next-countdown.danger { color: var(--dash-danger); }
  .cc-next-meta { font-size: 11px; color: var(--dash-ink-mute); margin-top: 2px; }
  .cc-next.empty .cc-next-headline { color: var(--dash-ink-mute); font-weight: 400; font-size: 13px; }

  /* Today tasks */
  .cc-todo-row {
    display: flex; align-items: center; gap: 8px;
    padding: 5px 0;
    font-size: 13px;
    color: var(--dash-ink);
  }
  .cc-todo-row input[type="checkbox"] {
    width: 15px; height: 15px;
    accent-color: var(--dash-accent);
    cursor: pointer;
  }
  .cc-todo-row.done { color: var(--dash-ink-mute); text-decoration: line-through; }
  .cc-todo-input-row {
    margin-top: 8px;
    display: flex; gap: 6px;
  }
  .cc-todo-input-row input {
    flex: 1;
    background: var(--dash-rail-2) !important;
    border: 1px solid var(--dash-line) !important;
    color: var(--dash-ink) !important;
    font-size: 12.5px;
    padding: 5px 9px;
    border-radius: 6px;
    outline: none;
  }
  .cc-todo-input-row input:focus { border-color: var(--dash-accent) !important; }
  .cc-todo-input-row button {
    background: var(--dash-rail-2); color: var(--dash-ink-dim);
    border: 1px solid var(--dash-line);
    padding: 4px 10px; font-size: 12px; border-radius: 6px; cursor: pointer;
  }
  .cc-todo-input-row button:hover { color: var(--dash-ink); border-color: var(--dash-line-2); }

  /* Week strip calendar — Mon-Sun with dots */
  .cc-week {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
    margin-top: 4px;
  }
  .cc-week-day {
    position: relative;
    padding: 5px 0 7px;
    border-radius: 7px;
    text-align: center;
    cursor: pointer;
    transition: background 0.12s;
  }
  .cc-week-day:hover { background: var(--dash-rail-2); }
  .cc-week-day.today { background: rgba(122,162,255,0.12); }
  .cc-week-day.today .cc-week-num { color: var(--dash-accent); font-weight: 600; }
  .cc-week-day.has-event::after {
    content: ''; position: absolute; bottom: 3px; left: 50%; transform: translateX(-50%);
    width: 4px; height: 4px; border-radius: 50%;
    background: var(--dash-accent);
  }
  .cc-week-day.today.has-event::after { background: var(--dash-warn); }
  .cc-week-dow { font-size: 9.5px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--dash-ink-mute); font-weight: 500; }
  .cc-week-num { font-size: 13.5px; color: var(--dash-ink-dim); margin-top: 1px; font-weight: 500; }
  .cc-week-nav {
    display: flex; align-items: center; justify-content: space-between;
    font-size: 12px; color: var(--dash-ink-dim); margin-bottom: 4px;
  }
  .cc-week-nav button { background: none; border: none; color: var(--dash-ink-mute); cursor: pointer; padding: 2px 6px; border-radius: 4px; }
  .cc-week-nav button:hover { color: var(--dash-ink); background: var(--dash-rail-2); }

  /* Day detail panel (reused from old calendar) — reposition inside block */
  #cal-day-panel {
    background: var(--dash-rail-2);
    border-radius: 7px;
    padding: 9px 10px !important;
    margin-top: 9px !important;
    border: 1px solid var(--dash-line) !important;
  }
  #cal-add-form {
    background: var(--dash-rail-2);
    border-radius: 7px;
    padding: 9px 10px !important;
    margin-top: 9px !important;
    border: 1px solid var(--dash-line) !important;
  }

  /* Ambient footer */
  .cc-ambient {
    padding: 10px 14px;
    border-top: 1px solid var(--dash-line);
    font-size: 11.5px;
    color: var(--dash-ink-mute);
    display: flex; flex-direction: column; gap: 5px;
    background: rgba(10,13,18,0.4);
    flex-shrink: 0;
  }
  .cc-ambient .line { display: flex; align-items: center; gap: 7px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .cc-ambient .line .ico { flex-shrink: 0; font-size: 11px; opacity: 0.7; }
  .cc-ambient .line .val { color: var(--dash-ink-dim); }"##;

const BODY_HTML: &str = r##"<!-- Login overlay -->
<div id="login-overlay" class="fixed inset-0 z-50 bg-gray-950 flex items-center justify-center">
  <div class="w-full max-w-sm px-4">
    <div class="text-center mb-8">
      <img src="/app-icon.jpg" class="w-16 h-16 rounded-2xl mx-auto" alt="Syntaur">
      <h1 class="text-2xl font-bold mt-2">Syntaur</h1>
      <p class="text-gray-400 text-sm mt-1">Sign in to your dashboard</p>
    </div>
    <div class="bg-gray-800 rounded-xl border border-gray-700 p-6">
      <div class="space-y-4">
        <div id="login-username-row" class="hidden">
          <label class="block text-sm font-medium text-gray-300 mb-1.5">Username</label>
          <input type="text" id="login-user" class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none" placeholder="Username" autocomplete="username">
        </div>
        <div>
          <label class="block text-sm font-medium text-gray-300 mb-1.5">Password</label>
          <input type="password" id="login-pass" class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none" placeholder="Dashboard password" onkeydown="if(event.key==='Enter')doLogin()" autocomplete="current-password">
        </div>
        <button onclick="doLogin()" class="w-full bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-6 rounded-lg transition-colors" id="login-btn">Sign In</button>
        <p id="login-error" class="text-sm text-red-400 hidden"></p>
        <p class="text-center text-xs text-gray-500">
          <a href="#" onclick="toggleUsernameField()" id="login-toggle" class="hover:text-gray-300">Sign in with username</a>
        </p>
      </div>
    </div>
  </div>
</div>

<!-- Top bar -->
<div class="border-b border-gray-800 bg-gray-900/50 backdrop-blur sticky top-0 z-40">
  <div class="px-4 py-2.5 flex items-center justify-between">
    <div class="flex items-center gap-3">
      <img src="/app-icon.jpg" class="h-8 w-8 rounded-lg" alt="">
      <span class="font-semibold">Syntaur</span>
      <span class="badge badge-green text-xs" id="status-badge">
        <span class="w-1.5 h-1.5 rounded-full bg-green-400 mr-1"></span>Online
      </span>
      <span id="license-badge" class="badge badge-gray text-xs hidden"></span>
    </div>
    <div class="flex items-center gap-3 text-sm">
      <button onclick="showPanel('main')" class="text-gray-400 hover:text-white transition-colors" id="nav-main">Home</button>
      <button onclick="showPanel('more')" class="text-gray-400 hover:text-white transition-colors" id="nav-more">Modules &amp; System</button>
      <a href="/settings" class="text-gray-400 hover:text-gray-300">Settings</a>
      <a href="/profile" class="text-gray-400 hover:text-gray-300" id="user-label" title="Profile"></a>
      <button onclick="doLogout()" class="text-gray-500 hover:text-red-400 transition-colors" title="Sign out">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
      </button>
    </div>
  </div>
  <!-- Mini Player (hidden when nothing playing) -->
  <div id="mini-player" class="hidden border-t border-gray-800/50 px-4 py-1.5 flex items-center gap-3 text-xs bg-gray-950/60">
    <a href="/music" class="flex-shrink-0 text-gray-500 hover:text-oc-500" title="Open Music">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
    </a>
    <div class="flex-1 min-w-0 overflow-hidden">
      <div class="flex items-center gap-2">
        <span id="mp-source-icon" class="flex-shrink-0"></span>
        <div class="flex-1 min-w-0 overflow-hidden whitespace-nowrap">
          <span id="mp-song" class="text-gray-200 font-medium">—</span>
          <span class="text-gray-600"> · </span>
          <span id="mp-artist" class="text-gray-500">—</span>
        </div>
      </div>
    </div>
    <div class="flex-shrink-0 flex items-center gap-1">
      <button onclick="mpControl('prev')" class="text-gray-500 hover:text-gray-200 px-1" title="Previous">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><polygon points="19,20 9,12 19,4"/><rect x="5" y="4" width="2" height="16"/></svg>
      </button>
      <button onclick="mpControl('play_pause')" id="mp-play" class="text-gray-300 hover:text-white px-1" title="Play/Pause">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>
      </button>
      <button onclick="mpControl('next')" class="text-gray-500 hover:text-gray-200 px-1" title="Next">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,4 15,12 5,20"/><rect x="17" y="4" width="2" height="16"/></svg>
      </button>
      <a href="/music" class="text-gray-600 hover:text-gray-300 px-1 text-xs" title="Expand">⤢</a>
    </div>
  </div>
</div>

<!-- Main panel: Command-center three-pane dashboard -->
<div class="cc-shell" id="panel-main">

  <!-- ────── LEFT RAIL: agent + threads + modules + status ────── -->
  <aside class="cc-left" id="cc-left">
    <div class="cc-left-scroll">
      <!-- Active agent picker -->
      <div class="cc-agent-picker" id="dash-agent-switcher" onclick="toggleDashAgentMenu()">
        <div class="cc-agent-avatar" id="cc-agent-initial">P</div>
        <div class="cc-agent-identity">
          <div class="cc-agent-name"><span id="chat-agent-name">Peter</span></div>
          <div class="cc-agent-status"><span class="dot"></span>online · ready</div>
        </div>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="color:var(--dash-ink-mute);flex-shrink:0"><path d="M6 9l6 6 6-6"/></svg>
        <div id="dash-agent-menu" class="hidden absolute left-2 right-2 top-full mt-1 bg-gray-800 border border-gray-700 rounded-lg shadow-xl py-1 z-50"></div>
      </div>

      <!-- Recent threads -->
      <div class="cc-section-title">Recent threads</div>
      <div id="recent-threads-list">
        <div class="cc-row cc-row-thread" style="color:var(--dash-ink-faint);padding:7px 8px">
          <div class="cc-row-label">No prior conversations yet</div>
        </div>
      </div>

      <!-- Modules navigation -->
      <div class="cc-section-title">Modules</div>
      <div id="module-nav-list">
        <a href="/tax" class="cc-row">
          <span class="cc-row-icon" style="color:#4ade80">&#128176;</span>
          <span class="cc-row-label">Tax</span>
          <span class="cc-row-badge hidden" id="mod-badge-tax"></span>
        </a>
        <a href="/knowledge" class="cc-row">
          <span class="cc-row-icon" style="color:#fbbf24">&#128218;</span>
          <span class="cc-row-label">Knowledge</span>
          <span class="cc-row-badge hidden" id="mod-badge-knowledge"></span>
        </a>
        <a href="/music" class="cc-row">
          <span class="cc-row-icon" style="color:#ec4899">&#127925;</span>
          <span class="cc-row-label">Music</span>
          <span class="cc-row-badge hidden" id="mod-badge-music"></span>
        </a>
        <a href="/coders" class="cc-row">
          <span class="cc-row-icon" style="color:#34d399">&#9000;</span>
          <span class="cc-row-label">Terminal</span>
        </a>
        <a href="/journal" class="cc-row">
          <span class="cc-row-icon" style="color:#60a5fa">&#128214;</span>
          <span class="cc-row-label">Journal</span>
        </a>
        <a href="/social" class="cc-row">
          <span class="cc-row-icon" style="color:#d49a3a">&#9733;</span>
          <span class="cc-row-label">Social</span>
          <span class="cc-row-badge hidden" id="mod-badge-social"></span>
        </a>
        <a href="/chat" class="cc-row">
          <span class="cc-row-icon" style="color:#a78bfa">&#127908;</span>
          <span class="cc-row-label">Voice chat</span>
        </a>
        <a href="/modules" class="cc-row" style="color:var(--dash-ink-mute)">
          <span class="cc-row-icon">&#9881;</span>
          <span class="cc-row-label">All modules &amp; settings</span>
        </a>
      </div>

      <!-- Quick actions -->
      <div class="cc-section-title">Quick actions</div>
      <a href="/history" class="cc-row">
        <span class="cc-row-icon" style="color:var(--dash-ink-mute)">&#128340;</span>
        <span class="cc-row-label">Conversation history</span>
      </a>
      <button onclick="showPhoneAccess()" class="cc-row" style="width:100%;background:none;border:none;font:inherit;text-align:left">
        <span class="cc-row-icon" style="color:var(--dash-ink-mute)">&#128241;</span>
        <span class="cc-row-label">Open on phone</span>
      </button>
    </div>

    <!-- Footer: system status + focus toggle -->
    <div class="cc-left-footer">
      <div class="footer-row"><span class="dot"></span><span><span id="stat-modules">--</span> modules · <span id="stat-tools">-- tools</span></span></div>
      <div class="footer-row" style="color:var(--dash-ink-faint)"><span id="stat-uptime">--</span> uptime · <span id="stat-agents">--</span></div>
      <button class="cc-focus-toggle" onclick="toggleFocusMode()">Focus mode <kbd>⌘ .</kbd></button>
    </div>
  </aside>

  <!-- ────── MAIN CANVAS: persona strip + hero/chat + input ────── -->
  <main class="cc-main">
    <!-- Persona presence strip -->
    <div class="cc-persona-strip">
      <div class="cc-agent-avatar" style="width:26px;height:26px;font-size:11px;border-radius:7px" id="cc-persona-initial">P</div>
      <span class="name" id="cc-persona-name">Peter</span>
      <span class="whisper" id="cc-persona-whisper">here &amp; ready</span>
      <div class="cc-persona-actions">
        <div class="flex bg-gray-800 rounded-lg text-xs" style="background:var(--dash-rail);border:1px solid var(--dash-line)">
          <button onclick="showChatTab('chat')" id="tab-chat" class="px-2.5 py-1 rounded-lg" style="background:var(--dash-rail-2);color:var(--dash-ink)">Chat</button>
          <button onclick="showChatTab('telegram')" id="tab-telegram" class="px-2.5 py-1 rounded-lg" style="color:var(--dash-ink-mute)">Telegram</button>
        </div>
        <button onclick="clearChat()" title="New thread">New</button>
        <button onclick="openFullChat()" title="Expand to full-screen chat">Expand</button>
      </div>
    </div>

    <!-- Empty-state hero — hidden once a conversation starts -->
    <div class="cc-hero" id="cc-hero">
      <div class="cc-hero-greeting"><span id="greeting-hello">Good evening</span>, <span class="accent" id="greeting-name">Sean</span>.</div>
      <div class="cc-hero-subtitle" id="greeting-subtitle">Ready when you are.</div>
      <div class="cc-hero-prompts" id="cc-hero-prompts">
        <button class="cc-hero-prompt" onclick="quickChat('What is on my calendar today?')">
          <span class="ico">📅</span> What's on my calendar today?
        </button>
        <button class="cc-hero-prompt" onclick="quickChat('Summarize any new activity I should know about.')">
          <span class="ico">✨</span> What should I know about today?
        </button>
        <button class="cc-hero-prompt" onclick="quickChat('What can you do?')">
          <span class="ico">💡</span> What can you do?
        </button>
        <button class="cc-hero-prompt" onclick="quickChat('Pick up where we left off yesterday.')">
          <span class="ico">↺</span> Resume yesterday's thread
        </button>
      </div>
    </div>

    <!-- Chat messages (populated by existing JS) — hidden while hero is shown -->
    <div class="cc-chat-messages hidden-when-hero" id="chat-messages">
      <div class="flex gap-3">
        <img src="/agent-avatar/main" class="w-7 h-7 rounded-full flex-shrink-0 mt-0.5" alt="">
        <div class="flex-1 text-sm text-gray-300" id="chat-greeting">
          <p id="welcome-sub">Hey! How can I help you?</p>
        </div>
      </div>
    </div>

    <!-- Telegram messages (alternative view when Telegram tab active) -->
    <div class="flex-1 overflow-y-auto p-4 space-y-3 hidden" id="telegram-messages">
      <p class="text-xs text-gray-600 text-center py-4">Loading Telegram messages...</p>
    </div>

    <!-- Input bar -->
    <div class="cc-input-bar" id="chat-input-area">
      <div id="chat-attachments" class="hidden mb-2 space-y-1" style="max-width:760px;margin:0 auto"></div>
      <div class="cc-input-inner">
        <button class="cc-input-btn" onclick="document.getElementById('chat-file-input').click()" title="Attach file">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/></svg>
        </button>
        <input type="file" id="chat-file-input" multiple class="hidden" onchange="attachFiles(this.files)">
        <textarea id="chat-input" rows="1" placeholder="Message Peter…" onkeydown="chatKeydown(event)" oninput="autoResize(this)"></textarea>
        <button class="cc-input-btn" title="Voice input (coming)" onclick="alert('Voice input from dashboard — TODO')">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
        </button>
        <button onclick="sendMessage()" class="cc-input-btn cc-input-send" id="send-btn" title="Send">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M22 2L11 13"/><path d="M22 2L15 22L11 13L2 9L22 2Z"/></svg>
        </button>
      </div>
    </div>
  </main>

  <!-- ────── RIGHT RAIL: Today ────── -->
  <aside class="cc-right" id="cc-right">
    <div class="cc-right-scroll">
      <!-- NEXT up -->
      <div class="cc-block cc-next empty" id="cc-next-block">
        <div class="cc-block-title"><span>Next up</span></div>
        <div class="cc-next-headline" id="next-up-text">Nothing imminent — a clear window.</div>
        <div class="cc-next-countdown hidden" id="next-up-countdown"></div>
        <div class="cc-next-meta hidden" id="next-up-meta"></div>
      </div>

      <!-- Today — tasks -->
      <div class="cc-block">
        <div class="cc-block-title"><span>Today</span><span class="count" id="todo-count">0</span></div>
        <div id="todo-list"></div>
        <div class="cc-todo-input-row">
          <input type="text" id="todo-input" placeholder="Add a task…" onkeydown="if(event.key==='Enter')addTodo()">
          <button onclick="addTodo()">Add</button>
        </div>
      </div>

      <!-- Week strip calendar (+ Month/Agenda toggle) -->
      <div class="cc-block">
        <div class="cc-block-title">
          <span>This week</span>
          <span class="flex items-center gap-1" style="background:var(--dash-rail-2);border:1px solid var(--dash-line);border-radius:5px;padding:1px">
            <button onclick="setCalView('week')" id="view-week-btn" style="font-size:9.5px;padding:2px 7px;background:var(--dash-rail);color:var(--dash-ink);border:none;border-radius:3px;cursor:pointer">Week</button>
            <button onclick="setCalView('month')" id="view-month-btn" style="font-size:9.5px;padding:2px 7px;background:none;color:var(--dash-ink-mute);border:none;border-radius:3px;cursor:pointer">Month</button>
            <button onclick="setCalView('agenda')" id="view-agenda-btn" style="font-size:9.5px;padding:2px 7px;background:none;color:var(--dash-ink-mute);border:none;border-radius:3px;cursor:pointer">Agenda</button>
            <label title="Import .ics" style="font-size:10px;padding:2px 5px;color:var(--dash-ink-mute);cursor:pointer">
              &#8615;<input type="file" accept=".ics,text/calendar" class="hidden" onchange="importIcs(event)">
            </label>
          </span>
        </div>
        <div class="cc-week-nav">
          <button onclick="calPrev()">&larr;</button>
          <span id="cal-title">This week</span>
          <button onclick="calNext()">&rarr;</button>
        </div>
        <!-- Week strip (default view) -->
        <div id="view-week">
          <div class="cc-week" id="cal-week-strip">
            <!-- Populated by JS -->
          </div>
        </div>
        <!-- Month view (hidden by default) -->
        <div id="view-month" class="hidden">
          <div class="grid grid-cols-7 gap-0.5 text-center" style="font-size:9.5px;color:var(--dash-ink-mute);margin-top:2px">
            <span class="py-1">Su</span><span class="py-1">Mo</span><span class="py-1">Tu</span><span class="py-1">We</span><span class="py-1">Th</span><span class="py-1">Fr</span><span class="py-1">Sa</span>
          </div>
          <div class="grid grid-cols-7 gap-0.5 text-center text-xs" id="cal-grid"></div>
        </div>
        <!-- Agenda -->
        <div id="view-agenda" class="hidden max-h-80 overflow-y-auto" style="margin-top:6px"></div>

        <!-- Day detail panel (shown when a date is clicked) -->
        <div id="cal-day-panel" class="hidden">
          <div class="flex items-center justify-between mb-2">
            <h4 class="text-sm font-medium" style="color:var(--dash-ink)" id="cal-day-title">Date</h4>
            <div class="flex gap-1">
              <button onclick="showAddEvent()" style="font-size:10.5px;background:var(--dash-accent);color:#0a0d12;padding:2px 8px;border-radius:4px;border:none;cursor:pointer">+ Add</button>
              <button onclick="closeDayPanel()" style="font-size:12px;color:var(--dash-ink-mute);background:none;border:none;cursor:pointer;padding:0 4px">&times;</button>
            </div>
          </div>
          <div id="cal-day-events" class="space-y-1" style="max-height:160px;overflow-y:auto">
            <p style="font-size:11.5px;color:var(--dash-ink-mute);font-style:italic">No events</p>
          </div>
        </div>

        <!-- Add/edit event form -->
        <div id="cal-add-form" class="hidden space-y-2">
          <h4 class="text-sm font-medium" style="color:var(--dash-ink)" id="cal-form-title">New Event</h4>
          <input type="text" id="cal-ev-title" class="w-full" placeholder="Event title" style="background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink);font-size:12.5px;padding:5px 8px;border-radius:5px;outline:none">
          <div class="flex gap-2">
            <input type="date" id="cal-ev-date" class="flex-1" style="background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink);font-size:12px;padding:5px 8px;border-radius:5px;outline:none">
            <input type="time" id="cal-ev-time" style="width:90px;background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink);font-size:12px;padding:5px 8px;border-radius:5px;outline:none">
          </div>
          <div class="flex gap-2 items-center flex-wrap">
            <label class="flex items-center gap-1.5 text-xs cursor-pointer" style="color:var(--dash-ink-mute)">
              <input type="checkbox" id="cal-ev-allday" onchange="document.getElementById('cal-ev-time').disabled=this.checked"> All day
            </label>
            <select id="cal-ev-recur" title="Recurrence" style="background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink-dim);font-size:11px;padding:3px 4px;border-radius:5px;outline:none">
              <option value="">Doesn't repeat</option>
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
              <option value="monthly">Monthly</option>
              <option value="yearly">Yearly</option>
            </select>
            <input type="number" id="cal-ev-remind" min="0" max="10080" placeholder="Remind (min)" title="Minutes before (0 = off)" style="width:88px;background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink);font-size:11px;padding:3px 6px;border-radius:5px;outline:none">
          </div>
          <input type="date" id="cal-ev-recur-end" class="w-full hidden" placeholder="Repeat until" style="background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink-dim);font-size:11px;padding:5px 8px;border-radius:5px;outline:none">
          <textarea id="cal-ev-desc" class="w-full resize-none" rows="2" placeholder="Description (optional)" style="background:var(--dash-rail);border:1px solid var(--dash-line);color:var(--dash-ink);font-size:12px;padding:5px 8px;border-radius:5px;outline:none"></textarea>
          <div class="flex gap-2 justify-end">
            <button onclick="cancelAddEvent()" style="font-size:11px;color:var(--dash-ink-mute);background:none;border:none;padding:4px 8px;cursor:pointer">Cancel</button>
            <button onclick="saveEvent()" id="cal-save-btn" style="font-size:11px;background:var(--dash-accent);color:#0a0d12;padding:4px 10px;border:none;border-radius:5px;cursor:pointer">Save</button>
          </div>
        </div>
      </div>
    </div>

    <!-- Ambient footer -->
    <div class="cc-ambient">
      <div class="line"><span class="ico">♪</span><span class="val" id="ambient-music">—</span></div>
      <div class="line"><span class="ico">🎙</span><span class="val" id="ambient-voice">Satellites quiet</span></div>
    </div>
  </aside>

</div>

<!-- Modules & System panel (hidden by default, shown on nav click) -->
<div class="hidden h-[calc(100vh-45px)] overflow-y-auto p-6" id="panel-more">
  <div class="max-w-4xl mx-auto space-y-6">

    <!-- Status cards -->
    <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
      <div class="card">
        <p class="text-xs text-gray-500 uppercase tracking-wider">Uptime</p>
        <p class="text-2xl font-semibold mt-1" id="stat-uptime2">--</p>
      </div>
      <div class="card">
        <p class="text-xs text-gray-500 uppercase tracking-wider">LLM Backend</p>
        <p class="text-sm font-medium mt-1 text-gray-300" id="stat-llm">Loading...</p>
        <p class="text-xs text-gray-500 mt-0.5" id="stat-llm-status"></p>
      </div>
      <div class="card">
        <p class="text-xs text-gray-500 uppercase tracking-wider">Modules</p>
        <p class="text-2xl font-semibold mt-1" id="stat-modules2">--</p>
      </div>
      <div class="card">
        <p class="text-xs text-gray-500 uppercase tracking-wider">Agents</p>
        <p class="text-sm font-medium mt-1 text-gray-300" id="stat-agents2">--</p>
      </div>
    </div>

    <!-- Modules grid -->
    <div class="card">
      <div class="flex items-center justify-between mb-3">
        <h3 class="font-medium text-sm text-gray-400">Active Modules</h3>
        <span class="text-xs text-gray-500" id="module-count"></span>
      </div>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-2" id="module-grid"></div>
    </div>

    <!-- System -->
    <div class="card">
      <div class="flex items-center justify-between mb-3">
        <h3 class="font-medium text-sm text-gray-400">System</h3>
        <span class="text-xs text-gray-500" id="sys-version"></span>
      </div>
      <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm" id="sys-info"></div>
    </div>

    <div class="text-center">
      <button onclick="showPanel('main')" class="text-sm text-gray-500 hover:text-gray-300">&larr; Back to Dashboard</button>
    </div>
  </div>
</div>

<!-- Phone access modal -->
<div id="phone-modal" class="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm hidden flex items-center justify-center p-4">
  <div class="bg-gray-800 rounded-2xl border border-gray-700 max-w-sm w-full p-6">
    <div class="flex items-center justify-between mb-4">
      <h3 class="font-semibold text-lg">Open on Your Phone</h3>
      <button onclick="document.getElementById('phone-modal').classList.add('hidden')" class="text-gray-400 hover:text-white text-xl">&times;</button>
    </div>
    <div class="text-center">
      <p class="text-sm text-gray-400 mb-4">Scan this QR code with your phone's camera:</p>
      <div class="bg-white p-4 rounded-xl inline-block">
        <img id="phone-qr" src="" alt="QR Code" class="w-48 h-48">
      </div>
      <p class="text-xs text-gray-500 font-mono mt-3 select-all" id="phone-url"></p>
    </div>
    <p class="text-xs text-gray-600 mt-3 text-center">Your phone must be on the same network, or use Tailscale for remote access.</p>
  </div>
</div>

<!-- Hidden elements for backward compat with JS -->
<span id="welcome-text" class="hidden"></span>
<span id="chat-container" class="hidden"></span>

<script>
let authToken = sessionStorage.getItem('syntaur_token') || '';

// Panel switching
function showPanel(name) {
  document.getElementById('panel-main').classList.toggle('hidden', name !== 'main');
  document.getElementById('panel-more').classList.toggle('hidden', name !== 'more');
  document.getElementById('nav-main').classList.toggle('text-white', name === 'main');
  document.getElementById('nav-more').classList.toggle('text-white', name === 'more');
  document.getElementById('nav-main').classList.toggle('text-gray-400', name !== 'main');
  document.getElementById('nav-more').classList.toggle('text-gray-400', name !== 'more');
}

function openFullChat() { location.href = '/chat'; }

// ── Chat/Telegram tab switching ──
let currentChatTab = 'chat';
let telegramLoaded = false;

function showChatTab(tab) {
  currentChatTab = tab;
  document.getElementById('chat-messages').classList.toggle('hidden', tab !== 'chat');
  document.getElementById('telegram-messages').classList.toggle('hidden', tab !== 'telegram');
  document.getElementById('chat-input-area').classList.toggle('hidden', tab !== 'chat');
  document.getElementById('tab-chat').className = `px-2.5 py-1 rounded-lg transition-colors ${tab === 'chat' ? 'bg-gray-700 text-gray-200' : 'text-gray-500 hover:text-gray-300'}`;
  document.getElementById('tab-telegram').className = `px-2.5 py-1 rounded-lg transition-colors ${tab === 'telegram' ? 'bg-gray-700 text-gray-200' : 'text-gray-500 hover:text-gray-300'}`;
  if (tab === 'telegram' && !telegramLoaded) loadTelegramMessages();
}

async function loadTelegramMessages() {
  const container = document.getElementById('telegram-messages');
  try {
    const resp = await authFetch(`/messages?token=${authToken}&n=50`);
    const messages = await resp.json();
    telegramLoaded = true;
    if (!messages || messages.length === 0) {
      container.innerHTML = '<div class="text-center py-8"><p class="text-sm text-gray-500">No Telegram messages yet</p><p class="text-xs text-gray-600 mt-2">Connect a Telegram bot in Settings to chat from your phone.</p><p class="text-xs text-gray-600 mt-1">Messages sent via Telegram will appear here.</p></div>';
      return;
    }
    container.innerHTML = messages.reverse().map(m => {
      const from = m.from || m.sender || 'Unknown';
      const text = m.text || m.message || '';
      const time = m.timestamp ? new Date(m.timestamp * 1000).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'}) : '';
      const date = m.timestamp ? new Date(m.timestamp * 1000).toLocaleDateString() : '';
      const isUser = m.direction === 'incoming' || m.from_user;
      return `
        <div class="flex gap-2 ${isUser ? '' : 'justify-end'}">
          ${isUser ? '<div class="w-6 h-6 rounded-full bg-blue-600 flex items-center justify-center text-xs flex-shrink-0">T</div>' : ''}
          <div class="max-w-[80%] ${isUser ? 'bg-gray-800 border border-gray-700' : 'bg-oc-900/40 border border-oc-800/40'} rounded-xl px-3 py-2">
            ${isUser ? '<p class="text-xs text-gray-500 mb-0.5">' + escapeHtml(from) + '</p>' : ''}
            <p class="text-sm text-gray-300">${escapeHtml(text)}</p>
            <p class="text-xs text-gray-600 mt-0.5 text-right">${time} ${date}</p>
          </div>
          ${!isUser ? '<div class="w-6 h-6 rounded-full bg-oc-600 flex items-center justify-center text-xs flex-shrink-0"></div>' : ''}
        </div>`;
    }).join('');
    container.scrollTop = container.scrollHeight;
  } catch(e) {
    container.innerHTML = '<p class="text-xs text-gray-600 text-center py-4">Could not load Telegram messages</p>';
  }
}

// ── Todo List (server-synced) ──
let todos = [];

async function loadTodos() {
  try {
    const resp = await authFetch(`/api/todos?token=${authToken}`);
    const data = await resp.json();
    todos = data.todos || [];
    renderTodos();
  } catch(e) { console.log('todo load:', e); }
}

function renderTodos() {
  const list = document.getElementById('todo-list');
  const count = document.getElementById('todo-count');
  const open = todos.filter(t => !t.done);
  const done = todos.filter(t => t.done);
  count.textContent = `${open.length} open`;

  if (todos.length === 0) {
    list.innerHTML = '<p class="text-xs text-gray-600 py-2">No tasks yet</p>';
    return;
  }

  let html = open.map(t => `
    <div class="flex items-center gap-2 group">
      <input type="checkbox" onchange="toggleTodo(${t.id})" class="rounded border-gray-600 bg-gray-900 text-oc-500 focus:ring-oc-500 cursor-pointer">
      <span class="flex-1 text-sm text-gray-300">${escapeHtml(t.text)}</span>
      ${t.due_date ? '<span class="text-xs text-gray-600">' + t.due_date + '</span>' : ''}
      <button onclick="deleteTodo(${t.id})" class="text-gray-700 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity text-xs">&times;</button>
    </div>
  `).join('');

  if (done.length > 0) {
    html += `
    <details class="mt-2">
      <summary class="text-xs text-gray-600 cursor-pointer hover:text-gray-400 select-none py-1">${done.length} completed</summary>
      <div class="space-y-1.5 mt-1.5">
        ${done.map(t => `
          <div class="flex items-center gap-2 group">
            <input type="checkbox" checked onchange="toggleTodo(${t.id})" class="rounded border-gray-600 bg-gray-900 text-oc-500 focus:ring-oc-500 cursor-pointer">
            <span class="flex-1 text-sm text-gray-600 line-through">${escapeHtml(t.text)}</span>
            <button onclick="deleteTodo(${t.id})" class="text-gray-700 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity text-xs">&times;</button>
          </div>
        `).join('')}
        <button onclick="deleteCompleted()" class="w-full mt-2 text-xs text-gray-600 hover:text-red-400 py-1.5 rounded-lg bg-gray-900 hover:bg-gray-800 transition-colors">Clear completed items</button>
      </div>
    </details>`;
  }

  list.innerHTML = html;
}

async function deleteCompleted() {
  const done = todos.filter(t => t.done);
  for (const t of done) {
    try {
      await authFetch(`/api/todos/${t.id}`, {
        method: 'DELETE', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({ token: authToken })
      });
    } catch(e) {}
  }
  await loadTodos();
}

async function addTodo() {
  const input = document.getElementById('todo-input');
  const text = input.value.trim();
  if (!text) return;
  input.value = '';
  try {
    await authFetch('/api/todos', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ token: authToken, text })
    });
    await loadTodos();
  } catch(e) { console.log('todo add:', e); }
}

async function toggleTodo(id) {
  const todo = todos.find(t => t.id === id);
  if (!todo) return;
  try {
    await authFetch(`/api/todos/${id}`, {
      method: 'PUT', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ token: authToken, done: !todo.done })
    });
    await loadTodos();
  } catch(e) { console.log('todo toggle:', e); }
}

async function deleteTodo(id) {
  try {
    await authFetch(`/api/todos/${id}`, {
      method: 'DELETE', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ token: authToken })
    });
    await loadTodos();
  } catch(e) { console.log('todo delete:', e); }
}

// ── Mini Calendar ──
let calDate = new Date();
let calEvents = [];
let calSelectedDay = null;
let calView = 'month';  // 'month' or 'agenda'
let calEditingId = null;  // event id being edited, or null for new

async function loadCalendarEvents() {
  const year = calDate.getFullYear();
  const month = calDate.getMonth();
  const start = `${year}-${String(month+1).padStart(2,'0')}-01`;
  const endDate = new Date(year, month+1, 0);
  const end = `${year}-${String(month+1).padStart(2,'0')}-${endDate.getDate()}`;
  try {
    const resp = await authFetch(`/api/calendar?token=${authToken}&start=${start}&end=${end}`);
    const data = await resp.json();
    calEvents = data.events || [];
  } catch(e) { calEvents = []; }
  renderCalendar();
  renderAgenda();
  if (calSelectedDay) showDayEvents(calSelectedDay);
}

function setCalView(v) {
  calView = v;
  document.getElementById('view-month').classList.toggle('hidden', v !== 'month');
  document.getElementById('view-agenda').classList.toggle('hidden', v !== 'agenda');
  document.getElementById('view-month-btn').classList.toggle('bg-oc-700', v === 'month');
  document.getElementById('view-month-btn').classList.toggle('text-white', v === 'month');
  document.getElementById('view-month-btn').classList.toggle('text-gray-400', v !== 'month');
  document.getElementById('view-agenda-btn').classList.toggle('bg-oc-700', v === 'agenda');
  document.getElementById('view-agenda-btn').classList.toggle('text-white', v === 'agenda');
  document.getElementById('view-agenda-btn').classList.toggle('text-gray-400', v !== 'agenda');
  if (v === 'agenda') renderAgenda();
}

function renderAgenda() {
  const container = document.getElementById('view-agenda');
  if (!container) return;
  const sorted = [...calEvents].sort((a,b) => (a.start_time||'').localeCompare(b.start_time||''));
  if (sorted.length === 0) {
    container.innerHTML = '<p class="text-xs text-gray-500 italic text-center py-4">No events this month</p>';
    return;
  }
  container.innerHTML = sorted.map(ev => {
    const date = (ev.start_time||'').substring(0, 10);
    const time = ev.all_day ? 'All day' : formatEventTime(ev.start_time);
    const desc = ev.description ? `<p class="text-xs text-gray-500 mt-0.5 truncate">${escapeHtml(ev.description)}</p>` : '';
    const recur = ev.recurrence_rule ? `<span class="text-[10px] text-oc-500">&#x21bb; ${ev.recurrence_rule}</span>` : '';
    const remind = ev.reminder_minutes ? `<span class="text-[10px] text-gray-500">&#x1f514; ${ev.reminder_minutes}m</span>` : '';
    const evId = ev.id;
    return `<div class="bg-gray-900 rounded-lg p-2 hover:bg-gray-700 cursor-pointer" onclick="editEvent(${evId})">
      <div class="flex items-center justify-between gap-2">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 text-xs">
            <span class="text-oc-500">${date}</span>
            <span class="text-gray-500">${time}</span>
            ${recur} ${remind}
          </div>
          <div class="text-sm text-gray-200 truncate">${escapeHtml(ev.title)}</div>
          ${desc}
        </div>
      </div>
    </div>`;
  }).join('');
}

function renderCalendar() {
  const grid = document.getElementById('cal-grid');
  const title = document.getElementById('cal-title');
  const year = calDate.getFullYear();
  const month = calDate.getMonth();
  const months = ['January','February','March','April','May','June','July','August','September','October','November','December'];
  title.textContent = `${months[month]} ${year}`;

  const firstDay = new Date(year, month, 1).getDay();
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const today = new Date();
  const isCurrentMonth = today.getFullYear() === year && today.getMonth() === month;

  const eventDays = new Map();
  for (const ev of calEvents) {
    if (!ev.start_time) continue;
    const parts = ev.start_time.substring(0,10).split('-');
    const evY = parseInt(parts[0]), evM = parseInt(parts[1])-1, evD = parseInt(parts[2]);
    if (evY === year && evM === month) eventDays.set(evD, (eventDays.get(evD) || 0) + 1);
  }

  let html = '';
  for (let i = 0; i < firstDay; i++) html += '<span class="py-1"></span>';
  for (let d = 1; d <= daysInMonth; d++) {
    const isToday = isCurrentMonth && d === today.getDate();
    const isSelected = calSelectedDay === d;
    const evCount = eventDays.get(d) || 0;
    const dot = evCount > 0 ? `<span class="block w-1 h-1 rounded-full bg-oc-500 mx-auto -mt-0.5"></span>` : '';
    let cls = 'py-1 rounded relative cursor-pointer transition-colors ';
    if (isSelected) cls += 'bg-oc-700 text-white ring-1 ring-oc-400 font-bold';
    else if (isToday) cls += 'bg-oc-600 text-white font-bold hover:bg-oc-500';
    else cls += 'text-gray-400 hover:bg-gray-700';
    html += `<span class="${cls}" data-day="${d}" onclick="selectCalDay(${d})" ondragover="event.preventDefault(); this.classList.add('ring-1','ring-oc-500')" ondragleave="this.classList.remove('ring-1','ring-oc-500')" ondrop="handleDropOnDay(event, ${d})">${d}${dot}</span>`;
  }
  grid.innerHTML = html;
}

function selectCalDay(day) {
  calSelectedDay = day;
  renderCalendar();
  showDayEvents(day);
}

function showDayEvents(day) {
  const panel = document.getElementById('cal-day-panel');
  const evList = document.getElementById('cal-day-events');
  const titleEl = document.getElementById('cal-day-title');
  const months = ['January','February','March','April','May','June','July','August','September','October','November','December'];
  const year = calDate.getFullYear();
  const month = calDate.getMonth();
  const dateStr = `${year}-${String(month+1).padStart(2,'0')}-${String(day).padStart(2,'0')}`;
  titleEl.textContent = `${months[month]} ${day}, ${year}`;

  const dayEvents = calEvents.filter(ev => ev.start_time && ev.start_time.startsWith(dateStr));

  if (dayEvents.length === 0) {
    evList.innerHTML = '<p class="text-xs text-gray-500 italic">No events scheduled</p>';
  } else {
    evList.innerHTML = dayEvents.map(ev => {
      const time = ev.all_day ? '<span class="text-oc-500">All day</span>' :
        `<span class="text-gray-500">${formatEventTime(ev.start_time)}</span>`;
      const desc = ev.description ? `<p class="text-xs text-gray-500 mt-0.5">${escapeHtml(ev.description)}</p>` : '';
      const recur = ev.recurrence_rule ? `<span class="text-[10px] text-oc-500" title="Recurring ${ev.recurrence_rule}">&#x21bb;</span>` : '';
      const remind = ev.reminder_minutes ? `<span class="text-[10px] text-gray-500" title="Remind ${ev.reminder_minutes}min before">&#x1f514;</span>` : '';
      const isInstance = ev.is_recurring_instance ? ' opacity-75' : '';
      return `<div class="bg-gray-900 rounded-lg p-2 group relative cursor-pointer${isInstance}" draggable="true" ondragstart="handleDragStart(event, ${ev.id}, '${ev.start_time}')" onclick="editEvent(${ev.id})">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2 text-xs flex-1 min-w-0">
            ${time}
            <span class="text-gray-200 truncate">${escapeHtml(ev.title)}</span>
            ${recur} ${remind}
          </div>
          <button onclick="event.stopPropagation(); deleteEvent(${ev.id})" class="hidden group-hover:block text-gray-600 hover:text-red-400 text-xs px-1 flex-shrink-0" title="Delete">&times;</button>
        </div>
        ${desc}
        ${ev.source && ev.source !== 'manual' ? '<span class="text-[10px] text-gray-600">' + escapeHtml(ev.source) + '</span>' : ''}
      </div>`;
    }).join('');
  }

  panel.classList.remove('hidden');
  document.getElementById('cal-add-form').classList.add('hidden');
  document.getElementById('cal-ev-date').value = dateStr;
}

function formatEventTime(startTime) {
  // start_time could be "2026-04-13" (all day) or "2026-04-13T14:30:00" or "2026-04-13 14:30"
  const tIdx = startTime.indexOf('T');
  const spIdx = startTime.indexOf(' ', 11);
  const timeStr = tIdx > 0 ? startTime.substring(tIdx+1, tIdx+6) : (spIdx > 0 ? startTime.substring(spIdx+1, spIdx+6) : '');
  if (!timeStr) return '';
  const [h, m] = timeStr.split(':').map(Number);
  const ampm = h >= 12 ? 'pm' : 'am';
  const h12 = h % 12 || 12;
  return `${h12}:${String(m).padStart(2,'0')}${ampm}`;
}

function closeDayPanel() {
  calSelectedDay = null;
  document.getElementById('cal-day-panel').classList.add('hidden');
  document.getElementById('cal-add-form').classList.add('hidden');
  renderCalendar();
}

function showAddEvent() {
  calEditingId = null;
  const form = document.getElementById('cal-add-form');
  form.classList.remove('hidden');
  document.getElementById('cal-form-title').textContent = 'New Event';
  document.getElementById('cal-ev-title').value = '';
  document.getElementById('cal-ev-desc').value = '';
  document.getElementById('cal-ev-time').value = '';
  document.getElementById('cal-ev-allday').checked = false;
  document.getElementById('cal-ev-time').disabled = false;
  document.getElementById('cal-ev-recur').value = '';
  document.getElementById('cal-ev-recur-end').value = '';
  document.getElementById('cal-ev-recur-end').classList.add('hidden');
  document.getElementById('cal-ev-remind').value = '';
  document.getElementById('cal-ev-title').focus();
}

function editEvent(id) {
  // Find the base event (may be a recurring instance, find by id)
  const ev = calEvents.find(e => e.id === id);
  if (!ev) return;
  calEditingId = id;
  const form = document.getElementById('cal-add-form');
  form.classList.remove('hidden');
  document.getElementById('cal-form-title').textContent = 'Edit Event';
  document.getElementById('cal-ev-title').value = ev.title || '';
  document.getElementById('cal-ev-desc').value = ev.description || '';
  const st = ev.start_time || '';
  const dateStr = st.substring(0, 10);
  document.getElementById('cal-ev-date').value = dateStr;
  if (ev.all_day) {
    document.getElementById('cal-ev-allday').checked = true;
    document.getElementById('cal-ev-time').value = '';
    document.getElementById('cal-ev-time').disabled = true;
  } else {
    document.getElementById('cal-ev-allday').checked = false;
    const timePart = st.length > 10 ? st.substring(11, 16) : '';
    document.getElementById('cal-ev-time').value = timePart;
    document.getElementById('cal-ev-time').disabled = false;
  }
  document.getElementById('cal-ev-recur').value = ev.recurrence_rule || '';
  document.getElementById('cal-ev-recur-end').value = ev.recurrence_end_date || '';
  document.getElementById('cal-ev-recur-end').classList.toggle('hidden', !ev.recurrence_rule);
  document.getElementById('cal-ev-remind').value = ev.reminder_minutes || '';
  document.getElementById('cal-ev-title').focus();
}

function cancelAddEvent() {
  document.getElementById('cal-add-form').classList.add('hidden');
  calEditingId = null;
}

// Toggle recur end visibility when recurrence changes
document.addEventListener('DOMContentLoaded', () => {
  const recurSel = document.getElementById('cal-ev-recur');
  if (recurSel) {
    recurSel.addEventListener('change', () => {
      document.getElementById('cal-ev-recur-end').classList.toggle('hidden', !recurSel.value);
    });
  }
});

async function saveEvent() {
  const title = document.getElementById('cal-ev-title').value.trim();
  if (!title) { document.getElementById('cal-ev-title').classList.add('border-red-500'); return; }
  document.getElementById('cal-ev-title').classList.remove('border-red-500');

  const date = document.getElementById('cal-ev-date').value;
  const time = document.getElementById('cal-ev-time').value;
  const allDay = document.getElementById('cal-ev-allday').checked;
  const desc = document.getElementById('cal-ev-desc').value.trim();
  const recur = document.getElementById('cal-ev-recur').value;
  const recurEnd = document.getElementById('cal-ev-recur-end').value;
  const remindStr = document.getElementById('cal-ev-remind').value;
  const remindMin = remindStr ? parseInt(remindStr) : null;

  const startTime = allDay || !time ? date : `${date}T${time}:00`;

  const payload = {
    token: authToken,
    title: title,
    description: desc || null,
    start_time: startTime,
    end_time: null,
    all_day: allDay,
    recurrence_rule: recur || null,
    recurrence_end_date: recurEnd || null,
    reminder_minutes: remindMin,
  };

  const btn = document.getElementById('cal-save-btn');
  btn.textContent = 'Saving...';
  btn.disabled = true;

  try {
    const url = calEditingId ? `/api/calendar/${calEditingId}` : '/api/calendar';
    const method = calEditingId ? 'PUT' : 'POST';
    const resp = await authFetch(url, {
      method: method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });
    if (resp.ok) {
      cancelAddEvent();
      await loadCalendarEvents();
    }
  } catch(e) { console.log('save event:', e); }
  btn.textContent = 'Save';
  btn.disabled = false;
}

// Drag and drop handlers
let dragEventId = null;
let dragEventTime = null;

function handleDragStart(ev, id, startTime) {
  dragEventId = id;
  dragEventTime = startTime;
  try { ev.dataTransfer.effectAllowed = 'move'; } catch(e) {}
}

async function handleDropOnDay(ev, day) {
  ev.preventDefault();
  ev.currentTarget.classList.remove('ring-1', 'ring-oc-500');
  if (!dragEventId) return;
  const year = calDate.getFullYear();
  const month = calDate.getMonth();
  const newDate = `${year}-${String(month+1).padStart(2,'0')}-${String(day).padStart(2,'0')}`;
  // Preserve time of day if event was timed
  let newStart = newDate;
  if (dragEventTime && dragEventTime.length > 10) {
    newStart = newDate + dragEventTime.substring(10);
  }
  try {
    await authFetch(`/api/calendar/${dragEventId}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: authToken, start_time: newStart })
    });
    await loadCalendarEvents();
  } catch(e) { console.log('drag reschedule:', e); }
  dragEventId = null;
  dragEventTime = null;
}

async function importIcs(ev) {
  const file = ev.target.files[0];
  if (!file) return;
  const text = await file.text();
  try {
    const resp = await authFetch('/api/calendar/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: authToken, ics_content: text })
    });
    if (resp.ok) {
      const data = await resp.json();
      alert(`Imported ${data.imported} events (${data.skipped} skipped)`);
      await loadCalendarEvents();
    } else {
      alert('Import failed');
    }
  } catch(e) {
    alert('Import error: ' + e.message);
  }
  ev.target.value = '';
}

async function deleteEvent(id) {
  if (!confirm('Delete this event?')) return;
  try {
    await authFetch(`/api/calendar/${id}?token=${authToken}`, { method: 'DELETE' });
    await loadCalendarEvents();
  } catch(e) { console.log('delete event:', e); }
}

function calPrev() { calDate.setMonth(calDate.getMonth() - 1); calSelectedDay = null; closeDayPanel(); loadCalendarEvents(); }
function calNext() { calDate.setMonth(calDate.getMonth() + 1); calSelectedDay = null; closeDayPanel(); loadCalendarEvents(); }

// ── Dashboard Agent Switcher ──
let dashAgents = [];
let dashCurrentAgent = 'main';
let dashAgentIdMap = {};

function toggleDashAgentMenu() {
  const menu = document.getElementById('dash-agent-menu');
  menu.classList.toggle('hidden');
  if (!menu.classList.contains('hidden')) {
    setTimeout(() => {
      document.addEventListener('click', function close(e) {
        if (!menu.contains(e.target) && !e.target.closest('#dash-agent-switcher')) {
          menu.classList.add('hidden');
        }
        document.removeEventListener('click', close);
      });
    }, 10);
  }
}

function buildDashAgentMenu() {
  const menu = document.getElementById('dash-agent-menu');
  menu.innerHTML = dashAgents.map(a => `
    <button onclick="switchDashAgent('${escapeHtml(a)}')" class="w-full text-left px-4 py-2 text-sm hover:bg-gray-700 transition-colors ${a === dashCurrentAgent ? 'text-oc-500 font-medium' : 'text-gray-300'}">
      ${escapeHtml(a)}
    </button>
  `).join('');
}

function switchDashAgent(name) {
  document.getElementById('dash-agent-menu').classList.add('hidden');
  if (name === dashCurrentAgent) return;
  dashCurrentAgent = name;
  document.getElementById('chat-agent-name').textContent = name;
  buildDashAgentMenu();
  // Clear chat and update greeting
  clearChat();
}

function getDashAgentId(name) {
  return dashAgentIdMap[name] || name.toLowerCase().replace(/\s+/g, '-');
}

function doLogout() {
  sessionStorage.removeItem('syntaur_token');
  window.location.href = '/';
}

// ── Tax Summary Widget ──
async function loadTaxSummary() {
  try {
    const resp = await authFetch(`/api/tax/summary?token=${authToken}`);
    const data = await resp.json();
    const el = document.getElementById('tax-summary-content');
    if (!data.total_cents && data.total_cents !== 0) {
      el.innerHTML = '<p class="text-xs text-gray-600">No expenses tracked yet</p>';
      return;
    }
    const cats = (data.categories || []).slice(0, 4);
    el.innerHTML = `
      <div class="space-y-2">
        <div class="flex justify-between text-sm">
          <span class="text-gray-400">Total</span>
          <span class="text-white font-medium">${data.total_display}</span>
        </div>
        <div class="flex justify-between text-sm">
          <span class="text-gray-400">Business</span>
          <span class="text-gray-300">${data.business_display}</span>
        </div>
        <div class="flex justify-between text-sm">
          <span class="text-gray-400">Deductible</span>
          <span class="text-green-400">${data.deductible_display}</span>
        </div>
        ${data.receipt_count > 0 ? `<div class="flex justify-between text-sm"><span class="text-gray-400">Receipts</span><span class="text-gray-300">${data.receipt_count}</span></div>` : ''}
      </div>
      ${cats.length > 0 ? `
        <div class="mt-3 pt-3 border-t border-gray-700 space-y-1">
          ${cats.map(c => `
            <div class="flex justify-between text-xs">
              <span class="text-gray-500">${c.category}</span>
              <span class="text-gray-400">${c.total_display}</span>
            </div>
          `).join('')}
        </div>
      ` : ''}`;
  } catch(e) {
    document.getElementById('tax-summary-content').innerHTML = '<p class="text-xs text-gray-600">Tax module not configured</p>';
  }
}

// Init widgets — rendered empty first, populated after auth
renderTodos();
renderCalendar();

// Login
function toggleUsernameField() {
  const row = document.getElementById('login-username-row');
  const toggle = document.getElementById('login-toggle');
  if (row.classList.contains('hidden')) {
    row.classList.remove('hidden');
    toggle.textContent = 'Sign in with password only';
    document.getElementById('login-user').focus();
  } else {
    row.classList.add('hidden');
    toggle.textContent = 'Sign in with username';
    document.getElementById('login-pass').focus();
  }
}

async function doLogin() {
  const pass = document.getElementById('login-pass').value;
  const userEl = document.getElementById('login-user');
  const username = userEl && !document.getElementById('login-username-row').classList.contains('hidden') ? userEl.value.trim() : null;
  const errEl = document.getElementById('login-error');
  errEl.classList.add('hidden');
  document.getElementById('login-btn').textContent = 'Signing in...';

  try {
    const resp = await fetch('/api/auth/login', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(username ? { password: pass, username } : { password: pass })
    });
    const data = await resp.json();
    if (data.success && data.token) {
      authToken = data.token;
      sessionStorage.setItem('syntaur_token', authToken);
      document.getElementById('login-overlay').classList.add('hidden');
      if (window._authResolve) { window._authResolve(); window._authResolve = null; }
    } else {
      errEl.textContent = data.error || 'Invalid password';
      errEl.classList.remove('hidden');
    }
  } catch(e) {
    errEl.textContent = 'Connection error';
    errEl.classList.remove('hidden');
  }
  document.getElementById('login-btn').textContent = 'Sign In';
}

// Check if already authenticated — blocks until login succeeds
async function checkAuth() {
  if (authToken) {
    // Verify token against an auth-protected endpoint
    try {
      const resp = await fetch('/api/setup/modules', {
        headers: { 'Authorization': 'Bearer ' + authToken }
      });
      if (resp.ok) {
        document.getElementById('login-overlay').classList.add('hidden');
        return;
      }
    } catch(e) {}
    // Token expired or invalid — clear it
    authToken = '';
    sessionStorage.removeItem('syntaur_token');
  }
  // Show login and wait for successful authentication
  document.getElementById('login-overlay').classList.remove('hidden');
  document.getElementById('login-pass').focus();
  return new Promise(resolve => { window._authResolve = resolve; });
}

// Chat is now embedded in the dashboard, no toggle needed

function chatKeydown(e) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
}

function autoResize(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, 120) + 'px';
}

// ── File attachments in chat ──────────────────────────────────────────
let pendingFiles = []; // {name, text, chunks, status}

function attachFiles(fileList) {
  for (const file of fileList) {
    const entry = { name: file.name, text: null, chunks: 0, status: 'uploading', file };
    pendingFiles.push(entry);
    renderAttachments();
    uploadChatFile(entry);
  }
  // Reset input so same file can be re-selected
  document.getElementById('chat-file-input').value = '';
}

async function uploadChatFile(entry) {
  const fd = new FormData();
  fd.append('token', authToken);
  fd.append('agent_id', getDashAgentId(dashCurrentAgent));
  fd.append('file', entry.file);
  fd.append('return_text', '1');
  try {
    const r = await fetch('/api/knowledge/upload', { method: 'POST', body: fd });
    const data = await r.json();
    if (data.ok) {
      entry.status = 'ready';
      entry.chunks = data.chunks || 0;
      entry.text = data.extracted_text || null;
    } else {
      entry.status = 'error';
      entry.error = data.error || 'Upload failed';
    }
  } catch (e) {
    entry.status = 'error';
    entry.error = e.message;
  }
  renderAttachments();
}

function removeAttachment(idx) {
  pendingFiles.splice(idx, 1);
  renderAttachments();
}

function renderAttachments() {
  const c = document.getElementById('chat-attachments');
  if (pendingFiles.length === 0) { c.classList.add('hidden'); c.innerHTML = ''; return; }
  c.classList.remove('hidden');
  c.innerHTML = pendingFiles.map((f, i) => {
    const icon = f.status === 'uploading' ? '<span class="animate-pulse">...</span>'
      : f.status === 'error' ? '<span class="text-red-400">!</span>'
      : '<span class="text-green-400">OK</span>';
    return '<div class="flex items-center gap-2 text-xs bg-gray-800 rounded-lg px-3 py-1.5">'
      + '<span class="truncate flex-1 text-gray-300">' + escapeHtml(f.name) + '</span>'
      + '<span class="flex-shrink-0">' + icon + '</span>'
      + (f.chunks ? '<span class="text-gray-500">' + f.chunks + ' chunks</span>' : '')
      + '<button onclick="removeAttachment(' + i + ')" class="text-gray-500 hover:text-red-400 ml-1">x</button>'
      + '</div>';
  }).join('');
}

// Drag & drop on chat area
(function() {
  const chatArea = document.getElementById('chat-messages');
  if (!chatArea) return;
  chatArea.addEventListener('dragover', e => { e.preventDefault(); chatArea.style.outline = '2px dashed #0ea5e9'; });
  chatArea.addEventListener('dragleave', e => { e.preventDefault(); chatArea.style.outline = ''; });
  chatArea.addEventListener('drop', e => {
    e.preventDefault();
    chatArea.style.outline = '';
    if (e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files.length) {
      attachFiles(e.dataTransfer.files);
    }
  });
})();

function quickChat(msg) {
  document.getElementById('chat-input').value = msg;
  sendMessage();
}

function clearChat() {
  const messages = document.getElementById('chat-messages');
  messages.innerHTML = `
    <div class="flex gap-3">
      <img src="/agent-avatar/main" class="w-7 h-7 rounded-full flex-shrink-0 mt-0.5" alt="">
      <div class="flex-1 text-sm text-gray-300">
        <p>Chat cleared. How can I help?</p>
      </div>
    </div>`;
}

let isSending = false;

async function sendMessage() {
  if (isSending) return;
  const input = document.getElementById('chat-input');
  let msg = input.value.trim();
  if (!msg && pendingFiles.length === 0) return;

  // Prepend attached file content to the message
  const readyFiles = pendingFiles.filter(f => f.status === 'ready' && f.text);
  if (readyFiles.length > 0) {
    let prefix = '';
    for (const f of readyFiles) {
      const preview = f.text.length > 50000 ? f.text.slice(0, 50000) + '\n\n[truncated — full content indexed, use internal_search for more]' : f.text;
      prefix += '[Attached file: ' + f.name + ']\n' + preview + '\n\n';
    }
    msg = prefix + (msg || 'Please review the attached file(s).');
  }
  pendingFiles = [];
  renderAttachments();

  if (!msg) return;
  input.value = '';
  input.style.height = 'auto';
  isSending = true;

  const messages = document.getElementById('chat-messages');
  const sendBtn = document.getElementById('send-btn');
  sendBtn.disabled = true;
  sendBtn.classList.add('opacity-50');

  // User message
  const userDiv = document.createElement('div');
  userDiv.className = 'flex gap-3 justify-end';
  userDiv.innerHTML = `
    <div class="max-w-[80%] bg-oc-900/40 border border-oc-800/50 rounded-xl rounded-br-sm px-4 py-2.5 text-sm">
      <p class="text-gray-200">${escapeHtml(msg)}</p>
    </div>`;
  messages.appendChild(userDiv);

  // Thinking indicator
  const aiDiv = document.createElement('div');
  aiDiv.className = 'flex gap-3';
  aiDiv.innerHTML = `
    <img src="/agent-avatar/main" class="w-7 h-7 rounded-full flex-shrink-0 mt-0.5" alt="">
    <div class="flex-1 text-sm">
      <div class="flex items-center gap-2 text-gray-400">
        <div class="flex gap-1">
          <span class="w-1.5 h-1.5 rounded-full bg-gray-400 animate-bounce" style="animation-delay:0ms"></span>
          <span class="w-1.5 h-1.5 rounded-full bg-gray-400 animate-bounce" style="animation-delay:150ms"></span>
          <span class="w-1.5 h-1.5 rounded-full bg-gray-400 animate-bounce" style="animation-delay:300ms"></span>
        </div>
        <span class="text-xs">Thinking...</span>
      </div>
    </div>`;
  messages.appendChild(aiDiv);
  messages.scrollTop = messages.scrollHeight;

  // Send to API
  const startTime = Date.now();
  try {
    const resp = await fetch('/api/message', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: msg, agent: getDashAgentId(dashCurrentAgent), token: authToken })
    });
    const data = await resp.json();
    const elapsed = ((Date.now() - startTime) / 1000).toFixed(1);
    const rounds = data.rounds || 0;

    if (data.response) {
      // Strip the [HANDBACK] marker from displayed text — it's control
      // metadata, not part of the user-visible message.
      const displayText = data.response.replace(/\s*\[HANDBACK\]\s*/gi, '');
      const rendered = renderMarkdown(displayText);
      const meta = rounds > 1 ? `<span class="text-xs text-gray-600 mt-2 block">${elapsed}s &middot; ${rounds} rounds</span>` : `<span class="text-xs text-gray-600 mt-2 block">${elapsed}s</span>`;
      aiDiv.querySelector('.flex-1').innerHTML = `
        <div class="text-gray-300 leading-relaxed chat-md">${rendered}</div>
        ${meta}
        <button onclick="copyText(this)" class="text-xs text-gray-600 hover:text-gray-400 mt-1" data-text="${escapeAttr(displayText)}">Copy</button>`;
      // Check for Maurice's [HANDBACK] marker in the raw response and
      // flip the agent selector back to the summoning persona.
      if (typeof checkForHandbackDash === 'function') checkForHandbackDash(data.response);
    } else if (data.error) {
      aiDiv.querySelector('.flex-1').innerHTML = `<p class="text-red-400 text-sm">${escapeHtml(data.error)}</p>`;
    }
  } catch(e) {
    aiDiv.querySelector('.flex-1').innerHTML = `<p class="text-red-400 text-sm">Connection error: ${e.message}</p>`;
  }

  messages.scrollTop = messages.scrollHeight;
  sendBtn.disabled = false;
  sendBtn.classList.remove('opacity-50');
  isSending = false;
  input.focus();
}

function copyText(btn) {
  navigator.clipboard.writeText(btn.dataset.text).then(() => {
    const orig = btn.textContent;
    btn.textContent = 'Copied!';
    setTimeout(() => btn.textContent = orig, 1500);
  });
}

function escapeHtml(s) {
  return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\n/g,'<br>');
}

function escapeAttr(s) {
  return s.replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

// Simple markdown renderer
function renderMarkdown(text) {
  let html = escapeHtml(text);

  // Code blocks (```lang\n...\n```)
  html = html.replace(/```(\w*)<br>([\s\S]*?)```/g, (_, lang, code) => {
    const langLabel = lang ? `<span class="text-xs text-gray-500 absolute top-2 right-2">${lang}</span>` : '';
    return `<div class="relative my-2"><pre class="bg-gray-950 border border-gray-700 rounded-lg p-3 overflow-x-auto text-xs font-mono text-gray-300">${langLabel}${code.replace(/<br>/g, '\n')}</pre></div>`;
  });

  // Inline code (`...`)
  html = html.replace(/`([^`]+)`/g, '<code class="bg-gray-800 px-1.5 py-0.5 rounded text-oc-500 text-xs font-mono">$1</code>');

  // Bold (**...**)
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong class="text-white font-semibold">$1</strong>');

  // Italic (*...*)
  html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');

  // Links [text](url)
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" class="text-oc-500 hover:text-oc-400 underline">$1</a>');

  // Bullet lists (- item)
  html = html.replace(/(^|<br>)- (.+?)(?=<br>|$)/g, '$1<span class="flex gap-2"><span class="text-gray-500">&bull;</span><span>$2</span></span>');

  // Numbered lists (1. item)
  html = html.replace(/(^|<br>)(\d+)\. (.+?)(?=<br>|$)/g, '$1<span class="flex gap-2"><span class="text-gray-500">$2.</span><span>$3</span></span>');

  // Headers (### ...)
  html = html.replace(/(^|<br>)### (.+?)(?=<br>|$)/g, '$1<p class="font-semibold text-white mt-2">$2</p>');
  html = html.replace(/(^|<br>)## (.+?)(?=<br>|$)/g, '$1<p class="font-bold text-white text-base mt-2">$2</p>');

  // Fix-options → clickable buttons (see chat.rs for the full rationale).
  html = renderFixOptionsDash(html);

  return html;
}

// Mirror of chat.rs renderFixOptions, tuned for dashboard's renderMarkdown
// output shape (uses <span class="flex gap-2"> instead of <div>). Emitted
// buttons submit the number into the dashboard chat input.
function renderFixOptionsDash(html) {
  const markerRe = /<strong[^>]*>\s*Fix options[^<]*<\/strong>\s*(?:<br>)?/i;
  const m = html.match(markerRe);
  if (!m) return html;
  const prefix = html.slice(0, m.index + m[0].length);
  const after = html.slice(m.index + m[0].length);
  const itemRe = /<span class="flex gap-2"><span class="text-gray-500">(\d+)\.<\/span><span>([\s\S]*?)<\/span><\/span>/g;
  const items = [];
  let match, firstStart = -1, lastEnd = -1;
  while ((match = itemRe.exec(after)) !== null) {
    if (firstStart < 0) firstStart = match.index;
    lastEnd = match.index + match[0].length;
    items.push({ num: match[1], inner: match[2] });
  }
  if (items.length < 2) return html;
  let btnHtml = '<div class="mt-2 mb-1 flex flex-wrap gap-2">';
  for (const it of items) {
    const labelMatch = it.inner.match(/<strong[^>]*>([^<]+)<\/strong>/);
    const label = labelMatch ? labelMatch[1].trim() : `Option ${it.num}`;
    const safeLabel = label.replace(/"/g, '&quot;');
    btnHtml += `<button onclick="pickFixOptionDash(${it.num}, this)" data-label="${safeLabel}" ` +
      `class="px-3 py-1.5 bg-oc-600 hover:bg-oc-700 text-white rounded-lg text-sm font-medium transition-colors shadow-sm">` +
      escapeHtml(label) +
      `</button>`;
  }
  btnHtml += '</div>';
  return prefix + btnHtml + after.slice(0, firstStart) +
    '<div class="text-xs text-gray-500 italic mt-1">Or reply with the number:</div>' +
    after.slice(firstStart, lastEnd) + after.slice(lastEnd);
}

function pickFixOptionDash(num, btn) {
  if (btn && btn.parentElement) {
    for (const b of btn.parentElement.children) {
      b.disabled = true;
      b.classList.remove('bg-oc-600','hover:bg-oc-700');
      b.classList.add('bg-gray-700','cursor-not-allowed','opacity-60');
    }
    btn.classList.remove('bg-gray-700','opacity-60');
    btn.classList.add('bg-oc-700','opacity-100');
  }
  const label = btn ? (btn.dataset.label || btn.textContent || '') : '';
  // "Let my debug specialist look at this" routes to the Maurice handoff.
  if (/\b(debug specialist|specialist look|get.*specialist)\b/i.test(label)
      || /\bmaurice\b/i.test(label)) {
    handoffToMauriceDash(btn);
    return;
  }
  const input = document.getElementById('chat-input') || document.getElementById('input');
  if (!input) return;
  input.value = String(num);
  if (typeof sendChat === 'function') sendChat();
  else if (typeof sendMessage === 'function') sendMessage();
}

// Navigate from dashboard chat to the /coders module for a handoff.
// Same URL-param protocol as chat.rs's handoffToMaurice — /coders reads
// the params and shows a handoff banner with Maurice pre-seeded with the
// failure context. On return, /coders posts the outcome report back into
// the original conversation.
function handoffToMauriceDash(btn) {
  const priorAgent = (typeof dashCurrentAgent !== 'undefined' && dashCurrentAgent) || 'Peter';
  let ctx = '';
  let el = btn ? btn.closest('.chat-md, [data-ai-msg]') : null;
  if (!el) {
    const msgs = document.querySelectorAll('.chat-md');
    if (msgs.length) el = msgs[msgs.length - 1];
  }
  if (el) ctx = (el.innerText || '').trim().slice(0, 2000);

  let ctxEnc = '';
  try {
    ctxEnc = btoa(unescape(encodeURIComponent(ctx)))
      .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  } catch(e) { ctxEnc = ''; }

  const params = new URLSearchParams();
  params.set('handoff', '1');
  params.set('return_agent', priorAgent);
  if (typeof dashConversationId !== 'undefined' && dashConversationId) params.set('conv_id', dashConversationId);
  if (typeof conversationId !== 'undefined' && conversationId) params.set('conv_id', conversationId);
  if (ctxEnc) params.set('ctx', ctxEnc);
  if (typeof authToken !== 'undefined' && authToken) params.set('token', authToken);

  location.href = '/coders?' + params.toString();
}

// Legacy — no-op now that handoff is a page navigation.
function checkForHandbackDash(_responseText) { /* no-op */ }

// Authenticated fetch helper
function authFetch(url, opts = {}) {
  opts.headers = opts.headers || {};
  if (authToken) opts.headers['Authorization'] = 'Bearer ' + authToken;
  return fetch(url, opts);
}

// Load dashboard data
async function loadDashboard() {
  // Small helper: set text only if element exists (avoids null-deref throws
  // from a DOM node that got removed or renamed since this code was written).
  const setText = (id, text) => { const e = document.getElementById(id); if (e) e.textContent = text; };
  const setHtml = (id, html) => { const e = document.getElementById(id); if (e) e.innerHTML = html; };
  // Each section is independent: a failure in one shouldn't clobber the
  // whole banner. Only a failure of the health probe itself flips the
  // status badge to red.
  const failed = [];

  // 1) Health — the ONLY section that reflects the banner status.
  let health = null;
  try {
    health = await (await authFetch('/health')).json();
    const uptime = health.uptime_secs || 0;
    const h = Math.floor(uptime / 3600);
    const m = Math.floor((uptime % 3600) / 60);
    const uptimeStr = h > 0 ? `${h}h ${m}m` : `${m}m`;
    setText('stat-uptime', uptimeStr);
    setText('stat-uptime2', uptimeStr);
    const agentNames = (health.agents || []).map(a => typeof a === 'object' ? a.name : a);
    setText('stat-agents', agentNames.join(', '));
    setText('stat-agents2', agentNames.join(', '));
    setText('sys-version', `v${health.version || '?'}`);
    const sb = document.getElementById('status-badge');
    if (sb) {
      sb.className = 'badge badge-green text-xs';
      sb.innerHTML = '<span class="w-1.5 h-1.5 rounded-full bg-green-400 mr-1"></span>Online';
      sb.removeAttribute('title');
    }
  } catch(e) {
    console.error('[dashboard] /health failed', e);
    failed.push('health');
    const sb = document.getElementById('status-badge');
    if (sb) {
      sb.className = 'badge badge-red text-xs';
      sb.innerHTML = '<span class="w-1.5 h-1.5 rounded-full bg-red-400 mr-1"></span>Offline';
      sb.title = `Gateway /health unreachable: ${e && e.message ? e.message : e}`;
    }
  }

  // 2) Agent switcher — uses health + /api/me. Independent failure OK.
  try {
    const rawAgents = (health && health.agents) || [];
    dashAgentIdMap = {};
    dashAgents = rawAgents.map(a => {
      if (typeof a === 'object') { dashAgentIdMap[a.name] = a.id; return a.name; }
      return a;
    });
    try {
      const me = await (await authFetch('/api/me?token=' + encodeURIComponent(authToken))).json();
      if (me.user && me.user.name) setText('user-label', me.user.name);
      for (const ua of (me.agents || [])) {
        if (!dashAgents.includes(ua.display_name)) {
          dashAgentIdMap[ua.display_name] = ua.agent_id;
          dashAgents.push(ua.display_name);
        }
      }
    } catch(e) { console.warn('[dashboard] /api/me failed', e); }
    if (dashAgents.length > 0 && !dashAgents.includes(dashCurrentAgent)) {
      dashCurrentAgent = dashAgents[0];
    }
    setText('chat-agent-name', dashCurrentAgent);
    buildDashAgentMenu();
  } catch(e) { console.warn('[dashboard] agent switcher failed', e); failed.push('agents'); }

  // 3) Setup status — greeting strings.
  let setup = null;
  try {
    setup = await (await authFetch('/api/setup/status')).json();
    if (setup && setup.agent_name) {
      setText('welcome-text', 'Welcome back');
      setText('welcome-sub', `${setup.agent_name} is online and ready.`);
      setText('chat-greeting', `Hey! I'm ${setup.agent_name}. How can I help you?`);
      setText('chat-agent-name', setup.agent_name);
    }
  } catch(e) { console.warn('[dashboard] /api/setup/status failed', e); failed.push('setup'); }

  // 4) Modules — the grid + stat counts.
  try {
    const mods = await (await authFetch('/api/setup/modules')).json();
    const core = Array.isArray(mods && mods.core_modules) ? mods.core_modules : [];
    const ext = Array.isArray(mods && mods.extension_modules) ? mods.extension_modules : [];
    const allMods = [...core, ...ext];
    const enabled = allMods.filter(m => m && m.enabled);
    const totalTools = enabled.reduce((s, m) => s + (m.tool_count || 0), 0);
    setText('stat-modules', enabled.length);
    setText('stat-modules2', enabled.length);
    setText('stat-tools', `${totalTools} tools`);
    setText('module-count', `${enabled.length} of ${allMods.length}`);
    setHtml('module-grid', enabled.map(m => `
      <div class="p-2.5 rounded-lg bg-gray-900 text-xs">
        <p class="font-medium text-gray-300 truncate">${escapeHtml(m.name || '')}</p>
        <p class="text-gray-500 mt-0.5">${m.tool_count || 0} tools</p>
      </div>
    `).join(''));
  } catch(e) { console.warn('[dashboard] /api/setup/modules failed', e); failed.push('modules'); }

  // 5) LLM status (derived from setup result when present).
  try {
    if (setup) {
      setText('stat-llm', setup.has_llm_configured ? 'Connected' : 'Not configured');
      setText('stat-llm-status', setup.has_llm_configured ? 'Primary + fallbacks' : '');
    }
  } catch(e) { console.warn('[dashboard] llm status failed', e); }

  // 6) License.
  try {
    const lic = await (await authFetch('/api/license/status')).json();
    const badge = document.getElementById('license-badge');
    if (badge) {
      badge.classList.remove('hidden');
      if (lic.mode === 'licensed') {
        badge.className = 'badge badge-green text-xs';
        badge.textContent = lic.license_tier || 'Licensed';
      } else if (lic.mode === 'demo') {
        const days = Math.ceil((lic.demo_remaining_secs || 0) / 86400);
        badge.className = 'badge bg-yellow-900/50 text-yellow-400 text-xs';
        badge.textContent = `Demo: ${days}d left`;
      } else if (lic.mode === 'free') {
        badge.className = 'badge badge-gray text-xs';
        badge.textContent = 'Free';
      } else {
        badge.className = 'badge badge-red text-xs cursor-pointer';
        badge.textContent = 'Demo Expired';
        badge.onclick = () => location.href = '/settings';
      }
    }
  } catch(e) { console.warn('[dashboard] /api/license/status failed', e); }

  if (failed.length) {
    console.warn('[dashboard] partial load, failed sections:', failed);
  }
}

// Phone access QR
function showPhoneAccess() {
  const url = location.origin;
  document.getElementById('phone-qr').src =
    `https://api.qrserver.com/v1/create-qr-code/?size=200x200&data=${encodeURIComponent(url)}&bgcolor=ffffff&color=0c4a6e`;
  document.getElementById('phone-url').textContent = url;
  document.getElementById('phone-modal').classList.remove('hidden');
}

// Init — check if setup is needed first
(async () => {
  try {
    const s = await (await authFetch('/api/setup/status')).json();
    if (!s.setup_complete) {
      location.href = '/setup';
      return;
    }
  } catch(e) {}
  await checkAuth();
  loadDashboard();
  loadTodos();
  loadCalendarEvents();
  setInterval(loadDashboard, 30000);
  setInterval(loadTodos, 60000);
})();
</script>

<script>
// Bug Report UI — injected on all authenticated pages (schema v10)
(function() {
  const BUG_TOKEN = sessionStorage.getItem('syntaur_token') || '';
  if (!BUG_TOKEN) return;
  const topBar = document.querySelector('.border-b.border-gray-800 .flex.items-center.justify-between');
  if (!topBar) return;
  const rightNav = topBar.lastElementChild;
  if (!rightNav) return;
  const bugBtn = document.createElement('button');
  bugBtn.title = 'Report a Bug';
  bugBtn.className = 'text-gray-500 hover:text-gray-300 transition-colors p-1 rounded';
  bugBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2l1.88 1.88M14.12 3.88L16 2M9 7.13v-1a3.003 3.003 0 116 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 014-4h4a4 4 0 014 4v3c0 3.3-2.7 6-6 6z"/><path d="M12 20v-9M6.53 9C4.6 8.8 3 7.1 3 5M6 13H2M6 17l-4 1M17.47 9c1.93-.2 3.53-1.9 3.53-4M18 13h4M18 17l4 1"/></svg>';
  bugBtn.onclick = function() { openBugModal(); };
  if (rightNav.tagName === 'DIV') { rightNav.prepend(bugBtn); }
  else { const w = document.createElement('div'); w.className='flex items-center gap-3'; topBar.replaceChild(w, rightNav); w.appendChild(bugBtn); w.appendChild(rightNav); }
  const overlay = document.createElement('div');
  overlay.id = 'bug-report-overlay';
  overlay.className = 'fixed inset-0 z-50 bg-black/60 backdrop-blur-sm hidden flex items-center justify-center';
  overlay.innerHTML = '<div class="bg-gray-800 border border-gray-700 rounded-2xl w-full max-w-lg mx-4 shadow-2xl"><div class="px-6 py-4 border-b border-gray-700 flex items-center justify-between"><h3 class="text-lg font-semibold text-white">Report a Bug</h3><button onclick="document.getElementById(\'bug-report-overlay\').classList.add(\'hidden\')" class="text-gray-400 hover:text-gray-200 text-xl leading-none">&times;</button></div><div class="px-6 py-5 space-y-4"><div><label class="block text-sm font-medium text-gray-300 mb-1.5">What went wrong?</label><textarea id="bug-desc" rows="4" class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-500 focus:border-sky-500 focus:ring-1 focus:ring-sky-500 outline-none resize-none text-sm" placeholder="Describe the bug..."></textarea></div><details class="text-sm"><summary class="text-gray-400 cursor-pointer hover:text-gray-300 select-none">System Info (auto-collected)</summary><pre id="bug-sysinfo" class="mt-2 bg-gray-900 border border-gray-700 rounded-lg p-3 text-xs text-gray-400 overflow-x-auto max-h-40 overflow-y-auto"></pre></details><div id="bug-feedback" class="hidden text-sm"></div></div><div class="px-6 py-4 border-t border-gray-700 flex justify-end gap-3"><button onclick="document.getElementById(\'bug-report-overlay\').classList.add(\'hidden\')" class="text-sm text-gray-400 hover:text-gray-300 px-4 py-2">Cancel</button><button id="bug-submit-btn" onclick="submitBugReport()" class="text-sm bg-sky-600 hover:bg-sky-700 text-white font-medium px-5 py-2 rounded-lg transition-colors">Submit</button></div></div>';
  document.body.appendChild(overlay);
  overlay.addEventListener('click', function(e) { if (e.target === overlay) overlay.classList.add('hidden'); });
  window.openBugModal = async function() {
    const ov = document.getElementById('bug-report-overlay');
    const desc = document.getElementById('bug-desc');
    const si = document.getElementById('bug-sysinfo');
    const fb = document.getElementById('bug-feedback');
    desc.value = ''; fb.classList.add('hidden');
    document.getElementById('bug-submit-btn').disabled = false;
    document.getElementById('bug-submit-btn').textContent = 'Submit';
    const info = { userAgent: navigator.userAgent, screen: screen.width+'x'+screen.height, window: innerWidth+'x'+innerHeight, page: location.href, time: new Date().toISOString() };
    try { const r = await fetch('/health'); if (r.ok) info.gateway = await r.json(); } catch(e) { info.gateway = 'unavailable'; }
    si.textContent = JSON.stringify(info, null, 2);
    ov.classList.remove('hidden'); desc.focus();
  };
  window.submitBugReport = async function() {
    const desc = document.getElementById('bug-desc').value.trim();
    const fb = document.getElementById('bug-feedback');
    const btn = document.getElementById('bug-submit-btn');
    if (!desc) { fb.className='text-sm text-red-400'; fb.textContent='Please describe the bug.'; fb.classList.remove('hidden'); return; }
    btn.disabled = true; btn.textContent = 'Submitting...'; fb.classList.add('hidden');
    const si = { userAgent: navigator.userAgent, screen: screen.width+'x'+screen.height, window: innerWidth+'x'+innerHeight, page: location.href, time: new Date().toISOString() };
    try { const r = await fetch('/health'); if (r.ok) si.gateway = await r.json(); } catch(e) {}
    try {
      const res = await fetch('/api/bug-reports', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({ token: BUG_TOKEN, description: desc, system_info: si, page_url: location.href }) });
      const data = await res.json();
      if (data.id) { fb.className='text-sm text-green-400'; fb.textContent='Bug report #'+data.id+' submitted. Thank you!'; fb.classList.remove('hidden'); btn.textContent='Submitted'; setTimeout(function(){ document.getElementById('bug-report-overlay').classList.add('hidden'); }, 2000); }
      else { fb.className='text-sm text-red-400'; fb.textContent=data.error||'Submission failed.'; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
    } catch(e) { fb.className='text-sm text-red-400'; fb.textContent='Network error: '+e.message; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
  };
})();


// ── Mini player ─────────────────────────────────────────────────────────────
let mpLastEntityId = null;

async function mpPoll() {
  if (typeof authToken === 'undefined' || !authToken) return;
  try {
    const resp = await fetch(`/api/music/now_playing?token=${authToken}`, { headers: { 'Authorization': 'Bearer ' + authToken } });
    const data = await resp.json();
    const mp = document.getElementById('mini-player');
    if (!mp) return;
    // Apply duck state to mini player visuals
    if (data.ducking) {
      mp.style.opacity = '0.55';
    } else {
      mp.style.opacity = '1';
    }
    document.querySelectorAll('audio').forEach(a => { a.volume = data.ducking ? 0.2 : 1.0; });
    const state = data.state || 'off';
    const song = data.song || '';
    const artist = data.artist || '';
    mpLastEntityId = data.entity_id || null;
    if (state === 'off' || !song) { mp.classList.add('hidden'); return; }
    mp.classList.remove('hidden');
    document.getElementById('mp-song').textContent = song;
    document.getElementById('mp-artist').textContent = artist || '—';
    const source = data.source || '';
    let icon;
    if (source === 'phone') icon = '\u{1F4F1}';
    else if (source === 'homepod') icon = '\u{1F3E0}';
    else if (source === 'appletv') icon = '\u{1F4FA}';
    else if (source === 'sonos') icon = '\u{1F50A}';
    else if (source === 'apple_music_recent') icon = '\u{1F570}';
    else icon = '\u{1F3B5}';
    const srcEl = document.getElementById('mp-source-icon');
    if (srcEl) { srcEl.textContent = icon; srcEl.title = data.device || source; }
    const pb = document.getElementById('mp-play');
    if (pb) {
      if (state === 'playing') pb.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>';
      else pb.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
    }
  } catch(e) { /* silent */ }
}

async function mpControl(action) {
  if (typeof authToken === 'undefined' || !authToken) return;
  try {
    await fetch('/api/music/control', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: authToken, action, entity_id: mpLastEntityId }),
    });
    setTimeout(mpPoll, 500);
  } catch(e) { console.log('mp-control:', e); }
}

(function startMpPolling(){
  const tick = () => { mpPoll(); };
  if (typeof authToken !== 'undefined' && authToken) { tick(); setInterval(tick, 5000); }
  else {
    const w = setInterval(() => {
      if (typeof authToken !== 'undefined' && authToken) { clearInterval(w); tick(); setInterval(tick, 5000); }
    }, 1000);
  }
})();

// ═══════════════════════════════════════════════════════════════════════
// Command-center dashboard — additions for 3-pane layout
// ═══════════════════════════════════════════════════════════════════════

// Time-of-day greeting — "Good morning / afternoon / evening / night".
function ccGreeting() {
  const h = new Date().getHours();
  if (h < 5)  return 'Up late';
  if (h < 12) return 'Good morning';
  if (h < 17) return 'Good afternoon';
  if (h < 22) return 'Good evening';
  return 'Winding down';
}

function ccTitleCase(s) {
  return (s || '').split(/\s+/).map(w => w ? w[0].toUpperCase() + w.slice(1) : w).join(' ');
}
function ccUpdateGreeting() {
  const greetEl = document.getElementById('greeting-hello');
  if (greetEl) greetEl.textContent = ccGreeting();
  // Pull user first-name if we can derive it from the user-label.
  const userLabel = document.getElementById('user-label');
  const nameEl = document.getElementById('greeting-name');
  if (nameEl && userLabel && userLabel.textContent.trim()) {
    const full = userLabel.textContent.trim();
    nameEl.textContent = ccTitleCase(full.split(' ')[0]);
  }
}

// Hero ↔ chat swap. When the chat has more than the greeting block, hide hero.
function ccSyncHero() {
  const hero = document.getElementById('cc-hero');
  const msgs = document.getElementById('chat-messages');
  if (!hero || !msgs) return;
  // Chat has "real" messages if there's more than one child (greeting + replies).
  const hasRealMessages = msgs.querySelectorAll(':scope > div').length > 1;
  hero.classList.toggle('hidden', hasRealMessages);
  msgs.classList.toggle('hidden-when-hero', !hasRealMessages);
}
// Observe chat-messages for additions so the hero hides automatically when a
// conversation starts + reappears on clearChat().
(function() {
  const msgs = document.getElementById('chat-messages');
  if (!msgs) return;
  new MutationObserver(ccSyncHero).observe(msgs, { childList: true });
  ccSyncHero();
})();

// Update the persona strip + agent-picker when agent selection changes.
function ccUpdatePersona() {
  const name = document.getElementById('chat-agent-name');
  const stripName = document.getElementById('cc-persona-name');
  const initial = document.getElementById('cc-agent-initial');
  const stripInit = document.getElementById('cc-persona-initial');
  const input = document.getElementById('chat-input');
  if (!name) return;
  const raw = name.textContent.trim() || 'Peter';
  // If the switcher still shows the literal "Chat" label, default to agent "P" for Peter.
  const n = raw === 'Chat' ? 'Peter' : raw;
  if (stripName) stripName.textContent = n;
  const letter = n.charAt(0).toUpperCase() || 'P';
  if (initial) initial.textContent = letter;
  if (stripInit) stripInit.textContent = letter;
  if (input) input.placeholder = 'Message ' + n + '…';
}
// Re-run on interval (catches agent-switcher mutations without wiring into
// every callsite).
setInterval(ccUpdatePersona, 1500);

// ── Week-strip calendar view ─────────────────────────────────────────
let ccWeekAnchor = null;   // ISO date (Monday) of the week being viewed
function ccIsoDate(d) { return d.toISOString().slice(0, 10); }
function ccMondayOf(d) {
  const x = new Date(d);
  const dow = (x.getDay() + 6) % 7;  // 0 = Mon
  x.setDate(x.getDate() - dow);
  return x;
}
function ccRenderWeek() {
  const grid = document.getElementById('cal-week-strip');
  const titleEl = document.getElementById('cal-title');
  if (!grid) return;
  if (!ccWeekAnchor) ccWeekAnchor = ccIsoDate(ccMondayOf(new Date()));
  const mon = new Date(ccWeekAnchor + 'T00:00');
  const today = new Date(); today.setHours(0,0,0,0);
  const days = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(mon); d.setDate(mon.getDate() + i);
    days.push(d);
  }
  // Title
  const mFmt = { month: 'short' };
  if (titleEl) {
    const first = days[0], last = days[6];
    if (first.getMonth() === last.getMonth()) {
      titleEl.textContent = first.toLocaleDateString([], mFmt) + ' ' + first.getDate() + '–' + last.getDate();
    } else {
      titleEl.textContent = first.toLocaleDateString([], mFmt) + ' ' + first.getDate() + ' – ' + last.toLocaleDateString([], mFmt) + ' ' + last.getDate();
    }
  }
  // Events lookup — uses calEvents from the legacy calendar code if present.
  const dows = ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];
  grid.innerHTML = days.map((d, i) => {
    const iso = ccIsoDate(d);
    const isToday = d.getTime() === today.getTime();
    const hasEvent = typeof calEvents !== 'undefined'
      && Array.isArray(calEvents)
      && calEvents.some(e => (e.date || '').slice(0,10) === iso);
    const cls = ['cc-week-day'];
    if (isToday) cls.push('today');
    if (hasEvent) cls.push('has-event');
    return `<div class="${cls.join(' ')}" onclick="ccWeekDayClick('${iso}')">
      <div class="cc-week-dow">${dows[i]}</div>
      <div class="cc-week-num">${d.getDate()}</div>
    </div>`;
  }).join('');
}
function ccWeekDayClick(iso) {
  // Reuse the existing day-panel rendering from the legacy calendar code.
  if (typeof selectDate === 'function') selectDate(iso);
  else if (typeof showDayEvents === 'function') showDayEvents(iso);
}
// Override setCalView to support 'week' in addition to month / agenda.
(function() {
  const orig = typeof setCalView === 'function' ? setCalView : null;
  window.setCalView = function(view) {
    const week = document.getElementById('view-week');
    const month = document.getElementById('view-month');
    const agenda = document.getElementById('view-agenda');
    const wb = document.getElementById('view-week-btn');
    const mb = document.getElementById('view-month-btn');
    const ab = document.getElementById('view-agenda-btn');
    if (week) week.classList.toggle('hidden', view !== 'week');
    if (month) month.classList.toggle('hidden', view !== 'month');
    if (agenda) agenda.classList.toggle('hidden', view !== 'agenda');
    const sel = { background: 'var(--dash-rail)', color: 'var(--dash-ink)' };
    const unsel = { background: 'none', color: 'var(--dash-ink-mute)' };
    [[wb,'week'],[mb,'month'],[ab,'agenda']].forEach(([btn, k]) => {
      if (!btn) return;
      const s = (view === k) ? sel : unsel;
      btn.style.background = s.background;
      btn.style.color = s.color;
    });
    if (view === 'week') ccRenderWeek();
    else if (orig && view !== 'week') orig(view);
  };
})();
// Override calPrev / calNext to step by week when week view is active.
(function() {
  const origPrev = typeof calPrev === 'function' ? calPrev : null;
  const origNext = typeof calNext === 'function' ? calNext : null;
  window.calPrev = function() {
    const weekVisible = !document.getElementById('view-week')?.classList.contains('hidden');
    if (weekVisible) {
      const d = new Date(ccWeekAnchor + 'T00:00'); d.setDate(d.getDate() - 7);
      ccWeekAnchor = ccIsoDate(d); ccRenderWeek(); return;
    }
    if (origPrev) origPrev();
  };
  window.calNext = function() {
    const weekVisible = !document.getElementById('view-week')?.classList.contains('hidden');
    if (weekVisible) {
      const d = new Date(ccWeekAnchor + 'T00:00'); d.setDate(d.getDate() + 7);
      ccWeekAnchor = ccIsoDate(d); ccRenderWeek(); return;
    }
    if (origNext) origNext();
  };
})();

// ── NEXT up — countdown to the most imminent event/task ─────────────
function ccRefreshNextUp() {
  const block = document.getElementById('cc-next-block');
  const text  = document.getElementById('next-up-text');
  const count = document.getElementById('next-up-countdown');
  const meta  = document.getElementById('next-up-meta');
  if (!block || !text) return;
  const now = new Date();
  let best = null;
  if (typeof calEvents !== 'undefined' && Array.isArray(calEvents)) {
    for (const e of calEvents) {
      if (!e.date) continue;
      const iso = e.allday ? e.date + 'T08:00' : (e.date + 'T' + (e.time || '09:00'));
      const when = new Date(iso);
      if (isNaN(when) || when < now) continue;
      if (!best || when < best.when) best = { ev: e, when };
    }
  }
  if (!best) {
    block.classList.remove('has-event'); block.classList.add('empty');
    text.textContent = 'Nothing imminent — a clear window.';
    count.classList.add('hidden'); meta.classList.add('hidden');
    return;
  }
  block.classList.add('has-event'); block.classList.remove('empty');
  text.textContent = best.ev.title || 'Event';
  const diff = best.when - now;
  const mins = Math.floor(diff / 60000);
  let str;
  if (mins < 60) str = mins + (mins === 1 ? ' minute' : ' minutes');
  else if (mins < 1440) {
    const h = Math.floor(mins / 60); const m = mins % 60;
    str = h + 'h' + (m ? ' ' + m + 'm' : '');
  } else {
    const days = Math.floor(mins / 1440);
    str = days + (days === 1 ? ' day' : ' days');
  }
  count.textContent = 'in ' + str;
  count.classList.remove('hidden');
  count.classList.remove('warn', 'danger');
  if (mins <= 30) count.classList.add('warn');
  if (mins <= 10) count.classList.add('danger');
  meta.textContent = best.when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  meta.classList.remove('hidden');
}
setInterval(ccRefreshNextUp, 30000);

// ── Recent threads in left rail ──────────────────────────────────────
async function ccLoadRecentThreads() {
  const box = document.getElementById('recent-threads-list');
  if (!box) return;
  try {
    const r = await authFetch('/api/conversations/recent?token=' + encodeURIComponent(authToken) + '&limit=6');
    if (!r.ok) return;
    const data = await r.json();
    const rows = data.conversations || data.items || data || [];
    if (!rows.length) {
      box.innerHTML = '<div class="cc-row cc-row-thread" style="color:var(--dash-ink-faint);padding:7px 8px"><div class="cc-row-label">No prior conversations yet</div></div>';
      return;
    }
    box.innerHTML = rows.slice(0, 6).map(c => {
      const title = c.title || c.preview || c.first_message || 'Untitled';
      const when = c.updated_at || c.created_at;
      const relative = ccRelativeTime(when);
      return `<a href="/chat?c=${encodeURIComponent(c.id)}" class="cc-row cc-row-thread">
        <div class="cc-row-label" title="${ccEsc(title)}">${ccEsc(title.slice(0, 48))}</div>
        <div class="cc-row-sub">${ccEsc(relative)}</div>
      </a>`;
    }).join('');
  } catch (e) { /* silent */ }
}
function ccRelativeTime(when) {
  if (!when) return '';
  const then = typeof when === 'number' ? new Date(when * 1000) : new Date(when);
  const diff = (Date.now() - then.getTime()) / 1000;
  if (diff < 60) return 'just now';
  if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
  if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
  return Math.floor(diff / 86400) + 'd ago';
}
function ccEsc(s) {
  return String(s || '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
}

// ── Module activity badges (tax receipts pending, music playing) ─────
async function ccRefreshModuleBadges() {
  // Music badge from the mini-player (already polling via mpPoll).
  const song = document.getElementById('mp-song');
  const musicBadge = document.getElementById('mod-badge-music');
  if (musicBadge && song) {
    const playing = song.textContent && song.textContent !== '—';
    musicBadge.classList.toggle('hidden', !playing);
    musicBadge.classList.toggle('active', playing);
    musicBadge.textContent = playing ? 'playing' : '';
  }
  // Tax badge — try to fetch pending-count; silent if endpoint missing.
  try {
    const r = await authFetch('/api/tax/deductions/pending/count?token=' + encodeURIComponent(authToken));
    if (r.ok) {
      const d = await r.json();
      const b = document.getElementById('mod-badge-tax');
      if (b) {
        const n = d.count || d.pending || 0;
        if (n > 0) { b.textContent = n; b.classList.remove('hidden'); b.classList.add('active'); }
        else b.classList.add('hidden');
      }
    }
  } catch(e) {}
}
setInterval(ccRefreshModuleBadges, 30000);

// ── Ambient footer state ─────────────────────────────────────────────
function ccRefreshAmbient() {
  const amEl = document.getElementById('ambient-music');
  const song = document.getElementById('mp-song');
  const artist = document.getElementById('mp-artist');
  if (amEl && song) {
    const s = (song.textContent || '').trim();
    const a = (artist?.textContent || '').trim();
    if (s && s !== '—') amEl.textContent = s + (a && a !== '—' ? ' · ' + a : '');
    else amEl.textContent = 'Nothing playing';
  }
}
setInterval(ccRefreshAmbient, 5000);

// ── Focus mode — Cmd/Ctrl + . to collapse the two rails ──────────────
function toggleFocusMode() {
  const shell = document.getElementById('panel-main');
  if (!shell) return;
  shell.classList.toggle('focus-mode');
}
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === '.') {
    e.preventDefault();
    toggleFocusMode();
  }
});

// ── Init sequence ────────────────────────────────────────────────────
(function() {
  const wait = setInterval(() => {
    if (typeof authToken !== 'undefined' && authToken) {
      clearInterval(wait);
      ccUpdateGreeting();
      setInterval(ccUpdateGreeting, 60000);
      ccRenderWeek();
      ccRefreshNextUp();
      ccLoadRecentThreads();
      setInterval(ccLoadRecentThreads, 120000);
      ccRefreshModuleBadges();
      ccRefreshAmbient();
      ccUpdatePersona();
    }
  }, 500);
})();

</script>"##;
