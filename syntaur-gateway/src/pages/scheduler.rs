//! /scheduler — Thaddeus's module. Large-format calendar with month/week/day
//! views, custom lists, habits, theme picker, and intake rail (voice / photo
//! / email proposals + replies). Month view lands by default on desktop;
//! mobile auto-switches to Day for readability.
//!
//! This file is intentionally big — the module is the showpiece that has to
//! feel better than Artful Agenda to the user who asked for it. Every pixel
//! decision below is aimed at that comparison.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Scheduler",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (top_bar("Scheduler", None))
        // Sub-bar — view toggle + current-period label + "jump to today"
        div class="sch-subbar" {
            div class="sch-subbar-inner" {
                div class="sch-view-toggle" role="tablist" aria-label="Calendar view" {
                    button class="sch-view-btn active" data-view="month" onclick="schSwitchView('month')" { "Month" }
                    button class="sch-view-btn"        data-view="week"  onclick="schSwitchView('week')"  { "Week" }
                    button class="sch-view-btn"        data-view="day"   onclick="schSwitchView('day')"   { "Day" }
                }
                div class="sch-period" {
                    button class="sch-nav-btn" onclick="schNav(-1)" aria-label="Previous period" { "‹" }
                    span id="sch-period-label" { "—" }
                    button class="sch-nav-btn" onclick="schNav(1)"  aria-label="Next period"     { "›" }
                    button class="sch-today-btn" onclick="schGoToday()" { "Today" }
                }
                div style="flex:1" {}
                button class="sch-sub-btn" onclick="schNlCreatePrompt()" title="New event from text (⌘N)" { "＋ Event" }
                button class="sch-sub-btn" onclick="schScheduleTodos()" title="Auto-schedule todos" { "🤖 Schedule" }
                button class="sch-sub-btn" onclick="schUndo()" title="Undo (⌘Z)" { "↶ Undo" }
                button class="sch-sub-btn" onclick="schPrint()" title="Printable view" { "🖨 Print" }
                button class="sch-theme-btn" onclick="schOpenThemes()" title="Change theme" {
                    span class="sch-theme-swatch" {}
                    span { "Theme" }
                }
                button class="sch-theme-btn" onclick="schOpenBorders()" title="Change notebook frame" {
                    span class="sch-border-swatch" {}
                    span { "Frame" }
                }
            }
        }
        div class="sch-shell" {
            (left_sidebar())
            (center_canvas())
            (right_rail())
        }
        (bottom_timeline())
        (theme_picker_modal())
        (event_modal())
        (proposal_modal())
        (list_items_modal())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

// ══════════════════════════════════════════════════════════════════════
// Layout zones
// ══════════════════════════════════════════════════════════════════════

fn left_sidebar() -> Markup {
    html! {
        aside class="sch-left" aria-label="Sidebar" {
            section class="sch-left-section" {
                div class="sch-mini-cal" id="sch-mini-cal" {
                    div class="sch-mini-head" {
                        button onclick="schMiniNav(-1)" class="sch-mini-arrow" { "‹" }
                        span id="sch-mini-label" {}
                        button onclick="schMiniNav(1)" class="sch-mini-arrow" { "›" }
                    }
                    div class="sch-mini-grid" id="sch-mini-grid" {}
                }
            }

            section class="sch-left-section" {
                div class="sch-section-head" {
                    h3 { "Lists" }
                    button class="sch-small-btn" onclick="schMealSetup()" title="Set up meal planner + auto-grocery" { "🍽" }
                    button class="sch-small-btn" onclick="schNewList()" title="New list" { "+" }
                }
                ul class="sch-list-list" id="sch-lists" {
                    li class="sch-list-row sch-list-active" data-list-id="todos" onclick="schSelectList('todos')" {
                        span class="sch-list-icon" { "☐" }
                        span class="sch-list-name" { "Todos" }
                    }
                }
            }

            section class="sch-left-section" {
                div class="sch-section-head" {
                    h3 { "Habits" }
                    button class="sch-small-btn" onclick="schNewHabit()" title="New habit" { "+" }
                }
                div id="sch-habits" class="sch-habits" {}
            }

            section class="sch-left-section" {
                div class="sch-section-head" {
                    h3 { "School feeds" }
                    button class="sch-small-btn" onclick="schNewSchoolFeed()" title="Add a school ICS feed" { "+" }
                }
                div id="sch-school-feeds" class="sch-school-feeds" {
                    p class="sch-empty" { "No feeds yet." }
                }
            }

            section class="sch-left-section sch-left-footer" {
                div class="sch-legend" {
                    div class="sch-legend-title" { "Legend" }
                    div class="sch-legend-rows" id="sch-legend" {
                        div class="sch-legend-row" { span class="sch-legend-dot" style="background:#3b82f6" {} span { "Google" } }
                        div class="sch-legend-row" { span class="sch-legend-dot" style="background:#059669" {} span { "iCloud" } }
                        div class="sch-legend-row" { span class="sch-legend-dot" style="background:#6366f1" {} span { "Outlook" } }
                        div class="sch-legend-row" { span class="sch-legend-dot" style="background:#0d9488" {} span { "Teams" } }
                        div class="sch-legend-row" { span class="sch-legend-dot" style="background:var(--sch-accent)" {} span { "Manual" } }
                    }
                }
            }
        }
    }
}

fn center_canvas() -> Markup {
    html! {
        main class="sch-main" aria-label="Calendar" {
            // Month view — lands by default.
            div id="view-month" class="sch-view sch-view-active" {
                div class="sch-month-head" {
                    @for d in &["Mon","Tue","Wed","Thu","Fri","Sat","Sun"] {
                        div class="sch-month-dow" { (d) }
                    }
                }
                div class="sch-month-grid" id="sch-month-grid" {}
            }
            // Week view — hourly grid, 7 columns, the "blown up" one the user
            // asked for. Renders lazily when first selected.
            div id="view-week" class="sch-view" {
                div class="sch-week-head" id="sch-week-head" {}
                div class="sch-week-body" id="sch-week-body" {}
            }
            // Day view — single column, most spacious for mobile.
            div id="view-day" class="sch-view" {
                div class="sch-day-head" id="sch-day-head" {}
                div class="sch-day-body" id="sch-day-body" {}
            }
        }
    }
}

fn right_rail() -> Markup {
    html! {
        aside class="sch-right" aria-label="Quick add + proposals" {
            div class="sch-quickadd" {
                button class="sch-qa-btn" onclick="schVoiceAdd()" title="Dictate an event" {
                    span class="sch-qa-icon" { "🎤" }
                    span class="sch-qa-label" { "Voice" }
                }
                button class="sch-qa-btn" onclick="schPhotoAdd()" title="Snap an appointment card" {
                    span class="sch-qa-icon" { "📷" }
                    span class="sch-qa-label" { "Photo" }
                }
                button class="sch-qa-btn" onclick="schEmailAdd()" title="Scan inbox for proposals" {
                    span class="sch-qa-icon" { "✉" }
                    span class="sch-qa-label" { "Email" }
                }
                input type="file" id="sch-photo-input" accept="image/*" capture="environment" style="display:none" onchange="schPhotoSelected(this)";
            }
            div class="sch-proposals" id="sch-proposals" {
                div class="sch-proposals-head" {
                    h3 { "Thaddeus proposes" }
                    span class="sch-proposals-count" id="sch-proposals-count" { "0" }
                }
                div class="sch-proposals-list" id="sch-proposals-list" {
                    p class="sch-empty" { "Quiet for now. " span class="sch-empty-sub" { "Proposals from voice, photo, and email appear here." } }
                }
            }
            div class="sch-patterns" id="sch-patterns" {
                div class="sch-proposals-head" {
                    h3 { "Patterns" }
                    span class="sch-proposals-count" id="sch-patterns-count" { "0" }
                }
                div class="sch-patterns-list" id="sch-patterns-list" {
                    p class="sch-empty" { "Nothing noticed yet." }
                }
            }
            div class="sch-meetprep" id="sch-meetprep" {
                div class="sch-proposals-head" {
                    h3 { "Meeting prep" }
                    span class="sch-proposals-count" id="sch-meetprep-count" { "0" }
                }
                div class="sch-meetprep-list" id="sch-meetprep-list" {
                    p class="sch-empty" { "Nothing upcoming." }
                }
            }
        }
    }
}

fn bottom_timeline() -> Markup {
    html! {
        div class="sch-timeline" id="sch-timeline" {
            span class="sch-timeline-label" { "Next 48h" }
            span class="sch-timeline-sep" { "·" }
            div class="sch-timeline-items" id="sch-timeline-items" {
                span class="sch-empty" { "Nothing scheduled." }
            }
        }
    }
}

fn theme_picker_modal() -> Markup {
    html! {
        div id="sch-theme-modal" class="sch-modal" hidden {
            div class="sch-modal-box sch-theme-box" {
                div class="sch-modal-head" {
                    h2 { "Themes" }
                    button class="sch-modal-close" onclick="schCloseThemes()" { "×" }
                }
                div class="sch-theme-grid" id="sch-theme-grid" {}
            }
        }
        div id="sch-border-modal" class="sch-modal" hidden {
            div class="sch-modal-box sch-theme-box" {
                div class="sch-modal-head" {
                    h2 { "Notebook frame" }
                    button class="sch-modal-close" onclick="schCloseBorders()" { "×" }
                }
                p class="sch-border-hint" { "Dresses the calendar in a notebook-style binding. Matches the theme underneath." }
                div class="sch-theme-grid" id="sch-border-grid" {}
                div style="padding: 10px 14px 14px; display: flex; gap: 8px; align-items: center; border-top: 1px solid var(--sch-border); margin-top: 6px;" {
                    span class="sch-border-hint" style="padding: 0; flex: 1;" { "Painted backdrop not lined up? Adjust its position and zoom, then save." }
                    button class="sch-btn-primary" onclick="schEnterBackdropEdit()" { "Adjust backdrop" }
                }
            }
        }
    }
}

fn event_modal() -> Markup {
    html! {
        div id="sch-event-modal" class="sch-modal" hidden {
            div class="sch-modal-box" {
                div class="sch-modal-head" {
                    h2 id="sch-event-modal-title" { "Event" }
                    button class="sch-modal-close" onclick="schCloseEventModal()" { "×" }
                }
                div class="sch-modal-body" {
                    label { "Title" input id="ev-title" type="text" class="sch-input"; }
                    div class="sch-row-2" {
                        label { "Start" input id="ev-start" type="datetime-local" class="sch-input"; }
                        label { "End"   input id="ev-end"   type="datetime-local" class="sch-input"; }
                    }
                    label { "Location" input id="ev-loc" type="text" class="sch-input"; }
                    label { "Color"
                        button type="button" class="sch-btn-ghost" onclick="schAddJitsi()" style="width:fit-content;align-self:flex-start" { "+ Video call link" }
                        div class="sch-color-swatches" id="ev-color" {
                            // 24 curated event colors — grouped pastels + brights + deep tones.
                            @for c in &[
                                "#e57373","#ef6c00","#fbc02d","#7cb342","#26a69a","#0288d1",
                                "#5c6bc0","#7e57c2","#d81b60","#8d6e63","#546e7a","#455a64",
                                "#f8b4c4","#f6dd95","#9bbfa2","#84a98c","#b7bde8","#b8a5d8",
                                "#6366f1","#3b82f6","#059669","#e07a5f","#b98b52","#4a3426",
                            ] {
                                button type="button" class="sch-swatch" data-color=(c) style={"background:"(c)} onclick="schPickColor(this)" {}
                            }
                            input type="color" id="ev-color-custom" class="sch-swatch sch-swatch-custom" value="#84a98c" title="Pick any color" onchange="schPickCustomColor(this)";
                        }
                    }
                }
                div class="sch-modal-foot" {
                    button class="sch-btn-danger" id="ev-delete" onclick="schEventDelete()" { "Delete" }
                    button class="sch-btn-ghost"  id="ev-dup"    onclick="schEventDuplicate()" { "Duplicate" }
                    button class="sch-btn-primary" onclick="schEventSave()" { "Save" }
                }
            }
        }
    }
}

fn proposal_modal() -> Markup {
    html! {
        div id="sch-proposal-modal" class="sch-modal" hidden {
            div class="sch-modal-box" {
                div class="sch-modal-head" {
                    h2 { "Review proposal" }
                    button class="sch-modal-close" onclick="schCloseProposalModal()" { "×" }
                }
                div class="sch-modal-body" id="sch-proposal-body" {}
                div class="sch-modal-foot" id="sch-proposal-foot" {}
            }
        }
    }
}

fn list_items_modal() -> Markup {
    html! {
        div id="sch-listitems-modal" class="sch-modal" hidden {
            div class="sch-modal-box" {
                div class="sch-modal-head" {
                    h2 id="sch-listitems-title" { "List" }
                    button class="sch-modal-close" onclick="schCloseListItems()" { "×" }
                }
                div class="sch-modal-body" {
                    div id="sch-listitems-hint" class="sch-listitems-hint" hidden {}
                    ul id="sch-listitems" class="sch-listitems" {}
                    div class="sch-listitems-add" {
                        input id="sch-listitems-input" type="text" class="sch-input" placeholder="Add an item…" onkeydown="schListItemsKey(event)";
                        button id="sch-listitems-add-btn" class="sch-btn-primary" onclick="schListItemsAdd()" { "Add" }
                    }
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Styles — layout + all 8 themes as CSS variable sets
// ══════════════════════════════════════════════════════════════════════

const EXTRA_STYLE: &str = r##"
/* ══ Theme: Garden (default) ══ */
body[data-sch-theme="garden"], body:not([data-sch-theme]) {
  --sch-bg:        #f2eedc;
  --sch-paper:     #f8f4e4;
  --sch-ink:       #2e3a2c;
  --sch-ink-dim:   #5a6b56;
  --sch-ink-faint: #8a9584;
  --sch-accent:    #84a98c;
  --sch-accent-2:  #cb997e;
  --sch-border:    #d9d1b8;
  --sch-shadow:    0 1px 2px rgba(50,60,45,0.08), 0 4px 12px rgba(50,60,45,0.05);
  --sch-font-heading: 'Cormorant Garamond', 'Garamond', Georgia, serif;
  --sch-font-body:    'Inter', system-ui, sans-serif;
  --sch-watermark-url: none;
}
body[data-sch-theme="paper"] {
  --sch-bg: #ede5d0; --sch-paper: #f5edde; --sch-ink: #2c2820; --sch-ink-dim: #544c3a;
  --sch-ink-faint: #8c846e; --sch-accent: #3d5a3d; --sch-accent-2: #a87f3c;
  --sch-border: #d3c9a8; --sch-shadow: 0 1px 2px rgba(40,35,25,0.08), 0 4px 12px rgba(40,35,25,0.05);
  --sch-font-heading: 'EB Garamond', Garamond, Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="midnight"] {
  --sch-bg: #0b0e14; --sch-paper: #141920; --sch-ink: #e8e6dc; --sch-ink-dim: #a69f8e;
  --sch-ink-faint: #6b6655; --sch-accent: #d4a648; --sch-accent-2: #8c7fb0;
  --sch-border: #252b35; --sch-shadow: 0 1px 2px rgba(0,0,0,0.4), 0 4px 16px rgba(0,0,0,0.35);
  --sch-font-heading: 'Playfair Display', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="linen"] {
  --sch-bg: #f5f0e6; --sch-paper: #fbf7ec; --sch-ink: #1f2a44; --sch-ink-dim: #47536e;
  --sch-ink-faint: #8893a8; --sch-accent: #1f2a44; --sch-accent-2: #b08d57;
  --sch-border: #d7cdb6; --sch-shadow: 0 1px 2px rgba(30,35,55,0.08), 0 4px 12px rgba(30,35,55,0.05);
  --sch-font-heading: 'Fraunces', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="desert"] {
  --sch-bg: #e9ddc7; --sch-paper: #f3e8cf; --sch-ink: #4a3426; --sch-ink-dim: #7a5a47;
  --sch-ink-faint: #a68a73; --sch-accent: #b4572e; --sch-accent-2: #81603a;
  --sch-border: #d7c0a1; --sch-shadow: 0 1px 2px rgba(70,50,35,0.08), 0 4px 12px rgba(70,50,35,0.05);
  --sch-font-heading: 'Cormorant Garamond', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="stationery"] {
  --sch-bg: #f5f7fa; --sch-paper: #ffffff; --sch-ink: #17233b; --sch-ink-dim: #4e5b72;
  --sch-ink-faint: #8fa0bb; --sch-accent: #5788c7; --sch-accent-2: #17233b;
  --sch-border: #d7dee8; --sch-shadow: 0 1px 2px rgba(25,40,70,0.06), 0 4px 10px rgba(25,40,70,0.04);
  --sch-font-heading: 'Libre Caslon Text', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="winter"] {
  --sch-bg: #e6ebf0; --sch-paper: #f3f6fa; --sch-ink: #2a3544; --sch-ink-dim: #56637a;
  --sch-ink-faint: #95a1b5; --sch-accent: #5f7a96; --sch-accent-2: #b8a17f;
  --sch-border: #cbd3de; --sch-shadow: 0 1px 2px rgba(40,55,75,0.08), 0 4px 12px rgba(40,55,75,0.04);
  --sch-font-heading: 'Crimson Pro', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}
body[data-sch-theme="cafe"] {
  --sch-bg: #efe3d2; --sch-paper: #f6ebd9; --sch-ink: #3a2618; --sch-ink-dim: #6a4a34;
  --sch-ink-faint: #9a7a62; --sch-accent: #b6834a; --sch-accent-2: #5a3625;
  --sch-border: #d6c0a1; --sch-shadow: 0 1px 2px rgba(60,40,25,0.08), 0 4px 12px rgba(60,40,25,0.05);
  --sch-font-heading: 'Source Serif Pro', Georgia, serif;
  --sch-font-body: 'Inter', system-ui, sans-serif;
}

/* ══ Layout ══ */
body { background: var(--sch-bg); color: var(--sch-ink); font-family: var(--sch-font-body); }

.sch-subbar {
  border-bottom: 1px solid var(--sch-border);
  background: color-mix(in srgb, var(--sch-paper) 80%, transparent);
  position: sticky; top: 48px; z-index: 30;
}
.sch-subbar-inner {
  max-width: 1800px; margin: 0 auto; padding: 8px 16px;
  display: flex; align-items: center; gap: 14px;
}
.sch-view-toggle { display: inline-flex; border: 1px solid var(--sch-border); border-radius: 8px; overflow: hidden; }
.sch-view-btn {
  background: transparent; border: none; color: var(--sch-ink-dim);
  padding: 6px 14px; font-size: 13px; font-family: inherit; cursor: pointer;
  border-right: 1px solid var(--sch-border);
}
.sch-view-btn:last-child { border-right: none; }
.sch-view-btn.active { background: var(--sch-accent); color: var(--sch-paper); }
.sch-view-btn:not(.active):hover { background: color-mix(in srgb, var(--sch-accent) 12%, transparent); color: var(--sch-ink); }

.sch-period { display: inline-flex; align-items: center; gap: 8px; font-family: var(--sch-font-heading); font-size: 20px; color: var(--sch-ink); }
.sch-nav-btn { background: transparent; border: 1px solid var(--sch-border); border-radius: 6px; width: 28px; height: 28px; font-size: 18px; color: var(--sch-ink-dim); cursor: pointer; line-height: 1; }
.sch-nav-btn:hover { color: var(--sch-ink); border-color: var(--sch-accent); }
.sch-today-btn { background: transparent; border: 1px solid var(--sch-border); border-radius: 6px; padding: 3px 10px; font-size: 12px; color: var(--sch-ink-dim); cursor: pointer; font-family: inherit; }
.sch-today-btn:hover { color: var(--sch-ink); border-color: var(--sch-accent); }
.sch-theme-btn {
  display: inline-flex; align-items: center; gap: 8px;
  background: transparent; border: 1px solid var(--sch-border); border-radius: 6px;
  padding: 4px 10px; font-size: 12px; color: var(--sch-ink-dim); cursor: pointer; font-family: inherit;
}
.sch-theme-btn:hover { border-color: var(--sch-accent); color: var(--sch-ink); }
.sch-theme-swatch { width: 12px; height: 12px; border-radius: 3px; background: var(--sch-accent); }

.sch-shell {
  display: grid;
  grid-template-columns: 250px 1fr 280px;
  gap: 0;
  max-width: 1800px;
  margin: 0 auto;
  min-height: calc(100vh - 160px);
}
.sch-left  { border-right: 1px solid var(--sch-border); background: var(--sch-paper); padding: 14px; overflow-y: auto; }
.sch-main  { background: var(--sch-bg); padding: 14px 18px; }
.sch-right { border-left: 1px solid var(--sch-border); background: var(--sch-paper); padding: 14px; overflow-y: auto; }

.sch-left-section { margin-bottom: 20px; }
.sch-section-head { display: flex; align-items: center; gap: 6px; margin-bottom: 8px; }
.sch-section-head h3 { font-family: var(--sch-font-heading); font-size: 15px; font-weight: 500; color: var(--sch-ink); flex: 1; margin: 0; }
.sch-small-btn {
  background: transparent; border: 1px solid var(--sch-border); border-radius: 4px;
  width: 20px; height: 20px; line-height: 1; font-size: 14px; color: var(--sch-ink-dim);
  cursor: pointer; padding: 0;
}
.sch-small-btn:hover { border-color: var(--sch-accent); color: var(--sch-accent); }

/* Mini calendar */
.sch-mini-cal { font-size: 11px; }
.sch-mini-head { display: flex; align-items: center; justify-content: space-between; font-family: var(--sch-font-heading); font-size: 14px; color: var(--sch-ink); margin-bottom: 6px; }
.sch-mini-arrow { background: transparent; border: none; color: var(--sch-ink-dim); cursor: pointer; padding: 2px 6px; font-size: 13px; }
.sch-mini-arrow:hover { color: var(--sch-accent); }
.sch-mini-grid { display: grid; grid-template-columns: repeat(7, 1fr); gap: 1px; }
.sch-mini-grid .dow { text-align: center; color: var(--sch-ink-faint); font-size: 10px; padding: 2px 0; }
.sch-mini-day { text-align: center; padding: 3px 0; border-radius: 3px; cursor: pointer; color: var(--sch-ink-dim); }
.sch-mini-day:hover { background: color-mix(in srgb, var(--sch-accent) 15%, transparent); color: var(--sch-ink); }
.sch-mini-day.today { background: var(--sch-accent); color: var(--sch-paper); font-weight: 600; }
.sch-mini-day.other-month { color: var(--sch-ink-faint); opacity: 0.4; }
.sch-mini-day.has-events::after { content: ''; display: block; width: 3px; height: 3px; border-radius: 50%; background: var(--sch-accent-2); margin: 1px auto -3px; }

/* Lists — caret-expand inline; modal fallback via ↗ */
.sch-list-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.sch-list-row {
  display: flex; align-items: center; gap: 6px;
  padding: 5px 6px; border-radius: 4px; cursor: pointer;
  font-size: 13px; color: var(--sch-ink-dim); user-select: none;
}
.sch-list-row:hover { background: color-mix(in srgb, var(--sch-accent) 10%, transparent); color: var(--sch-ink); }
.sch-list-row.sch-list-active { background: color-mix(in srgb, var(--sch-accent) 15%, transparent); color: var(--sch-ink); }
.sch-list-caret {
  display: inline-flex; align-items: center; justify-content: center;
  width: 12px; height: 12px; font-size: 10px; color: var(--sch-ink-faint);
  transition: transform 0.1s ease;
}
.sch-list-row.open .sch-list-caret { color: var(--sch-accent); }
.sch-list-icon { font-size: 13px; width: 16px; text-align: center; }
.sch-list-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sch-list-open-modal {
  background: transparent; border: none; color: var(--sch-ink-faint);
  cursor: pointer; font-size: 12px; padding: 0 4px; opacity: 0;
  transition: opacity 0.1s ease;
}
.sch-list-row:hover .sch-list-open-modal { opacity: 1; }
.sch-list-open-modal:hover { color: var(--sch-accent); }
.sch-list-items {
  list-style: none; margin: 0 0 4px 22px;
  padding: 6px 8px; border-left: 2px solid var(--sch-border);
  background: color-mix(in srgb, var(--sch-paper) 70%, transparent);
  border-radius: 0 4px 4px 0;
}
.sch-list-empty { font-size: 11px; color: var(--sch-ink-faint); padding: 2px 4px 6px; font-style: italic; }
.sch-inline-items { list-style: none; margin: 0 0 6px; padding: 0; display: flex; flex-direction: column; gap: 2px; max-height: 220px; overflow-y: auto; }
.sch-inline-item {
  display: flex; align-items: center; gap: 6px;
  padding: 3px 4px; font-size: 12px;
  border-radius: 3px;
}
.sch-inline-item:hover { background: color-mix(in srgb, var(--sch-accent) 8%, transparent); }
.sch-inline-item.checked { opacity: 0.55; }
.sch-inline-item.checked .sch-inline-text { text-decoration: line-through; }
.sch-inline-text { flex: 1; color: var(--sch-ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sch-inline-add { display: flex; gap: 4px; margin-top: 4px; }
.sch-inline-add .sch-input { flex: 1; padding: 4px 7px; font-size: 12px; }
.sch-inline-add .sch-btn-primary { padding: 4px 10px; font-size: 12px; }

/* Legacy — still used by the modal */
.sch-list-list { list-style: none; padding: 0; margin: 0; }
.sch-list-row {
  display: flex; align-items: center; gap: 8px;
  padding: 6px 8px; border-radius: 4px; cursor: pointer;
  color: var(--sch-ink-dim); font-size: 13px;
}
.sch-list-row:hover { background: color-mix(in srgb, var(--sch-accent) 10%, transparent); color: var(--sch-ink); }
.sch-list-row.sch-list-active { background: color-mix(in srgb, var(--sch-accent) 20%, transparent); color: var(--sch-ink); font-weight: 500; }
.sch-list-icon { width: 16px; text-align: center; color: var(--sch-accent); }

/* Habits */
.sch-habits { display: flex; flex-direction: column; gap: 6px; }
.sch-habit-row { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--sch-ink-dim); }
.sch-habit-name { flex: 1; }
.sch-habit-dots { display: inline-flex; gap: 3px; }
.sch-habit-dot {
  width: 14px; height: 14px; border-radius: 50%;
  border: 1px solid var(--sch-border); background: transparent; cursor: pointer; padding: 0;
}
.sch-habit-dot.filled { background: var(--sch-accent); border-color: var(--sch-accent); }
.sch-habit-dot.today:not(.filled) { border-color: var(--sch-accent); }

.sch-legend { font-size: 11px; }
.sch-legend-title { font-family: var(--sch-font-heading); font-size: 13px; color: var(--sch-ink); margin-bottom: 6px; }
.sch-legend-rows { display: flex; flex-direction: column; gap: 4px; }
.sch-legend-row { display: flex; align-items: center; gap: 6px; color: var(--sch-ink-dim); }
.sch-legend-dot { width: 8px; height: 8px; border-radius: 2px; }

/* ══ Calendar views — the core surface ══ */
.sch-view { display: none; }
.sch-view.sch-view-active { display: block; }

/* Month view */
.sch-month-head {
  display: grid; grid-template-columns: repeat(7, 1fr);
  margin-bottom: 6px;
}
.sch-month-dow {
  padding: 8px 10px;
  font-family: var(--sch-font-heading);
  font-size: 14px; color: var(--sch-ink-dim); letter-spacing: 0.02em;
  border-bottom: 1px solid var(--sch-border);
}
.sch-month-grid {
  display: grid; grid-template-columns: repeat(7, 1fr);
  gap: 1px; background: var(--sch-border);
  border: 1px solid var(--sch-border); border-radius: 6px; overflow: hidden;
  min-height: calc(100vh - 260px);
}
.sch-month-cell {
  background: var(--sch-paper); padding: 6px 8px;
  min-height: 110px;
  display: flex; flex-direction: column; gap: 3px;
  cursor: pointer;
  transition: background 0.15s;
}
.sch-month-cell:hover { background: color-mix(in srgb, var(--sch-accent) 6%, var(--sch-paper)); }
.sch-month-cell.other-month { opacity: 0.45; }
.sch-month-cell.weekend { background: color-mix(in srgb, var(--sch-border) 25%, var(--sch-paper)); }
.sch-month-cell.today .sch-date-num {
  background: var(--sch-accent); color: var(--sch-paper);
  width: 22px; height: 22px; border-radius: 50%;
  display: inline-flex; align-items: center; justify-content: center;
}
.sch-date-num { font-family: var(--sch-font-heading); font-size: 13px; color: var(--sch-ink); }
.sch-event-chip {
  background: var(--chip-color, var(--sch-accent));
  color: #fff;
  padding: 2px 6px; border-radius: 3px;
  font-size: 11px; line-height: 1.3;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  box-shadow: 0 1px 1px rgba(0,0,0,0.1);
  cursor: pointer;
}
.sch-event-chip.pending { border: 1px dashed rgba(255,255,255,0.8); opacity: 0.7; }
.sch-event-overflow { font-size: 10px; color: var(--sch-ink-faint); padding-left: 4px; }

/* Week view — the "blown up" showpiece */
.sch-week-head {
  display: grid; grid-template-columns: 56px repeat(7, 1fr);
  border-bottom: 1px solid var(--sch-border);
  position: sticky; top: 96px; z-index: 10; background: var(--sch-bg);
}
.sch-week-dow { padding: 8px 10px; border-right: 1px solid var(--sch-border); }
.sch-week-dow:last-child { border-right: none; }
.sch-week-dow-name { font-size: 11px; color: var(--sch-ink-faint); text-transform: uppercase; letter-spacing: 0.08em; }
.sch-week-dow-num { font-family: var(--sch-font-heading); font-size: 22px; color: var(--sch-ink); }
.sch-week-dow.today .sch-week-dow-num { color: var(--sch-accent); font-weight: 600; }
.sch-week-body {
  display: grid; grid-template-columns: 56px repeat(7, 1fr);
  position: relative;
  background: var(--sch-paper);
  border: 1px solid var(--sch-border); border-radius: 6px; overflow: hidden;
}
.sch-week-hour-col { border-right: 1px solid var(--sch-border); }
.sch-week-hour-label {
  height: 60px; padding: 2px 8px; text-align: right;
  font-size: 11px; color: var(--sch-ink-faint);
  border-bottom: 1px solid color-mix(in srgb, var(--sch-border) 60%, transparent);
}
.sch-week-day-col { position: relative; border-right: 1px solid var(--sch-border); }
.sch-week-day-col:last-child { border-right: none; }
.sch-week-day-col.weekend { background: color-mix(in srgb, var(--sch-border) 20%, var(--sch-paper)); }
.sch-week-hour-slot { height: 60px; border-bottom: 1px solid color-mix(in srgb, var(--sch-border) 60%, transparent); cursor: pointer; }
.sch-week-hour-slot:hover { background: color-mix(in srgb, var(--sch-accent) 10%, transparent); }
.sch-week-event {
  position: absolute; left: 2px; right: 2px;
  background: var(--chip-color, var(--sch-accent));
  color: #fff; padding: 4px 6px; border-radius: 4px;
  font-size: 11px; line-height: 1.3; overflow: hidden;
  box-shadow: 0 2px 6px rgba(0,0,0,0.15);
  cursor: pointer;
}
.sch-week-event .ev-title { font-weight: 600; }
.sch-week-event .ev-time  { opacity: 0.85; font-size: 10px; }
.sch-week-now-line { position: absolute; left: 0; right: 0; height: 2px; background: #ef4444; z-index: 3; pointer-events: none; }
.sch-week-now-line::before { content: ''; position: absolute; left: -4px; top: -3px; width: 8px; height: 8px; border-radius: 50%; background: #ef4444; }
.sch-resize-handle {
  position: absolute; left: 0; right: 0; bottom: 0;
  height: 6px; cursor: ns-resize;
  background: rgba(255,255,255,0.0);
}
.sch-week-event:hover .sch-resize-handle { background: rgba(255,255,255,0.3); }
.sch-week-event { touch-action: none; }

/* Day view — single-column, mobile default */
.sch-day-head { padding: 10px 12px; border-bottom: 1px solid var(--sch-border); }
.sch-day-label { font-family: var(--sch-font-heading); font-size: 22px; color: var(--sch-ink); }
.sch-day-sub { font-size: 12px; color: var(--sch-ink-dim); margin-top: 2px; }
.sch-day-body {
  display: grid; grid-template-columns: 56px 1fr; position: relative;
  background: var(--sch-paper);
  border: 1px solid var(--sch-border); border-radius: 6px; overflow: hidden;
}

/* ══ Right rail ══ */
.sch-quickadd { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 8px; margin-bottom: 18px; }
.sch-qa-btn {
  display: flex; flex-direction: column; align-items: center; gap: 4px;
  background: var(--sch-paper); border: 1px solid var(--sch-border); border-radius: 8px;
  padding: 12px 6px; cursor: pointer; color: var(--sch-ink);
  font-family: inherit; font-size: 11px;
}
.sch-qa-btn:hover { border-color: var(--sch-accent); background: color-mix(in srgb, var(--sch-accent) 8%, var(--sch-paper)); }
.sch-qa-icon { font-size: 20px; line-height: 1; }
.sch-qa-label { color: var(--sch-ink-dim); }

.sch-proposals, .sch-patterns { margin-bottom: 18px; }
.sch-proposals-head { display: flex; align-items: center; gap: 6px; margin-bottom: 8px; }
.sch-proposals-head h3 { flex: 1; font-family: var(--sch-font-heading); font-size: 14px; color: var(--sch-ink); margin: 0; }
.sch-proposals-count {
  background: var(--sch-accent); color: var(--sch-paper);
  border-radius: 999px; padding: 1px 7px; font-size: 11px; font-weight: 600;
}
.sch-proposals-count.zero { background: var(--sch-border); color: var(--sch-ink-faint); }

.sch-proposal-card {
  background: var(--sch-paper); border: 1px solid var(--sch-border); border-radius: 8px;
  padding: 10px 12px; margin-bottom: 8px; font-size: 12px; color: var(--sch-ink);
  box-shadow: var(--sch-shadow);
}
.sch-proposal-card .p-summary { font-weight: 600; margin-bottom: 2px; }
.sch-proposal-card .p-source  { color: var(--sch-ink-faint); font-size: 11px; margin-bottom: 6px; }
.sch-proposal-card .p-actions { display: flex; gap: 6px; flex-wrap: wrap; }
.sch-pbtn {
  background: transparent; border: 1px solid var(--sch-border); border-radius: 5px;
  padding: 3px 10px; font-size: 11px; color: var(--sch-ink-dim); cursor: pointer; font-family: inherit;
}
.sch-pbtn:hover { border-color: var(--sch-accent); color: var(--sch-ink); }
.sch-pbtn.ok    { background: var(--sch-accent); color: var(--sch-paper); border-color: var(--sch-accent); }
.sch-pbtn.rej   { color: #a03030; }

.sch-empty { color: var(--sch-ink-faint); font-size: 12px; line-height: 1.5; }
.sch-empty-sub { display: block; font-size: 11px; margin-top: 2px; }

/* ══ Bottom timeline ══ */
.sch-timeline {
  border-top: 1px solid var(--sch-border); background: var(--sch-paper);
  padding: 8px 18px; font-size: 12px; color: var(--sch-ink-dim);
  display: flex; align-items: center; gap: 8px; overflow-x: auto;
}
.sch-timeline-label { font-weight: 600; color: var(--sch-ink); }
.sch-timeline-sep   { color: var(--sch-ink-faint); }
.sch-timeline-items { display: flex; gap: 12px; white-space: nowrap; }
.sch-timeline-item { color: var(--sch-ink); }
.sch-timeline-item .t-time { color: var(--sch-accent); font-weight: 600; }

/* ══ Modals ══ */
.sch-modal {
  position: fixed; inset: 0; z-index: 60;
  background: rgba(20,15,5,0.55); backdrop-filter: blur(4px);
  display: flex; align-items: flex-start; justify-content: center; padding-top: 10vh;
  cursor: pointer; /* signal: click empty space to dismiss */
}
.sch-modal[hidden] { display: none; }
.sch-modal-box {
  background: var(--sch-paper); color: var(--sch-ink);
  border: 1px solid var(--sch-border); border-radius: 12px;
  width: 100%; max-width: 480px; overflow: hidden;
  box-shadow: 0 30px 80px rgba(0,0,0,0.4);
  font-family: var(--sch-font-body);
  cursor: default;
}
.sch-theme-box { max-width: 720px; }
.sch-modal-head { padding: 14px 18px; border-bottom: 1px solid var(--sch-border); display: flex; align-items: center; }
.sch-modal-head h2 { flex: 1; margin: 0; font-family: var(--sch-font-heading); font-size: 20px; font-weight: 500; }
.sch-modal-close { background: transparent; border: none; font-size: 22px; line-height: 1; color: var(--sch-ink-dim); cursor: pointer; }
.sch-modal-body { padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; }
.sch-modal-body label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--sch-ink-dim); }
.sch-row-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.sch-input {
  background: var(--sch-bg); border: 1px solid var(--sch-border); border-radius: 6px;
  padding: 8px 10px; font-size: 13px; color: var(--sch-ink); font-family: inherit;
}
.sch-input:focus { outline: none; border-color: var(--sch-accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--sch-accent) 20%, transparent); }
.sch-color-swatches { display: grid; grid-template-columns: repeat(12, 1fr); gap: 6px; padding: 2px 0; }
.sch-swatch { width: 22px; height: 22px; border-radius: 50%; border: 2px solid transparent; cursor: pointer; padding: 0; }
.sch-swatch.selected { border-color: var(--sch-ink); box-shadow: 0 0 0 2px var(--sch-paper), 0 0 0 4px var(--sch-accent); }
.sch-swatch-custom { appearance: none; -webkit-appearance: none; background: conic-gradient(from 0deg, red, yellow, lime, cyan, blue, magenta, red); }
.sch-swatch-custom::-webkit-color-swatch-wrapper { padding: 0; }
.sch-swatch-custom::-webkit-color-swatch { border: none; border-radius: 50%; }
.sch-modal-foot { padding: 12px 18px; border-top: 1px solid var(--sch-border); display: flex; gap: 8px; justify-content: flex-end; }
.sch-btn-primary { background: var(--sch-accent); color: var(--sch-paper); border: none; border-radius: 6px; padding: 7px 14px; font-size: 13px; cursor: pointer; font-family: inherit; }
.sch-btn-ghost   { background: transparent; border: 1px solid var(--sch-border); color: var(--sch-ink-dim); border-radius: 6px; padding: 6px 12px; font-size: 13px; cursor: pointer; font-family: inherit; }
.sch-btn-ghost:hover { color: var(--sch-ink); }
.sch-btn-danger  { background: transparent; border: 1px solid #a03030; color: #a03030; border-radius: 6px; padding: 6px 12px; font-size: 13px; cursor: pointer; font-family: inherit; margin-right: auto; }

/* Theme gallery tiles */
.sch-theme-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; padding: 16px; }
.sch-theme-tile {
  border: 2px solid var(--sch-border); border-radius: 8px; overflow: hidden;
  cursor: pointer; text-align: left; background: transparent;
  padding: 0; font-family: inherit;
}
.sch-theme-tile.active { border-color: var(--sch-accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--sch-accent) 25%, transparent); }
.sch-theme-preview { aspect-ratio: 1 / 1; display: flex; align-items: center; justify-content: center; font-family: 'Cormorant Garamond', Georgia, serif; font-size: 14px; }
.sch-theme-label { padding: 6px 8px; font-size: 11px; color: var(--sch-ink-dim); text-align: center; }

/* Sub-bar extra buttons */
.sch-sub-btn {
  background: transparent; border: 1px solid var(--sch-border); border-radius: 6px;
  padding: 4px 10px; font-size: 12px; color: var(--sch-ink-dim); cursor: pointer; font-family: inherit;
}
.sch-sub-btn:hover { border-color: var(--sch-accent); color: var(--sch-ink); background: color-mix(in srgb, var(--sch-accent) 6%, transparent); }

/* Weather chip on month cells */
.sch-wx {
  position: absolute; top: 3px; right: 5px;
  font-size: 10px; color: var(--sch-ink-faint);
  display: inline-flex; align-items: center; gap: 2px;
  pointer-events: none;
}
.sch-month-cell { position: relative; }

/* Location autocomplete — dropdown sits BELOW the input, never covers it.
 * The label wrapping `#ev-loc` gets position:relative so absolute children
 * stack against it correctly. Constrained width + max-height to prevent the
 * "floods the UI" problem Sean hit. */
.sch-event-modal .sch-modal-body label:has(#ev-loc) { position: relative; }
.sch-loc-ac {
  position: absolute; z-index: 35;
  top: 100%; left: 0; right: 0;
  background: var(--sch-paper); border: 1px solid var(--sch-border); border-radius: 6px;
  box-shadow: 0 8px 24px rgba(0,0,0,0.14);
  max-height: 180px; overflow-y: auto;
  margin-top: 4px;
}
.sch-loc-row { display: block; width: 100%; text-align: left; background: transparent; border: none;
  padding: 7px 10px; font-size: 12px; color: var(--sch-ink); font-family: inherit; cursor: pointer;
  border-bottom: 1px solid var(--sch-border); white-space: normal; line-height: 1.35; }
.sch-loc-row:last-child { border-bottom: none; }
.sch-loc-row:hover { background: color-mix(in srgb, var(--sch-accent) 12%, transparent); }
.sch-loc-hint { display: block; font-size: 10px; color: var(--sch-ink-faint); padding: 4px 10px; border-top: 1px solid var(--sch-border); background: color-mix(in srgb, var(--sch-accent) 4%, transparent); }

/* Sticker grid */
.sch-sticker-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; padding: 16px; }
.sch-sticker-cell { background: transparent; border: 1px solid var(--sch-border); border-radius: 8px;
  padding: 10px; cursor: pointer; aspect-ratio: 1/1; display: flex; align-items: center; justify-content: center; }
.sch-sticker-cell:hover { border-color: var(--sch-accent); background: color-mix(in srgb, var(--sch-accent) 8%, var(--sch-paper)); }
.sch-sticker-cell svg { width: 36px; height: 36px; }

/* Toast */
#sch-toast {
  position: fixed; left: 50%; bottom: 32px; transform: translateX(-50%);
  background: var(--sch-ink); color: var(--sch-paper);
  padding: 10px 18px; border-radius: 999px; font-size: 13px;
  box-shadow: 0 10px 30px rgba(0,0,0,0.25); z-index: 100;
  display: none; max-width: 90vw;
}

/* Print mode — clean, no sidebars */
@media print {
  .sch-subbar, .sch-left, .sch-right, .sch-timeline,
  .syntaur-topbar, .sch-modal, #sch-toast { display: none !important; }
  .sch-shell { grid-template-columns: 1fr !important; }
  .sch-main { padding: 8px !important; }
  body { background: white !important; color: black !important; }
  .sch-month-cell { background: white !important; border: 1px solid #ccc !important; }
  .sch-event-chip { box-shadow: none !important; border: 1px solid currentColor !important; }
}
body.sch-print-mode .sch-left, body.sch-print-mode .sch-right, body.sch-print-mode .sch-timeline,
body.sch-print-mode .sch-subbar { display: none !important; }

/* Mobile */
@media (max-width: 900px) {
  .sch-shell { grid-template-columns: 1fr; }
  .sch-left, .sch-right { border: none; border-bottom: 1px solid var(--sch-border); max-height: 40vh; }
  .sch-month-cell { min-height: 64px; padding: 4px 5px; }
  .sch-view-btn { padding: 5px 10px; font-size: 12px; }
}

/* ══ School feeds (left sidebar) ══ */
.sch-school-feeds { display: flex; flex-direction: column; gap: 4px; }
.sch-school-feed-row {
  display: flex; align-items: center; gap: 6px;
  padding: 4px 6px; border-radius: 4px;
  font-size: 12px; color: var(--sch-ink-dim);
}
.sch-school-feed-row:hover { background: color-mix(in srgb, var(--sch-accent) 10%, transparent); }
.sch-school-feed-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
.sch-school-feed-label { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sch-school-feed-btn {
  background: transparent; border: none; color: var(--sch-ink-faint);
  cursor: pointer; padding: 0 3px; font-size: 12px;
}
.sch-school-feed-btn:hover { color: var(--sch-accent); }

/* ══ Meeting prep cards (right rail) ══ */
.sch-meetprep { margin-top: 16px; }
.sch-meetprep-list { display: flex; flex-direction: column; gap: 8px; }
.sch-meetprep-card {
  border: 1px solid var(--sch-border); border-left: 3px solid var(--sch-accent);
  border-radius: 6px; padding: 10px 12px;
  background: color-mix(in srgb, var(--sch-accent) 5%, var(--sch-paper));
}
.sch-meetprep-title { font-weight: 600; color: var(--sch-ink); font-size: 13px; line-height: 1.3; }
.sch-meetprep-when  { font-size: 11px; color: var(--sch-ink-dim); margin-top: 2px; }
.sch-meetprep-sect  { margin-top: 8px; font-size: 11px; color: var(--sch-ink-dim); }
.sch-meetprep-sect strong { color: var(--sch-ink); font-weight: 500; margin-right: 4px; }
.sch-meetprep-email { display: block; padding: 3px 0; color: var(--sch-ink); font-size: 11px; }
.sch-meetprep-email em { color: var(--sch-ink-faint); font-style: normal; }

/* ══ Notebook frames (Artful Agenda parity — the wife-pleaser) ══ */
/* All six styles paint decorative pseudo-elements around .sch-shell so
   they frame the whole scheduler regardless of which view is active.
   Each uses pure CSS + inline-SVG data URLs — zero external assets, so
   deploys stay a single binary. Swap via [data-sch-border="<key>"]. */

.sch-border-swatch {
  display: inline-block; width: 16px; height: 14px; border-radius: 2px;
  background: linear-gradient(180deg, #e8d9ba 0%, #d9c69d 100%);
  box-shadow:
    inset 0 0 0 1px rgba(0,0,0,0.15),
    -3px 0 0 0 #b5b5b5,
    -3px 0 0 1px rgba(0,0,0,0.25);
}
.sch-border-hint {
  font-size: 12px; color: var(--sch-ink-dim);
  padding: 10px 14px 0; margin: 0;
}
.sch-shell { position: relative; }

/* Frame rewrite v4 — dramatically richer geometry. Drove the v3 result
 * in Playwright and confirmed the pieces were too small + too flat.
 * Per-style changes below each write substantially more detailed SVGs:
 * full 3D coil rings (not single arcs), chunky mushroom discs with
 * highlights, bigger patterned washi with proper drop-shadow, ornate
 * gilded corners with flourishes, fuller floral bouquets, Moleskine
 * cues that are actually visible.
 */

.sch-border-swatch {
  display: inline-block; width: 16px; height: 14px; border-radius: 2px;
  background: linear-gradient(180deg, #faf6ee 0%, #f5ebe0 100%);
  box-shadow:
    inset 0 0 0 1px rgba(92,74,58,0.18),
    -3px 0 0 0 #b8b8bc,
    -3px 0 0 1px rgba(46,46,50,0.6);
}
.sch-border-hint { font-size: 12px; color: var(--sch-ink-dim); padding: 10px 14px 0; margin: 0; }
.sch-shell { position: relative; }

/* none — off */
[data-sch-border="none"] .sch-shell { background: var(--sch-bg) !important; box-shadow: none !important; padding-left: 0 !important; padding-top: 0 !important; border: none !important; }
[data-sch-border="none"] .sch-shell::before,
[data-sch-border="none"] .sch-shell::after { content: none !important; }

/* Make every decorative style show across the whole scheduler */
[data-sch-border="notebook"] .sch-left, [data-sch-border="notebook"] .sch-main, [data-sch-border="notebook"] .sch-right,
body:not([data-sch-border]) .sch-left, body:not([data-sch-border]) .sch-main, body:not([data-sch-border]) .sch-right,
[data-sch-border="disc"] .sch-left, [data-sch-border="disc"] .sch-main, [data-sch-border="disc"] .sch-right,
[data-sch-border="moleskine"] .sch-left, [data-sch-border="moleskine"] .sch-main, [data-sch-border="moleskine"] .sch-right,
[data-sch-border="washi"] .sch-left, [data-sch-border="washi"] .sch-main, [data-sch-border="washi"] .sch-right,
[data-sch-border="ruled"] .sch-left, [data-sch-border="ruled"] .sch-main, [data-sch-border="ruled"] .sch-right,
[data-sch-border="gold-corners"] .sch-left, [data-sch-border="gold-corners"] .sch-main, [data-sch-border="gold-corners"] .sch-right,
[data-sch-border="pressed-flowers"] .sch-left, [data-sch-border="pressed-flowers"] .sch-main, [data-sch-border="pressed-flowers"] .sch-right,
[data-sch-border="vintage"] .sch-left, [data-sch-border="vintage"] .sch-main, [data-sch-border="vintage"] .sch-right { background: transparent !important; }

/* ─── Spiral notebook (default). Each coil is now a FULL 3D ring:
 *   1. Back arc (behind paper) — darker metal, lower opacity
 *   2. Inner shadow hole (perforation shadow)
 *   3. Front arc (in front of paper) — brighter metal
 *   4. White highlight stripe on upper-left of the front arc
 *   5. Bottom shadow on the front arc
 *   6. Slight drop shadow cast by the coil onto the paper (right side)
 * Per-loop: 40×36px viewBox, ~18deg tilt, repeats every 36px vertically.
 * The coil sits over the paper by 8px so the decoration is visible.
 */
[data-sch-border="notebook"] .sch-shell,
body:not([data-sch-border]) .sch-shell {
  background:
    /* Back-arc layer (drawn first, shows behind paper) */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='64' height='36' viewBox='0 0 64 36'%3E%3Cdefs%3E%3ClinearGradient id='bm' x1='0' y1='0' x2='0' y2='1'%3E%3Cstop offset='0' stop-color='%234a4a4e'/%3E%3Cstop offset='.5' stop-color='%23242428'/%3E%3Cstop offset='1' stop-color='%235e5e62'/%3E%3C/linearGradient%3E%3C/defs%3E%3Cg transform='rotate(-18 32 18)'%3E%3Cpath d='M 4 18 Q 32 30 60 18' fill='none' stroke='url(%23bm)' stroke-width='3.6' stroke-linecap='round' opacity='.78'/%3E%3C/g%3E%3C/svg%3E") left center / 64px 36px repeat-y,
    /* Subtle linen fibre noise */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2' seed='3'/%3E%3CfeColorMatrix values='0 0 0 0 .95 0 0 0 0 .92 0 0 0 0 .85 0 0 0 .1 0'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E"),
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
  padding-left: 80px;
  border-radius: 2px;
  box-shadow:
    0 1px 2px rgba(92,74,58,0.1),
    0 14px 38px rgba(92,74,58,0.16),
    inset 0 0 60px rgba(180,160,120,0.08);
}
/* Front arc + highlight + hole shadow — this covers the back arc and
 * paints the "in front of the paper" half of each coil loop. */
[data-sch-border="notebook"] .sch-shell::before,
body:not([data-sch-border]) .sch-shell::before {
  content: '';
  position: absolute; left: 0; top: 0; bottom: 0; width: 72px;
  pointer-events: none; z-index: 2;
  background: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='64' height='36' viewBox='0 0 64 36'%3E%3Cdefs%3E%3ClinearGradient id='fm' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%238e8e92'/%3E%3Cstop offset='.15' stop-color='%23e8e8eb'/%3E%3Cstop offset='.4' stop-color='%236f6f73'/%3E%3Cstop offset='.6' stop-color='%23d4d4d8'/%3E%3Cstop offset='.85' stop-color='%236e6e72'/%3E%3Cstop offset='1' stop-color='%23404044'/%3E%3C/linearGradient%3E%3CradialGradient id='hs' cx='.5' cy='.5' r='.4'%3E%3Cstop offset='0' stop-color='rgba(80,60,30,.55)'/%3E%3Cstop offset='.6' stop-color='rgba(80,60,30,.2)'/%3E%3Cstop offset='1' stop-color='rgba(80,60,30,0)'/%3E%3C/radialGradient%3E%3C/defs%3E%3C!-- perforation shadow in the paper --%3E%3Cellipse cx='32' cy='18' rx='12' ry='7' fill='url(%23hs)'/%3E%3Cg transform='rotate(-18 32 18)'%3E%3C!-- front arc of coil --%3E%3Cpath d='M 4 18 Q 32 4 60 18' fill='none' stroke='url(%23fm)' stroke-width='4' stroke-linecap='round'/%3E%3C!-- specular highlight --%3E%3Cpath d='M 12 12 Q 28 5 48 11' fill='none' stroke='rgba(255,255,255,.92)' stroke-width='1.4' stroke-linecap='round'/%3E%3C!-- subtle underside shadow --%3E%3Cpath d='M 8 21 Q 32 11 56 21' fill='none' stroke='rgba(0,0,0,.28)' stroke-width='1' stroke-linecap='round'/%3E%3C/g%3E%3C/svg%3E") left center / 64px 36px repeat-y;
  filter: drop-shadow(3px 1px 2px rgba(40,30,15,0.3));
}
[data-sch-border="notebook"] .sch-main,
body:not([data-sch-border]) .sch-main {
  background-image: linear-gradient(to bottom, transparent 39px, rgba(74,105,172,0.22) 39px, rgba(74,105,172,0.22) 40px);
  background-size: 100% 40px;
  background-color: transparent;
}

/* ─── Disc-bound. Mushroom-shape gold discs with rim highlight.
 * Each disc: 54px flange + darker stem shadow. Repeats every 88px.
 */
[data-sch-border="disc"] .sch-shell {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='70' height='88' viewBox='0 0 70 88'%3E%3Cdefs%3E%3CradialGradient id='dg' cx='.42' cy='.35' r='.65'%3E%3Cstop offset='0' stop-color='%23faebc0'/%3E%3Cstop offset='.35' stop-color='%23e8c778'/%3E%3Cstop offset='.7' stop-color='%23b48f3d'/%3E%3Cstop offset='1' stop-color='%23785b22'/%3E%3C/radialGradient%3E%3CradialGradient id='ds' cx='.5' cy='.5' r='.5'%3E%3Cstop offset='0' stop-color='rgba(60,40,10,.6)'/%3E%3Cstop offset='1' stop-color='rgba(60,40,10,0)'/%3E%3C/radialGradient%3E%3C/defs%3E%3C!-- cast shadow on paper --%3E%3Cellipse cx='35' cy='48' rx='26' ry='6' fill='url(%23ds)' opacity='.7'/%3E%3C!-- flange (outer disc) --%3E%3Ccircle cx='35' cy='44' r='26' fill='url(%23dg)'/%3E%3C!-- outer rim shadow --%3E%3Ccircle cx='35' cy='44' r='26' fill='none' stroke='rgba(60,40,10,.35)' stroke-width='1.2'/%3E%3C!-- inner metallic ring --%3E%3Ccircle cx='35' cy='44' r='18' fill='none' stroke='rgba(255,240,200,.55)' stroke-width='1'/%3E%3C!-- inner stem hole (darker) --%3E%3Ccircle cx='35' cy='44' r='9' fill='rgba(60,40,10,.35)'/%3E%3Ccircle cx='35' cy='44' r='9' fill='none' stroke='rgba(40,25,5,.6)' stroke-width='.8'/%3E%3C!-- highlight crescent --%3E%3Cpath d='M 18 32 A 20 20 0 0 1 52 32' fill='none' stroke='rgba(255,250,220,.75)' stroke-width='1.2'/%3E%3C/svg%3E") left center / 70px 88px repeat-y,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2' seed='7'/%3E%3CfeColorMatrix values='0 0 0 0 .95 0 0 0 0 .92 0 0 0 0 .85 0 0 0 .1 0'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E"),
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
  padding-left: 82px;
  border-radius: 2px;
  box-shadow: 0 1px 2px rgba(92,74,58,0.1), 0 14px 38px rgba(92,74,58,0.16);
}

/* ─── Moleskine. Much stronger visual cues now — elastic brought back
 *    but trimmed to a narrow strip at the very right edge (outside the
 *    content area), ribbon made a bolder visible tail, page-stack fore-
 *    edge amplified to 8 visible stripes. Darker ivory paper.
 */
[data-sch-border="moleskine"] .sch-shell {
  background:
    /* page stack fore-edge — 8 stripes, more visible */
    repeating-linear-gradient(90deg,
      transparent 0 calc(100% - 14px),
      rgba(140,120,80,0.45) calc(100% - 14px) calc(100% - 13px),
      rgba(220,210,180,0.4) calc(100% - 13px) calc(100% - 12px),
      rgba(140,120,80,0.45) calc(100% - 12px) calc(100% - 11px),
      rgba(220,210,180,0.4) calc(100% - 11px) calc(100% - 10px),
      rgba(140,120,80,0.45) calc(100% - 10px) calc(100% - 9px),
      rgba(220,210,180,0.4) calc(100% - 9px) calc(100% - 8px),
      rgba(140,120,80,0.45) calc(100% - 8px) calc(100% - 7px),
      rgba(220,210,180,0.4) calc(100% - 7px) calc(100% - 6px),
      rgba(140,120,80,0.45) calc(100% - 6px) calc(100% - 5px)),
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.85' numOctaves='2' seed='11'/%3E%3CfeColorMatrix values='0 0 0 0 .97 0 0 0 0 .94 0 0 0 0 .88 0 0 0 .08 0'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E"),
    linear-gradient(180deg, #f6efdd 0%, #e8ddc2 100%);
  border-radius: 8px 12px 12px 8px;
  box-shadow: 0 2px 4px rgba(40,30,15,0.14), 0 18px 44px rgba(40,30,15,0.22), inset 2px 0 4px rgba(0,0,0,0.15);
  padding-right: 28px;
}
/* Red ribbon bookmark — shown as a tail trailing off the bottom right. */
[data-sch-border="moleskine"] .sch-shell::before {
  content: ''; position: absolute;
  top: 40%; bottom: -32px;
  right: 80px; width: 8px;
  background: linear-gradient(90deg, #5e0f14 0%, #9c1f24 45%, #d63038 100%);
  clip-path: polygon(0 0, 100% 0, 100% calc(100% - 14px), 50% 100%, 0 calc(100% - 14px));
  box-shadow: 2px 2px 4px rgba(0,0,0,0.4);
  z-index: 3; pointer-events: none;
}
/* Black elastic closure peeking from behind the fore-edge (right side) */
[data-sch-border="moleskine"] .sch-shell::after {
  content: ''; position: absolute;
  top: -6px; bottom: -6px;
  right: -4px; width: 16px;
  background: linear-gradient(90deg, transparent 0, rgba(0,0,0,0.15) 40%, rgba(0,0,0,0.35) 70%, #1a1a1c 100%);
  border-radius: 0 10px 10px 0;
  z-index: 1; pointer-events: none;
}

/* ─── Washi. BIGGER tapes — 260×54 per corner (was 180×44). Tapes sit
 *    ON the paper (not bleeding off), with proper torn edges and a
 *    real box-shadow drop (no more multiply blending).
 */
[data-sch-border="washi"] .sch-shell {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2' seed='5'/%3E%3CfeColorMatrix values='0 0 0 0 .95 0 0 0 0 .92 0 0 0 0 .85 0 0 0 .1 0'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E"),
    linear-gradient(180deg, #faf6ee 0%, #f5ebe0 100%);
  border-radius: 2px;
  box-shadow: 0 1px 2px rgba(92,74,58,0.1), 0 12px 30px rgba(92,74,58,0.14);
  padding-top: 44px;
  padding-bottom: 44px;
}
[data-sch-border="washi"] .sch-shell::before {
  content: ''; position: absolute; inset: 0;
  pointer-events: none; z-index: 3;
  background:
    /* top-left coral polka */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='54' viewBox='0 0 260 54'%3E%3Cg transform='rotate(-4 130 27)'%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='%23f2a6b4' opacity='.93'/%3E%3Cg fill='%23fff' opacity='.6'%3E%3Ccircle cx='18' cy='20' r='3.5'/%3E%3Ccircle cx='42' cy='34' r='3.5'/%3E%3Ccircle cx='66' cy='20' r='3.5'/%3E%3Ccircle cx='90' cy='34' r='3.5'/%3E%3Ccircle cx='114' cy='20' r='3.5'/%3E%3Ccircle cx='138' cy='34' r='3.5'/%3E%3Ccircle cx='162' cy='20' r='3.5'/%3E%3Ccircle cx='186' cy='34' r='3.5'/%3E%3Ccircle cx='210' cy='20' r='3.5'/%3E%3Ccircle cx='234' cy='34' r='3.5'/%3E%3C/g%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='none' stroke='rgba(0,0,0,.12)' stroke-width='.8'/%3E%3C/g%3E%3C/svg%3E") 40px 16px / 260px 54px no-repeat,
    /* top-right sage diagonal stripes */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='54' viewBox='0 0 260 54'%3E%3Cg transform='rotate(5 130 27)'%3E%3Cdefs%3E%3Cpattern id='ss' width='14' height='14' patternUnits='userSpaceOnUse' patternTransform='rotate(45)'%3E%3Crect width='7' height='14' fill='%23ffffff' opacity='.38'/%3E%3C/pattern%3E%3C/defs%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='%23a0ba90' opacity='.93'/%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='url(%23ss)'/%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='none' stroke='rgba(0,0,0,.12)' stroke-width='.8'/%3E%3C/g%3E%3C/svg%3E") calc(100% - 40px) 16px / 260px 54px no-repeat,
    /* bottom-left mustard gingham */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='54' viewBox='0 0 260 54'%3E%3Cg transform='rotate(6 130 27)'%3E%3Cdefs%3E%3Cpattern id='gg' width='10' height='10' patternUnits='userSpaceOnUse'%3E%3Crect width='5' height='5' fill='rgba(255,255,255,.5)'/%3E%3Crect x='5' y='5' width='5' height='5' fill='rgba(255,255,255,.5)'/%3E%3C/pattern%3E%3C/defs%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='%23e6c254' opacity='.93'/%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='url(%23gg)'/%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='none' stroke='rgba(0,0,0,.12)' stroke-width='.8'/%3E%3C/g%3E%3C/svg%3E") 40px calc(100% - 16px) / 260px 54px no-repeat,
    /* bottom-right lavender floral */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='54' viewBox='0 0 260 54'%3E%3Cg transform='rotate(-5 130 27)'%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='%23b4a1d6' opacity='.93'/%3E%3Cg fill='white' opacity='.7'%3E%3Cg transform='translate(28 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(72 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(116 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(160 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(204 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(232 27)'%3E%3Ccircle cx='0' cy='-5' r='2.8'/%3E%3Ccircle cx='4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='3' cy='4' r='2.8'/%3E%3Ccircle cx='-3' cy='4' r='2.8'/%3E%3Ccircle cx='-4.8' cy='-1.5' r='2.8'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3C/g%3E%3Cpath d='M0,5 Q13,2 26,5 Q39,8 52,5 Q65,2 78,5 Q91,8 104,5 Q117,2 130,5 Q143,8 156,5 Q169,2 182,5 Q195,8 208,5 Q221,2 234,5 Q247,8 260,5 L260,49 Q247,52 234,49 Q221,46 208,49 Q195,52 182,49 Q169,46 156,49 Q143,52 130,49 Q117,46 104,49 Q91,52 78,49 Q65,46 52,49 Q39,52 26,49 Q13,46 0,49 Z' fill='none' stroke='rgba(0,0,0,.12)' stroke-width='.8'/%3E%3C/g%3E%3C/svg%3E") calc(100% - 40px) calc(100% - 16px) / 260px 54px no-repeat;
  filter: drop-shadow(0 3px 6px rgba(0,0,0,0.22));
}

/* ─── Legal pad — deeper yellow + bolder blue ruled lines + thicker
 *    red margin + bigger wire-o binding across the top.
 */
[data-sch-border="ruled"] .sch-shell {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='56' height='46' viewBox='0 0 56 46'%3E%3Cdefs%3E%3ClinearGradient id='wo' x1='0' y1='0' x2='0' y2='1'%3E%3Cstop offset='0' stop-color='%23dddde1'/%3E%3Cstop offset='.35' stop-color='%238a8a8e'/%3E%3Cstop offset='.7' stop-color='%233e3e42'/%3E%3Cstop offset='1' stop-color='%231e1e22'/%3E%3C/linearGradient%3E%3C/defs%3E%3C!-- wire-O double loop --%3E%3Cpath d='M 14 32 L 14 4 Q 28 -2 42 4 L 42 32' fill='none' stroke='url(%23wo)' stroke-width='3.5' stroke-linecap='round'/%3E%3Cpath d='M 18 30 L 18 6' stroke='rgba(255,255,255,.5)' stroke-width='1' fill='none'/%3E%3Cpath d='M 16 33 L 16 5' stroke='rgba(0,0,0,.3)' stroke-width='.6' fill='none'/%3E%3C/svg%3E") top left / 56px 46px repeat-x,
    repeating-linear-gradient(180deg, transparent 0 31px, rgba(60,110,180,0.34) 31px 33px),
    linear-gradient(90deg, transparent 64px, rgba(200,60,60,0.6) 64px 66px, transparent 66px),
    linear-gradient(180deg, #fff2b0 0%, #fce79e 100%);
  padding-top: 56px !important;
  padding-left: 76px !important;
  border-radius: 2px;
  box-shadow: 0 2px 4px rgba(92,74,58,0.14), 0 14px 36px rgba(92,74,58,0.18);
}
[data-sch-border="ruled"] .sch-main { background-color: transparent; background-image: none; }

/* ─── Gilded — wider ornate gold band + decorative corner flourishes.
 *    Swapped padding-box/border-box trick for a real border-image
 *    stretched from a detailed SVG with corner swirls. 10px border
 *    (was 3px).
 */
[data-sch-border="gold-corners"] .sch-shell {
  border: 14px solid transparent;
  border-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='200' viewBox='0 0 200 200'%3E%3Cdefs%3E%3ClinearGradient id='g1' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%23b8863c'/%3E%3Cstop offset='.2' stop-color='%23e4c570'/%3E%3Cstop offset='.5' stop-color='%23f7e08c'/%3E%3Cstop offset='.8' stop-color='%23b8863c'/%3E%3Cstop offset='1' stop-color='%237a5820'/%3E%3C/linearGradient%3E%3C/defs%3E%3C!-- outer frame --%3E%3Crect x='3' y='3' width='194' height='194' fill='none' stroke='url(%23g1)' stroke-width='3'/%3E%3C!-- inner pinstripe --%3E%3Crect x='14' y='14' width='172' height='172' fill='none' stroke='url(%23g1)' stroke-width='1'/%3E%3C!-- middle detail line --%3E%3Crect x='10' y='10' width='180' height='180' fill='none' stroke='url(%23g1)' stroke-width='.8' stroke-dasharray='2 3'/%3E%3Cg fill='url(%23g1)'%3E%3C!-- TL corner flourish --%3E%3Cpath d='M 3 3 L 30 3 Q 18 10 14 22 Q 10 10 3 30 Z'/%3E%3Cpath d='M 18 18 Q 26 14 30 10 Q 24 20 20 28 Q 14 24 10 18 Q 14 22 18 18 Z' opacity='.8'/%3E%3Ccircle cx='20' cy='20' r='2.6'/%3E%3Ccircle cx='26' cy='26' r='1.6'/%3E%3C!-- TR corner --%3E%3Cpath d='M 197 3 L 170 3 Q 182 10 186 22 Q 190 10 197 30 Z'/%3E%3Cpath d='M 182 18 Q 174 14 170 10 Q 176 20 180 28 Q 186 24 190 18 Q 186 22 182 18 Z' opacity='.8'/%3E%3Ccircle cx='180' cy='20' r='2.6'/%3E%3Ccircle cx='174' cy='26' r='1.6'/%3E%3C!-- BL corner --%3E%3Cpath d='M 3 197 L 30 197 Q 18 190 14 178 Q 10 190 3 170 Z'/%3E%3Ccircle cx='20' cy='180' r='2.6'/%3E%3Ccircle cx='26' cy='174' r='1.6'/%3E%3C!-- BR corner --%3E%3Cpath d='M 197 197 L 170 197 Q 182 190 186 178 Q 190 190 197 170 Z'/%3E%3Ccircle cx='180' cy='180' r='2.6'/%3E%3Ccircle cx='174' cy='174' r='1.6'/%3E%3C!-- midpoint diamonds --%3E%3Cpath d='M 100 4 L 108 12 L 100 20 L 92 12 Z'/%3E%3Cpath d='M 100 180 L 108 188 L 100 196 L 92 188 Z'/%3E%3Cpath d='M 4 100 L 12 108 L 20 100 L 12 92 Z'/%3E%3Cpath d='M 180 100 L 188 108 L 196 100 L 188 92 Z'/%3E%3C/g%3E%3C!-- decorative filigree between corners and midpoints --%3E%3Cg fill='none' stroke='url(%23g1)' stroke-width='1'%3E%3Cpath d='M 36 10 Q 50 6 64 10 Q 78 14 92 10'/%3E%3Cpath d='M 36 190 Q 50 194 64 190 Q 78 186 92 190'/%3E%3Cpath d='M 108 10 Q 122 6 136 10 Q 150 14 164 10'/%3E%3Cpath d='M 108 190 Q 122 194 136 190 Q 150 186 164 190'/%3E%3Cpath d='M 10 36 Q 6 50 10 64 Q 14 78 10 92'/%3E%3Cpath d='M 190 36 Q 194 50 190 64 Q 186 78 190 92'/%3E%3Cpath d='M 10 108 Q 6 122 10 136 Q 14 150 10 164'/%3E%3Cpath d='M 190 108 Q 194 122 190 136 Q 186 150 190 164'/%3E%3C/g%3E%3C/svg%3E") 60 fill / 14px / 0 stretch;
  background-color: transparent;
  border-radius: 2px;
}
[data-sch-border="gold-corners"] .sch-shell > * { background: linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%); }

/* ─── Floral bouquet — FAR richer composition. Top-left corner now
 *    has a substantial cluster of peony blooms + eucalyptus sprigs +
 *    wildflower accents, trailing to a secondary cluster bottom-right.
 *    250px composition instead of 130px.
 */
[data-sch-border="pressed-flowers"] .sch-shell {
  background: linear-gradient(180deg, #faf6ee 0%, #f5ebe0 100%);
  border-radius: 3px;
  box-shadow: 0 2px 4px rgba(92,74,58,0.1), 0 12px 32px rgba(92,74,58,0.14);
  padding: 42px 0;
}
[data-sch-border="pressed-flowers"] .sch-shell::before {
  content: ''; position: absolute; inset: 0; pointer-events: none; z-index: 3;
  mix-blend-mode: multiply; opacity: 0.95;
  background:
    /* TOP-LEFT large bouquet */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='280' height='260' viewBox='0 0 280 260'%3E%3Cg fill='none' stroke='%235f7a56' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M2 56 C 36 42 72 58 118 82 C 164 106 208 116 278 124'/%3E%3Cpath d='M14 18 C 44 32 68 58 88 82 C 112 110 142 124 172 120'/%3E%3Cpath d='M4 118 C 28 110 52 120 74 136'/%3E%3Cpath d='M36 2 C 48 16 58 34 66 54'/%3E%3C/g%3E%3C!-- eucalyptus leaves (sage) --%3E%3Cg fill='%238fa889'%3E%3Cellipse cx='32' cy='34' rx='11' ry='5' transform='rotate(25 32 34)'/%3E%3Cellipse cx='58' cy='60' rx='13' ry='6' transform='rotate(30 58 60)'/%3E%3Cellipse cx='88' cy='80' rx='13' ry='6' transform='rotate(-10 88 80)'/%3E%3Cellipse cx='56' cy='26' rx='9' ry='4.5' transform='rotate(-35 56 26)'/%3E%3Cellipse cx='82' cy='54' rx='9' ry='4.5' transform='rotate(45 82 54)'/%3E%3Cellipse cx='122' cy='98' rx='11' ry='5' transform='rotate(10 122 98)'/%3E%3Cellipse cx='158' cy='112' rx='10' ry='4.5' transform='rotate(-5 158 112)'/%3E%3Cellipse cx='38' cy='120' rx='8' ry='4' transform='rotate(15 38 120)'/%3E%3Cellipse cx='62' cy='128' rx='9' ry='4.5' transform='rotate(-10 62 128)'/%3E%3Cellipse cx='92' cy='140' rx='8' ry='4' transform='rotate(15 92 140)'/%3E%3Cellipse cx='46' cy='14' rx='6' ry='3' transform='rotate(-60 46 14)'/%3E%3C/g%3E%3C!-- peony — layered pink rose --%3E%3Cg transform='translate(72 86)'%3E%3Ccircle cx='0' cy='0' r='16' fill='%23e8a3b0'/%3E%3Ccircle cx='-4' cy='-2' r='10' fill='%23d48fa5'/%3E%3Ccircle cx='3' cy='-3' r='8' fill='%23c27890'/%3E%3Ccircle cx='0' cy='3' r='6' fill='%23e8a3b0'/%3E%3Ccircle cx='0' cy='-1' r='3' fill='%23a0607a'/%3E%3C/g%3E%3C!-- smaller pink blooms --%3E%3Cg fill='%23d48fa5'%3E%3Cg transform='translate(118 102)'%3E%3Ccircle cx='0' cy='-5' r='3.6'/%3E%3Ccircle cx='5' cy='-1.6' r='3.6'/%3E%3Ccircle cx='3.2' cy='4.2' r='3.6'/%3E%3Ccircle cx='-3.2' cy='4.2' r='3.6'/%3E%3Ccircle cx='-5' cy='-1.6' r='3.6'/%3E%3Ccircle cx='0' cy='0' r='2.2' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(158 52)'%3E%3Ccircle cx='0' cy='-4' r='3'/%3E%3Ccircle cx='4' cy='-1.3' r='3'/%3E%3Ccircle cx='2.4' cy='3.3' r='3'/%3E%3Ccircle cx='-2.4' cy='3.3' r='3'/%3E%3Ccircle cx='-4' cy='-1.3' r='3'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3Cg transform='translate(196 118)'%3E%3Ccircle cx='0' cy='-3.4' r='2.6'/%3E%3Ccircle cx='3.2' cy='-1' r='2.6'/%3E%3Ccircle cx='2' cy='2.8' r='2.6'/%3E%3Ccircle cx='-2' cy='2.8' r='2.6'/%3E%3Ccircle cx='-3.2' cy='-1' r='2.6'/%3E%3Ccircle cx='0' cy='0' r='1.5' fill='%23e6c25a'/%3E%3C/g%3E%3C/g%3E%3C!-- wildflower buds (muted coral) --%3E%3Cg fill='%23e08f6d'%3E%3Ccircle cx='28' cy='48' r='2.4'/%3E%3Ccircle cx='42' cy='70' r='2.4'/%3E%3Ccircle cx='100' cy='44' r='2'/%3E%3Ccircle cx='134' cy='122' r='2.2'/%3E%3C/g%3E%3C/svg%3E") top left / 300px 280px no-repeat,
    /* BOTTOM-RIGHT smaller bouquet */
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='180' height='160' viewBox='0 0 180 160'%3E%3Cg transform='translate(180 160) scale(-1 -1)'%3E%3Cg fill='none' stroke='%235f7a56' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M2 56 C 36 42 72 58 118 82 C 152 102 172 108 178 110'/%3E%3Cpath d='M14 18 C 44 32 68 58 88 82'/%3E%3C/g%3E%3Cg fill='%238fa889'%3E%3Cellipse cx='32' cy='34' rx='10' ry='5' transform='rotate(25 32 34)'/%3E%3Cellipse cx='58' cy='60' rx='12' ry='5.5' transform='rotate(30 58 60)'/%3E%3Cellipse cx='88' cy='80' rx='12' ry='5.5' transform='rotate(-10 88 80)'/%3E%3Cellipse cx='56' cy='26' rx='8' ry='4' transform='rotate(-35 56 26)'/%3E%3C/g%3E%3Cg transform='translate(72 86)'%3E%3Ccircle cx='0' cy='0' r='13' fill='%23e8a3b0'/%3E%3Ccircle cx='-3' cy='-2' r='8' fill='%23d48fa5'/%3E%3Ccircle cx='2' cy='-2' r='6' fill='%23c27890'/%3E%3Ccircle cx='0' cy='-1' r='2.5' fill='%23a0607a'/%3E%3C/g%3E%3Cg fill='%23d48fa5'%3E%3Cg transform='translate(118 100)'%3E%3Ccircle cx='0' cy='-4' r='3'/%3E%3Ccircle cx='4' cy='-1.3' r='3'/%3E%3Ccircle cx='2.4' cy='3.3' r='3'/%3E%3Ccircle cx='-2.4' cy='3.3' r='3'/%3E%3Ccircle cx='-4' cy='-1.3' r='3'/%3E%3Ccircle cx='0' cy='0' r='1.8' fill='%23e6c25a'/%3E%3C/g%3E%3C/g%3E%3C/g%3E%3C/svg%3E") bottom right / 200px 180px no-repeat;
}

/* ─── Vintage — more filigree on the corners + inner stamp-ring. */
[data-sch-border="vintage"] .sch-shell {
  background: radial-gradient(ellipse at center, #ebdbb0 0%, #d9c37f 70%, #b8a055 100%);
  box-shadow:
    inset 0 0 0 2px #6b4f2a,
    inset 0 0 0 3px #ebdbb0,
    inset 0 0 0 5px #6b4f2a,
    inset 0 0 0 7px #ebdbb0,
    inset 0 0 0 8px rgba(107,79,42,0.4),
    0 2px 4px rgba(30,20,10,0.12),
    0 14px 38px rgba(30,20,10,0.24);
  border-radius: 6px;
}
[data-sch-border="vintage"] .sch-shell::before {
  content: ''; position: absolute; inset: 0; pointer-events: none; z-index: 4;
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140' viewBox='0 0 140 140'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M12 56 Q 12 12 56 12'/%3E%3Cpath d='M22 56 Q 22 22 56 22'/%3E%3Cpath d='M32 56 Q 32 32 56 32'/%3E%3Cpath d='M12 66 L 42 66 M 66 12 L 66 42'/%3E%3Cpath d='M30 30 Q 38 22 46 22 Q 42 30 38 38 Z' stroke-width='1.4'/%3E%3Cpath d='M14 40 Q 18 34 24 32 M 16 48 Q 24 40 30 38' stroke-width='1.2'/%3E%3Cpath d='M40 16 Q 34 20 32 26 M 48 18 Q 42 26 40 32' stroke-width='1.2'/%3E%3Cpath d='M20 20 Q 28 28 34 34' stroke-width='.8'/%3E%3Ccircle cx='32' cy='32' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='42' cy='42' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='52' cy='52' r='1.6' fill='%234a3426'/%3E%3Cpath d='M54 54 Q 62 46 68 44 M 48 68 Q 56 60 64 58' stroke-width='1'/%3E%3C/g%3E%3C/svg%3E") top left / 140px 140px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140' viewBox='0 0 140 140'%3E%3Cg transform='translate(140 0) scale(-1 1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M12 56 Q 12 12 56 12'/%3E%3Cpath d='M22 56 Q 22 22 56 22'/%3E%3Cpath d='M32 56 Q 32 32 56 32'/%3E%3Cpath d='M12 66 L 42 66 M 66 12 L 66 42'/%3E%3Cpath d='M30 30 Q 38 22 46 22 Q 42 30 38 38 Z' stroke-width='1.4'/%3E%3Ccircle cx='32' cy='32' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='42' cy='42' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='52' cy='52' r='1.6' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") top right / 140px 140px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140' viewBox='0 0 140 140'%3E%3Cg transform='translate(0 140) scale(1 -1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M12 56 Q 12 12 56 12'/%3E%3Cpath d='M22 56 Q 22 22 56 22'/%3E%3Cpath d='M32 56 Q 32 32 56 32'/%3E%3Ccircle cx='32' cy='32' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='42' cy='42' r='2.4' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") bottom left / 140px 140px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140' viewBox='0 0 140 140'%3E%3Cg transform='translate(140 140) scale(-1 -1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M12 56 Q 12 12 56 12'/%3E%3Cpath d='M22 56 Q 22 22 56 22'/%3E%3Cpath d='M32 56 Q 32 32 56 32'/%3E%3Ccircle cx='32' cy='32' r='2.4' fill='%234a3426'/%3E%3Ccircle cx='42' cy='42' r='2.4' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") bottom right / 140px 140px no-repeat;
}

/* ══ Garden Backdrop — Sean's clean Gemini-generated backdrop ══
 *
 * Unlike the punched-overlay approach that fought the scheduler's
 * content height, this is a straight backdrop: cream paper with
 * watercolor botanicals at the four corners, no illustrated content
 * to clash with. The scheduler's layout renders on top in its natural
 * flex arrangement. `background-size: 100% 100%` stretches the image
 * to fill the shell — the watercolor corners stay at corners (even if
 * the shell is taller than the source), the middle is plain cream so
 * any stretching there is invisible.
 */
/* CSS variables drive backdrop position + scale so users can drag, zoom,
 * and stretch the painting independently on each axis. Values:
 *   --sch-bp-x / --sch-bp-y  — background-position percentages (pan)
 *   --sch-bp-w               — computed width string ("150%")
 *   --sch-bp-h               — computed height string ("120%" or "auto")
 * JS sets these from scale_x / scale_y so we can express "aspect-preserve"
 * (height=auto) without losing it in calc(). */
:root {
  --sch-bp-x: 50%;
  --sch-bp-y: 50%;
  --sch-bp-w: 100%;
  --sch-bp-h: auto;
}
[data-sch-border="garden-backdrop"] .sch-shell {
  background-color: #f4ead2;
  background-image: url('/scheduler-frame/garden-backdrop');
  background-repeat: no-repeat;
  background-position: var(--sch-bp-x) var(--sch-bp-y);
  background-size: var(--sch-bp-w) var(--sch-bp-h);
  padding: 24px;
  border-radius: 4px;
  box-shadow: 0 2px 6px rgba(40,30,15,0.12), 0 14px 36px rgba(40,30,15,0.18);
}
/* Suppress the default spiral-notebook coil. */
[data-sch-border="garden-backdrop"] .sch-shell::before { content: none !important; }
/* Layout zones go see-through so the watercolor corners show behind the
 * panels. Event chips / cards / today-marker still get solid backgrounds
 * for legibility via their own rules. */
[data-sch-border="garden-backdrop"] .sch-left,
[data-sch-border="garden-backdrop"] .sch-main,
[data-sch-border="garden-backdrop"] .sch-right {
  background: rgba(248, 240, 215, 0.35) !important;
}
/* Month-grid cells and sidebar cards — lighter tint so watercolor peeks
 * through the grid, but text stays readable. */
[data-sch-border="garden-backdrop"] .sch-cell {
  background: rgba(253, 248, 232, 0.55) !important;
}
[data-sch-border="garden-backdrop"] .sch-card,
[data-sch-border="garden-backdrop"] .sch-tp-card {
  background: rgba(253, 248, 232, 0.75) !important;
}

/* Adjust-backdrop edit mode — dim content, highlight the drag surface. */
.sch-shell.sch-editing-backdrop {
  cursor: grab;
}
.sch-shell.sch-editing-backdrop:active { cursor: grabbing; }
.sch-shell.sch-editing-backdrop .sch-left,
.sch-shell.sch-editing-backdrop .sch-main,
.sch-shell.sch-editing-backdrop .sch-right {
  opacity: 0.25;
  pointer-events: none;
  transition: opacity 0.2s ease;
}
.sch-backdrop-toolbar {
  position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%);
  background: rgba(17,24,39,0.95); color: #e5e7eb;
  border-radius: 999px; padding: 10px 16px;
  display: flex; gap: 10px; align-items: center;
  box-shadow: 0 10px 30px rgba(0,0,0,0.35);
  z-index: 50; font-size: 13px;
}
.sch-backdrop-toolbar button {
  background: transparent; color: inherit; border: 1px solid #374151;
  border-radius: 999px; padding: 5px 12px; cursor: pointer; font: inherit;
}
.sch-backdrop-toolbar button:hover { border-color: #6b7280; color: #fff; }
.sch-backdrop-toolbar .btn-save { background: #2563eb; border-color: #2563eb; color: #fff; }
.sch-backdrop-toolbar .btn-save:hover { background: #1d4ed8; }
.sch-backdrop-toolbar .hint { opacity: 0.7; font-size: 12px; margin-right: 6px; }

/* ─── Preview tiles with per-style mini illustrations. */
.sch-border-tile {
  background: var(--sch-paper); border: 1px solid var(--sch-border);
  border-radius: 8px; padding: 0; cursor: pointer;
  font-family: inherit; overflow: hidden;
  display: flex; flex-direction: column; align-items: stretch; gap: 0;
  transition: transform 0.12s ease, border-color 0.12s ease;
}
.sch-border-tile.active { outline: 2px solid var(--sch-accent); outline-offset: 2px; border-color: var(--sch-accent); }
.sch-border-tile:hover { border-color: var(--sch-accent); transform: translateY(-2px); }
.sch-border-preview {
  height: 130px; position: relative;
  background: var(--sch-bg); overflow: hidden; padding: 10px;
}
.sch-preview-paper {
  position: absolute; inset: 10px; border-radius: 2px;
  background: linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
  box-shadow: 0 1px 2px rgba(92,74,58,0.1), 0 4px 10px rgba(92,74,58,0.12);
}
.sch-preview-paper::before { content: ''; position: absolute; inset: 0; pointer-events: none; }

/* Painted-frame thumbnails in the border picker — show the actual artwork. */
.sch-border-thumb {
  position: absolute; inset: 0; width: 100%; height: 100%;
  object-fit: cover; object-position: center;
  border-radius: 3px;
  box-shadow: 0 1px 2px rgba(40,30,15,0.14), 0 4px 10px rgba(40,30,15,0.14);
}
.sch-border-tile.active .sch-border-thumb { filter: brightness(1.04) contrast(1.04); }

.sch-border-preview[data-preview="notebook"] .sch-preview-paper {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='26' height='24' viewBox='0 0 26 24'%3E%3Cdefs%3E%3ClinearGradient id='m' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%238e8e92'/%3E%3Cstop offset='.3' stop-color='%23ededf0'/%3E%3Cstop offset='.5' stop-color='%236f6f73'/%3E%3Cstop offset='.85' stop-color='%236e6e72'/%3E%3Cstop offset='1' stop-color='%23404044'/%3E%3C/linearGradient%3E%3C/defs%3E%3Cellipse cx='13' cy='12' rx='6' ry='3.5' fill='rgba(80,60,30,.25)'/%3E%3Cg transform='rotate(-18 13 12)'%3E%3Cpath d='M 2 12 Q 13 4 24 12' fill='none' stroke='url(%23m)' stroke-width='2.6' stroke-linecap='round'/%3E%3Cpath d='M 5 9 Q 13 5 21 9' fill='none' stroke='rgba(255,255,255,.85)' stroke-width='.9' stroke-linecap='round'/%3E%3Cpath d='M 4 13 Q 13 19 22 13' fill='none' stroke='rgba(0,0,0,.4)' stroke-width='1.6' stroke-linecap='round' opacity='.6'/%3E%3C/g%3E%3C/svg%3E") left center / 26px 24px repeat-y,
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
  padding-left: 30px;
}
.sch-border-preview[data-preview="notebook"] .sch-preview-paper::before {
  background: linear-gradient(to bottom, transparent 19px, rgba(74,105,172,0.22) 19px, rgba(74,105,172,0.22) 20px);
  background-size: 100% 20px; left: 30px;
}

.sch-border-preview[data-preview="disc"] .sch-preview-paper {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='36' height='38' viewBox='0 0 36 38'%3E%3Cdefs%3E%3CradialGradient id='dg' cx='.42' cy='.35' r='.65'%3E%3Cstop offset='0' stop-color='%23faebc0'/%3E%3Cstop offset='.35' stop-color='%23e8c778'/%3E%3Cstop offset='.7' stop-color='%23b48f3d'/%3E%3Cstop offset='1' stop-color='%23785b22'/%3E%3C/radialGradient%3E%3C/defs%3E%3Cellipse cx='18' cy='22' rx='12' ry='3' fill='rgba(60,40,10,.4)'/%3E%3Ccircle cx='18' cy='19' r='13' fill='url(%23dg)'/%3E%3Ccircle cx='18' cy='19' r='13' fill='none' stroke='rgba(60,40,10,.4)' stroke-width='.8'/%3E%3Ccircle cx='18' cy='19' r='8' fill='none' stroke='rgba(255,240,200,.6)' stroke-width='.7'/%3E%3Ccircle cx='18' cy='19' r='4' fill='rgba(60,40,10,.35)'/%3E%3Cpath d='M 10 12 A 9 9 0 0 1 26 12' fill='none' stroke='rgba(255,250,220,.8)' stroke-width='1'/%3E%3C/svg%3E") left center / 36px 38px repeat-y,
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
  padding-left: 38px;
}

.sch-border-preview[data-preview="moleskine"] .sch-preview-paper {
  background:
    repeating-linear-gradient(90deg,
      transparent 0 calc(100% - 8px),
      rgba(140,120,80,0.5) calc(100% - 8px) calc(100% - 7px),
      rgba(220,210,180,0.5) calc(100% - 7px) calc(100% - 6px),
      rgba(140,120,80,0.5) calc(100% - 6px) calc(100% - 5px),
      rgba(220,210,180,0.5) calc(100% - 5px) calc(100% - 4px),
      rgba(140,120,80,0.5) calc(100% - 4px) calc(100% - 3px)),
    linear-gradient(180deg, #f6efdd 0%, #e8ddc2 100%);
  border-radius: 3px 6px 6px 3px;
  box-shadow: 0 1px 2px rgba(40,30,15,0.15), 0 3px 8px rgba(40,30,15,0.18), inset 1px 0 3px rgba(0,0,0,0.1);
}
.sch-border-preview[data-preview="moleskine"] .sch-preview-paper::after {
  content: ''; position: absolute;
  top: 40%; bottom: -8px;
  right: 32%; width: 4px;
  background: linear-gradient(90deg, #5e0f14, #9c1f24, #d63038);
  clip-path: polygon(0 0, 100% 0, 100% calc(100% - 5px), 50% 100%, 0 calc(100% - 5px));
  box-shadow: 1px 1px 2px rgba(0,0,0,0.3);
}

.sch-border-preview[data-preview="washi"] .sch-preview-paper {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='90' height='22' viewBox='0 0 90 22'%3E%3Cg transform='rotate(-4 45 11)'%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='%23f2a6b4'/%3E%3Cg fill='%23fff' opacity='.6'%3E%3Ccircle cx='12' cy='8' r='1.6'/%3E%3Ccircle cx='28' cy='14' r='1.6'/%3E%3Ccircle cx='44' cy='8' r='1.6'/%3E%3Ccircle cx='60' cy='14' r='1.6'/%3E%3Ccircle cx='76' cy='8' r='1.6'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") 14px 8px / 90px 22px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='90' height='22' viewBox='0 0 90 22'%3E%3Cg transform='rotate(5 45 11)'%3E%3Cdefs%3E%3Cpattern id='s' width='8' height='8' patternUnits='userSpaceOnUse' patternTransform='rotate(45)'%3E%3Crect width='4' height='8' fill='%23ffffff' opacity='.42'/%3E%3C/pattern%3E%3C/defs%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='%23a0ba90'/%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='url(%23s)'/%3E%3C/g%3E%3C/svg%3E") calc(100% - 14px) 8px / 90px 22px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='90' height='22' viewBox='0 0 90 22'%3E%3Cg transform='rotate(6 45 11)'%3E%3Cdefs%3E%3Cpattern id='g' width='5' height='5' patternUnits='userSpaceOnUse'%3E%3Crect width='2.5' height='2.5' fill='rgba(255,255,255,.5)'/%3E%3Crect x='2.5' y='2.5' width='2.5' height='2.5' fill='rgba(255,255,255,.5)'/%3E%3C/pattern%3E%3C/defs%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='%23e6c254'/%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='url(%23g)'/%3E%3C/g%3E%3C/svg%3E") 14px calc(100% - 8px) / 90px 22px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='90' height='22' viewBox='0 0 90 22'%3E%3Cg transform='rotate(-5 45 11)'%3E%3Cpath d='M0,3 Q10,2 20,3 Q30,4 40,3 Q50,2 60,3 Q70,4 80,3 Q85,3 90,3 L90,19 Q85,19 80,19 Q70,20 60,19 Q50,18 40,19 Q30,20 20,19 Q10,18 0,19 Z' fill='%23b4a1d6'/%3E%3Cg fill='white' opacity='.7'%3E%3Ccircle cx='16' cy='11' r='1.4'/%3E%3Ccircle cx='34' cy='11' r='1.4'/%3E%3Ccircle cx='52' cy='11' r='1.4'/%3E%3Ccircle cx='70' cy='11' r='1.4'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") calc(100% - 14px) calc(100% - 8px) / 90px 22px no-repeat,
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
}

.sch-border-preview[data-preview="ruled"] .sch-preview-paper {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='26' height='22' viewBox='0 0 26 22'%3E%3Cdefs%3E%3ClinearGradient id='wo' x1='0' y1='0' x2='0' y2='1'%3E%3Cstop offset='0' stop-color='%23dddde1'/%3E%3Cstop offset='.5' stop-color='%238a8a8e'/%3E%3Cstop offset='1' stop-color='%231e1e22'/%3E%3C/linearGradient%3E%3C/defs%3E%3Cpath d='M 8 15 L 8 4 Q 13 0 18 4 L 18 15' fill='none' stroke='url(%23wo)' stroke-width='2' stroke-linecap='round'/%3E%3C/svg%3E") top left / 26px 22px repeat-x,
    repeating-linear-gradient(180deg, transparent 0 14px, rgba(60,110,180,0.34) 14px 15px),
    linear-gradient(90deg, transparent 32px, rgba(200,60,60,0.6) 32px 33px, transparent 33px),
    linear-gradient(180deg, #fff2b0 0%, #fce79e 100%);
  padding-top: 22px;
}

.sch-border-preview[data-preview="gold-corners"] .sch-preview-paper {
  border: 6px solid transparent;
  border-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='80' viewBox='0 0 80 80'%3E%3Cdefs%3E%3ClinearGradient id='g1' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%23b8863c'/%3E%3Cstop offset='.5' stop-color='%23f7e08c'/%3E%3Cstop offset='1' stop-color='%237a5820'/%3E%3C/linearGradient%3E%3C/defs%3E%3Crect x='2' y='2' width='76' height='76' fill='none' stroke='url(%23g1)' stroke-width='2'/%3E%3Crect x='6' y='6' width='68' height='68' fill='none' stroke='url(%23g1)' stroke-width='.6'/%3E%3Cg fill='url(%23g1)'%3E%3Cpath d='M 2 2 L 14 2 Q 8 6 6 12 Q 4 6 2 14 Z'/%3E%3Cpath d='M 78 2 L 66 2 Q 72 6 74 12 Q 76 6 78 14 Z'/%3E%3Cpath d='M 2 78 L 14 78 Q 8 74 6 68 Q 4 74 2 66 Z'/%3E%3Cpath d='M 78 78 L 66 78 Q 72 74 74 68 Q 76 74 78 66 Z'/%3E%3C/g%3E%3C/svg%3E") 24 fill / 6px / 0 stretch;
  background: linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
}

.sch-border-preview[data-preview="pressed-flowers"] .sch-preview-paper {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='110' height='90' viewBox='0 0 110 90'%3E%3Cg fill='none' stroke='%235f7a56' stroke-width='1.3' stroke-linecap='round'%3E%3Cpath d='M2 28 C 20 20 40 30 60 44 C 80 56 94 58 110 60'/%3E%3Cpath d='M10 6 C 22 14 32 28 42 40'/%3E%3C/g%3E%3Cg fill='%238fa889'%3E%3Cellipse cx='18' cy='18' rx='5' ry='2.4' transform='rotate(25 18 18)'/%3E%3Cellipse cx='32' cy='28' rx='6' ry='3' transform='rotate(30 32 28)'/%3E%3Cellipse cx='48' cy='38' rx='6' ry='3' transform='rotate(-10 48 38)'/%3E%3Cellipse cx='28' cy='10' rx='4' ry='2' transform='rotate(-35 28 10)'/%3E%3C/g%3E%3Cg transform='translate(36 44)'%3E%3Ccircle cx='0' cy='0' r='6.5' fill='%23e8a3b0'/%3E%3Ccircle cx='-2' cy='-1' r='4' fill='%23d48fa5'/%3E%3Ccircle cx='0' cy='-.5' r='1.5' fill='%23a0607a'/%3E%3C/g%3E%3Cg fill='%23d48fa5'%3E%3Cg transform='translate(64 54)'%3E%3Ccircle cx='0' cy='-2.6' r='1.8'/%3E%3Ccircle cx='2.4' cy='-.8' r='1.8'/%3E%3Ccircle cx='1.5' cy='2.3' r='1.8'/%3E%3Ccircle cx='-1.5' cy='2.3' r='1.8'/%3E%3Ccircle cx='-2.4' cy='-.8' r='1.8'/%3E%3Ccircle cx='0' cy='0' r='1.1' fill='%23e6c25a'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") top left / 110px 90px no-repeat,
    linear-gradient(180deg, #faf6ee 0%, #f4ead5 100%);
}

.sch-border-preview[data-preview="vintage"] .sch-preview-paper {
  background: radial-gradient(ellipse at center, #ebdbb0 0%, #d9c37f 70%, #b8a055 100%);
  box-shadow: inset 0 0 0 1px #6b4f2a, inset 0 0 0 2px #ebdbb0, inset 0 0 0 3px #6b4f2a;
}
.sch-border-preview[data-preview="vintage"] .sch-preview-paper::before {
  background:
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='34' height='34' viewBox='0 0 34 34'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='1' stroke-linecap='round'%3E%3Cpath d='M4 16 Q 4 4 16 4'/%3E%3Cpath d='M9 16 Q 9 9 16 9'/%3E%3Cpath d='M4 20 L 14 20 M 20 4 L 20 14'/%3E%3Ccircle cx='11' cy='11' r='1' fill='%234a3426'/%3E%3Ccircle cx='16' cy='16' r='1' fill='%234a3426'/%3E%3C/g%3E%3C/svg%3E") top left / 34px 34px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='34' height='34' viewBox='0 0 34 34'%3E%3Cg transform='translate(34 0) scale(-1 1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='1' stroke-linecap='round'%3E%3Cpath d='M4 16 Q 4 4 16 4'/%3E%3Cpath d='M9 16 Q 9 9 16 9'/%3E%3Ccircle cx='11' cy='11' r='1' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") top right / 34px 34px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='34' height='34' viewBox='0 0 34 34'%3E%3Cg transform='translate(0 34) scale(1 -1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='1' stroke-linecap='round'%3E%3Cpath d='M4 16 Q 4 4 16 4'/%3E%3Cpath d='M9 16 Q 9 9 16 9'/%3E%3Ccircle cx='11' cy='11' r='1' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") bottom left / 34px 34px no-repeat,
    url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='34' height='34' viewBox='0 0 34 34'%3E%3Cg transform='translate(34 34) scale(-1 -1)'%3E%3Cg fill='none' stroke='%234a3426' stroke-width='1' stroke-linecap='round'%3E%3Cpath d='M4 16 Q 4 4 16 4'/%3E%3Cpath d='M9 16 Q 9 9 16 9'/%3E%3Ccircle cx='11' cy='11' r='1' fill='%234a3426'/%3E%3C/g%3E%3C/g%3E%3C/svg%3E") bottom right / 34px 34px no-repeat;
}

.sch-border-preview[data-preview="none"] .sch-preview-paper {
  background: var(--sch-bg);
  border: 1px dashed var(--sch-ink-faint);
  box-shadow: none;
}

.sch-border-label { padding: 8px 10px; font-size: 13px; color: var(--sch-ink); font-family: var(--sch-font-heading); background: var(--sch-paper); text-align: center; }

/* ══ List-items modal (meal planner + any list) ══ */
.sch-listitems-hint {
  background: color-mix(in srgb, var(--sch-accent) 8%, transparent);
  border-left: 3px solid var(--sch-accent);
  padding: 8px 10px; margin-bottom: 10px; font-size: 12px;
  color: var(--sch-ink-dim); border-radius: 4px;
}
.sch-listitems {
  list-style: none; padding: 0; margin: 0 0 12px;
  display: flex; flex-direction: column; gap: 4px;
  max-height: 360px; overflow-y: auto;
}
.sch-listitem {
  display: flex; align-items: center; gap: 8px;
  padding: 6px 8px; border: 1px solid var(--sch-border);
  border-radius: 4px; background: var(--sch-paper);
}
.sch-listitem.checked { opacity: 0.55; }
.sch-listitem.checked .sch-listitem-text { text-decoration: line-through; }
.sch-listitem-check {
  width: 16px; height: 16px; border: 1px solid var(--sch-ink-dim);
  border-radius: 3px; cursor: pointer; background: transparent;
  display: flex; align-items: center; justify-content: center;
  padding: 0; font-size: 11px; line-height: 1; color: var(--sch-accent);
}
.sch-listitem-text { flex: 1; font-size: 13px; color: var(--sch-ink); }
.sch-listitem-del {
  background: transparent; border: none; color: var(--sch-ink-faint);
  cursor: pointer; font-size: 14px; padding: 0 4px;
}
.sch-listitem-del:hover { color: #e11d48; }
.sch-listitems-add { display: flex; gap: 6px; align-items: stretch; }
.sch-listitems-add .sch-input { flex: 1; }
"##;

// ══════════════════════════════════════════════════════════════════════
// Page JS — init, view rendering, theme picker, CRUD wiring
// ══════════════════════════════════════════════════════════════════════

const PAGE_JS: &str = r##"
(function() {
  const TOKEN = (function() {
    try { return localStorage.getItem('syntaur_token') || sessionStorage.getItem('syntaur_token') || ''; } catch(e) { return ''; }
  })();
  if (!TOKEN) { location.href = '/'; return; }

  // ── State ───────────────────────────────────────────────────────────
  const S = {
    view: 'month',         // month | week | day
    cursor: new Date(),    // date anchoring the current view
    events: [],            // all loaded events (calendar_events rows)
    lists: [],
    habits: [],
    habitEntries: {},      // habit_id → Set of YYYY-MM-DD
    approvals: [],
    patterns: [],
    schoolFeeds: [],
    meetingPrep: [],
    prefs: { theme: 'garden', default_view: 'month', week_starts_on: 1, border: 'notebook' },
    editEvent: null,       // event being edited in the modal
  };

  // ── Themes ──────────────────────────────────────────────────────────
  const THEMES = [
    { key: 'garden',     name: 'Garden',        accent: '#84a98c', bg: '#f2eedc', ink: '#2e3a2c' },
    { key: 'paper',      name: 'Paper & Ink',   accent: '#3d5a3d', bg: '#ede5d0', ink: '#2c2820' },
    { key: 'midnight',   name: 'Midnight',      accent: '#d4a648', bg: '#0b0e14', ink: '#e8e6dc' },
    { key: 'linen',      name: 'Linen',         accent: '#1f2a44', bg: '#f5f0e6', ink: '#1f2a44' },
    { key: 'desert',     name: 'Desert',        accent: '#b4572e', bg: '#e9ddc7', ink: '#4a3426' },
    { key: 'stationery', name: 'Stationery',    accent: '#5788c7', bg: '#f5f7fa', ink: '#17233b' },
    { key: 'winter',     name: 'Winter',        accent: '#5f7a96', bg: '#e6ebf0', ink: '#2a3544' },
    { key: 'cafe',       name: 'Café',          accent: '#b6834a', bg: '#efe3d2', ink: '#3a2618' },
  ];

  function applyTheme(key) {
    document.body.setAttribute('data-sch-theme', key);
    S.prefs.theme = key;
  }

  const BORDERS = [
    // Clean watercolor-corner backdrop — cream paper with botanical
    // accents at each corner, middle is open for content. The default.
    { key: 'garden-backdrop', name: 'Garden Backdrop', painted: true },
    { key: 'notebook',        name: 'Spiral notebook' },
    { key: 'disc',            name: 'Disc-bound' },
    { key: 'moleskine',       name: 'Moleskine' },
    { key: 'washi',           name: 'Washi tape' },
    { key: 'ruled',           name: 'Legal pad + wire' },
    { key: 'gold-corners',    name: 'Gilded edge' },
    { key: 'pressed-flowers', name: 'Floral bouquet' },
    { key: 'vintage',         name: 'Vintage parchment' },
    { key: 'none',            name: 'Clean' },
  ];
  function applyBorder(key) {
    document.body.setAttribute('data-sch-border', key || 'garden-backdrop');
    S.prefs.border = key || 'garden-backdrop';
  }

  // Backdrop position/scale — driven by CSS vars on the shell so the
  // painting stays placed correctly as the window resizes. Values are
  // fractions: x/y in [0,1] are CSS background-position percentages;
  // sx / sy are per-axis multipliers on shell width/height. sy=0 is a
  // sentinel meaning "auto — aspect-preserve" (the initial default).
  function applyBackdrop(x, y, sx, sy) {
    const shell = document.querySelector('.sch-shell');
    if (!shell) return;
    const clamped_x  = Math.max(-1, Math.min(2, x  ?? 0.5));
    const clamped_y  = Math.max(-1, Math.min(2, y  ?? 0.5));
    const clamped_sx = Math.max(0.25, Math.min(4, sx ?? 1));
    const clamped_sy = Math.max(0,    Math.min(4, sy ?? 0));
    shell.style.setProperty('--sch-bp-x', (clamped_x * 100).toFixed(2) + '%');
    shell.style.setProperty('--sch-bp-y', (clamped_y * 100).toFixed(2) + '%');
    shell.style.setProperty('--sch-bp-w', (clamped_sx * 100).toFixed(2) + '%');
    shell.style.setProperty('--sch-bp-h', clamped_sy > 0 ? (clamped_sy * 100).toFixed(2) + '%' : 'auto');
    S.prefs.backdrop_x = clamped_x;
    S.prefs.backdrop_y = clamped_y;
    S.prefs.backdrop_scale_x = clamped_sx;
    S.prefs.backdrop_scale_y = clamped_sy;
  }

  // ── Adjust-backdrop mode ──────────────────────────────────────────
  // Click the "Adjust" button in the frame picker to enter. Drag the
  // backdrop, scroll to zoom. Toolbar at the bottom to Reset / Save / Cancel.
  let schBackdropEditState = null;

  window.schEnterBackdropEdit = function() {
    const shell = document.querySelector('.sch-shell');
    if (!shell) return;
    if (schBackdropEditState) return; // already editing
    if ((S.prefs.border || 'garden-backdrop') !== 'garden-backdrop') {
      applyBorder('garden-backdrop');
    }
    schBackdropEditState = {
      original: {
        x:  S.prefs.backdrop_x         ?? 0.5,
        y:  S.prefs.backdrop_y         ?? 0.5,
        sx: S.prefs.backdrop_scale_x   ?? 1,
        sy: S.prefs.backdrop_scale_y   ?? 0,
      },
      current: {
        x:  S.prefs.backdrop_x         ?? 0.5,
        y:  S.prefs.backdrop_y         ?? 0.5,
        sx: S.prefs.backdrop_scale_x   ?? 1,
        sy: S.prefs.backdrop_scale_y   ?? 0,
      },
      dragging: false,
      startMouse: null,
      startCurrent: null,
    };
    shell.classList.add('sch-editing-backdrop');
    schCloseBorders();

    // Drag: mousedown on shell, pointer move, mouseup. Translate mouse
    // delta into fraction-of-shell delta and add to x/y.
    const onDown = (e) => {
      if (e.button !== 0) return;
      schBackdropEditState.dragging = true;
      schBackdropEditState.startMouse = { x: e.clientX, y: e.clientY };
      schBackdropEditState.startCurrent = { ...schBackdropEditState.current };
      e.preventDefault();
    };
    const onMove = (e) => {
      if (!schBackdropEditState || !schBackdropEditState.dragging) return;
      const rect = shell.getBoundingClientRect();
      const dx = (e.clientX - schBackdropEditState.startMouse.x) / rect.width;
      const dy = (e.clientY - schBackdropEditState.startMouse.y) / rect.height;
      // Dragging the image right (positive dx) should pan the image right,
      // which in CSS background-position is a HIGHER percentage on the
      // left side. For `background-position: X% Y%`, increasing X moves
      // the image's focal point further right, exposing the LEFT portion
      // of the image. So we subtract dx to match user intuition.
      schBackdropEditState.current.x = schBackdropEditState.startCurrent.x - dx;
      schBackdropEditState.current.y = schBackdropEditState.startCurrent.y - dy;
      schBackdropApplyCurrent();
    };
    const onUp = () => {
      if (schBackdropEditState) schBackdropEditState.dragging = false;
    };
    // Wheel zoom:
    //   plain      → uniform (width + height)
    //   shift+wh   → width only
    //   alt+wh     → height only (starts from 1.0 if currently "auto")
    const onWheel = (e) => {
      if (!schBackdropEditState) return;
      e.preventDefault();
      const delta = e.deltaY < 0 ? 1.05 : 1 / 1.05;
      const cur = schBackdropEditState.current;
      if (e.shiftKey && !e.altKey) {
        cur.sx *= delta;
      } else if (e.altKey && !e.shiftKey) {
        if (cur.sy === 0) cur.sy = 1; // lift from aspect-auto
        cur.sy *= delta;
      } else {
        cur.sx *= delta;
        if (cur.sy === 0) cur.sy = 1;
        cur.sy *= delta;
      }
      schBackdropApplyCurrent();
    };

    function schBackdropApplyCurrent() {
      const c = schBackdropEditState.current;
      applyBackdrop(c.x, c.y, c.sx, c.sy);
    }
    window.schBackdropNudgeW = function(factor) {
      if (!schBackdropEditState) return;
      schBackdropEditState.current.sx *= factor;
      schBackdropApplyCurrent();
    };
    window.schBackdropNudgeH = function(factor) {
      if (!schBackdropEditState) return;
      if (schBackdropEditState.current.sy === 0) schBackdropEditState.current.sy = 1;
      schBackdropEditState.current.sy *= factor;
      schBackdropApplyCurrent();
    };
    window.schBackdropAutoH = function() {
      if (!schBackdropEditState) return;
      schBackdropEditState.current.sy = 0;
      schBackdropApplyCurrent();
    };
    schBackdropEditState.handlers = { onDown, onMove, onUp, onWheel };
    shell.addEventListener('mousedown', onDown);
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    shell.addEventListener('wheel', onWheel, { passive: false });

    // Toolbar
    const bar = document.createElement('div');
    bar.className = 'sch-backdrop-toolbar';
    bar.id = 'sch-backdrop-toolbar';
    bar.innerHTML = `
      <span class="hint">Drag to pan · scroll = zoom · ⇧+scroll = width · ⌥+scroll = height</span>
      <button onclick="schBackdropNudgeW(1/1.05)" title="Narrower">−W</button>
      <button onclick="schBackdropNudgeW(1.05)"  title="Wider">+W</button>
      <button onclick="schBackdropNudgeH(1/1.05)" title="Shorter">−H</button>
      <button onclick="schBackdropNudgeH(1.05)"  title="Taller">+H</button>
      <button onclick="schBackdropAutoH()" title="Lock height to image aspect">Auto H</button>
      <button onclick="schBackdropReset()">Reset</button>
      <button onclick="schBackdropCancel()">Cancel</button>
      <button class="btn-save" onclick="schBackdropSave()">Save</button>
    `;
    document.body.appendChild(bar);
  };

  function schBackdropExit() {
    if (!schBackdropEditState) return;
    const shell = document.querySelector('.sch-shell');
    if (shell) {
      shell.classList.remove('sch-editing-backdrop');
      const h = schBackdropEditState.handlers;
      if (h) {
        shell.removeEventListener('mousedown', h.onDown);
        window.removeEventListener('mousemove', h.onMove);
        window.removeEventListener('mouseup', h.onUp);
        shell.removeEventListener('wheel', h.onWheel);
      }
    }
    const bar = document.getElementById('sch-backdrop-toolbar');
    if (bar) bar.remove();
    schBackdropEditState = null;
  }

  window.schBackdropReset = function() {
    if (!schBackdropEditState) return;
    schBackdropEditState.current = { x: 0.5, y: 0.5, sx: 1, sy: 0 };
    applyBackdrop(0.5, 0.5, 1, 0);
  };
  window.schBackdropCancel = function() {
    if (!schBackdropEditState) return;
    const o = schBackdropEditState.original;
    applyBackdrop(o.x, o.y, o.sx, o.sy);
    schBackdropExit();
  };
  window.schBackdropSave = async function() {
    if (!schBackdropEditState) return;
    const c = schBackdropEditState.current;
    try {
      await api('/api/scheduler/prefs', {
        method: 'POST',
        body: JSON.stringify({
          backdrop_x: c.x,
          backdrop_y: c.y,
          backdrop_scale_x: c.sx,
          backdrop_scale_y: c.sy,
        }),
      });
    } catch (e) { console.warn('[sch] save backdrop', e); }
    schBackdropExit();
  };

  // ── Modal UX: backdrop click dismisses, ESC closes top modal ──────
  // Every `.sch-modal` that wraps a `.sch-modal-box` picks this up: clicking
  // the dimmed area outside the box hides the modal. Native `prompt()` /
  // `confirm()` / `alert()` are replaced by schPrompt / schConfirm / schAlert
  // so chrome stays themed inside the WebKitGTK viewer.
  document.addEventListener('click', function(ev) {
    const m = ev.target;
    if (m && m.classList && m.classList.contains('sch-modal') && !m.hidden) {
      // Click was on the backdrop itself (not a descendant).
      m.hidden = true;
      // Dynamic modals (stickers, prompts) remove themselves on close.
      if (m.dataset && m.dataset.ephemeral === '1') setTimeout(() => m.remove(), 0);
    }
  });
  document.addEventListener('keydown', function(ev) {
    if (ev.key !== 'Escape') return;
    const visible = Array.from(document.querySelectorAll('.sch-modal')).filter(m => !m.hidden);
    const top = visible[visible.length - 1];
    if (!top) return;
    top.hidden = true;
    if (top.dataset && top.dataset.ephemeral === '1') setTimeout(() => top.remove(), 0);
    ev.preventDefault();
  });

  // Themed prompt / confirm / alert. All return Promises so call sites
  // stay linear — no more native popups breaking the theme inside WebKitGTK.
  function schDialog(opts) {
    return new Promise((resolve) => {
      const fields = opts.fields || [];
      const m = document.createElement('div');
      m.className = 'sch-modal';
      m.dataset.ephemeral = '1';
      const fieldsHtml = fields.map((f, i) => {
        const id = 'sch-dlg-f' + i;
        const input = f.type === 'textarea'
          ? `<textarea id="${id}" class="sch-input" rows="${f.rows || 3}" placeholder="${escAttr(f.placeholder || '')}">${escHtml(f.default || '')}</textarea>`
          : `<input id="${id}" type="${f.type || 'text'}" class="sch-input" placeholder="${escAttr(f.placeholder || '')}" value="${escAttr(f.default || '')}">`;
        return `<label><span>${escHtml(f.label || '')}</span>${input}</label>`;
      }).join('');
      const msg = opts.message ? `<p class="sch-dialog-msg">${escHtml(opts.message)}</p>` : '';
      const okClass  = opts.danger ? 'sch-btn-danger-solid' : 'sch-btn-primary';
      const cancel   = opts.hideCancel ? '' : `<button class="sch-btn-ghost" data-act="cancel">${escHtml(opts.cancelLabel || 'Cancel')}</button>`;
      m.innerHTML = `
        <div class="sch-modal-box" style="max-width:${opts.width || 420}px">
          <div class="sch-modal-head"><h2>${escHtml(opts.title || '')}</h2><button class="sch-modal-close" data-act="cancel">×</button></div>
          <div class="sch-modal-body">${msg}${fieldsHtml}</div>
          <div class="sch-modal-foot">${cancel}<button class="${okClass}" data-act="ok">${escHtml(opts.okLabel || 'OK')}</button></div>
        </div>`;
      document.body.appendChild(m);
      const first = m.querySelector('input,textarea'); if (first) setTimeout(() => first.focus(), 30);
      const close = (val) => { m.remove(); resolve(val); };
      m.addEventListener('click', (ev) => {
        const act = ev.target && ev.target.getAttribute && ev.target.getAttribute('data-act');
        if (act === 'cancel') { ev.stopPropagation(); close(null); }
        else if (act === 'ok') { ev.stopPropagation(); close(collect()); }
      });
      m.addEventListener('keydown', (ev) => {
        if (ev.key === 'Escape') { ev.preventDefault(); close(null); }
        if (ev.key === 'Enter' && ev.target.tagName !== 'TEXTAREA') { ev.preventDefault(); close(collect()); }
      });
      function collect() {
        if (fields.length === 0) return true;
        const out = {};
        fields.forEach((f, i) => {
          const el = m.querySelector('#sch-dlg-f' + i);
          out[f.name || 'value'] = el ? el.value : '';
        });
        return fields.length === 1 ? out[fields[0].name || 'value'] : out;
      }
    });
  }
  window.schPrompt = (title, placeholder, def) =>
    schDialog({ title, fields: [{ name: 'value', label: '', placeholder: placeholder || '', default: def || '' }] });
  window.schConfirm = (title, message, opts) =>
    schDialog({ title, message: message || '', okLabel: (opts && opts.okLabel) || 'OK', cancelLabel: (opts && opts.cancelLabel) || 'Cancel', danger: !!(opts && opts.danger), hideCancel: false })
      .then(v => v !== null);
  window.schAlert = (title, message) =>
    schDialog({ title, message: message || '', okLabel: 'OK', hideCancel: true }).then(() => true);

  // ── API helpers ─────────────────────────────────────────────────────
  // Phase 1.1 of the security plan: send the session token via
  // Authorization: Bearer only, never in the URL query or body. Server-
  // side middleware (`security::lift_bearer_to_body_and_query`) lifts it
  // back into query + body so existing handlers keep reading
  // `params.get("token")` / `body["token"]` without a refactor.
  async function api(path, opts) {
    opts = opts || {};
    opts.headers = Object.assign({
      'Content-Type': 'application/json',
      'Authorization': 'Bearer ' + TOKEN,
    }, opts.headers || {});
    const r = await fetch(path, opts);
    if (!r.ok) throw new Error('HTTP ' + r.status);
    return r.json();
  }

  // ── Load ────────────────────────────────────────────────────────────
  async function loadAll() {
    try {
      const prefs = await api('/api/scheduler/prefs');
      if (prefs && prefs.theme) applyTheme(prefs.theme);
      if (prefs && prefs.border) applyBorder(prefs.border);
      else applyBorder('garden-backdrop');
      if (prefs && prefs.default_view && window.innerWidth > 900) { S.view = prefs.default_view; }
      S.prefs = Object.assign(S.prefs, prefs || {});
      // Apply saved backdrop position + scale if present. Values live on
      // the shell via CSS variables and are percentage/multiplier-based
      // so the placement stays put when the viewport resizes.
      applyBackdrop(
        prefs && prefs.backdrop_x,
        prefs && prefs.backdrop_y,
        prefs && prefs.backdrop_scale_x,
        prefs && prefs.backdrop_scale_y
      );
    } catch(e) { console.warn('[sch] prefs load:', e); applyBorder('garden-backdrop'); }
    if (window.innerWidth <= 900) S.view = 'day';

    try {
      const start = windowStart();
      const end = windowEnd();
      const r = await api(`/api/calendar?start=${encodeURIComponent(start)}&end=${encodeURIComponent(end)}`);
      S.events = (r && r.events) || r || [];
    } catch(e) { console.warn('[sch] events load:', e); S.events = []; }

    try {
      const r = await api('/api/scheduler/lists');
      S.lists = (r && r.lists) || [];
    } catch(e) { console.warn('[sch] lists load:', e); }

    try {
      const r = await api('/api/scheduler/habits');
      S.habits = (r && r.habits) || [];
      S.habitEntries = {};
      (r && r.entries || []).forEach(e => {
        if (!S.habitEntries[e.habit_id]) S.habitEntries[e.habit_id] = new Set();
        if (e.done) S.habitEntries[e.habit_id].add(e.date);
      });
    } catch(e) { console.warn('[sch] habits load:', e); }

    try {
      const r = await api('/api/approvals?status=pending');
      S.approvals = (r && r.approvals) || [];
    } catch(e) { console.warn('[sch] approvals load:', e); }

    try { await loadSchoolFeeds(); } catch(e) {}
    try { await loadMeetingPrep(); } catch(e) {}

    renderAll();
  }

  // ── Period math ─────────────────────────────────────────────────────
  function fmtDate(d) {
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    return `${y}-${m}-${day}`;
  }
  function startOfMonth(d) { return new Date(d.getFullYear(), d.getMonth(), 1); }
  function endOfMonth(d)   { return new Date(d.getFullYear(), d.getMonth() + 1, 0, 23,59,59); }
  function startOfWeek(d)  {
    const x = new Date(d);
    const wk = S.prefs.week_starts_on || 1;
    const delta = (x.getDay() + 7 - (wk % 7)) % 7;
    x.setDate(x.getDate() - delta);
    x.setHours(0,0,0,0);
    return x;
  }
  function endOfWeek(d) { const s = startOfWeek(d); const e = new Date(s); e.setDate(e.getDate()+6); e.setHours(23,59,59,999); return e; }
  function addDays(d, n) { const x = new Date(d); x.setDate(x.getDate()+n); return x; }
  function sameDay(a, b) { return a.getFullYear()===b.getFullYear() && a.getMonth()===b.getMonth() && a.getDate()===b.getDate(); }

  function windowStart() {
    if (S.view === 'month') return fmtDate(addDays(startOfMonth(S.cursor), -7));
    if (S.view === 'week')  return fmtDate(startOfWeek(S.cursor));
    return fmtDate(S.cursor);
  }
  function windowEnd() {
    if (S.view === 'month') return fmtDate(addDays(endOfMonth(S.cursor), 7));
    if (S.view === 'week')  return fmtDate(endOfWeek(S.cursor));
    return fmtDate(S.cursor);
  }
  function periodLabel() {
    const MO = ['January','February','March','April','May','June','July','August','September','October','November','December'];
    if (S.view === 'month') return MO[S.cursor.getMonth()] + ' ' + S.cursor.getFullYear();
    if (S.view === 'week')  { const s = startOfWeek(S.cursor), e = addDays(s,6);
      return `${MO[s.getMonth()].slice(0,3)} ${s.getDate()} – ${MO[e.getMonth()].slice(0,3)} ${e.getDate()}, ${e.getFullYear()}`; }
    return `${MO[S.cursor.getMonth()]} ${S.cursor.getDate()}, ${S.cursor.getFullYear()}`;
  }

  function eventColor(ev) {
    if (ev.color && ev.color.length) return ev.color;
    const s = (ev.source || '').toLowerCase();
    if (s.includes('google'))  return '#3b82f6';
    if (s.includes('outlook')) return '#6366f1';
    if (s.includes('icloud'))  return '#059669';
    if (s.includes('teams'))   return '#0d9488';
    return getComputedStyle(document.body).getPropertyValue('--sch-accent').trim() || '#84a98c';
  }

  function eventsOnDay(d) {
    const key = fmtDate(d);
    return S.events.filter(ev => {
      const es = (ev.start_time || ev.start || '').slice(0,10);
      const ee = (ev.end_time   || ev.end   || es).slice(0,10);
      return es <= key && key <= ee;
    });
  }

  // ── Render orchestrator ────────────────────────────────────────────
  function renderAll() {
    document.getElementById('sch-period-label').textContent = periodLabel();
    renderMiniCal();
    renderLists();
    renderHabits();
    renderProposals();
    renderTimeline();
    if (S.view === 'month') renderMonth();
    if (S.view === 'week')  renderWeek();
    if (S.view === 'day')   renderDay();
    document.querySelectorAll('.sch-view-btn').forEach(b => b.classList.toggle('active', b.dataset.view === S.view));
    document.querySelectorAll('.sch-view').forEach(v => v.classList.toggle('sch-view-active', v.id === 'view-' + S.view));
  }

  // ── Mini calendar ──────────────────────────────────────────────────
  function renderMiniCal() {
    const MO = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
    document.getElementById('sch-mini-label').textContent = MO[S.cursor.getMonth()] + ' ' + S.cursor.getFullYear();
    const grid = document.getElementById('sch-mini-grid');
    const dows = ['Mo','Tu','We','Th','Fr','Sa','Su'];
    let html = dows.map(d => `<div class="dow">${d}</div>`).join('');
    const first = startOfMonth(S.cursor);
    const offset = (first.getDay() + 6) % 7;
    const startDay = addDays(first, -offset);
    const today = new Date();
    const hasEvents = new Set(S.events.map(ev => (ev.start_time||ev.start||'').slice(0,10)));
    for (let i = 0; i < 42; i++) {
      const d = addDays(startDay, i);
      const key = fmtDate(d);
      const classes = ['sch-mini-day'];
      if (d.getMonth() !== S.cursor.getMonth()) classes.push('other-month');
      if (sameDay(d, today)) classes.push('today');
      if (hasEvents.has(key)) classes.push('has-events');
      html += `<button class="${classes.join(' ')}" onclick="schJumpTo('${key}')">${d.getDate()}</button>`;
    }
    grid.innerHTML = html;
  }
  window.schMiniNav = function(dir) { S.cursor = new Date(S.cursor.getFullYear(), S.cursor.getMonth()+dir, 1); loadAll(); };
  window.schJumpTo  = function(key) { const [y,m,d] = key.split('-').map(Number); S.cursor = new Date(y, m-1, d); renderAll(); };

  // ── Month view ─────────────────────────────────────────────────────
  function renderMonth() {
    const grid = document.getElementById('sch-month-grid');
    const first = startOfMonth(S.cursor);
    const offset = (first.getDay() + 6) % 7;
    const startDay = addDays(first, -offset);
    const today = new Date();
    let html = '';
    for (let i = 0; i < 42; i++) {
      const d = addDays(startDay, i);
      const isOther = d.getMonth() !== S.cursor.getMonth();
      const isToday = sameDay(d, today);
      const isWeekend = d.getDay() === 0 || d.getDay() === 6;
      const evs = eventsOnDay(d);
      const chips = evs.slice(0, 3).map(ev => {
        const c = eventColor(ev);
        const title = ev.title || '(untitled)';
        const time  = (ev.start_time || ev.start || '').slice(11,16);
        const timeStr = time && !ev.all_day ? `${time} ` : '';
        const pending = ev._pending ? ' pending' : '';
        return `<div class="sch-event-chip${pending}" style="--chip-color:${c}" onclick="event.stopPropagation();schOpenEvent(${ev.id})" title="${escAttr(title)}">${escHtml(timeStr)}${escHtml(title)}</div>`;
      }).join('');
      const more = evs.length > 3 ? `<div class="sch-event-overflow">+${evs.length - 3} more</div>` : '';
      const classes = ['sch-month-cell'];
      if (isOther) classes.push('other-month');
      if (isToday) classes.push('today');
      if (isWeekend) classes.push('weekend');
      html += `<div class="${classes.join(' ')}" onclick="schClickDay('${fmtDate(d)}')">`
            + `<div><span class="sch-date-num">${d.getDate()}</span></div>`
            + chips + more
            + '</div>';
    }
    grid.innerHTML = html;
  }
  window.schClickDay = function(key) {
    // Click on empty day area → quick-create event at that date, default 9am – 10am
    const start = key + 'T09:00';
    const end = key + 'T10:00';
    openEventModal({ title: '', start_time: start, end_time: end, id: null });
  };

  // ── Week view ──────────────────────────────────────────────────────
  function renderWeek() {
    const head = document.getElementById('sch-week-head');
    const body = document.getElementById('sch-week-body');
    const ws = startOfWeek(S.cursor);
    const today = new Date();
    const DOW = ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];
    let headHtml = '<div class="sch-week-dow"></div>';
    for (let i = 0; i < 7; i++) {
      const d = addDays(ws, i);
      const isToday = sameDay(d, today);
      headHtml += `<div class="sch-week-dow${isToday?' today':''}">
        <div class="sch-week-dow-name">${DOW[i]}</div>
        <div class="sch-week-dow-num">${d.getDate()}</div>
      </div>`;
    }
    head.innerHTML = headHtml;

    // Build 24 hour rows × 7 day columns
    let bodyHtml = '<div class="sch-week-hour-col">';
    for (let h = 0; h < 24; h++) {
      const label = (h === 0 ? '12 am' : h < 12 ? h + ' am' : h === 12 ? '12 pm' : (h - 12) + ' pm');
      bodyHtml += `<div class="sch-week-hour-label">${label}</div>`;
    }
    bodyHtml += '</div>';
    for (let i = 0; i < 7; i++) {
      const d = addDays(ws, i);
      const dKey = fmtDate(d);
      const isWeekend = d.getDay() === 0 || d.getDay() === 6;
      bodyHtml += `<div class="sch-week-day-col${isWeekend?' weekend':''}" data-date="${dKey}">`;
      for (let h = 0; h < 24; h++) {
        bodyHtml += `<div class="sch-week-hour-slot" onclick="schWeekSlotClick('${dKey}', ${h})"></div>`;
      }
      // Overlay events
      const evs = eventsOnDay(d);
      evs.forEach(ev => {
        const st = new Date(ev.start_time || ev.start);
        const en = new Date(ev.end_time   || ev.end || st);
        const topMin  = st.getHours() * 60 + st.getMinutes();
        const endMin  = en.getHours() * 60 + en.getMinutes();
        const top = topMin; // 1 min = 1 px at 60px/hr
        const h   = Math.max(20, endMin - topMin);
        const c   = eventColor(ev);
        const tstr = `${String(st.getHours()).padStart(2,'0')}:${String(st.getMinutes()).padStart(2,'0')}`;
        bodyHtml += `<div class="sch-week-event" data-event-id="${ev.id}" style="top:${top}px;height:${h}px;--chip-color:${c};background:${c}" onpointerdown="schEventDragStart(event,${ev.id})" onclick="event.stopPropagation();schOpenEvent(${ev.id})">`
          + `<div class="ev-title">${escHtml(ev.title||'(untitled)')}</div>`
          + `<div class="ev-time">${tstr}</div>`
          + `<div class="sch-resize-handle" onpointerdown="schEventResizeStart(event,${ev.id})"></div>`
          + '</div>';
      });
      // Now-line on today's column
      if (sameDay(d, today)) {
        const mins = today.getHours() * 60 + today.getMinutes();
        bodyHtml += `<div class="sch-week-now-line" style="top:${mins}px"></div>`;
      }
      bodyHtml += '</div>';
    }
    body.innerHTML = bodyHtml;
  }
  window.schWeekSlotClick = function(dKey, hour) {
    const start = `${dKey}T${String(hour).padStart(2,'0')}:00`;
    const end   = `${dKey}T${String(Math.min(hour+1,23)).padStart(2,'0')}:00`;
    openEventModal({ title: '', start_time: start, end_time: end, id: null });
  };

  // ── Day view ───────────────────────────────────────────────────────
  function renderDay() {
    const d = S.cursor;
    const DOW = ['Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday'];
    document.getElementById('sch-day-head').innerHTML =
      `<div class="sch-day-label">${DOW[d.getDay()]}, ${d.toLocaleDateString()}</div>`
      + `<div class="sch-day-sub">${eventsOnDay(d).length} item(s)</div>`;
    const body = document.getElementById('sch-day-body');
    let html = '<div class="sch-week-hour-col">';
    for (let h = 0; h < 24; h++) {
      const label = (h === 0 ? '12 am' : h < 12 ? h + ' am' : h === 12 ? '12 pm' : (h - 12) + ' pm');
      html += `<div class="sch-week-hour-label">${label}</div>`;
    }
    html += '</div><div class="sch-week-day-col">';
    for (let h = 0; h < 24; h++) {
      html += `<div class="sch-week-hour-slot" onclick="schWeekSlotClick('${fmtDate(d)}', ${h})"></div>`;
    }
    const evs = eventsOnDay(d);
    evs.forEach(ev => {
      const st = new Date(ev.start_time || ev.start);
      const en = new Date(ev.end_time || ev.end || st);
      const top = st.getHours()*60+st.getMinutes();
      const h   = Math.max(20, (en.getHours()*60+en.getMinutes()) - top);
      const c   = eventColor(ev);
      html += `<div class="sch-week-event" data-event-id="${ev.id}" style="top:${top}px;height:${h}px;background:${c}" onpointerdown="schEventDragStart(event,${ev.id})" onclick="event.stopPropagation();schOpenEvent(${ev.id})">`
        + `<div class="ev-title">${escHtml(ev.title||'(untitled)')}</div>`
        + `<div class="sch-resize-handle" onpointerdown="schEventResizeStart(event,${ev.id})"></div></div>`;
    });
    const today = new Date();
    if (sameDay(d, today)) {
      const mins = today.getHours()*60+today.getMinutes();
      html += `<div class="sch-week-now-line" style="top:${mins}px"></div>`;
    }
    html += '</div>';
    body.innerHTML = html;
  }

  // ── View/nav controls ──────────────────────────────────────────────
  window.schSwitchView = function(v) { S.view = v; loadAll(); };
  window.schNav = function(dir) {
    if (S.view === 'month') S.cursor = new Date(S.cursor.getFullYear(), S.cursor.getMonth()+dir, 1);
    else if (S.view === 'week') S.cursor = addDays(S.cursor, 7 * dir);
    else S.cursor = addDays(S.cursor, dir);
    loadAll();
  };
  window.schGoToday = function() { S.cursor = new Date(); loadAll(); };

  // ── Event modal ────────────────────────────────────────────────────
  window.schOpenEvent = function(id) {
    const ev = S.events.find(e => e.id === id);
    if (!ev) return;
    openEventModal(ev);
  };
  function openEventModal(ev) {
    S.editEvent = ev;
    document.getElementById('sch-event-modal-title').textContent = ev.id ? 'Edit event' : 'New event';
    document.getElementById('ev-title').value = ev.title || '';
    document.getElementById('ev-start').value = (ev.start_time || ev.start || '').slice(0,16);
    document.getElementById('ev-end').value   = (ev.end_time   || ev.end   || '').slice(0,16);
    document.getElementById('ev-loc').value   = ev.location || '';
    const color = eventColor(ev);
    document.querySelectorAll('.sch-swatch').forEach(s => s.classList.toggle('selected', s.dataset.color === color));
    document.getElementById('ev-delete').style.display = ev.id ? '' : 'none';
    document.getElementById('ev-dup').style.display    = ev.id ? '' : 'none';
    document.getElementById('sch-event-modal').hidden = false;
  }
  window.schCloseEventModal = function() { document.getElementById('sch-event-modal').hidden = true; };
  window.schPickColor = function(el) {
    document.querySelectorAll('.sch-swatch').forEach(s => s.classList.remove('selected'));
    el.classList.add('selected');
    const custom = document.getElementById('ev-color-custom');
    if (custom && el !== custom) custom.value = el.dataset.color || '#84a98c';
  };
  window.schPickCustomColor = function(input) {
    document.querySelectorAll('.sch-swatch').forEach(s => s.classList.remove('selected'));
    input.dataset.color = input.value;
    input.classList.add('selected');
  };
  window.schEventSave = async function() {
    const ev = S.editEvent || {};
    const picked = document.querySelector('.sch-swatch.selected');
    const custom = document.getElementById('ev-color-custom');
    const pickedColor = picked
      ? (picked === custom ? custom.value : picked.dataset.color)
      : '';
    const payload = {
      title: document.getElementById('ev-title').value.trim() || '(untitled)',
      start_time: document.getElementById('ev-start').value,
      end_time:   document.getElementById('ev-end').value,
      location:   document.getElementById('ev-loc').value,
      color:      pickedColor,
    };
    try {
      if (ev.id) {
        const prev = { title: ev.title, start_time: ev.start_time || ev.start, end_time: ev.end_time || ev.end, location: ev.location || '', color: ev.color || '' };
        await api(`/api/calendar/${ev.id}`, { method: 'PUT',  body: JSON.stringify(payload) });
        pushUndo({ op: 'update-event', id: ev.id, prev });
      } else {
        const r = await api('/api/calendar', { method: 'POST', body: JSON.stringify(payload) });
        if (r && r.id) pushUndo({ op: 'create-event', id: r.id });
      }
      schCloseEventModal(); await loadAll();
    } catch(e) { schToast('Save failed: ' + e.message, 3000); }
  };
  window.schEventDelete = async function() {
    if (!S.editEvent || !S.editEvent.id) return;
    if (!(await schConfirm('Delete this event?', S.editEvent.title || '(untitled)', { okLabel: 'Delete', danger: true }))) return;
    const prev = {
      title: S.editEvent.title, start_time: S.editEvent.start_time, end_time: S.editEvent.end_time,
      location: S.editEvent.location || '', color: S.editEvent.color || ''
    };
    try {
      await api(`/api/calendar/${S.editEvent.id}`, { method: 'DELETE' });
      pushUndo({ op: 'delete-event', prev });
      schCloseEventModal(); await loadAll();
    } catch(e) { schToast('Delete failed: ' + e.message, 3000); }
  };
  window.schEventDuplicate = async function() {
    const ev = S.editEvent;
    if (!ev || !ev.id) return;
    const defaultDate = fmtDate(addDays(new Date(ev.start_time||ev.start), 1));
    const next = await schPrompt('Duplicate to date', 'YYYY-MM-DD', defaultDate);
    if (!next) return;
    const payload = {
      title: ev.title, location: ev.location || '', color: ev.color || '',
      start_time: next + 'T' + (ev.start_time || ev.start || '').slice(11,16),
      end_time:   next + 'T' + (ev.end_time   || ev.end   || '').slice(11,16),
    };
    try {
      const r = await api('/api/calendar', { method: 'POST', body: JSON.stringify(payload) });
      if (r && r.id) pushUndo({ op: 'create-event', id: r.id });
      schCloseEventModal(); await loadAll();
    } catch(e) { schToast('Duplicate failed: ' + e.message, 3000); }
  };

  // ── Lists + habits ─────────────────────────────────────────────────
  // Lists now expand INLINE below their row via a caret — no more
  // popup-into-another-window flow for the common case of glancing at /
  // adding to a list. Per-list expansion state lives in
  // S.listExpand (Set of list IDs currently open) and per-list items in
  // S.listItems (map of list_id → array of {id, text, checked}).
  if (!S.listExpand) S.listExpand = new Set();
  if (!S.listItems)  S.listItems  = {};

  function renderLists() {
    const el = document.getElementById('sch-lists');
    if (!el) return;
    const todosRow = `
      <li class="sch-list-row sch-list-active" data-list-id="todos" onclick="schSelectList('todos')">
        <span class="sch-list-caret"></span>
        <span class="sch-list-icon">☐</span>
        <span class="sch-list-name">Todos</span>
      </li>`;
    const customRows = S.lists.map(l => {
      const open = S.listExpand.has(l.id);
      const items = S.listItems[l.id] || [];
      const itemsHtml = open ? `
        <li class="sch-list-items" data-for="${l.id}">
          ${items.length === 0 ? '<div class="sch-list-empty">No items yet.</div>' : ''}
          <ul class="sch-inline-items">
            ${items.map(it => `
              <li class="sch-inline-item${it.checked ? ' checked' : ''}">
                <button class="sch-listitem-check" onclick="schListItemsToggle(${it.id})" title="Toggle">${it.checked ? '✓' : ''}</button>
                <span class="sch-inline-text">${escHtml(it.text)}</span>
                <button class="sch-listitem-del" onclick="schListItemsDelete(${it.id})" title="Remove">×</button>
              </li>
            `).join('')}
          </ul>
          <div class="sch-inline-add">
            <input type="text" class="sch-input" placeholder="${schListAddPlaceholder(l.id)}" onkeydown="schInlineListKey(event, ${l.id})">
            <button class="sch-btn-primary" onclick="schInlineListAdd(${l.id}, this.previousElementSibling)">Add</button>
          </div>
        </li>` : '';
      return `
        <li class="sch-list-row ${open ? 'open' : ''}" data-list-id="${l.id}" onclick="schToggleList(event, ${l.id})">
          <span class="sch-list-caret">${open ? '▾' : '▸'}</span>
          <span class="sch-list-icon" style="color:${l.color||'#94a3b8'}">${escHtml(l.icon||'•')}</span>
          <span class="sch-list-name">${escHtml(l.name)}</span>
          <button class="sch-list-open-modal" onclick="event.stopPropagation(); schOpenListItems(${l.id})" title="Open full">↗</button>
        </li>
        ${itemsHtml}`;
    }).join('');
    el.innerHTML = todosRow + customRows;
  }
  function schListAddPlaceholder(listId) {
    if (MEAL_LINK && MEAL_LINK.linked && MEAL_LINK.meal_list_id === listId) return 'Add a meal (auto-extracts ingredients)…';
    if (MEAL_LINK && MEAL_LINK.linked && MEAL_LINK.grocery_list_id === listId) return 'Add a grocery item…';
    return 'Add an item…';
  }
  window.schToggleList = async function(ev, listId) {
    ev.stopPropagation();
    if (S.listExpand.has(listId)) {
      S.listExpand.delete(listId);
    } else {
      S.listExpand.add(listId);
      await loadInlineListItems(listId);
    }
    renderLists();
  };
  async function loadInlineListItems(listId) {
    try {
      const r = await api(`/api/scheduler/lists/${listId}/items`);
      S.listItems[listId] = (r && r.items) || [];
    } catch(e) { S.listItems[listId] = []; }
  }
  window.schInlineListKey = function(ev, listId) {
    if (ev.key === 'Enter') { ev.preventDefault(); schInlineListAdd(listId, ev.target); }
  };
  window.schInlineListAdd = async function(listId, inputEl) {
    const text = (inputEl.value || '').trim();
    if (!text) return;
    inputEl.disabled = true;
    await loadMealLink();
    try {
      let createdId = null;
      let groceryIds = [];
      let groceryListId = null;
      if (MEAL_LINK && MEAL_LINK.linked && MEAL_LINK.meal_list_id === listId) {
        schToast('Thaddeus is extracting ingredients…', 2500);
        const r = await api('/api/scheduler/meal_add', { method: 'POST', body: JSON.stringify({ meal: text }) });
        createdId = r.meal_item_id || null;
        const n = r.added_to_groceries || 0;
        groceryListId = MEAL_LINK.grocery_list_id;
        // Re-read the grocery list and capture the IDs of items that weren't
        // there before — those are the ones this meal added.
        const before = (S.listItems[groceryListId] || []).map(i => i.id);
        await loadInlineListItems(groceryListId);
        groceryIds = (S.listItems[groceryListId] || []).map(i => i.id).filter(i => !before.includes(i));
        schToast(n > 0 ? `Added "${text}" + ${n} ingredient${n===1?'':'s'}` : `Added "${text}"`, 3500);
      } else {
        const r = await api(`/api/scheduler/lists/${listId}/items`, { method: 'POST', body: JSON.stringify({ text }) });
        createdId = r && r.id ? r.id : null;
      }
      inputEl.value = '';
      pushUndo({ op: 'create-list-item', id: createdId, listId, text, grocery_ids: groceryIds, grocery_list_id: groceryListId });
      await loadInlineListItems(listId);
      renderLists();
    } catch(e) { schToast('Add failed: ' + e.message, 2500); }
    finally { inputEl.disabled = false; inputEl.focus(); }
  };
  // Keep modal-based flow available for power users (opened via the ↗ button).
  window.schSelectList = function(id) { /* caret-expand handles default interaction */ };
  window.schSelectList = function(id) {
    if (id === 'todos') { /* todos handled by the dedicated todos view */ return; }
    schOpenListItems(id);
  };
  window.schNewList = async function() {
    const name = await schPrompt('New list', 'List name (e.g. Grocery, Bucket, Packing)');
    if (!name) return;
    try { await api('/api/scheduler/lists', { method: 'POST', body: JSON.stringify({ name, icon: '📋', color: '#94a3b8' }) }); await loadAll(); }
    catch(e) { schToast('Create failed: ' + e.message, 2500); }
  };

  // ── T3 #17 — Meal planner → auto-grocery linking ─────────────────
  let MEAL_LINK = null;  // { linked: bool, meal_list_id, grocery_list_id }
  async function loadMealLink() {
    try { MEAL_LINK = await api('/api/scheduler/meal_link'); }
    catch(e) { MEAL_LINK = { linked: false }; }
  }
  window.schMealSetup = async function() {
    try {
      const r = await api('/api/scheduler/meal_setup', { method: 'POST', body: JSON.stringify({}) });
      MEAL_LINK = { linked: true, meal_list_id: r.meal_list_id, grocery_list_id: r.grocery_list_id };
      schToast('Meals + Groceries linked. Add a meal to auto-populate the shopping list.', 3500);
      await loadAll();
      schOpenListItems(r.meal_list_id);
    } catch(e) { schToast('Setup failed: ' + e.message, 3000); }
  };

  // ── List-items modal (shared across all custom lists) ─────────────
  let CURRENT_LIST = null;  // { id, name, icon, color, items: [], is_meal, is_grocery }
  async function schOpenListItems(listId) {
    await loadMealLink();
    const list = S.lists.find(l => l.id === listId);
    if (!list) { schToast('List not found', 1500); return; }
    const is_meal    = MEAL_LINK && MEAL_LINK.linked && MEAL_LINK.meal_list_id    === listId;
    const is_grocery = MEAL_LINK && MEAL_LINK.linked && MEAL_LINK.grocery_list_id === listId;
    CURRENT_LIST = { id: listId, name: list.name, icon: list.icon, color: list.color, items: [], is_meal, is_grocery };
    document.getElementById('sch-listitems-title').textContent = `${list.icon || '•'} ${list.name}`;
    const hint = document.getElementById('sch-listitems-hint');
    const input = document.getElementById('sch-listitems-input');
    const addBtn = document.getElementById('sch-listitems-add-btn');
    if (is_meal) {
      hint.hidden = false;
      hint.innerHTML = `🤖 Thaddeus extracts ingredients from each meal and adds them to your Groceries list automatically.`;
      input.placeholder = 'Add a meal (e.g., chicken tacos)…';
      addBtn.textContent = 'Add meal';
    } else if (is_grocery) {
      hint.hidden = false;
      hint.innerHTML = `🛒 Linked to Meals — new meal items auto-populate here. Add one-offs directly with the box below.`;
      input.placeholder = 'Add a grocery item…';
      addBtn.textContent = 'Add';
    } else {
      hint.hidden = true;
      input.placeholder = 'Add an item…';
      addBtn.textContent = 'Add';
    }
    input.value = '';
    await loadListItems();
    document.getElementById('sch-listitems-modal').hidden = false;
    setTimeout(() => input.focus(), 50);
  }
  window.schCloseListItems = function() {
    document.getElementById('sch-listitems-modal').hidden = true;
    CURRENT_LIST = null;
  };
  async function loadListItems() {
    if (!CURRENT_LIST) return;
    try {
      const r = await api(`/api/scheduler/lists/${CURRENT_LIST.id}/items`);
      CURRENT_LIST.items = (r && r.items) || [];
    } catch(e) { CURRENT_LIST.items = []; }
    renderListItems();
  }
  function renderListItems() {
    const ul = document.getElementById('sch-listitems');
    if (!CURRENT_LIST.items.length) {
      ul.innerHTML = '<li class="sch-empty">No items yet.</li>';
      return;
    }
    ul.innerHTML = CURRENT_LIST.items.map(it => `
      <li class="sch-listitem${it.checked ? ' checked' : ''}" data-id="${it.id}">
        <button class="sch-listitem-check" onclick="schListItemsToggle(${it.id})" title="Toggle">${it.checked ? '✓' : ''}</button>
        <span class="sch-listitem-text">${escHtml(it.text)}</span>
        <button class="sch-listitem-del" onclick="schListItemsDelete(${it.id})" title="Delete">×</button>
      </li>
    `).join('');
  }
  window.schListItemsKey = function(ev) {
    if (ev.key === 'Enter') { ev.preventDefault(); schListItemsAdd(); }
  };
  window.schListItemsAdd = async function() {
    const input = document.getElementById('sch-listitems-input');
    const text = (input.value || '').trim();
    if (!text || !CURRENT_LIST) return;
    input.disabled = true;
    try {
      if (CURRENT_LIST.is_meal) {
        schToast('Thaddeus is extracting ingredients…', 2500);
        const r = await api('/api/scheduler/meal_add', { method: 'POST', body: JSON.stringify({ meal: text }) });
        const ing = (r.ingredients || []).slice(0, 6).join(', ');
        const n = r.added_to_groceries || 0;
        schToast(n > 0 ? `Added "${text}" + ${n} ingredient${n===1?'':'s'} to Groceries: ${ing}` : `Added "${text}"`, 4500);
      } else {
        await api(`/api/scheduler/lists/${CURRENT_LIST.id}/items`, { method: 'POST', body: JSON.stringify({ text }) });
      }
      input.value = '';
      await loadListItems();
    } catch(e) {
      if (e.message && e.message.indexOf('424') >= 0) {
        schToast('Set up the meal planner first (🍽 icon above Lists)', 3500);
      } else {
        schToast('Add failed: ' + e.message, 3000);
      }
    } finally {
      input.disabled = false;
      input.focus();
    }
  };
  window.schListItemsToggle = async function(id) {
    try { await api(`/api/scheduler/list_items/${id}/toggle`, { method: 'POST', body: JSON.stringify({}) }); }
    catch(e) { schToast('Toggle failed', 1500); return; }
    // Refresh both the modal view (if open) and every expanded inline list.
    if (CURRENT_LIST) await loadListItems();
    for (const lid of S.listExpand) await loadInlineListItems(lid);
    renderLists();
  };
  window.schListItemsDelete = async function(id) {
    try { await api(`/api/scheduler/list_items/${id}`, { method: 'DELETE' }); }
    catch(e) { schToast('Delete failed', 1500); return; }
    if (CURRENT_LIST) await loadListItems();
    for (const lid of S.listExpand) await loadInlineListItems(lid);
    renderLists();
  };

  // ── T3 #20 — School ICS feeds ─────────────────────────────────────
  async function loadSchoolFeeds() {
    try {
      const r = await api('/api/scheduler/school_feeds');
      S.schoolFeeds = (r && r.feeds) || [];
    } catch(e) { S.schoolFeeds = []; }
    renderSchoolFeeds();
  }
  function renderSchoolFeeds() {
    const el = document.getElementById('sch-school-feeds');
    if (!el) return;
    if (!S.schoolFeeds || !S.schoolFeeds.length) {
      el.innerHTML = '<p class="sch-empty" style="font-size:11px">No feeds yet. <span class="sch-empty-sub">Paste an ICS URL to auto-import + sync.</span></p>';
      return;
    }
    el.innerHTML = S.schoolFeeds.map(f => {
      const last = f.last_synced_at ? new Date(f.last_synced_at * 1000).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : 'never';
      const result = f.last_result || 'pending';
      return `<div class="sch-school-feed-row" title="${escAttr(f.feed_url)} · last ${last} · ${escAttr(result)}">
        <span class="sch-school-feed-dot" style="background:${f.color}"></span>
        <span class="sch-school-feed-label">${escHtml(f.label)}</span>
        <button class="sch-school-feed-btn" onclick="schSchoolFeedSync(${f.id})" title="Re-sync now">↻</button>
        <button class="sch-school-feed-btn" onclick="schSchoolFeedDelete(${f.id})" title="Remove">×</button>
      </div>`;
    }).join('');
  }
  window.schNewSchoolFeed = async function() {
    const result = await schDialog({
      title: 'Add school ICS feed',
      fields: [
        { name: 'label',    label: 'Label',    placeholder: "e.g. Jamie's 4th grade" },
        { name: 'feed_url', label: 'ICS URL',  placeholder: 'https:// or webcal://' },
      ],
      okLabel: 'Add',
    });
    if (!result) return;
    const label = (result.label || '').trim();
    const feed_url = (result.feed_url || '').trim();
    if (!label || !feed_url) return;
    schToast('Fetching feed…', 3000);
    try {
      const r = await api('/api/scheduler/school_feeds', { method: 'POST', body: JSON.stringify({ label: label.trim(), feed_url: feed_url.trim() }) });
      schToast(`Imported ${r.imported || 0} events. Auto-resyncs every 6h.`, 3500);
      await loadSchoolFeeds();
      await loadAll();
    } catch(e) { schToast('Feed setup failed: ' + e.message, 3500); }
  };
  window.schSchoolFeedSync = async function(id) {
    schToast('Re-syncing…', 2000);
    try {
      const r = await api(`/api/scheduler/school_feeds/${id}/sync`, { method: 'POST', body: JSON.stringify({}) });
      schToast(`Re-imported ${r.imported || 0} events`, 2500);
      await loadSchoolFeeds();
      await loadAll();
    } catch(e) { schToast('Sync failed', 2000); }
  };
  window.schSchoolFeedDelete = async function(id) {
    if (!(await schConfirm('Remove this school feed?', 'Its imported events will be deleted.', { okLabel: 'Remove', danger: true }))) return;
    try { await api(`/api/scheduler/school_feeds/${id}`, { method: 'DELETE' }); await loadSchoolFeeds(); await loadAll(); }
    catch(e) { schToast('Delete failed', 1500); }
  };

  // ── T2 #10 — Meeting prep cards ────────────────────────────────────
  async function loadMeetingPrep() {
    try {
      const r = await api('/api/scheduler/meeting_prep');
      S.meetingPrep = (r && r.cards) || [];
    } catch(e) { S.meetingPrep = []; }
    renderMeetingPrep();
  }
  function renderMeetingPrep() {
    const list = document.getElementById('sch-meetprep-list');
    const cnt  = document.getElementById('sch-meetprep-count');
    const cards = S.meetingPrep || [];
    cnt.textContent = cards.length;
    cnt.classList.toggle('zero', cards.length === 0);
    if (!cards.length) { list.innerHTML = '<p class="sch-empty">Nothing upcoming.</p>'; return; }
    const now = Date.now();
    list.innerHTML = cards.map(c => {
      const t = new Date(c.start_time);
      const mins = Math.round((t - now) / 60000);
      const when = mins <= 0 ? 'starting now' : `in ${mins} min`;
      const att = (c.attendees || []).length ? `<div class="sch-meetprep-sect"><strong>With</strong> ${(c.attendees||[]).map(escHtml).join(', ')}</div>` : '';
      const emails = (c.recent_emails || []).slice(0, 3).map(m => `<span class="sch-meetprep-email">${escHtml(m.subject || '(no subject)')} <em>— ${escHtml((m.from || '').split('<')[0].trim())}</em></span>`).join('');
      const emailsBlock = emails ? `<div class="sch-meetprep-sect"><strong>Recent emails</strong>${emails}</div>` : '';
      const jh = (c.journal_hits || []).slice(0, 2).map(j => `<span class="sch-meetprep-email">${escHtml((j.excerpt || '').slice(0, 120))}${(j.excerpt||'').length > 120 ? '…' : ''}</span>`).join('');
      const jhBlock = jh ? `<div class="sch-meetprep-sect"><strong>From journal</strong>${jh}</div>` : '';
      return `<div class="sch-meetprep-card">
        <div class="sch-meetprep-title">${escHtml(c.title || '(event)')}</div>
        <div class="sch-meetprep-when">${when}</div>
        ${att}${emailsBlock}${jhBlock}
      </div>`;
    }).join('');
  }
  window.schNewHabit = async function() {
    const name = await schPrompt('New habit', 'e.g. Drink water, Morning walk');
    if (!name) return;
    try { await api('/api/scheduler/habits', { method: 'POST', body: JSON.stringify({ name, icon: '●', color: '#84cc16' }) }); await loadAll(); }
    catch(e) { schToast('Create failed: ' + e.message, 2500); }
  };

  function renderHabits() {
    const el = document.getElementById('sch-habits');
    if (!el) return;
    if (!S.habits.length) { el.innerHTML = '<p class="sch-empty">No habits yet. Tap + to add one.</p>'; return; }
    const today = fmtDate(new Date());
    const last7 = []; for (let i = 6; i >= 0; i--) last7.push(fmtDate(addDays(new Date(), -i)));
    el.innerHTML = S.habits.map(h => {
      const set = S.habitEntries[h.id] || new Set();
      const dots = last7.map(k => {
        const filled = set.has(k);
        const isToday = k === today;
        return `<button class="sch-habit-dot${filled?' filled':''}${isToday?' today':''}" onclick="schHabitToggle(${h.id}, '${k}')" style="${filled?`background:${h.color||'#84cc16'};border-color:${h.color||'#84cc16'}`:''}"></button>`;
      }).join('');
      return `<div class="sch-habit-row"><span class="sch-habit-name">${escHtml(h.name)}</span><span class="sch-habit-dots">${dots}</span></div>`;
    }).join('');
  }
  window.schHabitToggle = async function(id, date) {
    try { await api(`/api/scheduler/habits/${id}/toggle`, { method: 'POST', body: JSON.stringify({ date }) }); await loadAll(); }
    catch(e) { schToast('Toggle failed: ' + e.message, 2000); }
  };

  // ── Proposals ──────────────────────────────────────────────────────
  function renderProposals() {
    const list = document.getElementById('sch-proposals-list');
    const cnt  = document.getElementById('sch-proposals-count');
    cnt.textContent = S.approvals.length;
    cnt.classList.toggle('zero', S.approvals.length === 0);
    if (!S.approvals.length) { list.innerHTML = '<p class="sch-empty">Quiet for now. <span class="sch-empty-sub">Proposals from voice, photo, and email appear here.</span></p>'; return; }
    list.innerHTML = S.approvals.map(a => `
      <div class="sch-proposal-card">
        <div class="p-summary">${escHtml(a.summary || '(no summary)')}</div>
        <div class="p-source">${escHtml(a.kind)} · ${escHtml(a.source || 'direct')}</div>
        <div class="p-actions">
          <button class="sch-pbtn ok" onclick="schApprove(${a.id})">Add</button>
          <button class="sch-pbtn rej" onclick="schReject(${a.id})">Decline</button>
          ${a.source && a.source.startsWith('gmail:') ? `<button class="sch-pbtn" onclick="schDraftReply(${a.id})">✉ Reply</button>` : ''}
        </div>
      </div>
    `).join('');
  }
  window.schApprove = async function(id) {
    // Snapshot current event IDs so we can identify which event the
    // approval turned into, then make THAT event undo-able.
    const before = new Set((S.events || []).map(e => e.id));
    try {
      await api(`/api/approvals/${id}/resolve`, { method: 'POST', body: JSON.stringify({ approved: true }) });
      await loadAll();
      const newIds = (S.events || []).map(e => e.id).filter(x => !before.has(x));
      if (newIds.length === 1) pushUndo({ op: 'approve-event', id: newIds[0] });
    } catch(e) { schToast('Approve failed: ' + e.message, 2500); }
  };
  window.schReject = async function(id) {
    try { await api(`/api/approvals/${id}/resolve`, { method: 'POST', body: JSON.stringify({ approved: false }) }); await loadAll(); }
    catch(e) { schToast('Reject failed: ' + e.message, 2500); }
  };
  window.schDraftReply = function(id) { schAlert('Email reply drafting', 'Ships with the Gmail connector pass.'); };

  // ── Bottom timeline ────────────────────────────────────────────────
  function renderTimeline() {
    const el = document.getElementById('sch-timeline-items');
    const now = new Date();
    const horizon = addDays(now, 2);
    const upcoming = S.events
      .map(ev => ({ ev, t: new Date(ev.start_time || ev.start) }))
      .filter(x => x.t >= now && x.t <= horizon)
      .sort((a,b) => a.t - b.t)
      .slice(0, 8);
    if (!upcoming.length) { el.innerHTML = '<span class="sch-empty">Nothing scheduled.</span>'; return; }
    el.innerHTML = upcoming.map(({ev, t}) => {
      const today = sameDay(t, now);
      const label = today ? '' : (sameDay(t, addDays(now,1)) ? 'Tmrw ' : t.toLocaleDateString(undefined,{weekday:'short'}) + ' ');
      const tstr  = `${String(t.getHours()).padStart(2,'0')}:${String(t.getMinutes()).padStart(2,'0')}`;
      return `<span class="sch-timeline-item"><span class="t-time">${label}${tstr}</span> ${escHtml(ev.title||'(untitled)')}</span>`;
    }).join(' <span class="sch-timeline-sep">·</span> ');
  }

  // ── Theme picker ───────────────────────────────────────────────────
  window.schOpenThemes = function() {
    const grid = document.getElementById('sch-theme-grid');
    grid.innerHTML = THEMES.map(t => `
      <button class="sch-theme-tile${t.key === S.prefs.theme ? ' active' : ''}" onclick="schPickTheme('${t.key}')">
        <div class="sch-theme-preview" style="background:${t.bg};color:${t.ink}">
          <span style="display:inline-block;padding:6px 12px;background:${t.accent};color:#fff;border-radius:4px">Tuesday</span>
        </div>
        <div class="sch-theme-label">${t.name}</div>
      </button>
    `).join('');
    document.getElementById('sch-theme-modal').hidden = false;
  };
  window.schCloseThemes = function() { document.getElementById('sch-theme-modal').hidden = true; };
  window.schPickTheme = async function(key) {
    applyTheme(key);
    try { await api('/api/scheduler/prefs', { method: 'POST', body: JSON.stringify({ theme: key }) }); } catch(e) {}
    schCloseThemes();
  };

  // ── Border picker (Artful Agenda parity) ──────────────────────────
  window.schOpenBorders = function() {
    const grid = document.getElementById('sch-border-grid');
    const current = S.prefs.border || 'garden-notebook';
    grid.innerHTML = BORDERS.map(b => `
      <button class="sch-border-tile${b.key === current ? ' active' : ''}" onclick="schPickBorder('${b.key}')" data-border="${b.key}">
        <div class="sch-border-preview" data-preview="${b.key}">
          ${b.painted
            ? `<img class="sch-border-thumb" src="/scheduler-frame/${b.key}" alt="" loading="lazy">`
            : `<div class="sch-preview-paper"></div>`}
        </div>
        <div class="sch-border-label">${escHtml(b.name)}</div>
      </button>
    `).join('');
    document.getElementById('sch-border-modal').hidden = false;
  };
  window.schCloseBorders = function() { document.getElementById('sch-border-modal').hidden = true; };
  window.schPickBorder = async function(key) {
    applyBorder(key);
    try { await api('/api/scheduler/prefs', { method: 'POST', body: JSON.stringify({ border: key }) }); } catch(e) {}
    schCloseBorders();
  };

  // ── Intake — now wired to real endpoints ───────────────────────────
  window.schVoiceAdd = async function() {
    if (!('mediaDevices' in navigator) || !navigator.mediaDevices.getUserMedia) {
      return schNlCreatePrompt();
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const rec = new MediaRecorder(stream);
      const chunks = [];
      rec.ondataavailable = e => chunks.push(e.data);
      schToast('Recording… tap Stop when done', 99999, () => rec.stop());
      rec.onstop = async () => {
        stream.getTracks().forEach(t => t.stop());
        schToast('Transcribing…', 1500);
        const blob = new Blob(chunks, { type: 'audio/webm' });
        const fd = new FormData();
        fd.append('audio', blob, 'voice.webm');
        fd.append('token', TOKEN);
        try {
          const r = await fetch('/api/voice/transcribe', { method: 'POST', body: fd });
          const d = await r.json();
          const transcript = d.text || d.transcript || '';
          if (!transcript) return schNlCreatePrompt();
          await schCreateFromText(transcript);
        } catch(e) {
          console.warn('[sch] stt:', e); schNlCreatePrompt();
        }
      };
      rec.start();
    } catch(e) { console.warn('[sch] mic:', e); schNlCreatePrompt(); }
  };
  window.schPhotoAdd = function() {
    document.getElementById('sch-photo-input').click();
  };
  window.schPhotoSelected = async function(input) {
    const f = input.files && input.files[0];
    if (!f) return;
    const reader = new FileReader();
    reader.onload = async () => {
      schToast('Reading the card…', 2000);
      try {
        const r = await api('/api/scheduler/photo_create', { method: 'POST',
          body: JSON.stringify({ image_data_url: reader.result }) });
        schToast(`Thaddeus proposed: ${r.summary}`, 3500);
        await loadAll();
      } catch(e) { schToast(`Photo parse failed: ${e.message}`, 4000); }
    };
    reader.readAsDataURL(f);
    input.value = '';
  };
  window.schEmailAdd = async function() {
    schToast('Scanning the inbox…', 3000);
    try {
      const r = await api('/api/scheduler/email_scan', { method: 'POST', body: JSON.stringify({}) });
      const n = r.scanned || 0;
      schToast(n > 0 ? `Thaddeus found ${n} new proposal${n === 1 ? '' : 's'}` : 'No new proposals right now', 3000);
      await loadAll();
    } catch(e) { schToast('Inbox scan needs Gmail connected — /settings', 4000); }
  };
  window.schCloseProposalModal = function() { document.getElementById('sch-proposal-modal').hidden = true; };

  // ── T1 #1 — Natural-language text create ──────────────────────────
  function schNlCreatePrompt() {
    schPrompt("New event", 'e.g. "Dentist Tuesday at 3pm"').then(text => {
      if (text && text.trim()) schCreateFromText(text.trim());
    });
  }
  async function schCreateFromText(text) {
    schToast('Thaddeus is parsing…', 1500);
    try {
      const r = await api('/api/scheduler/voice_create', { method: 'POST',
        body: JSON.stringify({ transcript: text }) });
      schToast(`Proposed: ${r.summary}`, 3000);
      await loadAll();
    } catch(e) { schToast(`Parse failed: ${e.message}`, 3500); }
  }

  // ── T1 #2 — Undo stack (extended 2026-04-19) ─────────────────────
  // Covers: calendar event create/update/delete, drag-move/resize, list-item
  // add, schedule-todos bulk proposals, habit adds. Ops carry the minimum
  // state needed to rebuild or remove. Stack caps at 30 — plenty for a
  // typical session without leaking memory if Sean leaves the tab open.
  const UNDO = [];
  function pushUndo(entry) { UNDO.push(entry); if (UNDO.length > 30) UNDO.shift(); }
  window.schUndo = async function() {
    const e = UNDO.pop();
    if (!e) return schToast('Nothing to undo', 1500);
    try {
      switch (e.op) {
        case 'delete-event':
          await api('/api/calendar', { method: 'POST', body: JSON.stringify(e.prev) });
          break;
        case 'update-event':
          await api(`/api/calendar/${e.id}`, { method: 'PUT', body: JSON.stringify(e.prev) });
          break;
        case 'create-event':
          await api(`/api/calendar/${e.id}`, { method: 'DELETE' });
          break;
        case 'create-list-item':
          if (e.id) await api(`/api/scheduler/list_items/${e.id}`, { method: 'DELETE' });
          // Also revoke any grocery items added alongside a meal-add.
          if (e.grocery_ids) for (const gid of e.grocery_ids) await api(`/api/scheduler/list_items/${gid}`, { method: 'DELETE' });
          if (S.listExpand && S.listExpand.has(e.listId)) await loadInlineListItems(e.listId);
          if (S.listExpand && e.grocery_list_id && S.listExpand.has(e.grocery_list_id)) await loadInlineListItems(e.grocery_list_id);
          break;
        case 'create-approvals':
          // schedule-todos created a batch of approvals; decline each.
          for (const aid of (e.ids || [])) {
            await api(`/api/approvals/${aid}/resolve`, { method: 'POST', body: JSON.stringify({ approved: false }) });
          }
          break;
        case 'approve-event':
          await api(`/api/calendar/${e.id}`, { method: 'DELETE' });
          break;
        // Legacy op names (kept for safety — clears if any still on stack)
        case 'delete': if (e.prev) await api('/api/calendar', { method: 'POST', body: JSON.stringify(e.prev) }); break;
        case 'update': if (e.id && e.prev) await api(`/api/calendar/${e.id}`, { method: 'PUT', body: JSON.stringify(e.prev) }); break;
        case 'create': if (e.id) await api(`/api/calendar/${e.id}`, { method: 'DELETE' }); break;
      }
      schToast('Undone', 1200);
      await loadAll();
      renderLists();
    } catch(err) { schToast(`Undo failed: ${err.message}`, 3000); }
  };

  // ── T1 #3 — Weather forecast (Open-Meteo, free, no key) ───────────
  const WX_ICONS = {0:'☀︎',1:'☀︎',2:'⛅',3:'☁︎',45:'🌫',48:'🌫',51:'🌦',53:'🌦',55:'🌦',61:'🌧',63:'🌧',65:'🌧',71:'🌨',73:'🌨',75:'🌨',80:'🌦',81:'🌧',82:'🌧',95:'⛈',96:'⛈',99:'⛈'};
  let WX_CACHE = null;
  async function loadWeather() {
    if (WX_CACHE && (Date.now() - WX_CACHE.ts) < 3600000) return WX_CACHE.data;
    // Best-effort geolocation — cached lat/lon in localStorage, else skip.
    let coords = null;
    try { const s = localStorage.getItem('syntaur_geo'); if (s) coords = JSON.parse(s); } catch(e) {}
    if (!coords) {
      try { coords = await new Promise((ok, nope) => navigator.geolocation.getCurrentPosition(
        p => ok({ lat: p.coords.latitude, lon: p.coords.longitude }),
        _ => nope(), { timeout: 2500 }
      )); localStorage.setItem('syntaur_geo', JSON.stringify(coords)); } catch(e) { return null; }
    }
    try {
      const url = `https://api.open-meteo.com/v1/forecast?latitude=${coords.lat}&longitude=${coords.lon}&daily=weathercode,temperature_2m_max,temperature_2m_min&forecast_days=10&timezone=auto`;
      const r = await fetch(url);
      const d = await r.json();
      WX_CACHE = { ts: Date.now(), data: d.daily || null };
      return WX_CACHE.data;
    } catch(e) { return null; }
  }
  function renderWeatherOnMonth(daily) {
    if (!daily || !daily.time) return;
    const grid = document.getElementById('sch-month-grid');
    if (!grid) return;
    const byDate = {};
    daily.time.forEach((t, i) => {
      byDate[t] = { code: daily.weathercode[i], hi: daily.temperature_2m_max[i], lo: daily.temperature_2m_min[i] };
    });
    grid.querySelectorAll('.sch-month-cell').forEach(cell => {
      const onclick = cell.getAttribute('onclick') || '';
      const m = onclick.match(/'(\d{4}-\d{2}-\d{2})'/);
      if (!m) return;
      const wx = byDate[m[1]];
      if (!wx) return;
      // Append as a small corner chip if not already there.
      if (cell.querySelector('.sch-wx')) return;
      const chip = document.createElement('div');
      chip.className = 'sch-wx';
      chip.innerHTML = `<span>${WX_ICONS[wx.code] || '·'}</span> <span>${Math.round(wx.hi)}°</span>`;
      cell.appendChild(chip);
    });
  }

  // ── T1 #4 — Keyboard shortcuts ─────────────────────────────────────
  document.addEventListener('keydown', function(ev) {
    // Skip if typing in an input / contentEditable
    const tgt = ev.target;
    if (tgt && (tgt.tagName === 'INPUT' || tgt.tagName === 'TEXTAREA' || tgt.isContentEditable)) return;
    const meta = ev.metaKey || ev.ctrlKey;
    if (meta && ev.key === 'z') { ev.preventDefault(); window.schUndo(); return; }
    if (meta && (ev.key === 'n' || ev.key === 'N')) { ev.preventDefault(); schNlCreatePrompt(); return; }
    if (!meta && ev.key === 'm') { schSwitchView('month'); return; }
    if (!meta && ev.key === 'w') { schSwitchView('week'); return; }
    if (!meta && ev.key === 'd') { schSwitchView('day'); return; }
    if (!meta && ev.key === 'j') { schNav(1); return; }
    if (!meta && ev.key === 'k') { schNav(-1); return; }
    if (!meta && ev.key === 't') { schGoToday(); return; }
  });

  // ── T1 #6 — Jitsi 1-click video call ──────────────────────────────
  window.schAddJitsi = function() {
    const slug = 'syntaur-' + Math.random().toString(36).slice(2, 10);
    const url = 'https://meet.jit.si/' + slug;
    const locEl = document.getElementById('ev-loc');
    if (locEl) { locEl.value = (locEl.value ? locEl.value + ' · ' : '') + url; }
    schToast('Jitsi link added', 1500);
  };

  // ── T1 #7 — Location autocomplete via OSM Nominatim ──────────────
  // Biases to a ~55-mile box around the user's cached geo (reused from the
  // weather chip's localStorage) so "Starbucks" doesn't dump 20 international
  // hits. Only goes wide when the user types a long or multi-word query,
  // signalling they want a distant place by name.
  let LOC_TIMER = null;
  async function locGeoBias() {
    try { const s = localStorage.getItem('syntaur_geo'); if (s) return JSON.parse(s); } catch(_) {}
    return null;
  }
  function wireLocationAutocomplete() {
    const input = document.getElementById('ev-loc');
    if (!input || input._wired) return;
    input._wired = true;
    // Wrap the input in a positioned container so the dropdown can
    // top:100%-anchor to it.
    const ac = document.createElement('div');
    ac.className = 'sch-loc-ac'; ac.hidden = true;
    // Label is the parent; make sure it's position:relative.
    const wrap = input.parentElement;
    if (wrap) wrap.style.position = 'relative';
    if (wrap) wrap.appendChild(ac);
    input.addEventListener('input', function() {
      clearTimeout(LOC_TIMER);
      const q = input.value.trim();
      if (q.length < 3) { ac.hidden = true; return; }
      LOC_TIMER = setTimeout(async () => {
        try {
          const geo = await locGeoBias();
          const qWords = q.split(/\s+/).length;
          // Wide query = 4+ words OR 25+ chars OR contains a US state/"state"
          // keyword. Otherwise, keep strictly local.
          const looksDistant = /\b(state|country|[A-Z]{2}\b|\d{5})\b/.test(q);
          const wide = qWords >= 4 || q.length >= 25 || looksDistant;
          let params = `q=${encodeURIComponent(q)}&format=json&limit=4&addressdetails=1&countrycodes=us`;
          let bounded = false;
          if (geo) {
            const d = 0.8; // ~55 mi
            params += `&viewbox=${geo.lon-d},${geo.lat+d},${geo.lon+d},${geo.lat-d}`;
            if (!wide) { params += '&bounded=1'; bounded = true; }
          }
          const url = `https://nominatim.openstreetmap.org/search?${params}`;
          const r = await fetch(url, { headers: { 'Accept': 'application/json' } });
          const d = await r.json();
          if (!Array.isArray(d) || !d.length) { ac.hidden = true; return; }
          const rows = d.slice(0, 4).map(h =>
            `<button class="sch-loc-row" type="button" onclick="schPickLoc(${JSON.stringify(h.display_name).replace(/"/g,'&quot;')})">${escHtml(h.display_name)}</button>`
          ).join('');
          const hint = bounded
            ? `<span class="sch-loc-hint">Showing local matches. Type more (e.g. state name) to widen.</span>`
            : '';
          ac.innerHTML = rows + hint;
          ac.hidden = false;
        } catch(e) { ac.hidden = true; }
      }, 350);
    });
    input.addEventListener('blur', () => setTimeout(() => { ac.hidden = true; }, 180));
  }
  window.schPickLoc = function(name) {
    const i = document.getElementById('ev-loc'); if (i) i.value = name;
    document.querySelectorAll('.sch-loc-ac').forEach(a => a.hidden = true);
  };

  // ── T2 #9 — "Schedule my todos" ───────────────────────────────────
  window.schScheduleTodos = async function() {
    schToast('Thaddeus is finding free time for your todos…', 3000);
    // Snapshot pending approval IDs so we know which are new after the call.
    const before = new Set((S.approvals || []).map(a => a.id));
    try {
      const r = await api('/api/scheduler/schedule_todos', { method: 'POST', body: JSON.stringify({}) });
      const n = r.proposed || 0;
      schToast(n > 0 ? `Proposed ${n} time-blocks — approve in the right rail or ⌘Z to revoke` : 'Nothing to schedule', 3500);
      await loadAll();
      const created = (S.approvals || []).filter(a => !before.has(a.id)).map(a => a.id);
      if (created.length) pushUndo({ op: 'create-approvals', ids: created });
    } catch(e) { schToast('Auto-schedule failed', 2500); }
  };

  // ── T2 #11 — Smart conflict detection on event create ────────────
  function findConflicts(payload, excludeId) {
    const s = new Date(payload.start_time), e = new Date(payload.end_time || payload.start_time);
    return S.events.filter(ev => ev.id !== excludeId).filter(ev => {
      const es = new Date(ev.start_time || ev.start);
      const ee = new Date(ev.end_time || ev.end || es);
      return s < ee && e > es;
    });
  }
  function proposeAlternativeSlots(payload, n = 3) {
    const len = Math.max(30*60000, new Date(payload.end_time) - new Date(payload.start_time));
    const slots = [];
    let cursor = new Date(payload.start_time); cursor.setMinutes(cursor.getMinutes() + 30);
    for (let i = 0; i < 48 && slots.length < n; i++) {
      const end = new Date(cursor.getTime() + len);
      const conflict = S.events.some(ev => {
        const es = new Date(ev.start_time || ev.start), ee = new Date(ev.end_time || ev.end || es);
        return cursor < ee && end > es;
      });
      if (!conflict && cursor.getHours() >= 7 && cursor.getHours() < 21) {
        slots.push(new Date(cursor));
      }
      cursor.setMinutes(cursor.getMinutes() + 30);
    }
    return slots;
  }

  // ── T2 #13 — Journal cross-link ───────────────────────────────────
  window.schOpenJournalForDay = function(dateKey) {
    window.location.href = '/journal?date=' + encodeURIComponent(dateKey);
  };

  // ── T4 #22 — Moon phase + sunrise/sunset ──────────────────────────
  function moonPhase(date) {
    // Conway's approximation. Returns 0..7 index for ["new","waxing cr","first q","waxing g","full","waning g","last q","waning cr"].
    const c = date.getFullYear() - 1900; const e = c * 12.3685;
    const m = Math.floor(date.getMonth() + 1 + e) % 30;
    const phase = Math.floor(m / 3.75) % 8;
    const icons = ['🌑','🌒','🌓','🌔','🌕','🌖','🌗','🌘'];
    return { idx: phase, icon: icons[phase] };
  }

  // ── T4 #23 — Seasonal theme rotation (opt-in) ────────────────────
  function seasonalThemeFor(month) {
    if (month >= 11 || month <= 1) return 'winter';
    if (month >= 2 && month <= 4)  return 'garden';
    if (month >= 5 && month <= 7)  return 'desert';
    return 'cafe';
  }
  window.schToggleSeasonal = async function() {
    S.prefs.seasonal = !S.prefs.seasonal;
    if (S.prefs.seasonal) { applyTheme(seasonalThemeFor(new Date().getMonth())); }
    schToast(S.prefs.seasonal ? 'Seasonal themes: on' : 'Seasonal themes: off', 1500);
  };

  // ── T4 #25 — Printable PDF export ────────────────────────────────
  window.schPrint = function() {
    document.body.classList.add('sch-print-mode');
    window.print();
    setTimeout(() => document.body.classList.remove('sch-print-mode'), 500);
  };

  // ── T4 #21 — Sticker picker ──────────────────────────────────────
  const STICKERS = [
    {key:'heart',svg:'<svg viewBox="0 0 24 24" fill="#e11d48"><path d="M12 21s-7-4.5-9.5-9A5.5 5.5 0 0112 7a5.5 5.5 0 019.5 5c-2.5 4.5-9.5 9-9.5 9z"/></svg>'},
    {key:'star',svg:'<svg viewBox="0 0 24 24" fill="#f59e0b"><path d="M12 2l2.9 6.5 7.1.7-5.3 4.8 1.7 7L12 17l-6.4 4 1.7-7L2 9.2l7.1-.7z"/></svg>'},
    {key:'leaf',svg:'<svg viewBox="0 0 24 24" fill="#84cc16"><path d="M17 4c-5 0-12 3-13 13 8-1 13-5 13-13z"/></svg>'},
    {key:'check',svg:'<svg viewBox="0 0 24 24" fill="#10b981"><path d="M9 16.2L4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4z"/></svg>'},
    {key:'flag',svg:'<svg viewBox="0 0 24 24" fill="#3b82f6"><path d="M6 3v18h2v-7h6l1 2h5V5h-7l-1-2H6z"/></svg>'},
    {key:'cake',svg:'<svg viewBox="0 0 24 24" fill="#ec4899"><path d="M12 3c.5 1-.5 2 0 3s-.5 2 0 3 .5 2 0 3H10V7c.5-1-.5-2 0-3s.5-2 .5-2zM6 13h12v2a3 3 0 01-3 3H9a3 3 0 01-3-3v-2zm0-3c1 0 1 1 2 1s1-1 2-1 1 1 2 1 1-1 2-1 1 1 2 1 1-1 2-1v2H6v-2z"/></svg>'},
    {key:'coffee',svg:'<svg viewBox="0 0 24 24" fill="#a16207"><path d="M4 19h14v2H4zM18 8V6h-3a4 4 0 00-8 0H4v8a5 5 0 005 5h4a5 5 0 005-5h1a3 3 0 000-6h-1z"/></svg>'},
    {key:'sun',svg:'<svg viewBox="0 0 24 24" fill="#eab308"><circle cx="12" cy="12" r="5"/><path d="M12 1v3M12 20v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M1 12h3M20 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1" stroke="#eab308" stroke-width="2"/></svg>'},
    {key:'music',svg:'<svg viewBox="0 0 24 24" fill="#8b5cf6"><path d="M12 3v10.55A4 4 0 1014 17V7h4V3h-6z"/></svg>'},
    {key:'book',svg:'<svg viewBox="0 0 24 24" fill="#0891b2"><path d="M4 4h7v16H4zm9 0h7v16h-7z"/></svg>'},
    {key:'plane',svg:'<svg viewBox="0 0 24 24" fill="#0ea5e9"><path d="M21 16v-2l-8-5V3.5A1.5 1.5 0 0011.5 2 1.5 1.5 0 0010 3.5V9l-8 5v2l8-2.5V19l-2 1.5V22l3.5-1 3.5 1v-1.5L13 19v-5.5z"/></svg>'},
    {key:'dumbbell',svg:'<svg viewBox="0 0 24 24" fill="#dc2626"><path d="M20.5 10H18V7.5a1.5 1.5 0 00-3 0V10h-6V7.5a1.5 1.5 0 00-3 0V10H3.5a1.5 1.5 0 000 3H6v2.5a1.5 1.5 0 003 0V13h6v2.5a1.5 1.5 0 003 0V13h2.5a1.5 1.5 0 000-3z"/></svg>'},
  ];
  window.schOpenStickerPickerFor = async function(dateKey) {
    const box = document.createElement('div');
    box.className = 'sch-modal'; box.id = 'sch-sticker-picker';
    box.innerHTML = `<div class="sch-modal-box"><div class="sch-modal-head"><h2>Stickers · ${dateKey}</h2><button class="sch-modal-close" onclick="this.closest('.sch-modal').remove()">×</button></div>`
      + `<div class="sch-sticker-grid">` + STICKERS.map(s => `<button class="sch-sticker-cell" onclick="schPlaceSticker('${dateKey}','${s.key}')">${s.svg}</button>`).join('')
      + `</div></div>`;
    document.body.appendChild(box);
  };
  window.schPlaceSticker = async function(date, key) {
    try { await api('/api/scheduler/stickers', { method: 'POST', body: JSON.stringify({ date, sticker_key: key, position: 'tr' }) }); schToast('Placed ' + key, 1200); }
    catch(e) { schToast('Could not place sticker', 1500); }
    const m = document.getElementById('sch-sticker-picker'); if (m) m.remove();
    await loadAll();
  };

  // ── T5 #29 — Global quick-capture shortcut ───────────────────────
  document.addEventListener('keydown', function(ev) {
    if ((ev.metaKey || ev.ctrlKey) && ev.shiftKey && (ev.key === 'N' || ev.key === 'n')) {
      ev.preventDefault(); schNlCreatePrompt();
    }
  });

  // ── Toasts (used by many features above) ─────────────────────────
  function schToast(msg, ms, onTap) {
    let el = document.getElementById('sch-toast');
    if (!el) { el = document.createElement('div'); el.id = 'sch-toast'; document.body.appendChild(el); }
    el.textContent = msg;
    el.style.display = 'block';
    if (onTap) { el.onclick = onTap; el.style.cursor = 'pointer'; } else { el.onclick = null; el.style.cursor = ''; }
    clearTimeout(el._t); el._t = setTimeout(() => { el.style.display = 'none'; }, ms || 2500);
  }

  // ── Hook weather + wiring + sticker long-press after month renders ─
  const _origRenderMonth = renderMonth;
  renderMonth = function() {
    _origRenderMonth();
    loadWeather().then(w => renderWeatherOnMonth(w));
    // Long-press on month cells opens sticker picker
    document.querySelectorAll('.sch-month-cell').forEach(cell => {
      if (cell._stickerWired) return; cell._stickerWired = true;
      let timer = null;
      cell.addEventListener('pointerdown', (ev) => {
        const onclick = cell.getAttribute('onclick') || '';
        const m = onclick.match(/'(\d{4}-\d{2}-\d{2})'/);
        if (!m) return;
        timer = setTimeout(() => { ev.preventDefault(); window.schOpenStickerPickerFor(m[1]); }, 650);
      });
      cell.addEventListener('pointerup', () => clearTimeout(timer));
      cell.addEventListener('pointerleave', () => clearTimeout(timer));
    });
  };
  const _origOpenEventModal = openEventModal;
  openEventModal = function(ev) {
    _origOpenEventModal(ev);
    setTimeout(wireLocationAutocomplete, 50);
  };

  // ── Drag-to-move + resize (week/day view) ──────────────────────────
  // Pointer-based so it works on mouse + touch. Click is preserved
  // because we only enter drag mode once the pointer has moved ≥ 4px.
  const DRAG = { active: false, mode: null, id: null, el: null, startY: 0, startX: 0, origTop: 0, origH: 0, origDayEl: null, moved: false };

  window.schEventDragStart = function(ev, id) {
    if (ev.target.classList && ev.target.classList.contains('sch-resize-handle')) return;
    ev.stopPropagation();
    DRAG.active = true; DRAG.mode = 'move'; DRAG.id = id;
    DRAG.el = ev.currentTarget;
    DRAG.startX = ev.clientX; DRAG.startY = ev.clientY;
    DRAG.origTop = parseInt(DRAG.el.style.top || '0', 10);
    DRAG.origH   = parseInt(DRAG.el.style.height || '40', 10);
    DRAG.origDayEl = DRAG.el.closest('.sch-week-day-col') || DRAG.el.parentElement;
    DRAG.moved = false;
    DRAG.el.setPointerCapture && DRAG.el.setPointerCapture(ev.pointerId);
    document.body.style.userSelect = 'none';
  };
  window.schEventResizeStart = function(ev, id) {
    ev.stopPropagation();
    DRAG.active = true; DRAG.mode = 'resize'; DRAG.id = id;
    DRAG.el = ev.currentTarget.parentElement;
    DRAG.startY = ev.clientY;
    DRAG.origH = parseInt(DRAG.el.style.height || '40', 10);
    DRAG.moved = false;
    DRAG.el.setPointerCapture && DRAG.el.setPointerCapture(ev.pointerId);
    document.body.style.userSelect = 'none';
  };
  document.addEventListener('pointermove', function(ev) {
    if (!DRAG.active) return;
    const dy = ev.clientY - DRAG.startY;
    const dx = ev.clientX - DRAG.startX;
    if (!DRAG.moved && Math.abs(dy) < 4 && Math.abs(dx) < 4) return;
    DRAG.moved = true;
    DRAG.el.style.opacity = '0.75';
    if (DRAG.mode === 'resize') {
      const snapped = Math.max(20, Math.round((DRAG.origH + dy) / 15) * 15);
      DRAG.el.style.height = snapped + 'px';
    } else if (DRAG.mode === 'move') {
      const newTop = Math.max(0, Math.round((DRAG.origTop + dy) / 15) * 15);
      DRAG.el.style.top = newTop + 'px';
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const col = el && el.closest && el.closest('.sch-week-day-col');
      if (col && col !== DRAG.origDayEl) {
        col.appendChild(DRAG.el);
        DRAG.origDayEl = col;
      }
    }
  });
  document.addEventListener('pointerup', async function(ev) {
    if (!DRAG.active) return;
    DRAG.active = false;
    document.body.style.userSelect = '';
    if (!DRAG.moved) return;
    const evt = S.events.find(x => x.id === DRAG.id);
    if (!evt || !DRAG.el) { await loadAll(); return; }
    const newTopPx = parseInt(DRAG.el.style.top || '0', 10);
    const newHPx   = parseInt(DRAG.el.style.height || '40', 10);
    const startMin = Math.max(0, Math.min(1439, newTopPx));
    const endMin   = Math.max(startMin + 15, Math.min(1440, newTopPx + newHPx));
    const dayCol   = DRAG.el.closest('.sch-week-day-col');
    const dayKey   = (dayCol && dayCol.dataset.date) || (evt.start_time || evt.start || '').slice(0, 10);
    const toISO = (k, m) => `${k}T${String(Math.floor(m/60)).padStart(2,'0')}:${String(m%60).padStart(2,'0')}`;
    const payload = { title: evt.title, location: evt.location || '', color: evt.color || '',
      start_time: toISO(dayKey, startMin), end_time: toISO(dayKey, endMin) };
    const prev = { title: evt.title, location: evt.location || '', color: evt.color || '',
      start_time: evt.start_time || evt.start, end_time: evt.end_time || evt.end };
    try {
      await api(`/api/calendar/${evt.id}`, { method: 'PUT', body: JSON.stringify(payload) });
      pushUndo({ op: 'update-event', id: evt.id, prev });
      await loadAll();
    } catch(e) { console.warn('[sch] commit drag:', e); await loadAll(); }
  });

  // ── Utils ──────────────────────────────────────────────────────────
  function escHtml(s) { return String(s==null?'':s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }
  function escAttr(s) { return escHtml(s).replace(/"/g,'&quot;'); }

  // ── Boot ────────────────────────────────────────────────────────────
  loadAll();
  // Refresh now-line every 30s in week/day view
  setInterval(() => { if (S.view === 'week' || S.view === 'day') renderAll(); }, 30000);
  // Poll meeting prep every 60s — cheap, cached server-side.
  setInterval(() => { loadMeetingPrep().catch(() => {}); }, 60000);
})();
"##;
