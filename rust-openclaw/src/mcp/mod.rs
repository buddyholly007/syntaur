//! MCP (Model Context Protocol) integration.
//!
//! Spawns one child process per configured MCP server, handshakes,
//! lists each server's tools, and exposes them under namespaced names so
//! they can coexist with syntaur's built-in tool registry.
//!
//! Wire-format tool names are `mcp__<server>__<tool>` (double underscore
//! separators) because OpenAI function names cannot contain `/`. The
//! human-readable form is `mcp/<server>/<tool>` and is used in logs.

mod client;

pub use client::{McpClient, McpError, McpTool, ServerInfo};

use std::collections::HashMap;
use std::sync::Arc;

use log::{error, info, warn};
use serde::Deserialize;
use serde_json::{json, Value};

/// Wire-format prefix on tool names for MCP-routed calls.
const WIRE_PREFIX: &str = "mcp__";
/// Internal prefix used in logs and config.
const NAMESPACE_PREFIX: &str = "mcp/";

/// Per-server config block under `config.mcp.servers["name"]`.
///
/// Each server entry in `syntaur.json` looks like:
/// ```json
/// "syntaur-server-fs": {
///     "command": "ssh",
///     "args": ["sean@192.168.1.35", "mcp-server-filesystem", "/home/sean/.openclaw"],
///     "env": {},
///     "enabled": true
/// }
/// ```
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub enabled: bool,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            enabled: true,
        }
    }
}

/// One indexed tool entry in the registry. Holds both wire and namespaced
/// names so we can map either way without re-parsing.
#[derive(Clone, Debug)]
pub struct McpToolEntry {
    pub server: String,
    pub original_name: String,
    pub namespaced_name: String,
    pub wire_name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Holds connected MCP clients and their flattened tool list.
/// Constructed once at startup; cloned around as `Arc<McpRegistry>`.
pub struct McpRegistry {
    clients: HashMap<String, Arc<McpClient>>,
    tools: Vec<McpToolEntry>,
}

impl McpRegistry {
    /// Empty registry — used when MCP is disabled or no servers are configured.
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            clients: HashMap::new(),
            tools: Vec::new(),
        })
    }

    /// Spawn all enabled servers from `config.mcp.servers` and build the registry.
    /// Servers that fail to spawn are logged and skipped — startup never fails
    /// because of one bad MCP server.
    pub async fn from_config(servers: &HashMap<String, Value>) -> Arc<Self> {
        let mut clients: HashMap<String, Arc<McpClient>> = HashMap::new();
        let mut tools: Vec<McpToolEntry> = Vec::new();

        if servers.is_empty() {
            info!("[mcp] no servers configured");
            return Self::empty();
        }

        for (name, raw) in servers {
            let cfg: McpServerConfig = match serde_json::from_value(raw.clone()) {
                Ok(c) => c,
                Err(e) => {
                    warn!("[mcp] bad config for '{}': {}", name, e);
                    continue;
                }
            };
            if !cfg.enabled {
                info!("[mcp] '{}' disabled in config — skipping", name);
                continue;
            }
            if cfg.command.is_empty() {
                warn!("[mcp] '{}' has no command — skipping", name);
                continue;
            }

            match McpClient::spawn(name.clone(), &cfg.command, &cfg.args, &cfg.env).await {
                Ok(client) => {
                    for tool in &client.tools {
                        let namespaced = format!("{}{}/{}", NAMESPACE_PREFIX, name, tool.name);
                        let wire = format!("{}{}__{}", WIRE_PREFIX, name, tool.name)
                            .replace('-', "_");
                        // OpenAI function names allow [a-zA-Z0-9_-] up to 64 chars.
                        // We strip dashes from the wire name to avoid LLM confusion;
                        // server lookup uses the saved fields, not the wire name.
                        tools.push(McpToolEntry {
                            server: name.clone(),
                            original_name: tool.name.clone(),
                            namespaced_name: namespaced,
                            wire_name: wire,
                            description: tool.description.clone(),
                            input_schema: tool.input_schema.clone(),
                        });
                    }
                    clients.insert(name.clone(), client);
                }
                Err(e) => {
                    error!("[mcp] failed to start '{}': {}", name, e);
                }
            }
        }

        info!(
            "[mcp] {} server(s) connected, {} total tool(s) registered",
            clients.len(),
            tools.len()
        );

        Arc::new(Self { clients, tools })
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn tools(&self) -> &[McpToolEntry] {
        &self.tools
    }

    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Convert MCP tool entries to OpenAI function-calling schema entries
    /// suitable for splicing into the LLM's `tools` array.
    pub fn tool_definitions(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                let desc = if t.description.is_empty() {
                    format!("[mcp:{}] {}", t.server, t.original_name)
                } else {
                    format!("[mcp:{}] {}", t.server, t.description)
                };
                let parameters = if t.input_schema.is_object() {
                    t.input_schema.clone()
                } else {
                    json!({"type": "object", "properties": {}})
                };
                json!({
                    "type": "function",
                    "function": {
                        "name": t.wire_name,
                        "description": desc,
                        "parameters": parameters,
                    }
                })
            })
            .collect()
    }

    /// True if the given tool name routes to MCP. Accepts both wire format
    /// (`mcp__server__tool`) and namespaced format (`mcp/server/tool`).
    pub fn is_mcp_tool(name: &str) -> bool {
        name.starts_with(WIRE_PREFIX) || name.starts_with(NAMESPACE_PREFIX)
    }

    /// Execute an MCP tool call. Looks up the entry by wire name (preferred)
    /// or namespaced name (fallback), routes to the correct server's client.
    pub async fn execute(&self, name: &str, args: Value) -> Result<String, String> {
        // Find the tool entry by wire or namespaced name
        let entry = self
            .tools
            .iter()
            .find(|t| t.wire_name == name || t.namespaced_name == name)
            .ok_or_else(|| format!("unknown MCP tool: {}", name))?;

        let client = self
            .clients
            .get(&entry.server)
            .ok_or_else(|| format!("server '{}' not connected", entry.server))?;

        client.call_tool(&entry.original_name, args).await
    }
}
