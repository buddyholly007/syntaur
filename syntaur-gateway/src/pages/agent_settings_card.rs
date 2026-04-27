//! Card-flip back-of-card markup for the per-chat agent settings cog.
//!
//! Sections accumulate across phases (see vault/projects/syntaur_per_chat_settings.md);
//! Phase 0 shipped the always-visible Resource Budget bar; Phase 1 (this
//! commit) adds the chat_card_flip wrapper + Identity section + chrome
//! (cog, Done button, section accordion); subsequent phases drop their
//! sections in below.
//!
//! All markup is rendered server-side via `maud` so it can be inlined
//! into any page's chat container with no extra fetch. The accompanying
//! JS boots up on `syntaur:page-arrived` and polls
//! `/api/compute/state` every 10s while the card is flipped.

use maud::{html, Markup, PreEscaped};

/// Wrap any chat surface in a flip container. (Legacy — retained for
/// /chat which was server-side wrapped in earlier ships. New surfaces
/// use [`cog_button`] + the slide-over [`agent_settings_overlay`]
/// instead, because wrapping arbitrary side panels (drawers, fixed-
/// positioned rails) broke their layout.)
///
/// 2026-04-26: Sean reported only /chat showed the cog after Phase 8
/// shipped. Root cause: side-panel wrap-and-move-into-cf-front strips
/// `position: fixed` / sibling-flex constraints. New approach is
/// non-invasive — we APPEND the cog to each panel without moving the
/// panel itself, and clicking the cog opens a viewport-anchored
/// slide-over drawer holding the full back-of-card.
pub fn chat_card_flip(agent_id: &str, front: Markup, _back: Markup) -> Markup {
    // /chat keeps its server-side mount. The cog is now a plain
    // overlay button (no wrap). Front content renders untouched.
    html! {
        div class="chat-cog-host" data-agent=(agent_id) {
            (cog_button(agent_id))
            (front)
        }
    }
}

/// The cog button itself — same markup whether emitted server-side or
/// JS-injected onto a side panel. Click is delegated globally; the
/// data-agent attribute identifies which agent's settings to load.
///
/// Sean's directive (2026-04-26): no "Settings" label, small icon
/// only, theme-matching. Style inherits via `currentColor` so a
/// scheduler-sage cog stays sage and a coders-phosphor cog stays
/// phosphor without per-module CSS.
pub fn cog_button(agent_id: &str) -> Markup {
    html! {
        button
            type="button"
            class="cf-cog"
            data-agent=(agent_id)
            aria-label="Agent settings"
            aria-pressed="false"
            title="Agent settings"
        {
            svg
                width="14" height="14" viewBox="0 0 24 24"
                fill="currentColor"
            {
                path d="M19.43 12.98c.04-.32.07-.64.07-.98s-.03-.66-.07-.98l2.11-1.65c.19-.15.24-.42.12-.64l-2-3.46c-.12-.22-.39-.3-.61-.22l-2.49 1c-.52-.4-1.08-.73-1.69-.98l-.38-2.65C14.46 2.18 14.25 2 14 2h-4c-.25 0-.46.18-.49.42l-.38 2.65c-.61.25-1.17.59-1.69.98l-2.49-1c-.23-.09-.49 0-.61.22l-2 3.46c-.13.22-.07.49.12.64l2.11 1.65c-.04.32-.07.65-.07.98s.03.66.07.98l-2.11 1.65c-.19.15-.24.42-.12.64l2 3.46c.12.22.39.3.61.22l2.49-1c.52.4 1.08.73 1.69.98l.38 2.65c.03.24.24.42.49.42h4c.25 0 .46-.18.49-.42l.38-2.65c.61-.25 1.17-.59 1.69-.98l2.49 1c.23.09.49 0 .61-.22l2-3.46c.12-.22.07-.49-.12-.64l-2.11-1.65zM12 15.5c-1.93 0-3.5-1.57-3.5-3.5s1.57-3.5 3.5-3.5 3.5 1.57 3.5 3.5-1.57 3.5-3.5 3.5z" {}
            }
        }
    }
}

/// Singleton viewport-anchored slide-over that holds the agent settings
/// drawer. Lazy-fills on first cog click via /api/agents/{id}/settings_back,
/// then re-fills as the user opens different agents' cogs. Lives in shell
/// alongside the bug-report overlay so every authed page has it.
pub fn agent_settings_overlay() -> Markup {
    html! {
        div id="syntaur-agent-overlay"
            class="syntaur-agent-overlay"
            data-open="false"
            aria-hidden="true"
        {
            div class="sao-scrim" data-action="close-overlay" {}
            aside class="sao-drawer" role="dialog" aria-label="Agent settings" {
                header class="sao-head" {
                    h2 class="sao-title" {
                        span data-bind="overlay-agent" { "Agent" }
                        " · settings"
                    }
                    button type="button"
                        class="sao-close"
                        data-action="close-overlay"
                        aria-label="Close settings"
                    {
                        svg width="14" height="14" viewBox="0 0 16 16"
                            fill="none" stroke="currentColor"
                            stroke-width="1.6" stroke-linecap="round"
                        {
                            path d="M3 3l10 10 M13 3L3 13" {}
                        }
                    }
                }
                div class="sao-body" data-bind="overlay-body" {
                    div class="sao-skel" { "Loading…" }
                }
            }
        }
    }
}

/// Full back-of-card markup — pinned Resource Budget bar followed by
/// nine collapsible sections. Phase 1 ships chrome + Identity; later
/// phases hydrate the others (each section keeps its own
/// data-section="<name>" hook so phased rollout doesn't disturb
/// neighbouring markup).
pub fn agent_settings_back(agent_id: &str) -> Markup {
    html! {
        div class="cf-back-inner" data-agent=(agent_id) {
            header class="cf-back-head" {
                button
                    type="button"
                    class="cf-back-done"
                    aria-label="Close settings"
                    title="Done (Esc)"
                {
                    svg width="14" height="14" viewBox="0 0 16 16"
                        fill="none" stroke="currentColor"
                        stroke-width="1.6" stroke-linecap="round"
                    {
                        path d="M3 3l10 10 M13 3L3 13" {}
                    }
                    " Done"
                }
                h2 class="cf-back-title" { "Agent settings" }
                span class="cf-back-saving" hidden { "Saving…" }
            }
            (resource_budget_bar(agent_id))
            (section_identity(agent_id))
            (section_brain(agent_id))
            (section_voice(agent_id))
            (section_persona(agent_id))
            (section_tools(agent_id))
            (section_memory(agent_id))
            (section_limits(agent_id))
            (section_maintenance(agent_id))
        }
    }
}

/// Identity section (Phase 1). Auto-saves on blur via PUT /api/agents/{id}/settings
/// with a partial-merge payload (e.g. `{"display_name": "Peter ✨"}`).
fn section_identity(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="identity" open {
            summary class="cf-section-head" { "Identity" }
            div class="cf-section-body" {
                div class="cf-row" {
                    label for=(format!("id-name-{agent_id}")) { "Display name" }
                    input
                        id=(format!("id-name-{agent_id}"))
                        type="text"
                        maxlength="60"
                        data-field="display_name"
                        autocomplete="off"
                        placeholder="(persona default)";
                }
                div class="cf-row" {
                    label { "Icon" }
                    div class="cf-icon-row" {
                        div class="cf-icon-preview" data-role="icon-preview" {
                            // Server fills with current icon URL or letter avatar.
                            span class="cf-icon-letter" { (PreEscaped("?")) }
                        }
                        label class="cf-icon-upload" {
                            "Upload…"
                            input
                                type="file"
                                accept="image/png,image/jpeg,image/webp"
                                data-field="icon"
                                hidden;
                        }
                        button
                            type="button"
                            class="cf-icon-clear"
                            data-action="clear-icon"
                            { "Use letter" }
                    }
                }
                div class="cf-row" {
                    label { "Accent color" }
                    div class="cf-accent-presets" data-field="accent_color" {
                        @for hex in ACCENT_PRESETS {
                            button
                                type="button"
                                class="cf-accent-swatch"
                                data-accent=(hex)
                                style=(format!("background:{hex}"))
                                aria-label=(format!("Accent {hex}"))
                                title=(*hex)
                                {}
                        }
                        input
                            type="color"
                            class="cf-accent-custom"
                            data-field="accent_color"
                            aria-label="Custom accent color";
                    }
                }
                div class="cf-row" {
                    label for=(format!("id-wake-{agent_id}")) { "Wake phrase" }
                    input
                        id=(format!("id-wake-{agent_id}"))
                        type="text"
                        maxlength="40"
                        data-field="wake_phrase"
                        autocomplete="off"
                        placeholder=(format!("Hey {agent_id}"));
                    span class="cf-hint" { "Connect a satellite mic to enable" }
                }
                div class="cf-row" {
                    label for=(format!("id-shortcut-{agent_id}")) { "Keyboard shortcut" }
                    input
                        id=(format!("id-shortcut-{agent_id}"))
                        type="text"
                        readonly
                        class="cf-shortcut-recorder"
                        data-field="shortcut"
                        placeholder="Click and press a key combo";
                    button
                        type="button"
                        class="cf-shortcut-clear"
                        data-action="clear-shortcut"
                        { "Clear" }
                }
            }
        }
    }
}

/// Brain (LLM) section. Drag-to-reorder priority list, Where badge, live
/// status dots, per-tier wait. "Add Brain Model" inline form opens a
/// drawer-within-the-back with Local-GPU / LAN-LLM / Cloud sources.
///
/// JS hydrates the list from `agent_settings.llm_chain_json` (or the
/// global default chain when NULL). Drag-reorder + per-row wait + add/remove
/// all PUT a fresh `llm_chain_json` blob.
fn section_brain(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="brain" {
            summary class="cf-section-head" { "Brain (LLM)" }
            div class="cf-section-body" {
                div class="cf-row" {
                    label { "Active" }
                    span class="cf-active-model" data-role="brain-active" { "(loading…)" }
                }
                div class="cf-row" {
                    label { "Use global defaults" }
                    label class="cf-toggle" {
                        input
                            type="checkbox"
                            data-field="llm_use_global"
                            checked;
                        span class="cf-toggle-slider" {}
                    }
                }
                ol class="cf-chain-list" data-chain="brain" data-agent=(agent_id) {
                    li class="cf-chain-skel" { "Loading priority list…" }
                }
                button
                    type="button"
                    class="cf-chain-add"
                    data-add-chain="brain"
                    { "+ Add Brain Model" }
                p class="cf-chain-preview" data-role="brain-preview" {}
            }
        }
    }
}

/// Voice section — two stacks (TTS + STT) following the Brain pattern,
/// then voice character (voice id, rate, pitch, Sample button).
fn section_voice(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="voice" {
            summary class="cf-section-head" { "Voice (TTS / STT)" }
            div class="cf-section-body" {
                h3 class="cf-subhead" { "TTS chain" }
                ol class="cf-chain-list" data-chain="tts" data-agent=(agent_id) {
                    li class="cf-chain-skel" { "Loading TTS chain…" }
                }
                button type="button" class="cf-chain-add" data-add-chain="tts"
                    { "+ Add TTS Model" }

                h3 class="cf-subhead" { "STT chain" }
                ol class="cf-chain-list" data-chain="stt" data-agent=(agent_id) {
                    li class="cf-chain-skel" { "Loading STT chain…" }
                }
                button type="button" class="cf-chain-add" data-add-chain="stt"
                    { "+ Add STT Model" }

                h3 class="cf-subhead" { "Voice character" }
                div class="cf-row" {
                    label for=(format!("v-id-{agent_id}")) { "Voice ID" }
                    select id=(format!("v-id-{agent_id}")) data-field="voice_id" {
                        option value="" { "(persona default)" }
                    }
                }
                div class="cf-row" {
                    label for=(format!("v-rate-{agent_id}")) { "Speaking rate" }
                    div class="cf-slider-row" {
                        input
                            id=(format!("v-rate-{agent_id}"))
                            type="range"
                            min="0.7" max="1.5" step="0.05"
                            data-field="speaking_rate"
                            value="1.0";
                        span class="cf-slider-val" data-bind="speaking_rate" { "1.0×" }
                    }
                }
                div class="cf-row" {
                    label for=(format!("v-pitch-{agent_id}")) { "Pitch" }
                    div class="cf-slider-row" {
                        input
                            id=(format!("v-pitch-{agent_id}"))
                            type="range"
                            min="-20" max="20" step="1"
                            data-field="pitch_shift"
                            value="0";
                        span class="cf-slider-val" data-bind="pitch_shift" { "0%" }
                    }
                }
                div class="cf-row" {
                    label {}
                    button
                        type="button"
                        class="cf-voice-sample"
                        data-action="voice-sample"
                        { "▶ Sample" }
                }
                div class="cf-row cf-row-stack" {
                    label class="cf-toggle" {
                        input
                            type="checkbox"
                            data-action="tts-on-reply-toggle";
                        span class="cf-toggle-slider" {}
                        " Speak agent replies aloud"
                    }
                    span class="cf-row-hint" {
                        "When enabled, the agent's responses are spoken via TTS as well as shown."
                    }
                }
            }
        }
    }
}

/// Persona prompt — read-only by default; "Edit (override default)"
/// toggle reveals a textarea pre-populated with the resolved default.
/// Variables card lists template substitutions (no editing). Reset link
/// nukes the override. "Test prompt" runs a one-shot say-hi against
/// the active brain.
fn section_persona(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="persona" {
            summary class="cf-section-head" { "Persona prompt" }
            div class="cf-section-body" {
                div class="cf-row cf-row-stack" {
                    div class="cf-persona-toolbar" {
                        label class="cf-toggle" {
                            input
                                type="checkbox"
                                data-action="persona-edit-toggle";
                            span class="cf-toggle-slider" {}
                            " Edit (override default)"
                        }
                        button type="button" class="cf-persona-paste"
                            data-action="persona-paste-clean"
                            { "Paste (cleaned)" }
                        button type="button" class="cf-persona-test"
                            data-action="persona-test"
                            { "Test prompt" }
                        button type="button" class="cf-persona-reset"
                            data-action="persona-reset"
                            { "Reset to default" }
                    }
                    textarea
                        class="cf-persona-text"
                        data-field="persona_prompt_override"
                        rows="20"
                        readonly
                        spellcheck="false"
                        placeholder="Loading default persona prompt…"
                        {}
                }
                div class="cf-row cf-row-stack" {
                    div class="cf-persona-vars" data-role="persona-vars" {
                        h4 { "Template variables" }
                        ul class="cf-var-list" data-bind="persona_vars" {
                            li { "{{user_first_name}}" }
                            li { "{{main_agent_name}}" }
                            li { "{{date_today}}" }
                        }
                    }
                }
            }
        }
    }
}

/// Tools — checkbox grid grouped by category; tools outside the
/// persona's max set greyed out (security boundary). Counter, search,
/// Test-a-tool button.
fn section_tools(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="tools" {
            summary class="cf-section-head" { "Tools" }
            div class="cf-section-body" {
                div class="cf-tools-toolbar" {
                    span class="cf-tools-counter" data-bind="tools-counter" {
                        "Tools allowed: 0 / 0"
                    }
                    input
                        type="search"
                        class="cf-tools-search"
                        placeholder="Filter tools…"
                        data-bind="tools-search";
                    button type="button" class="cf-tools-test"
                        data-action="tools-test"
                        { "Test a tool" }
                }
                div class="cf-tools-grid" data-agent=(agent_id) {
                    p class="cf-tools-skel" { "Loading 180+ tools…" }
                }
            }
        }
    }
}

fn section_memory(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="memory" {
            summary class="cf-section-head" { "Memory + behavior" }
            div class="cf-section-body" {
                div class="cf-row" {
                    label { "Memory mode" }
                    label class="cf-radio" {
                        input
                            type="radio"
                            name=(format!("mem-mode-{agent_id}"))
                            data-field="memory_mode"
                            value="persistent"
                            checked;
                        " Persistent"
                    }
                    label class="cf-radio" {
                        input
                            type="radio"
                            name=(format!("mem-mode-{agent_id}"))
                            data-field="memory_mode"
                            value="incognito";
                        " Incognito"
                    }
                }
                div class="cf-row" {
                    label for=(format!("ctx-{agent_id}")) { "Context budget" }
                    div class="cf-slider-row" {
                        input
                            id=(format!("ctx-{agent_id}"))
                            type="range"
                            min="4096" max="131072" step="2048"
                            data-field="context_budget"
                            value="32768";
                        span class="cf-slider-val" data-bind="context_budget" { "32k tokens" }
                    }
                }
                div class="cf-row" {
                    label for=(format!("temp-{agent_id}")) { "Temperature" }
                    div class="cf-slider-row" {
                        input
                            id=(format!("temp-{agent_id}"))
                            type="range"
                            min="0" max="1.5" step="0.05"
                            data-field="temperature"
                            value="0.7";
                        span class="cf-slider-val" data-bind="temperature" { "0.70" }
                    }
                }
                div class="cf-row" {
                    label { "Streaming" }
                    label class="cf-toggle" {
                        input type="checkbox" data-field="streaming" checked;
                        span class="cf-toggle-slider" {}
                    }
                }
                div class="cf-row" {
                    label { "Show \"thinking\" lines" }
                    label class="cf-toggle" {
                        input type="checkbox" data-field="show_thinking" checked;
                        span class="cf-toggle-slider" {}
                    }
                }
                div class="cf-row" {
                    label for=(format!("ho-{agent_id}")) { "Auto-handoff" }
                    select
                        id=(format!("ho-{agent_id}"))
                        data-field="handoff_threshold"
                    {
                        option value="never" { "Never" }
                        option value="loose" selected { "Loose" }
                        option value="strict" { "Strict (cap rounds at 6)" }
                    }
                }
            }
        }
    }
}

fn section_limits(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="limits" {
            summary class="cf-section-head" { "Limits" }
            div class="cf-section-body" {
                div class="cf-row" {
                    label for=(format!("lim-cost-{agent_id}")) { "Daily cost cap" }
                    div class="cf-input-prefix" {
                        span { "$" }
                        input
                            id=(format!("lim-cost-{agent_id}"))
                            type="number"
                            min="0" step="0.50"
                            data-field="daily_cost_cap_cents"
                            data-cents="1"
                            placeholder="(unlimited)";
                    }
                    span class="cf-hint" { "Soft-throttles to free tier at 80%" }
                }
                div class="cf-row" {
                    label for=(format!("lim-rounds-{agent_id}")) { "Per-turn rounds cap" }
                    input
                        id=(format!("lim-rounds-{agent_id}"))
                        type="number"
                        min="1" max="50"
                        data-field="rounds_cap"
                        value="6";
                }
                div class="cf-row" {
                    label for=(format!("lim-timeout-{agent_id}")) { "Per-message timeout" }
                    input
                        id=(format!("lim-timeout-{agent_id}"))
                        type="number"
                        min="5" max="600"
                        data-field="per_turn_timeout_secs"
                        value="60";
                    span class="cf-hint" { "seconds" }
                }
            }
        }
    }
}

fn section_maintenance(agent_id: &str) -> Markup {
    html! {
        details class="cf-section" data-section="maintenance" {
            summary class="cf-section-head" { "Maintenance" }
            div class="cf-section-body" {
                div class="cf-maint-row" {
                    button type="button" class="cf-maint-btn"
                        data-action="export-conversation"
                        { "Export this conversation" }
                    button type="button" class="cf-maint-btn"
                        data-action="export-persona"
                        { "Export persona (JSON)" }
                    button type="button" class="cf-maint-btn"
                        data-action="import-persona"
                        { "Import persona…" }
                }
                div class="cf-maint-row" {
                    label class="cf-toggle" {
                        input type="checkbox" data-field="dashboard_widget";
                        span class="cf-toggle-slider" {}
                        " Show on /dashboard widget grid"
                    }
                }
                div class="cf-maint-row" {
                    button type="button" class="cf-maint-btn cf-maint-warn"
                        data-action="clear-history"
                        { "Clear conversation history" }
                    button type="button" class="cf-maint-btn cf-maint-warn"
                        data-action="reset-persona"
                        { "Reset everything to defaults" }
                }
                p class="cf-maint-note" {
                    "Conversation export downloads a JSON file. Persona export
                     bundles your prompt + voice + tool allowlist + accent for
                     sharing or reimport."
                }
            }
        }
    }
}

const ACCENT_PRESETS: &[&str] = &[
    "#0ea5e9", "#a855f7", "#f97316", "#eab308",
    "#22c55e", "#06b6d4", "#ef4444", "#ec4899",
];

/// Resource Budget pill — the always-visible top-of-back component.
/// Renders an empty skeleton; JS hydrates from /api/compute/state.
pub fn resource_budget_bar(agent_id: &str) -> Markup {
    html! {
        section
            class="syntaur-resource-budget"
            data-agent=(agent_id)
        {
            header class="rb-head" {
                span class="rb-title" { "Resource Budget" }
                span class="rb-pref" {
                    label {
                        input
                            type="radio"
                            name=(format!("rb-pref-{agent_id}"))
                            value="auto"
                            checked;
                        " Auto"
                    }
                    label {
                        input
                            type="radio"
                            name=(format!("rb-pref-{agent_id}"))
                            value="prefer_local";
                        " Prefer Local"
                    }
                    label {
                        input
                            type="radio"
                            name=(format!("rb-pref-{agent_id}"))
                            value="prefer_cloud";
                        " Prefer Cloud"
                    }
                }
            }
            // JS replaces .rb-pools innerHTML on each /api/compute/state
            // tick. The skeleton state ("loading…") is shown until the
            // first response lands.
            div class="rb-pools" data-state="loading" {
                div class="rb-pool rb-pool-skel" { "Loading hardware…" }
            }
            // Conflict drawer — hidden until check_conflict() returns warnings.
            div class="rb-conflicts" hidden {}
        }
    }
}

/// CSS for the bar — kept here so it ships with the markup. Loaded once
/// per page via the (resource_budget_styles) helper.
pub fn resource_budget_styles() -> Markup {
    html! {
        style class="syntaur-page" { (PreEscaped(STYLES)) }
    }
}

/// JS — hydrates + live-updates every Resource Budget bar on the page.
/// Idempotency-guarded so SPA-revisits don't double-bind.
pub fn resource_budget_script() -> Markup {
    html! {
        script class="syntaur-page" { (PreEscaped(RESOURCE_BUDGET_JS)) }
    }
}

const STYLES: &str = r#"
/* ── Card flip wrapper ──────────────────────────────────────────── */
/* /chat's server-side wrapper. Just gives the cog a positioning context
   without moving any chat content into a sub-face. */
.chat-cog-host {
  position: relative;
  width: 100%;
  height: 100%;
  display: contents;
}
/* Side-panel cogs require a relative parent for the absolute cog. The
   JS auto-mounter sets `position: relative` on each registered panel
   only if its computed position is `static` — preserves any panel that
   already has fixed/absolute/relative positioning. */
[data-syntaur-cog-host] { position: relative; }

/* ─── Slide-over agent settings drawer (singleton, lives in shell) ───
   Cog click anywhere on the page opens this. Holds the back-of-card
   markup fetched lazily from /api/agents/{id}/settings_back. */
.syntaur-agent-overlay {
  position: fixed;
  inset: 0;
  z-index: 9000;
  pointer-events: none;
}
.syntaur-agent-overlay[data-open="true"] { pointer-events: auto; }
.syntaur-agent-overlay .sao-scrim {
  position: absolute;
  inset: 0;
  background: rgba(8, 10, 14, 0.55);
  opacity: 0;
  transition: opacity 240ms ease-out;
}
.syntaur-agent-overlay[data-open="true"] .sao-scrim { opacity: 1; }
.syntaur-agent-overlay .sao-drawer {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(720px, 92vw);
  background: #0c0e10;
  color: #ccd;
  border-left: 1px solid rgba(255,255,255,0.08);
  box-shadow: -8px 0 32px rgba(0,0,0,0.4);
  display: flex;
  flex-direction: column;
  transform: translateX(100%);
  transition: transform 280ms cubic-bezier(.4,.05,.2,1);
  overflow: hidden;
}
.syntaur-agent-overlay[data-open="true"] .sao-drawer {
  transform: translateX(0);
}
.syntaur-agent-overlay .sao-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-bottom: 1px solid rgba(255,255,255,0.06);
}
.syntaur-agent-overlay .sao-title {
  flex: 1;
  font-size: 13px;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: #aab;
  margin: 0;
}
.syntaur-agent-overlay .sao-close {
  background: rgba(255,255,255,0.05);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 6px;
  padding: 5px 8px;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.syntaur-agent-overlay .sao-close:hover {
  background: rgba(255,255,255,0.1);
}
.syntaur-agent-overlay .sao-body {
  flex: 1;
  overflow-y: auto;
  padding: 14px 16px;
}
.syntaur-agent-overlay .sao-skel {
  padding: 30px;
  color: #889;
  font-style: italic;
  text-align: center;
}
@media (prefers-reduced-motion: reduce) {
  .syntaur-agent-overlay .sao-drawer,
  .syntaur-agent-overlay .sao-scrim { transition: none; }
}

/* Small, theme-matching gear that lives next to the agent name in each
   chat surface's header. Default styling is intentionally chrome-free
   so the host header's color/background dominates — `currentColor` lets
   each module (scheduler-sage, coders-phosphor, knowledge-sepia, etc.)
   tint the gear without per-module CSS overrides. */
.cf-cog {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px; height: 22px;
  padding: 0; margin: 0;
  background: transparent;
  color: currentColor;
  opacity: 0.55;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  flex-shrink: 0;
  transition: opacity 120ms, background 120ms;
  vertical-align: middle;
}
.cf-cog:hover { opacity: 1; background: rgba(127,127,127,0.14); }
.cf-cog:focus-visible { opacity: 1; outline: 1.5px solid currentColor; outline-offset: 1px; }
.cf-cog svg { display: block; flex-shrink: 0; }

/* When the auto-mounter cannot find a header element to tuck the cog
   into, it lands at the panel's top-RIGHT corner (was top-left, which
   collided with avatars + agent names in every module). */
.cf-cog-fallback {
  position: absolute;
  top: 6px;
  right: 8px;
  z-index: 6;
}
.chat-card-flip[data-flipped="true"] .cf-cog { opacity: 0; }

/* Chat-input button row — three buttons (Attach + PTT + Talk-Mode)
   injected next to each surface's send button by the JS SEND_REGISTRY.
   Flows inline alongside the send button so the trio visually belongs
   to the input row. /chat is skipped — it has its own server-rendered
   icons. */
.cf-row-buttons {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-right: 6px;
  flex-shrink: 0;
}
.cf-rowbtn {
  background: transparent;
  color: #98a;
  border: 1px solid rgba(255,255,255,0.06);
  border-radius: 8px;
  width: 30px;
  height: 30px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: color 120ms, background 120ms, border-color 120ms;
  padding: 0;
}
.cf-rowbtn:hover {
  color: #fff;
  background: rgba(255,255,255,0.06);
  border-color: rgba(255,255,255,0.12);
}
.cf-rowbtn.cf-rec {
  color: #ef4444;
  background: rgba(239,68,68,0.1);
  border-color: rgba(239,68,68,0.4);
  animation: cf-pulse 1s infinite;
}
.cf-rowbtn.cf-busy {
  color: #fbbf24;
  border-color: rgba(251,191,36,0.4);
}
@keyframes cf-pulse {
  0%, 100% { box-shadow: 0 0 0 0 rgba(239,68,68,0.4); }
  50%      { box-shadow: 0 0 0 6px rgba(239,68,68,0); }
}

/* Attachment chip strip — sits above the input row when files are
   attached. Each chip carries dataset.path so a future structured
   send can include the path; today it also injects a [attached: …]
   marker into the input value so the user sees the file is riding
   along. */
.cf-chip-strip {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 4px 8px 6px;
}
.cf-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border-radius: 12px;
  background: rgba(99,102,241,0.18);
  color: #c7d2fe;
  font-size: 12px;
  line-height: 1.3;
  border: 1px solid rgba(99,102,241,0.3);
}
.cf-chip-x {
  background: none;
  border: none;
  color: #c7d2fe;
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  padding: 0 0 0 4px;
}
.cf-chip-x:hover { color: #fff; }

/* Talk-mode overlay — singleton, full-screen, breathing orb */
#cf-talk-overlay {
  position: fixed;
  inset: 0;
  display: none;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  background: rgba(15,23,42,0.95);
  z-index: 9999;
  backdrop-filter: blur(20px);
  font: 14px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif;
  color: #cbd5e1;
}
#cf-talk-close {
  position: absolute;
  top: 24px;
  right: 24px;
  background: transparent;
  border: 1px solid #475569;
  color: #cbd5e1;
  border-radius: 8px;
  padding: 8px 16px;
  cursor: pointer;
  font-size: 14px;
}
#cf-talk-close:hover { background: rgba(255,255,255,0.05); }
#cf-talk-orb {
  width: 200px;
  height: 200px;
  border-radius: 50%;
  background: radial-gradient(circle, #6366f1 0%, #3730a3 100%);
  box-shadow: 0 0 80px rgba(99,102,241,0.6);
  transition: transform 0.18s ease, background 0.4s ease, box-shadow 0.4s ease;
}
#cf-talk-orb.cf-orb-thinking {
  background: radial-gradient(circle, #fbbf24 0%, #b45309 100%);
  box-shadow: 0 0 80px rgba(251,191,36,0.5);
}
#cf-talk-orb.cf-orb-replying {
  background: radial-gradient(circle, #34d399 0%, #047857 100%);
  box-shadow: 0 0 80px rgba(52,211,153,0.5);
}
#cf-talk-status {
  margin-top: 32px;
  font-size: 18px;
  color: #cbd5e1;
}
#cf-talk-transcript {
  margin-top: 16px;
  font-size: 14px;
  color: #94a3b8;
  max-width: 600px;
  text-align: center;
  min-height: 40px;
  padding: 0 24px;
}

.cf-back .cf-back-inner {
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 14px 18px;
  background: #0c0e10;
  color: #ccd;
  font: 13px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
}
.cf-back-head {
  display: flex;
  align-items: center;
  gap: 12px;
  border-bottom: 1px solid rgba(255,255,255,0.06);
  padding-bottom: 8px;
  margin-bottom: 4px;
}
.cf-back-title {
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: #aab;
  margin: 0;
  flex: 1;
}
.cf-back-done {
  background: rgba(255,255,255,0.05);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 6px;
  padding: 4px 10px;
  font: inherit;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.cf-back-done:hover { background: rgba(255,255,255,0.1); }
.cf-back-saving {
  font-size: 11px;
  color: #6a8;
  letter-spacing: 0.06em;
}

.cf-section {
  border: 1px solid rgba(255,255,255,0.05);
  border-radius: 8px;
  background: rgba(255,255,255,0.01);
}
.cf-section-head {
  padding: 8px 12px;
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: #889;
  cursor: pointer;
  list-style: none;
  user-select: none;
}
.cf-section-head::-webkit-details-marker { display: none; }
.cf-section-head::before {
  content: "▸";
  display: inline-block;
  width: 12px;
  margin-right: 4px;
  transition: transform 120ms;
}
.cf-section[open] .cf-section-head::before { transform: rotate(90deg); }
.cf-section-body {
  padding: 8px 12px 12px;
  border-top: 1px solid rgba(255,255,255,0.05);
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.cf-section-stub {
  color: #667;
  font-style: italic;
}
.cf-stub-msg a { color: #6af; }

.cf-row {
  display: grid;
  grid-template-columns: 140px 1fr;
  align-items: center;
  gap: 8px;
}
.cf-row label {
  color: #889;
  font-size: 12px;
}
.cf-row input[type=text],
.cf-row input[type=color] {
  width: 100%;
  background: rgba(255,255,255,0.04);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 5px;
  padding: 5px 8px;
  font: inherit;
}
.cf-row input[type=color] {
  width: 36px;
  height: 26px;
  padding: 0;
}
.cf-icon-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}
.cf-icon-preview {
  width: 40px;
  height: 40px;
  border-radius: 8px;
  background: linear-gradient(135deg, #4a5568, #2d3748);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 18px;
  color: #fff;
  overflow: hidden;
}
.cf-icon-preview img {
  width: 100%; height: 100%; object-fit: cover;
}
.cf-icon-upload, .cf-icon-clear, .cf-shortcut-clear {
  background: rgba(255,255,255,0.05);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 5px;
  padding: 4px 10px;
  font: inherit;
  cursor: pointer;
}
.cf-icon-upload:hover, .cf-icon-clear:hover, .cf-shortcut-clear:hover {
  background: rgba(255,255,255,0.1);
}
.cf-accent-presets {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.cf-accent-swatch {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid transparent;
  cursor: pointer;
  padding: 0;
}
.cf-accent-swatch[aria-pressed="true"],
.cf-accent-swatch.is-active {
  border-color: #fff;
}
.cf-hint {
  color: #667;
  font-size: 11px;
  font-style: italic;
}
.cf-row-stack {
  grid-template-columns: 1fr;
}
.cf-subhead {
  font-size: 11px;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: #889;
  margin: 8px 0 4px;
}
.cf-toggle {
  display: inline-flex;
  align-items: center;
  cursor: pointer;
  gap: 6px;
  font-size: 12px;
}
.cf-toggle input[type=checkbox] {
  appearance: none;
  width: 30px;
  height: 16px;
  background: rgba(255,255,255,0.1);
  border-radius: 999px;
  position: relative;
  cursor: pointer;
  transition: background 120ms;
}
.cf-toggle input[type=checkbox]:checked {
  background: #4ade80;
}
.cf-toggle input[type=checkbox]::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: #fff;
  transition: left 120ms;
}
.cf-toggle input[type=checkbox]:checked::after {
  left: 16px;
}
.cf-toggle-slider { display: none; } /* style hook reserved; CSS uses ::after */

.cf-radio {
  display: inline-flex;
  align-items: center;
  cursor: pointer;
  margin-right: 12px;
  font-size: 12px;
}
.cf-radio input[type=radio] {
  margin-right: 4px;
}

.cf-slider-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.cf-slider-row input[type=range] {
  flex: 1;
}
.cf-slider-val {
  font-variant-numeric: tabular-nums;
  color: #889;
  font-size: 12px;
  min-width: 60px;
  text-align: right;
}

.cf-input-prefix {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.cf-input-prefix span { color: #889; }
.cf-input-prefix input { flex: 1; }

select[data-field] {
  background: rgba(255,255,255,0.04);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 5px;
  padding: 5px 8px;
  font: inherit;
}
input[type=number][data-field] {
  background: rgba(255,255,255,0.04);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 5px;
  padding: 5px 8px;
  font: inherit;
  width: 100px;
}

/* Brain / Voice priority list rows */
.cf-chain-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.cf-chain-list li {
  display: grid;
  grid-template-columns: auto 50px 1fr auto auto auto;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  background: rgba(255,255,255,0.03);
  border: 1px solid rgba(255,255,255,0.05);
  border-radius: 5px;
  font-size: 12px;
}
.cf-chain-list .cf-chain-grip {
  cursor: grab;
  color: #556;
  user-select: none;
}
.cf-chain-list .cf-chain-where {
  font-size: 10px;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  padding: 1px 5px;
  border-radius: 3px;
  text-align: center;
}
.cf-chain-list .cf-chain-where[data-where="local"] { background: #16a34a33; color: #4ade80; }
.cf-chain-list .cf-chain-where[data-where="lan"]   { background: #2563eb33; color: #60a5fa; }
.cf-chain-list .cf-chain-where[data-where="cloud"] { background: #d9770633; color: #fbbf24; }
.cf-chain-list .cf-chain-name {
  font-family: ui-monospace, Menlo, monospace;
  color: #ccd;
}
.cf-chain-list .cf-chain-dot {
  width: 8px; height: 8px; border-radius: 50%;
  display: inline-block;
}
.cf-chain-list .cf-chain-dot[data-status="closed"] { background: #4ade80; }
.cf-chain-list .cf-chain-dot[data-status="degraded"] { background: #fbbf24; }
.cf-chain-list .cf-chain-dot[data-status="open"]   { background: #ef4444; }
.cf-chain-list .cf-chain-wait {
  width: 50px;
}
.cf-chain-list .cf-chain-remove {
  background: none;
  border: none;
  color: #667;
  cursor: pointer;
}
.cf-chain-list .cf-chain-remove:hover { color: #ef4444; }
.cf-chain-add {
  background: rgba(255,255,255,0.04);
  color: #aab;
  border: 1px dashed rgba(255,255,255,0.15);
  border-radius: 5px;
  padding: 6px 12px;
  font: inherit;
  cursor: pointer;
  align-self: flex-start;
}
.cf-chain-add:hover { color: #fff; background: rgba(255,255,255,0.08); }
.cf-chain-skel { color: #667; font-style: italic; padding: 6px 8px; }
.cf-chain-preview {
  margin: 6px 0 0;
  font-size: 11px;
  color: #889;
  font-style: italic;
}
.cf-active-model { color: #4ade80; font-family: ui-monospace, Menlo, monospace; font-size: 12px; }

/* Persona */
.cf-persona-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  margin-bottom: 6px;
}
.cf-persona-toolbar button {
  background: rgba(255,255,255,0.05);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 5px;
  padding: 3px 10px;
  font: inherit;
  cursor: pointer;
  font-size: 11px;
}
.cf-persona-toolbar button:hover { background: rgba(255,255,255,0.1); }
.cf-persona-text {
  width: 100%;
  background: rgba(0,0,0,0.3);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 5px;
  padding: 8px;
  font: 12px ui-monospace, Menlo, monospace;
  resize: vertical;
}
.cf-persona-vars h4 {
  margin: 0 0 4px;
  font-size: 11px;
  color: #889;
  text-transform: uppercase;
  letter-spacing: 0.06em;
}
.cf-var-list {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  list-style: none;
  margin: 0;
  padding: 0;
}
.cf-var-list li {
  background: rgba(255,255,255,0.04);
  padding: 2px 6px;
  border-radius: 3px;
  font: 11px ui-monospace, Menlo, monospace;
  color: #aab;
}

/* Tools grid */
.cf-tools-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.cf-tools-counter {
  font-size: 11px;
  color: #889;
}
.cf-tools-search {
  flex: 1;
  background: rgba(255,255,255,0.04);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 5px;
  padding: 4px 8px;
  font: inherit;
  font-size: 12px;
}
.cf-tools-test, .cf-voice-sample {
  background: rgba(74, 222, 128, 0.1);
  color: #4ade80;
  border: 1px solid rgba(74, 222, 128, 0.3);
  border-radius: 5px;
  padding: 4px 12px;
  font: inherit;
  cursor: pointer;
  font-size: 12px;
}
.cf-tools-test:hover, .cf-voice-sample:hover {
  background: rgba(74, 222, 128, 0.2);
}
.cf-tools-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 4px;
  max-height: 280px;
  overflow-y: auto;
  padding: 4px;
  border: 1px solid rgba(255,255,255,0.05);
  border-radius: 5px;
}
.cf-tools-skel { color: #667; font-style: italic; padding: 12px; grid-column: 1/-1; text-align: center; }
.cf-tools-grid label.cf-tool {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 6px;
  border-radius: 3px;
  font-size: 12px;
  cursor: pointer;
}
.cf-tools-grid label.cf-tool:hover { background: rgba(255,255,255,0.04); }
.cf-tools-grid label.cf-tool.disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* Maintenance */
.cf-maint-row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.cf-maint-btn {
  background: rgba(255,255,255,0.04);
  color: #ccd;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 5px;
  padding: 5px 12px;
  font: inherit;
  cursor: pointer;
  font-size: 12px;
}
.cf-maint-btn:hover { background: rgba(255,255,255,0.08); }
.cf-maint-warn {
  background: rgba(239, 68, 68, 0.08);
  color: #fca5a5;
  border-color: rgba(239, 68, 68, 0.3);
}
.cf-maint-warn:hover {
  background: rgba(239, 68, 68, 0.15);
}
.cf-maint-note {
  margin: 6px 0 0;
  font-size: 11px;
  color: #667;
  font-style: italic;
}

/* ── Resource Budget bar ─────────────────────────────────────────── */
.syntaur-resource-budget {
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 10px;
  padding: 10px 12px;
  background: rgba(255,255,255,0.02);
  font: 12px/1.4 ui-monospace, Menlo, monospace;
  color: #ccd;
  position: sticky;
  top: 0;
  z-index: 5;
  backdrop-filter: blur(8px);
}
.syntaur-resource-budget .rb-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}
.syntaur-resource-budget .rb-title {
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: #9aa;
}
.syntaur-resource-budget .rb-pref {
  display: inline-flex;
  gap: 6px;
  font-size: 11px;
}
.syntaur-resource-budget .rb-pref label {
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  background: rgba(255,255,255,0.04);
}
.syntaur-resource-budget .rb-pref input[type=radio] {
  margin: 0 4px 0 0;
  vertical-align: middle;
}
.syntaur-resource-budget .rb-pools {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.syntaur-resource-budget .rb-pool {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 6px;
  background: rgba(255,255,255,0.04);
  border: 1px solid rgba(255,255,255,0.06);
  flex: 0 1 auto;
  min-width: 200px;
}
.syntaur-resource-budget .rb-pool-name {
  font-weight: 600;
  color: #fff;
}
.syntaur-resource-budget .rb-pool-meter {
  display: inline-block;
  width: 80px;
  height: 6px;
  background: rgba(255,255,255,0.08);
  border-radius: 3px;
  overflow: hidden;
  position: relative;
}
.syntaur-resource-budget .rb-pool-meter-fill {
  position: absolute;
  inset: 0 auto 0 0;
  background: linear-gradient(90deg, #4ade80, #22c55e);
  transition: width 240ms ease-out, background 240ms ease-out;
}
.syntaur-resource-budget .rb-pool[data-overflow="true"] .rb-pool-meter-fill {
  background: linear-gradient(90deg, #fbbf24, #f59e0b);
}
.syntaur-resource-budget .rb-pool[data-overflow="true"] {
  border-color: rgba(245, 158, 11, 0.5);
}
.syntaur-resource-budget .rb-pool-figs {
  font-variant-numeric: tabular-nums;
  color: #ccd;
}
.syntaur-resource-budget .rb-pool-skel {
  color: #889;
  font-style: italic;
}
.syntaur-resource-budget .rb-conflicts {
  margin-top: 8px;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(245, 158, 11, 0.08);
  border: 1px solid rgba(245, 158, 11, 0.3);
}
.syntaur-resource-budget .rb-conflict {
  display: block;
  margin-bottom: 4px;
  color: #fbbf24;
}
.syntaur-resource-budget .rb-conflict-suggest {
  margin-left: 12px;
  margin-top: 4px;
}
.syntaur-resource-budget .rb-conflict-suggest button {
  background: rgba(255,255,255,0.05);
  border: 1px solid rgba(255,255,255,0.1);
  color: #ccd;
  font: inherit;
  padding: 3px 8px;
  border-radius: 4px;
  cursor: pointer;
  margin: 2px 4px 2px 0;
}
.syntaur-resource-budget .rb-conflict-suggest button:hover {
  background: rgba(255,255,255,0.1);
}
"#;

const RESOURCE_BUDGET_JS: &str = r#"
(function () {
  // Idempotency: SPA navigation re-injects scripts; we don't want to spawn
  // a second poll loop over the same bars OR rebind flip handlers.
  if (window.__syntaurResourceBudgetLoaded) return;
  window.__syntaurResourceBudgetLoaded = true;

  // ── Slide-over agent settings overlay controller ────────────────────
  // Single overlay element lives in the shell. Cog click → fetch the
  // back-of-card markup for that agent, paint into the overlay body,
  // open. ESC / scrim / close button → close. Pre-Phase 8 we wrapped
  // the chat surface in a flip container, but that broke the layout
  // of fixed-position panels (drawer, rail). Slide-over is non-invasive
  // — every chat panel just gets a cog appended, click goes through
  // this controller.

  let overlayOpenedFromCog = null;
  async function openAgentOverlay(agentId, returnFocusTo) {
    const overlay = document.getElementById('syntaur-agent-overlay');
    if (!overlay) return;
    overlayOpenedFromCog = returnFocusTo || null;
    const body = overlay.querySelector('[data-bind="overlay-body"]');
    const titleEl = overlay.querySelector('[data-bind="overlay-agent"]');
    if (titleEl) titleEl.textContent = agentId;
    if (body && body.dataset.agent !== agentId) {
      body.dataset.agent = agentId;
      body.innerHTML = '<div class="sao-skel">Loading…</div>';
      try {
        const r = await fetch('/api/agents/' + encodeURIComponent(agentId) + '/settings_back', {
          credentials: 'same-origin',
          headers: { 'Accept': 'text/html' },
        });
        if (r.ok) body.innerHTML = await r.text();
        else body.innerHTML = '<div class="sao-skel">Settings unavailable.</div>';
      } catch (_) {
        body.innerHTML = '<div class="sao-skel">Settings unavailable.</div>';
      }
    }
    overlay.dataset.open = 'true';
    overlay.setAttribute('aria-hidden', 'false');
    window.dispatchEvent(new CustomEvent('syntaur:agent-overlay-open',
      { detail: { agent: agentId } }));
    setTimeout(() => {
      const focusable = overlay.querySelector('input, button, select, textarea');
      if (focusable) focusable.focus();
    }, 200);
  }
  function closeAgentOverlay() {
    const overlay = document.getElementById('syntaur-agent-overlay');
    if (!overlay) return;
    overlay.dataset.open = 'false';
    overlay.setAttribute('aria-hidden', 'true');
    if (overlayOpenedFromCog && overlayOpenedFromCog.focus) {
      overlayOpenedFromCog.focus();
    }
    overlayOpenedFromCog = null;
  }

  document.addEventListener('click', (ev) => {
    const cog = ev.target.closest('.cf-cog');
    if (cog) {
      ev.preventDefault();
      const agent = cog.dataset.agent
        || (cog.closest('[data-agent]') && cog.closest('[data-agent]').dataset.agent)
        || 'main';
      openAgentOverlay(agent, cog);
      return;
    }
    const closer = ev.target.closest('[data-action="close-overlay"]');
    if (closer) {
      ev.preventDefault();
      closeAgentOverlay();
      return;
    }
    const back_done = ev.target.closest('.cf-back-done');
    if (back_done) {
      ev.preventDefault();
      closeAgentOverlay();
      return;
    }
  });

  // ── Chat input button row (Attach + PTT + Talk-Mode) ────────────────
  // Three buttons inject next to each surface's send button. /chat has
  // its own equivalents server-rendered (#send-btn) — we skip it here.
  // The TTS-on-reply toggle moved to the cog drawer (Voice section);
  // the inline mic icon is gone.
  //   📎 cf-attach    — file picker → POST /api/upload → chip strip
  //   🎙️ cf-ptt       — push-to-talk → /api/voice/transcribe → input
  //   💬 cf-talk-mode — overlay with breathing orb + VAD loop
  const SEND_REGISTRY = [
    { selector: '#cortex-send-btn',    agent: 'cortex'   },  // /knowledge
    { selector: '#sch-thad-send',      agent: 'thaddeus' },  // /scheduler
    { selector: '.j-mushi-send',       agent: 'mushi'    },  // /journal
    { selector: '.sd-chat-send',       agent: 'main'     },  // /dashboard widget
  ];
  const INPUT_ROW_REGISTRY = [
    { selector: '.ai-input-row',       agent: 'maurice'  },  // /coders Maurice
  ];

  function _findInputFor(anchor) {
    let el = anchor;
    for (let i = 0; i < 5 && el; i++) {
      el = el.parentElement;
      if (!el) break;
      const ta = el.querySelector('textarea, input[type="text"], input:not([type])');
      if (ta) return ta;
    }
    return null;
  }

  function _triggerSend(sendBtn, input) {
    if (sendBtn) {
      sendBtn.click();
    } else if (input) {
      input.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Enter', code: 'Enter', bubbles: true, cancelable: true
      }));
    }
  }

  function _attachClick(agent, anchor) {
    const fid = '__cf_attach_' + agent;
    let fi = document.getElementById(fid);
    if (!fi) {
      fi = document.createElement('input');
      fi.type = 'file';
      fi.id = fid;
      fi.multiple = true;
      fi.accept = 'image/*,application/pdf,.pdf,.txt,.md,.csv,.json,.docx,.xlsx,.pptx,audio/*';
      fi.style.display = 'none';
      document.body.appendChild(fi);
    }
    const handler = async (e) => {
      const files = Array.from(e.target.files || []);
      e.target.value = '';
      for (const f of files) await _uploadOne(f, agent, anchor);
      fi.removeEventListener('change', handler);
    };
    fi.addEventListener('change', handler);
    fi.click();
  }

  async function _uploadOne(file, agent, anchor) {
    const fd = new FormData();
    fd.append('file', file, file.name);
    const tok = sessionStorage.getItem('syntaur_token') || '';
    try {
      const r = await fetch('/api/upload', {
        method: 'POST', body: fd,
        headers: tok ? { 'Authorization': 'Bearer ' + tok } : {}
      });
      const j = await r.json();
      if (!j.success) {
        console.error('[attach] upload failed:', j.error);
        return;
      }
      _addAttachmentChip(anchor, j, agent);
    } catch (err) {
      console.error('[attach] upload error:', err);
    }
  }

  function _addAttachmentChip(anchor, file, agent) {
    const row = anchor.closest(
      '.cortex-input-row, .sch-thad-input-row, .j-mushi-input-row, .sd-chat-form, .ai-input-row, .flex'
    ) || anchor.parentElement;
    if (!row) return;
    let strip = row.previousElementSibling;
    if (!strip || !strip.classList || !strip.classList.contains('cf-chip-strip')) {
      strip = document.createElement('div');
      strip.className = 'cf-chip-strip';
      strip.dataset.agent = agent;
      row.parentNode.insertBefore(strip, row);
    }
    const chip = document.createElement('span');
    chip.className = 'cf-chip';
    chip.dataset.path = file.path || '';
    chip.dataset.filename = file.filename || '';
    const icon = (file.content_type || '').startsWith('image/') ? '🖼' : '📄';
    chip.textContent = icon + ' ' + (file.filename || 'file') + ' ';
    const x = document.createElement('button');
    x.type = 'button';
    x.className = 'cf-chip-x';
    x.textContent = '×';
    x.addEventListener('click', () => {
      const input = _findInputFor(anchor);
      if (input) {
        const marker = '\n[attached: ' + (file.filename || 'file') + ']';
        if (input.value.includes(marker)) {
          input.value = input.value.replace(marker, '');
          input.dispatchEvent(new Event('input', { bubbles: true }));
        }
      }
      chip.remove();
    });
    chip.appendChild(x);
    strip.appendChild(chip);
    // Surface the attachment in the input so the user knows it's
    // riding along with the next message. Path is stored on the chip
    // for future structured-attachment send-flow upgrades.
    const input = _findInputFor(anchor);
    if (input) {
      const marker = '\n[attached: ' + (file.filename || 'file') + ']';
      if (!input.value.includes(marker)) {
        input.value = (input.value || '') + marker;
        input.dispatchEvent(new Event('input', { bubbles: true }));
      }
    }
  }

  // ── Mic preflight: getUserMedia requires a secure context ───────────
  // Plain HTTP over LAN (192.168.1.x:18789) silently fails getUserMedia
  // with NotAllowedError or "Permission denied" — there is no permission
  // *prompt* the user can accept on insecure origins. We catch this case
  // BEFORE asking for the mic so the user sees an actionable message
  // ("you're on http://; reach this gateway over Tailscale or HTTPS")
  // instead of a useless "Microphone permission denied" alert.
  function _micSecurityNote() {
    if (window.isSecureContext) return null;
    const host = location.hostname;
    return (
      'Microphone access needs a secure connection. You\'re on ' +
      location.protocol + '//' + host + ' — browsers only release the ' +
      'mic over HTTPS or localhost. Open Syntaur via your Tailscale ' +
      'name (e.g. https://syntaur.<tailnet>.ts.net) or set up the ' +
      'gateway\'s self-signed cert (Settings → Privacy → Trust local cert).'
    );
  }
  function _micErrorAlert(e) {
    const note = _micSecurityNote();
    if (note) { alert(note); return; }
    const msg = (e && (e.message || e.name)) ? (e.message || e.name) : String(e);
    alert('Microphone unavailable: ' + msg);
  }

  // ── Push-to-talk: tap → record → tap-stop → STT → input ─────────────
  async function _pttClick(btn, agent, sendBtn) {
    if (btn._stopFn) { btn._stopFn(); return; }
    const note = _micSecurityNote();
    if (note) { alert(note); return; }
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      _micErrorAlert(e);
      return;
    }
    const mr = new MediaRecorder(stream, { mimeType: 'audio/webm' });
    const chunks = [];
    mr.ondataavailable = (e) => e.data.size && chunks.push(e.data);
    btn.classList.add('cf-rec');
    btn.setAttribute('aria-pressed', 'true');
    let stopped = false;
    const finish = async () => {
      if (stopped) return;
      stopped = true;
      btn._stopFn = null;
      try { mr.stop(); } catch (_) {}
      stream.getTracks().forEach(t => { try { t.stop(); } catch (_) {} });
      btn.classList.remove('cf-rec');
      btn.setAttribute('aria-pressed', 'false');
      btn.classList.add('cf-busy');
      await new Promise(r => mr.addEventListener('stop', r, { once: true }));
      if (chunks.length) {
        const blob = new Blob(chunks, { type: 'audio/webm' });
        const fd = new FormData();
        fd.append('audio', blob, 'audio.webm');
        fd.append('token', sessionStorage.getItem('syntaur_token') || '');
        try {
          const r = await fetch('/api/voice/transcribe', { method: 'POST', body: fd });
          const j = await r.json();
          const text = (j.text || '').trim();
          if (text) {
            const input = _findInputFor(sendBtn || btn);
            if (input) {
              const sep = (input.value && !/\s$/.test(input.value)) ? ' ' : '';
              input.value = (input.value || '') + sep + text;
              input.dispatchEvent(new Event('input', { bubbles: true }));
              input.focus();
            }
          }
        } catch (e) { console.error('[ptt] STT failed:', e); }
      }
      btn.classList.remove('cf-busy');
    };
    btn._stopFn = finish;
    mr.start();
    setTimeout(() => { if (!stopped) finish(); }, 60000);
  }

  // ── Talk Mode: overlay with breathing orb + VAD loop ─────────────────
  let _talkActive = false;
  function _talkExit() {
    _talkActive = false;
    const overlay = document.getElementById('cf-talk-overlay');
    if (overlay) overlay.style.display = 'none';
  }
  function _talkClick(btn, agent, sendBtn) {
    if (_talkActive) { _talkExit(); return; }
    const note = _micSecurityNote();
    if (note) { alert(note); return; }
    let overlay = document.getElementById('cf-talk-overlay');
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.id = 'cf-talk-overlay';
      overlay.innerHTML =
        '<button id="cf-talk-close" type="button">✕ Exit</button>' +
        '<div id="cf-talk-orb"></div>' +
        '<div id="cf-talk-status">Listening…</div>' +
        '<div id="cf-talk-transcript"></div>';
      document.body.appendChild(overlay);
      document.getElementById('cf-talk-close').addEventListener('click', _talkExit);
    }
    overlay.dataset.agent = agent;
    overlay.style.display = 'flex';
    _talkLoop(agent, sendBtn);
  }
  async function _talkLoop(agent, sendBtn) {
    _talkActive = true;
    const orb = document.getElementById('cf-talk-orb');
    const status = document.getElementById('cf-talk-status');
    const transcript = document.getElementById('cf-talk-transcript');
    while (_talkActive) {
      status.textContent = 'Listening…';
      orb.className = '';
      transcript.textContent = '';
      let stream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch (e) {
        _micErrorAlert(e);
        _talkExit();
        return;
      }
      const mr = new MediaRecorder(stream, { mimeType: 'audio/webm' });
      const chunks = [];
      mr.ondataavailable = (e) => e.data.size && chunks.push(e.data);
      const ac = new (window.AudioContext || window.webkitAudioContext)();
      const src = ac.createMediaStreamSource(stream);
      const an = ac.createAnalyser();
      an.fftSize = 1024;
      src.connect(an);
      const buf = new Uint8Array(an.frequencyBinCount);
      let silentMs = 0;
      let speechSeen = false;
      const startedAt = Date.now();
      mr.start(100);
      await new Promise(resolve => {
        const tick = () => {
          if (!_talkActive) { resolve(); return; }
          an.getByteFrequencyData(buf);
          let sum = 0;
          for (let i = 0; i < buf.length; i++) sum += buf[i];
          const avg = sum / buf.length;
          if (avg > 14) {
            speechSeen = true;
            silentMs = 0;
            const scale = 1 + Math.min(avg / 200, 0.6);
            orb.style.transform = 'scale(' + scale.toFixed(2) + ')';
          } else if (speechSeen) {
            silentMs += 100;
            orb.style.transform = 'scale(1)';
          }
          const elapsed = Date.now() - startedAt;
          if ((speechSeen && silentMs > 1500) || elapsed > 30000) resolve();
          else setTimeout(tick, 100);
        };
        tick();
      });
      try { mr.stop(); } catch (_) {}
      stream.getTracks().forEach(t => { try { t.stop(); } catch (_) {} });
      try { ac.close(); } catch (_) {}
      if (!_talkActive) return;
      if (!speechSeen || !chunks.length) continue;
      status.textContent = 'Thinking…';
      orb.className = 'cf-orb-thinking';
      const blob = new Blob(chunks, { type: 'audio/webm' });
      const fd = new FormData();
      fd.append('audio', blob, 'audio.webm');
      fd.append('token', sessionStorage.getItem('syntaur_token') || '');
      let text = '';
      try {
        const r = await fetch('/api/voice/transcribe', { method: 'POST', body: fd });
        const j = await r.json();
        text = (j.text || '').trim();
      } catch (e) { text = ''; }
      if (!text) continue;
      transcript.textContent = '"' + text + '"';
      const input = _findInputFor(sendBtn);
      if (!input) {
        status.textContent = 'No input found — exiting';
        await new Promise(r => setTimeout(r, 1500));
        _talkExit();
        return;
      }
      input.value = text;
      input.dispatchEvent(new Event('input', { bubbles: true }));
      // Force TTS-on-reply ON during the conversation so the user
      // hears the reply; restore prior pref when the reply window closes.
      const ttsKey = 'syntaur:tts:' + agent;
      const prevTts = localStorage.getItem(ttsKey);
      localStorage.setItem(ttsKey, 'on');
      _triggerSend(sendBtn, input);
      status.textContent = 'Replying…';
      orb.className = 'cf-orb-replying';
      await new Promise(r => setTimeout(r, 5000));
      if (prevTts !== 'on') localStorage.setItem(ttsKey, prevTts || 'off');
    }
  }

  // ── Build button row + injection wiring ──────────────────────────────
  function _mkBtn(cls, title, agent, svg, fn) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'cf-rowbtn ' + cls;
    b.dataset.agent = agent;
    b.title = title;
    b.setAttribute('aria-label', title);
    b.innerHTML = svg;
    b.addEventListener('click', (e) => {
      e.preventDefault(); e.stopPropagation(); fn(b);
    });
    return b;
  }
  function _buildButtonRow(agent, sendBtn) {
    const wrap = document.createElement('span');
    wrap.className = 'cf-row-buttons';
    wrap.dataset.agent = agent;
    const SVG_CLIP = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/></svg>';
    const SVG_MIC  = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>';
    const SVG_TALK = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z"/></svg>';
    wrap.appendChild(_mkBtn('cf-attach',    'Attach file',                       agent, SVG_CLIP, (b) => _attachClick(agent, b)));
    wrap.appendChild(_mkBtn('cf-ptt',       'Voice → text (push to talk)',       agent, SVG_MIC,  (b) => _pttClick(b, agent, sendBtn)));
    wrap.appendChild(_mkBtn('cf-talk-mode', 'Conversation mode (hands-free)',    agent, SVG_TALK, (b) => _talkClick(b, agent, sendBtn)));
    return wrap;
  }
  function injectButtonsNextToSend(sendBtn, agent) {
    if (!sendBtn || !sendBtn.parentNode) return;
    if (sendBtn.id === 'send-btn') return;  // /chat has its own
    if (sendBtn.parentNode.querySelector('.cf-row-buttons[data-agent="' + agent + '"]')) return;
    const wrap = _buildButtonRow(agent, sendBtn);
    sendBtn.parentNode.insertBefore(wrap, sendBtn);
  }
  function injectButtonsAtRowStart(row, agent) {
    if (!row) return;
    if (row.querySelector('.cf-row-buttons[data-agent="' + agent + '"]')) return;
    const wrap = _buildButtonRow(agent, null);
    row.insertBefore(wrap, row.firstChild);
  }
  function injectAllButtons() {
    for (const e of SEND_REGISTRY) {
      document.querySelectorAll(e.selector).forEach(b => injectButtonsNextToSend(b, e.agent));
    }
    for (const e of INPUT_ROW_REGISTRY) {
      document.querySelectorAll(e.selector).forEach(r => injectButtonsAtRowStart(r, e.agent));
    }
  }

  injectAllButtons();
  window.addEventListener('syntaur:page-arrived', () => {
    autoMountSidePanels();
    injectAllButtons();
  });
  if (typeof MutationObserver !== 'undefined') {
    let scheduled = false;
    new MutationObserver((muts) => {
      for (const m of muts) {
        if (m.addedNodes && m.addedNodes.length) {
          if (!scheduled) {
            scheduled = true;
            requestAnimationFrame(() => {
              scheduled = false;
              autoMountSidePanels();
              injectAllButtons();
            });
          }
          return;
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  }

  // ── Auto-mount cog/mic on side-panel chat surfaces ───────────────────
  // /chat is mounted server-side via the chat_card_flip maud helper. The
  // OTHER surfaces (knowledge Cortex panel, scheduler Thad drawer, journal
  // Mushi rail, music Silvr card, coders Maurice tab) embed the chat in
  // raw HTML side panels. Rather than restructure each page's inline
  // markup, we detect known panel selectors at runtime and graft the
  // cog/mic + back-of-card around them. Idempotent — re-running just
  // re-syncs state.
  //
  // Each registry entry says: panel selector → agent id. Pages whose
  // panel doesn't match any selector get no cog (graceful degrade —
  // this means the user can only adjust those agents from /chat).
  // Verified against deployed DOM 2026-04-26 (gateway cb0783a969).
  // Earlier guesses (#sch-thad-chat, .mushi-rail, #silvr-chat, #ai-chat-card)
  // matched zero elements — only /knowledge worked. /music has no AI-reply
  // chat surface today (DJ is request-only, no thread), so it's omitted.
  // PANEL_REGISTRY entry shape:
  //   panel:  CSS selector for the chat surface (used for right-click,
  //           data-agent attribution, and idempotency tracking).
  //   header: CSS selector for the element to tuck the cog INTO so it
  //           sits next to the agent name. Resolution rule:
  //             1. panel.querySelector(header)
  //             2. panel.parentElement.querySelector(header)
  //             3. document.querySelector(header)
  //           If none match, the cog lands at the panel's top-right
  //           corner via .cf-cog-fallback.
  //   agent:  agent_id used by openAgentOverlay.
  //
  // Sean's directive (2026-04-26): cog must sit "in the upper section
  // corner of the chat next to the AI chat's personality name", not at
  // panel root. Each module's chat header is different — header
  // selectors below are verified against the deployed DOM.
  const PANEL_REGISTRY = [
    { panel: '#lib-cortex',      header: '.cortex-header',     agent: 'cortex'   },
    { panel: '#sch-thad-drawer', header: '.sch-thad-head',     agent: 'thaddeus' },
    { panel: '#j-mushi',         header: '.j-mushi-head',      agent: 'mushi'    },
    { panel: '#ai-messages',     header: '.maurice-header',    agent: 'maurice'  },
    { panel: '.sd-chat',         header: '.sd-card-head',      agent: 'main'     },
    { panel: '.positron-panel',  header: '.positron-header',   agent: 'positron' },  // /tax
    { panel: '#soc-chat-rail',   header: '.soc-chat-head',     agent: 'nyota'    },  // /social
  ];

  function autoMountSidePanels() {
    for (const entry of PANEL_REGISTRY) {
      document.querySelectorAll(entry.panel).forEach(panel => {
        attachCogToPanel(panel, entry.agent, entry.header);
      });
    }
  }

  function _findHeader(panel, headerSel) {
    if (!headerSel) return null;
    // Inside-first: most modules tuck their header inside the panel.
    const inner = panel.querySelector(headerSel);
    if (inner) return inner;
    // Walk up the ancestor chain; first ancestor whose querySelector
    // resolves wins. This pins to the OWN card/section rather than
    // grabbing the first match anywhere in the document — important
    // for /dashboard where every widget has its own .sd-card-head and
    // a global lookup would drag the cog onto a different widget.
    let cur = panel.parentElement;
    while (cur) {
      const hit = cur.querySelector(headerSel);
      if (hit) return hit;
      cur = cur.parentElement;
    }
    return null;
  }

  function _makeCog(agent) {
    const cog = document.createElement('button');
    cog.type = 'button';
    cog.className = 'cf-cog';
    cog.dataset.agent = agent;
    cog.setAttribute('aria-label', 'Agent settings for ' + agent);
    cog.setAttribute('aria-pressed', 'false');
    cog.title = 'Agent settings';
    cog.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M19.43 12.98c.04-.32.07-.64.07-.98s-.03-.66-.07-.98l2.11-1.65c.19-.15.24-.42.12-.64l-2-3.46c-.12-.22-.39-.3-.61-.22l-2.49 1c-.52-.4-1.08-.73-1.69-.98l-.38-2.65C14.46 2.18 14.25 2 14 2h-4c-.25 0-.46.18-.49.42l-.38 2.65c-.61.25-1.17.59-1.69.98l-2.49-1c-.23-.09-.49 0-.61.22l-2 3.46c-.13.22-.07.49.12.64l2.11 1.65c-.04.32-.07.65-.07.98s.03.66.07.98l-2.11 1.65c-.19.15-.24.42-.12.64l2 3.46c.12.22.39.3.61.22l2.49-1c.52.4 1.08.73 1.69.98l.38 2.65c.03.24.24.42.49.42h4c.25 0 .46-.18.49-.42l.38-2.65c.61-.25 1.17-.59 1.69-.98l2.49 1c.23.09.49 0 .61-.22l2-3.46c.12-.22.07-.49-.12-.64l-2.11-1.65zM12 15.5c-1.93 0-3.5-1.57-3.5-3.5s1.57-3.5 3.5-3.5 3.5 1.57 3.5 3.5-1.57 3.5-3.5 3.5z"/></svg>';
    return cog;
  }

  function attachCogToPanel(panel, agent, headerSel) {
    if (!panel) return;
    if (panel.dataset.syntaurCogHost === '1') return; // already mounted
    panel.dataset.syntaurCogHost = '1';
    panel.setAttribute('data-syntaur-cog-host', ''); // CSS hook for right-click bind
    panel.dataset.agent = agent;
    const cog = _makeCog(agent);
    const header = _findHeader(panel, headerSel);
    if (header) {
      // Inline mount: tuck the cog into the header so it sits next to
      // the agent name. When the header already has a close-x as its
      // last child, insert the cog BEFORE close so the close button
      // remains the rightmost affordance (matches conventional
      // header chrome). Single-child headers (e.g. dashboard widget
      // title row) just append. Header inherits its module's color
      // theme; cog uses currentColor so it matches automatically.
      if (header.children.length > 1) {
        header.insertBefore(cog, header.lastElementChild);
      } else {
        header.appendChild(cog);
      }
    } else {
      // Fallback: top-right corner of the panel. Avoid the legacy
      // top-LEFT placement that collided with every module's avatar.
      const cs = getComputedStyle(panel);
      if (cs.position === 'static') panel.style.position = 'relative';
      cog.classList.add('cf-cog-fallback');
      panel.appendChild(cog);
    }
  }

  // Run mount on initial load + after SPA navigations.
  autoMountSidePanels();

  // ── Global TTS-on-reply helper for chat surfaces ────────────────────
  // Pages call window.syntaurMaybeSpeak(text, agentId) after they render
  // an assistant message. If the per-agent mic toggle is on, we POST to
  // /api/tts and play the returned audio (ducking music if any). If off,
  // no-op. Single in-flight playback per page; new replies wait for the
  // current one to finish.
  let speakQueue = Promise.resolve();
  window.syntaurMaybeSpeak = function (text, agentId) {
    const id = agentId || 'main';
    if (localStorage.getItem('syntaur:tts:' + id) !== 'on') return;
    if (!text || !text.trim()) return;
    // Trim very-long replies to a sensible TTS budget (the existing
    // /api/tts handler already truncates, but a tighter cap here keeps
    // sample latency low).
    const t = text.length > 1500 ? text.slice(0, 1500) + '…' : text;
    speakQueue = speakQueue.then(async () => {
      try {
        const r = await fetch('/api/tts', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ text: t, agent_id: id }),
        });
        if (!r.ok) return;
        const j = await r.json();
        if (!j || !j.audio_url) return;
        await new Promise((resolve) => {
          const a = new Audio(j.audio_url);
          a.onended = () => resolve();
          a.onerror = () => resolve();
          // Best-effort music-duck. The endpoint is idempotent.
          fetch('/api/music/duck', {
            method: 'POST', credentials: 'same-origin',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ state: 'on', duration_secs: (j.estimated_duration_secs || 10) + 5 }),
          }).catch(() => {});
          a.play().catch(() => resolve());
        });
        fetch('/api/music/duck', {
          method: 'POST', credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ state: 'off' }),
        }).catch(() => {});
      } catch (_) {}
    });
  };

  document.addEventListener('keydown', (ev) => {
    if (ev.key !== 'Escape') return;
    const overlay = document.getElementById('syntaur-agent-overlay');
    if (overlay && overlay.dataset.open === 'true') {
      ev.preventDefault();
      closeAgentOverlay();
    }
  });

  // ── Phase 9 polish: right-click on a chat panel opens settings ─────
  // Power-user shortcut: right-click on a registered chat panel opens
  // the agent's settings overlay without needing to aim at the cog.
  // We intercept only the dead chrome of the panel — text selections,
  // links, inputs, and buttons keep the native context menu.
  document.addEventListener('contextmenu', (ev) => {
    const host = ev.target.closest && ev.target.closest('[data-syntaur-cog-host]');
    if (!host) return;
    const tag = (ev.target.tagName || '').toLowerCase();
    if (['a','input','textarea','button'].includes(tag)) return;
    const sel = window.getSelection && window.getSelection().toString();
    if (sel && sel.length > 0) return;
    ev.preventDefault();
    openAgentOverlay(host.dataset.agent || 'main', null);
  });

  // ── Identity section auto-save (Phase 1) ────────────────────────────
  // PUT /api/agents/{id}/settings with a partial-merge payload on blur
  // (or change for radio/color/file). Keeps the back panel "no Save
  // button" — every change persists immediately. The little "Saving…"
  // indicator in the back's header pulses while the request is in flight.

  function getCsrfHeaders() {
    return { 'Content-Type': 'application/json' };
  }
  function showSavingIndicator(card, on) {
    const ind = card.querySelector('.cf-back-saving');
    if (ind) ind.hidden = !on;
  }
  async function persistField(card, field, value) {
    const agent = card.dataset.agent;
    if (!agent) return;
    showSavingIndicator(card, true);
    const payload = {}; payload[field] = value;
    try {
      await fetch('/api/agents/' + encodeURIComponent(agent) + '/settings', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: getCsrfHeaders(),
        body: JSON.stringify(payload),
      });
    } catch (e) { /* eat — UI is auto-save best-effort */ }
    finally { showSavingIndicator(card, false); }
  }

  document.addEventListener('change', (ev) => {
    const back = ev.target.closest('.cf-back');
    if (!back) return;
    const card = back.closest('.chat-card-flip');
    if (!card) return;

    const field = ev.target.dataset.field;
    if (!field) return;

    if (ev.target.type === 'file') {
      // Icon upload uses POST /api/agents/{id}/icon multipart, then writes
      // the returned blob_id back via PUT settings.
      const file = ev.target.files && ev.target.files[0];
      if (!file) return;
      const fd = new FormData();
      fd.append('icon', file);
      showSavingIndicator(card, true);
      fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/icon', {
        method: 'POST', credentials: 'same-origin', body: fd,
      }).then(r => r.ok ? r.json() : Promise.reject(r))
        .then(j => persistField(card, 'icon_blob_id', j.blob_id))
        .catch(() => {})
        .finally(() => showSavingIndicator(card, false));
      return;
    }

    if (ev.target.type === 'color') {
      persistField(card, 'accent_color', ev.target.value);
      // Mark the matching preset (if any) as active.
      back.querySelectorAll('.cf-accent-swatch').forEach(s =>
        s.classList.toggle('is-active', s.dataset.accent === ev.target.value)
      );
      return;
    }

    persistField(card, field, ev.target.value);
  });

  document.addEventListener('blur', (ev) => {
    const back = ev.target.closest('.cf-back');
    if (!back) return;
    const card = back.closest('.chat-card-flip');
    if (!card) return;
    const field = ev.target.dataset.field;
    if (!field) return;
    if (ev.target.type === 'text' || ev.target.tagName === 'TEXTAREA') {
      persistField(card, field, ev.target.value || null);
    }
  }, true);

  // Accent preset clicks → write hex + sync the color picker
  document.addEventListener('click', (ev) => {
    const sw = ev.target.closest('.cf-accent-swatch');
    if (!sw) return;
    const back = sw.closest('.cf-back');
    if (!back) return;
    const card = back.closest('.chat-card-flip');
    const hex = sw.dataset.accent;
    back.querySelectorAll('.cf-accent-swatch').forEach(s =>
      s.classList.toggle('is-active', s === sw)
    );
    const colorInput = back.querySelector('.cf-accent-custom');
    if (colorInput) colorInput.value = hex;
    persistField(card, 'accent_color', hex);
  });

  // Shortcut recorder — capture next keydown, encode as combo string.
  document.addEventListener('focus', (ev) => {
    const r = ev.target.closest && ev.target.closest('.cf-shortcut-recorder');
    if (!r) return;
    r.placeholder = 'Press any key combo…';
    r._sci = (kev) => {
      kev.preventDefault();
      const parts = [];
      if (kev.metaKey) parts.push('cmd');
      if (kev.ctrlKey) parts.push('ctrl');
      if (kev.altKey) parts.push('alt');
      if (kev.shiftKey) parts.push('shift');
      const k = kev.key.length === 1 ? kev.key.toLowerCase() : kev.key;
      if (!['Control','Shift','Alt','Meta'].includes(k)) parts.push(k);
      const combo = parts.join('+');
      r.value = combo;
      const card = r.closest('.chat-card-flip');
      persistField(card, 'shortcut', combo || null);
      r.removeEventListener('keydown', r._sci);
    };
    r.addEventListener('keydown', r._sci);
  }, true);

  document.addEventListener('click', (ev) => {
    const clr = ev.target.closest('[data-action="clear-shortcut"]');
    if (clr) {
      const card = clr.closest('.chat-card-flip');
      const r = card.querySelector('.cf-shortcut-recorder');
      if (r) r.value = '';
      persistField(card, 'shortcut', null);
      return;
    }
    const ic = ev.target.closest('[data-action="clear-icon"]');
    if (ic) {
      const card = ic.closest('.chat-card-flip');
      const preview = card.querySelector('.cf-icon-preview');
      if (preview) preview.innerHTML = '<span class="cf-icon-letter">?</span>';
      persistField(card, 'icon_blob_id', null);
      return;
    }
  });

  // ── Slider value display sync ────────────────────────────────────────
  // Each slider has a sibling .cf-slider-val[data-bind=field] that mirrors
  // the current value with a unit suffix derived from the field name.
  function fmtSliderVal(field, raw) {
    const v = parseFloat(raw);
    if (field === 'speaking_rate') return v.toFixed(2) + '×';
    if (field === 'pitch_shift') return (v >= 0 ? '+' : '') + v.toFixed(0) + '%';
    if (field === 'temperature') return v.toFixed(2);
    if (field === 'context_budget') {
      if (v >= 1024) return (v / 1024).toFixed(0) + 'k tokens';
      return v + ' tokens';
    }
    return raw;
  }
  document.addEventListener('input', (ev) => {
    const el = ev.target.closest && ev.target.closest('input[type=range][data-field]');
    if (!el) return;
    const back = el.closest('.cf-back');
    if (!back) return;
    const field = el.dataset.field;
    const out = back.querySelector('.cf-slider-val[data-bind="' + field + '"]');
    if (out) out.textContent = fmtSliderVal(field, el.value);
  });

  // ── TTS-on-reply toggle (cog drawer Voice section) ───────────────────
  document.addEventListener('change', (ev) => {
    const tg = ev.target.closest && ev.target.closest('[data-action="tts-on-reply-toggle"]');
    if (!tg) return;
    const card = tg.closest('.chat-card-flip') || tg.closest('[data-agent]');
    const agent = (card && card.dataset && card.dataset.agent) || 'main';
    const key = 'syntaur:tts:' + agent;
    localStorage.setItem(key, tg.checked ? 'on' : 'off');
  });
  // Sync the toggle's checked state when the overlay opens for a given agent.
  window.addEventListener('syntaur:agent-overlay-open', (ev) => {
    const agent = (ev && ev.detail && ev.detail.agent) || 'main';
    const on = localStorage.getItem('syntaur:tts:' + agent) === 'on';
    document.querySelectorAll('[data-action="tts-on-reply-toggle"]').forEach(cb => {
      cb.checked = on;
    });
  });

  // ── Persona toolbar ──────────────────────────────────────────────────
  document.addEventListener('change', (ev) => {
    const tg = ev.target.closest && ev.target.closest('[data-action="persona-edit-toggle"]');
    if (!tg) return;
    const back = tg.closest('.cf-back');
    const ta = back.querySelector('.cf-persona-text');
    if (!ta) return;
    if (tg.checked) {
      ta.removeAttribute('readonly');
      ta.focus();
    } else {
      ta.setAttribute('readonly', '');
      // Reverting wipes the override — Server treats null = use default.
      const card = back.closest('.chat-card-flip');
      persistField(card, 'persona_prompt_override', null);
      ta.value = ta.dataset.default || '';
    }
  });

  document.addEventListener('click', async (ev) => {
    const card = (ev.target.closest && ev.target.closest('.chat-card-flip'));
    const back = (ev.target.closest && ev.target.closest('.cf-back'));
    if (!back || !card) return;

    const a = ev.target.closest('[data-action]');
    if (!a) return;
    const action = a.dataset.action;

    if (action === 'persona-paste-clean') {
      try {
        const txt = await navigator.clipboard.readText();
        const cleaned = txt
          .replace(/^\s*Hi!?\s*I'?m[^\n]*\n+/i, '')
          .replace(/[“”]/g, '"').replace(/[‘’]/g, "'")
          .replace(/—/g, '-').trim();
        const ta = back.querySelector('.cf-persona-text');
        if (ta) {
          ta.value = cleaned;
          persistField(card, 'persona_prompt_override', cleaned);
        }
      } catch (_) {}
      return;
    }
    if (action === 'persona-test') {
      const ta = back.querySelector('.cf-persona-text');
      if (!ta || !ta.value) { alert('Toggle "Edit" and paste a prompt first.'); return; }
      // One-shot: send "say hi" against the active brain with this prompt.
      try {
        const r = await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/test_prompt', {
          method: 'POST', credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ prompt: ta.value, message: 'say hi' }),
        });
        const j = await r.json();
        alert((j && j.reply) || 'No reply (endpoint pending).');
      } catch (_) { alert('Test failed.'); }
      return;
    }
    if (action === 'persona-reset') {
      if (!confirm('Reset persona prompt to the persona default?')) return;
      const ta = back.querySelector('.cf-persona-text');
      if (ta) ta.value = ta.dataset.default || '';
      persistField(card, 'persona_prompt_override', null);
      return;
    }
    if (action === 'voice-sample') {
      const voiceId = (back.querySelector('select[data-field="voice_id"]') || {}).value || '';
      const rate = (back.querySelector('input[data-field="speaking_rate"]') || {}).value || '1';
      const pitch = (back.querySelector('input[data-field="pitch_shift"]') || {}).value || '0';
      try {
        const r = await fetch('/api/voice/sample', {
          method: 'POST', credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            text: 'Good morning, Sean. Ready when you are.',
            voice_id: voiceId, rate: parseFloat(rate), pitch: parseFloat(pitch),
          }),
        });
        if (!r.ok) { alert('Sample endpoint unavailable.'); return; }
        const blob = await r.blob();
        new Audio(URL.createObjectURL(blob)).play().catch(() => {});
      } catch (_) {}
      return;
    }
    if (action === 'export-conversation') {
      window.open('/api/conversations/active?format=json', '_blank');
      return;
    }
    if (action === 'export-persona') {
      window.open('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/export', '_blank');
      return;
    }
    if (action === 'import-persona') {
      const inp = document.createElement('input');
      inp.type = 'file'; inp.accept = '.json,application/json';
      inp.onchange = async () => {
        const f = inp.files && inp.files[0]; if (!f) return;
        const fd = new FormData(); fd.append('persona', f);
        await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/import', {
          method: 'POST', credentials: 'same-origin', body: fd,
        });
        alert('Persona imported. Reflip to refresh.');
      };
      inp.click();
      return;
    }
    if (action === 'clear-history') {
      if (!confirm('Delete this agent’s entire conversation history?')) return;
      await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/history', {
        method: 'DELETE', credentials: 'same-origin',
      });
      alert('History cleared.');
      return;
    }
    if (action === 'reset-persona') {
      if (!confirm('Reset ALL agent settings to defaults? Wipes name, icon, persona, chains, etc.')) return;
      await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/settings', {
        method: 'DELETE', credentials: 'same-origin',
      });
      alert('Reset complete. Reflip to refresh.');
      return;
    }
    if (action === 'tools-test') {
      window.open('/tools-runner?agent=' + encodeURIComponent(card.dataset.agent), '_blank');
      return;
    }
  });

  // ── Tools grid filter ────────────────────────────────────────────────
  document.addEventListener('input', (ev) => {
    const s = ev.target.closest && ev.target.closest('[data-bind="tools-search"]');
    if (!s) return;
    const back = s.closest('.cf-back');
    const q = s.value.trim().toLowerCase();
    back.querySelectorAll('.cf-tools-grid .cf-tool').forEach(lab => {
      const name = (lab.textContent || '').toLowerCase();
      lab.style.display = !q || name.includes(q) ? '' : 'none';
    });
  });

  // ── Chain rendering (Brain / TTS / STT) ──────────────────────────────
  // Hydrates each .cf-chain-list[data-chain] from the agent's settings
  // payload (llm_chain_json / tts_chain_json / stt_chain_json) and updates
  // status dots from /api/llm/providers/health.
  function renderChain(listEl, chain, healthByProvider) {
    if (!Array.isArray(chain) || chain.length === 0) {
      listEl.innerHTML = '<li class="cf-chain-skel">No models configured. Use "+ Add" below.</li>';
      return;
    }
    listEl.innerHTML = chain.map((row, idx) => {
      const where = row.where || (row.provider && row.provider.includes('cloud') ? 'cloud' : 'local');
      const h = healthByProvider[row.provider] || { status: 'closed', latency_ms: 0 };
      const wait = row.wait_seconds || 30;
      return `<li data-idx="${idx}" draggable="true">
        <span class="cf-chain-grip">⋮⋮</span>
        <span class="cf-chain-where" data-where="${where}">${where}</span>
        <span class="cf-chain-name">${escapeHtml(row.provider)} · ${escapeHtml(row.model || '')}</span>
        <span class="cf-chain-dot" data-status="${h.status}" title="${h.status} · ${h.latency_ms}ms"></span>
        <input type="number" class="cf-chain-wait" min="1" max="600" value="${wait}" title="seconds before fallback">
        <button type="button" class="cf-chain-remove" title="Remove">×</button>
      </li>`;
    }).join('');
  }
  function escapeHtml(s) {
    if (s == null) return '';
    return String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  async function hydrateChains(card) {
    if (!card || !card.dataset.agent) return;
    let settings = {};
    let health = {};
    try {
      const [sR, hR] = await Promise.all([
        fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/settings', { credentials: 'same-origin' }),
        fetch('/api/llm/providers/health', { credentials: 'same-origin' }),
      ]);
      settings = sR.ok ? await sR.json() : {};
      health = hR.ok ? await hR.json() : {};
    } catch (_) {}
    const back = card.querySelector('.cf-back');
    if (!back) return;
    const parseChain = (raw) => {
      if (!raw) return [];
      try { return typeof raw === 'string' ? JSON.parse(raw) : raw; }
      catch (_) { return []; }
    };
    const healthMap = {};
    if (Array.isArray(health.providers)) {
      health.providers.forEach(p => { healthMap[p.name] = p; });
    }
    const lists = [
      ['brain', parseChain(settings.llm_chain_json)],
      ['tts',   parseChain(settings.tts_chain_json)],
      ['stt',   parseChain(settings.stt_chain_json)],
    ];
    for (const [kind, chain] of lists) {
      const el = back.querySelector('.cf-chain-list[data-chain="' + kind + '"]');
      if (el) renderChain(el, chain, healthMap);
    }
    // Active brain summary
    const brainHead = back.querySelector('[data-role="brain-active"]');
    if (brainHead) {
      const first = parseChain(settings.llm_chain_json)[0];
      brainHead.textContent = first
        ? `${first.provider} · ${first.model || ''}`
        : '(persona default chain)';
    }
  }

  // ── Hydrate Identity + chains on first flip ──────────────────────────
  // Lazy: only fetch when the user actually clicks the cog. Saves a
  // round-trip on pages where the user never opens settings.
  const hydrated = new WeakSet();
  document.addEventListener('click', async (ev) => {
    const cog = ev.target.closest('.cf-cog');
    if (!cog) return;
    const card = cog.closest('.chat-card-flip');
    if (!card || hydrated.has(card)) return;
    hydrated.add(card);
    try {
      const r = await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/settings', {
        credentials: 'same-origin',
      });
      if (!r.ok) return;
      const s = await r.json();
      const back = card.querySelector('.cf-back');
      if (!back) return;
      // Identity
      back.querySelectorAll('input[data-field]').forEach(input => {
        const f = input.dataset.field;
        if (s[f] === null || s[f] === undefined) return;
        if (input.type === 'file' || input.type === 'checkbox' || input.type === 'radio') return;
        input.value = s[f];
        // Trigger slider sync
        if (input.type === 'range') {
          const out = back.querySelector('.cf-slider-val[data-bind="' + f + '"]');
          if (out) out.textContent = fmtSliderVal(f, input.value);
        }
      });
      // Toggles + radios
      back.querySelectorAll('input[type=checkbox][data-field]').forEach(cb => {
        const v = s[cb.dataset.field];
        if (typeof v === 'boolean') cb.checked = v;
      });
      back.querySelectorAll('input[type=radio][data-field]').forEach(rb => {
        const v = s[rb.dataset.field];
        if (typeof v === 'string') rb.checked = (rb.value === v);
      });
      back.querySelectorAll('select[data-field]').forEach(sel => {
        const v = s[sel.dataset.field];
        if (v != null) sel.value = v;
      });
      if (s.accent_color) {
        back.querySelectorAll('.cf-accent-swatch').forEach(s2 =>
          s2.classList.toggle('is-active', s2.dataset.accent === s.accent_color)
        );
      }
      if (s.icon_blob_id) {
        const preview = back.querySelector('.cf-icon-preview');
        if (preview) preview.innerHTML =
          '<img alt="" src="/api/agents/' + encodeURIComponent(card.dataset.agent) + '/icon">';
      }
      // Persona default text — fetched separately so we can show it
      // even before the user toggles edit mode.
      try {
        const pr = await fetch('/api/agents/resolve_prompt?agent=' + encodeURIComponent(card.dataset.agent), {
          credentials: 'same-origin',
        });
        if (pr.ok) {
          const pj = await pr.json();
          const ta = back.querySelector('.cf-persona-text');
          if (ta && pj && (pj.prompt || pj.system_prompt)) {
            const txt = pj.prompt || pj.system_prompt || '';
            ta.dataset.default = txt;
            ta.value = s.persona_prompt_override || txt;
          }
        }
      } catch (_) {}
      // Brain + TTS + STT chains
      await hydrateChains(card);
    } catch (_) {}
  });

  const POLL_MS = 10000;

  function fmtMb(mb) {
    if (mb >= 1024) return (mb / 1024).toFixed(1) + ' GB';
    return mb + ' MB';
  }

  function renderPool(label, total, used) {
    const free = Math.max(0, total - used);
    const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
    const overflow = used > total;
    return `<div class="rb-pool" data-overflow="${overflow}">
      <span class="rb-pool-name">${label}</span>
      <span class="rb-pool-meter"><span class="rb-pool-meter-fill" style="width:${pct}%"></span></span>
      <span class="rb-pool-figs">${fmtMb(used)} / ${fmtMb(total)} <span style="color:#7a8">·  ${fmtMb(free)} free</span></span>
    </div>`;
  }

  function renderCloud(providers) {
    if (!providers || !providers.length) return '';
    return `<div class="rb-pool">
      <span class="rb-pool-name">Cloud</span>
      <span class="rb-pool-figs">${providers.length} provider${providers.length===1?'':'s'} configured</span>
    </div>`;
  }

  async function fetchState() {
    try {
      const r = await fetch('/api/compute/state', { credentials: 'same-origin' });
      if (!r.ok) return null;
      return await r.json();
    } catch (_) { return null; }
  }

  async function refreshOne(bar) {
    const state = await fetchState();
    const pools = bar.querySelector('.rb-pools');
    if (!state) {
      pools.dataset.state = 'error';
      pools.innerHTML = '<div class="rb-pool rb-pool-skel">Compute state unavailable</div>';
      return;
    }
    pools.dataset.state = 'live';
    let html = '';
    for (const g of (state.gpus || [])) {
      html += renderPool(g.name, g.vram_total_mb, g.vram_used_mb);
    }
    if (state.cpu_ram && state.cpu_ram.ram_total_mb > 0) {
      html += renderPool(
        `${state.cpu_ram.cpu_cores}-core / RAM`,
        state.cpu_ram.ram_total_mb,
        state.cpu_ram.ram_used_mb,
      );
    }
    html += renderCloud(state.cloud && state.cloud.providers);
    if (!html) html = '<div class="rb-pool rb-pool-skel">No compute detected</div>';
    pools.innerHTML = html;
  }

  // Schedule a poll loop per visible bar. The interval is shared across
  // all bars on a page (one fetch refreshes every bar's render).
  let pollHandle = null;
  function startPoll() {
    if (pollHandle) return;
    const tick = () => document.querySelectorAll('.syntaur-resource-budget').forEach(refreshOne);
    tick();
    pollHandle = setInterval(tick, POLL_MS);
  }
  function stopPoll() {
    if (pollHandle) { clearInterval(pollHandle); pollHandle = null; }
  }

  // Boot now if any bar is in the document, and re-check on SPA-arrival
  // (the back of a card may be rendered server-side but only "shown" when
  // the user clicks the cog — we still poll while it's offscreen so the
  // first flip shows live data instantly).
  function boot() {
    if (document.querySelector('.syntaur-resource-budget')) startPoll();
    else stopPoll();
  }
  boot();
  window.addEventListener('syntaur:page-arrived', boot);
  window.addEventListener('beforeunload', stopPoll);
})();
"#;
