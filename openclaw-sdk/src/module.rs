//! Module lifecycle trait and supporting types.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::manifest::ModuleManifest;
use crate::tool::ModuleTool;

/// Context provided to a module during initialization.
pub struct ModuleContext {
    /// Module-private persistent data directory
    /// (e.g. `/data/openclaw/modules/<module-id>/data/`).
    pub data_dir: PathBuf,
    /// Module-specific configuration from `openclaw.json`.
    pub config: Value,
    /// Shared HTTP client.
    pub http: Arc<reqwest::Client>,
    /// Gateway base URL for internal API calls (e.g. `http://127.0.0.1:18789`).
    pub gateway_url: String,
}

/// Handle to a background service managed by the init system.
pub struct ServiceHandle {
    /// Service name (used in logs and status).
    pub name: String,
    /// Async task that runs until cancelled.
    pub task: tokio::task::JoinHandle<()>,
}

/// What a module returns from `init()` — its tools, routes, and services.
pub struct ModuleHandle {
    /// Tools to register with the gateway's tool router.
    pub tools: Vec<Box<dyn ModuleTool>>,
    /// Background services to supervise.
    pub services: Vec<ServiceHandle>,
}

impl ModuleHandle {
    /// Convenience for modules that only provide tools.
    pub fn tools_only(tools: Vec<Box<dyn ModuleTool>>) -> Self {
        Self {
            tools,
            services: Vec::new(),
        }
    }

    /// Empty handle (module has no tools or services).
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            services: Vec::new(),
        }
    }
}

/// The module lifecycle trait.
///
/// Core modules implement this and register via `inventory::submit!()`.
/// Extension modules are separate binaries and don't use this trait —
/// they communicate via MCP protocol.
#[async_trait]
pub trait Module: Send + Sync + 'static {
    /// Module manifest (id, version, description, etc).
    fn manifest(&self) -> &ModuleManifest;

    /// Initialize the module. Called once at gateway startup.
    /// Returns a handle with the module's tools and services.
    async fn init(&self, ctx: ModuleContext) -> anyhow::Result<ModuleHandle>;

    /// Graceful shutdown. Called when the gateway is stopping.
    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
