//! Voice chat handler — OpenAI-compatible /v1/chat/completions endpoint.
//!
//! Lessons from Peter pipeline baked in:
//! - Lean system prompt (~350 tokens) with time injection
//! - Conversation memory: user+assistant text only (never tool intermediates)
//! - Max 5 turns, 5-min timeout for voice
//! - max_tokens capped at 200 for short TTS-friendly responses
//! - STT corrections for known Parakeet mishearings

use std::collections::HashMap;
use std::sync::Arc;
pub mod edge_tts;
pub mod io;
pub mod ui;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::Utc;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::sync::Mutex;

use crate::backend::router::BackendRouter;
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::conversation::ContextBudget;
use crate::task::Message;

// ── Voice system prompt (lean, ~350 tokens) ─────────────────────────────

fn voice_system_prompt() -> String {
    let now = chrono::Local::now();
    let time_str = now.format("%I:%M %p").to_string();
    let date_str = now.format("%A, %B %e, %Y").to_string();

    format!(
        r#"You are Syntaur, a helpful voice assistant. Be concise — 1-2 sentences max.

RULES:
1. Keep responses SHORT. The user is listening, not reading.
2. For factual questions, answer directly.
3. If you don't know something, say so briefly.
4. Never use markdown, code blocks, or formatting — this is spoken aloud.
5. Use natural, conversational language.

Current time: {} on {}."#,
        time_str, date_str
    )
}

// ── STT corrections (from Peter's Parakeet experience) ──────────────────

fn apply_stt_corrections(text: &str) -> String {
    let mut result = text.to_string();
    let corrections = [
        ("off his lights", "office lights"),
        ("off slides", "office lights"),
        ("the off slide", "the office light"),
        ("off his", "office"),
        ("livingroom", "living room"),
        ("bedrooom", "bedroom"),
    ];
    let lower = result.to_lowercase();
    for (from, to) in &corrections {
        if lower.contains(from) {
            result = result.replace(from, to);
            // Also handle case variations
            let from_cap = format!(
                "{}{}",
                from.chars().next().unwrap().to_uppercase(),
                &from[1..]
            );
            result = result.replace(&from_cap, to);
        }
    }
    result
}

// ── In-memory conversation store for voice (ephemeral, fast) ────────────

struct VoiceConversation {
    messages: Vec<(String, String)>, // (role, content) — text only
    last_active: Instant,
}

// Use a simple Mutex<HashMap> for voice conversations (short-lived, low contention)
static VOICE_CONVOS: std::sync::OnceLock<Mutex<HashMap<String, VoiceConversation>>> =
    std::sync::OnceLock::new();

fn voice_convos() -> &'static Mutex<HashMap<String, VoiceConversation>> {
    VOICE_CONVOS.get_or_init(|| Mutex::new(HashMap::new()))
}

const VOICE_TIMEOUT_SECS: u64 = 300; // 5 minutes
const VOICE_MAX_TURNS: usize = 5;

// ── OpenAI-compatible request/response types ────────────────────────────

#[derive(Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<OaiMessage>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stream: Option<bool>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct OaiMessage {
    pub role: String,
    pub content: Option<String>,
}

#[derive(Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OaiChoice>,
    pub usage: Option<OaiUsage>,
}

#[derive(Serialize)]
pub struct OaiChoice {
    pub index: u32,
    pub message: OaiMessage,
    pub finish_reason: Option<String>,
}

#[derive(Serialize)]
pub struct OaiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ── Handler ─────────────────────────────────────────────────────────────

pub struct VoiceState {
    pub backend_router: Arc<BackendRouter>,
    pub voice_secret: Option<String>,
}

pub async fn handle_voice_chat(
    State(state): State<Arc<VoiceState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, (StatusCode, String)> {
    let start = Instant::now();

    // Auth check (optional bearer token)
    if let Some(ref secret) = state.voice_secret {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
        if token != secret {
            return Err((StatusCode::UNAUTHORIZED, "invalid voice secret".into()));
        }
    }

    // Extract the latest user message
    let user_message = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .unwrap_or("");

    if user_message.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty user message".into()));
    }

    // Apply STT corrections
    let corrected = apply_stt_corrections(user_message);
    debug!("[voice] input: {:?} → {:?}", user_message, corrected);

    // Conversation memory keyed by model field (or "default")
    let caller_id = req.model.as_deref().unwrap_or("default").to_string();

    // Load + update conversation memory
    let history = {
        let mut convos = voice_convos().lock().await;

        // Purge expired conversations
        let now = Instant::now();
        convos.retain(|_, v| now.duration_since(v.last_active).as_secs() < VOICE_TIMEOUT_SECS);

        let convo = convos.entry(caller_id.clone()).or_insert_with(|| {
            VoiceConversation {
                messages: Vec::new(),
                last_active: now,
            }
        });
        convo.last_active = now;

        // Build history messages (only prior turns, not the new message)
        let history: Vec<Message> = convo
            .messages
            .iter()
            .map(|(role, content)| match role.as_str() {
                "assistant" => Message::assistant(content),
                _ => Message::user(content),
            })
            .collect();

        history
    };

    // Build final message list: system + history + new user message
    let budget = ContextBudget::voice();
    let trimmed_history = crate::conversation::trim_to_budget(&history, &budget);

    let mut messages = vec![Message::system(voice_system_prompt())];
    messages.extend(trimmed_history);
    messages.push(Message::user(&corrected));

    // Call LLM with voice-appropriate settings
    let request = CompletionRequest::simple("")
        .with_system(voice_system_prompt())
        .with_messages(messages)
        .with_max_tokens(budget.max_gen_tokens)
        .with_temperature(0.7);

    let response = state
        .backend_router
        .route(&request, &RoutePreferences::default())
        .await
        .map_err(|e| {
            warn!("[voice] LLM error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Sorry, I blanked out — try that again?".into(),
            )
        })?;

    let assistant_text = response.content.trim().to_string();
    let latency_ms = start.elapsed().as_millis() as u64;

    info!(
        "[voice] caller={} input={:?} response={:?} latency={}ms backend={}",
        caller_id,
        &corrected,
        &assistant_text,
        latency_ms,
        response.backend_id
    );

    // Save to conversation memory (text only, never tool intermediates)
    {
        let mut convos = voice_convos().lock().await;
        if let Some(convo) = convos.get_mut(&caller_id) {
            convo.messages.push(("user".into(), corrected.clone()));
            convo
                .messages
                .push(("assistant".into(), assistant_text.clone()));

            // Enforce max turns
            while convo.messages.len() > VOICE_MAX_TURNS * 2 {
                convo.messages.remove(0);
            }
        }
    }

    // Build OpenAI-compatible response
    let usage = response.tokens.as_ref().map(|t| OaiUsage {
        prompt_tokens: t.prompt_tokens,
        completion_tokens: t.completion_tokens,
        total_tokens: t.total_tokens,
    });

    Ok(Json(ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".into(),
        created: Utc::now().timestamp(),
        model: response.model,
        choices: vec![OaiChoice {
            index: 0,
            message: OaiMessage {
                role: "assistant".into(),
                content: Some(assistant_text),
            },
            finish_reason: response.finish_reason.or_else(|| Some("stop".into())),
        }],
        usage,
    }))
}
