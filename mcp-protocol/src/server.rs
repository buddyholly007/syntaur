//! Reusable MCP **server** scaffolding.
//!
//! `mcp-server-filesystem-rs` and `mcp-server-search-rs` are both built on
//! top of `run_stdio_server`: they implement the `ServerHandler` trait to
//! describe their tool list and provide a `call_tool` dispatcher, and
//! `run_stdio_server` handles the JSON-RPC framing, `initialize` handshake,
//! `tools/list`, and error formatting.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::BufReader;

use crate::error_codes;
use crate::messages::{
    ContentBlock, IncomingMessage, JsonRpcRequest, JsonRpcResponse, ServerInfo, ToolListEntry,
};
use crate::stdio::{read_line, write_frame};
use crate::{McpError, PROTOCOL_VERSION};

/// Static description of one tool that a server exposes.
///
/// `input_schema` is a JSON Schema (OpenAPI-style object schema) describing
/// the tool's `arguments` shape. The MCP client passes this verbatim to the
/// LLM as part of its function-calling tools array.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

impl ToolDef {
    pub fn to_list_entry(&self) -> ToolListEntry {
        ToolListEntry {
            name: self.name.to_string(),
            description: self.description.to_string(),
            input_schema: self.input_schema.clone(),
        }
    }
}

/// Result of a tool call. Servers usually return one or more `Text` blocks;
/// `is_error: true` lets the client distinguish handler errors from
/// transport errors.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

impl ToolCallResult {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: s.into() }],
            is_error: false,
        }
    }

    pub fn error(s: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: s.into() }],
            is_error: true,
        }
    }

    pub fn into_value(self) -> Value {
        let content: Vec<Value> = self
            .content
            .into_iter()
            .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
            .collect();
        json!({
            "content": content,
            "isError": self.is_error,
        })
    }
}

/// Trait implemented by each concrete MCP server. The runtime is shared via
/// `Arc<H>` so handler implementations can keep their own internal state
/// (e.g. allowed directories, HTTP clients) and stay `Send + Sync`.
#[async_trait]
pub trait ServerHandler: Send + Sync + 'static {
    /// Returned in the `initialize` response.
    fn server_info(&self) -> ServerInfo;

    /// Static list of tools this server exposes. Called once on startup and
    /// then on every `tools/list` request.
    fn tools(&self) -> Vec<ToolDef>;

    /// Dispatch a tool call. Implementations should never panic; return a
    /// `ToolCallResult::error` for handler-level failures.
    async fn call_tool(&self, name: &str, arguments: Value) -> ToolCallResult;
}

/// Drive an MCP server on the current process's stdin/stdout until EOF.
///
/// Reads requests one line at a time, dispatches `initialize`/`tools/list`/
/// `tools/call`/`ping` on its own, and forwards everything else to the
/// handler via `call_tool`. Logs to stderr; stdout is reserved for the
/// JSON-RPC stream.
pub async fn run_stdio_server<H: ServerHandler>(handler: Arc<H>) -> Result<(), McpError> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    loop {
        let line = match read_line(&mut reader).await? {
            Some(l) => l,
            None => {
                log::info!("[mcp-server] stdin closed, shutting down");
                return Ok(());
            }
        };

        let parsed: IncomingMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("[mcp-server] bad json on stdin: {} :: {}", e, line);
                continue;
            }
        };

        match parsed {
            IncomingMessage::Request(req) => {
                let resp = dispatch_request(&handler, req).await;
                if let Err(e) = write_frame(&mut stdout, &resp).await {
                    log::error!("[mcp-server] write_frame failed: {}", e);
                    return Err(e);
                }
            }
            IncomingMessage::Notification(notif) => {
                // Notifications don't get responses. We log + ignore.
                log::debug!("[mcp-server] notification: {}", notif.method);
            }
            IncomingMessage::Response(_) => {
                // Servers don't expect responses on stdin (we don't send
                // requests to the client). Just log and skip.
                log::debug!("[mcp-server] unexpected response on stdin");
            }
        }
    }
}

async fn dispatch_request<H: ServerHandler>(
    handler: &Arc<H>,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            let info = handler.server_info();
            let result = json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": info.name,
                    "version": info.version,
                }
            });
            JsonRpcResponse::ok(req.id, result)
        }
        "ping" => JsonRpcResponse::ok(req.id, json!({})),
        "tools/list" => {
            let tools: Vec<Value> = handler
                .tools()
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();
            JsonRpcResponse::ok(req.id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = match req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                Some(n) => n,
                None => {
                    return JsonRpcResponse::err(
                        req.id,
                        error_codes::INVALID_PARAMS,
                        "missing 'name' parameter",
                    );
                }
            };
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = handler.call_tool(&name, arguments).await;
            JsonRpcResponse::ok(req.id, result.into_value())
        }
        _ => JsonRpcResponse::err(
            req.id,
            error_codes::METHOD_NOT_FOUND,
            format!("unknown method: {}", req.method),
        ),
    }
}
