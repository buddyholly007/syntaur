//! Trait-based tool extensions.
//!
//! v5 Item 1 collapses the giant match into a uniform funnel that flows
//! through the `Tool` trait. Each built-in tool is a `pub struct FooTool;`
//! with its own `impl Tool for FooTool`.
//!
//! Data types (`Citation`, `Artifact`, `RichToolResult`, `ToolCapabilities`)
//! are defined in `openclaw-sdk` and re-exported here for backward
//! compatibility. The `Tool` trait and `ToolContext` remain gateway-internal
//! because they reference gateway subsystems (indexer, circuit breakers,
//! rate limiters).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::circuit_breaker::CircuitBreaker;
use crate::index::Indexer;
use crate::rate_limit::RateLimiter;

// ── Re-exports from openclaw-sdk ────────────────────────────────────────
//
// These were originally defined here. Now they live in openclaw-sdk so
// module authors can depend on a lightweight crate without pulling in
// the entire gateway. All existing `use crate::tools::extension::*`
// imports continue to work via these re-exports.

pub use openclaw_sdk::types::{Artifact, Citation, RichToolResult};
pub use openclaw_sdk::capabilities::ToolCapabilities;

// ── Gateway-internal context ────────────────────────────────────────────

/// Context handed to a `Tool::execute` call. Holds shared infrastructure
/// so tool impls don't have to be passed every dependency individually.
///
/// Lifetime parameter binds to borrows from the parent `ToolRegistry`.
pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub agent_id: &'a str,
    pub indexer: Option<Arc<Indexer>>,
    pub http: Option<Arc<reqwest::Client>>,
    pub rate_limiter: Option<Arc<Mutex<RateLimiter>>>,
    pub circuit_breakers: Option<Arc<Mutex<std::collections::HashMap<String, CircuitBreaker>>>>,
    pub allowed_scripts: &'a [String],
    pub user_id: i64,
}

// ── Gateway-internal Tool trait ─────────────────────────────────────────

/// A tool that can be registered into the extension HashMap on `ToolRegistry`.
/// Object-safe via `async_trait` so we can store `Arc<dyn Tool>`.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as the LLM sees it.
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &str {
        ""
    }

    /// JSON Schema for the tool's `arguments` parameter.
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    /// Operational metadata used by the uniform funnel.
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::default()
    }

    /// OpenAI function-calling schema entry.
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
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String>;
}
