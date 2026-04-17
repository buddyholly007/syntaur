//! /settings — command-center style settings page.
//!
//! Two-pane sidebar layout (10 sections) replacing the old 6-tab strip.
//! Deep-linkable via URL hash: `#account/profile`, `#integrations/telegram`,
//! etc. Includes a ⌘K command palette that indexes every setting leaf.
//!
//! Legacy tab HTML chunks are extracted from the former raw `BODY_HTML`
//! string and kept as `include_str!` blobs in `settings_chunks/` so the
//! existing JS (form wiring, LLM provider CRUD, sync connect flow, admin
//! invites, etc.) keeps working unchanged while we progressively rewrite
//! each tab into proper maud. PAGE_JS is likewise held as `page.js` and
//! included verbatim.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, Page};

// ── Legacy HTML blobs (progressively replaced with maud in later phases) ──
const LEGACY_GENERAL: &str = include_str!("settings_chunks/tab-general.html");
const LEGACY_LLM:     &str = include_str!("settings_chunks/tab-llm.html");
const LEGACY_SYNC:    &str = include_str!("settings_chunks/tab-sync.html");
const LEGACY_MEDIA:   &str = include_str!("settings_chunks/tab-media.html");
const LEGACY_SYSTEM:  &str = include_str!("settings_chunks/tab-system.html");
const LEGACY_USERS:   &str = include_str!("settings_chunks/tab-users.html");
const LEGACY_MODALS:  &str = include_str!("settings_chunks/modals.html");
const LEGACY_JS:      &str = include_str!("settings_chunks/page.js");

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Settings",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    // Substitute the server-rendered palette index into the JS template.
    let resolved_js = NEW_JS.replace("%%SS_INDEX%%", &palette_index_json());
    let body = html! {
        (top_bar())
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

        // Dirty-state banner (hidden by default; shown by JS on change)
        (dirty_banner())

        script { (PreEscaped(LEGACY_JS)) }
        script { (PreEscaped(resolved_js)) }
    };
    Html(shell(page, body).into_string())
}

// ══════════════════════════════════════════════════════════════════════
// Top bar + sidebar
// ══════════════════════════════════════════════════════════════════════

fn top_bar() -> Markup {
    html! {
        div class="ss-topbar" {
            div class="ss-topbar-inner" {
                a href="/" class="flex items-center gap-2 hover:opacity-80" {
                    img src="/app-icon.jpg" class="h-8 w-8 rounded-lg" alt="";
                    span class="font-semibold text-lg" { "Syntaur" }
                }
                span class="ss-crumb-sep" { "/" }
                span class="ss-crumb" { "Settings" }
                span class="ss-crumb-page" id="ss-current-crumb" { "" }
                div class="flex-1" {}
                button class="ss-search-hint" onclick="ssOpenPalette()" title="Search settings (⌘K)" {
                    svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" { circle cx="11" cy="11" r="7" {} path d="M21 21l-5-5" {} }
                    span { "Search…" }
                    kbd { "⌘K" }
                }
                a href="/" class="ss-link" { "← Dashboard" }
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
        // Home / welcome page — shown when no hash is set.
        (page_wrap("home", "home", "Settings", "Manage Syntaur, your agents, and how you connect.",
            home_body()))

        // ── ACCOUNT ────────────────────────────────────────
        (page_wrap("account", "profile", "Profile",
            "Your identity and personal info. Visible only to you.",
            account_profile_body()))
        (page_wrap("account", "security", "Password & security",
            "Change your password, manage active sessions.",
            account_security_body()))
        (page_wrap("account", "users", "Users (admin)",
            "Invite, promote, or remove users on this Syntaur instance.",
            html! { (PreEscaped(LEGACY_USERS)) }))

        // ── AGENTS ─────────────────────────────────────────
        (page_wrap("agents", "all", "All agents",
            "Create, import, and manage the agents in your Syntaur.",
            agents_all_body()))
        (page_wrap("agents", "personas", "Personas & tone",
            "The eight built-in personas (Peter, Kyron, Positron, Cortex, Silvr, Thaddeus, Maurice, Mushi) plus humor dials and memory protocol.",
            agents_personas_body()))

        // ── INTEGRATIONS ───────────────────────────────────
        (page_wrap("integrations", "telegram", "Telegram",
            "Chat with your agents from your phone.",
            html! { (PreEscaped(LEGACY_GENERAL)) }))
        (page_wrap("integrations", "homeassistant", "Home Assistant",
            "Let your agents see + control your smart home.",
            integrations_ha_body()))
        (page_wrap("integrations", "sync", "Sync",
            "Connect cloud services so agents can read your calendar, email, bank, and more.",
            html! { (PreEscaped(LEGACY_SYNC)) }))
        (page_wrap("integrations", "media", "Media bridge",
            "Local companion app that lets agents play hidden audio/video from Apple Music, Spotify, Tidal, and YouTube.",
            html! { (PreEscaped(LEGACY_MEDIA)) }))

        // ── LLM ────────────────────────────────────────────
        (page_wrap("llm", "providers", "Providers",
            "Which model answers each agent. Order + fallback + per-provider API keys.",
            html! { (PreEscaped(LEGACY_LLM)) }))

        // ── VOICE ──────────────────────────────────────────
        (page_wrap("voice", "satellites", "Satellites",
            "Smart speakers running your wake word. Train voice prints, assign rooms.",
            voice_satellites_body()))

        // ── MODULES ────────────────────────────────────────
        (page_wrap("modules", "installed", "Installed modules",
            "Turn module capabilities on and off. Each module has its own settings gear.",
            modules_installed_body()))

        // ── APPEARANCE ─────────────────────────────────────
        (page_wrap("appearance", "theme", "Theme",
            "Dashboard color palette and density. Individual modules keep their own themes.",
            appearance_theme_body()))

        // ── PRIVACY & DATA ─────────────────────────────────
        (page_wrap("privacy", "data", "What Syntaur stores",
            "A frank table of what's kept, where, and for how long.",
            privacy_data_body()))

        // ── SYSTEM ─────────────────────────────────────────
        (page_wrap("system", "gateway", "Gateway & ports",
            "Network + runtime configuration. Some fields require a gateway restart.",
            html! { (PreEscaped(LEGACY_SYSTEM)) }))
        (page_wrap("system", "danger", "Danger zone",
            "Destructive actions. Typed confirmation required.",
            system_danger_body()))

        // ── ABOUT ──────────────────────────────────────────
        (page_wrap("about", "info", "About this Syntaur",
            "Version, uptime, tool count, and attribution.",
            about_body()))
    }
}

fn page_wrap(section: &str, page: &str, title: &str, subtitle: &str, body: Markup) -> Markup {
    html! {
        section class="ss-page" data-section=(section) data-page=(page) id={ "ss-page-" (section) "-" (page) } {
            header class="ss-page-header" {
                h1 class="ss-page-title" { (title) }
                p class="ss-page-subtitle" { (subtitle) }
            }
            div class="ss-page-body" { (body) }
        }
    }
}

// ── Home welcome ───────────────────────────────────────────
fn home_body() -> Markup {
    html! {
        div class="ss-welcome-grid" {
            a class="ss-welcome-tile" href="#agents/all" onclick="ssNavigate('agents','all');return false;" {
                div class="ss-welcome-ico" { "🧑‍🚀" }
                div class="ss-welcome-label" { "Agents" }
                div class="ss-welcome-sub" { "Create, import, or promote" }
            }
            a class="ss-welcome-tile" href="#integrations/telegram" onclick="ssNavigate('integrations','telegram');return false;" {
                div class="ss-welcome-ico" { "🔗" }
                div class="ss-welcome-label" { "Integrations" }
                div class="ss-welcome-sub" { "Telegram, HA, Sync…" }
            }
            a class="ss-welcome-tile" href="#llm/providers" onclick="ssNavigate('llm','providers');return false;" {
                div class="ss-welcome-ico" { "🧠" }
                div class="ss-welcome-label" { "LLM" }
                div class="ss-welcome-sub" { "Providers + fallback" }
            }
            a class="ss-welcome-tile" href="#voice/satellites" onclick="ssNavigate('voice','satellites');return false;" {
                div class="ss-welcome-ico" { "🎙" }
                div class="ss-welcome-label" { "Voice" }
                div class="ss-welcome-sub" { "Wake word + satellites" }
            }
            a class="ss-welcome-tile" href="#privacy/data" onclick="ssNavigate('privacy','data');return false;" {
                div class="ss-welcome-ico" { "🛡" }
                div class="ss-welcome-label" { "Privacy" }
                div class="ss-welcome-sub" { "What's stored + export" }
            }
            a class="ss-welcome-tile" href="#about/info" onclick="ssNavigate('about','info');return false;" {
                div class="ss-welcome-ico" { "ℹ️" }
                div class="ss-welcome-label" { "About" }
                div class="ss-welcome-sub" { "Version + uptime" }
            }
        }
        div class="ss-tip" {
            "Tip — press "
            kbd { "⌘K" }
            " to jump to any setting."
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

// ── Agents ─────────────────────────────────────────────────
fn agents_all_body() -> Markup {
    html! {
        div class="ss-card" {
            p class="ss-help" {
                "You can run multiple main-thread agents (Peter / Kyron-tier privileges) "
                "and import agents from other platforms via Markdown, plain text, or JSON."
            }
            div class="ss-actions" {
                a href="/settings/agents" class="ss-btn-primary" { "Open agent manager →" }
            }
            p class="ss-help" style="margin-top:12px" {
                "The agent manager currently lives at "
                code { "/settings/agents" }
                " — will be folded inline in the next iteration."
            }
        }
    }
}

fn agents_personas_body() -> Markup {
    html! {
        div class="ss-card" {
            p class="ss-help" {
                "Syntaur seeds eight personas on first run. Each has a distinct role, tone, "
                "and memory scope. You can rename them during onboarding or any time from "
                "the individual agent's detail page."
            }
            div class="ss-persona-grid" {
                (persona_tile("🕷", "Peter", "Personal main agent", "Quiet-apartment Peter Parker — Sean's personal deployment"))
                (persona_tile("🧭", "Kyron", "Product-default main agent", "TARS + EDI + Ghost — loyal companion AI"))
                (persona_tile("🤖", "Positron", "Ledger / Tax", "Data (TNG) + C-3PO — analytical, formal"))
                (persona_tile("🔬", "Cortex", "Knowledge + Research", "Walter Bishop + Doc Brown — eccentric genius"))
                (persona_tile("🎸", "Silvr", "Music", "Johnny Silverhand + Creed Bratton — one-line picks"))
                (persona_tile("🎩", "Thaddeus", "Calendar + Todos", "Alfred + Jeeves + Carson — warm-British-butler"))
                (persona_tile("💻", "Maurice", "Coders", "Moss + Jared + Frink — earnest nerd pair programmer"))
                (persona_tile("🍃", "Mushi", "Journal", "Iroh + Mister Rogers + Troi — isolated, gentle wisdom"))
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
fn integrations_ha_body() -> Markup {
    html! {
        div class="ss-card" {
            div class="ss-field" {
                label class="ss-label" for="ha-url" { "Home Assistant URL" }
                input id="ha-url" class="ss-input" placeholder="http://homeassistant.local:8123" {}
                p class="ss-help" { "The URL of your Home Assistant instance, reachable from this gateway." }
            }
            div class="ss-field" {
                label class="ss-label" for="ha-token" { "Long-lived access token" }
                input id="ha-token" class="ss-input" type="password" placeholder="eyJhbGc…" {}
                p class="ss-help" { "Generate one in HA → Profile → Long-Lived Access Tokens." }
            }
            div class="ss-actions" {
                button class="ss-btn-secondary" onclick="ssHaTest()" { "Test connection" }
                button class="ss-btn-primary" onclick="ssHaSave()" { "Connect Home Assistant" }
            }
            p id="ss-ha-result" class="ss-help" {}
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
        div class="ss-card" {
            p class="ss-help" {
                "The dashboard uses a calm-neutral palette by default. Each module has its "
                "own theme (cyberpunk music, parchment knowledge, retro-CRT coders, LCARS tax) — "
                "those won't change."
            }
            div class="ss-field" {
                label class="ss-label" { "Dashboard density" }
                div class="ss-radio-row" {
                    label class="ss-radio" { input type="radio" name="density" value="comfy" checked; " Comfortable" }
                    label class="ss-radio" { input type="radio" name="density" value="compact"; " Compact" }
                }
                p class="ss-help" { "Compact trims padding in cards and widgets by ~30%." }
            }
            div class="ss-field" {
                label class="ss-label" { "Accent" }
                div class="ss-swatch-row" {
                    div class="ss-swatch" data-color="#7aa2ff" style="background:#7aa2ff" title="Calm blue (default)" {}
                    div class="ss-swatch" data-color="#7fbf8a" style="background:#7fbf8a" title="Sage" {}
                    div class="ss-swatch" data-color="#f0b470" style="background:#f0b470" title="Amber" {}
                    div class="ss-swatch" data-color="#d97a7a" style="background:#d97a7a" title="Clay" {}
                    div class="ss-swatch" data-color="#b797c7" style="background:#b797c7" title="Lavender" {}
                }
                p class="ss-help" { "Changes the dashboard accent color only; module themes are unaffected." }
            }
            p class="ss-help" style="color:var(--ss-warn)" {
                "(Live preview + persistence wiring coming — this pane currently shows options only.)"
            }
        }
    }
}

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
            p class="ss-help" { "(Functional toggles land in Phase G — structure shown now so you can see where they'll live.)" }
            div class="ss-toggle-row" { span { "LLM prompt logging" } input type="checkbox" disabled; }
            div class="ss-toggle-row" { span { "Voice-transcript auto-save" } input type="checkbox" disabled; }
            div class="ss-toggle-row" { span { "Anonymous telemetry" } input type="checkbox" disabled; }
        }
        div class="ss-card" {
            h3 class="ss-card-title" { "Export / import" }
            p class="ss-help" { "Download a portable copy of your config or import from a previous install." }
            div class="ss-actions" {
                button class="ss-btn-secondary" onclick="alert('Settings-as-code export — Phase H.')" { "Export config (JSON)" }
                button class="ss-btn-secondary" onclick="alert('Settings-as-code import — Phase H.')" { "Import config…" }
            }
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
                    div class="ss-danger-name" { "Reset all settings to defaults" }
                    div class="ss-help" { "Keeps your data (chats, memories, agents) — only config is reset." }
                }
                button class="ss-btn-danger" onclick="alert('Phase H.')" { "Reset…" }
            }
            div class="ss-danger-row" {
                div {
                    div class="ss-danger-name" { "Wipe all agent memories" }
                    div class="ss-help" { "Removes every saved memory across every agent (journal included only if opted in)." }
                }
                button class="ss-btn-danger" onclick="alert('Phase G.')" { "Wipe memories…" }
            }
            div class="ss-danger-row" {
                div {
                    div class="ss-danger-name" { "Factory reset" }
                    div class="ss-help" { "Erase all data and configuration. Same as a fresh install." }
                }
                button class="ss-btn-danger" onclick="alert('Phase G.')" { "Factory reset…" }
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
}

const SECTIONS: &[SectionDef] = &[
    SectionDef { slug: "account", title: "Account", pages: &[
        PageDef { slug: "profile", title: "Profile", badge: None, keywords: "name email username display", description: "Your identity and personal info" },
        PageDef { slug: "security", title: "Password & security", badge: None, keywords: "password login sessions signout logout 2fa", description: "Change password, active sessions" },
        PageDef { slug: "users", title: "Users", badge: Some("admin"), keywords: "invite team members users roles admin", description: "Invite and manage users (admin only)" },
    ]},
    SectionDef { slug: "agents", title: "Agents", pages: &[
        PageDef { slug: "all", title: "All agents", badge: None, keywords: "create import main thread agents peter felix kyron", description: "Create, import, and manage agents" },
        PageDef { slug: "personas", title: "Personas & tone", badge: None, keywords: "personas peter kyron positron cortex silvr thaddeus maurice mushi humor dial", description: "Built-in personas and tone dials" },
    ]},
    SectionDef { slug: "integrations", title: "Integrations", pages: &[
        PageDef { slug: "telegram", title: "Telegram", badge: None, keywords: "telegram bot phone chat messaging", description: "Chat from your phone via a Telegram bot" },
        PageDef { slug: "homeassistant", title: "Home Assistant", badge: None, keywords: "home assistant smart home ha homeassistant", description: "Connect to a Home Assistant instance" },
        PageDef { slug: "sync", title: "Sync", badge: None, keywords: "google microsoft gmail calendar drive sync oauth plaid stripe coinbase simplefin", description: "Cloud connectors (Google, Microsoft, bank, etc.)" },
        PageDef { slug: "media", title: "Media bridge", badge: None, keywords: "media apple music spotify tidal youtube bridge playback", description: "Local companion for hidden playback" },
    ]},
    SectionDef { slug: "llm", title: "LLM", pages: &[
        PageDef { slug: "providers", title: "Providers", badge: None, keywords: "openrouter lm studio turboquant fallback api key model provider openai anthropic claude", description: "Model providers, API keys, fallback chain" },
    ]},
    SectionDef { slug: "voice", title: "Voice", pages: &[
        PageDef { slug: "satellites", title: "Satellites", badge: None, keywords: "voice wake word satellite esphome speaker", description: "Voice satellites and wake word" },
    ]},
    SectionDef { slug: "modules", title: "Modules", pages: &[
        PageDef { slug: "installed", title: "Installed", badge: None, keywords: "modules extensions enable disable tax music knowledge coders journal", description: "Enable and disable modules" },
    ]},
    SectionDef { slug: "appearance", title: "Appearance", pages: &[
        PageDef { slug: "theme", title: "Theme", badge: None, keywords: "theme dark mode density accent color appearance", description: "Dashboard palette and density" },
    ]},
    SectionDef { slug: "privacy", title: "Privacy & data", pages: &[
        PageDef { slug: "data", title: "What Syntaur stores", badge: None, keywords: "privacy data retention telemetry logging export import", description: "What's stored, where, for how long" },
    ]},
    SectionDef { slug: "system", title: "System", pages: &[
        PageDef { slug: "gateway", title: "Gateway & ports", badge: None, keywords: "gateway port bind network restart system config", description: "Gateway network + runtime settings" },
        PageDef { slug: "danger", title: "Danger zone", badge: None, keywords: "reset wipe factory delete danger", description: "Destructive actions" },
    ]},
    SectionDef { slug: "about", title: "About", pages: &[
        PageDef { slug: "info", title: "About this Syntaur", badge: None, keywords: "about version uptime tools licenses", description: "Version, uptime, tool count" },
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
"##;

// Fresh JS used by the new shell. Legacy JS from `page.js` handles all the
// existing tab content (LLM CRUD, sync connect, invite dialog, etc.).
const NEW_JS: &str = r##"
// Settings index for the ⌘K palette. Generated server-side.
window.SS_INDEX = %%SS_INDEX%%;

function ssParseHash() {
  const h = (window.location.hash || '').replace(/^#/, '').trim();
  if (!h) return { section: 'home', page: 'home' };
  const [s, p] = h.split('/');
  return { section: s || 'home', page: p || '' };
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
    // Fallback to the first page in the section, then to home.
    target = document.querySelector(`.ss-page[data-section="${section}"]`);
    if (!target) target = document.getElementById('ss-page-home-home');
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
    // Map new page slugs to legacy tab ids.
    const LEGACY = {
      'integrations/telegram': 'general',
      'integrations/sync': 'sync',
      'integrations/media': 'media',
      'llm/providers': 'llm',
      'system/gateway': 'system',
      'account/users': 'users',
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
  window.addEventListener('hashchange', ssApplyRoute);
  setTimeout(() => { ssSnapshotForms(); }, 400);
  ssRefreshAbout();
})();
"##;
