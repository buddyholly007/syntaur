//! Unified sync connections — one API for all third-party integrations.
//!
//! Providers are stored in `sync_connections` with a generic
//! `credential` field (JSON) and `metadata` (JSON). Each provider has
//! a test probe to verify the credential is still valid; failed probes
//! flip `status` to `needs_reconnect` so the UI can surface them.
//!
//! Auto-renew:
//! - OAuth providers (gmail, google_calendar): use refresh_token when
//!   `expires_at` is within the next hour. Reuses `oauth_tokens` table.
//! - API-key providers (stripe, bluesky, alpaca): health-check daily.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, response::Json};
use log::{info, warn};

use crate::AppState;

// ── Provider catalog ────────────────────────────────────────────────────────

/// Kind of setup flow a provider uses.
#[derive(Debug, Clone, Copy)]
pub enum FlowKind {
    Oauth,        // OAuth2 redirect (handled by /api/oauth/start)
    ApiKey,       // Single API key or key+secret
    UrlOnly,      // Just a URL (ICS subscription)
    Pairing,      // Short code + QR (Telegram)
    LinkSdk,      // External SDK handles it (Plaid Link)
}

pub struct ProviderDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str, // "Email", "Calendar", "Finance", "Social", "Messaging"
    pub flow: FlowKind,
    pub instructions: &'static str,
    pub help_url: &'static str,
    pub scopes: &'static [(&'static str, &'static str)], // (id, label) for per-scope toggles
}

pub fn catalog() -> Vec<ProviderDef> {
    vec![
        ProviderDef {
            id: "gmail", name: "Gmail", category: "Email",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to let Syntaur read your inbox for receipts and confirmations.",
            help_url: "",
            scopes: &[("readonly", "Read-only"), ("modify", "Read + organize")],
        },
        ProviderDef {
            id: "google_calendar", name: "Google Calendar", category: "Calendar",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to sync your calendar events.",
            help_url: "",
            scopes: &[("readonly", "Read-only"), ("events", "Read + write events")],
        },
        ProviderDef {
            id: "ics_subscription", name: "ICS / Web Calendar", category: "Calendar",
            flow: FlowKind::UrlOnly,
            instructions: "Paste any .ics URL (iCloud, Google Calendar share link, webcal://). Syntaur fetches events every hour.",
            help_url: "https://support.google.com/calendar/answer/37648",
            scopes: &[],
        },
        ProviderDef {
            id: "telegram", name: "Telegram", category: "Messaging",
            flow: FlowKind::Pairing,
            instructions: "Scan the QR with your phone — it opens a chat with the Syntaur bot. Tap START to link your account.",
            help_url: "",
            scopes: &[],
        },
        ProviderDef {
            id: "stripe", name: "Stripe", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste a restricted (read-only) secret key. Used to pull receipts and payout data for taxes.",
            help_url: "https://dashboard.stripe.com/apikeys",
            scopes: &[],
        },
        ProviderDef {
            id: "bluesky", name: "Bluesky", category: "Social",
            flow: FlowKind::ApiKey,
            instructions: "Enter your handle and an app password (not your main password).",
            help_url: "https://bsky.app/settings/app-passwords",
            scopes: &[],
        },
        ProviderDef {
            id: "plaid", name: "Plaid (Banks)", category: "Finance",
            flow: FlowKind::LinkSdk,
            instructions: "Launch Plaid Link to connect 12,000+ banks. Pulls transactions and balances for taxes.",
            help_url: "",
            scopes: &[],
        },
        ProviderDef {
            id: "simplefin", name: "SimpleFIN", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste your SimpleFIN setup token. Aggregates multiple bank feeds into one.",
            help_url: "https://beta-bridge.simplefin.org/",
            scopes: &[],
        },
        ProviderDef {
            id: "alpaca", name: "Alpaca (Broker)", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste your Alpaca API key and secret. Tracks portfolio holdings and trade activity.",
            help_url: "https://app.alpaca.markets/paper/dashboard/overview",
            scopes: &[],
        },
        ProviderDef {
            id: "coinbase", name: "Coinbase", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste a read-only Coinbase API key and secret. Pulls crypto holdings and transactions.",
            help_url: "https://www.coinbase.com/settings/api",
            scopes: &[],
        },
    ]
}

// ── Request types ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SyncConnectRequest {
    pub token: String,
    pub provider: String,
    pub credential: serde_json::Value,
    pub metadata: Option<serde_json::Value>,
    pub display_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct SyncTestRequest {
    pub token: String,
    pub provider: String,
    pub credential: serde_json::Value,
}

#[derive(serde::Deserialize)]
pub struct TelegramPairRequest {
    pub token: String,
}

// ── Handlers ────────────────────────────────────────────────────────────────

pub async fn handle_sync_providers(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let connections: std::collections::HashMap<String, serde_json::Value> =
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut map = std::collections::HashMap::new();

            // sync_connections
            let mut stmt = conn.prepare(
                "SELECT provider, display_name, status, last_sync_at, last_check_at, last_error, metadata \
                 FROM sync_connections WHERE user_id = ?"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?, r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))).map_err(|e| e.to_string())?;
            for r in rows.flatten() {
                let (provider, display_name, status, last_sync, last_check, last_error, metadata_json) = r;
                let meta: serde_json::Value = metadata_json
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);
                map.insert(provider.clone(), serde_json::json!({
                    "connected": true,
                    "display_name": display_name,
                    "status": status,
                    "last_sync_at": last_sync,
                    "last_check_at": last_check,
                    "last_error": last_error,
                    "metadata": meta,
                }));
            }

            // connected_accounts (Plaid/SimpleFIN)
            let mut stmt = conn.prepare(
                "SELECT provider, institution_name, status, last_sync_at, error \
                 FROM connected_accounts WHERE user_id = ? AND status != 'disconnected'"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))).map_err(|e| e.to_string())?;
            for r in rows.flatten() {
                let (provider, inst, status, last_sync, err) = r;
                map.entry(provider.clone()).or_insert(serde_json::json!({
                    "connected": true,
                    "display_name": inst,
                    "status": status,
                    "last_sync_at": last_sync,
                    "last_error": err,
                }));
            }

            // investment_accounts (Alpaca/Coinbase)
            let mut stmt = conn.prepare(
                "SELECT broker, nickname, status, last_sync_at \
                 FROM investment_accounts WHERE user_id = ? AND status != 'disconnected'"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
            ))).map_err(|e| e.to_string())?;
            for r in rows.flatten() {
                let (broker, nick, status, last_sync) = r;
                map.entry(broker.clone()).or_insert(serde_json::json!({
                    "connected": true,
                    "display_name": nick,
                    "status": status,
                    "last_sync_at": last_sync,
                }));
            }

            // email_connections (Gmail)
            let mut stmt = conn.prepare(
                "SELECT provider, email_address, status, last_scan_at \
                 FROM email_connections WHERE user_id = ? AND status != 'disconnected'"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
            ))).map_err(|e| e.to_string())?;
            for r in rows.flatten() {
                let (provider, email, status, last_sync) = r;
                map.entry(provider.clone()).or_insert(serde_json::json!({
                    "connected": true,
                    "display_name": email,
                    "status": status,
                    "last_sync_at": last_sync,
                }));
            }

            // user_telegram_links (Telegram)
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM user_telegram_links WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)
            ).unwrap_or(0);
            if count > 0 {
                map.entry("telegram".to_string()).or_insert(serde_json::json!({
                    "connected": true,
                    "display_name": format!("{} chat(s) linked", count),
                    "status": "active",
                }));
            }

            Ok(map)
        }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // Merge with catalog
    let providers: Vec<serde_json::Value> = catalog().into_iter().map(|p| {
        let flow = match p.flow {
            FlowKind::Oauth => "oauth",
            FlowKind::ApiKey => "api_key",
            FlowKind::UrlOnly => "url",
            FlowKind::Pairing => "pairing",
            FlowKind::LinkSdk => "link_sdk",
        };
        let mut entry = serde_json::json!({
            "id": p.id,
            "name": p.name,
            "category": p.category,
            "flow": flow,
            "instructions": p.instructions,
            "help_url": p.help_url,
            "scopes": p.scopes.iter().map(|(id, label)| serde_json::json!({"id": id, "label": label})).collect::<Vec<_>>(),
            "connected": false,
        });
        if let Some(c) = connections.get(p.id) {
            if let serde_json::Value::Object(src) = c {
                if let serde_json::Value::Object(dst) = &mut entry {
                    for (k, v) in src { dst.insert(k.clone(), v.clone()); }
                }
            }
        }
        entry
    }).collect();

    Ok(Json(serde_json::json!({ "providers": providers })))
}

pub async fn handle_sync_connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SyncConnectRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Validate provider exists in catalog
    let valid = catalog().iter().any(|p| p.id == req.provider);
    if !valid { return Err(axum::http::StatusCode::BAD_REQUEST); }

    // Test the credential before saving (best-effort)
    let test_result = test_credential(&req.provider, &req.credential, &state.client).await;
    if let Err(msg) = &test_result {
        warn!("[sync] credential test failed for {}: {}", req.provider, msg);
        return Ok(Json(serde_json::json!({
            "success": false,
            "error": msg,
        })));
    }

    let now = chrono::Utc::now().timestamp();
    let provider = req.provider.clone();
    let credential_json = serde_json::to_string(&req.credential).unwrap_or_default();
    let metadata_json = req.metadata.as_ref().map(|m| serde_json::to_string(m).unwrap_or_default());
    let display_name = req.display_name.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sync_connections (user_id, provider, display_name, credential, metadata, status, created_at, updated_at, last_check_at)
             VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?)
             ON CONFLICT(user_id, provider) DO UPDATE SET
               display_name = excluded.display_name,
               credential = excluded.credential,
               metadata = excluded.metadata,
               status = 'active',
               last_error = NULL,
               updated_at = excluded.updated_at,
               last_check_at = excluded.last_check_at",
            rusqlite::params![uid, provider, display_name, credential_json, metadata_json, now, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("[sync] connected provider={} user={}", req.provider, uid);
    Ok(Json(serde_json::json!({ "success": true, "provider": req.provider })))
}

pub async fn handle_sync_disconnect(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(provider): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let _ = conn.execute("DELETE FROM sync_connections WHERE user_id = ? AND provider = ?",
                             rusqlite::params![uid, &provider]);
        // Also clear oauth_tokens, email_connections, connected_accounts, investment_accounts
        let _ = conn.execute("DELETE FROM oauth_tokens WHERE user_id = ? AND provider = ?",
                             rusqlite::params![uid, &provider]);
        let _ = conn.execute("UPDATE email_connections SET status='disconnected' WHERE user_id = ? AND provider = ?",
                             rusqlite::params![uid, &provider]);
        let _ = conn.execute("UPDATE connected_accounts SET status='disconnected' WHERE user_id = ? AND provider = ?",
                             rusqlite::params![uid, &provider]);
        let _ = conn.execute("UPDATE investment_accounts SET status='disconnected' WHERE user_id = ? AND broker = ?",
                             rusqlite::params![uid, &provider]);
        if provider == "telegram" {
            let _ = conn.execute("DELETE FROM user_telegram_links WHERE user_id = ?", rusqlite::params![uid]);
        }
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn handle_sync_test(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SyncTestRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _ = crate::resolve_principal(&state, &req.token).await?;
    let res = test_credential(&req.provider, &req.credential, &state.client).await;
    match res {
        Ok(info) => Ok(Json(serde_json::json!({ "ok": true, "info": info }))),
        Err(msg) => Ok(Json(serde_json::json!({ "ok": false, "error": msg }))),
    }
}

// ── Credential probes ───────────────────────────────────────────────────────

async fn test_credential(
    provider: &str,
    credential: &serde_json::Value,
    client: &reqwest::Client,
) -> Result<String, String> {
    match provider {
        "ics_subscription" => {
            let url = credential.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() { return Err("URL required".to_string()); }
            // Accept webcal:// → https://
            let fetch_url = if url.starts_with("webcal://") {
                format!("https://{}", &url[9..])
            } else { url.to_string() };
            let resp = client.get(&fetch_url).timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("fetch failed: {}", e))?;
            if !resp.status().is_success() { return Err(format!("HTTP {}", resp.status())); }
            let body = resp.text().await.unwrap_or_default();
            if !body.contains("BEGIN:VCALENDAR") { return Err("not a valid ICS feed".to_string()); }
            let count = body.matches("BEGIN:VEVENT").count();
            Ok(format!("{} events found", count))
        }
        "stripe" => {
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() { return Err("API key required".to_string()); }
            let resp = client.get("https://api.stripe.com/v1/account")
                .bearer_auth(key).timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Stripe API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Stripe rejected key ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let email = j.get("email").and_then(|v| v.as_str()).unwrap_or("account");
            Ok(format!("Verified: {}", email))
        }
        "bluesky" => {
            let handle = credential.get("handle").and_then(|v| v.as_str()).unwrap_or("");
            let pw = credential.get("app_password").and_then(|v| v.as_str()).unwrap_or("");
            if handle.is_empty() || pw.is_empty() { return Err("handle and app password required".to_string()); }
            let resp = client.post("https://bsky.social/xrpc/com.atproto.server.createSession")
                .json(&serde_json::json!({"identifier": handle, "password": pw}))
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Bluesky API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Bluesky rejected credentials ({})", resp.status()));
            }
            Ok(format!("Verified: @{}", handle))
        }
        "alpaca" => {
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            let sec = credential.get("api_secret").and_then(|v| v.as_str()).unwrap_or("");
            let base = credential.get("base_url").and_then(|v| v.as_str())
                .unwrap_or("https://api.alpaca.markets");
            if key.is_empty() || sec.is_empty() { return Err("API key and secret required".to_string()); }
            let url = format!("{}/v2/account", base);
            let resp = client.get(&url)
                .header("APCA-API-KEY-ID", key).header("APCA-API-SECRET-KEY", sec)
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Alpaca API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Alpaca rejected key ({})", resp.status()));
            }
            Ok("Verified".to_string())
        }
        "coinbase" => {
            // Coinbase Advanced Trade API requires HMAC signing — skip live test, just accept.
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            let sec = credential.get("api_secret").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() || sec.is_empty() { return Err("API key and secret required".to_string()); }
            Ok("Saved (live test skipped — HMAC-signed)".to_string())
        }
        "simplefin" => {
            let token = credential.get("setup_token").and_then(|v| v.as_str()).unwrap_or("");
            if token.is_empty() { return Err("setup token required".to_string()); }
            Ok("Saved".to_string())
        }
        // OAuth and pairing providers don't use this endpoint (they save via their own flows)
        _ => Ok("Saved".to_string()),
    }
}

// ── Telegram pairing ────────────────────────────────────────────────────────

pub async fn handle_telegram_pair_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TelegramPairRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Generate 8-char alphanumeric code
    let code: String = (0..8).map(|_| {
        let c = (rand::random::<u8>() % 36) as u8;
        if c < 10 { (b'0' + c) as char } else { (b'a' + c - 10) as char }
    }).collect();

    let bot_token = state.config.channels.telegram.bot_token.clone();
    if bot_token.is_empty() {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    let now = chrono::Utc::now().timestamp();
    let expires = now + 600; // 10 minutes
    let code_clone = code.clone();
    let bot_clone = bot_token.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Clean expired codes
        let _ = conn.execute("DELETE FROM telegram_pairings WHERE expires_at < ?", rusqlite::params![now]);
        conn.execute(
            "INSERT INTO telegram_pairings (user_id, code, bot_token, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![uid, code_clone, bot_clone, expires, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // Look up bot username
    let bot_username = get_bot_username(&bot_token, &state.client).await
        .unwrap_or_else(|_| "ClaudeGamingPC_bot".to_string());
    let deep_link = format!("https://t.me/{}?start={}", bot_username, code);

    Ok(Json(serde_json::json!({
        "code": code,
        "deep_link": deep_link,
        "expires_at": expires,
        "qr_url": format!("https://api.qrserver.com/v1/create-qr-code/?size=200x200&data={}",
                          url_encode(&deep_link)),
    })))
}

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            (b as char).to_string()
        } else {
            format!("%{:02X}", b)
        }
    }).collect()
}

async fn get_bot_username(token: &str, client: &reqwest::Client) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{}/getMe", token);
    let resp = client.get(&url).timeout(Duration::from_secs(10)).send().await
        .map_err(|e| e.to_string())?;
    let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    j.get("result").and_then(|r| r.get("username")).and_then(|u| u.as_str())
        .map(|s| s.to_string()).ok_or("no username".to_string())
}

/// Poll endpoint — UI checks if pairing was consumed. Returns {paired:true} when user has a link.
pub async fn handle_telegram_pair_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let code = params.get("code").cloned().unwrap_or_default();
    let db = state.db_path.clone();

    let (paired, chat_count): (bool, i64) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let consumed: Option<i64> = conn.query_row(
            "SELECT consumed_at FROM telegram_pairings WHERE code = ? AND user_id = ?",
            rusqlite::params![code, uid], |r| r.get(0)
        ).ok().flatten();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM user_telegram_links WHERE user_id = ?",
            rusqlite::params![uid], |r| r.get(0)
        ).unwrap_or(0);
        Ok((consumed.is_some(), count))
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "paired": paired,
        "chat_count": chat_count,
    })))
}

// ── Auto-renew background task ──────────────────────────────────────────────

pub fn spawn_sync_renewal_task(state: Arc<AppState>) {
    tokio::spawn(async move {
        info!("[sync-renewal] background task started (5min OAuth refresh, daily API-key health check)");
        let mut oauth_interval = tokio::time::interval(Duration::from_secs(300));
        let mut health_interval = tokio::time::interval(Duration::from_secs(3600)); // hourly — check if it's been 24h
        oauth_interval.tick().await; // skip first
        health_interval.tick().await;
        loop {
            tokio::select! {
                _ = oauth_interval.tick() => {
                    if let Err(e) = oauth_refresh_tick(&state).await {
                        warn!("[sync-renewal] oauth tick failed: {}", e);
                    }
                }
                _ = health_interval.tick() => {
                    if let Err(e) = health_check_tick(&state).await {
                        warn!("[sync-renewal] health tick failed: {}", e);
                    }
                }
            }
        }
    });
}

async fn oauth_refresh_tick(state: &Arc<AppState>) -> Result<(), String> {
    // Find oauth_tokens expiring in next hour with a refresh_token
    let now = chrono::Utc::now().timestamp();
    let cutoff = now + 3600;
    let db = state.db_path.clone();

    let to_refresh: Vec<(i64, String, String)> = tokio::task::spawn_blocking(move || -> Result<Vec<_>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT user_id, provider, refresh_token FROM oauth_tokens \
             WHERE refresh_token IS NOT NULL AND refresh_token != '' \
               AND expires_at IS NOT NULL AND expires_at < ? AND expires_at > 0"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![cutoff], |r| Ok((
            r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
        ))).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }).await.map_err(|e| e.to_string())??;

    for (uid, provider, refresh_token) in to_refresh {
        let Some(provider_cfg) = state.config.oauth.providers.get(&provider) else { continue };
        let res = refresh_oauth_token(
            &state.client,
            &provider_cfg.token_url,
            &provider_cfg.client_id,
            &provider_cfg.client_secret,
            &refresh_token,
        ).await;
        match res {
            Ok((new_access, new_refresh, new_expires)) => {
                let db = state.db_path.clone();
                let prov = provider.clone();
                let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                    let refresh = new_refresh.unwrap_or(refresh_token);
                    conn.execute(
                        "UPDATE oauth_tokens SET access_token = ?, refresh_token = ?, expires_at = ?, updated_at = ? WHERE user_id = ? AND provider = ?",
                        rusqlite::params![new_access, refresh, new_expires, now, uid, prov],
                    ).map_err(|e| e.to_string())?;
                    Ok(())
                }).await;
                info!("[sync-renewal] refreshed {} for user {}", provider, uid);
            }
            Err(e) => {
                warn!("[sync-renewal] refresh failed for {}/{}: {}", provider, uid, e);
                // Mark sync_connection as needs_reconnect
                let db = state.db_path.clone();
                let prov = provider.clone();
                let err = e.clone();
                let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                    let _ = conn.execute(
                        "UPDATE sync_connections SET status='needs_reconnect', last_error=?, last_check_at=? WHERE user_id = ? AND provider = ?",
                        rusqlite::params![err, now, uid, prov],
                    );
                    Ok(())
                }).await;
            }
        }
    }
    Ok(())
}

async fn refresh_oauth_token(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<(String, Option<String>, i64), String> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];
    let resp = client.post(token_url).form(&form).timeout(Duration::from_secs(15)).send().await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("token endpoint returned {}", resp.status()));
    }
    let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let access = j.get("access_token").and_then(|v| v.as_str()).ok_or("no access_token")?.to_string();
    let new_refresh = j.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
    let expires_in = j.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(3600);
    Ok((access, new_refresh, chrono::Utc::now().timestamp() + expires_in))
}

async fn health_check_tick(state: &Arc<AppState>) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    let day_ago = now - 86400;
    let db = state.db_path.clone();

    let to_check: Vec<(i64, i64, String, String)> = tokio::task::spawn_blocking(move || -> Result<Vec<_>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, provider, credential FROM sync_connections \
             WHERE status='active' AND (last_check_at IS NULL OR last_check_at < ?)"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![day_ago], |r| Ok((
            r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?,
        ))).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }).await.map_err(|e| e.to_string())??;

    for (id, _uid, provider, credential_json) in to_check {
        let credential: serde_json::Value = serde_json::from_str(&credential_json).unwrap_or_default();
        let result = test_credential(&provider, &credential, &state.client).await;
        let (status, error): (&str, Option<String>) = match &result {
            Ok(_) => ("active", None),
            Err(e) => ("needs_reconnect", Some(e.clone())),
        };
        let db = state.db_path.clone();
        let status_s = status.to_string();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let _ = conn.execute(
                "UPDATE sync_connections SET status=?, last_check_at=?, last_error=? WHERE id=?",
                rusqlite::params![status_s, now, error, id],
            );
            Ok(())
        }).await;
    }
    Ok(())
}
