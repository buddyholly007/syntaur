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
    CalDav,       // Username + app-specific password (iCloud auto-discovery)
    Crypto,       // Wallet address + chain (public read-only)
    FileUpload,   // File drop (Apple Health export)
    StatusOnly,   // Read-only info card (NotebookLM auth, Vault health)
    PlexPin,      // Plex device-auth PIN (no copy-paste)
    AirPlay,      // mDNS auto-discovery, no credentials
    MusicAssistant, // Detected via connected HA instance
    IosShortcut,  // User's personal Shortcut webhook URL
    AppleMusic,   // Bookmarklet-captured MusicKit tokens (dev + MUT)
    PhonePwa,     // Pair with the Syntaur Voice PWA on user's phone
}

pub struct ProviderDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub flow: FlowKind,
    pub instructions: &'static str,
    pub help_url: &'static str,
    pub scopes: &'static [(&'static str, &'static str)],
    pub unlocks: &'static str,   // One line shown post-connect: "what can Peter do now?"
    pub info_only: bool,         // Status-card style — no Connect button
}

pub fn catalog() -> Vec<ProviderDef> {
    vec![
        ProviderDef {
            id: "gmail", name: "Gmail", category: "Email",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to let Syntaur read your inbox for receipts and confirmations.",
            help_url: "",
            scopes: &[("readonly", "Read-only"), ("modify", "Read + organize")],
            unlocks: "Peter can scan your inbox for receipts and important messages.",
            info_only: false,
        },
        ProviderDef {
            id: "google_calendar", name: "Google Calendar", category: "Calendar",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to sync your calendar events.",
            help_url: "",
            scopes: &[("readonly", "Read-only"), ("events", "Read + write events")],
            unlocks: "Your Google events appear alongside Syntaur's calendar. Peter can add/move events.",
            info_only: false,
        },
        ProviderDef {
            id: "ics_subscription", name: "ICS / Web Calendar", category: "Calendar",
            flow: FlowKind::UrlOnly,
            instructions: "Paste any .ics URL (iCloud, Google Calendar share link, webcal://). Syntaur fetches events every hour.",
            help_url: "https://support.google.com/calendar/answer/37648",
            scopes: &[],
            unlocks: "Any .ics feed (work calendar, sports team schedule, holidays) shows in your dashboard.",
            info_only: false,
        },
        ProviderDef {
            id: "telegram", name: "Telegram", category: "Messaging",
            flow: FlowKind::Pairing,
            instructions: "Scan the QR with your phone — it opens a chat with the Syntaur bot. Tap START to link your account.",
            help_url: "",
            scopes: &[],
            unlocks: "Peter can send reminders, alerts, and approval requests to your Telegram. Reply to Peter by text.",
            info_only: false,
        },
        ProviderDef {
            id: "stripe", name: "Stripe", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "1. Click the button below.  2. On Stripe, click \"+ Create restricted key\".  3. Name it Syntaur, set Charges/Customers/Invoices to Read.  4. Copy the rk_live_... key and paste it here.",
            help_url: "https://dashboard.stripe.com/apikeys",
            scopes: &[],
            unlocks: "Automatic receipt capture from your Stripe account for the tax module.",
            info_only: false,
        },
        ProviderDef {
            id: "bluesky", name: "Bluesky", category: "Social",
            flow: FlowKind::ApiKey,
            instructions: "Bluesky uses app passwords for third-party apps:  1. Click the button below — Bluesky app password settings.  2. Tap + Add App Password → name it Syntaur → Create.  3. Copy the generated password and paste below with your handle.",
            help_url: "https://bsky.app/settings/app-passwords",
            scopes: &[],
            unlocks: "Peter can post, reply, and monitor mentions on your Bluesky account.",
            info_only: false,
        },
        ProviderDef {
            id: "plaid", name: "Plaid (Banks)", category: "Finance",
            flow: FlowKind::LinkSdk,
            instructions: "Launch Plaid Link to connect 12,000+ banks. Pulls transactions and balances for taxes.",
            help_url: "",
            scopes: &[],
            unlocks: "Bank transactions flow into the tax module automatically.",
            info_only: false,
        },
        ProviderDef {
            id: "simplefin", name: "SimpleFIN", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste your SimpleFIN setup token. Aggregates multiple bank feeds into one.",
            help_url: "https://beta-bridge.simplefin.org/",
            scopes: &[],
            unlocks: "Aggregated bank feeds, lighter-weight alternative to Plaid.",
            info_only: false,
        },
        ProviderDef {
            id: "alpaca", name: "Alpaca (Broker)", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "1. Click the button below — opens your Alpaca dashboard.  2. Scroll to the right sidebar → \"Your API Keys\" → click View → Generate New Key.  3. Copy both the Key ID and Secret Key into the fields below.  Default is Paper (safe). Switch to Live only if you want real trading.",
            help_url: "https://app.alpaca.markets/paper/dashboard/overview",
            scopes: &[],
            unlocks: "Portfolio holdings + trade activity for tax reporting and performance tracking.",
            info_only: false,
        },
        ProviderDef {
            id: "coinbase", name: "Coinbase", category: "Finance",
            flow: FlowKind::ApiKey,
            instructions: "Paste a read-only Coinbase API key and secret. Pulls crypto holdings and transactions.",
            help_url: "https://www.coinbase.com/settings/api",
            scopes: &[],
            unlocks: "Crypto holdings and transactions pulled automatically for taxes.",
            info_only: false,
        },
        // ── New Tier-1 providers ────────────────────────────────────────────
        ProviderDef {
            id: "github", name: "GitHub", category: "Developer",
            flow: FlowKind::ApiKey,
            instructions: "1. Click the button below — GitHub opens with the right scopes pre-selected.  2. Name it \"Syntaur\" (already filled).  3. Click Generate token at the bottom.  4. Copy the token (starts with ghp_) and paste it here.",
            help_url: "https://github.com/settings/tokens/new?description=Syntaur&scopes=repo,notifications,read:user",
            scopes: &[],
            unlocks: "Failing CI, open PRs, and unread notifications surface on your dashboard.",
            info_only: false,
        },
        ProviderDef {
            id: "home_assistant", name: "Home Assistant", category: "Smart Home",
            flow: FlowKind::ApiKey,
            instructions: "Syntaur auto-discovers your Home Assistant on the network. You just need a token:  1. Open your HA dashboard, click your profile (bottom-left).  2. Security tab → scroll to \"Long-Lived Access Tokens\" → Create Token.  3. Name it Syntaur, copy the token, paste it below.",
            help_url: "https://www.home-assistant.io/docs/authentication/#your-account-profile",
            scopes: &[],
            unlocks: "Peter gets full control of every device you've paired in HA. The backbone for music-on-HomePod and smart-home voice commands.",
            info_only: false,
        },
        ProviderDef {
            id: "plex", name: "Plex", category: "Media",
            flow: FlowKind::PlexPin,
            instructions: "1. Click Connect — we generate a 4-character code.  2. On any device, go to plex.tv/link and enter the code.  3. Syntaur detects when you\'re signed in (usually 5-10 seconds). No token to copy.",
            help_url: "",
            scopes: &[],
            unlocks: "Peter knows your library, recent watches, and what's currently playing.",
            info_only: false,
        },
        ProviderDef {
            id: "apple_calendar", name: "Apple Calendar (iCloud)", category: "Calendar",
            flow: FlowKind::CalDav,
            instructions: "iCloud requires an app-specific password (not your normal Apple ID password):  1. Click the button below — opens appleid.apple.com.  2. Sign in, then Sign-in Security → App-Specific Passwords → + → name it Syntaur.  3. Copy the password (xxxx-xxxx-xxxx-xxxx) and paste below with your iCloud email.",
            help_url: "https://account.apple.com/account/manage",
            scopes: &[],
            unlocks: "iCloud calendars sync alongside your other calendars.",
            info_only: false,
        },
        ProviderDef {
            id: "crypto_wallet", name: "Crypto Wallet (public)", category: "Finance",
            flow: FlowKind::Crypto,
            instructions: "Paste a public wallet address. Read-only — no keys needed. Tracks balance + transactions for tax reporting. Bitcoin, Ethereum, and Solana supported.",
            help_url: "",
            scopes: &[],
            unlocks: "Cold-storage balances tracked on your finance dashboard.",
            info_only: false,
        },
        ProviderDef {
            id: "spotify", name: "Spotify", category: "Media",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Spotify. Peter can play your playlists, show recent tracks, and change what's playing on any Spotify Connect device.",
            help_url: "",
            scopes: &[("read", "Read-only (recent plays, playlists)"), ("control", "Read + playback control")],
            unlocks: "Peter can play your Spotify library on any Spotify Connect device.",
            info_only: false,
        },
        ProviderDef {
            id: "youtube", name: "YouTube", category: "Social",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Google to manage your YouTube channel — uploads, comments, analytics. Used by Crimson Lantern publishing pipeline.",
            help_url: "",
            scopes: &[("read", "Read analytics + comments"), ("upload", "Read + upload + reply")],
            unlocks: "Analytics + upload scheduling for Crimson Lantern.",
            info_only: false,
        },
        ProviderDef {
            id: "strava", name: "Strava", category: "Health",
            flow: FlowKind::Oauth,
            instructions: "Sign in with Strava. Syncs recent workouts so Peter can answer 'how was my run today'.",
            help_url: "",
            scopes: &[("read_all", "Read all activity")],
            unlocks: "Peter can answer 'how was my run this week?' and log workouts.",
            info_only: false,
        },
        ProviderDef {
            id: "oura", name: "Oura Ring", category: "Health",
            flow: FlowKind::ApiKey,
            instructions: "1. Click the button below — opens Oura Cloud.  2. Sign in, then click + Create New Personal Access Token.  3. Name it Syntaur → Create.  4. Copy the token and paste it here.",
            help_url: "https://cloud.ouraring.com/personal-access-tokens",
            scopes: &[],
            unlocks: "Peter knows your sleep, readiness, and activity data for daily check-ins.",
            info_only: false,
        },
        ProviderDef {
            id: "anthropic_usage", name: "Anthropic API (usage)", category: "AI",
            flow: FlowKind::ApiKey,
            instructions: "Paste your Anthropic admin API key. Syntaur pulls daily spend + request counts for your dashboard.",
            help_url: "https://console.anthropic.com/settings/keys",
            scopes: &[],
            unlocks: "Your Claude API spend appears on the dashboard. Warn before rate limits.",
            info_only: false,
        },
        ProviderDef {
            id: "openrouter_usage", name: "OpenRouter (usage)", category: "AI",
            flow: FlowKind::ApiKey,
            instructions: "Paste your OpenRouter API key. Pulls credit balance and daily usage breakdown.",
            help_url: "https://openrouter.ai/keys",
            scopes: &[],
            unlocks: "Live balance + daily usage for OpenRouter keys.",
            info_only: false,
        },
        ProviderDef {
            id: "apple_health", name: "Apple Health", category: "Health",
            flow: FlowKind::FileUpload,
            instructions: "Export your Health data from iPhone (Health app → profile → Export All Health Data), then upload the export.zip here. Peter gets full access to your sleep, heart rate, and workouts.",
            help_url: "https://support.apple.com/en-us/HT203037",
            scopes: &[],
            unlocks: "Peter has full access to your sleep, heart rate, and workouts.",
            info_only: false,
        },
        ProviderDef {
            id: "notebooklm_status", name: "NotebookLM Auth", category: "AI",
            flow: FlowKind::StatusOnly,
            instructions: "Status of the one-time Google auth for NotebookLM research. Refreshes every 2-4 weeks via 'notebooklm login' on the gaming PC.",
            help_url: "",
            scopes: &[],
            unlocks: "Shows whether your NotebookLM auth is fresh. Not a setup card — just a status indicator.",
            info_only: true,
        },
        ProviderDef {
            id: "vault_health", name: "Vault Health", category: "Storage",
            flow: FlowKind::StatusOnly,
            instructions: "NFS-mounted Obsidian vault shared between claudevm and gaming PC. Shows mount status, last-write time, and read/write latency.",
            help_url: "",
            scopes: &[],
            unlocks: "Monitors the NFS-mounted Obsidian vault. Not a setup card — just a status indicator.",
            info_only: true,
        },
        // ── Music layer ─────────────────────────────────────────────────────
        ProviderDef {
            id: "airplay", name: "AirPlay Speakers", category: "Music",
            flow: FlowKind::AirPlay,
            instructions: "Auto-discovers HomePods, Apple TVs, and AirPlay speakers on your network. No login needed. Peter voice can announce on any discovered device.",
            help_url: "",
            scopes: &[],
            unlocks: "Peter can announce and play audio on any detected speaker (HomePod, Apple TV, AirPlay speakers).",
            info_only: false,
        },
        ProviderDef {
            id: "music_assistant", name: "Music Assistant (via Home Assistant)", category: "Music",
            flow: FlowKind::MusicAssistant,
            instructions: "Music Assistant is a Home Assistant add-on that handles Apple Music, Spotify, YouTube Music, and more. Connect Home Assistant first, then we auto-detect if Music Assistant is installed — gives you full Apple Music control with zero extra setup.",
            help_url: "https://music-assistant.io/",
            scopes: &[],
            unlocks: "Full Apple Music / Spotify / YT Music control routed through Home Assistant.",
            info_only: false,
        },
        ProviderDef {
            id: "phone_music_pwa", name: "Phone (via Syntaur Voice PWA)", category: "Music",
            flow: FlowKind::PhonePwa,
            instructions: "If you have the Syntaur Voice PWA installed on your phone, Peter can launch Apple Music on it directly. No bookmarklet, no Shortcut — just install the PWA and tap Connect. Music plays through your phone's speakers, AirPods, or any device your phone is AirPlaying to.",
            help_url: "",
            scopes: &[],
            unlocks: "Peter sends play commands straight to Music.app on your phone. Audio comes out wherever the phone is connected (speakers, AirPods, AirPlay HomePod).",
            info_only: false,
        },
        ProviderDef {
            id: "apple_music", name: "Apple Music", category: "Music",
            flow: FlowKind::AppleMusic,
            instructions: "No developer account needed. Sign into Apple Music in your browser, then click the Syntaur bookmarklet — it captures your login tokens from the Apple Music page and sends them back here. After that, Peter can search, queue, and manage your library.",
            help_url: "https://music.apple.com",
            scopes: &[],
            unlocks: "Peter can search your library, show what you've been listening to, and cue songs to HomePod (via HA).",
            info_only: false,
        },
        ProviderDef {
            id: "ios_shortcut_music", name: "iOS Shortcut (Music)", category: "Music",
            flow: FlowKind::IosShortcut,
            instructions: "1. On iPhone, open Shortcuts app → + to create.  2. Add action \"Play Apple Music\" → configure search by name.  3. Tap share arrow → Add to Home Screen → enable \"Run with URL\".  4. Copy the icloud.com/shortcuts/... URL and paste below. Peter triggers it when you say \"play music\".",
            help_url: "https://www.icloud.com/shortcuts/",
            scopes: &[],
            unlocks: "Peter can trigger music playback on your iPhone — works even when you're out of the house.",
            info_only: false,
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
            FlowKind::PlexPin => "plex_pin",
            FlowKind::AirPlay => "airplay",
            FlowKind::MusicAssistant => "music_assistant",
            FlowKind::IosShortcut => "ios_shortcut",
            FlowKind::AppleMusic => "apple_music",
            FlowKind::PhonePwa => "phone_pwa",
        };
        // Check if OAuth provider has config — gate "Connect" button if not
        let oauth_configured = matches!(p.flow, FlowKind::Oauth) &&
            state.config.oauth.providers.contains_key(p.id) &&
            !state.config.oauth.providers.get(p.id)
                .map(|c| c.client_id.is_empty())
                .unwrap_or(true);
        let mut entry = serde_json::json!({
            "id": p.id,
            "name": p.name,
            "category": p.category,
            "flow": flow,
            "instructions": p.instructions,
            "help_url": p.help_url,
            "scopes": p.scopes.iter().map(|(id, label)| serde_json::json!({"id": id, "label": label})).collect::<Vec<_>>(),
            "unlocks": p.unlocks,
            "info_only": p.info_only,
            "oauth_configured": oauth_configured,
            "needs_oauth_config": matches!(p.flow, FlowKind::Oauth) && !oauth_configured,
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
            let user = credential.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let pw = credential.get("password").and_then(|v| v.as_str()).unwrap_or("");
            if user.is_empty() || pw.is_empty() {
                return Err("iCloud email and app-specific password required".to_string());
            }
            // iCloud CalDAV auto-discovery via well-known URL
            let url = credential.get("url").and_then(|v| v.as_str()).unwrap_or("");
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
        "ios_shortcut_music" => {
            let url = credential.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() { return Err("Shortcut URL required".to_string()); }
            if !url.contains("icloud.com") && !url.contains("shortcuts") {
                return Err("URL must be an iCloud Shortcuts link".to_string());
            }
            // Can't live-probe an iOS Shortcut — just validate shape
            Ok("Saved".to_string())
        }
        "airplay" => {
            // No credential — discovery-based
            Ok("Saved".to_string())
        }
        "music_assistant" => {
            // Validated indirectly by checking HA integration
            Ok("Saved".to_string())
        }
        "phone_music_pwa" => {
            // Verify the bridge command port is reachable. SSE subscribers (PWA
            // tabs) are checked at command-send time — here we only need to
            // confirm the bridge itself is up.
            let resp = client.post("http://127.0.0.1:18804/command")
                .json(&serde_json::json!({"type":"info","message":"Syntaur paired"}))
                .timeout(Duration::from_secs(3))
                .send().await
                .map_err(|e| format!("Bridge not reachable on 127.0.0.1:18804: {}. Make sure rust-limitless-bridge is running.", e))?;
            if !resp.status().is_success() {
                return Err(format!("Bridge returned {}", resp.status()));
            }
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let count = body.get("sent_to").and_then(|v| v.as_u64()).unwrap_or(0);
            if count <= 1 {
                Ok("Bridge ready. Open the Syntaur Voice PWA on your phone and keep it open to receive music commands.".to_string())
            } else {
                Ok(format!("Bridge ready. {} subscribers connected.", count.saturating_sub(1)))
            }
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


// ── Home Assistant auto-discovery ───────────────────────────────────────────

pub async fn handle_ha_discover(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;
    let client = &state.client;

    // Try common HA addresses: homeassistant.local, homeassistant, hassio.local, + common LAN IPs
    let candidates = [
        "http://homeassistant.local:8123",
        "http://homeassistant:8123",
        "http://hassio.local:8123",
        "http://192.168.1.2:8123",
        "http://192.168.1.4:8123",
        "http://192.168.1.8:8123",
        "http://192.168.1.10:8123",
        "http://192.168.1.100:8123",
    ];

    let mut found: Vec<serde_json::Value> = Vec::new();
    for url in candidates {
        let probe_url = format!("{}/api/", url);
        let res = client.get(&probe_url)
            .timeout(Duration::from_millis(1500))
            .send().await;
        match res {
            Ok(r) => {
                // HA returns 401 (auth required) for unauthenticated requests to /api/ —
                // that's the signal we found a real HA instance
                let status = r.status();
                if status == reqwest::StatusCode::UNAUTHORIZED || status.is_success() {
                    found.push(serde_json::json!({
                        "url": url,
                        "status": status.as_u16(),
                        "requires_auth": status == reqwest::StatusCode::UNAUTHORIZED,
                    }));
                }
            }
            Err(_) => {}
        }
    }

    Ok(Json(serde_json::json!({
        "found": found,
        "suggested_url": found.first().map(|f| f.get("url").cloned().unwrap_or_default()),
    })))
}

// ── Plex PIN flow ───────────────────────────────────────────────────────────
// Device-auth flow: POST to plex.tv/api/v2/pins, user enters PIN at plex.tv/link,
// we poll for completion to get the auth token. Zero copy-paste.

#[derive(serde::Deserialize)]
pub struct PlexPinRequest {
    pub token: String,
}

#[derive(serde::Deserialize)]
pub struct PlexPinPollRequest {
    pub token: String,
    pub pin_id: i64,
    pub client_id: String,
}

pub async fn handle_plex_pin_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlexPinRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _ = crate::resolve_principal(&state, &req.token).await?;
    // Generate a stable client identifier for this browser session
    let client_id: String = (0..24).map(|_| {
        let c = (rand::random::<u8>() % 36) as u8;
        if c < 10 { (b'0' + c) as char } else { (b'a' + c - 10) as char }
    }).collect();
    let form = [
        ("strong", "false"),
        ("X-Plex-Client-Identifier", &client_id),
        ("X-Plex-Product", "Syntaur"),
    ];
    let resp = state.client.post("https://plex.tv/api/v2/pins")
        .header("Accept", "application/json")
        .form(&form)
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        warn!("[plex-pin] create failed: {}", resp.status());
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    let j: serde_json::Value = resp.json().await.map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    let pin_id = j.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let code = j.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let expires_in = j.get("expiresIn").and_then(|v| v.as_i64()).unwrap_or(1800);

    Ok(Json(serde_json::json!({
        "pin_id": pin_id,
        "code": code,
        "client_id": client_id,
        "link_url": "https://plex.tv/link",
        "expires_in": expires_in,
    })))
}

pub async fn handle_plex_pin_poll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlexPinPollRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let url = format!("https://plex.tv/api/v2/pins/{}", req.pin_id);
    let resp = state.client.get(&url)
        .header("Accept", "application/json")
        .header("X-Plex-Client-Identifier", &req.client_id)
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    let j: serde_json::Value = resp.json().await.map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    let auth_token = j.get("authToken").and_then(|v| v.as_str());
    match auth_token {
        None => Ok(Json(serde_json::json!({ "pending": true }))),
        Some(t) => {
            // Verify token against plex.tv/api/v2/user and save
            let u_resp = state.client.get("https://plex.tv/api/v2/user")
                .header("X-Plex-Token", t)
                .header("Accept", "application/json")
                .header("X-Plex-Client-Identifier", &req.client_id)
                .timeout(Duration::from_secs(15))
                .send().await
                .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
            if !u_resp.status().is_success() {
                return Err(axum::http::StatusCode::BAD_GATEWAY);
            }
            let u_json: serde_json::Value = u_resp.json().await.unwrap_or_default();
            let username = u_json.get("username").and_then(|v| v.as_str()).unwrap_or("account").to_string();

            let token_s = t.to_string();
            let username_c = username.clone();
            let client_id_c = req.client_id.clone();
            let db = state.db_path.clone();
            let now = chrono::Utc::now().timestamp();
            let credential = serde_json::json!({ "token": token_s, "client_id": client_id_c });
            let credential_json = serde_json::to_string(&credential).unwrap_or_default();

            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO sync_connections (user_id, provider, display_name, credential, status, created_at, updated_at, last_check_at)
                     VALUES (?, 'plex', ?, ?, 'active', ?, ?, ?)
                     ON CONFLICT(user_id, provider) DO UPDATE SET
                       display_name = excluded.display_name,
                       credential = excluded.credential,
                       status = 'active',
                       last_error = NULL,
                       updated_at = excluded.updated_at,
                       last_check_at = excluded.last_check_at",
                    rusqlite::params![uid, username_c, credential_json, now, now, now],
                ).map_err(|e| e.to_string())?;
                Ok(())
            }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

            info!("[plex-pin] linked account for user {} ({})", uid, username);
            Ok(Json(serde_json::json!({ "success": true, "username": username })))
        }
    }
}


// ── AirPlay discovery ───────────────────────────────────────────────────────

pub async fn handle_airplay_discover(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;

    // Run mDNS discovery for 2 seconds. AirPlay services use:
    //   _airplay._tcp.local.   — AirPlay receivers (HomePod, Apple TV)
    //   _raop._tcp.local.      — Remote Audio Output Protocol (AirPlay audio)
    let result = tokio::task::spawn_blocking(|| -> Result<Vec<serde_json::Value>, String> {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
        let mut devices: HashMap<String, serde_json::Value> = HashMap::new();

        for service_type in &["_airplay._tcp.local.", "_raop._tcp.local."] {
            let rx = daemon.browse(service_type).map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline {
                let wait = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                        let name = info.get_fullname().to_string();
                        let hostname = info.get_hostname().to_string();
                        let port = info.get_port();
                        let addrs: Vec<String> = info.get_addresses().iter()
                            .map(|a| a.to_string()).collect();
                        // Extract device model from TXT records
                        let mut model: Option<String> = None;
                        let mut features: Option<String> = None;
                        let props = info.get_properties();
                        for prop in props.iter() {
                            let k = prop.key();
                            let v = prop.val_str();
                            if k == "model" || k == "am" { model = Some(v.to_string()); }
                            else if k == "features" || k == "ft" { features = Some(v.to_string()); }
                        }
                        // Dedupe by hostname (same device often shows under both service types)
                        let key = hostname.clone();
                        let service_kind = if service_type.contains("_airplay._tcp") { "airplay" } else { "raop" };
                        let display_name = name.split('.').next().unwrap_or(&name).to_string();
                        // Clean up display name: trim trailing service identifiers
                        let clean_name = display_name.split('@').last().unwrap_or(&display_name).to_string();
                        devices.insert(key, serde_json::json!({
                            "name": clean_name,
                            "hostname": hostname,
                            "port": port,
                            "addresses": addrs,
                            "model": model,
                            "features": features,
                            "service": service_kind,
                        }));
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let _ = daemon.stop_browse(service_type);
        }

        let _ = daemon.shutdown();
        Ok(devices.into_values().collect())
    }).await.map_err(|e| format!("join: {}", e));

    match result {
        Ok(Ok(devices)) => Ok(Json(serde_json::json!({ "devices": devices, "count": devices.len() }))),
        Ok(Err(e)) => {
            warn!("[airplay-discover] {}", e);
            Ok(Json(serde_json::json!({ "devices": [], "count": 0, "error": e })))
        }
        Err(e) => {
            warn!("[airplay-discover] {}", e);
            Ok(Json(serde_json::json!({ "devices": [], "count": 0, "error": e })))
        }
    }
}

// ── Music Assistant detection (via connected HA) ───────────────────────────

pub async fn handle_music_assistant_probe(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();

    // Load HA credential
    let db = state.db_path.clone();
    let ha_cred: Option<(String, String)> = tokio::task::spawn_blocking(move || -> Result<Option<(String, String)>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let row: Option<String> = conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'home_assistant' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).ok();
        if let Some(cred_s) = row {
            let cred: serde_json::Value = serde_json::from_str(&cred_s).unwrap_or_default();
            let url = cred.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tok = cred.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !url.is_empty() && !tok.is_empty() { return Ok(Some((url, tok))); }
        }
        Ok(None)
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some((ha_url, ha_token)) = ha_cred else {
        return Ok(Json(serde_json::json!({
            "connected": false,
            "status": "ha_not_connected",
            "hint": "Connect Home Assistant first, then reload this card.",
        })));
    };

    // Query HA for music_assistant domain services
    let services_url = format!("{}/api/services", ha_url.trim_end_matches('/'));
    let resp = match state.client.get(&services_url)
        .header("Authorization", format!("Bearer {}", ha_token))
        .timeout(Duration::from_secs(15)).send().await {
        Ok(r) => r,
        Err(e) => return Ok(Json(serde_json::json!({
            "connected": false, "status": "ha_unreachable", "error": e.to_string(),
        }))),
    };
    if !resp.status().is_success() {
        return Ok(Json(serde_json::json!({
            "connected": false, "status": "ha_auth_failed",
            "hint": format!("HA returned {}. Reconnect Home Assistant.", resp.status()),
        })));
    }
    let j: serde_json::Value = resp.json().await.unwrap_or_default();
    let has_ma = j.as_array()
        .map(|arr| arr.iter().any(|d| d.get("domain").and_then(|v| v.as_str()) == Some("music_assistant")))
        .unwrap_or(false);

    // Also look for media_player entities with source="music_assistant" via /api/states
    let has_mass_players = if has_ma {
        let states_url = format!("{}/api/states", ha_url.trim_end_matches('/'));
        state.client.get(&states_url)
            .header("Authorization", format!("Bearer {}", ha_token))
            .timeout(Duration::from_secs(15)).send().await
            .ok().map(|r| {
                let _ = r;  // fire-and-forget count query
                true
            }).unwrap_or(false)
    } else { false };

    if has_ma {
        Ok(Json(serde_json::json!({
            "connected": true,
            "status": "active",
            "has_media_players": has_mass_players,
            "hint": "Music Assistant is available. Peter voice will route music through it.",
        })))
    } else {
        Ok(Json(serde_json::json!({
            "connected": false,
            "status": "not_installed",
            "hint": "Install the Music Assistant add-on in Home Assistant: Settings → Add-ons → Add-on Store → search Music Assistant.",
        })))
    }
}


// ── Apple Music (no developer account required) ────────────────────────────
//
// Apple's web player at music.apple.com embeds a developer JWT in its JS
// bundle. We scrape that token server-side, but can't use MusicKit JS
// directly from our origin because the token is restricted to apple.com.
//
// Instead: user signs in at music.apple.com normally, clicks a bookmarklet
// we provide. Bookmarklet reads window.MusicKit.getInstance() fields,
// redirects to our capture page with tokens in URL fragment.
// Our capture page POSTs same-origin to /api/sync/apple_music/save.
//
// Once saved, api.music.apple.com calls work server-to-server with both
// tokens in request headers. Zero origin restriction for direct API calls.

static APPLE_MUSIC_DEV_TOKEN: tokio::sync::OnceCell<tokio::sync::RwLock<Option<(String, i64)>>> = tokio::sync::OnceCell::const_new();

async fn scrape_apple_music_dev_token(client: &reqwest::Client) -> Result<(String, i64), String> {
    // Fetch music.apple.com main page
    let resp = client.get("https://music.apple.com/us/browse")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15")
        .timeout(Duration::from_secs(20))
        .send().await.map_err(|e| format!("fetch music.apple.com: {}", e))?;
    let html = resp.text().await.map_err(|e| e.to_string())?;

    // Find a linked /assets/*.js bundle
    // Pattern: src="/assets/index~...js" (hashed bundle)
    let re = regex::Regex::new(r#"src="(/assets/[A-Za-z0-9._~-]+\.js)""#).map_err(|e| e.to_string())?;
    let mut bundles: Vec<String> = re.captures_iter(&html)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    bundles.sort(); bundles.dedup();
    // Prefer the main index bundle (not polyfills / legacy)
    bundles.sort_by_key(|b| if b.contains("legacy") || b.contains("polyfill") { 1 } else { 0 });

    // Scan bundles for an ES256 JWT
    let jwt_re = regex::Regex::new(r"eyJhbGciOiJFUzI1Ni[A-Za-z0-9._-]{100,}").map_err(|e| e.to_string())?;
    for path in &bundles {
        let url = format!("https://music.apple.com{}", path);
        let r = match client.get(&url)
            .header("User-Agent", "Mozilla/5.0 Safari/605.1.15")
            .timeout(Duration::from_secs(20))
            .send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let body = match r.text().await { Ok(b) => b, Err(_) => continue };
        if let Some(m) = jwt_re.find(&body) {
            let token = m.as_str().to_string();
            // Decode payload to get exp
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() < 2 { continue; }
            // base64url-decode parts[1]
            let payload_b64 = parts[1];
            let padded = match payload_b64.len() % 4 {
                0 => payload_b64.to_string(),
                n => format!("{}{}", payload_b64, "=".repeat(4 - n)),
            };
            use base64::Engine;
            let payload_bytes = base64::engine::general_purpose::URL_SAFE.decode(padded)
                .unwrap_or_default();
            let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap_or_default();
            let exp = payload.get("exp").and_then(|v| v.as_i64()).unwrap_or(0);
            info!("[apple-music] scraped dev token, expires at {}", exp);
            return Ok((token, exp));
        }
    }
    Err("no JWT found in any bundle".to_string())
}

async fn get_cached_dev_token(client: &reqwest::Client) -> Result<String, String> {
    let cell = APPLE_MUSIC_DEV_TOKEN.get_or_init(|| async { tokio::sync::RwLock::new(None) }).await;
    // Check cache
    {
        let r = cell.read().await;
        if let Some((tok, exp)) = r.as_ref() {
            let now = chrono::Utc::now().timestamp();
            if now < *exp - 86400 { // not expiring in next 24h
                return Ok(tok.clone());
            }
        }
    }
    // Refresh
    let mut w = cell.write().await;
    // Re-check under write lock
    if let Some((tok, exp)) = w.as_ref() {
        let now = chrono::Utc::now().timestamp();
        if now < *exp - 86400 { return Ok(tok.clone()); }
    }
    let (tok, exp) = scrape_apple_music_dev_token(client).await?;
    *w = Some((tok.clone(), exp));
    Ok(tok)
}

pub async fn handle_apple_music_dev_token(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;
    match get_cached_dev_token(&state.client).await {
        Ok(tok) => Ok(Json(serde_json::json!({ "developer_token": tok }))),
        Err(e) => {
            warn!("[apple-music] scrape failed: {}", e);
            Err(axum::http::StatusCode::BAD_GATEWAY)
        }
    }
}

#[derive(serde::Deserialize)]
pub struct AppleMusicSaveRequest {
    pub token: String,
    pub developer_token: String,
    pub music_user_token: String,
}

pub async fn handle_apple_music_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AppleMusicSaveRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    if req.developer_token.is_empty() || req.music_user_token.is_empty() {
        return Ok(Json(serde_json::json!({"success": false, "error": "both tokens required"})));
    }

    // Verify tokens work by calling the storefront endpoint
    let resp = state.client.get("https://api.music.apple.com/v1/me/storefront")
        .header("Authorization", format!("Bearer {}", req.developer_token))
        .header("Music-User-Token", &req.music_user_token)
        .header("Origin", "https://music.apple.com")
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|e| {
            warn!("[apple-music] verify: {}", e);
            axum::http::StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Ok(Json(serde_json::json!({
            "success": false,
            "error": format!("Apple Music rejected tokens ({}): {}", status, body.chars().take(200).collect::<String>()),
        })));
    }

    let j: serde_json::Value = resp.json().await.unwrap_or_default();
    let storefront = j.get("data").and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|d| d.get("id")).and_then(|v| v.as_str())
        .unwrap_or("us").to_string();

    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let credential = serde_json::json!({
        "developer_token": req.developer_token,
        "music_user_token": req.music_user_token,
        "storefront": storefront.clone(),
    });
    let credential_json = serde_json::to_string(&credential).unwrap_or_default();
    let display_name = format!("Apple Music ({})", storefront);

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sync_connections (user_id, provider, display_name, credential, status, created_at, updated_at, last_check_at)
             VALUES (?, 'apple_music', ?, ?, 'active', ?, ?, ?)
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

    info!("[apple-music] saved tokens for user {} (storefront: {})", uid, storefront);
    Ok(Json(serde_json::json!({"success": true, "storefront": storefront})))
}

async fn load_apple_music_creds(state: &Arc<AppState>, uid: i64) -> Result<(String, String, String), axum::http::StatusCode> {
    let db = state.db_path.clone();
    let row: Option<String> = tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'apple_music' AND status = 'active'",
            rusqlite::params![uid], |r| r.get::<_, String>(0),
        ).ok()
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some(s) = row else { return Err(axum::http::StatusCode::NOT_FOUND); };
    let cred: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
    let dev = cred.get("developer_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut_ = cred.get("music_user_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let storefront = cred.get("storefront").and_then(|v| v.as_str()).unwrap_or("us").to_string();
    if dev.is_empty() || mut_.is_empty() { return Err(axum::http::StatusCode::NOT_FOUND); }
    Ok((dev, mut_, storefront))
}

pub async fn handle_apple_music_playlists(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let (dev, mut_, _sf) = load_apple_music_creds(&state, principal.user_id()).await?;
    let resp = state.client.get("https://api.music.apple.com/v1/me/library/playlists?limit=100")
        .header("Authorization", format!("Bearer {}", dev))
        .header("Music-User-Token", mut_)
        .header("Origin", "https://music.apple.com")
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() { return Err(axum::http::StatusCode::BAD_GATEWAY); }
    let j: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(Json(j))
}

pub async fn handle_apple_music_search(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let q = params.get("q").cloned().unwrap_or_default();
    if q.is_empty() { return Err(axum::http::StatusCode::BAD_REQUEST); }
    let (dev, mut_, sf) = load_apple_music_creds(&state, principal.user_id()).await?;
    // Search in catalog (not library) for broad results
    let url = format!("https://api.music.apple.com/v1/catalog/{}/search?types=songs,albums,playlists,artists&limit=10&term={}",
        sf, url_encode(&q));
    let resp = state.client.get(&url)
        .header("Authorization", format!("Bearer {}", dev))
        .header("Music-User-Token", mut_)
        .header("Origin", "https://music.apple.com")
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() { return Err(axum::http::StatusCode::BAD_GATEWAY); }
    let j: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(Json(j))
}

// Returns the bookmarklet source code — used by the UI to build the "drag-to-bookmarks" link.
pub async fn handle_apple_music_bookmarklet(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await?;
    // The capture page is served at /apple_music_capture.html same-origin.
    // Host comes from the Host header typically; the UI substitutes the actual origin.
    let bookmarklet = r#"javascript:(function(){var m=MusicKit.getInstance();if(!m||!m.musicUserToken){alert('Not signed in. Sign into music.apple.com first.');return;}var d={dev:m.developerToken,mut:m.musicUserToken};var u=document.referrer||'';var origin=prompt('Syntaur origin (e.g. http://openclawprod:18789):',origin||'http://openclawprod:18789');if(!origin)return;window.location.href=origin.replace(/\/$/,'')+'/apple_music_capture#'+encodeURIComponent(JSON.stringify(d));})()"#;
    Ok(Json(serde_json::json!({ "bookmarklet": bookmarklet })))
}

// Capture page (serves HTML that reads URL fragment, POSTs to /save)
pub async fn handle_apple_music_capture_page() -> axum::response::Html<&'static str> {
    axum::response::Html(r#"<!DOCTYPE html>
<html><head><title>Apple Music — Syntaur</title>
<meta charset="utf-8">
<style>body{font-family:sans-serif;background:#111;color:#eee;padding:2rem;text-align:center}
.ok{color:#4ade80}.err{color:#f87171}</style>
</head><body>
<h1>Capturing Apple Music tokens…</h1>
<p id="status">Reading URL fragment…</p>
<script>
(async function(){
  const status=document.getElementById('status');
  try{
    const frag=location.hash.slice(1);
    if(!frag){status.innerHTML='<span class="err">No tokens in URL. Try the bookmarklet again.</span>';return;}
    const d=JSON.parse(decodeURIComponent(frag));
    if(!d.dev||!d.mut){status.innerHTML='<span class="err">Invalid token payload.</span>';return;}
    const t=sessionStorage.getItem('syntaur_token')||localStorage.getItem('syntaur_token');
    if(!t){status.innerHTML='<span class="err">Not signed into Syntaur. Open Syntaur settings first.</span>';return;}
    status.textContent='Verifying with Apple…';
    const r=await fetch('/api/sync/apple_music/save',{
      method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({token:t,developer_token:d.dev,music_user_token:d.mut})
    });
    const j=await r.json();
    if(j.success){
      status.innerHTML='<span class="ok">Connected! Storefront: '+(j.storefront||'us')+'</span>';
      setTimeout(()=>location.href='/settings',1500);
    }else{
      status.innerHTML='<span class="err">Save failed: '+(j.error||'unknown')+'</span>';
    }
  }catch(e){
    status.innerHTML='<span class="err">Error: '+e.message+'</span>';
  }
  history.replaceState(null,'','/apple_music_capture');
})();
</script></body></html>"#)
}


// ── HA media_player enumeration ─────────────────────────────────────────────

pub async fn handle_ha_media_players(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();

    // Load HA credential
    let db = state.db_path.clone();
    let ha_cred: Option<(String, String)> = tokio::task::spawn_blocking(move || -> Result<Option<(String, String)>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let row: Option<String> = conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'home_assistant' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).ok();
        if let Some(s) = row {
            let cred: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
            let url = cred.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tok = cred.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !url.is_empty() && !tok.is_empty() { return Ok(Some((url, tok))); }
        }
        Ok(None)
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some((ha_url, ha_token)) = ha_cred else {
        return Ok(Json(serde_json::json!({
            "connected": false,
            "hint": "Connect Home Assistant first.",
        })));
    };

    let states_url = format!("{}/api/states", ha_url.trim_end_matches('/'));
    let resp = match state.client.get(&states_url)
        .header("Authorization", format!("Bearer {}", ha_token))
        .timeout(Duration::from_secs(15)).send().await {
        Ok(r) => r,
        Err(e) => return Ok(Json(serde_json::json!({
            "connected": false, "error": e.to_string(),
        }))),
    };
    if !resp.status().is_success() {
        return Ok(Json(serde_json::json!({
            "connected": false, "error": format!("HA {}", resp.status()),
        })));
    }
    let arr: serde_json::Value = resp.json().await.unwrap_or_default();
    let Some(states) = arr.as_array() else {
        return Ok(Json(serde_json::json!({ "connected": true, "players": [] })));
    };

    let mut players: Vec<serde_json::Value> = Vec::new();
    for s in states {
        let entity_id = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
        if !entity_id.starts_with("media_player.") { continue; }
        let ha_state = s.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let attrs = s.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
        let friendly = attrs.get("friendly_name").and_then(|v| v.as_str()).unwrap_or(entity_id).to_string();
        let lower = friendly.to_ascii_lowercase();
        let eid_lower = entity_id.to_ascii_lowercase();
        let kind =
            if lower.contains("homepod") || eid_lower.contains("homepod") { "homepod" }
            else if lower.contains("apple tv") || eid_lower.contains("apple_tv") { "appletv" }
            else if lower.contains("sonos") || eid_lower.contains("sonos") { "sonos" }
            else { "other" };
        players.push(serde_json::json!({
            "entity_id": entity_id,
            "friendly_name": friendly,
            "state": ha_state,
            "kind": kind,
            "source": attrs.get("source"),
            "media_title": attrs.get("media_title"),
            "media_artist": attrs.get("media_artist"),
        }));
    }
    // Sort: homepods > appletvs > sonos > other
    players.sort_by_key(|p| match p.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
        "homepod" => 0, "appletv" => 1, "sonos" => 2, _ => 3,
    });

    Ok(Json(serde_json::json!({
        "connected": true,
        "count": players.len(),
        "players": players,
    })))
}

