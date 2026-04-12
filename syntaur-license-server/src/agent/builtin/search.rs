use std::time::Instant;

use async_trait::async_trait;
use log::debug;

use crate::agent::{Agent, AgentContext, AgentError, AgentType};
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::task::{TaskCategory, TaskPayload, TaskResult};

const SYSTEM_PROMPT: &str = r#"You are a search and information retrieval specialist. Your role is to:
1. Analyze the user's query to understand what information is needed
2. Formulate precise search-oriented responses
3. Distinguish between facts you know and information that would need to be looked up
4. Provide structured, citation-ready responses

When you have relevant knowledge, provide it directly with confidence indicators.
When the query requires real-time or recent data, clearly state what would need to be searched."#;

/// Sub-agent specialized for internet search and information retrieval.
pub struct SearchAgent;

impl SearchAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Agent for SearchAgent {
    fn id(&self) -> &str {
        "search"
    }

    fn name(&self) -> &str {
        "Search Agent"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Sub
    }

    fn capabilities(&self) -> &[TaskCategory] {
        &[TaskCategory::Search]
    }

    fn description(&self) -> &str {
        "Specialized agent for internet search and information retrieval"
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

        let request = CompletionRequest::simple(&task.instruction)
            .with_system(SYSTEM_PROMPT)
            .with_tags(vec!["search".into()]);

        let preferences = RoutePreferences {
            required_tags: vec!["search".into()],
            ..Default::default()
        };

        let response = ctx
            .backend_router
            .route(&request, &preferences)
            .await
            .or_else(|_| {
                // Fallback: try without tag requirement
                debug!("[search] no search-tagged backend, falling back to general");
                Err(AgentError::BackendFailure("retrying without tags".into()))
            });

        // If tag-filtered route failed, retry without tags
        let response = match response {
            Ok(r) => r,
            Err(_) => {
                let request = CompletionRequest::simple(&task.instruction)
                    .with_system(SYSTEM_PROMPT);
                let preferences = RoutePreferences::default();
                ctx.backend_router
                    .route(&request, &preferences)
                    .await
                    .map_err(|e| AgentError::BackendFailure(e.to_string()))?
            }
        };

        let output = serde_json::json!({
            "content": response.content,
            "source": "llm_knowledge",
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
