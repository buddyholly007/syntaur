//! `/smart-home` — Smart Home dashboard.
//!
//! Locked layout per [[projects/syntaur_smart_home_dashboard]] (approved
//! 2026-04-26 from `/home/sean/Desktop/syntaur-smart-home-mockup.html`).
//! Three-column glass-morphism: Lights / Security / Energy tiles on the
//! left, Climate ring center, Room cards right. Scenes row across the
//! top, status footer with nav at the bottom. Energy lives as a left
//! tile drawer — there is no `/energy` module by design.
//!
//! Phase 2D ships the shell with placeholder data and the JS hooks the
//! later phases bind onto:
//!   - 2E: Climate ring drag-knob + setpoint setter
//!   - 2F: Lights/Security/Energy live tile data + sparklines
//!   - 2G: Room cards from `GET /api/smart-home/rooms`
//!   - 2H: Scenes wired to `POST /api/smart-home/scenes/{id}/activate`
//!   - 2I: Energy drawer with calendar heatmap
//!   - 2J: Tariff settings under Settings → Smart Home → Energy
//!   - 2K: Anomaly detection on top of energy ingestion
//!
//! Earlier modules (the 2-pane room-sidebar + device-grid + scan modal +
//! automation builder + BLE anchor editor) lived here through Track A
//! Week 1; that JS has been retired in favor of the locked design. The
//! deeper surfaces (scan, automations, BLE, camera, diagnostics) will
//! reappear as drawers + sub-pages off the new shell as their phases
//! land.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Smart Home",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: Some("Smart Home"),
        topbar_status: None,
    };

    let body = html! {
        (top_bar("Smart Home", None))

        div class="sh-app" {
            // ── Hero strip: clock + date + weather + status pills ──
            header class="sh-hero" {
                div id="sh-clock" class="sh-clock" { "—" }
                div id="sh-date" class="sh-date" { "—" }
                span class="sh-spacer" {}
                div class="sh-weather" {
                    span id="sh-weather-temp" { "—" }
                    span class="sh-icon-pill" id="sh-weather-cond" { "Loading" }
                }
                div class="sh-status-pills" {
                    span class="sh-icon-pill" {
                        span class="sh-dot" {}
                        "All systems"
                    }
                    span class="sh-icon-pill" id="sh-wifi-pill" { "Wi-Fi" }
                }
            }

            // ── Scenes row ─────────────────────────────────────────
            section class="sh-scenes" id="sh-scenes" {
                @for (slug, glyph, name, sub) in &[
                    ("good-morning", "☀", "Good Morning", "Bright & energizing"),
                    ("away",         "⌂", "Away",         "Secure & efficient"),
                    ("movie-mode",   "▷", "Movie Mode",   "Dim & immersive"),
                    ("night",        "☾", "Night",        "Relax & unwind"),
                ] {
                    button type="button" class="sh-scene" data-scene-slug=(slug) {
                        div class="sh-scene-glyph" { (glyph) }
                        div class="sh-scene-meta" {
                            div class="sh-scene-name" { (name) }
                            div class="sh-scene-sub"  { (sub) }
                        }
                    }
                }
            }

            // ── Main 3-column ──────────────────────────────────────
            section class="sh-main" {
                // LEFT: summary tiles
                div class="sh-col sh-col-left" {
                    button type="button" class="sh-tile sh-tile-lights" data-drawer="lights" {
                        div class="sh-tile-head" {
                            span class="sh-tile-label" { "Lights" }
                            span class="sh-tile-glyph" { "☀" }
                        }
                        div class="sh-tile-primary" id="sh-lights-primary" {
                            "—"
                            span class="sh-tile-unit" { "loading" }
                        }
                        div class="sh-tile-secondary" id="sh-lights-secondary" { "—" }
                        svg class="sh-sparkline" viewBox="0 0 100 32" preserveAspectRatio="none" {
                            path id="sh-lights-spark"
                                 d="M0,16 L100,16"
                                 fill="none" stroke="var(--sh-accent-warn)"
                                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" {}
                        }
                    }

                    button type="button" class="sh-tile sh-tile-security" data-drawer="security" {
                        div class="sh-tile-head" {
                            span class="sh-tile-label" { "Security" }
                            span class="sh-tile-glyph" { "⛨" }
                        }
                        div class="sh-tile-primary" id="sh-security-primary" { "—" }
                        div class="sh-tile-secondary" id="sh-security-secondary" { "—" }
                    }

                    button type="button" class="sh-tile sh-tile-energy" data-drawer="energy" {
                        div class="sh-tile-head" {
                            span class="sh-tile-label" { "Energy" }
                            span class="sh-tile-glyph" { "⚡" }
                        }
                        div class="sh-tile-primary" id="sh-energy-primary" {
                            "—"
                            span class="sh-tile-unit" { "kW now" }
                        }
                        div class="sh-tile-secondary" id="sh-energy-secondary" { "— kWh today" }
                        svg class="sh-sparkline" viewBox="0 0 100 32" preserveAspectRatio="none" {
                            g id="sh-energy-spark" fill="var(--sh-accent-warm)" {}
                        }
                    }
                }

                // CENTER: Climate ring (2E wires the drag-knob + setpoint)
                div class="sh-climate" {
                    div class="sh-climate-head" {
                        span class="sh-tile-label" { "Climate" }
                        span class="sh-mode-pill" id="sh-climate-mode" {
                            span class="sh-dot" {}
                            "Loading"
                        }
                    }

                    div class="sh-ring-wrap" {
                        svg viewBox="0 0 200 200" {
                            defs {
                                linearGradient id="shRingGrad" x1="0%" y1="0%" x2="100%" y2="100%" {
                                    stop offset="0%"   stop-color="#62a8ff" {}
                                    stop offset="100%" stop-color="#5ee2ff" {}
                                }
                            }
                            // 270° track. Circumference 2π·88 = 552.92;
                            // 270° arc = 414.69. Rotated 45° puts the
                            // gap at the bottom.
                            circle class="sh-ring-track" cx="100" cy="100" r="88"
                                   stroke-dasharray="414.69 552.92"
                                   transform="rotate(45 100 100)" {}
                            circle id="sh-ring-fill" class="sh-ring-fill"
                                   cx="100" cy="100" r="88"
                                   stroke-dasharray="0 552.92"
                                   transform="rotate(45 100 100)" {}
                            circle id="sh-ring-knob" class="sh-ring-knob"
                                   cx="100" cy="12" r="9" {}
                        }
                        div class="sh-ring-center" {
                            div id="sh-ring-setpoint" class="sh-ring-setpoint" {
                                "—"
                                span class="sh-ring-deg" { "°" }
                            }
                            div class="sh-ring-label-small" { "Setpoint" }
                            div class="sh-ring-current" {
                                "Currently "
                                strong id="sh-ring-current" { "—°" }
                            }
                        }
                    }

                    div class="sh-control-row" {
                        button type="button" class="sh-ctrl-btn" data-mode="heat" {
                            span class="sh-ctrl-icon" { "☀" }
                            span { "Heat" }
                        }
                        button type="button" class="sh-ctrl-btn" data-mode="cool" {
                            span class="sh-ctrl-icon" { "❄" }
                            span { "Cool" }
                        }
                        button type="button" class="sh-ctrl-btn" data-mode="auto" {
                            span class="sh-ctrl-icon" { "⟳" }
                            span { "Auto" }
                        }
                    }

                    div class="sh-env-chips" {
                        span class="sh-env-chip" {
                            "Outdoor "
                            strong id="sh-env-outdoor" { "—°" }
                        }
                        span class="sh-env-chip" {
                            "Humidity "
                            strong id="sh-env-humidity" { "—%" }
                        }
                        span class="sh-env-chip" {
                            "Air "
                            strong id="sh-env-air" { "—" }
                        }
                    }
                }

                // RIGHT: room cards (Phase 2G fills from /api/smart-home/rooms)
                div class="sh-col sh-col-right" id="sh-rooms" {
                    // Skeleton placeholders. Replaced by JS once
                    // /api/smart-home/rooms returns. Empty state lands
                    // here if list is empty.
                    div class="sh-room-card sh-room-skeleton" { "Loading rooms…" }
                }
            }

            // ── Footer ────────────────────────────────────────────
            footer class="sh-footer" {
                span class="sh-footer-health" {
                    span class="sh-dot" {}
                    span id="sh-footer-status" { "All systems normal" }
                }
                span class="sh-spacer" {}
                a class="sh-nav-link" href="#" data-drawer="rooms-all" { "View all rooms" }
                a class="sh-nav-link" href="/smart-home/privacy" { "Privacy" }
                a class="sh-nav-link" href="/smart-home/firmware" { "Firmware" }
                a class="sh-nav-link" href="/settings#smart-home" { "Settings" }
            }
        }

        // ── Drawers (Phase 2F/2G/2I fill these in) ─────────────────
        div id="sh-drawer-root" class="sh-drawer-root" hidden {
            div class="sh-drawer-scrim" data-drawer-close="1" {}
            aside class="sh-drawer" role="dialog" aria-modal="true" {
                header class="sh-drawer-head" {
                    h2 id="sh-drawer-title" { "" }
                    button type="button" class="sh-drawer-close" data-drawer-close="1" { "×" }
                }
                div id="sh-drawer-body" class="sh-drawer-body" {}
            }
        }

        script { (PreEscaped(SMART_HOME_JS)) }
    };

    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r#"
:root {
  --sh-bg-0: #0a1420;
  --sh-bg-1: #122236;
  --sh-glass: rgba(255, 255, 255, 0.05);
  --sh-glass-strong: rgba(255, 255, 255, 0.09);
  --sh-border: rgba(255, 255, 255, 0.13);
  --sh-text: #e8f0fa;
  --sh-text-dim: #94a8c0;
  --sh-text-faint: #5e7290;
  --sh-accent-cyan: #5ee2ff;
  --sh-accent-cool: #62a8ff;
  --sh-accent-warm: #ffb168;
  --sh-accent-good: #71e8a3;
  --sh-accent-warn: #ffd86b;
}

.sh-app {
  position: relative;
  min-height: calc(100vh - 48px);
  display: grid;
  grid-template-rows: 56px 76px 1fr 56px;
  gap: 14px;
  padding: 14px 22px;
  color: var(--sh-text);
  font: 14px/1.4 -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", "Inter", sans-serif;
  background:
    radial-gradient(ellipse at 30% 20%, rgba(110, 200, 255, 0.12), transparent 50%),
    radial-gradient(ellipse at 80% 90%, rgba(255, 177, 104, 0.10), transparent 55%),
    linear-gradient(165deg, var(--sh-bg-0) 0%, var(--sh-bg-1) 100%);
  overflow: hidden;
}
.sh-app::before {
  content: "";
  position: absolute; inset: 0;
  background:
    radial-gradient(circle at 18% 75%, rgba(94, 226, 255, 0.06) 0%, transparent 35%),
    radial-gradient(circle at 90% 30%, rgba(123, 162, 220, 0.07) 0%, transparent 40%);
  pointer-events: none;
  z-index: 0;
}
.sh-app > * { position: relative; z-index: 1; }

/* ── Hero strip ───────────────────────────────────── */
.sh-hero {
  display: flex; align-items: center; gap: 22px;
  padding: 0 18px;
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 14px;
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
}
.sh-clock { font-size: 22px; font-weight: 600; letter-spacing: 0.5px; }
.sh-clock .sh-ampm { font-size: 12px; color: var(--sh-text-faint); margin-left: 4px; font-weight: 500; }
.sh-date { color: var(--sh-text-dim); }
.sh-spacer { flex: 1; }
.sh-weather, .sh-status-pills {
  display: flex; align-items: center; gap: 10px; color: var(--sh-text-dim);
}
.sh-icon-pill {
  display: inline-flex; align-items: center; gap: 5px;
  padding: 4px 10px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 999px;
  font-size: 12px;
  color: var(--sh-text-dim);
}
.sh-dot {
  width: 7px; height: 7px; border-radius: 50%;
  background: var(--sh-accent-good);
  box-shadow: 0 0 6px var(--sh-accent-good);
}

/* ── Scenes row ───────────────────────────────────── */
.sh-scenes {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 12px;
}
.sh-scene {
  display: flex; align-items: center; gap: 12px;
  padding: 14px 18px;
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 14px;
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
  cursor: pointer;
  text-align: left;
  color: inherit;
  font: inherit;
  transition: all 0.15s ease;
}
.sh-scene:hover {
  background: var(--sh-glass-strong);
  border-color: rgba(94, 226, 255, 0.30);
  transform: translateY(-1px);
}
.sh-scene.sh-firing {
  border-color: var(--sh-accent-cyan);
  box-shadow: 0 0 0 2px rgba(94, 226, 255, 0.20);
}
.sh-scene-glyph {
  width: 36px; height: 36px;
  border-radius: 10px;
  display: grid; place-items: center;
  background: rgba(94, 226, 255, 0.10);
  border: 1px solid rgba(94, 226, 255, 0.22);
  font-size: 18px;
  color: var(--sh-accent-cyan);
}
.sh-scene-meta { display: flex; flex-direction: column; }
.sh-scene-name { font-weight: 600; font-size: 13px; color: var(--sh-text); }
.sh-scene-sub  { font-size: 11px; color: var(--sh-text-faint); }

/* ── Main grid ─────────────────────────────────────── */
.sh-main {
  display: grid;
  grid-template-columns: 1fr 1.3fr 1fr;
  gap: 14px;
  overflow: hidden;
  min-height: 0;
}
.sh-col {
  display: flex; flex-direction: column;
  gap: 14px;
  overflow-y: auto;
  min-height: 0;
}
.sh-col::-webkit-scrollbar { width: 0; }

/* ── Summary tile ─────────────────────────────────── */
.sh-tile {
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 16px;
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
  padding: 16px 18px;
  cursor: pointer;
  text-align: left;
  color: inherit;
  font: inherit;
  width: 100%;
  transition: all 0.15s ease;
}
.sh-tile:hover {
  background: var(--sh-glass-strong);
  border-color: rgba(94, 226, 255, 0.30);
}
.sh-tile-head {
  display: flex; align-items: center; justify-content: space-between;
  margin-bottom: 10px;
}
.sh-tile-label {
  font-size: 11px;
  letter-spacing: 1.5px;
  text-transform: uppercase;
  color: var(--sh-text-faint);
  font-weight: 600;
}
.sh-tile-glyph {
  width: 28px; height: 28px;
  border-radius: 8px;
  display: grid; place-items: center;
  font-size: 14px;
}
.sh-tile-lights .sh-tile-glyph   { background: rgba(255, 216, 107, 0.12); color: var(--sh-accent-warn); }
.sh-tile-security .sh-tile-glyph { background: rgba(113, 232, 163, 0.12); color: var(--sh-accent-good); }
.sh-tile-energy .sh-tile-glyph   { background: rgba(255, 177, 104, 0.12); color: var(--sh-accent-warm); }
.sh-tile-primary {
  font-size: 24px; font-weight: 600;
  color: var(--sh-text);
  line-height: 1.1;
}
.sh-tile-unit {
  font-size: 13px; font-weight: 500;
  color: var(--sh-text-dim);
  margin-left: 4px;
}
.sh-tile-secondary {
  margin-top: 4px;
  font-size: 12px;
  color: var(--sh-text-dim);
}
.sh-sparkline {
  margin-top: 12px;
  height: 32px;
  width: 100%;
}

/* ── Climate card ─────────────────────────────────── */
.sh-climate {
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 22px;
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  padding: 22px;
  display: flex; flex-direction: column;
  align-items: center;
  justify-content: space-between;
  min-height: 0;
}
.sh-climate-head {
  display: flex; align-items: center; justify-content: space-between;
  width: 100%;
}
.sh-mode-pill {
  display: inline-flex; align-items: center; gap: 6px;
  font-size: 11px;
  padding: 4px 10px;
  background: rgba(98, 168, 255, 0.10);
  color: var(--sh-accent-cool);
  border: 1px solid rgba(98, 168, 255, 0.22);
  border-radius: 999px;
}
.sh-mode-pill .sh-dot {
  width: 6px; height: 6px;
  background: var(--sh-accent-cool);
  box-shadow: 0 0 6px var(--sh-accent-cool);
}

.sh-ring-wrap {
  position: relative;
  width: 280px; height: 280px;
  margin: 4px 0;
}
.sh-ring-wrap svg { width: 100%; height: 100%; transform: rotate(-90deg); }
.sh-ring-track {
  fill: none;
  stroke: rgba(255, 255, 255, 0.06);
  stroke-width: 14;
  stroke-linecap: round;
}
.sh-ring-fill {
  fill: none;
  stroke: url(#shRingGrad);
  stroke-width: 14;
  stroke-linecap: round;
  transition: stroke-dasharray 0.4s ease;
  filter: drop-shadow(0 0 8px rgba(94, 226, 255, 0.35));
}
.sh-ring-knob {
  fill: var(--sh-text);
  stroke: var(--sh-accent-cyan);
  stroke-width: 3;
  cursor: grab;
  filter: drop-shadow(0 0 8px rgba(94, 226, 255, 0.6));
}
.sh-ring-knob.sh-dragging { cursor: grabbing; }

.sh-ring-center {
  position: absolute; inset: 0;
  display: flex; flex-direction: column;
  align-items: center; justify-content: center;
  text-align: center;
  pointer-events: none;
}
.sh-ring-setpoint {
  font-size: 60px; font-weight: 200;
  line-height: 1;
  color: var(--sh-text);
  letter-spacing: -2px;
}
.sh-ring-deg {
  font-size: 28px; vertical-align: top; margin-left: 2px;
  color: var(--sh-accent-cyan);
}
.sh-ring-label-small {
  margin-top: 8px;
  font-size: 11px; letter-spacing: 1.5px; text-transform: uppercase;
  color: var(--sh-text-faint); font-weight: 600;
}
.sh-ring-current {
  margin-top: 4px;
  font-size: 13px; color: var(--sh-text-dim);
}
.sh-ring-current strong { color: var(--sh-text); font-weight: 600; }

.sh-control-row {
  width: 100%;
  display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px;
  margin-top: 14px;
}
.sh-ctrl-btn {
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.08);
  color: var(--sh-text-dim);
  border-radius: 10px;
  padding: 10px;
  font-size: 12px; font-weight: 600;
  cursor: pointer;
  display: flex; flex-direction: column; align-items: center; gap: 4px;
  transition: all 0.15s ease;
}
.sh-ctrl-btn:hover {
  background: var(--sh-glass-strong);
  color: var(--sh-text);
}
.sh-ctrl-btn.sh-active {
  background: rgba(98, 168, 255, 0.14);
  border-color: rgba(98, 168, 255, 0.36);
  color: var(--sh-accent-cool);
}
.sh-ctrl-icon { font-size: 16px; }

.sh-env-chips {
  margin-top: 14px;
  width: 100%;
  display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px;
}
.sh-env-chip {
  display: flex; align-items: center; gap: 6px;
  font-size: 11px; color: var(--sh-text-dim);
  padding: 6px 10px;
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(255, 255, 255, 0.06);
  border-radius: 8px;
}
.sh-env-chip strong { color: var(--sh-text); font-weight: 600; }

/* ── Room cards ───────────────────────────────────── */
.sh-room-card {
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 14px;
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
  padding: 14px 16px;
  display: flex; flex-direction: column;
  gap: 10px;
  cursor: pointer;
  transition: all 0.15s ease;
}
.sh-room-card:hover {
  background: var(--sh-glass-strong);
  border-color: rgba(94, 226, 255, 0.30);
}
.sh-room-skeleton {
  cursor: default;
  color: var(--sh-text-faint);
  text-align: center;
  font-size: 12px;
  padding: 28px 16px;
}
.sh-room-head {
  display: flex; align-items: center; justify-content: space-between;
}
.sh-room-name { font-weight: 600; font-size: 14px; color: var(--sh-text); }
.sh-room-on { font-size: 11px; color: var(--sh-accent-good); }
.sh-room-on.sh-off { color: var(--sh-text-faint); }
.sh-room-stats {
  display: flex; align-items: center; gap: 14px;
  font-size: 12px; color: var(--sh-text-dim);
}
.sh-room-stats strong { color: var(--sh-text); font-weight: 600; }
.sh-room-controls {
  display: flex; align-items: center; gap: 10px;
}

.sh-toggle {
  width: 38px; height: 22px;
  background: rgba(255, 255, 255, 0.10);
  border-radius: 999px;
  position: relative;
  cursor: pointer;
  border: 0;
  padding: 0;
  flex: 0 0 auto;
  transition: all 0.2s ease;
}
.sh-toggle::after {
  content: "";
  width: 18px; height: 18px;
  background: var(--sh-text);
  border-radius: 50%;
  position: absolute; top: 2px; left: 2px;
  transition: all 0.2s ease;
}
.sh-toggle.sh-on {
  background: rgba(94, 226, 255, 0.35);
  box-shadow: 0 0 8px rgba(94, 226, 255, 0.3) inset;
}
.sh-toggle.sh-on::after { left: 18px; background: var(--sh-accent-cyan); }
.sh-dim-bar {
  flex: 1;
  height: 6px;
  background: rgba(255, 255, 255, 0.06);
  border-radius: 999px;
  position: relative;
  overflow: hidden;
}
.sh-dim-bar .sh-dim-fill {
  height: 100%;
  background: linear-gradient(90deg, rgba(255, 216, 107, 0.4), var(--sh-accent-warn));
  border-radius: 999px;
}

/* ── Footer ───────────────────────────────────────── */
.sh-footer {
  display: flex; align-items: center; gap: 14px;
  padding: 0 18px;
  background: var(--sh-glass);
  border: 1px solid var(--sh-border);
  border-radius: 14px;
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
  color: var(--sh-text-dim);
  font-size: 12px;
}
.sh-footer-health { display: flex; align-items: center; gap: 8px; }
.sh-nav-link {
  color: var(--sh-text-dim);
  text-decoration: none;
  padding: 6px 12px;
  border-radius: 8px;
  transition: all 0.15s ease;
}
.sh-nav-link:hover {
  background: var(--sh-glass-strong);
  color: var(--sh-text);
}

/* ── Drawer (used by left tiles + room cards) ─────── */
.sh-drawer-root {
  position: fixed; inset: 0;
  z-index: 50;
  display: grid;
  grid-template-columns: 1fr min(600px, 95vw);
}
.sh-drawer-scrim {
  background: rgba(0, 0, 0, 0.55);
  backdrop-filter: blur(2px);
}
.sh-drawer {
  background: linear-gradient(165deg, var(--sh-bg-1), var(--sh-bg-0));
  border-left: 1px solid var(--sh-border);
  display: flex; flex-direction: column;
  overflow: hidden;
  color: var(--sh-text);
}
.sh-drawer-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--sh-border);
}
.sh-drawer-head h2 { margin: 0; font-size: 16px; font-weight: 600; }
.sh-drawer-close {
  background: transparent; border: 0;
  color: var(--sh-text-dim);
  font-size: 24px; cursor: pointer;
  padding: 0 6px;
}
.sh-drawer-close:hover { color: var(--sh-text); }
.sh-drawer-body {
  padding: 18px 20px;
  overflow-y: auto;
  font-size: 13px;
  color: var(--sh-text-dim);
}
"#;

const SMART_HOME_JS: &str = r#"
// Phase 2D bootstrap. Wires:
//   - clock + date refresh (1s)
//   - scene-card click → POST /api/smart-home/scenes/{id}/activate
//     once the scene id catalog is fetched
//   - room-card list → GET /api/smart-home/rooms
//   - tile / nav-link drawer open + ESC close
//
// Phase 2E will own the climate ring drag-knob; 2F the live tile data;
// 2G the room toggle/dim wiring; 2I the energy drawer body.

(function () {
  // ── helpers ─────────────────────────────────────────
  async function shFetch(path, opts) {
    const r = await fetch(path, opts || {});
    if (!r.ok) {
      let msg = 'HTTP ' + r.status;
      try { const j = await r.json(); if (j && j.error) msg += ': ' + j.error; } catch (_) {}
      throw new Error(msg);
    }
    return r.json();
  }
  function $(id) { return document.getElementById(id); }

  // ── clock + date ────────────────────────────────────
  function tickClock() {
    const now = new Date();
    let hh = now.getHours();
    const mm = String(now.getMinutes()).padStart(2, '0');
    const ampm = hh >= 12 ? 'PM' : 'AM';
    hh = hh % 12; if (hh === 0) hh = 12;
    const clock = $('sh-clock');
    if (clock) {
      clock.innerHTML =
        String(hh).padStart(2, '0') + ':' + mm +
        '<span class="sh-ampm">' + ampm + '</span>';
    }
    const date = $('sh-date');
    if (date) {
      date.textContent = now.toLocaleDateString(undefined, {
        weekday: 'long', month: 'long', day: 'numeric'
      });
    }
  }
  tickClock();
  setInterval(tickClock, 1000);

  // ── scenes ─────────────────────────────────────────
  // Map slug → scene id once the user has seeded scenes (or pre-seeded
  // by the gateway on first launch). Until then the click is a no-op
  // that toasts a hint.
  const sceneIdsBySlug = {};
  async function loadScenes() {
    try {
      const data = await shFetch('/api/smart-home/scenes');
      const list = (data && (data.scenes || data.list)) || [];
      list.forEach((s) => {
        const slug = (s.slug || s.name || '').toString().toLowerCase()
          .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
        sceneIdsBySlug[slug] = s.id;
      });
    } catch (e) {
      console.warn('[smart-home] scene fetch failed:', e.message || e);
    }
  }
  loadScenes();

  document.querySelectorAll('.sh-scene').forEach((el) => {
    el.addEventListener('click', async () => {
      const slug = el.dataset.sceneSlug;
      const id = sceneIdsBySlug[slug];
      if (!id) {
        console.info('[smart-home] scene', slug, 'not yet seeded — Phase 2H wires this');
        return;
      }
      el.classList.add('sh-firing');
      try {
        await shFetch('/api/smart-home/scenes/' + id + '/activate', { method: 'POST' });
      } catch (e) {
        console.warn('[smart-home] scene activate failed:', e.message || e);
      } finally {
        setTimeout(() => el.classList.remove('sh-firing'), 700);
      }
    });
  });

  // ── room cards ─────────────────────────────────────
  // ROOM_STATE caches { rooms, devicesByRoom } so toggle/dim handlers
  // know which devices to dispatch to without re-fetching. Refreshed
  // by loadRoomCards on the same 30s cadence as the tiles.
  const ROOM_STATE = { rooms: [], devicesByRoom: new Map(), pendingDim: new Map() };

  function aggregateRoom(roomId) {
    const list = ROOM_STATE.devicesByRoom.get(roomId) || [];
    const lights = list.filter((d) => LIGHT_KINDS.includes(d.kind));
    const onLights = lights.filter((d) => isOn(parseState(d)));
    let avgLevelPct = 0;
    if (onLights.length) {
      let sum = 0, n = 0;
      onLights.forEach((d) => {
        const s = parseState(d);
        const lvl = (typeof s.level === 'number') ? s.level
                  : (typeof s.brightness === 'number') ? s.brightness
                  : null;
        if (lvl != null) {
          // Accept either 0..1 fraction or 0..100 percent.
          sum += lvl <= 1 ? lvl * 100 : lvl;
          n += 1;
        }
      });
      avgLevelPct = n ? Math.round(sum / n) : 100;
    }
    return {
      lightsTotal: lights.length,
      lightsOn: onLights.length,
      avgLevelPct,
    };
  }

  function renderRoomCard(room) {
    const card = document.createElement('div');
    card.className = 'sh-room-card';
    card.dataset.roomId = room.id;
    const agg = aggregateRoom(room.id);

    const allOn = agg.lightsOn === agg.lightsTotal && agg.lightsTotal > 0;
    const onText = agg.lightsTotal === 0
      ? 'No lights'
      : agg.lightsOn + ' light' + (agg.lightsOn === 1 ? '' : 's') + (agg.lightsOn > 0 ? ' on' : ' off');

    card.innerHTML =
      '<div class="sh-room-head">' +
        '<span class="sh-room-name"></span>' +
        '<span class="sh-room-on' + (agg.lightsOn === 0 ? ' sh-off' : '') + '"></span>' +
      '</div>' +
      '<div class="sh-room-stats">' +
        '<span class="stat">Brightness <strong>' + agg.avgLevelPct + '%</strong></span>' +
      '</div>' +
      '<div class="sh-room-controls">' +
        '<button type="button" class="sh-toggle' + (allOn ? ' sh-on' : '') + '" aria-label="Toggle room"></button>' +
        '<div class="sh-dim-bar"><div class="sh-dim-fill" style="width:' + (agg.lightsOn ? agg.avgLevelPct : 0) + '%"></div></div>' +
      '</div>';
    card.querySelector('.sh-room-name').textContent = room.name || 'Untitled';
    card.querySelector('.sh-room-on').textContent = onText;

    // Toggle: send {on: !allOn} to every light in the room.
    const toggle = card.querySelector('.sh-toggle');
    toggle.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      const desiredOn = !toggle.classList.contains('sh-on');
      toggle.classList.toggle('sh-on', desiredOn);
      const lights = (ROOM_STATE.devicesByRoom.get(room.id) || [])
        .filter((d) => LIGHT_KINDS.includes(d.kind));
      await Promise.allSettled(lights.map((d) =>
        shFetch('/api/smart-home/control', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ device_id: d.id, state: { on: desiredOn } }),
        })
      ));
    });

    // Dim bar: pointer x → percent, debounced POST level.
    const bar = card.querySelector('.sh-dim-bar');
    const fill = bar.querySelector('.sh-dim-fill');
    function pctFromPointer(ev) {
      const r = bar.getBoundingClientRect();
      let p = (ev.clientX - r.left) / r.width;
      p = Math.max(0, Math.min(1, p));
      return Math.round(p * 100);
    }
    function commitLevel(pct) {
      const lights = (ROOM_STATE.devicesByRoom.get(room.id) || [])
        .filter((d) => LIGHT_KINDS.includes(d.kind));
      const level = pct / 100;
      lights.forEach((d) => {
        shFetch('/api/smart-home/control', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ device_id: d.id, state: { on: pct > 0, level } }),
        }).catch((e) => console.warn('[smart-home] dim POST failed:', e.message || e));
      });
    }
    let dimDragging = false;
    function onDimMove(ev) {
      if (!dimDragging) return;
      const pct = pctFromPointer(ev);
      fill.style.width = pct + '%';
      const prev = ROOM_STATE.pendingDim.get(room.id);
      if (prev) clearTimeout(prev);
      ROOM_STATE.pendingDim.set(room.id, setTimeout(() => commitLevel(pct), 350));
    }
    function onDimUp() {
      dimDragging = false;
      window.removeEventListener('pointermove', onDimMove);
      window.removeEventListener('pointerup', onDimUp);
    }
    bar.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      dimDragging = true;
      const pct = pctFromPointer(ev);
      fill.style.width = pct + '%';
      const prev = ROOM_STATE.pendingDim.get(room.id);
      if (prev) clearTimeout(prev);
      ROOM_STATE.pendingDim.set(room.id, setTimeout(() => commitLevel(pct), 350));
      window.addEventListener('pointermove', onDimMove);
      window.addEventListener('pointerup', onDimUp);
    });

    // Click-expand drawer (Phase 2I will fill the body with per-device controls).
    card.addEventListener('click', () => openRoomDrawer(room));

    return card;
  }

  function openRoomDrawer(room) {
    const root = $('sh-drawer-root');
    const title = $('sh-drawer-title');
    const body = $('sh-drawer-body');
    if (!root || !title || !body) return;
    title.textContent = room.name || 'Room';
    const list = (ROOM_STATE.devicesByRoom.get(room.id) || []);
    if (!list.length) {
      body.innerHTML =
        '<p style="color:var(--sh-text-faint)">No devices in this room. ' +
        'Assign devices to a room from the device list (Phase 2G follow-up wires the per-device controls inside this drawer).</p>';
    } else {
      body.innerHTML = '';
      list.forEach((d) => {
        const row = document.createElement('div');
        row.style.cssText = 'display:flex;align-items:center;gap:10px;padding:8px 0;border-bottom:1px solid rgba(255,255,255,0.06);';
        const name = document.createElement('span');
        name.style.cssText = 'flex:1;color:var(--sh-text);';
        name.textContent = d.name || '(unnamed)';
        const kind = document.createElement('span');
        kind.style.cssText = 'font-size:11px;color:var(--sh-text-faint);text-transform:uppercase;letter-spacing:1px;';
        kind.textContent = d.kind || '';
        row.appendChild(name);
        row.appendChild(kind);
        body.appendChild(row);
      });
    }
    root.hidden = false;
  }

  async function loadRoomCards() {
    const wrap = $('sh-rooms');
    if (!wrap) return;
    try {
      const [roomsResp, devsResp] = await Promise.all([
        shFetch('/api/smart-home/rooms'),
        shFetch('/api/smart-home/devices'),
      ]);
      const rooms = (roomsResp && roomsResp.rooms) || [];
      const devices = (devsResp && devsResp.devices) || [];
      ROOM_STATE.rooms = rooms;
      ROOM_STATE.devicesByRoom = new Map();
      devices.forEach((d) => {
        if (d.room_id == null) return;
        const arr = ROOM_STATE.devicesByRoom.get(d.room_id) || [];
        arr.push(d);
        ROOM_STATE.devicesByRoom.set(d.room_id, arr);
      });

      wrap.innerHTML = '';
      if (!rooms.length) {
        const empty = document.createElement('div');
        empty.className = 'sh-room-card sh-room-skeleton';
        empty.textContent = 'No rooms yet. Add rooms in Settings → Smart Home.';
        wrap.appendChild(empty);
        return;
      }
      rooms.forEach((r) => wrap.appendChild(renderRoomCard(r)));
    } catch (e) {
      wrap.innerHTML =
        '<div class="sh-room-card sh-room-skeleton">' +
        'Couldn’t load rooms: ' + (e.message || e) +
        '</div>';
    }
  }
  loadRoomCards();
  setInterval(loadRoomCards, 30000);

  // ── climate ring (Nexia today; multi-driver later) ─
  // The arc covers 270° (gap at bottom). At local 0° (which the
  // wrapping `transform: rotate(-90deg)` maps to 12 o'clock visually,
  // i.e. straight up), the knob sits at the top. The track is rotated
  // by 45° so the gap centers on the bottom — meaning the visible arc
  // starts at 7:30 (135° on the clock face) and wraps clockwise to
  // 4:30 (-135° / 225°). We map the user's setpoint range linearly
  // onto that arc.
  const RING = {
    minF: 50, maxF: 90,            // visible setpoint range, °F
    arcDeg: 270,                   // sweep
    arcStartCwFromTop: 135,        // visible start (clockwise from 12 o'clock)
    radius: 88,
    cx: 100, cy: 100,
    circumference: 2 * Math.PI * 88,
    state: {
      zone: null,
      mode: 'OFF',
      heat: null, cool: null,
      currentSetpoint: null,
      currentTemp: null,
      scale: 'F',
      pollTimer: null,
      pendingSet: null,
      dragging: false,
    },
  };

  function setpointToFraction(sp) {
    const f = (sp - RING.minF) / (RING.maxF - RING.minF);
    return Math.max(0, Math.min(1, f));
  }
  function fractionToAngleCw(f) {
    // 0 → 135° clockwise from top. 1 → 135° + 270° = 405° (≡ 45°).
    return RING.arcStartCwFromTop + f * RING.arcDeg;
  }
  // Knob position in the SVG's local frame. The SVG itself is wrapped
  // in `transform: rotate(-90deg)`, so to put the knob at clock-face
  // angle θ we map (cx + r·cos(θ-90°), cy + r·sin(θ-90°)).
  function knobXY(angleCwFromTop) {
    const rad = ((angleCwFromTop - 90) * Math.PI) / 180;
    return {
      x: RING.cx + RING.radius * Math.cos(rad),
      y: RING.cy + RING.radius * Math.sin(rad),
    };
  }

  function renderRing() {
    const fill = $('sh-ring-fill');
    const knob = $('sh-ring-knob');
    const setpointEl = $('sh-ring-setpoint');
    const currentEl = $('sh-ring-current');
    if (!fill || !knob || !setpointEl || !currentEl) return;

    const sp = RING.state.currentSetpoint;
    if (sp == null) {
      setpointEl.innerHTML = '—<span class="sh-ring-deg">°</span>';
      currentEl.textContent = '—°';
      fill.setAttribute('stroke-dasharray', '0 ' + RING.circumference.toFixed(2));
      const top = knobXY(RING.arcStartCwFromTop);
      knob.setAttribute('cx', top.x.toFixed(2));
      knob.setAttribute('cy', top.y.toFixed(2));
      return;
    }
    const frac = setpointToFraction(sp);
    const dashLen = (RING.circumference * RING.arcDeg / 360) * frac;
    fill.setAttribute(
      'stroke-dasharray',
      dashLen.toFixed(2) + ' ' + RING.circumference.toFixed(2)
    );
    const angle = fractionToAngleCw(frac);
    const xy = knobXY(angle);
    knob.setAttribute('cx', xy.x.toFixed(2));
    knob.setAttribute('cy', xy.y.toFixed(2));
    setpointEl.innerHTML =
      Math.round(sp) + '<span class="sh-ring-deg">°</span>';
    if (RING.state.currentTemp != null) {
      currentEl.textContent = Math.round(RING.state.currentTemp) + '°';
    }
  }

  function updateModePill() {
    const pill = $('sh-climate-mode');
    if (!pill) return;
    const m = (RING.state.mode || '').toUpperCase();
    let label = 'Off', color = 'var(--sh-text-faint)';
    if (m === 'COOL') { label = 'Cooling'; color = 'var(--sh-accent-cool)'; }
    else if (m === 'HEAT') { label = 'Heating'; color = 'var(--sh-accent-warm)'; }
    else if (m === 'AUTO') { label = 'Auto'; color = 'var(--sh-accent-cyan)'; }
    pill.innerHTML = '<span class="sh-dot"></span>' + label;
    pill.style.color = color;
    pill.style.borderColor = 'rgba(98, 168, 255, 0.22)';
    document.querySelectorAll('.sh-ctrl-btn').forEach((b) => {
      b.classList.toggle('sh-active', (b.dataset.mode || '').toUpperCase() === m);
    });
  }

  async function loadClimate() {
    try {
      const data = await shFetch('/api/smart-home/nexia/thermostats');
      const list = (data && data.thermostats) || [];
      if (!list.length) return;
      const t = list[0];
      const z = t.zone || {};
      RING.state.zone = z.id || null;
      RING.state.mode = (t.mode || 'OFF').toUpperCase();
      RING.state.heat = z.heat_setpoint;
      RING.state.cool = z.cool_setpoint;
      RING.state.currentTemp = z.temperature;
      RING.state.scale = z.scale || 'F';
      // Outdoor temp + humidity bubble up if Nexia returns them.
      const out = $('sh-env-outdoor');
      const hum = $('sh-env-humidity');
      if (out && t.outdoor_temperature != null) out.textContent = Math.round(t.outdoor_temperature) + '°';
      if (hum && t.indoor_humidity != null) hum.textContent = Math.round(t.indoor_humidity) + '%';
      // Don't fight the user — if they're mid-drag, skip the
      // setpoint snap-back. The poll's job is to surface drift in
      // current temp + mode + chip data, not to overrule pending
      // input.
      if (!RING.state.dragging) {
        // Pick which setpoint to track on the ring based on mode.
        // AUTO shows cool (the Nexia UX convention is to drag cool
        // then auto-shift heat — out of scope for v1; a long-press
        // toggle between heat/cool is the v1.1 plan).
        RING.state.currentSetpoint =
          RING.state.mode === 'HEAT' ? z.heat_setpoint : z.cool_setpoint;
        renderRing();
      } else {
        // Still update the "Currently —°" subline + env chips —
        // those don't conflict with the drag.
        const cur = $('sh-ring-current');
        if (cur && z.temperature != null) cur.textContent = Math.round(z.temperature) + '°';
      }
      updateModePill();
    } catch (e) {
      // 424 = no creds yet. Quietly leave the ring in placeholder state.
      console.info('[smart-home] climate load skipped:', e.message || e);
    }
  }

  // Poll every 30s for current temp drift. Drag-set updates fire
  // their own optimistic render so the user doesn't wait on the loop.
  function startClimatePoll() {
    if (RING.state.pollTimer) clearInterval(RING.state.pollTimer);
    RING.state.pollTimer = setInterval(loadClimate, 30000);
  }
  loadClimate().then(startClimatePoll);

  // Drag handler.
  function pointerToSetpoint(ev, ringSvg) {
    const rect = ringSvg.getBoundingClientRect();
    const cx = rect.left + rect.width / 2;
    const cy = rect.top + rect.height / 2;
    const dx = ev.clientX - cx;
    const dy = ev.clientY - cy;
    // atan2: 0 = 3 o'clock, π/2 = 6 o'clock. Convert to clockwise-from-top.
    let deg = (Math.atan2(dy, dx) * 180) / Math.PI + 90;
    if (deg < 0) deg += 360;
    // Map the visible arc (135°..405°) to fraction 0..1.
    const start = RING.arcStartCwFromTop;
    const end = start + RING.arcDeg;
    let f;
    if (deg >= start && deg <= end) {
      f = (deg - start) / RING.arcDeg;
    } else if (deg < start) {
      f = (deg + 360 - start) / RING.arcDeg;
      if (f > 1) {
        // Inside the bottom gap — clamp to nearer end.
        f = (deg + 180) / 180 < 1 ? 0 : 1;
      }
    } else {
      f = 1;
    }
    f = Math.max(0, Math.min(1, f));
    const sp = RING.minF + f * (RING.maxF - RING.minF);
    return Math.round(sp);
  }

  function bindRingDrag() {
    const knob = $('sh-ring-knob');
    const wrap = document.querySelector('.sh-ring-wrap svg');
    if (!knob || !wrap) return;
    let dragging = false;
    function onMove(ev) {
      if (!dragging) return;
      const sp = pointerToSetpoint(ev, wrap);
      RING.state.currentSetpoint = sp;
      renderRing();
      // Debounced POST.
      if (RING.state.pendingSet) clearTimeout(RING.state.pendingSet);
      RING.state.pendingSet = setTimeout(() => commitSetpoint(sp), 400);
    }
    function onUp() {
      if (!dragging) return;
      dragging = false;
      knob.classList.remove('sh-dragging');
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    }
    function onUpFinal() {
      onUp();
      RING.state.dragging = false;
    }
    knob.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      if (RING.state.zone == null) {
        console.info('[smart-home] climate: no thermostat configured');
        return;
      }
      dragging = true;
      RING.state.dragging = true;
      knob.classList.add('sh-dragging');
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUpFinal);
    });
  }
  bindRingDrag();

  async function commitSetpoint(sp) {
    if (RING.state.zone == null) return;
    const body = { zone_id: RING.state.zone };
    if (RING.state.mode === 'HEAT') body.heat = sp;
    else body.cool = sp;
    try {
      await shFetch('/api/smart-home/nexia/setpoint', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
      });
    } catch (e) {
      console.warn('[smart-home] setpoint POST failed:', e.message || e);
    }
  }

  // Mode buttons.
  document.querySelectorAll('.sh-ctrl-btn').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const mode = (btn.dataset.mode || '').toUpperCase();
      if (!mode || RING.state.zone == null) return;
      RING.state.mode = mode;
      updateModePill();
      // Re-pick which setpoint the ring drives now.
      RING.state.currentSetpoint =
        mode === 'HEAT' ? RING.state.heat : RING.state.cool;
      renderRing();
      try {
        await shFetch('/api/smart-home/nexia/mode', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ zone_id: RING.state.zone, mode }),
        });
      } catch (e) {
        console.warn('[smart-home] mode POST failed:', e.message || e);
      }
    });
  });

  // ── live tiles (Lights / Security / Energy) ────────
  const SECURITY_KINDS = ['sensor_motion', 'sensor_contact', 'sensor_climate', 'sensor_smoke', 'sensor_water'];
  const LIGHT_KINDS = ['light', 'switch'];

  function isOn(state) {
    if (!state) return false;
    if (typeof state.on === 'boolean') return state.on;
    // Some drivers report `on_off` instead.
    if (typeof state.on_off === 'boolean') return state.on_off;
    return false;
  }

  function parseState(dev) {
    if (!dev || !dev.state_json) return {};
    try { return JSON.parse(dev.state_json); } catch (_) { return {}; }
  }

  async function loadTiles() {
    const lightsP = $('sh-lights-primary');
    const lightsS = $('sh-lights-secondary');
    const secP = $('sh-security-primary');
    const secS = $('sh-security-secondary');
    const energyP = $('sh-energy-primary');
    const energyS = $('sh-energy-secondary');

    let devices = [];
    try {
      const data = await shFetch('/api/smart-home/devices');
      devices = (data && data.devices) || [];
    } catch (e) {
      if (lightsP) lightsP.textContent = '—';
      console.warn('[smart-home] devices fetch failed:', e.message || e);
      return;
    }

    // Lights tile.
    const lights = devices.filter((d) => LIGHT_KINDS.includes(d.kind));
    const lightsOn = lights.filter((d) => isOn(parseState(d))).length;
    const lightsOff = lights.length - lightsOn;
    if (lightsP) {
      lightsP.innerHTML = lightsOn +
        '<span class="sh-tile-unit">on • ' + lightsOff + ' off</span>';
    }
    if (lightsS) {
      const rooms = new Set(
        lights.filter((d) => isOn(parseState(d)))
              .map((d) => d.room_id)
              .filter((id) => id != null)
      );
      lightsS.textContent = lights.length === 0
        ? 'No lights yet'
        : rooms.size + (rooms.size === 1 ? ' room active' : ' rooms active');
    }

    // Security tile.
    const sensors = devices.filter((d) => SECURITY_KINDS.includes(d.kind));
    const locks = devices.filter((d) => d.kind === 'lock');
    let issuesCount = 0;
    try {
      const diag = await shFetch('/api/smart-home/diagnostics/summary');
      issuesCount = ((diag && diag.active_issues) || []).filter((i) => i.kind !== 'offline').length;
    } catch (_) { /* leave at 0 — no diagnostics yet */ }
    if (secP) secP.textContent = issuesCount === 0 ? 'All Secure' : issuesCount + ' alert' + (issuesCount === 1 ? '' : 's');
    if (secS) {
      const parts = [];
      if (sensors.length) parts.push(sensors.length + ' sensor' + (sensors.length === 1 ? '' : 's'));
      if (locks.length) parts.push(locks.length + ' lock' + (locks.length === 1 ? '' : 's'));
      secS.textContent = parts.length ? parts.join(' • ') : 'No sensors yet';
    }
  }

  async function loadEnergy() {
    const energyP = $('sh-energy-primary');
    const energyS = $('sh-energy-secondary');
    const spark = $('sh-energy-spark');
    try {
      const data = await shFetch('/api/smart-home/energy/summary');
      const entries = (data && data.devices) || [];
      const totalW = entries.reduce(
        (acc, d) => acc + (typeof d.current_watts === 'number' ? d.current_watts : 0),
        0
      );
      const todayKwh = (data && typeof data.today_kwh === 'number') ? data.today_kwh : 0;
      if (energyP) {
        const kw = (totalW / 1000).toFixed(2);
        energyP.innerHTML = kw + '<span class="sh-tile-unit">kW now</span>';
      }
      if (energyS) {
        energyS.textContent = todayKwh.toFixed(1) + ' kWh today';
      }
      // Bar sparkline from per-device current_watts. Up to 17 bars
      // (mockup count). Padded with low bars when device count <17.
      if (spark) {
        const watts = entries
          .map((d) => (typeof d.current_watts === 'number' ? d.current_watts : 0))
          .filter((w) => w > 0)
          .sort((a, b) => b - a)
          .slice(0, 17);
        while (watts.length < 17) watts.push(0);
        const peak = Math.max(1, ...watts);
        let svg = '';
        watts.forEach((w, i) => {
          const x = i * 6;
          const h = Math.max(2, Math.round((w / peak) * 24));
          const y = 32 - h - 2;
          svg += '<rect x="' + x + '" y="' + y + '" width="3" height="' + h + '" rx="1"/>';
        });
        spark.innerHTML = svg;
      }
    } catch (e) {
      if (energyP) energyP.textContent = '—';
      if (energyS) energyS.textContent = 'Energy data unavailable';
      console.info('[smart-home] energy load skipped:', e.message || e);
    }
  }

  loadTiles();
  loadEnergy();
  setInterval(loadTiles, 30000);
  setInterval(loadEnergy, 30000);

  // ── drawer (placeholder bodies for tiles) ──────────
  const DRAWER_BODIES = {
    lights:    'Lights drawer is a follow-up. It will show every room × every bulb with capability-aware controls.',
    security:  'Security drawer is a follow-up. It will show sensors, alarm armed state, and recent events.',
    energy:    'Energy drawer lands in Phase 2I. It will show a calendar heatmap, per-day hourly bars, and a per-device leaderboard.',
    'rooms-all': 'Full rooms management lands in Phase 2G. For now use Settings → Smart Home → Rooms.',
  };
  function openDrawer(key) {
    const root = $('sh-drawer-root');
    const title = $('sh-drawer-title');
    const body = $('sh-drawer-body');
    if (!root || !title || !body) return;
    title.textContent = ({
      lights: 'Lights', security: 'Security', energy: 'Energy',
      'rooms-all': 'All rooms',
    })[key] || 'Detail';
    if (key === 'energy') {
      renderEnergyDrawer(body);
    } else {
      body.textContent = DRAWER_BODIES[key] || 'Coming soon.';
    }
    root.hidden = false;
  }
  function closeDrawer() {
    const root = $('sh-drawer-root');
    if (root) root.hidden = true;
  }
  document.querySelectorAll('[data-drawer]').forEach((el) => {
    el.addEventListener('click', (ev) => {
      ev.preventDefault();
      openDrawer(el.dataset.drawer);
    });
  });
  document.querySelectorAll('[data-drawer-close]').forEach((el) => {
    el.addEventListener('click', closeDrawer);
  });
  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') closeDrawer();
  });
})();
"#;
