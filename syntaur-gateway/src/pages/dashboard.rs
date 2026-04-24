//! `/dashboard` — customizable widget grid with ambient theming.
//!
//! Replaces the former tri-pane command-center layout. The dashboard is
//! now a simple grid host: widgets come from the `dashboard_widgets`
//! registry, each render scales by size preset (S 2×2 / M 4×2 / L 4×4 /
//! XL 8×4), and the user's layout persists in `dashboard_layout` as
//! JSON. View mode is locked and clean; clicking the pencil circle
//! bottom-right enters edit mode — drag to reorder, cycle the size
//! chip, remove, or add from the drawer.
//!
//! Per-module themes (Music cyberpunk, Knowledge parchment, Coders CRT)
//! are untouched — this ambient theme only applies to `body.syntaur-ambient`,
//! which only the dashboard + Settings → Appearance preview turn on.
//!
//! Previous tri-pane implementation lives in git history at commit
//! `cb62df8`; pull it from there if anything is worth salvaging.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::dashboard_widgets::{registry, WidgetContext, WidgetSize};
use super::shared::{shell, top_bar, Page};
use super::theme::{THEME_SCRIPT, THEME_STYLE};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Dashboard",
        authed: true,
        extra_style: None,
        // Paint `syntaur-ambient` from first paint so the body is
        // already using the theme tokens when the browser paints the
        // page. Prevents the dark-slate→white flash that used to happen
        // while theme.rs was still loading.
        body_class: Some("syntaur-ambient min-h-screen"),
        head_boot: Some(DASHBOARD_HEAD_BOOT),
    };
    let body = html! {
        (top_bar("Dashboard", None))
        style { (PreEscaped(THEME_STYLE)) }
        style { (PreEscaped(DASHBOARD_STYLE)) }
        // Auth-fetch helper has to exist before THEME_SCRIPT so
        // /api/appearance can also authenticate; widgets re-use the
        // same helper from their inline `<script>` tags at render time.
        script { (PreEscaped(SD_FETCH_HELPER)) }
        // Ribbon backdrop — full-bleed SVG with 12 hand-authored
        // bezier filaments. Replaces the old flat body::before motes
        // layer. Gated on html.rb-on (set in head-boot) so cards can
        // switch to translucent without flashing.
        (PreEscaped(RIBBON_SVG))
        main class="sd-root" id="sd-root" {
            (greeting_strip())
            (sun_indicator())
            (customize_bar())
            div class="sd-grid" id="sd-grid" data-mode="view" {}
        }
        (drawer())
        script { (PreEscaped(THEME_SCRIPT)) }
        script { (PreEscaped(widget_templates_js())) }
        script { (PreEscaped(DASHBOARD_SCRIPT)) }
        script { (PreEscaped(RIBBON_SCRIPT)) }
    };
    Html(shell(page, body).into_string())
}

// ─── Pre-body-paint boot script ────────────────────────────────────────
//
// Runs synchronously in `<head>` before the browser paints the body.
// Reads the cached appearance pref from localStorage and flips
// `html.theme-light` on if the user has explicitly chosen light — so
// the first paint lands in the correct palette instead of flashing
// from dark to light once theme.rs finishes loading.
//
// Dark is the default. We only add `theme-light` if:
//   - theme_mode === 'light', or
//   - theme_mode === 'schedule' and the current clock is inside the
//     user's light window, or
//   - theme_mode === 'auto' AND the user has set latitude/longitude
//     AND the current time is between sunrise and sunset.
const DASHBOARD_HEAD_BOOT: &str = r##"
(function() {
  try {
    var raw = localStorage.getItem('syntaur:appearance');
    if (!raw) return;
    var p = JSON.parse(raw);
    if (!p || !p.theme_mode) return;
    var now = new Date();
    var minNow = now.getHours() * 60 + now.getMinutes();
    var isLight = false;
    if (p.theme_mode === 'light') {
      isLight = true;
    } else if (p.theme_mode === 'schedule') {
      var rs = p.light_start_min || 420, dk = p.dark_start_min || 1140;
      isLight = minNow >= rs && minNow < dk;
    } else if (p.theme_mode === 'auto' && p.latitude != null && p.longitude != null) {
      // Rough: between 7am and 7pm → light. The full NOAA calc runs
      // post-paint in theme.rs; this is just for first-paint accuracy.
      isLight = minNow >= 420 && minNow < 1140;
    }
    if (isLight) document.documentElement.classList.add('theme-light');
    if (p.accent && ({sage:135,indigo:265,ochre:70,gray:260})[p.accent] != null) {
      document.documentElement.style.setProperty('--accent-h',
        String(({sage:135,indigo:265,ochre:70,gray:260})[p.accent]));
    }
    // Pre-paint time-of-day bucket so the gradient bg lands on first
    // frame instead of flashing midday dark then fading to morning cream.
    // theme.rs apply() re-runs this with the full NOAA calc post-paint.
    var rs = p.light_start_min || 420, dk = p.dark_start_min || 1140;
    var tod;
    if (isLight) {
      tod = 'morning';
    } else {
      var preDusk = minNow >= (dk - 120) && minNow < dk;
      var overnight = minNow < rs || minNow >= dk;
      tod = (preDusk || overnight) ? 'evening' : 'midday';
    }
    // Body doesn't exist yet at head-script time; set on <html> as a
    // data attribute, and syntaur-ambient.theme-ready in theme.rs will
    // promote it to body class. Meanwhile add the class pre-emptively
    // when body parses — use a tiny MutationObserver fallback.
    document.documentElement.setAttribute('data-tod', tod);
    var setTod = function() {
      if (!document.body) return false;
      document.body.classList.remove('tod-morning','tod-midday','tod-evening');
      document.body.classList.add('tod-' + tod);
      return true;
    };
    if (!setTod()) {
      var obs = new MutationObserver(function() { if (setTod()) obs.disconnect(); });
      obs.observe(document.documentElement, { childList: true });
    }
  } catch (e) { /* first-paint best-effort; theme.rs will re-apply */ }
  // Enable ribbon-backdrop mode pre-paint so card translucency and
  // motes suppression land on the first frame.
  document.documentElement.classList.add('rb-on');
})();
"##;

fn customize_bar() -> Markup {
    html! {
        // In-view Customize control — replaces the unlabeled dual FABs.
        // Single click flips the grid into edit mode (tiles grow resize
        // chips, drag to reorder, × to remove), and exposes inline
        // "+ Add widget", "Reset", and "Done ✓" buttons. Always
        // discoverable in the top-right of the main grid area.
        section class="sd-customize" id="sd-customize" {
            div class="sd-customize-left" {
                button class="sd-focus-chip" id="sd-focus-chip" data-focus="normal" aria-label="Focus mode" title="Cycle focus: Normal → Focus → Quiet" {
                    svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" {
                        circle cx="12" cy="12" r="8" {}
                        circle cx="12" cy="12" r="3" {}
                    }
                    span class="sd-focus-chip-label" { "Normal" }
                }
            }
            div class="sd-customize-right" {
                // Edit-mode pill cluster — hidden by default, shown when in edit mode.
                div class="sd-edit-pill" id="sd-edit-pill" hidden {
                    button class="sd-edit-act" id="sd-add-widget" { "+ Add widget" }
                    button class="sd-edit-act" id="sd-reset-layout" { "Reset" }
                    button class="sd-edit-act sd-edit-act-done" id="sd-edit-done" { "Done ✓" }
                }
                button class="sd-customize-btn" id="sd-customize-btn" aria-label="Customize dashboard" {
                    svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" {
                        path d="M12 20h9" {}
                        path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5Z" {}
                    }
                    span { "Customize" }
                }
            }
        }
    }
}

// ─── Greeting + sun-position strip ─────────────────────────────────────
//
// Both render as placeholders server-side; JS fills them in with the
// current user name, time-phased greeting, live clock, and sunrise /
// sunset derived from the user's stored lat/lon (falls back gracefully
// to a generic greeting if location unset).

fn greeting_strip() -> Markup {
    html! {
        section class="sd-greeting" id="sd-greeting" {
            div class="sd-greeting-left" {
                h1 class="sd-greeting-hello" id="sd-greeting-hello" { "Hello" }
                div class="sd-greeting-sub" id="sd-greeting-sub" { "Welcome back" }
            }
            div class="sd-greeting-right" {
                div class="sd-clock" id="sd-clock" { "--:--" }
                div class="sd-clock-date" id="sd-clock-date" { "" }
            }
        }
    }
}

fn sun_indicator() -> Markup {
    html! {
        section class="sd-sun" id="sd-sun" aria-hidden="true" {
            div class="sd-sun-track" {
                div class="sd-sun-dot" id="sd-sun-dot" {}
                div class="sd-sun-tick sd-sun-tick-rise" id="sd-sun-tick-rise" {}
                div class="sd-sun-tick sd-sun-tick-set"  id="sd-sun-tick-set"  {}
            }
            div class="sd-sun-labels" {
                span class="sd-sun-label-rise" id="sd-sun-label-rise" { "—" }
                span class="sd-sun-label-now"  id="sd-sun-label-now"  { "" }
                span class="sd-sun-label-set"  id="sd-sun-label-set"  { "—" }
            }
        }
    }
}

// ─── Widget drawer (Add widget…) ───────────────────────────────────────

fn drawer() -> Markup {
    let catalog = registry();
    html! {
        aside class="sd-drawer" id="sd-drawer" aria-hidden="true" {
            div class="sd-drawer-head" {
                span class="sd-drawer-title" { "Add widget" }
                button class="sd-drawer-close" id="sd-drawer-close" aria-label="Close" { "×" }
            }
            div class="sd-drawer-hint" { "Click or drag onto the dashboard to add." }
            div class="sd-drawer-body" {
                @for w in &catalog {
                    button class="sd-drawer-item" data-kind=(w.kind()) draggable="true" {
                        div class="sd-drawer-item-title" { (w.title()) }
                        div class="sd-drawer-item-desc" { (w.description()) }
                    }
                }
            }
        }
    }
}

// (The old floating-circle FAB stack was replaced by `customize_bar()`
// — a single, labeled "Customize" button inline at the top of the grid.
// Keeps everything discoverable without the two-mystery-circles UX.)

// ─── Widget templates (rendered server-side as hidden <template>s) ─────
//
// Each widget produces four `<template>` elements — one per size. The
// client-side grid clones the right template when adding or resizing a
// widget instance, so adding a widget is zero-round-trip.

fn widget_templates_js() -> String {
    let mut out = String::from("window.__SYNTAUR_WIDGETS = {\n");
    for w in registry() {
        let kind = w.kind();
        out.push_str(&format!("  '{}': {{\n", kind));
        out.push_str(&format!("    kind: '{}',\n", kind));
        out.push_str(&format!("    title: '{}',\n", w.title().replace('\'', "\\'")));
        let (dw, dh) = w.default_size();
        let (mw, mh) = w.min_size();
        let (xw, xh) = w.max_size();
        out.push_str(&format!("    defaultSize: [{},{}],\n", dw, dh));
        out.push_str(&format!("    minSize: [{},{}],\n", mw, mh));
        out.push_str(&format!("    maxSize: [{},{}],\n", xw, xh));
        out.push_str("    templates: {\n");
        for (label, size) in &[("S", WidgetSize::S), ("M", WidgetSize::M), ("L", WidgetSize::L), ("XL", WidgetSize::Xl)] {
            let ctx = WidgetContext { instance_id: String::from("__SD_ID__") };
            let markup: Markup = w.render(*size, &ctx);
            let html_str = markup.into_string()
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${")
                // A literal `</script>` anywhere in a widget template —
                // e.g. the close of its inline helper script — terminates
                // the OUTER `<script>window.__SYNTAUR_WIDGETS = {…}</script>`
                // block the moment the browser parses it, even though it's
                // inside a JS template literal. Breaking the token with
                // `<\/script>` keeps the JS string intact while the HTML
                // parser sees a non-closing tag. Ran into this with the
                // Chat/Todo/Calendar widgets that embed per-instance JS.
                .replace("</script>", "<\\/script>");
            out.push_str(&format!("      '{}': `{}`,\n", label, html_str));
        }
        out.push_str("    },\n");
        out.push_str("  },\n");
    }
    out.push_str("};\n");
    out
}

// ─── Styles ─────────────────────────────────────────────────────────────

const DASHBOARD_STYLE: &str = r##"
/* Syntaur ambient dashboard — calm, customizable, adaptive.
   Inherits --bg, --fg, --accent etc. from theme.rs tokens. */

/* Ribbon-backdrop gradient — via a fixed full-viewport pseudo-element
   so it can't lose to html/body canvas propagation quirks or any
   author rule. z-index -2 so it sits behind the SVG (z-index -1)
   and behind content (z-index 0). Vars update with tod-* class on
   body (they inherit through <body> to its ::before pseudo). */
html.rb-on body.syntaur-ambient::after {
  content: "";
  position: fixed;
  inset: 0;
  z-index: -2;
  pointer-events: none;
  background:
    radial-gradient(ellipse 55% 40% at 80% 10%, var(--bg-vignette), transparent 65%),
    radial-gradient(ellipse 70% 55% at 15% 95%, var(--bg-vignette2), transparent 65%),
    linear-gradient(162deg, var(--bg-top) 0%, var(--bg-bot) 100%);
}
/* Kill syntaur-ambient's flat var(--bg) so the pseudo-element shows. */
html.rb-on body.syntaur-ambient {
  background: transparent !important;
}

.sd-root {
  max-width: 1400px;
  margin: 0 auto;
  padding: 32px 24px 120px;
  min-height: calc(100vh - 48px);
  background: transparent;
}
body.sd-focus .sd-root { max-width: 760px; padding-top: 48px; }

/* ── Greeting strip ─────────────────────────────────────────────── */
.sd-greeting {
  display: flex; align-items: baseline; justify-content: space-between;
  padding: 0 4px 16px; gap: 16px;
  opacity: 0; animation: sdFadeRise 600ms ease-out 0ms forwards;
}
.sd-greeting-hello {
  font-size: 28px; font-weight: 300; color: var(--fg); letter-spacing: -0.01em;
  margin: 0; line-height: 1.2;
}
.sd-greeting-hello .sd-greeting-name { font-weight: 500; color: var(--accent); }
.sd-greeting-sub { color: var(--fg-mute); font-size: 14px; margin-top: 2px; }
.sd-clock {
  font-size: 22px; font-weight: 400; color: var(--fg-dim);
  font-variant-numeric: tabular-nums; letter-spacing: 0.02em;
}
.sd-clock-date { color: var(--fg-mute); font-size: 12px; margin-top: 2px; text-align: right; letter-spacing: 0.06em; text-transform: uppercase; }

/* ── Sun-position indicator ─────────────────────────────────────── */
.sd-sun {
  margin-bottom: 28px;
  opacity: 0; animation: sdFadeRise 600ms ease-out 120ms forwards;
}
.sd-sun-track {
  position: relative; height: 4px; border-radius: 2px;
  background: linear-gradient(
    to right,
    oklch(calc(var(--bg-l) + 0.04) 0.012 calc(var(--accent-h) - 20)),
    oklch(0.72 calc(var(--accent-c) + 0.02) var(--accent-h)),
    oklch(calc(var(--bg-l) + 0.04) 0.012 calc(var(--accent-h) + 20))
  );
}
.sd-sun-dot {
  position: absolute; top: 50%;
  width: 12px; height: 12px; border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in oklab, var(--accent) 20%, transparent),
              0 2px 8px color-mix(in oklab, var(--accent) 35%, transparent);
  transform: translate(-50%, -50%);
  left: 0%;
  transition: left 800ms ease-out, background 800ms ease-out;
}
.sd-sun-tick {
  position: absolute; top: -3px; width: 2px; height: 10px;
  background: var(--fg-mute); opacity: 0.5; border-radius: 1px;
  transform: translateX(-50%);
}
.sd-sun-labels {
  display: flex; justify-content: space-between; margin-top: 10px;
  font-size: 11px; color: var(--fg-mute); letter-spacing: 0.08em; text-transform: uppercase;
  font-variant-numeric: tabular-nums;
}
.sd-sun-label-now { color: var(--fg-dim); font-weight: 500; }

@keyframes sdFadeRise {
  from { opacity: 0; transform: translateY(8px); }
  to   { opacity: 1; transform: translateY(0); }
}

.sd-grid {
  display: grid;
  grid-template-columns: repeat(8, 1fr);
  /* minmax so tiles never go below 72px (aesthetic minimum for the
     small tiles), but grow to fit content when a widget renders more
     than the nominal size-s/m/l span expected. Fixes clipped labels
     in Quick Actions, Calendar, etc. when grid gap + padding eat the
     budget. Matches the concept's `minmax(88px, auto)` pattern. */
  grid-auto-rows: minmax(72px, auto);
  gap: 16px;
}
@media (max-width: 900px) {
  .sd-grid { grid-template-columns: repeat(4, 1fr); }
}

.sd-tile {
  position: relative;
  background: var(--bg-card);
  border: 1px solid var(--line);
  border-radius: var(--radius-card);
  box-shadow: var(--shadow-soft);
  overflow: hidden;
  display: flex;
  flex-direction: column;
  transition: transform 250ms ease-out, box-shadow 250ms ease-out, border-color 200ms ease-out;
  opacity: 0;
  animation: sdFadeRise 500ms ease-out forwards;
  animation-delay: calc(var(--sd-idx, 0) * 80ms + 200ms);
}
.sd-grid[data-mode="view"] .sd-tile:hover {
  transform: translateY(-2px);
  box-shadow: 0 2px 4px rgb(0 0 0 / 0.14), 0 14px 40px rgb(0 0 0 / 0.14);
  border-color: var(--accent-line);
}
html.theme-light .sd-grid[data-mode="view"] .sd-tile:hover {
  box-shadow: 0 2px 4px rgb(0 0 0 / 0.06), 0 14px 40px rgb(0 0 0 / 0.08);
}

/* Skeleton shimmer — shown while a tile's first data fetch is in flight.
   Widget JS removes .loading once it renders.
   CRITICAL: pointer-events: none. The shimmer is purely decorative and
   MUST NOT intercept clicks. Without this, widgets with no data-slots
   (Quick Actions — fully server-rendered) sit with `.loading` for the
   full 4s fallback timeout, and any anchor underneath the pseudo is
   click-dead during that window. Bug found by puppeteer audit:
   elementFromPoint on Quick-Action top-row tiles returned .sd-card-body
   because this ::after was on top. */
.sd-tile.loading .sd-card-body::after {
  content: ""; position: absolute; inset: 44px 20px 20px;
  border-radius: 10px;
  background: linear-gradient(
    90deg,
    var(--bg-hover) 0%,
    var(--bg-elev) 50%,
    var(--bg-hover) 100%
  );
  background-size: 200% 100%;
  animation: sdShimmer 1.6s ease-in-out infinite;
  opacity: 0.6;
  pointer-events: none;
}
@keyframes sdShimmer {
  0%   { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
.sd-tile.errored .sd-error-banner {
  display: flex; align-items: center; gap: 8px;
  margin: 8px 20px 16px; padding: 8px 12px;
  background: color-mix(in oklab, var(--danger) 12%, transparent);
  border: 1px solid color-mix(in oklab, var(--danger) 30%, transparent);
  border-radius: 10px;
  color: var(--fg); font-size: 13px;
}
.sd-error-banner { display: none; }
.sd-error-retry {
  margin-left: auto; padding: 4px 10px; border-radius: 999px;
  background: transparent; border: 1px solid var(--line);
  color: var(--fg-dim); cursor: pointer; font-size: 12px;
}
.sd-error-retry:hover { color: var(--fg); border-color: var(--accent); }

/* Number count-up — the tween is scripted (JS rAF loop on first
   render only); this class simply opts the value into tabular nums. */
.sd-big-num, .sd-sys-value-big { font-variant-numeric: tabular-nums; }
.sd-tile.size-s  { grid-column: span 2; grid-row: span 2; }
.sd-tile.size-m  { grid-column: span 4; grid-row: span 2; }
.sd-tile.size-l  { grid-column: span 4; grid-row: span 4; }
.sd-tile.size-xl { grid-column: span 8; grid-row: span 4; }
@media (max-width: 900px) {
  .sd-tile.size-xl, .sd-tile.size-l, .sd-tile.size-m { grid-column: span 4; }
}

body.sd-quiet .sd-tile.sd-tile-system { display: none; }

/* ── Tile chrome (card + body) ──────────────────────────────────── */
.sd-card { display: flex; flex-direction: column; height: 100%; }
.sd-card-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 20px 0;
  color: var(--fg-mute);
  font-size: 12px; font-weight: 600;
  letter-spacing: 0.08em; text-transform: uppercase;
}
.sd-card-body { padding: 12px 20px 20px; flex: 1; min-height: 0; display:flex; flex-direction:column; }
.sd-card-foot { padding: 0 20px 16px; }

.sd-big-num { font-size: 40px; font-weight: 700; color: var(--fg); line-height: 1; letter-spacing: -0.02em; font-variant-numeric: tabular-nums; }
.sd-mute { color: var(--fg-mute); font-size: 13px; }
.sd-label { color: var(--fg-mute); font-size: 11px; letter-spacing: 0.1em; text-transform: uppercase; margin-bottom: 6px; font-weight: 600; }
.sd-action { color: var(--accent); font-size: 13px; font-weight: 500; text-decoration: none; }
.sd-action:hover { color: var(--fg); }

.sd-s-stack { display:flex; flex-direction:column; justify-content:center; gap:4px; height:100%; }

.sd-m-row { display: grid; grid-template-columns: auto 1fr; gap: 16px; align-items: center; flex: 1; }
.sd-m-right { min-width: 0; }
.sd-next-title { color: var(--fg); font-weight: 500; font-size: 14px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sd-next-time { color: var(--fg-dim); font-size: 13px; font-variant-numeric: tabular-nums; }

.sd-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 6px; flex:1; overflow:hidden; }
.sd-list-item { display: grid; grid-template-columns: 64px 1fr; gap: 10px; padding: 6px 0; border-bottom: 1px solid var(--line-soft); font-size: 14px; align-items: baseline; }
.sd-list-item:last-child { border-bottom: none; }
.sd-list-time { color: var(--fg-mute); font-variant-numeric: tabular-nums; font-size: 12px; }
.sd-list-title { color: var(--fg); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sd-list-empty { color: var(--fg-mute); font-size: 13px; font-style: italic; }

.sd-xl-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 24px; flex: 1; min-height: 0; }
.sd-xl-left, .sd-xl-right { display: flex; flex-direction: column; min-height: 0; }

.sd-weekbar { display:flex; gap: 4px; align-items:flex-end; height: 48px; }
.sd-weekbar-day { flex:1; background: var(--accent-soft); border-radius: 4px; min-height: 6px; }
.sd-weather { color: var(--fg); font-size: 14px; }

/* Journal / Research widgets */
.sd-journal-latest { flex: 1; display: flex; flex-direction: column; gap: 4px; }
.sd-journal-text { color: var(--fg); font-size: 14px; line-height: 1.5; }
.sd-journal-list .sd-list-title { white-space: normal; font-size: 13px; line-height: 1.4; }

/* System widget */
.sd-sys-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; flex: 1; align-content: center; }
.sd-sys-grid-l { grid-template-columns: 1fr 1fr; gap: 24px; }
.sd-sys-value { color: var(--fg); font-size: 16px; font-weight: 500; font-variant-numeric: tabular-nums; }
.sd-sys-value-big { font-size: 28px; letter-spacing: -0.01em; }

/* Chat widget */
.sd-chat { display:flex; flex-direction:column; gap:8px; flex:1; min-height:0; }
.sd-chat-chips { display:flex; gap:6px; flex-wrap:wrap; min-height:28px; }
.sd-chat-chip {
  background: transparent; border: 1px solid var(--line);
  color: var(--fg-dim); padding: 3px 10px; border-radius: 999px;
  font-size: 12px; cursor: pointer; font-family: inherit;
  transition: border-color 120ms, color 120ms, background 120ms;
}
.sd-chat-chip:hover { color: var(--fg); border-color: var(--accent); }
.sd-chat-chip.active {
  color: var(--accent-ink); background: var(--accent); border-color: transparent;
}
.sd-chat-log {
  flex: 1; min-height: 60px; overflow-y: auto;
  padding: 6px 0; display: flex; flex-direction: column; gap: 8px;
  scrollbar-width: thin;
}
.sd-chat-welcome { font-size: 14px; color: var(--fg); }
.sd-chat-msg { display: flex; flex-direction: column; gap: 2px; font-size: 14px; }
.sd-chat-msg-user { align-self: flex-end; max-width: 85%; }
.sd-chat-msg-agent { align-self: flex-start; max-width: 95%; }
.sd-chat-who { font-size: 11px; color: var(--fg-mute); text-transform: uppercase; letter-spacing: 0.08em; }
.sd-chat-body {
  padding: 8px 12px; border-radius: 12px;
  background: var(--bg-hover); color: var(--fg);
  white-space: pre-wrap; word-break: break-word; line-height: 1.4;
}
.sd-chat-msg-user .sd-chat-body { background: var(--accent-soft); }
.sd-chat-typing .sd-chat-body { opacity: 0.6; font-style: italic; }
.sd-chat-form { display: flex; gap: 6px; align-items: center; }
.sd-chat-input {
  flex: 1; padding: 8px 12px; border-radius: 10px;
  background: var(--bg-hover); border: 1px solid var(--line);
  color: var(--fg); font-family: inherit; font-size: 14px;
  outline: none; transition: border-color 150ms;
}
.sd-chat-input:focus { border-color: var(--accent); }
.sd-chat-send {
  background: var(--accent); color: var(--accent-ink);
  border: none; width: 32px; height: 32px; border-radius: 50%;
  cursor: pointer; display: flex; align-items: center; justify-content: center;
}
.sd-chat-send:hover { filter: brightness(1.05); }
.sd-chat-foot { font-size: 12px; }

/* Todo widget */
.sd-todo-list { list-style: none; padding: 0; margin: 0; flex: 1; overflow-y: auto;
  display: flex; flex-direction: column; gap: 2px; scrollbar-width: thin; }
.sd-todo-item {
  display: grid; grid-template-columns: 1fr auto; gap: 8px;
  align-items: center; padding: 4px 0;
  border-bottom: 1px solid var(--line-soft);
  font-size: 14px;
}
.sd-todo-item:last-child { border-bottom: none; }
.sd-todo-item.done .sd-todo-text { text-decoration: line-through; color: var(--fg-mute); }
.sd-todo-check { display: flex; gap: 10px; align-items: center; cursor: pointer; min-width: 0; }
.sd-todo-check input[type="checkbox"] {
  accent-color: var(--accent); cursor: pointer; flex-shrink: 0;
}
.sd-todo-text {
  color: var(--fg); min-width: 0; overflow-wrap: anywhere;
}
.sd-todo-del {
  background: transparent; border: none; color: var(--fg-mute);
  cursor: pointer; font-size: 16px; padding: 2px 8px; opacity: 0;
  transition: opacity 150ms, color 150ms;
}
.sd-todo-item:hover .sd-todo-del { opacity: 1; }
.sd-todo-del:hover { color: var(--danger); }
.sd-todo-form { display: flex; gap: 6px; margin-top: 8px; }
.sd-todo-input {
  flex: 1; padding: 6px 10px; border-radius: 8px;
  background: var(--bg-hover); border: 1px solid var(--line);
  color: var(--fg); font-family: inherit; font-size: 13px; outline: none;
  transition: border-color 150ms;
}
.sd-todo-input:focus { border-color: var(--accent); }
.sd-todo-add {
  background: var(--accent); color: var(--accent-ink); border: none;
  width: 28px; height: 28px; border-radius: 50%; cursor: pointer;
  font-size: 16px; line-height: 1;
}
.sd-todo-add:hover { filter: brightness(1.05); }

/* Calendar widget */
.sd-cal-today-num { font-size: 44px; font-weight: 600; color: var(--fg); letter-spacing: -0.02em; line-height: 1; font-variant-numeric: tabular-nums; }
.sd-cal-today-cell {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  padding: 0 8px;
  border-right: 1px solid var(--line-soft);
}
.sd-cal-today-dow {
  font-size: 10px; color: var(--fg-mute); letter-spacing: 0.12em;
  text-transform: uppercase; font-weight: 600; margin-bottom: 4px;
}
.sd-cal-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 6px; }
.sd-cal-title { font-weight: 500; color: var(--fg); font-size: 14px; }
.sd-cal-nav {
  background: transparent; border: 1px solid var(--line);
  color: var(--fg-dim); cursor: pointer;
  width: 24px; height: 24px; border-radius: 50%;
  display: flex; align-items: center; justify-content: center;
  font-size: 14px;
}
.sd-cal-nav:hover { border-color: var(--accent); color: var(--fg); }
.sd-cal-dow { display: grid; grid-template-columns: repeat(7, 1fr); gap: 2px; margin-bottom: 4px; }
.sd-cal-dow-cell { text-align: center; font-size: 10px; color: var(--fg-mute); letter-spacing: 0.08em; text-transform: uppercase; font-weight: 600; }
.sd-cal-grid { display: grid; grid-template-columns: repeat(7, 1fr); gap: 2px; flex: 1; min-height: 0; }
.sd-cal-cell {
  position: relative;
  aspect-ratio: 1 / 1; min-height: 22px;
  display: flex; align-items: center; justify-content: center;
  border-radius: 6px; color: var(--fg-dim); font-size: 12px;
  text-decoration: none; cursor: pointer;
  transition: background 120ms, color 120ms;
  font-variant-numeric: tabular-nums;
}
.sd-cal-cell:hover { background: var(--bg-hover); color: var(--fg); }
.sd-cal-cell-pad { cursor: default; opacity: 0; pointer-events: none; }
.sd-cal-today {
  background: var(--accent); color: var(--accent-ink) !important; font-weight: 600;
}
.sd-cal-today:hover { background: var(--accent); filter: brightness(1.05); }
.sd-cal-has-event::after {
  content: ""; position: absolute; bottom: 3px; left: 50%;
  width: 4px; height: 4px; border-radius: 50%;
  background: var(--accent); transform: translateX(-50%);
}
.sd-cal-today.sd-cal-has-event::after { background: var(--accent-ink); }

/* Quick Actions widget */
.sd-qa-grid { display: grid; gap: 8px; flex: 1; align-content: start; }
.sd-qa-1 { grid-template-columns: 1fr; grid-template-rows: 1fr; }
.sd-qa-4 { grid-template-columns: repeat(2, 1fr); grid-template-rows: repeat(2, 1fr); }
.sd-qa-8 { grid-template-columns: repeat(4, 1fr); grid-template-rows: repeat(2, 1fr); }
.sd-qa-tile {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  gap: 4px; padding: 10px 8px;
  background: var(--bg-hover); border: 1px solid var(--line-soft);
  border-radius: 12px;
  color: var(--fg); text-decoration: none;
  font-size: 12px; font-weight: 500;
  transition: background 150ms, border-color 150ms, transform 150ms;
}
.sd-qa-tile:hover { background: var(--accent-soft); border-color: var(--accent-line); transform: translateY(-1px); }
.sd-qa-glyph { font-size: 18px; color: var(--accent); line-height: 1; }
.sd-qa-label { color: var(--fg); }

/* Now-playing widget */
.sd-np-row { display: grid; grid-template-columns: 48px 1fr auto; gap: 12px; align-items: center; flex:1; }
.sd-np-row-l { grid-template-columns: 96px 1fr; align-items: start; }
.sd-np-art { width: 48px; height: 48px; border-radius: 8px; background: var(--bg-hover); background-size: cover; background-position: center; flex-shrink: 0; }
.sd-np-art-l { width: 96px; height: 96px; border-radius: 10px; }
.sd-np-art-xl { width: 180px; height: 180px; border-radius: 14px; }
.sd-np-art-empty { background: linear-gradient(135deg, var(--accent-soft), var(--bg-hover)); }
.sd-np-title { color: var(--fg); font-weight: 500; font-size: 15px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sd-np-title-s { color: var(--fg); font-weight: 500; font-size: 14px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.sd-np-title-xl { font-size: 22px; font-weight: 600; }
.sd-np-ctrls { display: flex; gap: 4px; flex-shrink: 0; }
.sd-np-ctrls-l { margin-top: 12px; align-items:center; }
.sd-np-btn { background: transparent; border: 1px solid var(--line); color: var(--fg); width: 32px; height: 32px; border-radius: 50%; cursor: pointer; display:flex; align-items:center; justify-content:center; font-size: 14px; transition: border-color 150ms, background 150ms; }
.sd-np-btn:hover { border-color: var(--accent); background: var(--accent-soft); }
.sd-np-play { background: var(--accent-soft); border-color: var(--accent-line); }
.sd-np-progress { height: 3px; background: var(--line); border-radius: 2px; overflow: hidden; margin-top: 8px; }
.sd-np-progress-fill { height: 100%; background: var(--accent); transition: width 400ms linear; }

/* ── Edit-mode chrome ───────────────────────────────────────────── */
.sd-grid[data-mode="edit"] .sd-tile {
  cursor: grab;
  border-style: dashed;
  border-color: var(--accent-line);
  box-shadow: 0 0 0 1px color-mix(in oklab, var(--accent) 20%, transparent);
}
/* Subtle accent tint on hover only, so content stays legible while
   editing. The earlier 40% overlay washed out every widget's text. */
.sd-grid[data-mode="edit"] .sd-tile:hover {
  background: color-mix(in oklab, var(--accent) 4%, var(--bg-card));
}
/* NOTE: don't use `.sd-tile > * { position: relative }` here — it
   overrides the absolute positioning on .sd-tile-tools (which has
   lower selector specificity) and drops the tools to the bottom of
   the tile in normal flow. Leave children untouched. */
.sd-tile-tools {
  display: none;
  position: absolute; top: 8px; right: 8px;
  gap: 4px; z-index: 3;
}
.sd-grid[data-mode="edit"] .sd-tile-tools { display: flex; }
.sd-tool-btn {
  width: 28px; height: 28px; border-radius: 50%;
  background: var(--bg-elev); border: 1px solid var(--line);
  color: var(--fg-dim); cursor: pointer;
  display: flex; align-items: center; justify-content: center;
  font-size: 11px; font-weight: 600;
}
.sd-tool-btn:hover { color: var(--fg); border-color: var(--accent); }
.sd-size-chip { min-width: 34px; padding: 0 8px; border-radius: 14px; width: auto; }

.sd-tile.sd-dragging { opacity: 0.5; }
.sd-drop-before { box-shadow: inset 0 3px 0 var(--accent); }
.sd-drop-after  { box-shadow: inset 0 -3px 0 var(--accent); }

/* Empty state */
.sd-empty {
  grid-column: 1 / -1; grid-row: span 3;
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  padding: 48px 20px; color: var(--fg-mute); gap: 12px;
  background: var(--bg-elev); border: 1px dashed var(--line); border-radius: var(--radius-card);
}
.sd-empty-title { color: var(--fg); font-size: 17px; font-weight: 500; }

/* ── Customize bar (replaces floating FAB stack) ────────────────── */
.sd-customize {
  display: flex; justify-content: space-between; align-items: center;
  gap: 12px; margin-bottom: 16px;
  opacity: 0; animation: sdFadeRise 500ms ease-out 180ms forwards;
  /* Must sit above the drawer (z-index: 90) so the Done button stays
     clickable when the drawer is open. Previously the drawer overlapped
     the right-aligned Done/Reset/+ Add widget pill and swallowed clicks. */
  position: relative; z-index: 95;
}
.sd-customize-left, .sd-customize-right { display: flex; gap: 8px; align-items: center; }
.sd-customize-btn, .sd-focus-chip, .sd-edit-act {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 6px 12px; border-radius: 999px;
  background: var(--bg-elev); border: 1px solid var(--line);
  color: var(--fg-dim); cursor: pointer;
  font-size: 13px; font-family: inherit; font-weight: 500;
  white-space: nowrap;
  transition: border-color 150ms, color 150ms, background 150ms;
}
.sd-customize-btn:hover, .sd-focus-chip:hover, .sd-edit-act:hover {
  color: var(--fg); border-color: var(--accent);
}
.sd-customize-btn.active { background: var(--accent); color: var(--accent-ink); border-color: transparent; }
.sd-focus-chip[data-focus="focus"] { border-color: var(--accent-line); color: var(--fg); }
.sd-focus-chip[data-focus="quiet"] { border-color: var(--accent-line); color: var(--fg); opacity: 0.7; }

.sd-edit-pill {
  display: flex; align-items: center; gap: 6px;
  padding: 2px; background: var(--bg-elev); border: 1px solid var(--accent-line);
  border-radius: 999px;
}
.sd-edit-pill[hidden] { display: none; }
.sd-edit-act { border: none; background: transparent; padding: 6px 10px; }
.sd-edit-act:hover { background: var(--bg-hover); }
.sd-edit-act-done { color: var(--accent); font-weight: 600; }

/* First-run tooltip — shown once via localStorage. */
.sd-first-run-tip {
  position: absolute; top: calc(100% + 8px); right: 0;
  background: var(--bg-card); border: 1px solid var(--accent-line);
  border-radius: 12px; padding: 10px 14px; z-index: 50;
  font-size: 13px; color: var(--fg); max-width: 260px;
  box-shadow: var(--shadow-soft);
}
.sd-first-run-tip::before {
  content: ""; position: absolute; top: -6px; right: 24px;
  width: 10px; height: 10px; background: var(--bg-card);
  border-top: 1px solid var(--accent-line); border-left: 1px solid var(--accent-line);
  transform: rotate(45deg);
}
.sd-first-run-tip .sd-tip-close {
  margin-left: 8px; color: var(--fg-mute); background: transparent;
  border: none; cursor: pointer; font-size: 14px;
}

/* ── Drawer (Add widget) ────────────────────────────────────────── */
.sd-drawer {
  position: fixed; top: 48px; right: 0; bottom: 0;
  width: 360px; z-index: 90;
  background: var(--bg-elev); border-left: 1px solid var(--line);
  transform: translateX(100%); transition: transform 300ms ease-out;
  display: flex; flex-direction: column;
}
.sd-drawer.open { transform: translateX(0); }
.sd-drawer-head { display: flex; align-items: center; justify-content: space-between; padding: 20px 24px; border-bottom: 1px solid var(--line); }
.sd-drawer-title { font-weight: 600; color: var(--fg); }
.sd-drawer-close { background: transparent; border: none; color: var(--fg-mute); cursor: pointer; font-size: 24px; line-height: 1; }
.sd-drawer-hint { padding: 0 24px 8px; color: var(--fg-mute); font-size: 12px; }
.sd-drawer-body { padding: 12px; overflow-y: auto; flex: 1; display: flex; flex-direction: column; gap: 8px; }
.sd-drawer-item { background: var(--bg-card); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; text-align: left; cursor: grab; color: inherit; font-family: inherit; transition: border-color 200ms, background 200ms, transform 100ms; }
.sd-drawer-item:hover { border-color: var(--accent); background: var(--bg-hover); }
.sd-drawer-item:active { cursor: grabbing; transform: scale(0.98); }
.sd-drawer-item.dragging { opacity: 0.6; }
.sd-drawer-item-title { color: var(--fg); font-weight: 500; margin-bottom: 4px; }
.sd-drawer-item-desc { color: var(--fg-mute); font-size: 13px; line-height: 1.4; }

/* Highlight the grid when a drawer item is being dragged over it. */
.sd-grid.sd-drop-target {
  outline: 2px dashed var(--accent); outline-offset: 4px; border-radius: 16px;
  background: color-mix(in oklab, var(--accent) 6%, transparent);
}
"##;

// ─── Client-side grid logic ────────────────────────────────────────────

// Auth-fetch helper shared by the theme script and every widget. Runs
// before anything else on the dashboard so `window.sdFetch` is
// available to the full stack, not just the widget inline scripts.
// Without this, /api/* returns 401 because no Bearer token is sent.
const SD_FETCH_HELPER: &str = r##"
(function() {
  function getToken() {
    try {
      return sessionStorage.getItem('syntaur_token')
        || localStorage.getItem('syntaur_token')
        || '';
    } catch (_e) { return ''; }
  }
  // Short-circuit /api/* requests when we have no auth token. Firing
  // the request causes Chromium to emit "Failed to load resource: 401"
  // to the browser console even when caller handles it — that's a UA
  // network-panel log, not suppressible from JS. Widgets see the same
  // 401 response shape and render their empty / signed-out state, just
  // without the network round-trip + console noise. Non-API fetches
  // still go through as-is.
  const unauthed401 = () => Promise.resolve(new Response(
    JSON.stringify({ error: 'unauthenticated' }),
    { status: 401, headers: { 'content-type': 'application/json' } }
  ));
  window.sdFetch = function(url, opts) {
    opts = opts || {};
    opts.credentials = opts.credentials || 'same-origin';
    const h = new Headers(opts.headers || {});
    const tk = getToken();
    if (tk && !h.has('authorization')) h.set('Authorization', 'Bearer ' + tk);
    opts.headers = h;
    // Skip the network hit on unauth /api/* — synthesize a 401.
    if (!tk && typeof url === 'string' && url.startsWith('/api/')) {
      return unauthed401();
    }
    return fetch(url, opts);
  };
  window.sdToken = getToken;
})();
"##;

const DASHBOARD_SCRIPT: &str = r##"
(function() {
  const SIZES = ['S','M','L','XL'];
  const SIZE_CELLS = { S: [2,2], M: [4,2], L: [4,4], XL: [8,4] };
  const grid = document.getElementById('sd-grid');

  // ── Greeting + live clock ─────────────────────────────────────
  function greetingFor(hour) {
    if (hour < 5)  return 'Good night';
    if (hour < 12) return 'Good morning';
    if (hour < 17) return 'Good afternoon';
    if (hour < 21) return 'Good evening';
    return 'Good night';
  }
  async function fetchMe() {
    try {
      const r = await window.sdFetch('/api/me', { credentials:'same-origin' });
      if (!r.ok) return null;
      const j = await r.json();
      return j && j.user ? j.user : null;
    } catch { return null; }
  }
  function nameFrom(user) {
    if (!user) return null;
    const raw = user.display_name || user.name || user.username || user.email || '';
    // If we only have an email, use the local part.
    const short = raw.includes('@') ? raw.split('@')[0] : raw;
    // Capitalize first letter for friendliness.
    return short ? short.charAt(0).toUpperCase() + short.slice(1) : null;
  }
  function renderClock() {
    const now = new Date();
    const h = now.getHours(), m = now.getMinutes();
    const clock = document.getElementById('sd-clock');
    if (clock) clock.textContent =
      String(h).padStart(2,'0') + ':' + String(m).padStart(2,'0');
    const date = document.getElementById('sd-clock-date');
    if (date) date.textContent = now.toLocaleDateString(undefined,
      { weekday:'short', month:'short', day:'numeric' });
  }
  async function initGreeting() {
    renderClock();
    setInterval(renderClock, 30 * 1000);
    const user = await fetchMe();
    const name = nameFrom(user);
    const hello = document.getElementById('sd-greeting-hello');
    if (hello) {
      const phrase = greetingFor(new Date().getHours());
      hello.innerHTML = name
        ? `${phrase}, <span class="sd-greeting-name">${name}</span>`
        : phrase;
    }
    const sub = document.getElementById('sd-greeting-sub');
    if (sub) {
      // Subtle one-liner that rotates by time-of-day. Calming, not
      // try-hard. Wife-judge approved.
      const h = new Date().getHours();
      const lines = h < 5  ? ['The house is asleep.', 'Quiet hours.']
                  : h < 12 ? ['Fresh start.', 'The day is wide open.']
                  : h < 17 ? ['Afternoon glow.', 'Steady as she goes.']
                  : h < 21 ? ['Winding down.', 'The evening settles in.']
                  :          ['The day is done.', 'Rest well.'];
      sub.textContent = lines[Math.floor(Math.random() * lines.length)];
    }
  }

  // ── Sun-position indicator ────────────────────────────────────
  //
  // Uses the already-computed sunrise/sunset from window.__syntaurThemeState
  // (set by theme.rs). Falls back to 7am-7pm if location isn't set.
  function fmtMin(min) {
    const h = Math.floor(min / 60) % 24, m = Math.floor(min % 60);
    const ampm = h < 12 ? 'am' : 'pm';
    const hh = ((h + 11) % 12) + 1;
    return hh + ':' + String(m).padStart(2,'0') + ampm;
  }
  function renderSun() {
    const st = window.__syntaurThemeState;
    const now = new Date();
    const minNow = now.getHours() * 60 + now.getMinutes();
    const rise = st ? st.rise : 420, set = st ? st.set : 1140;
    // Dot position: map 0..1440 → 0..100%.
    const pct = Math.max(0, Math.min(100, (minNow / 1440) * 100));
    const dot = document.getElementById('sd-sun-dot');
    if (dot) dot.style.left = pct + '%';
    const risePct = (rise / 1440) * 100;
    const setPct  = (set / 1440) * 100;
    const tickR = document.getElementById('sd-sun-tick-rise');
    const tickS = document.getElementById('sd-sun-tick-set');
    if (tickR) tickR.style.left = risePct + '%';
    if (tickS) tickS.style.left = setPct + '%';
    const lr = document.getElementById('sd-sun-label-rise');
    const ls = document.getElementById('sd-sun-label-set');
    const ln = document.getElementById('sd-sun-label-now');
    if (lr) lr.textContent = '↑ ' + fmtMin(rise);
    if (ls) ls.textContent = fmtMin(set) + ' ↓';
    if (ln) {
      const isDay = minNow >= rise && minNow < set;
      ln.textContent = isDay ? 'Daylight' : 'Nighttime';
    }
  }

  // ── Count-up tween ───────────────────────────────────────────
  //
  // Observes .sd-big-num / .sd-sys-value-big. First time each becomes a
  // real number (not '—' / '' / '0' on render), tween from 0→target.
  // Re-renders with the same value skip. Re-renders with a different
  // value set directly (no animation — calm, not flashy).
  const animatedOnce = new WeakSet();
  function tweenNumber(el, targetRaw) {
    const target = parseInt(String(targetRaw).replace(/[^0-9-]/g, ''), 10);
    if (!isFinite(target) || target === 0) { el.textContent = String(targetRaw); return; }
    const start = performance.now(), dur = 650, from = 0;
    function step(t) {
      const e = Math.min(1, (t - start) / dur);
      const eased = 1 - Math.pow(1 - e, 3);
      const v = Math.round(from + (target - from) * eased);
      el.textContent = String(v);
      if (e < 1) requestAnimationFrame(step);
      else el.textContent = String(targetRaw);
    }
    requestAnimationFrame(step);
  }
  function wireCountUp(root) {
    const selector = '.sd-big-num, .sd-sys-value-big';
    const mo = new MutationObserver(muts => {
      for (const mut of muts) {
        const el = mut.target.nodeType === 1 ? mut.target : mut.target.parentElement;
        if (!el || !el.matches?.(selector)) continue;
        if (animatedOnce.has(el)) continue;
        const text = el.textContent.trim();
        if (!text || text === '—' || text === '-') continue;
        animatedOnce.add(el);
        tweenNumber(el, text);
      }
    });
    mo.observe(root, { subtree: true, childList: true, characterData: true });
  }

  // ── Skeleton removal ─────────────────────────────────────────
  //
  // Every tile renders with `.loading`; we strip it after a short grace
  // window so the first real fetch can resolve, and strip it
  // immediately if any [data-slot] inside receives real text.
  function wireSkeleton(tile) {
    let done = false;
    const clear = () => { if (done) return; done = true; tile.classList.remove('loading'); };
    // Clear on first real text in any data-slot.
    const mo = new MutationObserver(() => {
      const slots = tile.querySelectorAll('[data-slot]');
      for (const s of slots) {
        const t = s.textContent.trim();
        if (t && t !== '—' && t !== '-' && !t.toLowerCase().startsWith('loading')) { clear(); mo.disconnect(); return; }
      }
    });
    mo.observe(tile, { subtree: true, childList: true, characterData: true });
    // Hard timeout so a slow / broken endpoint doesn't leave tiles
    // shimmering forever — after 4 s we drop the skeleton and show
    // whatever template was rendered (empty states include helpful copy).
    setTimeout(clear, 4000);
  }

  const drawer = document.getElementById('sd-drawer');
  const drawerClose = document.getElementById('sd-drawer-close');
  const customizeBtn = document.getElementById('sd-customize-btn');
  const editPill = document.getElementById('sd-edit-pill');
  const focusChip = document.getElementById('sd-focus-chip');
  const addBtn = document.getElementById('sd-add-widget');
  const resetBtn = document.getElementById('sd-reset-layout');
  const doneBtn = document.getElementById('sd-edit-done');

  let layout = [];  // [{id, kind, size}]
  let mode = 'view';
  let nextId = 1;

  function saveLayout() {
    window.sdFetch('/api/dashboard/layout', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ items: layout })
    }).catch(() => {});
  }

  async function loadLayout() {
    try {
      const r = await window.sdFetch('/api/dashboard/layout', { credentials: 'same-origin' });
      if (!r.ok) { layout = defaultLayout(); return; }
      const d = await r.json();
      layout = Array.isArray(d.items) && d.items.length ? d.items : defaultLayout();
    } catch { layout = defaultLayout(); }
    nextId = layout.reduce((m, it) => Math.max(m, it.id || 0), 0) + 1;
  }

  function defaultLayout() {
    // First-run layout for a brand-new user. Mirrors the server-side
    // default in dashboard_api.rs::default_layout(); keep them in sync.
    return [
      { id: 1, kind: 'chat',         size: 'L' },
      { id: 2, kind: 'todo',         size: 'M' },
      { id: 3, kind: 'calendar',     size: 'M' },
      { id: 4, kind: 'today',        size: 'M' },
      { id: 5, kind: 'quick_actions',size: 'M' },
      { id: 6, kind: 'now_playing',  size: 'M' },
    ];
  }

  function renderGrid() {
    grid.innerHTML = '';
    if (!layout.length) {
      grid.innerHTML = `
        <div class="sd-empty">
          <div class="sd-empty-title">Your dashboard is empty</div>
          <div class="sd-mute">Click <b>Customize</b> above to add widgets.</div>
          <button class="sd-customize-btn" id="sd-empty-add" style="margin-top:8px">+ Add your first widget</button>
        </div>`;
      const emptyBtn = document.getElementById('sd-empty-add');
      if (emptyBtn) emptyBtn.addEventListener('click', () => {
        setMode('edit');
        drawer.classList.add('open');
        drawer.setAttribute('aria-hidden', 'false');
      });
      return;
    }
    layout.forEach((item, idx) => {
      const def = window.__SYNTAUR_WIDGETS[item.kind];
      if (!def) return;
      const tile = document.createElement('div');
      tile.style.setProperty('--sd-idx', String(idx));
      tile.dataset.id = item.id;
      tile.dataset.idx = idx;
      tile.dataset.kind = item.kind;
      tile.dataset.size = item.size;
      tile.id = `sd-tile-${item.id}`;
      const tplHtml = (def.templates[item.size] || def.templates.M || '').replace(/__SD_ID__/g, `sd-tile-${item.id}`);
      tile.innerHTML = tplHtml;
      // Only show skeleton shimmer for widgets that actually fetch data
      // (have at least one `data-slot` populated by their inline script).
      // Static widgets like Quick Actions would otherwise sit with
      // `.loading` until the 4s fallback timer, blocking clicks
      // under the shimmer ::after pseudo-element.
      const needsSkeleton = tile.querySelector('[data-slot]') !== null;
      tile.className = `sd-tile size-${item.size.toLowerCase()}${needsSkeleton ? ' loading' : ''}`;
      // Error banner — hidden until a widget sets tile.classList.add('errored').
      const err = document.createElement('div');
      err.className = 'sd-error-banner';
      err.innerHTML = `<span class="sd-error-msg">Couldn't load</span><button class="sd-error-retry" onclick="location.reload()">Retry</button>`;
      tile.appendChild(err);
      const tools = document.createElement('div');
      tools.className = 'sd-tile-tools';
      tools.innerHTML = `
        <button class="sd-tool-btn sd-size-chip" data-act="cycle-size" title="Change size">${item.size}</button>
        <button class="sd-tool-btn" data-act="remove" title="Remove">×</button>`;
      tile.appendChild(tools);
      grid.appendChild(tile);
      if (needsSkeleton) wireSkeleton(tile);
      // Re-execute <script> blocks from the template (innerHTML skips them).
      tile.querySelectorAll('script').forEach(s => {
        const newScript = document.createElement('script');
        newScript.textContent = s.textContent;
        s.replaceWith(newScript);
      });
    });
    wireTileTools();
    if (mode === 'edit') wireDrag();
  }

  function wireTileTools() {
    grid.querySelectorAll('.sd-tile-tools').forEach(tools => {
      tools.querySelectorAll('button').forEach(btn => {
        btn.onclick = ev => {
          ev.stopPropagation();
          const tile = btn.closest('.sd-tile');
          const id = parseInt(tile.dataset.id, 10);
          const act = btn.dataset.act;
          const it = layout.find(x => x.id === id);
          if (!it) return;
          if (act === 'remove') {
            layout = layout.filter(x => x.id !== id);
          } else if (act === 'cycle-size') {
            const def = window.__SYNTAUR_WIDGETS[it.kind];
            const [minW, minH] = def.minSize, [maxW, maxH] = def.maxSize;
            const order = SIZES.filter(s => {
              const [w,h] = SIZE_CELLS[s];
              return w >= minW && h >= minH && w <= maxW && h <= maxH;
            });
            const i = order.indexOf(it.size);
            it.size = order[(i + 1) % order.length];
          }
          renderGrid(); saveLayout();
        };
      });
    });
  }

  // ─── Drag to reorder (edit mode only) ────────────────────────────
  let dragId = null;
  function wireDrag() {
    grid.querySelectorAll('.sd-tile').forEach(tile => {
      tile.draggable = true;
      tile.addEventListener('dragstart', ev => {
        dragId = parseInt(tile.dataset.id, 10);
        tile.classList.add('sd-dragging');
        ev.dataTransfer.effectAllowed = 'move';
      });
      tile.addEventListener('dragend', () => { tile.classList.remove('sd-dragging'); clearDropMarkers(); dragId = null; });
      tile.addEventListener('dragover', ev => {
        ev.preventDefault();
        if (dragId == null || parseInt(tile.dataset.id,10) === dragId) return;
        clearDropMarkers();
        const rect = tile.getBoundingClientRect();
        const before = (ev.clientX - rect.left) < rect.width / 2;
        tile.classList.add(before ? 'sd-drop-before' : 'sd-drop-after');
      });
      tile.addEventListener('dragleave', () => { tile.classList.remove('sd-drop-before','sd-drop-after'); });
      tile.addEventListener('drop', ev => {
        ev.preventDefault();
        if (dragId == null) return;
        const tgtId = parseInt(tile.dataset.id, 10);
        if (tgtId === dragId) return;
        const before = tile.classList.contains('sd-drop-before');
        const src = layout.find(x => x.id === dragId);
        layout = layout.filter(x => x.id !== dragId);
        const tgtIdx = layout.findIndex(x => x.id === tgtId);
        layout.splice(before ? tgtIdx : tgtIdx + 1, 0, src);
        clearDropMarkers();
        renderGrid(); saveLayout();
      });
    });
  }
  function clearDropMarkers() {
    grid.querySelectorAll('.sd-drop-before, .sd-drop-after').forEach(el => el.classList.remove('sd-drop-before','sd-drop-after'));
  }

  // ─── Mode toggles ───────────────────────────────────────────────
  function setMode(m) {
    mode = m;
    grid.dataset.mode = m;
    if (editPill) editPill.hidden = (m !== 'edit');
    if (customizeBtn) customizeBtn.classList.toggle('active', m === 'edit');
    if (customizeBtn) customizeBtn.querySelector('span').textContent = (m === 'edit') ? 'Editing' : 'Customize';
    renderGrid();
  }

  customizeBtn.addEventListener('click', () => {
    setMode(mode === 'edit' ? 'view' : 'edit');
    // Dismiss any first-run tip after the user clicks through once.
    try { localStorage.setItem('syntaur:dashboard:seen-customize', '1'); } catch {}
    const tip = document.getElementById('sd-first-run-tip');
    if (tip) tip.remove();
  });

  // Focus chip cycles: Normal → Focus → Quiet → Normal.
  const FOCUS_CYCLE = ['normal', 'focus', 'quiet'];
  const FOCUS_LABEL = { normal: 'Normal', focus: 'Focus', quiet: 'Quiet' };
  focusChip.addEventListener('click', () => {
    const cur = focusChip.dataset.focus || 'normal';
    const next = FOCUS_CYCLE[(FOCUS_CYCLE.indexOf(cur) + 1) % FOCUS_CYCLE.length];
    focusChip.dataset.focus = next;
    const label = focusChip.querySelector('.sd-focus-chip-label');
    if (label) label.textContent = FOCUS_LABEL[next];
    document.body.classList.toggle('sd-focus', next === 'focus');
    document.body.classList.toggle('sd-quiet', next === 'quiet');
    try { localStorage.setItem('syntaur:dashboard:focus', next); } catch {}
  });
  // Restore focus mode from last session.
  try {
    const savedFocus = localStorage.getItem('syntaur:dashboard:focus');
    if (savedFocus && FOCUS_CYCLE.indexOf(savedFocus) >= 0 && savedFocus !== 'normal') {
      focusChip.dataset.focus = savedFocus;
      const lbl = focusChip.querySelector('.sd-focus-chip-label');
      if (lbl) lbl.textContent = FOCUS_LABEL[savedFocus];
      document.body.classList.toggle('sd-focus', savedFocus === 'focus');
      document.body.classList.toggle('sd-quiet', savedFocus === 'quiet');
    }
  } catch {}

  addBtn.addEventListener('click', () => {
    drawer.classList.add('open');
    drawer.setAttribute('aria-hidden', 'false');
  });
  drawerClose.addEventListener('click', () => {
    drawer.classList.remove('open');
    drawer.setAttribute('aria-hidden', 'true');
  });
  // Add-widget helper — shared by click, dblclick, and drop.
  function addWidget(kind) {
    const def = window.__SYNTAUR_WIDGETS[kind];
    if (!def) { console.warn('[dashboard] unknown widget kind:', kind); return; }
    const [dw, dh] = def.defaultSize;
    const size = Object.keys(SIZE_CELLS).find(s => SIZE_CELLS[s][0] === dw && SIZE_CELLS[s][1] === dh) || 'M';
    layout.push({ id: nextId++, kind, size });
    drawer.classList.remove('open');
    drawer.setAttribute('aria-hidden', 'true');
    renderGrid(); saveLayout();
  }

  // Debounce add-widget calls — a double-click fires BOTH `click` and
  // `dblclick`, which otherwise added the same widget twice. 400ms is
  // long enough to absorb a normal dblclick cadence (~200-300ms) but
  // short enough that intentional repeated clicks still register.
  let lastAdd = 0;
  function addWidgetDebounced(kind) {
    const now = Date.now();
    if (now - lastAdd < 400) return;
    lastAdd = now;
    addWidget(kind);
  }

  drawer.querySelectorAll('.sd-drawer-item').forEach(btn => {
    // Click and double-click both add — Sean tried both and either should
    // work. Both go through the debouncer so a dblclick = 1 add, not 2.
    btn.addEventListener('click', () => addWidgetDebounced(btn.dataset.kind));
    btn.addEventListener('dblclick', () => addWidgetDebounced(btn.dataset.kind));
    // Drag-from-drawer to grid (the third path). The grid itself accepts
    // the drop via the handlers installed below; dataTransfer carries the
    // widget kind so the grid drop handler can call addWidget without
    // needing a closure over the source element.
    btn.addEventListener('dragstart', ev => {
      ev.dataTransfer.setData('application/x-syntaur-widget', btn.dataset.kind);
      ev.dataTransfer.effectAllowed = 'copy';
      btn.classList.add('dragging');
    });
    btn.addEventListener('dragend', () => btn.classList.remove('dragging'));
  });

  // Grid accepts a widget drop from the drawer regardless of view/edit mode —
  // dropping is itself an "add" action, so we auto-flip into view.
  grid.addEventListener('dragover', ev => {
    if (ev.dataTransfer && Array.from(ev.dataTransfer.types || []).includes('application/x-syntaur-widget')) {
      ev.preventDefault();
      ev.dataTransfer.dropEffect = 'copy';
      grid.classList.add('sd-drop-target');
    }
  });
  grid.addEventListener('dragleave', ev => {
    // Only clear when leaving the grid entirely (firing dragleave between
    // children fires this handler too; relatedTarget tells us).
    if (!ev.relatedTarget || !grid.contains(ev.relatedTarget)) grid.classList.remove('sd-drop-target');
  });
  grid.addEventListener('drop', ev => {
    const kind = ev.dataTransfer && ev.dataTransfer.getData('application/x-syntaur-widget');
    if (!kind) return;
    ev.preventDefault();
    grid.classList.remove('sd-drop-target');
    addWidget(kind);
  });
  resetBtn.addEventListener('click', () => {
    if (!confirm('Reset dashboard layout to defaults?')) return;
    layout = defaultLayout();
    nextId = layout.length + 1;
    renderGrid(); saveLayout();
  });
  doneBtn.addEventListener('click', () => setMode('view'));

  // First-run tooltip — shown once so the user knows where Customize is.
  // Auto-dismisses after 10s so it doesn't linger and obstruct widgets
  // (the prior behavior left it covering the right edge of the Chat
  // widget on mobile forever until the user found the × button).
  try {
    if (!localStorage.getItem('syntaur:dashboard:seen-customize')) {
      const tip = document.createElement('div');
      tip.className = 'sd-first-run-tip';
      tip.id = 'sd-first-run-tip';
      tip.innerHTML = 'Click <b>Customize</b> to add or rearrange widgets. <button class="sd-tip-close" aria-label="Dismiss">×</button>';
      const right = document.querySelector('.sd-customize-right');
      if (right) {
        right.style.position = 'relative';
        right.appendChild(tip);
        const dismiss = () => {
          if (!tip.isConnected) return;
          tip.style.transition = 'opacity 300ms ease';
          tip.style.opacity = '0';
          setTimeout(() => tip.remove(), 320);
          try { localStorage.setItem('syntaur:dashboard:seen-customize', '1'); } catch {}
        };
        tip.querySelector('.sd-tip-close').addEventListener('click', dismiss);
        // Auto-dismiss after 10s — enough to read, short enough not
        // to overlay widgets if the user never notices the ×.
        setTimeout(dismiss, 10000);
      }
    }
  } catch {}

  // ── Bootstrapping ────────────────────────────────────────────
  initGreeting();
  wireCountUp(grid);
  // Sun indicator: wait one tick so theme.rs has populated
  // __syntaurThemeState, then render + re-render periodically.
  setTimeout(renderSun, 300);
  setInterval(renderSun, 60 * 1000);

  loadLayout().then(renderGrid);
})();
"##;


// ─── Ribbon backdrop (inline SVG, zero JS for the visual) ────────────
//
// 12 hand-authored bezier filaments, each a <g class="rb"> with three
// stacked paths (halo wide+soft, mid, sharp bright core). Paths are
// routed to leave the top-left reading zone clear for the greeting.
// Strokes use currentColor so palette swaps (morning/midday/evening)
// flow through theme.rs CSS vars without JS.
//
// Breathing: pure CSS opacity keyframe reading --rb-base / --rb-peak
// (declared @property-ed in theme.rs so .ignite can interpolate them
// smoothly). Bloom is the 18px halo stroke at 0.15 opacity — no
// filter, no mix-blend-mode — to match the concept at
// ~/dashboard-concept/index.html.
//
// Hue cycle a/b/c/d/e/a/b/d/c/e/b/a matches the concept.
//
// viewBox is 1600x900 with xMidYMid slice so the paths drape the full
// viewport at any aspect ratio.
const RIBBON_SVG: &str = r##"
<svg class="sd-rb" aria-hidden="true" viewBox="0 0 1600 900" preserveAspectRatio="xMidYMid slice">
  <g class="rb" style="color: var(--rib-a); --rb-dur: 13s; --rb-delay: -2s;">
    <path class="halo" d="M -50 210 C 180 240, 400 220, 700 180 S 1180 60, 1400 300 S 1680 500, 1700 420"/>
    <path class="mid"  d="M -50 210 C 180 240, 400 220, 700 180 S 1180 60, 1400 300 S 1680 500, 1700 420"/>
    <path class="core" d="M -50 210 C 180 240, 400 220, 700 180 S 1180 60, 1400 300 S 1680 500, 1700 420"/>
  </g>
  <g class="rb" style="color: var(--rib-b); --rb-dur: 17s; --rb-delay: -5s;">
    <path class="halo" d="M -50 250 C 280 320, 500 140, 760 260 S 1100 420, 1360 280 S 1620 180, 1700 240"/>
    <path class="mid"  d="M -50 250 C 280 320, 500 140, 760 260 S 1100 420, 1360 280 S 1620 180, 1700 240"/>
    <path class="core" d="M -50 250 C 280 320, 500 140, 760 260 S 1100 420, 1360 280 S 1620 180, 1700 240"/>
  </g>
  <g class="rb" style="color: var(--rib-c); --rb-dur: 19s; --rb-delay: -9s;">
    <path class="halo" d="M -80 520 C 180 380, 440 620, 720 500 S 1060 340, 1320 500 S 1620 620, 1720 540"/>
    <path class="mid"  d="M -80 520 C 180 380, 440 620, 720 500 S 1060 340, 1320 500 S 1620 620, 1720 540"/>
    <path class="core" d="M -80 520 C 180 380, 440 620, 720 500 S 1060 340, 1320 500 S 1620 620, 1720 540"/>
  </g>
  <g class="rb" style="color: var(--rib-d); --rb-dur: 15s; --rb-delay: -3s;">
    <path class="halo" d="M -60 720 C 240 820, 460 560, 760 700 S 1140 860, 1400 680 S 1680 580, 1720 640"/>
    <path class="mid"  d="M -60 720 C 240 820, 460 560, 760 700 S 1140 860, 1400 680 S 1680 580, 1720 640"/>
    <path class="core" d="M -60 720 C 240 820, 460 560, 760 700 S 1140 860, 1400 680 S 1680 580, 1720 640"/>
  </g>
  <g class="rb" style="color: var(--rib-e); --rb-dur: 21s; --rb-delay: -11s;">
    <path class="halo" d="M -60 25 C 260 8, 520 12, 760 40 S 1140 280, 1420 160 S 1660 80, 1700 140"/>
    <path class="mid"  d="M -60 25 C 260 8, 520 12, 760 40 S 1140 280, 1420 160 S 1660 80, 1700 140"/>
    <path class="core" d="M -60 25 C 260 8, 520 12, 760 40 S 1140 280, 1420 160 S 1660 80, 1700 140"/>
  </g>
  <g class="rb" style="color: var(--rib-a); --rb-dur: 23s; --rb-delay: -14s;">
    <path class="halo" d="M -80 640 C 200 700, 420 500, 700 620 S 1040 780, 1300 620 S 1620 520, 1720 600"/>
    <path class="mid"  d="M -80 640 C 200 700, 420 500, 700 620 S 1040 780, 1300 620 S 1620 520, 1720 600"/>
    <path class="core" d="M -80 640 C 200 700, 420 500, 700 620 S 1040 780, 1300 620 S 1620 520, 1720 600"/>
  </g>
  <g class="rb" style="color: var(--rib-b); --rb-dur: 16s; --rb-delay: -7s;">
    <path class="halo" d="M -60 380 C 260 460, 520 260, 780 400 S 1140 540, 1420 380 S 1660 300, 1720 360"/>
    <path class="mid"  d="M -60 380 C 260 460, 520 260, 780 400 S 1140 540, 1420 380 S 1660 300, 1720 360"/>
    <path class="core" d="M -60 380 C 260 460, 520 260, 780 400 S 1140 540, 1420 380 S 1660 300, 1720 360"/>
  </g>
  <g class="rb" style="color: var(--rib-d); --rb-dur: 25s; --rb-delay: -16s;">
    <path class="halo" d="M -80 300 C 220 200, 480 440, 740 320 S 1080 160, 1360 320 S 1620 460, 1720 400"/>
    <path class="mid"  d="M -80 300 C 220 200, 480 440, 740 320 S 1080 160, 1360 320 S 1620 460, 1720 400"/>
    <path class="core" d="M -80 300 C 220 200, 480 440, 740 320 S 1080 160, 1360 320 S 1620 460, 1720 400"/>
  </g>
  <g class="rb" style="color: var(--rib-c); --rb-dur: 14s; --rb-delay: -8s;">
    <path class="halo" d="M -60 800 C 240 720, 460 860, 760 760 S 1140 660, 1400 800 S 1680 880, 1720 820"/>
    <path class="mid"  d="M -60 800 C 240 720, 460 860, 760 760 S 1140 660, 1400 800 S 1680 880, 1720 820"/>
    <path class="core" d="M -60 800 C 240 720, 460 860, 760 760 S 1140 660, 1400 800 S 1680 880, 1720 820"/>
  </g>
  <g class="rb" style="color: var(--rib-e); --rb-dur: 18s; --rb-delay: -4s;">
    <path class="halo" d="M -60 8 C 280 20, 540 14, 780 28 S 1140 220, 1420 80 S 1660 40, 1700 60"/>
    <path class="mid"  d="M -60 8 C 280 20, 540 14, 780 28 S 1140 220, 1420 80 S 1660 40, 1700 60"/>
    <path class="core" d="M -60 8 C 280 20, 540 14, 780 28 S 1140 220, 1420 80 S 1660 40, 1700 60"/>
  </g>
  <g class="rb" style="color: var(--rib-b); --rb-dur: 12s; --rb-delay: -1s;">
    <path class="halo" d="M -60 440 C 220 360, 460 580, 720 460 S 1080 280, 1360 460 S 1640 580, 1720 500"/>
    <path class="mid"  d="M -60 440 C 220 360, 460 580, 720 460 S 1080 280, 1360 460 S 1640 580, 1720 500"/>
    <path class="core" d="M -60 440 C 220 360, 460 580, 720 460 S 1080 280, 1360 460 S 1640 580, 1720 500"/>
  </g>
  <g class="rb" style="color: var(--rib-a); --rb-dur: 20s; --rb-delay: -13s;">
    <path class="halo" d="M -80 580 C 200 520, 480 680, 740 580 S 1080 420, 1340 580 S 1640 700, 1720 640"/>
    <path class="mid"  d="M -80 580 C 200 520, 480 680, 740 580 S 1080 420, 1340 580 S 1640 700, 1720 640"/>
    <path class="core" d="M -80 580 C 200 520, 480 680, 740 580 S 1080 420, 1340 580 S 1640 700, 1720 640"/>
  </g>
</svg>
"##;

// Ribbon-backdrop JS:
//   1. Rotate --sd-hover-hue through the ribbon palette per tile so
//      adjacent cards glow in different colors on hover.
//   2. Ignite: add .ignite to a few random ribbons for ~6s, drifting
//      the breathing band higher. CSS @property on --rb-base/--rb-peak
//      does the 3s ease (theme.rs). Fires every 20-40s on a random
//      ribbon, and half the ribbons on each hour boundary. Exposes
//      window.sdIgnite(n) so widgets (activity, new message) can
//      trigger it manually.
//   3. Pause breathing when tab hidden.
const RIBBON_SCRIPT: &str = r##"
(function() {
  const HUES = ['var(--rib-a)','var(--rib-b)','var(--rib-c)','var(--rib-d)','var(--rib-e)'];
  let idx = 0;
  function paint(tile) {
    if (!tile || tile.dataset.rbHued === '1') return;
    tile.style.setProperty('--sd-hover-hue', HUES[idx % HUES.length]);
    idx++;
    tile.dataset.rbHued = '1';
  }
  document.querySelectorAll('.sd-tile').forEach(paint);
  const grid = document.getElementById('sd-grid');
  if (grid) {
    new MutationObserver((muts) => {
      for (const m of muts) for (const n of m.addedNodes) {
        if (n.nodeType === 1 && n.classList && n.classList.contains('sd-tile')) paint(n);
      }
    }).observe(grid, { childList: true });
  }
  // On hover, advance the hue cursor so repeat hovers feel alive.
  let hoverCursor = 0;
  document.querySelectorAll('.sd-tile').forEach((tile) => {
    tile.addEventListener('mouseenter', () => {
      hoverCursor = (hoverCursor + 1) % HUES.length;
      tile.style.setProperty('--sd-hover-hue', HUES[hoverCursor]);
    });
  });

  // ── Ignite / surge ───────────────────────────────────────────────
  function ribbons() { return document.querySelectorAll('.sd-rb .rb'); }
  function ignite(count) {
    const rbs = Array.from(ribbons());
    if (!rbs.length) return;
    // Fisher-Yates shuffle so consecutive fires pick different ribbons.
    for (let i = rbs.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [rbs[i], rbs[j]] = [rbs[j], rbs[i]];
    }
    rbs.slice(0, Math.max(1, count | 0)).forEach((rb, i) => {
      // Stagger multi-ribbon ignites so the surge has some rhythm.
      setTimeout(() => {
        rb.classList.add('ignite');
        setTimeout(() => rb.classList.remove('ignite'), 6000);
      }, i * 600);
    });
  }
  window.sdIgnite = ignite;

  // Simulated activity pulse: every 20-40s, ignite 1 ribbon. Skipped
  // while the tab is hidden so background tabs stay idle.
  function scheduleActivity() {
    const next = 20000 + Math.random() * 20000;
    setTimeout(() => {
      if (!document.hidden) ignite(1);
      scheduleActivity();
    }, next);
  }
  scheduleActivity();

  // Hour boundary: ignite half the ribbons to mark the tick-over.
  function scheduleHour() {
    const now = new Date();
    const next = new Date(now);
    next.setHours(now.getHours() + 1, 0, 1, 0);
    setTimeout(() => {
      const n = Math.max(2, Math.ceil(ribbons().length / 2));
      if (!document.hidden) ignite(n);
      scheduleHour();
    }, next - now);
  }
  scheduleHour();

  // Pause breathing when tab hidden — zero GPU when not looking.
  document.addEventListener('visibilitychange', () => {
    document.body.classList.toggle('rb-paused', document.hidden);
  });
})();
"##;
