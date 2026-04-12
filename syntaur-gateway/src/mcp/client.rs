//! Minimal MCP client over stdio.
//!
//! Implements the subset of the Model Context Protocol needed to:
//!   1. Spawn a child process implementing an MCP server
//!   2. Perform the JSON-RPC `initialize` handshake
//!   3. List the server's tools (`tools/list`)
//!   4. Invoke tools (`tools/call`)
//!
//! Transport is newline-delimited JSON-RPC 2.0 on the child's stdin/stdout
//! and JSON-RPC framing comes from the shared `mcp-protocol` crate so this
//! client and the in-tree Rust MCP servers can never drift on the wire.
//!
//! No automatic respawn — if the server dies, all pending requests fail
//! and subsequent calls return errors. The registry caller can decide
//! whether to mark the server unhealthy and log it.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, info, warn};
use mcp_protocol::{
    ClientInfo, IncomingMessage, JsonRpcNotification, JsonRpcRequest, RequestId, ServerInfo as PServerInfo,
    PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

const REQUEST_TIMEOUT_SECS: u64 = 60;
const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
const WRITER_CHANNEL_DEPTH: usize = 64;

/// JSON-RPC error returned by an MCP server.
#[derive(Debug, Clone)]
pub struct McpError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[mcp:{}] {}", self.code, self.message)
    }
}

/// Server identity returned by `initialize`. Mirrors `mcp_protocol::ServerInfo`
/// but kept here so callers don't need a transitive dep on the protocol crate.
#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

impl From<PServerInfo> for ServerInfo {
    fn from(p: PServerInfo) -> Self {
        Self {
            name: p.name,
            version: p.version,
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>>;

pub struct McpClient {
    pub name: String,
    pub server_info: ServerInfo,
    pub tools: Vec<McpTool>,
    next_id: AtomicU64,
    pending: PendingMap,
    write_tx: mpsc::Sender<String>,
    // Keep the child handle alive for the lifetime of the client. `kill_on_drop`
    // ensures the subprocess is reaped if the McpClient is dropped.
    _child: Mutex<Child>,
}

impl McpClient {
    /// Spawn an MCP server child process and complete the handshake.
    /// Returns a fully-initialized client with `server_info` and `tools` populated.
    pub async fn spawn(
        name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Arc<Self>, String> {
        info!("[mcp:{}] spawning: {} {:?}", name, command, args);

        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn failed for '{}': {}", command, e))?;

        let stdin = child.stdin.take().ok_or("missing child stdin")?;
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (write_tx, mut write_rx) = mpsc::channel::<String>(WRITER_CHANNEL_DEPTH);

        // Writer task: serializes all stdin writes through one task so we never
        // interleave bytes from concurrent callers.
        let name_w = name.clone();
        let mut stdin_w = stdin;
        tokio::spawn(async move {
            while let Some(line) = write_rx.recv().await {
                if let Err(e) = stdin_w.write_all(line.as_bytes()).await {
                    error!("[mcp:{}] write failed: {}", name_w, e);
                    break;
                }
                if let Err(e) = stdin_w.write_all(b"\n").await {
                    error!("[mcp:{}] write newline failed: {}", name_w, e);
                    break;
                }
                if let Err(e) = stdin_w.flush().await {
                    debug!("[mcp:{}] flush failed: {}", name_w, e);
                }
            }
            debug!("[mcp:{}] writer task exiting", name_w);
        });

        // Reader task: demuxes responses by request id and resolves the matching
        // oneshot. Notifications (no id) are logged at debug level.
        let pending_r = Arc::clone(&pending);
        let name_r = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        info!("[mcp:{}] stdout closed (server exited)", name_r);
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<IncomingMessage>(trimmed) {
                            Ok(msg) => Self::handle_message(&name_r, msg, &pending_r).await,
                            Err(e) => warn!(
                                "[mcp:{}] bad JSON: {} — line: {}",
                                name_r,
                                e,
                                &trimmed[..trimmed.len().min(200)]
                            ),
                        }
                    }
                    Err(e) => {
                        error!("[mcp:{}] read error: {}", name_r, e);
                        break;
                    }
                }
            }
            // On exit, fail all pending requests so callers don't hang forever.
            let mut p = pending_r.lock().await;
            for (_, tx) in p.drain() {
                let _ = tx.send(Err(McpError {
                    code: -32603,
                    message: "mcp server exited".to_string(),
                }));
            }
        });

        // Stderr task: log only. We never block on stderr.
        let name_e = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            debug!("[mcp:{}:stderr] {}", name_e, trimmed);
                        }
                    }
                }
            }
        });

        // Build a temporary client just for the handshake. ServerInfo and tools
        // get filled in below before we return the final Arc.
        let temp = Self {
            name: name.clone(),
            server_info: ServerInfo::default(),
            tools: Vec::new(),
            next_id: AtomicU64::new(1),
            pending: Arc::clone(&pending),
            write_tx: write_tx.clone(),
            _child: Mutex::new(child),
        };

        let client_info = ClientInfo {
            name: "syntaur".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };

        // Step 1: initialize handshake (must complete within HANDSHAKE_TIMEOUT_SECS)
        let init_result = timeout(
            Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
            temp.call_method(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": client_info,
                }),
            ),
        )
        .await
        .map_err(|_| format!("initialize timed out after {}s", HANDSHAKE_TIMEOUT_SECS))?
        .map_err(|e| format!("initialize failed: {}", e))?;

        let server_info: ServerInfo = serde_json::from_value::<PServerInfo>(
            init_result
                .get("serverInfo")
                .cloned()
                .unwrap_or_else(|| json!({"name":"?","version":"?"})),
        )
        .unwrap_or_default()
        .into();
        info!(
            "[mcp:{}] connected to {} v{}",
            name, server_info.name, server_info.version
        );

        // Step 2: send the initialized notification (required by spec, no response)
        temp.send_notification("notifications/initialized", json!({}))
            .await;

        // Step 3: list tools
        let tools_result = timeout(
            Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
            temp.call_method("tools/list", json!({})),
        )
        .await
        .map_err(|_| format!("tools/list timed out after {}s", HANDSHAKE_TIMEOUT_SECS))?
        .map_err(|e| format!("tools/list failed: {}", e))?;

        let tools: Vec<McpTool> = tools_result
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        Some(McpTool {
                            name: t.get("name")?.as_str()?.to_string(),
                            description: t
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input_schema: t
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "object"})),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        info!("[mcp:{}] {} tools available", name, tools.len());
        for t in &tools {
            debug!("[mcp:{}]   - {}", name, t.name);
        }

        Ok(Arc::new(Self {
            server_info,
            tools,
            ..temp
        }))
    }

    async fn handle_message(name: &str, msg: IncomingMessage, pending: &PendingMap) {
        match msg {
            IncomingMessage::Response(resp) => {
                let id = match resp.id {
                    RequestId::Number(n) => n as u64,
                    RequestId::String(s) => {
                        debug!("[mcp:{}] string response id {} ignored", name, s);
                        return;
                    }
                };
                let mut p = pending.lock().await;
                if let Some(tx) = p.remove(&id) {
                    if let Some(err) = resp.error {
                        let _ = tx.send(Err(McpError {
                            code: err.code,
                            message: err.message,
                        }));
                    } else {
                        let _ = tx.send(Ok(resp.result.unwrap_or(Value::Null)));
                    }
                } else {
                    debug!("[mcp:{}] orphan response id {}", name, id);
                }
            }
            IncomingMessage::Notification(n) => {
                debug!("[mcp:{}] notification: {}", name, n.method);
            }
            IncomingMessage::Request(r) => {
                debug!("[mcp:{}] unexpected request from server: {}", name, r.method);
            }
        }
    }

    async fn call_method(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let req = JsonRpcRequest::new(id, method, params);
        let line = serde_json::to_string(&req).map_err(|e| McpError {
            code: -32603,
            message: format!("serialize request: {}", e),
        })?;

        if self.write_tx.send(line).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(McpError {
                code: -32603,
                message: "writer channel closed".to_string(),
            });
        }

        match timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(McpError {
                code: -32603,
                message: "response channel dropped".to_string(),
            }),
            Err(_) => {
                // Cleanup so we don't leak the pending entry on timeout.
                self.pending.lock().await.remove(&id);
                Err(McpError {
                    code: -32603,
                    message: format!("request timeout after {}s", REQUEST_TIMEOUT_SECS),
                })
            }
        }
    }

    async fn send_notification(&self, method: &str, params: Value) {
        let n = JsonRpcNotification::new(method, params);
        if let Ok(line) = serde_json::to_string(&n) {
            let _ = self.write_tx.send(line).await;
        }
    }

    /// Invoke a tool by its server-side name (not the namespaced wire name).
    /// Returns the concatenated text content from the response, or an error
    /// if the server returned `isError: true` or any transport-level failure.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .call_method(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await
            .map_err(|e| format!("{}", e))?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let text = result
            .get("content")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut parts = Vec::new();
                for item in arr {
                    if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                        parts.push(t.to_string());
                    } else if let Some(t) = item.get("type").and_then(|v| v.as_str()) {
                        // Non-text content (image, resource, etc.) — preserve a marker.
                        parts.push(format!("[{} content]", t));
                    }
                }
                parts.join("\n")
            })
            .unwrap_or_else(|| result.to_string());

        if is_error {
            Err(text)
        } else {
            Ok(text)
        }
    }
}
