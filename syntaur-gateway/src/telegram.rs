use crate::config::TelegramAccount;
use crate::llm::{ChatMessage, LlmChain};
use log::{debug, error, info, warn};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const TG_API: &str = "https://api.telegram.org";
const MAX_MESSAGE_LEN: usize = 4000;

async fn run_llm_with_tools(
    llm_chain: &LlmChain,
    messages: &mut Vec<ChatMessage>,
    bot: &TelegramBot,
    client: &Client,
    workspace: &std::path::Path,
    mcp: Option<Arc<crate::mcp::McpRegistry>>,
    approval_ctx: Option<crate::tools::ApprovalContext>,
    rate_limiter: Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>,
    circuit_breakers: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, crate::circuit_breaker::CircuitBreaker>>,
    >,
    config: Option<Arc<crate::config::Config>>,
) -> String {
    let mut tool_registry = crate::tools::ToolRegistry::with_mcp(workspace.to_path_buf(), mcp);
    if let Some(ctx) = approval_ctx {
        tool_registry.set_approval(ctx);
    }
    tool_registry.set_infra(rate_limiter, circuit_breakers);
    // Inject sub-agent delegation tool
    if let Some(cfg) = config {
        let delegate: Arc<dyn crate::tools::extension::Tool> =
            Arc::new(crate::tools::subagent::DelegateTool::new(cfg, client.clone()));
        tool_registry.add_extension_tools(&[delegate]);
    }
    let tools = tool_registry.tool_definitions();
    let max_tool_rounds = 30;

    for round in 0..max_tool_rounds {
        let result = match llm_chain.call_raw(messages, Some(&tools)).await {
            Ok(r) => r,
            Err(e) => {
                error!("[tg:{}] LLM error: {}", bot.account_id, e);
                return format!("Sorry, I encountered an error: {}", e);
            }
        };

        match result {
            crate::llm::LlmResult::Text(text) => {
                info!("[tg:{}] LLM text response (round {}): {} chars", bot.account_id, round, text.len());
                return text;
            }
            crate::llm::LlmResult::ToolCalls { content, tool_calls } => {
                info!("[tg:{}] LLM requested {} tool call(s) (round {})", bot.account_id, tool_calls.len(), round);

                // Add assistant message with tool calls to history
                messages.push(ChatMessage::assistant_with_tools(&content, tool_calls.clone()));

                // Execute each tool call
                for tc in &tool_calls {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let func = tc.get("function").cloned().unwrap_or_default();
                    let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let args_str = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                    let args: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));

                    info!("[tg:{}] Executing tool: {}({})", bot.account_id, name, &args_str[..args_str.len().min(100)]);

                    let tool_call = crate::tools::ToolCall { id: id.clone(), name: name.clone(), arguments: args };
                    let result = tool_registry.execute(&tool_call).await;

                    info!("[tg:{}] Tool {}: success={}, output={} chars", bot.account_id, name, result.success, result.output.len());

                    // Truncate large tool results to prevent context bloat
                    let mut output = result.output;
                    if output.len() > 1500 {
                        output = format!("{}...\n[truncated — {} chars total]", &output[..1200], output.len());
                    }

                    // Add round budget warning when approaching limit
                    let remaining = max_tool_rounds - round - 1;
                    if remaining <= 8 && remaining > 0 {
                        output.push_str(&format!("\n\n[Round {}/{} — {} remaining. Finish your task or report status.]", round + 1, max_tool_rounds, remaining));
                    }

                    messages.push(ChatMessage::tool_result(&id, &output));
                }

                // Continue loop — LLM will process tool results and respond
                // Send typing indicator while we loop
                tg_send_typing(client, &bot.token, bot.allow_from.first().copied().unwrap_or(0)).await;
            }
        }
    }

    // Hit max rounds — force a final text-only call (no tools) so LLM must produce text
    warn!("[tg:{}] Maximum tool rounds ({}) — forcing text-only response", bot.account_id, max_tool_rounds);
    messages.push(ChatMessage::system("You have used the maximum number of tool calls. Respond now with a text answer based on the information you have gathered. Do NOT request any more tools."));

    match llm_chain.call(messages).await {
        Ok(text) => {
            info!("[tg:{}] Forced text response: {} chars", bot.account_id, text.len());
            text
        }
        Err(e) => {
            error!("[tg:{}] Final LLM call failed: {}", bot.account_id, e);
            format!("Sorry, I encountered an error: {}", e)
        }
    }
}

fn log_message(account_id: &str, agent_id: &str, user_id: i64, user_text: &str, response: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let dir = format!("{}/.syntaur", home);
    let log_path = format!("{}/messages.jsonl", dir);

    let entry = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "bot": account_id,
        "agent": agent_id,
        "user_id": user_id,
        "user": user_text,
        "assistant": response,
    });

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut file) = opts.open(&log_path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", entry);
    }

    // Rotate: if file > 1MB, rename to messages-YYYY-MM-DD.jsonl
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1_048_576 {
            let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let archive = format!("{}/messages-{}.jsonl", dir, date);
            let _ = std::fs::rename(&log_path, &archive);

            // Clean up archives older than 7 days
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("messages-") && name.ends_with(".jsonl") {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(modified) = meta.modified() {
                                if modified.elapsed().map_or(false, |d| d.as_secs() > 7 * 86400) {
                                    let _ = std::fs::remove_file(entry.path());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Telegram Types ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TgResponse {
    ok: bool,
    result: Option<Vec<TgUpdate>>,
}

#[derive(Deserialize, Clone)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
    callback_query: Option<TgCallbackQuery>,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
struct TgCallbackQuery {
    id: String,
    from: TgUser,
    data: Option<String>,
}

#[derive(Deserialize, Clone)]
struct TgMessage {
    text: Option<String>,
    chat: TgChat,
    from: Option<TgUser>,
}

#[derive(Deserialize, Clone)]
struct TgChat {
    id: i64,
}

#[derive(Deserialize, Clone)]
struct TgUser {
    id: i64,
    first_name: Option<String>,
}

// ── Telegram API Functions ──────────────────────────────────────────────────

fn clean_markdown(text: &str) -> String {
    let mut cleaned = text.to_string();
    // Smart quotes → plain quotes
    cleaned = cleaned.replace('\u{2018}', "'"); // '
    cleaned = cleaned.replace('\u{2019}', "'"); // '
    cleaned = cleaned.replace('\u{201C}', "\""); // "
    cleaned = cleaned.replace('\u{201D}', "\""); // "
    cleaned = cleaned.replace('\u{2014}', " - "); // em dash
    cleaned = cleaned.replace('\u{2013}', " - "); // en dash
    // Strip **bold** markers
    cleaned = cleaned.replace("**", "");
    // Strip __underline__ markers
    cleaned = cleaned.replace("__", "");
    // Strip --- horizontal rules
    cleaned = cleaned.replace("\n---\n", "\n\n");
    cleaned = cleaned.replace("\n---", "\n");
    // Strip ``` code fences (keep content)
    cleaned = regex::Regex::new(r"```\w*\n?").unwrap().replace_all(&cleaned, "").to_string();
    // Strip inline backticks (keep content)
    cleaned = cleaned.replace('`', "");
    // Strip # headers (keep text)
    cleaned = regex::Regex::new(r"(?m)^#{1,4}\s+").unwrap().replace_all(&cleaned, "").to_string();
    // Strip * bullet points (keep text)
    cleaned = regex::Regex::new(r"(?m)^\* ").unwrap().replace_all(&cleaned, "- ").to_string();
    // Collapse triple+ newlines
    cleaned = regex::Regex::new(r"\n{3,}").unwrap().replace_all(&cleaned, "\n\n").to_string();
    cleaned.trim().to_string()
}

pub async fn tg_send(client: &Client, token: &str, chat_id: i64, text: &str) {
    let text = &clean_markdown(text);
    let mut remaining: &str = text;
    while !remaining.is_empty() {
        let end = remaining.len().min(MAX_MESSAGE_LEN);
        let chunk = &remaining[..end];
        remaining = &remaining[end..];

        let payload = serde_json::json!({
            "chat_id": chat_id,
            "text": chunk,
        });

        match client
            .post(format!("{}/bot{}/sendMessage", TG_API, token))
            .json(&payload)
            .timeout(Duration::from_secs(15))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    error!("tg_send failed: HTTP {} — {}", status, &body[..body.len().min(200)]);
                }
            }
            Err(e) => {
                error!("tg_send error: {}", e);
            }
        }
    }
}

pub async fn tg_send_typing(client: &Client, token: &str, chat_id: i64) {
    let _ = client
        .post(format!("{}/bot{}/sendChatAction", TG_API, token))
        .json(&serde_json::json!({"chat_id": chat_id, "action": "typing"}))
        .timeout(Duration::from_secs(5))
        .send()
        .await;
}

async fn tg_get_updates(client: &Client, token: &str, offset: i64) -> Vec<TgUpdate> {
    // allowed_updates explicitly includes callback_query so the inline
    // keyboard buttons from approval prompts get delivered.
    let url = format!(
        "{}/bot{}/getUpdates?offset={}&limit=10&timeout=5&allowed_updates=%5B%22message%22%2C%22callback_query%22%5D",
        TG_API, token, offset
    );
    match client.get(&url).timeout(Duration::from_secs(15)).send().await {
        Ok(resp) => match resp.json::<TgResponse>().await {
            Ok(r) if r.ok => r.result.unwrap_or_default(),
            _ => Vec::new(),
        },
        Err(e) => {
            debug!("getUpdates error: {}", e);
            Vec::new()
        }
    }
}

// ── Offset Persistence ──────────────────────────────────────────────────────

fn offsets_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    PathBuf::from(format!("{}/.syntaur/offsets.json", home))
}

fn load_offsets() -> HashMap<String, i64> {
    let path = offsets_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_offsets(offsets: &HashMap<String, i64>) {
    let path = offsets_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(offsets) {
        let _ = std::fs::write(&path, json);
    }
}

// ── Conversation History ────────────────────────────────────────────────────

pub struct ConversationStore {
    histories: HashMap<String, Vec<ChatMessage>>,
    max_history: usize,
    persist_path: std::path::PathBuf,
}

impl ConversationStore {
    pub fn new(max_history: usize) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
        let persist_path = std::path::PathBuf::from(format!("{}/.syntaur/conversations.json", home));

        // Load from disk
        let histories: HashMap<String, Vec<ChatMessage>> = std::fs::read_to_string(&persist_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let count: usize = histories.values().map(|v| v.len()).sum();
        if count > 0 {
            info!("Loaded {} conversation messages from disk", count);
        }

        Self {
            histories,
            max_history,
            persist_path,
        }
    }

    pub fn get_history(&self, key: &str) -> Vec<ChatMessage> {
        self.histories.get(key).cloned().unwrap_or_default()
    }

    pub fn add_exchange(&mut self, key: &str, user_msg: &str, assistant_msg: &str) {
        let history = self.histories.entry(key.to_string()).or_default();
        history.push(ChatMessage::user(user_msg));
        history.push(ChatMessage::assistant(assistant_msg));

        // Trim to max
        let max = self.max_history * 2;
        if history.len() > max {
            *history = history[history.len() - max..].to_vec();
        }

        self.save();
    }

    pub fn clear(&mut self, key: &str) {
        self.histories.remove(key);
        self.save();
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string(&self.histories) {
            let _ = std::fs::write(&self.persist_path, &json);
        }
    }
}

// ── Bot Poller ──────────────────────────────────────────────────────────────

pub struct TelegramBot {
    pub account_id: String,
    pub agent_id: String,
    pub token: String,
    pub name: String,
    pub allow_from: Vec<i64>,
}

pub async fn run_bot(
    bot: TelegramBot,
    client: Client,
    llm_chain: Arc<LlmChain>,
    conversations: Arc<Mutex<ConversationStore>>,
    system_prompt: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    mcp: Arc<crate::mcp::McpRegistry>,
    approval_registry: Arc<crate::approval::ApprovalRegistry>,
    approval_store: Option<Arc<crate::approval::PendingActionStore>>,
    rate_limiter: Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>,
    circuit_breakers: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, crate::circuit_breaker::CircuitBreaker>>,
    >,
    users: Arc<crate::auth::UserStore>,
    plan_registry: Arc<crate::plans::PlanRegistry>,
    plan_store: Arc<crate::plans::PlanStore>,
    app_state: Arc<crate::AppState>,
) {
    info!("[tg:{}] Polling bot @{} for agent {}", bot.account_id, bot.name, bot.agent_id);

    let mut offsets = load_offsets();
    let mut offset = offsets.get(&bot.account_id).copied().unwrap_or(0);

    // On startup: get latest update to avoid 409 conflicts
    if offset == 0 {
        let updates = tg_get_updates(&client, &bot.token, -1).await;
        if let Some(last) = updates.last() {
            offset = last.update_id + 1;
            offsets.insert(bot.account_id.clone(), offset);
            save_offsets(&offsets);
            info!("[tg:{}] Initial offset set to {}", bot.account_id, offset);
        }
    }

    loop {
        let updates = tg_get_updates(&client, &bot.token, offset).await;

        for update in updates {
            offset = update.update_id + 1;

            // Callback query? — approve/deny button press
            if let Some(cb) = update.callback_query {
                let data = cb.data.unwrap_or_default();
                let user_id = cb.from.id;
                if !bot.allow_from.contains(&user_id) {
                    info!("[tg:{}] callback from non-allowlisted user {}", bot.account_id, user_id);
                    continue;
                }
                // Internal callbacks use "verb:numeric_id" (approve:123, plan_deny:456).
                // External callbacks (rust-social-manager) use "kind:action:string_id"
                // (bsky-post:reject:bsky_2026-04-11). Try internal first; buffer otherwise.
                let handled = if let Some((verb, id_str)) = data.split_once(':') {
                    if let Ok(parsed_id) = id_str.parse::<i64>() {
                        let (approved, ack_text) = match verb {
                            "approve" => {
                                let resolved = approval_registry.resolve_bool(parsed_id, true).await;
                                info!(
                                    "[tg:{}] callback approve:{} resolved={}",
                                    bot.account_id, parsed_id, resolved
                                );
                                (true, "Approved (once)")
                            }
                            "approve_session" => {
                                use crate::approval::ApprovalScope;
                                let resolved = approval_registry.resolve(parsed_id, ApprovalScope::Session).await;
                                info!(
                                    "[tg:{}] callback approve_session:{} resolved={}",
                                    bot.account_id, parsed_id, resolved
                                );
                                (true, "Approved for session")
                            }
                            "approve_always" => {
                                use crate::approval::ApprovalScope;
                                let resolved = approval_registry.resolve(parsed_id, ApprovalScope::Always).await;
                                info!(
                                    "[tg:{}] callback approve_always:{} resolved={}",
                                    bot.account_id, parsed_id, resolved
                                );
                                (true, "Always allowed")
                            }
                            "deny" => {
                                use crate::approval::ApprovalScope;
                                let resolved = approval_registry.resolve(parsed_id, ApprovalScope::Denied).await;
                                info!(
                                    "[tg:{}] callback deny:{} resolved={}",
                                    bot.account_id, parsed_id, resolved
                                );
                                (false, "Denied")
                            }
                            "plan_approve" => {
                                if let Err(e) = plan_store.mark_approved(parsed_id).await {
                                    warn!("[tg:{}] plan_approve {}: {}", bot.account_id, parsed_id, e);
                                }
                                plan_registry.resolve(parsed_id, true).await;
                                crate::spawn_plan_executor(
                                    Arc::clone(&app_state),
                                    parsed_id,
                                );
                                info!(
                                    "[tg:{}] plan_approve:{} → executing",
                                    bot.account_id, parsed_id
                                );
                                (true, "Plan approved — executing")
                            }
                            "plan_deny" => {
                                if let Err(e) = plan_store.mark_denied(parsed_id).await {
                                    warn!("[tg:{}] plan_deny {}: {}", bot.account_id, parsed_id, e);
                                }
                                plan_registry.resolve(parsed_id, false).await;
                                info!("[tg:{}] plan_deny:{}", bot.account_id, parsed_id);
                                (false, "Plan denied")
                            }
                            _ => {
                                // Known verb format but unrecognized verb → buffer
                                (false, "")
                            }
                        };
                        if !ack_text.is_empty() {
                            let _ = approved;
                            let answer_url = format!(
                                "{}/bot{}/answerCallbackQuery",
                                TG_API, bot.token
                            );
                            let _ = client
                                .post(&answer_url)
                                .json(&serde_json::json!({
                                    "callback_query_id": cb.id,
                                    "text": ack_text,
                                    "show_alert": false,
                                }))
                                .timeout(Duration::from_secs(5))
                                .send()
                                .await;
                            true
                        } else {
                            false // unrecognized verb → fall through to buffer
                        }
                    } else {
                        false // non-numeric id → fall through to buffer
                    }
                } else {
                    false // no colon → fall through to buffer
                };
                if !handled {
                    // social-draft:<verb>:<id> — routed to the social engine.
                    // We handle these in-band so the draft card can be approved
                    // from Telegram without buffering to the external endpoint.
                    if data.starts_with("social-draft:") {
                        // Look up the draft's owner user_id.
                        let parts: Vec<&str> = data.splitn(3, ':').collect();
                        let draft_id: i64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(-1);
                        let db = app_state.db_path.clone();
                        let owner: Option<i64> = tokio::task::spawn_blocking(move || {
                            let conn = rusqlite::Connection::open(&db).ok()?;
                            conn.query_row(
                                "SELECT user_id FROM social_drafts WHERE id = ?",
                                rusqlite::params![draft_id],
                                |r| r.get::<_, i64>(0),
                            ).ok()
                        }).await.ok().flatten();
                        if let Some(uid) = owner {
                            let result = crate::social::engine::telegram_callback_dispatch(
                                Arc::clone(&app_state), uid, &data
                            ).await;
                            let ack_text = match &result {
                                Ok(s) => s.chars().take(40).collect::<String>(),
                                Err(e) => e.chars().take(40).collect::<String>(),
                            };
                            let answer_url = format!("{}/bot{}/answerCallbackQuery", TG_API, bot.token);
                            let _ = client.post(&answer_url)
                                .json(&serde_json::json!({
                                    "callback_query_id": cb.id,
                                    "text": ack_text,
                                    "show_alert": false,
                                }))
                                .timeout(Duration::from_secs(5))
                                .send().await;
                            info!("[tg:{}] social-draft dispatch user={} draft={} result={:?}", bot.account_id, uid, draft_id, result);
                            continue;
                        }
                    }
                    // Intentionally omit bot.token — the consumer already has its
                    // own token for the bot it cares about. Leaking bot_token on a
                    // LAN endpoint would surrender full bot control.
                    let cb_record = serde_json::json!({
                        "bot_id": bot.account_id,
                        "callback_id": cb.id,
                        "data": data,
                        "received_at": chrono::Utc::now().to_rfc3339(),
                    });
                    app_state.external_callbacks.lock().await.push(cb_record);
                    info!("[tg:{}] buffered external callback: {} (id={})", bot.account_id, data, cb.id);
                }
                continue;
            }

            let msg = match update.message {
                Some(m) => m,
                None => continue,
            };

            let chat_id = msg.chat.id;
            let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);
            let text = msg.text.as_deref().unwrap_or("").trim().to_string();

            // Access control
            if !bot.allow_from.is_empty() && !bot.allow_from.contains(&user_id) {
                debug!("[tg:{}] Ignoring message from unauthorized user {}", bot.account_id, user_id);
                continue;
            }

            if text.is_empty() {
                continue;
            }

            // Commands
            match text.to_lowercase().as_str() {
                "/clear" => {
                    let conv_key = format!("{}_{}", bot.account_id, chat_id);
                    conversations.lock().await.clear(&conv_key);
                    tg_send(&client, &bot.token, chat_id, "Conversation cleared.").await;
                    continue;
                }
                "/status" => {
                    tg_send(&client, &bot.token, chat_id, &format!(
                        "Agent: {}\nBot: {}\nStatus: running",
                        bot.agent_id, bot.name
                    )).await;
                    continue;
                }
                "/help" => {
                    tg_send(&client, &bot.token, chat_id,
                        "Send any message. Commands:\n/status — bot status\n/clear — reset conversation\n/help — this message"
                    ).await;
                    continue;
                }
                _ => {}
            }

            if text.starts_with('/') {
                continue;
            }

            info!("[tg:{}] Message from {}: {}", bot.account_id, user_id, &text[..text.len().min(80)]);

            // Rate limiting (simple inline — 30 messages/minute per user)
            {
                static RATE_COUNTS: std::sync::LazyLock<tokio::sync::Mutex<std::collections::HashMap<i64, (u32, std::time::Instant)>>> =
                    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
                let mut counts = RATE_COUNTS.lock().await;
                let entry = counts.entry(user_id).or_insert((0, std::time::Instant::now()));
                if entry.1.elapsed() > std::time::Duration::from_secs(60) {
                    *entry = (0, std::time::Instant::now());
                }
                entry.0 += 1;
                if entry.0 > 30 {
                    warn!("[tg:{}] Rate limited user {}", bot.account_id, user_id);
                    tg_send(&client, &bot.token, chat_id, "Rate limited — try again in a minute.").await;
                    continue;
                }
            }

            tg_send_typing(&client, &bot.token, chat_id).await;

            // Build messages with history + LCM context management
            let conv_key = format!("{}_{}", bot.account_id, chat_id);
            let mut messages = vec![
                ChatMessage::system(&system_prompt),
            ];

            {
                let convos = conversations.lock().await;
                let history = convos.get_history(&conv_key);
                messages.extend(history);
            }

            messages.push(ChatMessage::user(&text));

            // LCM: check if context needs compaction (rough token estimate)
            let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
            let est_tokens = total_chars / 4;
            if est_tokens > 100_000 { // ~75% of 131K context
                info!("[tg:{}] Context large (~{}K tokens), triggering LCM compaction", bot.account_id, est_tokens / 1000);
                // Keep system prompt + last 32 messages
                let system = messages.remove(0);
                let fresh_tail: Vec<ChatMessage> = messages.iter().rev().take(32).cloned().collect::<Vec<_>>().into_iter().rev().collect();

                // Summarize the old messages
                let old_count = messages.len() - fresh_tail.len();
                if old_count > 4 {
                    let old_msgs: Vec<ChatMessage> = messages[..old_count].to_vec();
                    let summary_text = old_msgs.iter()
                        .map(|m| format!("{}: {}", m.role, &m.content[..m.content.len().min(200)]))
                        .collect::<Vec<_>>()
                        .join("\n");

                    messages = vec![system];
                    messages.push(ChatMessage::system(&format!("[Earlier conversation summary]\n{}", &summary_text[..summary_text.len().min(2000)])));
                    messages.extend(fresh_tail);
                    info!("[tg:{}] Compacted: {} old messages summarized, {} fresh kept", bot.account_id, old_count, messages.len() - 2);
                } else {
                    messages.insert(0, system);
                }
            }

            // Call LLM (with tool-call loop)
            info!("[tg:{}] Calling LLM with {} messages", bot.account_id, messages.len());
            let workspace = crate::config::Config::default().agent_workspace(&bot.agent_id);
            // Look up the user who owns this Telegram chat (v5 Item 3).
            // Falls back to 0 = legacy admin when the chat isn't linked
            // to any real user, which matches the "fresh install keeps
            // working" guarantee.
            let chat_user_id = users
                .resolve_telegram_chat(&bot.token, chat_id)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            // Build the approval context for this turn — bound to the calling
            // user's chat for the inline keyboard prompts.
            let approval_ctx = approval_store.as_ref().map(|store| {
                crate::tools::ApprovalContext {
                    store: Arc::clone(store),
                    registry: Arc::clone(&approval_registry),
                    bot_token: bot.token.clone(),
                    chat_id,
                    http_client: client.clone(),
                    user_id: chat_user_id,
                    conversation_id: None, // set per-conversation when available
                }
            });
            let response = run_llm_with_tools(
                &llm_chain,
                &mut messages,
                &bot,
                &client,
                &workspace,
                Some(Arc::clone(&mcp)),
                approval_ctx,
                Arc::clone(&rate_limiter),
                Arc::clone(&circuit_breakers),
                Some(Arc::new(app_state.config.clone())),
            ).await;

            // Save to history
            {
                let mut convos = conversations.lock().await;
                convos.add_exchange(&conv_key, &text, &response);
            }

            // Store in LCM SQLite for cross-session memory
            {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
                let db_path = crate::resolve_data_dir().join("lcm.db").to_string_lossy().to_string();
                let lcm_config = crate::config::LcmConfig::default();
                let lcm = crate::lcm::LcmManager::new(&db_path, lcm_config);
                lcm.store_message(&bot.agent_id, &conv_key, "user", &text);
                lcm.store_message(&bot.agent_id, &conv_key, "assistant", &response);
            }

            // Send response
            info!("[tg:{}] Sending response to {}", bot.account_id, chat_id);
            tg_send(&client, &bot.token, chat_id, &response).await;
            info!("[tg:{}] Response sent", bot.account_id);

            // Log to conversation file for external access
            log_message(&bot.account_id, &bot.agent_id, user_id, &text, &response);
        }

        // Persist offset
        offsets.insert(bot.account_id.clone(), offset);
        save_offsets(&offsets);

        // Check shutdown
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = shutdown_rx.changed() => {
                info!("[tg:{}] Shutting down", bot.account_id);
                break;
            }
        }
    }
}
