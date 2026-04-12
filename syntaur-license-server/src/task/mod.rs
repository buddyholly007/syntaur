pub mod executor;

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Categories of work that agents can perform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    Conversation,
    Search,
    Coding,
    Research,
    Planning,
    ToolExecution,
    Custom(String),
}

impl fmt::Display for TaskCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conversation => write!(f, "conversation"),
            Self::Search => write!(f, "search"),
            Self::Coding => write!(f, "coding"),
            Self::Research => write!(f, "research"),
            Self::Planning => write!(f, "planning"),
            Self::ToolExecution => write!(f, "tool_execution"),
            Self::Custom(s) => write!(f, "custom:{}", s),
        }
    }
}

/// Payload sent to an agent for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPayload {
    pub id: Uuid,
    pub category: TaskCategory,
    pub instruction: String,
    #[serde(default)]
    pub context: serde_json::Value,
    #[serde(with = "crate::config::duration_secs")]
    pub timeout: Duration,
    pub parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Conversation history for context (role, content pairs).
    #[serde(default)]
    pub messages: Vec<Message>,
}

impl TaskPayload {
    pub fn new(category: TaskCategory, instruction: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            category,
            instruction: instruction.into(),
            context: serde_json::Value::Null,
            timeout: Duration::from_secs(120),
            parent_task_id: None,
            metadata: HashMap::new(),
            messages: Vec::new(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_parent(mut self, parent_id: Uuid) -> Self {
        self.parent_task_id = Some(parent_id);
        self
    }

    pub fn with_context(mut self, ctx: serde_json::Value) -> Self {
        self.context = ctx;
        self
    }

    pub fn with_messages(mut self, msgs: Vec<Message>) -> Self {
        self.messages = msgs;
        self
    }
}

/// Real-time status events emitted during task execution.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum TaskEvent {
    /// A sub-agent has been invoked.
    #[serde(rename = "agent_start")]
    AgentStart { agent_id: String, task_summary: String },
    /// A sub-agent completed.
    #[serde(rename = "agent_done")]
    AgentDone { agent_id: String, duration_ms: u64 },
    /// Thinking / planning phase.
    #[serde(rename = "status")]
    Status { message: String },
}

/// Status of a completed task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Success,
    Failed,
    Timeout,
    Cancelled,
}

/// Result returned after agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub status: TaskStatus,
    pub output: serde_json::Value,
    #[serde(with = "crate::config::duration_secs")]
    pub duration: Duration,
    pub agent_id: String,
    pub backend_id: String,
    pub tokens_used: Option<TokenUsage>,
}

impl TaskResult {
    pub fn success(task_id: Uuid, output: serde_json::Value, agent_id: &str, backend_id: &str, duration: Duration) -> Self {
        Self {
            task_id,
            status: TaskStatus::Success,
            output,
            duration,
            agent_id: agent_id.to_string(),
            backend_id: backend_id.to_string(),
            tokens_used: None,
        }
    }

    pub fn failed(task_id: Uuid, error: &str, agent_id: &str, duration: Duration) -> Self {
        Self {
            task_id,
            status: TaskStatus::Failed,
            output: serde_json::json!({ "error": error }),
            duration,
            agent_id: agent_id.to_string(),
            backend_id: String::new(),
            tokens_used: None,
        }
    }

    pub fn output_text(&self) -> Option<&str> {
        self.output.as_str()
            .or_else(|| self.output.get("content").and_then(|v| v.as_str()))
            .or_else(|| self.output.get("text").and_then(|v| v.as_str()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: MessageRole::System, content: content.into(), agent_id: None }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: MessageRole::User, content: content.into(), agent_id: None }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: MessageRole::Assistant, content: content.into(), agent_id: None }
    }
}
