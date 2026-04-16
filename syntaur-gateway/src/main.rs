mod approval;
mod audit;
mod auth;
mod circuit_breaker;
mod config;
mod connectors;
mod conversations;
mod cron;
mod hooks;
mod index;
mod lcm;
mod llm;
mod mcp;
mod oauth;
mod rate_limit;
mod research;
mod telegram;
mod plans;
mod skills;
mod slash;
mod tool_hooks;
mod tools;
mod voice;
mod voice_chat;
mod voice_ws;
mod voice_api;
mod modules;
mod pages;
mod setup;
mod license;
mod tax;
mod financial;
mod calendar_reminder;
mod sync;
mod music;
pub mod crypto;
pub mod terminal;

/// Brand name constant — used in user-facing messages.
pub const BRAND: &str = "Syntaur";

/// Resolve the data directory (~/.syntaur/). Creates it if it doesn't exist.
pub fn resolve_data_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dir = std::path::PathBuf::from(&home).join(".syntaur");
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
}

use axum::{extract::State, response::Json, routing::{get, post}, Router};
use config::{Config, ConfigLoadResult};
use log::{error, info, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};

// ── Shared State ────────────────────────────────────────────────────────────

pub struct AppState {
    pub config: Config,
    pub client: reqwest::Client,
    pub start_time: Instant,
    pub stats: Mutex<GatewayStats>,
    pub mcp: Arc<mcp::McpRegistry>,
    pub indexer: Option<Arc<index::Indexer>>,
    pub research_store: Option<Arc<research::SessionStore>>,
    pub research_events: std::sync::Arc<std::sync::Mutex<HashMap<String, tokio::sync::broadcast::Sender<research::ResearchEvent>>>>,
    pub message_events: std::sync::Arc<std::sync::Mutex<HashMap<String, tokio::sync::broadcast::Sender<AgentTurnEvent>>>>,
    pub approval_store: Option<Arc<approval::PendingActionStore>>,
    pub approval_registry: Arc<approval::ApprovalRegistry>,
    pub openapi_tools: Vec<Arc<dyn crate::tools::extension::Tool>>,
    pub conversations: Option<Arc<conversations::ConversationManager>>,
    pub lcm: Option<Arc<lcm::LcmManager>>,
    /// Per-tool rate limiter (token bucket) shared across requests so that
    /// per-tool quotas survive registry rebuilds. v5 Item 1 Stage 4.
    pub tool_rate_limiter: Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>,
    /// Per-circuit-name circuit breakers shared across requests. Tools with
    /// the same `capabilities().circuit_name` share one breaker so a single
    /// failure cluster opens the whole group. v5 Item 1 Stage 4.
    pub tool_circuit_breakers:
        Arc<tokio::sync::Mutex<HashMap<String, crate::circuit_breaker::CircuitBreaker>>>,
    /// Path to index.db for direct queries (bug reports, etc).
    pub db_path: PathBuf,
    /// Path to the syntaur.json config file (for settings UI edits).
    pub config_path: PathBuf,
    /// Per-user auth store (users, tokens, Telegram links). v5 Item 3.
    pub users: Arc<auth::UserStore>,
    /// In-memory state cache for in-flight OAuth2 authorization_code
    /// flows (CSRF state + PKCE verifier). v5 Item 4.
    pub oauth_state: Arc<oauth::OAuthStateCache>,
    /// Persistent OAuth2 authorization_code token cache (oauth_tokens
    /// table). v5 Item 4.
    pub oauth_tokens: Arc<oauth::AuthCodeTokenCache>,
    /// User-configurable PreToolUse / PostToolUse hooks (4features Stage 2)
    pub tool_hooks: Arc<tool_hooks::HookStore>,
    /// Skills registry — named, reusable workflows. (4features Stage 3)
    pub skills: Arc<skills::SkillStore>,
    /// Plan store — persisted multi-step approval-gated plans. (4features Stage 4)
    pub plans: Arc<plans::PlanStore>,
    /// In-process plan approval registry — wakes propose_plan once the
    /// telegram callback (`plan_approve:N` / `plan_deny:N`) fires. (4features Stage 4)
    pub plan_registry: Arc<plans::PlanRegistry>,
    /// Slash command registry — `/foo` shortcuts the user can invoke
    /// from Telegram or the HTTP /api/slash endpoint. (4features Stage 5)
    pub slash: Arc<slash::SlashStore>,
    /// Data sharing mode: "shared" | "isolated" | "selective"
    pub sharing_mode: Arc<tokio::sync::RwLock<String>>,
    /// Tool names disabled by the module system (from disabled modules).
    pub disabled_tools: Vec<&'static str>,
    /// Bearer secret required for /v1/chat/completions when set
    /// in connectors.home_assistant.voice_secret. None = open.
    pub ha_voice_secret: Option<String>,
    /// Phase 0 voice skill router. Embedding-based dispatcher that lets
    /// the voice path expose ~6 base tools to Qwen while routing the
    /// long tail (~30+ skills) through a single `find_tool(intent)` call.
    /// Lazily populated at startup with the seed entries; expand as more
    /// pure-Rust skills land. None when fastembed init fails (degrades
    /// gracefully — find_tool returns "router has no entries").
    pub tool_router: Option<Arc<tokio::sync::RwLock<crate::tools::router::ToolRouter>>>,
    /// Buffer for Telegram callback_query events that Syntaur doesn't handle
    /// internally (e.g. bsky-post:approve:*, yt-reply:*, threads-post:*).
    /// External consumers (rust-social-manager bsky-approve) drain via
    /// GET /external-callbacks.
    pub external_callbacks: Arc<Mutex<Vec<serde_json::Value>>>,
    /// Clones of every connector registered with the scheduler, keyed by
    /// `connector.name()`. Lets the /knowledge UI trigger a manual re-sync
    /// by calling `load_full()` + `indexer.put_document()` on demand.
    /// Populated by the scheduler setup block; empty on boot failure.
    pub connectors: Arc<std::sync::RwLock<HashMap<String, Arc<dyn connectors::FullConnector>>>>,
    /// Uploaded-files connector specifically — held separately so upload
    /// and delete handlers can reach its filesystem root.
    pub uploaded_files: Option<Arc<connectors::sources::uploaded_files::UploadedFilesConnector>>,
    /// Terminal module manager (mod-coders). None when module is disabled.
    pub terminal: Option<Arc<terminal::TerminalManager>>,
}

/// Run the `bootstrap-admin` CLI subcommand. Parses `--name <name>` from
/// args, opens the user store at `~/.syntaur/index.db`, creates a new
/// user, mints their first token, and prints the token once to stdout.
///
/// The first bootstrap is the **transition point** from legacy-admin mode
/// to real per-user auth: once any user exists in the table, the legacy
/// global token stops being honored (see `auth::legacy_admin_enabled`).
///
/// v5 Item 3 Stage 5.
async fn run_bootstrap_admin(args: &[String]) {
    // Parse `--name <name>` (required).
    let mut name: Option<String> = None;
    let mut it = args.iter().skip(1); // skip "bootstrap-admin"
    while let Some(arg) = it.next() {
        if arg == "--name" {
            name = it.next().cloned();
        }
    }
    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => {
            eprintln!("usage: syntaur bootstrap-admin --name <name>");
            std::process::exit(2);
        }
    };

    let data_dir_str = resolve_data_dir().to_string_lossy().to_string();
    let db_path = PathBuf::from(format!("{}/index.db", data_dir_str));

    // Ensure the schema is migrated before touching any user tables —
    // opening the Indexer runs the migration idempotently.
    if let Err(e) = index::Indexer::open(db_path.clone()) {
        eprintln!("error: failed to open/migrate index.db: {}", e);
        std::process::exit(1);
    }

    let store = match auth::UserStore::open(db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to open user store: {}", e);
            std::process::exit(1);
        }
    };

    let user = match store.create_user(&name).await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: create_user: {}", e);
            std::process::exit(1);
        }
    };

    let token = match store.mint_token(user.id, "bootstrap").await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: mint_token: {}", e);
            std::process::exit(1);
        }
    };

    println!("Created user id={} name={}", user.id, user.name);
    println!("Token (shown once — save it now):");
    println!("{}", token);
    println!();
    println!("Legacy admin fallback is now DISABLED because a real user exists.");
    println!("Use the new token in Authorization: Bearer <token> or ?token=<token>.");
}

/// Run the `mint-token` CLI subcommand.
///
/// Mints a new API token for an **existing** user — sibling of
/// `bootstrap-admin` which is for *creating* users. Solves the prior pain
/// where you'd lose the bootstrap token and have no way back in without
/// hand-rolling a SHA256+base64url+SQL insert (and getting it wrong).
///
/// Usage:
///   `syntaur mint-token --user <name|id> [--label <label>]`
///
/// Examples:
///   `syntaur mint-token --user sean`
///   `syntaur mint-token --user 1 --label laptop-cli`
async fn run_mint_token(args: &[String]) {
    let mut user_arg: Option<String> = None;
    let mut label: String = "cli-mint".to_string();
    let mut it = args.iter().skip(1); // skip "mint-token"
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--user" | "-u" => {
                user_arg = it.next().cloned();
            }
            "--label" | "-l" => {
                if let Some(v) = it.next() {
                    label = v.clone();
                }
            }
            other => {
                eprintln!("warn: unknown arg '{}'", other);
            }
        }
    }
    let user_arg = match user_arg {
        Some(v) if !v.is_empty() => v,
        _ => {
            eprintln!("usage: syntaur mint-token --user <name|id> [--label <label>]");
            std::process::exit(2);
        }
    };

    let db_path = PathBuf::from(format!("{}/index.db", resolve_data_dir().to_string_lossy()));

    // Migrate first so the users table exists.
    if let Err(e) = index::Indexer::open(db_path.clone()) {
        eprintln!("error: failed to open/migrate index.db: {}", e);
        std::process::exit(1);
    }

    let store = match auth::UserStore::open(db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to open user store: {}", e);
            std::process::exit(1);
        }
    };

    // Resolve --user as either a numeric id or a username.
    let user = if let Ok(id) = user_arg.parse::<i64>() {
        match store.get_user(id).await {
            Ok(Some(u)) => u,
            Ok(None) => {
                eprintln!("error: no user with id={}", id);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("error: get_user({}): {}", id, e);
                std::process::exit(1);
            }
        }
    } else {
        // Look up by name from the full list — list_users() is small (<100 users)
        // so this is cheaper than adding a dedicated SQL helper.
        match store.list_users().await {
            Ok(users) => match users.into_iter().find(|u| u.name == user_arg) {
                Some(u) => u,
                None => {
                    eprintln!("error: no user named '{}'", user_arg);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("error: list_users: {}", e);
                std::process::exit(1);
            }
        }
    };

    let token = match store.mint_token(user.id, &label).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: mint_token: {}", e);
            std::process::exit(1);
        }
    };

    println!("Minted token for user id={} name={} label={}", user.id, user.name, label);
    println!("Token (shown once — save it now):");
    println!("{}", token);
}

/// Resolve an incoming raw bearer token to a `Principal`. Mirrors the
/// full axum extractor's logic but works with the current token-in-body
/// style most handlers use (`ApiMessageRequest { token, ... }`).
///
/// Lookup order:
///   1. `user_api_tokens` via `UserStore::resolve_token`
///   2. Legacy global token in `gateway.auth.token` — only if the users
///      table is empty (`legacy_admin_enabled`)
///
/// Returns `Err(StatusCode::UNAUTHORIZED)` on miss so handlers can
/// `?`-propagate straight into an HTTP response.
///
/// v5 Item 3 Stage 3.
pub async fn resolve_principal(
    state: &AppState,
    raw: &str,
) -> Result<auth::Principal, axum::http::StatusCode> {
    use axum::http::StatusCode;

    if let Ok(Some(resolved)) = state.users.resolve_token(raw).await {
        return Ok(auth::Principal::User {
            id: resolved.user_id,
            name: resolved.user_name,
            role: resolved.user_role,
        });
    }
    if auth::legacy_admin_enabled(&state.users).await {
        // Constant-time comparison to prevent timing side-channels on the
        // legacy global token.
        let expected = state.config.gateway.auth.token.as_bytes();
        let given = raw.as_bytes();
        if expected.len() == given.len() {
            let mut diff: u8 = 0;
            for (a, b) in expected.iter().zip(given.iter()) {
                diff |= a ^ b;
            }
            if diff == 0 {
                return Ok(auth::Principal::LegacyAdmin);
            }
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

/// Streaming event emitted during a /api/message turn so SSE clients can
/// observe progress in real time. Mirrors the shape of research::ResearchEvent.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentTurnEvent {
    Started { turn_id: String, agent: String, message: String },
    LlmCallStarted { turn_id: String, round: usize },
    ToolCallStarted { turn_id: String, round: usize, tool_name: String, args_preview: String },
    ToolCallCompleted { turn_id: String, round: usize, tool_name: String, success: bool, output_chars: usize },
    Complete { turn_id: String, response: String, rounds: usize, duration_ms: u64 },
    Error { turn_id: String, message: String },
}

impl AgentTurnEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete { .. } | Self::Error { .. })
    }
}

#[derive(serde::Serialize, Clone, Default)]
pub struct GatewayStats {
    pub uptime_secs: u64,
    pub config_warnings: Vec<String>,
    pub agents: Vec<String>,
    pub telegram_bots: usize,
    pub cron_jobs: usize,
    pub llm_providers: Vec<String>,
    pub messages_processed: u64,
    pub llm_calls: u64,
    pub cron_runs: u64,
    pub errors: u64,
}

// ── HTTP Handlers ───────────────────────────────────────────────────────────

/// Drain buffered external callbacks (Telegram callback_query events that
/// Syntaur doesn't handle internally). Used by rust-social-manager bsky-approve.
async fn handle_external_callbacks(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<serde_json::Value>>, axum::http::StatusCode> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let principal = resolve_principal(&state, raw).await?;
    require_admin(&principal)?;
    let mut buf = state.external_callbacks.lock().await;
    let drained: Vec<serde_json::Value> = buf.drain(..).collect();
    Ok(Json(drained))
}

async fn handle_health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let uptime = state.start_time.elapsed().as_secs();

    // Build per-provider stats from the primary agent's LLM chain
    let default_agent = state.config.agents.list.first().map(|a| a.id.as_str()).unwrap_or("main");
    let chain = llm::LlmChain::from_config(&state.config, default_agent, state.client.clone());
    let providers = chain.provider_stats().await;

    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime,
        "agents": state.config.agents.list.iter().map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": a.extra.get("name").and_then(|v| v.as_str()).unwrap_or(&a.id)
            })
        }).collect::<Vec<serde_json::Value>>(),
        "providers": providers,
    }))
}

async fn handle_stats(State(state): State<Arc<AppState>>) -> Json<GatewayStats> {
    let mut stats = state.stats.lock().await;
    stats.uptime_secs = state.start_time.elapsed().as_secs();
    Json(stats.clone())
}

#[derive(serde::Deserialize)]
struct ApiMessageRequest {
    token: String,
    agent: Option<String>,
    message: String,
    /// Optional: append this turn to an existing conversation
    conversation_id: Option<String>,
}

// ── Bug Reports ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct BugReportRequest {
    token: String,
    description: String,
    system_info: Option<serde_json::Value>,
    page_url: Option<String>,
}

async fn handle_open_url(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    resolve_principal(&state, token).await?;

    let url = params.get("url").cloned().unwrap_or_default();
    // Only allow https:// URLs — block file://, javascript://, etc.
    if !url.starts_with("https://") {
        return Ok(Json(serde_json::json!({"success": false, "error": "Only https:// URLs allowed"})));
    }
    let result = {
        #[cfg(target_os = "linux")]
        { std::process::Command::new("xdg-open").arg(&url).spawn() }
        #[cfg(target_os = "macos")]
        { std::process::Command::new("open").arg(&url).spawn() }
        #[cfg(target_os = "windows")]
        { std::process::Command::new("cmd").args(["/C", "start", &url]).spawn() }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        { Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "unsupported")) }
    };
    match result {
        Ok(_) => Ok(Json(serde_json::json!({"success": true}))),
        Err(e) => Ok(Json(serde_json::json!({"success": false, "error": e.to_string()}))),
    }
}

async fn handle_bug_report_submit(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BugReportRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;

    if req.description.trim().is_empty() {
        return Ok(Json(serde_json::json!({"error": "description is required"})));
    }

    let user_id = principal.user_id();
    let user_name = match &principal {
        auth::Principal::User { name, .. } => name.clone(),
        auth::Principal::LegacyAdmin => "admin".to_string(),
    };
    let description = req.description.clone();
    let system_info = req.system_info.as_ref().map(|v| v.to_string());
    let page_url = req.page_url.clone();
    let db_path = state.db_path.clone();

    let report_id: i64 = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| format!("open db: {}", e))?;
        conn.execute(
            "INSERT INTO bug_reports (user_id, user_name, description, system_info, page_url) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![user_id, &user_name, &description, &system_info, &page_url],
        ).map_err(|e| format!("insert: {}", e))?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        error!("[bug-report] {}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let user_display = match &principal {
        auth::Principal::User { name, .. } => name.clone(),
        auth::Principal::LegacyAdmin => "admin".to_string(),
    };
    info!("[bug-report] #{} from {}", report_id, user_display);

    Ok(Json(serde_json::json!({
        "id": report_id,
        "status": "submitted",
    })))
}

async fn handle_bug_report_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;

    let db_path = state.db_path.clone();
    let status_filter = params.get("status").cloned().unwrap_or_else(|| "all".to_string());

    let reports: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| format!("open: {}", e))?;
        let sql = if status_filter == "all" {
            "SELECT id, user_name, description, status, created_at FROM bug_reports ORDER BY id DESC"
        } else {
            "SELECT id, user_name, description, status, created_at FROM bug_reports WHERE status = ?1 ORDER BY id DESC"
        };
        let mut stmt = conn.prepare(sql).map_err(|e| format!("prepare: {}", e))?;
        let params: Vec<&dyn rusqlite::types::ToSql> = if status_filter == "all" {
            vec![]
        } else {
            vec![&status_filter as &dyn rusqlite::types::ToSql]
        };
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "user_name": row.get::<_, Option<String>>(1)?,
                "description": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "created_at": row.get::<_, Option<String>>(4)?,
            }))
        }).map_err(|e| format!("query: {}", e))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        error!("[bug-report] list: {}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({ "reports": reports })))
}

// ── Todos ───────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct TodoCreateRequest { token: String, text: String, due_date: Option<String> }
#[derive(serde::Deserialize)]
struct TodoUpdateRequest { token: String, done: Option<bool> }
#[derive(serde::Deserialize)]
struct TodoDeleteRequest { token: String }

async fn handle_todo_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let todos = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT id, text, done, due_date, created_at, completed_at FROM todos WHERE user_id = ? ORDER BY done ASC, created_at DESC".to_string(),
             vec![Box::new(uid)])
        } else {
            ("SELECT id, text, done, due_date, created_at, completed_at FROM todos ORDER BY done ASC, created_at DESC".to_string(),
             vec![])
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |r| Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?, "text": r.get::<_, String>(1)?,
            "done": r.get::<_, i64>(2)? != 0, "due_date": r.get::<_, Option<String>>(3)?,
            "created_at": r.get::<_, i64>(4)?, "completed_at": r.get::<_, Option<i64>>(5)?,
        }))).map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "todos": todos })))
}

async fn handle_todo_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TodoCreateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let text = req.text.clone();
    let due = req.due_date.clone();
    let now = chrono::Utc::now().timestamp();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO todos (user_id, text, due_date, created_at) VALUES (?, ?, ?, ?)",
            rusqlite::params![uid, &text, &due, now]).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "id": id, "text": req.text, "done": false })))
}

async fn handle_todo_update(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(req): Json<TodoUpdateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let done = req.done.unwrap_or(false);
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let completed = if done { Some(now) } else { None };
        if let Some(uid) = scope {
            conn.execute("UPDATE todos SET done = ?, completed_at = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![done as i64, completed, id, uid]).map_err(|e| e.to_string())?;
        } else {
            conn.execute("UPDATE todos SET done = ?, completed_at = ? WHERE id = ?",
                rusqlite::params![done as i64, completed, id]).map_err(|e| e.to_string())?;
        }
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "id": id, "done": done })))
}

async fn handle_todo_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(req): Json<TodoDeleteRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        if let Some(uid) = scope {
            conn.execute("DELETE FROM todos WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
        } else {
            conn.execute("DELETE FROM todos WHERE id = ?",
                rusqlite::params![id]).map_err(|e| e.to_string())?;
        }
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ── Calendar Events ─────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct CalendarEventCreateRequest {
    token: String,
    title: String,
    description: Option<String>,
    start_time: String,
    end_time: Option<String>,
    all_day: Option<bool>,
    recurrence_rule: Option<String>,
    recurrence_end_date: Option<String>,
    reminder_minutes: Option<i64>,
}

#[derive(serde::Deserialize)]
struct CalendarEventUpdateRequest {
    token: String,
    title: Option<String>,
    description: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    all_day: Option<bool>,
    recurrence_rule: Option<String>,
    recurrence_end_date: Option<String>,
    reminder_minutes: Option<i64>,
}

#[derive(serde::Deserialize)]
struct CalendarIcsImportRequest {
    token: String,
    ics_content: String,
}

/// Expand a base event into virtual instances within [range_start, range_end].
/// Returns a list of JSON objects with `occurrence_date` set for recurring instances.
use chrono::Datelike as _CalDatelike;

fn expand_recurrence(
    base: &serde_json::Value,
    range_start: &str,
    range_end: &str,
) -> Vec<serde_json::Value> {
    let rule = base.get("recurrence_rule").and_then(|v| v.as_str()).unwrap_or("");
    let start_time = base.get("start_time").and_then(|v| v.as_str()).unwrap_or("");
    if rule.is_empty() || start_time.is_empty() {
        // Not recurring — compare by date prefix so timed events on the end
        // date still match when range_end is "YYYY-MM-DD".
        let evt_date = if start_time.len() >= 10 { &start_time[..10] } else { start_time };
        let rs = if range_start.len() >= 10 { &range_start[..10] } else { range_start };
        let re = if range_end.len() >= 10 { &range_end[..10] } else { range_end };
        if evt_date >= rs && evt_date <= re {
            let mut ev = base.clone();
            ev["is_recurring_instance"] = serde_json::json!(false);
            return vec![ev];
        }
        return vec![];
    }

    // Parse base date (first 10 chars: YYYY-MM-DD)
    if start_time.len() < 10 { return vec![]; }
    let base_date_str = &start_time[..10];
    let base_time_part = if start_time.len() > 10 { &start_time[10..] } else { "" };

    let base_date = match chrono::NaiveDate::parse_from_str(base_date_str, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let range_start_date = chrono::NaiveDate::parse_from_str(&range_start[..range_start.len().min(10)], "%Y-%m-%d").unwrap_or(base_date);
    let range_end_date = chrono::NaiveDate::parse_from_str(&range_end[..range_end.len().min(10)], "%Y-%m-%d").unwrap_or(base_date);

    // Respect recurrence_end_date
    let rec_end = base.get("recurrence_end_date").and_then(|v| v.as_str())
        .and_then(|s| chrono::NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d").ok());

    let mut out = Vec::new();
    let mut cur = base_date;
    let mut safety = 0;
    while cur <= range_end_date && safety < 2000 {
        safety += 1;
        if let Some(end_d) = rec_end { if cur > end_d { break; } }
        if cur >= range_start_date && cur >= base_date {
            let date_str = cur.format("%Y-%m-%d").to_string();
            let occ_time = format!("{}{}", date_str, base_time_part);
            let mut ev = base.clone();
            ev["start_time"] = serde_json::json!(occ_time);
            ev["occurrence_date"] = serde_json::json!(date_str);
            ev["is_recurring_instance"] = serde_json::json!(cur != base_date);
            out.push(ev);
        }
        cur = match rule {
            "daily" => cur.succ_opt().unwrap_or(cur),
            "weekly" => cur.checked_add_days(chrono::Days::new(7)).unwrap_or(cur),
            "monthly" => {
                let m = cur.month();
                let y = cur.year();
                let (ny, nm) = if m == 12 { (y+1, 1) } else { (y, m+1) };
                // Clamp day to last day of target month
                let target_day = cur.day();
                let max_day = match nm {
                    1|3|5|7|8|10|12 => 31,
                    4|6|9|11 => 30,
                    2 => if (ny % 4 == 0 && ny % 100 != 0) || ny % 400 == 0 { 29 } else { 28 },
                    _ => 28,
                };
                chrono::NaiveDate::from_ymd_opt(ny, nm, target_day.min(max_day)).unwrap_or(cur)
            },
            "yearly" => chrono::NaiveDate::from_ymd_opt(cur.year()+1, cur.month(), cur.day()).unwrap_or(cur),
            _ => break,
        };
        if cur == base_date && safety > 1 { break; } // prevent infinite loop
    }
    out
}

async fn handle_calendar_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();
    let events = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Always fetch all user events (recurring ones may have start_time before range_start)
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT id, title, description, start_time, end_time, all_day, source, created_at, \
              recurrence_rule, recurrence_end_date, reminder_minutes, updated_at \
              FROM calendar_events WHERE user_id = ? ORDER BY start_time".to_string(),
             vec![Box::new(uid)])
        } else {
            ("SELECT id, title, description, start_time, end_time, all_day, source, created_at, \
              recurrence_rule, recurrence_end_date, reminder_minutes, updated_at \
              FROM calendar_events ORDER BY start_time".to_string(),
             vec![])
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |r| Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?,
            "description": r.get::<_, Option<String>>(2)?, "start_time": r.get::<_, String>(3)?,
            "end_time": r.get::<_, Option<String>>(4)?, "all_day": r.get::<_, i64>(5)? != 0,
            "source": r.get::<_, String>(6)?, "created_at": r.get::<_, i64>(7)?,
            "recurrence_rule": r.get::<_, Option<String>>(8)?,
            "recurrence_end_date": r.get::<_, Option<String>>(9)?,
            "reminder_minutes": r.get::<_, Option<i64>>(10)?,
            "updated_at": r.get::<_, Option<i64>>(11)?,
        }))).map_err(|e| e.to_string())?;
        let base_events: Vec<serde_json::Value> = rows.filter_map(|r| r.ok()).collect();

        // Expand recurring events into the requested range
        let range_s = start.clone().unwrap_or_else(|| "1900-01-01".to_string());
        let range_e = end.clone().unwrap_or_else(|| "2099-12-31".to_string());
        let mut expanded: Vec<serde_json::Value> = Vec::new();
        for ev in &base_events {
            let mut instances = expand_recurrence(ev, &range_s, &range_e);
            expanded.append(&mut instances);
        }
        // Sort by start_time
        expanded.sort_by(|a, b| {
            let a_t = a.get("start_time").and_then(|v| v.as_str()).unwrap_or("");
            let b_t = b.get("start_time").and_then(|v| v.as_str()).unwrap_or("");
            a_t.cmp(b_t)
        });
        Ok(expanded)
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "events": events })))
}

async fn handle_calendar_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CalendarEventCreateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let title = req.title.clone();
    let desc = req.description.clone();
    let start = req.start_time.clone();
    let end = req.end_time.clone();
    let all_day = req.all_day.unwrap_or(false);
    let rrule = req.recurrence_rule.clone().filter(|s| !s.is_empty() && s != "none");
    let rend = req.recurrence_end_date.clone().filter(|s| !s.is_empty());
    let rmins = req.reminder_minutes;
    let now = chrono::Utc::now().timestamp();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO calendar_events (user_id, title, description, start_time, end_time, all_day, source, created_at, recurrence_rule, recurrence_end_date, reminder_minutes, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'manual', ?, ?, ?, ?, ?)",
            rusqlite::params![uid, &title, &desc, &start, &end, all_day as i64, now, &rrule, &rend, &rmins, now],
        ).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "id": id, "title": req.title })))
}

async fn handle_calendar_update(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(event_id): axum::extract::Path<i64>,
    Json(req): Json<CalendarEventUpdateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let updated = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Load current row, apply partial updates
        let row: Option<(String, Option<String>, String, Option<String>, i64, Option<String>, Option<String>, Option<i64>)> = if let Some(uid) = scope {
            conn.query_row(
                "SELECT title, description, start_time, end_time, all_day, recurrence_rule, recurrence_end_date, reminder_minutes FROM calendar_events WHERE id = ? AND user_id = ?",
                rusqlite::params![event_id, uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            ).ok()
        } else {
            conn.query_row(
                "SELECT title, description, start_time, end_time, all_day, recurrence_rule, recurrence_end_date, reminder_minutes FROM calendar_events WHERE id = ?",
                rusqlite::params![event_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            ).ok()
        };
        let Some((cur_title, cur_desc, cur_start, cur_end, cur_all_day, cur_rrule, cur_rend, cur_rmins)) = row else {
            return Ok(false);
        };
        let new_title = req.title.unwrap_or(cur_title);
        let new_desc = req.description.or(cur_desc);
        let new_start = req.start_time.unwrap_or(cur_start);
        let new_end = req.end_time.or(cur_end);
        let new_all_day = req.all_day.map(|b| b as i64).unwrap_or(cur_all_day);
        let new_rrule = req.recurrence_rule.map(|s| if s == "none" || s.is_empty() { None } else { Some(s) }).unwrap_or(cur_rrule);
        let new_rend = req.recurrence_end_date.map(|s| if s.is_empty() { None } else { Some(s) }).unwrap_or(cur_rend);
        let new_rmins = req.reminder_minutes.or(cur_rmins);
        let count = if let Some(uid) = scope {
            conn.execute(
                "UPDATE calendar_events SET title=?, description=?, start_time=?, end_time=?, all_day=?, recurrence_rule=?, recurrence_end_date=?, reminder_minutes=?, updated_at=? WHERE id=? AND user_id=?",
                rusqlite::params![new_title, new_desc, new_start, new_end, new_all_day, new_rrule, new_rend, new_rmins, now, event_id, uid],
            ).map_err(|e| e.to_string())?
        } else {
            conn.execute(
                "UPDATE calendar_events SET title=?, description=?, start_time=?, end_time=?, all_day=?, recurrence_rule=?, recurrence_end_date=?, reminder_minutes=?, updated_at=? WHERE id=?",
                rusqlite::params![new_title, new_desc, new_start, new_end, new_all_day, new_rrule, new_rend, new_rmins, now, event_id],
            ).map_err(|e| e.to_string())?
        };
        Ok(count > 0)
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    if updated {
        Ok(Json(serde_json::json!({ "updated": true })))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn handle_calendar_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(event_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let deleted = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let count = if let Some(uid) = scope {
            conn.execute(
                "DELETE FROM calendar_events WHERE id = ? AND user_id = ?",
                rusqlite::params![event_id, uid],
            ).map_err(|e| e.to_string())?
        } else {
            conn.execute(
                "DELETE FROM calendar_events WHERE id = ?",
                rusqlite::params![event_id],
            ).map_err(|e| e.to_string())?
        };
        // Clean up reminder tracking
        let _ = conn.execute("DELETE FROM calendar_reminders_sent WHERE event_id = ?", rusqlite::params![event_id]);
        Ok(count > 0)
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    if deleted {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

/// Parse ICS date (20260415T103000Z, 20260415T103000, 20260415) to start_time string
fn parse_ics_date(s: &str) -> Option<(String, bool)> {
    let s = s.trim().trim_end_matches('Z');
    // All-day date only: 20260415 or 2026-04-15
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == 'T').collect();
    if digits.len() == 8 {
        // YYYYMMDD
        let y = &digits[..4]; let m = &digits[4..6]; let d = &digits[6..8];
        return Some((format!("{}-{}-{}", y, m, d), true));
    }
    if digits.len() >= 15 && digits.contains('T') {
        // YYYYMMDDTHHMMSS
        let parts: Vec<&str> = digits.split('T').collect();
        if parts.len() == 2 && parts[0].len() == 8 && parts[1].len() >= 6 {
            let y = &parts[0][..4]; let m = &parts[0][4..6]; let d = &parts[0][6..8];
            let hh = &parts[1][..2]; let mm = &parts[1][2..4]; let ss = &parts[1][4..6];
            return Some((format!("{}-{}-{}T{}:{}:{}", y, m, d, hh, mm, ss), false));
        }
    }
    None
}

/// Unescape ICS text (\n → newline, \, → comma, etc.)
fn ics_unescape(s: &str) -> String {
    s.replace(r"\n", "\n").replace(r"\N", "\n")
     .replace(r"\,", ",").replace(r"\;", ";").replace(r"\\", r"\")
}

async fn handle_calendar_ics_import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CalendarIcsImportRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let content = req.ics_content.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(usize, usize), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Unfold lines (continuation lines start with space or tab)
        let mut lines: Vec<String> = Vec::new();
        for raw in content.lines() {
            let raw = raw.trim_end_matches('\r');
            if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
                let last = lines.last_mut().unwrap();
                last.push_str(&raw[1..]);
            } else {
                lines.push(raw.to_string());
            }
        }
        let now = chrono::Utc::now().timestamp();
        let mut imported = 0;
        let mut skipped = 0;
        let mut in_event = false;
        let mut title = String::new();
        let mut desc: Option<String> = None;
        let mut start_time = String::new();
        let mut end_time: Option<String> = None;
        let mut all_day = false;
        let mut rrule_freq: Option<String> = None;
        let mut rrule_until: Option<String> = None;
        for line in &lines {
            if line == "BEGIN:VEVENT" {
                in_event = true;
                title.clear(); desc = None; start_time.clear(); end_time = None;
                all_day = false; rrule_freq = None; rrule_until = None;
            } else if line == "END:VEVENT" {
                if in_event && !title.is_empty() && !start_time.is_empty() {
                    let res = conn.execute(
                        "INSERT INTO calendar_events (user_id, title, description, start_time, end_time, all_day, source, created_at, recurrence_rule, recurrence_end_date, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'ics', ?, ?, ?, ?)",
                        rusqlite::params![uid, &title, &desc, &start_time, &end_time, all_day as i64, now, &rrule_freq, &rrule_until, now],
                    );
                    match res {
                        Ok(_) => imported += 1,
                        Err(_) => skipped += 1,
                    }
                } else {
                    skipped += 1;
                }
                in_event = false;
            } else if in_event {
                // Split on first colon, but keep parameters before (e.g. DTSTART;VALUE=DATE:20260415)
                if let Some(colon_idx) = line.find(':') {
                    let (key_part, val) = line.split_at(colon_idx);
                    let val = &val[1..];
                    let key = key_part.split(';').next().unwrap_or(key_part);
                    match key {
                        "SUMMARY" => title = ics_unescape(val),
                        "DESCRIPTION" => desc = Some(ics_unescape(val)),
                        "DTSTART" => {
                            if let Some((t, ad)) = parse_ics_date(val) {
                                start_time = t; all_day = ad;
                            }
                        }
                        "DTEND" => {
                            if let Some((t, _)) = parse_ics_date(val) {
                                end_time = Some(t);
                            }
                        }
                        "RRULE" => {
                            // Parse FREQ=DAILY;UNTIL=20261231T235959Z;...
                            for part in val.split(';') {
                                let mut kv = part.splitn(2, '=');
                                let k = kv.next().unwrap_or("");
                                let v = kv.next().unwrap_or("");
                                match k {
                                    "FREQ" => rrule_freq = Some(v.to_ascii_lowercase()),
                                    "UNTIL" => {
                                        if let Some((t, _)) = parse_ics_date(v) {
                                            rrule_until = Some(t.chars().take(10).collect());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok((imported, skipped))
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({
        "imported": result.0,
        "skipped": result.1,
    })))
}

// ── Agent resolution ────────────────────────────────────────────────────────

/// Resolved agent context — everything needed to dispatch a message to an agent.
/// Handles both system agents (from config) and user agents (from DB).
struct ResolvedAgent {
    /// The agent_id to use for LLM routing (base_agent for user agents).
    llm_agent_id: String,
    /// The workspace directory for system prompt files.
    workspace: std::path::PathBuf,
    /// Custom system prompt to prepend (from user agent, if any).
    custom_prompt: Option<String>,
    /// Script execution allowlist.
    allowlist: Vec<String>,
}

/// Resolve an agent_id to its full context. Checks system agents first,
/// then falls back to user_agents table for the given user.
async fn resolve_agent(state: &AppState, agent_id: &str, user_id: i64) -> ResolvedAgent {
    // Check if it's a system agent (defined in config)
    let is_system = state.config.agents.list.iter().any(|a| a.id == agent_id);
    if is_system {
        return ResolvedAgent {
            llm_agent_id: agent_id.to_string(),
            workspace: state.config.agent_workspace(agent_id),
            custom_prompt: None,
            allowlist: state.config.agent_script_allowlist(agent_id),
        };
    }

    // Check user_agents table
    if let Ok(Some(ua)) = state.users.get_user_agent(user_id, agent_id).await {
        let base = &ua.base_agent;
        let workspace = if let Some(ref ws) = ua.workspace {
            let expanded = ws.replace("~", &std::env::var("HOME").unwrap_or_default());
            std::path::PathBuf::from(expanded)
        } else {
            // Use per-user data_dir if set, otherwise default
            let base_dir = match state.users.get_data_dir(user_id).await {
                Some(d) => std::path::PathBuf::from(d),
                None => resolve_data_dir().join(format!("users/{}", user_id)),
            };
            let ws = base_dir.join(format!("agents/{}", agent_id));
            if let Err(e) = std::fs::create_dir_all(&ws) {
                log::warn!("[agent] failed to create workspace {:?}: {}", ws, e);
            }
            ws
        };
        return ResolvedAgent {
            llm_agent_id: base.clone(),
            workspace,
            custom_prompt: ua.system_prompt,
            allowlist: state.config.agent_script_allowlist(base),
        };
    }

    // Fallback: treat as system agent "main" with the requested id
    ResolvedAgent {
        llm_agent_id: agent_id.to_string(),
        workspace: state.config.agent_workspace(agent_id),
        custom_prompt: None,
        allowlist: state.config.agent_script_allowlist(agent_id),
    }
}

// ── Chat ────────────────────────────────────────────────────────────────────

async fn handle_api_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ApiMessageRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;

    let agent_id = req.agent.unwrap_or_else(|| "main".to_string());
    let resolved = resolve_agent(&state, &agent_id, principal.user_id()).await;
    let workspace = resolved.workspace;

    // Load system prompt for agent
    let mut context_parts = Vec::new();
    if let Some(custom) = &resolved.custom_prompt {
        context_parts.push(custom.clone());
    }
    for file in &["SOUL.md", "IDENTITY.md", "TOOLS.md", "USER.md", "BRIEF.md", "PLAN.md", "MEMORY.md"] {
        if let Ok(content) = std::fs::read_to_string(workspace.join(file)) {
            if !content.trim().is_empty() {
                context_parts.push(content);
            }
        }
    }
    let mut system_prompt = if context_parts.is_empty() {
        format!("You are agent {}", agent_id)
    } else {
        context_parts.join("\n\n---\n\n")
    };
    // Inject per-user personality docs (bio, preferences, writing style)
    let personality = state.users.personality_prompt(principal.user_id(), &agent_id, 4000).await;
    if !personality.is_empty() {
        system_prompt.push_str("\n\n---\n\n");
        system_prompt.push_str(&personality);
    }
    // Inject tax context so the agent has full financial awareness
    {
        let db = state.db_path.clone();
        let uid = principal.user_id();
        let year: i64 = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025);
        if let Ok(ctx) = tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&db).ok()?;
            let ctx = crate::tax::build_tax_context(&conn, uid, year);
            if ctx.is_empty() { None } else { Some(ctx) }
        }).await {
            if let Some(tax_ctx) = ctx {
                system_prompt.push_str("\n\n---\n\n");
                system_prompt.push_str(&tax_ctx);
            }
        }
    }

    // Build LLM chain — use llm_agent_id (base agent for user agents)
    let llm_chain = std::sync::Arc::new(llm::LlmChain::from_config(&state.config, &resolved.llm_agent_id, state.client.clone()));

    // Build messages — start with system prompt, then optional conversation history
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = state.users.visible_user_ids(principal.user_id(), &sharing_mode, "conversations", Some(&agent_id)).await;
    let mut messages = vec![llm::ChatMessage::system(&system_prompt)];
    if let (Some(cid), Some(mgr)) = (req.conversation_id.as_deref(), &state.conversations) {
        if mgr.get(cid, scope.clone()).await.is_none() {
            return Err(axum::http::StatusCode::NOT_FOUND);
        }
        let prior = mgr.messages(cid, scope.clone()).await;
        for m in prior {
            match m.role.as_str() {
                "user" => messages.push(llm::ChatMessage::user(&m.content)),
                "assistant" => messages.push(llm::ChatMessage::assistant(&m.content)),
                _ => {}
            }
        }
    }
    messages.push(llm::ChatMessage::user(&req.message));
    if let (Some(cid), Some(mgr)) = (req.conversation_id.as_deref(), &state.conversations) {
        let _ = mgr.append(cid, "user", &req.message).await;
        if let Some(lcm) = &state.lcm {
            lcm.store_message(&agent_id, cid, "user", &req.message);
        }
    }

    // Call LLM with tools
    let mut tool_registry = crate::tools::ToolRegistry::with_extensions_and_allowlist(
        workspace.clone(),
        agent_id.clone(),
        Some(state.mcp.clone()),
        state.indexer.clone(),
        &resolved.allowlist,
    );
    tool_registry.set_infra(
        Arc::clone(&state.tool_rate_limiter),
        Arc::clone(&state.tool_circuit_breakers),
    );
    tool_registry.set_user_id(principal.user_id());
    tool_registry.set_db_path(state.db_path.clone());
    tool_registry.set_tool_hooks(Arc::clone(&state.tool_hooks));
    {
        let run_skill: Arc<dyn crate::tools::extension::Tool> =
            Arc::new(skills::RunSkillTool { store: Arc::clone(&state.skills) });
        let delegate: Arc<dyn crate::tools::extension::Tool> =
            Arc::new(crate::tools::subagent::DelegateTool::new(
                Arc::new(state.config.clone()),
                state.client.clone(),
            ));
        tool_registry.add_extension_tools(&[run_skill, delegate]);
    }
    tool_registry.apply_module_filter(&state.disabled_tools);
    let tools = tool_registry.tool_definitions();
    let max_rounds = 30;

    for round in 0..max_rounds {
        let result = match llm_chain.call_raw(&messages, Some(&tools)).await {
            Ok(r) => r,
            Err(e) => return Ok(Json(serde_json::json!({"error": e}))),
        };

        match result {
            llm::LlmResult::Text(text) => {
                if let (Some(cid), Some(mgr)) = (req.conversation_id.as_deref(), &state.conversations) {
                    let _ = mgr.append(cid, "assistant", &text).await;
                    if let Some(lcm) = &state.lcm {
                        lcm.store_message(&agent_id, cid, "assistant", &text);
                    }
                }
                return Ok(Json(serde_json::json!({"response": text, "rounds": round, "conversation_id": req.conversation_id})));
            }
            llm::LlmResult::ToolCalls { content, tool_calls } => {
                messages.push(llm::ChatMessage::assistant_with_tools(&content, tool_calls.clone()));
                for tc in &tool_calls {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let func = tc.get("function").cloned().unwrap_or_default();
                    let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let args_str = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                    let args: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    let tool_call = crate::tools::ToolCall { id: id.clone(), name, arguments: args };
                    let result = tool_registry.execute(&tool_call).await;

                    // Truncate large results to prevent context bloat
                    let mut output = result.output;
                    if output.len() > 1500 {
                        output = format!("{}...\n[truncated — {} chars total]", &output[..1200], output.len());
                    }

                    // Round budget warning
                    let remaining = max_rounds - round - 1;
                    if remaining <= 8 && remaining > 0 {
                        output.push_str(&format!("\n\n[Round {}/{} — {} remaining. Finish your task or report status.]", round + 1, max_rounds, remaining));
                    }

                    messages.push(llm::ChatMessage::tool_result(&id, &output));
                }
            }
        }
    }

    // Force final text
    messages.push(llm::ChatMessage::system("Respond with text now. No more tools."));
    match llm_chain.call(&messages).await {
        Ok(text) => Ok(Json(serde_json::json!({"response": text, "rounds": max_rounds}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct ResearchApiRequest {
    token: String,
    agent: Option<String>,
    query: String,
    time_budget_secs: Option<u64>,
    /// Cache TTL in seconds. Defaults to 21600 (6 hours). 0 disables cache.
    cache_max_age_secs: Option<i64>,
    /// Optional clarification answers from a prior /api/research/clarify call.
    clarification_answers: Option<String>,
}

async fn handle_research(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResearchApiRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    if let Err(e) = research::validate_query(&req.query) {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    let agent_id = req.agent.unwrap_or_else(|| "main".to_string());
    let workspace = state.config.agent_workspace(&agent_id);

    info!("[research] api request: agent={} query_len={}", agent_id, req.query.len());

    // Build a tool registry for the research session. Same wiring as the
    // /api/message handler — the research subtasks will filter to a
    // restricted tool set inside their own loop.
    let allowlist = state.config.agent_script_allowlist(&agent_id);
    let mut tr = crate::tools::ToolRegistry::with_extensions_and_allowlist(
        workspace,
        agent_id.clone(),
        Some(state.mcp.clone()),
        state.indexer.clone(),
        &allowlist,
    );
    tr.add_extension_tools(&state.openapi_tools);
    tr.set_infra(
        Arc::clone(&state.tool_rate_limiter),
        Arc::clone(&state.tool_circuit_breakers),
    );
    tr.set_user_id(principal.user_id());
    tr.set_db_path(state.db_path.clone());
    tr.set_tool_hooks(Arc::clone(&state.tool_hooks));
    {
        let run_skill: Arc<dyn crate::tools::extension::Tool> =
            Arc::new(skills::RunSkillTool { store: Arc::clone(&state.skills) });
        let delegate: Arc<dyn crate::tools::extension::Tool> =
            Arc::new(crate::tools::subagent::DelegateTool::new(
                Arc::new(state.config.clone()),
                state.client.clone(),
            ));
        tr.add_extension_tools(&[run_skill, delegate]);
    }
    let tool_registry = std::sync::Arc::new(tr);

    // Research uses two chains: full for subtasks (quality matters) and fast
    // for plan/report phases (cheaper, no quality cliff).
    let llm_chain = std::sync::Arc::new(
        llm::LlmChain::from_config(&state.config, &agent_id, state.client.clone()),
    );
    let llm_chain_fast = std::sync::Arc::new(
        llm::LlmChain::from_config_fast(&state.config, &agent_id, state.client.clone()),
    );

    let report = research::run_research(
        research::ResearchRequest {
            query: req.query,
            agent_id: agent_id.clone(),
            time_budget_secs: req.time_budget_secs,
            cache_max_age_secs: Some(req.cache_max_age_secs.unwrap_or(21600)),
            session_id_override: None,
            clarification_answers: req.clarification_answers.clone(),
            user_id: principal.user_id(),
        },
        llm_chain,
        llm_chain_fast,
        tool_registry,
        state.research_store.clone(),
        None,
    )
    .await;

    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

#[derive(serde::Deserialize)]
struct ConvCreateRequest {
    token: String,
    agent: Option<String>,
    title: Option<String>,
}

async fn handle_conv_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConvCreateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let mgr = match &state.conversations {
        Some(m) => m,
        None => return Ok(Json(serde_json::json!({"error": "conversations not available"}))),
    };
    let agent = req.agent.unwrap_or_else(|| "main".to_string());
    let title = req.title.unwrap_or_else(|| {
        format!("New conversation {}", chrono::Utc::now().format("%Y-%m-%d %H:%M"))
    });
    // Stamp the new conversation with the caller's user_id so subsequent
    // reads can filter on it.
    match mgr.create(&agent, &title, principal.user_id()).await {
        Ok(id) => Ok(Json(serde_json::json!({"id": id, "agent": agent, "title": title}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_conv_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let mgr = match &state.conversations {
        Some(m) => m,
        None => return Ok(Json(serde_json::json!({"error": "conversations not available"}))),
    };
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = state.users.visible_user_ids(principal.user_id(), &sharing_mode, "conversations", None).await;
    let conv = mgr.get(&id, scope.clone()).await;
    if conv.is_none() {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }
    let messages = mgr.messages(&id, scope).await;
    Ok(Json(serde_json::json!({
        "conversation": conv,
        "messages": messages,
    })))
}

async fn handle_conv_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let mgr = match &state.conversations {
        Some(m) => m,
        None => return Ok(Json(serde_json::json!({"error": "conversations not available"}))),
    };
    let agent = params.get("agent").map(|s| s.as_str()).unwrap_or("main");
    let limit: usize = params.get("limit").and_then(|s| s.parse().ok()).unwrap_or(20);
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = state.users.visible_user_ids(principal.user_id(), &sharing_mode, "conversations", Some(agent)).await;
    let convs = mgr.list_recent(agent, limit, scope).await;
    Ok(Json(serde_json::json!({"conversations": convs})))
}

#[derive(serde::Deserialize)]
struct ResearchClarifyRequest {
    token: String,
    agent: Option<String>,
    query: String,
}

async fn handle_message_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ApiMessageRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let agent_id = req.agent.clone().unwrap_or_else(|| "main".to_string());
    let turn_id = format!("turn-{}", uuid::Uuid::new_v4().simple());

    // Allocate broadcast channel BEFORE spawning the task
    let (tx, _rx0) = tokio::sync::broadcast::channel::<AgentTurnEvent>(64);
    {
        let mut map = state.message_events.lock().unwrap();
        map.insert(turn_id.clone(), tx.clone());
    }

    // Resolve agent BEFORE spawning (needs async DB lookup)
    let resolved = resolve_agent(&state, &agent_id, principal.user_id()).await;
    let sharing_mode = state.sharing_mode.read().await.clone();

    // Snapshot what the background task needs
    let state_clone = Arc::clone(&state);
    let turn_id_for_task = turn_id.clone();
    let agent_for_task = agent_id.clone();
    let message = req.message.clone();
    let conv_id = req.conversation_id.clone();
    let principal_scope = state.users.visible_user_ids(principal.user_id(), &sharing_mode, "conversations", Some(&agent_id)).await;
    let principal_user_id = principal.user_id();

    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let _ = tx.send(AgentTurnEvent::Started {
            turn_id: turn_id_for_task.clone(),
            agent: agent_for_task.clone(),
            message: message.chars().take(200).collect(),
        });

        let workspace = resolved.workspace;
        let mut context_parts = Vec::new();
        if let Some(custom) = &resolved.custom_prompt {
            context_parts.push(custom.clone());
        }
        for file in &["SOUL.md", "IDENTITY.md", "TOOLS.md", "USER.md", "BRIEF.md", "PLAN.md", "MEMORY.md"] {
            if let Ok(content) = std::fs::read_to_string(workspace.join(file)) {
                if !content.trim().is_empty() {
                    context_parts.push(content);
                }
            }
        }
        let mut system_prompt = if context_parts.is_empty() {
            format!("You are agent {}", agent_for_task)
        } else {
            context_parts.join("\n\n---\n\n")
        };
        // Inject per-user personality docs
        let personality = state_clone.users.personality_prompt(principal_user_id, &agent_for_task, 4000).await;
        if !personality.is_empty() {
            system_prompt.push_str("\n\n---\n\n");
            system_prompt.push_str(&personality);
        }
        // Tax context injection
        {
            let db = state_clone.db_path.clone();
            let uid = principal_user_id;
            let year: i64 = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025);
            if let Ok(Some(tax_ctx)) = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                let ctx = crate::tax::build_tax_context(&conn, uid, year);
                if ctx.is_empty() { None } else { Some(ctx) }
            }).await {
                system_prompt.push_str("\n\n---\n\n");
                system_prompt.push_str(&tax_ctx);
            }
        }

        let llm_chain = std::sync::Arc::new(
            llm::LlmChain::from_config(&state_clone.config, &resolved.llm_agent_id, state_clone.client.clone()),
        );

        let mut messages = vec![llm::ChatMessage::system(&system_prompt)];
        if let (Some(cid), Some(mgr)) = (conv_id.as_deref(), &state_clone.conversations) {
            if mgr.get(cid, principal_scope.clone()).await.is_some() {
                let prior = mgr.messages(cid, principal_scope.clone()).await;
                for m in prior {
                    match m.role.as_str() {
                        "user" => messages.push(llm::ChatMessage::user(&m.content)),
                        "assistant" => messages.push(llm::ChatMessage::assistant(&m.content)),
                        _ => {}
                    }
                }
            }
        }
        let _ = principal_user_id;
        messages.push(llm::ChatMessage::user(&message));
        if let (Some(cid), Some(mgr)) = (conv_id.as_deref(), &state_clone.conversations) {
            let _ = mgr.append(cid, "user", &message).await;
            if let Some(lcm) = &state_clone.lcm {
                lcm.store_message(&agent_for_task, cid, "user", &message);
            }
        }

        let mut tr = crate::tools::ToolRegistry::with_extensions_and_allowlist(
            workspace,
            agent_for_task.clone(),
            Some(state_clone.mcp.clone()),
            state_clone.indexer.clone(),
            &resolved.allowlist,
        );
        tr.add_extension_tools(&state_clone.openapi_tools);
        tr.set_infra(
            Arc::clone(&state_clone.tool_rate_limiter),
            Arc::clone(&state_clone.tool_circuit_breakers),
        );
        tr.set_user_id(principal_user_id);
        tr.set_db_path(state_clone.db_path.clone());
        tr.set_tool_hooks(Arc::clone(&state_clone.tool_hooks));
        {
            let run_skill: Arc<dyn crate::tools::extension::Tool> =
                Arc::new(skills::RunSkillTool { store: Arc::clone(&state_clone.skills) });
            let delegate: Arc<dyn crate::tools::extension::Tool> =
                Arc::new(crate::tools::subagent::DelegateTool::new(
                    Arc::new(state_clone.config.clone()),
                    state_clone.client.clone(),
                ));
            tr.add_extension_tools(&[run_skill, delegate]);
        }
        tr.apply_module_filter(&state.disabled_tools);
        let tool_registry = std::sync::Arc::new(tr);
    let tools = tool_registry.tool_definitions();
        let max_rounds = 30;

        for round in 0..max_rounds {
            let _ = tx.send(AgentTurnEvent::LlmCallStarted {
                turn_id: turn_id_for_task.clone(),
                round,
            });
            let result = match llm_chain.call_raw(&messages, Some(&tools)).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(AgentTurnEvent::Error {
                        turn_id: turn_id_for_task.clone(),
                        message: format!("LLM error: {}", e),
                    });
                    return;
                }
            };
            match result {
                llm::LlmResult::Text(text) => {
                    if let (Some(cid), Some(mgr)) = (conv_id.as_deref(), &state_clone.conversations) {
                        let _ = mgr.append(cid, "assistant", &text).await;
                        if let Some(lcm) = &state_clone.lcm {
                            lcm.store_message(&agent_for_task, cid, "assistant", &text);
                        }
                    }
                    let _ = tx.send(AgentTurnEvent::Complete {
                        turn_id: turn_id_for_task.clone(),
                        response: text,
                        rounds: round,
                        duration_ms: started.elapsed().as_millis() as u64,
                    });
                    return;
                }
                llm::LlmResult::ToolCalls { content, tool_calls } => {
                    messages.push(llm::ChatMessage::assistant_with_tools(&content, tool_calls.clone()));
                    for tc in &tool_calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let func = tc.get("function").cloned().unwrap_or_default();
                        let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let args_str = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                        let args: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                        let preview: String = args_str.chars().take(120).collect();
                        let _ = tx.send(AgentTurnEvent::ToolCallStarted {
                            turn_id: turn_id_for_task.clone(),
                            round,
                            tool_name: name.clone(),
                            args_preview: preview,
                        });
                        let tool_call = crate::tools::ToolCall { id: id.clone(), name: name.clone(), arguments: args };
                        let result = tool_registry.execute(&tool_call).await;
                        let _ = tx.send(AgentTurnEvent::ToolCallCompleted {
                            turn_id: turn_id_for_task.clone(),
                            round,
                            tool_name: name,
                            success: result.success,
                            output_chars: result.output.len(),
                        });
                        let mut output = result.output;
                        if output.len() > 1500 {
                            output = format!("{}...\n[truncated — {} chars total]", &output[..1200], output.len());
                        }
                        messages.push(llm::ChatMessage::tool_result(&id, &output));
                    }
                }
            }
        }
        let _ = tx.send(AgentTurnEvent::Error {
            turn_id: turn_id_for_task.clone(),
            message: format!("max rounds ({}) reached without final response", max_rounds),
        });

        // Cleanup: drop broadcast channel after a short delay
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        if let Ok(mut map) = state_clone.message_events.lock() {
            map.remove(&turn_id_for_task);
        }
    });

    Ok(Json(serde_json::json!({"turn_id": turn_id, "status": "running"})))
}

async fn handle_message_stream(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    use axum::response::IntoResponse;
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    let receiver = {
        let map = state.message_events.lock().unwrap();
        match map.get(&id) {
            Some(tx) => tx.subscribe(),
            None => return Err(axum::http::StatusCode::NOT_FOUND),
        }
    };
    use futures_util::stream::StreamExt;
    let stream = async_stream::stream! {
        let mut rx = receiver;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let is_terminal = ev.is_terminal();
                    let json = serde_json::to_string(&ev).unwrap_or_default();
                    let event = axum::response::sse::Event::default().data(json);
                    yield Ok::<_, std::convert::Infallible>(event);
                    if is_terminal { break; }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    };
    Ok(axum::response::sse::Sse::new(stream.boxed())
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response())
}

async fn handle_research_clarify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResearchClarifyRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _principal = resolve_principal(&state, &req.token).await?;
    let agent_id = req.agent.unwrap_or_else(|| "main".to_string());
    let llm_chain_fast = std::sync::Arc::new(
        llm::LlmChain::from_config_fast(&state.config, &agent_id, state.client.clone()),
    );
    match research::run_clarify(&req.query, &llm_chain_fast).await {
        Ok(result) => Ok(Json(serde_json::to_value(result).unwrap_or_default())),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_research_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResearchApiRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    if let Err(e) = research::validate_query(&req.query) {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    let agent_id = req.agent.unwrap_or_else(|| "main".to_string());
    let workspace = state.config.agent_workspace(&agent_id);

    // Create the session row IMMEDIATELY so /stream can find it
    let store = match &state.research_store {
        Some(s) => Arc::clone(s),
        None => return Ok(Json(serde_json::json!({"error": "research store not available"}))),
    };
    let session_id = match store.create(&agent_id, &req.query, principal.user_id()).await {
        Ok(id) => id,
        Err(e) => return Ok(Json(serde_json::json!({"error": format!("create session: {}", e)}))),
    };

    // Allocate a broadcast channel for this session BEFORE spawning the task
    let (tx, _rx0) = tokio::sync::broadcast::channel::<research::ResearchEvent>(64);
    {
        let mut map = state.research_events.lock().unwrap();
        map.insert(session_id.clone(), tx.clone());
    }

    // Snapshot what the background task needs
    let tool_registry = {
        let allowlist = state.config.agent_script_allowlist(&agent_id);
        let mut tr = crate::tools::ToolRegistry::with_extensions_and_allowlist(
            workspace,
            agent_id.clone(),
            Some(state.mcp.clone()),
            state.indexer.clone(),
            &allowlist,
        );
        tr.set_infra(
            Arc::clone(&state.tool_rate_limiter),
            Arc::clone(&state.tool_circuit_breakers),
        );
        tr.set_user_id(principal.user_id());
        tr.set_db_path(state.db_path.clone());
        tr.set_tool_hooks(Arc::clone(&state.tool_hooks));
        {
            let run_skill: Arc<dyn crate::tools::extension::Tool> =
                Arc::new(skills::RunSkillTool { store: Arc::clone(&state.skills) });
            let delegate: Arc<dyn crate::tools::extension::Tool> =
                Arc::new(crate::tools::subagent::DelegateTool::new(
                    Arc::new(state.config.clone()),
                    state.client.clone(),
                ));
            tr.add_extension_tools(&[run_skill, delegate]);
        }
        std::sync::Arc::new(tr)
    };
    let llm_chain = std::sync::Arc::new(
        llm::LlmChain::from_config(&state.config, &agent_id, state.client.clone()),
    );
    let llm_chain_fast = std::sync::Arc::new(
        llm::LlmChain::from_config_fast(&state.config, &agent_id, state.client.clone()),
    );
    let state_for_cleanup = Arc::clone(&state);
    let session_id_for_task = session_id.clone();
    let query = req.query;
    let agent_for_task = agent_id.clone();
    let time_budget = req.time_budget_secs;
    let cache_age = Some(req.cache_max_age_secs.unwrap_or(21600));
    let clarification_answers_for_task = req.clarification_answers.clone();

    // NOTE: research_store::create was already called above and the session id
    // is committed. We pass session_store=None to run_research because that
    // function ALSO calls create() — calling it twice would orphan the first.
    // Instead we let run_research checkpoint into a NEW session and we use
    // the externally-created id only for the broadcast channel mapping.
    // TRADE-OFF: the streamed session_id is the broadcast key, not the row id.
    // For now we accept this — the row id can be looked up via list_recent.
    // A cleaner version would let run_research take a pre-created id.

    let store_for_task = Arc::clone(&store);
    let session_id_for_run = session_id.clone();
    let task_user_id = principal.user_id();
    tokio::spawn(async move {
        let _report = research::run_research(
            research::ResearchRequest {
                query,
                agent_id: agent_for_task,
                time_budget_secs: time_budget,
                cache_max_age_secs: cache_age,
                session_id_override: Some(session_id_for_run),
                clarification_answers: clarification_answers_for_task,
                user_id: task_user_id,
            },
            llm_chain,
            llm_chain_fast,
            tool_registry,
            Some(store_for_task),
            Some(tx),
        )
        .await;
        // Cleanup: drop the broadcast sender from the map after a short delay
        // so any laggy SSE subscribers can still drain pending events.
        let id = session_id_for_task;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        if let Ok(mut map) = state_for_cleanup.research_events.lock() {
            map.remove(&id);
        }
    });

    Ok(Json(serde_json::json!({"session_id": session_id, "status": "running"})))
}

async fn handle_research_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let store = match &state.research_store {
        Some(s) => s,
        None => return Ok(Json(serde_json::json!({"error": "research store not available"}))),
    };
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = state.users.visible_user_ids(principal.user_id(), &sharing_mode, "research", None).await;
    match store.get(&id, scope).await {
        Some(report) => Ok(Json(serde_json::to_value(report).unwrap_or_default())),
        None => Err(axum::http::StatusCode::NOT_FOUND),
    }
}

async fn handle_research_stream(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    use axum::response::IntoResponse;
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    // Look up the broadcast sender for this id
    let receiver = {
        let map = state.research_events.lock().unwrap();
        match map.get(&id) {
            Some(tx) => tx.subscribe(),
            None => return Err(axum::http::StatusCode::NOT_FOUND),
        }
    };

    // Convert broadcast receiver into an SSE event stream. Stop after a
    // terminal event (Complete or Error).
    use futures_util::stream::StreamExt;
    let stream = async_stream::stream! {
        let mut rx = receiver;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let is_terminal = ev.is_terminal();
                    let json = serde_json::to_string(&ev).unwrap_or_default();
                    let event = axum::response::sse::Event::default().data(json);
                    yield Ok::<_, std::convert::Infallible>(event);
                    if is_terminal { break; }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    };
    Ok(axum::response::sse::Sse::new(stream.boxed())
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response())
}

async fn handle_messages(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, axum::http::StatusCode> {
    // Require auth token
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let log_path = format!("{}/.syntaur/messages.jsonl", home);
    let n: usize = params.get("n").and_then(|v| v.parse().ok()).unwrap_or(20);

    let messages: Vec<serde_json::Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(n)
        .collect();

    Ok(Json(messages))
}

// ── Knowledge (RAG) API ────────────────────────────────────────────────────
//
// Exposes the `Indexer` + connector framework to the /knowledge UI:
//   GET  /api/knowledge/stats               — overall + per-source counts
//   GET  /api/knowledge/search?q&k&source   — hybrid search
//   GET  /api/knowledge/docs?source&limit   — recent documents
//   POST /api/knowledge/upload (multipart)  — upload one file, ingest now
//   POST /api/knowledge/resync/{source}     — trigger a load_full refresh
//   POST /api/knowledge/docs/delete         — remove a document

async fn handle_knowledge_stats(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    let indexer = match &state.indexer {
        Some(i) => i,
        None => return Ok(Json(serde_json::json!({"error": "indexer not available"}))),
    };
    let agent_ids = params
        .get("agent")
        .filter(|s| !s.is_empty())
        .map(|a| vec![a.clone(), "shared".to_string()]);
    let overall = indexer.stats(agent_ids.clone()).await;
    let per_source = indexer.stats_per_source(agent_ids).await;
    Ok(Json(serde_json::json!({
        "documents": overall.documents,
        "chunks": overall.chunks,
        "sources": overall.sources,
        "per_source": per_source,
    })))
}

async fn handle_knowledge_search(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    let indexer = match &state.indexer {
        Some(i) => i,
        None => return Ok(Json(serde_json::json!({"error": "indexer not available"}))),
    };
    let q_text = match params.get("q").filter(|s| !s.trim().is_empty()) {
        Some(q) => q.clone(),
        None => return Ok(Json(serde_json::json!({"error": "missing 'q'"}))),
    };
    let k: usize = params
        .get("k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
        .clamp(1, 50);
    let source = params
        .get("source")
        .filter(|s| !s.is_empty())
        .map(String::from);
    let agent_ids = params
        .get("agent")
        .filter(|s| !s.is_empty())
        .map(|a| vec![a.clone(), "shared".to_string()]);
    match indexer.search_hybrid(q_text.clone(), k, source, agent_ids).await {
        Ok(hits) => Ok(Json(serde_json::json!({ "hits": hits }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_knowledge_docs(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    let indexer = match &state.indexer {
        Some(i) => i,
        None => return Ok(Json(serde_json::json!({"error": "indexer not available"}))),
    };
    let source = params
        .get("source")
        .filter(|s| !s.is_empty())
        .map(String::from);
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(25)
        .clamp(1, 200);
    let agent_ids = params
        .get("agent")
        .filter(|s| !s.is_empty())
        .map(|a| vec![a.clone(), "shared".to_string()]);
    let docs = indexer.list_recent_documents(source, limit, agent_ids).await;
    Ok(Json(serde_json::json!({ "documents": docs })))
}

#[derive(serde::Deserialize)]
struct KnowledgeResyncReq {
    token: String,
}

async fn handle_knowledge_resync(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(source): axum::extract::Path<String>,
    Json(req): Json<KnowledgeResyncReq>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _principal = resolve_principal(&state, &req.token).await?;
    let indexer = match &state.indexer {
        Some(i) => Arc::clone(i),
        None => return Ok(Json(serde_json::json!({"error": "indexer not available"}))),
    };
    let connector = {
        let map = match state.connectors.read() {
            Ok(m) => m,
            Err(_) => return Ok(Json(serde_json::json!({"error": "connector map poisoned"}))),
        };
        match map.get(&source) {
            Some(c) => Arc::clone(c),
            None => {
                return Ok(Json(
                    serde_json::json!({"error": format!("unknown source '{}'", source)}),
                ))
            }
        }
    };
    let started = std::time::Instant::now();
    let docs = match connector.load_full().await {
        Ok(d) => d,
        Err(e) => return Ok(Json(serde_json::json!({"error": format!("load_full: {}", e)}))),
    };
    let total = docs.len();
    let mut errors = 0usize;
    for d in docs {
        if let Err(e) = indexer.put_document(d).await {
            errors += 1;
            warn!("[knowledge] resync {} put failed: {}", source, e);
        }
    }
    // Prune stale entries that no longer exist in the source.
    match connector.list_ids().await {
        Ok(ids) => {
            let keep: Vec<String> = ids.into_iter().map(|d| d.external_id).collect();
            if let Err(e) = indexer.prune(&source, keep).await {
                warn!("[knowledge] resync {} prune failed: {}", source, e);
            }
        }
        Err(e) => warn!("[knowledge] resync {} list_ids failed: {}", source, e),
    }
    let _ = indexer
        .set_connector_cursor(
            &source,
            &serde_json::json!({"last_refresh": chrono::Utc::now().to_rfc3339()}).to_string(),
        )
        .await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "source": source,
        "indexed": total,
        "errors": errors,
        "duration_ms": started.elapsed().as_millis() as u64,
    })))
}

#[derive(serde::Deserialize)]
struct KnowledgeDocDeleteReq {
    token: String,
    doc_id: i64,
}

async fn handle_knowledge_doc_delete(
    State(state): State<Arc<AppState>>,
    Json(req): Json<KnowledgeDocDeleteReq>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _principal = resolve_principal(&state, &req.token).await?;
    let indexer = match &state.indexer {
        Some(i) => i,
        None => return Ok(Json(serde_json::json!({"error": "indexer not available"}))),
    };
    let (source, external_id) = match indexer.get_document_ident(req.doc_id).await {
        Some(p) => p,
        None => {
            return Ok(Json(
                serde_json::json!({"error": format!("doc {} not found", req.doc_id)}),
            ))
        }
    };
    if let Err(e) = indexer.delete_document(&source, &external_id).await {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    // For uploaded_files, also remove the backing file from disk.
    if source == connectors::sources::uploaded_files::SOURCE_NAME {
        if let Some(uf) = &state.uploaded_files {
            uf.delete_by_external_id(&external_id);
        }
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn handle_knowledge_upload(
    State(state): State<Arc<AppState>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let uf = match &state.uploaded_files {
        Some(u) => u,
        None => return Ok(Json(serde_json::json!({"ok": false, "error": "uploads disabled"}))),
    };
    let indexer = match &state.indexer {
        Some(i) => i,
        None => return Ok(Json(serde_json::json!({"ok": false, "error": "indexer not available"}))),
    };

    // Pull the token, agent_id, and return_text fields (multipart field order
    // isn't guaranteed, so we scan all fields).
    let mut token: Option<String> = None;
    let mut agent_id: String = "shared".to_string();
    let mut return_text = false;
    let mut file_bytes: Option<(String, Vec<u8>)> = None; // (original filename, bytes)

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "token" {
            if let Ok(text) = field.text().await {
                token = Some(text);
            }
        } else if name == "agent_id" {
            if let Ok(text) = field.text().await {
                if !text.is_empty() {
                    agent_id = text;
                }
            }
        } else if name == "return_text" {
            if let Ok(text) = field.text().await {
                return_text = text == "1" || text == "true";
            }
        } else if name == "file" {
            let filename = field
                .file_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "upload".to_string());
            match field.bytes().await {
                Ok(b) => file_bytes = Some((filename, b.to_vec())),
                Err(e) => {
                    return Ok(Json(serde_json::json!({
                        "ok": false,
                        "error": format!("read upload: {}", e),
                    })))
                }
            }
        }
    }

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => return Err(axum::http::StatusCode::UNAUTHORIZED),
    };
    let principal = resolve_principal(&state, &token).await?;

    let (orig_name, data) = match file_bytes {
        Some(pair) => pair,
        None => return Ok(Json(serde_json::json!({"ok": false, "error": "no file uploaded"}))),
    };

    let max_bytes = state.config.security.max_upload_size_mb.saturating_mul(1024 * 1024);
    if (data.len() as u64) > max_bytes {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": format!("file exceeds {}MB limit", state.config.security.max_upload_size_mb),
        })));
    }

    // Write to uploads/knowledge/<timestamp>-<uuid>-<sanitized>.
    let ext = std::path::Path::new(&orig_name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let stem = std::path::Path::new(&orig_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("upload");
    let sanitized_stem: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    let unique_name = format!(
        "{}-{}-{}.{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8],
        sanitized_stem,
        ext,
    );
    let target = uf.root().join(&unique_name);
    if let Err(e) = std::fs::write(&target, &data) {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": format!("write: {}", e),
        })));
    }
    // Sidecar: preserves the original filename for display.
    let sidecar = uf.root().join(format!("{}.meta.json", unique_name));
    let _ = std::fs::write(
        &sidecar,
        serde_json::json!({
            "original_filename": orig_name,
            "uploaded_at": chrono::Utc::now().to_rfc3339(),
            "bytes": data.len(),
        })
        .to_string(),
    );

    // Extract + ingest synchronously so the UI can report chunk count.
    let mut doc = match connectors::sources::uploaded_files::file_to_doc(&target) {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Ok(Json(serde_json::json!({
                "ok": false,
                "error": format!("unsupported file type: .{}", ext),
            })))
        }
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "ok": false,
                "error": format!("extract: {}", e),
            })))
        }
    };
    doc.agent_id = agent_id;
    doc.user_id = principal.user_id();
    let body_len = doc.body.len();
    let extracted_text = if return_text { Some(doc.body.clone()) } else { None };
    // Rough chunk count: matches Indexer::chunk_text(800, 150).
    let approx_chunks = (body_len / 650).max(1);
    if let Err(e) = indexer.put_document(doc).await {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": format!("index: {}", e),
        })));
    }
    info!(
        "[knowledge] uploaded {} ({} bytes, ~{} chunks) -> {}",
        orig_name, body_len, approx_chunks, unique_name
    );
    let mut resp = serde_json::json!({
        "ok": true,
        "filename": orig_name,
        "bytes": body_len,
        "chunks": approx_chunks,
        "external_id": unique_name,
    });
    if let Some(text) = extracted_text {
        resp["extracted_text"] = serde_json::Value::String(text);
    }
    Ok(Json(resp))
}

async fn handle_research_recent(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = resolve_principal(&state, token).await?;
    let store = match &state.research_store {
        Some(s) => s,
        None => return Ok(Json(serde_json::json!({"error": "research store not available"}))),
    };
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(15)
        .clamp(1, 100);
    let rows = store.list_recent_all(limit).await;
    Ok(Json(serde_json::json!({ "sessions": rows })))
}

// ── Admin endpoints (v5 Item 3 Stage 5) ────────────────────────────────────
//
// All admin endpoints require `principal.is_admin()`. At the moment, only
// the legacy admin (empty users table + global token) is privileged; once
// the first user is bootstrapped the admin surface is effectively locked
// down. A follow-up polish pass can add an `is_admin` column on `users`.

pub fn require_admin(principal: &auth::Principal) -> Result<(), axum::http::StatusCode> {
    if principal.is_admin() {
        Ok(())
    } else {
        Err(axum::http::StatusCode::FORBIDDEN)
    }
}

#[derive(serde::Deserialize)]
struct AdminCreateUserRequest {
    token: String,
    name: String,
}

async fn handle_admin_create_user(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminCreateUserRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.users.create_user(&req.name).await {
        Ok(u) => Ok(Json(serde_json::to_value(u).unwrap_or_default())),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_list_users(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    match state.users.list_users().await {
        Ok(users) => Ok(Json(serde_json::json!({"users": users}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminMintTokenRequest {
    token: String,
    name: String,
}

async fn handle_admin_mint_token(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<i64>,
    Json(req): Json<AdminMintTokenRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.users.mint_token(user_id, &req.name).await {
        Ok(raw) => Ok(Json(serde_json::json!({
            "user_id": user_id,
            "token": raw,
            "note": "shown once — save this value"
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminRevokeRequest {
    token: String,
}

async fn handle_admin_revoke_token(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(token_id): axum::extract::Path<i64>,
    Json(req): Json<AdminRevokeRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.users.revoke_token(token_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"revoked": token_id}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminLinkTelegramRequest {
    token: String,
    bot_token: String,
    chat_id: i64,
}

async fn handle_admin_link_telegram(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<i64>,
    Json(req): Json<AdminLinkTelegramRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state
        .users
        .link_telegram(user_id, &req.bot_token, req.chat_id)
        .await
    {
        Ok(()) => Ok(Json(serde_json::json!({
            "linked": true,
            "user_id": user_id,
            "chat_id": req.chat_id
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Multi-user management endpoints ────────────────────────────────────────

#[derive(serde::Deserialize)]
struct AdminInviteRequest {
    token: String,
    name_hint: Option<String>,
    #[serde(default = "default_user_role")]
    role: String,
    #[serde(default = "default_invite_hours")]
    expires_hours: u64,
    sharing_preset: Option<String>,
}
fn default_user_role() -> String { "user".to_string() }
fn default_invite_hours() -> u64 { 72 }

async fn handle_admin_invite(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminInviteRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.users.create_invite(
        principal.user_id(),
        req.name_hint.as_deref(),
        &req.role,
        req.expires_hours,
        req.sharing_preset.as_deref(),
    ).await {
        Ok(invite) => Ok(Json(serde_json::json!({
            "ok": true,
            "code": invite.code,
            "expires_at": invite.expires_at,
            "register_url": format!("/register?code={}", invite.code),
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_list_invites(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    match state.users.list_invites().await {
        Ok(invites) => Ok(Json(serde_json::json!({"invites": invites}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct RegisterRequest {
    code: String,
    name: String,
    password: String,
}

async fn handle_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if req.name.trim().is_empty() || req.password.len() < 8 {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": "Name required, password must be at least 8 characters"
        })));
    }
    // Validate and consume invite
    let invite = match state.users.consume_invite(&req.code, 0).await {
        Ok(inv) => inv,
        Err(e) => return Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    };
    // Create user with password
    let password_hash = match crate::auth::users::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => return Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    };
    let user = match state.users.create_user_full(&req.name, &invite.role, Some(&password_hash)).await {
        Ok(u) => u,
        Err(e) => return Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    };
    // Update invite with the actual user_id
    let _ = state.users.consume_invite(&req.code, user.id).await;
    // Apply sharing preset if the invite had one
    if let Some(ref preset) = invite.sharing_preset {
        if !preset.is_empty() {
            let _ = state.users.apply_sharing_preset(invite.created_by, user.id, preset).await;
        }
    }
    // Mint a session token
    let token = match state.users.mint_token_with_expiry(user.id, "registration", Some(48)).await {
        Ok(t) => t,
        Err(e) => return Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    };
    Ok(Json(serde_json::json!({
        "ok": true,
        "user": user,
        "token": token,
        "needs_onboarding": true,
    })))
}

#[derive(serde::Deserialize)]
struct AdminUpdateUserRequest {
    token: String,
    role: Option<String>,
    disabled: Option<bool>,
}

async fn handle_admin_update_user(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<i64>,
    Json(req): Json<AdminUpdateUserRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    if let Some(ref role) = req.role {
        if let Err(e) = state.users.update_user_role(user_id, role).await {
            return Ok(Json(serde_json::json!({"error": e})));
        }
    }
    if let Some(disabled) = req.disabled {
        if disabled {
            if let Err(e) = state.users.disable_user(user_id).await {
                return Ok(Json(serde_json::json!({"error": e})));
            }
        } else {
            if let Err(e) = state.users.enable_user(user_id).await {
                return Ok(Json(serde_json::json!({"error": e})));
            }
        }
    }
    match state.users.get_user(user_id).await {
        Ok(Some(u)) => Ok(Json(serde_json::json!({"ok": true, "user": u}))),
        Ok(None) => Ok(Json(serde_json::json!({"error": "user not found"}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_delete_user(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    match state.users.delete_user(user_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_me(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let user = state.users.get_user(user_id).await.ok().flatten();
    let agents = state.users.list_user_agents(user_id).await.unwrap_or_default();
    let sharing_mode = state.sharing_mode.read().await.clone();
    let user_data_dir = state.users.get_data_dir(user_id).await;
    let data_dir = user_data_dir.unwrap_or_else(|| resolve_data_dir().to_string_lossy().to_string());
    let onboarding_complete = state.users.is_onboarding_complete(user_id).await;
    Ok(Json(serde_json::json!({
        "user": user,
        "role": principal.role(),
        "agents": agents,
        "sharing_mode": sharing_mode,
        "data_dir": data_dir,
        "onboarding_complete": onboarding_complete,
    })))
}

#[derive(serde::Deserialize)]
struct ChangePasswordRequest {
    token: String,
    current_password: Option<String>,
    new_password: String,
}

async fn handle_change_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let user_id = principal.user_id();
    if req.new_password.len() < 4 {
        return Ok(Json(serde_json::json!({"ok": false, "error": "Password must be at least 4 characters"})));
    }
    // If user already has a password, verify current
    if state.users.has_password(user_id).await.unwrap_or(false) {
        let current = req.current_password.as_deref().unwrap_or("");
        if !state.users.verify_password(user_id, current).await.unwrap_or(false) {
            return Ok(Json(serde_json::json!({"ok": false, "error": "Current password is incorrect"})));
        }
    }
    match state.users.set_password(user_id, &req.new_password).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    }
}

// ── Sharing config endpoints ──────────────────────────────────────────────

async fn handle_admin_get_sharing(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    let mode = state.sharing_mode.read().await.clone();
    Ok(Json(serde_json::json!({"mode": mode})))
}

#[derive(serde::Deserialize)]
struct AdminSetSharingRequest {
    token: String,
    mode: String,
}

async fn handle_admin_set_sharing(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminSetSharingRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    if let Err(e) = state.users.set_sharing_mode(&req.mode, principal.user_id()).await {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    *state.sharing_mode.write().await = req.mode.clone();
    Ok(Json(serde_json::json!({"ok": true, "mode": req.mode})))
}

// ── User agent endpoints ──────────────────────────────────────────────────

async fn handle_me_agents(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let agents = state.users.list_user_agents(principal.user_id()).await.unwrap_or_default();
    Ok(Json(serde_json::json!({"agents": agents})))
}

#[derive(serde::Deserialize)]
struct CreateUserAgentRequest {
    token: String,
    agent_id: String,
    display_name: String,
    #[serde(default = "default_base_agent")]
    base_agent: String,
    system_prompt: Option<String>,
}
fn default_base_agent() -> String { "main".to_string() }

async fn handle_create_user_agent(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUserAgentRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    match state.users.create_user_agent(
        principal.user_id(),
        &req.agent_id,
        &req.display_name,
        &req.base_agent,
        req.system_prompt.as_deref(),
    ).await {
        Ok(agent) => Ok(Json(serde_json::json!({"ok": true, "agent": agent}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct UpdateUserAgentRequest {
    token: String,
    display_name: Option<String>,
    system_prompt: Option<Option<String>>,
    enabled: Option<bool>,
}

async fn handle_update_user_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    Json(req): Json<UpdateUserAgentRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    match state.users.update_user_agent(
        principal.user_id(),
        &agent_id,
        req.display_name.as_deref(),
        req.system_prompt.as_ref().map(|o| o.as_deref()),
        req.enabled,
    ).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_delete_user_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    match state.users.delete_user_agent(principal.user_id(), &agent_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Sharing grants API ────────────────────────────────────────────────────

async fn handle_admin_sharing_options(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    // Collect available resource types and their resource_ids
    let agents: Vec<String> = state.config.agents.list.iter().map(|a| a.id.clone()).collect();
    let modules: Vec<&str> = crate::modules::CORE_MODULES.iter().map(|m| m.id).collect();
    Ok(Json(serde_json::json!({
        "resource_types": [
            {"type": "oauth", "label": "OAuth Connections", "ids": ["*"]},
            {"type": "sync_connection", "label": "Sync Connectors", "ids": ["*"]},
            {"type": "music", "label": "Music", "ids": ["*"]},
            {"type": "knowledge", "label": "Knowledge Bases", "ids": agents},
            {"type": "conversations", "label": "Conversations", "ids": agents},
            {"type": "tax", "label": "Tax Data", "ids": ["*"]},
            {"type": "calendar", "label": "Calendar", "ids": ["*"]},
            {"type": "todos", "label": "Todos", "ids": ["*"]},
            {"type": "research", "label": "Research", "ids": ["*"]},
        ],
        "agents": agents,
        "modules": modules,
    })))
}

async fn handle_admin_sharing_grants_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    let user_id: i64 = params.get("user_id").and_then(|s| s.parse().ok()).unwrap_or(0);
    match state.users.list_grants_for_user(user_id).await {
        Ok(grants) => Ok(Json(serde_json::json!({"grants": grants}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminSetGrantsRequest {
    token: String,
    grantee_user_id: i64,
    grants: Vec<GrantEntry>,
}

#[derive(serde::Deserialize)]
struct GrantEntry {
    resource_type: String,
    resource_id: Option<String>,
}

async fn handle_admin_set_sharing_grants(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminSetGrantsRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    let grant_tuples: Vec<(String, Option<String>)> = req.grants.iter()
        .map(|g| (g.resource_type.clone(), g.resource_id.clone()))
        .collect();
    match state.users.set_grants(principal.user_id(), req.grantee_user_id, &grant_tuples).await {
        Ok(count) => Ok(Json(serde_json::json!({"ok": true, "count": count}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_delete_sharing_grant(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(grant_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    match state.users.delete_grant(grant_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Personality docs API ──────────────────────────────────────────────────

async fn handle_personality_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let agent_id = params.get("agent_id").map(|s| s.as_str()).unwrap_or("main");
    match state.users.list_personality_docs(principal.user_id(), agent_id).await {
        Ok(docs) => Ok(Json(serde_json::json!({"docs": docs}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct CreatePersonalityDocRequest {
    token: String,
    agent_id: String,
    doc_type: String,
    title: String,
    content: String,
}

async fn handle_personality_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePersonalityDocRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    match state.users.create_personality_doc(
        principal.user_id(), &req.agent_id, &req.doc_type, &req.title, &req.content,
    ).await {
        Ok(doc) => Ok(Json(serde_json::json!({"ok": true, "doc": doc}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct UpdatePersonalityDocRequest {
    token: String,
    title: Option<String>,
    content: Option<String>,
}

async fn handle_personality_update(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(doc_id): axum::extract::Path<i64>,
    Json(req): Json<UpdatePersonalityDocRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    match state.users.update_personality_doc(doc_id, principal.user_id(), req.title.as_deref(), req.content.as_deref()).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_personality_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(doc_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    match state.users.delete_personality_doc(doc_id, principal.user_id()).await {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Onboarding API ───────────────────────────────────────────────────────

async fn handle_onboarding_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let complete = state.users.is_onboarding_complete(principal.user_id()).await;
    Ok(Json(serde_json::json!({"complete": complete})))
}

async fn handle_onboarding_complete(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    let _ = state.users.set_onboarding_complete(principal.user_id()).await;
    Ok(Json(serde_json::json!({"ok": true})))
}

// ── Data location migration ───────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct DataLocationRequest {
    token: String,
    path: String,
}

async fn handle_data_location_change(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DataLocationRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let user_id = principal.user_id();
    let new_dir = std::path::PathBuf::from(&req.path);

    // Validate: path must be absolute
    if !new_dir.is_absolute() {
        return Ok(Json(serde_json::json!({"ok": false, "error": "Path must be absolute"})));
    }

    // Create the new directory
    if let Err(e) = std::fs::create_dir_all(&new_dir) {
        return Ok(Json(serde_json::json!({"ok": false, "error": format!("Cannot create directory: {e}")})));
    }

    // Determine old data location
    let system_data_dir = resolve_data_dir();
    let old_user_dir = state.users.get_data_dir(user_id).await
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| system_data_dir.join(format!("users/{user_id}")));

    let mut migrated_count = 0u64;

    // Migrate user agent workspaces
    let old_agents = old_user_dir.join("agents");
    let new_agents = new_dir.join("agents");
    if old_agents.is_dir() {
        if let Err(e) = std::fs::create_dir_all(&new_agents) {
            log::warn!("[data-location] create agents dir: {e}");
        } else {
            migrated_count += migrate_dir_contents(&old_agents, &new_agents);
        }
    }

    // Migrate user uploads (knowledge files)
    let old_uploads = system_data_dir.join("uploads/knowledge");
    let new_uploads = new_dir.join("uploads/knowledge");
    // Only migrate if user had their own upload dir, not the shared one
    let user_upload_dir = old_user_dir.join("uploads/knowledge");
    if user_upload_dir.is_dir() {
        if let Err(e) = std::fs::create_dir_all(&new_uploads) {
            log::warn!("[data-location] create uploads dir: {e}");
        } else {
            migrated_count += migrate_dir_contents(&user_upload_dir, &new_uploads);
        }
    }

    // Save new data_dir
    if let Err(e) = state.users.set_data_dir(user_id, &req.path).await {
        return Ok(Json(serde_json::json!({"ok": false, "error": e})));
    }

    info!(
        "[data-location] user {} moved data to {} ({} files migrated)",
        user_id, req.path, migrated_count
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "migrated": migrated_count > 0,
        "files_moved": migrated_count,
        "new_path": req.path,
    })))
}

/// Move all files and subdirectories from src to dst. Returns count of files moved.
fn migrate_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> u64 {
    let mut count = 0u64;
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let src_path = entry.path();
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let dst_path = dst.join(&name);

        if src_path.is_dir() {
            let _ = std::fs::create_dir_all(&dst_path);
            count += migrate_dir_contents(&src_path, &dst_path);
            // Remove empty source dir after migration
            let _ = std::fs::remove_dir(&src_path);
        } else {
            // Copy then remove (safer than rename across filesystems)
            match std::fs::copy(&src_path, &dst_path) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&src_path);
                    count += 1;
                }
                Err(e) => {
                    log::warn!("[data-location] copy {:?} -> {:?}: {}", src_path, dst_path, e);
                }
            }
        }
    }
    count
}

// ── Tool hooks endpoints (4features Stage 2) ──────────────────────────────

#[derive(serde::Deserialize, Debug)]
struct AdminCreateHookRequest {
    token: String,
    event: String,                  // 'pre_tool_call' | 'post_tool_call'
    #[serde(default)]
    match_pattern: serde_json::Value,
    action: String,                 // 'telegram_notify' | 'audit_log' | 'block' | 'run_skill'
    #[serde(default)]
    action_config: serde_json::Value,
}

async fn handle_admin_create_hook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminCreateHookRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    let event = match req.event.as_str() {
        "pre_tool_call" => tool_hooks::HookEvent::PreToolCall,
        "post_tool_call" => tool_hooks::HookEvent::PostToolCall,
        _ => return Ok(Json(serde_json::json!({"error": "event must be pre_tool_call or post_tool_call"}))),
    };
    match state
        .tool_hooks
        .create(event, &req.match_pattern, &req.action, &req.action_config)
        .await
    {
        Ok(id) => Ok(Json(serde_json::json!({"id": id, "created": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_list_hooks(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    match state.tool_hooks.list().await {
        Ok(hooks) => Ok(Json(serde_json::json!({"hooks": hooks}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminDeleteHookRequest {
    token: String,
}

async fn handle_admin_delete_hook(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(hook_id): axum::extract::Path<i64>,
    Json(req): Json<AdminDeleteHookRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.tool_hooks.delete(hook_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"deleted": hook_id}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Skills admin endpoints (4features Stage 3) ────────────────────────────

#[derive(serde::Deserialize)]
struct AdminCreateSkillRequest {
    token: String,
    name: String,
    description: String,
    #[serde(default = "default_main_agent")]
    agent_id: String,
    kind: String,                  // 'binary' | 'prompt' | 'tool_chain'
    body: String,
    #[serde(default)]
    args_schema: Option<serde_json::Value>,
    #[serde(default)]
    requires_approval: bool,
}

fn default_main_agent() -> String {
    "main".to_string()
}

async fn handle_admin_create_skill(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminCreateSkillRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    let kind = match req.kind.as_str() {
        "binary" => skills::SkillKind::Binary,
        "prompt" => skills::SkillKind::Prompt,
        "tool_chain" => skills::SkillKind::ToolChain,
        _ => {
            return Ok(Json(serde_json::json!({
                "error": "kind must be binary, prompt, or tool_chain"
            })));
        }
    };
    match state
        .skills
        .create(
            &req.name,
            &req.description,
            &req.agent_id,
            kind,
            &req.body,
            req.args_schema.as_ref(),
            req.requires_approval,
        )
        .await
    {
        Ok(id) => Ok(Json(serde_json::json!({"id": id, "created": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_list_skills(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    let agent_filter = params.get("agent").map(|s| s.as_str());
    match state.skills.list(agent_filter).await {
        Ok(skills) => Ok(Json(serde_json::json!({"skills": skills}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminDeleteSkillRequest {
    token: String,
}

async fn handle_admin_delete_skill(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(skill_id): axum::extract::Path<i64>,
    Json(req): Json<AdminDeleteSkillRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.skills.delete(skill_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"deleted": skill_id}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct RunSkillRequest {
    token: String,
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

async fn handle_run_skill(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunSkillRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.skills.run(&req.name, &req.args).await {
        Ok(out) => Ok(Json(serde_json::json!({"ok": true, "output": out}))),
        Err(e) => Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    }
}

// ── Plans endpoints (4features Stage 4) ───────────────────────────────────

#[derive(serde::Deserialize)]
struct ProposePlanRequest {
    token: String,
    #[serde(default = "default_main_agent")]
    agent_id: String,
    title: String,
    #[serde(default)]
    rationale: String,
    steps: Vec<plans::ProposeStep>,
    /// If true, send the Telegram approval keyboard immediately. Defaults
    /// to true; set to false for purely-headless plans (admin-approve via
    /// the HTTP endpoint instead).
    #[serde(default = "default_true")]
    send_telegram: bool,
}

fn default_true() -> bool {
    true
}

async fn handle_propose_plan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProposePlanRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    let plan_id = match state
        .plans
        .create(
            principal.user_id(),
            &req.agent_id,
            &req.title,
            &req.rationale,
            &req.steps,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => return Ok(Json(serde_json::json!({"error": e}))),
    };

    if req.send_telegram {
        // Look up the plan we just created so we can render the steps in
        // the approval keyboard message.
        if let Ok(Some((plan, steps))) = state.plans.get(plan_id).await {
            let bot_token = &state.config.channels.telegram.bot_token;
            let chat_id = state.config.channels.telegram.extra.get("chatId")
                .and_then(|v| v.as_i64())
                .or_else(|| state.config.channels.telegram.accounts.values()
                    .next()
                    .and_then(|a| a.extra.get("chatId"))
                    .and_then(|v| v.as_i64()))
                .unwrap_or(0);
            if bot_token.is_empty() || chat_id == 0 {
                warn!("[plans] No Telegram bot_token/chatId configured — skipping approval send");
            } else if let Err(e) = plans::send_approval(
                &state.client,
                bot_token,
                chat_id,
                plan_id,
                &plan,
                &steps,
            )
            .await
            {
                warn!("[plans] failed to send approval for {}: {}", plan_id, e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "plan_id": plan_id,
        "status": "pending"
    })))
}

#[derive(serde::Deserialize)]
struct ApprovePlanRequest {
    token: String,
    /// If true, mark approved AND spawn the executor in the background.
    /// If false, mark approved without executing.
    #[serde(default = "default_true")]
    execute: bool,
}

async fn handle_approve_plan(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(plan_id): axum::extract::Path<i64>,
    Json(req): Json<ApprovePlanRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    // Verify ownership: only the plan creator or an admin can approve.
    match state.plans.get(plan_id).await {
        Ok(Some((plan, _))) => {
            if plan.user_id != principal.user_id() && !principal.is_admin() {
                return Err(axum::http::StatusCode::FORBIDDEN);
            }
        }
        Ok(None) => return Ok(Json(serde_json::json!({"error": "not found"}))),
        Err(e) => return Ok(Json(serde_json::json!({"error": e}))),
    }
    if let Err(e) = state.plans.mark_approved(plan_id).await {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    // Wake up any propose_plan caller blocked on the in-memory registry
    state.plan_registry.resolve(plan_id, true).await;

    if req.execute {
        spawn_plan_executor(Arc::clone(&state), plan_id);
    }
    Ok(Json(serde_json::json!({
        "plan_id": plan_id,
        "approved": true,
        "executing": req.execute
    })))
}

#[derive(serde::Deserialize)]
struct DenyPlanRequest {
    token: String,
}

async fn handle_deny_plan(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(plan_id): axum::extract::Path<i64>,
    Json(req): Json<DenyPlanRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    // Verify ownership: only the plan creator or an admin can deny.
    match state.plans.get(plan_id).await {
        Ok(Some((plan, _))) => {
            if plan.user_id != principal.user_id() && !principal.is_admin() {
                return Err(axum::http::StatusCode::FORBIDDEN);
            }
        }
        Ok(None) => return Ok(Json(serde_json::json!({"error": "not found"}))),
        Err(e) => return Ok(Json(serde_json::json!({"error": e}))),
    }
    if let Err(e) = state.plans.mark_denied(plan_id).await {
        return Ok(Json(serde_json::json!({"error": e})));
    }
    state.plan_registry.resolve(plan_id, false).await;
    Ok(Json(serde_json::json!({"plan_id": plan_id, "denied": true})))
}

async fn handle_get_plan(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(plan_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    match state.plans.get(plan_id).await {
        Ok(Some((plan, steps))) => {
            // Verify ownership: only the plan creator or an admin can view.
            if plan.user_id != principal.user_id() && !principal.is_admin() {
                return Err(axum::http::StatusCode::FORBIDDEN);
            }
            Ok(Json(serde_json::json!({"plan": plan, "steps": steps})))
        }
        Ok(None) => Ok(Json(serde_json::json!({"error": "not found"}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_list_plans(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    // Non-admin users see only their own plans; admin sees all.
    let filter = if principal.is_admin() {
        None
    } else {
        Some(principal.user_id())
    };
    match state.plans.list(filter).await {
        Ok(plans) => Ok(Json(serde_json::json!({"plans": plans}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

// ── Slash command endpoints (4features Stage 5) ───────────────────────────

#[derive(serde::Deserialize)]
struct AdminCreateSlashRequest {
    token: String,
    name: String,                // without the leading /
    description: String,
    #[serde(default)]
    agent_filter: Option<String>,
    kind: String,                // 'direct' | 'text_prompt' | 'skill_ref'
    body_template: String,
    #[serde(default)]
    args_schema: Option<serde_json::Value>,
}

async fn handle_admin_create_slash(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminCreateSlashRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    let kind = match req.kind.as_str() {
        "direct" => slash::SlashKind::Direct,
        "text_prompt" => slash::SlashKind::TextPrompt,
        "skill_ref" => slash::SlashKind::SkillRef,
        _ => {
            return Ok(Json(serde_json::json!({
                "error": "kind must be direct, text_prompt, or skill_ref"
            })));
        }
    };
    // Strip a leading / if the caller sent one — names are stored without it.
    let name = req.name.strip_prefix('/').unwrap_or(&req.name);
    match state
        .slash
        .create(
            name,
            &req.description,
            req.agent_filter.as_deref(),
            kind,
            &req.body_template,
            req.args_schema.as_ref(),
        )
        .await
    {
        Ok(id) => Ok(Json(serde_json::json!({"id": id, "name": name, "created": true}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

async fn handle_admin_list_slash(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = resolve_principal(&state, token).await?;
    require_admin(&principal)?;
    let agent_filter = params.get("agent").map(|s| s.as_str());
    match state.slash.list(agent_filter).await {
        Ok(rows) => Ok(Json(serde_json::json!({"slash_commands": rows}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct AdminDeleteSlashRequest {
    token: String,
}

async fn handle_admin_delete_slash(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(slash_id): axum::extract::Path<i64>,
    Json(req): Json<AdminDeleteSlashRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    require_admin(&principal)?;
    match state.slash.delete(slash_id).await {
        Ok(()) => Ok(Json(serde_json::json!({"deleted": slash_id}))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct DispatchSlashRequest {
    token: String,
    /// Slash command name, with or without the leading /.
    name: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    args: serde_json::Value,
}

/// Dispatch a slash command. Looks up by name (+ optional agent), then
/// branches by kind:
///   - `direct` → returns `{kind: direct, endpoint, args}` so the caller
///     (typically Telegram poller or admin UI) can post the args itself.
///     We don't proxy the call here to avoid leaking the gateway's own
///     auth into a synthetic-recursive call.
///   - `text_prompt` → returns `{kind: text_prompt, prompt}` with the
///     expanded template. Caller treats it as a normal user message.
///   - `skill_ref` → invokes the skill and returns its output.
async fn handle_dispatch_slash(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DispatchSlashRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _principal = resolve_principal(&state, &req.token).await?;
    let name = req.name.strip_prefix('/').unwrap_or(&req.name);
    let row = match state
        .slash
        .get_by_name(name, req.agent_id.as_deref())
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Ok(Json(serde_json::json!({
                "error": format!("slash command '/{}' not found", name)
            })));
        }
        Err(e) => return Ok(Json(serde_json::json!({"error": e}))),
    };
    match row.kind {
        slash::SlashKind::Direct => Ok(Json(serde_json::json!({
            "kind": "direct",
            "endpoint": row.body_template,
            "args": req.args,
        }))),
        slash::SlashKind::TextPrompt => {
            let prompt = slash::expand_template(&row.body_template, &req.args);
            Ok(Json(serde_json::json!({
                "kind": "text_prompt",
                "prompt": prompt,
            })))
        }
        slash::SlashKind::SkillRef => {
            // body_template holds the skill name
            match state.skills.run(&row.body_template, &req.args).await {
                Ok(output) => Ok(Json(serde_json::json!({
                    "kind": "skill_ref",
                    "skill": row.body_template,
                    "output": output,
                }))),
                Err(e) => Ok(Json(serde_json::json!({
                    "kind": "skill_ref",
                    "skill": row.body_template,
                    "error": e,
                }))),
            }
        }
    }
}

/// Spawn the plan executor as a detached background task. The dispatcher
/// closure routes step execution by `StepKind`:
///   - `Tool` → call the tool by name via the existing tool registry
///   - `Skill` → call SkillStore::run
///   - `Note` → handled inside the executor itself, never reaches us
///
/// The executor uses a fresh ToolRegistry per step (cheap; same pattern
/// the HTTP handlers use) so it picks up the current state of hooks /
/// extensions / approvals.
pub(crate) fn spawn_plan_executor(state: Arc<AppState>, plan_id: i64) {
    tokio::spawn(async move {
        let store = Arc::clone(&state.plans);
        // Fetch the plan to get its user_id for scoped execution.
        let plan_user_id = match store.get(plan_id).await {
            Ok(Some((plan, _))) => plan.user_id,
            _ => 0,
        };
        let dispatcher = move |kind: plans::StepKind, target: String, args: serde_json::Value| {
            let state = Arc::clone(&state);
            async move {
                match kind {
                    plans::StepKind::Note => Ok(target),
                    plans::StepKind::Skill => state.skills.run(&target, &args).await,
                    plans::StepKind::Tool => {
                        // Build a minimal one-shot ToolRegistry on a workspace
                        // we know exists. Plans run scoped to the plan creator's
                        // user_id so tool hooks and audit entries are attributed
                        // correctly.
                        let workspace = crate::resolve_data_dir().join("workspace-main");
                        let mut tr = crate::tools::ToolRegistry::with_extensions(
                            workspace,
                            "main".to_string(),
                            Some(Arc::clone(&state.mcp)),
                            state.indexer.clone(),
                        );
                        tr.add_extension_tools(&state.openapi_tools);
                        tr.set_infra(
                            Arc::clone(&state.tool_rate_limiter),
                            Arc::clone(&state.tool_circuit_breakers),
                        );
                        tr.set_user_id(plan_user_id);
                        tr.set_db_path(state.db_path.clone());
                        tr.set_tool_hooks(Arc::clone(&state.tool_hooks));
                        {
                            let run_skill: Arc<dyn crate::tools::extension::Tool> =
                                Arc::new(skills::RunSkillTool {
                                    store: Arc::clone(&state.skills),
                                });
                            let delegate: Arc<dyn crate::tools::extension::Tool> =
                                Arc::new(crate::tools::subagent::DelegateTool::new(
                                    Arc::new(state.config.clone()),
                                    state.client.clone(),
                                ));
                            tr.add_extension_tools(&[run_skill, delegate]);
                        }
                        let call = crate::tools::ToolCall {
                            id: format!("plan-{}-step", plan_id),
                            name: target.clone(),
                            arguments: args,
                        };
                        let result = tr.execute(&call).await;
                        if result.success {
                            Ok(result.output)
                        } else {
                            Err(result.output)
                        }
                    }
                }
            }
        };
        if let Err(e) = plans::execute_plan(store, plan_id, dispatcher).await {
            warn!("[plans:{}] executor failed: {}", plan_id, e);
        }
    });
}

// ── OAuth2 authorization_code endpoints (v5 Item 4 Stage 3) ───────────────
//
// These endpoints implement the user-interactive OAuth flow. The flow:
//   1. Caller hits POST /api/oauth/start { provider } → returns auth_url
//   2. User opens auth_url in browser, approves, gets redirected to
//      GET /api/oauth/callback?code=...&state=...
//   3. Callback exchanges code for tokens, persists to oauth_tokens
//   4. Subsequent tool calls look up the token by (user_id, provider)
//
// The provider catalog (auth/token URLs, client credentials) lives in
// `config.oauth.providers` keyed by provider name. /start looks up the
// provider by name, mints a state + PKCE pair, stores them in the
// in-memory state cache, and returns the URL the user should visit.

#[derive(serde::Deserialize)]
struct OAuthStartRequest {
    token: String,
    provider: String,
}

async fn handle_oauth_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OAuthStartRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;

    // Resolve credentials: check oauth_config table (runtime) first,
    // then fall back to static config.oauth.providers. Identity-provider-aware:
    // one Google config unlocks gmail/calendar/youtube_music/youtube.
    let creds = sync::resolve_oauth_credentials(&state, &req.provider).await;
    let Some((client_id, _client_secret, authorization_url, _token_url, scopes)) = creds else {
        return Ok(Json(serde_json::json!({
            "error": format!("OAuth not configured for '{}'. Go to Sync settings → find this provider → Setup OAuth.", req.provider),
            "needs_config": true,
            "identity_provider": sync::identity_for(&req.provider),
        })));
    };

    // Redirect URI: prefer static config if present, else compute from request origin.
    // For simplicity, use the existing static config as the redirect URI source.
    let redirect_uri = state.config.oauth.providers.get(&req.provider)
        .map(|p| p.redirect_uri.clone())
        .or_else(|| sync::identity_for(&req.provider)
            .and_then(|i| state.config.oauth.providers.get(i))
            .map(|p| p.redirect_uri.clone()))
        .unwrap_or_else(|| format!("http://localhost:{}/api/oauth/callback", state.config.gateway.port));

    let pkce = oauth::PkcePair::generate();
    let state_value = oauth::pkce::generate_state();

    state
        .oauth_state
        .insert(
            state_value.clone(),
            oauth::PendingAuthEntry {
                user_id: principal.user_id(),
                provider: req.provider.clone(),
                code_verifier: pkce.verifier.clone(),
                redirect_uri: redirect_uri.clone(),
                created_at: std::time::Instant::now(),
            },
        )
        .await;

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent",
        authorization_url,
        urlencode(&client_id),
        urlencode(&redirect_uri),
        urlencode(&scopes),
        urlencode(&state_value),
        urlencode(&pkce.challenge),
    );

    Ok(Json(serde_json::json!({
        "provider": req.provider,
        "auth_url": auth_url,
        "expires_in_secs": 600,
    })))
}

#[derive(serde::Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn handle_oauth_callback(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<OAuthCallbackQuery>,
) -> Result<axum::response::Html<String>, axum::http::StatusCode> {
    if let Some(err) = q.error {
        return Ok(axum::response::Html(format!(
            "<h1>OAuth error</h1><p>provider rejected: {}</p>",
            html_escape(&err)
        )));
    }
    let code = q
        .code
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let state_value = q
        .state
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

    let entry = match state.oauth_state.take(&state_value).await {
        Some(e) => e,
        None => {
            return Ok(axum::response::Html(
                "<h1>OAuth error</h1><p>state mismatch or expired \
                — start the flow again.</p>"
                    .to_string(),
            ));
        }
    };
    let provider_cfg = match state.config.oauth.providers.get(&entry.provider) {
        Some(p) => p,
        None => {
            return Ok(axum::response::Html(format!(
                "<h1>OAuth error</h1><p>provider '{}' no longer configured</p>",
                html_escape(&entry.provider)
            )));
        }
    };

    // Exchange the code for tokens.
    let http = &state.client;
    let form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("client_id", &provider_cfg.client_id),
        ("client_secret", &provider_cfg.client_secret),
        ("redirect_uri", &entry.redirect_uri),
        ("code_verifier", &entry.code_verifier),
    ];
    let resp = match http
        .post(&provider_cfg.token_url)
        .form(&form)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(axum::response::Html(format!(
                "<h1>OAuth error</h1><p>token exchange failed: {}</p>",
                html_escape(&e.to_string())
            )));
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Ok(axum::response::Html(format!(
            "<h1>OAuth error</h1><p>provider returned {}</p><pre>{}</pre>",
            status,
            html_escape(&body)
        )));
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            return Ok(axum::response::Html(format!(
                "<h1>OAuth error</h1><p>parse: {}</p>",
                html_escape(&e.to_string())
            )));
        }
    };

    let access = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let refresh = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_in = body.get("expires_in").and_then(|v| v.as_i64());
    let now = chrono::Utc::now().timestamp();
    let expires_at = expires_in.map(|s| now + s);
    let scope = body
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Err(e) = state
        .oauth_tokens
        .upsert(
            entry.user_id,
            &entry.provider,
            &access,
            refresh.as_deref(),
            expires_at,
            &scope,
        )
        .await
    {
        return Ok(axum::response::Html(format!(
            "<h1>OAuth error</h1><p>persist: {}</p>",
            html_escape(&e)
        )));
    }

    Ok(axum::response::Html(format!(
        "<h1>Connected</h1><p>provider <code>{}</code> linked to user {}.</p>\
         <p>You can close this tab and return to your client.</p>",
        html_escape(&entry.provider),
        entry.user_id
    )))
}

#[derive(serde::Deserialize)]
struct OAuthStatusQuery {
    token: String,
    provider: String,
}

async fn handle_oauth_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<OAuthStatusQuery>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &q.token).await?;
    match state
        .oauth_tokens
        .peek(principal.user_id(), &q.provider)
        .await
    {
        Ok(Some(row)) => Ok(Json(serde_json::json!({
            "connected": true,
            "provider": row.provider,
            "scope": row.scope,
            "expires_at": row.expires_at,
            "updated_at": row.updated_at,
        }))),
        Ok(None) => Ok(Json(serde_json::json!({
            "connected": false,
            "provider": q.provider,
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

#[derive(serde::Deserialize)]
struct OAuthDisconnectRequest {
    token: String,
    provider: String,
}

async fn handle_oauth_disconnect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OAuthDisconnectRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = resolve_principal(&state, &req.token).await?;
    match state
        .oauth_tokens
        .delete(principal.user_id(), &req.provider)
        .await
    {
        Ok(()) => Ok(Json(serde_json::json!({
            "disconnected": true,
            "provider": req.provider,
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e}))),
    }
}

/// Tiny URL-encoder so we don't need a `url` crate dep just for this.
/// Encodes everything that isn't an unreserved RFC 3986 char.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// HTML escape for the few user-controlled values we splat into the
/// callback page. Same minimal subset PHP's htmlspecialchars covers.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Set up panic hook that logs instead of crashing
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        let location = info.location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        eprintln!("[PANIC] {} at {}", msg, location);
    }));

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    // Subcommand dispatch — v5 Item 3 Stage 5.
    // `syntaur bootstrap-admin --name <name>` creates the first
    // real user + mints a token, prints it once to stdout, exits. Used
    // to graduate from the legacy global-token era into per-user auth.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(raw_args.first().map(|s| s.as_str()), Some("bootstrap-admin")) {
        run_bootstrap_admin(&raw_args).await;
        return;
    }
    // `syntaur mint-token --user <name|id> [--label <label>]` mints
    // an extra API token for an existing user. Sibling of bootstrap-admin
    // for the case where you've lost the original token but still need
    // admin access — e.g. user_id=1 (sean) needs a fresh credential.
    if matches!(raw_args.first().map(|s| s.as_str()), Some("mint-token")) {
        run_mint_token(&raw_args).await;
        return;
    }

    let data_dir_str = crate::resolve_data_dir().to_string_lossy().to_string();
    info!("syntaur v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
            resolve_data_dir().join("syntaur.json")
        });

    info!("Loading config from {}", config_path.display());
    let ConfigLoadResult { config, warnings } = config::load_config(&config_path);

    for w in &warnings {
        warn!("Config: {}", w);
    }

    // Log what we loaded
    info!("  Agents: {}", config.agents.list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(", "));
    info!("  LLM providers: {}", config.models.providers.keys().cloned().collect::<Vec<_>>().join(", "));
    info!("  Telegram accounts: {}", config.telegram_accounts().iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", "));
    info!("  Gateway port: {}", config.gateway.port);

    // Open the document index. Failure here is non-fatal — internal_search
    // just won't be available, but the rest of the system runs.
    let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let index_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let indexer: Option<Arc<index::Indexer>> = match index::Indexer::open(index_path.clone()) {
        Ok(idx) => {
            let stats = idx.stats(None).await;
            info!(
                "  Indexer: {} docs, {} chunks across {} sources",
                stats.documents, stats.chunks, stats.sources
            );
            Some(idx)
        }
        Err(e) => {
            warn!("Indexer disabled: {}", e);
            None
        }
    };

    // Open the research session store on the same database file. Indexer
    // migrations are already complete by this point so the v2 research
    // tables exist.
    let research_store: Option<Arc<research::SessionStore>> = if indexer.is_some() {
        match research::SessionStore::open(index_path.clone()) {
            Ok(s) => {
                info!("  Research session store: ready");
                Some(s)
            }
            Err(e) => {
                warn!("Research session store disabled: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Open the approval pending-actions store on the same database file.
    // Schema v3 migration creates the pending_actions table.
    let approval_store: Option<Arc<approval::PendingActionStore>> = if indexer.is_some() {
        match approval::PendingActionStore::open(index_path) {
            Ok(s) => {
                info!("  Approval store: ready");
                Some(s)
            }
            Err(e) => {
                warn!("Approval store disabled: {}", e);
                None
            }
        }
    } else {
        None
    };
    // Now that the research_store exists, attach it as a stale notifier on the
    // indexer so any subsequent put_document on a CHANGED doc invalidates
    // cached research sessions that cited it.
    let indexer = if let (Some(idx), Some(rs)) = (indexer.as_ref(), research_store.as_ref()) {
        let notifier: Arc<dyn index::StaleNotifier> = Arc::clone(rs) as Arc<dyn index::StaleNotifier>;
        Some(Arc::clone(idx).with_stale_notifier(notifier))
    } else {
        indexer
    };

    let approval_registry = approval::ApprovalRegistry::new();

    // Conversation manager — explicit session/resume for the main agent loop.
    // Same database as the indexer (schema v5 added the conversations_v2 tables).
    let home_dir2 = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let conv_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let conversations: Option<Arc<conversations::ConversationManager>> = if indexer.is_some() {
        match conversations::ConversationManager::open(conv_path) {
            Ok(c) => {
                info!("  Conversations: ready");
                Some(c)
            }
            Err(e) => {
                warn!("Conversations disabled: {}", e);
                None
            }
        }
    } else {
        None
    };

    // User store — schema v7 tables on the same index.db. Opens always
    // succeed because the tables are created by the migration runner at
    // Indexer::open above; if the indexer path failed we still need a
    // UserStore so the Principal extractor has something to resolve
    // tokens against, which is why this is unconditional (vs
    // conversations/lcm which gate on indexer).
    // v5 Item 3.
    let users_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let users = auth::UserStore::open(users_path).unwrap_or_else(|e| {
        warn!("UserStore open failed ({}); legacy admin token only", e);
        // Build an in-memory UserStore as a last-resort fallback so the
        // Principal extractor still works — it will always report
        // is_empty() = true and fall through to the legacy token path.
        auth::UserStore::open(PathBuf::from(":memory:"))
            .expect("in-memory UserStore")
    });
    info!("  Users: ready");

    // Tool hooks — user-configurable PreToolUse / PostToolUse callbacks.
    // Loaded from schema v9 `tool_hooks` table. 4features Stage 2.
    let hooks_db_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let hooks_http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let hooks_tg_token = config.channels.telegram.bot_token.clone();
    let hooks_tg_chat_id = config.channels.telegram.extra.get("chatId")
        .and_then(|v| v.as_i64())
        .or_else(|| config.channels.telegram.accounts.values()
            .next()
            .and_then(|a| a.extra.get("chatId"))
            .and_then(|v| v.as_i64()))
        .unwrap_or(0);
    let tool_hooks_store = match tool_hooks::HookStore::open(
        hooks_db_path,
        hooks_tg_token,
        hooks_tg_chat_id,
        hooks_http,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!("HookStore open failed ({}); falling back to in-memory", e);
            tool_hooks::HookStore::open(
                PathBuf::from(":memory:"),
                String::new(),
                0,
                reqwest::Client::new(),
            )
            .await
            .expect("in-memory HookStore")
        }
    };
    info!("  Tool hooks: ready");

    // Skills registry — named, reusable workflows. 4features Stage 3.
    let skills_db_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let skills_store = match skills::SkillStore::open(skills_db_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("SkillStore open failed ({}); falling back to in-memory", e);
            skills::SkillStore::open(PathBuf::from(":memory:"))
                .expect("in-memory SkillStore")
        }
    };
    info!("  Skills: ready");

    // Plans store — persisted multi-step plans, gated by Telegram approval.
    // 4features Stage 4.
    let plans_db_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let plans_store = match plans::PlanStore::open(plans_db_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("PlanStore open failed ({}); falling back to in-memory", e);
            plans::PlanStore::open(PathBuf::from(":memory:"))
                .expect("in-memory PlanStore")
        }
    };
    let plan_registry = plans::PlanRegistry::new();
    info!("  Plans: ready");

    // Slash commands — short user-invocable shortcuts. 4features Stage 5.
    let slash_db_path = PathBuf::from(format!("{}/index.db", data_dir_str));
    let slash_store = match slash::SlashStore::open(slash_db_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("SlashStore open failed ({}); falling back to in-memory", e);
            slash::SlashStore::open(PathBuf::from(":memory:"))
                .expect("in-memory SlashStore")
        }
    };
    info!("  Slash commands: ready");

    // OAuth2 authorization_code caches — in-memory state + persistent
    // oauth_tokens on the same index.db (schema v8). v5 Item 4.
    let oauth_state = oauth::OAuthStateCache::new();
    let oauth_tokens_http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    let oauth_tokens_path = PathBuf::from(format!("{}/index.db", data_dir_str));

    // Master key for encrypting OAuth tokens at rest (F9)
    let master_key = crypto::load_or_create_key(Path::new(&data_dir_str))
        .unwrap_or_else(|e| {
            warn!("Failed to load/create master key ({}); generating ephemeral key", e);
            use aes_gcm::aead::KeyInit;
            aes_gcm::Aes256Gcm::generate_key(aes_gcm::aead::OsRng)
        });

    let oauth_tokens = oauth::AuthCodeTokenCache::open(oauth_tokens_path, oauth_tokens_http, master_key.clone())
        .unwrap_or_else(|e| {
            warn!("AuthCodeTokenCache open failed ({}); /connect disabled", e);
            oauth::AuthCodeTokenCache::open(
                PathBuf::from(":memory:"),
                reqwest::Client::new(),
                master_key,
            )
            .expect("in-memory AuthCodeTokenCache")
        });
    info!(
        "  OAuth: {} provider(s) configured",
        config.oauth.providers.len()
    );

    // LCM manager — wraps lcm.db for context-window summarization. Lives
    // alongside the conversation manager so we can write each conv message
    // through to LCM and benefit from its summarization later.
    let lcm: Option<Arc<lcm::LcmManager>> = {
        let lcm_path = format!("{}/lcm.db", data_dir_str);
        let cfg = config.lcm_config();
        Some(Arc::new(lcm::LcmManager::new(&lcm_path, cfg)))
    };
    info!("  LCM: ready");


    // Log module status
    crate::modules::log_module_status(&config.modules);

    // Initialize Voice Journal module config (if enabled)
    if let Some(entry) = config.modules.entries.get("mod-voice-journal") {
        if entry.enabled {
            let vj_config = crate::tools::voice_journal::VoiceJournalConfig::from_value(&entry.config);
            log::info!("[voice-journal] storage: {}, wake_word: {:?}, consent: {}",
                vj_config.storage_path, vj_config.wake_word, vj_config.consent_mode);
            crate::tools::voice_journal::init_config(vj_config);
        }
    }
    // Load OpenAPI tools from config.openapi.specs. Each spec generates one
    // Tool per allowlisted endpoint. Failures are logged and skipped.
    let openapi_http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default();
    let oauth_cache = crate::tools::openapi::OAuthTokenCache::new();
    let openapi_tools = crate::tools::openapi::load_from_config(
        &config.openapi.specs,
        &openapi_http,
        &oauth_cache,
        Some(Arc::clone(&oauth_tokens)),
    );
    info!("  OpenAPI: {} tool(s) loaded", openapi_tools.len());

    // Inject extension modules from ~/.syntaur/modules/ into the MCP server map.
    // This lets extension modules piggyback on the existing MCP lifecycle.
    let mut mcp_servers = config.mcp.servers.clone();
    let modules_dir = resolve_data_dir().join("modules");
    modules::inject_extension_modules(&mut mcp_servers, &modules_dir, &config.modules);

    // Spawn MCP servers from config + injected extension modules.
    // Failures are logged and skipped — startup never aborts because of MCP.
    let mcp_registry = mcp::McpRegistry::from_config(&mcp_servers).await;
    info!(
        "  MCP: {} server(s), {} tool(s)",
        mcp_registry.server_count(),
        mcp_registry.tools().len()
    );

    // ── Phase 0: Build the voice ToolRouter ──
    //
    // Embedding-based dispatcher used by the voice path's `find_tool` skill.
    // Initialized here so the BGE-small ONNX model is downloaded + loaded
    // once at startup, not on first request. We populate it with a small seed
    // of stateless built-in tools to prove the dispatch flow end-to-end;
    // future phases will register many more skills (timers, calendar, music,
    // weather, etc.) here.
    //
    // Failures are non-fatal: if fastembed init or seed loading fails, the
    // gateway still boots and `find_tool` returns "no entries" until the
    // operator fixes the underlying problem. The other voice tools
    // (control_light, set_thermostat, query_state, call_ha_service,
    // web_search) keep working regardless.
    // Initialize the timer store + background tick task BEFORE building the
    // router so TimerTool has its OnceLock in place when it's first executed.
    let _timer_store = crate::tools::timers::init_timer_store();
    crate::tools::timers::spawn_timer_tick(Arc::clone(&_timer_store));
    info!("[timers] store loaded, background tick spawned");

    let tool_router = match crate::tools::router::ToolRouter::new() {
        Ok(router) => {
            use crate::tools::built_in_tools::{
                EmailReadTool, EmailSendTool, ListFilesTool, ReadFileTool,
                SendTelegramTool, WebFetchTool, WebSearchTool,
            };
            use crate::tools::code_execute::CodeExecuteTool;
            use crate::tools::extension::Tool as ToolTrait;
            use crate::tools::router::{RouterEntry, ToolCategory};
            use crate::tools::announce::AnnounceTool;
            use crate::tools::calendar::CalendarTool;
            use crate::tools::household::{BotStatusTool, LedgerQueryTool, SystemHealthTool};
            use crate::tools::camera::CameraTool;
            use crate::tools::matter::MatterTool;
            use crate::tools::media_control::MediaControlTool;
            use crate::tools::music::MusicTool;
            use crate::tools::news::NewsTool;
            use crate::tools::notes::NotesTool;
            use crate::tools::scene::SceneTool;
            use crate::tools::wikipedia::WikipediaTool;
            use crate::tools::shopping_list::ShoppingListTool;
            use crate::tools::timers::TimerTool;
            use crate::tools::weather::WeatherTool;

            let seed_entries: Vec<RouterEntry> = vec![
                RouterEntry {
                    tool: Arc::new(WebSearchTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Info,
                    voice_description:
                        "Search the web for general questions, news, facts, and current events. \
                         Returns a list of titles, URLs, and snippets."
                            .to_string(),
                    example_intents: vec![
                        "what's in the news today".to_string(),
                        "search for the best ramen recipes".to_string(),
                        "google: who won the World Series in 2025".to_string(),
                        "look up the weather forecast for Sacramento".to_string(),
                        "what's the population of Tokyo".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(WebFetchTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Info,
                    voice_description:
                        "Fetch the contents of a specific URL and return the page text. Use \
                         when the user gives you a URL or asks you to read a specific webpage."
                            .to_string(),
                    example_intents: vec![
                        "read this article: https://example.com/foo".to_string(),
                        "what does the page at example.com say".to_string(),
                        "fetch and summarize that link I sent".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(CodeExecuteTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Dev,
                    voice_description:
                        "Run a small Python or bash snippet in a sandboxed environment for \
                         calculations, data parsing, conversions, or quick computations. Use \
                         when the user asks for math, conversions, or anything that needs \
                         actual code to compute."
                            .to_string(),
                    example_intents: vec![
                        "what is 17 percent of 240".to_string(),
                        "convert 50 fahrenheit to celsius".to_string(),
                        "calculate how much paint I need for a 12 by 14 room".to_string(),
                        "how many seconds in 4 hours".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(ReadFileTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Dev,
                    voice_description:
                        "Read a file from the workspace and return its contents. Use when the \
                         user asks to read a note, log, or document by filename."
                            .to_string(),
                    example_intents: vec![
                        "read my notes file".to_string(),
                        "what's in today's memory".to_string(),
                        "show me the contents of pending_tasks.md".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(ListFilesTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Dev,
                    voice_description:
                        "List files in a workspace directory. Use to discover available files \
                         when the user asks what's in a folder."
                            .to_string(),
                    example_intents: vec![
                        "what files do I have in my workspace".to_string(),
                        "list the files in my notes folder".to_string(),
                        "show me everything in the songs directory".to_string(),
                    ],
                },
                // ── Phase 1: existing tools as router entries ──
                RouterEntry {
                    tool: Arc::new(EmailReadTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Email,
                    voice_description:
                        "Read recent emails from the inbox. Shows sender, subject, and \
                         body preview for the most recent messages."
                            .to_string(),
                    example_intents: vec![
                        "read my email".to_string(),
                        "check my inbox".to_string(),
                        "any new emails".to_string(),
                        "read my latest email".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(EmailSendTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Email,
                    voice_description:
                        "Send an email. Requires a recipient, subject, and body."
                            .to_string(),
                    example_intents: vec![
                        "send an email to john".to_string(),
                        "email the team about the meeting".to_string(),
                        "draft an email to support".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(SendTelegramTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Personal,
                    voice_description:
                        "Send a Telegram message to Sean via the Claude bot."
                            .to_string(),
                    example_intents: vec![
                        "send a telegram message".to_string(),
                        "notify me via telegram".to_string(),
                        "send a message to my phone".to_string(),
                    ],
                },
                // ── Phase 2 tools ──
                RouterEntry {
                    tool: Arc::new(ShoppingListTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Personal,
                    voice_description:
                        "Manage shopping lists and todo lists. Add items, read the \
                         current list, remove items, or clear. Supports named lists \
                         like shopping, grocery, todo, or any custom name."
                            .to_string(),
                    example_intents: vec![
                        "add milk to the shopping list".to_string(),
                        "add eggs and bread to my list".to_string(),
                        "what's on my shopping list".to_string(),
                        "remove bananas from the list".to_string(),
                        "clear my grocery list".to_string(),
                        "add fix the fence to my todo list".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(AnnounceTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::SmartHome,
                    voice_description:
                        "Speak a message out loud through the satellite speaker. Use \
                         to announce things, broadcast messages, or say something aloud."
                            .to_string(),
                    example_intents: vec![
                        "announce dinner is ready".to_string(),
                        "say hello over the speaker".to_string(),
                        "tell everyone it's time to go".to_string(),
                        "broadcast a message".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(CalendarTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Calendar,
                    voice_description:
                        "Read upcoming events or add new events to Google Calendar or iCloud \
                         Calendar. Shows today's agenda, this week's events, or creates \
                         meetings and appointments."
                            .to_string(),
                    example_intents: vec![
                        "what's on my calendar today".to_string(),
                        "any meetings this week".to_string(),
                        "add a meeting tomorrow at 2pm".to_string(),
                        "schedule a dentist appointment for Friday at 10am".to_string(),
                        "what's my schedule look like".to_string(),
                        "put a reminder on my calendar".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(MusicTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Media,
                    voice_description:
                        "Control music playback. Play songs, albums, playlists from Apple Music \
                         or Plex. Pause, skip, adjust volume, search for music, or check what \
                         is currently playing."
                            .to_string(),
                    example_intents: vec![
                        "play some jazz".to_string(),
                        "play focus music".to_string(),
                        "search for Miles Davis".to_string(),
                        "what's playing on Plex".to_string(),
                        "skip this song".to_string(),
                        "set volume to 50".to_string(),
                        "play my workout playlist".to_string(),
                    ],
                },
                // ── Phase 3: household status tools ──
                RouterEntry {
                    tool: Arc::new(BotStatusTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Household,
                    voice_description:
                        "Check the status of the trading bots: stock bot, crypto bot, \
                         leveraged bot, options bot. Shows which are running, recent \
                         health checks, and any alerts."
                            .to_string(),
                    example_intents: vec![
                        "how are my bots doing".to_string(),
                        "are the trading bots running".to_string(),
                        "check the crypto bot".to_string(),
                        "bot status".to_string(),
                        "any trading alerts".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(SystemHealthTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Household,
                    voice_description:
                        "Check the health of the home infrastructure: LLM endpoints, \
                         syntaur service, bot monitor, system uptime, load average."
                            .to_string(),
                    example_intents: vec![
                        "system health check".to_string(),
                        "is everything running".to_string(),
                        "check the servers".to_string(),
                        "how's the infrastructure".to_string(),
                        "server status".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(LedgerQueryTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Household,
                    voice_description:
                        "Query the financial ledger for account balances, recent transactions, \
                         or expense summaries. Covers personal finances and Cherry Woodworks business."
                            .to_string(),
                    example_intents: vec![
                        "what are my account balances".to_string(),
                        "show recent transactions".to_string(),
                        "how much did I spend this month".to_string(),
                        "cherry woodworks expenses".to_string(),
                        "check my finances".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(NotesTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Personal,
                    voice_description:
                        "Save quick voice notes or reminders. Add a note to remember \
                         something later, read back all saved notes, or clear them."
                            .to_string(),
                    example_intents: vec![
                        "remember to call the plumber".to_string(),
                        "note that the garage door needs fixing".to_string(),
                        "what were my notes".to_string(),
                        "read my notes".to_string(),
                        "clear my notes".to_string(),
                        "remind me to buy flowers for Adriana".to_string(),
                    ],
                },
                // ── Phase 1: new pure-Rust tools ──
                RouterEntry {
                    tool: Arc::new(WeatherTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Info,
                    voice_description:
                        "Get current weather conditions and forecast for a location. \
                         Returns temperature, conditions, humidity, wind, and tomorrow's \
                         forecast. Accepts city names, zip codes, or defaults to Sacramento."
                            .to_string(),
                    example_intents: vec![
                        "what's the weather".to_string(),
                        "what's the temperature outside".to_string(),
                        "is it going to rain tomorrow".to_string(),
                        "weather in New York".to_string(),
                        "what's the forecast for this weekend".to_string(),
                        "how hot is it right now".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(TimerTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Timers,
                    voice_description:
                        "Start, list, or cancel countdown timers. When a timer expires, \
                         Peter announces it out loud via the satellite speaker. Supports \
                         multiple named timers running simultaneously."
                            .to_string(),
                    example_intents: vec![
                        "set a 5 minute timer".to_string(),
                        "set a timer for 30 seconds".to_string(),
                        "timer for 10 minutes called chicken".to_string(),
                        "how long on my timer".to_string(),
                        "list my timers".to_string(),
                        "cancel the chicken timer".to_string(),
                        "set an alarm for 2 hours".to_string(),
                    ],
                },
                // ── Phase 3b: quick-win tools ──
                RouterEntry {
                    tool: Arc::new(NewsTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Info,
                    voice_description: "Get current news headlines, optionally filtered by topic.".to_string(),
                    example_intents: vec![
                        "what's in the news".to_string(),
                        "any tech news today".to_string(),
                        "Sacramento news".to_string(),
                        "sports headlines".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(WikipediaTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Info,
                    voice_description: "Look up facts on Wikipedia. Great for 'what is X' questions about people, places, concepts.".to_string(),
                    example_intents: vec![
                        "what is a quokka".to_string(),
                        "tell me about the Roman Empire".to_string(),
                        "who was Nikola Tesla".to_string(),
                        "what is quantum computing".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(SceneTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::SmartHome,
                    voice_description: "Activate a Home Assistant scene or list available scenes. Scenes configure multiple devices at once.".to_string(),
                    example_intents: vec![
                        "activate movie night scene".to_string(),
                        "run the good morning scene".to_string(),
                        "what scenes do I have".to_string(),
                        "bedtime mode".to_string(),
                    ],
                },
                // ── Phase 4: Matter direct control ──
                RouterEntry {
                    tool: Arc::new(MatterTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::SmartHome,
                    voice_description: "Control Matter smart home devices directly. List all devices, turn on/off, set brightness, change color temperature. Bypasses Home Assistant for direct device control.".to_string(),
                    example_intents: vec![
                        "list all matter devices".to_string(),
                        "turn off matter node 11".to_string(),
                        "set matter device brightness to 50 percent".to_string(),
                        "toggle the smart bulb".to_string(),
                        "matter device status".to_string(),
                    ],
                },
                // ── Phase 5: direct protocol tools ──
                RouterEntry {
                    tool: Arc::new(CameraTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::SmartHome,
                    voice_description: "Check security cameras via Frigate NVR. See recent detections (people, cars, animals), camera status, or get snapshot URLs.".to_string(),
                    example_intents: vec![
                        "any activity on the cameras".to_string(),
                        "who's at the front door".to_string(),
                        "check the driveway camera".to_string(),
                        "camera status".to_string(),
                        "show me the backyard".to_string(),
                    ],
                },
                RouterEntry {
                    tool: Arc::new(MediaControlTool) as Arc<dyn ToolTrait>,
                    category: ToolCategory::Media,
                    voice_description: "Control media playback on Apple TV, Samsung TV, or satellite speaker. Play, pause, skip, volume, mute, status.".to_string(),
                    example_intents: vec![
                        "pause the TV".to_string(),
                        "turn up the volume".to_string(),
                        "what's playing on the Apple TV".to_string(),
                        "skip this song".to_string(),
                        "mute the TV".to_string(),
                        "set volume to 50 percent".to_string(),
                    ],
                },
            ];

            let n = seed_entries.len();
            let mut router_w = router.write().await;
            match router_w.add_batch(seed_entries).await {
                Ok(()) => {
                    info!("[router] seeded with {} initial skills", n);
                }
                Err(e) => {
                    warn!("[router] failed to seed initial skills: {}", e);
                }
            }
            drop(router_w);
            Some(router)
        }
        Err(e) => {
            warn!(
                "[router] init failed (find_tool will be degraded): {} \
                 — fastembed model download or ONNX runtime issue",
                e
            );
            None
        }
    };

    // Uploaded-files connector: reads `<data_dir>/uploads/knowledge/` and
    // feeds the index. Held separately in AppState so the upload + delete
    // handlers can write directly without going through the scheduler.
    let uploaded_files_connector: Option<Arc<connectors::sources::uploaded_files::UploadedFilesConnector>> = {
        let root = PathBuf::from(&data_dir_str).join("uploads").join("knowledge");
        let c = Arc::new(
            connectors::sources::uploaded_files::UploadedFilesConnector::new(root.clone()),
        );
        c.ensure_root();
        Some(c)
    };

    // Build shared state
    let state = Arc::new(AppState {
        config: config.clone(),
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default(),
        start_time: Instant::now(),
        stats: Mutex::new(GatewayStats {
            config_warnings: warnings.clone(),
            agents: config.agents.list.iter().map(|a| a.id.clone()).collect(),
            telegram_bots: config.telegram_accounts().len(),
            llm_providers: config.models.providers.keys().cloned().collect(),
            ..Default::default()
        }),
        mcp: Arc::clone(&mcp_registry),
        indexer: indexer.clone(),
        research_store: research_store.clone(),
        research_events: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
        message_events: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
        approval_store: approval_store.clone(),
        approval_registry: Arc::clone(&approval_registry),
        openapi_tools: openapi_tools.clone(),
        conversations: conversations.clone(),
        lcm: lcm.clone(),
        tool_rate_limiter: Arc::new(tokio::sync::Mutex::new(
            crate::rate_limit::RateLimiter::new(),
        )),
        tool_circuit_breakers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        db_path: PathBuf::from(format!("{}/index.db", data_dir_str)),
        config_path: config_path.clone(),
        users: Arc::clone(&users),
        oauth_state: Arc::clone(&oauth_state),
        oauth_tokens: Arc::clone(&oauth_tokens),
        tool_hooks: Arc::clone(&tool_hooks_store),
        skills: Arc::clone(&skills_store),
        plans: Arc::clone(&plans_store),
        plan_registry: Arc::clone(&plan_registry),
        slash: Arc::clone(&slash_store),
        ha_voice_secret: config
            .connectors
            .home_assistant
            .as_ref()
            .and_then(|h| h.voice_secret.clone())
            .filter(|s| !s.is_empty()),
        sharing_mode: Arc::new(tokio::sync::RwLock::new(
            users.get_sharing_mode().await.unwrap_or_else(|_| "shared".to_string()),
        )),
        disabled_tools: config.modules.disabled_tools(),
        tool_router,
        external_callbacks: Arc::new(Mutex::new(Vec::new())),
        connectors: Arc::new(std::sync::RwLock::new(HashMap::new())),
        uploaded_files: uploaded_files_connector.clone(),
        terminal: {
            let coders_module = modules::CORE_MODULES.iter().find(|m| m.id == "mod-coders");
            let is_enabled = coders_module.map(|m| config.modules.is_enabled(m)).unwrap_or(false);
            if is_enabled {
                let tc = config.modules.entries.get("mod-coders")
                    .and_then(|e| serde_json::from_value::<terminal::TerminalConfig>(e.config.clone()).ok())
                    .unwrap_or_default();
                Some(Arc::new(terminal::TerminalManager::new(
                    PathBuf::from(format!("{}/index.db", data_dir_str)),
                    master_key.clone(),
                    tc,
                )))
            } else {
                None
            }
        },
    });

    // Calendar reminder background task: checks for upcoming events every 60s.
    // Silently no-ops if no Telegram bot token is configured.
    if "calendar_reminder::spawn_reminder_task" != "disabled" {
        crate::calendar_reminder::spawn_reminder_task(Arc::clone(&state));
        info!("[calendar-reminder] spawned background reminder task");
    }

    // Sync auto-renewal: OAuth refresh (5min), API-key health check (daily)
    crate::sync::spawn_sync_renewal_task(Arc::clone(&state));
    info!("[sync-renewal] spawned background renewal task");

    // Initialize the global Home Assistant REST client used by the
    // voice chat tools (control_light, set_thermostat, query_state,
    // call_ha_service). Skipped silently when no connector is configured.
    if let Some(ha) = &state.config.connectors.home_assistant {
        if ha.enabled && !ha.base_url.is_empty() && !ha.bearer_token.is_empty() {
            let client = tools::home_assistant::HomeAssistantClient::new(
                ha.base_url.clone(),
                ha.bearer_token.clone(),
            );
            tools::home_assistant::init_home_assistant(client);
            info!("[ha] connector wired: base_url={}", ha.base_url);
        } else {
            info!("[ha] home_assistant connector present but disabled or incomplete");
        }
    }

    // HTTP server
    let port = config.gateway.port;
    let bind_addr = match config.gateway.bind.as_str() {
        "loopback" => format!("127.0.0.1:{}", port),
        _ => format!("0.0.0.0:{}", port),
    };

    // Use alternate port if configured via env (for parallel run with Node.js)
    let bind_addr = std::env::var("SYNTAUR_PORT")
        .ok()
        .map(|p| format!("127.0.0.1:{}", p))
        .unwrap_or(bind_addr);

    // Bind port FIRST — before spawning any tasks
    // This prevents orphaned Telegram pollers if port is in use
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => {
            info!("HTTP server bound to {}", bind_addr);
            l
        }
        Err(e) => {
            error!("Cannot bind to {}: {}. Is another instance running?", bind_addr, e);
            std::process::exit(1);
        }
    };

    // Shutdown signal — all tasks watch this
    let (shutdown_tx, _) = watch::channel(false);

    let app = Router::new()
        .route("/voice/tts/{filename}", get(voice::handle_tts_audio))
        .route("/health", get(handle_health))
        .route("/stats", get(handle_stats))
        .route("/messages", get(handle_messages))
        .route("/api/message", post(handle_api_message))
        .route("/api/research", post(handle_research))
        .route("/api/research/start", post(handle_research_start))
        .route("/api/research/recent", get(handle_research_recent))
        .route("/api/research/{id}", get(handle_research_get))
        .route("/api/research/{id}/stream", get(handle_research_stream))
        .route("/api/research/clarify", post(handle_research_clarify))
        .route("/api/knowledge/stats", get(handle_knowledge_stats))
        .route("/api/knowledge/search", get(handle_knowledge_search))
        .route("/api/knowledge/docs", get(handle_knowledge_docs))
        .route("/api/knowledge/docs/delete", post(handle_knowledge_doc_delete))
        .route("/api/knowledge/upload", post(handle_knowledge_upload))
        .route("/api/knowledge/resync/{source}", post(handle_knowledge_resync))
        .route("/api/message/start", post(handle_message_start))
        .route("/api/message/{id}/stream", get(handle_message_stream))
        .route("/api/conversations", post(handle_conv_create))
        .route("/api/conversations", get(handle_conv_list))
        .route("/api/conversations/{id}", get(handle_conv_get))
        // v5 Item 3 Stage 5: admin endpoints (users, tokens, telegram links)
        .route("/api/admin/users", post(handle_admin_create_user))
        .route("/api/admin/users", get(handle_admin_list_users))
        .route("/api/admin/users/{id}", axum::routing::put(handle_admin_update_user))
        .route("/api/admin/users/{id}", axum::routing::delete(handle_admin_delete_user))
        .route("/api/admin/users/{id}/tokens", post(handle_admin_mint_token))
        .route(
            "/api/admin/tokens/{token_id}",
            axum::routing::delete(handle_admin_revoke_token),
        )
        .route(
            "/api/admin/users/{id}/telegram-links",
            post(handle_admin_link_telegram),
        )
        // Multi-user: invites, registration, profile, sharing
        .route("/api/admin/invites", post(handle_admin_invite))
        .route("/api/admin/invites", get(handle_admin_list_invites))
        .route("/api/admin/sharing", get(handle_admin_get_sharing))
        .route("/api/admin/sharing", axum::routing::put(handle_admin_set_sharing))
        .route("/api/auth/register", post(handle_register))
        .route("/api/me", get(handle_me))
        .route("/api/me/password", axum::routing::put(handle_change_password))
        .route("/api/me/agents", get(handle_me_agents))
        .route("/api/me/agents", post(handle_create_user_agent))
        .route("/api/me/agents/{agent_id}", axum::routing::put(handle_update_user_agent))
        .route("/api/me/agents/{agent_id}", axum::routing::delete(handle_delete_user_agent))
        // Sharing grants
        .route("/api/admin/sharing/options", get(handle_admin_sharing_options))
        .route("/api/admin/sharing/grants", get(handle_admin_sharing_grants_list))
        .route("/api/admin/sharing/grants", axum::routing::put(handle_admin_set_sharing_grants))
        .route("/api/admin/sharing/grants/{id}", axum::routing::delete(handle_admin_delete_sharing_grant))
        // Personality docs
        .route("/api/me/personality", get(handle_personality_list))
        .route("/api/me/personality", post(handle_personality_create))
        .route("/api/me/personality/{id}", axum::routing::put(handle_personality_update))
        .route("/api/me/personality/{id}", axum::routing::delete(handle_personality_delete))
        // Onboarding
        .route("/api/me/onboarding", get(handle_onboarding_status))
        .route("/api/me/onboarding/complete", post(handle_onboarding_complete))
        .route("/api/me/data-location", axum::routing::put(handle_data_location_change))
        // v5 Item 4: OAuth2 authorization_code endpoints
        .route("/api/oauth/start", post(handle_oauth_start))
        .route("/api/oauth/callback", get(handle_oauth_callback))
        .route("/api/oauth/status", get(handle_oauth_status))
        .route("/api/oauth/disconnect", post(handle_oauth_disconnect))
        // 4-feature batch (schema v9): tool_hooks admin endpoints
        .route("/api/admin/hooks", post(handle_admin_create_hook))
        .route("/api/admin/hooks", get(handle_admin_list_hooks))
        .route(
            "/api/admin/hooks/{id}",
            axum::routing::delete(handle_admin_delete_hook),
        )
        // 4-feature batch (schema v9): skills admin + run endpoints
        .route("/api/admin/skills", post(handle_admin_create_skill))
        .route("/api/admin/skills", get(handle_admin_list_skills))
        .route(
            "/api/admin/skills/{id}",
            axum::routing::delete(handle_admin_delete_skill),
        )
        .route("/api/skills/run", post(handle_run_skill))
        // 4-feature batch (schema v9): plans (propose / approve / deny / list / get)
        .route("/api/plans", post(handle_propose_plan))
        .route("/api/plans", get(handle_list_plans))
        .route("/api/plans/{id}", get(handle_get_plan))
        .route("/api/plans/{id}/approve", post(handle_approve_plan))
        .route("/api/plans/{id}/deny", post(handle_deny_plan))
        // 4-feature batch (schema v9): slash commands
        .route("/api/admin/slash", post(handle_admin_create_slash))
        .route("/api/admin/slash", get(handle_admin_list_slash))
        .route(
            "/api/admin/slash/{id}",
            axum::routing::delete(handle_admin_delete_slash),
        )
        .route("/api/slash", post(handle_dispatch_slash))
        .route("/external-callbacks", get(handle_external_callbacks))
        .route("/v1/chat/completions", post(voice_chat::handle_chat_completions))
        // Voice Journal module routes
        .route("/ws/stt", get(voice_ws::ws_stt_handler))
        .route("/api/journal", get(voice_api::get_journal))
        .route("/api/journal/dates", get(voice_api::get_journal_dates))
        .route("/api/journal/search", get(voice_api::search_journal))
        .route("/api/journal/sessions", get(voice_api::get_sessions))
        .route("/api/tts", post(voice_api::synthesize_speech))
        // Terminal / Coders module routes
        .route("/coders", get(pages::coders::render))
        .route("/ws/terminal/{session_id}", get(terminal::ws::ws_terminal_handler))
        .route("/api/terminal/sessions", get(terminal::session::list_sessions))
        .route("/api/terminal/sessions", post(terminal::session::create_session))
        .route("/api/terminal/sessions/{id}", axum::routing::delete(terminal::session::kill_session))
        .route("/api/terminal/hosts", get(terminal::hosts::list_hosts))
        .route("/api/terminal/hosts", post(terminal::hosts::create_host))
        .route("/api/terminal/hosts/{id}", axum::routing::put(terminal::hosts::update_host))
        .route("/api/terminal/hosts/{id}", axum::routing::delete(terminal::hosts::delete_host))
        .route("/api/terminal/hosts/{id}/test", post(terminal::hosts::test_connection))
        .route("/api/terminal/sftp/{host_id}/ls", get(terminal::sftp::list_dir))
        .route("/api/terminal/sftp/{host_id}/read", get(terminal::sftp::read_file))
        .route("/api/terminal/sftp/{host_id}/upload", post(terminal::sftp::upload_file))
        .route("/api/terminal/sftp/{host_id}/mkdir", post(terminal::sftp::mkdir))
        .route("/api/terminal/sftp/{host_id}/rm", axum::routing::delete(terminal::sftp::rm))
        .route("/api/terminal/snippets", get(terminal::hosts::list_hosts)) // placeholder — snippet CRUD
        .route("/api/terminal/forwards", get(terminal::forwarding::list_forwards))
        .route("/api/terminal/forwards", post(terminal::forwarding::create_forward))
        .route("/api/terminal/forwards/{id}", axum::routing::delete(terminal::forwarding::delete_forward))
        // Setup wizard endpoints (installer + dashboard)
        .route("/", get(pages::dashboard::render))
        .route("/icon.svg", get(setup::handle_icon))
        .route("/favicon.ico", get(setup::handle_favicon))
        .route("/favicon-32.png", get(setup::handle_favicon_png))
        .route("/app-icon.jpg", get(setup::handle_app_icon))
        .route("/logo.jpg", get(setup::handle_logo))
        .route("/avatar.png", get(setup::handle_avatar))
        .route("/icon-192.png", get(setup::handle_icon_192))
        .route("/icon-512.png", get(setup::handle_icon_512))
        .route("/logo-mark.jpg", get(setup::handle_logo_mark))
        .route("/agent-avatar/{agent_id}", get(setup::handle_agent_avatar))
        .route("/api/agent-avatar/{agent_id}", post(setup::handle_agent_avatar_upload))
        .route("/manifest.json", get(setup::handle_manifest))
        .route("/tailwind.js", get(setup::handle_tailwind))
        .route("/coders/xterm.min.js", get(setup::handle_xterm_js))
        .route("/coders/xterm.css", get(setup::handle_xterm_css))
        .route("/coders/xterm-addon-fit.js", get(setup::handle_xterm_fit))
        .route("/coders/xterm-addon-search.js", get(setup::handle_xterm_search))
        .route("/coders/xterm-addon-web-links.js", get(setup::handle_xterm_weblinks))
        .route("/fonts.css", get(setup::handle_fonts_css))
        .route("/fonts/{filename}", get(setup::handle_font_file))
        .route("/setup", get(pages::setup::render))
        .route("/modules", get(pages::modules::render))
        .route("/journal", get(pages::journal::render))
        .route("/music", get(pages::music::render))
        .route("/voice-setup", get(pages::voice_setup::render))
        .route("/settings", get(pages::settings::render))
        .route("/tax", get(pages::tax::render))
        .route("/chat", get(pages::chat::render))
        .route("/history", get(pages::history::render))
        .route("/knowledge", get(pages::knowledge::render))
        .route("/research", get(pages::research::render))
        .route("/landing", get(pages::landing::render))
        .route("/register", get(pages::register::render))
        .route("/onboarding", get(pages::onboarding::render))
        .route("/profile", get(pages::profile::render))
        .route("/api/auth/login", post(setup::handle_login))
        .route("/api/setup/status", get(setup::handle_setup_status))
        .route("/api/setup/scan", get(setup::handle_hardware_scan))
        .route("/api/setup/fix-firewall", post(setup::handle_fix_firewall))
        .route("/api/setup/check-tailscale", get(setup::handle_check_tailscale))
        .route("/api/setup/ssh-pubkey", get(setup::handle_ssh_pubkey))
        .route("/api/setup/test-gpu", post(setup::handle_test_gpu))
        .route("/api/upload", post(setup::handle_file_upload))
        .route("/api/setup/test-llm", post(setup::handle_test_llm))
        .route("/api/setup/test-telegram", post(setup::handle_test_telegram))
        .route("/api/setup/test-ha", post(setup::handle_test_ha))
        .route("/api/setup/modules", get(setup::handle_setup_modules))
        .route("/api/modules/toggle", post(setup::handle_module_toggle))
        .route("/api/license/status", get(setup::handle_license_status))
        .route("/api/license/activate", post(setup::handle_license_activate))
        .route("/api/setup/apply", post(setup::handle_setup_apply))
        .route("/api/settings/install-shortcut", post(setup::handle_install_shortcut))
        .route("/api/settings/provider", post(setup::handle_save_provider))
        .route("/api/open-url", get(handle_open_url))
        .route("/api/bug-reports", post(handle_bug_report_submit))
        .route("/api/bug-reports", get(handle_bug_report_list))
        .route("/api/tax/receipts", post(tax::handle_receipt_upload))
        .route("/api/tax/receipts", get(tax::handle_receipt_list))
        .route("/api/tax/receipts/{id}/image", get(tax::handle_receipt_image))
        .route("/api/tax/expenses", post(tax::handle_expense_create))
        .route("/api/tax/expenses", get(tax::handle_expense_list))
        .route("/api/tax/summary", get(tax::handle_expense_summary))
        .route("/api/tax/categories", get(tax::handle_category_list))
        .route("/api/tax/categories", post(tax::handle_category_create))
        .route("/api/tax/categories/{id}", axum::routing::delete(tax::handle_category_delete))
        .route("/api/tax/documents", post(tax::handle_tax_doc_upload))
        .route("/api/tax/documents", get(tax::handle_tax_doc_list))
        .route("/api/tax/documents/{id}/image", get(tax::handle_tax_doc_image))
        .route("/api/tax/documents/{id}/field", axum::routing::put(tax::handle_tax_doc_update_field))
        .route("/api/tax/documents/{id}/status", axum::routing::put(tax::handle_tax_doc_update_status))
        .route("/api/tax/income", get(tax::handle_income_list))
        .route("/api/tax/brackets/status", get(tax::handle_bracket_status))
        .route("/api/updates/check", get(tax::handle_update_check))
        .route("/api/tax/export", get(tax::handle_expense_export))
        .route("/api/tax/export/txf", get(tax::handle_txf_export))
        .route("/api/tax/export/csv-irs", get(tax::handle_csv_irs_export))
        .route("/api/tax/extension", get(tax::handle_extension))
        .route("/api/tax/extension/status", get(tax::handle_extension_status))
        .route("/api/tax/extension/create", post(tax::handle_extension_create))
        .route("/api/tax/extension/{id}/file", axum::routing::put(tax::handle_extension_file))
        .route("/api/tax/extension/{id}/confirm", axum::routing::put(tax::handle_extension_confirm))
        // Taxpayer profile + dependents
        .route("/api/tax/profile", get(tax::handle_taxpayer_profile_get))
        .route("/api/tax/profile", post(tax::handle_taxpayer_profile_save))
        .route("/api/tax/dependents", post(tax::handle_dependent_save))
        .route("/api/tax/dependents/{id}", axum::routing::delete(tax::handle_dependent_delete))
        // Items 10-16: smart routing, statements, property, deduction, insurance, wizard, brackets
        .route("/api/tax/upload", post(tax::handle_smart_upload))
        .route("/api/tax/statements/transactions", get(tax::handle_statement_transactions))
        .route("/api/tax/property", get(tax::handle_property_profile_get))
        .route("/api/tax/property", post(tax::handle_property_profile_save))
        .route("/api/tax/deduction/autofill", get(tax::handle_deduction_autofill))
        .route("/api/tax/insurance/classify", post(tax::handle_insurance_classify))
        .route("/api/tax/wizard", get(tax::handle_tax_prep_wizard))
        .route("/api/tax/brackets/fetch", get(tax::handle_brackets_auto_fetch))
        // Deduction questionnaire + auto-scanner + review queue
        .route("/api/tax/questionnaire", get(tax::handle_questionnaire_get))
        .route("/api/tax/questionnaire", post(tax::handle_questionnaire_save))
        .route("/api/tax/deductions/scan", post(tax::handle_deduction_scan))
        .route("/api/tax/deductions/deep-scan", post(tax::handle_deduction_deep_scan))
        .route("/api/tax/deductions/candidates", get(tax::handle_deduction_candidates_list))
        .route("/api/tax/deductions/candidates/{id}/context", get(tax::handle_deduction_candidate_context))
        .route("/api/tax/deductions/candidates/{id}/review", axum::routing::put(tax::handle_deduction_review))
        .route("/api/tax/deductions/bulk-review", post(tax::handle_deduction_bulk_review))
        .route("/api/tax/deductions/summary", get(tax::handle_deduction_summary))
        // Tax credits + estimated payments + projections (Phase 1)
        .route("/api/tax/credits", get(tax::handle_credits_list))
        .route("/api/tax/credits/eligibility", get(tax::handle_credits_eligibility))
        .route("/api/tax/credits/education", post(tax::handle_education_expense_create))
        .route("/api/tax/credits/childcare", post(tax::handle_childcare_expense_create))
        .route("/api/tax/credits/energy", post(tax::handle_energy_improvement_create))
        .route("/api/tax/estimated-payments", get(tax::handle_estimated_payments_list))
        .route("/api/tax/estimated-payments", post(tax::handle_estimated_payment_create))
        .route("/api/tax/estimated-payments/recommended", get(tax::handle_estimated_recommended))
        .route("/api/tax/projection", get(tax::handle_tax_projection))
        // Depreciation / assets (Phase 2A)
        .route("/api/tax/assets", get(tax::handle_asset_list))
        .route("/api/tax/assets", post(tax::handle_asset_create))
        .route("/api/tax/assets/{id}/schedule", get(tax::handle_depreciation_schedule))
        .route("/api/tax/vehicle-usage", post(tax::handle_vehicle_usage_create))
        // Investment tax engine (Phase 2B)
        .route("/api/tax/lots", get(tax::handle_lots_list))
        .route("/api/tax/lots", post(tax::handle_lot_create))
        .route("/api/tax/lots/sell", post(tax::handle_lot_sell))
        .route("/api/tax/wash-sales", get(tax::handle_wash_sales))
        .route("/api/tax/form-8949", get(tax::handle_form_8949))
        .route("/api/tax/k1", get(tax::handle_k1_list))
        .route("/api/tax/k1", post(tax::handle_k1_create))
        .route("/api/tax/capital-gains/summary", get(tax::handle_capital_gains_summary))
        // AI Tax Advisor (Phase 4)
        .route("/api/tax/context", get(tax::handle_tax_context))
        .route("/api/tax/audit-risk", get(tax::handle_audit_risk))
        .route("/api/tax/insights", get(tax::handle_tax_insights))
        .route("/api/tax/what-if", post(tax::handle_what_if))
        // State tax engine (Phase 3)
        .route("/api/tax/state/estimate", get(tax::handle_state_tax_estimate))
        .route("/api/tax/state/profile", get(tax::handle_state_profile_list))
        .route("/api/tax/state/profile", post(tax::handle_state_profile_save))
        .route("/api/tax/state/supported", get(tax::handle_supported_states))
        // Business entities (Phase 5)
        .route("/api/tax/entities", get(tax::handle_entity_list))
        .route("/api/tax/entities", post(tax::handle_entity_create))
        .route("/api/tax/entities/{id}/summary", get(tax::handle_entity_summary))
        .route("/api/tax/entities/{id}/income", post(tax::handle_entity_income_create))
        .route("/api/tax/entities/{id}/expenses", post(tax::handle_entity_expense_create))
        .route("/api/tax/entities/{id}/shareholders", post(tax::handle_shareholder_save))
        .route("/api/tax/entities/{id}/k1", get(tax::handle_entity_k1_generate))
        .route("/api/tax/entities/{id}/1099", get(tax::handle_1099_list))
        .route("/api/tax/entities/{id}/1099", post(tax::handle_1099_issue))
        .route("/api/tax/entity-comparison", get(tax::handle_entity_comparison))
        // Paper filing (printable return)
        .route("/api/tax/print", get(tax::handle_print_return))
        // Module licensing
        .route("/api/modules/status", get(tax::handle_module_status))
        .route("/api/modules/trial", post(tax::handle_start_trial))
        .route("/api/modules/activate", post(tax::handle_activate_license))
        // Financial integrations (Plaid, SimpleFIN, Alpaca, Coinbase, Stripe, Gmail)
        .route("/api/financial/connections", get(financial::handle_connections_list))
        .route("/api/financial/connections/{id}", axum::routing::delete(financial::handle_connection_delete))
        .route("/api/financial/plaid/link-token", post(financial::handle_plaid_link_token))
        .route("/api/financial/plaid/exchange", post(financial::handle_plaid_exchange))
        .route("/api/financial/plaid/sync", post(financial::handle_plaid_sync))
        .route("/api/financial/plaid/webhook", post(financial::handle_plaid_webhook))
        .route("/api/financial/simplefin/connect", post(financial::handle_simplefin_connect))
        .route("/api/financial/simplefin/sync", post(financial::handle_simplefin_sync))
        .route("/api/financial/stripe/checkout", post(financial::handle_stripe_checkout))
        .route("/api/financial/stripe/webhook", post(financial::handle_stripe_webhook))
        .route("/api/financial/investments/summary", get(financial::handle_investment_summary))
        .route("/api/financial/investments/transactions", get(financial::handle_investment_transactions))
        .route("/api/financial/alpaca/connect", post(financial::handle_alpaca_connect))
        .route("/api/financial/alpaca/sync", post(financial::handle_alpaca_sync))
        .route("/api/financial/coinbase/connect", post(financial::handle_coinbase_connect))
        .route("/api/financial/coinbase/sync", post(financial::handle_coinbase_sync))
        .route("/api/financial/gmail/connect", post(financial::handle_gmail_connect))
        .route("/api/financial/gmail/scan", post(financial::handle_gmail_scan))
        .route("/api/todos", get(handle_todo_list))
        .route("/api/todos", post(handle_todo_create))
        .route("/api/todos/{id}", axum::routing::put(handle_todo_update))
        .route("/api/todos/{id}", axum::routing::delete(handle_todo_delete))
        .route("/api/calendar", get(handle_calendar_list))
        .route("/api/calendar", post(handle_calendar_create))
        .route("/api/calendar/{id}", axum::routing::delete(handle_calendar_delete))
        .route("/api/calendar/{id}", axum::routing::put(handle_calendar_update))
        .route("/api/calendar/import", post(handle_calendar_ics_import))
        .route("/api/sync/providers", get(sync::handle_sync_providers))
        .route("/api/sync/connect", post(sync::handle_sync_connect))
        .route("/api/sync/test", post(sync::handle_sync_test))
        .route("/api/sync/connections/{provider}", axum::routing::delete(sync::handle_sync_disconnect))
        .route("/api/sync/telegram/pair", post(sync::handle_telegram_pair_create))
        .route("/api/sync/telegram/status", get(sync::handle_telegram_pair_status))
        .route("/api/sync/health/upload", post(sync::handle_health_upload))
        .route("/api/sync/notebooklm/status", get(sync::handle_notebooklm_status))
        .route("/api/sync/vault/status", get(sync::handle_vault_status))
        .route("/api/sync/homeassistant/discover", get(sync::handle_ha_discover))
        .route("/api/sync/plex/pin", post(sync::handle_plex_pin_create))
        .route("/api/sync/plex/poll", post(sync::handle_plex_pin_poll))
        .route("/api/sync/airplay/discover", get(sync::handle_airplay_discover))
        .route("/api/sync/music_assistant/probe", get(sync::handle_music_assistant_probe))
        .route("/api/sync/apple_music/dev_token", get(sync::handle_apple_music_dev_token))
        .route("/api/sync/apple_music/save", post(sync::handle_apple_music_save))
        .route("/api/sync/apple_music/playlists", get(sync::handle_apple_music_playlists))
        .route("/api/sync/apple_music/search", get(sync::handle_apple_music_search))
        .route("/api/sync/apple_music/bookmarklet", get(sync::handle_apple_music_bookmarklet))
        .route("/api/sync/home_assistant/media_players", get(sync::handle_ha_media_players))
        .route("/api/music/now_playing", get(music::handle_music_now_playing))
        .route("/api/music/control", post(music::handle_music_control))
        .route("/api/music/speakers", get(music::handle_music_speakers))
        .route("/api/music/group", post(music::handle_music_group))
        .route("/api/music/eq", post(music::handle_music_eq))
        .route("/api/music/dj", post(music::handle_music_dj))
        .route("/api/music/pwa_state", post(music::handle_pwa_state))
        .route("/api/music/set_preferred_target", post(music::handle_set_preferred_target))
        .route("/api/admin/oauth_config", post(sync::handle_oauth_config_save))
        .route("/api/admin/oauth_config", get(sync::handle_oauth_config_list))
        .route("/api/admin/oauth_config/{identity_provider}", axum::routing::delete(sync::handle_oauth_config_delete))
        .route("/api/music/spotify_play", post(music::handle_spotify_play))
        .route("/api/music/spotify_token", get(music::handle_spotify_token))
        .route("/api/music/prefs", get(music::handle_music_prefs_list))
        .route("/api/music/prefs", post(music::handle_music_pref_save))
        .route("/api/music/prefs/{id}", axum::routing::delete(music::handle_music_pref_delete))
        .route("/api/music/duck", post(music::handle_music_duck))
        .route("/api/music/duck_state", get(music::handle_music_duck_state))
        .route("/api/music/duck/v", get(music::handle_duck_volume_simple))
        .route("/api/music/shortcut_setup", get(music::handle_shortcut_setup_guide))
        .route("/api/music/local_events", get(music::handle_local_events))
        .route("/apple_music_capture", get(sync::handle_apple_music_capture_page))
        .with_state(Arc::clone(&state))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            setup::first_run_redirect,
        ))
        .layer({
            use tower_http::cors::{CorsLayer, AllowOrigin};
            use axum::http::{header, HeaderValue, Method};
            CorsLayer::new()
                .allow_origin(AllowOrigin::list([
                    "http://localhost:18789".parse::<HeaderValue>().unwrap(),
                    "http://127.0.0.1:18789".parse::<HeaderValue>().unwrap(),
                    "http://localhost:18790".parse::<HeaderValue>().unwrap(),
                    "http://127.0.0.1:18790".parse::<HeaderValue>().unwrap(),
                ]))
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::ACCEPT,
                ])
                .allow_credentials(true)
        })
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)); // 16 MB

    // Security warnings before server start
    if config.gateway.bind != "loopback" {
        warn!("Gateway is bound to 0.0.0.0 — accessible from the network without TLS. \
               Consider setting gateway.bind = \"loopback\" or deploying a TLS reverse proxy.");
    }

    info!("HTTP server on {}", bind_addr);
    info!("Dashboard: http://127.0.0.1:{}", config.gateway.port);
    info!("Open 'Syntaur' from your app launcher, or visit the URL above.");

    // Install default tax brackets config if not present
    {
        let brackets_path = format!("{}/tax_brackets.json", data_dir_str);
        if !std::path::Path::new(&brackets_path).exists() {
            let default = include_str!("../static/tax_brackets.json");
            if let Err(e) = std::fs::write(&brackets_path, default) {
                warn!("Could not write default tax brackets: {}", e);
            } else {
                info!("Installed default tax brackets config");
            }
        }
        if let Some(warning) = tax::brackets_stale() {
            warn!("Tax brackets: {}", warning);
        }
    }

    // Auto-open browser on first start (only if interactive terminal)
    if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() || cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        let url = format!("http://127.0.0.1:{}", config.gateway.port);
        tokio::spawn(async move {
            // Small delay to let the server bind
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = open_browser(&url);
        });
    }

    // Start cron scheduler
    let cron_path = config_path.parent()
        .unwrap_or(Path::new("."))
        .join("cron/jobs.json");

    if cron_path.exists() {
        let mut scheduler = cron::CronScheduler::new(cron_path.clone());
        // Provide config for agent-turn cron jobs (LLM + tools)
        scheduler.set_config(Arc::new(config.clone()));
        scheduler.set_mcp(Arc::clone(&mcp_registry));
        scheduler.set_tool_infra(
            Arc::clone(&state.tool_rate_limiter),
            Arc::clone(&state.tool_circuit_breakers),
        );
        // Provide Telegram tokens for cron delivery
        let mut tg_tokens = HashMap::new();
        for (id, acc) in config.telegram_accounts() {
            tg_tokens.insert(id, acc.bot_token.clone());
        }
        scheduler.set_telegram_tokens(tg_tokens);
        let initial_jobs = scheduler.load_jobs();
        let enabled = initial_jobs.iter().filter(|j| j.enabled).count();
        info!("Cron: {} jobs loaded ({} enabled)", initial_jobs.len(), enabled);

        {
            let mut stats = state.stats.lock().await;
            stats.cron_jobs = enabled;
        }

        let state_cron = Arc::clone(&state);
        let mut shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            info!("Cron scheduler started (checking every 30s)");
            loop {
                scheduler.tick().await;
                {
                    let mut stats = state_cron.stats.lock().await;
                    stats.cron_runs += 1;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    _ = shutdown_rx.changed() => {
                        info!("Cron scheduler shutting down");
                        break;
                    }
                }
            }
        });
    } else {
        info!("Cron: no jobs.json found at {}", cron_path.display());
    }

    // Start Telegram bots
    let accounts = config.telegram_accounts();
    let conversations = Arc::new(Mutex::new(telegram::ConversationStore::new(20)));

    for (account_id, account) in &accounts {
        // Find which agent this account is bound to
        let agent_id = config.bindings.iter()
            .find(|b| b.match_rule.as_ref()
                .and_then(|m| m.get("accountId"))
                .and_then(|v| v.as_str())
                .map_or(false, |id| id == account_id))
            .map(|b| b.agent_id.clone())
            .unwrap_or_else(|| "main".to_string());

        let allow_from: Vec<i64> = account.allow_from.iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        // Build LLM chain for this agent
        let llm_chain = Arc::new(llm::LlmChain::from_config(&config, &agent_id, state.client.clone()));

        // Load agent workspace context for system prompt
        let workspace = config.agent_workspace(&agent_id);
        let mut context_parts = Vec::new();

        // Auto-inject workspace files (same as Syntaur)
        for file in &["SOUL.md", "IDENTITY.md", "TOOLS.md", "USER.md", "BRIEF.md"] {
            if let Ok(content) = std::fs::read_to_string(workspace.join(file)) {
                if !content.trim().is_empty() {
                    context_parts.push(content);
                }
            }
        }

        // Load today's memory if it exists
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        if let Ok(memory) = std::fs::read_to_string(workspace.join("memory").join(format!("{}.md", today))) {
            context_parts.push(format!("[Today's memory]\n{}", memory));
        }

        // Load PLAN.md if exists (in-progress work)
        if let Ok(plan) = std::fs::read_to_string(workspace.join("PLAN.md")) {
            if !plan.trim().is_empty() {
                context_parts.push(format!("[Current plan — resume if incomplete]\n{}", plan));
            }
        }

        // Load PENDING_TASKS.md
        if let Ok(tasks) = std::fs::read_to_string(workspace.join("PENDING_TASKS.md")) {
            if !tasks.trim().is_empty() {
                context_parts.push(format!("[Pending tasks]\n{}", tasks));
            }
        }

        // Load MEMORY.md
        if let Ok(memory) = std::fs::read_to_string(workspace.join("MEMORY.md")) {
            if !memory.trim().is_empty() {
                context_parts.push(format!("[Persistent memory]\n{}", memory));
            }
        }

        let system_prompt = if context_parts.is_empty() {
            format!("You are {}, an AI assistant.", account.name)
        } else {
            context_parts.join("\n\n---\n\n")
        };

        info!("  Agent {}: loaded {} context files from {}", agent_id, context_parts.len(), workspace.display());

        let bot = telegram::TelegramBot {
            account_id: account_id.clone(),
            agent_id: agent_id.clone(),
            token: account.bot_token.clone(),
            name: account.name.clone(),
            allow_from,
        };

        let client = state.client.clone();
        let convos = Arc::clone(&conversations);
        let shutdown_rx = shutdown_tx.subscribe();
        let mcp_for_bot = Arc::clone(&state.mcp);
        let approval_for_bot = Arc::clone(&state.approval_registry);
        let approval_store_for_bot = state.approval_store.clone();
        let rate_limiter_for_bot = Arc::clone(&state.tool_rate_limiter);
        let breakers_for_bot = Arc::clone(&state.tool_circuit_breakers);
        let users_for_bot = Arc::clone(&state.users);
        let plan_registry_for_bot = Arc::clone(&state.plan_registry);
        let plan_store_for_bot = Arc::clone(&state.plans);
        let app_state_for_bot = Arc::clone(&state);

        tokio::spawn(telegram::run_bot(
            bot,
            client,
            llm_chain,
            convos,
            system_prompt,
            shutdown_rx,
            mcp_for_bot,
            approval_for_bot,
            approval_store_for_bot,
            rate_limiter_for_bot,
            breakers_for_bot,
            users_for_bot,
            plan_registry_for_bot,
            plan_store_for_bot,
            app_state_for_bot,
        ));
    }

    // Spawn the connector scheduler with workspace_files indexing all configured
    // agent workspaces. Initial load runs synchronously so the index is warm
    // before the first agent turn. Refresh every 5 minutes, prune every hour.
    if let Some(idx) = &indexer {
        let mut sched = connectors::ConnectorScheduler::new(Arc::clone(idx));

        // Registers a connector with the scheduler AND with state.connectors
        // so the /knowledge page can trigger a manual re-sync.
        let register = |sched: &mut connectors::ConnectorScheduler,
                        conn: Arc<dyn connectors::FullConnector>,
                        refresh_secs: u64,
                        prune_secs: u64| {
            if let Ok(mut map) = state.connectors.write() {
                map.insert(conn.name().to_string(), Arc::clone(&conn));
            }
            sched.add(connectors::ConnectorEntry {
                connector: conn,
                refresh_secs,
                prune_secs,
            });
        };

        let workspaces: Vec<(String, PathBuf)> = config
            .agents
            .list
            .iter()
            .map(|a| (a.id.clone(), config.agent_workspace(&a.id)))
            .collect();
        info!("[connector] indexing {} workspace(s)", workspaces.len());
        register(
            &mut sched,
            std::sync::Arc::new(
                connectors::sources::workspace_files::WorkspaceFilesConnector::new(workspaces),
            ),
            300,
            3600,
        );

        // Uploaded files — user-uploaded documents from /knowledge. Refresh
        // often (60s) so deletions and out-of-band drops show up quickly.
        if let Some(uf) = &state.uploaded_files {
            register(
                &mut sched,
                Arc::clone(uf) as Arc<dyn connectors::FullConnector>,
                60,
                3600,
            );
        }

        // execution_log connector — auto-detect from ~/bots/data
        let bots_base = config
            .connectors
            .execution_log_base
            .clone()
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
                format!("{}/bots/data", home)
            });
        register(
            &mut sched,
            std::sync::Arc::new(
                connectors::sources::execution_log::ExecutionLogConnector::auto_detect(
                    std::path::PathBuf::from(&bots_base),
                ),
            ),
            600,
            86400,
        );

        if let Some(p) = &config.connectors.paperless {
            if p.enabled && !p.base_url.is_empty() && !p.token.is_empty() {
                let http = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .unwrap_or_default();
                register(
                    &mut sched,
                    std::sync::Arc::new(
                        connectors::sources::paperless::PaperlessConnector::new(
                            p.base_url.clone(),
                            p.token.clone(),
                            http,
                        ),
                    ),
                    1800,
                    86400,
                );
            }
        }

        if let Some(b) = &config.connectors.bluesky {
            if b.enabled && !b.actor.is_empty() {
                let http = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap_or_default();
                register(
                    &mut sched,
                    std::sync::Arc::new(
                        connectors::sources::bluesky::BlueskyConnector::new(b.actor.clone(), http),
                    ),
                    900,
                    86400,
                );
            }
        }

        if let Some(g) = &config.connectors.github {
            if g.enabled && !g.user.is_empty() && !g.token.is_empty() {
                let http = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .unwrap_or_default();
                register(
                    &mut sched,
                    std::sync::Arc::new(
                        connectors::sources::github::GithubConnector::new(
                            g.user.clone(),
                            g.token.clone(),
                            http,
                        ),
                    ),
                    1800,
                    86400,
                );
            }
        }

        for ec in &config.connectors.email {
            if !ec.enabled || ec.host.is_empty() || ec.username.is_empty() {
                continue;
            }
            register(
                &mut sched,
                std::sync::Arc::new(
                    connectors::sources::email::EmailConnector::new(
                        ec.account_id.clone(),
                        ec.host.clone(),
                        ec.port,
                        ec.username.clone(),
                        ec.password.clone(),
                    ),
                ),
                1800,
                86400,
            );
        }

        let scheduler_shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            sched.warm_up_then_spawn(scheduler_shutdown_rx).await;
        });
    }

    // Satellite voice client — connects directly to ESPHome satellite,
    // replacing HA for the voice pipeline. Enabled when satellite_host is set.
    if let Some(ha_cfg) = &config.connectors.home_assistant {
        if let Some(sat_host) = &ha_cfg.satellite_host {
            if !sat_host.is_empty() {
                let sat_config = voice::satellite_client::SatelliteConfig {
                    host: sat_host.clone(),
                    noise_psk: ha_cfg.noise_psk.clone().unwrap_or_default(),
                    stt_host: "127.0.0.1:10300".to_string(),
                    gateway_url: "http://127.0.0.1:18789".to_string(),
                    gateway_secret: ha_cfg.voice_secret.clone().unwrap_or_default(),
                    tts_host: ha_cfg.tts_host.clone().unwrap_or_else(|| "192.168.1.69:10400".to_string()),
                };
                info!(
                    "Satellite voice client: {} (STT: {}, TTS: {})",
                    sat_config.host, sat_config.stt_host, sat_config.tts_host
                );
                tokio::spawn(voice::satellite_client::run_satellite_client(sat_config));
            }
        }
    }

    // Graceful shutdown on SIGTERM/SIGINT
    let shutdown = async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM");
            tokio::select! {
                _ = ctrl_c => info!("SIGINT received"),
                _ = sigterm.recv() => info!("SIGTERM received"),
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.expect("failed to listen for Ctrl+C");
            info!("SIGINT received");
        }

        info!("Shutting down — stopping all tasks");
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_secs(2)).await;
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap_or_else(|e| error!("HTTP server error: {}", e));

    info!("Shutdown complete");
}


/// Open the default browser to a URL. Cross-platform.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/c", "start", url]).spawn()?;
    }
    Ok(())
}
