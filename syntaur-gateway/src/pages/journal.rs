//! /journal — Mushi's room.
//!
//! Two-pane sidebar layout (framework §3). Left: timeline + chip nav.
//! Main: one of {Today, Timeline, Moments, Training, Settings}. Right:
//! collapsed-by-default Mushi chat rail backed by /api/message with
//! agent=journal so the conversation stays inside journal isolation.
//!
//! Palette: tea-house at dusk — warm amber + cream on near-black.
//! Typography: Crimson Text for entries (distinct from Cortex's EB
//! Garamond). Persona voice: warmth 9, proactivity 2, advice 0 — "Take
//! your time." is a valid response.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Journal",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (top_bar())
        (chip_bar())
        div class="j-shell" {
            (left_rail())
            (main_pane())
            (mushi_rail())
        }
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

// ── Top bar — paper-and-tea ──────────────────────────────────────────

fn top_bar() -> Markup {
    html! {
        div class="j-topbar" {
            div class="j-topbar-inner" {
                div class="flex items-center gap-3 min-w-0" {
                    a href="/" class="flex items-center gap-2 hover:opacity-80 flex-shrink-0" {
                        img src="/app-icon.jpg" class="h-8 w-8 rounded" alt="";
                        span class="j-brand" { "Syntaur" }
                    }
                    span class="j-leaf" aria-hidden="true" { "❧" }
                    span class="j-section" { "Journal" }
                    span class="j-subtle" { "· Take your time." }
                }
                div class="flex items-center gap-4 text-sm" {
                    button id="j-mushi-toggle" class="j-link" onclick="toggleMushi()" title="Open a window to Mushi" {
                        "Talk with Mushi"
                    }
                    a href="/" class="j-link" { "Home" }
                    a href="/voice-setup" class="j-link" { "Voice Setup" }
                }
            }
        }
    }
}

// ── Chip tab bar ─────────────────────────────────────────────────────

fn chip_bar() -> Markup {
    html! {
        div class="j-chipbar" {
            button id="j-chip-today"    class="j-chip active" onclick="showTab('today')"    { "Today" }
            button id="j-chip-timeline" class="j-chip"        onclick="showTab('timeline')" { "Timeline" }
            button id="j-chip-moments"  class="j-chip"        onclick="showTab('moments')"  { "Moments" }
            button id="j-chip-training" class="j-chip"        onclick="showTab('training')" { "Training" }
            button id="j-chip-settings" class="j-chip"        onclick="showTab('settings')" { "Settings" }
        }
    }
}

// ── Left rail: timeline navigator ────────────────────────────────────

fn left_rail() -> Markup {
    html! {
        aside class="j-rail" {
            section class="j-rail-section" {
                div class="j-rail-eyebrow" { "This week" }
                div class="j-weekstrip" id="j-weekstrip" {
                    // 7 dots rendered by JS (Mon..Sun in user's week)
                }
            }
            section class="j-rail-section" {
                div class="j-rail-eyebrow" { "Calendar" }
                div class="j-month" id="j-month" {}
                div class="j-month-nav" {
                    button onclick="monthShift(-1)" class="j-link j-small" { "←" }
                    span id="j-month-label" class="j-month-label" { "" }
                    button onclick="monthShift(1)" class="j-link j-small" { "→" }
                }
            }
            section class="j-rail-section" {
                div class="j-rail-eyebrow" { "Recent days" }
                div id="j-recent" class="j-recent" {}
            }
            section class="j-rail-section j-rail-footer" {
                div class="j-search-row" {
                    input type="text" id="j-search" placeholder="search the archive…"
                        class="j-input"
                        onkeydown="if(event.key==='Enter')doSearch()";
                }
                div id="j-search-note" class="j-subtle j-small" { "" }
            }
        }
    }
}

// ── Main pane: five views stacked, JS toggles .hidden ────────────────

fn main_pane() -> Markup {
    html! {
        main class="j-main" id="j-main" {
            // ── Today ────────────────────────────────────────────────
            section id="j-view-today" class="j-view" {
                header class="j-dayhead" {
                    div {
                        div class="j-eyebrow" { "today" }
                        h1 class="j-daytitle" id="j-daytitle" { "" }
                        p class="j-dayprompt" id="j-dayprompt" { "What is here today?" }
                    }
                    div class="j-daynav" {
                        button onclick="navDate(-1)" class="j-btn-ghost" { "← Previous" }
                        input type="date" id="j-date" class="j-input j-date-input" onchange="loadDay(this.value)";
                        button onclick="navDate(1)" class="j-btn-ghost" { "Next →" }
                    }
                }
                div id="j-entries" class="j-entries" {
                    div class="j-empty j-subtle" { "…" }
                }
                // Search results render here when a query is active
                div id="j-search-results" class="j-entries hidden" {}
            }

            // ── Timeline ─────────────────────────────────────────────
            section id="j-view-timeline" class="j-view hidden" {
                header class="j-dayhead" {
                    div {
                        div class="j-eyebrow" { "timeline" }
                        h1 class="j-daytitle" { "A year in pages" }
                        p class="j-dayprompt" { "Each dot is a day you made room to notice." }
                    }
                }
                div class="j-year" id="j-year" {}
                div class="j-timeline-stats" {
                    div class="j-medallion" {
                        div class="j-medallion-label" { "days recorded" }
                        div class="j-medallion-value" id="j-stat-days" { "·" }
                    }
                    div class="j-medallion" {
                        div class="j-medallion-label" { "hours captured" }
                        div class="j-medallion-value" id="j-stat-hours" { "·" }
                    }
                    div class="j-medallion" {
                        div class="j-medallion-label" { "moments saved" }
                        div class="j-medallion-value" id="j-stat-moments" { "·" }
                    }
                }
            }

            // ── Moments ──────────────────────────────────────────────
            section id="j-view-moments" class="j-view hidden" {
                header class="j-dayhead" {
                    div {
                        div class="j-eyebrow" { "moments" }
                        h1 class="j-daytitle" { "What you have noticed" }
                        p class="j-dayprompt" { "Starred fragments, kept close." }
                    }
                }
                div id="j-moments-list" class="j-entries" {
                    div class="j-empty j-subtle" { "Nothing here yet. Open a day and tap ❋ on a line that matters." }
                }
            }

            // ── Training ─────────────────────────────────────────────
            section id="j-view-training" class="j-view hidden" {
                header class="j-dayhead" {
                    div {
                        div class="j-eyebrow" { "training" }
                        h1 class="j-daytitle" { "The voice of you" }
                        p class="j-dayprompt" { "Wake-word samples and training clips gathered from your sessions. Keep the best, remove the rest." }
                    }
                }
                div class="j-training-cols" {
                    section class="j-paper" {
                        h3 class="j-paper-title" { "Training clips" }
                        div class="j-subtle j-small" id="j-clips-count" { "…" }
                        div id="j-clips-list" class="j-clip-list" {}
                    }
                    section class="j-paper" {
                        h3 class="j-paper-title" { "Wake-word samples" }
                        div class="j-subtle j-small" id="j-wake-count" { "…" }
                        div id="j-wake-list" class="j-clip-list" {}
                    }
                }
            }

            // ── Settings ─────────────────────────────────────────────
            section id="j-view-settings" class="j-view hidden" {
                header class="j-dayhead" {
                    div {
                        div class="j-eyebrow" { "settings" }
                        h1 class="j-daytitle" { "Your journal, your terms" }
                        p class="j-dayprompt" { "Read-only view of the recording pipeline configuration. Edit in `~/.openclaw/openclaw.json`." }
                    }
                }
                div id="j-settings" class="j-settings" {
                    div class="j-empty j-subtle" { "…" }
                }
                div class="j-export-row" {
                    a href="/api/journal/export" class="j-btn-primary" { "Export everything as Markdown" }
                    a href="/voice-setup" class="j-btn-ghost" { "Voice setup guide" }
                }
            }
        }
    }
}

// ── Right rail: Mushi chat ───────────────────────────────────────────

fn mushi_rail() -> Markup {
    html! {
        aside id="j-mushi" class="j-mushi collapsed" {
            div class="j-mushi-head" {
                div class="j-mushi-sigil" { "無" }
                div class="j-mushi-ident" {
                    div class="j-mushi-name" { "Mushi" }
                    div class="j-mushi-role" { "Wise companion · isolated" }
                }
                button class="j-mushi-close" onclick="toggleMushi()" title="Step away" { "×" }
            }
            div class="j-mushi-note j-small j-subtle" {
                "What you share here stays here. Mushi does not carry this to other agents — "
                "except task text you explicitly approve."
            }
            div id="j-mushi-msgs" class="j-mushi-msgs" {
                div class="j-mushi-greeting" {
                    p { em { "Some tea?" } " Come sit a moment. What is on your mind?" }
                    div class="j-mushi-suggests" {
                        button onclick="mushiAsk('Any tasks hiding in today\\'s entries?')" { "Any tasks hiding in today?" }
                        button onclick="mushiAsk('What patterns have you noticed this week?')" { "Patterns this week?" }
                        button onclick="mushiAsk('I just want to sit with this for a moment.')" { "Sit with this" }
                    }
                }
            }
            div id="j-mushi-tasks" class="j-mushi-tasks hidden" {}
            div class="j-mushi-input-row" {
                textarea id="j-mushi-input" rows="1" placeholder="share a thought…"
                    onkeydown="if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();mushiSend()}"
                    oninput="this.style.height='auto';this.style.height=Math.min(this.scrollHeight,110)+'px'" {}
                button class="j-mushi-send" onclick="mushiSend()" title="Send" { "→" }
            }
        }
    }
}

// ── Styles ───────────────────────────────────────────────────────────

const EXTRA_STYLE: &str = r##"
  /* Crimson Text — Open Font License (free) — book-weight serif with a
     warmer, softer silhouette than EB Garamond. Used for all journal
     reading so the entries feel bookish, not terminal-like. */
  @import url('https://fonts.googleapis.com/css2?family=Crimson+Text:ital,wght@0,400;0,600;1,400;1,600&display=swap');

  /* Tea-house palette — warm amber + cream on near-black, not as heavy
     as Cortex's aged-leather, lighter touch of atmosphere. */
  :root {
    --j-bg:       #0e0a07;
    --j-bg-2:     #15100a;
    --j-surface:  #1e160e;
    --j-cream:    #ede0c4;
    --j-cream-2:  #d8c8a6;
    --j-cream-3:  #a8997a;
    --j-paper:    #f3e8cf;
    --j-paper-2:  #e6d8b8;
    --j-ink:      #1a130a;
    --j-ink-mute: #55432a;
    --j-amber:    #d8a049;
    --j-amber-soft: rgba(216,160,73,0.28);
    --j-rule:     rgba(216,160,73,0.22);
    --j-moss:     #8fbc8f;   /* matches persona-header journal accent */
  }

  body {
    background: var(--j-bg);
    color: var(--j-cream);
    font-family: 'Crimson Text', 'Iowan Old Style', Georgia, serif;
  }
  /* Faint radial warm glow — a single paper lantern. */
  body::before {
    content: '';
    position: fixed; inset: 0; z-index: 0; pointer-events: none;
    background: radial-gradient(ellipse 65% 55% at 52% 42%, rgba(216,160,73,0.06), transparent 70%);
  }

  /* ── Top bar ─────────────────────────────────────────────────── */
  .j-topbar {
    position: sticky; top: 0; z-index: 40;
    border-bottom: 1px solid var(--j-rule);
    background: linear-gradient(180deg, rgba(20,14,7,0.9), rgba(14,10,7,0.9));
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
  }
  .j-topbar-inner {
    max-width: 1400px; margin: 0 auto;
    padding: 10px 20px;
    display: flex; align-items: center; justify-content: space-between; gap: 16px;
  }
  .j-brand {
    font-family: 'Crimson Text', serif;
    font-weight: 600; font-size: 18px; letter-spacing: 0.03em;
    color: var(--j-cream);
  }
  .j-leaf { color: var(--j-amber); font-size: 16px; margin: 0 2px; }
  .j-section {
    font-family: 'Crimson Text', serif;
    font-style: italic; font-size: 17px; color: var(--j-cream-2);
    letter-spacing: 0.01em;
  }
  .j-subtle { color: var(--j-cream-3); font-style: italic; }
  .j-link {
    font-family: 'Crimson Text', serif; color: var(--j-cream-2);
    text-decoration: none; background: none; border: 0; cursor: pointer;
    font-size: 14px; padding: 4px 6px; border-radius: 3px;
    transition: color 0.15s, background 0.15s;
  }
  .j-link:hover { color: var(--j-amber); }
  #j-mushi-toggle.active { color: var(--j-amber); background: var(--j-amber-soft); }

  /* ── Chip bar ────────────────────────────────────────────────── */
  .j-chipbar {
    max-width: 1400px; margin: 0 auto;
    padding: 8px 20px 10px;
    display: flex; gap: 6px; flex-wrap: wrap;
    border-bottom: 1px solid var(--j-rule);
    position: sticky; top: 50px; z-index: 35;
    background: linear-gradient(180deg, rgba(14,10,7,0.85), rgba(14,10,7,0.85));
    backdrop-filter: blur(3px);
  }
  .j-chip {
    padding: 6px 14px;
    font-family: 'Crimson Text', serif; font-size: 14px;
    letter-spacing: 0.06em; text-transform: lowercase;
    color: var(--j-cream-3); background: transparent;
    border: 1px solid transparent; border-radius: 999px;
    cursor: pointer; transition: all 0.15s;
  }
  .j-chip:hover { color: var(--j-cream); }
  .j-chip.active {
    color: var(--j-amber); border-color: var(--j-amber-soft);
    background: rgba(216,160,73,0.08);
  }

  /* ── Shell: left rail | main | right rail ────────────────────── */
  .j-shell {
    max-width: 1400px; margin: 0 auto;
    display: grid;
    grid-template-columns: 260px 1fr 0;
    min-height: calc(100vh - 104px);
    transition: grid-template-columns 0.22s ease;
  }
  .j-shell.mushi-open {
    grid-template-columns: 260px 1fr 340px;
  }
  @media (max-width: 1100px) {
    .j-shell.mushi-open { grid-template-columns: 260px 1fr 300px; }
  }
  @media (max-width: 900px) {
    .j-shell { grid-template-columns: 1fr; }
    .j-shell.mushi-open { grid-template-columns: 1fr; }
    .j-rail { display: none; }
    .j-mushi { display: none; }
    .j-shell.mushi-open .j-mushi { display: flex; position: fixed; inset: 0; z-index: 60; width: 100%; }
  }

  /* ── Left rail ───────────────────────────────────────────────── */
  .j-rail {
    border-right: 1px solid var(--j-rule);
    padding: 18px 16px 24px;
    display: flex; flex-direction: column; gap: 18px;
    overflow-y: auto; max-height: calc(100vh - 104px);
  }
  .j-rail-section { display: flex; flex-direction: column; gap: 8px; }
  .j-rail-eyebrow {
    font-style: italic; font-size: 12px;
    color: var(--j-amber); letter-spacing: 0.08em; text-transform: lowercase;
  }
  .j-weekstrip {
    display: grid; grid-template-columns: repeat(7, 1fr); gap: 4px;
  }
  .j-week-cell {
    aspect-ratio: 1/1; border-radius: 3px;
    background: var(--j-surface); color: var(--j-cream-3);
    display: flex; align-items: center; justify-content: center;
    font-size: 10px; cursor: pointer; border: 1px solid transparent;
    transition: all 0.12s;
  }
  .j-week-cell.has { background: rgba(216,160,73,0.3); color: var(--j-cream); }
  .j-week-cell.today { border-color: var(--j-amber); }
  .j-week-cell:hover { border-color: var(--j-amber-soft); }

  .j-month {
    display: grid; grid-template-columns: repeat(7, 1fr); gap: 3px;
    font-size: 11px;
  }
  .j-month-dow {
    color: var(--j-ink-mute); text-align: center; font-size: 10px;
    font-style: italic; padding-bottom: 2px;
  }
  .j-month-cell {
    aspect-ratio: 1/1; border-radius: 3px;
    display: flex; align-items: center; justify-content: center;
    color: var(--j-cream-3); background: transparent;
    cursor: pointer; transition: all 0.12s;
  }
  .j-month-cell:hover { background: rgba(216,160,73,0.12); color: var(--j-cream); }
  .j-month-cell.has { color: var(--j-cream); font-weight: 600; }
  .j-month-cell.has::after {
    content: ''; position: absolute;
    transform: translateY(8px);
    width: 3px; height: 3px; border-radius: 50%; background: var(--j-amber);
  }
  .j-month-cell.current { background: var(--j-amber); color: var(--j-ink); font-weight: 700; }
  .j-month-cell { position: relative; }
  .j-month-cell.other { opacity: 0.25; }
  .j-month-nav {
    display: flex; align-items: center; justify-content: space-between;
    padding: 2px 4px;
  }
  .j-month-label {
    font-style: italic; font-size: 13px; color: var(--j-cream-2);
  }

  .j-recent { display: flex; flex-direction: column; gap: 4px; max-height: 180px; overflow-y: auto; }
  .j-recent-btn {
    text-align: left; padding: 5px 8px; border-radius: 3px;
    font-family: 'Crimson Text', serif; font-size: 13px;
    color: var(--j-cream-2); background: transparent; border: 0; cursor: pointer;
    transition: all 0.12s;
  }
  .j-recent-btn:hover { background: rgba(216,160,73,0.08); color: var(--j-cream); }
  .j-recent-btn.current { color: var(--j-amber); background: var(--j-amber-soft); }

  .j-rail-footer { margin-top: auto; }
  .j-input {
    width: 100%; box-sizing: border-box;
    padding: 7px 10px; border-radius: 3px;
    background: var(--j-surface); border: 1px solid var(--j-rule);
    color: var(--j-cream); font-family: 'Crimson Text', serif; font-size: 14px;
    outline: none; transition: border 0.12s;
  }
  .j-input:focus { border-color: var(--j-amber-soft); }
  .j-input::placeholder { color: var(--j-ink-mute); font-style: italic; }
  .j-search-row { display: flex; gap: 6px; }
  .j-date-input {
    font-family: 'Crimson Text', serif;
    color-scheme: dark;
  }
  .j-small { font-size: 11px; }

  /* ── Main pane ───────────────────────────────────────────────── */
  .j-main {
    padding: 24px 28px 60px;
    overflow-y: auto; max-height: calc(100vh - 104px);
  }
  .j-view { display: block; }
  .j-view.hidden { display: none !important; }

  .j-dayhead {
    display: flex; align-items: flex-start; justify-content: space-between; gap: 18px;
    margin-bottom: 18px; flex-wrap: wrap;
  }
  .j-eyebrow {
    font-style: italic; font-size: 12px;
    color: var(--j-amber); letter-spacing: 0.1em; text-transform: lowercase;
  }
  .j-daytitle {
    font-family: 'Crimson Text', serif; font-weight: 600;
    font-size: 30px; color: var(--j-cream);
    margin: 3px 0 4px; letter-spacing: 0.005em;
  }
  .j-dayprompt {
    font-family: 'Crimson Text', serif; font-style: italic;
    font-size: 16px; color: var(--j-cream-3);
    margin: 0; max-width: 38em; line-height: 1.5;
  }
  .j-daynav { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .j-date-input { max-width: 170px; }

  .j-btn-ghost {
    background: transparent; border: 1px solid var(--j-rule);
    color: var(--j-cream-2); font-family: 'Crimson Text', serif;
    font-size: 13px; padding: 6px 12px; border-radius: 3px; cursor: pointer;
    transition: all 0.15s;
  }
  .j-btn-ghost:hover { color: var(--j-amber); border-color: var(--j-amber-soft); }
  .j-btn-primary {
    background: var(--j-amber); color: var(--j-ink);
    font-family: 'Crimson Text', serif; font-weight: 600;
    font-size: 14px; padding: 8px 16px; border-radius: 3px;
    border: 0; cursor: pointer; text-decoration: none; display: inline-block;
    transition: filter 0.15s;
  }
  .j-btn-primary:hover { filter: brightness(1.1); }

  /* Entry cards — newspaper column, generous leading. */
  .j-entries { display: flex; flex-direction: column; gap: 12px; max-width: 640px; }
  .j-entry {
    background: var(--j-surface); border: 1px solid var(--j-rule);
    border-radius: 3px; padding: 14px 18px;
    position: relative;
  }
  .j-entry-meta {
    display: flex; align-items: center; gap: 10px; margin-bottom: 8px;
  }
  .j-entry-time {
    font-family: 'Crimson Text', serif; font-style: italic;
    color: var(--j-cream-3); font-size: 13px;
  }
  .j-entry-source {
    font-size: 11px; padding: 2px 8px; border-radius: 999px;
    background: var(--j-amber-soft); color: var(--j-amber);
    letter-spacing: 0.05em; text-transform: lowercase;
  }
  .j-entry-source.phone    { background: rgba(96,165,250,0.15);  color: #8fbdff; }
  .j-entry-source.wearable { background: rgba(191,139,210,0.15); color: #c9a4dc; }
  .j-entry-body {
    font-family: 'Crimson Text', serif; font-size: 16px;
    color: var(--j-cream); line-height: 1.65;
  }
  .j-entry-body mark {
    background: rgba(216,160,73,0.18); color: var(--j-amber);
    padding: 0 2px; border-radius: 2px;
  }
  .j-entry-star {
    position: absolute; top: 10px; right: 12px;
    background: transparent; border: 0; cursor: pointer;
    color: var(--j-ink-mute); font-size: 18px;
    transition: color 0.12s; padding: 4px;
  }
  .j-entry-star:hover { color: var(--j-amber); }
  .j-entry-star.starred { color: var(--j-amber); }
  .j-empty { padding: 32px 0; text-align: center; }

  mark.j-highlight {
    background: var(--j-amber-soft); color: var(--j-amber);
    padding: 0 3px; border-radius: 2px;
  }

  /* ── Timeline / year-in-pixels ───────────────────────────────── */
  .j-year {
    display: grid; grid-template-columns: repeat(53, 1fr); gap: 2px;
    max-width: 780px; margin: 12px 0 24px;
  }
  .j-year-cell {
    aspect-ratio: 1/1; border-radius: 2px;
    background: var(--j-surface); transition: background 0.12s;
    cursor: pointer;
  }
  .j-year-cell.l1 { background: rgba(216,160,73,0.18); }
  .j-year-cell.l2 { background: rgba(216,160,73,0.40); }
  .j-year-cell.l3 { background: rgba(216,160,73,0.65); }
  .j-year-cell.l4 { background: var(--j-amber); }
  .j-year-cell:hover { outline: 1px solid var(--j-amber); }
  .j-timeline-stats {
    display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px;
    max-width: 520px;
  }
  .j-medallion {
    background: var(--j-paper); color: var(--j-ink);
    padding: 14px 12px; text-align: center;
    border: 1px solid var(--j-paper-2);
    border-radius: 2px;
    box-shadow: 0 4px 8px rgba(0,0,0,0.35);
  }
  .j-medallion-label {
    font-style: italic; font-size: 11px; color: var(--j-ink-mute);
    letter-spacing: 0.08em; text-transform: lowercase;
  }
  .j-medallion-value {
    font-weight: 600; font-size: 26px; line-height: 1; margin-top: 4px;
  }

  /* ── Training ────────────────────────────────────────────────── */
  .j-training-cols {
    display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 18px; max-width: 900px;
  }
  .j-paper {
    background: var(--j-surface); border: 1px solid var(--j-rule);
    border-radius: 3px; padding: 16px;
  }
  .j-paper-title {
    font-family: 'Crimson Text', serif; font-weight: 600;
    color: var(--j-cream); font-size: 18px; margin: 0 0 2px;
  }
  .j-clip-list {
    display: flex; flex-direction: column; gap: 6px; margin-top: 10px;
    max-height: 420px; overflow-y: auto;
  }
  .j-clip {
    display: flex; align-items: center; justify-content: space-between;
    gap: 8px; padding: 6px 10px;
    background: rgba(30,22,14,0.55); border-radius: 3px;
  }
  .j-clip-name {
    font-family: 'Crimson Text', serif; font-size: 13px;
    color: var(--j-cream-2); overflow: hidden; text-overflow: ellipsis;
    white-space: nowrap; flex: 1; min-width: 0;
  }
  .j-clip-meta { color: var(--j-ink-mute); font-size: 11px; font-style: italic; }
  .j-clip-remove {
    background: transparent; border: 1px solid transparent;
    color: var(--j-cream-3); font-family: 'Crimson Text', serif;
    font-size: 12px; padding: 2px 8px; border-radius: 3px; cursor: pointer;
  }
  .j-clip-remove:hover { color: #e5866a; border-color: rgba(229,134,106,0.4); }

  /* ── Settings ────────────────────────────────────────────────── */
  .j-settings {
    background: var(--j-surface); border: 1px solid var(--j-rule);
    border-radius: 3px; padding: 16px 20px; margin-bottom: 16px;
    max-width: 640px;
  }
  .j-setting-row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 0; border-bottom: 1px solid var(--j-rule);
    gap: 12px;
  }
  .j-setting-row:last-child { border-bottom: 0; }
  .j-setting-k {
    font-family: 'Crimson Text', serif; font-style: italic;
    color: var(--j-cream-3); font-size: 13px;
  }
  .j-setting-v {
    font-family: 'Crimson Text', serif; color: var(--j-cream);
    font-size: 14px; font-variant-numeric: tabular-nums;
  }
  .j-export-row {
    display: flex; align-items: center; gap: 10px; flex-wrap: wrap;
    max-width: 640px;
  }

  /* ── Mushi rail ──────────────────────────────────────────────── */
  .j-mushi {
    display: flex; flex-direction: column;
    border-left: 1px solid var(--j-rule);
    background: linear-gradient(180deg, rgba(30,22,14,0.85), rgba(20,14,8,0.85));
    overflow: hidden;
    transition: opacity 0.22s ease;
  }
  .j-shell:not(.mushi-open) .j-mushi { opacity: 0; pointer-events: none; }
  .j-mushi-head {
    display: flex; align-items: center; gap: 10px;
    padding: 14px 14px 10px; border-bottom: 1px solid var(--j-rule);
  }
  .j-mushi-sigil {
    width: 32px; height: 32px;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--j-amber); color: var(--j-amber);
    font-family: 'Crimson Text', serif; font-size: 18px;
    border-radius: 50%;
  }
  .j-mushi-ident { flex: 1; min-width: 0; }
  .j-mushi-name {
    font-family: 'Crimson Text', serif; font-weight: 600;
    color: var(--j-cream); font-size: 15px;
  }
  .j-mushi-role {
    font-style: italic; font-size: 11px; color: var(--j-cream-3);
    letter-spacing: 0.04em;
  }
  .j-mushi-close {
    background: transparent; border: 0; color: var(--j-cream-3);
    font-size: 22px; cursor: pointer; padding: 0 4px;
    transition: color 0.12s;
  }
  .j-mushi-close:hover { color: var(--j-amber); }
  .j-mushi-note {
    padding: 8px 14px; border-bottom: 1px solid var(--j-rule);
    line-height: 1.45;
  }
  .j-mushi-msgs {
    flex: 1; overflow-y: auto; padding: 14px;
    display: flex; flex-direction: column; gap: 10px;
  }
  .j-mushi-greeting p { margin: 0; line-height: 1.55; color: var(--j-cream-2); }
  .j-mushi-suggests {
    margin-top: 12px; display: flex; flex-direction: column; gap: 5px;
  }
  .j-mushi-suggests button {
    background: transparent; border: 1px solid var(--j-rule);
    color: var(--j-cream-2); font-family: 'Crimson Text', serif;
    font-style: italic; font-size: 13px; padding: 6px 10px;
    border-radius: 3px; cursor: pointer; text-align: left;
    transition: all 0.15s;
  }
  .j-mushi-suggests button:hover { color: var(--j-amber); border-color: var(--j-amber-soft); }
  .j-mushi-msg {
    max-width: 90%; padding: 8px 12px; border-radius: 12px;
    font-size: 14px; line-height: 1.5;
  }
  .j-mushi-msg.user {
    align-self: flex-end; background: var(--j-amber-soft); color: var(--j-cream);
    border-bottom-right-radius: 2px;
  }
  .j-mushi-msg.mushi {
    align-self: flex-start; background: rgba(30,22,14,0.9);
    border: 1px solid var(--j-rule); color: var(--j-cream);
    border-bottom-left-radius: 2px;
  }
  .j-mushi-msg.mushi em { color: var(--j-amber); }
  .j-mushi-tasks {
    border-top: 1px solid var(--j-rule); padding: 10px 14px;
    background: rgba(216,160,73,0.05);
  }
  .j-mushi-tasks h4 {
    font-family: 'Crimson Text', serif; font-weight: 600;
    margin: 0 0 6px; font-size: 14px; color: var(--j-cream);
  }
  .j-mushi-task {
    display: flex; align-items: flex-start; gap: 8px; padding: 4px 0;
  }
  .j-mushi-task label {
    font-family: 'Crimson Text', serif; font-size: 13px; color: var(--j-cream-2);
    line-height: 1.4; cursor: pointer; flex: 1;
  }
  .j-mushi-tasks .j-mushi-task-actions {
    display: flex; gap: 8px; margin-top: 8px;
  }
  .j-mushi-input-row {
    display: flex; gap: 6px; padding: 10px 12px;
    border-top: 1px solid var(--j-rule);
  }
  #j-mushi-input {
    flex: 1; background: var(--j-surface); border: 1px solid var(--j-rule);
    color: var(--j-cream); font-family: 'Crimson Text', serif;
    font-size: 14px; padding: 8px 10px; border-radius: 3px; resize: none;
    outline: none; max-height: 110px;
  }
  #j-mushi-input:focus { border-color: var(--j-amber-soft); }
  #j-mushi-input::placeholder { color: var(--j-ink-mute); font-style: italic; }
  .j-mushi-send {
    background: var(--j-amber); color: var(--j-ink);
    border: 0; width: 36px; border-radius: 3px; cursor: pointer;
    font-size: 18px; font-weight: 700;
  }
  .j-mushi-send:hover { filter: brightness(1.1); }

  /* Scrollbars — sepia thumbs */
  .j-main::-webkit-scrollbar, .j-rail::-webkit-scrollbar,
  .j-mushi-msgs::-webkit-scrollbar, .j-clip-list::-webkit-scrollbar,
  .j-recent::-webkit-scrollbar { width: 8px; }
  .j-main::-webkit-scrollbar-track, .j-rail::-webkit-scrollbar-track,
  .j-mushi-msgs::-webkit-scrollbar-track, .j-clip-list::-webkit-scrollbar-track,
  .j-recent::-webkit-scrollbar-track { background: transparent; }
  .j-main::-webkit-scrollbar-thumb, .j-rail::-webkit-scrollbar-thumb,
  .j-mushi-msgs::-webkit-scrollbar-thumb, .j-clip-list::-webkit-scrollbar-thumb,
  .j-recent::-webkit-scrollbar-thumb { background: var(--j-ink-mute); border-radius: 4px; }
"##;

// ── Client JS ────────────────────────────────────────────────────────

const PAGE_JS: &str = r##"
const token = localStorage.getItem('syntaur_token') || sessionStorage.getItem('syntaur_token') || '';
let currentDate = new Date().toISOString().split('T')[0];
let monthCursor = (function(){ const d = new Date(currentDate + 'T12:00:00'); return {y: d.getFullYear(), m: d.getMonth()}; })();
let availableDates = new Set();
let currentTab = 'today';
let mushiConvId = null;
let lastTurnId = null;
let searchQuery = '';

// ── Utilities ────────────────────────────────────────────────────────
function esc(s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }
function escAttr(s) { return esc(s).replace(/"/g,'&quot;'); }
function ymd(d) { return d.toISOString().split('T')[0]; }
function longDate(dateStr) {
  const d = new Date(dateStr + 'T12:00:00');
  return d.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' });
}
function shortDate(dateStr) {
  const d = new Date(dateStr + 'T12:00:00');
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}
async function api(path, opts) {
  const o = opts || {};
  o.headers = Object.assign({ 'Authorization': 'Bearer ' + token }, o.headers || {});
  if (o.body && typeof o.body === 'object' && !(o.body instanceof FormData)) {
    o.headers['Content-Type'] = 'application/json';
    o.body = JSON.stringify(o.body);
  }
  const r = await fetch(path, o);
  if (!r.ok) throw new Error('HTTP ' + r.status);
  const ct = r.headers.get('content-type') || '';
  return ct.includes('json') ? r.json() : r.text();
}

// ── Tab switching ────────────────────────────────────────────────────
function showTab(name) {
  currentTab = name;
  ['today','timeline','moments','training','settings'].forEach(t => {
    const chip = document.getElementById('j-chip-' + t);
    const view = document.getElementById('j-view-' + t);
    if (chip) chip.classList.toggle('active', t === name);
    if (view) view.classList.toggle('hidden', t !== name);
  });
  if (name === 'timeline') loadTimeline();
  else if (name === 'moments') loadMoments();
  else if (name === 'training') loadTraining();
  else if (name === 'settings') loadSettings();
}

// ── Today view ───────────────────────────────────────────────────────
async function loadDay(date) {
  currentDate = date;
  searchQuery = '';
  document.getElementById('j-date').value = date;
  document.getElementById('j-daytitle').textContent = longDate(date);
  document.getElementById('j-dayprompt').textContent = gentlePrompt(date);
  const container = document.getElementById('j-entries');
  document.getElementById('j-search-results').classList.add('hidden');
  container.classList.remove('hidden');
  container.innerHTML = '<div class="j-empty j-subtle">…</div>';
  try {
    const data = await api('/api/journal?date=' + date);
    if (!data.content) {
      container.innerHTML = emptyDayMarkup(date);
      return;
    }
    const moments = await loadMomentsForDate(date);
    const momentSet = new Set(moments.map(m => m.text));
    const lines = data.content.split(/\r?\n/);
    let html = '';
    for (const line of lines) {
      if (!line.trim() || line.startsWith('# ')) continue;
      const m = line.match(/^\*\*(\d+:\d+)\*\*\s*(\[.*?\])?\s*(.*)/);
      if (!m) continue;
      const time = m[1];
      const source = (m[2] || '').replace(/[\[\]]/g,'');
      const text = m[3];
      const sourceCls = source.toLowerCase().includes('phone') ? 'phone' :
                        source.toLowerCase().includes('wearable') ? 'wearable' : '';
      const starred = momentSet.has(text);
      html += entryCard({ time, source, sourceCls, text, starred, date });
    }
    container.innerHTML = html || emptyDayMarkup(date);
  } catch (e) {
    container.innerHTML = '<div class="j-empty j-subtle">That day is quiet.</div>';
  }
  highlightMonth();
  refreshRecent();
}

function emptyDayMarkup(date) {
  const today = ymd(new Date());
  if (date === today) {
    return '<div class="j-empty">' +
      '<p class="j-subtle" style="font-size:16px">Nothing recorded today — yet.</p>' +
      '<p class="j-subtle" style="margin-top:8px"><a href="/voice-setup" class="j-link" style="color:var(--j-amber)">Set up voice capture</a> to start.</p>' +
      '</div>';
  }
  return '<div class="j-empty j-subtle">No entries for ' + esc(longDate(date)) + '.</div>';
}

function entryCard({ time, source, sourceCls, text, starred, date }) {
  const srcTag = source ? '<span class="j-entry-source ' + sourceCls + '">' + esc(source) + '</span>' : '';
  const starCls = starred ? 'j-entry-star starred' : 'j-entry-star';
  const starGlyph = starred ? '❋' : '❋';
  return '<article class="j-entry">' +
    '<button class="' + starCls + '" title="' + (starred ? 'Unstar' : 'Keep this moment') + '" ' +
      'onclick="toggleStar(this, \'' + esc(date) + '\', \'' + esc(time) + '\', \'' + escAttr(source) + '\', \'' + escAttr(text) + '\')">' +
      starGlyph + '</button>' +
    '<div class="j-entry-meta">' +
      '<span class="j-entry-time">' + esc(time) + '</span>' + srcTag +
    '</div>' +
    '<p class="j-entry-body">' + esc(text) + '</p>' +
  '</article>';
}

function gentlePrompt(date) {
  const prompts = [
    'What is here today?',
    'Take your time.',
    'No need to gather it all at once.',
    'What has settled, and what has not?',
    'What is worth sitting with?',
    'Some days speak softly.'
  ];
  // stable-ish per-date pick
  let h = 0; for (const c of date) h = (h * 31 + c.charCodeAt(0)) | 0;
  return prompts[Math.abs(h) % prompts.length];
}

function navDate(delta) {
  const d = new Date(currentDate + 'T12:00:00');
  d.setDate(d.getDate() + delta);
  loadDay(ymd(d));
}

// ── Search ───────────────────────────────────────────────────────────
async function doSearch() {
  const q = document.getElementById('j-search').value.trim();
  if (!q) { document.getElementById('j-search-note').textContent = ''; loadDay(currentDate); return; }
  searchQuery = q;
  const results = document.getElementById('j-search-results');
  const todayPane = document.getElementById('j-entries');
  todayPane.classList.add('hidden');
  results.classList.remove('hidden');
  results.innerHTML = '<div class="j-empty j-subtle">searching…</div>';
  document.getElementById('j-search-note').textContent = 'searching "' + q + '"';
  try {
    const data = await api('/api/journal/search?q=' + encodeURIComponent(q) + '&max=10');
    if (!data.results || data.results.length === 0) {
      results.innerHTML = '<div class="j-empty j-subtle">No results for "' + esc(q) + '".</div>';
      document.getElementById('j-search-note').textContent = '0 results';
      return;
    }
    let html = '<div class="j-subtle j-small" style="margin-bottom:10px">Found in ' + data.total_days + ' day(s).</div>';
    for (const r of data.results) {
      html += '<article class="j-entry">' +
        '<div class="j-entry-meta"><button class="j-link" onclick="showTab(\'today\');loadDay(\'' + r.date + '\');">' +
        esc(longDate(r.date)) + '</button></div>';
      for (const line of r.matches.slice(0, 5)) {
        const mm = line.match(/^\*\*(\d+:\d+)\*\*\s*(\[.*?\])?\s*(.*)/);
        const body = mm ? mm[3] : line;
        const highlighted = esc(body).replace(new RegExp(esc(q), 'gi'), '<mark class="j-highlight">$&</mark>');
        html += '<p class="j-entry-body" style="font-size:14px">' + highlighted + '</p>';
      }
      if (r.count > 5) html += '<p class="j-subtle j-small">…and ' + (r.count - 5) + ' more</p>';
      html += '</article>';
    }
    results.innerHTML = html;
    document.getElementById('j-search-note').textContent = data.total_days + ' day(s)';
  } catch(e) {
    results.innerHTML = '<div class="j-empty j-subtle">Search failed.</div>';
  }
}

// ── Left rail timeline ───────────────────────────────────────────────
async function loadDates() {
  try {
    const data = await api('/api/journal/dates');
    availableDates = new Set(data.dates || []);
  } catch(e) { availableDates = new Set(); }
  refreshWeek();
  refreshMonth();
  refreshRecent();
}

function refreshWeek() {
  const el = document.getElementById('j-weekstrip');
  const today = new Date(); today.setHours(12,0,0,0);
  const dow = (today.getDay() + 6) % 7; // Mon=0
  const monday = new Date(today); monday.setDate(today.getDate() - dow);
  const labels = ['M','T','W','T','F','S','S'];
  let html = '';
  for (let i = 0; i < 7; i++) {
    const d = new Date(monday); d.setDate(monday.getDate() + i);
    const s = ymd(d);
    const has = availableDates.has(s);
    const isToday = s === ymd(new Date());
    const cls = ['j-week-cell']; if (has) cls.push('has'); if (isToday) cls.push('today');
    html += '<div class="' + cls.join(' ') + '" title="' + esc(longDate(s)) + '" onclick="loadDayFromRail(\'' + s + '\')">' + labels[i] + '</div>';
  }
  el.innerHTML = html;
}

function refreshMonth() {
  const el = document.getElementById('j-month');
  const label = document.getElementById('j-month-label');
  const y = monthCursor.y, m = monthCursor.m;
  const first = new Date(y, m, 1);
  const startDow = (first.getDay() + 6) % 7;
  const daysInMonth = new Date(y, m + 1, 0).getDate();
  const prevDays = new Date(y, m, 0).getDate();
  const dows = ['M','T','W','T','F','S','S'];
  let html = '';
  for (const d of dows) html += '<div class="j-month-dow">' + d + '</div>';
  for (let i = startDow; i > 0; i--) {
    html += '<div class="j-month-cell other">' + (prevDays - i + 1) + '</div>';
  }
  for (let d = 1; d <= daysInMonth; d++) {
    const s = ymd(new Date(y, m, d));
    const has = availableDates.has(s);
    const current = s === currentDate;
    const cls = ['j-month-cell']; if (has) cls.push('has'); if (current) cls.push('current');
    html += '<div class="' + cls.join(' ') + '" onclick="loadDayFromRail(\'' + s + '\')">' + d + '</div>';
  }
  el.innerHTML = html;
  label.textContent = new Date(y, m, 1).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
}

function monthShift(delta) {
  let nm = monthCursor.m + delta, ny = monthCursor.y;
  while (nm < 0) { nm += 12; ny -= 1; }
  while (nm > 11) { nm -= 12; ny += 1; }
  monthCursor = { y: ny, m: nm };
  refreshMonth();
}

function highlightMonth() {
  const d = new Date(currentDate + 'T12:00:00');
  if (d.getFullYear() !== monthCursor.y || d.getMonth() !== monthCursor.m) {
    monthCursor = { y: d.getFullYear(), m: d.getMonth() };
  }
  refreshMonth();
}

function refreshRecent() {
  const el = document.getElementById('j-recent');
  const list = Array.from(availableDates).sort().reverse().slice(0, 14);
  if (!list.length) { el.innerHTML = '<div class="j-subtle j-small">No days yet.</div>'; return; }
  el.innerHTML = list.map(d => {
    const cls = d === currentDate ? 'j-recent-btn current' : 'j-recent-btn';
    return '<button class="' + cls + '" onclick="loadDayFromRail(\'' + d + '\')">' + shortDate(d) + '</button>';
  }).join('');
}

function loadDayFromRail(d) {
  if (currentTab !== 'today') showTab('today');
  loadDay(d);
}

// ── Timeline view ────────────────────────────────────────────────────
async function loadTimeline() {
  const year = document.getElementById('j-year');
  // Build a 52-week grid ending today, columns = weeks, rows = days (Mon..Sun)
  const today = new Date(); today.setHours(12,0,0,0);
  const end = new Date(today);
  // normalize end to Sunday (so the column is complete)
  const endDow = (end.getDay() + 6) % 7; // Mon=0..Sun=6
  end.setDate(end.getDate() + (6 - endDow));
  const start = new Date(end); start.setDate(end.getDate() - 52*7 + 1);

  // fetch sessions to estimate intensity
  let intensityByDate = {};
  try {
    const sess = await api('/api/journal/sessions?limit=500');
    const arr = (sess.sessions || []);
    for (const s of arr) {
      const d = (s.started_at || '').substring(0, 10);
      if (!d) continue;
      intensityByDate[d] = (intensityByDate[d] || 0) + (s.duration_secs || 0);
    }
  } catch(e) {}

  let html = '';
  const cursor = new Date(start);
  while (cursor <= end) {
    const d = ymd(cursor);
    const has = availableDates.has(d);
    const secs = intensityByDate[d] || 0;
    let lvl = 0;
    if (has || secs > 0) {
      if (secs >= 3600) lvl = 4;
      else if (secs >= 1800) lvl = 3;
      else if (secs >= 600) lvl = 2;
      else lvl = 1;
    }
    const cls = 'j-year-cell' + (lvl ? ' l' + lvl : '');
    html += '<div class="' + cls + '" title="' + esc(longDate(d)) + (secs ? (' — ' + Math.round(secs/60) + ' min') : '') + '" onclick="showTab(\'today\');loadDay(\'' + d + '\');"></div>';
    cursor.setDate(cursor.getDate() + 1);
  }
  year.innerHTML = html;

  document.getElementById('j-stat-days').textContent = availableDates.size;
  try {
    const s = await api('/api/journal/sessions?limit=1000');
    document.getElementById('j-stat-hours').textContent = (s.total_duration_hours || 0).toFixed(1);
  } catch(e) { document.getElementById('j-stat-hours').textContent = '·'; }
  try {
    const m = await api('/api/journal/moments?limit=500');
    document.getElementById('j-stat-moments').textContent = m.count || 0;
  } catch(e) { document.getElementById('j-stat-moments').textContent = '·'; }
}

// ── Moments ──────────────────────────────────────────────────────────
async function loadMomentsForDate(date) {
  try {
    const data = await api('/api/journal/moments?date=' + date + '&limit=500');
    return data.moments || [];
  } catch(e) { return []; }
}

async function loadMoments() {
  const el = document.getElementById('j-moments-list');
  el.innerHTML = '<div class="j-empty j-subtle">…</div>';
  try {
    const data = await api('/api/journal/moments?limit=200');
    const moments = data.moments || [];
    if (!moments.length) {
      el.innerHTML = '<div class="j-empty j-subtle">Nothing here yet. Open a day and tap ❋ on a line that matters.</div>';
      return;
    }
    el.innerHTML = moments.map(m => {
      const src = m.source ? '<span class="j-entry-source">' + esc(m.source) + '</span>' : '';
      const time = m.time_of_day ? '<span class="j-entry-time">' + esc(m.time_of_day) + '</span>' : '';
      return '<article class="j-entry">' +
        '<button class="j-entry-star starred" title="Unstar" onclick="unstarMoment(' + m.id + ')">❋</button>' +
        '<div class="j-entry-meta">' +
        '<button class="j-link" onclick="showTab(\'today\');loadDay(\'' + m.date + '\');">' + esc(shortDate(m.date)) + '</button>' +
        time + src + '</div>' +
        '<p class="j-entry-body">' + esc(m.text) + '</p>' +
      '</article>';
    }).join('');
  } catch(e) {
    el.innerHTML = '<div class="j-empty j-subtle">Could not load moments.</div>';
  }
}

async function toggleStar(btn, date, time, source, text) {
  const isStarred = btn.classList.contains('starred');
  if (isStarred) {
    // Find the moment id for this (date, text) and delete
    try {
      const list = await loadMomentsForDate(date);
      const mm = list.find(m => m.text === text);
      if (mm) {
        await api('/api/journal/moments/' + mm.id + '?token=' + encodeURIComponent(token), { method: 'DELETE' });
        btn.classList.remove('starred');
        btn.title = 'Keep this moment';
      }
    } catch(e) {}
  } else {
    try {
      await api('/api/journal/moments', {
        method: 'POST',
        body: { token, date, text, source: source || null, time_of_day: time || null }
      });
      btn.classList.add('starred');
      btn.title = 'Unstar';
    } catch(e) {}
  }
}

async function unstarMoment(id) {
  try {
    await api('/api/journal/moments/' + id + '?token=' + encodeURIComponent(token), { method: 'DELETE' });
    loadMoments();
  } catch(e) {}
}

// ── Training ─────────────────────────────────────────────────────────
async function loadTraining() {
  document.getElementById('j-clips-list').innerHTML = '<div class="j-subtle j-small">…</div>';
  document.getElementById('j-wake-list').innerHTML = '<div class="j-subtle j-small">…</div>';
  try {
    const data = await api('/api/journal/training?limit=200');
    const render = (arr, kind) => arr.map(c => {
      const kb = (c.size_bytes / 1024).toFixed(0);
      return '<div class="j-clip">' +
        '<span class="j-clip-name" title="' + escAttr(c.name) + '">' + esc(c.name) + '</span>' +
        '<span class="j-clip-meta">' + kb + ' KB</span>' +
        '<button class="j-clip-remove" onclick="removeClip(\'' + kind + '\', \'' + escAttr(c.name) + '\')">remove</button>' +
      '</div>';
    }).join('') || '<div class="j-subtle j-small">empty</div>';
    document.getElementById('j-clips-list').innerHTML = render(data.clips || [], 'clip');
    document.getElementById('j-wake-list').innerHTML = render(data.wake_words || [], 'wake_word');
    document.getElementById('j-clips-count').textContent = (data.clip_count || 0) + ' file(s)';
    document.getElementById('j-wake-count').textContent = (data.wake_word_count || 0) + ' file(s)';
  } catch(e) {
    document.getElementById('j-clips-list').innerHTML = '<div class="j-subtle j-small">Could not load.</div>';
    document.getElementById('j-wake-list').innerHTML = '<div class="j-subtle j-small">Could not load.</div>';
  }
}

async function removeClip(kind, name) {
  if (!confirm('Remove ' + name + '?')) return;
  try {
    await api('/api/journal/training/delete', { method: 'POST', body: { token, kind, name } });
    loadTraining();
  } catch(e) { alert('Remove failed.'); }
}

// ── Settings ─────────────────────────────────────────────────────────
async function loadSettings() {
  const el = document.getElementById('j-settings');
  try {
    const s = await api('/api/journal/settings');
    el.innerHTML = [
      row('Storage path', s.storage_path),
      row('Wearable port', s.wearable_port),
      row('Wake word', s.wake_word || '(unset)'),
      row('Consent mode', s.consent_mode),
      row('Auto-cleanup days', s.auto_cleanup_days),
      row('Training clips enabled', s.training_clips ? 'yes' : 'no'),
      row('Wake-word min samples', s.wake_word_min_clips),
      row('Days recorded', s.journal_days_recorded)
    ].join('');
  } catch(e) {
    el.innerHTML = '<div class="j-subtle">Could not load settings.</div>';
  }
  function row(k, v) {
    return '<div class="j-setting-row">' +
      '<span class="j-setting-k">' + esc(k) + '</span>' +
      '<span class="j-setting-v">' + esc(String(v)) + '</span>' +
    '</div>';
  }
}

// ── Mushi chat ───────────────────────────────────────────────────────
function toggleMushi() {
  const shell = document.querySelector('.j-shell');
  const btn = document.getElementById('j-mushi-toggle');
  const open = !shell.classList.contains('mushi-open');
  shell.classList.toggle('mushi-open', open);
  btn.classList.toggle('active', open);
  if (open) setTimeout(() => { const inp = document.getElementById('j-mushi-input'); if (inp) inp.focus(); }, 220);
}

function mushiAsk(text) {
  const inp = document.getElementById('j-mushi-input');
  inp.value = text;
  mushiSend();
}

async function ensureMushiConv() {
  if (mushiConvId) return mushiConvId;
  try {
    const r = await fetch('/api/conversations', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, agent: 'journal' })
    });
    const d = await r.json();
    if (d.id) { mushiConvId = d.id; return d.id; }
  } catch(e) {}
  return null;
}

async function mushiSend() {
  const inp = document.getElementById('j-mushi-input');
  const msg = inp.value.trim();
  if (!msg) return;
  inp.value = '';
  inp.style.height = 'auto';
  appendMushiMsg('user', msg);
  const thinking = appendMushiMsg('mushi', '<em>…</em>');

  const convId = await ensureMushiConv();
  try {
    const r = await fetch('/api/message', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, message: msg, agent: 'journal', conversation_id: convId })
    });
    const d = await r.json();
    thinking.querySelector('.j-mushi-msg-body').innerHTML = d.response ? renderInline(d.response) : '<em>(no reply)</em>';

    // If user asked about tasks, try task extraction
    if (/task|todo|what do i need|hiding in/i.test(msg)) offerTaskReview(convId);
  } catch(e) {
    thinking.querySelector('.j-mushi-msg-body').innerHTML = '<em>(lost my words just now)</em>';
  }
}

function appendMushiMsg(who, html) {
  const list = document.getElementById('j-mushi-msgs');
  // hide greeting after first message
  const g = list.querySelector('.j-mushi-greeting');
  if (g) g.remove();
  const div = document.createElement('div');
  div.className = 'j-mushi-msg ' + (who === 'user' ? 'user' : 'mushi');
  div.innerHTML = '<div class="j-mushi-msg-body">' + html + '</div>';
  list.appendChild(div);
  list.scrollTop = list.scrollHeight;
  return div;
}

function renderInline(text) {
  return esc(text)
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    .replace(/\n/g, '<br/>');
}

async function offerTaskReview(convId) {
  try {
    const r = await fetch('/api/journal/extract_tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, conversation_id: convId })
    });
    const d = await r.json();
    const tasks = d.tasks || [];
    if (!tasks.length) return;
    const box = document.getElementById('j-mushi-tasks');
    box.classList.remove('hidden');
    box.innerHTML = '<h4>Tasks you might be carrying</h4>' +
      tasks.map((t, i) => {
        const text = typeof t === 'string' ? t : (t.text || JSON.stringify(t));
        return '<div class="j-mushi-task">' +
          '<input type="checkbox" id="j-mushi-task-' + i + '" checked>' +
          '<label for="j-mushi-task-' + i + '">' + esc(text) + '</label>' +
        '</div>';
      }).join('') +
      '<div class="j-mushi-task-actions">' +
        '<button class="j-btn-primary" onclick="approveTasks()">Route selected to Thaddeus</button>' +
        '<button class="j-btn-ghost" onclick="dismissTasks()">Keep private</button>' +
      '</div>';
    box._tasks = tasks;
  } catch(e) {}
}

async function approveTasks() {
  const box = document.getElementById('j-mushi-tasks');
  const tasks = box._tasks || [];
  const selected = [];
  tasks.forEach((t, i) => {
    const cb = document.getElementById('j-mushi-task-' + i);
    if (cb && cb.checked) selected.push(typeof t === 'string' ? t : (t.text || ''));
  });
  if (!selected.length) { dismissTasks(); return; }
  try {
    const r = await fetch('/api/journal/route_tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, tasks: selected })
    });
    const d = await r.json();
    box.innerHTML = '<p class="j-subtle" style="margin:0">' + (d.routed || 0) + ' sent along. The rest stayed here.</p>';
    setTimeout(() => box.classList.add('hidden'), 3500);
  } catch(e) {
    box.innerHTML = '<p class="j-subtle" style="margin:0">Could not route.</p>';
  }
}

function dismissTasks() {
  const box = document.getElementById('j-mushi-tasks');
  box.classList.add('hidden');
  box.innerHTML = '';
}

// ── Boot ─────────────────────────────────────────────────────────────
(async function init() {
  await loadDates();
  loadDay(currentDate);
})();
"##;
