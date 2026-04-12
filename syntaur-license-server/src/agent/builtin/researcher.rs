use std::time::Instant;

use async_trait::async_trait;

use crate::agent::{Agent, AgentContext, AgentError, AgentType};
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::task::{Message, TaskCategory, TaskPayload, TaskResult};

const SYSTEM_PROMPT: &str = r#"You are a research and analysis specialist. Your role is to:
1. Perform deep analysis of topics, data, and problems
2. Synthesize information from multiple angles
3. Identify patterns, trends, and implications
4. Provide well-structured, thorough research reports
5. Cite reasoning and distinguish between established facts and inferences

Approach each task methodically:
- Break complex questions into components
- Analyze each component thoroughly
- Synthesize findings into a coherent summary
- Flag uncertainties and areas needing further investigation"#;

/// Sub-agent specialized for deep research, analysis, and synthesis.
pub struct ResearcherAgent;

impl ResearcherAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Agent for ResearcherAgent {
    fn id(&self) -> &str {
        "researcher"
    }

    fn name(&self) -> &str {
        "Research Agent"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Sub
    }

    fn capabilities(&self) -> &[TaskCategory] {
        &[TaskCategory::Research]
    }

    fn description(&self) -> &str {
        "Specialized agent for deep research, analysis, and information synthesis"
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

        if let Some(ctx_str) = task.context.as_str() {
            messages.push(Message::system(format!("Research context:\n{}", ctx_str)));
        }

        for msg in &task.messages {
            messages.push(msg.clone());
        }

        if task.messages.is_empty() {
            messages.push(Message::user(&task.instruction));
        }

        let request = CompletionRequest::simple("")
            .with_system(SYSTEM_PROMPT)
            .with_messages(messages)
            .with_max_tokens(4096); // Research tasks often need longer outputs

        let preferences = RoutePreferences::default();

        let response = ctx
            .backend_router
            .route(&request, &preferences)
            .await
            .map_err(|e| AgentError::BackendFailure(e.to_string()))?;

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
