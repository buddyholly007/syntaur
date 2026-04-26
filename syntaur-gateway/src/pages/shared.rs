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
#[derive(Default)]
pub struct Page {
    pub title: &'static str,
    /// Authenticated pages get the bug-report overlay + button injected.
    pub authed: bool,
    /// Optional page-specific `<style>` block (after the shared styles).
    pub extra_style: Option<&'static str>,
    /// Override the default body class. Dashboard uses this to paint
    /// `syntaur-ambient` from first paint (no dark→light flash once
    /// theme.rs runs post-paint).
    pub body_class: Option<&'static str>,
    /// Extra `<script>` emitted at the top of `<head>`. Runs
    /// synchronously before any body markup paints — ideal for
    /// reading localStorage pref + applying `html.theme-light` so
    /// the first paint is already in the right palette.
    pub head_boot: Option<&'static str>,
    /// Top-bar crumb override. Defaults to `title` when None. Set this
    /// when the crumb shown next to the brand should differ from the
    /// browser-tab title (rare).
    pub crumb: Option<&'static str>,
    /// Top-bar status pill (right of crumb). None hides the pill.
    /// Module pages with a single meaningful state set this; most
    /// modules pass None.
    pub topbar_status: Option<ModuleStatus>,
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

#[derive(Clone)]
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
                    // Marker class lets the SPA router find + replace
                    // per-page styles when content-swapping between modules.
                    // Stable styles above (BASE / TOP_BAR) are not marked,
                    // so they survive the swap.
                    style class="syntaur-page" { (PreEscaped(extra)) }
                }
                @if let Some(boot) = page.head_boot {
                    script class="syntaur-page-boot" { (PreEscaped(boot)) }
                }
            }
            body class=(page.body_class.unwrap_or("bg-gray-950 text-gray-100 min-h-screen")) {
                // Top bar lives at body root, OUTSIDE #syntaur-app-content.
                // This is what makes SPA navigation tractable: the audio
                // element (rendered inside top_bar()) is never inside the
                // swap zone, so it's never destroyed mid-playback. Pages
                // that want a different crumb than their title set
                // page.crumb; same for status via page.topbar_status.
                @if page.authed {
                    (top_bar(page.crumb.unwrap_or(page.title), page.topbar_status.clone()))
                }
                // Stable container the SPA router replaces on navigation.
                // EVERYTHING outside this container persists across nav.
                main id="syntaur-app-content" data-page=(page.title) {
                    (body_content)
                }
                @if page.authed {
                    (PreEscaped(GLOBAL_MINI_PLAYER_HTML))
                    (PreEscaped(TOP_BAR_SCRIPT))
                    (PreEscaped(SPA_ROUTER_SCRIPT))
                    (bug_report_overlay())
                    // Per-chat agent settings cog auto-mounter — runs on
                    // every authed page so /knowledge, /scheduler, /journal,
                    // /coders, /dashboard etc. all get the cog injected onto
                    // their respective chat panels (PANEL_REGISTRY) plus the
                    // mic-next-to-send (SEND_REGISTRY / INPUT_ROW_REGISTRY).
                    // /chat additionally wraps server-side via chat_card_flip.
                    (super::agent_settings_card::resource_budget_styles())
                    (super::agent_settings_card::agent_settings_overlay())
                    (super::agent_settings_card::resource_budget_script())
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
            // Persistent music pipeline: this hidden <audio> element
            // is in the top-bar so it lives on every page. On every
            // page load, the init script below reads localStorage and
            // resumes playback from the saved trackId + currentTime,
            // which is what gives the cross-module "music keeps
            // playing" behaviour. The existing floating mini-player
            // pill (rendered after this top-bar via
            // GLOBAL_MINI_PLAYER_HTML) drives play/pause/next on it.
            audio id="global-audio" preload="auto" style="display:none" {}
            div.spacer {}
            button.modules-btn type="button" onclick="openModulesPalette()" aria-label="Jump to module" {
                span { "Modules" }
                span.kbd { "⌘K" }
            }
            button.bugrpt-btn type="button" onclick="openBugModal && openBugModal()" title="Report a bug" aria-label="Report a bug" {
                svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" {
                    path d="M8 2l1.88 1.88M14.12 3.88L16 2M9 7.13v-1a3.003 3.003 0 116 0v1" {}
                    path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 014-4h4a4 4 0 014 4v3c0 3.3-2.7 6-6 6z" {}
                    path d="M12 20v-9M6.53 9C4.6 8.8 3 7.1 3 5M6 13H2M6 17l-4 1M17.47 9c1.93-.2 3.53-1.9 3.53-4M18 13h4M18 17l4 1" {}
                }
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
/* SPA shell: the swappable container is a flex column with an
   explicit height of `viewport - top-bar`. Pages whose root layout
   wants `height: 100vh` (coders' CRT workshop-root) or
   `min-height: calc(100vh - Npx)` (settings .ss-shell) used to anchor
   to body's viewport context; after the SPA refactor lifted top_bar
   out and wrapped content in this main, those root layouts had no
   parent with a real height and collapsed to 0px on SPA-arrival in
   WebKit (fresh-load worked because the layout pipeline computes
   viewport-relative units differently from the swap path). Fix:
   give main a real height + make it a flex container so children
   that say `height: 100%` or `flex: 1` have something concrete to
   claim. The 48px subtracts the shell-rendered top bar. */
main#syntaur-app-content {
  display: flex; flex-direction: column;
  min-height: calc(100vh - 48px);
  height: calc(100vh - 48px);
}
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

/* Global mini-player pill — fixed bottom-right, visible on every authed page */
#syntaur-mini-player {
    position: fixed; right: 16px; bottom: 16px; z-index: 55;
    background: rgba(15, 23, 42, 0.92); backdrop-filter: blur(14px);
    border: 1px solid rgb(55, 65, 81); border-radius: 999px;
    padding: 6px 12px 6px 8px;
    display: inline-flex; align-items: center; gap: 10px;
    color: rgb(229, 231, 235); font-family: Inter, system-ui, sans-serif; font-size: 12px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
    max-width: 420px;
    transition: opacity 0.25s;
}
#syntaur-mini-player[hidden] { display: none; }
#syntaur-mini-player .smp-cover {
    width: 32px; height: 32px; border-radius: 6px;
    background: rgb(31, 41, 55); display: inline-flex; align-items: center; justify-content: center;
    color: rgb(156, 163, 175); text-decoration: none; flex-shrink: 0;
    overflow: hidden; position: relative;
}
#syntaur-mini-player .smp-cover .smp-art {
    position: absolute; inset: 0;
    background-size: cover; background-position: center;
    background-repeat: no-repeat;
    transition: opacity 200ms ease;
}
#syntaur-mini-player .smp-cover:not(.has-art) .smp-art { display: none; }
#syntaur-mini-player .smp-cover.has-art .smp-cover-fallback { display: none; }
#syntaur-mini-player .smp-cover:hover { color: rgb(229, 231, 235); background: rgb(55, 65, 81); }
#syntaur-mini-player .smp-meta { min-width: 0; display: flex; flex-direction: column; max-width: 240px; gap: 2px; }
#syntaur-mini-player .smp-title {
    font-weight: 600; color: rgb(229, 231, 235);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    line-height: 1.25;
}
#syntaur-mini-player .smp-sub   {
    color: rgb(156, 163, 175); font-size: 11px; line-height: 1.2;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
/* Click-to-seek progress bar — sits below the title/artist lines.
   Hidden when no track is loaded (smp-progress-fill width = 0%). */
#syntaur-mini-player .smp-progress {
    height: 3px; background: rgba(148, 163, 184, 0.25);
    border-radius: 2px; cursor: pointer; margin-top: 1px;
    overflow: hidden; min-width: 80px;
    transition: background 120ms ease;
}
#syntaur-mini-player .smp-progress:hover { background: rgba(148, 163, 184, 0.4); }
#syntaur-mini-player .smp-progress-fill {
    height: 100%; width: 0%; background: rgb(56, 189, 248);
    border-radius: 2px;
    transition: width 200ms linear;
}
#syntaur-mini-player .smp-btn {
    background: transparent; border: none; cursor: pointer;
    color: rgb(156, 163, 175); padding: 4px; border-radius: 50%;
    display: inline-flex; align-items: center; justify-content: center;
    width: 24px; height: 24px; flex-shrink: 0;
}
#syntaur-mini-player .smp-btn:hover { color: rgb(229, 231, 235); background: rgba(55, 65, 81, 0.6); }
@media (max-width: 640px) {
    #syntaur-mini-player { right: 8px; bottom: 8px; max-width: calc(100vw - 16px); }
    #syntaur-mini-player .smp-meta { max-width: 160px; }
}
"#;

const GLOBAL_MINI_PLAYER_HTML: &str = r#"
<div id="syntaur-mini-player" hidden aria-label="Now playing">
  <a href="/music" class="smp-cover" title="Open music module" aria-label="Music">
    <span class="smp-art" id="smp-art" aria-hidden="true"></span>
    <svg class="smp-cover-fallback" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
  </a>
  <div class="smp-meta">
    <div class="smp-title" id="smp-title">—</div>
    <div class="smp-sub"   id="smp-sub">—</div>
    <div class="smp-progress" id="smp-progress" role="progressbar" aria-label="Track progress" aria-valuemin="0" aria-valuemax="100" aria-valuenow="0">
      <div class="smp-progress-fill" id="smp-progress-fill"></div>
    </div>
  </div>
  <button class="smp-btn" id="smp-play" onclick="syntaurMpControl('play_pause')" aria-label="Play / pause">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>
  </button>
  <button class="smp-btn" onclick="syntaurMpControl('next')" aria-label="Next track">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,4 15,12 5,20"/><rect x="16" y="4" width="2" height="16"/></svg>
  </button>
</div>
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
  // SPA-safe idempotency. The SPA router re-executes body inline
  // scripts after content swap; without this guard, each navigation
  // would double-bind every listener on the persisted global-audio
  // element + accumulate intervals. Once we've installed our wires
  // they survive across navigations, so re-runs become no-ops.
  if (window.__syntaurTopBarBound) return;
  window.__syntaurTopBarBound = true;

  // ── universal token magic-link pickup ──────────────────
  // Any page with ?token=ocp_… will seed sessionStorage and strip
  // the token from the URL bar (so it isn't bookmarked / shared).
  // Lets a URL like https://host/scheduler?token=… recover a
  // logged-out session without making the user re-login. Only
  // accepts tokens that look like our `ocp_` prefix to avoid
  // arbitrary-string mass-assignment.
  try {
    const url = new URL(window.location.href);
    const tok = url.searchParams.get('token') || '';
    if (tok && tok.startsWith('ocp_') && tok.length > 16) {
      try {
        sessionStorage.setItem('syntaur_token', tok);
        localStorage.setItem('syntaur_token', tok);
      } catch (_) {}
      url.searchParams.delete('token');
      const cleaned = url.pathname + (url.search ? url.search : '') + url.hash;
      try { history.replaceState(null, '', cleaned); } catch (_) {}
      // Promote the URL token to a durable HttpOnly cookie so a future
      // sessionStorage wipe doesn't strand the user. Fire-and-forget;
      // failure is non-fatal because storage is already seeded.
      try {
        fetch('/api/auth/refresh-cookie', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Authorization': 'Bearer ' + tok },
        }).catch(() => {});
      } catch (_) {}
    }
  } catch (_) {}

  // ── cookie backfill for already-logged-in users ────────
  // If storage has a token but the browser has no syntaur_token
  // cookie, install the cookie so a future storage wipe (deploy
  // storm, browser eviction) doesn't strand the session.
  // One-shot per page, fire-and-forget.
  try {
    const stored = (sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '');
    const hasCookie = (document.cookie || '').split(';').some(c => c.trim().startsWith('syntaur_token='));
    if (stored && stored.startsWith('ocp_') && !hasCookie) {
      fetch('/api/auth/refresh-cookie', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Authorization': 'Bearer ' + stored },
      }).catch(() => {});
    }
  } catch (_) {}

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
    { path: '/scheduler',  name: 'Scheduler', sub: 'Thaddeus' },
    { path: '/tax',        name: 'Tax',       sub: 'Positron' },
    { path: '/knowledge',  name: 'Knowledge', sub: 'Cortex' },
    { path: '/music',      name: 'Music',     sub: 'Silvr' },
    { path: '/journal',    name: 'Journal',   sub: 'Mushi' },
    { path: '/library',    name: 'Library',   sub: 'Maxine' },
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
      // Route through syntaurGo so the SPA router gets the click —
      // hard `location.href=` was killing music continuity when users
      // navigated via ⌘K. syntaurGo falls back to location.href if
      // the router isn't installed yet.
      `<li data-path="${m.path}" onclick="window.closeModulesPalette&&window.closeModulesPalette();window.syntaurGo&&window.syntaurGo('${m.path}')">`
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

  // ── Persistent music: global-audio + floating pill ────────────
  // Single source of truth is localStorage.syntaurMusic — a JSON blob
  // {trackId, title, artist, album, position, playing} that every page
  // reads at mount and every state change writes to. The <audio
  // id="global-audio"> element in the top bar auto-resumes from this
  // on page load, which means music keeps going while Sean navigates
  // Syntaur. The floating pill (#syntaur-mini-player) shows the track
  // + drives play/pause/next.
  //
  // Cloud playback (HA / Apple / Spotify) still falls back to the
  // /api/music/now_playing poll — we only override when a LOCAL
  // session is active.

  const MUSIC_KEY = 'syntaurMusic';
  function readMusic() {
    try { return JSON.parse(localStorage.getItem(MUSIC_KEY) || 'null'); } catch(_e) { return null; }
  }
  function writeMusic(s) {
    try { if (s) localStorage.setItem(MUSIC_KEY, JSON.stringify(s)); else localStorage.removeItem(MUSIC_KEY); } catch(_e) {}
  }
  function token() {
    try { return localStorage.getItem('syntaur_token') || sessionStorage.getItem('syntaur_token') || ''; } catch(_e) { return ''; }
  }

  const ga = document.getElementById('global-audio');

  // Persistence guard. ga.load() resets currentTime to 0 and emits a
  // timeupdate before our loadedmetadata seek can run. Without this
  // gate the timeupdate handler clobbers the saved position to 0,
  // which is why pill-play used to "restart from the beginning" after
  // navigation. Held true from `resumeSavedMusic` start until the
  // post-seek state has been re-written.
  let suspendPersist = false;
  // The browser will block ga.play() on a fresh document with no user
  // gesture — we don't want that block to flip localStorage's
  // `playing` to false, because the user *intends* to keep playing
  // and will click the pill in a moment. While true, the `pause`
  // listener leaves `playing` alone.
  let awaitingUserGestureToResume = false;

  // Resume saved playback on page load. The /music page hooks into
  // the SAME global-audio element via playLocalTrack — when the user
  // clicks a track there, playLocalTrack stops the resumed audio
  // cleanly before starting the new one.
  function resumeSavedMusic() {
    if (!ga) return;
    const s = readMusic();
    if (!s || !s.trackId) return;
    try {
      suspendPersist = true;
      ga.src = '/api/music/local/file/' + s.trackId + '?token=' + encodeURIComponent(token());
      ga.load();
      ga.addEventListener('loadedmetadata', function once() {
        ga.removeEventListener('loadedmetadata', once);
        if (s.position && s.position > 0 && s.position < ga.duration) {
          try { ga.currentTime = s.position; } catch(_e) {}
        }
        // Re-write the saved state at the seeked position so the
        // first post-resume timeupdate doesn't write a stale value.
        const fresh = readMusic();
        if (fresh) {
          fresh.position = ga.currentTime;
          // Keep `playing` as-is — autoplay block doesn't change intent.
          writeMusic(fresh);
        }
        suspendPersist = false;
        if (s.playing) {
          awaitingUserGestureToResume = true;
          const p = ga.play();
          if (p && typeof p.then === 'function') {
            p.then(() => { awaitingUserGestureToResume = false; })
             .catch(() => {
               // Stays true; pause listener won't flip `playing`. The
               // pill / dashboard widget will render a "Resume" affordance
               // and the next user click resolves the gesture.
             });
          }
        }
      }, { once: true });
    } catch(_e) { suspendPersist = false; }
  }

  // Persist position + playing flag on every state change.
  if (ga) {
    ga.addEventListener('timeupdate', () => {
      if (suspendPersist) return;
      const s = readMusic();
      if (!s) return;
      s.position = ga.currentTime;
      s.playing = !ga.paused;
      writeMusic(s);
      paintProgress();
    });
    ga.addEventListener('loadedmetadata', () => paintProgress());
    ga.addEventListener('seeked', () => paintProgress());
    ga.addEventListener('pause', () => {
      if (awaitingUserGestureToResume) { updatePill(); return; }
      const s = readMusic();
      if (s) { s.playing = false; writeMusic(s); }
      updatePill();
    });
    ga.addEventListener('play', () => {
      awaitingUserGestureToResume = false;
      const s = readMusic();
      if (s) { s.playing = true; writeMusic(s); }
      updatePill();
    });
    ga.addEventListener('ended', () => {
      writeMusic(null);
      updatePill();
    });
    ga.addEventListener('error', () => {
      writeMusic(null);
      updatePill();
    });
  }

  // Set MediaSession metadata so OS-level lockscreen / hardware-key
  // controls show the right track name + art on every page where
  // local audio is playing — not just /music. Idempotent: safe to
  // call repeatedly with the same values.
  function syncMediaSession(s) {
    if (!('mediaSession' in navigator) || !s || !s.trackId) return;
    try {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: s.title || ('Track ' + s.trackId),
        artist: s.artist || '',
        album: s.album || '',
        artwork: [
          { src: '/api/music/local/art/' + s.trackId, sizes: '512x512', type: 'image/jpeg' },
        ],
      });
      navigator.mediaSession.playbackState = (ga && !ga.paused) ? 'playing' : 'paused';
      navigator.mediaSession.setActionHandler('play',  () => { if (ga && ga.paused) ga.play(); });
      navigator.mediaSession.setActionHandler('pause', () => { if (ga && !ga.paused) ga.pause(); });
      navigator.mediaSession.setActionHandler('seekto', (e) => { if (ga && e.seekTime != null) ga.currentTime = e.seekTime; });
      navigator.mediaSession.setActionHandler('seekforward',  () => { if (ga) ga.currentTime = Math.min((ga.duration || 1e9), ga.currentTime + 10); });
      navigator.mediaSession.setActionHandler('seekbackward', () => { if (ga) ga.currentTime = Math.max(0, ga.currentTime - 10); });
    } catch(_e) {}
  }

  // Paint the pill's progress bar fill + aria-valuenow. Cheap; called
  // on every timeupdate. Hidden when there's no duration yet.
  function paintProgress() {
    const wrap = document.getElementById('smp-progress');
    const fill = document.getElementById('smp-progress-fill');
    if (!wrap || !fill || !ga) return;
    const dur = ga.duration;
    if (!isFinite(dur) || dur <= 0) {
      fill.style.width = '0%';
      wrap.setAttribute('aria-valuenow', '0');
      return;
    }
    const pct = Math.max(0, Math.min(100, (ga.currentTime / dur) * 100));
    fill.style.width = pct.toFixed(2) + '%';
    wrap.setAttribute('aria-valuenow', Math.round(pct).toString());
  }

  // Repaint the floating pill from either local state (preferred) or
  // a cloud now_playing poll (fallback).
  let _lastCloudEntity = null;
  async function updatePill() {
    const pill = document.getElementById('syntaur-mini-player');
    if (!pill) return;

    // Prefer local state.
    const s = readMusic();
    if (s && s.trackId) {
      pill.hidden = false;
      pill.style.opacity = '1';
      pill.dataset.source = 'local';
      const title = document.getElementById('smp-title');
      const sub = document.getElementById('smp-sub');
      if (title) title.textContent = s.title || 'Track ' + s.trackId;
      if (sub) sub.textContent = (s.artist || '') + (s.album ? ' · ' + s.album : '');
      const cover = pill.querySelector('.smp-cover');
      const art = document.getElementById('smp-art');
      if (cover && art) {
        const url = '/api/music/local/art/' + s.trackId;
        art.style.backgroundImage = "url('" + url + "')";
        cover.classList.add('has-art');
      }
      const pb = document.getElementById('smp-play');
      if (pb) {
        const playing = ga && !ga.paused;
        pb.innerHTML = playing
          ? '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>'
          : '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
      }
      paintProgress();
      syncMediaSession(s);
      return;
    }
    // Hide art slot when falling back to cloud or empty state.
    const cover = pill.querySelector('.smp-cover');
    if (cover) cover.classList.remove('has-art');
    // Cloud sources don't surface progress through the same pipeline,
    // so leave the bar at 0% (visually a hairline) until cloud paints
    // its own progress fraction below.
    const fill = document.getElementById('smp-progress-fill');
    if (fill) fill.style.width = '0%';

    // No local — try cloud.
    const tk = token();
    if (!tk) { pill.hidden = true; return; }
    try {
      const r = await fetch('/api/music/now_playing?token=' + encodeURIComponent(tk));
      if (!r.ok) { pill.hidden = true; return; }
      const d = await r.json();
      const state = d.state || 'off';
      _lastCloudEntity = d.entity_id || null;
      if (state === 'off' || !d.song) { pill.hidden = true; return; }
      pill.hidden = false;
      pill.dataset.source = 'cloud';
      pill.style.opacity = d.ducking ? '0.55' : '1';
      const title = document.getElementById('smp-title');
      const sub = document.getElementById('smp-sub');
      if (title) title.textContent = d.song;
      if (sub) sub.textContent = d.artist || d.source || '';
      const pb = document.getElementById('smp-play');
      if (pb) {
        pb.innerHTML = state === 'playing'
          ? '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>'
          : '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
      }
    } catch(_e) {}
  }

  window.syntaurMpControl = async function(action) {
    // Local path (global-audio) takes priority when a local session
    // is active. Falls through to cloud control only for cloud sessions.
    const pill = document.getElementById('syntaur-mini-player');
    if (pill && pill.dataset.source === 'local' && ga) {
      if (action === 'play_pause') {
        if (ga.paused) ga.play().catch(()=>{}); else ga.pause();
      } else if (action === 'next' || action === 'prev') {
        // Next/prev for local require a queue — surfaced on /music.
        if (action === 'next' && window.syntaurGo) window.syntaurGo('/music');
        else if (action === 'next') window.location.href = '/music';
      }
      return;
    }
    const tk = token();
    if (!tk) return;
    try {
      await fetch('/api/music/control', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: tk, action, entity_id: _lastCloudEntity }),
      });
      setTimeout(updatePill, 400);
    } catch(_e) {}
  };

  // Click-to-seek on the pill's progress bar. Cheap math: target.x
  // relative to bar width × duration. Only meaningful when local
  // session is active and ga has a known duration.
  (function bindPillSeek() {
    const wrap = document.getElementById('smp-progress');
    if (!wrap) return;
    wrap.addEventListener('click', (ev) => {
      if (!ga || !isFinite(ga.duration) || ga.duration <= 0) return;
      const rect = wrap.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, ev.clientX - rect.left));
      const frac = rect.width > 0 ? x / rect.width : 0;
      try { ga.currentTime = frac * ga.duration; } catch(_e) {}
    });
  })();

  // Bootstrap: resume any saved session + paint the pill on every
  // page load. The /music page uses the same global-audio element —
  // when the user clicks a new track there, playLocalTrack stops the
  // resumed audio cleanly before starting the new one.
  resumeSavedMusic();
  // syncMediaSession depends on resumeSavedMusic having set ga.src,
  // so it's safe to call right after — the metadata fields come from
  // localStorage (which already has the right title/artist/album).
  {
    const _s = readMusic();
    if (_s) syncMediaSession(_s);
  }
  updatePill();
  // Cloud poll stays on its old cadence for pages where local isn't active.
  setInterval(() => { if (!readMusic()) updatePill(); }, 6000);
  // Local-pill icon updates on timer so pause/play stays in sync.
  setInterval(() => { if (readMusic()) updatePill(); }, 1000);

  // ── pre-restart autosave coordinator ──────────────────────
  // Pages register autosave callbacks via window.SyntaurAutosave.register(scope, scopeKey, getValue).
  // Every 15s we poll /health and on restart_pending=true we flush every
  // registered hook to /api/drafts/save. On reconnect, modules call
  // SyntaurAutosave.restore(scope) to fetch + rehydrate.
  //
  // Why a global queue rather than per-page polling:
  //   • One /health call per tab regardless of how many composers are open
  //   • Single flush moment so every form gets the same drain window
  //   • Modules don't need to know about the gateway lifecycle — they just
  //     hand us a getter and a rehydrator
  window.SyntaurAutosave = (function() {
    const hooks = new Map(); // key -> { scope, scopeKey, get }
    let drainStarted = 0;
    let toastShown = false;

    function tokenHeader() {
      const t = (sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '');
      return t ? { 'Authorization': 'Bearer ' + t } : {};
    }

    function register(scope, scopeKey, getValue) {
      const key = scope + ':' + scopeKey;
      hooks.set(key, { scope, scopeKey, get: getValue });
      return function unregister() { hooks.delete(key); };
    }

    async function flushAll() {
      const headers = Object.assign({ 'Content-Type': 'application/json' }, tokenHeader());
      const promises = [];
      for (const [, h] of hooks) {
        let value;
        try { value = h.get(); } catch (e) { continue; }
        if (value == null) continue;
        // Skip empty strings + empty objects so we don't spam drafts table.
        if (typeof value === 'string' && !value.trim()) continue;
        if (typeof value === 'object' && Object.keys(value).length === 0) continue;
        promises.push(fetch('/api/drafts/save', {
          method: 'POST', headers,
          body: JSON.stringify({ scope: h.scope, scope_key: h.scopeKey, value }),
        }).catch(() => {}));
      }
      try { await Promise.allSettled(promises); } catch (_) {}
    }

    async function restore(scope) {
      try {
        const r = await fetch('/api/drafts/' + encodeURIComponent(scope), { headers: tokenHeader() });
        if (!r.ok) return [];
        const j = await r.json();
        return Array.isArray(j.drafts) ? j.drafts : [];
      } catch (_) { return []; }
    }

    async function discard(scope, scopeKey) {
      try {
        await fetch('/api/drafts/' + encodeURIComponent(scope) + '/' + encodeURIComponent(scopeKey), {
          method: 'DELETE', headers: tokenHeader(),
        });
      } catch (_) {}
    }

    function showRestartToast(secsAgo) {
      if (toastShown) return;
      toastShown = true;
      const t = document.createElement('div');
      t.style.cssText = 'position:fixed;bottom:16px;left:50%;transform:translateX(-50%);background:#1f2937;color:#fff;padding:10px 16px;border-radius:8px;font-size:13px;font-family:system-ui,sans-serif;box-shadow:0 4px 12px rgba(0,0,0,0.4);z-index:99999;display:flex;align-items:center;gap:10px;';
      t.innerHTML = '<span style="width:8px;height:8px;background:#fbbf24;border-radius:50%;animation:syntaur-pulse 1s infinite;"></span><span>Gateway restarting — your work is being saved…</span>';
      const style = document.createElement('style');
      style.textContent = '@keyframes syntaur-pulse { 0%,100% { opacity: 1 } 50% { opacity: 0.4 } }';
      document.head.appendChild(style);
      document.body.appendChild(t);
      setTimeout(() => { try { t.remove(); } catch (_) {} }, 12000);
    }

    let lastUptime = null;
    async function poll() {
      try {
        const r = await fetch('/health', { headers: tokenHeader() });
        if (!r.ok) return;
        const j = await r.json();
        // Detect a restart-cycle completion: uptime went down → flush
        // any drafts the page has by registering. Modules use this to
        // surface "restore your draft?" prompts on page load.
        if (lastUptime != null && j.uptime_secs < lastUptime - 5) {
          window.dispatchEvent(new CustomEvent('syntaur:gateway-restored', { detail: { uptime: j.uptime_secs } }));
        }
        lastUptime = j.uptime_secs;
        if (j.restart_pending && j.restart_pending_since && j.restart_pending_since !== drainStarted) {
          drainStarted = j.restart_pending_since;
          showRestartToast();
          // Fire-and-forget — every registered hook flushes in parallel.
          flushAll();
        }
      } catch (_) {}
    }
    poll();
    setInterval(poll, 15000);

    // Beforeunload safety net — if user closes the tab while restart is
    // pending, flush synchronously via sendBeacon (works even after the
    // page is being torn down).
    window.addEventListener('beforeunload', function() {
      if (!drainStarted) return;
      const headers = tokenHeader();
      for (const [, h] of hooks) {
        let value;
        try { value = h.get(); } catch (_) { continue; }
        if (value == null) continue;
        const blob = new Blob([JSON.stringify({ scope: h.scope, scope_key: h.scopeKey, value })], { type: 'application/json' });
        // sendBeacon ignores Authorization header, so cookie auth must
        // be active. Falls back gracefully if not.
        try { navigator.sendBeacon('/api/drafts/save', blob); } catch (_) {}
      }
    });

    return { register, restore, discard, flushAll };
  })();
})();
</script>
"##;

/// SPA-style content swap navigation. Click an internal link → fetch the
/// new HTML → replace ONLY #syntaur-app-content's innerHTML → push
/// history. The top bar (with its <audio id="global-audio">), modules
/// palette, mini-player pill, and bug-report overlay all live OUTSIDE
/// #syntaur-app-content, so they're never touched by the swap. Audio
/// keeps playing through navigation with zero gap because the audio
/// element is never detached from the DOM.
///
/// CSP note: the gateway sets `script-src 'self' 'unsafe-inline'` for
/// HTML responses (security.rs line 519-521), so cloned scripts without
/// the original page's nonce still execute. If CSP later moves to
/// strict-dynamic + nonce-only, the router needs to forward nonces.
///
/// Opt-out per link with `data-spa="no"`. Modified clicks (cmd/ctrl,
/// middle, target=_blank) fall through to native navigation. Errors of
/// any kind (404, parse failure, network) fall back to a hard redirect
/// so the user never lands on a broken half-swapped page.
const SPA_ROUTER_SCRIPT: &str = r##"
<script>
(function() {
  if (window.__syntaurSpaInstalled) return;
  window.__syntaurSpaInstalled = true;

  function shouldRoute(a, ev) {
    if (!a || ev.defaultPrevented) return false;
    if (a.dataset.spa === 'no') return false;
    if (ev.button !== 0) return false;
    if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return false;
    if (a.target && a.target !== '' && a.target !== '_self') return false;
    if (a.hasAttribute('download')) return false;
    const href = a.getAttribute('href');
    if (!href) return false;
    if (href.startsWith('#')) return false;
    // Different-origin or non-http(s) — let browser handle.
    if (/^[a-z][a-z0-9+.\-]*:/i.test(href) && !href.startsWith(location.origin)) return false;
    if (/^https?:\/\//i.test(href) && new URL(href).origin !== location.origin) return false;
    // Backend endpoints + static assets — never SPA-route.
    if (/^\/(api|tailwind|favicon|app-icon|icon-|manifest|static|assets|robots|sitemap|\.well-known)/i.test(href)) return false;
    if (/\.(jpg|jpeg|png|gif|webp|svg|ico|css|js|json|pdf|zip|woff|woff2|mp3|mp4|wav|ogg|flac|m3u|m4a)(\?|$)/i.test(href)) return false;
    return true;
  }

  let inflightCtrl = null;

  async function navigate(url, isPopState) {
    if (inflightCtrl) inflightCtrl.abort();
    inflightCtrl = new AbortController();
    try {
      const resp = await fetch(url, {
        credentials: 'same-origin',
        headers: { 'Accept': 'text/html' },
        signal: inflightCtrl.signal,
      });
      if (!resp.ok) { location.href = url; return; }
      const ct = resp.headers.get('content-type') || '';
      if (!ct.includes('text/html')) { location.href = url; return; }
      const html = await resp.text();
      const doc = new DOMParser().parseFromString(html, 'text/html');

      // ── title
      if (doc.title) document.title = doc.title;

      // ── per-page <style class="syntaur-page"> swap. Stable styles
      // (BASE / TOP_BAR) carry no marker class and survive untouched.
      document.head.querySelectorAll('style.syntaur-page').forEach(s => s.remove());
      doc.head.querySelectorAll('style.syntaur-page').forEach(s => {
        document.head.appendChild(s.cloneNode(true));
      });
      // ── per-page head_boot script (re-execute via clone)
      document.head.querySelectorAll('script.syntaur-page-boot').forEach(s => s.remove());
      doc.head.querySelectorAll('script.syntaur-page-boot').forEach(s => {
        const fresh = document.createElement('script');
        for (const a of s.attributes) fresh.setAttribute(a.name, a.value);
        fresh.textContent = s.textContent;
        document.head.appendChild(fresh);
      });

      // ── Update top-bar crumb + status from new page's data-page
      // attribute on the new content container. This lets us avoid
      // a full top-bar re-render (which would re-create #global-audio
      // and break music continuity). The top bar's #module-name span
      // gets its text content updated; the status pill's class /
      // text are updated if the new page exposes them.
      const newMain = doc.getElementById('syntaur-app-content');
      const moduleName = document.querySelector('.syntaur-topbar .module-name');
      if (moduleName && newMain && newMain.dataset.page) {
        moduleName.textContent = newMain.dataset.page;
      }
      // ── Body class (some pages opt into ambient theming)
      if (doc.body && doc.body.className) document.body.className = doc.body.className;

      // ── Replace ONLY #syntaur-app-content's innerHTML. The top bar,
      // audio element, pill, palette, and bug overlay all live outside
      // this container and are not touched.
      const liveMain = document.getElementById('syntaur-app-content');
      if (!liveMain) { location.href = url; return; }
      if (newMain) {
        liveMain.innerHTML = newMain.innerHTML;
        liveMain.dataset.page = newMain.dataset.page || '';
      } else {
        // New page didn't render the expected wrapper — fall back to a
        // hard navigation rather than guessing.
        location.href = url;
        return;
      }

      // ── Force a synchronous layout pass after the innerHTML swap.
      // WebKitGTK's layout pipeline doesn't always re-establish the
      // height-context chain when the new content's root layout uses
      // `height: 100vh` (coders' workshop-root), `height: 100%`, or
      // `flex: 1` against an ancestor whose previous layout was
      // computed under a different content tree. Reading offsetHeight
      // is the canonical browser-portable trick to trigger an immediate
      // reflow. Chromium ignores it; WebKit uses it.
      void liveMain.offsetHeight;
      void document.body.offsetHeight;

      // ── Re-execute the inline scripts inside the new container so
      // each module's IIFE fires. innerHTML doesn't auto-run scripts.
      //
      // Important: this is best-effort. Most page scripts have top-level
      // `let`/`const`/`function` declarations; the first execution puts
      // those bindings in the global lexical environment, and a second
      // execution throws `SyntaxError: Identifier 'X' has already been
      // declared`, aborting the whole script body. Pages that need to
      // re-bind to the freshly-swapped DOM should register a
      // `syntaur:page-arrived` listener on first load — that listener
      // survives the redeclaration abort and gets fired below regardless
      // of whether scripts re-executed cleanly.
      window.__syntaurVisitedPages = window.__syntaurVisitedPages || new Set();
      const pageKey = (newMain && newMain.dataset.page) || url;
      const firstVisit = !window.__syntaurVisitedPages.has(pageKey);
      if (pageKey) window.__syntaurVisitedPages.add(pageKey);
      const oldScripts = Array.from(liveMain.querySelectorAll('script'));
      for (const old of oldScripts) {
        const fresh = document.createElement('script');
        for (const a of old.attributes) fresh.setAttribute(a.name, a.value);
        fresh.textContent = old.textContent;
        if (old.parentNode) old.parentNode.replaceChild(fresh, old);
      }

      // ── One more reflow after scripts run, then on next two
      // animation frames — covers any IIFE that mutates DOM/CSS in
      // ways that the engine should re-layout again.
      void liveMain.offsetHeight;
      requestAnimationFrame(() => {
        requestAnimationFrame(() => { void liveMain.offsetHeight; });
      });

      // ── History
      if (!isPopState) history.pushState({ syntaurSpa: true }, '', url);
      window.scrollTo(0, 0);

      // ── Subscribers
      // syntaur:navigated — fires every navigation (legacy, page-agnostic)
      // syntaur:page-arrived — fires every arrival including revisits;
      //   detail.firstVisit tells the listener whether scripts just ran
      //   or were skipped (so the listener knows whether it needs to
      //   trigger re-bind / re-fetch itself).
      window.dispatchEvent(new CustomEvent('syntaur:navigated', { detail: { url } }));
      window.dispatchEvent(new CustomEvent('syntaur:page-arrived', { detail: { page: pageKey, url, firstVisit } }));
    } catch (e) {
      if (e && e.name === 'AbortError') return;
      // Anything else — fall back to a real navigation. Better to
      // visibly reload than to leave a half-swapped page.
      location.href = url;
    } finally {
      inflightCtrl = null;
    }
  }

  document.addEventListener('click', (ev) => {
    const a = ev.target && ev.target.closest && ev.target.closest('a[href]');
    if (!shouldRoute(a, ev)) return;
    let url;
    try { url = new URL(a.getAttribute('href'), location.href).toString(); }
    catch (_) { return; }
    if (new URL(url).origin !== location.origin) return;
    ev.preventDefault();
    navigate(url, false);
  });

  window.addEventListener('popstate', () => {
    navigate(location.href, true);
  });

  // Programmatic-navigation helper. Anywhere we'd otherwise write
  // `location.href = '/some/internal/path'`, use syntaurGo() instead
  // so the navigation goes through the SPA router and music keeps
  // playing. Same-origin only — external URLs and downloads fall
  // through to a real navigation. Falls back to location.href if
  // the router was somehow not installed.
  window.syntaurGo = function(href) {
    if (!href) return;
    let url;
    try { url = new URL(href, location.href).toString(); }
    catch (_) { location.href = href; return; }
    if (new URL(url).origin !== location.origin) {
      // Different origin (e.g., OAuth redirect) — hard nav.
      location.href = href;
      return;
    }
    navigate(url, false);
  };
})();
</script>
"##;

/// Bug-report overlay + submit flow. Placed by JS into `.syntaur-topbar` as a
/// small icon button just before the avatar. Pure vanilla JS.
const BUG_REPORT_JS: &str = r#"
(function() {
  // SPA-safe: once installed, don't re-append the overlay or re-bind
  // listeners on subsequent body swaps. The overlay is in PERSIST_IDS
  // so it survives navigation; rerunning would create a duplicate.
  if (window.__syntaurBugReportBound) return;
  window.__syntaurBugReportBound = true;
  // Button is rendered server-side in top_bar(). This script owns the
  // modal + submit. Token lookup happens lazily on open/submit so the
  // modal installs even before the login flow has written to storage.
  function currentToken() {
    try { return sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || ''; } catch(_e) { return ''; }
  }
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
      const res = await fetch('/api/bug-reports', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({ token: currentToken(), description: desc, system_info: si, page_url: location.href }) });
      const data = await res.json();
      if (data.id) { fb.className='text-sm text-green-400'; fb.textContent='Bug report #'+data.id+' submitted. Thank you!'; fb.classList.remove('hidden'); btn.textContent='Submitted'; setTimeout(function(){ document.getElementById('bug-report-overlay').classList.add('hidden'); }, 2000); }
      else { fb.className='text-sm text-red-400'; fb.textContent=data.error||'Submission failed.'; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
    } catch(e) { fb.className='text-sm text-red-400'; fb.textContent='Network error: '+e.message; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
  };
})();
"#;
