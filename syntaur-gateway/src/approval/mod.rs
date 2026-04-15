//! Approval gates: human-in-the-loop for risky tool calls.
//!
//! How it works:
//!   1. Tools are tagged as "requires approval" via `requires_approval()`
//!   2. When the dispatcher sees such a tool call, it queues a `PendingAction`
//!      in SQLite, sends a Telegram message to the agent's bound chat with
//!      Approve/Deny inline keyboard buttons, and blocks waiting on a
//!      one-shot channel
//!   3. The Telegram poller (in telegram.rs) catches the inline-keyboard
//!      callback_query, looks up the pending action by id, marks it as
//!      approved/denied, and signals the waiting dispatcher
//!   4. On approval, the dispatcher executes the original tool. On denial
//!      (or timeout), it returns an error.
//!
//! Storage: `pending_actions` table on the same index.db, schema v3.

mod store;

pub use store::{PendingAction, PendingActionStore, PendingStatus};

use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use serde_json::Value;
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;

use crate::tools::ToolCall;

const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 600; // 10 minutes

/// Names of tools that require explicit user approval before execution.
/// Covers all destructive, write, exec, and browser-automation tools.
pub const REQUIRES_APPROVAL: &[&str] = &[
    // Shell execution
    "exec", "shell", "run",
    // File modification
    "write", "file_write", "edit", "file_edit",
    // Browser automation (X11 input / form interaction)
    "browser_fill_form", "browser_click", "browser_click_at",
    "browser_hold_at", "browser_execute_js",
    // Account creation / social posting
    "create_email_account", "create_facebook_account",
    "create_instagram_account", "meta_oauth",
    "threads_post", "email_send_account",
    // SMS/phone
    "sms_get_number", "sms_wait_for_code",
];

/// Approval response tier — what scope of approval the user granted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApprovalScope {
    Once,
    Session,
    Always,
    Denied,
}

/// In-process registry of pending approvals waiting on a oneshot channel.
/// Also holds session-scoped approval cache so users aren't nagged
/// for the same tool in the same conversation.
pub struct ApprovalRegistry {
    pending: Mutex<std::collections::HashMap<i64, oneshot::Sender<ApprovalScope>>>,
    /// Session cache: (conversation_id, tool_name) → approved.
    /// Cleared when conversation ends.
    session_approvals: Mutex<std::collections::HashSet<(String, String)>>,
}

impl ApprovalRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(std::collections::HashMap::new()),
            session_approvals: Mutex::new(std::collections::HashSet::new()),
        })
    }

    /// Check if a tool is session-approved for a conversation.
    pub async fn is_session_approved(&self, conversation_id: &str, tool_name: &str) -> bool {
        let map = self.session_approvals.lock().await;
        map.contains(&(conversation_id.to_string(), tool_name.to_string()))
    }

    /// Grant session-scoped approval for a tool in a conversation.
    pub async fn grant_session(&self, conversation_id: &str, tool_name: &str) {
        let mut map = self.session_approvals.lock().await;
        map.insert((conversation_id.to_string(), tool_name.to_string()));
    }

    /// Clear all session approvals for a conversation (on conversation end).
    pub async fn clear_session(&self, conversation_id: &str) {
        let mut map = self.session_approvals.lock().await;
        map.retain(|(cid, _)| cid != conversation_id);
    }

    /// Register a pending action and return a oneshot Receiver to await on.
    pub async fn register(&self, action_id: i64) -> oneshot::Receiver<ApprovalScope> {
        let (tx, rx) = oneshot::channel();
        let mut map = self.pending.lock().await;
        map.insert(action_id, tx);
        rx
    }

    /// Resolve a pending action. Called by the Telegram callback handler.
    pub async fn resolve(&self, action_id: i64, scope: ApprovalScope) -> bool {
        let mut map = self.pending.lock().await;
        if let Some(tx) = map.remove(&action_id) {
            let _ = tx.send(scope);
            true
        } else {
            false
        }
    }

    // Backwards-compatible resolve with bool
    pub async fn resolve_bool(&self, action_id: i64, approved: bool) -> bool {
        self.resolve(action_id, if approved { ApprovalScope::Once } else { ApprovalScope::Denied }).await
    }
}

impl Default for ApprovalRegistry {
    fn default() -> Self {
        Self {
            pending: Mutex::new(std::collections::HashMap::new()),
            session_approvals: Mutex::new(std::collections::HashSet::new()),
        }
    }
}

/// Format a human-readable summary of a tool call for the approval message.
/// Truncates long arguments and pretty-prints JSON.
pub fn summarize_call(call: &ToolCall) -> String {
    let args_str = serde_json::to_string_pretty(&call.arguments).unwrap_or_default();
    let truncated = if args_str.len() > 800 {
        format!("{}...\n[{} chars total]", &args_str[..700], args_str.len())
    } else {
        args_str
    };
    format!("Tool: `{}`\n\nArguments:\n```json\n{}\n```", call.name, truncated)
}

/// Send the approval prompt to Telegram and wait for the user's response.
/// Returns the approval scope (Once, Session, Always, or Denied).
pub async fn request_approval(
    bot_token: &str,
    chat_id: i64,
    action_id: i64,
    call: &ToolCall,
    registry: Arc<ApprovalRegistry>,
    client: &reqwest::Client,
) -> Result<ApprovalScope, String> {
    let summary = summarize_call(call);
    let text = format!(
        "🔐 *Approval requested*\n\n{}\n\nChoose how to proceed:",
        summary
    );

    let keyboard = serde_json::json!({
        "inline_keyboard": [
            [
                {"text": "✅ Once", "callback_data": format!("approve:{}", action_id)},
                {"text": "🔄 Session", "callback_data": format!("approve_session:{}", action_id)},
                {"text": "✅ Always", "callback_data": format!("approve_always:{}", action_id)},
            ],
            [
                {"text": "❌ Deny", "callback_data": format!("deny:{}", action_id)}
            ]
        ]
    });

    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown",
        "reply_markup": keyboard,
    });

    let resp = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("send approval prompt: {}", e))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("telegram error: {}", body));
    }
    info!(
        "[approval] sent prompt for action {} to chat {}",
        action_id, chat_id
    );

    let rx = registry.register(action_id).await;
    match timeout(Duration::from_secs(DEFAULT_APPROVAL_TIMEOUT_SECS), rx).await {
        Ok(Ok(scope)) => {
            info!("[approval] action {} resolved: {:?}", action_id, scope);
            Ok(scope)
        }
        Ok(Err(_)) => {
            warn!("[approval] action {} channel dropped", action_id);
            Err("approval channel dropped".to_string())
        }
        Err(_) => {
            let mut map = registry.pending.lock().await;
            map.remove(&action_id);
            warn!("[approval] action {} timed out", action_id);
            Err(format!(
                "approval timed out after {}s",
                DEFAULT_APPROVAL_TIMEOUT_SECS
            ))
        }
    }
}

/// Suppress unused-import warnings if Value becomes unused.
#[allow(dead_code)]
fn _ensure_value_used() -> Value {
    Value::Null
}
