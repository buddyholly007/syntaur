//! Shared page shell — `<head>`, `<body>` wrapper, and the bug-report
//! overlay. Each page builds its own top bar (pages have meaningfully
//! different top-bar content: crumbs vs voice-journal title, one right
//! link vs many, different logo treatments) so the shell stays
//! opinion-free about that.

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
                @if let Some(extra) = page.extra_style {
                    style { (PreEscaped(extra)) }
                }
            }
            body class="bg-gray-950 text-gray-100 min-h-screen" {
                (body_content)
                @if page.authed {
                    (bug_report_overlay())
                }
            }
        }
    }
}

/// Standard top bar with brand + breadcrumb + "← Dashboard" right link.
/// Used by `/modules`, `/settings`, etc. Pages with non-standard right
/// nav (e.g. `/history` which has two links) should inline their own.
pub fn top_bar_standard(crumb: &str) -> Markup {
    html! {
        div class="border-b border-gray-800 bg-gray-900/50 backdrop-blur sticky top-0 z-40" {
            div class="max-w-5xl mx-auto px-4 py-3 flex items-center justify-between" {
                div class="flex items-center gap-3" {
                    a href="/" class="flex items-center gap-2 hover:opacity-80" {
                        img src="/app-icon.jpg" class="h-8 w-8 rounded-lg" alt="";
                        span class="font-semibold text-lg" { "Syntaur" }
                    }
                    span class="text-gray-600" { "/" }
                    span class="text-gray-400" { (crumb) }
                }
                a href="/" class="text-sm text-gray-400 hover:text-gray-300" { "← Dashboard" }
            }
        }
    }
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

/// Bug-report overlay + submit flow. Pure vanilla JS — could be
/// generated from a typed struct later, but for the first migrations we
/// preserve behavior exactly.
const BUG_REPORT_JS: &str = r#"
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
"#;
