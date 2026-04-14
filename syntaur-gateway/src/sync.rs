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
    CalDav,       // URL + username + app-specific password (Apple/Nextcloud)
    Crypto,       // Wallet address + chain (public read-only)
    FileUpload,   // File drop (Apple Health export)
    StatusOnly,   // Read-only info card (NotebookLM auth, Vault health)
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
        // ── New Tier-1 providers ────────────────────────────────────────────
        ProviderDef {
            id: "github", name: "GitHub", category: "Developer",
            flow: FlowKind::ApiKey,
            instructions: "Paste a fine-grained Personal Access Token (classic PATs also work). Used to surface failing CI runs, open PRs, and unread notifications on your dashboard.",
            help_url: "https://github.com/settings/tokens?type=beta",
            scopes: &[("read", "Read repo status + notifications"), ("write", "Read + comment/close issues")],
        },
        ProviderDef {
            id: "home_assistant", name: "Home Assistant", category: "Smart Home",
            flow: FlowKind::ApiKey,
            instructions: "Paste your HA URL (e.g. http://homeassistant.local:8123) and a long-lived access token. Syntaur learns your devices and gives Peter voice full HA control.",
            help_url: "https://www.home-assistant.io/docs/authentication/#your-account-profile",
            scopes: &[],
        },
        ProviderDef {
            id: "plex", name: "Plex", category: "Media",
            flow: FlowKind::ApiKey,
            instructions: "Paste your plex.tv auth token and server URL. Peter can query 'what were we watching last night' and control playback.",
            help_url: "https://support.plex.tv/articles/204059436-finding-an-authentication-token-x-plex-token/",
            scopes: &[],
        },
        ProviderDef {
            id: "apple_calendar", name: "Apple Calendar (iCloud)", category: "Calendar",
            flow: FlowKind::CalDav,
            instructions: "Enter your iCloud account and an app-specific password (generate one at appleid.apple.com). Syntaur syncs your iCloud calendars alongside Google Calendar.",
            help_url: "https://support.apple.com/en-us/HT204397",
            scopes: &[("readonly", "Read-only"), ("events", "Read + write events")],
        },
        ProviderDef {
            id: "crypto_wallet", name: "Crypto Wallet (public)", category: "Finance",
            flow: FlowKind::Crypto,
            instructions: "Paste a public wallet address. Read-only — no keys needed. Tracks balance + transactions for tax reporting. Bitcoin, Ethereum, and Solana supported.",
            help_url: "",
            scopes: &[],
        },
        ProviderDef {
            id: "spotify", name: "Spotify", category: "Media",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Spotify. Peter can play your playlists, show recent tracks, and change what's playing on any Spotify Connect device.",
            help_url: "",
            scopes: &[("read", "Read-only (recent plays, playlists)"), ("control", "Read + playback control")],
        },
        ProviderDef {
            id: "youtube", name: "YouTube", category: "Social",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to manage your YouTube channel — uploads, comments, analytics. Used by Crimson Lantern publishing pipeline.",
            help_url: "",
            scopes: &[("read", "Read analytics + comments"), ("upload", "Read + upload + reply")],
        },
        ProviderDef {
            id: "strava", name: "Strava", category: "Health",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Strava. Syncs recent workouts so Peter can answer 'how was my run today'.",
            help_url: "",
            scopes: &[("read_all", "Read all activity")],
        },
        ProviderDef {
            id: "oura", name: "Oura Ring", category: "Health",
            flow: FlowKind::ApiKey,
            instructions: "Paste a Personal Access Token from cloud.ouraring.com. Syncs sleep, readiness, and activity data.",
            help_url: "https://cloud.ouraring.com/personal-access-tokens",
            scopes: &[],
        },
        ProviderDef {
            id: "anthropic_usage", name: "Anthropic API (usage)", category: "AI",
            flow: FlowKind::ApiKey,
            instructions: "Paste your Anthropic admin API key. Syntaur pulls daily spend + request counts for your dashboard.",
            help_url: "https://console.anthropic.com/settings/keys",
            scopes: &[],
        },
        ProviderDef {
            id: "openrouter_usage", name: "OpenRouter (usage)", category: "AI",
            flow: FlowKind::ApiKey,
            instructions: "Paste your OpenRouter API key. Pulls credit balance and daily usage breakdown.",
            help_url: "https://openrouter.ai/keys",
            scopes: &[],
        },
        ProviderDef {
            id: "apple_health", name: "Apple Health", category: "Health",
            flow: FlowKind::FileUpload,
            instructions: "Export your Health data from iPhone (Health app → profile → Export All Health Data), then upload the export.zip here. Peter gets full access to your sleep, heart rate, and workouts.",
            help_url: "https://support.apple.com/en-us/HT203037",
            scopes: &[],
        },
        ProviderDef {
            id: "notebooklm_status", name: "NotebookLM Auth", category: "AI",
            flow: FlowKind::StatusOnly,
            instructions: "Status of the one-time Google auth for NotebookLM research. Refreshes every 2-4 weeks via 'notebooklm login' on the gaming PC.",
            help_url: "",
            scopes: &[],
        },
        ProviderDef {
            id: "vault_health", name: "Vault Health", category: "Storage",
            flow: FlowKind::StatusOnly,
            instructions: "NFS-mounted Obsidian vault shared between claudevm and gaming PC. Shows mount status, last-write time, and read/write latency.",
            help_url: "",
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
            FlowKind::CalDav => "caldav",
            FlowKind::Crypto => "crypto",
            FlowKind::FileUpload => "file_upload",
            FlowKind::StatusOnly => "status",
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
        "github" => {
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() { return Err("API key required".to_string()); }
            let resp = client.get("https://api.github.com/user")
                .header("Authorization", format!("Bearer {}", key))
                .header("User-Agent", "syntaur")
                .header("Accept", "application/vnd.github+json")
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("GitHub API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("GitHub rejected token ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let login = j.get("login").and_then(|v| v.as_str()).unwrap_or("?");
            Ok(format!("@{}", login))
        }
        "home_assistant" => {
            let url = credential.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let token = credential.get("token").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() || token.is_empty() { return Err("URL and token required".to_string()); }
            let api_url = format!("{}/api/", url.trim_end_matches('/'));
            let resp = client.get(&api_url)
                .header("Authorization", format!("Bearer {}", token))
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("HA API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("HA rejected token ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let msg = j.get("message").and_then(|v| v.as_str()).unwrap_or("API reachable");
            Ok(msg.to_string())
        }
        "plex" => {
            let token = credential.get("token").and_then(|v| v.as_str()).unwrap_or("");
            if token.is_empty() { return Err("Plex token required".to_string()); }
            // Test against plex.tv — works without needing a server URL
            let resp = client.get("https://plex.tv/api/v2/user")
                .header("X-Plex-Token", token)
                .header("Accept", "application/json")
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Plex API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Plex rejected token ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let uname = j.get("username").and_then(|v| v.as_str()).unwrap_or("account");
            Ok(format!("Verified: {}", uname))
        }
        "apple_calendar" => {
            let url = credential.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let user = credential.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let pw = credential.get("password").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() || user.is_empty() || pw.is_empty() {
                return Err("URL, username, and app-specific password required".to_string());
            }
            // iCloud CalDAV: PROPFIND against principal URL
            let effective = if url.is_empty() || url == "auto" {
                "https://caldav.icloud.com/.well-known/caldav".to_string()
            } else { url.to_string() };
            let resp = client.request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap_or(reqwest::Method::GET),
                &effective)
                .basic_auth(user, Some(pw))
                .header("Depth", "0")
                .header("Content-Type", "application/xml")
                .body(r#"<?xml version="1.0"?><propfind xmlns="DAV:"><prop><current-user-principal/></prop></propfind>"#)
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("CalDAV: {}", e))?;
            let status = resp.status();
            if !(status == reqwest::StatusCode::MULTI_STATUS || status.is_success()) {
                return Err(format!("CalDAV rejected ({})", status));
            }
            Ok("Verified".to_string())
        }
        "crypto_wallet" => {
            let address = credential.get("address").and_then(|v| v.as_str()).unwrap_or("");
            let chain = credential.get("chain").and_then(|v| v.as_str()).unwrap_or("btc");
            if address.is_empty() { return Err("Wallet address required".to_string()); }
            match chain {
                "btc" => {
                    let u = format!("https://blockstream.info/api/address/{}", address);
                    let r = client.get(&u).timeout(Duration::from_secs(15)).send().await
                        .map_err(|e| format!("Blockstream: {}", e))?;
                    if !r.status().is_success() { return Err(format!("Invalid BTC address ({})", r.status())); }
                    let j: serde_json::Value = r.json().await.unwrap_or_default();
                    let funded = j.get("chain_stats").and_then(|s| s.get("funded_txo_sum")).and_then(|v| v.as_i64()).unwrap_or(0);
                    let spent = j.get("chain_stats").and_then(|s| s.get("spent_txo_sum")).and_then(|v| v.as_i64()).unwrap_or(0);
                    let sats = funded - spent;
                    Ok(format!("{:.8} BTC", sats as f64 / 100_000_000.0))
                }
                "eth" => {
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "eth_getBalance",
                        "params": [address, "latest"]
                    });
                    let r = client.post("https://ethereum-rpc.publicnode.com")
                        .json(&body).timeout(Duration::from_secs(15)).send().await
                        .map_err(|e| format!("ETH RPC: {}", e))?;
                    if !r.status().is_success() { return Err(format!("ETH RPC ({})", r.status())); }
                    let j: serde_json::Value = r.json().await.unwrap_or_default();
                    let hex = j.get("result").and_then(|v| v.as_str()).unwrap_or("0x0");
                    let wei = i128::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap_or(0);
                    Ok(format!("{:.6} ETH", wei as f64 / 1e18))
                }
                "sol" => {
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "getBalance",
                        "params": [address]
                    });
                    let r = client.post("https://api.mainnet-beta.solana.com")
                        .json(&body).timeout(Duration::from_secs(15)).send().await
                        .map_err(|e| format!("Solana RPC: {}", e))?;
                    if !r.status().is_success() { return Err(format!("SOL RPC ({})", r.status())); }
                    let j: serde_json::Value = r.json().await.unwrap_or_default();
                    let lamports = j.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_i64()).unwrap_or(0);
                    Ok(format!("{:.6} SOL", lamports as f64 / 1e9))
                }
                _ => Err(format!("Unsupported chain: {}", chain)),
            }
        }
        "oura" => {
            let token = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            if token.is_empty() { return Err("Personal Access Token required".to_string()); }
            let resp = client.get("https://api.ouraring.com/v2/usercollection/personal_info")
                .header("Authorization", format!("Bearer {}", token))
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Oura API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Oura rejected token ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.unwrap_or_default();
            let email = j.get("email").and_then(|v| v.as_str()).unwrap_or("account");
            Ok(format!("Verified: {}", email))
        }
        "anthropic_usage" => {
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() { return Err("API key required".to_string()); }
            let resp = client.get("https://api.anthropic.com/v1/models")
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("Anthropic API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("Anthropic rejected key ({})", resp.status()));
            }
            Ok("Verified".to_string())
        }
        "openrouter_usage" => {
            let key = credential.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() { return Err("API key required".to_string()); }
            let resp = client.get("https://openrouter.ai/api/v1/auth/key")
                .header("Authorization", format!("Bearer {}", key))
                .timeout(Duration::from_secs(15)).send().await
                .map_err(|e| format!("OpenRouter API: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("OpenRouter rejected key ({})", resp.status()));
            }
            let j: serde_json::Value = resp.json().await.unwrap_or_default();
            let limit = j.get("data").and_then(|d| d.get("limit")).and_then(|v| v.as_f64());
            let usage = j.get("data").and_then(|d| d.get("usage")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let info = match limit {
                Some(l) => format!("${:.2} / ${:.2} used", usage, l),
                None => format!("${:.2} used (no limit)", usage),
            };
            Ok(info)
        }
        "apple_health" => {
            // File upload provider — credential is just a marker that a file was uploaded
            let path = credential.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() { return Err("Upload required".to_string()); }
            Ok("File received".to_string())
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


// ── Apple Health file upload ────────────────────────────────────────────────

pub async fn handle_health_upload(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    if body.is_empty() { return Err(axum::http::StatusCode::BAD_REQUEST); }
    if body.len() > 500 * 1024 * 1024 { // 500 MB cap
        return Err(axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    // Store under ~/.syntaur/uploads/health/{uid}/export-{ts}.zip (or .xml)
    let data_dir = crate::resolve_data_dir();
    let upload_dir = data_dir.join("uploads").join("health").join(uid.to_string());
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        warn!("[health-upload] mkdir failed: {}", e);
        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
    let filename = format!("export-{}.bin", chrono::Utc::now().timestamp());
    let fpath = upload_dir.join(&filename);
    let path_str = fpath.to_string_lossy().to_string();
    if let Err(e) = std::fs::write(&fpath, &body) {
        warn!("[health-upload] write failed: {}", e);
        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Quick sanity check — is this actually a Health export?
    // Apple exports as .zip containing export.xml. We accept both.
    let file_size = body.len();
    let mut detected_kind = "unknown";
    let head: &[u8] = &body[..body.len().min(8)];
    if head.starts_with(b"PK") { detected_kind = "zip"; }
    else if head.starts_with(b"<?xml") || head.starts_with(b"<Health") { detected_kind = "xml"; }

    // Save as sync_connection
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let credential = serde_json::json!({"file_path": path_str, "size": file_size, "kind": detected_kind});
    let credential_json = serde_json::to_string(&credential).unwrap_or_default();
    let display_name = format!("{} ({} MB)", filename, file_size / 1_048_576);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sync_connections (user_id, provider, display_name, credential, status, created_at, updated_at, last_check_at)
             VALUES (?, 'apple_health', ?, ?, 'active', ?, ?, ?)
             ON CONFLICT(user_id, provider) DO UPDATE SET
               display_name = excluded.display_name,
               credential = excluded.credential,
               status = 'active',
               last_error = NULL,
               updated_at = excluded.updated_at,
               last_check_at = excluded.last_check_at",
            rusqlite::params![uid, display_name, credential_json, now, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("[health-upload] stored {} bytes for user {} ({})", file_size, uid, detected_kind);
    Ok(Json(serde_json::json!({
        "success": true,
        "size": file_size,
        "kind": detected_kind,
        "path": path_str,
    })))
}

// ── Status-only cards ───────────────────────────────────────────────────────

pub async fn handle_notebooklm_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;

    // Check common notebooklm-py storage_state.json locations
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let candidates = [
        format!("{}/.notebooklm/storage_state.json", home),
        format!("{}/notebooklm-py/storage_state.json", home),
        format!("{}/.config/notebooklm/storage_state.json", home),
        "/home/sean/notebooklm-py/storage_state.json".to_string(),
    ];

    for path in &candidates {
        if let Ok(meta) = std::fs::metadata(path) {
            let modified_secs = meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let now = chrono::Utc::now().timestamp();
            let age_days = (now - modified_secs) / 86400;
            // Google session cookies last ~14 days typical — warn past 10
            let status = if age_days > 14 { "stale" }
                         else if age_days > 10 { "warning" }
                         else { "ok" };
            return Ok(Json(serde_json::json!({
                "connected": true,
                "status": status,
                "path": path,
                "age_days": age_days,
                "last_login": modified_secs,
                "hint": "Run `notebooklm login` on gaming PC to refresh",
            })));
        }
    }

    Ok(Json(serde_json::json!({
        "connected": false,
        "status": "not_configured",
        "hint": "Install notebooklm-py and run `notebooklm login` on gaming PC",
    })))
}

pub async fn handle_vault_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;

    let vault_path = "/home/sean/vault";
    let check_start = std::time::Instant::now();

    let mounted = std::path::Path::new(vault_path).exists();
    let mut last_write: Option<i64> = None;
    let mut file_count = 0;
    let mut read_latency_ms = 0_u128;

    if mounted {
        // Read latency: stat a known file
        let memory_md = format!("{}/MEMORY.md", vault_path);
        let t0 = std::time::Instant::now();
        if let Ok(meta) = std::fs::metadata(&memory_md) {
            read_latency_ms = t0.elapsed().as_millis();
            if let Ok(m) = meta.modified() {
                if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                    last_write = Some(d.as_secs() as i64);
                }
            }
        }
        // Count files (quick scan — shallow)
        if let Ok(entries) = std::fs::read_dir(format!("{}/projects", vault_path)) {
            file_count = entries.count();
        }
    }

    // Write latency: touch a test file in vault
    let mut write_latency_ms = 0_u128;
    let mut writable = false;
    if mounted {
        let test_path = format!("{}/.health-check", vault_path);
        let t0 = std::time::Instant::now();
        if std::fs::write(&test_path, chrono::Utc::now().timestamp().to_string().as_bytes()).is_ok() {
            write_latency_ms = t0.elapsed().as_millis();
            writable = true;
            let _ = std::fs::remove_file(&test_path);
        }
    }

    let status = if !mounted { "offline" }
                 else if !writable { "read_only" }
                 else if read_latency_ms > 500 || write_latency_ms > 1000 { "degraded" }
                 else { "healthy" };

    Ok(Json(serde_json::json!({
        "connected": mounted,
        "status": status,
        "mounted": mounted,
        "writable": writable,
        "path": vault_path,
        "projects_count": file_count,
        "last_write_at": last_write,
        "read_latency_ms": read_latency_ms,
        "write_latency_ms": write_latency_ms,
        "check_took_ms": check_start.elapsed().as_millis() as u64,
    })))
}

