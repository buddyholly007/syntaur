//! Single-subtask research agent.
//!
//! Each subtask gets a fresh, context-isolated LLM loop with a restricted
//! tool set. The agent sees ONLY its task description — no original query,
//! no other subtasks, no conversation history. This is the Onyx
//! "context-isolated research agent" pattern.
//!
//! Tool budget: 8 rounds max. The orchestrator catches the limit and
//! marks the subtask as truncated.

use std::sync::Arc;
use std::time::Instant;

use log::{info, warn};
use serde_json::Value;

use crate::llm::{ChatMessage, LlmChain, LlmResult};
use crate::tools::extension::Citation;
use crate::tools::{ToolCall, ToolRegistry};

use super::evidence::EvidenceItem;
use super::prompts::SUBTASK_SYSTEM_PROMPT;

const MAX_SUBTASK_ROUNDS: usize = 8;

/// Names of tools the research subtask is allowed to call. Other tools (file_ops,
/// browser, account creation, etc.) are filtered out of the schema we send to the LLM.
const ALLOWED_TOOLS: &[&str] = &[
    "internal_search",
    "web_search",
    "web_fetch",
    "code_execute",
];

/// Run one research subtask. Returns a fully-populated `EvidenceItem` whether
/// the subtask succeeded, errored, or hit the round budget.
pub async fn run_subtask(
    step_index: usize,
    task: String,
    llm_chain: Arc<LlmChain>,
    tool_registry: Arc<ToolRegistry>,
) -> EvidenceItem {
    let started = Instant::now();

    // Restricted tool list — schema only includes the 4 research tools.
    let all_tools = tool_registry.tool_definitions();
    let tools: Vec<Value> = all_tools
        .into_iter()
        .filter(|t| {
            let name = t
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            ALLOWED_TOOLS.contains(&name)
        })
        .collect();

    let mut messages = vec![
        ChatMessage::system(SUBTASK_SYSTEM_PROMPT),
        ChatMessage::user(&task),
    ];

    let mut citations: Vec<Citation> = Vec::new();
    let mut tools_used: Vec<String> = Vec::new();
    let mut rounds_used: usize = 0;
    let mut error: Option<String> = None;
    let mut summary = String::new();

    'outer: for round in 0..MAX_SUBTASK_ROUNDS {
        rounds_used = round + 1;
        let result = match llm_chain.call_raw(&messages, Some(&tools)).await {
            Ok(r) => r,
            Err(e) => {
                error = Some(format!("LLM error round {}: {}", round, e));
                // Best-effort: try to extract a summary from what we have so far.
                // This avoids losing tool-call work when the LLM provider fails
                // mid-loop. We add a system message asking for an immediate
                // text-only answer and try once more (without tools to reduce
                // the chance of another tool-call cycle).
                if round > 0 {
                    messages.push(ChatMessage::system(
                        "An LLM error occurred. Reply with your best final answer NOW based on the tool results you have already received. Do not call any more tools.",
                    ));
                    if let Ok(text) = llm_chain.call(&messages).await {
                        if !text.trim().is_empty() {
                            summary = text;
                        }
                    }
                }
                break;
            }
        };

        match result {
            LlmResult::Text(text) => {
                summary = text;
                break;
            }
            LlmResult::ToolCalls {
                content,
                tool_calls,
            } => {
                messages.push(ChatMessage::assistant_with_tools(&content, tool_calls.clone()));
                for tc in &tool_calls {
                    let id = tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let func = tc.get("function").cloned().unwrap_or_default();
                    let name = func
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args_str = func
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let args: Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));

                    // Hard-stop disallowed tools (LLM might still try to call something
                    // outside the schema if it's seen the name elsewhere).
                    if !ALLOWED_TOOLS.contains(&name.as_str()) {
                        warn!(
                            "[research:subtask {}] LLM called disallowed tool: {}",
                            step_index, name
                        );
                        messages.push(ChatMessage::tool_result(
                            &id,
                            &format!(
                                "Tool '{}' is not available in research subtasks. Allowed: {}",
                                name,
                                ALLOWED_TOOLS.join(", ")
                            ),
                        ));
                        continue;
                    }

                    let call = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: args,
                    };

                    let rich = tool_registry.execute_rich(&call).await;

                    if !tools_used.contains(&name) {
                        tools_used.push(name.clone());
                    }
                    // Bubble citations up
                    citations.extend(rich.citations.iter().cloned());

                    // Send rich content as the tool message body. Truncate hard
                    // to keep the subtask context small.
                    let mut output = rich.to_text();
                    if output.len() > 4000 {
                        output = format!(
                            "{}...\n[truncated — {} chars total]",
                            &output[..3500],
                            output.len()
                        );
                    }
                    messages.push(ChatMessage::tool_result(&id, &output));
                }

                if round + 1 == MAX_SUBTASK_ROUNDS {
                    // Hit the limit — try to extract a final answer
                    messages.push(ChatMessage::system(
                        "Round budget exhausted. Reply with your best final answer NOW based on what you have. Do not call any more tools.",
                    ));
                    let final_result = llm_chain.call(&messages).await;
                    match final_result {
                        Ok(text) => {
                            summary = text;
                            error = Some("subtask round budget exhausted".to_string());
                        }
                        Err(e) => {
                            error = Some(format!("subtask exhausted + final call failed: {}", e));
                        }
                    }
                    break 'outer;
                }
            }
        }
    }

    // De-duplicate citations by (source, external_id)
    let mut seen: std::collections::HashSet<(String, String)> = Default::default();
    citations.retain(|c| seen.insert((c.source.clone(), c.external_id.clone())));

    let item = EvidenceItem {
        step_index,
        task,
        summary,
        citations,
        tools_used,
        duration_ms: started.elapsed().as_millis() as u64,
        rounds_used,
        error,
    };

    info!(
        "[research:subtask {}] done in {}ms, {} rounds, {} citations, error={}",
        step_index,
        item.duration_ms,
        item.rounds_used,
        item.citations.len(),
        item.error.as_deref().unwrap_or("none")
    );

    item
}
