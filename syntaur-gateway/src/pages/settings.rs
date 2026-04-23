//! /settings — command-center style settings page.
//!
//! Two-pane sidebar layout organized by user goal — Set up / Helpers /
//! Connect things / Account / Appearance / Advanced. Six groups replace
//! the old 10-section engineering taxonomy. Deep-linkable via URL hash:
//! `#setup/start`, `#helpers/all`, `#connect/telegram`, `#advanced/brain`.
//! Old hashes (`#llm/providers`, `#integrations/telegram`, etc.) are
//! silently redirected by the JS in NEW_JS so bookmarks keep working.
//! Includes a ⌘K command palette that indexes every setting leaf.
//!
//! Legacy tab HTML chunks are extracted from the former raw `BODY_HTML`
//! string and kept as `include_str!` blobs in `settings_chunks/` so the
//! existing JS (form wiring, LLM provider CRUD, sync connect flow, admin
//! invites, etc.) keeps working unchanged while we progressively rewrite
//! each tab into proper maud. PAGE_JS is likewise held as `page.js` and
//! included verbatim.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar as shared_top_bar, Page};

// Legacy JS + shared modals stay as raw strings (JS can't be maud; modals
// are a dense tangle of dialogs the JS expects at specific IDs).
const LEGACY_MODALS: &str = include_str!("settings_chunks/modals.html");
const LEGACY_JS:     &str = include_str!("settings_chunks/page.js");
// The six former HTML tab bodies are now real maud functions in
// `settings_legacy::tab_*` — type-checked at compile time, with every
// legacy ID / class / onclick preserved. See `pages/settings_legacy.rs`.
use crate::pages::settings_legacy::{tab_general, tab_llm, tab_sync, tab_media, tab_system, tab_users};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Settings",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    // Substitute the server-rendered palette index into the JS template.
    let resolved_js = NEW_JS.replace("%%SS_INDEX%%", &palette_index_json());
    let body = html! {
        (shared_top_bar("Settings", None))
        (sub_crumb_bar())
        // Agents page CSS — scoped via `.agent-*` class names so it's safe
        // to load globally. Powers the inlined Agent manager on the Agents
        // → All agents sub-page.
        style { (PreEscaped(crate::pages::settings_agents::AGENT_CSS)) }
        div class="ss-shell" {
            (sidebar())
            main class="ss-main" id="ss-main" {
                (content_area())
            }
        }
        // Legacy modals, dialogs, and any DOM nodes the JS expects to find
        // at load. Preserved verbatim until we migrate each to maud.
        (PreEscaped(LEGACY_MODALS))

        // Command palette overlay (⌘K)
        (cmdk_palette())

        // Global "Am I done?" banner — sticky at top of every page except
        // the Setup landing (which has its own larger version). JS hides
        // this on #setup/start and lights it up green when required steps
        // are both done; amber with a CTA when not.
        (setup_status_banner())

        // Global toast stack — ssToast(msg, kind, opts) drops a pill here.
        div id="ss-toasts" class="ss-toast-stack" aria-live="polite" {}

        // Dirty-state banner (hidden by default; shown by JS on change)
        (dirty_banner())

        script { (PreEscaped(LEGACY_JS)) }
        // Agents JS powers the inlined agent manager (agentsLoad / create /
        // delete / import). Runs against the same `/api/agents/*` endpoints.
        script { (PreEscaped(crate::pages::settings_agents::AGENT_JS)) }
        script { (PreEscaped(resolved_js)) }
    };
    Html(shell(page, body).into_string())
}

// ══════════════════════════════════════════════════════════════════════
// Settings-specific sub-crumb bar
// ══════════════════════════════════════════════════════════════════════
// Global navigation now lives in `shared::top_bar`. This thin bar sits
// below it and shows (a) the current settings sub-section ("Agents → Add
// new"), and (b) the in-settings search hint that opens the
// `ssOpenPalette` fuzzy finder. Keep the `#ss-current-crumb` id so
// existing sub-section navigation JS doesn't break.

fn sub_crumb_bar() -> Markup {
    html! {
        div class="ss-subbar" {
            div class="ss-subbar-inner" {
                span class="ss-crumb-page" id="ss-current-crumb" { "" }
                div class="flex-1" {}
                button class="ss-search-hint" onclick="ssOpenPalette()" title="Search settings" {
                    svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" { circle cx="11" cy="11" r="7" {} path d="M21 21l-5-5" {} }
                    span { "Search settings" }
                }
            }
        }
    }
}

fn sidebar() -> Markup {
    html! {
        aside class="ss-sidebar" id="ss-sidebar" {
            div class="ss-sidebar-search" {
                svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" { circle cx="11" cy="11" r="7" {} path d="M21 21l-5-5" {} }
                input id="ss-sidebar-search" type="text" placeholder="Filter…" oninput="ssFilterSidebar(this.value)";
            }
            nav class="ss-sidebar-nav" id="ss-sidebar-nav" {
                @for section in SECTIONS { (sidebar_section(section)) }
            }
        }
    }
}

fn sidebar_section(sec: &SectionDef) -> Markup {
    html! {
        div class="ss-sec" data-section=(sec.slug) {
            div class="ss-sec-title" { (sec.title) }
            @for page in sec.pages {
                a class="ss-nav-item"
                   data-section=(sec.slug)
                   data-page=(page.slug)
                   href={ "#" (sec.slug) "/" (page.slug) }
                   onclick={ "ssNavigate('" (sec.slug) "','" (page.slug) "');return false;" } {
                    span class="ss-nav-label" { (page.title) }
                    @if let Some(badge) = page.badge {
                        span class="ss-nav-badge" { (badge) }
                    }
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Content area — every sub-page rendered, only the active one visible.
// Client-side JS toggles `.ss-page.active` based on URL hash.
// ══════════════════════════════════════════════════════════════════════

fn content_area() -> Markup {
    html! {
        // ── SET UP SYNTAUR ─────────────────────────────────
        // Default landing. Phase 2 rewrites this into a full stepped flow;
        // for now it reuses the existing welcome / checklist body.
        (page_wrap("setup", "start", "Set up Syntaur",
            "One page to get you running. Everything else is optional.",
            home_body()))

        // ── YOUR HELPERS ───────────────────────────────────
        (page_wrap("helpers", "all", "Your helpers",
            "Create, import, and manage the helpers you talk to.",
            agents_all_body()))
        (page_wrap("helpers", "personas", "Built-in helper styles",
            "Eight built-in styles, each with its own voice and role.",
            agents_personas_body()))

        // ── CONNECT THINGS ─────────────────────────────────
        (page_wrap("connect", "things", "All connections",
            "Everything Syntaur can connect to, with one-click sign-in.",
            connect_things_body()))
        (page_wrap("connect", "telegram", "Telegram (phone)",
            "Chat with Syntaur from your phone through a Telegram bot.",
            integration_telegram_body()))
        (page_wrap("connect", "homeassistant", "Smart home",
            "Give Syntaur access to lights, locks, temperature, and more.",
            integration_ha_body()))
        (page_wrap("connect", "sync", "Cloud services",
            "Sign in to cloud accounts Syntaur can read from.",
            integration_sync_body()))
        (page_wrap("connect", "media", "Music companion",
            "Small app on your computer so Syntaur can play music in the background.",
            integration_media_body()))

        // ── YOUR ACCOUNT ───────────────────────────────────
        (page_wrap("account", "profile", "Profile",
            "Your name and how Syntaur addresses you.",
            account_profile_body()))
        (page_wrap("account", "security", "Password",
            "Change your password, sign out of other browsers.",
            account_security_body()))
        (page_wrap("account", "api-tokens", "API tokens",
            "Long-lived tokens for scripts, CLIs, and third-party integrations. Never paste these into the login form — use your password instead.",
            account_api_tokens_body()))
        (page_wrap("account", "users", "Other people on this Syntaur",
            "Invite, enable, or remove users. Admin only.",
            tab_users()))
        (page_wrap("account", "privacy", "What Syntaur stores",
            "A plain table of what's kept, where, and for how long.",
            privacy_data_body()))

        // ── HOW SYNTAUR LOOKS ──────────────────────────────
        (page_wrap("appearance", "theme", "Color and layout",
            "Dashboard colors and spacing. Module themes stay as-is.",
            appearance_theme_body()))

        // ── ADVANCED ───────────────────────────────────────
        (page_wrap("advanced", "brain", "Brain settings",
            "Which AI model runs each helper. Backup brains, API keys, and model details.",
            html! {
                // Helpers cross-link — so users see which of their named
                // helpers will be affected when they change the model chain.
                // JS fills in the count/names from /health.
                div class="ss-agent-brain-link" {
                    span class="ss-agent-brain-label" { "These models power every helper — " }
                    span class="ss-agent-brain-name" id="ss-brain-agent-list" { "your helpers" }
                    span class="ss-agent-brain-label" { "." }
                    a class="ss-agent-brain-change" href="#helpers/all" onclick="ssNavigate('helpers','all');return false;" {
                        "See helpers →"
                    }
                }
                div class="ss-card" {
                    div class="ss-card-head" {
                        h3 class="ss-card-title" { "AI model providers" }
                        (status_pill("ss-pill-llm"))
                    }
                    p class="ss-help" {
                        "Providers are tried in order. If the main one is down, "
                        "Syntaur automatically uses the backup."
                    }
                    (tab_llm())
                }
            }))
        (page_wrap("advanced", "voice", "Voice speakers",
            "Smart speakers that listen for the wake word and talk to Syntaur.",
            html! {
                div class="ss-card" {
                    div class="ss-card-head" {
                        h3 class="ss-card-title" { "Voice speakers" }
                        (status_pill("ss-pill-voice"))
                    }
                    (voice_satellites_body())
                }
            }))
        (page_wrap("advanced", "modules", "Modules",
            "Turn Syntaur features on and off. Each module has its own settings page.",
            modules_installed_body()))
        (page_wrap("advanced", "gateway", "Gateway and ports",
            "Network and runtime configuration. Some changes need a restart. Admin only.",
            tab_system()))
        (page_wrap("advanced", "danger", "Danger zone",
            "Destructive actions. Typed confirmation required. Admin only.",
            system_danger_body()))
        (page_wrap("advanced", "about", "About Syntaur",
            "Version, uptime, tool count, and credits.",
            about_body()))
    }
}

// Connect things — the unified provider table. Every integration
// Syntaur supports in one place, with a status pill and a single action
// button. JS fn ssRefreshConnections() hydrates status from
// /api/settings/integration_status + /api/sync/providers. The underlying
// connect flows live on the per-category sub-pages (telegram / ha /
// sync / media) — this page is the overview, not a rewrite of every
// flow.
fn connect_things_body() -> Markup {
    html! {
        p class="ss-help" {
            "Every service Syntaur can connect to. ✓ means signed in. "
            "Click any row to manage it."
        }

        // Everyday
        (connect_category("Everyday", &[
            ConnectRow { id: "tailscale",     name: "Remote access (Tailscale)", desc: "HTTPS from anywhere — phone on cellular, no VPN toggle", icon: "🌐", target: ("connect", "tailscale") },
            ConnectRow { id: "telegram",      name: "Phone (Telegram)",   desc: "Chat with Syntaur from anywhere",         icon: "📱", target: ("connect", "telegram") },
            ConnectRow { id: "homeassistant", name: "Smart home",         desc: "Home Assistant — lights, locks, sensors", icon: "🏠", target: ("connect", "homeassistant") },
            ConnectRow { id: "voice",         name: "Voice speakers",     desc: "Speakers with wake word (Hey Peter)",      icon: "🎙", target: ("advanced", "voice") },
        ]))

        // Google / Microsoft / Apple — the major cloud accounts that
        // bring along calendar, email, and drive in one sign-in.
        (connect_category("Your accounts", &[
            ConnectRow { id: "sync:google_workspace", name: "Google",     desc: "Calendar, email, Drive, YouTube",  icon: "🔵", target: ("connect", "sync") },
            ConnectRow { id: "sync:microsoft",       name: "Microsoft",  desc: "Outlook, OneDrive, Teams",          icon: "🟦", target: ("connect", "sync") },
            ConnectRow { id: "sync:apple",           name: "Apple",       desc: "iCloud calendar + reminders",       icon: "🍎", target: ("connect", "sync") },
        ]))

        // Finance
        (connect_category("Money", &[
            ConnectRow { id: "sync:plaid",     name: "Banks (Plaid)",    desc: "Checking, savings, credit cards",   icon: "🏦", target: ("connect", "sync") },
            ConnectRow { id: "sync:simplefin", name: "Banks (SimpleFIN)", desc: "Low-cost alternative to Plaid",     icon: "🏦", target: ("connect", "sync") },
            ConnectRow { id: "sync:stripe",    name: "Stripe",            desc: "Payments you've received",          icon: "💳", target: ("connect", "sync") },
            ConnectRow { id: "sync:coinbase",  name: "Coinbase",          desc: "Crypto balances + transactions",    icon: "₿",  target: ("connect", "sync") },
        ]))

        // Music
        (connect_category("Music", &[
            ConnectRow { id: "media_bridge", name: "Music companion app", desc: "Play music in the background without popup tabs", icon: "🎶", target: ("connect", "media") },
            ConnectRow { id: "sync:spotify",      name: "Spotify",        desc: "Your library + playback",           icon: "🟢", target: ("connect", "sync") },
            ConnectRow { id: "sync:apple_music",  name: "Apple Music",    desc: "Your library + playback",           icon: "🍎", target: ("connect", "sync") },
            ConnectRow { id: "sync:tidal",        name: "Tidal",          desc: "Your library + playback",           icon: "🌊", target: ("connect", "sync") },
            ConnectRow { id: "sync:youtube_music", name: "YouTube Music", desc: "Your library + playback",           icon: "🎵", target: ("connect", "sync") },
        ]))

        div class="ss-help" style="margin-top: 8px" {
            "Don't see something? "
            a href="#connect/sync" class="ss-link" onclick="ssNavigate('connect','sync');return false;" {
                "The full provider list lives here →"
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ConnectRow {
    /// Matches the `id` field in /api/settings/integration_status OR
    /// `sync:<provider>` for per-sync entries from /api/sync/providers.
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    icon: &'static str,
    /// Which sub-page handles the actual connect/manage flow.
    target: (&'static str, &'static str),
}

fn connect_category(title: &str, rows: &[ConnectRow]) -> Markup {
    html! {
        div class="ss-conn-section" {
            h3 class="ss-conn-title" { (title) }
            div class="ss-conn-list" {
                @for row in rows {
                    @if row.id == "tailscale" {
                        // Tailscale deep-links out to the dedicated setup
                        // page which has the OAuth / auth-key flow. The
                        // rest of the Connect rows stay inside Settings'
                        // hash-router.
                        a class="ss-conn-row"
                           data-conn=(row.id)
                           href="/setup/tailscale" {
                            span class="ss-conn-ico" { (row.icon) }
                            div class="ss-conn-main" {
                                div class="ss-conn-name" { (row.name) }
                                div class="ss-conn-desc" { (row.desc) }
                            }
                            span class="ss-conn-status" data-conn-status=(row.id) { "checking…" }
                            span class="ss-conn-action" { "Manage →" }
                        }
                    } @else {
                        a class="ss-conn-row"
                           data-conn=(row.id)
                           href={ "#" (row.target.0) "/" (row.target.1) }
                           onclick={ "ssNavigate('" (row.target.0) "','" (row.target.1) "');return false;" } {
                            span class="ss-conn-ico" { (row.icon) }
                            div class="ss-conn-main" {
                                div class="ss-conn-name" { (row.name) }
                                div class="ss-conn-desc" { (row.desc) }
                            }
                            span class="ss-conn-status" data-conn-status=(row.id) { "checking…" }
                            span class="ss-conn-action" { "Manage →" }
                        }
                    }
                }
            }
        }
    }
}

fn page_wrap(section: &str, page: &str, title: &str, subtitle: &str, body: Markup) -> Markup {
    let scope = lookup_scope(section, page);
    html! {
        section class="ss-page" data-section=(section) data-page=(page) id={ "ss-page-" (section) "-" (page) } {
            header class="ss-page-header" {
                div class="ss-page-title-row" {
                    h1 class="ss-page-title" { (title) }
                    @if !scope.is_empty() {
                        span class={ "ss-scope ss-scope-" (scope) } title="Who this setting applies to" { (scope) }
                    }
                }
                p class="ss-page-subtitle" { (subtitle) }
            }
            div class="ss-page-body" { (body) }
        }
    }
}

fn lookup_scope(section: &str, page: &str) -> &'static str {
    for s in SECTIONS {
        if s.slug == section {
            for p in s.pages {
                if p.slug == page { return p.scope; }
            }
        }
    }
    ""
}

// ── Set up Syntaur — stepped one-page flow ─────────────────
// Each step is a card showing EITHER the action button(s) if not done
// yet, OR the done-state with what was chosen. A single "Syntaur is
// ready" banner at the top lights up when both required steps are
// satisfied. JS fn ssRefreshSetup() fills each step's state from
// /health + /api/setup/status + /api/agents/list.
fn home_body() -> Markup {
    html! {
        // Ready banner — hidden until both required steps are done.
        div class="ss-ready" id="ss-ready-banner" style="display:none" {
            div class="ss-ready-check" { "✓" }
            div class="ss-ready-body" {
                div class="ss-ready-title" { "Syntaur is ready." }
                div class="ss-ready-sub" { "You can close this page and start using it." }
            }
            a href="/" class="ss-btn-primary" { "Go to Syntaur →" }
        }

        // Warm hero when still setting up.
        div class="ss-setup-hero" id="ss-setup-hero" {
            div class="ss-setup-eyebrow" { "Set up" }
            h2 class="ss-setup-title" { "Let's get you going." }
            p class="ss-setup-sub" {
                "Each step takes about a minute. Steps 1 and 2 are required; "
                "3 and 4 are optional — you can add them whenever."
            }
        }

        // STEP 1 — Brain
        (setup_step(
            "brain", "1", "Give Syntaur a brain",
            "This is the AI model that powers every helper you talk to.",
            html! {
                div class="ss-step-actions" {
                    button class="ss-btn-primary" onclick="ssSetupGoBrain()" {
                        "Set up a brain →"
                    }
                    a class="ss-step-help" href="#advanced/brain" onclick="ssNavigate('advanced','brain');return false;" {
                        "I already have one, take me to the details"
                    }
                }
            },
        ))

        // STEP 2 — Helper
        (setup_step(
            "helper", "2", "Meet your helper",
            "Pick which helper Syntaur talks to you as by default. You can add more later.",
            html! {
                div class="ss-step-actions" {
                    button class="ss-btn-primary" onclick="ssNavigate('helpers','all')" {
                        "Pick your main helper →"
                    }
                    a class="ss-step-help" href="#helpers/personas" onclick="ssNavigate('helpers','personas');return false;" {
                        "Show me the built-in styles"
                    }
                }
            },
        ))

        // STEP 3 — Where
        div class="ss-step" data-step="where" {
            div class="ss-step-num" { "3" }
            div class="ss-step-body" {
                div class="ss-step-head" {
                    div class="ss-step-title" { "Where do you want to talk to Syntaur?" }
                    div class="ss-step-optional" { "optional" }
                }
                div class="ss-step-desc" {
                    "You can always use this page. Add your phone or a voice speaker if you want."
                }
                div class="ss-where-row" data-where="computer" {
                    span class="ss-where-check ss-where-done" { "✓" }
                    span class="ss-where-label" { "On this computer" }
                    span class="ss-where-note" { "already set up" }
                }
                div class="ss-where-row" data-where="phone" id="ss-where-phone" {
                    span class="ss-where-check" { "○" }
                    span class="ss-where-label" { "On your phone (Telegram)" }
                    a class="ss-where-action" href="#connect/telegram" onclick="ssNavigate('connect','telegram');return false;" { "Connect →" }
                }
                div class="ss-where-row" data-where="voice" id="ss-where-voice" {
                    span class="ss-where-check" { "○" }
                    span class="ss-where-label" { "With your voice (smart speaker)" }
                    a class="ss-where-action" href="#advanced/voice" onclick="ssNavigate('advanced','voice');return false;" { "Show me how →" }
                }
            }
        }

        // STEP 4 — Connect the things Syntaur can see
        div class="ss-step" data-step="things" {
            div class="ss-step-num" { "4" }
            div class="ss-step-body" {
                div class="ss-step-head" {
                    div class="ss-step-title" { "Connect the things Syntaur can see" }
                    div class="ss-step-optional" { "optional" }
                }
                div class="ss-step-desc" {
                    "Give Syntaur access to your calendar, music, and home so "
                    "helpers can use them. Nothing gets shared until you sign in."
                }
                div class="ss-things-grid" {
                    a class="ss-things-tile" href="#connect/sync" onclick="ssNavigate('connect','sync');return false;" {
                        div class="ss-things-ico" { "📅" }
                        div class="ss-things-label" { "Calendar" }
                        div class="ss-things-sub" { "Google or Apple" }
                    }
                    a class="ss-things-tile" href="#connect/sync" onclick="ssNavigate('connect','sync');return false;" {
                        div class="ss-things-ico" { "🎵" }
                        div class="ss-things-label" { "Music" }
                        div class="ss-things-sub" { "Spotify, Apple Music, more" }
                    }
                    a class="ss-things-tile" href="#connect/homeassistant" onclick="ssNavigate('connect','homeassistant');return false;" {
                        div class="ss-things-ico" { "🏠" }
                        div class="ss-things-label" { "Smart home" }
                        div class="ss-things-sub" { "Lights, locks, sensors" }
                    }
                    a class="ss-things-tile" href="#connect/things" onclick="ssNavigate('connect','things');return false;" {
                        div class="ss-things-ico" { "➕" }
                        div class="ss-things-label" { "See everything" }
                        div class="ss-things-sub" { "Banks, email, more" }
                    }
                }
            }
        }

        div class="ss-tip" {
            "Tip — press "
            kbd { "⌘K" }
            " to jump to any setting."
        }
    }
}

// A stepped-flow card. Shows the action area by default, and JS swaps
// in a done state (filled by ssRefreshSetup) once the step is complete.
fn setup_step(id: &str, num: &str, title: &str, desc: &str, action: Markup) -> Markup {
    html! {
        div class="ss-step" data-step=(id) id={ "ss-step-" (id) } {
            div class="ss-step-num" { (num) }
            div class="ss-step-body" {
                div class="ss-step-head" {
                    div class="ss-step-title" { (title) }
                }
                div class="ss-step-desc" { (desc) }
                div class="ss-step-state" id={ "ss-step-" (id) "-state" } {
                    // Default: show the action. JS replaces with done-state
                    // when the step has been completed.
                    (action)
                }
            }
        }
    }
}

// ── Account ────────────────────────────────────────────────
fn account_profile_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-field" {
                label class="ss-label" for="acct-name" { "Display name" }
                input id="acct-name" class="ss-input" placeholder="How agents address you" {}
                p class="ss-help" { "Your agents use this when addressing you. First-name is typical." }
            }
            div class="ss-field" {
                label class="ss-label" for="acct-username" { "Username" }
                input id="acct-username" class="ss-input" placeholder="For login" readonly {}
                p class="ss-help" { "Login identifier. Currently read-only." }
            }
            div class="ss-field" {
                label class="ss-label" for="acct-email" { "Email (optional)" }
                input id="acct-email" class="ss-input" type="email" placeholder="For password recovery" {}
                p class="ss-help" { "Optional. Only used for password recovery if enabled." }
            }
            div class="ss-actions" {
                a href="/profile" class="ss-btn-secondary" { "Full profile page →" }
                button class="ss-btn-primary" onclick="ssSaveAccountProfile()" { "Save profile" }
            }
        }
    }
}

fn account_security_body() -> Markup {
    html! {
        div class="ss-card" {
            h3 class="ss-card-title" { "Password" }
            div class="ss-field" {
                label class="ss-label" for="acct-old-pass" { "Current password" }
                input id="acct-old-pass" class="ss-input" type="password" {}
            }
            div class="ss-field" {
                label class="ss-label" for="acct-new-pass" { "New password" }
                input id="acct-new-pass" class="ss-input" type="password" {}
                p class="ss-help" { "At least 8 characters. Any printable character is allowed." }
            }
            div class="ss-actions" {
                button class="ss-btn-primary" onclick="ssChangePassword()" { "Change password" }
            }
        }
        div class="ss-card" {
            h3 class="ss-card-title" { "Active sessions" }
            p class="ss-help" { "Browsers and apps currently signed in with your account." }
            div id="ss-sessions-list" class="ss-list" {}
            div class="ss-actions" {
                button class="ss-btn-secondary" onclick="ssSignOutOthers()" { "Sign out everywhere else" }
            }
        }
    }
}

// ── API tokens (per-user) ──────────────────────────────────
// List + create + revoke flow wired to GET/POST/DELETE /api/me/tokens. JS
// in LEGACY_JS exposes the ssTokensLoad / ssTokensCreate / ssTokensRevoke
// functions that hit those endpoints.
fn account_api_tokens_body() -> Markup {
    html! {
        div class="ss-card" {
            h3 class="ss-card-title" { "What these are" }
            p class="ss-help" {
                "API tokens let scripts and command-line tools talk to Syntaur on your behalf. "
                "They start with "
                code { "ocp_" }
                " and don't expire unless you give them a lifetime or revoke them. "
                "They are NOT meant for logging into the web dashboard — use your password there."
            }
        }

        div class="ss-card" {
            h3 class="ss-card-title" { "Create a new token" }
            div class="ss-field" {
                label class="ss-label" for="tok-name" { "Name" }
                input id="tok-name" class="ss-input" type="text" placeholder="laptop-cli, home-script, …" maxlength="64" {}
                p class="ss-help" { "A label you'll recognize later. 1–64 characters." }
            }
            div class="ss-field" {
                label class="ss-label" for="tok-ttl" { "Lifetime" }
                select id="tok-ttl" class="ss-input" {
                    option value="" { "Never expires" }
                    option value="24" { "1 day" }
                    option value="168" { "1 week" }
                    option value="720" selected { "30 days" }
                    option value="8760" { "1 year" }
                }
            }
            div class="ss-actions" {
                button class="ss-btn-primary" onclick="ssTokensCreate()" { "Create token" }
            }
            // The one-time display. JS unhides this row after a successful
            // mint. Copy-to-clipboard button + revoke-now escape hatch are
            // part of the block so users don't need to hunt for them.
            div id="tok-reveal" class="ss-card" style="display:none;border:1px solid var(--ss-accent, #3b82f6);margin-top:12px" {
                p class="ss-help" style="color:var(--ss-accent,#3b82f6)" {
                    "Copy this token now. It will not be shown again."
                }
                div style="display:flex;gap:8px;align-items:center" {
                    code id="tok-reveal-value" style="flex:1;padding:8px;background:var(--ss-panel-2,#141a24);border-radius:6px;word-break:break-all;font-size:12px" { "" }
                    button class="ss-btn-secondary" onclick="ssTokensCopy()" { "Copy" }
                }
            }
        }

        div class="ss-card" {
            div class="ss-card-head" {
                h3 class="ss-card-title" { "Your tokens" }
                button class="ss-btn-secondary" onclick="ssTokensLoad()" { "Refresh" }
            }
            div id="tok-list" class="ss-list" {
                p class="ss-help" id="tok-list-empty" { "Loading…" }
            }
        }
    }
}

// ── Agents ─────────────────────────────────────────────────
fn agents_all_body() -> Markup {
    html! {
        // Brain cross-link — so users aren't surprised when changing
        // their brain affects every helper. JS fills in the current
        // brain name from /health.
        div class="ss-agent-brain-link" {
            span class="ss-agent-brain-label" { "Every helper uses the same AI model — " }
            span class="ss-agent-brain-name" id="ss-agent-brain-name" { "the one you picked" }
            span class="ss-agent-brain-label" { "." }
            a class="ss-agent-brain-change" href="#advanced/brain" onclick="ssNavigate('advanced','brain');return false;" {
                "Change brain →"
            }
        }
        p class="ss-help" {
            "You can have several helpers with different names and personalities, "
            "and import helpers from other tools as Markdown, plain text, or JSON."
        }
        // Inlined — same component that powers the standalone /settings/agents page.
        (crate::pages::settings_agents::inline_body())
    }
}

fn agents_personas_body() -> Markup {
    html! {
        div class="ss-card" {
            p class="ss-help" {
                "Syntaur comes with eight helper styles. Each has its own voice, role, "
                "and memory. You can rename any of them during setup or from a helper's "
                "detail page."
            }
            div class="ss-persona-grid" {
                (persona_tile("🕷", "Peter", "Personal main helper", "Friendly and warm. Helps with anything you ask."))
                (persona_tile("🧭", "Kyron", "Default main helper", "Calm, competent, always here."))
                (persona_tile("🤖", "Positron", "Ledger + tax", "Precise and formal. Never guesses at numbers."))
                (persona_tile("🔬", "Cortex", "Knowledge + research", "Curious and deep-diving. Loves a tangent."))
                (persona_tile("🎸", "Silvr", "Music", "Picks the next song in one line. No explanation."))
                (persona_tile("🎩", "Thaddeus", "Calendar + todos", "Warm butler energy. Quietly devoted."))
                (persona_tile("💻", "Maurice", "Coders", "Earnest pair programmer. Shows his work."))
                (persona_tile("🍃", "Mushi", "Journal", "Gentle presence. Doesn't rush you."))
            }
        }
    }
}

fn persona_tile(ico: &str, name: &str, role: &str, inspiration: &str) -> Markup {
    html! {
        div class="ss-persona" {
            div class="ss-persona-ico" { (ico) }
            div class="ss-persona-body" {
                div class="ss-persona-name" { (name) }
                div class="ss-persona-role" { (role) }
                div class="ss-persona-insp" { (inspiration) }
            }
        }
    }
}

// ── Integrations ───────────────────────────────────────────
// Status pill — JS fills textContent + class based on /api/settings/
// integration_status. Server renders the empty skeleton with a neutral class.
fn status_pill(id: &str) -> Markup {
    html! {
        span id=(id) class="ss-pill ss-pill-unknown" { "checking…" }
    }
}

fn integration_telegram_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-card-head" {
                h3 class="ss-card-title" { "Telegram bot" }
                (status_pill("ss-pill-telegram"))
            }
            p class="ss-help" {
                "Connecting a Telegram bot lets you chat with your agents from your phone. "
                "Syntaur relays messages between Telegram and your local chat — "
                "nothing of yours ever leaves your server except what the LLM needs."
            }
            // Fold in the legacy Telegram config block — the form fields are
            // already wired to working JS in page.js.
            (tab_general())
        }
    }
}

fn integration_ha_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-card-head" {
                h3 class="ss-card-title" { "Home Assistant" }
                (status_pill("ss-pill-ha"))
            }
            p class="ss-help" {
                "Connect a Home Assistant instance so your agents can read sensor "
                "state (lights, locks, temperatures) and send commands back."
            }
            div class="ss-field" {
                label class="ss-label" for="ha-url" { "Home Assistant URL" }
                input id="ha-url" class="ss-input" placeholder="http://homeassistant.local:8123" {}
                p class="ss-help" { "Reachable from this gateway — same LAN or via Tailscale." }
            }
            div class="ss-field" {
                label class="ss-label" for="ha-token" { "Long-lived access token" }
                input id="ha-token" class="ss-input" type="password" placeholder="eyJhbGc…" {}
                p class="ss-help" { "Generate in HA → Profile → Long-Lived Access Tokens." }
            }
            div class="ss-actions" {
                button class="ss-btn-secondary" onclick="ssHaTest()" { "Test connection" }
                button class="ss-btn-primary" onclick="ssHaSave()" { "Save" }
            }
            p id="ss-ha-result" class="ss-help" {}
        }
    }
}

fn integration_sync_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-card-head" {
                h3 class="ss-card-title" { "Sync providers" }
                (status_pill("ss-pill-sync"))
            }
            p class="ss-help" {
                "Cloud connectors — Google Workspace, Microsoft 365, Plaid banks, "
                "Stripe, Coinbase, SimpleFIN, etc. Grants read-only access unless "
                "a specific provider asks for more."
            }
            (tab_sync())
        }
    }
}

fn integration_media_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-card-head" {
                h3 class="ss-card-title" { "Media bridge" }
                (status_pill("ss-pill-media"))
            }
            p class="ss-help" {
                "Companion app that runs a headless Chromium on your local machine "
                "so agents can play Apple Music / Spotify / Tidal / YouTube hidden "
                "in the background — no popup tabs."
            }
            (tab_media())
        }
    }
}

// ── Voice ──────────────────────────────────────────────────
fn voice_satellites_body() -> Markup {
    html! {
        div class="ss-card" {
            p class="ss-help" {
                "Voice satellites are ESP32/WEB devices running the Syntaur wake word. "
                "Each one can be assigned a room, a default persona, and a voice print."
            }
            div class="ss-actions" {
                a href="/voice-setup" class="ss-btn-primary" { "Open voice setup →" }
            }
        }
    }
}

// ── Modules ────────────────────────────────────────────────
fn modules_installed_body() -> Markup {
    html! {
        div class="ss-card" {
            p class="ss-help" {
                "Each Syntaur module adds agent capabilities (Tax, Music, Coders, Knowledge, Journal, Voice). "
                "Module-specific settings live inside each module's page — look for the gear icon."
            }
            div class="ss-actions" {
                a href="/modules" class="ss-btn-primary" { "Manage modules →" }
            }
            div class="ss-module-grid" {
                (module_tile("💰", "Tax", "/tax", "Receipts, expenses, investments, Form 4868"))
                (module_tile("📚", "Knowledge", "/knowledge", "RAG index + research workflow (Cortex)"))
                (module_tile("🎵", "Music", "/music", "AI DJ with taste graph"))
                (module_tile("💻", "Terminal", "/coders", "Web SSH, pair-programmer"))
                (module_tile("📓", "Journal", "/journal", "Isolated voice journal (Mushi)"))
                (module_tile("🎙", "Voice", "/voice-setup", "Wake word + satellites"))
            }
        }
    }
}

fn module_tile(ico: &str, name: &str, url: &str, desc: &str) -> Markup {
    html! {
        a class="ss-module" href=(url) {
            div class="ss-module-ico" { (ico) }
            div class="ss-module-body" {
                div class="ss-module-name" { (name) }
                div class="ss-module-desc" { (desc) }
            }
        }
    }
}

// ── Appearance ─────────────────────────────────────────────
fn appearance_theme_body() -> Markup {
    html! {
        // Ambient theme tokens — used only for the live preview strip on
        // this page (via body.syntaur-ambient in the JS below). Per-module
        // themes (Music, Knowledge, etc.) are unaffected.
        style { (PreEscaped(crate::pages::theme::THEME_STYLE)) }
        div class="ss-card sd-appearance-root" {
            p class="ss-help" {
                "These settings control the "
                strong { "Dashboard" }
                " only. Each module (Music, Knowledge, Coders, Tax) keeps its own theme."
            }

            // Live preview strip — shows the chosen palette at a glance.
            div class="sd-preview" id="sd-preview" {
                div class="sd-preview-card" {
                    div class="sd-label" { "Preview" }
                    div class="sd-big-num" { "24" }
                    div class="sd-mute" { "widgets available" }
                }
                div class="sd-preview-card sd-preview-card-accent" {
                    div class="sd-label" { "Accent" }
                    div class="sd-preview-accent-block" {}
                }
            }

            div class="ss-field" {
                label class="ss-label" { "Accent" }
                div class="sd-accent-row" {
                    @for (key, name, hue) in [("sage","Sage",135.0), ("indigo","Soft indigo",265.0), ("ochre","Dusk ochre",70.0), ("gray","Warm gray",260.0)] {
                        button type="button" class="sd-accent-chip" data-accent=(key)
                               style=(format!("--swatch: oklch(0.68 0.08 {hue})")) {
                            span class="sd-accent-swatch" {}
                            span class="sd-accent-name" { (name) }
                        }
                    }
                }
                p class="ss-help" { "Hue drifts ±18° through the day (if Hue shift is on) so morning feels cool and dusk feels warm." }
            }

            div class="ss-field" {
                label class="ss-label" { "Theme mode" }
                div class="ss-radio-row" {
                    label class="ss-radio" { input type="radio" name="theme_mode" value="auto"; " Auto (sunrise → sunset)" }
                    label class="ss-radio" { input type="radio" name="theme_mode" value="light"; " Light always" }
                    label class="ss-radio" { input type="radio" name="theme_mode" value="dark"; " Dark always" }
                    label class="ss-radio" { input type="radio" name="theme_mode" value="schedule"; " Schedule" }
                }
                p class="ss-help" { "Auto needs your location below. Schedule uses your fixed light/dark times." }
            }

            div class="ss-field" {
                label class="ss-label" { "Hue shift through the day" }
                label class="ss-toggle" {
                    input type="checkbox" id="sd-hue-shift";
                    span class="ss-toggle-track" { span class="ss-toggle-thumb" {} }
                    span { " Warmer at dusk, cooler at dawn (±18°)" }
                }
            }

            div class="ss-field" {
                label class="ss-label" { "Ambient mode" }
                label class="ss-toggle" {
                    input type="checkbox" id="sd-ambient-mode";
                    span class="ss-toggle-track" { span class="ss-toggle-thumb" {} }
                    span { " Subtle breathing + dusk motes + weather reflection" }
                }
                p class="ss-help" { "Adds a faint living feel to the dashboard. Off by default. Honors your OS reduced-motion setting." }
            }

            div class="ss-field sd-location-field" {
                label class="ss-label" { "Location (for sunrise/sunset)" }
                div class="sd-location-row" {
                    input type="number" id="sd-lat" class="ss-input" step="0.0001" placeholder="Latitude (e.g. 47.6062)";
                    input type="number" id="sd-lon" class="ss-input" step="0.0001" placeholder="Longitude (e.g. -122.3321)";
                    button type="button" class="ss-btn" id="sd-geolocate" { "Use my location" }
                }
                p class="ss-help" { "Only stored on this install — never sent anywhere else." }
            }

            div class="sd-schedule-field ss-field" id="sd-schedule-field" style="display:none" {
                label class="ss-label" { "Schedule (24-hour)" }
                div class="sd-location-row" {
                    label class="sd-inline" { "Light starts " input type="time" id="sd-light-start" class="ss-input" value="07:00"; }
                    label class="sd-inline" { "Dark starts " input type="time" id="sd-dark-start" class="ss-input" value="19:00"; }
                }
            }

            div class="ss-save-row" {
                button type="button" class="ss-btn ss-btn-primary" id="sd-appearance-save" { "Save" }
                span class="ss-save-status" id="sd-appearance-status" {}
            }
        }

        style { (PreEscaped(APPEARANCE_STYLE)) }
        script { (PreEscaped(crate::pages::theme::THEME_SCRIPT)) }
        script { (PreEscaped(APPEARANCE_SCRIPT)) }
    }
}

const APPEARANCE_STYLE: &str = r##"
.sd-appearance-root { background: var(--bg-card, inherit); }
.sd-preview { display: flex; gap: 12px; margin: 12px 0 24px; padding: 16px; background: var(--bg-elev); border: 1px solid var(--line); border-radius: 14px; }
.sd-preview-card { flex:1; padding: 16px; background: var(--bg-card); border: 1px solid var(--line); border-radius: 12px; }
.sd-preview-card-accent { display:flex; flex-direction:column; justify-content:space-between; }
.sd-preview-accent-block { margin-top: 8px; height: 32px; border-radius: 8px; background: var(--accent); }
.sd-big-num { font-size: 32px; font-weight: 700; color: var(--fg); font-variant-numeric: tabular-nums; line-height: 1; }
.sd-mute { color: var(--fg-mute); font-size: 13px; margin-top: 4px; }
.sd-label { color: var(--fg-mute); font-size: 11px; letter-spacing: 0.1em; text-transform: uppercase; font-weight: 600; }
.sd-accent-row { display: flex; gap: 10px; flex-wrap: wrap; }
.sd-accent-chip { display: flex; align-items: center; gap: 10px; padding: 8px 14px 8px 10px; border-radius: 999px; border: 1px solid var(--line, rgba(255,255,255,0.1)); background: transparent; color: inherit; font: inherit; cursor: pointer; transition: border-color 200ms, background 200ms; }
.sd-accent-chip:hover { border-color: var(--swatch); }
.sd-accent-chip[data-active="true"] { border-color: var(--swatch); background: color-mix(in oklab, var(--swatch) 14%, transparent); }
.sd-accent-swatch { width: 20px; height: 20px; border-radius: 50%; background: var(--swatch); flex-shrink: 0; box-shadow: 0 0 0 1px rgb(0 0 0 / 0.1); }
.sd-accent-name { color: inherit; font-size: 14px; }
.sd-location-row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.sd-location-row .ss-input { flex: 1; min-width: 180px; }
.sd-inline { display: flex; align-items: center; gap: 8px; font-size: 14px; }
.ss-save-row { margin-top: 24px; display: flex; gap: 12px; align-items: center; }
.ss-save-status { color: var(--fg-mute); font-size: 13px; }
"##;

const APPEARANCE_SCRIPT: &str = r##"
(function() {
  const root = document.querySelector('.sd-appearance-root');
  if (!root) return;
  // Enable ambient preview on this page only (not all of Settings).
  document.body.classList.add('syntaur-ambient');

  const $ = sel => root.querySelector(sel);
  const chips = root.querySelectorAll('.sd-accent-chip');
  const modeRadios = root.querySelectorAll('input[name=theme_mode]');
  const hueShift = $('#sd-hue-shift');
  const ambientMode = $('#sd-ambient-mode');
  const lat = $('#sd-lat'), lon = $('#sd-lon');
  const geoBtn = $('#sd-geolocate');
  const scheduleField = $('#sd-schedule-field');
  const lightStart = $('#sd-light-start'), darkStart = $('#sd-dark-start');
  const saveBtn = $('#sd-appearance-save'), status = $('#sd-appearance-status');

  const DEFAULT = { accent:'sage', theme_mode:'auto', hue_shift:1, latitude:null, longitude:null, light_start_min:420, dark_start_min:1140, ambient_mode:0 };
  let state = { ...DEFAULT };

  function toHM(min) { const h = Math.floor(min/60), m = min%60; return `${String(h).padStart(2,'0')}:${String(m).padStart(2,'0')}`; }
  function fromHM(s) { if (!s) return 0; const [h,m] = s.split(':').map(Number); return h*60+m; }

  function hydrateUI() {
    chips.forEach(c => c.dataset.active = (c.dataset.accent === state.accent) ? 'true' : 'false');
    modeRadios.forEach(r => { r.checked = (r.value === state.theme_mode); });
    hueShift.checked = !!state.hue_shift;
    if (ambientMode) ambientMode.checked = !!state.ambient_mode;
    lat.value = state.latitude != null ? state.latitude : '';
    lon.value = state.longitude != null ? state.longitude : '';
    lightStart.value = toHM(state.light_start_min);
    darkStart.value = toHM(state.dark_start_min);
    scheduleField.style.display = (state.theme_mode === 'schedule') ? 'block' : 'none';
    if (window.SyntaurTheme) window.SyntaurTheme.setPref(state);
  }

  function collect() {
    state.hue_shift = hueShift.checked ? 1 : 0;
    state.ambient_mode = ambientMode && ambientMode.checked ? 1 : 0;
    state.latitude = lat.value ? parseFloat(lat.value) : null;
    state.longitude = lon.value ? parseFloat(lon.value) : null;
    state.light_start_min = fromHM(lightStart.value);
    state.dark_start_min = fromHM(darkStart.value);
    if (window.SyntaurTheme) window.SyntaurTheme.setPref(state);
  }

  chips.forEach(c => c.addEventListener('click', () => {
    state.accent = c.dataset.accent;
    hydrateUI();
  }));
  modeRadios.forEach(r => r.addEventListener('change', () => {
    state.theme_mode = r.value;
    scheduleField.style.display = (r.value === 'schedule') ? 'block' : 'none';
    collect();
  }));
  [hueShift, ambientMode, lat, lon, lightStart, darkStart].forEach(el => el && el.addEventListener('input', collect));
  if (ambientMode) ambientMode.addEventListener('change', collect);

  geoBtn.addEventListener('click', () => {
    if (!navigator.geolocation) { status.textContent = 'Geolocation not supported'; return; }
    status.textContent = 'Locating…';
    navigator.geolocation.getCurrentPosition(pos => {
      lat.value = pos.coords.latitude.toFixed(4);
      lon.value = pos.coords.longitude.toFixed(4);
      collect();
      status.textContent = 'Location set';
      setTimeout(() => status.textContent = '', 2500);
    }, err => { status.textContent = 'Could not get location (' + err.message + ')'; });
  });

  saveBtn.addEventListener('click', async () => {
    collect();
    status.textContent = 'Saving…';
    try {
      const r = await fetch('/api/appearance', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(state)
      });
      if (!r.ok) { status.textContent = 'Save failed'; return; }
      status.textContent = 'Saved';
      setTimeout(() => status.textContent = '', 2000);
    } catch { status.textContent = 'Save failed'; }
  });

  // Initial load
  fetch('/api/appearance', { credentials:'same-origin' })
    .then(r => r.ok ? r.json() : null)
    .then(d => { if (d && d.accent) state = d; hydrateUI(); })
    .catch(() => hydrateUI());
})();
"##;

// ── Privacy & data ─────────────────────────────────────────
fn privacy_data_body() -> Markup {
    html! {
        div class="ss-card" {
            h3 class="ss-card-title" { "What Syntaur stores" }
            table class="ss-table" {
                thead { tr {
                    th { "What" } th { "Where" } th { "How long" } th { "Who sees it" }
                }}
                tbody {
                    tr { td { "Chat messages" } td { "~/.syntaur/index.db" } td { "Forever, until you delete" } td { "You only" } }
                    tr { td { "Uploaded documents" } td { "~/.syntaur/uploads/" } td { "Until you remove them" } td { "You + granted agents" } }
                    tr { td { "Agent memories" } td { "~/.syntaur/index.db (agent_memories table)" } td { "Until you delete per memory" } td { "Per agent, per your sharing config" } }
                    tr { td { "LLM prompts" } td { "Sent to your configured provider (OpenRouter, local, etc.)" } td { "Provider-dependent (see their retention policy)" } td { "Your provider" } }
                    tr { td { "Telegram messages" } td { "Relayed via Telegram Gateway; stored in telegram_messages table" } td { "30 days default, configurable" } td { "You only" } }
                    tr { td { "Voice transcripts" } td { "~/.syntaur/voice-data/ (local STT)" } td { "Session-only unless saved to journal" } td { "You only" } }
                }
            }
        }
        div class="ss-card" {
            h3 class="ss-card-title" { "Controls" }
            p class="ss-help" { "Toggles persist server-side as user preferences. Changes take effect immediately." }
            div class="ss-toggle-row" {
                div { div { "LLM prompt logging" } div class="ss-help" { "Store the full prompt + response of every LLM call in ~/.syntaur/lcm.db. Useful for debugging, burns storage fast." } }
                label class="ss-switch" { input type="checkbox" id="pref-llm-logging" data-pref="llm_logging" onchange="ssSavePref(this)"; span class="ss-switch-slider" {} }
            }
            div class="ss-toggle-row" {
                div { div { "Voice-transcript auto-save" } div class="ss-help" { "Automatically save every voice interaction to the journal. Off = session-only transcripts." } }
                label class="ss-switch" { input type="checkbox" id="pref-voice-autosave" data-pref="voice_autosave" onchange="ssSavePref(this)"; span class="ss-switch-slider" {} }
            }
            div class="ss-toggle-row" {
                div { div { "Anonymous telemetry" } div class="ss-help" { "Currently off by default. Syntaur does not call home unless you explicitly enable it." } }
                label class="ss-switch" { input type="checkbox" id="pref-telemetry" data-pref="telemetry" onchange="ssSavePref(this)"; span class="ss-switch-slider" {} }
            }
            div class="ss-toggle-row" {
                div { div { "Retain chat history" } div class="ss-help" { "Off = conversations wiped on close. On = persist forever until you delete." } }
                label class="ss-switch" { input type="checkbox" id="pref-chat-retention" data-pref="chat_retention" onchange="ssSavePref(this)" checked; span class="ss-switch-slider" {} }
            }
        }
        div class="ss-card" {
            h3 class="ss-card-title" { "Export / import" }
            p class="ss-help" {
                "Download a portable copy of your config (agents, preferences, non-secret settings) "
                "as " code { "syntaur.json" } ". Secrets — API keys, passwords, OAuth tokens — are "
                "never included. Version-control it, share across installs, restore after a reinstall."
            }
            div class="ss-actions" {
                button class="ss-btn-secondary" onclick="ssExportConfig()" { "Export config (JSON)" }
                button class="ss-btn-secondary" onclick="ssImportConfig()" { "Import config…" }
                input type="file" id="ss-import-file" accept=".json,application/json" class="hidden" onchange="ssHandleImport(this.files[0])";
            }
            p id="ss-export-status" class="ss-help" {}
        }
    }
}

// ── System › danger zone ───────────────────────────────────
fn system_danger_body() -> Markup {
    html! {
        div class="ss-card ss-danger" {
            h3 class="ss-card-title" style="color:var(--ss-danger)" { "Danger zone" }
            p class="ss-help" { "These actions cannot be undone. Typed confirmation required." }
            div class="ss-danger-row" {
                div {
                    div class="ss-danger-name" { "Reset all preferences" }
                    div class="ss-help" { "Clears every row in user_preferences. Keeps your agents, chats, and memories intact." }
                }
                button class="ss-btn-danger" onclick="ssDangerConfirm('reset preferences','ssResetPreferences')" { "Reset…" }
            }
            div class="ss-danger-row" {
                div {
                    div class="ss-danger-name" { "Wipe all agent memories" }
                    div class="ss-help" { "Removes every saved memory across every agent (journal is isolated and never auto-exported, but this wipes it too)." }
                }
                button class="ss-btn-danger" onclick="ssDangerConfirm('wipe all memories','ssWipeMemories')" { "Wipe memories…" }
            }
            div class="ss-danger-row" {
                div {
                    div class="ss-danger-name" { "Factory reset" }
                    div class="ss-help" { "Erase this user's data and configuration. Same as re-running onboarding. Admin-only and irreversible." }
                }
                button class="ss-btn-danger" onclick="ssDangerConfirm('factory reset','ssFactoryReset')" { "Factory reset…" }
            }

            // Hidden type-confirm dialog, reused across all danger actions.
            div id="ss-danger-modal" class="ss-modal hidden" {
                div class="ss-modal-scrim" onclick="ssDangerClose()" {}
                div class="ss-modal-inner" {
                    h3 class="ss-modal-title" { "Confirm destructive action" }
                    p class="ss-modal-body" id="ss-danger-prompt" {}
                    div class="ss-field" {
                        label class="ss-label" for="ss-danger-input" { "Type the phrase to continue" }
                        input id="ss-danger-input" class="ss-input" autocomplete="off" spellcheck="false" {}
                    }
                    div class="ss-actions" {
                        button class="ss-btn-secondary" onclick="ssDangerClose()" { "Cancel" }
                        button id="ss-danger-go" class="ss-btn-danger" disabled onclick="ssDangerExecute()" { "Proceed" }
                    }
                }
            }
        }
    }
}

// ── About ──────────────────────────────────────────────────
fn about_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-about-grid" {
                div { div class="ss-about-label" { "Version" } div class="ss-about-val" id="ss-version" { "—" } }
                div { div class="ss-about-label" { "Uptime" } div class="ss-about-val" id="ss-uptime" { "—" } }
                div { div class="ss-about-label" { "Modules" } div class="ss-about-val" id="ss-modules" { "—" } }
                div { div class="ss-about-label" { "Tools" } div class="ss-about-val" id="ss-tools" { "—" } }
                div { div class="ss-about-label" { "Agents" } div class="ss-about-val" id="ss-agents" { "—" } }
                div { div class="ss-about-label" { "LLM" } div class="ss-about-val" id="ss-llm" { "—" } }
            }
        }
        div class="ss-card" {
            h3 class="ss-card-title" { "Links" }
            ul class="ss-link-list" {
                li { a href="https://github.com/buddyholly007/syntaur" target="_blank" class="ss-link" { "GitHub repo" } }
                li { a href="https://github.com/buddyholly007/syntaur/issues" target="_blank" class="ss-link" { "Report an issue" } }
                li { a href="/history" class="ss-link" { "Conversation history" } }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Cmd-K command palette
// ══════════════════════════════════════════════════════════════════════

fn cmdk_palette() -> Markup {
    html! {
        div id="ss-palette" class="ss-palette hidden" role="dialog" aria-label="Search settings" {
            div class="ss-palette-scrim" onclick="ssClosePalette()" {}
            div class="ss-palette-inner" {
                div class="ss-palette-search-row" {
                    svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" { circle cx="11" cy="11" r="7" {} path d="M21 21l-5-5" {} }
                    input id="ss-palette-input" type="text" placeholder="Search settings — try 'telegram', 'gateway port', 'dark'…" oninput="ssPaletteSearch(this.value)" onkeydown="ssPaletteKey(event)";
                    kbd { "esc" }
                }
                div id="ss-palette-results" class="ss-palette-results" {}
                div class="ss-palette-footer" {
                    span { kbd { "↑↓" } " navigate" }
                    span { kbd { "↵" } " jump" }
                    span { kbd { "esc" } " close" }
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Dirty-state banner (hidden by default; shown by JS on form change)
// ══════════════════════════════════════════════════════════════════════

fn dirty_banner() -> Markup {
    html! {
        div id="ss-dirty" class="ss-dirty hidden" role="status" aria-live="polite" {
            span class="ss-dirty-dot" {}
            span class="ss-dirty-text" { "You have unsaved changes" }
            button class="ss-btn-secondary-sm" onclick="ssRevertDirty()" { "Revert" }
            button class="ss-btn-primary-sm" onclick="ssSaveDirty()" { "Save" }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Sticky "Am I done?" banner — shown at the top of every settings page
// except the Setup landing. JS fills it with one of:
//   • hidden      (on #setup/start — the hero takes over there)
//   • green done  ("Syntaur is fully set up.")
//   • amber todo  ("1 thing left: pick a brain" / "2 things left: …")
// ══════════════════════════════════════════════════════════════════════

fn setup_status_banner() -> Markup {
    html! {
        div id="ss-setup-status" class="ss-setup-status" style="display:none" {
            span class="ss-setup-status-icon" id="ss-setup-status-icon" { "" }
            span class="ss-setup-status-text" id="ss-setup-status-text" { "" }
            a class="ss-setup-status-cta" id="ss-setup-status-cta" href="#setup/start" onclick="ssNavigate('setup','start');return false;" { "" }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
// Section / page index — drives sidebar + palette.
// ══════════════════════════════════════════════════════════════════════

struct SectionDef {
    slug: &'static str,
    title: &'static str,
    pages: &'static [PageDef],
}
struct PageDef {
    slug: &'static str,
    title: &'static str,
    badge: Option<&'static str>,
    /// Keywords for the ⌘K palette — space-separated, lowercase.
    keywords: &'static str,
    description: &'static str,
    /// Scope badge shown on the page header + nav — per-user, server, admin.
    scope: &'static str,
}

const SECTIONS: &[SectionDef] = &[
    SectionDef { slug: "account", title: "Account", pages: &[
        PageDef { slug: "profile", title: "Profile", badge: None, scope: "per-user", keywords: "name email username display", description: "Your identity and personal info" },
        PageDef { slug: "security", title: "Password & security", badge: None, scope: "per-user", keywords: "password login sessions signout logout 2fa", description: "Change password, active sessions" },
        PageDef { slug: "api-tokens", title: "API tokens", badge: None, scope: "per-user", keywords: "api token integration cli ocp bearer authorization scripts", description: "Long-lived tokens for scripts and CLI integrations. Not for login." },
        PageDef { slug: "users", title: "Users", badge: Some("admin"), scope: "admin", keywords: "invite team members users roles admin", description: "Invite and manage users (admin only)" },
    ]},
    SectionDef { slug: "agents", title: "Agents", pages: &[
        PageDef { slug: "all", title: "All agents", badge: None, scope: "per-user", keywords: "create import main thread agents peter felix kyron", description: "Create, import, and manage agents" },
        PageDef { slug: "personas", title: "Personas & tone", badge: None, scope: "per-user", keywords: "personas peter kyron positron cortex silvr thaddeus maurice mushi humor dial", description: "Built-in personas and tone dials" },
    ]},
    SectionDef { slug: "integrations", title: "Integrations", pages: &[
        PageDef { slug: "telegram", title: "Telegram", badge: None, scope: "per-user", keywords: "telegram bot phone chat messaging", description: "Chat from your phone via a Telegram bot" },
        PageDef { slug: "homeassistant", title: "Home Assistant", badge: None, scope: "server-wide", keywords: "home assistant smart home ha homeassistant", description: "Connect to a Home Assistant instance" },
        PageDef { slug: "sync", title: "Sync", badge: None, scope: "per-user", keywords: "google microsoft gmail calendar drive sync oauth plaid stripe coinbase simplefin", description: "Cloud connectors (Google, Microsoft, bank, etc.)" },
        PageDef { slug: "media", title: "Media bridge", badge: None, scope: "per-user", keywords: "media apple music spotify tidal youtube bridge playback", description: "Local companion for hidden playback" },
    ]},
    SectionDef { slug: "llm", title: "LLM", pages: &[
        PageDef { slug: "providers", title: "Providers", badge: None, scope: "server-wide", keywords: "openrouter lm studio turboquant fallback api key model provider openai anthropic claude", description: "Model providers, API keys, fallback chain" },
    ]},
    SectionDef { slug: "voice", title: "Voice", pages: &[
        PageDef { slug: "satellites", title: "Satellites", badge: None, scope: "server-wide", keywords: "voice wake word satellite esphome speaker", description: "Voice satellites and wake word" },
    ]},
    SectionDef { slug: "modules", title: "Modules", pages: &[
        PageDef { slug: "installed", title: "Installed", badge: None, scope: "server-wide", keywords: "modules extensions enable disable tax music knowledge coders journal", description: "Enable and disable modules" },
    ]},
    SectionDef { slug: "appearance", title: "Appearance", pages: &[
        PageDef { slug: "theme", title: "Theme", badge: None, scope: "per-user", keywords: "theme dark mode density accent color appearance", description: "Dashboard palette and density" },
    ]},
    SectionDef { slug: "privacy", title: "Privacy & data", pages: &[
        PageDef { slug: "data", title: "What Syntaur stores", badge: None, scope: "per-user", keywords: "privacy data retention telemetry logging export import", description: "What's stored, where, for how long" },
    ]},
    SectionDef { slug: "system", title: "System", pages: &[
        PageDef { slug: "gateway", title: "Gateway & ports", badge: None, scope: "admin", keywords: "gateway port bind network restart system config", description: "Gateway network + runtime settings" },
        PageDef { slug: "danger", title: "Danger zone", badge: None, scope: "admin", keywords: "reset wipe factory delete danger", description: "Destructive actions" },
    ]},
    SectionDef { slug: "about", title: "About", pages: &[
        PageDef { slug: "info", title: "About this Syntaur", badge: None, scope: "server-wide", keywords: "about version uptime tools licenses", description: "Version, uptime, tool count" },
    ]},
];

// Produce the search-index JSON for the client-side palette.
fn palette_index_json() -> String {
    let mut items = Vec::new();
    for s in SECTIONS {
        for p in s.pages {
            items.push(format!(
                r#"{{"section":"{s}","page":"{p}","section_title":"{st}","title":"{t}","keywords":"{k}","description":"{d}","badge":{b}}}"#,
                s = s.slug, p = p.slug, st = s.title, t = p.title,
                k = p.keywords, d = p.description,
                b = match p.badge { Some(v) => format!(r#""{}""#, v), None => "null".to_string() },
            ));
        }
    }
    format!("[{}]", items.join(","))
}

// ══════════════════════════════════════════════════════════════════════
// Styles + new JS
// ══════════════════════════════════════════════════════════════════════

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }

  :root {
    --ss-bg:        #0b0f15;
    --ss-panel:     #0f141c;
    --ss-panel-2:   #141a24;
    --ss-line:      #1d242f;
    --ss-line-2:    #2a3340;
    --ss-ink:       #e6e9ee;
    --ss-ink-dim:   #9aa3af;
    --ss-ink-mute:  #6a7380;
    --ss-ink-faint: #434b57;
    --ss-accent:    #7aa2ff;
    --ss-accent-2:  #4d6fd0;
    --ss-warn:      #f0b470;
    --ss-danger:    #d97a7a;
    --ss-success:   #7fbf8a;
  }
  body.bg-gray-950 { background: var(--ss-bg) !important; color: var(--ss-ink) !important; }

  /* Preserve legacy form classes referenced by embedded HTML chunks */
  .card { background: var(--ss-panel); border: 1px solid var(--ss-line); border-radius: 10px; padding: 20px; }
  .input { width: 100%; background: var(--ss-panel-2); border: 1px solid var(--ss-line-2); border-radius: 6px; padding: 9px 11px; color: var(--ss-ink); font-size: 13.5px; outline: none; }
  .input:focus { border-color: var(--ss-accent); box-shadow: 0 0 0 1px var(--ss-accent); }
  .label { color: var(--ss-ink); font-size: 13px; font-weight: 500; display: block; margin-bottom: 6px; }
  .btn-primary { background: var(--ss-accent); color: #0a0d12; font-weight: 500; padding: 8px 16px; border-radius: 7px; border: none; cursor: pointer; font-size: 13px; }
  .btn-primary:hover { background: #9bbcff; }
  .btn-secondary { background: var(--ss-panel-2); color: var(--ss-ink); font-weight: 500; padding: 8px 16px; border-radius: 7px; border: 1px solid var(--ss-line-2); cursor: pointer; font-size: 13px; }
  .btn-secondary:hover { border-color: var(--ss-accent); }
  .badge { display: inline-flex; align-items: center; padding: 2px 8px; border-radius: 999px; font-size: 10.5px; font-weight: 500; }
  .badge-green { background: rgba(127,191,138,0.14); color: var(--ss-success); }
  .badge-red { background: rgba(217,122,122,0.14); color: var(--ss-danger); }
  .tab { padding: 7px 14px; font-size: 13px; font-weight: 500; border-radius: 7px; cursor: pointer; background: transparent; border: none; color: var(--ss-ink-dim); }
  .tab.active { background: var(--ss-panel); color: var(--ss-ink); }
  .tab-content { }
  .tab-content.hidden { display: none; }

  /* Top bar */
  .ss-topbar { background: rgba(10,13,18,0.85); border-bottom: 1px solid var(--ss-line); backdrop-filter: blur(8px); position: sticky; top: 0; z-index: 40; }
  .ss-topbar-inner { max-width: 1400px; margin: 0 auto; padding: 10px 18px; display: flex; align-items: center; gap: 10px; }
  /* Settings sub-crumb bar — rides below the shared top bar, sticky to it. */
  .ss-subbar { background: rgba(10,13,18,0.6); border-bottom: 1px solid var(--ss-line); position: sticky; top: 48px; z-index: 30; }
  .ss-subbar-inner { max-width: 1400px; margin: 0 auto; padding: 6px 18px; display: flex; align-items: center; gap: 10px; min-height: 32px; }
  .ss-crumb-sep { color: var(--ss-ink-faint); margin: 0 2px; }
  .ss-crumb { color: var(--ss-ink-dim); font-size: 13.5px; }
  .ss-crumb-page { color: var(--ss-ink); font-size: 13.5px; }
  .ss-crumb-page:not(:empty)::before { content: " / "; color: var(--ss-ink-faint); margin: 0 4px; }
  .ss-link { color: var(--ss-ink-dim); font-size: 13px; text-decoration: none; }
  .ss-link:hover { color: var(--ss-ink); }
  .ss-search-hint {
    display: flex; align-items: center; gap: 6px;
    background: var(--ss-panel); border: 1px solid var(--ss-line-2);
    border-radius: 7px; padding: 5px 9px;
    font-size: 12px; color: var(--ss-ink-dim); cursor: pointer;
  }
  .ss-search-hint:hover { border-color: var(--ss-accent); color: var(--ss-ink); }
  .ss-search-hint kbd { font-family: 'SF Mono', ui-monospace, monospace; font-size: 10px; background: var(--ss-panel-2); padding: 1px 5px; border-radius: 3px; color: var(--ss-ink-dim); }

  /* Two-pane shell */
  .ss-shell { display: grid; grid-template-columns: 260px 1fr; max-width: 1400px; margin: 0 auto; min-height: calc(100vh - 53px); }
  @media (max-width: 900px) { .ss-shell { grid-template-columns: 1fr; } .ss-sidebar { display: none; } .ss-sidebar.open { display: block; position: fixed; inset: 53px 0 0 0; z-index: 50; background: var(--ss-panel); overflow-y: auto; } }

  /* Sidebar */
  .ss-sidebar { border-right: 1px solid var(--ss-line); background: var(--ss-panel); padding: 12px 0; overflow-y: auto; }
  .ss-sidebar-search { padding: 2px 14px 8px; display: flex; align-items: center; gap: 6px; border-bottom: 1px solid var(--ss-line); margin-bottom: 6px; }
  .ss-sidebar-search svg { color: var(--ss-ink-mute); flex-shrink: 0; }
  .ss-sidebar-search input { flex: 1; background: transparent; border: none; outline: none; color: var(--ss-ink); font-size: 12.5px; padding: 6px 0; }
  .ss-sidebar-search input::placeholder { color: var(--ss-ink-mute); }
  .ss-sidebar-nav { padding: 4px 8px; }
  .ss-sec { margin-bottom: 10px; }
  .ss-sec.ss-hidden { display: none; }
  .ss-sec-title { font-size: 10px; font-weight: 600; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ss-ink-mute); padding: 10px 10px 4px; }
  .ss-nav-item {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 10px; font-size: 13px; line-height: 1.3;
    color: var(--ss-ink-dim); text-decoration: none;
    border-radius: 6px; cursor: pointer;
    transition: background 0.12s, color 0.12s;
  }
  .ss-nav-item:hover { background: var(--ss-panel-2); color: var(--ss-ink); }
  .ss-nav-item.active { background: rgba(122,162,255,0.1); color: var(--ss-ink); position: relative; }
  .ss-nav-item.active::before { content: ''; position: absolute; left: 0; top: 4px; bottom: 4px; width: 2px; background: var(--ss-accent); border-radius: 2px; }
  .ss-nav-item.ss-hidden { display: none; }
  .ss-nav-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ss-nav-badge { font-size: 9.5px; padding: 1px 7px; background: var(--ss-line-2); color: var(--ss-ink-mute); border-radius: 999px; font-weight: 500; text-transform: lowercase; }

  /* Main content area */
  .ss-main { padding: 28px 32px 80px; min-width: 0; }
  .ss-page { display: none; max-width: 760px; }
  .ss-page.active { display: block; }
  .ss-page-header { margin-bottom: 20px; border-bottom: 1px solid var(--ss-line); padding-bottom: 14px; }
  .ss-page-title { font-size: 22px; font-weight: 600; letter-spacing: -0.01em; color: var(--ss-ink); margin-bottom: 4px; }
  .ss-page-subtitle { font-size: 13.5px; color: var(--ss-ink-dim); line-height: 1.5; }
  .ss-page-body > * + * { margin-top: 16px; }

  /* Cards */
  .ss-card { background: var(--ss-panel); border: 1px solid var(--ss-line); border-radius: 10px; padding: 18px 20px; }
  .ss-card-title { font-size: 14px; font-weight: 600; color: var(--ss-ink); margin-bottom: 12px; }
  .ss-card.ss-danger { border-color: rgba(217,122,122,0.35); }

  /* Fields */
  .ss-field { margin-bottom: 14px; }
  .ss-field:last-child { margin-bottom: 0; }
  .ss-label { display: block; font-size: 12.5px; font-weight: 500; color: var(--ss-ink); margin-bottom: 6px; }
  .ss-input, .ss-field input, .ss-field select, .ss-field textarea {
    width: 100%; background: var(--ss-panel-2); border: 1px solid var(--ss-line-2);
    border-radius: 6px; padding: 8px 11px; color: var(--ss-ink); font-size: 13.5px;
    outline: none; font-family: inherit;
  }
  .ss-input:focus, .ss-field input:focus, .ss-field select:focus, .ss-field textarea:focus { border-color: var(--ss-accent); box-shadow: 0 0 0 1px var(--ss-accent); }
  .ss-help { font-size: 11.5px; color: var(--ss-ink-mute); margin-top: 6px; line-height: 1.5; }
  .ss-actions { display: flex; gap: 8px; margin-top: 14px; flex-wrap: wrap; align-items: center; }
  .ss-btn-primary { background: var(--ss-accent); color: #0a0d12; border: none; padding: 7px 14px; border-radius: 7px; font-size: 13px; font-weight: 500; cursor: pointer; transition: background 0.12s; }
  .ss-btn-primary:hover { background: #9bbcff; }
  .ss-btn-secondary { background: var(--ss-panel-2); color: var(--ss-ink); border: 1px solid var(--ss-line-2); padding: 7px 14px; border-radius: 7px; font-size: 13px; font-weight: 500; cursor: pointer; text-decoration: none; display: inline-block; transition: border-color 0.12s; }
  .ss-btn-secondary:hover { border-color: var(--ss-accent); color: var(--ss-ink); }
  .ss-btn-danger { background: rgba(217,122,122,0.15); color: var(--ss-danger); border: 1px solid rgba(217,122,122,0.35); padding: 7px 14px; border-radius: 7px; font-size: 13px; font-weight: 500; cursor: pointer; }
  .ss-btn-danger:hover { background: rgba(217,122,122,0.25); }

  .ss-radio-row { display: flex; gap: 18px; }
  .ss-radio { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--ss-ink); cursor: pointer; }
  .ss-swatch-row { display: flex; gap: 8px; }
  .ss-swatch { width: 28px; height: 28px; border-radius: 7px; cursor: pointer; border: 2px solid transparent; transition: border-color 0.12s; }
  .ss-swatch:hover, .ss-swatch.active { border-color: var(--ss-ink); }
  .ss-toggle-row { display: flex; align-items: center; justify-content: space-between; padding: 9px 0; border-bottom: 1px dashed var(--ss-line); font-size: 13px; }
  .ss-toggle-row:last-child { border-bottom: none; }

  /* Welcome grid */
  .ss-welcome-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  @media (max-width: 700px) { .ss-welcome-grid { grid-template-columns: repeat(2, 1fr); } }
  .ss-welcome-tile {
    background: var(--ss-panel); border: 1px solid var(--ss-line); border-radius: 10px;
    padding: 14px 16px; text-decoration: none; color: var(--ss-ink);
    transition: border-color 0.12s, transform 0.12s;
    display: block;
  }
  .ss-welcome-tile:hover { border-color: var(--ss-accent); transform: translateY(-1px); }
  .ss-welcome-ico { font-size: 22px; margin-bottom: 4px; }
  .ss-welcome-label { font-size: 14px; font-weight: 600; }
  .ss-welcome-sub { font-size: 11.5px; color: var(--ss-ink-mute); margin-top: 2px; }
  .ss-tip { margin-top: 22px; padding: 10px 14px; background: var(--ss-panel-2); border: 1px dashed var(--ss-line-2); border-radius: 7px; font-size: 12.5px; color: var(--ss-ink-mute); }
  .ss-tip kbd { font-family: 'SF Mono', ui-monospace, monospace; font-size: 10.5px; background: var(--ss-panel); padding: 1px 6px; border-radius: 4px; color: var(--ss-ink-dim); border: 1px solid var(--ss-line-2); }

  /* Personas grid */
  .ss-persona-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px; margin-top: 8px; }
  @media (max-width: 600px) { .ss-persona-grid { grid-template-columns: 1fr; } }
  .ss-persona { display: flex; gap: 10px; padding: 12px; background: var(--ss-panel-2); border: 1px solid var(--ss-line); border-radius: 8px; }
  .ss-persona-ico { font-size: 22px; flex-shrink: 0; }
  .ss-persona-name { font-size: 13.5px; font-weight: 600; color: var(--ss-ink); }
  .ss-persona-role { font-size: 11.5px; color: var(--ss-accent); margin-top: 1px; }
  .ss-persona-insp { font-size: 11.5px; color: var(--ss-ink-mute); margin-top: 3px; line-height: 1.4; }

  /* Modules grid */
  .ss-module-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px; margin-top: 14px; }
  @media (max-width: 600px) { .ss-module-grid { grid-template-columns: 1fr; } }
  .ss-module { display: flex; gap: 10px; padding: 12px 14px; background: var(--ss-panel-2); border: 1px solid var(--ss-line); border-radius: 8px; text-decoration: none; color: var(--ss-ink); transition: border-color 0.12s; }
  .ss-module:hover { border-color: var(--ss-accent); }
  .ss-module-ico { font-size: 18px; flex-shrink: 0; }
  .ss-module-name { font-size: 13px; font-weight: 600; color: var(--ss-ink); }
  .ss-module-desc { font-size: 11.5px; color: var(--ss-ink-mute); margin-top: 2px; line-height: 1.4; }

  /* Privacy table */
  .ss-table { width: 100%; border-collapse: collapse; margin-top: 8px; }
  .ss-table th, .ss-table td { padding: 8px 10px; text-align: left; border-bottom: 1px solid var(--ss-line); font-size: 12.5px; }
  .ss-table th { color: var(--ss-ink-mute); font-weight: 500; font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; }
  .ss-table td { color: var(--ss-ink-dim); }

  /* About */
  .ss-about-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; }
  @media (max-width: 600px) { .ss-about-grid { grid-template-columns: repeat(2, 1fr); } }
  .ss-about-label { font-size: 10.5px; font-weight: 600; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ss-ink-mute); }
  .ss-about-val { font-size: 16px; font-weight: 600; color: var(--ss-ink); margin-top: 3px; }
  .ss-link-list { list-style: none; padding: 0; margin: 0; }
  .ss-link-list li { padding: 5px 0; }

  /* Danger */
  .ss-danger-row { display: flex; align-items: center; justify-content: space-between; padding: 12px 0; border-top: 1px solid var(--ss-line); gap: 12px; }
  .ss-danger-row:first-of-type { border-top: none; }
  .ss-danger-name { font-size: 13.5px; font-weight: 500; color: var(--ss-ink); }

  /* Cmd-K palette */
  .ss-palette { position: fixed; inset: 0; z-index: 100; display: flex; align-items: flex-start; justify-content: center; padding-top: 12vh; }
  .ss-palette.hidden { display: none; }
  .ss-palette-scrim { position: absolute; inset: 0; background: rgba(0,0,0,0.55); backdrop-filter: blur(4px); }
  .ss-palette-inner { position: relative; width: 640px; max-width: calc(100vw - 32px); background: var(--ss-panel); border: 1px solid var(--ss-line-2); border-radius: 12px; box-shadow: 0 24px 48px rgba(0,0,0,0.6); overflow: hidden; }
  .ss-palette-search-row { display: flex; align-items: center; gap: 10px; padding: 14px 16px; border-bottom: 1px solid var(--ss-line); }
  .ss-palette-search-row svg { color: var(--ss-ink-mute); flex-shrink: 0; }
  .ss-palette-search-row input { flex: 1; background: transparent; border: none; outline: none; color: var(--ss-ink); font-size: 15px; }
  .ss-palette-search-row input::placeholder { color: var(--ss-ink-mute); }
  .ss-palette-search-row kbd { font-family: 'SF Mono', ui-monospace, monospace; font-size: 10.5px; background: var(--ss-panel-2); padding: 2px 6px; border-radius: 4px; color: var(--ss-ink-dim); border: 1px solid var(--ss-line-2); }
  .ss-palette-results { max-height: 340px; overflow-y: auto; padding: 4px; }
  .ss-palette-result { padding: 8px 12px; border-radius: 6px; cursor: pointer; display: flex; align-items: center; gap: 10px; }
  .ss-palette-result:hover, .ss-palette-result.focused { background: var(--ss-panel-2); }
  .ss-palette-result-main { flex: 1; min-width: 0; }
  .ss-palette-result-title { font-size: 13.5px; color: var(--ss-ink); font-weight: 500; }
  .ss-palette-result-section { font-size: 11px; color: var(--ss-ink-mute); margin-top: 1px; }
  .ss-palette-footer { display: flex; gap: 14px; padding: 8px 14px; border-top: 1px solid var(--ss-line); font-size: 10.5px; color: var(--ss-ink-mute); }
  .ss-palette-footer kbd { font-family: 'SF Mono', ui-monospace, monospace; font-size: 9.5px; background: var(--ss-panel-2); padding: 1px 5px; border-radius: 3px; color: var(--ss-ink-dim); border: 1px solid var(--ss-line-2); margin-right: 3px; }
  .ss-palette-empty { padding: 20px; text-align: center; color: var(--ss-ink-mute); font-size: 13px; }

  /* Dirty banner */
  .ss-dirty {
    position: fixed; left: 50%; transform: translateX(-50%);
    bottom: 20px; z-index: 80;
    background: var(--ss-panel); border: 1px solid var(--ss-line-2);
    border-radius: 10px; padding: 10px 14px;
    display: flex; align-items: center; gap: 10px;
    font-size: 13px; color: var(--ss-ink);
    box-shadow: 0 10px 32px rgba(0,0,0,0.4);
  }
  .ss-dirty.hidden { display: none; }
  .ss-dirty-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ss-warn); animation: ss-pulse 1.6s infinite; }
  @keyframes ss-pulse { 0%,100% { opacity: 1; } 50% { opacity: 0.5; } }
  .ss-btn-primary-sm { background: var(--ss-accent); color: #0a0d12; border: none; padding: 5px 12px; border-radius: 6px; font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .ss-btn-secondary-sm { background: transparent; color: var(--ss-ink-dim); border: 1px solid var(--ss-line-2); padding: 4px 11px; border-radius: 6px; font-size: 12.5px; cursor: pointer; }
  .ss-btn-secondary-sm:hover { color: var(--ss-ink); }

  /* Scroll-to highlight pulse */
  .ss-highlight-pulse { animation: ss-highlight 2s ease-out 1; }
  @keyframes ss-highlight {
    0% { box-shadow: 0 0 0 0 rgba(122,162,255,0); }
    15% { box-shadow: 0 0 0 4px rgba(122,162,255,0.35); }
    100% { box-shadow: 0 0 0 0 rgba(122,162,255,0); }
  }

  /* ── Scope chip ────────────────────────────────────────── */
  .ss-page-title-row { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .ss-scope {
    font-size: 10px; font-weight: 500; letter-spacing: 0.05em;
    padding: 2px 8px; border-radius: 999px;
    background: var(--ss-panel-2); color: var(--ss-ink-mute);
    border: 1px solid var(--ss-line-2);
    text-transform: lowercase;
  }
  .ss-scope-per-user { background: rgba(122,162,255,0.1); color: var(--ss-accent); border-color: rgba(122,162,255,0.25); }
  .ss-scope-server-wide { background: rgba(127,191,138,0.1); color: var(--ss-success); border-color: rgba(127,191,138,0.25); }
  .ss-scope-admin { background: rgba(217,122,122,0.1); color: var(--ss-danger); border-color: rgba(217,122,122,0.25); }

  /* ── Getting-started checklist ─────────────────────────── */
  .ss-gs-card { background: linear-gradient(135deg, rgba(122,162,255,0.06), var(--ss-panel)); border: 1px solid var(--ss-line-2); border-radius: 12px; padding: 18px 20px; margin-bottom: 16px; }
  .ss-gs-card.ss-gs-done { display: none; }
  .ss-gs-head { display: flex; align-items: center; justify-content: space-between; gap: 14px; margin-bottom: 12px; }
  .ss-gs-eyebrow { font-size: 10.5px; font-weight: 600; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ss-accent); }
  .ss-gs-title { font-size: 15px; font-weight: 600; color: var(--ss-ink); margin-top: 2px; }
  .ss-gs-progress { position: relative; display: flex; align-items: center; justify-content: center; }
  .ss-gs-ring-bg { stroke: var(--ss-line-2); }
  .ss-gs-ring-fill { stroke: var(--ss-accent); transition: stroke-dashoffset 0.4s ease-out; }
  .ss-gs-progress-text { position: absolute; font-size: 10.5px; font-weight: 600; color: var(--ss-ink); font-family: ui-monospace, monospace; }
  .ss-gs-list { list-style: none; padding: 0; margin: 0; }
  .ss-gs-list li { display: flex; align-items: center; gap: 10px; padding: 7px 0; font-size: 13px; border-top: 1px dashed var(--ss-line); }
  .ss-gs-list li:first-child { border-top: none; }
  .ss-gs-list li.done { color: var(--ss-ink-mute); text-decoration: line-through; }
  .ss-gs-check {
    width: 16px; height: 16px; border-radius: 50%;
    border: 1.5px solid var(--ss-line-2); flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    font-size: 9px; color: transparent;
  }
  .ss-gs-list li.done .ss-gs-check { border-color: var(--ss-success); background: var(--ss-success); color: #0a0d12; }
  .ss-gs-list li.done .ss-gs-check::after { content: '✓'; font-size: 10px; }
  .ss-gs-list li > span:first-child + * { flex: 1; }
  .ss-gs-go { color: var(--ss-accent); font-size: 11.5px; text-decoration: none; margin-left: auto; flex-shrink: 0; }
  .ss-gs-list li.done .ss-gs-go { display: none; }

  /* ── Toggle switch ─────────────────────────────────────── */
  .ss-switch { display: inline-block; position: relative; width: 36px; height: 20px; flex-shrink: 0; cursor: pointer; }
  .ss-switch input { opacity: 0; width: 0; height: 0; }
  .ss-switch-slider { position: absolute; inset: 0; background: var(--ss-line-2); border-radius: 999px; transition: background 0.12s; }
  .ss-switch-slider::before { content: ''; position: absolute; width: 14px; height: 14px; left: 3px; top: 3px; background: var(--ss-ink-dim); border-radius: 50%; transition: transform 0.18s, background 0.12s; }
  .ss-switch input:checked + .ss-switch-slider { background: var(--ss-accent); }
  .ss-switch input:checked + .ss-switch-slider::before { transform: translateX(16px); background: #0a0d12; }
  .ss-toggle-row { align-items: flex-start; }
  .ss-toggle-row > div { flex: 1; min-width: 0; }

  /* ── Modal (type-confirm) ──────────────────────────────── */
  .ss-modal { position: fixed; inset: 0; z-index: 90; display: flex; align-items: center; justify-content: center; padding: 24px; }
  .ss-modal.hidden { display: none; }
  .ss-modal-scrim { position: absolute; inset: 0; background: rgba(0,0,0,0.55); backdrop-filter: blur(4px); }
  .ss-modal-inner { position: relative; background: var(--ss-panel); border: 1px solid var(--ss-line-2); border-radius: 12px; padding: 22px 24px; width: 100%; max-width: 440px; box-shadow: 0 24px 48px rgba(0,0,0,0.6); }
  .ss-modal-title { font-size: 16px; font-weight: 600; color: var(--ss-ink); margin-bottom: 6px; }
  .ss-modal-body { font-size: 13.5px; color: var(--ss-ink-dim); margin-bottom: 14px; line-height: 1.5; }

  /* ── Restart banner ────────────────────────────────────── */
  .ss-restart {
    position: fixed; left: 50%; transform: translateX(-50%);
    top: 60px; z-index: 70;
    background: rgba(240,180,112,0.12); color: var(--ss-warn);
    border: 1px solid rgba(240,180,112,0.35); border-radius: 10px;
    padding: 8px 14px; display: flex; align-items: center; gap: 10px;
    font-size: 12.5px;
  }
  .ss-restart.hidden { display: none; }
  .ss-restart button { background: var(--ss-warn); color: #0a0d12; border: none; padding: 4px 10px; border-radius: 6px; font-size: 12px; font-weight: 500; cursor: pointer; }

  .hidden { display: none !important; }

  /* ── Integration status pill ──────────────────────────── */
  .ss-pill {
    display: inline-flex; align-items: center; gap: 5px;
    padding: 3px 10px;
    font-size: 10.5px; font-weight: 500; letter-spacing: 0.03em;
    text-transform: uppercase;
    border-radius: 999px;
  }
  .ss-pill::before {
    content: ''; width: 6px; height: 6px; border-radius: 50%;
    display: inline-block; flex-shrink: 0;
  }
  .ss-pill-unknown    { background: var(--ss-panel-2); color: var(--ss-ink-mute); border: 1px solid var(--ss-line-2); }
  .ss-pill-unknown::before    { background: var(--ss-ink-mute); }
  .ss-pill-connected  { background: rgba(127,191,138,0.14); color: var(--ss-success); border: 1px solid rgba(127,191,138,0.35); }
  .ss-pill-connected::before  { background: var(--ss-success); }
  .ss-pill-degraded   { background: rgba(240,180,112,0.14); color: var(--ss-warn);  border: 1px solid rgba(240,180,112,0.35); }
  .ss-pill-degraded::before   { background: var(--ss-warn); }
  .ss-pill-error      { background: rgba(217,122,122,0.14); color: var(--ss-danger); border: 1px solid rgba(217,122,122,0.35); }
  .ss-pill-error::before      { background: var(--ss-danger); }
  .ss-pill-not_configured { background: var(--ss-panel-2); color: var(--ss-ink-mute); border: 1px solid var(--ss-line-2); }
  .ss-pill-not_configured::before { background: var(--ss-ink-faint); }

  .ss-card-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 10px; }

  /* ── Set-up stepped flow ─────────────────────────────── */
  .ss-ready {
    display: flex; align-items: center; gap: 14px;
    background: linear-gradient(135deg, rgba(127,191,138,0.15), rgba(127,191,138,0.06));
    border: 1px solid rgba(127,191,138,0.35);
    border-radius: 12px; padding: 16px 20px; margin-bottom: 16px;
  }
  .ss-ready-check {
    width: 34px; height: 34px; border-radius: 50%;
    background: var(--ss-success); color: #0a0d12;
    display: flex; align-items: center; justify-content: center;
    font-weight: 700; font-size: 18px; flex-shrink: 0;
  }
  .ss-ready-body { flex: 1; }
  .ss-ready-title { font-size: 15px; font-weight: 600; color: var(--ss-ink); }
  .ss-ready-sub { font-size: 12.5px; color: var(--ss-ink-dim); margin-top: 2px; }

  .ss-setup-hero { margin-bottom: 16px; }
  .ss-setup-eyebrow { font-size: 10.5px; font-weight: 600; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ss-accent); }
  .ss-setup-title { font-size: 19px; font-weight: 600; color: var(--ss-ink); margin-top: 3px; }
  .ss-setup-sub { font-size: 13px; color: var(--ss-ink-dim); margin-top: 4px; line-height: 1.5; max-width: 620px; }

  .ss-step {
    display: flex; gap: 14px;
    background: var(--ss-panel); border: 1px solid var(--ss-line);
    border-radius: 12px; padding: 16px 18px;
    margin-bottom: 12px;
    transition: border-color 0.15s;
  }
  .ss-step.ss-step-done { border-color: rgba(127,191,138,0.3); background: linear-gradient(135deg, rgba(127,191,138,0.05), var(--ss-panel)); }
  .ss-step-num {
    width: 28px; height: 28px; border-radius: 50%;
    background: var(--ss-panel-2); border: 1px solid var(--ss-line-2);
    color: var(--ss-ink-dim);
    display: flex; align-items: center; justify-content: center;
    font-size: 13px; font-weight: 600; flex-shrink: 0;
    margin-top: 2px;
  }
  .ss-step.ss-step-done .ss-step-num { background: var(--ss-success); color: #0a0d12; border-color: var(--ss-success); }
  .ss-step.ss-step-done .ss-step-num::before { content: '✓'; }
  .ss-step.ss-step-done .ss-step-num > * { display: none; }
  .ss-step-body { flex: 1; min-width: 0; }
  .ss-step-head { display: flex; align-items: center; gap: 10px; margin-bottom: 2px; }
  .ss-step-title { font-size: 14.5px; font-weight: 600; color: var(--ss-ink); }
  .ss-step-optional { font-size: 10.5px; padding: 1px 8px; border-radius: 999px; background: var(--ss-panel-2); color: var(--ss-ink-mute); border: 1px solid var(--ss-line-2); }
  .ss-step-desc { font-size: 12.5px; color: var(--ss-ink-dim); line-height: 1.5; margin-bottom: 10px; }
  .ss-step-actions { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; }
  .ss-step-help { font-size: 12px; color: var(--ss-ink-dim); text-decoration: none; cursor: pointer; }
  .ss-step-help:hover { color: var(--ss-accent); }

  /* Done-state row inside a step (JS fills this in). */
  .ss-step-done-row {
    display: flex; align-items: center; gap: 10px;
    background: rgba(127,191,138,0.08); border: 1px solid rgba(127,191,138,0.25);
    border-radius: 8px; padding: 8px 12px;
    font-size: 13px;
  }
  .ss-step-done-icon { color: var(--ss-success); font-weight: 700; }
  .ss-step-done-text { flex: 1; color: var(--ss-ink); }
  .ss-step-done-change { color: var(--ss-ink-dim); font-size: 12px; text-decoration: none; }
  .ss-step-done-change:hover { color: var(--ss-accent); }

  /* Step 3 "where" rows */
  .ss-where-row {
    display: flex; align-items: center; gap: 10px;
    padding: 9px 12px; border-radius: 8px;
    background: var(--ss-panel-2); border: 1px solid var(--ss-line);
    font-size: 13px; margin-bottom: 6px;
  }
  .ss-where-row:last-child { margin-bottom: 0; }
  .ss-where-check {
    width: 20px; height: 20px; border-radius: 50%;
    display: flex; align-items: center; justify-content: center;
    font-size: 12px; flex-shrink: 0;
    color: var(--ss-ink-mute); border: 1px solid var(--ss-line-2);
  }
  .ss-where-check.ss-where-done { background: var(--ss-success); color: #0a0d12; border-color: var(--ss-success); font-weight: 700; }
  .ss-where-label { flex: 1; color: var(--ss-ink); }
  .ss-where-note { font-size: 11.5px; color: var(--ss-ink-mute); }
  .ss-where-action { font-size: 12px; color: var(--ss-accent); text-decoration: none; padding: 4px 10px; border-radius: 6px; border: 1px solid var(--ss-line-2); background: var(--ss-panel); }
  .ss-where-action:hover { border-color: var(--ss-accent); }

  /* Unified Connect things page */
  .ss-conn-section { margin-top: 14px; }
  .ss-conn-section:first-child { margin-top: 0; }
  .ss-conn-title {
    font-size: 11px; font-weight: 600; letter-spacing: 0.09em;
    text-transform: uppercase; color: var(--ss-ink-mute);
    padding: 0 2px 8px; margin-bottom: 4px;
    border-bottom: 1px solid var(--ss-line);
  }
  .ss-conn-list { display: flex; flex-direction: column; gap: 4px; }
  .ss-conn-row {
    display: flex; align-items: center; gap: 12px;
    padding: 10px 12px; border-radius: 8px;
    text-decoration: none; color: var(--ss-ink);
    background: var(--ss-panel);
    border: 1px solid var(--ss-line);
    transition: border-color 0.12s, background 0.12s;
  }
  .ss-conn-row:hover { border-color: var(--ss-accent); background: var(--ss-panel-2); }
  .ss-conn-ico { font-size: 18px; flex-shrink: 0; width: 24px; text-align: center; }
  .ss-conn-main { flex: 1; min-width: 0; }
  .ss-conn-name { font-size: 13.5px; font-weight: 500; color: var(--ss-ink); }
  .ss-conn-desc { font-size: 11.5px; color: var(--ss-ink-mute); margin-top: 1px; }
  .ss-conn-status {
    font-size: 11px; font-weight: 500;
    padding: 2px 9px; border-radius: 999px;
    background: var(--ss-panel-2); color: var(--ss-ink-mute);
    border: 1px solid var(--ss-line-2);
    white-space: nowrap;
  }
  .ss-conn-status.ss-conn-connected {
    background: rgba(127,191,138,0.12);
    color: var(--ss-success);
    border-color: rgba(127,191,138,0.3);
  }
  .ss-conn-status.ss-conn-error {
    background: rgba(217,122,122,0.12);
    color: var(--ss-danger);
    border-color: rgba(217,122,122,0.3);
  }
  .ss-conn-action { font-size: 11.5px; color: var(--ss-ink-mute); white-space: nowrap; }
  .ss-conn-row:hover .ss-conn-action { color: var(--ss-accent); }

  /* Cross-link bubble that surfaces the invisible coupling between
     the Helpers page and the Brain page. Shows the "other side" of the
     relationship so users see what changes when they change a model. */
  .ss-agent-brain-link {
    display: flex; align-items: center; gap: 8px;
    padding: 10px 14px; margin-bottom: 14px;
    background: rgba(122,162,255,0.06);
    border: 1px solid rgba(122,162,255,0.2);
    border-radius: 8px;
    font-size: 12.5px; color: var(--ss-ink-dim);
    flex-wrap: wrap;
  }
  .ss-agent-brain-label {}
  .ss-agent-brain-name { color: var(--ss-ink); font-weight: 500; }
  .ss-agent-brain-change {
    margin-left: auto;
    font-size: 12px; font-weight: 500;
    color: var(--ss-accent); text-decoration: none;
    padding: 3px 9px; border-radius: 6px;
    border: 1px solid var(--ss-line-2);
    background: var(--ss-panel);
  }
  .ss-agent-brain-change:hover { border-color: var(--ss-accent); }

  /* Plain-language tooltip — for technical terms we can't fully
     avoid. Usage: <span class="ss-tt" data-tip="The graphics card…">GPU</span>
     Tip: also works with keyboard focus for accessibility. */
  .ss-tt {
    position: relative;
    border-bottom: 1px dotted var(--ss-ink-mute);
    cursor: help;
  }
  .ss-tt::before {
    content: '?';
    display: inline-flex;
    align-items: center; justify-content: center;
    width: 13px; height: 13px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--ss-panel-2);
    color: var(--ss-ink-mute);
    font-size: 9px; font-weight: 700;
    vertical-align: text-top;
    transition: background 0.12s, color 0.12s;
  }
  .ss-tt:hover::before, .ss-tt:focus::before {
    background: var(--ss-accent); color: #0a0d12;
  }
  .ss-tt::after {
    content: attr(data-tip);
    position: absolute;
    bottom: calc(100% + 8px); left: 50%;
    transform: translateX(-50%);
    width: 240px; max-width: 80vw;
    padding: 8px 12px;
    background: var(--ss-panel);
    border: 1px solid var(--ss-line-2);
    border-radius: 6px;
    box-shadow: 0 6px 14px rgba(0,0,0,0.4);
    font-size: 11.5px;
    color: var(--ss-ink);
    line-height: 1.5;
    font-weight: normal; letter-spacing: 0;
    text-align: left;
    white-space: normal;
    opacity: 0; pointer-events: none;
    transition: opacity 0.15s;
    z-index: 40;
  }
  .ss-tt:hover::after, .ss-tt:focus::after { opacity: 1; }

  /* Global toast stack for save-success / error / undo */
  .ss-toast-stack {
    position: fixed; bottom: 80px; right: 20px; z-index: 90;
    display: flex; flex-direction: column; gap: 8px; align-items: flex-end;
    pointer-events: none;
  }
  .ss-toast {
    pointer-events: auto;
    display: flex; align-items: center; gap: 10px;
    min-width: 220px; max-width: 360px;
    padding: 9px 14px;
    background: var(--ss-panel); border: 1px solid var(--ss-line-2);
    border-radius: 8px;
    font-size: 13px; color: var(--ss-ink);
    box-shadow: 0 8px 24px rgba(0,0,0,0.35);
    animation: ss-toast-in 0.22s ease-out;
  }
  @keyframes ss-toast-in {
    from { opacity: 0; transform: translateX(10px); }
    to   { opacity: 1; transform: translateX(0); }
  }
  .ss-toast.ss-toast-leaving { animation: ss-toast-out 0.22s ease-in forwards; }
  @keyframes ss-toast-out {
    from { opacity: 1; transform: translateX(0); }
    to   { opacity: 0; transform: translateX(10px); }
  }
  .ss-toast-icon { font-weight: 700; flex-shrink: 0; }
  .ss-toast.ss-toast-success { border-color: rgba(127,191,138,0.4); }
  .ss-toast.ss-toast-success .ss-toast-icon { color: var(--ss-success); }
  .ss-toast.ss-toast-error { border-color: rgba(217,122,122,0.4); }
  .ss-toast.ss-toast-error .ss-toast-icon { color: var(--ss-danger); }
  .ss-toast.ss-toast-info .ss-toast-icon { color: var(--ss-accent); }
  .ss-toast-body { flex: 1; line-height: 1.4; }
  .ss-toast-body .ss-toast-fix { display: block; margin-top: 2px; font-size: 11.5px; color: var(--ss-ink-mute); }
  .ss-toast-undo {
    background: transparent; color: var(--ss-accent); border: none;
    padding: 2px 6px; font-size: 12px; font-weight: 500; cursor: pointer;
    border-radius: 4px;
  }
  .ss-toast-undo:hover { background: rgba(122,162,255,0.1); }
  .ss-toast-close {
    background: transparent; color: var(--ss-ink-mute); border: none;
    padding: 0 2px; font-size: 14px; cursor: pointer; line-height: 1;
  }
  .ss-toast-close:hover { color: var(--ss-ink); }

  /* Inline field "saved" pill — anchored next to a label */
  .ss-field-saved {
    display: inline-flex; align-items: center; gap: 4px;
    margin-left: 8px; font-size: 11px;
    color: var(--ss-success); font-weight: 500;
    animation: ss-fade-out 1.8s ease-out 0.2s forwards;
  }
  @keyframes ss-fade-out {
    0%, 60% { opacity: 1; }
    100% { opacity: 0; }
  }

  /* Sticky "Am I done?" banner (shows on every settings page except Setup) */
  .ss-setup-status {
    position: sticky; top: 53px; z-index: 30;
    display: flex; align-items: center; gap: 10px;
    padding: 9px 16px; margin: 0 0 16px 0;
    border-radius: 8px;
    font-size: 12.5px;
    border: 1px solid transparent;
  }
  .ss-setup-status.ss-status-done {
    background: rgba(127,191,138,0.08);
    border-color: rgba(127,191,138,0.3);
    color: var(--ss-ink);
  }
  .ss-setup-status.ss-status-todo {
    background: rgba(240,180,112,0.08);
    border-color: rgba(240,180,112,0.3);
    color: var(--ss-ink);
  }
  .ss-setup-status-icon { font-weight: 700; font-size: 14px; flex-shrink: 0; }
  .ss-setup-status.ss-status-done .ss-setup-status-icon { color: var(--ss-success); }
  .ss-setup-status.ss-status-todo .ss-setup-status-icon { color: var(--ss-warn); }
  .ss-setup-status-text { flex: 1; }
  .ss-setup-status-cta {
    font-size: 12px; font-weight: 500;
    padding: 4px 11px; border-radius: 6px;
    text-decoration: none;
    background: var(--ss-panel); border: 1px solid var(--ss-line-2);
    color: var(--ss-ink);
  }
  .ss-setup-status-cta:hover { border-color: var(--ss-accent); color: var(--ss-accent); }

  /* Step 4 "things" tiles */
  .ss-things-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
  @media (max-width: 600px) { .ss-things-grid { grid-template-columns: repeat(2, 1fr); } }
  .ss-things-tile {
    display: block; padding: 12px 10px; border-radius: 8px;
    background: var(--ss-panel-2); border: 1px solid var(--ss-line);
    text-decoration: none; color: var(--ss-ink); text-align: center;
    transition: border-color 0.12s;
  }
  .ss-things-tile:hover { border-color: var(--ss-accent); }
  .ss-things-ico { font-size: 20px; margin-bottom: 4px; }
  .ss-things-label { font-size: 12.5px; font-weight: 600; }
  .ss-things-sub { font-size: 10.5px; color: var(--ss-ink-mute); margin-top: 2px; }
"##;

// Fresh JS used by the new shell. Legacy JS from `page.js` handles all the
// existing tab content (LLM CRUD, sync connect, invite dialog, etc.).
const NEW_JS: &str = r##"
// Settings index for the ⌘K palette. Generated server-side.
window.SS_INDEX = %%SS_INDEX%%;

// Phase 1.1: shared auth helpers. Every /api fetch goes through these so
// the session token travels in Authorization: Bearer, never in a URL or
// JSON body. Server middleware `security::lift_bearer_to_body_and_query`
// copies the header back into query+body for handlers that still read
// those positions.
function _ssTok() { return sessionStorage.getItem('syntaur_token') || ''; }
function ssAuthH() { return { 'Authorization': 'Bearer ' + _ssTok() }; }
function ssJsonAuthH() { return { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + _ssTok() }; }


// Old section/page slugs (pre-2026-04-18 restructure). Any hash that
// matches a key here is silently rewritten to its new destination so
// bookmarks, checklist links, and shared URLs keep working.
const SS_HASH_REDIRECT = {
  'home/home':                 'setup/start',
  'home/':                     'setup/start',
  'agents/all':                'helpers/all',
  'agents/personas':           'helpers/personas',
  'integrations/telegram':     'connect/telegram',
  'integrations/homeassistant':'connect/homeassistant',
  'integrations/sync':         'connect/sync',
  'integrations/media':        'connect/media',
  'llm/providers':             'advanced/brain',
  'voice/satellites':          'advanced/voice',
  'modules/installed':         'advanced/modules',
  'privacy/data':              'account/privacy',
  'system/gateway':            'advanced/gateway',
  'system/danger':             'advanced/danger',
  'about/info':                'advanced/about',
};

function ssParseHash() {
  const raw = (window.location.hash || '').replace(/^#/, '').trim();
  if (!raw) return { section: 'setup', page: 'start' };
  // Apply legacy redirect before splitting.
  const redirected = SS_HASH_REDIRECT[raw] || raw;
  if (redirected !== raw) {
    // Rewrite the URL so the redirect is sticky across reloads.
    try { window.history.replaceState(null, '', '#' + redirected); } catch(e) {}
  }
  const [s, p] = redirected.split('/');
  return { section: s || 'setup', page: p || 'start' };
}

function ssApplyRoute() {
  const { section, page } = ssParseHash();
  // Highlight nav row
  document.querySelectorAll('.ss-nav-item').forEach(el => el.classList.remove('active'));
  const nav = document.querySelector(`.ss-nav-item[data-section="${section}"][data-page="${page}"]`);
  if (nav) nav.classList.add('active');

  // Show only the matching page
  document.querySelectorAll('.ss-page').forEach(el => el.classList.remove('active'));
  let target = document.getElementById(`ss-page-${section}-${page}`);
  if (!target) {
    // Fallback to the first page in the section, then to the setup landing.
    target = document.querySelector(`.ss-page[data-section="${section}"]`);
    if (!target) target = document.getElementById('ss-page-setup-start');
  }
  if (target) {
    target.classList.add('active');
    target.classList.add('ss-highlight-pulse');
    setTimeout(() => target.classList.remove('ss-highlight-pulse'), 2000);
    // Update breadcrumb
    const crumb = document.getElementById('ss-current-crumb');
    if (crumb) {
      const h1 = target.querySelector('.ss-page-title');
      crumb.textContent = h1 ? h1.textContent : '';
    }
    // Fire legacy showTab so existing JS still initializes the right tab.
    // New slugs map to the same legacy tab ids the existing JS expects.
    const LEGACY = {
      'connect/telegram':  'general',
      'connect/sync':      'sync',
      'connect/media':     'media',
      'advanced/brain':    'llm',
      'advanced/gateway':  'system',
      'account/users':     'users',
    };
    const key = `${section}/${page}`;
    const legacyTab = LEGACY[key];
    if (legacyTab && typeof showTab === 'function') {
      try { showTab(legacyTab); } catch(e) {}
    }
  }
}

function ssNavigate(section, page) {
  window.location.hash = `${section}/${page}`;
  ssApplyRoute();
  // Scroll to top on navigation
  const main = document.getElementById('ss-main');
  if (main) main.scrollTop = 0;
  window.scrollTo({ top: 0, behavior: 'smooth' });
}

// Live-filter the sidebar by typed query.
function ssFilterSidebar(q) {
  const needle = q.toLowerCase().trim();
  document.querySelectorAll('.ss-sec').forEach(sec => {
    let anyMatch = false;
    sec.querySelectorAll('.ss-nav-item').forEach(item => {
      const label = item.querySelector('.ss-nav-label')?.textContent.toLowerCase() || '';
      const sectionTitle = sec.querySelector('.ss-sec-title')?.textContent.toLowerCase() || '';
      const match = !needle || label.includes(needle) || sectionTitle.includes(needle);
      item.classList.toggle('ss-hidden', !match);
      if (match) anyMatch = true;
    });
    sec.classList.toggle('ss-hidden', !anyMatch);
  });
}

// ── ⌘K palette ─────────────────────────────────────────────
let ssPaletteFocusIdx = 0;
let ssPaletteFiltered = [];

function ssOpenPalette() {
  const el = document.getElementById('ss-palette');
  if (!el) return;
  el.classList.remove('hidden');
  document.getElementById('ss-palette-input').value = '';
  ssPaletteSearch('');
  setTimeout(() => document.getElementById('ss-palette-input').focus(), 30);
}
function ssClosePalette() {
  const el = document.getElementById('ss-palette');
  if (el) el.classList.add('hidden');
}
function ssPaletteSearch(q) {
  const needle = (q || '').toLowerCase().trim();
  const all = window.SS_INDEX || [];
  ssPaletteFiltered = !needle ? all : all.filter(x => {
    const hay = `${x.title} ${x.section_title} ${x.description} ${x.keywords}`.toLowerCase();
    return hay.includes(needle);
  });
  ssPaletteFocusIdx = 0;
  ssPaletteRender();
}
function ssPaletteRender() {
  const box = document.getElementById('ss-palette-results');
  if (!box) return;
  if (!ssPaletteFiltered.length) {
    box.innerHTML = `<div class="ss-palette-empty">No settings matched. Try another keyword.</div>`;
    return;
  }
  box.innerHTML = ssPaletteFiltered.map((x, i) => `
    <div class="ss-palette-result ${i === ssPaletteFocusIdx ? 'focused' : ''}"
         onmouseenter="ssPaletteFocusIdx=${i};ssPaletteRender()"
         onclick="ssPaletteJump(${i})">
      <div class="ss-palette-result-main">
        <div class="ss-palette-result-title">${x.title}</div>
        <div class="ss-palette-result-section">${x.section_title} · ${x.description}</div>
      </div>
      ${x.badge ? `<span class="ss-nav-badge">${x.badge}</span>` : ''}
    </div>
  `).join('');
}
function ssPaletteJump(i) {
  const x = ssPaletteFiltered[i];
  if (!x) return;
  ssClosePalette();
  ssNavigate(x.section, x.page);
}
function ssPaletteKey(ev) {
  if (ev.key === 'Escape') { ssClosePalette(); return; }
  if (ev.key === 'ArrowDown') { ev.preventDefault(); ssPaletteFocusIdx = Math.min(ssPaletteFocusIdx + 1, ssPaletteFiltered.length - 1); ssPaletteRender(); }
  if (ev.key === 'ArrowUp')   { ev.preventDefault(); ssPaletteFocusIdx = Math.max(ssPaletteFocusIdx - 1, 0); ssPaletteRender(); }
  if (ev.key === 'Enter')     { ev.preventDefault(); ssPaletteJump(ssPaletteFocusIdx); }
}

// Global shortcut: Cmd/Ctrl + K opens palette anywhere on the page.
document.addEventListener('keydown', (ev) => {
  if ((ev.metaKey || ev.ctrlKey) && ev.key.toLowerCase() === 'k') {
    ev.preventDefault();
    ssOpenPalette();
  }
});

// ── Dirty state detection ─────────────────────────────────
let ssOriginalFormState = null;
let ssDirty = false;
function ssSnapshotForms() {
  const snap = {};
  document.querySelectorAll('input, textarea, select').forEach(el => {
    if (!el.id) return;
    if (el.type === 'checkbox' || el.type === 'radio') snap[el.id] = el.checked;
    else snap[el.id] = el.value;
  });
  ssOriginalFormState = snap;
}
function ssCheckDirty() {
  if (!ssOriginalFormState) return false;
  for (const [id, original] of Object.entries(ssOriginalFormState)) {
    const el = document.getElementById(id);
    if (!el) continue;
    const cur = (el.type === 'checkbox' || el.type === 'radio') ? el.checked : el.value;
    if (cur !== original) return true;
  }
  return false;
}
function ssSetDirty(flag) {
  ssDirty = flag;
  const banner = document.getElementById('ss-dirty');
  if (banner) banner.classList.toggle('hidden', !flag);
}
function ssRevertDirty() {
  if (!ssOriginalFormState) return;
  for (const [id, original] of Object.entries(ssOriginalFormState)) {
    const el = document.getElementById(id);
    if (!el) continue;
    if (el.type === 'checkbox' || el.type === 'radio') el.checked = original;
    else el.value = original;
  }
  ssSetDirty(false);
}
function ssSaveDirty() {
  // Legacy JS owns the actual save wiring — this button just hides the banner.
  // Forms submit themselves on blur / explicit save button.
  ssSetDirty(false);
  ssSnapshotForms();
  // Toast
  const banner = document.getElementById('ss-dirty');
  if (banner) {
    const text = banner.querySelector('.ss-dirty-text');
    if (text) text.textContent = 'Saved ✓';
    banner.classList.remove('hidden');
    setTimeout(() => ssSetDirty(false), 1400);
    if (text) setTimeout(() => { text.textContent = 'You have unsaved changes'; }, 1400);
  }
}
document.addEventListener('input', (ev) => {
  if (ev.target.matches('.ss-page input, .ss-page select, .ss-page textarea')) {
    ssSetDirty(ssCheckDirty());
  }
});
window.addEventListener('beforeunload', (ev) => {
  if (ssDirty) { ev.preventDefault(); ev.returnValue = ''; }
});

// Placeholder handlers wired to the new cards.
function ssSaveAccountProfile() {
  ssSaveDirty();  // minimal wiring; real save lives in the legacy profile path
  window.location.href = '/profile';
}
function ssChangePassword() { alert('Password change flows through the existing account API. Wiring coming.'); }
function ssSignOutOthers()  { alert('Sign-out-other-sessions flow — Phase E.'); }
function ssHaTest() { const r = document.getElementById('ss-ha-result'); if (r) r.textContent = 'Testing… (endpoint wiring coming)'; }
function ssHaSave() { const r = document.getElementById('ss-ha-result'); if (r) r.textContent = 'Save — wiring coming.'; }

// ── Setup flow status ──────────────────────────────────────
// Fetches /health + /api/setup/status, updates each step card to show
// either its action buttons or a done-state row. Lights up the "Syntaur
// is ready" banner on the Setup landing, and the sticky "am I done?"
// banner on every other page.
async function ssRefreshSetup() {
  const hero = document.getElementById('ss-setup-hero');
  const ready = document.getElementById('ss-ready-banner');
  const stickyEl = document.getElementById('ss-setup-status');

  const state = { brain: null, helper: null, phone: false, voice: false };

  try {
    const hr = await fetch('/health');
    if (hr.ok) {
      const h = await hr.json();
      const providers = h.providers || [];
      if (providers.length > 0) {
        state.brain = providers[0].name || 'a model provider';
      }
      const agents = h.agents || [];
      if (agents.length > 0) {
        const a = agents[0];
        state.helper = (typeof a === 'object' ? (a.name || a.id) : a) || 'Peter';
      }
      state.phone = !!(h.telegram_configured || h.telegram || (h.features || []).includes('telegram'));
      state.voice = !!(h.voice_configured || (h.features || []).includes('voice'));
    }
  } catch(e) {}

  // Paint the sticky "am I done?" banner — hidden on the setup page,
  // green on every other page when both required steps are satisfied,
  // amber with a CTA when not.
  if (stickyEl) {
    const onSetup = (ssParseHash().section === 'setup');
    if (onSetup) {
      stickyEl.style.display = 'none';
    } else {
      stickyEl.style.display = 'flex';
      const icon = document.getElementById('ss-setup-status-icon');
      const text = document.getElementById('ss-setup-status-text');
      const cta  = document.getElementById('ss-setup-status-cta');
      const missing = [];
      if (!state.brain)  missing.push('pick a brain');
      if (!state.helper) missing.push('pick a main helper');
      if (missing.length === 0) {
        stickyEl.className = 'ss-setup-status ss-status-done';
        if (icon) icon.textContent = '✓';
        if (text) text.textContent = 'Syntaur is fully set up.';
        if (cta) { cta.textContent = 'What else can I do? →'; }
      } else {
        stickyEl.className = 'ss-setup-status ss-status-todo';
        if (icon) icon.textContent = '⚠';
        if (text) {
          text.textContent = missing.length === 1
            ? `1 thing left: ${missing[0]}.`
            : `${missing.length} things left: ${missing.join(' and ')}.`;
        }
        if (cta) cta.textContent = 'Finish setup →';
      }
    }
  }

  // Cross-link labels — Helpers page shows current brain, Brain page
  // shows count of helpers. These appear on those pages regardless of
  // whether the user has completed setup.
  const brainLabel = document.getElementById('ss-agent-brain-name');
  if (brainLabel) {
    brainLabel.textContent = state.brain ? state.brain : 'not set yet';
  }
  const helperList = document.getElementById('ss-brain-agent-list');
  if (helperList) {
    try {
      const hr = await fetch('/health');
      if (hr.ok) {
        const h = await hr.json();
        const names = (h.agents || []).map(a => (typeof a === 'object' ? (a.name || a.id) : a)).filter(Boolean);
        if (names.length === 0) {
          helperList.textContent = 'your helpers';
        } else if (names.length === 1) {
          helperList.textContent = names[0];
        } else if (names.length <= 3) {
          helperList.textContent = names.join(', ');
        } else {
          helperList.textContent = `${names.slice(0, 2).join(', ')}, and ${names.length - 2} more`;
        }
      }
    } catch(e) {}
  }

  if (!hero) return;  // not on the setup page — step painting below is skipped

  // Paint step 1 (brain)
  const s1 = document.getElementById('ss-step-brain');
  const s1state = document.getElementById('ss-step-brain-state');
  if (s1 && s1state) {
    if (state.brain) {
      s1.classList.add('ss-step-done');
      s1state.innerHTML = `
        <div class="ss-step-done-row">
          <span class="ss-step-done-icon">✓</span>
          <span class="ss-step-done-text">Using <strong>${esc(state.brain)}</strong></span>
          <a class="ss-step-done-change" href="#advanced/brain" onclick="ssNavigate('advanced','brain');return false;">Change →</a>
        </div>`;
    } else {
      s1.classList.remove('ss-step-done');
      // Keep the server-rendered action buttons as-is on first paint.
    }
  }

  // Paint step 2 (helper)
  const s2 = document.getElementById('ss-step-helper');
  const s2state = document.getElementById('ss-step-helper-state');
  if (s2 && s2state) {
    if (state.helper) {
      s2.classList.add('ss-step-done');
      s2state.innerHTML = `
        <div class="ss-step-done-row">
          <span class="ss-step-done-icon">✓</span>
          <span class="ss-step-done-text">You'll be talking to <strong>${esc(state.helper)}</strong></span>
          <a class="ss-step-done-change" href="#helpers/all" onclick="ssNavigate('helpers','all');return false;">Manage helpers →</a>
        </div>`;
    } else {
      s2.classList.remove('ss-step-done');
    }
  }

  // Paint step 3 (where — phone + voice rows)
  const phoneRow = document.getElementById('ss-where-phone');
  if (phoneRow) {
    const check = phoneRow.querySelector('.ss-where-check');
    if (state.phone) {
      check.classList.add('ss-where-done');
      check.textContent = '✓';
    } else {
      check.classList.remove('ss-where-done');
      check.textContent = '○';
    }
  }
  const voiceRow = document.getElementById('ss-where-voice');
  if (voiceRow) {
    const check = voiceRow.querySelector('.ss-where-check');
    if (state.voice) {
      check.classList.add('ss-where-done');
      check.textContent = '✓';
    } else {
      check.classList.remove('ss-where-done');
      check.textContent = '○';
    }
  }

  // Ready banner — only show when both required steps are done.
  if (ready) {
    if (state.brain && state.helper) {
      ready.style.display = 'flex';
      if (hero) hero.style.display = 'none';
    } else {
      ready.style.display = 'none';
      if (hero) hero.style.display = '';
    }
  }
}

// "Set up a brain" button — jumps to Advanced → Brain settings with a
// focus hint so the user lands directly in the provider form.
function ssSetupGoBrain() {
  ssNavigate('advanced', 'brain');
  // Scroll the first configurable provider card into view after route applies.
  setTimeout(() => {
    const el = document.querySelector('#ss-page-advanced-brain .ss-card');
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, 120);
}

// ── Global toast / save feedback helpers ───────────────────
// ssToast(message, kind, opts)  — bottom-right pill, auto-dismiss.
//   kind:    'success' (default) | 'error' | 'info'
//   opts:    { fix: "plain-english what to do",   // shown under message
//              undo: fn,                          // shows Undo button (10s)
//              ttl: ms }                          // override auto-dismiss
// ssFieldSaved(id)  — tiny green "Saved" pill next to a form field's label,
//   fades out after ~2s. Use for any field with an id whose ancestor
//   .ss-field carries a <label>.
// ssDescribeError(err) — turns a fetch/caught exception into a plain-
//   English sentence. Use before passing to ssToast so users don't see
//   "TypeError: Failed to fetch".
function ssToast(message, kind, opts) {
  kind = kind || 'success';
  opts = opts || {};
  const stack = document.getElementById('ss-toasts');
  if (!stack) { if (kind === 'error') console.error(message); return; }
  const el = document.createElement('div');
  el.className = 'ss-toast ss-toast-' + kind;
  const icon = kind === 'error' ? '!' : (kind === 'info' ? 'i' : '✓');
  const body = document.createElement('div');
  body.className = 'ss-toast-body';
  body.textContent = message;
  if (opts.fix) {
    const fix = document.createElement('span');
    fix.className = 'ss-toast-fix';
    fix.textContent = opts.fix;
    body.appendChild(fix);
  }
  const iconEl = document.createElement('span');
  iconEl.className = 'ss-toast-icon';
  iconEl.textContent = icon;
  el.appendChild(iconEl);
  el.appendChild(body);

  let dismissTimer = null;
  const dismiss = () => {
    if (!el.parentNode) return;
    el.classList.add('ss-toast-leaving');
    setTimeout(() => el.remove(), 220);
  };

  if (opts.undo) {
    const undoBtn = document.createElement('button');
    undoBtn.className = 'ss-toast-undo';
    undoBtn.textContent = 'Undo';
    undoBtn.onclick = () => { try { opts.undo(); } catch(e) {} dismiss(); };
    el.appendChild(undoBtn);
  }
  const closeBtn = document.createElement('button');
  closeBtn.className = 'ss-toast-close';
  closeBtn.setAttribute('aria-label', 'Dismiss');
  closeBtn.textContent = '✕';
  closeBtn.onclick = () => { if (dismissTimer) clearTimeout(dismissTimer); dismiss(); };
  el.appendChild(closeBtn);

  stack.appendChild(el);
  const ttl = opts.ttl != null ? opts.ttl
            : (opts.undo ? 10000
            : (kind === 'error' ? 5000 : 2000));
  dismissTimer = setTimeout(dismiss, ttl);
}

function ssFieldSaved(id) {
  const el = document.getElementById(id);
  if (!el) return;
  const field = el.closest('.ss-field');
  if (!field) return;
  const label = field.querySelector('label, .ss-label');
  if (!label) return;
  // Remove any previous pill first.
  field.querySelectorAll('.ss-field-saved').forEach(x => x.remove());
  const pill = document.createElement('span');
  pill.className = 'ss-field-saved';
  pill.textContent = '✓ Saved';
  label.appendChild(pill);
  setTimeout(() => pill.remove(), 2200);
}

function ssDescribeError(err) {
  if (!err) return 'Something went wrong.';
  const msg = (typeof err === 'string') ? err : (err.message || String(err));
  // Translate the common browser fetch failures to plain English.
  if (/Failed to fetch|NetworkError|ERR_NETWORK/i.test(msg)) {
    return "Couldn't reach Syntaur. Check your internet or the gateway.";
  }
  if (/401|unauthoriz/i.test(msg)) {
    return "Syntaur didn't recognize your sign-in. Try reloading the page.";
  }
  if (/403|forbidden/i.test(msg)) {
    return "You don't have permission for that.";
  }
  if (/404/.test(msg)) {
    return "Syntaur couldn't find that. It may have been renamed or removed.";
  }
  if (/429|rate.?limit/i.test(msg)) {
    return "The AI is busy right now. Wait a minute and try again.";
  }
  if (/5\d\d|server error|Internal Server/i.test(msg)) {
    return "Syntaur hit an internal error. Check the gateway log for details.";
  }
  return msg;
}

// Shared destructive-action prompt — replaces ad-hoc confirm() calls.
// Usage: ssConfirmDestructive("Delete Peter?", "this removes every memory", () => doDelete())
function ssConfirmDestructive(title, detail, onConfirm) {
  const yes = window.confirm(title + (detail ? '\n\n' + detail : '') + '\n\nThis cannot be undone.');
  if (yes) {
    try { onConfirm(); } catch(e) { ssToast(ssDescribeError(e), 'error'); }
  }
}

// ── Privacy preferences (persist via /api/settings/preferences) ──────
async function ssLoadPreferences() {
  try {
    const r = await fetch('/api/settings/preferences', { headers: ssAuthH() });
    if (!r.ok) return;
    const prefs = await r.json();
    document.querySelectorAll('input[data-pref]').forEach(inp => {
      const key = inp.dataset.pref;
      const v = prefs[key];
      if (inp.type === 'checkbox') inp.checked = (v === '1' || v === 'true' || v === true);
      else inp.value = v || '';
    });
  } catch(e) { console.error('load prefs:', e); }
}
async function ssSavePref(inp) {
  const key = inp.dataset.pref;
  const value = inp.type === 'checkbox' ? (inp.checked ? '1' : '0') : inp.value;
  try {
    const r = await fetch('/api/settings/preferences', {
      method: 'PUT',
      headers: ssJsonAuthH(),
      body: JSON.stringify({ key, value }),
    });
    if (!r.ok) throw new Error('save failed');
    localStorage.setItem('ss_privacy_reviewed', '1');
    if (inp.id) ssFieldSaved(inp.id);
  } catch(e) {
    ssToast("Couldn't save that setting.", 'error', { fix: ssDescribeError(e) });
  }
}

// ── Export / import config ──────────────────────────────────
async function ssExportConfig() {
  const status = document.getElementById('ss-export-status');
  if (status) { status.textContent = 'Preparing…'; status.style.color = 'var(--ss-ink-mute)'; }
  try {
    const r = await fetch('/api/settings/export', { headers: ssAuthH() });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const data = await r.json();
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'syntaur-export-' + new Date().toISOString().slice(0,10) + '.json';
    a.click();
    URL.revokeObjectURL(a.href);
    if (status) { status.textContent = '✓ Exported ' + Object.keys(data.preferences || {}).length + ' prefs, ' + (data.agents || []).length + ' agents'; status.style.color = 'var(--ss-success)'; }
  } catch(e) {
    if (status) { status.textContent = 'Error: ' + e.message; status.style.color = 'var(--ss-danger)'; }
  }
}
function ssImportConfig() { document.getElementById('ss-import-file').click(); }
async function ssHandleImport(file) {
  if (!file) return;
  const status = document.getElementById('ss-export-status');
  if (status) { status.textContent = 'Reading ' + file.name + '…'; status.style.color = 'var(--ss-ink-mute)'; }
  try {
    const text = await file.text();
    const data = JSON.parse(text);
    if (!data.syntaur_export_version) throw new Error('Not a Syntaur export file');
    const preview = [
      (data.agents || []).length + ' agents',
      Object.keys(data.preferences || {}).length + ' preferences',
    ].join(', ');
    if (!confirm('Import ' + preview + ' from this file?\n\nNote: this adds agents (with name collisions getting numeric suffixes) and overwrites any matching preferences. Secrets are never imported.')) {
      if (status) status.textContent = '';
      return;
    }
    // Import preferences — one PUT per key (small number of keys so OK).
    for (const [key, val] of Object.entries(data.preferences || {})) {
      await fetch('/api/settings/preferences', {
        method: 'PUT',
        headers: ssJsonAuthH(),
        body: JSON.stringify({ key, value: String(val) }),
      });
    }
    // Agents: use the existing /api/agents/create endpoint — secrets are never in the export.
    for (const a of data.agents || []) {
      if (!a.display_name || !a.system_prompt) continue;
      try {
        await fetch('/api/agents/create', {
          method: 'POST',
          headers: ssJsonAuthH(),
          body: JSON.stringify({
            display_name: a.display_name,
            description: a.description || null,
            system_prompt: a.system_prompt,
            is_main_thread: !!a.is_main_thread,
            avatar_color: a.avatar_color || null,
          }),
        });
      } catch(e) {}
    }
    ssLoadPreferences();
    if (status) { status.textContent = '✓ Imported ' + preview; status.style.color = 'var(--ss-success)'; }
  } catch(e) {
    if (status) { status.textContent = 'Error: ' + e.message; status.style.color = 'var(--ss-danger)'; }
  }
  document.getElementById('ss-import-file').value = '';
}

// ── Danger zone ────────────────────────────────────────────
let ssDangerAction = null;
let ssDangerPhrase = null;
function ssDangerConfirm(phrase, action) {
  ssDangerAction = action;
  ssDangerPhrase = phrase;
  const modal = document.getElementById('ss-danger-modal');
  const prompt = document.getElementById('ss-danger-prompt');
  const input = document.getElementById('ss-danger-input');
  const go = document.getElementById('ss-danger-go');
  if (prompt) prompt.innerHTML = 'Type <strong>' + phrase + '</strong> to confirm. This cannot be undone.';
  if (input) { input.value = ''; input.oninput = () => { go.disabled = (input.value.trim() !== phrase); }; }
  if (go) go.disabled = true;
  if (modal) modal.classList.remove('hidden');
  setTimeout(() => input && input.focus(), 30);
}
function ssDangerClose() {
  const modal = document.getElementById('ss-danger-modal');
  if (modal) modal.classList.add('hidden');
  ssDangerAction = null; ssDangerPhrase = null;
}
function ssDangerExecute() {
  if (!ssDangerAction || typeof window[ssDangerAction] !== 'function') { ssDangerClose(); return; }
  window[ssDangerAction]();
  ssDangerClose();
}
async function ssResetPreferences() {
  const prefs = document.querySelectorAll('input[data-pref]');
  for (const inp of prefs) {
    if (inp.type === 'checkbox') inp.checked = false;
    const key = inp.dataset.pref;
    try {
      await fetch('/api/settings/preferences', {
        method: 'PUT',
        headers: ssJsonAuthH(),
        body: JSON.stringify({ key, value: null }),
      });
    } catch(e) {}
  }
  ssToast('Preferences reset.', 'success');
}
async function ssWipeMemories() {
  try {
    const r = await fetch('/api/settings/wipe_memories', {
      method: 'POST',
      headers: ssJsonAuthH(),
      body: JSON.stringify({ confirm: 'wipe all memories' }),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || data.message || 'wipe failed');
    ssToast('Wiped ' + (data.deleted || 0) + ' memory rows. Helpers will rebuild context from scratch.', 'success', { ttl: 4000 });
  } catch(e) { ssToast("Couldn't wipe memories.", 'error', { fix: ssDescribeError(e) }); }
}
async function ssFactoryReset() {
  try {
    const r = await fetch('/api/settings/factory_reset', {
      method: 'POST',
      headers: ssJsonAuthH(),
      body: JSON.stringify({ confirm: 'factory reset' }),
    });
    if (r.status === 403) {
      ssToast('Factory reset is admin-only.', 'error', { fix: 'Ask the gateway admin to run it.' });
      return;
    }
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || 'reset failed');
    const wiped = data.wiped || {};
    const summary = Object.entries(wiped).map(([k, v]) => k + '=' + v).join(', ');
    ssToast('Factory reset complete. Returning to the dashboard…', 'success', { ttl: 2500, fix: summary });
    setTimeout(() => { window.location.href = '/'; }, 2500);
  } catch(e) { ssToast("Factory reset failed.", 'error', { fix: ssDescribeError(e) }); }
}

// ── Connect-things unified table hydration ────────────────
// Paints one status badge per row (Connected / not connected). Reads
// the aggregate /api/settings/integration_status for telegram / ha /
// voice / media bridge, and /api/sync/providers for per-service sync
// connections (prefix `sync:`).
async function ssRefreshConnections() {
  const rows = document.querySelectorAll('[data-conn-status]');
  if (!rows.length) return;
  let agg = null, sync = null, tailscale = null;
  try {
    const r = await fetch('/api/settings/integration_status', { headers: ssAuthH() });
    if (r.ok) agg = await r.json();
  } catch(e) {}
  try {
    const r = await fetch('/api/sync/providers', { headers: ssAuthH() });
    if (r.ok) sync = await r.json();
  } catch(e) {}
  try {
    const r = await fetch('/api/setup/tailscale/status', { headers: ssAuthH() });
    if (r.ok) tailscale = await r.json();
  } catch(e) {}
  const connected = sync && sync.connections ? sync.connections : {};

  rows.forEach(el => {
    const id = el.dataset.connStatus;
    let status = 'unknown', label = 'not connected';
    if (id.startsWith('sync:')) {
      const provider = id.slice(5);
      if (connected[provider]) { status = 'connected'; label = '✓ connected'; }
    } else if (id === 'tailscale') {
      if (tailscale && tailscale.connected) { status = 'connected'; label = '✓ ' + (tailscale.hostname || 'connected'); }
      else if (tailscale && tailscale.enabled) { status = 'error'; label = 'starting up…'; }
    } else if (agg) {
      const key = id === 'telegram' ? 'telegram'
                : id === 'homeassistant' ? 'homeassistant'
                : id === 'voice' ? 'voice'
                : id === 'media_bridge' ? 'media_bridge'
                : null;
      if (key && agg[key]) {
        const s = agg[key].status;
        if (s === 'connected') { status = 'connected'; label = '✓ connected'; }
        else if (s === 'error' || s === 'degraded') { status = 'error'; label = 'needs attention'; }
      }
    }
    el.className = 'ss-conn-status' + (status === 'connected' ? ' ss-conn-connected'
                                     : status === 'error' ? ' ss-conn-error' : '');
    el.textContent = label;
    // Update the action label so connected rows say "Manage" and empty rows say "Connect".
    const row = el.closest('.ss-conn-row');
    if (row) {
      const action = row.querySelector('.ss-conn-action');
      if (action) action.textContent = status === 'connected' ? 'Manage →' : 'Connect →';
    }
  });
}

// Live integration status pills — updates every 30s.
async function ssRefreshIntegrationStatus() {
  try {
    const r = await fetch('/api/settings/integration_status', { headers: ssAuthH() });
    if (!r.ok) return;
    const data = await r.json();
    const map = {
      'ss-pill-telegram': { s: data.telegram, labels: { connected: 'connected', not_configured: 'not configured' } },
      'ss-pill-ha':       { s: data.homeassistant, labels: { connected: 'connected', not_configured: 'not configured' } },
      'ss-pill-sync':     { s: data.sync,  labels: { connected: (data.sync?.connections || 0) + ' connected', not_configured: 'no providers' } },
      'ss-pill-media':    { s: data.media_bridge, labels: { connected: 'bridge running', not_configured: 'bridge offline' } },
      'ss-pill-llm':      { s: data.llm,   labels: { connected: (data.llm?.live || 0) + '/' + (data.llm?.total || 0) + ' live', not_configured: 'no providers', degraded: 'partial', error: 'error' } },
      'ss-pill-voice':    { s: data.voice, labels: { connected: (data.voice?.satellites || 0) + ' satellites', not_configured: 'none' } },
    };
    for (const [id, info] of Object.entries(map)) {
      const el = document.getElementById(id);
      if (!el || !info.s) continue;
      const status = info.s.status || 'unknown';
      el.className = 'ss-pill ss-pill-' + status;
      el.textContent = info.labels[status] || status;
    }
  } catch(e) { /* keep pills in "checking…" state */ }
}
setInterval(ssRefreshIntegrationStatus, 30000);

// About page — pull live stats.
async function ssRefreshAbout() {
  try {
    const r = await fetch('/health');
    if (!r.ok) return;
    const d = await r.json();
    const set = (id, v) => { const el = document.getElementById(id); if (el) el.textContent = v; };
    set('ss-version', d.version || '—');
    set('ss-uptime', d.uptime_secs != null ? `${Math.round(d.uptime_secs / 60)}m` : '—');
    set('ss-modules', (d.modules || []).length);
    set('ss-tools', d.tool_count || '—');
    set('ss-agents', (d.agents || []).map(a => a.name || a.id).join(', ') || '—');
    set('ss-llm', (d.providers || []).map(p => p.name).join(', ') || '—');
  } catch(e) {}
}

// Init: apply the URL-hash route, snapshot forms, refresh About, set up listeners.
(function () {
  ssApplyRoute();
  window.addEventListener('hashchange', () => { ssApplyRoute(); ssRefreshSetup(); });
  setTimeout(() => { ssSnapshotForms(); }, 400);
  ssRefreshAbout();
  ssLoadPreferences();
  ssRefreshSetup();
  ssRefreshIntegrationStatus();
  ssRefreshConnections();
  // Re-refresh setup state every 60s + on return from other tabs so it
  // reflects what the user just did elsewhere.
  setInterval(ssRefreshSetup, 60000);
  setInterval(ssRefreshConnections, 30000);
  window.addEventListener('focus', () => { ssRefreshSetup(); ssRefreshIntegrationStatus(); ssRefreshConnections(); });
})();
"##;
