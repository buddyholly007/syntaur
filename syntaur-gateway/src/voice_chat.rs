//! OpenAI-compatible `/v1/chat/completions` endpoint that fronts Syntaur
//! for voice clients (Home Assistant's `extended_openai_conversation`).
//!
//! ## Why this exists
//! HA's voice pipeline already has a working OpenAI client (the
//! HACS `extended_openai_conversation` integration). Pointing it directly
//! at TurboQuant + rust-llm-proxy works for chitchat but is brittle for
//! tool use:
//!   * HA exposes tools as a single `execute_services` catch-all that the
//!     LLM has to fill in correctly. The Qwen 3.5 distilled-reasoning model
//!     hallucinates entity IDs, leaks `<tool_call>` blobs, and burns its
//!     token budget on internal monologue.
//!   * HA executes the tool calls itself by literally POSTing to
//!     `/api/services/...`. Failures (wrong entity_id, wrong arg shape) are
//!     surfaced back to the LLM as text errors that the model then has to
//!     reason about.
//!
//! Routing through Syntaur fixes both:
//!   1. We give the LLM **typed tools** (`control_light`, `set_thermostat`,
//!      `query_state`, `call_ha_service`) with crisp JSON schemas, so the
//!      model has way less surface area to hallucinate.
//!   2. The whole tool-call loop runs **server-side in Syntaur** — no HA
//!      round-trips between rounds. We only return the FINAL plain-text
//!      response to HA, which speaks it via Wyoming TTS.
//!
//! ## Wire shape
//! Accepts the standard OpenAI `POST /v1/chat/completions` body
//! (`model`, `messages[]`, optional `tools[]`, optional `tool_choice`,
//! optional `max_tokens`, etc.). The `model` field is ignored — Syntaur
//! always routes through its configured LlmChain. The caller's `tools[]`
//! is also ignored — we replace it with Syntaur's own ToolRegistry tools
//! so the LLM only sees the schemas we vetted.
//!
//! Returns the canonical OpenAI v1 chat completion shape:
//! ```json
//! {
//!   "id": "voicechat-...",
//!   "object": "chat.completion",
//!   "created": 1234567890,
//!   "model": "syntaur-voice",
//!   "choices": [{
//!     "index": 0,
//!     "message": {"role": "assistant", "content": "Done."},
//!     "finish_reason": "stop"
//!   }],
//!   "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
//! }
//! ```
//!
//! ## Auth
//! Optional bearer token in the `Authorization` header. When set in
//! `~/.syntaur/syntaur.json` `connectors.home_assistant.shared_secret`,
//! callers must present that exact token. When unset, the endpoint is
//! open on the bind address (so put it on a LAN-only port).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::llm::{self, LlmResult};
use crate::tools::{self, ToolCall};
use crate::AppState;

// ── Conversation memory ─────────────────────────────────────────────────────

/// How long a conversation stays alive after the last interaction.
/// Within this window, Peter remembers prior exchanges ("make them warmer").
/// After it expires, the next wake trigger starts a fresh conversation.
const CONVERSATION_TIMEOUT_SECS: u64 = 300; // 5 minutes

/// Max turns (user + assistant pairs) stored per conversation.
/// Only final text exchanges are stored — tool_call/tool_result
/// intermediates are NOT kept, preventing context bloat that confuses
/// Qwen into hallucinating tool completions instead of calling tools.
const CONVERSATION_MAX_TURNS: usize = 10;

/// A single conversation's state — messages + last-active timestamp.
struct Conversation {
    messages: Vec<llm::ChatMessage>,
    last_active: Instant,
}

/// Process-global conversation store, keyed by a caller identifier
/// (device_id or fallback). Lazily initialized.
static CONVERSATIONS: std::sync::OnceLock<Mutex<HashMap<String, Conversation>>> =
    std::sync::OnceLock::new();

fn conversations() -> &'static Mutex<HashMap<String, Conversation>> {
    CONVERSATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// STT post-processing — correct known Parakeet misrecognitions for
/// smart home vocabulary. These are consistent patterns, not random errors.
fn stt_corrections(text: &str) -> String {
    let mut result = text.to_string();

    // Parakeet consistently hears "office" as "off his" or "off slides"
    let corrections: &[(&str, &str)] = &[
        ("off his lights", "office lights"),
        ("off slides", "office lights"),
        ("off his light", "office light"),
        ("the off slide", "the office light"),
        ("off is lights", "office lights"),
        ("offish lights", "office lights"),
        // Add more patterns as they're discovered
    ];

    let lower = result.to_lowercase();
    for (from, to) in corrections {
        if lower.contains(from) {
            // Case-insensitive replacement
            let start = lower.find(from).unwrap();
            result = format!("{}{}{}", &result[..start], to, &result[start + from.len()..]);
            break;
        }
    }

    result
}

/// Per-call hard ceiling. Voice commands need to feel snappy and the
/// 27B model can churn through tokens fast. 8 rounds is enough for any
/// reasonable command (typical: 1 tool call + 1 text response = 2 rounds)
/// while preventing runaway loops if the model goes off the rails.
const VOICE_MAX_ROUNDS: usize = 8;

/// Voice system prompt — the full brief to the LLM. This is the ONLY system
/// message the model sees for voice requests; the caller (HA's
/// extended_openai_conversation) often ships an 8000-char persona prompt
/// of its own, but including it as well bloats the context to ~2500 tokens
/// and reliably tips the Qwen distillation into reasoning instead of
/// acting. Persona flavor lives here, along with tool discipline.
const VOICE_SYSTEM_PROMPT: &str = r#"You ARE Peter Parker — witty, casual, quick. Say "yeah" not "yes", use "dude", "man" naturally. 1-2 sentences max. Never end with a question. Never say "As an AI."

RULES:
1. ALWAYS call a tool for actions. NEVER claim you did something without a tool call. Call tool first, then confirm.
2. Lights: use matter tool. matter(action="on/off/brightness/color_temp", room="ROOM", value=N). Brightness 0-254, color_temp in kelvin (warm=2700, cool=6500).
3. Thermostat: set_thermostat. Volume: call_ha_service(media_player, volume_set, media_player.satellite1_918358_sat1_media_player, volume_level=0.0-1.0).
4. Weather→weather. Timers→timer. Lists→shopping_list. Time→answer from prompt. Everything else→find_tool.
5. STT errors happen. "off slides"="office lights". Match closest room name. Don't ask to repeat.
6. Never reason out loud. Just act and respond.

Rooms: kitchen, office, living room, master bedroom, master bathroom, dining room, entryway, laundry room, outside, kids bathroom, half bathroom, williams bedroom, anastasias bedroom
"#;

#[derive(Deserialize, Debug)]
pub struct OpenAiChatRequest {
    #[serde(default)]
    #[allow(dead_code)]
    pub model: Option<String>,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    #[allow(dead_code)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub temperature: Option<f64>,
    /// HA's extended_openai_conversation may send these — we ignore them
    /// but accept their presence so deserialization doesn't fail.
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct OpenAiChatResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: &'static str,
    pub choices: Vec<OpenAiChoice>,
    pub usage: OpenAiUsage,
}

#[derive(Serialize)]
pub struct OpenAiChoice {
    pub index: u32,
    pub message: OpenAiAssistantMessage,
    pub finish_reason: &'static str,
}

#[derive(Serialize)]
pub struct OpenAiAssistantMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Serialize, Default)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Build a canonical "Done." style fallback response so HA always gets
/// something to TTS even when the LLM produces nothing usable.
fn fallback_response(text: &str) -> OpenAiChatResponse {
    OpenAiChatResponse {
        id: format!("voicechat-{}", chrono::Utc::now().timestamp()),
        object: "chat.completion",
        created: chrono::Utc::now().timestamp(),
        model: "syntaur-voice",
        choices: vec![OpenAiChoice {
            index: 0,
            message: OpenAiAssistantMessage {
                role: "assistant",
                content: text.to_string(),
            },
            finish_reason: "stop",
        }],
        usage: OpenAiUsage::default(),
    }
}

/// Validate the bearer token if a shared secret is configured.
///
/// * If no secret is set AND `require_auth` is true  -> UNAUTHORIZED
/// * If no secret is set AND `require_auth` is false -> Ok (LAN-only open mode)
/// * If a secret is set, use constant-time XOR comparison.
fn check_auth(headers: &HeaderMap, expected: Option<&str>, require_auth: bool) -> Result<(), StatusCode> {
    let Some(secret) = expected else {
        if require_auth {
            warn!("[voice_chat] auth required but no shared secret configured");
            return Err(StatusCode::UNAUTHORIZED);
        }
        return Ok(());
    };
    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = header.strip_prefix("Bearer ").unwrap_or(header);

    // Constant-time comparison: XOR every byte, accumulate into a single flag.
    let a = presented.as_bytes();
    let b = secret.as_bytes();
    let len_match = a.len() == b.len();
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    if len_match && diff == 0 {
        Ok(())
    } else {
        warn!("[voice_chat] auth failed: missing or wrong bearer token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// POST /v1/chat/completions handler.
pub async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OpenAiChatRequest>,
) -> Result<Json<OpenAiChatResponse>, StatusCode> {
    check_auth(&headers, state.ha_voice_secret.as_deref(), state.config.security.require_voice_auth)?;

    info!(
        "[voice_chat] request: {} messages, model={:?}",
        req.messages.len(),
        req.model
    );

    // 1. Use Syntaur's voice system prompt — and ONLY that. Discard
    //    whatever the caller sent. HA's extended_openai_conversation ships
    //    an ~8000 char persona prompt of its own which, when stacked with
    //    our tool-discipline prompt, bloats context to ~2500 tokens and
    //    reliably tips the Qwen distillation into reasoning rather than
    //    acting. Persona flavor lives in VOICE_SYSTEM_PROMPT now that
    //    Syntaur is the brain.
    //    Inject current date/time so Peter always knows without a tool call.
    // Use America/Los_Angeles (Sean's timezone) instead of server local (UTC)
    use chrono::TimeZone;
    let tz: chrono_tz::Tz = "America/Los_Angeles".parse().unwrap();
    let now = chrono::Utc::now().with_timezone(&tz);
    let system_prompt = format!(
        "{}\n\nCurrent time: {}",
        VOICE_SYSTEM_PROMPT,
        now.format("%-I:%M %p"),
    );

    // 2. Extract ONLY the latest user message from HA's request.
    //    HA's extended_openai_conversation sends the FULL accumulated
    //    conversation history with every wake-word trigger. We ignore all
    //    of it — Syntaur owns the conversation state, not HA.
    let raw_user_text = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .unwrap_or("")
        .to_string();

    // 2a. STT post-processing — correct known Parakeet misrecognitions.
    let latest_user_text = stt_corrections(&raw_user_text);

    info!("[voice_chat] STT input: \"{}\"", latest_user_text);

    // 2b. Conversation memory — keyed by a caller identifier.
    //     Within CONVERSATION_TIMEOUT_SECS of the last exchange, Peter
    //     remembers context ("make them warmer", "turn those off too").
    //     After timeout, conversation resets to fresh.
    let caller_id = req
        .model
        .as_deref()
        .unwrap_or("default")
        .to_string();

    let mut conv_store = conversations().lock().await;

    // Expire stale conversations
    let now = Instant::now();
    conv_store.retain(|_, c| now.duration_since(c.last_active).as_secs() < CONVERSATION_TIMEOUT_SECS);

    // Get or create conversation for this caller
    let conv = conv_store.entry(caller_id.clone()).or_insert_with(|| {
        info!("[voice_chat] new conversation for caller={}", caller_id);
        Conversation {
            messages: Vec::new(),
            last_active: now,
        }
    });

    // Check if conversation expired (shouldn't happen after retain, but
    // handles the case where the entry existed but is stale)
    if now.duration_since(conv.last_active).as_secs() >= CONVERSATION_TIMEOUT_SECS {
        info!("[voice_chat] conversation expired for caller={}, resetting", caller_id);
        conv.messages.clear();
    }
    conv.last_active = now;

    // Add the new user message
    conv.messages.push(llm::ChatMessage::user(&latest_user_text));

    // Trim to max turns (each turn = 1 user + 1 assistant = 2 messages)
    while conv.messages.len() > CONVERSATION_MAX_TURNS * 2 {
        conv.messages.remove(0);
    }

    // Build the LLM message list: system prompt + conversation history.
    // Only user/assistant text pairs are in conv.messages — no tool_call
    // or tool_result intermediates, keeping context lean so the LLM
    // doesn't get confused and skip tool calls.
    let mut messages: Vec<llm::ChatMessage> =
        Vec::with_capacity(conv.messages.len() + 1);
    messages.push(llm::ChatMessage::system(&system_prompt));
    messages.extend(conv.messages.clone());

    // Drop the lock before the LLM call (we'll re-acquire to save the
    // assistant response after the tool loop completes)
    drop(conv_store);

    // 3. Build the LLM chain (uses the voice agent's model selection,
    //    falling back to "main" if no voice agent is configured).
    let agent_id = state
        .config
        .agent_model("voice")
        .primary
        .as_str()
        .is_empty()
        .then(|| "main".to_string())
        .unwrap_or_else(|| "voice".to_string());
    let llm_chain = Arc::new(llm::LlmChain::from_config(
        &state.config,
        &agent_id,
        state.client.clone(),
    ));

    // 4. Build the ToolRegistry — we want a SUBSET focused on HA control
    //    plus a couple Syntaur extras (memory read for persistence,
    //    web_search if the user asks something the model can't answer).
    //    The full registry is overkill and bloats the LLM context.
    let workspace = state.config.agent_workspace(&agent_id);
    let mut registry = tools::ToolRegistry::with_extensions(
        workspace,
        agent_id.clone(),
        Some(state.mcp.clone()),
        state.indexer.clone(),
    );
    registry.set_infra(
        Arc::clone(&state.tool_rate_limiter),
        Arc::clone(&state.tool_circuit_breakers),
    );
    registry.set_user_id(0); // voice path = legacy admin
    registry.set_db_path(state.db_path.clone());
    registry.set_tool_hooks(Arc::clone(&state.tool_hooks));
    registry.set_http_client(Arc::new(state.client.clone()));

    // The voice path uses ONLY a curated tool surface. We re-register the
    // HA tools explicitly so they appear regardless of how the registry
    // was built — `with_extensions` doesn't know about them by default.
    // We also register `find_tool` (Phase 0 voice skill router) so the LLM
    // can dispatch to long-tail skills (timers, calendar, music, weather,
    // …) without having to see all of them in its function-calling list.
    {
        use crate::tools::extension::Tool as ToolTrait;
        use crate::tools::find_tool::FindToolByIntent;
        use crate::tools::home_assistant::{
            HaCallServiceTool, HaControlLightTool, HaQueryStateTool, HaSetThermostatTool,
        };
        use crate::tools::matter::MatterTool;
        use crate::tools::announce::AnnounceTool;
        use crate::tools::calendar::CalendarTool;
        use crate::tools::music::MusicTool;
        use crate::tools::shopping_list::ShoppingListTool;
        use crate::tools::timers::TimerTool;
        use crate::tools::weather::WeatherTool;
        let mut voice_tools: Vec<Arc<dyn ToolTrait>> = vec![
            Arc::new(MatterTool),           // Primary: direct Matter/Thread control
            Arc::new(HaControlLightTool),    // Fallback: HA REST for non-Matter devices
            Arc::new(HaSetThermostatTool),
            Arc::new(HaQueryStateTool),
            Arc::new(HaCallServiceTool),
            // Phase 1-2 direct tools — high-use daily skills.
            Arc::new(WeatherTool),
            Arc::new(TimerTool),
            Arc::new(ShoppingListTool),
            Arc::new(AnnounceTool),
            Arc::new(CalendarTool),
            Arc::new(MusicTool),
        ];
        // find_tool only registers if both the router and the LlmChain are
        // available. The LlmChain is the same one that voice_chat uses for
        // the outer loop — find_tool's inner arg-extraction call goes
        // through this same chain (TurboQuant primary, Nemotron fallback).
        if let Some(router) = &state.tool_router {
            voice_tools.push(Arc::new(FindToolByIntent {
                router: Arc::clone(router),
                llm: Arc::clone(&llm_chain),
            }));
        }
        registry.add_extension_tools(&voice_tools);
    }

    // Filter the tool definitions sent to the LLM — only HA tools + a
    // tiny set of useful escapes. Huge tool lists confuse small models.
    // find_tool is the Phase 0 dispatcher — it lets the LLM reach the
    // long tail of skills without each one cluttering the visible list.
    let allowed_tool_names: &[&str] = &[
        "matter",           // All light control via Matter/Thread
        "set_thermostat",
        "query_state",
        "call_ha_service",
        "web_search",
        "find_tool",
        "weather",
        "timer",
        "shopping_list",
        "announce",
        "calendar",
        "music",
    ];
    let all_tools = registry.tool_definitions();
    let tools: Vec<Value> = all_tools
        .into_iter()
        .filter(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|n| allowed_tool_names.contains(&n))
                .unwrap_or(false)
        })
        .collect();
    info!("[voice_chat] {} tools exposed to LLM", tools.len());

    // 5. Tool-call loop
    for round in 0..VOICE_MAX_ROUNDS {
        let result = match llm_chain.call_raw(&messages, Some(&tools)).await {
            Ok(r) => r,
            Err(e) => {
                warn!("[voice_chat] LLM call failed at round {}: {}", round, e);
                return Ok(Json(fallback_response(
                    "Sorry, I'm having trouble reaching my brain right now.",
                )));
            }
        };

        match result {
            LlmResult::Text(text) => {
                let cleaned = text.trim();
                let final_text = if cleaned.is_empty() {
                    "Sorry, I blanked out — try that again?".to_string()
                } else {
                    cleaned.to_string()
                };
                info!(
                    "[voice_chat] done in {} round(s): {} chars",
                    round + 1,
                    final_text.len()
                );

                // Save Peter's response to conversation memory
                let mut conv_store = conversations().lock().await;
                if let Some(conv) = conv_store.get_mut(&caller_id) {
                    conv.messages.push(llm::ChatMessage::assistant(&final_text));
                }
                drop(conv_store);

                return Ok(Json(fallback_response(&final_text)));
            }
            LlmResult::ToolCalls { content, tool_calls } => {
                info!(
                    "[voice_chat] round {}: {} tool call(s)",
                    round,
                    tool_calls.len()
                );
                messages.push(llm::ChatMessage::assistant_with_tools(&content, tool_calls.clone()));
                for tc in &tool_calls {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let func = tc.get("function").cloned().unwrap_or(json!({}));
                    let name = func
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args_str = func
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                    let call = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: args,
                    };
                    let tool_result = registry.execute(&call).await;
                    let mut output = tool_result.output;
                    if output.len() > 800 {
                        output = format!("{}…[truncated]", &output[..600]);
                    }
                    info!(
                        "[voice_chat]   tool {} -> {}: {}",
                        name,
                        if tool_result.success { "ok" } else { "err" },
                        output.chars().take(120).collect::<String>()
                    );
                    messages.push(llm::ChatMessage::tool_result(&id, &output));
                }
            }
        }
    }

    // 6. Out of rounds — force a final text response
    warn!("[voice_chat] exhausted {} rounds, forcing final text", VOICE_MAX_ROUNDS);
    messages.push(llm::ChatMessage::system(
        "Stop calling tools. Reply with a brief sentence about what you did or couldn't do.",
    ));
    let final_text = match llm_chain.call(&messages).await {
        Ok(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                "I tried but couldn't quite finish that one.".to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => "I tried but couldn't quite finish that one.".to_string(),
    };

    // Save to conversation memory
    let mut conv_store = conversations().lock().await;
    if let Some(conv) = conv_store.get_mut(&caller_id) {
        conv.messages.push(llm::ChatMessage::assistant(&final_text));
    }
    drop(conv_store);

    Ok(Json(fallback_response(&final_text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_response_shape() {
        let r = fallback_response("Done.");
        assert_eq!(r.object, "chat.completion");
        assert_eq!(r.choices.len(), 1);
        assert_eq!(r.choices[0].message.role, "assistant");
        assert_eq!(r.choices[0].message.content, "Done.");
        assert_eq!(r.choices[0].finish_reason, "stop");
    }

    #[test]
    fn test_check_auth_no_secret_not_required_passes() {
        let h = HeaderMap::new();
        assert!(check_auth(&h, None, false).is_ok());
    }

    #[test]
    fn test_check_auth_no_secret_required_fails() {
        let h = HeaderMap::new();
        assert!(check_auth(&h, None, true).is_err());
    }

    #[test]
    fn test_check_auth_correct_secret_passes() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer s3cret".parse().unwrap());
        assert!(check_auth(&h, Some("s3cret"), true).is_ok());
    }

    #[test]
    fn test_check_auth_wrong_secret_fails() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer wrong".parse().unwrap());
        assert!(check_auth(&h, Some("right"), true).is_err());
    }

    #[test]
    fn test_openai_request_minimal_parses() {
        let body = json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "user", "content": "turn on the kitchen lights"}
            ]
        });
        let req: OpenAiChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
    }

    #[test]
    fn test_openai_request_with_tools_parses_and_ignores() {
        // HA's extended_openai_conversation will send a tools array — we
        // accept it but ignore it. Make sure we don't error on parse.
        let body = json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type":"function","function":{"name":"execute_services","parameters":{}}}],
            "tool_choice": "auto",
            "max_tokens": 256,
            "temperature": 0.7
        });
        let req: OpenAiChatRequest = serde_json::from_value(body).unwrap();
        assert!(req.tools.is_some());
        assert_eq!(req.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_openai_request_with_tool_role_message() {
        // Mid-conversation requests will include tool result messages.
        let body = json!({
            "messages": [
                {"role": "user", "content": "do it"},
                {"role": "assistant", "tool_calls": [{"id":"call_1","type":"function","function":{"name":"control_light","arguments":"{}"}}]},
                {"role": "tool", "tool_call_id": "call_1", "content": "ok"}
            ]
        });
        let req: OpenAiChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[1].role, "assistant");
        assert!(req.messages[1].tool_calls.is_some());
        assert_eq!(req.messages[2].role, "tool");
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));
    }
}
