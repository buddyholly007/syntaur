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
                    div class="soc-empty" {
                        div class="soc-empty-sigil" { "✎" }
                        h2 class="soc-empty-h" { "Nothing drafted yet." }
                        p class="soc-empty-p" {
                            "Before you can compose, connect at least one platform so Nyota knows where things go."
                        }
                        a href="#connections" class="soc-cta" onclick="socGoto('connections')" {
                            "Go to Connections →"
                        }
                    }
                }

                // ─ Queue ───────────────────────────────────────────────
                section id="pane-queue" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Queue" }
                        p class="soc-subhead" { "Drafts waiting on your yes, and posts scheduled for later." }
                    }
                    div class="soc-empty" {
                        div class="soc-empty-sigil" { "⋯" }
                        h2 class="soc-empty-h" { "Queue is quiet." }
                        p class="soc-empty-p" {
                            "Drafts show up here when Nyota writes one on a schedule, or when you save something for later."
                        }
                    }
                }

                // ─ Inbox ───────────────────────────────────────────────
                section id="pane-inbox" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Inbox" }
                        p class="soc-subhead" { "Mentions, replies, and comments across your connected platforms." }
                    }
                    div class="soc-empty" {
                        div class="soc-empty-sigil" { "✉" }
                        h2 class="soc-empty-h" { "Nothing waiting on a reply." }
                        p class="soc-empty-p" {
                            "Once a platform is connected, new mentions and comments land here. Nyota drafts replies; you approve or edit."
                        }
                    }
                }

                // ─ Analytics ───────────────────────────────────────────
                section id="pane-analytics" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Analytics" }
                        p class="soc-subhead" { "What landed, what didn't, what you've been posting about." }
                    }
                    div class="soc-empty" {
                        div class="soc-empty-sigil" { "◔" }
                        h2 class="soc-empty-h" { "No posts yet, no story to tell." }
                        p class="soc-empty-p" {
                            "After you've posted a few things, this pane will show the posts that resonated, the topics you've leaned on, and how often you've been showing up. Not a growth chart — just a mirror."
                        }
                    }
                }

                // ─ Connections ─────────────────────────────────────────
                section id="pane-connections" class="soc-pane" {
                    div class="soc-pane-head" {
                        h1 class="soc-h1" { "Connections" }
                        p class="soc-subhead" { "Which platforms Nyota can speak to, and how healthy each one is." }
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
                        p class="soc-subhead" { "Voice, schedule, engagement strategy, notifications, privacy." }
                    }
                    div class="soc-settings-stack" {
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Brand voice" }
                            p class="soc-setting-p" { "One paragraph describing how you want to sound. Nyota seeds every LLM draft with this." }
                            p class="soc-setting-hint" { "Not set yet — you'll configure this in a later phase." }
                        }
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Posting schedule" }
                            p class="soc-setting-p" { "When daily auto-drafts land in your queue. Default: 9am local." }
                            p class="soc-setting-hint" { "Not set yet." }
                        }
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Engagement strategy" }
                            p class="soc-setting-p" { "Presets — artist, small business, creator, podcaster — or custom rules. Turn off entirely if you don't want auto-engagement." }
                            p class="soc-setting-hint" { "Not set yet." }
                        }
                        div class="soc-setting-card" {
                            h3 class="soc-setting-h" { "Notifications" }
                            p class="soc-setting-p" { "Where drafts, replies, and alerts reach you. Web dashboard + Telegram mirror are both supported." }
                            p class="soc-setting-hint" { "Not set yet." }
                        }
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
                div class="soc-chat-body" {
                    div class="soc-chat-msg soc-chat-msg-nyota" {
                        p { "Frequencies open. Walk me through what you want to say — I'll help you land it." }
                        p class="soc-chat-signoff" { "—Nyota" }
                    }
                }
                form class="soc-chat-form" onsubmit="return socChatSend(event)" {
                    input type="text" id="soc-chat-input" placeholder="Say the thing..." autocomplete="off";
                    button type="submit" class="soc-btn" { "Send" }
                }
                p class="soc-chat-note" { "Chat wiring ships in a later phase — this rail is a placeholder for now." }
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
.soc-note {
  margin-top: 24px; padding: 14px 18px;
  font-family: 'Playfair Display', Georgia, serif; font-style: italic;
  color: var(--soc-ink-soft); font-size: 14px;
  border-left: 2px solid var(--soc-amber);
  background: #fffdf6;
}

/* Settings */
.soc-settings-stack { display: flex; flex-direction: column; gap: 12px; max-width: 640px; }
.soc-setting-card { border: 1px solid var(--soc-rule); border-radius: 10px; padding: 16px 18px; background: #fffdf6; }
.soc-setting-h { font-family: 'Playfair Display', Georgia, serif; font-size: 15px; font-weight: 600; margin: 0 0 4px; color: var(--soc-ink); }
.soc-setting-p { font-size: 13px; color: var(--soc-ink-mute); margin: 0 0 4px; }
.soc-setting-hint { font-size: 12px; color: var(--soc-ink-soft); margin: 0; font-style: italic; }

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
.soc-chat-note { margin-top: 10px; font-size: 11px; color: var(--soc-ink-soft); font-style: italic; }

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

function socRenderPlatforms(connMap) {
  const grid = document.getElementById('soc-platform-grid');
  if (!grid) return;
  grid.innerHTML = SOC_PLATFORMS.map(p => {
    const conn = connMap[p.id];
    const statusKey = conn ? (conn.status || 'connected') : 'not_configured';
    const sLabel = (SOC_STATUS_LABELS[statusKey] || SOC_STATUS_LABELS.not_configured);
    const handle = conn && conn.display_name ? `<p class="soc-platform-handle">${socEscape(conn.display_name)}</p>` : '';
    const detail = conn && conn.status_detail ? `<p class="soc-platform-detail">${socEscape(conn.status_detail)}</p>` : '';
    const btnLabel = conn ? (statusKey === 'connected' ? 'Connected' : 'Reconnect — next phase') : 'Connect — next phase';
    return `
      <div class="soc-platform-card soc-tone-${socEscape(p.tone)}">
        <div class="soc-platform-head">
          <span class="soc-platform-name">${socEscape(p.name)}</span>
          <span class="soc-pill ${sLabel.cls}">${sLabel.label}</span>
        </div>
        ${handle}
        <p class="soc-platform-sub">${socEscape(p.sub)}</p>
        ${detail}
        <button class="soc-btn-ghost" disabled>${btnLabel}</button>
      </div>`;
  }).join('');
}

async function socRefreshConnections() {
  socRenderPlatforms({});
  try {
    const r = await socAuthFetch(`/api/social/connections?token=${encodeURIComponent(socAuthToken())}`);
    if (!r.ok) return;
    const rows = await r.json();
    const map = {};
    for (const c of rows) map[c.platform] = c;
    socRenderPlatforms(map);
  } catch (_) { /* page renders stub cards even if fetch fails */ }
}
window.addEventListener('DOMContentLoaded', socRefreshConnections);
window.addEventListener('focus', socRefreshConnections);

// Deep-linkable section switching via hash.
function socActivate(section) {
  document.querySelectorAll('.soc-nav-row').forEach(el => {
    el.classList.toggle('soc-nav-active', el.dataset.section === section);
  });
  document.querySelectorAll('.soc-pane').forEach(el => {
    el.classList.toggle('soc-pane-active', el.id === 'pane-' + section);
  });
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
function socChatSend(ev) {
  ev.preventDefault();
  const input = document.getElementById('soc-chat-input');
  if (!input.value.trim()) return false;
  // Wiring ships in a later phase.
  input.value = '';
  return false;
}
"##;
