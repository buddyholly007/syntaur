//! Shared MCP (Model Context Protocol) types and helpers.
//!
//! This crate carries the JSON-RPC framing, request/response/notification
//! types, error codes, and the stdio framing helpers used by both:
//!   * Syntaur's MCP **client** (spawning servers as children)
//!   * `mcp-server-filesystem-rs` and `mcp-server-search-rs` **servers**
//!
//! Wire format is newline-delimited JSON-RPC 2.0. The protocol version we
//! speak is `2024-11-05` (the same the Anthropic SDKs default to).

pub mod messages;
pub mod stdio;
pub mod server;

pub use messages::*;
pub use server::{ServerHandler, ToolDef, run_stdio_server};

/// MCP protocol version this crate speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC error codes used by MCP. Matches the spec section 5.1.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// Top-level error returned by both client- and server-side helpers.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rpc error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("transport closed")]
    TransportClosed,
    #[error("timeout")]
    Timeout,
    #[error("{0}")]
    Other(String),
}

impl McpError {
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self::Rpc {
            code,
            message: message.into(),
        }
    }
}
