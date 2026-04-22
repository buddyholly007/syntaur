//! `/api/setup/*` endpoints for first-run configuration.
//!
//! These endpoints are called by the installer and the dashboard's
//! setup wizard. They handle LLM connection testing, Telegram pairing,
//! and initial config generation.
//!
//! Setup endpoints are available without authentication when no admin
//! user exists yet (first-run). After setup completes, they require
//! admin auth like other `/api/admin/*` endpoints.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::AppState;

pub async fn require_setup_auth(state: &AppState, token: &str) -> Result<(), StatusCode> {
    if is_first_run(state) { return Ok(()); }
    if !state.config.security.require_setup_auth_after_first_run { return Ok(()); }
    let principal = crate::resolve_principal(state, token).await?;
    crate::require_admin(&principal)?;
    Ok(())
}

/// Handler-layer defense-in-depth for first-run bootstrap endpoints.
///
/// The outer `security::bootstrap_loopback_only` middleware already
/// rejects non-loopback peers while the users table is empty — but the
/// external-review guidance is that the setup handlers themselves must
/// also enforce the first-run model rather than trusting middleware
/// alone. This helper re-checks the peer IP at the handler layer and
/// refuses with 403 + audit entry if anything non-loopback reaches the
/// handler during first-run. Callers pass the `ConnectInfo<SocketAddr>`
/// they've been given by axum.
///
/// Once the users table has at least one row, the check is a no-op —
/// normal auth (require_setup_auth) takes over and admins can run
/// setup from any address they're authenticated on.
pub async fn require_first_run_loopback(
    state: &Arc<AppState>,
    peer: std::net::SocketAddr,
) -> Result<(), StatusCode> {
    if !is_first_run(state) {
        return Ok(());
    }
    if peer.ip().is_loopback() {
        return Ok(());
    }
    log::error!(
        "[setup/first-run] handler-level defense-in-depth REJECTED non-loopback peer {peer} \
         during first-run. Middleware should have caught this. Treat as a misconfiguration \
         or a bypass attempt."
    );
    crate::security::audit_log(
        state,
        None,
        "setup.first_run.non_loopback_rejected",
        None,
        serde_json::json!({ "peer": peer.to_string() }),
        Some(peer.ip().to_string()),
        None,
    ).await;
    Err(StatusCode::FORBIDDEN)
}

fn extract_token_from_headers(headers: &axum::http::HeaderMap) -> String {
    headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .unwrap_or("").to_string()
}

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TestLlmRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct TestLlmResponse {
    pub success: bool,
    pub models: Vec<String>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct TestTelegramRequest {
    pub bot_token: String,
}

#[derive(Serialize)]
pub struct TestTelegramResponse {
    pub success: bool,
    pub bot_name: Option<String>,
    pub bot_username: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct TestHaRequest {
    pub base_url: String,
    pub token: String,
}

#[derive(Serialize)]
pub struct TestHaResponse {
    pub success: bool,
    pub version: Option<String>,
    pub device_count: Option<usize>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct SetupStatusResponse {
    pub setup_complete: bool,
    pub has_admin_user: bool,
    pub has_llm_configured: bool,
    pub agent_name: Option<String>,
    pub version: String,
    pub security_warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct ModuleListResponse {
    pub core_modules: Vec<ModuleInfo>,
    pub extension_modules: Vec<ModuleInfo>,
}

#[derive(Serialize)]
pub struct ModuleInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tool_count: usize,
    pub enabled: bool,
    pub tier: String,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// GET /api/setup/status — check if first-run setup is needed.
pub async fn handle_setup_status(
    State(state): State<Arc<AppState>>,
) -> Json<SetupStatusResponse> {
    let has_admin = state.users.list_users().await
        .map(|users| !users.is_empty())
        .unwrap_or(false);

    let has_llm = !state.config.models.providers.is_empty();

    let agent_name = state.config.agents.list.first()
        .map(|a| {
            a.extra.get("name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| a.id.clone())
        });

    let security_warnings = state.config.security.warnings();

    Json(SetupStatusResponse {
        setup_complete: has_admin && has_llm,
        has_admin_user: has_admin,
        has_llm_configured: has_llm,
        agent_name,
        version: env!("CARGO_PKG_VERSION").to_string(),
        security_warnings,
    })
}

/// POST /api/setup/test-llm — test an LLM connection.
pub async fn handle_test_llm(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TestLlmRequest>,
) -> Result<Json<TestLlmResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let client = &state.client;
    let start = std::time::Instant::now();

    // Try to fetch model list from the endpoint
    let models_url = format!("{}/models", req.base_url.trim_end_matches('/'));
    let mut request = client.get(&models_url);

    if let Some(key) = &req.api_key {
        if !key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", key));
        }
    }

    match request.timeout(std::time::Duration::from_secs(10)).send().await {
        Ok(resp) => {
            let latency = start.elapsed().as_millis() as u64;
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                let models: Vec<String> = body.get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                info!("[setup] LLM test OK: {} ({} models, {}ms)", req.base_url, models.len(), latency);
                Ok(Json(TestLlmResponse { success: true, models, latency_ms: latency, error: None }))
            } else {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                warn!("[setup] LLM test failed: {} -> {} {}", req.base_url, status, body);
                Ok(Json(TestLlmResponse {
                    success: false, models: Vec::new(), latency_ms: latency,
                    error: Some(format!("HTTP {}: {}", status, body.chars().take(200).collect::<String>())),
                }))
            }
        }
        Err(e) => {
            warn!("[setup] LLM test error: {} -> {}", req.base_url, e);
            Ok(Json(TestLlmResponse {
                success: false, models: Vec::new(), latency_ms: 0,
                error: Some(format!("Connection failed: {}", e)),
            }))
        }
    }
}

/// POST /api/setup/test-telegram — test a Telegram bot token.
pub async fn handle_test_telegram(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TestTelegramRequest>,
) -> Result<Json<TestTelegramResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let client = &state.client;
    let url = format!("https://api.telegram.org/bot{}/getMe", req.bot_token);

    match client.get(&url).timeout(std::time::Duration::from_secs(5)).send().await {
        Ok(resp) => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                let result = body.get("result").unwrap_or(&serde_json::Value::Null);
                let bot_name = result.get("first_name").and_then(|v| v.as_str()).map(String::from);
                let bot_username = result.get("username").and_then(|v| v.as_str()).map(String::from);
                info!("[setup] Telegram test OK: @{}", bot_username.as_deref().unwrap_or("?"));
                Ok(Json(TestTelegramResponse {
                    success: true, bot_name, bot_username, error: None,
                }))
            } else {
                let desc = body.get("description").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                Ok(Json(TestTelegramResponse {
                    success: false, bot_name: None, bot_username: None,
                    error: Some(desc.to_string()),
                }))
            }
        }
        Err(e) => {
            Ok(Json(TestTelegramResponse {
                success: false, bot_name: None, bot_username: None,
                error: Some(format!("Connection failed: {}", e)),
            }))
        }
    }
}

/// POST /api/setup/test-ha — test a Home Assistant connection.
pub async fn handle_test_ha(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TestHaRequest>,
) -> Result<Json<TestHaResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let client = &state.client;
    let url = format!("{}/api/", req.base_url.trim_end_matches('/'));

    match client.get(&url)
        .header("Authorization", format!("Bearer {}", req.token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                let version = body.get("version").and_then(|v| v.as_str()).map(String::from);

                // Try to get device count
                let states_url = format!("{}/api/states", req.base_url.trim_end_matches('/'));
                let device_count: Option<usize> = match client.get(&states_url)
                    .header("Authorization", format!("Bearer {}", req.token))
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(r) => r.json::<serde_json::Value>().await.ok()
                        .and_then(|v| v.as_array().map(|a| a.len())),
                    Err(_) => None,
                };

                info!("[setup] HA test OK: v{}", version.as_deref().unwrap_or("?"));
                Ok(Json(TestHaResponse {
                    success: true, version, device_count, error: None,
                }))
            } else {
                Ok(Json(TestHaResponse {
                    success: false, version: None, device_count: None,
                    error: Some(format!("HTTP {}", resp.status())),
                }))
            }
        }
        Err(e) => {
            Ok(Json(TestHaResponse {
                success: false, version: None, device_count: None,
                error: Some(format!("Connection failed: {}", e)),
            }))
        }
    }
}

/// GET /api/setup/modules — list available modules.
pub async fn handle_setup_modules(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ModuleListResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let mut core_modules = Vec::new();
    for m in crate::modules::CORE_MODULES {
        let enabled = state.config.modules.entries.get(m.id)
            .map(|e| e.enabled).unwrap_or(m.default_enabled);
        core_modules.push(ModuleInfo {
            id: m.id.to_string(),
            name: m.name.to_string(),
            description: m.description.to_string(),
            tool_count: m.tools.len(),
            enabled,
            tier: "core".to_string(),
        });
    }

    // Scan extension modules
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let modules_dir = crate::resolve_data_dir().join("modules");
    let mut extension_modules = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&modules_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("syntaur.module.toml").exists().then(|| entry.path().join("syntaur.module.toml")).unwrap_or_else(|| entry.path().join("syntaur.module.toml"));
            if let Ok(manifest) = syntaur_sdk::ModuleManifest::from_file(&manifest_path) {
                let enabled = state.config.modules.entries.get(&manifest.id)
                    .map(|e| e.enabled).unwrap_or(true);
                extension_modules.push(ModuleInfo {
                    id: manifest.id,
                    name: manifest.name,
                    description: manifest.description,
                    tool_count: manifest.tools.len(),
                    enabled,
                    tier: "extension".to_string(),
                });
            }
        }
    }

    Ok(Json(ModuleListResponse { core_modules, extension_modules }))
}


/// Check if this is a first-run (no admin users exist and no LLM configured).
pub fn is_first_run(state: &AppState) -> bool {
    // If the config has no model providers, it's a first run
    state.config.models.providers.is_empty()
}

/// Middleware layer that redirects to /setup when in first-run mode.
/// Allows /setup, /api/setup/*, and static assets through.
pub async fn first_run_redirect(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();

    // Always allow setup-related paths, auth, static assets, and health check
    if path == "/setup"
        || path.starts_with("/api/setup/")
        || path.starts_with("/api/auth/")
        || path == "/health"
        || path == "/icon.svg"
        || path == "/favicon.ico"
        || path == "/favicon-32.png"
        || path == "/app-icon.jpg"
        || path == "/logo.jpg"
        || path == "/avatar.png"
        || path == "/icon-192.png"
        || path == "/icon-512.png"
        || path == "/logo-mark.jpg"
        || path.starts_with("/agent-avatar/")
        || path.starts_with("/scheduler-frame/")
        || path == "/manifest.json"
        || path == "/tailwind.js"
        || path.starts_with("/coders/xterm")
        || path == "/fonts.css"
        || path.starts_with("/fonts/")
        || path.starts_with("/static/")
    {
        return next.run(req).await;
    }

    // If first-run, redirect to setup
    if is_first_run(&state) {
        return axum::response::Redirect::temporary("/setup").into_response();
    }

    next.run(req).await
}


/// POST /api/auth/login — exchange password or token for a valid API token.
/// Tries: gateway password, gateway token, user API token.
pub async fn handle_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // Global login rate limit (legacy — cheap first gate).
    let limit = state.config.security.rate_limit_login_per_minute;
    if limit > 0 {
        let mut rl = state.tool_rate_limiter.lock().await;
        if let Err(_wait) = rl.check("login_global", limit, 60) {
            log::warn!("[auth] Login rate limit exceeded ({}/min)", limit);
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }

    // Per-account login limiter. Blocks distributed password guesses
    // against a single username that the global token-bucket doesn't see.
    // Returns the same generic error as a bad-password attempt so the
    // attacker can't probe whether a lockout is active.
    let identity = req.username.clone().unwrap_or_default();
    let wait = state.login_limiter.login_wait_seconds(&identity);
    if wait > 0 {
        log::warn!("[auth] login lockout active identity={:?} {}s", identity, wait);
        crate::security::audit_log(
            &state,
            None,
            "auth.login.locked",
            None,
            serde_json::json!({ "identity": identity, "wait_secs": wait }),
            None, None,
        ).await;
        return Ok(Json(LoginResponse {
            success: false,
            token: None,
            error: Some("Too many failed attempts. Please wait a few minutes and try again.".to_string()),
        }));
    }

    // Try 1: check if it's a valid user API token (ocp_*)
    if let Ok(Some(_resolved)) = state.users.resolve_token(&req.password).await {
        state.login_limiter.note_login_success(&identity);
        return Ok(Json(LoginResponse {
            success: true,
            token: Some(req.password.clone()),
            error: None,
        }));
    }

    // Try 2: per-user password auth (username + password)
    if let Some(ref username) = req.username {
        if let Ok(Some(user)) = state.users.get_user_by_name(username).await {
            if user.disabled {
                return Ok(Json(LoginResponse {
                    success: false,
                    token: None,
                    error: Some("Account is disabled".to_string()),
                }));
            }
            if state.users.verify_password(user.id, &req.password).await.unwrap_or(false) {
                if let Ok(token) = state.users.mint_token_with_expiry(user.id, "dashboard-session", Some(48)).await {
                    state.login_limiter.note_login_success(&identity);
                    crate::security::audit_log(
                        &state,
                        Some(user.id),
                        "auth.login.success",
                        Some(&format!("user:{}", user.id)),
                        serde_json::json!({ "method": "password" }),
                        None, None,
                    ).await;
                    return Ok(Json(LoginResponse {
                        success: true,
                        token: Some(token),
                        error: None,
                    }));
                }
            }
            state.login_limiter.note_login_failure(&identity);
            crate::security::audit_log(
                &state,
                Some(user.id),
                "auth.login.fail",
                Some(&format!("user:{}", user.id)),
                serde_json::json!({ "reason": "bad_password", "username": username }),
                None, None,
            ).await;
            return Ok(Json(LoginResponse {
                success: false,
                token: None,
                error: Some("Invalid username or password".to_string()),
            }));
        }
        // Unknown username. Still notes a failure against the typed
        // identity so enumeration + guess is throttled together.
        state.login_limiter.note_login_failure(&identity);
        crate::security::audit_log(
            &state,
            None,
            "auth.login.fail",
            None,
            serde_json::json!({ "reason": "unknown_username", "username": username }),
            None, None,
        ).await;
        return Ok(Json(LoginResponse {
            success: false,
            token: None,
            error: Some("Invalid username or password".to_string()),
        }));
    }

    // Try 3: if no username was supplied, try the primary admin user's
    // password (user id=1). Lets `password-only` login forms keep working
    // without the user having to remember their own username — the common
    // solo-install case.
    //
    // The pre-v0.5.0 legacy paths (Try 4 / Try 5 against
    // `gateway.auth.password` / `gateway.auth.token`) were removed. Every
    // login must now hit a real user row. Fresh installs land on
    // `/setup/register` instead.
    let mut admin_password_match = false;
    if req.username.is_none() {
        if let Ok(Some(admin)) = state.users.get_user(1).await {
            if !admin.disabled
                && state.users.verify_password(1, &req.password).await.unwrap_or(false)
            {
                admin_password_match = true;
            }
        }
    }

    if admin_password_match {
        state.login_limiter.note_login_success(&identity);
        if let Ok(users) = state.users.list_users().await {
            if let Some(user) = users.first() {
                if let Ok(token) = state.users.mint_token_with_expiry(user.id, "dashboard-session", Some(48)).await {
                    return Ok(Json(LoginResponse {
                        success: true,
                        token: Some(token),
                        error: None,
                    }));
                }
            }
        }
        // mint_token failure is a real server error — don't silently succeed.
        return Ok(Json(LoginResponse {
            success: false,
            token: None,
            error: Some("Login succeeded but token mint failed. Check gateway logs.".to_string()),
        }));
    }

    state.login_limiter.note_login_failure(&identity);
    Ok(Json(LoginResponse {
        success: false,
        token: None,
        error: Some("Invalid password".to_string()),
    }))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
    pub username: Option<String>,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub success: bool,
    pub token: Option<String>,
    pub error: Option<String>,
}


/// POST /api/modules/toggle — enable or disable a module.
/// Updates syntaur.json on disk. Requires gateway restart to take effect.
pub async fn handle_module_toggle(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ModuleToggleRequest>,
) -> Result<Json<ModuleToggleResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let config_path = crate::resolve_data_dir().join("syntaur.json");

    // Read current config
    let config_text = match std::fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(e) => {
            return Ok(Json(ModuleToggleResponse {
                success: false,
                message: format!("Cannot read config: {}", e),
                restart_required: false,
            }));
        }
    };

    let mut config: serde_json::Value = match serde_json::from_str(&config_text) {
        Ok(v) => v,
        Err(e) => {
            return Ok(Json(ModuleToggleResponse {
                success: false,
                message: format!("Cannot parse config: {}", e),
                restart_required: false,
            }));
        }
    };

    // Ensure modules.entries exists
    if config.get("modules").is_none() {
        config["modules"] = serde_json::json!({ "entries": {} });
    }
    if config["modules"].get("entries").is_none() {
        config["modules"]["entries"] = serde_json::json!({});
    }

    // Set the module's enabled state
    config["modules"]["entries"][&req.module_id] = serde_json::json!({
        "enabled": req.enabled
    });

    // Write back
    match std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap_or_default()) {
        Ok(_) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
            }
            let action = if req.enabled { "enabled" } else { "disabled" };
            log::info!("[setup] Module '{}' {} via dashboard", req.module_id, action);
            Ok(Json(ModuleToggleResponse {
                success: true,
                message: format!("Module '{}' {}. Restart gateway to apply.", req.module_id, action),
                restart_required: true,
            }))
        }
        Err(e) => {
            Ok(Json(ModuleToggleResponse {
                success: false,
                message: format!("Cannot write config: {}", e),
                restart_required: false,
            }))
        }
    }
}

#[derive(Deserialize)]
pub struct ModuleToggleRequest {
    pub module_id: String,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct ModuleToggleResponse {
    pub success: bool,
    pub message: String,
    pub restart_required: bool,
}




/// GET /api/license/status — check license status.
pub async fn handle_license_status(
    State(state): State<Arc<AppState>>,
) -> Json<crate::license::LicenseStatus> {
    let data_dir = crate::resolve_data_dir();
    Json(crate::license::check_license(&data_dir))
}

/// POST /api/license/activate — apply a license key.
pub async fn handle_license_activate(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LicenseActivateRequest>,
) -> Result<Json<LicenseActivateResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let data_dir = crate::resolve_data_dir();
    match crate::license::apply_license_key(&data_dir, &req.key) {
        Ok(email) => Ok(Json(LicenseActivateResponse {
            success: true,
            message: format!("License activated for {}", email),
            error: None,
        })),
        Err(e) => Ok(Json(LicenseActivateResponse {
            success: false,
            message: String::new(),
            error: Some(e),
        })),
    }
}

#[derive(Deserialize)]
pub struct LicenseActivateRequest {
    pub key: String,
}

#[derive(Serialize)]
pub struct LicenseActivateResponse {
    pub success: bool,
    pub message: String,
    pub error: Option<String>,
}


/// Rewrite `gateway.auth.password` in the live syntaur.json config so the
/// gateway password and the admin user password never drift. Called after a
/// successful admin user password change (user_id == 1). Uses an atomic
/// temp-file + rename so a crashed write cannot corrupt the config. Refuses
/// to silently overwrite a `{{vault.*}}` template — the caller must handle
/// that case explicitly because we don't want to accidentally break vault-
/// managed deployments.
pub async fn sync_gateway_password(state: &AppState, new_password: &str) -> Result<(), String> {
    let path = &state.config_path;
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    let mut config: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;

    let gw_auth = config
        .pointer_mut("/gateway/auth")
        .ok_or_else(|| "config missing gateway.auth".to_string())?;

    if let Some(existing) = gw_auth.get("password").and_then(|v| v.as_str()) {
        if existing.starts_with("{{vault.") {
            return Err(
                "gateway password is a vault template; change via vault, not via UI"
                    .to_string(),
            );
        }
    }

    gw_auth["password"] = serde_json::Value::String(new_password.to_string());

    let serialized = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("serialize: {}", e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serialized)
        .map_err(|e| format!("write tmp: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {}: {}", path.display(), e))?;

    Ok(())
}

/// POST /api/setup/apply — apply the full setup configuration.
/// Writes config file + agent workspace from installer choices.
pub async fn handle_setup_apply(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetupApplyRequest>,
) -> Result<Json<SetupApplyResponse>, StatusCode> {
    // Defense in depth: even if middleware is misconfigured, handler
    // refuses non-loopback peers during first-run.
    require_first_run_loopback(&state, peer).await?;
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let data_dir = crate::resolve_data_dir();

    // Build config JSON
    let mut providers = serde_json::Map::new();

    // Primary LLM
    if let Some(ref primary) = req.llm_primary {
        providers.insert("primary".to_string(), build_provider(primary));
    }

    // Fallbacks
    for (i, fb) in req.llm_fallbacks.iter().enumerate() {
        providers.insert(format!("fallback-{}", i + 1), build_provider(fb));
    }

    let agent_id = slug(&req.agent_name);

    let mut config = serde_json::json!({
        "gateway": {
            "port": 18789,
            "auth": {
                "mode": "password",
                "password": req.password,
                "token": generate_token()
            }
        },
        "models": {
            "providers": providers
        },
        "agents": {
            "defaults": {
                "model": { "primary": "primary", "fallbacks": [] },
                "tools": { "profile": "full" }
            },
            "list": [{
                "id": &agent_id,
                "tools": { "profile": "full" }
            }]
        },
        "channels": {},
        "bindings": [],
        "mcp": { "servers": {} },
        "modules": { "entries": {} },
        "session": {},
        "plugins": {},
        "hooks": {},
        "commands": {}
    });

    // Telegram
    if let (Some(ref token), Some(chat_id)) = (&req.telegram_token, req.telegram_chat_id) {
        if !token.is_empty() {
            config["channels"]["telegram"] = serde_json::json!({
                "type": "telegram",
                "token": token,
                "allowed_chat_ids": [chat_id]
            });
            config["bindings"] = serde_json::json!([{
                "agent": &agent_id,
                "channel": "telegram"
            }]);
        }
    }

    // Home Assistant
    if let (Some(ref url), Some(ref token)) = (&req.ha_url, &req.ha_token) {
        if !url.is_empty() && !token.is_empty() {
            config["connectors"] = serde_json::json!({
                "home_assistant": {
                    "base_url": url,
                    "token": token
                }
            });
        }
    }

    // Disabled modules
    let mut mod_entries = serde_json::Map::new();
    for m in &req.disabled_modules {
        mod_entries.insert(m.clone(), serde_json::json!({ "enabled": false }));
    }
    config["modules"]["entries"] = serde_json::Value::Object(mod_entries);

    // Image generation — only write a block if the user opted into local SD
    // or paid OpenRouter. No block == Pollinations default (free, zero-config).
    if let Some(ig) = &req.image_gen {
        let mut img = serde_json::Map::new();
        if let Some(u) = ig.local_sd_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            img.insert("local_sd_url".to_string(), serde_json::json!(u));
        }
        if let Some(m) = ig.local_sd_model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            img.insert("local_sd_model".to_string(), serde_json::json!(m));
        }
        if let Some(m) = ig.openrouter_paid_model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            img.insert("openrouter_paid_model".to_string(), serde_json::json!(m));
        }
        if !img.is_empty() {
            config["image_gen"] = serde_json::Value::Object(img);
        }
    }

    // Create the admin user row. Pre-v0.5.0 the setup wizard only wrote
    // `gateway.auth.password` and the login handler checked that config
    // field directly. That legacy path is gone — handle_login now
    // consults the argon2id-hashed password on the first user row. So
    // we must land that row here, or the operator's first login after
    // restart fails with "Invalid username or password."
    //
    // If user id=1 already exists (setup re-run, or bootstrap-admin CLI
    // already ran), we try to reset their password to match what the
    // wizard just accepted. That keeps the "change here, works
    // everywhere" invariant.
    let password_hash = match crate::auth::users::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            return Ok(Json(SetupApplyResponse {
                success: false,
                message: format!("Failed to hash admin password: {e}"),
            }));
        }
    };
    let admin_name = if req.user_name.trim().is_empty() { "admin".to_string() } else { req.user_name.clone() };
    match state.users.get_user(1).await {
        Ok(Some(_)) => {
            if let Err(e) = state.users.set_password(1, &req.password).await {
                log::warn!("[setup] reset admin password failed (continuing): {e}");
            }
        }
        _ => {
            if let Err(e) = state.users.create_user_full(&admin_name, "admin", Some(&password_hash)).await {
                return Ok(Json(SetupApplyResponse {
                    success: false,
                    message: format!("Failed to create admin user: {e}"),
                }));
            }
        }
    }

    // Write config (backup existing first)
    let config_path = data_dir.join("syntaur.json");
    if config_path.exists() {
        let backup = data_dir.join("syntaur.json.bak.pre-setup");
        let _ = std::fs::copy(&config_path, &backup);
        log::info!("[setup] Backed up existing config to {:?}", backup);
    }
    if let Err(e) = std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap_or_default()) {
        return Ok(Json(SetupApplyResponse {
            success: false,
            message: format!("Failed to write config: {}", e),
        }));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    // Create agent workspace
    let workspace = data_dir.join(format!("workspace-{}", agent_id));
    let _ = std::fs::create_dir_all(workspace.join("memory"));
    let _ = std::fs::create_dir_all(workspace.join("skills"));

    // Write template files
    let templates: &[(&str, &str)] = &[
        ("AGENTS.md", include_str!("../../syntaur-defaults/agent-template/AGENTS.md")),
        ("SOUL.md", include_str!("../../syntaur-defaults/agent-template/SOUL.md")),
        ("HEARTBEAT.md", include_str!("../../syntaur-defaults/agent-template/HEARTBEAT.md")),
        ("MEMORY.md", include_str!("../../syntaur-defaults/agent-template/MEMORY.md")),
        ("PENDING_TASKS.md", include_str!("../../syntaur-defaults/agent-template/PENDING_TASKS.md")),
        ("README.md", include_str!("../../syntaur-defaults/agent-template/README.md")),
        ("FIRST_RUN.md", include_str!("../../syntaur-defaults/agent-template/FIRST_RUN.md")),
    ];

    for (filename, template) in templates {
        let content = template
            .replace("{{agent_name}}", &req.agent_name)
            .replace("{{user_name}}", &req.user_name);
        // Simple conditional removal for disabled features
        let content = if req.voice_enabled {
            content
        } else {
            remove_conditional(&content, "voice_enabled")
        };
        let content = if req.ha_url.is_some() {
            content
        } else {
            remove_conditional(&content, "smart_home_enabled")
        };
        let content = if req.telegram_token.is_some() {
            content
        } else {
            remove_conditional(&content, "telegram_enabled")
        };
        // Fill remaining template vars with defaults
        let content = content
            .replace("{{install_date}}", &chrono::Utc::now().format("%Y-%m-%d").to_string())
            .replace("{{llm_primary}}", req.llm_primary.as_ref().map(|p| p.provider.as_str()).unwrap_or("Not configured"))
            .replace("{{llm_fallback}}", &req.llm_fallbacks.iter().map(|f| f.provider.as_str()).collect::<Vec<_>>().join(", "))
            .replace("{{stt_engine}}", "default")
            .replace("{{tts_engine}}", "default")
            .replace("{{ha_url}}", req.ha_url.as_deref().unwrap_or(""));

        let _ = std::fs::write(workspace.join(filename), content);
    }

    // Create cron directory
    let _ = std::fs::create_dir_all(data_dir.join("cron"));
    if !data_dir.join("cron/jobs.json").exists() {
        let _ = std::fs::write(data_dir.join("cron/jobs.json"), "[]");
    }

    log::info!("[setup] Configuration applied: agent={}, workspace={}", agent_id, workspace.display());

    Ok(Json(SetupApplyResponse {
        success: true,
        message: format!("Setup complete! {} is ready. Restart the gateway to apply.", req.agent_name),
    }))
}

#[derive(Deserialize)]
pub struct SetupApplyRequest {
    pub agent_name: String,
    pub user_name: String,
    pub password: String,
    pub llm_primary: Option<LlmProviderInput>,
    pub llm_fallbacks: Vec<LlmProviderInput>,
    pub voice_enabled: bool,
    pub telegram_token: Option<String>,
    pub telegram_chat_id: Option<i64>,
    pub ha_url: Option<String>,
    pub ha_token: Option<String>,
    pub disabled_modules: Vec<String>,
    /// Image generation provider selection, written through to
    /// `config.image_gen`. Missing = Pollinations default (zero config).
    #[serde(default)]
    pub image_gen: Option<ImageGenInput>,
}

#[derive(Deserialize, Default)]
pub struct ImageGenInput {
    #[serde(default)]
    pub local_sd_url: Option<String>,
    #[serde(default)]
    pub local_sd_model: Option<String>,
    #[serde(default)]
    pub openrouter_paid_model: Option<String>,
}

#[derive(Deserialize)]
pub struct LlmProviderInput {
    pub provider: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct SetupApplyResponse {
    pub success: bool,
    pub message: String,
}

fn build_provider(input: &LlmProviderInput) -> serde_json::Value {
    let base_url = match input.provider.as_str() {
        "openrouter" => input.base_url.as_deref().unwrap_or("https://openrouter.ai/api/v1"),
        "groq" => input.base_url.as_deref().unwrap_or("https://api.groq.com/openai/v1"),
        "cerebras" => input.base_url.as_deref().unwrap_or("https://api.cerebras.ai/v1"),
        "openai" => input.base_url.as_deref().unwrap_or("https://api.openai.com/v1"),
        "anthropic" => input.base_url.as_deref().unwrap_or("https://api.anthropic.com/v1"),
        _ => input.base_url.as_deref().unwrap_or("http://127.0.0.1:11434/v1"),
    };
    let api = if input.provider == "anthropic" { "anthropic" } else { "openai-completions" };
    let model_id = input.model.as_deref().unwrap_or("default");
    let mut obj = serde_json::json!({
        "api": api,
        "baseUrl": base_url,
        "models": [{"id": model_id, "name": model_id}],
    });
    if let Some(ref key) = input.api_key {
        if !key.is_empty() {
            obj["apiKey"] = serde_json::json!(key);
        }
    }
    obj
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

fn remove_conditional(input: &str, flag: &str) -> String {
    let open = format!("{{{{#if {}}}}}", flag);
    let close = "{{/if}}";
    let mut result = String::new();
    let mut remaining = input;
    while let Some(start) = remaining.find(&open) {
        result.push_str(&remaining[..start]);
        let after = &remaining[start + open.len()..];
        if let Some(end) = after.find(close) {
            remaining = &after[end + close.len()..];
        } else {
            remaining = "";
        }
    }
    result.push_str(remaining);
    result
}

/// GET /icon.svg
pub async fn handle_icon() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "image/svg+xml".parse().unwrap());
    headers.insert("cache-control", "public, max-age=86400".parse().unwrap());
    (headers, include_str!("../static/icon.svg"))
}

/// GET /manifest.json
pub async fn handle_manifest() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/manifest+json".parse().unwrap());
    (headers, include_str!("../static/manifest.json"))
}


/// GET /favicon.ico
pub async fn handle_favicon() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/x-icon".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/favicon.ico"))
}

/// GET /favicon-32.png
pub async fn handle_favicon_png() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/png".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/favicon-32.png"))
}

/// GET /app-icon.jpg
pub async fn handle_app_icon() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/jpeg".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/app-icon.jpg"))
}

/// GET /logo.jpg
pub async fn handle_logo() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/jpeg".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/logo.jpg"))
}

/// GET /library-bg.webp — Renaissance scholar's-desk image used as the
/// /knowledge page backdrop. ~210 KB WebP, 3168x1344 (~21:9).
pub async fn handle_library_bg() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/webp".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/library-bg.webp"))
}

/// GET /avatar.png
pub async fn handle_avatar() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/png".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/avatar.png"))
}

/// GET /icon-192.png
pub async fn handle_icon_192() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/png".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/icon-192.png"))
}

/// GET /icon-512.png
pub async fn handle_icon_512() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/png".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/icon-512.png"))
}

/// GET /logo-mark.jpg
pub async fn handle_logo_mark() -> (axum::http::HeaderMap, &'static [u8]) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/jpeg".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (h, include_bytes!("../static/logo-mark.jpg"))
}

/// Sanitize agent_id to prevent path traversal — alphanumeric, hyphens, underscores only.
fn sanitize_agent_id(id: &str) -> Result<&str, (StatusCode, String)> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err((StatusCode::BAD_REQUEST, "Invalid agent ID".to_string()));
    }
    Ok(id)
}

/// GET /agent-avatar/{agent_id} — serve agent avatar. Public so `<img>`
/// tags can load without attaching a bearer token (browsers don't forward
/// Authorization headers on image subresources). Avatars are brand/persona
/// illustrations, not sensitive user data. Upload endpoint remains admin-only.
pub async fn handle_agent_avatar(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    _headers: axum::http::HeaderMap,
) -> Result<(axum::http::HeaderMap, Vec<u8>), StatusCode> {
    let agent_id = sanitize_agent_id(&agent_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut h = axum::http::HeaderMap::new();
    h.insert("cache-control", "public, max-age=300".parse().unwrap());

    // Check for custom avatar on disk
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let custom_path = format!("{}/.syntaur/avatars/{}.jpg", home, agent_id);
    if let Ok(data) = std::fs::read(&custom_path) {
        h.insert("content-type", "image/jpeg".parse().unwrap());
        return Ok((h, data));
    }
    let custom_png = format!("{}/.syntaur/avatars/{}.png", home, agent_id);
    if let Ok(data) = std::fs::read(&custom_png) {
        h.insert("content-type", "image/png".parse().unwrap());
        return Ok((h, data));
    }

    // Embedded persona icons (ship with binary, overridden by disk upload above)
    h.insert("content-type", "image/png".parse().unwrap());
    let embedded: Option<&[u8]> = match agent_id {
        "kyron"    => Some(include_bytes!("../static/personas/kyron.png")),
        "positron" => Some(include_bytes!("../static/personas/positron.png")),
        "cortex"   => Some(include_bytes!("../static/personas/cortex.png")),
        "silvr"    => Some(include_bytes!("../static/personas/silvr.png")),
        "thaddeus" => Some(include_bytes!("../static/personas/thaddeus.png")),
        "maurice"  => Some(include_bytes!("../static/personas/maurice.png")),
        "nyota"    => Some(include_bytes!("../static/personas/nyota.png")),
        "mushi"    => Some(include_bytes!("../static/personas/mushi.png")),
        _ => None,
    };
    if let Some(bytes) = embedded {
        return Ok((h, bytes.to_vec()));
    }

    // Final fallback: generic app icon
    Ok((h, include_bytes!("../static/avatar.png").to_vec()))
}

/// GET /scheduler-frame/{key} — serve a decorative scheduler frame image.
/// Public (same as persona avatars) — no user data, just brand artwork.
/// Keys map to `static/scheduler-frames/{key}.webp`. The Scheduler CSS
/// picks one of these via `[data-sch-border]` and renders real calendar
/// content on top.
pub async fn handle_scheduler_frame(
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), StatusCode> {
    // Sanitize key: lowercase + digits + hyphen + dot (for user-uploaded
    // filenames like "custom-ab12cd34.webp"). Prevents path traversal.
    if key.is_empty() || key.len() > 60 ||
       !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut h = axum::http::HeaderMap::new();
    h.insert("content-type", "image/webp".parse().unwrap());
    h.insert("cache-control", "public, max-age=604800, immutable".parse().unwrap());

    // Built-in embedded frames first.
    let embedded: Option<&[u8]> = match key.as_str() {
        "garden-notebook" => Some(include_bytes!("../static/scheduler-frames/garden-notebook.webp")),
        "garden-backdrop" => Some(include_bytes!("../static/scheduler-frames/garden-backdrop.webp")),
        "heirloom"        => Some(include_bytes!("../static/scheduler-frames/heirloom.webp")),
        "woodland"        => Some(include_bytes!("../static/scheduler-frames/woodland.webp")),
        "cosmos"          => Some(include_bytes!("../static/scheduler-frames/cosmos.webp")),
        "field-journal"   => Some(include_bytes!("../static/scheduler-frames/field-journal.webp")),
        _ => None,
    };
    if let Some(bytes) = embedded {
        return Ok((h, bytes.to_vec()));
    }

    // User-uploaded backdrops live at ~/.syntaur/backdrops/{filename}.
    // Public read by design — same reasoning as agent avatars: ambient
    // artwork, not sensitive data, and the random filename serves as
    // the authorization (unguessable). Only accept `custom-` prefix.
    if key.starts_with("custom-") && !key.contains("/") && !key.contains("..") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let path = format!("{}/.syntaur/backdrops/{}", home, key);
        if let Ok(data) = std::fs::read(&path) {
            let ct = if key.ends_with(".png") { "image/png" }
                else if key.ends_with(".jpg") || key.ends_with(".jpeg") { "image/jpeg" }
                else { "image/webp" };
            h.insert("content-type", ct.parse().unwrap());
            // User uploads get shorter cache so changes land quickly; still
            // cacheable but busted by the unguessable filename on replace.
            h.insert("cache-control", "public, max-age=60".parse().unwrap());
            return Ok((h, data));
        }
    }
    Err(StatusCode::NOT_FOUND)
}

/// POST /api/scheduler/backdrop — upload a custom backdrop image. Saves
/// under ~/.syntaur/backdrops/custom-<random>.<ext>, stores the filename
/// in scheduler_prefs.custom_backdrop_file, and cleans up the previous
/// upload. Auth-gated (per-user). 5 MB max.
pub async fn handle_scheduler_backdrop_upload(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|s| (s, "Unauthorized".to_string()))?;
    let user_id = principal.user_id();

    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No image data".to_string()));
    }
    if body.len() > 5 * 1024 * 1024 {
        return Err((StatusCode::BAD_REQUEST, "Image too large (max 5MB)".to_string()));
    }

    let content_type = headers.get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/webp");
    let ext = if content_type.contains("png") { "png" }
        else if content_type.contains("jpeg") || content_type.contains("jpg") { "jpg" }
        else { "webp" };

    // Unguessable suffix serves as the authorization: GET is public but
    // the filename can't be enumerated. 96 bits of entropy is plenty.
    use rand::{rngs::OsRng, RngCore};
    let mut raw = [0u8; 12];
    OsRng.fill_bytes(&mut raw);
    let suffix: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
    let filename = format!("custom-{}.{}", suffix, ext);

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dir = format!("{}/.syntaur/backdrops", home);
    let _ = std::fs::create_dir_all(&dir);
    let path = format!("{}/{}", dir, filename);

    // Look up the previous upload for this user so we can clean it up.
    let db_path = state.db_path.clone();
    let old_filename = tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db_path).ok()?;
        conn.query_row(
            "SELECT custom_backdrop_file FROM scheduler_prefs WHERE user_id = ?",
            rusqlite::params![user_id],
            |r| r.get::<_, String>(0),
        ).ok()
    }).await.ok().flatten();

    std::fs::write(&path, &body).map_err(|e|
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save: {}", e))
    )?;

    // Record new filename + flip the border to `custom` so the UI picks
    // it up immediately on next load without a separate prefs update call.
    let db_path2 = state.db_path.clone();
    let fn_for_db = filename.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db_path2)?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT OR IGNORE INTO scheduler_prefs (user_id, updated_at) VALUES (?, ?)",
            rusqlite::params![user_id, now],
        )?;
        conn.execute(
            "UPDATE scheduler_prefs SET custom_backdrop_file = ?, border = 'custom', updated_at = ? WHERE user_id = ?",
            rusqlite::params![fn_for_db, now, user_id],
        )?;
        Ok(())
    }).await;

    // Best-effort cleanup of previous upload. An orphaned file is
    // acceptable worst-case — it only gets re-overwritten on next upload.
    if let Some(old) = old_filename {
        if !old.is_empty() && old != filename && old.starts_with("custom-") {
            let _ = std::fs::remove_file(format!("{}/{}", dir, old));
        }
    }

    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "file": filename,
    })))
}

/// DELETE /api/scheduler/backdrop — revert the user's custom backdrop.
/// Removes the uploaded file from ~/.syntaur/backdrops/, clears
/// scheduler_prefs.custom_backdrop_file, and flips the user's border back
/// to the default ("garden-backdrop") so the UI has something to render.
pub async fn handle_scheduler_backdrop_delete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|s| (s, "Unauthorized".to_string()))?;
    let user_id = principal.user_id();

    let db_path = state.db_path.clone();
    let (old_filename, had_custom_border): (Option<String>, bool) =
        tokio::task::spawn_blocking(move || -> (Option<String>, bool) {
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(_) => return (None, false),
            };
            let row: Option<(String, String)> = conn.query_row(
                "SELECT custom_backdrop_file, border FROM scheduler_prefs WHERE user_id = ?",
                rusqlite::params![user_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ).ok();
            match row {
                Some((f, b)) => {
                    let fname = if f.is_empty() { None } else { Some(f) };
                    (fname, b == "custom")
                }
                None => (None, false),
            }
        }).await.unwrap_or((None, false));

    // Always clear the filename. Flip border off 'custom' only if it
    // was still 'custom' — don't clobber a user who has already moved
    // on to a different backdrop choice (the file would still be
    // stale on disk and worth cleaning up).
    let db_path2 = state.db_path.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db_path2)?;
        let now = chrono::Utc::now().timestamp();
        if had_custom_border {
            conn.execute(
                "UPDATE scheduler_prefs SET custom_backdrop_file = '', border = 'garden-backdrop', updated_at = ? WHERE user_id = ?",
                rusqlite::params![now, user_id],
            )?;
        } else {
            conn.execute(
                "UPDATE scheduler_prefs SET custom_backdrop_file = '', updated_at = ? WHERE user_id = ?",
                rusqlite::params![now, user_id],
            )?;
        }
        Ok(())
    }).await;

    // Best-effort delete. Leaving an orphan file is acceptable; it
    // can't leak (filename is unguessable) and gets no further refs.
    if let Some(old) = old_filename {
        if old.starts_with("custom-") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
            let path = format!("{}/.syntaur/backdrops/{}", home, old);
            let _ = std::fs::remove_file(&path);
        }
    }

    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "border": if had_custom_border { "garden-backdrop" } else { "unchanged" },
    })))
}

/// POST /api/agent-avatar/{agent_id} — upload custom agent avatar (admin only)
pub async fn handle_agent_avatar_upload(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    // Auth: require admin
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|s| (s, "Unauthorized".to_string()))?;
    crate::require_admin(&principal)
        .map_err(|s| (s, "Admin required".to_string()))?;

    let agent_id = sanitize_agent_id(&agent_id)?;

    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No image data".to_string()));
    }
    if body.len() > 5 * 1024 * 1024 {
        return Err((StatusCode::BAD_REQUEST, "Image too large (max 5MB)".to_string()));
    }

    let content_type = headers.get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg");
    let ext = if content_type.contains("png") { "png" } else { "jpg" };

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dir = format!("{}/.syntaur/avatars", home);
    let _ = std::fs::create_dir_all(&dir);
    let path = format!("{}/{}.{}", dir, agent_id, ext);

    std::fs::write(&path, &body).map_err(|e|
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save avatar: {}", e))
    )?;

    Ok(axum::Json(serde_json::json!({ "success": true, "path": path })))
}


/// GET /tailwind.js — bundled Tailwind CSS (no CDN dependency)
pub async fn handle_tailwind() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/tailwind.js"))
}

/// GET /coders/xterm.min.js
pub async fn handle_xterm_js() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/xterm.min.js"))
}
/// GET /coders/xterm.css
pub async fn handle_xterm_css() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/css".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/xterm.css"))
}
/// GET /coders/xterm-addon-fit.js
pub async fn handle_xterm_fit() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/xterm-addon-fit.js"))
}
/// GET /coders/xterm-addon-search.js
pub async fn handle_xterm_search() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/xterm-addon-search.js"))
}
/// GET /coders/xterm-addon-web-links.js
pub async fn handle_xterm_weblinks() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/xterm-addon-web-links.js"))
}

/// GET /fonts.css — bundled font definitions (no CDN dependency)
pub async fn handle_fonts_css() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/css".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/fonts.css"))
}

/// GET /fonts/{filename} — bundled font files
pub async fn handle_font_file(
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Result<(axum::http::HeaderMap, &'static [u8]), StatusCode> {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "font/ttf".parse().unwrap());
    headers.insert("cache-control", "public, max-age=2592000".parse().unwrap());

    // Match embedded font files by name
    let data: &[u8] = match filename.as_str() {
        "UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuDyYMZg.ttf" =>
            include_bytes!("../static/fonts/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuDyYMZg.ttf"),
        "UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuFuYMZg.ttf" =>
            include_bytes!("../static/fonts/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuFuYMZg.ttf"),
        "UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuGKYMZg.ttf" =>
            include_bytes!("../static/fonts/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuGKYMZg.ttf"),
        "UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuI6fMZg.ttf" =>
            include_bytes!("../static/fonts/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuI6fMZg.ttf"),
        "UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuLyfMZg.ttf" =>
            include_bytes!("../static/fonts/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuLyfMZg.ttf"),
        "tDbY2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8-qxjPQ.ttf" =>
            include_bytes!("../static/fonts/tDbY2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8-qxjPQ.ttf"),
        "tDbY2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8yKxjPQ.ttf" =>
            include_bytes!("../static/fonts/tDbY2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8yKxjPQ.ttf"),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok((headers, data))
}

/// GET /api/setup/scan — run a hardware scan and return results.
/// Returns GPU, CPU, RAM, disk info for the setup wizard.
pub async fn handle_hardware_scan(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<HardwareScanResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let (ram_total, ram_avail) = read_ram();
    let local = detect_local_compute(ram_total);
    let disk_free = read_disk_free();
    let tier = classify_compute_tier(&local, ram_total);

    // Scan network for compute (NVIDIA / Apple / AMD / CPU-only) + LLM services.
    let (network_compute, gpu_scan_blocked) = scan_network_compute().await;
    let network_llms = scan_network_llms().await;

    // Upgrade tier if any LAN node outranks the local box.
    let effective_tier = {
        let local_score = tier_score(&tier);
        let best_network = network_compute.iter()
            .map(|c| tier_score(&classify_compute_tier(
                &LocalCompute {
                    kind: c.kind.clone(),
                    name: Some(c.name.clone()),
                    memory_gb: c.memory_gb.parse::<f64>().ok(),
                    memory_kind: c.memory_kind.clone(),
                    runtime_hint: c.runtime_hint.clone(),
                },
                (c.memory_gb.parse::<f64>().unwrap_or(0.0) * 1024.0) as u64,
            )))
            .max()
            .unwrap_or(0);
        if best_network > local_score && !network_compute.is_empty() {
            format!("{} (network)", tier_label(best_network))
        } else if !network_llms.is_empty() && local_score < 3 {
            "Network LLM available".to_string()
        } else {
            tier
        }
    };

    Ok(Json(HardwareScanResponse {
        cpu: local.name.clone().unwrap_or_else(|| read_cpu_model()),
        ram_total_gb: format!("{:.1}", ram_total as f64 / 1024.0),
        ram_available_gb: format!("{:.1}", ram_avail as f64 / 1024.0),
        gpu_name: if local.kind != "cpu" { local.name.clone() } else { None },
        gpu_vram_gb: if local.kind != "cpu" { local.memory_gb.map(|v| format!("{:.1}", v)) } else { None },
        compute_kind: local.kind.clone(),
        compute_memory_kind: local.memory_kind.clone(),
        compute_runtime: local.runtime_hint.clone(),
        disk_free_gb: disk_free,
        tier: effective_tier,
        network_llms,
        network_compute,
        gpu_scan_blocked,
        local_ip: detect_local_ip(),
    }))
}

/// Numeric scoring used to compare local vs network compute so the wizard
/// can pick the best AI host regardless of whether it's NVIDIA/Apple/CPU.
fn tier_score(tier: &str) -> u8 {
    // Strip optional "(network ...)" suffix before matching.
    let base = tier.split_whitespace().next().unwrap_or(tier);
    match base {
        "Powerful" => 4,
        "Capable" => 3,
        "Limited" => 2,
        "CPU-only" => 1,
        "Minimal" => 0,
        _ => 0,
    }
}
fn tier_label(score: u8) -> &'static str {
    match score {
        4 => "Powerful", 3 => "Capable", 2 => "Limited",
        1 => "CPU-only", _ => "Minimal",
    }
}

#[derive(Serialize)]
pub struct HardwareScanResponse {
    pub cpu: String,
    pub ram_total_gb: String,
    pub ram_available_gb: String,
    pub gpu_name: Option<String>,
    pub gpu_vram_gb: Option<String>,
    /// "nvidia" | "apple" | "amd" | "intel-gpu" | "cpu"
    pub compute_kind: String,
    /// "VRAM" | "unified" | "RAM"
    pub compute_memory_kind: String,
    /// Suggested inference runtime: "CUDA" | "Metal" | "Vulkan" | "llama.cpp-CPU".
    /// AMD is Vulkan (not ROCm) — Vulkan works on any Radeon + iGPU without
    /// the ROCm install matrix, at 60-80% of CUDA perf. TTS (Orpheus) and
    /// vision (Frigate ONNX) roles stay CUDA-only; see GPU_ROLES in the wizard.
    pub compute_runtime: Option<String>,
    pub disk_free_gb: String,
    pub tier: String,
    pub network_llms: Vec<NetworkLlmInfo>,
    pub network_compute: Vec<NetworkComputeInfo>,
    pub gpu_scan_blocked: bool,
    pub local_ip: Option<String>,
}

fn detect_local_ip() -> Option<String> {
    // Connect to a public address (doesn't actually send data) to find our local IP
    std::net::UdpSocket::bind("0.0.0.0:0").ok()
        .and_then(|s| {
            s.connect("8.8.8.8:80").ok()?;
            s.local_addr().ok()
        })
        .map(|addr| addr.ip().to_string())
}

/// A compute node found on the LAN — could be an NVIDIA GPU host, an
/// Apple Silicon Mac with unified memory, an AMD/Intel GPU host, or just
/// a CPU-only box with enough RAM to run small models via llama.cpp.
#[derive(Serialize, Clone)]
pub struct NetworkComputeInfo {
    pub host: String,
    /// "nvidia" | "apple" | "amd" | "intel-gpu" | "cpu"
    pub kind: String,
    pub name: String,
    /// Numeric GB as a string (VRAM, unified, or RAM depending on kind).
    /// Empty when the node was discovered but not probed (e.g. Mac without
    /// Remote Login enabled — seen via mDNS but no way to inspect capacity).
    pub memory_gb: String,
    /// "VRAM" | "unified" | "RAM"
    pub memory_kind: String,
    /// Suggested inference runtime for this node.
    pub runtime_hint: Option<String>,
    /// How we found it: "ssh" = full probe with chip + memory; "mdns" =
    /// discovered via Bonjour broadcast, capacity unknown.
    pub source: String,
    /// User-facing next step when the node can't be fully probed.
    pub probe_hint: Option<String>,
}

#[derive(Serialize)]
pub struct NetworkLlmInfo {
    pub name: String,
    pub url: String,
    pub models: Vec<String>,
    pub host: String,
    pub gpu_name: Option<String>,
    pub gpu_vram_gb: Option<String>,
}

/// Locally-detected compute description. Keep `pub(crate)` visibility
/// for reuse in `detect_remote_gpu` fallback + scan pipeline.
#[derive(Clone)]
pub(crate) struct LocalCompute {
    pub kind: String,
    pub name: Option<String>,
    pub memory_gb: Option<f64>,
    pub memory_kind: String,
    pub runtime_hint: Option<String>,
}

/// Shell script run over SSH (or locally). Emits tagged key=value lines so
/// we can parse regardless of OS. Tries NVIDIA → Apple → AMD in that order,
/// then always emits CPU + RAM so a CPU-only host still surfaces as usable.
///
/// POSIX sh only — no bashisms. Runs on macOS and Linux.
const PROBE_SCRIPT: &str = r#"
OS=$(uname -s 2>/dev/null); ARCH=$(uname -m 2>/dev/null)
echo "os=$OS"; echo "arch=$ARCH"
if command -v nvidia-smi >/dev/null 2>&1; then
  nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>/dev/null | head -1 | awk -F, '{n=$1;m=$2;sub(/^ +/,"",n);sub(/ +$/,"",n);sub(/^ +/,"",m);printf "nvidia=%s|%s\n",n,m}'
elif [ "$OS" = "Darwin" ]; then
  C=$(sysctl -n machdep.cpu.brand_string 2>/dev/null)
  M=$(sysctl -n hw.memsize 2>/dev/null)
  echo "apple=$C|$M"
elif command -v rocm-smi >/dev/null 2>&1; then
  N=$(rocm-smi --showproductname 2>/dev/null | grep -i 'card series' | head -1 | sed 's/.*: *//')
  V=$(rocm-smi --showmeminfo vram --csv 2>/dev/null | tail -1 | awk -F, '{print $2}')
  [ -n "$N" ] && echo "amd=$N|$V"
elif command -v lspci >/dev/null 2>&1; then
  # AMD/Radeon via lspci — catches Radeon hosts without rocm-smi
  # (the common case; Vulkan doesn't need ROCm). VRAM is unknown from
  # lspci alone, reported as 0; classifier falls back to RAM-based tier.
  A=$(lspci 2>/dev/null | grep -iE 'vga|3d|display' | grep -iE 'amd|ati|radeon' | head -1 | sed 's/.*: *//')
  if [ -n "$A" ]; then
    echo "amd=$A|0"
  else
    I=$(lspci 2>/dev/null | grep -iE 'vga|3d|display' | grep -iE 'intel.*(arc|xe)' | head -1 | sed 's/.*: *//')
    [ -n "$I" ] && echo "intel-gpu=$I|0"
  fi
fi
if [ "$OS" = "Darwin" ]; then
  CPU=$(sysctl -n machdep.cpu.brand_string 2>/dev/null)
  R=$(sysctl -n hw.memsize 2>/dev/null); RAM_KB=$(( R / 1024 ))
else
  CPU=$(grep -m1 '^model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2 | sed 's/^ *//')
  RAM_KB=$(grep -m1 '^MemTotal' /proc/meminfo 2>/dev/null | awk '{print $2}')
fi
echo "cpu=$CPU"
echo "ram_kb=$RAM_KB"
"#;

/// Parse the PROBE_SCRIPT output into a LocalCompute. Called on both the
/// local host (stdout of `sh -c PROBE_SCRIPT`) and remote hosts (stdout of
/// SSH'd probe).
pub(crate) fn parse_probe_output(stdout: &str) -> LocalCompute {
    let mut kv = std::collections::HashMap::new();
    for line in stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let split_name_mem = |s: &str| -> (String, Option<f64>) {
        let mut it = s.splitn(2, '|');
        let name = it.next().unwrap_or("").trim().to_string();
        let mem_raw = it.next().unwrap_or("0").trim().to_string();
        let mem_gb = mem_raw.parse::<f64>().ok();
        (name, mem_gb)
    };

    if let Some(v) = kv.get("nvidia") {
        let (name, mb) = split_name_mem(v);
        return LocalCompute {
            kind: "nvidia".into(),
            name: Some(name),
            memory_gb: mb.map(|m| m / 1024.0),  // MB → GB
            memory_kind: "VRAM".into(),
            runtime_hint: Some("CUDA".into()),
        };
    }
    if let Some(v) = kv.get("apple") {
        let (name, bytes) = split_name_mem(v);
        return LocalCompute {
            kind: "apple".into(),
            name: Some(name),
            memory_gb: bytes.map(|b| b / 1024.0 / 1024.0 / 1024.0),  // bytes → GB
            memory_kind: "unified".into(),
            runtime_hint: Some("Metal".into()),
        };
    }
    if let Some(v) = kv.get("amd") {
        let (name, mb) = split_name_mem(v);
        return LocalCompute {
            kind: "amd".into(),
            name: Some(name),
            memory_gb: mb.map(|m| m / 1024.0),
            memory_kind: "VRAM".into(),
            // Vulkan, not ROCm — Radeon support matrix is too narrow and the
            // install story is brutal on non-supported cards. Vulkan
            // llama.cpp gets ~60-80% of CUDA perf on almost any Radeon
            // (plus Vega/RDNA iGPUs) with no extra install burden.
            runtime_hint: Some("Vulkan".into()),
        };
    }
    if let Some(v) = kv.get("intel-gpu") {
        let (name, _) = split_name_mem(v);
        return LocalCompute {
            kind: "intel-gpu".into(),
            name: Some(name),
            memory_gb: None,
            memory_kind: "VRAM".into(),
            runtime_hint: Some("OpenVINO".into()),
        };
    }
    // CPU fallback
    let cpu_name = kv.get("cpu").cloned().filter(|s| !s.is_empty());
    let ram_kb: u64 = kv.get("ram_kb").and_then(|s| s.parse().ok()).unwrap_or(0);
    LocalCompute {
        kind: "cpu".into(),
        name: cpu_name,
        memory_gb: if ram_kb > 0 { Some(ram_kb as f64 / 1024.0 / 1024.0) } else { None },
        memory_kind: "RAM".into(),
        runtime_hint: if ram_kb > 0 { Some("llama.cpp-CPU".into()) } else { None },
    }
}

fn read_cpu_model() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
            if let Some(line) = info.lines().find(|l| l.starts_with("model name")) {
                if let Some(name) = line.split(':').nth(1) {
                    return name.trim().to_string();
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]).output() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    "Unknown CPU".to_string()
}

fn read_ram() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
            let parse = |prefix: &str| -> u64 {
                info.lines()
                    .find(|l| l.starts_with(prefix))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0) / 1024 // KB to MB
            };
            return (parse("MemTotal:"), parse("MemAvailable:"));
        }
    }
    (0, 0)
}

/// Run the unified compute probe locally.
fn detect_local_compute(ram_total_mb: u64) -> LocalCompute {
    let out = std::process::Command::new("sh")
        .args(["-c", PROBE_SCRIPT])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let mut lc = parse_probe_output(&String::from_utf8_lossy(&o.stdout));
            // Ensure CPU-only nodes at least report our computed RAM if the
            // probe couldn't read it.
            if lc.kind == "cpu" && lc.memory_gb.is_none() && ram_total_mb > 0 {
                lc.memory_gb = Some(ram_total_mb as f64 / 1024.0);
            }
            return lc;
        }
    }
    // Shell probe failed — degrade to minimal CPU report.
    LocalCompute {
        kind: "cpu".into(),
        name: Some(read_cpu_model()),
        memory_gb: Some(ram_total_mb as f64 / 1024.0),
        memory_kind: "RAM".into(),
        runtime_hint: None,
    }
}

fn read_disk_free() -> String {
    if let Ok(out) = std::process::Command::new("df").args(["-BG", "/"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().nth(1) {
            if let Some(avail) = line.split_whitespace().nth(3) {
                return avail.trim_end_matches('G').to_string();
            }
        }
    }
    "?".to_string()
}

/// Classify a compute node's AI usefulness. Thresholds reflect what each
/// memory regime can actually run today (q4 quantization, llama.cpp-tier
/// tooling): NVIDIA VRAM is tightest, Apple unified is mid (you can trade
/// RAM for VRAM), CPU-only is generous because 70B-q4 in 64GB RAM is very
/// slow but possible.
fn classify_compute_tier(lc: &LocalCompute, fallback_ram_mb: u64) -> String {
    let mem = lc.memory_gb.unwrap_or(0.0);
    let ram_gb = fallback_ram_mb as f64 / 1024.0;
    match lc.kind.as_str() {
        "nvidia" | "amd" => {
            if mem >= 16.0 { "Powerful".into() }
            else if mem >= 8.0 { "Capable".into() }
            else if mem >= 4.0 { "Limited".into() }
            else if ram_gb >= 16.0 { "CPU-only".into() }
            else { "Minimal".into() }
        }
        "apple" => {
            // Unified memory — a Mac with 32GB can run a 30B-q4 model
            // comfortably; 16GB runs 13B; 8GB is 7B territory.
            if mem >= 32.0 { "Powerful".into() }
            else if mem >= 16.0 { "Capable".into() }
            else if mem >= 8.0 { "Limited".into() }
            else { "CPU-only".into() }
        }
        "intel-gpu" => {
            // Intel Arc/Xe — limited ML tooling today; treat as CPU-assist.
            if ram_gb >= 16.0 { "Limited".into() } else { "CPU-only".into() }
        }
        _ => {
            // CPU-only node. 32GB+ RAM can do small-model serving; 16GB
            // is small-model inference only; below that, assistant-only.
            if mem >= 32.0 { "Limited".into() }
            else if mem >= 16.0 { "CPU-only".into() }
            else if mem >= 8.0 { "CPU-only".into() }
            else { "Minimal".into() }
        }
    }
}


async fn scan_network_llms() -> Vec<NetworkLlmInfo> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut found = Vec::new();

    // Get local subnet
    let home_ip = std::env::var("HOME").unwrap_or_default();
    let subnet = get_subnet_prefix();

    // Check known LLM ports on the subnet
    let ports = [(11434, "Ollama"), (1234, "LM Studio"), (1235, "llama.cpp/TurboQuant"), (1236, "LLM Proxy")];
    let mut handles = Vec::new();

    for i in 1..=254u8 {
        let ip = format!("{}.{}", subnet, i);
        for &(port, name) in &ports {
            let client = client.clone();
            let ip = ip.clone();
            let name = name.to_string();
            handles.push(tokio::spawn(async move {
                let url = format!("http://{}:{}", ip, port);
                let models_url = if port == 11434 {
                    format!("{}/api/tags", url)
                } else {
                    format!("{}/v1/models", url)
                };
                if let Ok(resp) = client.get(&models_url).send().await {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let models: Vec<String> = if port == 11434 {
                            body.get("models").and_then(|m| m.as_array())
                                .map(|a| a.iter().filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from)).collect())
                                .unwrap_or_default()
                        } else {
                            body.get("data").and_then(|d| d.as_array())
                                .map(|a| a.iter().filter_map(|m| m.get("id").and_then(|n| n.as_str()).map(String::from)).collect())
                                .unwrap_or_default()
                        };
                        // Try to detect GPU on the remote host
                        let (gpu_name, gpu_vram) = detect_remote_gpu(&client, &ip).await;
                        return Some(NetworkLlmInfo {
                            name: format!("{} at {}:{}", name, ip, port),
                            host: ip.clone(),
                            url,
                            models,
                            gpu_name,
                            gpu_vram_gb: gpu_vram,
                        });
                    }
                }
                None
            }));
        }
    }

    for handle in handles {
        if let Ok(Some(info)) = handle.await {
            found.push(info);
        }
    }

    found
}

fn get_subnet_prefix() -> String {
    if let Ok(out) = std::process::Command::new("ip").args(["route", "get", "1.1.1.1"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(src) = s.split("src ").nth(1).and_then(|s| s.split_whitespace().next()) {
            if let Some(prefix) = src.rsplitn(2, '.').nth(1) {
                return prefix.to_string();
            }
        }
    }
    "192.168.1".to_string()
}


/// Run [PROBE_SCRIPT] against a remote host by SSH'ing in and piping the
/// script to `sh` on stdin. This sidesteps the user's login shell entirely
/// — critical because some hosts in our LAN (e.g. gaming PC) log in to
/// fish, which rejects POSIX `$()` + `=` assignment syntax.
async fn run_probe_over_ssh(host: &str, timeout_secs: &str) -> Option<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    use std::process::Stdio;

    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o", &format!("ConnectTimeout={}", timeout_secs),
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            &format!("sean@{}", host),
            "sh",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(PROBE_SCRIPT.as_bytes()).await.ok()?;
        drop(stdin); // close so `sh` sees EOF and exits
    }

    let out = child.wait_with_output().await.ok()?;
    if !out.status.success() { return None; }
    Some(out.stdout)
}

/// Probe a remote host's compute over SSH. Returns `Some(NetworkComputeInfo)`
/// if SSH connected and produced usable output, `None` if unreachable/denied.
async fn probe_remote_compute(host: &str) -> Option<NetworkComputeInfo> {
    let stdout = run_probe_over_ssh(host, "1").await?;
    let lc = parse_probe_output(&String::from_utf8_lossy(&stdout));
    // Skip hosts where we couldn't identify anything useful.
    if lc.name.as_deref().unwrap_or("").is_empty() && lc.kind == "cpu" && lc.memory_gb.unwrap_or(0.0) < 4.0 {
        return None;
    }
    let mem = lc.memory_gb.unwrap_or(0.0);
    Some(NetworkComputeInfo {
        host: host.to_string(),
        kind: lc.kind,
        name: lc.name.unwrap_or_else(|| "(unknown)".to_string()),
        memory_gb: format!("{:.1}", mem),
        memory_kind: lc.memory_kind,
        runtime_hint: lc.runtime_hint,
        source: "ssh".to_string(),
        probe_hint: None,
    })
}

/// Discover Apple devices on the LAN via mDNS/Bonjour. Returns lightweight
/// `NetworkComputeInfo` entries for Mac-class hardware (MacBook / iMac /
/// Mac Studio / Mac Pro / Mac mini). iPhones, iPads, Apple TVs, HomePods
/// are skipped — they're not AI-accessible targets.
///
/// We probe `_device-info._tcp.local.` (Apple's discovery service with a
/// `model=...` TXT record) and `_ssh._tcp.local.` (Remote-Login-enabled
/// Macs). Both are passive broadcasts; no auth, no state changes.
async fn scan_mdns_apple() -> Vec<NetworkComputeInfo> {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // Run the blocking mDNS daemon on a thread pool so we don't block
    // tokio's executor.
    let result = tokio::task::spawn_blocking(|| -> Vec<NetworkComputeInfo> {
        let daemon = match mdns_sd::ServiceDaemon::new() {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        // Keyed by IP so we don't emit duplicates when a Mac responds on
        // both service types.
        let mut devices: HashMap<String, NetworkComputeInfo> = HashMap::new();

        for service_type in &["_device-info._tcp.local.", "_ssh._tcp.local.", "_rfb._tcp.local."] {
            let rx = match daemon.browse(service_type) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline {
                let wait = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                        let hostname = info.get_hostname().to_string();
                        let addrs: Vec<String> = info.get_addresses().iter()
                            .map(|a| a.to_string())
                            .filter(|a| a.contains('.'))  // IPv4 only for simplicity
                            .collect();
                        // Parse TXT records — Apple device-info includes "model=..."
                        let mut model: Option<String> = None;
                        for prop in info.get_properties().iter() {
                            if prop.key() == "model" {
                                model = Some(prop.val_str().to_string());
                            }
                        }
                        let (display_name, is_mac) = classify_apple_model(&model, &hostname);
                        if !is_mac { continue; }  // Skip iOS / tvOS / audioOS
                        for ip in &addrs {
                            devices.entry(ip.clone()).or_insert_with(|| NetworkComputeInfo {
                                host: ip.clone(),
                                kind: "apple".to_string(),
                                name: display_name.clone(),
                                memory_gb: String::new(),
                                memory_kind: "unified".to_string(),
                                runtime_hint: Some("Metal".to_string()),
                                source: "mdns".to_string(),
                                probe_hint: Some(
                                    "Enable Remote Login on this Mac (System Settings → General → Sharing) and authorize this host's SSH key to see chip and unified-memory details."
                                        .to_string(),
                                ),
                            });
                        }
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let _ = daemon.stop_browse(service_type);
        }
        let _ = daemon.shutdown();
        devices.into_values().collect()
    }).await;

    result.unwrap_or_default()
}

/// Map an Apple model string (from mDNS TXT `model=...`, e.g.
/// "MacBookAir10,1", "iPhone13,2", "AppleTV14,1") to a friendly name and
/// a flag for whether it's a Mac-class host (AI-usable).
fn classify_apple_model(model: &Option<String>, fallback_hostname: &str) -> (String, bool) {
    // Trim ".local." and trailing index numbers from hostname for display
    // when model is unavailable.
    let clean_hostname = fallback_hostname
        .trim_end_matches('.')
        .trim_end_matches(".local")
        .to_string();

    let Some(m) = model.as_deref() else {
        // No model TXT — guess from hostname
        let lower = clean_hostname.to_lowercase();
        let is_mac = lower.contains("macbook") || lower.contains("imac")
            || lower.contains("mac-mini") || lower.contains("macmini")
            || lower.contains("mac-studio") || lower.contains("macpro");
        return (clean_hostname, is_mac);
    };
    // Known Mac-class model prefixes
    let friendly = match m {
        s if s.starts_with("MacBookAir") => "MacBook Air",
        s if s.starts_with("MacBookPro") => "MacBook Pro",
        s if s.starts_with("MacBook")    => "MacBook",
        s if s.starts_with("iMacPro")    => "iMac Pro",
        s if s.starts_with("iMac")       => "iMac",
        s if s.starts_with("Macmini")    => "Mac mini",
        s if s.starts_with("MacStudio")  => "Mac Studio",
        s if s.starts_with("MacPro")     => "Mac Pro",
        s if s.starts_with("Mac")        => "Mac",
        // Non-Mac Apple hardware we deliberately skip
        s if s.starts_with("iPhone")   => return (format!("iPhone ({})", s), false),
        s if s.starts_with("iPad")     => return (format!("iPad ({})", s), false),
        s if s.starts_with("iPod")     => return (format!("iPod ({})", s), false),
        s if s.starts_with("AppleTV")  => return (format!("Apple TV ({})", s), false),
        s if s.starts_with("AudioAccessory") => return (format!("HomePod ({})", s), false),
        s if s.starts_with("Watch")    => return (format!("Apple Watch ({})", s), false),
        other => return (other.to_string(), false),
    };
    (format!("{} ({})", friendly, m), true)
}

/// Decorator used by scan_network_llms to annotate an LLM-serving host
/// with its compute profile. Wraps probe_remote_compute for back-compat
/// with the (name, memory_gb_string) shape the callsite still expects.
async fn detect_remote_gpu(_client: &reqwest::Client, host: &str) -> (Option<String>, Option<String>) {
    if host == "127.0.0.1" || host == "localhost" {
        let (ram, _) = read_ram();
        let lc = detect_local_compute(ram);
        if lc.kind != "cpu" {
            return (lc.name, lc.memory_gb.map(|m| format!("{:.1}", m)));
        }
        return (None, None);
    }
    match probe_remote_compute(host).await {
        Some(c) if c.kind != "cpu" => (Some(c.name), Some(c.memory_gb)),
        _ => (None, None),
    }
}

/// Scan every IP on the local subnet via SSH + PROBE_SCRIPT, collecting
/// all AI-usable compute: NVIDIA, Apple Silicon, AMD, Intel, and CPU-only
/// hosts with enough RAM for small-model inference. Returns (found, blocked).
async fn scan_network_compute() -> (Vec<NetworkComputeInfo>, bool) {
    let subnet = get_subnet_prefix();
    let mut handles = Vec::new();

    for i in 1..=254u8 {
        let ip = format!("{}.{}", subnet, i);
        handles.push(tokio::spawn(async move {
            // Try the unified probe script. If SSH fails entirely, mark as
            // potentially blocked. If SSH succeeds but the host has nothing
            // useful (< 4 GB RAM, no GPU), probe_remote_compute returns None.
            let probed = probe_remote_compute(&ip).await;
            match probed {
                Some(c) => (Some(c), false),
                None => {
                    // Distinguish "SSH unreachable" from "reachable but nothing".
                    // Re-try with just `echo ok` to see if ssh works at all.
                    let reachable = tokio::process::Command::new("ssh")
                        .args([
                            "-o", "ConnectTimeout=1",
                            "-o", "StrictHostKeyChecking=no",
                            "-o", "BatchMode=yes",
                            &format!("sean@{}", ip),
                            "true",
                        ])
                        .output()
                        .await
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                    (None, !reachable)
                }
            }
        }));
    }

    // Run the SSH sweep and the mDNS Apple scan concurrently — mDNS is
    // multicast so it can't be parallelized per-host, but it can overlap
    // the full SSH pass.
    let mdns_fut = scan_mdns_apple();

    let mut found: Vec<NetworkComputeInfo> = Vec::new();
    let mut any_blocked = false;
    for handle in handles {
        if let Ok((compute, was_blocked)) = handle.await {
            if let Some(c) = compute { found.push(c); }
            if was_blocked { any_blocked = true; }
        }
    }

    // Merge mDNS-discovered Macs. SSH results win on conflict (they carry
    // real capacity); mDNS fills in Macs with Remote Login disabled.
    let mdns_devices = mdns_fut.await;
    let known_hosts: std::collections::HashSet<String> =
        found.iter().map(|c| c.host.clone()).collect();
    for d in mdns_devices {
        if !known_hosts.contains(&d.host) {
            found.push(d);
        }
    }

    // Only report blocked when we found nothing useful AND some hosts
    // refused the SSH probe — avoids shouting "firewall!" when the LAN is
    // just quiet.
    let blocked = found.is_empty() && any_blocked;
    (found, blocked)
}


/// POST /api/setup/fix-firewall — attempt to add firewall rules for GPU scanning.
/// Tries to enable SSH outbound on the local subnet.
pub async fn handle_fix_firewall(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<FirewallFixResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    // Try to add iptables/ufw rule for SSH scanning
    // This requires elevated permissions — try sudo

    // Method 1: ufw (Ubuntu/Debian)
    let ufw_result = std::process::Command::new("sudo")
        .args(["-n", "ufw", "allow", "out", "22/tcp"])
        .output();

    if let Ok(output) = ufw_result {
        if output.status.success() {
            // Also reload ufw
            let _ = std::process::Command::new("sudo")
                .args(["-n", "ufw", "reload"])
                .output();
            log::info!("[setup] Firewall rule added via ufw: allow out 22/tcp");
            return Ok(Json(FirewallFixResponse {
                success: true,
                message: "Firewall updated — SSH outbound allowed for GPU scanning.".to_string(),
            }));
        }
    }

    // Method 2: iptables directly
    let ipt_result = std::process::Command::new("sudo")
        .args(["-n", "iptables", "-A", "OUTPUT", "-p", "tcp", "--dport", "22", "-j", "ACCEPT"])
        .output();

    if let Ok(output) = ipt_result {
        if output.status.success() {
            log::info!("[setup] Firewall rule added via iptables: allow outbound SSH");
            return Ok(Json(FirewallFixResponse {
                success: true,
                message: "Firewall updated via iptables — SSH outbound allowed.".to_string(),
            }));
        }
    }

    // Method 3: firewalld
    let fwd_result = std::process::Command::new("sudo")
        .args(["-n", "firewall-cmd", "--add-service=ssh", "--permanent"])
        .output();

    if let Ok(output) = fwd_result {
        if output.status.success() {
            let _ = std::process::Command::new("sudo")
                .args(["-n", "firewall-cmd", "--reload"])
                .output();
            log::info!("[setup] Firewall rule added via firewalld");
            return Ok(Json(FirewallFixResponse {
                success: true,
                message: "Firewall updated via firewalld.".to_string(),
            }));
        }
    }

    // None worked — likely need password for sudo
    Ok(Json(FirewallFixResponse {
        success: false,
        message: "Could not modify firewall automatically (sudo access required). Try running: sudo ufw allow out 22/tcp && sudo ufw reload".to_string(),
    }))
}

#[derive(Serialize)]
pub struct FirewallFixResponse {
    pub success: bool,
    pub message: String,
}


/// GET /api/setup/check-tailscale — check if Tailscale is installed and connected.
pub async fn handle_check_tailscale(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<TailscaleStatus>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    // Check if tailscale binary exists
    let installed = std::process::Command::new("which")
        .arg("tailscale")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !installed {
        return Ok(Json(TailscaleStatus {
            installed: false,
            connected: false,
            ip: None,
            hostname: None,
            url: None,
        }));
    }

    // Check tailscale status
    let status_output = std::process::Command::new("tailscale")
        .arg("status")
        .output();

    let connected = status_output.as_ref()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Get tailscale IP
    let ip = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    // Get tailscale hostname
    let hostname = std::process::Command::new("tailscale")
        .args(["status", "--self", "--json"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()
        })
        .and_then(|v| v.get("Self").and_then(|s| s.get("DNSName")).and_then(|d| d.as_str()).map(|s| s.trim_end_matches('.').to_string()));

    let port = state.config.gateway.port;
    let url = if let Some(ref h) = hostname {
        Some(format!("http://{}:{}", h, port))
    } else if let Some(ref i) = ip {
        Some(format!("http://{}:{}", i, port))
    } else {
        None
    };

    Ok(Json(TailscaleStatus {
        installed,
        connected,
        ip,
        hostname,
        url,
    }))
}

#[derive(Serialize)]
pub struct TailscaleStatus {
    pub installed: bool,
    pub connected: bool,
    pub ip: Option<String>,
    pub hostname: Option<String>,
    pub url: Option<String>,
}

/// GET /api/setup/ssh-pubkey — return this machine's SSH public key for the GPU guide.
pub async fn handle_ssh_pubkey(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    
    // Try common key locations
    for key_file in &["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"] {
        let path = format!("{}/.ssh/{}", home, key_file);
        if let Ok(key) = std::fs::read_to_string(&path) {
            return Ok(Json(serde_json::json!({ "key": key.trim() })));
        }
    }

    // No key found — try to generate one
    let key_path = format!("{}/.ssh/id_ed25519", home);
    let _ = std::fs::create_dir_all(format!("{}/.ssh", home));

    if let Ok(output) = std::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-f", &key_path, "-N", "", "-q"])
        .output()
    {
        if output.status.success() {
            if let Ok(key) = std::fs::read_to_string(format!("{}.pub", key_path)) {
                return Ok(Json(serde_json::json!({ "key": key.trim() })));
            }
        }
    }

    Ok(Json(serde_json::json!({ "error": "Could not find or generate an SSH key. You may need to run: ssh-keygen -t ed25519" })))
}

/// POST /api/setup/test-gpu — test SSH connection to a remote host and
/// detect whatever compute it has (NVIDIA / Apple / AMD / CPU-only).
pub async fn handle_test_gpu(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TestGpuRequest>,
) -> Result<Json<TestGpuResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    // Pipe PROBE_SCRIPT to `sh` on stdin (sidesteps fish/zsh login shells).
    use tokio::io::AsyncWriteExt;
    use std::process::Stdio;
    let child_result = tokio::process::Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=5",
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            &format!("{}@{}", req.username, req.host),
            "sh",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let output = match child_result {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(PROBE_SCRIPT.as_bytes()).await;
                drop(stdin);
            }
            child.wait_with_output().await
        }
        Err(e) => Err(e),
    };

    match output {
        Ok(out) if out.status.success() => {
            let lc = parse_probe_output(&String::from_utf8_lossy(&out.stdout));
            let mem = lc.memory_gb.unwrap_or(0.0);
            let name = lc.name.clone().unwrap_or_else(|| "(unknown host)".to_string());
            let gpus = vec![GpuResult {
                name: format!(
                    "{}{}",
                    name,
                    lc.runtime_hint.as_ref().map(|r| format!(" — {}", r)).unwrap_or_default(),
                ),
                vram_gb: format!("{:.1}", mem),
            }];
            let error = if lc.kind == "cpu" && mem < 8.0 {
                Some("Connected, but this host has no GPU and not enough RAM for local AI.".to_string())
            } else {
                None
            };
            Ok(Json(TestGpuResponse { connected: true, gpus, error }))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let connected = !stderr.contains("Permission denied") && !stderr.contains("Connection refused");
            let error = if stderr.contains("Permission denied") {
                "SSH key not accepted. Make sure you completed Step 1 (copy the key) and Step 2 (paste it on the target computer).".to_string()
            } else if stderr.contains("Connection refused") {
                "SSH is not running on that computer. See the 'Don\'t have SSH set up?' section above.".to_string()
            } else if stderr.contains("No route") || stderr.contains("timed out") {
                "Could not reach that IP address. Make sure both computers are on the same network.".to_string()
            } else {
                format!("Connection issue: {}", stderr.chars().take(200).collect::<String>())
            };
            Ok(Json(TestGpuResponse { connected, gpus: Vec::new(), error: Some(error) }))
        }
        Err(e) => {
            Ok(Json(TestGpuResponse {
                connected: false,
                gpus: Vec::new(),
                error: Some(format!("Could not run SSH: {}", e)),
            }))
        }
    }
}

#[derive(Deserialize)]
pub struct TestGpuRequest {
    pub host: String,
    pub username: String,
}

#[derive(Serialize)]
pub struct TestGpuResponse {
    pub connected: bool,
    pub gpus: Vec<GpuResult>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct GpuResult {
    pub name: String,
    pub vram_gb: String,
}


/// POST /api/upload — upload a file for use in chat.
/// Saves to a temp dir and returns the file path + preview of content.
pub async fn handle_file_upload(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<FileUploadResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let upload_dir = crate::resolve_data_dir().join("uploads");
    let _ = std::fs::create_dir_all(&upload_dir);

    while let Ok(Some(field)) = multipart.next_field().await {
        let filename = field.file_name().unwrap_or("upload").to_string();
        let content_type = field.content_type().unwrap_or("application/octet-stream").to_string();

        let data = match field.bytes().await {
            Ok(d) => d,
            Err(e) => {
                return Ok(Json(FileUploadResponse {
                    success: false,
                    filename: filename.clone(),
                    path: None,
                    preview: None,
                    size_bytes: 0,
                    content_type: content_type.clone(),
                    error: Some(format!("Read error: {}", e)),
                }));
            }
        };

        let size = data.len();

        // Enforce upload size limit
        let max_bytes = state.config.security.max_upload_size_mb * 1024 * 1024;
        if size as u64 > max_bytes {
            return Ok(Json(FileUploadResponse {
                success: false,
                filename,
                path: None,
                preview: None,
                size_bytes: size,
                content_type,
                error: Some(format!("File exceeds {}MB upload limit", state.config.security.max_upload_size_mb)),
            }));
        }

        // Generate unique filename
        let ext = std::path::Path::new(&filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt");
        let unique_name = format!("{}-{}.{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S"),
            &uuid::Uuid::new_v4().to_string()[..8],
            ext
        );
        let file_path = upload_dir.join(&unique_name);

        if let Err(e) = std::fs::write(&file_path, &data) {
            return Ok(Json(FileUploadResponse {
                success: false,
                filename,
                path: None,
                preview: None,
                size_bytes: size,
                content_type,
                error: Some(format!("Write error: {}", e)),
            }));
        }

        // Generate preview
        let preview = generate_preview(&data, &content_type, &filename);

        log::info!("[upload] {} ({} bytes, {}) -> {}", filename, size, content_type, file_path.display());

        return Ok(Json(FileUploadResponse {
            success: true,
            filename,
            path: Some(file_path.to_string_lossy().to_string()),
            preview: Some(preview),
            size_bytes: size,
            content_type,
            error: None,
        }));
    }

    Ok(Json(FileUploadResponse {
        success: false,
        filename: String::new(),
        path: None,
        preview: None,
        size_bytes: 0,
        content_type: String::new(),
        error: Some("No file in request".to_string()),
    }))
}

fn generate_preview(data: &[u8], content_type: &str, filename: &str) -> String {
    // Text-based files: show first 2000 chars
    if content_type.starts_with("text/") 
        || filename.ends_with(".md") || filename.ends_with(".json")
        || filename.ends_with(".toml") || filename.ends_with(".yaml")
        || filename.ends_with(".yml") || filename.ends_with(".xml")
        || filename.ends_with(".csv") || filename.ends_with(".rs")
        || filename.ends_with(".py") || filename.ends_with(".js")
        || filename.ends_with(".ts") || filename.ends_with(".html")
        || filename.ends_with(".css") || filename.ends_with(".sh")
        || filename.ends_with(".sql") || filename.ends_with(".log")
    {
        let text = String::from_utf8_lossy(data);
        let preview: String = text.chars().take(2000).collect();
        if text.len() > 2000 {
            format!("{}\n\n[...truncated, {} total chars]", preview, text.len())
        } else {
            preview
        }
    } else if content_type == "application/pdf" || filename.ends_with(".pdf") {
        format!("[PDF file, {} bytes — content extraction available in chat]", data.len())
    } else if content_type.starts_with("image/") {
        format!("[Image: {} — visual analysis available in chat]", filename)
    } else {
        format!("[Binary file: {}, {} bytes]", filename, data.len())
    }
}

#[derive(Serialize)]
pub struct FileUploadResponse {
    pub success: bool,
    pub filename: String,
    pub path: Option<String>,
    pub preview: Option<String>,
    pub size_bytes: usize,
    pub content_type: String,
    pub error: Option<String>,
}

// ── Desktop Shortcut Installation ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct InstallShortcutRequest {
    pub target: String, // "menu" or "desktop"
}

#[derive(Serialize)]
pub struct InstallShortcutResponse {
    pub success: bool,
    pub message: String,
}

const SHORTCUT_ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#0ea5e9"/>
      <stop offset="100%" stop-color="#0369a1"/>
    </linearGradient>
  </defs>
  <rect width="64" height="64" rx="12" fill="#0a0a0a"/>
  <path d="M16 44 C16 38 20 34 26 34 L38 34 C44 34 48 38 48 44 L48 48 C48 52 44 52 42 48 L40 44 L38 48 C36 52 32 52 30 48 L28 44 L26 48 C24 52 20 52 18 48 L16 44Z" fill="url(#g)"/>
  <path d="M30 34 L30 20 C30 16 32 14 34 14 L34 14 C36 14 38 16 38 20 L38 34" fill="url(#g)"/>
  <circle cx="34" cy="11" r="5" fill="url(#g)"/>
  <path d="M36 20 L46 16 L48 14" stroke="url(#g)" stroke-width="2.5" stroke-linecap="round" fill="none"/>
  <path d="M16 38 C12 36 10 32 12 28" stroke="url(#g)" stroke-width="2" stroke-linecap="round" fill="none"/>
</svg>"##;

/// POST /api/settings/install-shortcut — create desktop or app-menu shortcut.
pub async fn handle_install_shortcut(
    axum::Json(req): axum::Json<InstallShortcutRequest>,
) -> axum::Json<InstallShortcutResponse> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dashboard_url = "http://localhost:18789";

    #[cfg(target_os = "linux")]
    {
        // Install icons — SVG for GTK desktops, PNG for KDE/XFCE
        let icon_base = format!("{}/.local/share/icons/hicolor", home);
        let svg_dir = format!("{}/scalable/apps", icon_base);
        let _ = std::fs::create_dir_all(&svg_dir);
        let _ = std::fs::write(format!("{}/syntaur.svg", svg_dir), SHORTCUT_ICON_SVG);

        // Generate PNG icons via Python (works everywhere, no extra deps)
        let _ = std::process::Command::new("python3")
            .arg("-c")
            .arg(format!(r#"
import struct, zlib, os
def create_png(size, path):
    raw = b''
    for y in range(size):
        raw += b'\x00'
        for x in range(size):
            t = y / size
            r = int(14 + t * (3 - 14))
            g = int(165 + t * (105 - 165))
            b = int(233 + t * (161 - 233))
            cx, cy = abs(x - size//2), abs(y - size//2)
            corner = size//2 - size//8
            if cx > corner and cy > corner:
                if ((cx - corner)**2 + (cy - corner)**2)**0.5 > size//8:
                    raw += bytes([0, 0, 0, 0]); continue
            raw += bytes([r, g, b, 255])
    def chunk(ct, d):
        c = ct + d
        return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    hdr = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', hdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
    with open(path, 'wb') as f: f.write(png)
for s in [48, 64, 128, 256]:
    d = os.path.expanduser('~/.local/share/icons/hicolor/{{}}x{{}}/apps'.format(s, s))
    os.makedirs(d, exist_ok=True)
    create_png(s, d + '/syntaur.png')
"#))
            .output();

        // Update icon caches (GTK and KDE)
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["-f", "-t", &icon_base])
            .output();
        let _ = std::process::Command::new("kbuildsycoca5").output();

        // Detect viewer binary location
        let exec_line = std::env::current_exe().ok()
            .and_then(|p| p.parent().map(|d| d.join("syntaur-viewer")))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| {
                // Only consider persistent install locations — never /tmp,
                // which clears on reboot and would silently break the shortcut.
                let candidate = format!("{}/.local/bin/syntaur-viewer", home);
                std::path::Path::new(&candidate).exists().then_some(candidate)
            })
            .unwrap_or_else(|| format!("xdg-open {}", dashboard_url));

        let desktop_content = format!(
            "[Desktop Entry]\nName=Syntaur\nComment=Your personal AI platform\nExec={}\nIcon=syntaur\nType=Application\nCategories=Utility;Development;\nStartupNotify=false\n",
            exec_line
        );

        if req.target == "desktop" {
            let desktop_dir = std::env::var("XDG_DESKTOP_DIR")
                .unwrap_or_else(|_| format!("{}/Desktop", home));
            let _ = std::fs::create_dir_all(&desktop_dir);
            let path = format!("{}/syntaur.desktop", desktop_dir);

            return match std::fs::write(&path, &desktop_content) {
                Ok(_) => {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));

                    // GNOME trust
                    let _ = std::process::Command::new("gio")
                        .args(["set", &path, "metadata::trusted", "true"])
                        .output();

                    // KDE Plasma trust — kioclient5 marks the file as trusted
                    let _ = std::process::Command::new("kioclient5")
                        .args(["exec", &path])
                        .env("QT_QPA_PLATFORM", "offscreen")
                        .output();

                    axum::Json(InstallShortcutResponse {
                        success: true,
                        message: "Desktop shortcut created — look for 'Syntaur' on your desktop.".into(),
                    })
                }
                Err(e) => axum::Json(InstallShortcutResponse {
                    success: false,
                    message: format!("Could not create desktop shortcut: {}", e),
                }),
            };
        } else {
            let app_dir = format!("{}/.local/share/applications", home);
            let _ = std::fs::create_dir_all(&app_dir);
            let path = format!("{}/syntaur.desktop", app_dir);

            return match std::fs::write(&path, &desktop_content) {
                Ok(_) => axum::Json(InstallShortcutResponse {
                    success: true,
                    message: "App launcher shortcut installed — find 'Syntaur' in your application menu.".into(),
                }),
                Err(e) => axum::Json(InstallShortcutResponse {
                    success: false,
                    message: format!("Could not create app shortcut: {}", e),
                }),
            };
        }
    }

    #[cfg(target_os = "macos")]
    {
        if req.target == "desktop" {
            let path = format!("{}/Desktop/Syntaur.webloc", home);
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                <plist version=\"1.0\"><dict><key>URL</key><string>{}</string></dict></plist>",
                dashboard_url
            );
            return match std::fs::write(&path, content) {
                Ok(_) => axum::Json(InstallShortcutResponse {
                    success: true,
                    message: "Desktop shortcut created — look for 'Syntaur' on your desktop.".into(),
                }),
                Err(e) => axum::Json(InstallShortcutResponse {
                    success: false,
                    message: format!("Could not create desktop shortcut: {}", e),
                }),
            };
        } else {
            // Create ~/Applications/Syntaur.app
            let app_path = format!("{}/Applications/Syntaur.app", home);
            let _ = std::fs::create_dir_all(format!("{}/Contents/MacOS", app_path));
            let _ = std::fs::create_dir_all(format!("{}/Contents/Resources", app_path));
            let _ = std::fs::write(
                format!("{}/Contents/Info.plist", app_path),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                    <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                    <plist version=\"1.0\"><dict>\
                    <key>CFBundleName</key><string>Syntaur</string>\
                    <key>CFBundleDisplayName</key><string>Syntaur</string>\
                    <key>CFBundleIdentifier</key><string>dev.syntaur.app</string>\
                    <key>CFBundleVersion</key><string>{}</string>\
                    <key>CFBundleExecutable</key><string>syntaur-open</string>\
                    <key>LSUIElement</key><true/>\
                    </dict></plist>",
                    env!("CARGO_PKG_VERSION")
                ),
            );
            let launcher = format!("{}/Contents/MacOS/syntaur-open", app_path);
            let _ = std::fs::write(&launcher, format!("#!/bin/sh\nopen \"{}\"\n", dashboard_url));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::write(format!("{}/Contents/Resources/icon.svg", app_path), SHORTCUT_ICON_SVG);

            return axum::Json(InstallShortcutResponse {
                success: true,
                message: "App installed to ~/Applications — find 'Syntaur' in Spotlight or Launchpad.".into(),
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        let target_dir = if req.target == "desktop" {
            std::env::var("USERPROFILE").unwrap_or_default() + "\\Desktop"
        } else {
            std::env::var("APPDATA").unwrap_or_default() + "\\Microsoft\\Windows\\Start Menu\\Programs"
        };

        // Use PowerShell to create .lnk via COM (the only reliable way on Windows)
        let ps_script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
            $s = $ws.CreateShortcut('{}\\Syntaur.lnk'); \
            $s.TargetPath = '{}'; \
            $s.Description = 'Syntaur - Your personal AI platform'; \
            $s.Save()",
            target_dir.replace('\'', "''"),
            dashboard_url,
        );

        return match std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_script])
            .output()
        {
            Ok(out) if out.status.success() => {
                let where_msg = if req.target == "desktop" { "your desktop" } else { "the Start Menu" };
                axum::Json(InstallShortcutResponse {
                    success: true,
                    message: format!("Shortcut created — find 'Syntaur' in {}.", where_msg),
                })
            }
            Ok(out) => axum::Json(InstallShortcutResponse {
                success: false,
                message: format!("PowerShell shortcut creation failed: {}", String::from_utf8_lossy(&out.stderr)),
            }),
            Err(e) => axum::Json(InstallShortcutResponse {
                success: false,
                message: format!("Could not run PowerShell: {}", e),
            }),
        };
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    axum::Json(InstallShortcutResponse {
        success: false,
        message: "Desktop shortcuts are not supported on this platform. Bookmark http://localhost:18789 in your browser instead.".into(),
    })
}

// ── LLM Provider Setup ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ProviderSaveRequest {
    pub token: String,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub api: String,
    pub model_id: String,
}

pub async fn handle_save_provider(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    axum::Json(req): axum::Json<ProviderSaveRequest>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let _ = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (axum::http::StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    // Read current config
    let config_path = state.config_path.clone();
    let mut config: serde_json::Value = {
        let text = std::fs::read_to_string(&config_path)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot read config: {}", e)))?;
        serde_json::from_str(&text)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot parse config: {}", e)))?
    };

    // Build provider entry
    let mut provider = serde_json::json!({
        "name": req.name,
        "url": req.base_url,
        "api": req.api,
        "model": req.model_id,
    });
    if let Some(key) = &req.api_key {
        if !key.is_empty() {
            provider["apiKey"] = serde_json::json!(key);
        }
    }

    // Ensure llm.providers array exists
    if config.get("llm").is_none() {
        config["llm"] = serde_json::json!({ "providers": [] });
    }
    if config["llm"].get("providers").is_none() {
        config["llm"]["providers"] = serde_json::json!([]);
    }

    // Check if provider with this name already exists — update it
    let providers = config["llm"]["providers"].as_array_mut()
        .ok_or((axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Invalid providers array".to_string()))?;
    let existing = providers.iter().position(|p| p.get("name").and_then(|n| n.as_str()) == Some(&req.name));
    if let Some(idx) = existing {
        providers[idx] = provider;
    } else {
        providers.push(provider);
    }

    // Write config back
    let text = serde_json::to_string_pretty(&config)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Serialize error: {}", e)))?;
    std::fs::write(&config_path, &text)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot write config: {}", e)))?;

    log::info!("[settings] Saved LLM provider '{}' ({})", req.name, req.model_id);

    Ok(axum::Json(serde_json::json!({
        "success": true,
        "message": format!("Provider '{}' saved. Restart the gateway to apply.", req.name),
    })))
}
