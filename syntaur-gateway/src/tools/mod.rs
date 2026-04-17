pub mod account;
pub mod browser;
pub mod agent_memory;
pub mod image_gen;
pub mod built_in_tools;
pub mod captcha;
pub mod captcha_bridge;
pub mod email;
pub mod sms;
pub mod file_ops;
pub mod shell;
pub mod memory;
pub mod web;
pub mod extension;
pub mod internal_search;
pub mod code_execute;
pub mod openapi;
pub mod home_assistant;
// Phase 0 voice skill router: embedding-based intent → tool dispatch.
pub mod router;
pub mod find_tool;
// Phase 1 pure-Rust skills: direct-API, no HA involvement.
pub mod weather;
pub mod timers;
// Phase 2 skills.
pub mod shopping_list;
pub mod announce;
pub mod calendar;
pub mod music;
// Phase 3: household status tools (read-only).
pub mod household;
pub mod notes;
// Phase 3b: quick-win tools.
pub mod news;
pub mod wikipedia;
pub mod scene;
pub mod media_control;
// Phase 4: Matter device control via python-matter-server WebSocket.
pub mod matter;
// Phase 5: direct protocol tools.
pub mod camera;
// Sub-agent delegation: search, coder, researcher specialists.
pub mod subagent;
// Phase 6: Voice journal — record, transcribe, search.
pub mod voice_journal;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::approval;
use crate::circuit_breaker::{CircuitBreaker, CircuitState};
use crate::index::Indexer;
use crate::mcp::McpRegistry;
use crate::rate_limit::RateLimiter;
use code_execute::CodeExecuteTool;
use extension::{Tool, ToolCapabilities, ToolContext};
use internal_search::InternalSearchTool;

// ── Tool Call Types ─────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Serialize, Clone, Debug)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub success: bool,
    pub output: String,
}

// ── Tool Registry ───────────────────────────────────────────────────────────

pub struct ToolRegistry {
    workspace_root: PathBuf,
    allowed_scripts: Vec<String>,
    mcp: Option<Arc<McpRegistry>>,
    indexer: Option<Arc<Indexer>>,
    agent_id: String,
    extensions: HashMap<String, Arc<dyn Tool>>,
    approval: Option<ApprovalContext>,
    /// Tools requiring approval beyond the hardcoded REQUIRES_APPROVAL list.
    /// Computed per-agent at registry construction time from config.
    custom_requires_approval: Vec<String>,
    /// Tools to skip approval for (overrides hardcoded list per agent).
    custom_never_approval: Vec<String>,
    /// Per-tool token-bucket rate limiter shared across registry rebuilds
    /// (lives on AppState). v5 Item 1 Stage 4.
    rate_limiter: Option<Arc<Mutex<RateLimiter>>>,
    /// Per-circuit-name breakers shared across registry rebuilds. Tools with
    /// matching `capabilities().circuit_name` share one breaker so a single
    /// failure cluster opens the whole group. v5 Item 1 Stage 4.
    circuit_breakers: Option<Arc<Mutex<HashMap<String, CircuitBreaker>>>>,
    /// Owning user_id for the principal that built this registry. 0 = legacy
    /// admin. Propagates into ToolContext so per-user tools (OAuth2
    /// authorization_code) can look up the right token. v5 Item 3 → Item 4.
    user_id: i64,
    /// User-configurable PreToolUse / PostToolUse hooks. None disables
    /// the hook firing path entirely (for tests / fresh registries that
    /// haven't been wired up yet). 4features Stage 2.
    tool_hooks: Option<Arc<crate::tool_hooks::HookStore>>,
    /// Shared HTTP client for tools that make outbound requests (weather,
    /// web_fetch, etc). When None, those tools get `ctx.http = None` and
    /// must handle gracefully. Set via `set_http_client`.
    http_client: Option<Arc<reqwest::Client>>,
    /// Path to index.db for tools that need direct DB access (todos, calendar).
    db_path: Option<PathBuf>,
}

/// Per-request approval wiring. Set when the agent has an approval-capable
/// Telegram chat bound to it; None disables approval gating.
#[derive(Clone)]
pub struct ApprovalContext {
    pub store: Arc<approval::PendingActionStore>,
    pub registry: Arc<approval::ApprovalRegistry>,
    pub bot_token: String,
    pub chat_id: i64,
    pub http_client: reqwest::Client,
    /// Owning user_id for pending actions created from this context.
    /// 0 = legacy admin. v5 Item 3.
    pub user_id: i64,
    /// Conversation ID for session-scoped approvals.
    pub conversation_id: Option<String>,
}

impl ToolRegistry {
    /// Construct a registry without MCP. Use `with_mcp` when MCP is available.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self::with_mcp(workspace_root, None)
    }

    /// Attach an approval context — when set, tools matching the
    /// REQUIRES_APPROVAL list will be gated behind a Telegram approve/deny.
    pub fn set_approval(&mut self, ctx: ApprovalContext) {
        self.approval = Some(ctx);
    }

    /// Override the per-agent approval list. `extra_required` adds tools to
    /// the hardcoded REQUIRES_APPROVAL set; `never_required` removes tools.
    pub fn set_approval_overrides(&mut self, extra_required: Vec<String>, never_required: Vec<String>) {
        self.custom_requires_approval = extra_required;
        self.custom_never_approval = never_required;
    }

    /// Add already-constructed extension tools (used to inject OpenAPI tools
    /// loaded once at startup rather than rebuilding per request).
    pub fn add_extension_tools(&mut self, tools: &[Arc<dyn Tool>]) {
        for t in tools {
            self.extensions.insert(t.name().to_string(), Arc::clone(t));
        }
    }

    /// Remove tools from disabled modules. Called after construction
    /// when the modules config is available.
    pub fn apply_module_filter(&mut self, disabled_tools: &[&str]) {
        if disabled_tools.is_empty() { return; }
        let before = self.extensions.len();
        self.extensions.retain(|name, _| !disabled_tools.contains(&name.as_str()));
        let removed = before - self.extensions.len();
        if removed > 0 {
            log::info!("[modules] filtered {} tools from disabled modules", removed);
        }
    }

    /// Wire shared rate-limiter + circuit-breaker state into the registry so
    /// the dispatch funnel can apply both per-tool. Caller passes the Arcs
    /// from `AppState` so the state survives registry rebuilds.
    /// v5 Item 1 Stage 4.
    pub fn set_infra(
        &mut self,
        rate_limiter: Arc<Mutex<RateLimiter>>,
        circuit_breakers: Arc<Mutex<HashMap<String, CircuitBreaker>>>,
    ) {
        self.rate_limiter = Some(rate_limiter);
        self.circuit_breakers = Some(circuit_breakers);
    }

    /// Set the owning user_id (v5 Item 3 → Item 4). The HTTP handlers
    /// build the registry per-request and immediately call this with
    /// `principal.user_id()`.
    pub fn set_user_id(&mut self, user_id: i64) {
        self.user_id = user_id;
    }

    /// Wire user-configurable tool hooks (PreToolUse / PostToolUse).
    /// 4features Stage 2.
    pub fn set_tool_hooks(&mut self, hooks: Arc<crate::tool_hooks::HookStore>) {
        self.tool_hooks = Some(hooks);
    }

    /// Construct a registry, optionally wiring in an MCP registry whose tools
    /// will be exposed under `mcp__<server>__<tool>` wire names.
    pub fn with_mcp(workspace_root: PathBuf, mcp: Option<Arc<McpRegistry>>) -> Self {
        Self::with_extensions(workspace_root, "main".to_string(), mcp, None)
    }

    /// Full constructor: MCP + Indexer + agent identity. Trait-based tools
    /// (like internal_search) are auto-registered when their dependency is
    /// available — internal_search needs an Indexer.
    pub fn with_extensions(
        workspace_root: PathBuf,
        agent_id: String,
        mcp: Option<Arc<McpRegistry>>,
        indexer: Option<Arc<Indexer>>,
    ) -> Self {
        Self::with_extensions_and_allowlist(workspace_root, agent_id, mcp, indexer, &[])
    }

    /// Full constructor with explicit script allowlist from agent config.
    pub fn with_extensions_and_allowlist(
        workspace_root: PathBuf,
        agent_id: String,
        mcp: Option<Arc<McpRegistry>>,
        indexer: Option<Arc<Indexer>>,
        config_allowlist: &[String],
    ) -> Self {
        // Build allowed script paths from workspace skills directory
        let mut allowed = Vec::new();
        if let Ok(entries) = std::fs::read_dir(workspace_root.join("skills")) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Ok(scripts) = std::fs::read_dir(entry.path()) {
                        for script in scripts.flatten() {
                            let path = script.path();
                            if path.extension().map_or(false, |e| e == "py" || e == "sh") {
                                allowed.push(path.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }
        // Merge agent-specific allowlist from config
        for entry in config_allowlist {
            let expanded = entry.replace("~", &std::env::var("HOME").unwrap_or_default());
            if !allowed.contains(&expanded) {
                allowed.push(expanded);
            }
        }

        // Auto-register trait-based tools whose dependencies are available
        let mut extensions: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        if indexer.is_some() {
            let t: Arc<dyn Tool> = Arc::new(InternalSearchTool);
            extensions.insert(t.name().to_string(), t);
        }
        if code_execute::bwrap_available() {
            let t: Arc<dyn Tool> = Arc::new(CodeExecuteTool);
            extensions.insert(t.name().to_string(), t);
        }

        // Register migrated built-in tools (v5 Item 1: ToolRegistry refactor).
        // Each entry replaces a former match arm in execute(). Order is the
        // v5 plan migration order (memory → file_ops → shell → ...) so it's
        // easy to see which family is in / out at any commit.
        macro_rules! reg {
            ($t:expr) => {{
                let arc: Arc<dyn Tool> = Arc::new($t);
                extensions.insert(arc.name().to_string(), arc);
            }};
            // Register one tool under multiple names (alias support so old
            // alias names from the legacy match — `file_read`, `shell`, etc —
            // still resolve to the migrated implementation).
            ($t:expr, $($alias:literal),+) => {{
                let arc: Arc<dyn Tool> = Arc::new($t);
                extensions.insert(arc.name().to_string(), Arc::clone(&arc));
                $(extensions.insert($alias.to_string(), Arc::clone(&arc));)+
            }};
        }
        reg!(agent_memory::MemorySaveTool);
        reg!(agent_memory::MemoryRecallTool);
        reg!(agent_memory::MemoryListTool);
        reg!(agent_memory::MemoryUpdateTool);
        reg!(agent_memory::MemoryForgetTool);
        reg!(built_in_tools::MemoryReadTool);
        reg!(built_in_tools::MemoryWriteTool);
        reg!(built_in_tools::PlanProposeTool);
        reg!(built_in_tools::ReadFileTool, "file_read");
        reg!(built_in_tools::WriteFileTool, "file_write");
        reg!(built_in_tools::EditFileTool, "file_edit");
        reg!(built_in_tools::ListFilesTool);
        reg!(built_in_tools::ExecTool, "shell", "run");
        reg!(built_in_tools::WebSearchTool);
        reg!(built_in_tools::WebFetchTool);
        reg!(built_in_tools::JsonQueryTool);
        reg!(built_in_tools::SendTelegramTool);
        reg!(built_in_tools::EmailReadTool);
        reg!(built_in_tools::EmailSendTool);
        reg!(built_in_tools::SmsGetNumberTool);
        reg!(built_in_tools::SmsReadTool);
        reg!(built_in_tools::SmsWaitForCodeTool);
        reg!(built_in_tools::SolveCaptchaTool, "browser_solve_captcha");
        reg!(built_in_tools::CaptchaBridgeSolveTool);
        reg!(built_in_tools::CaptchaBridgeBalanceTool);
        reg!(built_in_tools::CaptchaBridgeListSitesTool);
        reg!(built_in_tools::OfficeCreateTool);
        reg!(built_in_tools::OfficeViewTool);
        reg!(built_in_tools::OfficeGetTool);
        reg!(built_in_tools::OfficeSetTool);
        reg!(built_in_tools::OfficeBatchTool);
        reg!(built_in_tools::OfficeMergeTool);
        reg!(built_in_tools::OfficeSkillTool);
        reg!(built_in_tools::CreateInstagramAccountTool);
        reg!(built_in_tools::MetaOauthTool);
        reg!(built_in_tools::MetaRefreshTokenTool);
        reg!(built_in_tools::ThreadsPostTool);
        reg!(built_in_tools::CreateFacebookAccountTool);
        reg!(built_in_tools::CreateEmailAccountTool);
        reg!(built_in_tools::YoutubeTokenRefreshTool);
        reg!(built_in_tools::YoutubeReauthTool);
        reg!(built_in_tools::BrowserOpenTool);
        reg!(built_in_tools::BrowserCloseTool);
        reg!(built_in_tools::BrowserOpenAndFillTool);
        reg!(built_in_tools::BrowserFillFormTool);
        reg!(built_in_tools::BrowserFillTool);
        reg!(built_in_tools::BrowserSelectTool);
        reg!(built_in_tools::BrowserSetDropdownTool);
        reg!(built_in_tools::BrowserClickTool);
        reg!(built_in_tools::BrowserReadBriefTool);
        reg!(built_in_tools::BrowserReadTool);
        reg!(built_in_tools::BrowserFindInputsTool);
        reg!(built_in_tools::AddTodoTool);
        reg!(built_in_tools::CompleteTodoTool);
        reg!(built_in_tools::ListTodosTool);
        reg!(built_in_tools::AddCalendarEventTool);
        reg!(built_in_tools::LogExpenseTool);
        reg!(built_in_tools::ExpenseSummaryTool);
        reg!(built_in_tools::GetIncomeTool);
        reg!(built_in_tools::EstimateTaxTool);
        reg!(built_in_tools::ScanReceiptTool);
        reg!(built_in_tools::UpdateTaxBracketsTool);
        reg!(built_in_tools::TaxPrepWizardTool);
        reg!(built_in_tools::FetchTaxBracketsTool);
        reg!(built_in_tools::PropertyProfileTool);
        reg!(built_in_tools::DeductionAutofillTool);
        reg!(built_in_tools::UpdateTaxProfileTool);
        reg!(built_in_tools::BrowserScreenshotTool);
        reg!(built_in_tools::BrowserExecuteJsTool);
        reg!(built_in_tools::BrowserClickAtTool);
        reg!(built_in_tools::BrowserHoldAtTool);
        reg!(built_in_tools::BrowserWaitTool);
        // Voice Journal module
        reg!(voice_journal::SearchJournalTool);
        reg!(voice_journal::JournalSummaryTool);
        reg!(voice_journal::ListRecordingsTool);

        // OpenAPI tools registered via add_openapi_tools() after construction

        Self {
            workspace_root,
            allowed_scripts: allowed,
            mcp,
            indexer,
            agent_id,
            extensions,
            approval: None,
            custom_requires_approval: Vec::new(),
            custom_never_approval: Vec::new(),
            rate_limiter: None,
            circuit_breakers: None,
            user_id: 0,
            tool_hooks: None,
            http_client: None,
            db_path: None,
        }
    }

    /// Set the db_path for tools that need direct database access (todos, calendar).
    pub fn set_db_path(&mut self, path: PathBuf) { self.db_path = Some(path); }

    /// Set the shared HTTP client for outbound-request tools. Call from
    /// voice_chat or any other path that wants tools like `weather` and
    /// `web_fetch` to have a working HTTP client in their ToolContext.
    pub fn set_http_client(&mut self, client: Arc<reqwest::Client>) {
        self.http_client = Some(client);
    }

    /// Execute a tool call. MCP-routed names (mcp__server__tool) are dispatched
    /// to the connected MCP client; everything else falls through to the
    /// built-in match below.
    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        // Approval gate: if this tool name requires approval (per the hardcoded
        // list OR per-agent custom additions, and not in the per-agent never list)
        // AND we have an approval context wired in, queue a pending action and wait.
        let requires_approval =
            (approval::REQUIRES_APPROVAL.contains(&call.name.as_str())
                || self.custom_requires_approval.iter().any(|s| s == &call.name)
                || McpRegistry::is_mcp_tool(&call.name)) // MCP tools require approval by default
                && !self.custom_never_approval.iter().any(|s| s == &call.name);
        if requires_approval {
            if let Some(ctx) = &self.approval {
                // Check session cache first — skip prompt if already approved this session
                let conv_id = ctx.conversation_id.as_deref().unwrap_or("");
                if !conv_id.is_empty() && ctx.registry.is_session_approved(conv_id, &call.name).await {
                    // Session-approved — skip prompt, fall through
                } else {
                    let args_json = serde_json::to_string(&call.arguments).unwrap_or_default();
                    let action_id = match ctx
                        .store
                        .create(&self.agent_id, &call.name, &args_json, ctx.user_id)
                        .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            return ToolResult {
                                tool_call_id: call.id.clone(),
                                success: false,
                                output: format!("Error: failed to queue approval: {}", e),
                            };
                        }
                    };
                    let result = approval::request_approval(
                        &ctx.bot_token,
                        ctx.chat_id,
                        action_id,
                        call,
                        Arc::clone(&ctx.registry),
                        &ctx.http_client,
                    )
                    .await;
                    let (resolved_status, allow) = match &result {
                        Ok(approval::ApprovalScope::Once) => (approval::PendingStatus::Approved, true),
                        Ok(approval::ApprovalScope::Session) => (approval::PendingStatus::Approved, true),
                        Ok(approval::ApprovalScope::Always) => (approval::PendingStatus::Approved, true),
                        Ok(approval::ApprovalScope::Denied) => (approval::PendingStatus::Denied, false),
                        Err(_) => (approval::PendingStatus::TimedOut, false),
                    };

                    // Handle session and always scopes
                    if let Ok(scope) = &result {
                        match scope {
                            approval::ApprovalScope::Session => {
                                if !conv_id.is_empty() {
                                    ctx.registry.grant_session(conv_id, &call.name).await;
                                }
                            }
                            approval::ApprovalScope::Always => {
                                // Add to custom_never_approval so it persists
                                // (this modifies in-memory; persisting to config is a future step)
                                if !conv_id.is_empty() {
                                    ctx.registry.grant_session(conv_id, &call.name).await;
                                }
                                info!("[approval] '{}' permanently allowed by user", call.name);
                            }
                            _ => {}
                        }
                    }

                    let user_scope = if ctx.user_id == 0 { None } else { Some(ctx.user_id) };
                    let _ = ctx
                        .store
                        .resolve(action_id, resolved_status, Some("telegram".to_string()), user_scope)
                        .await;
                    if !allow {
                        return ToolResult {
                            tool_call_id: call.id.clone(),
                            success: false,
                            output: format!(
                                "Error: tool '{}' was not approved (status: {:?})",
                                call.name, resolved_status
                            ),
                        };
                    }
                }
                // Approved — fall through to normal dispatch
            } else {
                // Fail closed: no approval context means we can't get approval
                warn!(
                    "Tool '{}' requires approval but no approval context is wired — BLOCKED",
                    call.name
                );
                return ToolResult {
                    tool_call_id: call.id.clone(),
                    success: false,
                    output: format!(
                        "Error: tool '{}' requires approval but no approval channel is configured. \
                         Set up Telegram integration to enable tool approvals.",
                        call.name
                    ),
                };
            }
        }

        if McpRegistry::is_mcp_tool(&call.name) {
            return match &self.mcp {
                Some(reg) => {
                    let r = reg.execute(&call.name, call.arguments.clone()).await;
                    ToolResult {
                        tool_call_id: call.id.clone(),
                        success: r.is_ok(),
                        output: r.unwrap_or_else(|e| format!("Error: {}", e)),
                    }
                }
                None => ToolResult {
                    tool_call_id: call.id.clone(),
                    success: false,
                    output: "Error: MCP not configured".to_string(),
                },
            };
        }

        // Uniform funnel for trait-based tools. The extension HashMap is the
        // primary path; built-in match below is the fallback for tools not
        // yet migrated. Stage 4 of the v5 ToolRegistry refactor will plug
        // rate_limit + circuit_breaker into `dispatch_extension`.
        if let Some(rich) = self.dispatch_extension(call).await {
            return match rich {
                Ok(r) => ToolResult {
                    tool_call_id: call.id.clone(),
                    success: true,
                    output: r.to_text(),
                },
                Err(e) => ToolResult {
                    tool_call_id: call.id.clone(),
                    success: false,
                    output: format!("Error: {}", e),
                },
            };
        }

        // All built-in tools migrated to the trait extension HashMap
        // (Stages 3a–3e of v5 Item 1). If we get here, the tool name is
        // unknown to BOTH MCP and the trait registry.
        warn!("Unknown tool: {}", call.name);
        ToolResult {
            tool_call_id: call.id.clone(),
            success: false,
            output: format!("Error: Unknown tool: {}", call.name),
        }
    }

    /// Like `execute()` but returns the full `RichToolResult` so callers
    /// (currently the research subtask agent) can recover citations,
    /// artifacts, and structured payloads from trait-based tools. Built-in
    /// tools and MCP tools return a text-only RichToolResult — they don't
    /// produce structured outputs in v1.
    pub async fn execute_rich(&self, call: &ToolCall) -> extension::RichToolResult {
        // MCP path
        if McpRegistry::is_mcp_tool(&call.name) {
            let result = match &self.mcp {
                Some(reg) => reg.execute(&call.name, call.arguments.clone()).await,
                None => Err("MCP not configured".to_string()),
            };
            return match result {
                Ok(text) => extension::RichToolResult::text(text),
                Err(e) => extension::RichToolResult::text(format!("Error: {}", e)),
            };
        }

        // Trait-based extension path — these return RichToolResult natively
        // via the same funnel as `execute()`.
        if let Some(rich) = self.dispatch_extension(call).await {
            return rich.unwrap_or_else(|e| {
                extension::RichToolResult::text(format!("Error: {}", e))
            });
        }

        // Built-in match path: dispatch via legacy execute() and wrap.
        let tr = self.execute(call).await;
        extension::RichToolResult::text(tr.output)
    }

    /// Look up `call.name` in the trait extension HashMap and execute it
    /// through the uniform funnel. Returns `None` if the tool is not
    /// trait-registered (caller should fall through to MCP). Returns
    /// `Some(Result<RichToolResult, String>)` on hit.
    ///
    /// This is the single dispatch point that v5 Item 1 Stage 4 wired up
    /// with rate limiting + circuit breakers. Order of operations:
    ///   1. Look up the tool and its `capabilities()`.
    ///   2. Token-bucket rate limit per tool name (5/min default).
    ///   3. If `circuit_name` is set, check the breaker — if open, error out.
    ///   4. Run the tool. If `circuit_name` is set, wrap in the breaker's
    ///      adaptive timeout.
    ///   5. Record success/failure on the breaker for adaptive p95 timeout.
    async fn dispatch_extension(
        &self,
        call: &ToolCall,
    ) -> Option<Result<extension::RichToolResult, String>> {
        let ext_tool = self.extensions.get(&call.name)?;
        info!(
            "[ext-tool:{}] called by agent {} with args: {}",
            call.name,
            self.agent_id,
            serde_json::to_string(&call.arguments).unwrap_or_default()
        );

        let caps = ext_tool.capabilities();

        // 0. PreToolCall hooks (4features Stage 2). User-configurable
        // hooks fire BEFORE rate limit / circuit / execution. A hook
        // with action='block' returns Err and short-circuits the call;
        // anything else (telegram_notify, audit_log) just runs the side
        // effect and returns Ok.
        if let Some(hooks) = &self.tool_hooks {
            if let Err(block_msg) = hooks
                .fire_pre(&call.name, &self.agent_id, &call.arguments)
                .await
            {
                warn!(
                    "[funnel:{}] blocked by pre-hook: {}",
                    call.name, block_msg
                );
                return Some(Err(format!("blocked by pre-hook: {}", block_msg)));
            }
        }

        // 1. Per-tool rate limit (opt-in via capabilities). Most tools are
        // unlimited because agents legitimately call them many times per
        // turn during research/form-filling/loops. Tools that hit costly
        // external APIs (e.g. OpenRouter, paid-tier services) opt in via
        // `ToolCapabilities::rate_limit = Some((capacity, per_secs))`.
        if let (Some((capacity, per_secs)), Some(rl)) = (caps.rate_limit, &self.rate_limiter) {
            let key = format!("tool:{}", call.name);
            let mut g = rl.lock().await;
            if let Err(wait) = g.check(&key, capacity, per_secs) {
                warn!(
                    "[funnel:{}] rate-limited ({}/{}s): retry in {:.1}s",
                    call.name, capacity, per_secs, wait
                );
                return Some(Err(format!(
                    "Rate limit exceeded for tool '{}'. Retry in {:.1}s.",
                    call.name, wait
                )));
            }
        }

        // 2. Circuit breaker check + adaptive timeout. Only tools that
        // declare a circuit_name participate; everything else runs without
        // a wall-clock cap from us (the tool's own internal timeouts apply).
        let circuit_name = caps.circuit_name.map(String::from);
        let mut adaptive_timeout: Option<Duration> = None;
        if let (Some(name), Some(cbs)) = (circuit_name.as_ref(), &self.circuit_breakers) {
            let mut g = cbs.lock().await;
            let breaker = g
                .entry(name.clone())
                .or_insert_with(|| CircuitBreaker::new(name, Duration::from_secs(60)));
            if !breaker.can_execute() {
                warn!(
                    "[funnel:{}] circuit '{}' is open ({:?}), skipping",
                    call.name,
                    name,
                    breaker.state()
                );
                return Some(Err(format!(
                    "Circuit '{}' is open — too many recent failures",
                    name
                )));
            }
            adaptive_timeout = Some(breaker.timeout());
        }

        // 3. Execute the tool. Wrap in adaptive timeout iff a circuit owns it.
        let ctx = ToolContext {
            workspace: &self.workspace_root,
            agent_id: &self.agent_id,
            indexer: self.indexer.clone(),
            http: self.http_client.clone(),
            rate_limiter: self.rate_limiter.clone(),
            circuit_breakers: self.circuit_breakers.clone(),
            allowed_scripts: &self.allowed_scripts,
            user_id: self.user_id,
            db_path: self.db_path.as_deref(),
            conversation_id: None,
        };
        let started = Instant::now();
        let result = match adaptive_timeout {
            Some(t) => {
                match tokio::time::timeout(t, ext_tool.execute(call.arguments.clone(), &ctx))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => Err(format!(
                        "Tool '{}' timed out after {}s",
                        call.name,
                        t.as_secs()
                    )),
                }
            }
            None => ext_tool.execute(call.arguments.clone(), &ctx).await,
        };
        let latency_ms = started.elapsed().as_millis() as u64;

        // 4. Record success / failure on the breaker.
        if let (Some(name), Some(cbs)) = (circuit_name.as_ref(), &self.circuit_breakers) {
            let mut g = cbs.lock().await;
            if let Some(breaker) = g.get_mut(name) {
                if result.is_ok() {
                    breaker.record_success(latency_ms);
                } else {
                    let was_timeout = result
                        .as_ref()
                        .err()
                        .map(|e| e.contains("timed out"))
                        .unwrap_or(false);
                    breaker.record_failure(was_timeout);
                    if breaker.state() == CircuitState::Open {
                        warn!(
                            "[funnel:{}] circuit '{}' tripped to OPEN",
                            call.name, name
                        );
                    }
                }
            }
        }

        // 5. PostToolCall hooks. Side effects only — never blocks the
        // already-completed tool call. Pattern matching includes the
        // success flag so failure-only and success-only hooks both work.
        if let Some(hooks) = &self.tool_hooks {
            let success = result.is_ok();
            let output_text = match &result {
                Ok(r) => r.content.clone(),
                Err(e) => format!("Error: {}", e),
            };
            hooks
                .fire_post(&call.name, &self.agent_id, &call.arguments, success, &output_text)
                .await;
        }

        Some(result)
    }

    /// Get tool definitions for the LLM (OpenAI function calling format).
    /// Every built-in tool now lives in the extensions HashMap (Stage 3a–3e
    /// of v5 Item 1 collapsed the legacy match into trait-based dispatch),
    /// so this is just MCP + extensions concatenated. Tool name uniqueness
    /// is the registrar's responsibility.
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        let mut defs: Vec<serde_json::Value> = Vec::new();
        if let Some(reg) = &self.mcp {
            defs.extend(reg.tool_definitions());
        }
        for tool in self.extensions.values() {
            defs.push(tool.schema());
        }
        defs
    }

    /// Legacy schema list — every entry has been migrated to a trait-based
    /// tool in `built_in_tools.rs`. Kept as an empty stub so the existing
    /// `tool_definitions()` plumbing doesn't have to special-case the
    /// post-migration state. Safe to delete in a follow-up cleanup.
    #[allow(dead_code)]
    fn built_in_definitions() -> Vec<serde_json::Value> {
        // Every family migrated to built_in_tools.rs in v5 Item 1
        // (Stages 3a–3e). Schemas now live next to their trait impls.
        Vec::new()
    }
}

/// Parse tool calls from LLM response JSON
pub fn parse_tool_calls(response: &serde_json::Value) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    // OpenAI format: choices[0].message.tool_calls
    if let Some(tool_calls) = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("tool_calls"))
        .and_then(|tc| tc.as_array())
    {
        for tc in tool_calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(func) = tc.get("function") {
                let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args_str = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                let arguments = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                if !name.is_empty() {
                    calls.push(ToolCall { id, name, arguments });
                }
            }
        }
    }

    calls
}

/// Execute an office-cli command by calling the binary
pub(super) async fn office_cli_exec(tool_name: &str, args: &serde_json::Value) -> Result<String, String> {
    let cli = "/home/sean/.local/bin/office-cli";

    let mut cmd_args: Vec<String> = Vec::new();

    match tool_name {
        "office_create" => {
            cmd_args.push("create".to_string());
            cmd_args.push(args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string());
        }
        "office_view" => {
            cmd_args.push("view".to_string());
            cmd_args.push(args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string());
        }
        "office_get" => {
            cmd_args.push("get".to_string());
            cmd_args.push(args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string());
            cmd_args.push(args.get("ref").and_then(|v| v.as_str()).unwrap_or("A1").to_string());
        }
        "office_set" => {
            cmd_args.push("set".to_string());
            cmd_args.push(args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string());
            cmd_args.push(args.get("ref").and_then(|v| v.as_str()).unwrap_or("A1").to_string());
            cmd_args.push("--value".to_string());
            cmd_args.push(args.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string());
            if let Some(t) = args.get("type").and_then(|v| v.as_str()) {
                cmd_args.push("--type".to_string());
                cmd_args.push(t.to_string());
            }
        }
        "office_batch" => {
            cmd_args.push("batch".to_string());
            cmd_args.push(args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string());
            cmd_args.push("--ops".to_string());
            cmd_args.push(args.get("ops").map(|v| v.to_string()).unwrap_or("[]".to_string()));
        }
        "office_merge" => {
            cmd_args.push("merge".to_string());
            cmd_args.push(args.get("template").and_then(|v| v.as_str()).unwrap_or("").to_string());
            cmd_args.push("--data".to_string());
            cmd_args.push(args.get("data").map(|v| v.to_string()).unwrap_or("{}".to_string()));
            if let Some(o) = args.get("output").and_then(|v| v.as_str()) {
                cmd_args.push("--output".to_string());
                cmd_args.push(o.to_string());
            }
        }
        "office_skill" => {
            cmd_args.push("skill".to_string());
            cmd_args.push(args.get("skill").and_then(|v| v.as_str()).unwrap_or("").to_string());
            cmd_args.push("--data".to_string());
            cmd_args.push(args.get("data").map(|v| v.to_string()).unwrap_or("{}".to_string()));
        }
        _ => return Err(format!("Unknown office tool: {}", tool_name)),
    }

    let output = tokio::process::Command::new(cli)
        .args(&cmd_args)
        .output()
        .await
        .map_err(|e| format!("Failed to run office-cli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        // Return the JSON error from office-cli
        if !stdout.is_empty() {
            Ok(stdout) // office-cli returns JSON errors on stdout
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}
