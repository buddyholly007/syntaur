//! The module-facing tool trait.
//!
//! Module authors implement [`ModuleTool`] to expose tools to the LLM.
//! The gateway wraps these into its internal dispatch system, handling
//! rate limiting, circuit breaking, and approval gates transparently.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::ToolCapabilities;
use crate::types::RichToolResult;

/// Context provided to a module tool during execution.
///
/// This is the public-facing context — it contains only fields that
/// module authors should use. Gateway internals (indexer, circuit
/// breakers, rate limiters) are handled by the dispatch layer.
pub struct ModuleToolContext {
    /// Workspace directory for the active agent.
    pub workspace: PathBuf,
    /// Agent ID that triggered this tool call.
    pub agent_id: String,
    /// User ID of the principal (0 = legacy admin).
    pub user_id: i64,
    /// Shared HTTP client with connection pooling.
    pub http: Arc<reqwest::Client>,
    /// Module-specific configuration from `syntaur.json`.
    pub config: Value,
    /// Module-private persistent data directory.
    pub data_dir: PathBuf,
}

impl ModuleToolContext {
    /// Read a file relative to the workspace.
    pub fn workspace_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.workspace.join(relative)
    }
}

/// A tool that can be provided by a module.
///
/// This is the public API for module authors. The gateway adapts this
/// into its internal `Tool` trait, adding circuit breakers, rate
/// limiting, and approval gates transparently.
#[async_trait]
pub trait ModuleTool: Send + Sync + 'static {
    /// Tool name as the LLM sees it. Must be unique across all modules.
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's arguments.
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    /// Operational capabilities metadata.
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::default()
    }

    /// OpenAI function-calling schema entry. Default builds from
    /// `name()`, `description()`, and `parameters()`.
    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters(),
            }
        })
    }

    /// Execute the tool with parsed arguments.
    async fn execute(
        &self,
        args: Value,
        ctx: &ModuleToolContext,
    ) -> Result<RichToolResult, String>;
}
