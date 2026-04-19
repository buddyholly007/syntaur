//! Shared page shell — `<head>`, `<body>` wrapper, unified top bar, and the
//! bug-report overlay. The first 48px of every authenticated page is rendered
//! here so navigation is identical across modules: logo, wordmark, single-
//! chevron separator, module name, one optional status pill, then `Modules
//! ⌘K` and the avatar dropdown with Dashboard / Modules / Settings / Profile
//! / Log out. Modules keep their own theming *inside* the canvas below the
//! bar (ornaments, subtitles, tab rows) so identity stays visible without
//! breaking muscle memory.

use maud::{html, Markup, PreEscaped, DOCTYPE};

/// Describes a page for the shell — everything that goes in `<head>` or
/// decides whether to inject the bug-report overlay.
pub struct Page {
    pub title: &'static str,
    /// Authenticated pages get the bug-report overlay + button injected.
    pub authed: bool,
    /// Optional page-specific `<style>` block (after the shared styles).
    pub extra_style: Option<&'static str>,
}

/// One of four standard module-status shapes shown in the top bar right
/// after the crumb. Same colors, same pill, same slot — just a text label
/// that each module supplies. Modules with no status pass `None`.
#[derive(Debug, Clone, Copy)]
pub enum ModuleStatusKind {
    Ok,
    Info,
    Warn,
    Pause,
}

pub struct ModuleStatus {
    pub kind: ModuleStatusKind,
    pub text: String,
}

impl ModuleStatus {
    pub fn ok(text: impl Into<String>) -> Self { Self { kind: ModuleStatusKind::Ok, text: text.into() } }
    pub fn info(text: impl Into<String>) -> Self { Self { kind: ModuleStatusKind::Info, text: text.into() } }
    pub fn warn(text: impl Into<String>) -> Self { Self { kind: ModuleStatusKind::Warn, text: text.into() } }
    pub fn pause(text: impl Into<String>) -> Self { Self { kind: ModuleStatusKind::Pause, text: text.into() } }
    fn class(&self) -> &'static str {
        match self.kind {
            ModuleStatusKind::Ok => "ok",
            ModuleStatusKind::Info => "info",
            ModuleStatusKind::Warn => "warn",
            ModuleStatusKind::Pause => "pause",
        }
    }
}

/// Render a full HTML document. The page's `body_content` is wrapped in
/// `<body>` and followed by the bug-report overlay if `authed`.
pub fn shell(page: Page, body_content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" class="dark" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                meta name="theme-color" content="#0284c7";
                link rel="icon" href="/favicon.ico" type="image/x-icon";
                link rel="icon" href="/favicon-32.png" type="image/png" sizes="32x32";
                link rel="apple-touch-icon" href="/icon-192.png";
                link rel="manifest" href="/manifest.json";
                meta name="apple-mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-status-bar-style" content="black-translucent";
                title { "Syntaur — " (page.title) }
                script src="/tailwind.js" {}
                script { (PreEscaped(TAILWIND_CONFIG)) }
                style { (PreEscaped(BASE_STYLES)) }
                style { (PreEscaped(TOP_BAR_STYLES)) }
                @if let Some(extra) = page.extra_style {
                    style { (PreEscaped(extra)) }
                }
            }
            body class="bg-gray-950 text-gray-100 min-h-screen" {
                (body_content)
                @if page.authed {
                    (PreEscaped(TOP_BAR_SCRIPT))
                    (bug_report_overlay())
                }
            }
        }
    }
}

/// Unified top bar. Every authenticated page renders this as its first
/// element. The `crumb` is the module name shown after `Syntaur ›`.
/// Pass `status` when the module has a single meaningful state to
/// surface (Dashboard: Online, Music: Bridge live, Tax: deadline,
/// Social: Paused). Otherwise pass `None`.
pub fn top_bar(crumb: &str, status: Option<ModuleStatus>) -> Markup {
    html! {
        header.syntaur-topbar role="navigation" aria-label="Syntaur navigation" {
            a.brand href="/dashboard" aria-label="Dashboard" {
                img.brand-mark src="/app-icon.jpg" alt="";
                span.brand-text { "Syntaur" }
            }
            span.crumb-sep aria-hidden="true" { "›" }
            span.module-name { (crumb) }
            @if let Some(s) = &status {
                span class=(format!("module-status-pill {}", s.class())) {
                    span.dot {}
                    span.txt { (s.text.as_str()) }
                }
            }
            div.spacer {}
            button.modules-btn type="button" onclick="openModulesPalette()" aria-label="Jump to module" {
                span { "Modules" }
                span.kbd { "⌘K" }
            }
            div.avatar-wrap {
                button.avatar-btn type="button" onclick="toggleAvatarMenu(event)" aria-haspopup="menu" aria-expanded="false" id="avatar-btn" {
                    img src="/app-icon.jpg" alt="Account";
                    span.chev aria-hidden="true" { "▾" }
                }
                div.avatar-menu id="avatar-menu" role="menu" hidden {
                    a.mi href="/dashboard" role="menuitem" { "Dashboard" }
                    button.mi type="button" onclick="openModulesPalette(); toggleAvatarMenu(event)" role="menuitem" { "Modules…" }
                    div.sep {}
                    a.mi href="/settings" role="menuitem" { "Settings" }
                    a.mi href="/profile" role="menuitem" { "Profile" }
                    div.sep {}
                    button.mi type="button" onclick="syntaurLogout()" role="menuitem" { "Log out" }
                }
            }
        }
        (PreEscaped(MODULES_PALETTE_HTML))
    }
}

/// Back-compat alias for the many pages already using `top_bar_standard`.
/// Renders `top_bar(crumb, None)`.
pub fn top_bar_standard(crumb: &str) -> Markup {
    top_bar(crumb, None)
}

/// Bug-report modal + JS, injected on authenticated pages.
fn bug_report_overlay() -> Markup {
    html! {
        script { (PreEscaped(BUG_REPORT_JS)) }
    }
}

// ── Embedded styles / scripts ──────────────────────────────────────────
// These live in .rs files so linguist counts them toward Rust. They're
// also trivially searchable with `grep` across src/.

const TAILWIND_CONFIG: &str = r#"
tailwind.config = {
  darkMode: 'class',
  theme: { extend: { colors: { oc: {
    500: '#0ea5e9', 600: '#0284c7', 700: '#0369a1',
    50: '#f0f9ff', 100: '#e0f2fe', 800: '#075985', 900: '#0c4a6e'
  } } } }
};
"#;

const BASE_STYLES: &str = r#"
@import url('/fonts.css');
body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
.card { @apply bg-gray-800 rounded-xl border border-gray-700; }
.badge { @apply inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium; }
.badge-green { @apply bg-green-900/50 text-green-400; }
.badge-red { @apply bg-red-900/50 text-red-400; }
.badge-blue { @apply bg-blue-900/50 text-blue-400; }
.badge-gray { @apply bg-gray-700 text-gray-400; }
.toggle { @apply relative inline-flex h-6 w-11 items-center rounded-full transition-colors cursor-pointer; }
.toggle-dot { @apply inline-block h-4 w-4 transform rounded-full bg-white transition-transform; }
"#;

const TOP_BAR_STYLES: &str = r#"
/* ── Syntaur unified top bar ─────────────────────────────── */
.syntaur-topbar {
    position: sticky; top: 0; z-index: 40;
    height: 48px;
    display: flex; align-items: center;
    padding: 0 16px; gap: 12px;
    background: rgba(15, 23, 42, 0.82);
    backdrop-filter: blur(14px);
    -webkit-backdrop-filter: blur(14px);
    border-bottom: 1px solid rgb(31, 41, 55);
    font-family: Inter, sans-serif;
    color: rgb(229,231,235);
}
.syntaur-topbar .brand {
    display: flex; align-items: center; gap: 8px;
    color: rgb(243, 244, 246); text-decoration: none;
}
.syntaur-topbar .brand:hover { opacity: 0.85; }
.syntaur-topbar .brand-mark { height: 26px; width: 26px; border-radius: 6px; }
.syntaur-topbar .brand-text { font-size: 15px; font-weight: 600; letter-spacing: -0.01em; }
.syntaur-topbar .crumb-sep { color: rgb(75, 85, 99); font-size: 16px; user-select: none; line-height: 1; }
.syntaur-topbar .module-name { color: rgb(209, 213, 219); font-size: 14px; font-weight: 500; }
.syntaur-topbar .spacer { flex: 1; }

/* Module status pill — 4 shapes, single slot */
.module-status-pill {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 2px 10px;
    border-radius: 999px;
    font-size: 12px;
    border: 1px solid;
    margin-left: 2px;
    white-space: nowrap;
}
.module-status-pill .dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
.module-status-pill.ok    { border-color: rgba(34,197,94,0.4);  color: rgb(134,239,172); background: rgba(34,197,94,0.08); }
.module-status-pill.ok    .dot { background: rgb(34,197,94); box-shadow: 0 0 6px rgba(34,197,94,0.5); }
.module-status-pill.info  { border-color: rgba(59,130,246,0.4); color: rgb(147,197,253); background: rgba(59,130,246,0.08); }
.module-status-pill.info  .dot { background: rgb(59,130,246); }
.module-status-pill.warn  { border-color: rgba(234,179,8,0.4);  color: rgb(253,224,71); background: rgba(234,179,8,0.08); }
.module-status-pill.warn  .dot { background: rgb(234,179,8); }
.module-status-pill.pause { border-color: rgba(148,163,184,0.35); color: rgb(203,213,225); background: rgba(148,163,184,0.08); }
.module-status-pill.pause .dot { background: rgb(148,163,184); }

/* Modules button + keyboard hint */
.syntaur-topbar .modules-btn {
    display: inline-flex; align-items: center; gap: 8px;
    color: rgb(156,163,175); font-size: 13px;
    padding: 6px 10px; border-radius: 6px;
    background: transparent; border: none; cursor: pointer;
    font-family: inherit;
}
.syntaur-topbar .modules-btn:hover { color: rgb(229,231,235); background: rgba(55,65,81,0.4); }
.syntaur-topbar .modules-btn .kbd {
    font-family: 'SF Mono', 'JetBrains Mono', ui-monospace, monospace;
    font-size: 10px; padding: 1px 5px;
    border: 1px solid rgb(55,65,81); border-radius: 3px;
    background: rgba(0,0,0,0.2);
    color: rgb(156,163,175);
}

/* Avatar + dropdown */
.syntaur-topbar .avatar-wrap { position: relative; }
.syntaur-topbar .avatar-btn {
    display: inline-flex; align-items: center; gap: 4px;
    background: transparent; border: none; cursor: pointer;
    padding: 2px 6px 2px 2px; border-radius: 999px;
}
.syntaur-topbar .avatar-btn:hover { background: rgba(55,65,81,0.5); }
.syntaur-topbar .avatar-btn img { height: 28px; width: 28px; border-radius: 50%; object-fit: cover; }
.syntaur-topbar .avatar-btn .chev { color: rgb(156,163,175); font-size: 10px; }
.avatar-menu {
    position: absolute; top: calc(100% + 6px); right: 0;
    min-width: 180px;
    background: rgb(17, 24, 39);
    border: 1px solid rgb(55,65,81);
    border-radius: 8px;
    padding: 4px;
    box-shadow: 0 10px 30px rgba(0,0,0,0.4);
    z-index: 50;
    flex-direction: column;
    display: flex;
}
.avatar-menu[hidden] { display: none; }
.avatar-menu .mi {
    display: block; padding: 8px 12px;
    color: rgb(229,231,235); font-size: 13px;
    text-decoration: none;
    background: transparent; border: none; text-align: left;
    cursor: pointer; border-radius: 4px;
    font-family: inherit; width: 100%;
}
.avatar-menu .mi:hover { background: rgba(55,65,81,0.6); }
.avatar-menu .sep { height: 1px; background: rgb(55,65,81); margin: 4px 0; }

/* Modules palette (cmd-K) */
#modules-palette {
    position: fixed; inset: 0; z-index: 60;
    background: rgba(0,0,0,0.55); backdrop-filter: blur(4px);
    display: flex; align-items: flex-start; justify-content: center;
    padding-top: 15vh;
}
#modules-palette[hidden] { display: none; }
#modules-palette .box {
    width: 100%; max-width: 480px;
    background: rgb(17,24,39);
    border: 1px solid rgb(55,65,81);
    border-radius: 12px;
    box-shadow: 0 30px 80px rgba(0,0,0,0.5);
    overflow: hidden;
    font-family: Inter, sans-serif;
}
#modules-palette .search {
    width: 100%; padding: 14px 16px;
    background: transparent; border: none; outline: none;
    color: rgb(229,231,235); font-size: 15px;
    border-bottom: 1px solid rgb(55,65,81);
    font-family: inherit;
}
#modules-palette .list {
    list-style: none; padding: 6px; margin: 0;
    max-height: 50vh; overflow-y: auto;
}
#modules-palette .list li {
    padding: 10px 12px; border-radius: 6px;
    color: rgb(229,231,235); font-size: 14px; cursor: pointer;
    display: flex; align-items: center; gap: 10px;
}
#modules-palette .list li:hover,
#modules-palette .list li.active { background: rgba(59,130,246,0.18); color: rgb(191,219,254); }
#modules-palette .list .m-icon { color: rgb(156,163,175); width: 14px; text-align: center; }
#modules-palette .list .m-sub { color: rgb(156,163,175); font-size: 12px; margin-left: auto; }

/* Ensure the bug-report button placed by BUG_REPORT_JS fits the new bar */
.syntaur-topbar .bugrpt-btn {
    color: rgb(107,114,128); background: transparent; border: none; cursor: pointer;
    padding: 4px; border-radius: 4px; display: inline-flex; align-items: center;
}
.syntaur-topbar .bugrpt-btn:hover { color: rgb(209,213,219); background: rgba(55,65,81,0.4); }
"#;

const MODULES_PALETTE_HTML: &str = r#"
<div id="modules-palette" hidden aria-hidden="true">
  <div class="box" role="dialog" aria-label="Jump to module">
    <input class="search" type="text" placeholder="Jump to module… (esc to close)" aria-label="Filter modules">
    <ul class="list"></ul>
  </div>
</div>
"#;

const TOP_BAR_SCRIPT: &str = r##"
<script>
(function() {
  // ── avatar menu toggle ─────────────────────────────────
  window.toggleAvatarMenu = function(ev) {
    if (ev && ev.stopPropagation) ev.stopPropagation();
    const m = document.getElementById('avatar-menu');
    const btn = document.getElementById('avatar-btn');
    if (!m) return;
    m.hidden = !m.hidden;
    if (btn) btn.setAttribute('aria-expanded', m.hidden ? 'false' : 'true');
  };
  document.addEventListener('click', function(ev) {
    const m = document.getElementById('avatar-menu');
    if (!m || m.hidden) return;
    if (ev.target && ev.target.closest && ev.target.closest('.avatar-menu, .avatar-btn')) return;
    m.hidden = true;
    const btn = document.getElementById('avatar-btn');
    if (btn) btn.setAttribute('aria-expanded', 'false');
  });

  window.syntaurLogout = function() {
    try { localStorage.removeItem('syntaur_token'); sessionStorage.removeItem('syntaur_token'); } catch(e) {}
    document.cookie = 'syntaur_token=; expires=Thu, 01 Jan 1970 00:00:01 GMT; path=/';
    window.location.href = '/';
  };

  // ── modules palette (⌘K / Ctrl-K) ──────────────────────
  const SYNTAUR_MODULES = [
    { path: '/chat',       name: 'Chat',      sub: 'Main agent' },
    { path: '/coders',     name: 'Coders',    sub: 'Maurice' },
    { path: '/tax',        name: 'Tax',       sub: 'Positron' },
    { path: '/knowledge',  name: 'Knowledge', sub: 'Cortex' },
    { path: '/music',      name: 'Music',     sub: 'Silvr' },
    { path: '/journal',    name: 'Journal',   sub: 'Mushi' },
    { path: '/social',     name: 'Social',    sub: 'Nyota' },
    { path: '/dashboard',  name: 'Dashboard', sub: 'Overview' },
    { path: '/settings',   name: 'Settings',  sub: null },
    { path: '/profile',    name: 'Profile',   sub: null },
    { path: '/history',    name: 'History',   sub: null },
  ];

  function renderList(q) {
    const list = document.querySelector('#modules-palette .list');
    if (!list) return;
    const needle = (q || '').trim().toLowerCase();
    const items = SYNTAUR_MODULES.filter(m =>
      !needle || m.name.toLowerCase().includes(needle) || (m.sub || '').toLowerCase().includes(needle)
    );
    if (items.length === 0) {
      list.innerHTML = '<li style="color:#6b7280;cursor:default"><span class="m-icon">—</span> No match</li>';
      return;
    }
    list.innerHTML = items.map(m =>
      `<li data-path="${m.path}" onclick="location.href='${m.path}'">`
      + `<span class="m-icon">›</span>`
      + `<span>${m.name}</span>`
      + (m.sub ? `<span class="m-sub">${m.sub}</span>` : '')
      + `</li>`
    ).join('');
    list.querySelector('li').classList.add('active');
  }
  window.openModulesPalette = function() {
    const m = document.getElementById('modules-palette');
    if (!m) return;
    m.hidden = false;
    m.setAttribute('aria-hidden', 'false');
    const input = m.querySelector('.search');
    if (input) { input.value = ''; setTimeout(() => input.focus(), 10); }
    renderList('');
  };
  window.closeModulesPalette = function() {
    const m = document.getElementById('modules-palette');
    if (!m) return;
    m.hidden = true;
    m.setAttribute('aria-hidden', 'true');
  };

  document.addEventListener('keydown', function(ev) {
    if ((ev.ctrlKey || ev.metaKey) && ev.key && ev.key.toLowerCase() === 'k') {
      // Settings has its own ⌘K palette (in-settings fuzzy search). Don't
      // steal the shortcut there — user can still click the "Modules"
      // button in the top bar.
      if (location.pathname.startsWith('/settings')) return;
      ev.preventDefault();
      window.openModulesPalette();
      return;
    }
    const m = document.getElementById('modules-palette');
    if (!m || m.hidden) return;
    if (ev.key === 'Escape') { ev.preventDefault(); window.closeModulesPalette(); return; }
    const list = m.querySelector('.list');
    if (!list) return;
    const items = Array.from(list.querySelectorAll('li[data-path]'));
    if (items.length === 0) return;
    let idx = items.findIndex(li => li.classList.contains('active'));
    if (idx < 0) idx = 0;
    if (ev.key === 'ArrowDown') {
      ev.preventDefault();
      items[idx].classList.remove('active');
      items[(idx + 1) % items.length].classList.add('active');
      items[(idx + 1) % items.length].scrollIntoView({ block: 'nearest' });
    } else if (ev.key === 'ArrowUp') {
      ev.preventDefault();
      items[idx].classList.remove('active');
      const nxt = (idx - 1 + items.length) % items.length;
      items[nxt].classList.add('active');
      items[nxt].scrollIntoView({ block: 'nearest' });
    } else if (ev.key === 'Enter') {
      ev.preventDefault();
      items[idx].click();
    }
  });
  document.addEventListener('input', function(ev) {
    if (ev.target && ev.target.closest && ev.target.closest('#modules-palette .search')) {
      renderList(ev.target.value);
    }
  });
  // Click outside the palette box to close
  document.addEventListener('click', function(ev) {
    const m = document.getElementById('modules-palette');
    if (!m || m.hidden) return;
    if (!ev.target.closest('#modules-palette .box, .modules-btn')) {
      window.closeModulesPalette();
    }
  });
})();
</script>
"##;

/// Bug-report overlay + submit flow. Placed by JS into `.syntaur-topbar` as a
/// small icon button just before the avatar. Pure vanilla JS.
const BUG_REPORT_JS: &str = r#"
(function() {
  const BUG_TOKEN = sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '';
  if (!BUG_TOKEN) return;
  const topBar = document.querySelector('.syntaur-topbar');
  const avatar = topBar && topBar.querySelector('.avatar-wrap');
  if (!topBar || !avatar) return;
  const bugBtn = document.createElement('button');
  bugBtn.title = 'Report a Bug';
  bugBtn.type = 'button';
  bugBtn.className = 'bugrpt-btn';
  bugBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2l1.88 1.88M14.12 3.88L16 2M9 7.13v-1a3.003 3.003 0 116 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 014-4h4a4 4 0 014 4v3c0 3.3-2.7 6-6 6z"/><path d="M12 20v-9M6.53 9C4.6 8.8 3 7.1 3 5M6 13H2M6 17l-4 1M17.47 9c1.93-.2 3.53-1.9 3.53-4M18 13h4M18 17l4 1"/></svg>';
  bugBtn.onclick = function() { openBugModal(); };
  topBar.insertBefore(bugBtn, avatar);
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
"#;
