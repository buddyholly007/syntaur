//! Sub-agent delegation tool.
//!
//! Allows the primary agent to delegate specialized tasks to focused
//! sub-agents (search, coder, researcher) that run with their own
//! system prompts and return results as tool output.

use async_trait::async_trait;
use log::info;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::config::Config;
use crate::llm::{ChatMessage, LlmChain};

use super::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

// ── Sub-agent system prompts ────────────────────────────────────────────

const SEARCH_PROMPT: &str = r#"You are a search and information retrieval specialist. Your role is to:
1. Analyze the query to understand what information is needed
2. Provide accurate, well-structured answers from your knowledge
3. Distinguish between facts you know confidently and information that may be outdated
4. When you lack current data, clearly state what would need to be looked up

Be concise and cite your reasoning."#;

const CODER_PROMPT: &str = r#"You are an expert software engineer. Your role is to:
1. Write clean, correct, production-quality code
2. Review code for bugs, security issues, and improvements
3. Follow the conventions of the language/framework being used
4. Include appropriate error handling
5. Flag any security concerns

Respond with code and brief explanations. Prefer readability over cleverness."#;

const RESEARCHER_PROMPT: &str = r#"You are a research and analysis specialist. Your role is to:
1. Perform deep analysis of topics, data, and problems
2. Synthesize information from multiple angles
3. Identify patterns, implications, and trade-offs
4. Provide well-structured findings with clear reasoning
5. Flag uncertainties and areas needing further investigation

Be thorough and methodical. Structure your response with clear sections."#;

// ── Delegate tool ───────────────────────────────────────────────────────

pub struct DelegateTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl DelegateTool {
    pub fn new(config: Arc<Config>, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    fn system_prompt(agent_name: &str) -> Result<&'static str, String> {
        match agent_name {
            "search" => Ok(SEARCH_PROMPT),
            "coder" => Ok(CODER_PROMPT),
            "researcher" => Ok(RESEARCHER_PROMPT),
            other => Err(format!(
                "Unknown sub-agent '{}'. Available: search, coder, researcher",
                other
            )),
        }
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to a specialized sub-agent. Use when the task needs focused expertise: 'search' for information retrieval, 'coder' for code generation/review, 'researcher' for deep analysis. The sub-agent runs independently and returns its result."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "enum": ["search", "coder", "researcher"],
                    "description": "Which specialist sub-agent to use"
                },
                "task": {
                    "type": "string",
                    "description": "Clear description of what the sub-agent should do"
                },
                "context": {
                    "type": "string",
                    "description": "Optional additional context from the conversation to include"
                }
            },
            "required": ["agent", "task"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            network: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let agent_name = args
            .get("agent")
            .and_then(|v| v.as_str())
            .ok_or("missing 'agent' parameter")?;
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or("missing 'task' parameter")?;
        let context = args
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let system_prompt = Self::system_prompt(agent_name)?;

        info!(
            "[subagent:{}] delegated by agent={} task_len={}",
            agent_name,
            ctx.agent_id,
            task.len()
        );

        // Build the sub-agent's message list
        let mut messages = vec![ChatMessage::system(system_prompt)];
        if !context.is_empty() {
            messages.push(ChatMessage::system(&format!("Context:\n{}", context)));
        }
        messages.push(ChatMessage::user(task));

        // Use the parent agent's LLM chain (inherits provider priority + load-aware routing)
        let chain = LlmChain::from_config(&self.config, ctx.agent_id, self.client.clone());
        let result = chain
            .call(&messages)
            .await
            .map_err(|e| format!("Sub-agent '{}' failed: {}", agent_name, e))?;

        info!(
            "[subagent:{}] completed, response_len={}",
            agent_name,
            result.len()
        );

        Ok(RichToolResult::text(result))
    }
}
