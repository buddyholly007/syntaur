//! JSON-RPC 2.0 message types used by MCP.
//!
//! We deliberately model these as `serde_json::Value`-flavored structs rather
//! than fully typing every method's params/result, because new MCP capabilities
//! show up regularly and the wire format is forgiving. Strictly typed wrappers
//! live above this layer (in the client and server crates).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request id. MCP servers we care about always use integer ids,
/// but the spec allows strings. We accept both and let the framework treat
/// them opaquely.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        RequestId::Number(n)
    }
}

impl From<u64> for RequestId {
    fn from(n: u64) -> Self {
        RequestId::Number(n as i64)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        RequestId::String(s)
    }
}

/// JSON-RPC request frame. `params` is optional per spec but virtually always
/// present in MCP traffic, so we represent it as `Value::Null` when absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC notification (no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC response frame. Either `result` or `error` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Catch-all for anything that arrives on the wire. Used by readers that need
/// to dispatch on whether something is a request, response, or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

impl JsonRpcResponse {
    pub fn ok(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: RequestId, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcErrorObject {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

impl JsonRpcRequest {
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

// ---- MCP-specific result shapes ----

/// `serverInfo` block returned by `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// `clientInfo` block sent in `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// One tool entry as returned by `tools/list`. We keep `input_schema` opaque
/// because servers vary in which JSON Schema features they include.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

/// Content block in a `tools/call` response. Most servers only emit text
/// content; we also model image/audio/blob/resource for forward-compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Blob {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        resource: Value,
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}
