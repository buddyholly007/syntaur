pub mod builtin;
pub mod registry;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::backend::router::BackendRouter;
use crate::task::{TaskCategory, TaskPayload, TaskResult};

/// Whether an agent is a top-level orchestrator or a specialized worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// Top-level agent with its own sub-agent tree.
    Major,
    /// Specialized worker underneath a major agent.
    Sub,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Major => write!(f, "major"),
            Self::Sub => write!(f, "sub"),
        }
    }
}

/// Context provided to agents during execution.
pub struct AgentContext {
    pub backend_router: Arc<BackendRouter>,
    pub conversation_id: Option<String>,
    /// Allow major agents to invoke sub-agents through the registry.
    pub sub_agent_runner: Option<Arc<dyn SubAgentRunner>>,
    /// Optional channel for emitting real-time status events.
    pub event_tx: Option<tokio::sync::mpsc::Sender<crate::task::TaskEvent>>,
}

/// Trait for running sub-agents from within a major agent.
#[async_trait]
pub trait SubAgentRunner: Send + Sync {
    async fn run_sub_agent(
        &self,
        agent_id: &str,
        task: TaskPayload,
    ) -> Result<TaskResult, AgentError>;

    async fn run_parallel(
        &self,
        tasks: Vec<(String, TaskPayload)>,
    ) -> Vec<Result<TaskResult, AgentError>>;
}

/// The standard contract every agent must fulfill.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Unique identifier.
    fn id(&self) -> &str;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Major or sub-agent.
    fn agent_type(&self) -> AgentType;

    /// What task categories this agent can handle.
    fn capabilities(&self) -> &[TaskCategory];

    /// Short description of what this agent does.
    fn description(&self) -> &str;

    /// Which major agent this sub-agent belongs to (None for major agents).
    fn parent_agent_id(&self) -> Option<&str> {
        None
    }

    /// Execute a task and return a result.
    async fn execute(
        &self,
        task: TaskPayload,
        ctx: &AgentContext,
    ) -> Result<TaskResult, AgentError>;
}

/// Errors from agent execution.
#[derive(Debug, Clone)]
pub enum AgentError {
    /// The task category is not supported by this agent.
    UnsupportedTask(String),
    /// Backend failure during execution.
    BackendFailure(String),
    /// The task timed out.
    Timeout,
    /// Internal agent error.
    Internal(String),
    /// All fallback options exhausted.
    AllFallbacksFailed(Vec<String>),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTask(msg) => write!(f, "unsupported task: {}", msg),
            Self::BackendFailure(msg) => write!(f, "backend failure: {}", msg),
            Self::Timeout => write!(f, "agent execution timed out"),
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
            Self::AllFallbacksFailed(errors) => {
                write!(f, "all fallbacks failed: {}", errors.join("; "))
            }
        }
    }
}

impl std::error::Error for AgentError {}
