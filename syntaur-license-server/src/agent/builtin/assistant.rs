use std::time::Instant;

use async_trait::async_trait;
use log::{debug, info};

use crate::agent::{Agent, AgentContext, AgentError, AgentType};
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::task::{Message, TaskCategory, TaskPayload, TaskResult};

/// Build the system prompt with time, user context, and category hints.
fn build_system_prompt(category: &TaskCategory, user_context: Option<&str>) -> String {
    let now = chrono::Local::now();
    let time_str = now.format("%I:%M %p").to_string();
    let date_str = now.format("%A, %B %e, %Y").to_string();

    let mut prompt = String::from(
        "You are Syntaur, an intelligent AI assistant that is genuinely helpful, \
         precise, and adapts to how each person communicates.\n\n",
    );

    // Category-specific behavior
    match category {
        TaskCategory::Coding => {
            prompt.push_str(
                "The user needs help with code. Write clean, correct, production-quality code. \
                 Use the appropriate language conventions. Include error handling. \
                 Only add comments where the logic isn't self-evident.\n\n",
            );
        }
        TaskCategory::Research => {
            prompt.push_str(
                "The user needs deep analysis. Be thorough and methodical. Structure your \
                 response with clear sections. Distinguish facts from inferences. \
                 Flag uncertainties.\n\n",
            );
        }
        TaskCategory::Search => {
            prompt.push_str(
                "The user needs information. Be direct and cite your reasoning. \
                 If you lack current data, say so clearly.\n\n",
            );
        }
        _ => {
            prompt.push_str(
                "RULES:\n\
                 - Be direct. Lead with the answer, not the reasoning.\n\
                 - Match the user's tone — casual if they're casual, detailed if they're detailed.\n\
                 - If a task needs specialized work, indicate [search], [code], or [research] \
                   so the orchestrator can delegate to the right specialist.\n\
                 - If you don't know something, say so. Never fabricate.\n\n",
            );
        }
    }

    // User context
    if let Some(ctx) = user_context {
        prompt.push_str(&format!("About the user: {}\n\n", ctx));
    }

    // Time injection
    prompt.push_str(&format!("Current time: {} on {}.\n", time_str, date_str));

    prompt
}

/// Per-category max_tokens defaults.
fn max_tokens_for_category(category: &TaskCategory) -> u32 {
    match category {
        TaskCategory::Coding => 8192,
        TaskCategory::Research => 8192,
        TaskCategory::Search => 2048,
        TaskCategory::Conversation => 4096,
        TaskCategory::Planning => 4096,
        _ => 4096,
    }
}

/// The primary major agent — handles general conversation and coordinates
/// delegation to sub-agents through the orchestrator.
pub struct AssistantAgent;

impl AssistantAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Agent for AssistantAgent {
    fn id(&self) -> &str {
        "assistant"
    }

    fn name(&self) -> &str {
        "Syntaur Assistant"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Major
    }

    fn capabilities(&self) -> &[TaskCategory] {
        &[TaskCategory::Conversation, TaskCategory::Planning]
    }

    fn description(&self) -> &str {
        "Primary AI assistant for general conversation, reasoning, and task coordination"
    }

    async fn execute(
        &self,
        task: TaskPayload,
        ctx: &AgentContext,
    ) -> Result<TaskResult, AgentError> {
        let start = Instant::now();

        // Extract user context from task metadata (injected by API layer)
        let user_context = task.metadata.get("user_context").cloned();
        let system_prompt =
            build_system_prompt(&task.category, user_context.as_deref());

        // Build messages
        let mut messages = Vec::new();
        for msg in &task.messages {
            messages.push(msg.clone());
        }
        if task.messages.is_empty()
            || task
                .messages
                .last()
                .map(|m| m.content != task.instruction)
                .unwrap_or(true)
        {
            messages.push(Message::user(&task.instruction));
        }

        let max_tokens = max_tokens_for_category(&task.category);

        let request = CompletionRequest::simple("")
            .with_system(&system_prompt)
            .with_messages(messages)
            .with_max_tokens(max_tokens);

        let preferences = RoutePreferences::default();

        // Emit thinking status
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx
                .send(crate::task::TaskEvent::Status {
                    message: "Thinking...".into(),
                })
                .await;
        }

        let response = ctx
            .backend_router
            .route(&request, &preferences)
            .await
            .map_err(|e| AgentError::BackendFailure(e.to_string()))?;

        debug!(
            "[assistant] completed via {} in {:.1}ms",
            response.backend_id,
            start.elapsed().as_secs_f64() * 1000.0
        );

        // Check if the response indicates delegation is needed
        let content = &response.content;
        let mut sub_results = Vec::new();

        if let Some(runner) = &ctx.sub_agent_runner {
            if should_delegate_search(content) {
                let query = extract_search_query(content);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx
                        .send(crate::task::TaskEvent::AgentStart {
                            agent_id: "search".into(),
                            task_summary: format!(
                                "Searching: {}",
                                &query[..query.len().min(60)]
                            ),
                        })
                        .await;
                }
                info!("[assistant] delegating search sub-task");
                let sub_task =
                    TaskPayload::new(TaskCategory::Search, &query).with_parent(task.id);
                let sub_start = Instant::now();
                if let Ok(result) = runner.run_sub_agent("search", sub_task).await {
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx
                            .send(crate::task::TaskEvent::AgentDone {
                                agent_id: "search".into(),
                                duration_ms: sub_start.elapsed().as_millis() as u64,
                            })
                            .await;
                    }
                    sub_results.push(result);
                }
            }

            if should_delegate_coding(content) {
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx
                        .send(crate::task::TaskEvent::AgentStart {
                            agent_id: "coder".into(),
                            task_summary: format!(
                                "Coding: {}",
                                &task.instruction[..task.instruction.len().min(60)]
                            ),
                        })
                        .await;
                }
                info!("[assistant] delegating coding sub-task");
                let sub_task = TaskPayload::new(TaskCategory::Coding, &task.instruction)
                    .with_parent(task.id);
                let sub_start = Instant::now();
                if let Ok(result) = runner.run_sub_agent("coder", sub_task).await {
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx
                            .send(crate::task::TaskEvent::AgentDone {
                                agent_id: "coder".into(),
                                duration_ms: sub_start.elapsed().as_millis() as u64,
                            })
                            .await;
                    }
                    sub_results.push(result);
                }
            }
        }

        // Build final output — never return empty
        let final_content = if content.trim().is_empty() && sub_results.is_empty() {
            "I'm not sure how to respond to that. Could you rephrase?".to_string()
        } else {
            content.clone()
        };

        let output = if sub_results.is_empty() {
            serde_json::json!({
                "content": final_content,
                "model": response.model,
            })
        } else {
            let sub_outputs: Vec<_> = sub_results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "agent": r.agent_id,
                        "output": r.output,
                    })
                })
                .collect();
            serde_json::json!({
                "content": final_content,
                "model": response.model,
                "sub_results": sub_outputs,
            })
        };

        let mut result = TaskResult::success(
            task.id,
            output,
            self.id(),
            &response.backend_id,
            start.elapsed(),
        );
        result.tokens_used = response.tokens;

        Ok(result)
    }
}

fn should_delegate_search(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("[search]") || lower.contains("[web search]") || lower.contains("[lookup]")
}

fn should_delegate_coding(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("[code]") || lower.contains("[coding]") || lower.contains("[implement]")
}

fn extract_search_query(content: &str) -> String {
    let lower = content.to_lowercase();
    for marker in &["[search]", "[web search]", "[lookup]"] {
        if let Some(pos) = lower.find(marker) {
            let after = &content[pos + marker.len()..];
            let query = after
                .lines()
                .next()
                .unwrap_or(after)
                .trim()
                .trim_matches(|c: char| c == ':' || c == '"' || c == '\'' || c.is_whitespace());
            if !query.is_empty() {
                return query.to_string();
            }
        }
    }
    content.to_string()
}
