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
    };
    let body = html! {
        (top_bar("Dashboard", None))
        style { (PreEscaped(THEME_STYLE)) }
        style { (PreEscaped(DASHBOARD_STYLE)) }
        main class="sd-root" id="sd-root" {
            (greeting_strip())
            (sun_indicator())
            div class="sd-grid" id="sd-grid" data-mode="view" {}
        }
        (drawer())
        (floating_controls())
        script { (PreEscaped(THEME_SCRIPT)) }
        script { (PreEscaped(widget_templates_js())) }
        script { (PreEscaped(DASHBOARD_SCRIPT)) }
    };
    Html(shell(page, body).into_string())
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
            div class="sd-drawer-body" {
                @for w in &catalog {
                    button class="sd-drawer-item" data-kind=(w.kind()) {
                        div class="sd-drawer-item-title" { (w.title()) }
                        div class="sd-drawer-item-desc" { (w.description()) }
                    }
                }
            }
        }
    }
}

// ─── Floating circles (Edit + Focus) ───────────────────────────────────

fn floating_controls() -> Markup {
    html! {
        div class="sd-fab-stack" id="sd-fab" {
            // Focus circle — expands into Normal / Focus / Quiet options.
            div class="sd-fab-group" id="sd-focus-group" data-open="false" {
                button class="sd-fab" id="sd-focus-btn" aria-label="Focus modes" title="Focus" {
                    svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" {
                        circle cx="12" cy="12" r="8" {}
                        circle cx="12" cy="12" r="3" {}
                    }
                }
                div class="sd-fab-menu" role="menu" {
                    button class="sd-fab-opt" data-focus="normal" { "Normal" }
                    button class="sd-fab-opt" data-focus="focus"  { "Focus" }
                    button class="sd-fab-opt" data-focus="quiet"  { "Quiet" }
                }
            }
            // Edit circle — click to toggle edit mode + expand options.
            div class="sd-fab-group" id="sd-edit-group" data-open="false" {
                button class="sd-fab sd-fab-primary" id="sd-edit-btn" aria-label="Edit dashboard" title="Edit" {
                    svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" {
                        path d="M12 20h9" {}
                        path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5Z" {}
                    }
                }
                div class="sd-fab-menu sd-fab-menu-wide" role="menu" {
                    button class="sd-fab-opt" id="sd-add-widget"   { "+ Add widget" }
                    button class="sd-fab-opt" id="sd-reset-layout" { "Reset layout" }
                    button class="sd-fab-opt sd-fab-opt-done" id="sd-edit-done" { "Done ✓" }
                }
            }
        }
    }
}

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
                .replace("${", "\\${");
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

.sd-root {
  max-width: 1400px;
  margin: 0 auto;
  padding: 32px 24px 120px;
  min-height: calc(100vh - 48px);
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
  grid-auto-rows: 72px;
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
   Widget JS removes .loading once it renders. */
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
}
.sd-grid[data-mode="edit"] .sd-tile::before {
  content: "";
  position: absolute; inset: 0;
  background: var(--accent-soft);
  pointer-events: none;
  opacity: 0.4;
  z-index: 0;
}
.sd-grid[data-mode="edit"] .sd-tile > * { position: relative; z-index: 1; }
.sd-tile-tools {
  display: none;
  position: absolute; top: 8px; right: 8px;
  gap: 4px; z-index: 2;
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

/* ── Floating circles (bottom-right FAB stack) ──────────────────── */
.sd-fab-stack {
  position: fixed; bottom: 24px; right: 24px; z-index: 100;
  display: flex; flex-direction: column; gap: 16px; align-items: flex-end;
}
.sd-fab-group { position: relative; display: flex; flex-direction: column; align-items: flex-end; }
.sd-fab {
  width: 44px; height: 44px; border-radius: 50%;
  background: var(--bg-elev); border: 1px solid var(--line);
  color: var(--fg-dim); cursor: pointer;
  display: flex; align-items: center; justify-content: center;
  box-shadow: var(--shadow-soft);
  transition: transform 200ms ease-out, border-color 200ms, color 200ms, background 200ms;
}
.sd-fab:hover { transform: translateY(-1px); color: var(--fg); border-color: var(--accent); }
.sd-fab-primary { background: var(--accent); color: var(--accent-ink); border-color: transparent; }
.sd-fab-primary:hover { color: var(--accent-ink); border-color: transparent; filter: brightness(1.05); }
.sd-fab-group[data-open="true"] .sd-fab { border-color: var(--accent); }

.sd-fab-menu {
  position: absolute; bottom: 56px; right: 0;
  background: var(--bg-elev); border: 1px solid var(--line);
  border-radius: 14px; padding: 6px;
  display: none; flex-direction: column; gap: 2px;
  min-width: 160px; box-shadow: var(--shadow-soft);
}
.sd-fab-menu-wide { min-width: 200px; flex-direction: row; }
.sd-fab-group[data-open="true"] .sd-fab-menu { display: flex; }
.sd-fab-opt {
  background: transparent; border: none; color: var(--fg);
  padding: 8px 12px; border-radius: 8px; cursor: pointer;
  text-align: left; font-size: 14px; font-family: inherit;
  white-space: nowrap;
}
.sd-fab-opt:hover { background: var(--bg-hover); }
.sd-fab-opt-done { color: var(--accent); font-weight: 500; }

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
.sd-drawer-body { padding: 12px; overflow-y: auto; flex: 1; display: flex; flex-direction: column; gap: 8px; }
.sd-drawer-item { background: var(--bg-card); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; text-align: left; cursor: pointer; color: inherit; font-family: inherit; transition: border-color 200ms, background 200ms; }
.sd-drawer-item:hover { border-color: var(--accent); background: var(--bg-hover); }
.sd-drawer-item-title { color: var(--fg); font-weight: 500; margin-bottom: 4px; }
.sd-drawer-item-desc { color: var(--fg-mute); font-size: 13px; line-height: 1.4; }
"##;

// ─── Client-side grid logic ────────────────────────────────────────────

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
      const r = await fetch('/api/me', { credentials:'same-origin' });
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
  const editBtn = document.getElementById('sd-edit-btn');
  const editGroup = document.getElementById('sd-edit-group');
  const focusBtn = document.getElementById('sd-focus-btn');
  const focusGroup = document.getElementById('sd-focus-group');
  const addBtn = document.getElementById('sd-add-widget');
  const resetBtn = document.getElementById('sd-reset-layout');
  const doneBtn = document.getElementById('sd-edit-done');

  let layout = [];  // [{id, kind, size}]
  let mode = 'view';
  let nextId = 1;

  function saveLayout() {
    fetch('/api/dashboard/layout', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ items: layout })
    }).catch(() => {});
  }

  async function loadLayout() {
    try {
      const r = await fetch('/api/dashboard/layout', { credentials: 'same-origin' });
      if (!r.ok) { layout = defaultLayout(); return; }
      const d = await r.json();
      layout = Array.isArray(d.items) && d.items.length ? d.items : defaultLayout();
    } catch { layout = defaultLayout(); }
    nextId = layout.reduce((m, it) => Math.max(m, it.id || 0), 0) + 1;
  }

  function defaultLayout() {
    return [
      { id: 1, kind: 'today', size: 'M' },
      { id: 2, kind: 'now_playing', size: 'M' },
    ];
  }

  function renderGrid() {
    grid.innerHTML = '';
    if (!layout.length) {
      grid.innerHTML = `
        <div class="sd-empty">
          <div class="sd-empty-title">Your dashboard is empty</div>
          <div class="sd-mute">Click the pencil in the bottom-right to add widgets.</div>
        </div>`;
      return;
    }
    layout.forEach((item, idx) => {
      const def = window.__SYNTAUR_WIDGETS[item.kind];
      if (!def) return;
      const tile = document.createElement('div');
      tile.className = `sd-tile size-${item.size.toLowerCase()} loading`;
      tile.style.setProperty('--sd-idx', String(idx));
      tile.dataset.id = item.id;
      tile.dataset.idx = idx;
      tile.dataset.kind = item.kind;
      tile.dataset.size = item.size;
      tile.id = `sd-tile-${item.id}`;
      const tplHtml = (def.templates[item.size] || def.templates.M || '').replace(/__SD_ID__/g, `sd-tile-${item.id}`);
      tile.innerHTML = tplHtml;
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
      wireSkeleton(tile);
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
  function setMode(m) { mode = m; grid.dataset.mode = m; renderGrid(); }

  function toggleGroup(group, other) {
    const open = group.dataset.open !== 'true';
    group.dataset.open = open ? 'true' : 'false';
    if (other) other.dataset.open = 'false';
  }

  editBtn.addEventListener('click', () => {
    toggleGroup(editGroup, focusGroup);
    if (editGroup.dataset.open === 'true' && mode !== 'edit') setMode('edit');
  });
  focusBtn.addEventListener('click', () => toggleGroup(focusGroup, editGroup));

  focusGroup.querySelectorAll('[data-focus]').forEach(btn => {
    btn.addEventListener('click', () => {
      const f = btn.dataset.focus;
      document.body.classList.toggle('sd-focus', f === 'focus');
      document.body.classList.toggle('sd-quiet', f === 'quiet');
      focusGroup.dataset.open = 'false';
    });
  });

  addBtn.addEventListener('click', () => { drawer.classList.add('open'); drawer.setAttribute('aria-hidden','false'); editGroup.dataset.open = 'false'; });
  drawerClose.addEventListener('click', () => { drawer.classList.remove('open'); drawer.setAttribute('aria-hidden','true'); });
  drawer.querySelectorAll('.sd-drawer-item').forEach(btn => {
    btn.addEventListener('click', () => {
      const kind = btn.dataset.kind;
      const def = window.__SYNTAUR_WIDGETS[kind];
      if (!def) return;
      const [dw, dh] = def.defaultSize;
      const size = Object.keys(SIZE_CELLS).find(s => SIZE_CELLS[s][0] === dw && SIZE_CELLS[s][1] === dh) || 'M';
      layout.push({ id: nextId++, kind, size });
      drawer.classList.remove('open');
      drawer.setAttribute('aria-hidden','true');
      renderGrid(); saveLayout();
    });
  });
  resetBtn.addEventListener('click', () => {
    if (!confirm('Reset dashboard layout to defaults?')) return;
    layout = defaultLayout();
    nextId = layout.length + 1;
    renderGrid(); saveLayout();
  });
  doneBtn.addEventListener('click', () => { setMode('view'); editGroup.dataset.open = 'false'; });

  // Outside-click closes any open group.
  document.addEventListener('click', ev => {
    if (!ev.target.closest('#sd-fab')) {
      editGroup.dataset.open = 'false';
      focusGroup.dataset.open = 'false';
    }
  });

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
