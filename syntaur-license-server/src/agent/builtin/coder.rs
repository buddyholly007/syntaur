use std::time::Instant;

use async_trait::async_trait;

use crate::agent::{Agent, AgentContext, AgentError, AgentType};
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::task::{Message, TaskCategory, TaskPayload, TaskResult};

const SYSTEM_PROMPT: &str = r#"You are an expert software engineer. Your role is to:
1. Write clean, correct, production-quality code
2. Review code for bugs, security issues, and improvements
3. Explain code and architectural decisions clearly
4. Suggest appropriate data structures, algorithms, and patterns

Rules:
- Always include error handling appropriate to the language
- Prefer readability over cleverness
- Follow the conventions of the language/framework being used
- When generating code, include only the code and brief explanatory comments
- Flag any security concerns you notice"#;

/// Sub-agent specialized for code generation, review, and analysis.
pub struct CoderAgent;

impl CoderAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Agent for CoderAgent {
    fn id(&self) -> &str {
        "coder"
    }

    fn name(&self) -> &str {
        "Coder Agent"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Sub
    }

    fn capabilities(&self) -> &[TaskCategory] {
        &[TaskCategory::Coding]
    }

    fn description(&self) -> &str {
        "Specialized agent for code generation, review, and software engineering tasks"
    }

    fn parent_agent_id(&self) -> Option<&str> {
        Some("assistant")
    }

    async fn execute(
        &self,
        task: TaskPayload,
        ctx: &AgentContext,
    ) -> Result<TaskResult, AgentError> {
        let start = Instant::now();

        let mut messages = Vec::new();

        // Include any context from parent task
        if let Some(ctx_str) = task.context.as_str() {
            messages.push(Message::system(format!("Context:\n{}", ctx_str)));
        }

        // Include conversation history
        for msg in &task.messages {
            messages.push(msg.clone());
        }

        if task.messages.is_empty() {
            messages.push(Message::user(&task.instruction));
        }

        let request = CompletionRequest::simple("")
            .with_system(SYSTEM_PROMPT)
            .with_messages(messages)
            .with_tags(vec!["coding".into()]);

        let preferences = RoutePreferences {
            required_tags: vec!["coding".into()],
            ..Default::default()
        };

        // Try with coding tag first, fall back to any backend
        let response = match ctx.backend_router.route(&request, &preferences).await {
            Ok(r) => r,
            Err(_) => {
                let request = CompletionRequest::simple("")
                    .with_system(SYSTEM_PROMPT)
                    .with_messages(
                        task.messages
                            .iter()
                            .cloned()
                            .chain(std::iter::once(Message::user(&task.instruction)))
                            .collect(),
                    );
                ctx.backend_router
                    .route(&request, &RoutePreferences::default())
                    .await
                    .map_err(|e| AgentError::BackendFailure(e.to_string()))?
            }
        };

        let output = serde_json::json!({
            "content": response.content,
            "model": response.model,
        });

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
