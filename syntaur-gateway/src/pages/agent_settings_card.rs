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

/// Wrap any chat surface in a flip container. `front` = the existing
/// chat markup (rendered as-is by the caller). `back` = the settings
/// panel — by default the full nine-section layout returned by
/// [`agent_settings_back`], but callers may pass a stripped-down
/// "Open in /chat" link for tiny surfaces (dashboard widget S size).
///
/// The `agent_id` is the per-card identity key — every card flips into
/// its own agent's settings, so `/chat`'s flip toggles Peter while
/// `/scheduler`'s flips Thaddeus.
pub fn chat_card_flip(agent_id: &str, front: Markup, back: Markup) -> Markup {
    html! {
        div class="chat-card-flip" data-agent=(agent_id) data-flipped="false" {
            // Cog lives in the FRONT face's top-right; flipping reveals
            // the back. The Done button on the back flips it back.
            div class="cf-face cf-front" {
                // Mic is NOT in the corner — it's injected by JS next to
                // each surface's send button (see SEND_REGISTRY below in
                // RESOURCE_BUDGET_JS). Sean's spec: "cog in upper right
                // or left and mic next to the text send button".
                button
                    type="button"
                    class="cf-cog"
                    aria-label="Agent settings"
                    aria-pressed="false"
                    title="Agent settings"
                {
                    svg
                        width="14" height="14" viewBox="0 0 16 16"
                        fill="none" stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round" stroke-linejoin="round"
                    {
                        circle cx="8" cy="8" r="2.2" {}
                        path d="M8 1.5v1.6 M8 12.9v1.6 M14.5 8h-1.6 M3.1 8H1.5 M12.6 3.4l-1.1 1.1 M4.5 11.5l-1.1 1.1 M12.6 12.6l-1.1-1.1 M4.5 4.5l-1.1-1.1" {}
                    }
                }
                (front)
            }
            div class="cf-face cf-back" aria-hidden="true" {
                (back)
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
.chat-card-flip {
  position: relative;
  perspective: 1200px;
  width: 100%;
  height: 100%;
  /* Establish containing block so .cf-face's absolute inset:0 measures
     against this element. Caller must size .chat-card-flip to whatever
     the chat surface needs (already true on most surfaces). */
}
.chat-card-flip > .cf-face {
  position: absolute;
  inset: 0;
  transform-style: preserve-3d;
  -webkit-backface-visibility: hidden;
  backface-visibility: hidden;
  transition: transform 320ms cubic-bezier(.6,.05,.4,1);
  background: inherit;
  border-radius: inherit;
  overflow: auto;
}
.chat-card-flip > .cf-front { transform: rotateY(0deg); }
.chat-card-flip > .cf-back  { transform: rotateY(180deg); }
.chat-card-flip[data-flipped="true"] > .cf-front {
  transform: rotateY(-180deg);
  pointer-events: none;
}
.chat-card-flip[data-flipped="true"] > .cf-back {
  transform: rotateY(0deg);
  pointer-events: auto;
}
.chat-card-flip[data-flipped="true"] > .cf-back[aria-hidden] {
  /* Once flipped to back, expose to assistive tech. JS toggles too —
     this is the no-JS fallback. */
}
@media (prefers-reduced-motion: reduce) {
  .chat-card-flip > .cf-face { transition: none; }
}

.cf-cog {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 4;
  background: rgba(255,255,255,0.04);
  color: #aab;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 999px;
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: color 120ms, background 120ms;
}
.cf-cog:hover { color: #fff; background: rgba(255,255,255,0.08); }
.chat-card-flip[data-flipped="true"] .cf-cog {
  opacity: 0;
}

/* Inline mic — injected next to each surface's send button by the JS
   SEND_REGISTRY. NOT positioned absolutely; flows inline alongside the
   send button so it visually belongs to the input row. */
.cf-mic-inline {
  background: rgba(255,255,255,0.04);
  color: #aab;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 999px;
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: color 120ms, background 120ms;
  flex-shrink: 0;
  margin-right: 6px;
}
.cf-mic-inline:hover { color: #fff; background: rgba(255,255,255,0.08); }
.cf-mic-inline[aria-pressed="true"] {
  background: rgba(74, 222, 128, 0.15);
  color: #4ade80;
  border-color: rgba(74, 222, 128, 0.4);
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

  // ── Card flip controller ────────────────────────────────────────────
  // The settings card and its corresponding chat card are the SAME
  // .chat-card-flip; we toggle data-flipped="true|false". ESC + the
  // back's "Done" button flip back. Focus is moved to the back's first
  // focusable on flip-in and back to the cog on flip-out.

  function flipCard(card, to) {
    if (!card) return;
    const next = to === 'back' ? 'true' : 'false';
    if (card.dataset.flipped === next) return;
    card.dataset.flipped = next;
    const back = card.querySelector('.cf-back');
    if (back) back.setAttribute('aria-hidden', next === 'true' ? 'false' : 'true');
    const cog = card.querySelector('.cf-cog');
    if (cog) cog.setAttribute('aria-pressed', next);
    if (next === 'true') {
      const focusable = card.querySelector('.cf-back input, .cf-back button, .cf-back select, .cf-back textarea');
      if (focusable) setTimeout(() => focusable.focus(), 200);
    } else if (cog) {
      cog.focus();
    }
  }

  document.addEventListener('click', (ev) => {
    const cog = ev.target.closest('.cf-cog');
    if (cog) {
      ev.preventDefault();
      flipCard(cog.closest('.chat-card-flip'), 'back');
      return;
    }
    const done = ev.target.closest('.cf-back-done');
    if (done) {
      ev.preventDefault();
      flipCard(done.closest('.chat-card-flip'), 'front');
      return;
    }
    const mic = ev.target.closest('.cf-mic-inline');
    if (mic) {
      ev.preventDefault();
      const agent = mic.dataset.agent || 'main';
      const key = 'syntaur:tts:' + agent;
      const next = localStorage.getItem(key) === 'on' ? 'off' : 'on';
      localStorage.setItem(key, next);
      mic.setAttribute('aria-pressed', next === 'on' ? 'true' : 'false');
      return;
    }
  });

  // ── Inline mic injection next to each surface's send button ──────────
  // Sean's spec: mic lives next to the text send button, not in the
  // card corner. Each surface registers its send button selector + the
  // owning agent. Idempotent — re-running just resyncs aria-pressed.
  const SEND_REGISTRY = [
    { selector: '#send-btn',          agent: 'main'     },  // /chat
    { selector: '#cortex-ask-btn',    agent: 'cortex'   },  // /knowledge
    { selector: '#sch-thad-send',     agent: 'thaddeus' },  // /scheduler
    { selector: '#mushi-send-btn',    agent: 'mushi'    },  // /journal
    { selector: '#silvr-ask',         agent: 'silvr'    },  // /music
    { selector: '#ai-send-btn',       agent: 'maurice'  },  // /coders
  ];

  function injectMicNextToSend(sendBtn, agent) {
    if (!sendBtn || !sendBtn.parentNode) return;
    // Skip if a mic for this agent already exists in this row.
    const existing = sendBtn.parentNode.querySelector(
      '.cf-mic-inline[data-agent="' + agent + '"]'
    );
    if (existing) {
      const on = localStorage.getItem('syntaur:tts:' + agent) === 'on';
      existing.setAttribute('aria-pressed', on ? 'true' : 'false');
      return;
    }
    const mic = document.createElement('button');
    mic.type = 'button';
    mic.className = 'cf-mic-inline';
    mic.dataset.agent = agent;
    mic.title = 'Speak replies (TTS)';
    mic.setAttribute('aria-label', 'Toggle voice replies for ' + agent);
    const on = localStorage.getItem('syntaur:tts:' + agent) === 'on';
    mic.setAttribute('aria-pressed', on ? 'true' : 'false');
    mic.innerHTML = '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 1.5a2.5 2.5 0 00-2.5 2.5v4a2.5 2.5 0 005 0V4A2.5 2.5 0 008 1.5z"/><path d="M3.5 7.5v0.5a4.5 4.5 0 009 0V7.5 M8 12.5v2 M5.5 14.5h5"/></svg>';
    sendBtn.parentNode.insertBefore(mic, sendBtn);
  }

  function injectAllInlineMics() {
    for (const entry of SEND_REGISTRY) {
      document.querySelectorAll(entry.selector).forEach(b =>
        injectMicNextToSend(b, entry.agent)
      );
    }
  }

  // Run once at boot, then again after SPA navigations + DOM changes
  // (some pages render their input row asynchronously after first load).
  injectAllInlineMics();
  window.addEventListener('syntaur:page-arrived', () => {
    autoMountSidePanels();
    injectAllInlineMics();
  });
  // MutationObserver catches input rows that materialize after page load
  // (e.g., scheduler's drawer that opens lazily, music's promotable tabs).
  if (typeof MutationObserver !== 'undefined') {
    new MutationObserver((muts) => {
      // Cheap check: any added node could be a chat input row. Just re-scan.
      for (const m of muts) {
        if (m.addedNodes && m.addedNodes.length) {
          injectAllInlineMics();
          break;
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
  const PANEL_REGISTRY = [
    { selector: '#lib-cortex',   agent: 'cortex'    },  // /knowledge
    { selector: '#sch-thad-chat',agent: 'thaddeus'  },  // /scheduler
    { selector: '.mushi-rail',   agent: 'mushi'     },  // /journal
    { selector: '#silvr-chat',   agent: 'silvr'     },  // /music
    { selector: '#ai-chat-card', agent: 'maurice'   },  // /coders
  ];

  function autoMountSidePanels() {
    for (const entry of PANEL_REGISTRY) {
      const panel = document.querySelector(entry.selector);
      if (!panel) continue;
      if (panel.closest('.chat-card-flip')) continue;  // already wrapped
      wrapPanel(panel, entry.agent);
    }
  }

  function wrapPanel(panel, agent) {
    // Build the flip wrapper around the existing panel element.
    const wrap = document.createElement('div');
    wrap.className = 'chat-card-flip';
    wrap.dataset.agent = agent;
    wrap.dataset.flipped = 'false';
    // Inherit positioning from the panel so the absolute-positioned faces
    // measure correctly.
    wrap.style.position = panel.style.position || 'relative';

    const front = document.createElement('div');
    front.className = 'cf-face cf-front';
    // Cog
    const cog = document.createElement('button');
    cog.type = 'button';
    cog.className = 'cf-cog';
    cog.setAttribute('aria-label', 'Agent settings');
    cog.setAttribute('aria-pressed', 'false');
    cog.title = 'Agent settings';
    cog.innerHTML = '<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v1.6 M8 12.9v1.6 M14.5 8h-1.6 M3.1 8H1.5 M12.6 3.4l-1.1 1.1 M4.5 11.5l-1.1 1.1 M12.6 12.6l-1.1-1.1 M4.5 4.5l-1.1-1.1"/></svg>';
    // Mic
    const mic = document.createElement('button');
    mic.type = 'button';
    mic.className = 'cf-mic';
    mic.setAttribute('aria-label', 'Toggle voice replies');
    mic.setAttribute('aria-pressed', localStorage.getItem('syntaur:tts:' + agent) === 'on' ? 'true' : 'false');
    mic.title = 'Speak replies (TTS)';
    mic.innerHTML = '<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 1.5a2.5 2.5 0 00-2.5 2.5v4a2.5 2.5 0 005 0V4A2.5 2.5 0 008 1.5z"/><path d="M3.5 7.5v0.5a4.5 4.5 0 009 0V7.5 M8 12.5v2 M5.5 14.5h5"/></svg>';
    front.appendChild(cog);
    front.appendChild(mic);

    // Back: empty placeholder. We GET the back markup from the server
    // on first flip so we don't pay the ~10KB cost up-front for surfaces
    // the user never opens.
    const back = document.createElement('div');
    back.className = 'cf-face cf-back';
    back.setAttribute('aria-hidden', 'true');
    back.dataset.lazyLoad = '1';

    // Insert the wrapper in the panel's place, then move the panel into
    // the front face.
    panel.parentNode.insertBefore(wrap, panel);
    front.appendChild(panel);
    wrap.appendChild(front);
    wrap.appendChild(back);

    // Make the front absolutely fill the wrapper but keep the panel's
    // intrinsic content layout (overflow: auto on the face handles
    // scroll). This is safer than rotating the panel directly and lets
    // the existing panel CSS continue to work.
  }

  // Lazy-fetch the back markup on first flip for auto-mounted panels.
  document.addEventListener('click', async (ev) => {
    const cog = ev.target.closest('.cf-cog');
    if (!cog) return;
    const card = cog.closest('.chat-card-flip');
    const back = card && card.querySelector('.cf-back');
    if (!back || back.dataset.lazyLoad !== '1') return;
    if (back.dataset.lazyLoaded === '1') return;
    back.dataset.lazyLoaded = '1';
    try {
      const r = await fetch('/api/agents/' + encodeURIComponent(card.dataset.agent) + '/settings_back', {
        credentials: 'same-origin',
        headers: { 'Accept': 'text/html' },
      });
      if (!r.ok) {
        back.innerHTML = '<div style="padding:14px;color:#aab">Settings unavailable.</div>';
        return;
      }
      back.innerHTML = await r.text();
    } catch (_) {
      back.innerHTML = '<div style="padding:14px;color:#aab">Settings unavailable.</div>';
    }
  });

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
    const flipped = document.querySelector('.chat-card-flip[data-flipped="true"]');
    if (flipped) {
      ev.preventDefault();
      flipCard(flipped, 'front');
    }
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
