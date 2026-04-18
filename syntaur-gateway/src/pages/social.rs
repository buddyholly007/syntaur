//! /social — Social Media Manager module. Scaffold landing page only;
//! real functionality ships in subsequent phases. See
//! `research/crimson_lantern_syntaur_migration.md` in the vault for the
//! full plan.
//!
//! Persona: Nyota (see agents/defaults.rs::PROMPT_NYOTA).
//! Aesthetic per module framework: "Backstage at dusk" — warm amber
//! accents on off-white content surfaces, Playfair serif headings +
//! Inter body. Calm, pre-show, supportive.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Social",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        // ── Top bar ────────────────────────────────────────────────────────
        div class="soc-topbar" {
            div class="soc-topbar-inner" {
                a href="/" class="soc-crumb-link" { "Syntaur" }
                span class="soc-crumb-sep" { "/" }
                span class="soc-crumb-current" { "Social" }
                div class="soc-topbar-right" {
                    span id="soc-pause-pill" class="soc-pause-pill soc-pause-hidden" title="Something is paused — see Settings → Pause" {
                        "Paused"
                    }
                    span class="soc-persona-pill" title="Module specialist: Nyota" {
                        span class="soc-persona-sigil" { "★" }
                        span class="soc-persona-name" { "Nyota" }
                    }
                    button id="soc-chat-toggle" class="soc-chat-btn" onclick="socToggleChat()" {
                        "Talk with Nyota"
                    }
                }
            }
            div class="soc-brand-sub" {
                "Backstage. Quiet. Ready when you are."
            }
        }

        // ── Main layout: sidebar + content + optional chat rail ───────────
        div class="soc-shell" {
            // Left sidebar nav
            nav class="soc-sidebar" aria-label="Social module navigation" {
                div class="soc-nav-title" { "The room" }
                a href="#compose"     class="soc-nav-row soc-nav-active" data-section="compose"     { span class="soc-nav-icon" { "✎" }  span class="soc-nav-label" { "Compose" } }
                a href="#queue"       class="soc-nav-row"               data-section="queue"       { span class="soc-nav-icon" { "⋯" }  span class="soc-nav-label" { "Queue" } }
                a href="#inbox"       class="soc-nav-row"               data-section="inbox"       { span class="soc-nav-icon" { "✉" }  span class="soc-nav-label" { "Inbox" } }
                a href="#analytics"   class="soc-nav-row"               data-section="analytics"   { span class="soc-nav-icon" { "◔" }  span class="soc-nav-label" { "Analytics" } }
                a href="#connections" class="soc-nav-row"               data-section="connections" { span class="soc-nav-icon" { "◈" }  span class="soc-nav-label" { "Connections" } }
                a href="#settings"    class="soc-nav-row"               data-section="settings"    { span class="soc-nav-icon" { "❧" }  span class="soc-nav-label" { "Settings" } }

                div class="soc-nav-footer" {
                    p class="soc-nav-footer-note" {
                        "Your voice. Clean landings. No metrics chasing."
                    }
                }
            }

            // Main content area — all six panes render, JS shows the active one
            main class="soc-content" {

                // ─ Compose ─────────────────────────────────────────────
                section id="pane-compose" class="soc-pane soc-pane-active" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Compose" }
                        p class="soc-subhead" { "Write something. One thought, one draft. Nyota will help you land it cleanly." }
                    }
                    div class="soc-compose-form" {
                        div class="soc-field-group" {
                            label class="soc-field-label" { "Platform" }
                            div id="soc-compose-platforms" class="soc-compose-platforms" {
                                // rendered by JS from SOC_CONNECTIONS_MAP
                            }
                        }
                        div class="soc-field-group" {
                            div class="soc-field-labelrow" {
                                label class="soc-field-label" for="soc-compose-text" { "Your post" }
                                button type="button" class="soc-assist-btn" onclick="socComposeGenerate()" { "✨ Have Nyota draft one" }
                            }
                            textarea id="soc-compose-text" class="soc-field-input soc-set-textarea" rows="5" placeholder="Write the thing you want to say. Or click 'Have Nyota draft one' to get a starting point from your brand voice and pillars." oninput="socComposeCharCount()" {}
                            div class="soc-compose-counts" id="soc-compose-counts" { "" }
                        }
                        div class="soc-compose-actions" {
                            span class="soc-compose-status" id="soc-compose-status" { "" }
                            button class="soc-btn-ghost" onclick="socComposeSaveDraft()" { "Save as draft" }
                            button class="soc-btn" onclick="socComposePostNow()" { "Post now" }
                        }
                    }
                }

                // ─ Queue ───────────────────────────────────────────────
                section id="pane-queue" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Queue" }
                        p class="soc-subhead" { "Drafts waiting on your yes, and posts scheduled for later." }
                    }
                    div id="soc-queue-list" class="soc-queue-list" {
                        // rendered from /api/social/drafts
                    }
                }

                // ─ Inbox ───────────────────────────────────────────────
                section id="pane-inbox" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Inbox" }
                        p class="soc-subhead" { "Mentions, replies, and comments across your connected platforms." }
                    }
                    div id="soc-inbox-list" class="soc-queue-list" {
                        // rendered from /api/social/replies
                    }
                }

                // ─ Analytics ───────────────────────────────────────────
                section id="pane-analytics" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Analytics" }
                        p class="soc-subhead" { "What landed, what didn't, what you've been posting about." }
                    }
                    div id="soc-analytics-stats" class="soc-analytics-stats" {}
                    div id="soc-alerts-list" class="soc-alerts" {}
                }

                // ─ Connections ─────────────────────────────────────────
                section id="pane-connections" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Connections" }
                        p class="soc-subhead" { "Which platforms Nyota can speak to, and how healthy each one is." }
                    }
                    // First-run nudge — shown when user has a connection but no brand voice yet.
                    div id="soc-firstrun-banner" class="soc-firstrun-banner soc-firstrun-hidden" {
                        div class="soc-firstrun-sigil" { "★" }
                        div {
                            p class="soc-firstrun-h" { "One more thing before drafts start landing." }
                            p class="soc-firstrun-p" {
                                "You're connected, but I don't have your voice yet. Give me a paragraph — who you are, how you sound, what you avoid — and I'll seed every draft with it. "
                                a href="#settings" onclick="socGoto('settings'); return false;" { "Go to Settings → Voice & audience" }
                                "."
                            }
                            p class="soc-firstrun-sig" { "—Nyota" }
                        }
                    }
                    div class="soc-platform-grid" id="soc-platform-grid" {
                        // Cards render from the PLATFORMS JS constant on load.
                        // Each card is updated with live status from /api/social/connections
                        // via socRefreshConnections(). Stubbed "Connect" buttons land in Phase 2.
                    }
                    div class="soc-note" {
                        "Each platform has its own quirks. Nyota will walk you through the connect flow one at a time, with screenshots and a plain-language error if something goes sideways."
                    }
                }

                // ─ Settings ────────────────────────────────────────────
                section id="pane-settings" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Settings" }
                        p class="soc-subhead" { "Voice, audience, approval, engagement, notifications, pause, privacy, and a couple advanced knobs." }
                    }
                    div class="soc-settings-stack" {
                        // Voice & audience
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Voice & audience" }
                            p class="soc-setting-p" { "Nyota seeds every draft with this. Make it specific — your values, your aesthetic, what you avoid." }
                            div class="soc-field-group" {
                                div class="soc-field-labelrow" {
                                    label class="soc-field-label" for="soc-set-brand-voice" { "Brand voice" }
                                    button type="button" class="soc-assist-btn" onclick="socAssistOpen('brand_voice','soc-set-brand-voice')" title="Let Nyota draft this from sample posts" { "✨ Help me draft it" }
                                }
                                textarea id="soc-set-brand-voice" class="soc-field-input soc-set-textarea" rows="5" placeholder="e.g. Crimson Lantern is an indie singer-songwriter. Warm and grounded. First person. Never uses AI language (delve, tapestry, resonate). Talks like a person who actually made the thing." oninput="socMarkDirty()" {}
                            }
                            div class="soc-field-group" {
                                div class="soc-field-labelrow" {
                                    label class="soc-field-label" for="soc-set-audience" { "Who you're writing for" }
                                    button type="button" class="soc-assist-btn" onclick="socAssistOpen('audience','soc-set-audience')" title="Let Nyota sharpen a rough sketch" { "✨ Sharpen" }
                                }
                                textarea id="soc-set-audience" class="soc-field-input soc-set-textarea soc-set-textarea-short" rows="2" placeholder="e.g. Fans of indie folk who care about craft over virality. Late 20s to early 40s." oninput="socMarkDirty()" {}
                            }
                        }
                        // Approval mode
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Approval mode" }
                            p class="soc-setting-p" { "How much Nyota can ship on her own. Start with always-review; move to auto-post once you trust a few drafts." }
                            label class="soc-set-radio" {
                                input type="radio" name="soc-set-approval" value="always_review" onchange="socMarkDirty()";
                                div {
                                    strong { "Always review" }
                                    p { "Every draft waits for your yes. Recommended when you're starting out." }
                                }
                            }
                            label class="soc-set-radio" {
                                input type="radio" name="soc-set-approval" value="auto_post_routine" onchange="socMarkDirty()";
                                div {
                                    strong { "Auto-post routine, review risky" }
                                    p { "Recurring stuff goes live automatically. Nyota flags anything she isn't sure about." }
                                }
                            }
                            label class="soc-set-radio" {
                                input type="radio" name="soc-set-approval" value="auto_post_all" onchange="socMarkDirty()";
                                div {
                                    strong { "Auto-post all" }
                                    p { "High-trust mode. Nyota posts without review. You'll see the result in Queue either way." }
                                }
                            }
                        }
                        // Engagement strategy
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Engagement strategy" }
                            p class="soc-setting-p" { "How actively Nyota likes, follows, and unfollows on your behalf. Pick a preset or turn it off." }
                            div class="soc-set-row" {
                                label class="soc-set-label" for="soc-set-engage-preset" { "Preset" }
                                select id="soc-set-engage-preset" class="soc-field-input soc-set-select" onchange="socEngagePresetChanged(); socMarkDirty();" {
                                    option value="off" { "Off — no auto-engagement" }
                                    option value="artist" { "Artist — music + creator community" }
                                    option value="small_business" { "Small business — local community" }
                                    option value="creator" { "Creator — general audience" }
                                    option value="podcaster" { "Podcaster — podcast community" }
                                    option value="custom" { "Custom — you pick the numbers" }
                                }
                            }
                            div id="soc-engage-custom" class="soc-set-custom" {
                                div class="soc-set-row" {
                                    label class="soc-set-label" for="soc-set-engage-likes" { "Likes per day" }
                                    input type="number" id="soc-set-engage-likes" class="soc-field-input soc-set-num" min="0" max="200" value="20" oninput="socMarkDirty()";
                                }
                                div class="soc-set-row" {
                                    label class="soc-set-label" for="soc-set-engage-follows" { "Follows per day" }
                                    input type="number" id="soc-set-engage-follows" class="soc-field-input soc-set-num" min="0" max="100" value="15" oninput="socMarkDirty()";
                                }
                                div class="soc-set-row" {
                                    label class="soc-set-label" for="soc-set-engage-unfollow" { "Unfollow after (days)" }
                                    input type="number" id="soc-set-engage-unfollow" class="soc-field-input soc-set-num" min="0" max="365" value="7" oninput="socMarkDirty()";
                                }
                            }
                        }
                        // Notifications
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Notifications" }
                            p class="soc-setting-p" { "Where drafts, replies, and alerts reach you. Web dashboard is always on. Telegram mirror is handy on your phone." }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-notify-telegram" onchange="socMarkDirty()";
                                span { "Send approval requests to Telegram" }
                            }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-notify-stats" onchange="socMarkDirty()";
                                span { "Weekly stats summary" }
                            }
                        }
                        // Pause
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Pause" }
                            p class="soc-setting-p" { "Master switches. Vacation mode, quiet period, whatever life asks for. Flip these back when you're ready." }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-pause-posting" onchange="socMarkDirty()";
                                span { "Pause all posting (drafts still land in Queue for review)" }
                            }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-pause-engagement" onchange="socMarkDirty()";
                                span { "Pause engagement (no likes, follows, unfollows)" }
                            }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-pause-notifications" onchange="socMarkDirty()";
                                span { "Quiet notifications" }
                            }
                        }
                        // Privacy
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Privacy" }
                            p class="soc-setting-p" { "Which other modules Nyota may read for richer drafts. Everything is opt-in. Journal is never accessible to Nyota, regardless of these toggles." }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-privacy-calendar" onchange="socMarkDirty()";
                                span { "Calendar — upcoming shows, deadlines, travel" }
                            }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-privacy-music" onchange="socMarkDirty()";
                                span { "Music module — new tracks, recent listens" }
                            }
                            label class="soc-set-check" {
                                input type="checkbox" id="soc-set-privacy-research" onchange="socMarkDirty()";
                                span { "Research — knowledge base + saved sources" }
                            }
                            p class="soc-setting-hint" { "Journal is hardcoded-isolated. No toggle." }
                        }
                        // Advanced: tone + blocklist
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Advanced — tone + blocklist" }
                            p class="soc-setting-p" { "Fine-tune Nyota's dials and tell her what to avoid. Most users leave these alone." }
                            div class="soc-set-row" {
                                label class="soc-set-label" for="soc-set-tone-humor" { "Humor (0 = dry, 10 = playful)" }
                                input type="range" id="soc-set-tone-humor" class="soc-set-slider" min="0" max="10" value="4" oninput="socSliderLabel('soc-set-tone-humor'); socMarkDirty()";
                                span class="soc-set-sliderval" id="soc-set-tone-humor-val" { "4" }
                            }
                            div class="soc-set-row" {
                                label class="soc-set-label" for="soc-set-tone-formality" { "Formality (0 = casual, 10 = formal)" }
                                input type="range" id="soc-set-tone-formality" class="soc-set-slider" min="0" max="10" value="4" oninput="socSliderLabel('soc-set-tone-formality'); socMarkDirty()";
                                span class="soc-set-sliderval" id="soc-set-tone-formality-val" { "4" }
                            }
                            div class="soc-field-group" {
                                div class="soc-field-labelrow" {
                                    label class="soc-field-label" for="soc-set-blocklist" { "Blocklist (comma-separated)" }
                                    button type="button" class="soc-assist-btn" onclick="socAssistOpen('blocklist','soc-set-blocklist')" title="Describe what you avoid, Nyota drafts a list" { "✨ Draft from description" }
                                }
                                textarea id="soc-set-blocklist" class="soc-field-input soc-set-textarea soc-set-textarea-short" rows="2" placeholder="e.g. grind, hustle, delve, unpack, tapestry, resonate, crushing it" oninput="socMarkDirty()" {}
                            }
                        }
                    }
                    // Sticky save bar — only visible when dirty.
                    div id="soc-settings-savebar" class="soc-savebar soc-savebar-hidden" {
                        span class="soc-savebar-msg" { "You have unsaved changes." }
                        button class="soc-btn-ghost soc-savebar-btn" onclick="socRevertSettings()" { "Revert" }
                        button class="soc-btn soc-savebar-btn" onclick="socSaveSettings()" { "Save" }
                    }
                }
            }

            // ── Toast (for settings save confirmation) ────────────────────
            div id="soc-toast" { "" }

            // ── Nyota-assist modal ────────────────────────────────────────
            div id="soc-assist-modal" {
                div class="soc-modal-backdrop" onclick="socAssistClose()" {}
                div class="soc-modal-card" role="dialog" aria-modal="true" {
                    div class="soc-modal-head" {
                        div {
                            h2 class="soc-modal-title" id="soc-assist-title" { "Help me draft" }
                            p class="soc-modal-sub" id="soc-assist-subtitle" { "" }
                        }
                        button class="soc-modal-close" onclick="socAssistClose()" aria-label="Close" { "×" }
                    }
                    div class="soc-modal-body" {
                        textarea id="soc-assist-input" class="soc-field-input soc-assist-input" rows="6" {}
                        div id="soc-assist-result-row" style="display:none" {
                            div class="soc-assist-draft-head" { "Nyota's draft (edit freely after):" }
                            div id="soc-assist-result" class="soc-assist-draft" { "" }
                        }
                        div id="soc-assist-status" class="soc-assist-status" { "" }
                        p class="soc-modal-note" { "—Nyota" }
                    }
                    div class="soc-modal-foot" {
                        button class="soc-btn-ghost" onclick="socAssistClose()" { "Cancel" }
                        button class="soc-btn-secondary" onclick="socAssistSend()" id="soc-assist-submit" { "Draft it" }
                        button class="soc-btn" onclick="socAssistKeep()" { "Keep this draft" }
                    }
                }
            }

            // ── Per-connection slide-in panel ─────────────────────────────
            aside id="soc-plat-panel" class="soc-plat-panel" aria-label="Platform settings" {
                div class="soc-plat-head" {
                    div {
                        h2 class="soc-plat-title" id="soc-plat-title" { "" }
                        p class="soc-plat-handle" id="soc-plat-handle" { "" }
                    }
                    button class="soc-modal-close" onclick="socClosePlatformPanel()" aria-label="Close" { "×" }
                }
                div class="soc-plat-body" {
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Voice override" }
                        p class="soc-setting-p" id="soc-plat-voice-placeholder" { "" }
                        textarea id="soc-plat-voice" class="soc-field-input soc-set-textarea soc-set-textarea-short" rows="3" placeholder="Optional. e.g. More formal for LinkedIn, or terser for Threads." {}
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Content pillars" }
                        p class="soc-setting-p" { "3–5 recurring post types Nyota rotates through when drafting. One per line." }
                        textarea id="soc-plat-pillars" class="soc-field-input soc-set-textarea" rows="5" placeholder="e.g.\nBehind the song — one paragraph on what a track is about\nRelease announcements — new single or EP drops\nGigs + rehearsal — show reminders, studio updates\nQuiet reflections — songwriting or music industry\nCommunity — boost other indie artists" {}
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Posting cadence" }
                        p class="soc-setting-p" { "Which days a draft lands in your Queue, and whether it auto-posts without review." }
                        div class="soc-plat-daygrid" {
                            @for (id, label) in [("mon","M"), ("tue","T"), ("wed","W"), ("thu","T"), ("fri","F"), ("sat","S"), ("sun","S")] {
                                label class="soc-plat-day" {
                                    input type="checkbox" id={"soc-plat-day-" (id)};
                                    span { (label) }
                                }
                            }
                        }
                        div class="soc-set-row" {
                            label class="soc-set-label" for="soc-plat-time" { "Time of day (local)" }
                            input type="time" id="soc-plat-time" class="soc-field-input soc-set-time" value="09:00";
                        }
                        label class="soc-set-check" {
                            input type="checkbox" id="soc-plat-autopost";
                            span { "Auto-post (no review) on this platform" }
                        }
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Engagement — hashtags" }
                        p class="soc-setting-p" { "Comma-separated hashtags Nyota engages on. Blank = inherit your global strategy." }
                        textarea id="soc-plat-hashtags" class="soc-field-input soc-set-textarea soc-set-textarea-short" rows="2" placeholder="e.g. indiefolk, songwriter, indiemusic, acoustic" {}
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Signature / CTA" }
                        p class="soc-setting-p" { "Optional outro for this platform. Blank = no signature." }
                        textarea id="soc-plat-signature" class="soc-field-input soc-set-textarea soc-set-textarea-short" rows="2" placeholder="e.g. New single out now → crimsonlantern.band" {}
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Approval override" }
                        p class="soc-setting-p" { "Override your global approval mode for this platform only." }
                        select id="soc-plat-approval" class="soc-field-input soc-set-select" {
                            option value="inherit" { "Inherit global setting" }
                            option value="always_review" { "Always review" }
                            option value="auto_post_routine" { "Auto-post routine, review risky" }
                            option value="auto_post_all" { "Auto-post all" }
                        }
                    }
                    div class="soc-setting-card" {
                        h3 class="soc-setting-h" { "Pause this platform" }
                        p class="soc-setting-p" { "Mute this one without pausing everything. Useful when one account is in a rough spot." }
                        label class="soc-set-check" {
                            input type="checkbox" id="soc-plat-pause";
                            span { "Pause posting + engagement on this platform" }
                        }
                    }
                    div class="soc-setting-card soc-plat-danger" {
                        h3 class="soc-setting-h" { "Disconnect" }
                        p class="soc-setting-p" { "Remove this platform entirely. You can reconnect later with fresh credentials." }
                        button class="soc-btn-danger" onclick="socPlatformDisconnect()" { "Disconnect…" }
                    }
                }
                div class="soc-plat-foot" {
                    span class="soc-assist-status" id="soc-plat-status" { "" }
                    button class="soc-btn-ghost" onclick="socClosePlatformPanel()" { "Cancel" }
                    button class="soc-btn" onclick="socPlatformPanelSave()" { "Save" }
                }
            }

            // ── Reconnect modal (hidden by default) ───────────────────────
            div id="soc-modal" {
                div class="soc-modal-backdrop" onclick="socCloseModal()" {}
                div class="soc-modal-card" role="dialog" aria-modal="true" {
                    div class="soc-modal-head" {
                        div {
                            h2 class="soc-modal-title" id="soc-modal-title" { "Reconnect" }
                            p class="soc-modal-sub" id="soc-modal-sub" { "" }
                        }
                        button class="soc-modal-close" onclick="socCloseModal()" aria-label="Close" { "×" }
                    }
                    div class="soc-modal-body" id="soc-modal-body" {
                        // wizard content + form render here from the descriptor
                    }
                    div class="soc-modal-foot" {
                        button class="soc-btn-ghost" onclick="socCloseModal()" { "Cancel" }
                        button class="soc-btn" id="soc-modal-submit" onclick="socModalSubmit()" { "Reconnect" }
                    }
                }
            }

            // ── Nyota chat rail (collapsed by default) ────────────────────
            aside id="soc-chat-rail" class="soc-chat-rail soc-chat-collapsed" aria-label="Nyota chat" {
                div class="soc-chat-head" {
                    span class="soc-chat-sigil" { "★" }
                    div {
                        div class="soc-chat-title" { "Nyota" }
                        div class="soc-chat-sub" { "Comms" }
                    }
                    button class="soc-chat-close" onclick="socToggleChat()" aria-label="Close chat" { "×" }
                }
                div class="soc-chat-body" id="soc-chat-body" {
                    div class="soc-chat-msg soc-chat-msg-nyota" {
                        p { "Frequencies open. Walk me through what you want to say — I'll help you land it." }
                        p class="soc-chat-signoff" { "—Nyota" }
                    }
                }
                form class="soc-chat-form" onsubmit="return socChatSend(event)" {
                    input type="text" id="soc-chat-input" placeholder="Say the thing..." autocomplete="off";
                    button type="submit" class="soc-btn" { "Send" }
                }
            }
        }

        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"
/* Backstage at dusk palette.
 * Stage lighting: soft warm spot + dark house + off-white content surface.
 */
:root {
  --soc-bg:         #161210;           /* dark house */
  --soc-surface:    #fbf6ec;           /* off-white gel / unlit page */
  --soc-surface-2:  #f4ecdf;           /* rehearsal-book paper */
  --soc-ink:        #1a1715;
  --soc-ink-mute:   #5a514a;
  --soc-ink-soft:   #8a7f76;
  --soc-amber:      #d49a3a;           /* warm spot */
  --soc-amber-deep: #a26a19;
  --soc-amber-soft: #f3d9a8;
  --soc-rule:       #e3d6bf;
  --soc-shadow:     0 1px 0 rgba(0,0,0,0.05), 0 6px 24px -12px rgba(73,49,12,0.18);
}

body { background: var(--soc-bg); color: var(--soc-ink); }

.soc-topbar {
  background: linear-gradient(180deg, #1d1815 0%, #161210 100%);
  border-bottom: 1px solid #2a221b;
  padding: 14px 22px 10px;
}
.soc-topbar-inner {
  max-width: 1440px; margin: 0 auto;
  display: flex; align-items: center; gap: 10px;
  font-family: 'Inter', system-ui, sans-serif; font-size: 14px;
}
.soc-crumb-link { color: var(--soc-amber); text-decoration: none; letter-spacing: .02em; }
.soc-crumb-link:hover { color: #e7b45e; }
.soc-crumb-sep { color: #5a4a38; }
.soc-crumb-current {
  color: #f5e8cf;
  font-family: 'Playfair Display', 'Iowan Old Style', Georgia, serif;
  font-size: 18px; letter-spacing: .01em;
}
.soc-topbar-right { margin-left: auto; display: flex; align-items: center; gap: 12px; }
.soc-persona-pill {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 4px 10px; border: 1px solid #4a3a28;
  border-radius: 999px; background: #1f1812;
  color: #e7c98a; font-size: 12px; font-weight: 500;
}
.soc-persona-sigil { color: var(--soc-amber); font-size: 10px; }
.soc-chat-btn {
  background: transparent; border: 1px solid #4a3a28; color: #e7c98a;
  padding: 5px 12px; border-radius: 6px; font-size: 12px; cursor: pointer;
  transition: background .18s ease;
}
.soc-chat-btn:hover { background: #2a1f16; }
.soc-chat-btn.active { background: var(--soc-amber); color: #1a1208; border-color: var(--soc-amber); }
.soc-brand-sub {
  max-width: 1440px; margin: 4px auto 0; padding-left: 2px;
  color: #7a6a55; font-size: 12px; font-style: italic;
  font-family: 'Playfair Display', Georgia, serif;
}

.soc-shell {
  max-width: 1440px; margin: 0 auto;
  display: grid; grid-template-columns: 240px 1fr;
  min-height: calc(100vh - 120px);
  background: var(--soc-surface);
  box-shadow: var(--soc-shadow);
}
.soc-shell:has(.soc-chat-rail:not(.soc-chat-collapsed)) {
  grid-template-columns: 240px 1fr 320px;
}

.soc-sidebar {
  border-right: 1px solid var(--soc-rule);
  padding: 20px 14px;
  background: var(--soc-surface-2);
  font-family: 'Inter', sans-serif;
}
.soc-nav-title {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 12px; letter-spacing: .18em; text-transform: uppercase;
  color: var(--soc-ink-soft);
  padding: 6px 10px 14px;
}
.soc-nav-row {
  display: flex; align-items: center; gap: 10px;
  padding: 8px 10px; border-radius: 6px; margin-bottom: 2px;
  color: var(--soc-ink-mute); text-decoration: none; font-size: 14px;
  transition: background .15s ease, color .15s ease;
}
.soc-nav-row:hover { background: #ece2cd; color: var(--soc-ink); }
.soc-nav-row.soc-nav-active {
  background: var(--soc-amber-soft);
  color: var(--soc-amber-deep);
  box-shadow: inset 2px 0 0 var(--soc-amber);
}
.soc-nav-icon { width: 18px; text-align: center; color: var(--soc-amber); font-size: 14px; }
.soc-nav-label { font-weight: 500; letter-spacing: .01em; }
.soc-nav-footer { margin-top: 32px; padding: 12px 10px; border-top: 1px dashed var(--soc-rule); }
.soc-nav-footer-note {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 12px; font-style: italic; color: var(--soc-ink-soft); line-height: 1.5;
}

.soc-content { padding: 36px 40px; font-family: 'Inter', sans-serif; color: var(--soc-ink); }
.soc-pane { display: none; }
.soc-pane.soc-pane-active { display: block; }
.soc-pane-head { margin-bottom: 24px; padding-bottom: 14px; border-bottom: 1px solid var(--soc-rule); }
.soc-h1 {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 28px; font-weight: 600; letter-spacing: .005em;
  margin: 0 0 6px; color: var(--soc-ink);
}
.soc-subhead { font-size: 14px; color: var(--soc-ink-soft); margin: 0; max-width: 52ch; }

.soc-empty {
  max-width: 520px; margin: 80px auto;
  text-align: center; padding: 28px;
  border: 1px dashed var(--soc-rule); border-radius: 12px;
  background: #fffdf6;
}
.soc-empty-sigil {
  font-size: 36px; color: var(--soc-amber); margin-bottom: 10px;
  font-family: 'Playfair Display', Georgia, serif;
}
.soc-empty-h {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 18px; font-weight: 500; margin: 0 0 10px; color: var(--soc-ink);
}
.soc-empty-p {
  font-size: 14px; color: var(--soc-ink-mute); margin: 0 0 18px;
  line-height: 1.55; max-width: 44ch; margin-left: auto; margin-right: auto;
}
.soc-cta {
  display: inline-block; padding: 8px 16px; border-radius: 6px;
  background: var(--soc-amber); color: #1a1208; text-decoration: none;
  font-weight: 500; font-size: 13px;
  transition: background .15s ease;
}
.soc-cta:hover { background: var(--soc-amber-deep); color: #fdf5e6; }

/* Connections grid */
.soc-platform-grid {
  display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: 14px; margin-top: 10px;
}
.soc-platform-card {
  border: 1px solid var(--soc-rule); border-radius: 10px; padding: 16px;
  background: #fffdf6;
}
.soc-platform-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 6px; }
.soc-platform-name { font-weight: 600; color: var(--soc-ink); font-size: 14px; letter-spacing: .01em; }
.soc-platform-sub { font-size: 12px; color: var(--soc-ink-soft); margin: 0 0 12px; }
.soc-pill {
  font-size: 10px; padding: 2px 8px; border-radius: 999px;
  text-transform: uppercase; letter-spacing: .1em; font-weight: 600;
}
.soc-pill-idle      { background: #eee4ce; color: #8a7558; }
.soc-pill-connected { background: #d8ecd0; color: #3f6a2e; }
.soc-pill-degraded  { background: #f5e3ba; color: #8a6214; }
.soc-pill-error     { background: #f3d1cb; color: #953224; }
.soc-platform-handle { font-size: 12px; color: var(--soc-ink); font-weight: 500; margin: 2px 0 8px; font-family: 'JetBrains Mono', ui-monospace, monospace; }
.soc-platform-detail { font-size: 11px; color: var(--soc-amber-deep); margin: 0 0 10px; font-style: italic; }
.soc-btn-ghost {
  width: 100%; padding: 7px 10px; border-radius: 6px;
  border: 1px solid var(--soc-rule); background: transparent; color: var(--soc-ink-soft);
  font-size: 12px; cursor: not-allowed;
}
/* Secondary / muted: clickable but not urgent — used on healthy cards so
 * the user can still rotate credentials or re-verify without the button
 * shouting at them the way the primary amber does. */
.soc-btn-secondary {
  width: 100%; padding: 7px 10px; border-radius: 6px;
  border: 1px solid var(--soc-amber); background: transparent; color: var(--soc-amber-deep);
  font-size: 12px; cursor: pointer;
  transition: background .15s ease, color .15s ease;
}
.soc-btn-secondary:hover { background: var(--soc-amber-soft); color: var(--soc-amber-deep); }
.soc-note {
  margin-top: 24px; padding: 14px 18px;
  font-family: 'Playfair Display', Georgia, serif; font-style: italic;
  color: var(--soc-ink-soft); font-size: 14px;
  border-left: 2px solid var(--soc-amber);
  background: #fffdf6;
}

/* Settings */
.soc-settings-stack { display: flex; flex-direction: column; gap: 12px; max-width: 640px; padding-bottom: 80px; }
.soc-setting-card { border: 1px solid var(--soc-rule); border-radius: 10px; padding: 16px 18px; background: #fffdf6; }
.soc-setting-h { font-family: 'Playfair Display', Georgia, serif; font-size: 15px; font-weight: 600; margin: 0 0 6px; color: var(--soc-ink); }
.soc-setting-p { font-size: 13px; color: var(--soc-ink-mute); margin: 0 0 12px; }
.soc-setting-hint { font-size: 12px; color: var(--soc-ink-soft); margin: 0; font-style: italic; }

/* Setting form controls */
.soc-set-textarea { width: 100%; font-family: 'Inter', sans-serif; font-size: 13px; resize: vertical; min-height: 96px; }
.soc-set-row { display: flex; align-items: center; gap: 12px; margin-top: 10px; }
.soc-set-label { font-size: 12px; font-weight: 600; color: var(--soc-ink); min-width: 160px; }
.soc-set-time, .soc-set-num { width: 120px; font-family: 'JetBrains Mono', ui-monospace, monospace; }
.soc-set-select { flex: 1; max-width: 320px; background: #fffdf6; }
.soc-set-check { display: flex; align-items: center; gap: 8px; margin-top: 8px; font-size: 13px; color: var(--soc-ink-mute); cursor: pointer; }
.soc-set-check input[type="checkbox"] { accent-color: var(--soc-amber); }
.soc-set-custom { margin-top: 10px; padding: 12px; border-left: 2px solid var(--soc-amber); background: #fdf7e9; border-radius: 0 6px 6px 0; display: none; }
.soc-set-custom.soc-set-custom-open { display: block; }

/* Sticky save bar */
.soc-savebar {
  position: sticky; bottom: 0; margin-top: 16px; max-width: 640px;
  background: #fffdf6; border: 1px solid var(--soc-amber);
  border-radius: 8px; padding: 10px 14px;
  display: flex; align-items: center; gap: 10px;
  box-shadow: 0 4px 18px -8px rgba(73,49,12,0.3);
  transition: opacity .18s ease, transform .18s ease;
}
.soc-savebar-hidden { opacity: 0; pointer-events: none; transform: translateY(8px); }
.soc-savebar-msg { flex: 1; font-size: 13px; color: var(--soc-amber-deep); font-style: italic; }
.soc-savebar-btn { width: auto; padding: 6px 14px; font-size: 12px; cursor: pointer; }

/* Toast */
#soc-toast {
  position: fixed; bottom: 24px; right: 24px; z-index: 120;
  background: #1f3820; color: #e8f1e4; padding: 10px 16px; border-radius: 8px;
  font-size: 13px; box-shadow: 0 8px 32px -8px rgba(0,0,0,.4);
  opacity: 0; transform: translateY(10px); pointer-events: none;
  transition: opacity .2s ease, transform .2s ease;
  font-family: 'Inter', sans-serif;
}
#soc-toast.soc-toast-open { opacity: 1; transform: translateY(0); }

/* Chat rail */
.soc-chat-rail {
  border-left: 1px solid var(--soc-rule);
  background: var(--soc-surface-2);
  padding: 16px 14px;
  font-family: 'Inter', sans-serif;
  display: flex; flex-direction: column;
}
.soc-chat-collapsed { display: none !important; }
.soc-chat-head { display: flex; align-items: center; gap: 10px; padding-bottom: 10px; border-bottom: 1px solid var(--soc-rule); margin-bottom: 14px; }
.soc-chat-sigil {
  display: inline-flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; border-radius: 999px;
  background: var(--soc-amber-soft); color: var(--soc-amber-deep);
  font-family: 'Playfair Display', Georgia, serif;
}
.soc-chat-title { font-family: 'Playfair Display', Georgia, serif; font-weight: 600; color: var(--soc-ink); }
.soc-chat-sub { font-size: 11px; color: var(--soc-ink-soft); font-style: italic; }
.soc-chat-close { margin-left: auto; background: transparent; border: 0; color: var(--soc-ink-soft); font-size: 18px; cursor: pointer; }
.soc-chat-body { flex: 1; overflow-y: auto; }
.soc-chat-msg { margin-bottom: 14px; font-size: 13px; color: var(--soc-ink); line-height: 1.55; }
.soc-chat-msg p { margin: 0 0 6px; }
.soc-chat-signoff { font-family: 'Playfair Display', Georgia, serif; font-style: italic; color: var(--soc-ink-soft); font-size: 12px; }
.soc-chat-form { display: flex; gap: 6px; margin-top: 10px; }
.soc-chat-form input {
  flex: 1; padding: 7px 10px; border-radius: 6px; border: 1px solid var(--soc-rule);
  background: #fffdf6; color: var(--soc-ink); font-size: 13px;
}
.soc-btn { padding: 7px 14px; border-radius: 6px; background: var(--soc-amber); color: #1a1208; border: 0; font-weight: 500; font-size: 13px; cursor: pointer; }
.soc-chat-msg-user {
  background: var(--soc-surface-2); border-left: 3px solid var(--soc-ink-soft);
  padding: 8px 10px; border-radius: 0 6px 6px 0; margin-bottom: 10px; font-size: 13px;
}
.soc-chat-msg-user p { margin: 0; color: var(--soc-ink); }
.soc-chat-err { color: #953224; background: #f3d1cb; padding: 6px 10px; border-radius: 4px; }

/* Thinking bubble — three bouncing dots + rotating Nyota-voice thought */
.soc-chat-thinking {
  display: flex; align-items: center; gap: 10px;
  padding: 8px 2px 10px 2px;
}
.soc-chat-dots { display: inline-flex; gap: 4px; }
.soc-chat-dots span {
  width: 6px; height: 6px; border-radius: 999px;
  background: var(--soc-amber);
  animation: soc-chat-bounce 1.2s infinite ease-in-out;
}
.soc-chat-dots span:nth-child(2) { animation-delay: .15s; }
.soc-chat-dots span:nth-child(3) { animation-delay: .3s; }
@keyframes soc-chat-bounce {
  0%, 80%, 100% { transform: translateY(0); opacity: .4; }
  40%           { transform: translateY(-4px); opacity: 1; }
}
.soc-chat-thought {
  font-size: 12px; font-style: italic; color: var(--soc-ink-soft);
  transition: opacity .25s ease;
}

/* Compose pane */
.soc-compose-form { max-width: 720px; display: flex; flex-direction: column; gap: 16px; background: #fffdf6; border: 1px solid var(--soc-rule); border-radius: 10px; padding: 20px 22px; }
.soc-compose-platforms { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 4px; }
.soc-compose-plat {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 6px 12px; border: 1px solid var(--soc-rule); border-radius: 999px;
  background: #fffdf6; cursor: pointer; font-size: 12px; color: var(--soc-ink-mute);
  transition: border-color .12s ease, background .12s ease, color .12s ease;
}
.soc-compose-plat:has(input:checked) { background: var(--soc-amber-soft); border-color: var(--soc-amber); color: var(--soc-amber-deep); font-weight: 600; }
.soc-compose-plat input { accent-color: var(--soc-amber); margin: 0; }
.soc-compose-counts { margin-top: 6px; font-size: 11px; color: var(--soc-ink-soft); font-family: 'JetBrains Mono', ui-monospace, monospace; }
.soc-compose-count { margin-right: 10px; }
.soc-compose-over { color: #953224; }
.soc-compose-hint { font-style: italic; }
.soc-compose-actions { display: flex; justify-content: flex-end; align-items: center; gap: 10px; margin-top: 4px; }
.soc-compose-actions button { width: auto; padding: 7px 16px; font-size: 13px; }
.soc-compose-status { flex: 1; font-size: 12px; color: var(--soc-ink-soft); font-style: italic; }
.soc-compose-status-busy { color: var(--soc-amber-deep); }
.soc-compose-status-error { color: #953224; background: #f3d1cb; padding: 6px 10px; border-radius: 4px; font-style: normal; }

/* Queue + Inbox shared card look */
.soc-queue-list { display: flex; flex-direction: column; gap: 12px; }
.soc-q-card { background: #fffdf6; border: 1px solid var(--soc-rule); border-radius: 10px; padding: 14px 16px; }
.soc-q-head { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; flex-wrap: wrap; font-size: 12px; color: var(--soc-ink-soft); }
.soc-q-platform { font-family: 'JetBrains Mono', ui-monospace, monospace; font-weight: 600; color: var(--soc-ink); text-transform: lowercase; }
.soc-q-status-pending  { background: #eee4ce; color: #8a7558; }
.soc-q-status-posted   { background: #d8ecd0; color: #3f6a2e; }
.soc-q-status-rejected { background: #e5e0d4; color: var(--soc-ink-soft); }
.soc-q-status-failed   { background: #f3d1cb; color: #953224; }
.soc-q-pillar { background: var(--soc-amber-soft); color: var(--soc-amber-deep); padding: 2px 8px; border-radius: 999px; font-size: 10px; text-transform: uppercase; letter-spacing: .08em; font-weight: 600; }
.soc-q-when { margin-left: auto; font-style: italic; }
.soc-q-text { width: 100%; min-height: 64px; font-family: 'Inter', sans-serif; font-size: 13px; resize: vertical; }
.soc-q-actions { display: flex; gap: 8px; align-items: center; margin-top: 10px; }
.soc-q-actions button { width: auto; padding: 6px 14px; font-size: 12px; }
.soc-q-posted-uri { font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 11px; color: var(--soc-ink-soft); word-break: break-all; }
.soc-q-error { font-size: 12px; color: #953224; flex: 1; }
.soc-inbox-parent { padding: 10px 12px; background: var(--soc-surface-2); border-left: 3px solid var(--soc-ink-soft); border-radius: 0 6px 6px 0; font-size: 13px; color: var(--soc-ink-mute); margin-bottom: 10px; font-style: italic; }

/* Analytics */
.soc-analytics-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 12px; }
.soc-stats-card { background: #fffdf6; border: 1px solid var(--soc-rule); border-radius: 10px; padding: 16px 18px; }
.soc-stats-plat { font-family: 'Playfair Display', Georgia, serif; font-size: 16px; font-weight: 600; color: var(--soc-ink); margin: 0 0 12px; text-transform: lowercase; }
.soc-stats-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.soc-stats-grid > div { display: flex; flex-direction: column; }
.soc-stats-label { font-size: 10px; color: var(--soc-ink-soft); text-transform: uppercase; letter-spacing: .08em; }
.soc-stats-val { font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 20px; color: var(--soc-ink); font-weight: 600; }
.soc-stats-asof { margin-top: 10px; font-size: 11px; color: var(--soc-ink-soft); font-style: italic; }
.soc-alerts { margin-top: 16px; }
.soc-alerts-h { font-family: 'Playfair Display', Georgia, serif; font-size: 15px; font-weight: 600; color: var(--soc-ink); margin: 0 0 8px; }
.soc-alert-card { background: #fdf5f2; border: 1px solid #e0c0b7; border-left: 3px solid #c94c3a; border-radius: 6px; padding: 10px 12px; margin-bottom: 8px; display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.soc-alert-plat { font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 11px; color: var(--soc-ink-mute); }
.soc-alert-detail { margin: 0; flex: 1 1 100%; font-size: 13px; color: var(--soc-ink); }

/* Modal.
 * Default is hidden — the .soc-modal-open class opts IN to flex display.
 * Putting display on an ID selector would lose the specificity fight
 * against any hidden-state class, so the container stays displayless
 * and we toggle one class on/off in JS.
 */
#soc-modal, #soc-assist-modal {
  display: none;
  position: fixed; inset: 0; z-index: 100;
  align-items: center; justify-content: center;
}
#soc-modal.soc-modal-open, #soc-assist-modal.soc-modal-open { display: flex; }
.soc-modal-backdrop {
  position: absolute; inset: 0;
  background: rgba(22, 18, 16, 0.72); backdrop-filter: blur(2px);
}
.soc-modal-card {
  position: relative; z-index: 1;
  width: min(520px, 92vw); max-height: 86vh; overflow-y: auto;
  background: var(--soc-surface);
  border-radius: 14px; box-shadow: 0 12px 60px -20px rgba(73, 49, 12, 0.6);
  display: flex; flex-direction: column;
}
.soc-modal-head {
  padding: 20px 24px 14px; border-bottom: 1px solid var(--soc-rule);
  display: flex; align-items: flex-start; gap: 12px;
}
.soc-modal-title {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 22px; font-weight: 600; margin: 0; color: var(--soc-ink);
}
.soc-modal-sub { margin: 3px 0 0; font-size: 13px; color: var(--soc-ink-soft); font-style: italic; }
.soc-modal-close {
  margin-left: auto; background: transparent; border: 0;
  color: var(--soc-ink-soft); font-size: 24px; cursor: pointer; padding: 0 4px;
}
.soc-modal-body { padding: 18px 24px; flex: 1; }
.soc-modal-foot {
  padding: 14px 24px; border-top: 1px solid var(--soc-rule);
  display: flex; justify-content: flex-end; gap: 8px;
  background: var(--soc-surface-2);
  border-radius: 0 0 14px 14px;
}
.soc-wizard-step {
  background: #fffdf6; border: 1px solid var(--soc-rule); border-radius: 8px;
  padding: 12px 14px; margin-bottom: 10px;
}
.soc-wizard-step-title {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 14px; font-weight: 600; margin: 0 0 4px; color: var(--soc-ink);
}
.soc-wizard-step-body {
  font-size: 13px; color: var(--soc-ink-mute); line-height: 1.5; margin: 0;
}
.soc-wizard-step-body strong { color: var(--soc-ink); }
.soc-wizard-step-body code {
  background: #efe3c9; color: var(--soc-amber-deep);
  padding: 1px 5px; border-radius: 3px; font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 12px;
}
.soc-wizard-link {
  display: inline-block; margin-top: 6px; font-size: 12px;
  color: var(--soc-amber-deep); text-decoration: underline; text-underline-offset: 2px;
}
.soc-field-group { margin-top: 14px; }
.soc-field-label {
  display: block; font-size: 12px; font-weight: 600; color: var(--soc-ink);
  margin-bottom: 4px; letter-spacing: .02em;
}
.soc-field-input {
  width: 100%; padding: 8px 12px; border-radius: 6px;
  border: 1px solid var(--soc-rule); background: #fffdf6;
  color: var(--soc-ink); font-size: 14px;
  font-family: 'JetBrains Mono', ui-monospace, monospace;
}
.soc-field-input:focus { outline: 2px solid var(--soc-amber); outline-offset: 0; border-color: var(--soc-amber); }
.soc-field-help {
  font-size: 11px; color: var(--soc-ink-soft); margin-top: 4px; font-style: italic;
}
.soc-modal-error {
  margin-top: 14px; padding: 10px 12px;
  background: #f3d1cb; color: #712a21; border-radius: 6px;
  font-size: 13px; border-left: 3px solid #b33a2a;
}
.soc-modal-busy {
  margin-top: 14px; padding: 10px 12px; font-style: italic;
  color: var(--soc-ink-soft); font-size: 13px;
}
.soc-modal-info {
  margin-top: 14px; padding: 12px 14px;
  background: #fffdf6; border: 1px solid var(--soc-rule); border-left: 3px solid var(--soc-amber);
  border-radius: 6px;
}
.soc-modal-info strong {
  display: block; font-family: 'Playfair Display', Georgia, serif;
  font-size: 14px; color: var(--soc-ink); margin-bottom: 4px;
}
.soc-modal-info p { margin: 0; font-size: 13px; color: var(--soc-ink-mute); line-height: 1.5; }
.soc-modal-info code {
  background: #efe3c9; color: var(--soc-amber-deep);
  padding: 1px 5px; border-radius: 3px; font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 12px;
}
.soc-modal-note {
  margin: 12px 4px 0; font-family: 'Playfair Display', Georgia, serif;
  font-style: italic; color: var(--soc-ink-soft); font-size: 13px; text-align: right;
}

/* Additional setting controls */
.soc-field-labelrow { display: flex; align-items: center; justify-content: space-between; margin-bottom: 4px; }
.soc-assist-btn {
  font-size: 11px; padding: 3px 10px; border-radius: 999px;
  background: var(--soc-amber-soft); color: var(--soc-amber-deep); border: 1px solid var(--soc-amber);
  cursor: pointer; transition: background .15s ease;
}
.soc-assist-btn:hover { background: var(--soc-amber); color: #1a1208; }
.soc-set-textarea-short { min-height: 56px; }
.soc-set-radio {
  display: flex; align-items: flex-start; gap: 10px;
  padding: 10px 12px; margin-top: 8px;
  background: #fffdf6; border: 1px solid var(--soc-rule); border-radius: 8px;
  cursor: pointer; transition: border-color .15s ease, background .15s ease;
}
.soc-set-radio:hover { border-color: var(--soc-amber); }
.soc-set-radio input[type="radio"] { accent-color: var(--soc-amber); margin-top: 2px; }
.soc-set-radio strong { font-family: 'Playfair Display', Georgia, serif; font-size: 14px; color: var(--soc-ink); display: block; margin-bottom: 2px; }
.soc-set-radio p { font-size: 12px; color: var(--soc-ink-mute); margin: 0; line-height: 1.45; }
.soc-set-slider { flex: 1; max-width: 260px; accent-color: var(--soc-amber); }
.soc-set-sliderval {
  font-family: 'JetBrains Mono', ui-monospace, monospace;
  font-size: 13px; color: var(--soc-amber-deep); min-width: 24px; text-align: right;
}

/* Top-bar pause pill */
.soc-pause-pill {
  display: inline-flex; align-items: center;
  padding: 4px 10px; border-radius: 999px;
  background: #f5e3ba; color: #8a6214; border: 1px solid #d6a94a;
  font-size: 11px; font-weight: 600; letter-spacing: .1em; text-transform: uppercase;
}
.soc-pause-hidden { display: none !important; }

/* First-run banner on Connections */
.soc-firstrun-banner {
  margin: 12px 0 20px; padding: 16px 18px;
  background: #fffdf6; border: 1px solid var(--soc-amber); border-left: 4px solid var(--soc-amber);
  border-radius: 8px; display: flex; gap: 14px; align-items: flex-start;
}
.soc-firstrun-hidden { display: none !important; }
.soc-firstrun-sigil {
  flex-shrink: 0; width: 32px; height: 32px; border-radius: 999px;
  background: var(--soc-amber-soft); color: var(--soc-amber-deep);
  display: flex; align-items: center; justify-content: center;
  font-family: 'Playfair Display', Georgia, serif; font-size: 14px;
}
.soc-firstrun-h {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 16px; font-weight: 600; color: var(--soc-ink); margin: 0 0 6px;
}
.soc-firstrun-p { font-size: 13px; color: var(--soc-ink-mute); margin: 0 0 6px; line-height: 1.55; }
.soc-firstrun-p a { color: var(--soc-amber-deep); text-decoration: underline; text-underline-offset: 2px; }
.soc-firstrun-sig {
  font-family: 'Playfair Display', Georgia, serif;
  font-style: italic; color: var(--soc-ink-soft); font-size: 12px; margin: 4px 0 0;
}

/* Clickable healthy cards + manage hint */
.soc-platform-card-clickable { cursor: pointer; transition: transform .15s ease, box-shadow .15s ease, border-color .15s ease; }
.soc-platform-card-clickable:hover { transform: translateY(-1px); box-shadow: 0 4px 18px -10px rgba(73,49,12,0.4); border-color: var(--soc-amber); }
.soc-platform-manage {
  margin-top: 10px; font-size: 11px; color: var(--soc-ink-soft); font-style: italic; text-align: right;
}
.soc-platform-card-clickable:hover .soc-platform-manage { color: var(--soc-amber-deep); }

/* Per-connection slide-in panel */
.soc-plat-panel {
  position: fixed; top: 0; right: 0; bottom: 0; width: min(520px, 92vw);
  background: var(--soc-surface);
  border-left: 1px solid var(--soc-rule);
  box-shadow: -12px 0 40px -10px rgba(73,49,12,0.35);
  display: flex; flex-direction: column;
  transform: translateX(100%); transition: transform .22s ease;
  z-index: 90;
}
.soc-plat-panel.soc-plat-open { transform: translateX(0); }
.soc-plat-head {
  padding: 18px 22px 14px; border-bottom: 1px solid var(--soc-rule);
  display: flex; align-items: flex-start; gap: 12px;
  background: var(--soc-surface-2);
}
.soc-plat-title {
  font-family: 'Playfair Display', Georgia, serif;
  font-size: 22px; font-weight: 600; margin: 0; color: var(--soc-ink);
}
.soc-plat-handle { margin: 3px 0 0; font-size: 13px; color: var(--soc-ink-soft); font-family: 'JetBrains Mono', ui-monospace, monospace; }
.soc-plat-handle code { background: transparent; padding: 0; color: var(--soc-ink); }
.soc-plat-body { flex: 1; overflow-y: auto; padding: 18px 22px; display: flex; flex-direction: column; gap: 12px; }
.soc-plat-foot {
  padding: 12px 22px; border-top: 1px solid var(--soc-rule);
  display: flex; justify-content: flex-end; gap: 8px; align-items: center;
  background: var(--soc-surface-2);
}
.soc-plat-foot .soc-assist-status { flex: 1; font-style: italic; color: var(--soc-ink-soft); font-size: 12px; }
.soc-plat-daygrid { display: flex; gap: 6px; margin: 10px 0; }
.soc-plat-day {
  flex: 1; display: flex; flex-direction: column; align-items: center; gap: 4px;
  padding: 6px 0; border: 1px solid var(--soc-rule); border-radius: 6px;
  cursor: pointer; font-size: 11px; color: var(--soc-ink-soft); background: #fffdf6;
  transition: border-color .12s ease, background .12s ease, color .12s ease;
}
.soc-plat-day:has(input:checked) { border-color: var(--soc-amber); background: var(--soc-amber-soft); color: var(--soc-amber-deep); font-weight: 600; }
.soc-plat-day input { accent-color: var(--soc-amber); margin: 0; }
.soc-plat-day span { font-family: 'Playfair Display', Georgia, serif; font-size: 13px; }
.soc-plat-danger { border-color: #e0c0b7; background: #fdf5f2; }
.soc-btn-danger {
  padding: 7px 14px; border-radius: 6px;
  background: #c94c3a; color: #fff; border: 0; font-weight: 500; font-size: 13px; cursor: pointer;
  transition: background .15s ease;
}
.soc-btn-danger:hover { background: #a33a2a; }

/* Nyota-assist modal body bits */
.soc-assist-input { width: 100%; resize: vertical; font-family: 'Inter', sans-serif; font-size: 13px; }
.soc-assist-draft-head { font-size: 12px; font-weight: 600; color: var(--soc-ink); margin: 14px 0 4px; letter-spacing: .02em; }
.soc-assist-draft {
  padding: 12px; background: #fffdf6; border: 1px solid var(--soc-rule);
  border-left: 3px solid var(--soc-amber); border-radius: 6px;
  font-size: 13px; line-height: 1.55; color: var(--soc-ink); white-space: pre-wrap;
  font-family: 'Inter', sans-serif;
}
.soc-assist-status { margin-top: 10px; font-size: 13px; color: var(--soc-ink-soft); font-style: italic; }
.soc-assist-status-busy { color: var(--soc-amber-deep); }
.soc-assist-status-error { color: #953224; background: #f3d1cb; padding: 8px 10px; border-radius: 4px; font-style: normal; }

/* Tone accents on platform cards (subtle) */
.soc-tone-skyblue  { border-top: 2px solid #7fb2d4; }
.soc-tone-graphite { border-top: 2px solid #6b6760; }
.soc-tone-crimson  { border-top: 2px solid #c94c3a; }
.soc-tone-rose     { border-top: 2px solid #c96a8e; }
.soc-tone-steel    { border-top: 2px solid #6b82a5; }
.soc-tone-navy     { border-top: 2px solid #3a5382; }

@media (max-width: 900px) {
  .soc-shell { grid-template-columns: 1fr; }
  .soc-sidebar { border-right: 0; border-bottom: 1px solid var(--soc-rule); }
  .soc-shell:has(.soc-chat-rail:not(.soc-chat-collapsed)) { grid-template-columns: 1fr; }
}
"##;

const PAGE_JS: &str = r##"
const SOC_PLATFORMS = [
  { id: 'bluesky',   name: 'Bluesky',     sub: 'AT Protocol app password',            tone: 'skyblue' },
  { id: 'threads',   name: 'Threads',     sub: 'Meta OAuth2 (via Facebook Graph)',    tone: 'graphite' },
  { id: 'youtube',   name: 'YouTube',     sub: 'Google OAuth2, community posts + comments', tone: 'crimson' },
  { id: 'instagram', name: 'Instagram',   sub: 'Meta OAuth2 (Business or Creator)',   tone: 'rose' },
  { id: 'linkedin',  name: 'LinkedIn',    sub: 'LinkedIn OAuth2 (Member / Page)',     tone: 'steel' },
  { id: 'tiktok',    name: 'TikTok',      sub: 'TikTok for Developers',                tone: 'graphite' },
  { id: 'facebook',  name: 'Facebook',    sub: 'Meta OAuth2 (Page posts)',             tone: 'navy' },
  { id: 'twitter',   name: 'X / Twitter', sub: 'Paid API tier required',               tone: 'graphite' },
];

const SOC_STATUS_LABELS = {
  connected:      { label: 'connected',     cls: 'soc-pill-connected' },
  degraded:       { label: 'degraded',      cls: 'soc-pill-degraded' },
  error:          { label: 'error',         cls: 'soc-pill-error' },
  expired:        { label: 'expired',       cls: 'soc-pill-error' },
  not_configured: { label: 'not connected', cls: 'soc-pill-idle' },
};

function socAuthToken() {
  return sessionStorage.getItem('syntaur_token') || '';
}

function socAuthFetch(url, opts) {
  opts = opts || {};
  opts.headers = opts.headers || {};
  const tok = socAuthToken();
  if (tok) opts.headers['Authorization'] = 'Bearer ' + tok;
  return fetch(url, opts);
}

function socEscape(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// Descriptor cache — the auth_flow drives the wizard modal. Populated on
// first load; refreshed when the user returns to the tab. A descriptor
// with kind == "not_implemented" means the Connect button stays disabled.
let SOC_DESCRIPTORS = {};

function socRenderPlatforms(connMap) {
  const grid = document.getElementById('soc-platform-grid');
  if (!grid) return;
  grid.innerHTML = SOC_PLATFORMS.map(p => {
    const conn = connMap[p.id];
    const desc = SOC_DESCRIPTORS[p.id];
    const statusKey = conn ? (conn.status || 'connected') : 'not_configured';
    const sLabel = (SOC_STATUS_LABELS[statusKey] || SOC_STATUS_LABELS.not_configured);
    const handle = conn && conn.display_name ? `<p class="soc-platform-handle">${socEscape(conn.display_name)}</p>` : '';
    const detail = conn && conn.status_detail ? `<p class="soc-platform-detail">${socEscape(conn.status_detail)}</p>` : '';
    const kind = desc && desc.auth_flow ? desc.auth_flow.kind : 'unknown';
    const hasAdapter = kind && kind !== 'not_implemented' && kind !== 'unknown';
    const isHealthy = statusKey === 'connected';

    // Per-state affordance:
    //  - live adapter + healthy           → whole card clicks into the
    //    per-platform settings panel (voice override, pillars, cadence,
    //    engagement, signature, disconnect). No primary button needed —
    //    the card body is the affordance, plus a subtle "Manage →" hint
    //  - live adapter + broken connection → primary "Reconnect" button
    //  - live adapter + never connected   → primary "Connect" button
    //  - stubbed platform                 → ghost "What's coming" explainer
    let buttonHtml = '';
    let cardCls = `soc-platform-card soc-tone-${socEscape(p.tone)}`;
    let cardOnClick = '';
    if (!hasAdapter) {
      buttonHtml = `<button class="soc-btn-ghost" onclick="socOpenModal('${p.id}')">What's coming</button>`;
    } else if (!isHealthy) {
      const lbl = conn ? 'Reconnect' : 'Connect';
      buttonHtml = `<button class="soc-btn" onclick="socOpenModal('${p.id}')">${lbl}</button>`;
    } else {
      // Healthy: clickable card → platform panel
      cardCls += ' soc-platform-card-clickable';
      cardOnClick = `onclick="socOpenPlatformPanel('${p.id}')"`;
      buttonHtml = `<div class="soc-platform-manage">Manage →</div>`;
    }

    return `
      <div class="${cardCls}" ${cardOnClick}>
        <div class="soc-platform-head">
          <span class="soc-platform-name">${socEscape(p.name)}</span>
          <span class="soc-pill ${sLabel.cls}">${sLabel.label}</span>
        </div>
        ${handle}
        <p class="soc-platform-sub">${socEscape(p.sub)}</p>
        ${detail}
        ${buttonHtml}
      </div>`;
  }).join('');
}

async function socLoadDescriptors() {
  try {
    const tok = socAuthToken();
    const r = await socAuthFetch(`/api/social/platforms?token=${encodeURIComponent(tok)}`);
    if (!r.ok) return;
    const data = await r.json();
    const map = {};
    (data.platforms || []).forEach(d => { map[d.id] = d; });
    SOC_DESCRIPTORS = map;
  } catch (_) { /* descriptors optional for initial render */ }
}

// ── Modal / wizard ──────────────────────────────────────────────────────────

let SOC_MODAL_PLATFORM = null;
let SOC_CONNECTIONS_MAP = {};

function socRenderMd(md) {
  // Intentionally minimal: bold + code. Full markdown is overkill for
  // wizard steps and safer to avoid arbitrary HTML injection.
  return socEscape(md)
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/`([^`]+)`/g, '<code>$1</code>');
}

// Per-platform "coming soon" copy in Nyota's voice, shown when the
// adapter isn't wired yet. Honest about what's planned without making
// empty promises.
const SOC_COMING_SOON = {
  threads:   "Threads runs through Meta's developer platform — same OAuth app as Instagram and Facebook, which is why I'm wiring those three together in one session. Business verification is the long pole; once that's in, the wizard lands with screenshots for every step.",
  instagram: "Instagram connects through the same Meta OAuth app as Threads and Facebook. I'm doing that family in one go so you only go through Meta's setup once.",
  facebook:  "Facebook pages share Meta's OAuth app with Threads and Instagram. Same session will cover all three.",
  linkedin:  "LinkedIn has its own OAuth flow plus a scope list you'll want to review before granting. The wizard will walk through each one.",
  tiktok:    "TikTok for Developers has a separate app-registration flow from everyone else. The wizard covers the business-vs-creator account distinction that trips people up.",
  twitter:   "X requires a paid API tier — Basic is $100/month as of the last time I checked. The wizard will start by confirming your credits are live, then take you through the OAuth. Until you've signed up, this one's a dead end and I'd rather be honest about it than pretend.",
};

function socWizardHtml(desc, conn) {
  const flow = desc.auth_flow || {};
  const kind = flow.kind || 'unknown';

  // "Coming soon" path — platforms without a live adapter yet.
  if (kind === 'not_implemented' || kind === 'unknown') {
    const copy = SOC_COMING_SOON[desc.id] || "Adapter for this platform is planned — it'll arrive in a future release. Nyota's chat rail will walk you through the setup when it does.";
    return `
      <div class="soc-wizard-step">
        <h3 class="soc-wizard-step-title">What's coming for ${socEscape(desc.display_name)}</h3>
        <p class="soc-wizard-step-body">${socEscape(copy)}</p>
      </div>
      <p class="soc-modal-note">—Nyota</p>
      <div id="soc-modal-status"></div>`;
  }

  // Reconnect vs first-time-connect: if a row already exists the user
  // has already done all the provider-console setup work. Skip the
  // setup steps and show a brief re-auth prompt instead.
  const isReconnect = !!conn;

  // Setup wizard — first-time connect only.
  const stepsHtml = isReconnect ? '' : (flow.setup_steps || []).map((s, i) => {
    const linkHtml = s.deep_link
      ? `<a class="soc-wizard-link" href="${socEscape(s.deep_link)}" target="_blank" rel="noopener">${socEscape(s.deep_link)}</a>`
      : '';
    return `
      <div class="soc-wizard-step">
        <h3 class="soc-wizard-step-title">${i+1}. ${socEscape(s.title)}</h3>
        <p class="soc-wizard-step-body">${socRenderMd(s.body_md || '')}</p>
        ${linkHtml}
      </div>`;
  }).join('');

  // Reconnect intro — short explainer pointing at whatever the user
  // most likely needs to do. For app_password platforms we surface the
  // first setup-step deep link so they can rotate the password if they
  // suspect that's what broke. For OAuth we show a stored-token banner.
  let introHtml = '';
  if (isReconnect) {
    if (kind === 'app_password') {
      const firstLinkStep = (flow.setup_steps || []).find(s => s.deep_link);
      const handleNote = conn.display_name
        ? `You were connected as <code>${socEscape(conn.display_name)}</code>.`
        : '';
      const rotateHelp = firstLinkStep
        ? `If you think your app password rotated (or you'd just rather make a fresh one), generate one here first: <a class="soc-wizard-link" href="${socEscape(firstLinkStep.deep_link)}" target="_blank" rel="noopener">${socEscape(firstLinkStep.deep_link)}</a>`
        : '';
      introHtml = `
        <div class="soc-wizard-step">
          <h3 class="soc-wizard-step-title">Reconnect ${socEscape(desc.display_name)}</h3>
          <p class="soc-wizard-step-body">${handleNote} Paste your handle and app password below — I'll verify them with ${socEscape(desc.display_name)} and save a fresh session.</p>
          ${rotateHelp ? `<p class="soc-wizard-step-body" style="margin-top:8px">${rotateHelp}</p>` : ''}
        </div>`;
    } else if (kind === 'oauth2') {
      const handleNote = conn.display_name
        ? `Connected as <code>${socEscape(conn.display_name)}</code>. `
        : '';
      introHtml = `
        <div class="soc-modal-info">
          <strong>You already have a connection on file.</strong>
          <p>${handleNote}Hit <em>Refresh</em> and I'll use your stored refresh token to get a fresh access token from ${socEscape(desc.display_name)}. No re-login required, no forms to fill.</p>
        </div>`;
    } else if (kind === 'paid') {
      introHtml = `
        <div class="soc-modal-info">
          <strong>Reconnect ${socEscape(desc.display_name)}</strong>
          <p>${socEscape(desc.display_name)} requires a paid API tier. If your subscription lapsed, renew it first; then come back here to paste your refreshed key.</p>
        </div>`;
    }
  }

  // Form section — only the data-entry parts. For reconnect-OAuth2 the
  // "form" is just the Refresh button itself, handled in socOpenModal.
  let formHtml = '';
  if (kind === 'app_password') {
    const labels = flow.field_labels || ['Field 1', 'Field 2'];
    const helps  = flow.field_helps  || ['', ''];
    formHtml = `
      <div class="soc-field-group">
        <label class="soc-field-label" for="soc-field-handle">${socEscape(labels[0])}</label>
        <input type="text" id="soc-field-handle" class="soc-field-input" autocomplete="off" spellcheck="false" value="${isReconnect && conn.display_name ? socEscape(conn.display_name) : ''}">
        <div class="soc-field-help">${socEscape(helps[0])}</div>
      </div>
      <div class="soc-field-group">
        <label class="soc-field-label" for="soc-field-password">${socEscape(labels[1])}</label>
        <input type="password" id="soc-field-password" class="soc-field-input" autocomplete="new-password" spellcheck="false">
        <div class="soc-field-help">${socEscape(helps[1])}</div>
      </div>`;
  } else if (kind === 'oauth2' && !isReconnect) {
    formHtml = `
      <div class="soc-modal-info">
        <strong>First-time OAuth connect</strong>
        <p>The popup-based OAuth flow arrives in the next release. For now, if you already have tokens from an existing integration, import them via the <code>/api/social/connections</code> endpoint and come back — the Refresh path takes over from there.</p>
      </div>`;
  } else if (kind === 'paid' && !isReconnect) {
    formHtml = `<div class="soc-modal-info">This platform requires a paid API tier. Sign up first, then come back and paste your key.</div>`;
  }

  return introHtml + stepsHtml + formHtml + '<div id="soc-modal-status"></div>';
}

function socOpenModal(platformId) {
  const desc = SOC_DESCRIPTORS[platformId];
  if (!desc) return;
  SOC_MODAL_PLATFORM = platformId;
  const conn = SOC_CONNECTIONS_MAP[platformId] || null;
  const flow = desc.auth_flow || {};
  const kind = flow.kind || 'unknown';

  const titlePrefix = (kind === 'not_implemented' || kind === 'unknown')
    ? 'About '
    : (conn ? 'Reconnect ' : 'Connect ');
  document.getElementById('soc-modal-title').textContent = titlePrefix + desc.display_name;
  document.getElementById('soc-modal-sub').textContent = desc.tagline || '';
  document.getElementById('soc-modal-body').innerHTML = socWizardHtml(desc, conn);

  const submit = document.getElementById('soc-modal-submit');
  // Enable the submit button only when there's actually a path to take:
  // app_password (filled by user) or oauth2-refresh (existing conn).
  if (kind === 'app_password') {
    submit.disabled = false;
    submit.textContent = conn ? 'Reconnect' : 'Connect';
    submit.dataset.mode = 'fields';
  } else if (kind === 'oauth2' && conn) {
    submit.disabled = false;
    submit.textContent = 'Refresh';
    submit.dataset.mode = 'refresh';
  } else {
    submit.disabled = true;
    submit.textContent = 'OK';
    submit.dataset.mode = 'close';
  }

  document.getElementById('soc-modal').classList.add('soc-modal-open');
}

function socCloseModal() {
  document.getElementById('soc-modal').classList.remove('soc-modal-open');
  SOC_MODAL_PLATFORM = null;
}

function socSetStatus(kind, msg) {
  const el = document.getElementById('soc-modal-status');
  if (!el) return;
  if (!msg) { el.innerHTML = ''; return; }
  const cls = kind === 'error' ? 'soc-modal-error' : 'soc-modal-busy';
  el.innerHTML = `<div class="${cls}">${socEscape(msg)}</div>`;
}

async function socModalSubmit() {
  const platformId = SOC_MODAL_PLATFORM;
  if (!platformId) return;
  const submit = document.getElementById('soc-modal-submit');
  const mode = submit.dataset.mode || 'close';

  if (mode === 'close') { socCloseModal(); return; }

  let fields = {};
  let verb = 'Reconnecting';
  if (mode === 'fields') {
    const handle = (document.getElementById('soc-field-handle').value || '').trim();
    const password = (document.getElementById('soc-field-password').value || '').trim();
    if (!handle || !password) {
      socSetStatus('error', 'Both fields are required.');
      return;
    }
    fields = { handle: handle, app_password: password };
  } else if (mode === 'refresh') {
    verb = 'Refreshing';
    // Empty fields → backend detects existing row + rotates via adapter.refresh()
    fields = {};
  }

  const originalLabel = submit.textContent;
  submit.disabled = true;
  submit.textContent = verb + '…';
  socSetStatus('busy', mode === 'refresh'
    ? 'Asking the platform for a fresh access token…'
    : 'Asking the platform to verify those credentials…');

  try {
    const r = await socAuthFetch(`/api/social/connections/reconnect/${encodeURIComponent(platformId)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: socAuthToken(), fields: fields }),
    });
    const data = await r.json();
    if (!r.ok || !data.ok) {
      socSetStatus('error', data.error || (verb + ' failed.'));
      submit.disabled = false; submit.textContent = 'Try again';
      return;
    }
    socSetStatus('busy', `Connected as ${data.display_name}. Refreshing…`);
    await socRefreshConnections();
    setTimeout(() => socCloseModal(), 600);
  } catch (e) {
    socSetStatus('error', 'Network error — ' + (e.message || 'try again in a moment.'));
    submit.disabled = false; submit.textContent = originalLabel;
  }
}

async function socRefreshConnections() {
  if (Object.keys(SOC_DESCRIPTORS).length === 0) {
    await socLoadDescriptors();
  }
  socRenderPlatforms(SOC_CONNECTIONS_MAP);
  try {
    const r = await socAuthFetch(`/api/social/connections?token=${encodeURIComponent(socAuthToken())}`);
    if (!r.ok) return;
    const rows = await r.json();
    const map = {};
    for (const c of rows) map[c.platform] = c;
    SOC_CONNECTIONS_MAP = map;
    socRenderPlatforms(map);
    socUpdateFirstRunBanner();
    // Re-render Compose's platform picker now that we know what's connected.
    if (typeof socComposeRenderPlatforms === 'function') socComposeRenderPlatforms();
  } catch (_) { /* page renders stub cards even if fetch fails */ }
}
window.addEventListener('DOMContentLoaded', socRefreshConnections);
window.addEventListener('focus', socRefreshConnections);
// Escape closes the modal — expected keyboard habit for dialogs.
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && document.getElementById('soc-modal').classList.contains('soc-modal-open')) {
    socCloseModal();
  }
});

// ── Settings load / save ────────────────────────────────────────────────────
//
// Backed by the existing /api/settings/preferences endpoint (user_preferences
// table per framework §7). Keys are namespaced 'social.*' so they don't
// clash with other modules. Values are strings (JSON.stringify for bools +
// numbers so the round-trip is lossless).

const SOC_PREF_KEYS = [
  // Voice & audience
  'social.brand_voice',
  'social.audience',
  // Approval
  'social.approval_mode',
  // Posting schedule
  'social.schedule.enabled',
  'social.schedule.time',
  // Engagement
  'social.engage.preset',
  'social.engage.likes_per_day',
  'social.engage.follows_per_day',
  'social.engage.unfollow_after_days',
  // Notifications
  'social.notify.telegram',
  'social.notify.stats',
  // Pause
  'social.pause.posting',
  'social.pause.engagement',
  'social.pause.notifications',
  // Privacy
  'social.privacy.calendar',
  'social.privacy.music',
  'social.privacy.research',
  // Advanced — tone + blocklist
  'social.tone.humor',
  'social.tone.formality',
  'social.blocklist.words',
];

let SOC_SETTINGS_BASELINE = {};  // last-loaded values, for dirty + revert
let SOC_SETTINGS_LOADED = false;

function socPrefGet(map, key, fallback) {
  const v = map[key];
  return (v === null || v === undefined) ? fallback : v;
}

function socApplySettingsToUI(map) {
  // Voice & audience
  document.getElementById('soc-set-brand-voice').value = socPrefGet(map, 'social.brand_voice', '');
  document.getElementById('soc-set-audience').value    = socPrefGet(map, 'social.audience', '');
  // Approval mode
  const approval = socPrefGet(map, 'social.approval_mode', 'always_review');
  const approvalRadio = document.querySelector(`input[name="soc-set-approval"][value="${approval}"]`);
  if (approvalRadio) approvalRadio.checked = true;
  // Posting schedule
  document.getElementById('soc-set-schedule-enabled').checked = socPrefGet(map, 'social.schedule.enabled', 'false') === 'true';
  document.getElementById('soc-set-schedule-time').value = socPrefGet(map, 'social.schedule.time', '09:00');
  // Engagement
  document.getElementById('soc-set-engage-preset').value = socPrefGet(map, 'social.engage.preset', 'off');
  document.getElementById('soc-set-engage-likes').value = socPrefGet(map, 'social.engage.likes_per_day', '20');
  document.getElementById('soc-set-engage-follows').value = socPrefGet(map, 'social.engage.follows_per_day', '15');
  document.getElementById('soc-set-engage-unfollow').value = socPrefGet(map, 'social.engage.unfollow_after_days', '7');
  // Notifications
  document.getElementById('soc-set-notify-telegram').checked = socPrefGet(map, 'social.notify.telegram', 'true') === 'true';
  document.getElementById('soc-set-notify-stats').checked = socPrefGet(map, 'social.notify.stats', 'false') === 'true';
  // Pause
  document.getElementById('soc-set-pause-posting').checked = socPrefGet(map, 'social.pause.posting', 'false') === 'true';
  document.getElementById('soc-set-pause-engagement').checked = socPrefGet(map, 'social.pause.engagement', 'false') === 'true';
  document.getElementById('soc-set-pause-notifications').checked = socPrefGet(map, 'social.pause.notifications', 'false') === 'true';
  // Privacy
  document.getElementById('soc-set-privacy-calendar').checked = socPrefGet(map, 'social.privacy.calendar', 'false') === 'true';
  document.getElementById('soc-set-privacy-music').checked = socPrefGet(map, 'social.privacy.music', 'false') === 'true';
  document.getElementById('soc-set-privacy-research').checked = socPrefGet(map, 'social.privacy.research', 'false') === 'true';
  // Advanced — tone + blocklist
  document.getElementById('soc-set-tone-humor').value = socPrefGet(map, 'social.tone.humor', '4');
  document.getElementById('soc-set-tone-formality').value = socPrefGet(map, 'social.tone.formality', '4');
  socSliderLabel('soc-set-tone-humor');
  socSliderLabel('soc-set-tone-formality');
  document.getElementById('soc-set-blocklist').value = socPrefGet(map, 'social.blocklist.words', '');
  socEngagePresetChanged();
  // Global pause indicator in top bar
  socRefreshPauseBadge();
}

function socCollectSettingsFromUI() {
  const approvalChecked = document.querySelector('input[name="soc-set-approval"]:checked');
  return {
    'social.brand_voice':            document.getElementById('soc-set-brand-voice').value,
    'social.audience':               document.getElementById('soc-set-audience').value,
    'social.approval_mode':          (approvalChecked && approvalChecked.value) || 'always_review',
    'social.schedule.enabled':       String(document.getElementById('soc-set-schedule-enabled').checked),
    'social.schedule.time':          document.getElementById('soc-set-schedule-time').value || '09:00',
    'social.engage.preset':          document.getElementById('soc-set-engage-preset').value,
    'social.engage.likes_per_day':   String(document.getElementById('soc-set-engage-likes').value || '20'),
    'social.engage.follows_per_day': String(document.getElementById('soc-set-engage-follows').value || '15'),
    'social.engage.unfollow_after_days': String(document.getElementById('soc-set-engage-unfollow').value || '7'),
    'social.notify.telegram':        String(document.getElementById('soc-set-notify-telegram').checked),
    'social.notify.stats':           String(document.getElementById('soc-set-notify-stats').checked),
    'social.pause.posting':          String(document.getElementById('soc-set-pause-posting').checked),
    'social.pause.engagement':       String(document.getElementById('soc-set-pause-engagement').checked),
    'social.pause.notifications':    String(document.getElementById('soc-set-pause-notifications').checked),
    'social.privacy.calendar':       String(document.getElementById('soc-set-privacy-calendar').checked),
    'social.privacy.music':          String(document.getElementById('soc-set-privacy-music').checked),
    'social.privacy.research':       String(document.getElementById('soc-set-privacy-research').checked),
    'social.tone.humor':             String(document.getElementById('soc-set-tone-humor').value || '4'),
    'social.tone.formality':         String(document.getElementById('soc-set-tone-formality').value || '4'),
    'social.blocklist.words':        document.getElementById('soc-set-blocklist').value,
  };
}

function socSliderLabel(id) {
  const el = document.getElementById(id);
  const lbl = document.getElementById(id + '-val');
  if (el && lbl) lbl.textContent = el.value;
}

function socEngagePresetChanged() {
  const preset = document.getElementById('soc-set-engage-preset').value;
  const box = document.getElementById('soc-engage-custom');
  if (!box) return;
  if (preset === 'custom') box.classList.add('soc-set-custom-open');
  else box.classList.remove('soc-set-custom-open');
}

function socMarkDirty() {
  if (!SOC_SETTINGS_LOADED) return;  // don't flag dirty during initial populate
  const now = socCollectSettingsFromUI();
  const dirty = SOC_PREF_KEYS.some(k => (now[k] || '') !== (SOC_SETTINGS_BASELINE[k] || ''));
  const bar = document.getElementById('soc-settings-savebar');
  if (!bar) return;
  if (dirty) bar.classList.remove('soc-savebar-hidden');
  else bar.classList.add('soc-savebar-hidden');
}

async function socLoadSettings() {
  try {
    const tok = socAuthToken();
    const r = await socAuthFetch(`/api/settings/preferences?token=${encodeURIComponent(tok)}`);
    if (!r.ok) return;
    const prefs = await r.json();
    SOC_SETTINGS_BASELINE = {};
    for (const k of SOC_PREF_KEYS) SOC_SETTINGS_BASELINE[k] = prefs[k] || '';
    SOC_SETTINGS_LOADED = false;
    socApplySettingsToUI(prefs);
    SOC_SETTINGS_LOADED = true;
    document.getElementById('soc-settings-savebar').classList.add('soc-savebar-hidden');
    socUpdateFirstRunBanner();
  } catch (_) {}
}

// Load settings early (not just on Settings-pane entry) so the first-run
// banner + pause badge reflect reality on any page load.
window.addEventListener('DOMContentLoaded', () => { socLoadSettings(); });

// ── Compose ────────────────────────────────────────────────────────────────

const SOC_PLATFORM_LIMITS = {
  bluesky: 300,
  threads: 500,
  youtube: 1200,
  instagram: 2200,
  facebook: 5000,
  linkedin: 3000,
  tiktok:  150,
  twitter: 280,
};

function socComposeRenderPlatforms() {
  const el = document.getElementById('soc-compose-platforms');
  if (!el) return;
  const connected = Object.values(SOC_CONNECTIONS_MAP || {}).filter(c => c && c.status === 'connected');
  if (connected.length === 0) {
    el.innerHTML = `<div class="soc-note">No connected platforms yet. <a href="#connections" onclick="socGoto('connections');return false;" class="soc-wizard-link">Connect one →</a></div>`;
    return;
  }
  el.innerHTML = connected.map(c => {
    const desc = SOC_DESCRIPTORS[c.platform];
    const name = desc ? desc.display_name : c.platform;
    return `
      <label class="soc-compose-plat" data-platform="${socEscape(c.platform)}">
        <input type="checkbox" value="${socEscape(c.platform)}" onchange="socComposeCharCount()">
        <span>${socEscape(name)}</span>
      </label>`;
  }).join('');
}

function socComposeSelected() {
  return Array.from(document.querySelectorAll('#soc-compose-platforms input:checked')).map(el => el.value);
}

function socComposeCharCount() {
  const text = document.getElementById('soc-compose-text').value || '';
  const chosen = socComposeSelected();
  const el = document.getElementById('soc-compose-counts');
  if (chosen.length === 0) { el.innerHTML = '<span class="soc-compose-hint">Pick a platform to see the character count.</span>'; return; }
  el.innerHTML = chosen.map(p => {
    const limit = SOC_PLATFORM_LIMITS[p] || 1000;
    const over = text.length > limit;
    return `<span class="soc-compose-count ${over ? 'soc-compose-over' : ''}">${p}: ${text.length}/${limit}</span>`;
  }).join(' · ');
}

async function socComposeGenerate() {
  const chosen = socComposeSelected();
  if (chosen.length === 0) { socComposeStatus('error', 'Pick a platform first so Nyota knows the vibe.'); return; }
  const platform = chosen[0];
  socComposeStatus('busy', `Nyota is drafting for ${platform}…`);
  try {
    const r = await socAuthFetch('/api/social/drafts', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: socAuthToken(), platform: platform, generate: true, source: 'manual' }),
    });
    const data = await r.json();
    if (!r.ok || !data.ok) { socComposeStatus('error', data.error || 'Draft failed.'); return; }
    // Draft landed in Queue. Pull it into the composer for editing.
    document.getElementById('soc-compose-text').value = data.text || '';
    socComposeCharCount();
    socComposeStatus('', `Drafted. Landed in Queue as id ${data.id} — edit here, or approve it from Queue.`);
    socRefreshQueue();
  } catch (e) {
    socComposeStatus('error', 'Network error — try again.');
  }
}

async function socComposeSaveDraft() {
  const text = document.getElementById('soc-compose-text').value.trim();
  const chosen = socComposeSelected();
  if (!text) { socComposeStatus('error', 'Nothing to save — write something first.'); return; }
  if (chosen.length === 0) { socComposeStatus('error', 'Pick at least one platform.'); return; }
  socComposeStatus('busy', 'Saving…');
  let savedCount = 0;
  for (const platform of chosen) {
    try {
      const r = await socAuthFetch('/api/social/drafts', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: socAuthToken(), platform: platform, text: text, source: 'manual' }),
      });
      if (r.ok) savedCount++;
    } catch (_) {}
  }
  socComposeStatus('', `Saved ${savedCount} draft(s). Queue has them waiting.`);
  document.getElementById('soc-compose-text').value = '';
  socComposeCharCount();
  socRefreshQueue();
}

async function socComposePostNow() {
  const text = document.getElementById('soc-compose-text').value.trim();
  const chosen = socComposeSelected();
  if (!text) { socComposeStatus('error', 'Nothing to post — write something first.'); return; }
  if (chosen.length === 0) { socComposeStatus('error', 'Pick at least one platform.'); return; }
  socComposeStatus('busy', `Posting to ${chosen.join(', ')}…`);
  let posted = 0, failed = [];
  for (const platform of chosen) {
    try {
      const r = await socAuthFetch('/api/social/drafts', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: socAuthToken(), platform: platform, text: text, source: 'manual' }),
      });
      const data = await r.json();
      if (!r.ok || !data.ok) { failed.push(`${platform}: ${data.error || 'create failed'}`); continue; }
      const r2 = await socAuthFetch(`/api/social/drafts/${data.id}/approve`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: socAuthToken() }),
      });
      const d2 = await r2.json();
      if (r2.ok && d2.ok) posted++; else failed.push(`${platform}: ${d2.error || 'post failed'}`);
    } catch (e) { failed.push(`${platform}: ${e.message}`); }
  }
  if (posted && !failed.length) {
    socComposeStatus('', `Posted to ${posted} platform${posted>1?'s':''}. Check Queue for the published URIs.`);
    document.getElementById('soc-compose-text').value = '';
    socComposeCharCount();
  } else if (posted) {
    socComposeStatus('error', `Posted to ${posted}, but: ${failed.join(' | ')}`);
  } else {
    socComposeStatus('error', failed.join(' | ') || 'All posts failed.');
  }
  socRefreshQueue();
}

function socComposeStatus(kind, msg) {
  const el = document.getElementById('soc-compose-status');
  if (!el) return;
  el.textContent = msg || '';
  el.className = 'soc-compose-status' + (kind ? ' soc-compose-status-' + kind : '');
}

// ── Queue ──────────────────────────────────────────────────────────────────

async function socRefreshQueue() {
  const el = document.getElementById('soc-queue-list');
  if (!el) return;
  try {
    const r = await socAuthFetch(`/api/social/drafts?token=${encodeURIComponent(socAuthToken())}`);
    if (!r.ok) return;
    const drafts = await r.json();
    if (!drafts.length) {
      el.innerHTML = `<div class="soc-empty">
        <div class="soc-empty-sigil">⋯</div>
        <h2 class="soc-empty-h">Queue is quiet.</h2>
        <p class="soc-empty-p">Drafts appear here when Nyota writes one on your schedule, or when you save one from Compose.</p>
      </div>`;
      return;
    }
    el.innerHTML = drafts.map(d => socQueueCardHtml(d)).join('');
  } catch (_) {}
}

function socQueueCardHtml(d) {
  const statusCls = {
    pending:   'soc-q-status-pending',
    posted:    'soc-q-status-posted',
    rejected:  'soc-q-status-rejected',
    failed:    'soc-q-status-failed',
    approved:  'soc-q-status-posted',
  }[d.status] || '';
  const when = new Date(d.updated_at * 1000).toLocaleString();
  let actions = '';
  if (d.status === 'pending') {
    actions = `
      <button class="soc-btn" onclick="socQueueApprove(${d.id})">Approve &amp; post</button>
      <button class="soc-btn-secondary" onclick="socQueueRedraft(${d.id})">Redraft</button>
      <button class="soc-btn-ghost" onclick="socQueueReject(${d.id})">Reject</button>`;
  } else if (d.status === 'posted' && d.posted_uri) {
    actions = `<span class="soc-q-posted-uri">${socEscape(d.posted_uri)}</span>`;
  } else if (d.status === 'failed' && d.error_detail) {
    actions = `<span class="soc-q-error">${socEscape(d.error_detail)}</span> <button class="soc-btn-secondary" onclick="socQueueApprove(${d.id})">Retry</button>`;
  }
  const pillar = d.pillar ? `<span class="soc-q-pillar">${socEscape(d.pillar)}</span>` : '';
  return `
    <div class="soc-q-card" data-id="${d.id}">
      <div class="soc-q-head">
        <span class="soc-q-platform">${socEscape(d.platform)}</span>
        <span class="soc-pill ${statusCls}">${d.status}</span>
        ${pillar}
        <span class="soc-q-when">${when}</span>
      </div>
      <textarea class="soc-field-input soc-q-text" rows="4" data-id="${d.id}">${socEscape(d.text)}</textarea>
      <div class="soc-q-actions">${actions}</div>
    </div>`;
}

async function socQueueApprove(id) {
  const ta = document.querySelector(`.soc-q-text[data-id="${id}"]`);
  const edited = ta ? ta.value : undefined;
  const r = await socAuthFetch(`/api/social/drafts/${id}/approve`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: socAuthToken(), edited_text: edited }),
  });
  const d = await r.json();
  if (r.ok && d.ok) socToast(`Posted → ${d.uri || 'done'}`);
  else socToast(`Publish failed: ${d.error || 'unknown'}`);
  socRefreshQueue();
}

async function socQueueRedraft(id) {
  const hint = prompt('Redraft hint (optional — what should Nyota try differently?):') || '';
  const r = await socAuthFetch(`/api/social/drafts/${id}/redraft`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: socAuthToken(), hint: hint }),
  });
  const d = await r.json();
  if (r.ok && d.ok) socToast('Redrafted.');
  else socToast(`Redraft failed: ${d.error || 'unknown'}`);
  socRefreshQueue();
}

async function socQueueReject(id) {
  if (!confirm('Reject this draft?')) return;
  const r = await socAuthFetch(`/api/social/drafts/${id}`, {
    method: 'DELETE', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: socAuthToken() }),
  });
  if (r.ok) socToast('Rejected.');
  socRefreshQueue();
}

// ── Inbox ──────────────────────────────────────────────────────────────────

async function socRefreshInbox() {
  const el = document.getElementById('soc-inbox-list');
  if (!el) return;
  try {
    const r = await socAuthFetch(`/api/social/replies?token=${encodeURIComponent(socAuthToken())}`);
    if (!r.ok) return;
    const replies = await r.json();
    if (!replies.length) {
      el.innerHTML = `<div class="soc-empty">
        <div class="soc-empty-sigil">✉</div>
        <h2 class="soc-empty-h">Nothing waiting on a reply.</h2>
        <p class="soc-empty-p">New mentions and comments land here as they come in. Nyota drafts replies; you approve or edit.</p>
      </div>`;
      return;
    }
    el.innerHTML = replies.map(r => socInboxCardHtml(r)).join('');
  } catch (_) {}
}

function socInboxCardHtml(r) {
  const when = new Date(r.created_at * 1000).toLocaleString();
  const pending = r.status === 'pending';
  const actions = pending ? `
    <button class="soc-btn" onclick="socInboxApprove(${r.id})">Approve reply</button>
    <button class="soc-btn-ghost" onclick="socInboxReject(${r.id})">Skip</button>
  ` : `<span class="soc-pill soc-q-status-posted">${r.status}</span>`;
  return `
    <div class="soc-q-card" data-id="${r.id}">
      <div class="soc-q-head">
        <span class="soc-q-platform">${socEscape(r.platform)}</span>
        <span class="soc-q-when">from ${socEscape(r.parent_author || '?')} · ${when}</span>
      </div>
      <div class="soc-inbox-parent">${socEscape(r.parent_text || '(no text)')}</div>
      <textarea class="soc-field-input soc-q-text" rows="3" data-reply="${r.id}">${socEscape(r.draft_text || '')}</textarea>
      <div class="soc-q-actions">${actions}</div>
    </div>`;
}

async function socInboxApprove(id) {
  const ta = document.querySelector(`.soc-q-text[data-reply="${id}"]`);
  const edited = ta ? ta.value : undefined;
  const r = await socAuthFetch(`/api/social/replies/${id}/approve`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: socAuthToken(), edited_text: edited }),
  });
  const d = await r.json();
  if (r.ok && d.ok) socToast('Reply posted.');
  else socToast(`Failed: ${d.error || 'unknown'}`);
  socRefreshInbox();
}

async function socInboxReject(id) {
  const r = await socAuthFetch(`/api/social/replies/${id}`, {
    method: 'DELETE', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: socAuthToken() }),
  });
  if (r.ok) socToast('Skipped.');
  socRefreshInbox();
}

// ── Analytics ──────────────────────────────────────────────────────────────

async function socRefreshAnalytics() {
  const stats = document.getElementById('soc-analytics-stats');
  const alerts = document.getElementById('soc-alerts-list');
  if (!stats) return;
  try {
    const r = await socAuthFetch(`/api/social/stats?token=${encodeURIComponent(socAuthToken())}`);
    const data = r.ok ? await r.json() : { snapshots: [] };
    const snaps = data.snapshots || [];
    if (!snaps.length) {
      stats.innerHTML = `<div class="soc-empty">
        <div class="soc-empty-sigil">◔</div>
        <h2 class="soc-empty-h">No snapshots yet.</h2>
        <p class="soc-empty-p">Nyota takes a weekly stats snapshot per platform. First one lands after the next Monday 10am tick.</p>
      </div>`;
    } else {
      // Group by platform, show latest snapshot per
      const byPlat = {};
      for (const s of snaps) { if (!byPlat[s.platform] || s.as_of > byPlat[s.platform].as_of) byPlat[s.platform] = s; }
      stats.innerHTML = Object.values(byPlat).map(s => `
        <div class="soc-stats-card">
          <h3 class="soc-stats-plat">${socEscape(s.platform)}</h3>
          <div class="soc-stats-grid">
            <div><span class="soc-stats-label">Followers</span><span class="soc-stats-val">${s.followers ?? '—'}</span></div>
            <div><span class="soc-stats-label">Following</span><span class="soc-stats-val">${s.following ?? '—'}</span></div>
            <div><span class="soc-stats-label">Posts</span><span class="soc-stats-val">${s.posts_count ?? '—'}</span></div>
          </div>
          <div class="soc-stats-asof">snapshot ${new Date(s.as_of * 1000).toLocaleString()}</div>
        </div>`).join('');
    }
  } catch (_) {}
  // Alerts
  try {
    const r = await socAuthFetch(`/api/social/alerts?token=${encodeURIComponent(socAuthToken())}`);
    const data = r.ok ? await r.json() : { alerts: [] };
    const list = data.alerts || [];
    if (!list.length) { alerts.innerHTML = ''; return; }
    alerts.innerHTML = `<h3 class="soc-alerts-h">Open alerts</h3>` + list.map(a => `
      <div class="soc-alert-card">
        <span class="soc-pill soc-q-status-failed">${socEscape(a.alert_type)}</span>
        <span class="soc-alert-plat">${socEscape(a.platform)}</span>
        <p class="soc-alert-detail">${socEscape(a.detail)}</p>
      </div>`).join('');
  } catch (_) {}
}

// Refresh the right pane content when user navigates there
const _socPrevActivate = socActivate;
socActivate = function(section) {
  _socPrevActivate(section);
  if (section === 'compose')   { socComposeRenderPlatforms(); socComposeCharCount(); }
  if (section === 'queue')     { socRefreshQueue(); }
  if (section === 'inbox')     { socRefreshInbox(); }
  if (section === 'analytics') { socRefreshAnalytics(); }
};
// Also kick the first pane's loader on initial render
window.addEventListener('DOMContentLoaded', () => { socComposeRenderPlatforms(); socComposeCharCount(); });

function socRevertSettings() {
  SOC_SETTINGS_LOADED = false;
  socApplySettingsToUI(SOC_SETTINGS_BASELINE);
  SOC_SETTINGS_LOADED = true;
  document.getElementById('soc-settings-savebar').classList.add('soc-savebar-hidden');
  socToast('Reverted unsaved changes.');
}

async function socSaveSettings() {
  const now = socCollectSettingsFromUI();
  const changed = SOC_PREF_KEYS.filter(k => (now[k] || '') !== (SOC_SETTINGS_BASELINE[k] || ''));
  if (changed.length === 0) {
    document.getElementById('soc-settings-savebar').classList.add('soc-savebar-hidden');
    return;
  }
  const tok = socAuthToken();
  let ok = true;
  for (const key of changed) {
    try {
      const r = await socAuthFetch('/api/settings/preferences', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: tok, key: key, value: now[key] }),
      });
      if (!r.ok) { ok = false; break; }
    } catch (_) { ok = false; break; }
  }
  if (ok) {
    SOC_SETTINGS_BASELINE = { ...now };
    document.getElementById('soc-settings-savebar').classList.add('soc-savebar-hidden');
    socToast('Settings saved.');
    socRefreshPauseBadge();
    socUpdateFirstRunBanner();
  } else {
    socToast('Couldn\'t save — try again in a moment.');
  }
}

// ── Pause indicator in top bar ─────────────────────────────────────────────

function socRefreshPauseBadge() {
  const paused =
    document.getElementById('soc-set-pause-posting')?.checked ||
    document.getElementById('soc-set-pause-engagement')?.checked ||
    document.getElementById('soc-set-pause-notifications')?.checked;
  const pill = document.getElementById('soc-pause-pill');
  if (!pill) return;
  if (paused) pill.classList.remove('soc-pause-hidden');
  else        pill.classList.add('soc-pause-hidden');
}

// ── First-run nudge ────────────────────────────────────────────────────────
// On Connections, if the user has a connected platform but no brand voice
// saved, show a warm Nyota-voice banner linking to Settings.

function socUpdateFirstRunBanner() {
  const banner = document.getElementById('soc-firstrun-banner');
  if (!banner) return;
  const haveConnection = Object.values(SOC_CONNECTIONS_MAP || {}).some(c => c && c.status === 'connected');
  const haveVoice = (SOC_SETTINGS_BASELINE['social.brand_voice'] || '').trim().length > 0;
  if (haveConnection && !haveVoice) banner.classList.remove('soc-firstrun-hidden');
  else                              banner.classList.add('soc-firstrun-hidden');
}

// ── Nyota-assist modal ─────────────────────────────────────────────────────
// Inline helper that asks Nyota to draft a field value from a short user
// description (or sample posts for brand_voice).

let SOC_ASSIST_TARGET = null;  // { intent, fieldId }

const SOC_ASSIST_CONFIG = {
  brand_voice: {
    title: 'Help me draft your brand voice',
    subtitle: 'Paste 2–4 of your recent posts — I\'ll listen for tone, what you care about, what you avoid, and draft a paragraph. You can edit it after.',
    prompt: 'Paste posts (one per line is fine)…',
    key: 'sample_posts',
    rows: 8,
  },
  audience: {
    title: 'Sharpen your audience sketch',
    subtitle: 'Who are you writing for? A few words is enough — I\'ll tighten it.',
    prompt: 'e.g. people who love indie folk, listen to vinyl, late 20s-early 40s',
    key: 'content',
    rows: 3,
  },
  blocklist: {
    title: 'Draft a blocklist from a description',
    subtitle: 'Describe what to avoid — words, vibes, phrases. I\'ll produce a comma-separated list you can edit.',
    prompt: 'e.g. no growth-hacker language, no tapestry/delve/unpack, no grindset',
    key: 'content',
    rows: 4,
  },
};

function socAssistOpen(intent, fieldId) {
  const cfg = SOC_ASSIST_CONFIG[intent];
  if (!cfg) return;
  SOC_ASSIST_TARGET = { intent, fieldId };
  document.getElementById('soc-assist-title').textContent = cfg.title;
  document.getElementById('soc-assist-subtitle').textContent = cfg.subtitle;
  const ta = document.getElementById('soc-assist-input');
  ta.value = '';
  ta.rows = cfg.rows;
  ta.placeholder = cfg.prompt;
  document.getElementById('soc-assist-result').textContent = '';
  document.getElementById('soc-assist-result-row').style.display = 'none';
  document.getElementById('soc-assist-status').textContent = '';
  document.getElementById('soc-assist-submit').disabled = false;
  document.getElementById('soc-assist-submit').textContent = 'Draft it';
  document.getElementById('soc-assist-modal').classList.add('soc-modal-open');
  setTimeout(() => ta.focus(), 100);
}

function socAssistClose() {
  document.getElementById('soc-assist-modal').classList.remove('soc-modal-open');
  SOC_ASSIST_TARGET = null;
}

async function socAssistSend() {
  if (!SOC_ASSIST_TARGET) return;
  const { intent, fieldId } = SOC_ASSIST_TARGET;
  const cfg = SOC_ASSIST_CONFIG[intent];
  const ta = document.getElementById('soc-assist-input');
  const val = ta.value.trim();
  if (!val) { socAssistStatus('error', 'Give me something to work with.'); return; }
  const submit = document.getElementById('soc-assist-submit');
  submit.disabled = true; submit.textContent = 'Drafting…';
  socAssistStatus('busy', 'Listening…');
  const body = { token: socAuthToken(), intent: intent };
  if (cfg.key === 'sample_posts') {
    body.sample_posts = val.split(/\n\n+|\n/).map(s => s.trim()).filter(s => s.length > 0);
  } else {
    body.content = val;
  }
  try {
    const r = await socAuthFetch('/api/social/nyota/assist', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const data = await r.json();
    if (!r.ok || !data.ok) {
      socAssistStatus('error', data.error || 'I couldn\'t draft that.');
      submit.disabled = false; submit.textContent = 'Try again';
      return;
    }
    document.getElementById('soc-assist-result').textContent = data.draft;
    document.getElementById('soc-assist-result-row').style.display = 'block';
    socAssistStatus('', '');
    submit.disabled = false; submit.textContent = 'Re-draft';
  } catch (e) {
    socAssistStatus('error', 'Network error — try again.');
    submit.disabled = false; submit.textContent = 'Try again';
  }
}

function socAssistKeep() {
  if (!SOC_ASSIST_TARGET) return;
  const draft = document.getElementById('soc-assist-result').textContent;
  const field = document.getElementById(SOC_ASSIST_TARGET.fieldId);
  if (field) {
    field.value = draft;
    socMarkDirty();
  }
  socAssistClose();
  socToast('Dropped into the field — edit it before you save.');
}

function socAssistStatus(kind, msg) {
  const el = document.getElementById('soc-assist-status');
  if (!el) return;
  el.textContent = msg || '';
  el.className = 'soc-assist-status' + (kind ? ' soc-assist-status-' + kind : '');
}

// ── Per-connection slide-in panel ──────────────────────────────────────────

const SOC_PLATFORM_KEYS = [
  'voice_override',
  'pillars',
  'cadence.days',
  'cadence.time',
  'cadence.auto_post',
  'engage.hashtags',
  'signature',
  'approval_override',
  'pause',
];

let SOC_PANEL_PLATFORM = null;
let SOC_PANEL_BASELINE = {};

function socPlatformPrefKey(platformId, suffix) { return `social.platform.${platformId}.${suffix}`; }

async function socOpenPlatformPanel(platformId) {
  const conn = SOC_CONNECTIONS_MAP[platformId];
  const desc = SOC_DESCRIPTORS[platformId];
  if (!conn || !desc) return;
  SOC_PANEL_PLATFORM = platformId;

  // Load current prefs (from the global preferences fetch)
  const tok = socAuthToken();
  try {
    const r = await socAuthFetch(`/api/settings/preferences?token=${encodeURIComponent(tok)}`);
    const prefs = r.ok ? await r.json() : {};
    SOC_PANEL_BASELINE = {};
    for (const suffix of SOC_PLATFORM_KEYS) {
      SOC_PANEL_BASELINE[suffix] = prefs[socPlatformPrefKey(platformId, suffix)] || '';
    }
  } catch (_) { SOC_PANEL_BASELINE = {}; }

  const handleLine = conn.display_name ? `Connected as <code>${socEscape(conn.display_name)}</code>` : '';
  document.getElementById('soc-plat-title').textContent = desc.display_name;
  document.getElementById('soc-plat-handle').innerHTML = handleLine;

  const globalBrand = SOC_SETTINGS_BASELINE['social.brand_voice'] || '';
  document.getElementById('soc-plat-voice-placeholder').textContent =
    globalBrand ? `Blank = inherit global voice: "${globalBrand.slice(0, 120)}${globalBrand.length > 120 ? '…' : ''}"` : 'Blank = use your global brand voice.';

  document.getElementById('soc-plat-voice').value = SOC_PANEL_BASELINE['voice_override'] || '';
  // Pillars: JSON array or newline-separated
  let pillarsText = '';
  try {
    const arr = JSON.parse(SOC_PANEL_BASELINE['pillars'] || '[]');
    if (Array.isArray(arr)) pillarsText = arr.join('\n');
  } catch (_) { pillarsText = SOC_PANEL_BASELINE['pillars'] || ''; }
  document.getElementById('soc-plat-pillars').value = pillarsText;

  const days = (SOC_PANEL_BASELINE['cadence.days'] || '').split(',').filter(d => d);
  ['mon','tue','wed','thu','fri','sat','sun'].forEach(d => {
    const box = document.getElementById('soc-plat-day-' + d);
    if (box) box.checked = days.includes(d);
  });
  document.getElementById('soc-plat-time').value = SOC_PANEL_BASELINE['cadence.time'] || '09:00';
  document.getElementById('soc-plat-autopost').checked = SOC_PANEL_BASELINE['cadence.auto_post'] === 'true';

  document.getElementById('soc-plat-hashtags').value = SOC_PANEL_BASELINE['engage.hashtags'] || '';
  document.getElementById('soc-plat-signature').value = SOC_PANEL_BASELINE['signature'] || '';
  document.getElementById('soc-plat-approval').value = SOC_PANEL_BASELINE['approval_override'] || 'inherit';
  document.getElementById('soc-plat-pause').checked = SOC_PANEL_BASELINE['pause'] === 'true';

  document.getElementById('soc-plat-status').textContent = '';
  document.getElementById('soc-plat-panel').classList.add('soc-plat-open');
}

function socClosePlatformPanel() {
  document.getElementById('soc-plat-panel').classList.remove('soc-plat-open');
  SOC_PANEL_PLATFORM = null;
}

async function socPlatformPanelSave() {
  if (!SOC_PANEL_PLATFORM) return;
  const platformId = SOC_PANEL_PLATFORM;

  const days = ['mon','tue','wed','thu','fri','sat','sun']
    .filter(d => document.getElementById('soc-plat-day-' + d)?.checked);
  const pillarsArr = (document.getElementById('soc-plat-pillars').value || '')
    .split('\n').map(s => s.trim()).filter(s => s);

  const now = {
    'voice_override':     document.getElementById('soc-plat-voice').value,
    'pillars':            JSON.stringify(pillarsArr),
    'cadence.days':       days.join(','),
    'cadence.time':       document.getElementById('soc-plat-time').value || '09:00',
    'cadence.auto_post':  String(document.getElementById('soc-plat-autopost').checked),
    'engage.hashtags':    document.getElementById('soc-plat-hashtags').value,
    'signature':          document.getElementById('soc-plat-signature').value,
    'approval_override':  document.getElementById('soc-plat-approval').value,
    'pause':              String(document.getElementById('soc-plat-pause').checked),
  };
  const changed = SOC_PLATFORM_KEYS.filter(k => (now[k] || '') !== (SOC_PANEL_BASELINE[k] || ''));
  if (changed.length === 0) { socClosePlatformPanel(); return; }

  document.getElementById('soc-plat-status').textContent = 'Saving…';
  const tok = socAuthToken();
  let ok = true;
  for (const suffix of changed) {
    try {
      const r = await socAuthFetch('/api/settings/preferences', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: tok, key: socPlatformPrefKey(platformId, suffix), value: now[suffix] }),
      });
      if (!r.ok) { ok = false; break; }
    } catch (_) { ok = false; break; }
  }
  if (ok) {
    SOC_PANEL_BASELINE = { ...now };
    socToast(`${platformId} settings saved.`);
    socClosePlatformPanel();
  } else {
    document.getElementById('soc-plat-status').textContent = 'Couldn\'t save — try again.';
  }
}

async function socPlatformDisconnect() {
  if (!SOC_PANEL_PLATFORM) return;
  const platformId = SOC_PANEL_PLATFORM;
  const conn = SOC_CONNECTIONS_MAP[platformId];
  if (!conn) return;
  const confirmText = prompt(`Disconnect ${platformId}? Type "disconnect" to confirm.`);
  if (confirmText !== 'disconnect') return;
  try {
    const r = await socAuthFetch(`/api/social/connections/${conn.id}`, {
      method: 'DELETE',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: socAuthToken() }),
    });
    if (r.ok) {
      socToast(`${platformId} disconnected.`);
      socClosePlatformPanel();
      await socRefreshConnections();
    } else {
      socToast('Couldn\'t disconnect — try again.');
    }
  } catch (_) { socToast('Network error — try again.'); }
}

function socToast(msg) {
  const t = document.getElementById('soc-toast');
  if (!t) return;
  t.textContent = msg;
  t.classList.add('soc-toast-open');
  setTimeout(() => t.classList.remove('soc-toast-open'), 2500);
}

// Browser-level guard against losing unsaved changes on navigation.
window.addEventListener('beforeunload', (e) => {
  const bar = document.getElementById('soc-settings-savebar');
  if (bar && !bar.classList.contains('soc-savebar-hidden')) {
    e.preventDefault();
    e.returnValue = '';
  }
});

// Load settings on first entry to the pane + whenever the user returns to it.
let SOC_SETTINGS_LOADED_ONCE = false;
function socOnSectionChange(section) {
  if (section === 'settings' && !SOC_SETTINGS_LOADED_ONCE) {
    SOC_SETTINGS_LOADED_ONCE = true;
    socLoadSettings();
  }
}
window.addEventListener('focus', () => {
  if (SOC_SETTINGS_LOADED_ONCE) socLoadSettings();
});

// Deep-linkable section switching via hash.
function socActivate(section) {
  document.querySelectorAll('.soc-nav-row').forEach(el => {
    el.classList.toggle('soc-nav-active', el.dataset.section === section);
  });
  document.querySelectorAll('.soc-pane').forEach(el => {
    el.classList.toggle('soc-pane-active', el.id === 'pane-' + section);
  });
  socOnSectionChange(section);
}
function socGoto(section) {
  location.hash = section;
  socActivate(section);
}
document.querySelectorAll('.soc-nav-row').forEach(el => {
  el.addEventListener('click', e => {
    e.preventDefault();
    const s = el.dataset.section;
    if (s) socGoto(s);
  });
});
const initial = (location.hash || '#compose').replace('#', '');
socActivate(initial);

// Chat rail toggle.
function socToggleChat() {
  const rail = document.getElementById('soc-chat-rail');
  const btn  = document.getElementById('soc-chat-toggle');
  const open = rail.classList.toggle('soc-chat-collapsed');
  if (open) { btn.classList.remove('active'); } else { btn.classList.add('active'); }
}

// Nyota chat rail — posts to /api/message with agent='social' so the
// conversation lands on Nyota's persona. Keeps its own rolling
// conversation_id so the context accumulates across turns.
let SOC_CHAT_CONV_ID = null;

// Mirror of main.rs::persona_thinking_bank("nyota") — shown as the
// grey italic thought line while an LLM turn is in flight, matching
// the thinking-bubble pattern every other persona's chat surface uses.
const SOC_NYOTA_THOUGHTS = [
  'Reading the draft.',
  'One moment — checking the line.',
  'Looking at recent posts.',
  'Pulling the draft queue.',
  'Checking what\u2019s scheduled.',
  'Re-reading the tone.',
];

function socChatAppend(role, text) {
  const body = document.getElementById('soc-chat-body');
  if (!body) return;
  const div = document.createElement('div');
  div.className = 'soc-chat-msg ' + (role === 'user' ? 'soc-chat-msg-user' : 'soc-chat-msg-nyota');
  if (role === 'user') {
    div.innerHTML = `<p>${socEscape(text)}</p>`;
  } else {
    div.innerHTML = `<p>${socEscape(text).replace(/\n\n+/g,'</p><p>').replace(/\n/g,'<br>')}</p><p class="soc-chat-signoff">—Nyota</p>`;
  }
  body.appendChild(div);
  body.scrollTop = body.scrollHeight;
  return div;
}

function socChatThinkingBubble() {
  const body = document.getElementById('soc-chat-body');
  if (!body) return null;
  const div = document.createElement('div');
  div.className = 'soc-chat-msg soc-chat-msg-nyota soc-chat-thinking';
  div.innerHTML = `
    <span class="soc-chat-dots"><span></span><span></span><span></span></span>
    <span class="soc-chat-thought"></span>`;
  body.appendChild(div);
  body.scrollTop = body.scrollHeight;
  const thought = div.querySelector('.soc-chat-thought');
  // Rotate a phrase from the Nyota bank every 3s so the bubble feels alive
  let i = Math.floor(Math.random() * SOC_NYOTA_THOUGHTS.length);
  thought.textContent = SOC_NYOTA_THOUGHTS[i];
  const handle = setInterval(() => {
    i = (i + 1) % SOC_NYOTA_THOUGHTS.length;
    if (thought) thought.textContent = SOC_NYOTA_THOUGHTS[i];
  }, 3000);
  return { el: div, stop: () => clearInterval(handle) };
}

async function socChatSend(ev) {
  ev.preventDefault();
  const input = document.getElementById('soc-chat-input');
  const text = (input.value || '').trim();
  if (!text) return false;
  input.value = '';
  socChatAppend('user', text);
  const bubble = socChatThinkingBubble();
  try {
    // Lazily create a conversation the first time we need one.
    if (!SOC_CHAT_CONV_ID) {
      const cr = await socAuthFetch('/api/conversations', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: socAuthToken(), agent: 'social' }),
      });
      const cd = cr.ok ? await cr.json() : {};
      if (cd.id) SOC_CHAT_CONV_ID = cd.id;
    }
    const r = await socAuthFetch('/api/message', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        token: socAuthToken(),
        agent: 'social',
        message: text,
        conversation_id: SOC_CHAT_CONV_ID,
      }),
    });
    const data = await r.json();
    const reply = data.response || data.error || '(no response)';
    if (bubble) {
      bubble.stop();
      bubble.el.classList.remove('soc-chat-thinking');
      bubble.el.innerHTML = `<p>${socEscape(reply).replace(/\n\n+/g,'</p><p>').replace(/\n/g,'<br>')}</p><p class="soc-chat-signoff">—Nyota</p>`;
    }
    document.getElementById('soc-chat-body').scrollTop = 1e9;
  } catch (e) {
    if (bubble) {
      bubble.stop();
      bubble.el.classList.remove('soc-chat-thinking');
      bubble.el.innerHTML = `<p class="soc-chat-err">Couldn't reach Nyota — ${socEscape(e.message || 'try again')}.</p>`;
    }
  }
  return false;
}
"##;
