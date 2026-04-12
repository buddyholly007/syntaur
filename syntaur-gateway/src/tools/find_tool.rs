//! `find_tool` — voice-side dispatcher Tool that routes natural-language
//! intents to the right downstream tool via the embedding-based ToolRouter.
//!
//! ## Why this is its own module
//!
//! `tools/router/` holds the routing infrastructure (Embedder, ToolRouter,
//! RouterEntry). This file holds the Tool trait impl that exposes routing
//! to the LLM. Keeping them split means the router can be unit-tested with
//! a mock embedder while find_tool's wiring (LlmChain access for inner
//! arg extraction, ToolContext threading, etc.) lives separately.
//!
//! ## Flow per call
//!
//! 1. LLM decides none of its 5 typed tools fit and calls
//!    `find_tool(intent="set a 5 minute timer")`.
//! 2. ToolRouter::find embeds the intent and cosine-matches against all
//!    registered RouterEntries. Returns Match (idx, score) or NoMatch.
//! 3. On Match: build a small inner-LLM prompt asking the model to extract
//!    a JSON args object from the intent that satisfies the matched tool's
//!    parameter schema.
//! 4. Parse the inner LLM's output as JSON. If parse fails, return an
//!    error to the outer LLM with both the parse error and the raw text
//!    so the outer LLM can recover (e.g. retry with a more specific intent).
//! 5. Execute the matched tool with the parsed args via its existing
//!    `Tool::execute` method.
//! 6. Return the matched tool's result to the outer LLM.
//!
//! ## Latency characteristics
//!
//! Per dispatched call: ~50 ms embed + ~500-1500 ms inner LLM extraction
//! + tool's own execution time. For voice this is acceptable for skills
//! that aren't on the curated direct-call list. Direct-call tools
//! (control_light, set_thermostat, query_state) STAY in the curated 5-tool
//! voice set so simple commands skip find_tool entirely and stay sub-2s.

use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::llm::{ChatMessage, LlmChain};
use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};
use crate::tools::router::{FindResult, ToolRouter};

pub struct FindToolByIntent {
    pub router: Arc<RwLock<ToolRouter>>,
    /// LlmChain used for the inner argument-extraction call. Typically
    /// the same chain the outer voice_chat handler is using, threaded
    /// in via AppState. The arg extraction is JSON-only so it doesn't
    /// need tools enabled.
    pub llm: Arc<LlmChain>,
}

#[async_trait]
impl Tool for FindToolByIntent {
    fn name(&self) -> &str {
        "find_tool"
    }

    fn description(&self) -> &str {
        "Find and execute the right syntaur skill for a request when none of \
         the typed tools (control_light, set_thermostat, query_state, \
         call_ha_service, web_search) fit. Pass a natural-language description \
         of what to do, including any details. The router matches your intent \
         to one of many registered skills (timers, calendar, music, email, \
         weather, shopping list, file ops, code execution, etc.) and runs it. \
         Examples: 'set a 5 minute timer for chicken', 'add milk to the \
         shopping list', 'what is the weather', 'read my latest email', \
         'play some focus music', 'add a meeting to tomorrow at 2pm'."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": {
                    "type": "string",
                    "description": "Natural-language description of the task. Include any parameters \
                                    (durations, names, locations, recipients, etc.) so the router \
                                    can extract them for the matched skill."
                }
            },
            "required": ["intent"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        // The router itself is read-only, but the dispatched downstream tool
        // can do anything. We mark this as non-idempotent because successive
        // calls with the same intent could call different downstream tools
        // (e.g. router updated mid-call) or have side effects (timer started,
        // email sent). Conservative.
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: false,
            requires_approval: None,
            circuit_name: Some("find_tool"),
            rate_limit: None,
        }
    }

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext<'_>,
    ) -> Result<RichToolResult, String> {
        let intent = args
            .get("intent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "find_tool: 'intent' parameter is required".to_string())?
            .trim();
        if intent.is_empty() {
            return Err("find_tool: intent is empty".to_string());
        }

        let router = self.router.read().await;
        if router.is_empty() {
            return Ok(RichToolResult::text(
                "find_tool: router has no entries registered. \
                 Register skills via ToolRouter::add_batch at startup.",
            ));
        }

        let result = router.find(intent).await?;

        let (idx, confidence) = match result {
            FindResult::Match { index, confidence } => (index, confidence),
            FindResult::NoMatch { best: Some((idx, score)) } => {
                let entry = router.get(idx).expect("router::get for top-1 idx");
                return Ok(RichToolResult::text(format!(
                    "No skill matched the intent '{}' with sufficient confidence \
                     (best guess: '{}' at {:.2} similarity, below the {:.2} threshold). \
                     Try rephrasing the intent more specifically, or use a different tool. \
                     The closest skill is: {}",
                    intent.chars().take(120).collect::<String>(),
                    entry.tool.name(),
                    score,
                    0.55,
                    entry.voice_description
                )));
            }
            FindResult::NoMatch { best: None } => {
                return Ok(RichToolResult::text(
                    "find_tool: router has no entries to match against",
                ));
            }
        };

        let entry = router
            .get(idx)
            .ok_or_else(|| format!("router idx {} out of range", idx))?;
        let tool_name = entry.tool.name().to_string();
        let tool_description = entry.tool.description().to_string();
        let parameter_schema = entry.tool.parameters();

        info!(
            "[find_tool] dispatching intent='{}' → {} (confidence {:.3})",
            intent.chars().take(80).collect::<String>(),
            tool_name,
            confidence
        );

        // ── Inner LLM call: extract JSON args from intent ──
        //
        // We deliberately use a tiny, focused system prompt that says
        // "output JSON only" to keep Qwen3.5-distilled from drifting into
        // chain-of-thought mode. The schema is included verbatim so the
        // model has the exact field names + types to fill in.
        let system = "You are a precise JSON argument extractor. Read the user's \
                      intent and the tool's parameter schema, then output ONLY a \
                      valid JSON object that satisfies the schema. No prose, no \
                      explanation, no markdown code fences, no comments. \
                      If a required parameter cannot be inferred from the intent, \
                      omit it and let the tool use its default. \
                      If an optional parameter is not mentioned, omit it.";

        let user_prompt = format!(
            "Tool: {}\nDescription: {}\nParameter schema:\n{}\n\nUser intent: {}\n\nReturn only the JSON object.",
            tool_name,
            tool_description,
            serde_json::to_string_pretty(&parameter_schema).unwrap_or_default(),
            intent
        );

        let messages = vec![
            ChatMessage::system(system),
            ChatMessage::user(&user_prompt),
        ];

        let raw_response = self
            .llm
            .call(&messages)
            .await
            .map_err(|e| format!("find_tool: inner LLM extraction failed: {}", e))?;

        // Strip common LLM noise: code fences, leading "Here is the JSON:",
        // think tags (which the sanitizer should already drop, but defense in depth).
        let cleaned = strip_json_noise(&raw_response);

        let extracted_args: Value = serde_json::from_str(&cleaned).map_err(|e| {
            warn!(
                "[find_tool] arg extraction returned non-JSON for tool {}: {} | raw='{}'",
                tool_name,
                e,
                raw_response.chars().take(200).collect::<String>()
            );
            format!(
                "find_tool: extracted arguments are not valid JSON for {}: {}. \
                 Raw model output: '{}'",
                tool_name,
                e,
                raw_response.chars().take(200).collect::<String>()
            )
        })?;

        info!(
            "[find_tool] executing {} with args={}",
            tool_name,
            serde_json::to_string(&extracted_args).unwrap_or_default()
        );

        // Drop the read lock before executing the tool — the tool may itself
        // dispatch via the router (recursion is unlikely but cheap to support)
        // and we don't want a write lock starvation scenario.
        let tool_ref = entry.tool.clone();
        drop(router);

        let result = tool_ref.execute(extracted_args, ctx).await?;
        Ok(result)
    }
}

/// Best-effort cleanup of LLM-generated JSON output. Strips common
/// wrappings: ```json fences, leading "Here's the JSON:", trailing prose,
/// and `<think>` tags that the sanitizer might have missed. Returns the
/// substring most likely to be a parseable JSON object.
fn strip_json_noise(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Drop think tags if any leaked through.
    if let Ok(re) = regex::Regex::new(r"(?s)<think>.*?</think>") {
        s = re.replace_all(&s, "").trim().to_string();
    }

    // Drop ```json … ``` and ``` … ``` fences.
    if s.starts_with("```") {
        if let Some(start) = s.find('\n') {
            s = s[start + 1..].to_string();
        }
    }
    if s.ends_with("```") {
        s = s.trim_end_matches("```").trim().to_string();
    }

    // If there's text before the first `{`, drop it. Likewise after the
    // matching closing `}`.
    if let Some(open) = s.find('{') {
        if open > 0 {
            s = s[open..].to_string();
        }
    }
    if let Some(close) = s.rfind('}') {
        s = s[..=close].to_string();
    }

    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_json_code_fence() {
        let raw = "```json\n{\"foo\": 1}\n```";
        assert_eq!(strip_json_noise(raw), r#"{"foo": 1}"#);
    }

    #[test]
    fn strips_plain_code_fence() {
        let raw = "```\n{\"foo\": 1}\n```";
        assert_eq!(strip_json_noise(raw), r#"{"foo": 1}"#);
    }

    #[test]
    fn strips_leading_prose() {
        let raw = "Here is the JSON:\n{\"foo\": 1}";
        assert_eq!(strip_json_noise(raw), r#"{"foo": 1}"#);
    }

    #[test]
    fn strips_trailing_prose() {
        let raw = "{\"foo\": 1}\n\nHope that helps!";
        assert_eq!(strip_json_noise(raw), r#"{"foo": 1}"#);
    }

    #[test]
    fn strips_think_tags() {
        let raw = "<think>let me think...</think>\n{\"foo\": 1}";
        assert_eq!(strip_json_noise(raw), r#"{"foo": 1}"#);
    }

    #[test]
    fn passthrough_clean_json() {
        let raw = r#"{"name": "chicken", "duration_seconds": 300}"#;
        assert_eq!(strip_json_noise(raw), raw);
    }
}
