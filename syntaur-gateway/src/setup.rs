//! `/api/setup/*` endpoints for first-run configuration.
//!
//! These endpoints are called by the installer and the dashboard's
//! setup wizard. They handle LLM connection testing, Telegram pairing,
//! and initial config generation.
//!
//! Setup endpoints are available without authentication when no admin
//! user exists yet (first-run). After setup completes, they require
//! admin auth like other `/api/admin/*` endpoints.

use std::sync::Arc;

use axum::extract::State;
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


/// GET /setup — serve the setup wizard HTML page.
pub async fn handle_setup_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/setup.html"))
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
        || path == "/manifest.json"
        || path == "/tailwind.js"
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


/// GET / — serve the dashboard HTML page.
pub async fn handle_dashboard() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/dashboard.html"))
}


/// POST /api/auth/login — exchange password or token for a valid API token.
/// Tries: gateway password, gateway token, user API token.
pub async fn handle_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // Try 1: check if it's a valid user API token (ocp_*)
    if let Ok(Some(resolved)) = state.users.resolve_token(&req.password).await {
        return Ok(Json(LoginResponse {
            success: true,
            token: Some(req.password.clone()),
            error: None,
        }));
    }

    // Try 2: check gateway password (constant-time comparison)
    let gw_auth = &state.config.gateway.auth;
    let password_match = gw_auth.extra.get("password")
        .and_then(|v| v.as_str())
        .map(|p| constant_time_eq(p.as_bytes(), req.password.as_bytes()))
        .unwrap_or(false);

    // Try 3: check gateway token directly (constant-time comparison)
    let token_match = constant_time_eq(gw_auth.token.as_bytes(), req.password.as_bytes());

    if password_match || token_match {
        // Mint a session token for the first user (or create one)
        if let Ok(users) = state.users.list_users().await {
            if let Some(user) = users.first() {
                if let Ok(token) = state.users.mint_token(user.id, "dashboard-session").await {
                    return Ok(Json(LoginResponse {
                        success: true,
                        token: Some(token),
                        error: None,
                    }));
                }
            }
        }
        // Fallback to gateway token (works when no users exist)
        return Ok(Json(LoginResponse {
            success: true,
            token: Some(gw_auth.token.clone()),
            error: None,
        }));
    }

    Ok(Json(LoginResponse {
        success: false,
        token: None,
        error: Some("Invalid password".to_string()),
    }))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
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




pub async fn handle_music_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/music.html"))
}

/// GET /settings
pub async fn handle_settings_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/settings.html"))
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


/// POST /api/setup/apply — apply the full setup configuration.
/// Writes config file + agent workspace from installer choices.
pub async fn handle_setup_apply(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetupApplyRequest>,
) -> Result<Json<SetupApplyResponse>, StatusCode> {
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

/// Constant-time byte comparison to prevent timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

/// GET /chat
pub async fn handle_chat_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/chat.html"))
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

/// GET /agent-avatar/{agent_id} — serve custom agent avatar or default
pub async fn handle_agent_avatar(
    axum::extract::Path(agent_id): axum::extract::Path<String>,
) -> (axum::http::HeaderMap, Vec<u8>) {
    let mut h = axum::http::HeaderMap::new();
    h.insert("cache-control", "public, max-age=300".parse().unwrap());

    // Check for custom avatar on disk
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let custom_path = format!("{}/.syntaur/avatars/{}.jpg", home, agent_id);
    if let Ok(data) = std::fs::read(&custom_path) {
        h.insert("content-type", "image/jpeg".parse().unwrap());
        return (h, data);
    }
    let custom_png = format!("{}/.syntaur/avatars/{}.png", home, agent_id);
    if let Ok(data) = std::fs::read(&custom_png) {
        h.insert("content-type", "image/png".parse().unwrap());
        return (h, data);
    }

    // Default: serve the app icon
    h.insert("content-type", "image/png".parse().unwrap());
    (h, include_bytes!("../static/avatar.png").to_vec())
}

/// POST /api/agent-avatar/{agent_id} — upload custom agent avatar
pub async fn handle_agent_avatar_upload(
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
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

/// GET /tax — tax & expenses page
pub async fn handle_tax_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../static/tax.html"))
}

/// GET /tailwind.js — bundled Tailwind CSS (no CDN dependency)
pub async fn handle_tailwind() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    headers.insert("cache-control", "public, max-age=604800".parse().unwrap());
    (headers, include_str!("../static/tailwind.js"))
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
    let cpu_model = read_cpu_model();
    let (ram_total, ram_avail) = read_ram();
    let gpu = detect_gpu();
    let disk_free = read_disk_free();
    let tier = classify_hw_tier(&gpu.1, ram_total);

    // Scan network for GPUs and LLM services
    let (network_gpus, gpu_scan_blocked) = scan_network_gpus().await;
    let network_llms = scan_network_llms().await;

    // Upgrade tier if we found network GPUs or LLMs
    let effective_tier = if !network_gpus.is_empty() {
        let best_vram: f64 = network_gpus.iter()
            .filter_map(|g| g.vram_gb.parse::<f64>().ok())
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0.0);
        if best_vram >= 16.0 { "Powerful (network GPU)".to_string() }
        else if best_vram >= 8.0 { "Capable (network GPU)".to_string() }
        else { "Limited (network GPU)".to_string() }
    } else if !network_llms.is_empty() {
        "Network LLM available".to_string()
    } else {
        tier
    };

    Ok(Json(HardwareScanResponse {
        cpu: cpu_model,
        ram_total_gb: format!("{:.1}", ram_total as f64 / 1024.0),
        ram_available_gb: format!("{:.1}", ram_avail as f64 / 1024.0),
        gpu_name: gpu.0,
        gpu_vram_gb: gpu.1,
        disk_free_gb: disk_free,
        tier: effective_tier,
        network_llms,
        network_gpus,
        gpu_scan_blocked,
        local_ip: detect_local_ip(),
    }))
}

#[derive(Serialize)]
pub struct HardwareScanResponse {
    pub cpu: String,
    pub ram_total_gb: String,
    pub ram_available_gb: String,
    pub gpu_name: Option<String>,
    pub gpu_vram_gb: Option<String>,
    pub disk_free_gb: String,
    pub tier: String,
    pub network_llms: Vec<NetworkLlmInfo>,
    pub network_gpus: Vec<NetworkGpuInfo>,
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

#[derive(Serialize)]
pub struct NetworkGpuInfo {
    pub host: String,
    pub name: String,
    pub vram_gb: String,
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

fn detect_gpu() -> (Option<String>, Option<String>) {
    if let Ok(out) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().next() {
                let parts: Vec<&str> = line.splitn(2, ',').map(|s| s.trim()).collect();
                if parts.len() == 2 {
                    let vram_mb: f64 = parts[1].parse().unwrap_or(0.0);
                    return (
                        Some(parts[0].to_string()),
                        Some(format!("{:.1}", vram_mb / 1024.0)),
                    );
                }
            }
        }
    }
    (None, None)
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

fn classify_hw_tier(gpu_vram: &Option<String>, ram_mb: u64) -> String {
    if let Some(vram) = gpu_vram {
        let gb: f64 = vram.parse().unwrap_or(0.0);
        if gb >= 16.0 { return "Powerful".to_string(); }
        if gb >= 8.0 { return "Capable".to_string(); }
        if gb >= 4.0 { return "Limited".to_string(); }
    }
    if ram_mb >= 8000 { return "CPU-only".to_string(); }
    "Minimal".to_string()
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


/// Try to detect GPU on a remote host by checking if it responds to
/// nvidia-smi style info. We check a known endpoint pattern or fall back
/// to inferring from the model being served.
async fn detect_remote_gpu(client: &reqwest::Client, host: &str) -> (Option<String>, Option<String>) {
    // Try llama.cpp /props endpoint which sometimes has system info
    // For now, use a simpler heuristic: if the host is serving a large model
    // (e.g., 27B+), it likely has a beefy GPU
    
    // Try nvidia-smi via a simple HTTP wrapper if available
    // This would be a custom endpoint; for now check nvidia-smi locally
    // if the service is on localhost
    if host == "127.0.0.1" || host == "localhost" {
        return detect_gpu(); // Use the existing local detection
    }

    // For remote hosts, try to query GPU info via a lightweight endpoint
    // Many LLM servers expose /v1/system or similar
    if let Ok(resp) = client.get(&format!("http://{}:3000/api/gpu", host)).send().await {
        if let Ok(body) = resp.json::<serde_json::Value>().await {
            let name = body.get("name").and_then(|v| v.as_str()).map(String::from);
            let vram = body.get("vram_gb").and_then(|v| v.as_str()).map(String::from);
            if name.is_some() { return (name, vram); }
        }
    }

    // Heuristic: if serving a 27B+ model, likely has 24GB+ GPU
    // We already have model info from the caller, but we don't pass it here
    // So just return None and let the UI show the model info instead
    (None, None)
}


/// Scan the local network for NVIDIA GPUs by trying to SSH or probe known ports.
/// Returns (gpus_found, was_blocked).
async fn scan_network_gpus() -> (Vec<NetworkGpuInfo>, bool) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build() {
        Ok(c) => c,
        Err(_) => return (Vec::new(), false),
    };

    let subnet = get_subnet_prefix();
    let mut found = Vec::new();
    let mut blocked = false;
    let mut handles = Vec::new();

    // Scan for NVIDIA GPU management on common discovery methods:
    // 1. NVIDIA DCGM exporter (port 9400) - common in GPU servers
    // 2. node_exporter with GPU metrics (port 9100)
    // 3. Try SSH with key auth to run nvidia-smi
    for i in 1..=254u8 {
        let ip = format!("{}.{}", subnet, i);
        let client_c = client.clone();
        handles.push(tokio::spawn(async move {
            // Method 1: Try SSH nvidia-smi (most reliable)
            if let Ok(output) = tokio::process::Command::new("ssh")
                .args([
                    "-o", "ConnectTimeout=1",
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "BatchMode=yes",
                    &format!("sean@{}", ip),
                    "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits"
                ])
                .output()
                .await
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut gpus = Vec::new();
                    for line in stdout.lines() {
                        let parts: Vec<&str> = line.splitn(2, ',').map(|s| s.trim()).collect();
                        if parts.len() == 2 {
                            let vram_mb: f64 = parts[1].parse().unwrap_or(0.0);
                            gpus.push(NetworkGpuInfo {
                                host: ip.clone(),
                                name: parts[0].to_string(),
                                vram_gb: format!("{:.1}", vram_mb / 1024.0),
                            });
                        }
                    }
                    if !gpus.is_empty() {
                        return (gpus, false);
                    }
                }
                // SSH connected but nvidia-smi not found = no GPU on that host
                return (Vec::new(), false);
            }
            // SSH failed = might be blocked
            (Vec::new(), true)
        }));
    }

    let mut any_blocked = false;
    for handle in handles {
        if let Ok((gpus, was_blocked)) = handle.await {
            found.extend(gpus);
            if was_blocked { any_blocked = true; }
        }
    }

    // Only report blocked if we found NO gpus at all and some hosts were unreachable
    blocked = found.is_empty() && any_blocked;

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

/// POST /api/setup/test-gpu — test SSH connection to a remote host and scan for GPUs.
pub async fn handle_test_gpu(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TestGpuRequest>,
) -> Result<Json<TestGpuResponse>, StatusCode> {
    require_setup_auth(&state, &extract_token_from_headers(&headers)).await?;
    let output = tokio::process::Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=5",
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            &format!("{}@{}", req.username, req.host),
            "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits"
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let gpus: Vec<GpuResult> = stdout.lines().filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, ',').map(|s| s.trim()).collect();
                if parts.len() == 2 {
                    let vram_mb: f64 = parts[1].parse().unwrap_or(0.0);
                    Some(GpuResult {
                        name: parts[0].to_string(),
                        vram_gb: format!("{:.1}", vram_mb / 1024.0),
                    })
                } else {
                    None
                }
            }).collect();

            Ok(Json(TestGpuResponse {
                connected: true,
                gpus,
                error: None,
            }))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let connected = !stderr.contains("Permission denied") && !stderr.contains("Connection refused");
            let error = if stderr.contains("Permission denied") {
                "SSH key not accepted. Make sure you completed Step 1 (copy the key) and Step 2 (paste it on the GPU computer).".to_string()
            } else if stderr.contains("Connection refused") {
                "SSH is not running on that computer. See the 'Don\'t have SSH set up?' section above.".to_string()
            } else if stderr.contains("No route") || stderr.contains("timed out") {
                "Could not reach that IP address. Make sure both computers are on the same network.".to_string()
            } else if stderr.contains("command not found") || stderr.contains("not found") {
                // Connected but no nvidia-smi
                "Connected successfully but nvidia-smi was not found — no NVIDIA GPU on that computer.".to_string()
            } else {
                format!("Connection issue: {}", stderr.chars().take(200).collect::<String>())
            };
            Ok(Json(TestGpuResponse {
                connected,
                gpus: Vec::new(),
                error: Some(error),
            }))
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
                let candidates = [
                    format!("{}/.local/bin/syntaur-viewer", home),
                    "/tmp/syntaur-viewer".to_string(),
                ];
                candidates.into_iter().find(|p| std::path::Path::new(p).exists())
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
