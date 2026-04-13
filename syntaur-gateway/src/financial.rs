//! Financial integrations — Plaid, Stripe, Alpaca, Coinbase, SimpleFIN, Gmail.
//!
//! All external API calls use `state.client` (reqwest::Client). DB operations
//! run via `tokio::task::spawn_blocking` with rusqlite. Every handler follows
//! the same auth + module-gating pattern established in `tax.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};

use crate::tax::{cents_to_display, check_module_access, parse_cents};
use crate::AppState;

// ── Configuration ──────────────────────────────────────────────────────────

/// Top-level integrations config loaded from `state.config.extra["integrations"]`.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct IntegrationsConfig {
    pub plaid: Option<PlaidConfig>,
    pub stripe: Option<StripeConfig>,
    pub alpaca: Option<AlpacaConfig>,
    pub coinbase: Option<CoinbaseConfig>,
    pub simplefin: Option<SimplefinConfig>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PlaidConfig {
    pub client_id: String,
    pub secret: String,
    /// "sandbox", "development", or "production"
    #[serde(default = "default_plaid_env")]
    pub environment: String,
}

fn default_plaid_env() -> String {
    "sandbox".to_string()
}

impl PlaidConfig {
    pub fn base_url(&self) -> &str {
        match self.environment.as_str() {
            "production" => "https://production.plaid.com",
            "development" => "https://development.plaid.com",
            _ => "https://sandbox.plaid.com",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StripeConfig {
    pub secret_key: String,
    pub webhook_secret: Option<String>,
    /// URL the user returns to after successful payment.
    pub success_url: Option<String>,
    /// URL the user returns to if they cancel payment.
    pub cancel_url: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AlpacaConfig {
    pub api_key: String,
    pub api_secret: String,
    /// "live" or "paper" (default: "paper")
    #[serde(default = "default_alpaca_env")]
    pub environment: String,
}

fn default_alpaca_env() -> String {
    "paper".to_string()
}

impl AlpacaConfig {
    pub fn base_url(&self) -> &str {
        match self.environment.as_str() {
            "live" => "https://api.alpaca.markets",
            _ => "https://paper-api.alpaca.markets",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CoinbaseConfig {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SimplefinConfig {
    /// Access URL (obtained after exchanging setup token). Contains embedded credentials.
    pub access_url: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Load integrations config from the `extra` map of the gateway config.
fn load_integrations(state: &AppState) -> IntegrationsConfig {
    state
        .config
        .extra
        .get("integrations")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Module gate: all financial endpoints (except Stripe webhook) require tax module access.
async fn require_financial_access(state: &AppState, user_id: i64) -> Result<(), (StatusCode, String)> {
    let db = state.db_path.clone();
    let uid = user_id;
    let access = tokio::task::spawn_blocking(move || -> Result<crate::tax::ModuleAccess, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        check_module_access(&conn, uid, "tax")
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if access.granted {
        return Ok(());
    }

    let msg = match access.reason.as_str() {
        "trial_expired" => {
            "Your free trial has ended. Upgrade to Syntaur Pro ($49) to unlock financial integrations."
                .to_string()
        }
        _ => "Financial integrations require Syntaur Pro ($49) or a free trial.".to_string(),
    };

    Err((
        StatusCode::PAYMENT_REQUIRED,
        serde_json::json!({
            "error": "module_locked",
            "module": "tax",
            "message": msg,
            "reason": access.reason,
            "trial_available": access.reason == "no_access",
            "pro_price": "$49.00",
        })
        .to_string(),
    ))
}

/// Ensure the v16 financial tables exist. Called lazily on first use.
/// Ensure supplementary tables exist. The core tables (connected_accounts,
/// investment_accounts, investment_transactions, email_connections) are created
/// by schema v16 in index/schema.rs. This only creates tables the agent module
/// needs that aren't in the migration (positions cache, financial transactions).
fn ensure_financial_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS investment_positions (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id         INTEGER NOT NULL DEFAULT 0,
            provider        TEXT NOT NULL,
            symbol          TEXT NOT NULL,
            qty             REAL NOT NULL DEFAULT 0.0,
            avg_cost_cents  INTEGER,
            current_price_cents INTEGER,
            market_value_cents  INTEGER,
            unrealized_pl_cents INTEGER,
            asset_class     TEXT,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_invest_pos_user ON investment_positions(user_id);
        CREATE INDEX IF NOT EXISTS idx_invest_pos_symbol ON investment_positions(symbol);

        CREATE TABLE IF NOT EXISTS financial_transactions (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id         INTEGER NOT NULL DEFAULT 0,
            account_id      INTEGER,
            provider        TEXT NOT NULL,
            external_id     TEXT,
            name            TEXT NOT NULL,
            amount_cents    INTEGER NOT NULL,
            date            TEXT NOT NULL,
            category        TEXT,
            merchant_name   TEXT,
            pending         INTEGER NOT NULL DEFAULT 0,
            metadata_json   TEXT NOT NULL DEFAULT '{}',
            created_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_fin_tx_user ON financial_transactions(user_id);
        CREATE INDEX IF NOT EXISTS idx_fin_tx_date ON financial_transactions(date);
        CREATE INDEX IF NOT EXISTS idx_fin_tx_account ON financial_transactions(account_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_fin_tx_ext ON financial_transactions(user_id, provider, external_id) WHERE external_id IS NOT NULL;
        "#,
    )
    .map_err(|e| format!("Failed to create financial tables: {}", e))
}

// ── Request / Response types ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct PlaidLinkTokenRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct PlaidExchangeRequest {
    pub token: String,
    pub public_token: String,
    pub account_name: Option<String>,
}

#[derive(Deserialize)]
pub struct PlaidSyncRequest {
    pub token: String,
    pub account_id: i64,
}

#[derive(Deserialize)]
pub struct ConnectionDeleteRequest {
    pub token: String,
    pub account_id: i64,
}

#[derive(Deserialize)]
pub struct AlpacaConnectRequest {
    pub token: String,
    pub api_key: String,
    pub api_secret: String,
    #[serde(default = "default_alpaca_env")]
    pub environment: String,
}

#[derive(Deserialize)]
pub struct AlpacaSyncRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct CoinbaseConnectRequest {
    pub token: String,
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Deserialize)]
pub struct CoinbaseSyncRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct SimplefinConnectRequest {
    pub token: String,
    pub setup_token: String,
}

#[derive(Deserialize)]
pub struct SimplefinSyncRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct StripeCheckoutRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct GmailConnectRequest {
    pub token: String,
    pub oauth_code: String,
}

#[derive(Deserialize)]
pub struct GmailScanRequest {
    pub token: String,
    #[serde(default = "default_gmail_max")]
    pub max_results: u32,
}

fn default_gmail_max() -> u32 {
    50
}

// ════════════════════════════════════════════════════════════════════════════
//  PLAID
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/plaid/link-token
///
/// Creates a Plaid Link token so the frontend can launch the Plaid Link widget.
/// Returns `{ link_token, expiration }`.
pub async fn handle_plaid_link_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlaidLinkTokenRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let config = load_integrations(&state);
    let plaid = config.plaid.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Plaid is not configured. Add integrations.plaid to your config.".to_string(),
        )
    })?;

    let user_id_str = format!("user-{}", principal.user_id());
    let body = serde_json::json!({
        "client_id": plaid.client_id,
        "secret": plaid.secret,
        "user": { "client_user_id": user_id_str },
        "client_name": "Syntaur",
        "products": ["transactions"],
        "country_codes": ["US"],
        "language": "en",
    });

    let url = format!("{}/link/token/create", plaid.base_url());
    let resp = state
        .client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Plaid link/token/create failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                format!("Plaid API error: {}", e),
            )
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        error!("[financial] Plaid link/token/create returned {}: {}", status, &text[..text.len().min(500)]);
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Plaid returned {}: {}", status, &text[..text.len().min(200)]),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Plaid returned invalid JSON: {}", e),
        )
    })?;

    info!(
        "[financial] Created Plaid link token for user {}",
        principal.user_id()
    );

    Ok(Json(serde_json::json!({
        "link_token": parsed["link_token"],
        "expiration": parsed["expiration"],
    })))
}

/// POST /api/financial/plaid/exchange
///
/// Exchange a Plaid `public_token` (from Link widget success) for a permanent
/// `access_token`. Saves the connected account to the DB.
pub async fn handle_plaid_exchange(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlaidExchangeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let config = load_integrations(&state);
    let plaid = config.plaid.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Plaid is not configured.".to_string(),
        )
    })?;

    let body = serde_json::json!({
        "client_id": plaid.client_id,
        "secret": plaid.secret,
        "public_token": req.public_token,
    });

    let url = format!("{}/item/public_token/exchange", plaid.base_url());
    let resp = state
        .client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Plaid exchange failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Plaid API error: {}", e))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        error!("[financial] Plaid exchange returned {}: {}", status, &text[..text.len().min(500)]);
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Plaid exchange failed: {}", &text[..text.len().min(200)]),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Invalid JSON from Plaid: {}", e),
        )
    })?;

    let access_token = parsed["access_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let item_id = parsed["item_id"].as_str().unwrap_or("").to_string();

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let acct_name = req.account_name.clone().unwrap_or_else(|| "Plaid Account".to_string());
    let now = chrono::Utc::now().timestamp();
    let item_id_log = item_id.clone();

    let account_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;
        conn.execute(
            "INSERT INTO connected_accounts (user_id, provider, account_name, access_token, item_id, status, created_at, updated_at) \
             VALUES (?, 'plaid', ?, ?, ?, 'active', ?, ?)",
            rusqlite::params![uid, &acct_name, &access_token, &item_id, now, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!(
        "[financial] Plaid account #{} connected for user {} (item {})",
        account_id,
        uid,
        &item_id_log[..item_id_log.len().min(12)]
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": account_id,
        "account_name": req.account_name.as_deref().unwrap_or("Plaid Account"),
        "item_id": item_id_log,
    })))
}

/// POST /api/financial/plaid/sync
///
/// Incremental transaction sync using Plaid's /transactions/sync endpoint.
/// Uses a cursor to fetch only new/modified/removed transactions since the last sync.
pub async fn handle_plaid_sync(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlaidSyncRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let config = load_integrations(&state);
    let plaid = config.plaid.ok_or_else(|| {
        (StatusCode::NOT_FOUND, "Plaid is not configured.".to_string())
    })?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let acct_id = req.account_id;

    // Load the connected account
    let (access_token, cursor) = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, String), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            conn.query_row(
                "SELECT access_token, COALESCE(cursor, '') FROM connected_accounts WHERE id = ? AND user_id = ? AND provider = 'plaid'",
                rusqlite::params![acct_id, uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(|e| format!("Account not found: {}", e))
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::NOT_FOUND, e))?
    };

    // Sync loop — Plaid may paginate
    let mut current_cursor = cursor;
    let mut total_added = 0i64;
    let mut total_modified = 0i64;
    let mut total_removed = 0i64;
    let mut has_more = true;

    while has_more {
        let mut sync_body = serde_json::json!({
            "client_id": plaid.client_id,
            "secret": plaid.secret,
            "access_token": access_token,
        });
        if !current_cursor.is_empty() {
            sync_body["cursor"] = serde_json::Value::String(current_cursor.clone());
        }

        let url = format!("{}/transactions/sync", plaid.base_url());
        let resp = state
            .client
            .post(&url)
            .json(&sync_body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| {
                error!("[financial] Plaid transactions/sync failed: {}", e);
                (StatusCode::BAD_GATEWAY, format!("Plaid sync error: {}", e))
            })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            error!("[financial] Plaid sync returned {}: {}", status, &text[..text.len().min(500)]);
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("Plaid sync failed: {}", &text[..text.len().min(200)]),
            ));
        }

        let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            (StatusCode::BAD_GATEWAY, format!("Invalid JSON: {}", e))
        })?;

        let added = parsed["added"].as_array().cloned().unwrap_or_default();
        let modified = parsed["modified"].as_array().cloned().unwrap_or_default();
        let removed = parsed["removed"].as_array().cloned().unwrap_or_default();
        let next_cursor = parsed["next_cursor"]
            .as_str()
            .unwrap_or("")
            .to_string();
        has_more = parsed["has_more"].as_bool().unwrap_or(false);

        // Persist transactions
        let db3 = db.clone();
        let a_count = added.len() as i64;
        let m_count = modified.len() as i64;
        let r_count = removed.len() as i64;
        let cursor_val = next_cursor.clone();

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db3).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            let now = chrono::Utc::now().timestamp();

            // Insert/update added transactions
            for txn in &added {
                let ext_id = txn["transaction_id"].as_str().unwrap_or("");
                let name = txn["name"].as_str().unwrap_or("Unknown");
                let amount = txn["amount"].as_f64().unwrap_or(0.0);
                // Plaid amounts are positive for debits, negative for credits
                let amount_cents = (amount * 100.0).round() as i64;
                let date = txn["date"].as_str().unwrap_or("");
                let category = txn["category"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let merchant = txn["merchant_name"].as_str().unwrap_or("");
                let pending = txn["pending"].as_bool().unwrap_or(false) as i64;

                conn.execute(
                    "INSERT OR REPLACE INTO financial_transactions \
                     (user_id, account_id, provider, external_id, name, amount_cents, date, category, merchant_name, pending, created_at) \
                     VALUES (?, ?, 'plaid', ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![uid, acct_id, ext_id, name, amount_cents, date, category, merchant, pending, now],
                )
                .map_err(|e| e.to_string())?;
            }

            // Update modified transactions
            for txn in &modified {
                let ext_id = txn["transaction_id"].as_str().unwrap_or("");
                let name = txn["name"].as_str().unwrap_or("Unknown");
                let amount = txn["amount"].as_f64().unwrap_or(0.0);
                let amount_cents = (amount * 100.0).round() as i64;
                let date = txn["date"].as_str().unwrap_or("");
                let category = txn["category"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let merchant = txn["merchant_name"].as_str().unwrap_or("");
                let pending = txn["pending"].as_bool().unwrap_or(false) as i64;

                conn.execute(
                    "UPDATE financial_transactions SET name = ?, amount_cents = ?, date = ?, \
                     category = ?, merchant_name = ?, pending = ? \
                     WHERE user_id = ? AND provider = 'plaid' AND external_id = ?",
                    rusqlite::params![name, amount_cents, date, category, merchant, pending, uid, ext_id],
                )
                .map_err(|e| e.to_string())?;
            }

            // Remove deleted transactions
            for txn in &removed {
                let ext_id = txn["transaction_id"].as_str().unwrap_or("");
                if !ext_id.is_empty() {
                    conn.execute(
                        "DELETE FROM financial_transactions WHERE user_id = ? AND provider = 'plaid' AND external_id = ?",
                        rusqlite::params![uid, ext_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }

            // Update cursor on the connected account
            conn.execute(
                "UPDATE connected_accounts SET cursor = ?, updated_at = ? WHERE id = ?",
                rusqlite::params![&cursor_val, now, acct_id],
            )
            .map_err(|e| e.to_string())?;

            Ok(())
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        total_added += a_count;
        total_modified += m_count;
        total_removed += r_count;
        current_cursor = next_cursor;
    }

    info!(
        "[financial] Plaid sync for account #{}: +{} ~{} -{}",
        acct_id, total_added, total_modified, total_removed
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": acct_id,
        "added": total_added,
        "modified": total_modified,
        "removed": total_removed,
    })))
}

/// POST /api/financial/plaid/webhook
///
/// Receives Plaid webhook notifications (DEFAULT_UPDATE, TRANSACTIONS_REMOVED, etc).
/// No auth token required — Plaid sends these server-to-server.
pub async fn handle_plaid_webhook(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let webhook_type = body["webhook_type"].as_str().unwrap_or("UNKNOWN");
    let webhook_code = body["webhook_code"].as_str().unwrap_or("UNKNOWN");
    let item_id = body["item_id"].as_str().unwrap_or("");

    info!(
        "[financial] Plaid webhook: type={} code={} item={}",
        webhook_type,
        webhook_code,
        &item_id[..item_id.len().min(12)]
    );

    match (webhook_type, webhook_code) {
        ("TRANSACTIONS", "DEFAULT_UPDATE") | ("TRANSACTIONS", "INITIAL_UPDATE") => {
            // New transactions available — mark the account as needing sync
            let db = state.db_path.clone();
            let iid = item_id.to_string();
            let now = chrono::Utc::now().timestamp();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                ensure_financial_tables(&conn)?;
                conn.execute(
                    "UPDATE connected_accounts SET metadata_json = json_set(COALESCE(metadata_json, '{}'), '$.needs_sync', 1), updated_at = ? WHERE item_id = ?",
                    rusqlite::params![now, &iid],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

            info!("[financial] Marked Plaid item {} for sync", &item_id[..item_id.len().min(12)]);
        }
        ("ITEM", "ERROR") => {
            let error_msg = body["error"]["error_message"]
                .as_str()
                .unwrap_or("Unknown error");
            warn!(
                "[financial] Plaid item error for {}: {}",
                &item_id[..item_id.len().min(12)],
                error_msg
            );

            let db = state.db_path.clone();
            let iid = item_id.to_string();
            let err = error_msg.to_string();
            let now = chrono::Utc::now().timestamp();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                ensure_financial_tables(&conn)?;
                conn.execute(
                    "UPDATE connected_accounts SET status = 'error', \
                     metadata_json = json_set(COALESCE(metadata_json, '{}'), '$.last_error', ?), \
                     updated_at = ? WHERE item_id = ?",
                    rusqlite::params![&err, now, &iid],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }
        _ => {
            info!(
                "[financial] Ignoring Plaid webhook: {}/{}",
                webhook_type, webhook_code
            );
        }
    }

    Ok(Json(serde_json::json!({ "received": true })))
}

// ── Connected Accounts ─────────────────────────────────────────────────────

/// GET /api/financial/connections
///
/// List all connected financial accounts for the authenticated user.
pub async fn handle_connections_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    let accounts = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, provider, account_name, account_id, status, metadata_json, created_at, updated_at \
                 FROM connected_accounts WHERE user_id = ? ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![uid], |r| {
                let meta_str: String = r.get(5)?;
                let meta: serde_json::Value =
                    serde_json::from_str(&meta_str).unwrap_or(serde_json::json!({}));
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?,
                    "provider": r.get::<_, String>(1)?,
                    "account_name": r.get::<_, Option<String>>(2)?,
                    "account_id": r.get::<_, Option<String>>(3)?,
                    "status": r.get::<_, String>(4)?,
                    "metadata": meta,
                    "created_at": r.get::<_, i64>(6)?,
                    "updated_at": r.get::<_, i64>(7)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

/// DELETE /api/financial/connections
///
/// Remove a connected account (soft-delete: sets status to 'disconnected').
pub async fn handle_connection_delete(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectionDeleteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let acct_id = req.account_id;
    let now = chrono::Utc::now().timestamp();

    let deleted = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;
        let rows = conn
            .execute(
                "UPDATE connected_accounts SET status = 'disconnected', access_token = NULL, updated_at = ? \
                 WHERE id = ? AND user_id = ?",
                rusqlite::params![now, acct_id, uid],
            )
            .map_err(|e| e.to_string())?;
        Ok(rows > 0)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Account not found.".to_string()));
    }

    info!(
        "[financial] Disconnected account #{} for user {}",
        acct_id, uid
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": acct_id,
    })))
}

// ════════════════════════════════════════════════════════════════════════════
//  STRIPE
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/stripe/checkout
///
/// Creates a Stripe Checkout session for $49 Syntaur Pro. Returns the
/// checkout URL so the frontend can redirect the user to Stripe.
pub async fn handle_stripe_checkout(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StripeCheckoutRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    let config = load_integrations(&state);
    let stripe = config.stripe.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Stripe is not configured. Add integrations.stripe to your config.".to_string(),
        )
    })?;

    let success_url = stripe
        .success_url
        .as_deref()
        .unwrap_or("http://localhost:3000/settings?payment=success");
    let cancel_url = stripe
        .cancel_url
        .as_deref()
        .unwrap_or("http://localhost:3000/settings?payment=cancelled");

    let uid = principal.user_id();

    // Stripe uses form-encoded params, not JSON
    let params = [
        ("mode", "payment"),
        ("success_url", success_url),
        ("cancel_url", cancel_url),
        ("line_items[0][price_data][currency]", "usd"),
        (
            "line_items[0][price_data][product_data][name]",
            "Syntaur Pro",
        ),
        (
            "line_items[0][price_data][product_data][description]",
            "Lifetime access to all Syntaur modules including Tax & Expenses, Financial Integrations, and future premium features.",
        ),
        ("line_items[0][price_data][unit_amount]", "4900"),
        ("line_items[0][quantity]", "1"),
        ("payment_intent_data[metadata][user_id]", &uid.to_string()),
    ];

    let resp = state
        .client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .bearer_auth(&stripe.secret_key)
        .form(&params)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Stripe checkout creation failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                format!("Stripe API error: {}", e),
            )
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        error!("[financial] Stripe returned {}: {}", status, &text[..text.len().min(500)]);
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Stripe error: {}", &text[..text.len().min(200)]),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Invalid JSON from Stripe: {}", e),
        )
    })?;

    let checkout_url = parsed["url"].as_str().unwrap_or("").to_string();
    let session_id = parsed["id"].as_str().unwrap_or("").to_string();

    info!(
        "[financial] Stripe checkout session {} created for user {}",
        &session_id[..session_id.len().min(20)],
        uid
    );

    Ok(Json(serde_json::json!({
        "checkout_url": checkout_url,
        "session_id": session_id,
    })))
}

/// POST /api/financial/stripe/webhook
///
/// Receives Stripe webhook events. Verifies the signature (if webhook_secret
/// is configured) and processes `checkout.session.completed` events to
/// activate Pro licenses.
pub async fn handle_stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = load_integrations(&state);
    let stripe = config.stripe.ok_or_else(|| {
        (StatusCode::NOT_FOUND, "Stripe not configured.".to_string())
    })?;

    let body_str =
        std::str::from_utf8(&body).map_err(|_| (StatusCode::BAD_REQUEST, "Invalid body".to_string()))?;

    // Verify webhook signature if configured
    if let Some(ref wh_secret) = stripe.webhook_secret {
        let sig_header = headers
            .get("stripe-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !verify_stripe_signature(body_str, sig_header, wh_secret) {
            warn!("[financial] Stripe webhook signature verification failed");
            return Err((
                StatusCode::UNAUTHORIZED,
                "Invalid webhook signature.".to_string(),
            ));
        }
    }

    let event: serde_json::Value = serde_json::from_str(body_str).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid JSON: {}", e),
        )
    })?;

    let event_type = event["type"].as_str().unwrap_or("");
    info!("[financial] Stripe webhook: {}", event_type);

    if event_type == "checkout.session.completed" {
        let session = &event["data"]["object"];
        let payment_status = session["payment_status"].as_str().unwrap_or("");

        if payment_status == "paid" {
            let user_id_str = session["payment_intent"]["metadata"]["user_id"]
                .as_str()
                .or_else(|| session["metadata"]["user_id"].as_str())
                .unwrap_or("0");
            let user_id: i64 = user_id_str.parse().unwrap_or(0);
            let payment_id = session["payment_intent"]
                .as_str()
                .or_else(|| session["id"].as_str())
                .unwrap_or("")
                .to_string();

            if user_id > 0 {
                let db = state.db_path.clone();
                let pid = payment_id.clone();
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                    let now = chrono::Utc::now().timestamp();
                    conn.execute(
                        "INSERT OR REPLACE INTO user_licenses (user_id, license_type, purchased_at, payment_id, amount_cents) \
                         VALUES (?, 'pro', ?, ?, 4900)",
                        rusqlite::params![user_id, now, &pid],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

                info!(
                    "[financial] Pro license activated for user {} via Stripe payment {}",
                    user_id,
                    &payment_id[..payment_id.len().min(20)]
                );
            } else {
                warn!("[financial] Stripe checkout completed but no user_id in metadata");
            }
        }
    }

    Ok(Json(serde_json::json!({ "received": true })))
}

/// Compute HMAC-SHA256 using the `sha2` crate (no `hmac` crate dependency).
///
/// HMAC(K, m) = H((K' ^ opad) || H((K' ^ ipad) || m))
/// where K' = H(K) if len(K) > block_size, else K padded to block_size.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    const BLOCK_SIZE: usize = 64;

    // Step 1: derive K' (key padded/hashed to block size)
    let mut k_prime = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hash = Sha256::digest(key);
        k_prime[..32].copy_from_slice(&hash);
    } else {
        k_prime[..key.len()].copy_from_slice(key);
    }

    // Step 2: inner hash — H((K' ^ ipad) || message)
    let mut inner = Sha256::new();
    let mut ipad = [0x36u8; BLOCK_SIZE];
    for (i, b) in ipad.iter_mut().enumerate() {
        *b ^= k_prime[i];
    }
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    // Step 3: outer hash — H((K' ^ opad) || inner_hash)
    let mut outer = Sha256::new();
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for (i, b) in opad.iter_mut().enumerate() {
        *b ^= k_prime[i];
    }
    outer.update(&opad);
    outer.update(&inner_hash);

    let result = outer.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Verify a Stripe webhook signature.
///
/// The `stripe-signature` header contains `t=<timestamp>,v1=<sig>`.
/// The expected signature is HMAC-SHA256 of `{timestamp}.{body}`.
fn verify_stripe_signature(body: &str, sig_header: &str, secret: &str) -> bool {
    let mut timestamp = "";
    let mut signature = "";

    for part in sig_header.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 {
            match kv[0] {
                "t" => timestamp = kv[1],
                "v1" => signature = kv[1],
                _ => {}
            }
        }
    }

    if timestamp.is_empty() || signature.is_empty() {
        return false;
    }

    // Check timestamp tolerance (5 min)
    if let Ok(ts) = timestamp.parse::<i64>() {
        let now = chrono::Utc::now().timestamp();
        if (now - ts).abs() > 300 {
            warn!("[financial] Stripe webhook timestamp too old: {}s delta", now - ts);
            return false;
        }
    }

    let payload = format!("{}.{}", timestamp, body);
    let mac = hmac_sha256(secret.as_bytes(), payload.as_bytes());
    let expected = hex::encode(mac);

    // Constant-time compare
    if expected.len() != signature.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in expected.bytes().zip(signature.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ════════════════════════════════════════════════════════════════════════════
//  ALPACA
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/alpaca/connect
///
/// Save Alpaca API credentials. Verifies them by calling GET /v2/account.
pub async fn handle_alpaca_connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlpacaConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let base_url = match req.environment.as_str() {
        "live" => "https://api.alpaca.markets",
        _ => "https://paper-api.alpaca.markets",
    };

    // Verify credentials by calling /v2/account
    let resp = state
        .client
        .get(format!("{}/v2/account", base_url))
        .header("APCA-API-KEY-ID", &req.api_key)
        .header("APCA-API-SECRET-KEY", &req.api_secret)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Alpaca verify failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Alpaca API error: {}", e))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Alpaca credentials invalid (HTTP {}). Double-check your API key and secret.",
                status
            ),
        ));
    }

    let acct: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let acct_number = acct["account_number"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let acct_number_log = acct_number.clone();

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let env = req.environment.clone();
    let key = req.api_key.clone();
    let secret = req.api_secret.clone();

    let account_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;

        // Upsert: replace existing Alpaca connection for this user
        conn.execute(
            "DELETE FROM investment_accounts WHERE user_id = ? AND broker = 'alpaca'",
            rusqlite::params![uid],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT INTO investment_accounts (user_id, broker, api_key, api_secret, base_url, nickname, status, created_at) \
             VALUES (?, 'alpaca', ?, ?, ?, ?, 'active', ?)",
            rusqlite::params![uid, &key, &secret, &env, format!("Alpaca ({})", env), now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!(
        "[financial] Alpaca connected for user {} (account {})",
        uid, acct_number_log
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": account_id,
        "alpaca_account": acct_number_log,
        "environment": req.environment,
    })))
}

/// POST /api/financial/alpaca/sync
///
/// Pull account activities (fills, dividends) and current positions from Alpaca.
pub async fn handle_alpaca_sync(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlpacaSyncRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Load Alpaca credentials from DB
    let (api_key, api_secret, base_url_str) = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, String, String), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            conn.query_row(
                "SELECT api_key, COALESCE(api_secret,''), COALESCE(base_url,'https://paper-api.alpaca.markets') FROM investment_accounts WHERE user_id = ? AND broker = 'alpaca' AND status = 'active'",
                rusqlite::params![uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            ).map_err(|_| "Alpaca not connected. Use /api/financial/alpaca/connect first.".to_string())
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
    };

    let base_url = base_url_str.as_str();

    // 1. Fetch account activities (fills + dividends)
    let activities_resp = state
        .client
        .get(format!(
            "{}/v2/account/activities?activity_types=FILL,DIV&direction=desc&page_size=200",
            base_url
        ))
        .header("APCA-API-KEY-ID", &api_key)
        .header("APCA-API-SECRET-KEY", &api_secret)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Alpaca activities fetch failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Alpaca API error: {}", e))
        })?;

    let activities_status = activities_resp.status();
    let activities_text = activities_resp.text().await.unwrap_or_default();

    if !activities_status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "Alpaca activities returned {}: {}",
                activities_status,
                &activities_text[..activities_text.len().min(200)]
            ),
        ));
    }

    let activities: Vec<serde_json::Value> =
        serde_json::from_str(&activities_text).unwrap_or_default();

    // 2. Fetch current positions
    let positions_resp = state
        .client
        .get(format!("{}/v2/positions", base_url))
        .header("APCA-API-KEY-ID", &api_key)
        .header("APCA-API-SECRET-KEY", &api_secret)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Alpaca positions fetch failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Alpaca API error: {}", e))
        })?;

    let positions_status = positions_resp.status();
    let positions_text = positions_resp.text().await.unwrap_or_default();

    if !positions_status.is_success() {
        warn!(
            "[financial] Alpaca positions returned {}: {}",
            positions_status,
            &positions_text[..positions_text.len().min(200)]
        );
    }

    let positions: Vec<serde_json::Value> =
        serde_json::from_str(&positions_text).unwrap_or_default();

    // 3. Persist to DB
    let db3 = db.clone();
    let act_count = activities.len();
    let pos_count = positions.len();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db3).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;
        let now = chrono::Utc::now().timestamp();

        // Insert activities as investment_transactions
        for act in &activities {
            let ext_id = act["id"].as_str().unwrap_or("");
            if ext_id.is_empty() {
                continue;
            }

            let activity_type = act["activity_type"].as_str().unwrap_or("");
            let symbol = act["symbol"].as_str().unwrap_or("");
            let side = act["side"].as_str().unwrap_or("");
            let qty = act["qty"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| act["qty"].as_f64())
                .unwrap_or(0.0);
            let price = act["price"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| act["price"].as_f64())
                .unwrap_or(0.0);
            let price_cents = (price * 100.0).round() as i64;
            let total_cents = (qty * price * 100.0).round() as i64;

            let tx_type = match activity_type {
                "FILL" => {
                    if side == "buy" {
                        "buy"
                    } else {
                        "sell"
                    }
                }
                "DIV" => "dividend",
                _ => activity_type,
            };

            let tx_date = act["transaction_time"]
                .as_str()
                .or_else(|| act["date"].as_str())
                .unwrap_or("");

            // Extract just date portion
            let date_only = if tx_date.len() >= 10 {
                &tx_date[..10]
            } else {
                tx_date
            };

            conn.execute(
                "INSERT OR IGNORE INTO investment_transactions \
                 (user_id, provider, external_id, symbol, side, qty, price_cents, total_cents, tx_type, tx_date, created_at) \
                 VALUES (?, 'alpaca', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    uid, ext_id, symbol, side, qty, price_cents, total_cents, tx_type, date_only, now
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        // Upsert positions
        conn.execute(
            "DELETE FROM investment_positions WHERE user_id = ? AND provider = 'alpaca'",
            rusqlite::params![uid],
        )
        .map_err(|e| e.to_string())?;

        for pos in &positions {
            let symbol = pos["symbol"].as_str().unwrap_or("");
            let qty = pos["qty"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| pos["qty"].as_f64())
                .unwrap_or(0.0);
            let avg_cost = pos["avg_entry_price"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| pos["avg_entry_price"].as_f64())
                .unwrap_or(0.0);
            let current_price = pos["current_price"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| pos["current_price"].as_f64())
                .unwrap_or(0.0);
            let market_value = pos["market_value"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| pos["market_value"].as_f64())
                .unwrap_or(0.0);
            let unrealized_pl = pos["unrealized_pl"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| pos["unrealized_pl"].as_f64())
                .unwrap_or(0.0);
            let asset_class = pos["asset_class"].as_str().unwrap_or("us_equity");

            conn.execute(
                "INSERT INTO investment_positions \
                 (user_id, provider, symbol, qty, avg_cost_cents, current_price_cents, market_value_cents, unrealized_pl_cents, asset_class, updated_at) \
                 VALUES (?, 'alpaca', ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    uid,
                    symbol,
                    qty,
                    (avg_cost * 100.0).round() as i64,
                    (current_price * 100.0).round() as i64,
                    (market_value * 100.0).round() as i64,
                    (unrealized_pl * 100.0).round() as i64,
                    asset_class,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!(
        "[financial] Alpaca sync for user {}: {} activities, {} positions",
        uid, act_count, pos_count
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "activities_synced": act_count,
        "positions_synced": pos_count,
    })))
}

// ════════════════════════════════════════════════════════════════════════════
//  COINBASE
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/coinbase/connect
///
/// Save Coinbase API credentials. Verifies by calling GET /v2/user.
pub async fn handle_coinbase_connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CoinbaseConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    // Verify credentials by calling /v2/user
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let message = format!("{}GET/v2/user", timestamp);
    let sig = coinbase_sign(&message, &req.api_secret);

    let resp = state
        .client
        .get("https://api.coinbase.com/v2/user")
        .header("CB-ACCESS-KEY", &req.api_key)
        .header("CB-ACCESS-SIGN", &sig)
        .header("CB-ACCESS-TIMESTAMP", &timestamp)
        .header("CB-VERSION", "2024-01-01")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Coinbase verify failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Coinbase API error: {}", e))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Coinbase credentials invalid (HTTP {}). Check your API key and secret.",
                status
            ),
        ));
    }

    let user_data: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let cb_name = user_data["data"]["name"]
        .as_str()
        .unwrap_or("Coinbase User")
        .to_string();
    let cb_name_log = cb_name.clone();

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let key = req.api_key.clone();
    let secret = req.api_secret.clone();

    let account_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;

        conn.execute(
            "DELETE FROM connected_accounts WHERE user_id = ? AND provider = 'coinbase'",
            rusqlite::params![uid],
        )
        .map_err(|e| e.to_string())?;

        let meta = serde_json::json!({
            "api_key": key,
            "api_secret": secret,
        });
        conn.execute(
            "INSERT INTO connected_accounts (user_id, provider, account_name, metadata_json, status, created_at, updated_at) \
             VALUES (?, 'coinbase', ?, ?, 'active', ?, ?)",
            rusqlite::params![uid, &cb_name, meta.to_string(), now, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!(
        "[financial] Coinbase connected for user {} ({})",
        uid, cb_name_log
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": account_id,
        "coinbase_user": cb_name_log,
    })))
}

/// POST /api/financial/coinbase/sync
///
/// Pull transaction history from all Coinbase accounts.
pub async fn handle_coinbase_sync(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CoinbaseSyncRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Load credentials
    let (api_key, api_secret) = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, String), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            let meta_json: String = conn
                .query_row(
                    "SELECT metadata_json FROM connected_accounts WHERE user_id = ? AND provider = 'coinbase' AND status = 'active'",
                    rusqlite::params![uid],
                    |r| r.get(0),
                )
                .map_err(|_| "Coinbase not connected.".to_string())?;
            let meta: serde_json::Value =
                serde_json::from_str(&meta_json).unwrap_or_default();
            Ok((
                meta["api_key"].as_str().unwrap_or("").to_string(),
                meta["api_secret"].as_str().unwrap_or("").to_string(),
            ))
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
    };

    // 1. List Coinbase accounts
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let message = format!("{}GET/v2/accounts", timestamp);
    let sig = coinbase_sign(&message, &api_secret);

    let resp = state
        .client
        .get("https://api.coinbase.com/v2/accounts?limit=100")
        .header("CB-ACCESS-KEY", &api_key)
        .header("CB-ACCESS-SIGN", &sig)
        .header("CB-ACCESS-TIMESTAMP", &timestamp)
        .header("CB-VERSION", "2024-01-01")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Coinbase accounts fetch failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Coinbase API error: {}", e))
        })?;

    let resp_status = resp.status();
    let resp_text = resp.text().await.unwrap_or_default();

    if !resp_status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "Coinbase accounts returned {}: {}",
                resp_status,
                &resp_text[..resp_text.len().min(200)]
            ),
        ));
    }

    let accounts_data: serde_json::Value =
        serde_json::from_str(&resp_text).unwrap_or_default();
    let accounts = accounts_data["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // 2. For each account with a non-zero balance, fetch transactions
    let mut total_txns = 0usize;
    let mut total_positions = 0usize;

    // Collect positions
    let db4 = db.clone();
    let accounts_clone = accounts.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db4).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "DELETE FROM investment_positions WHERE user_id = ? AND provider = 'coinbase'",
            rusqlite::params![uid],
        )
        .map_err(|e| e.to_string())?;

        for acct in &accounts_clone {
            let currency = acct["currency"]["code"]
                .as_str()
                .or_else(|| acct["currency"].as_str())
                .unwrap_or("");
            let balance_str = acct["balance"]["amount"].as_str().unwrap_or("0");
            let balance: f64 = balance_str.parse().unwrap_or(0.0);

            if balance.abs() < 0.000001 {
                continue;
            }

            let native_balance_str = acct["native_balance"]["amount"].as_str().unwrap_or("0");
            let native_balance: f64 = native_balance_str.parse().unwrap_or(0.0);
            let market_value_cents = (native_balance * 100.0).round() as i64;

            conn.execute(
                "INSERT INTO investment_positions \
                 (user_id, provider, symbol, qty, market_value_cents, asset_class, updated_at) \
                 VALUES (?, 'coinbase', ?, ?, ?, 'crypto', ?)",
                rusqlite::params![uid, currency, balance, market_value_cents, now],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    total_positions = accounts.iter().filter(|a| {
        let b = a["balance"]["amount"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        b.abs() > 0.000001
    }).count();

    // 3. Fetch transactions for each account
    for acct in &accounts {
        let acct_id = match acct["id"].as_str() {
            Some(id) => id,
            None => continue,
        };

        let path = format!("/v2/accounts/{}/transactions", acct_id);
        let ts = chrono::Utc::now().timestamp().to_string();
        let msg = format!("{}GET{}", ts, path);
        let s = coinbase_sign(&msg, &api_secret);

        let url = format!("https://api.coinbase.com{}?limit=100", path);
        let tx_resp = state
            .client
            .get(&url)
            .header("CB-ACCESS-KEY", &api_key)
            .header("CB-ACCESS-SIGN", &s)
            .header("CB-ACCESS-TIMESTAMP", &ts)
            .header("CB-VERSION", "2024-01-01")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;

        let tx_resp = match tx_resp {
            Ok(r) => r,
            Err(e) => {
                warn!("[financial] Coinbase txn fetch for {} failed: {}", acct_id, e);
                continue;
            }
        };

        if !tx_resp.status().is_success() {
            continue;
        }

        let tx_text = tx_resp.text().await.unwrap_or_default();
        let tx_data: serde_json::Value =
            serde_json::from_str(&tx_text).unwrap_or_default();
        let txns = tx_data["data"].as_array().cloned().unwrap_or_default();

        let db5 = db.clone();
        let txn_count = txns.len();

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db5).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            let now = chrono::Utc::now().timestamp();

            for txn in &txns {
                let ext_id = txn["id"].as_str().unwrap_or("");
                if ext_id.is_empty() {
                    continue;
                }

                let tx_type = txn["type"].as_str().unwrap_or("unknown");
                let amount_str = txn["amount"]["amount"].as_str().unwrap_or("0");
                let amount: f64 = amount_str.parse().unwrap_or(0.0);
                let native_str = txn["native_amount"]["amount"].as_str().unwrap_or("0");
                let native: f64 = native_str.parse().unwrap_or(0.0);
                let total_cents = (native * 100.0).round() as i64;
                let symbol = txn["amount"]["currency"].as_str().unwrap_or("");
                let description = txn["details"]["title"].as_str().unwrap_or("");
                let created_at_str = txn["created_at"].as_str().unwrap_or("");

                // Extract date portion
                let tx_date = if created_at_str.len() >= 10 {
                    &created_at_str[..10]
                } else {
                    created_at_str
                };

                let side = match tx_type {
                    "buy" => "buy",
                    "sell" => "sell",
                    "send" => "send",
                    "receive" | "fiat_deposit" => "receive",
                    _ => tx_type,
                };

                conn.execute(
                    "INSERT OR IGNORE INTO investment_transactions \
                     (user_id, provider, external_id, symbol, side, qty, total_cents, tx_type, tx_date, description, created_at) \
                     VALUES (?, 'coinbase', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        uid, ext_id, symbol, side, amount.abs(), total_cents, tx_type, tx_date, description, now
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        total_txns += txn_count;
    }

    info!(
        "[financial] Coinbase sync for user {}: {} txns, {} positions",
        uid, total_txns, total_positions
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "transactions_synced": total_txns,
        "positions_synced": total_positions,
    })))
}

/// Compute Coinbase v2 HMAC-SHA256 signature.
fn coinbase_sign(message: &str, secret: &str) -> String {
    hex::encode(hmac_sha256(secret.as_bytes(), message.as_bytes()))
}

// ════════════════════════════════════════════════════════════════════════════
//  SIMPLEFIN
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/simplefin/connect
///
/// Exchange a SimpleFIN setup token for an access URL. The setup token is
/// a base64-encoded URL; POST to it to claim the token and receive back
/// the access URL with embedded basic-auth credentials.
pub async fn handle_simplefin_connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SimplefinConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    // Decode setup token (base64-encoded URL)
    let claim_url = String::from_utf8(
        base64_decode(&req.setup_token)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid setup token (not valid base64).".to_string()))?,
    )
    .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid setup token (not valid UTF-8).".to_string()))?;

    // POST to claim URL to get the access URL
    let resp = state
        .client
        .post(&claim_url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] SimpleFIN claim failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                format!("SimpleFIN claim error: {}", e),
            )
        })?;

    let status = resp.status();
    let access_url = resp.text().await.unwrap_or_default().trim().to_string();

    if !status.is_success() || access_url.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "SimpleFIN claim failed (HTTP {}). The setup token may have already been used or expired.",
                status
            ),
        ));
    }

    // Save access URL to DB
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let url_clone = access_url.clone();

    let account_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;

        conn.execute(
            "DELETE FROM connected_accounts WHERE user_id = ? AND provider = 'simplefin'",
            rusqlite::params![uid],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT INTO connected_accounts (user_id, provider, account_name, access_token, status, created_at, updated_at) \
             VALUES (?, 'simplefin', 'SimpleFIN Bridge', ?, 'active', ?, ?)",
            rusqlite::params![uid, &url_clone, now, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!(
        "[financial] SimpleFIN connected for user {} (account #{})",
        uid, account_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "account_id": account_id,
    })))
}

/// POST /api/financial/simplefin/sync
///
/// Pull accounts and transactions from SimpleFIN. The access URL contains
/// embedded basic-auth credentials.
pub async fn handle_simplefin_sync(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SimplefinSyncRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Load access URL
    let (acct_db_id, access_url) = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<(i64, String), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            conn.query_row(
                "SELECT id, access_token FROM connected_accounts WHERE user_id = ? AND provider = 'simplefin' AND status = 'active'",
                rusqlite::params![uid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(|_| "SimpleFIN not connected.".to_string())
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
    };

    // Fetch accounts (the access URL already has credentials embedded)
    let accounts_url = format!("{}/accounts", access_url.trim_end_matches('/'));
    let resp = state
        .client
        .get(&accounts_url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] SimpleFIN fetch failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("SimpleFIN error: {}", e))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "SimpleFIN returned {}: {}",
                status,
                &text[..text.len().min(200)]
            ),
        ));
    }

    let data: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        (StatusCode::BAD_GATEWAY, format!("Invalid JSON: {}", e))
    })?;

    let sf_accounts = data["accounts"].as_array().cloned().unwrap_or_default();
    let mut total_txns = 0usize;

    for sf_acct in &sf_accounts {
        let sf_acct_id = sf_acct["id"].as_str().unwrap_or("");
        let sf_acct_name = sf_acct["name"].as_str().unwrap_or("Unknown");
        let transactions = sf_acct["transactions"].as_array().cloned().unwrap_or_default();
        let txn_count = transactions.len();

        let db3 = db.clone();
        let acct_name = sf_acct_name.to_string();
        let acct_ext_id = sf_acct_id.to_string();

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db3).map_err(|e| e.to_string())?;
            ensure_financial_tables(&conn)?;
            let now = chrono::Utc::now().timestamp();

            for txn in &transactions {
                let ext_id = txn["id"].as_str().unwrap_or("");
                if ext_id.is_empty() {
                    continue;
                }

                let description = txn["description"]
                    .as_str()
                    .or_else(|| txn["payee"].as_str())
                    .unwrap_or("Unknown");
                let amount = txn["amount"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| txn["amount"].as_f64())
                    .unwrap_or(0.0);
                let amount_cents = (amount * 100.0).round() as i64;

                let posted = txn["posted"]
                    .as_i64()
                    .or_else(|| txn["transacted_at"].as_i64())
                    .unwrap_or(0);
                let date = if posted > 0 {
                    chrono::DateTime::from_timestamp(posted, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_default()
                } else {
                    txn["date"].as_str().unwrap_or("").to_string()
                };

                let pending = txn["pending"].as_bool().unwrap_or(false) as i64;

                conn.execute(
                    "INSERT OR IGNORE INTO financial_transactions \
                     (user_id, account_id, provider, external_id, name, amount_cents, date, merchant_name, pending, created_at) \
                     VALUES (?, ?, 'simplefin', ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        uid, acct_db_id, ext_id, description, amount_cents, &date, description, pending, now
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        total_txns += txn_count;
    }

    // Update sync timestamp
    let db6 = db.clone();
    let now = chrono::Utc::now().timestamp();
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = rusqlite::Connection::open(&db6) {
            let _ = conn.execute(
                "UPDATE connected_accounts SET updated_at = ? WHERE id = ?",
                rusqlite::params![now, acct_db_id],
            );
        }
    })
    .await;

    info!(
        "[financial] SimpleFIN sync for user {}: {} accounts, {} txns",
        uid,
        sf_accounts.len(),
        total_txns
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "accounts_found": sf_accounts.len(),
        "transactions_synced": total_txns,
    })))
}

/// Base64 decode helper — supports both standard and URL-safe variants.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let cleaned = input.trim();
    // Try standard first, then URL-safe
    base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(cleaned))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cleaned))
        .map_err(|e| format!("Base64 decode error: {}", e))
}

// ════════════════════════════════════════════════════════════════════════════
//  INVESTMENTS (aggregated views)
// ════════════════════════════════════════════════════════════════════════════

/// GET /api/financial/investments/summary
///
/// Aggregated investment data across all connected brokers (Alpaca + Coinbase).
pub async fn handle_investment_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;

        // Positions by provider
        let mut stmt = conn
            .prepare(
                "SELECT provider, symbol, qty, avg_cost_cents, current_price_cents, \
                 market_value_cents, unrealized_pl_cents, asset_class, updated_at \
                 FROM investment_positions WHERE user_id = ? ORDER BY market_value_cents DESC",
            )
            .map_err(|e| e.to_string())?;

        let positions: Vec<serde_json::Value> = stmt
            .query_map(rusqlite::params![uid], |r| {
                let mv: i64 = r.get(5)?;
                let upl: i64 = r.get(6)?;
                Ok(serde_json::json!({
                    "provider": r.get::<_, String>(0)?,
                    "symbol": r.get::<_, String>(1)?,
                    "qty": r.get::<_, f64>(2)?,
                    "avg_cost_cents": r.get::<_, Option<i64>>(3)?,
                    "avg_cost_display": r.get::<_, Option<i64>>(3)?.map(cents_to_display),
                    "current_price_cents": r.get::<_, Option<i64>>(4)?,
                    "current_price_display": r.get::<_, Option<i64>>(4)?.map(cents_to_display),
                    "market_value_cents": mv,
                    "market_value_display": cents_to_display(mv),
                    "unrealized_pl_cents": upl,
                    "unrealized_pl_display": cents_to_display(upl),
                    "asset_class": r.get::<_, Option<String>>(7)?,
                    "updated_at": r.get::<_, i64>(8)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        // Totals
        let total_value: i64 = positions
            .iter()
            .filter_map(|p| p["market_value_cents"].as_i64())
            .sum();
        let total_upl: i64 = positions
            .iter()
            .filter_map(|p| p["unrealized_pl_cents"].as_i64())
            .sum();

        // By asset class
        let mut by_class: HashMap<String, i64> = HashMap::new();
        for p in &positions {
            let cls = p["asset_class"]
                .as_str()
                .unwrap_or("other")
                .to_string();
            let mv = p["market_value_cents"].as_i64().unwrap_or(0);
            *by_class.entry(cls).or_insert(0) += mv;
        }
        let allocation: Vec<serde_json::Value> = by_class
            .iter()
            .map(|(cls, val)| {
                serde_json::json!({
                    "asset_class": cls,
                    "market_value_cents": val,
                    "market_value_display": cents_to_display(*val),
                    "pct": if total_value > 0 { (*val as f64 / total_value as f64 * 100.0).round() } else { 0.0 },
                })
            })
            .collect();

        // Count connected providers
        let provider_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT provider) FROM connected_accounts WHERE user_id = ? AND status = 'active' AND provider IN ('alpaca', 'coinbase')",
                rusqlite::params![uid],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Ok(serde_json::json!({
            "positions": positions,
            "total_market_value_cents": total_value,
            "total_market_value_display": cents_to_display(total_value),
            "total_unrealized_pl_cents": total_upl,
            "total_unrealized_pl_display": cents_to_display(total_upl),
            "allocation": allocation,
            "connected_providers": provider_count,
        }))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

/// GET /api/financial/investments/transactions
///
/// List investment transactions with optional filters (provider, symbol, date range).
pub async fn handle_investment_transactions(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let provider_filter = params.get("provider").cloned();
    let symbol_filter = params.get("symbol").cloned();
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();
    let limit: i64 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    let transactions = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        ensure_financial_tables(&conn)?;

        let mut sql = "SELECT id, provider, external_id, symbol, side, qty, price_cents, total_cents, \
                       fee_cents, tx_type, tx_date, description, created_at \
                       FROM investment_transactions WHERE user_id = ?"
            .to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(uid)];

        if let Some(ref prov) = provider_filter {
            sql.push_str(" AND provider = ?");
            params_vec.push(Box::new(prov.clone()));
        }
        if let Some(ref sym) = symbol_filter {
            sql.push_str(" AND symbol = ?");
            params_vec.push(Box::new(sym.clone()));
        }
        if let Some(ref s) = start {
            sql.push_str(" AND tx_date >= ?");
            params_vec.push(Box::new(s.clone()));
        }
        if let Some(ref e) = end {
            sql.push_str(" AND tx_date <= ?");
            params_vec.push(Box::new(e.clone()));
        }
        sql.push_str(" ORDER BY tx_date DESC LIMIT ?");
        params_vec.push(Box::new(limit));

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |r| {
                let price: Option<i64> = r.get(6)?;
                let total: Option<i64> = r.get(7)?;
                let fee: i64 = r.get(8)?;
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?,
                    "provider": r.get::<_, String>(1)?,
                    "external_id": r.get::<_, Option<String>>(2)?,
                    "symbol": r.get::<_, Option<String>>(3)?,
                    "side": r.get::<_, Option<String>>(4)?,
                    "qty": r.get::<_, Option<f64>>(5)?,
                    "price_cents": price,
                    "price_display": price.map(cents_to_display),
                    "total_cents": total,
                    "total_display": total.map(cents_to_display),
                    "fee_cents": fee,
                    "fee_display": cents_to_display(fee),
                    "tx_type": r.get::<_, String>(9)?,
                    "tx_date": r.get::<_, String>(10)?,
                    "description": r.get::<_, Option<String>>(11)?,
                    "created_at": r.get::<_, i64>(12)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "transactions": transactions })))
}

// ════════════════════════════════════════════════════════════════════════════
//  GMAIL RECEIPT PARSING
// ════════════════════════════════════════════════════════════════════════════

/// POST /api/financial/gmail/connect
///
/// Exchange a Google OAuth2 authorization code for tokens and store them.
/// Uses the existing OAuth2 infrastructure.
pub async fn handle_gmail_connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GmailConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    // Look up the Google OAuth2 provider config
    let google_provider = state.config.oauth.providers.get("google").ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Google OAuth2 provider not configured. Add oauth.providers.google to your config.".to_string(),
        )
    })?;

    // Exchange authorization code for tokens
    let resp = state
        .client
        .post(&google_provider.token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &req.oauth_code),
            ("client_id", &google_provider.client_id),
            ("client_secret", &google_provider.client_secret),
            ("redirect_uri", &google_provider.redirect_uri),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Gmail OAuth exchange failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Google OAuth error: {}", e))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "Google OAuth token exchange failed ({}): {}",
                status,
                &text[..text.len().min(200)]
            ),
        ));
    }

    let tokens: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        (StatusCode::BAD_GATEWAY, format!("Invalid JSON: {}", e))
    })?;

    let access_token = tokens["access_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let refresh_token = tokens["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = tokens["expires_in"].as_i64().unwrap_or(3600);

    // Store in oauth_tokens table
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + expires_in;
    let at = access_token.clone();
    let rt = refresh_token.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO oauth_tokens (user_id, provider, access_token, refresh_token, expires_at, scope, created_at, updated_at) \
             VALUES (?, 'gmail', ?, ?, ?, 'https://www.googleapis.com/auth/gmail.readonly', ?, ?)",
            rusqlite::params![uid, &at, &rt, expires_at, now, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!("[financial] Gmail connected for user {}", uid);

    Ok(Json(serde_json::json!({
        "success": true,
        "has_refresh_token": refresh_token.is_some(),
    })))
}

/// POST /api/financial/gmail/scan
///
/// Scan Gmail for receipt/order confirmation emails and extract amounts.
pub async fn handle_gmail_scan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GmailScanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_financial_access(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Load Gmail access token
    let access_token = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            let (token, expires_at): (String, Option<i64>) = conn
                .query_row(
                    "SELECT access_token, expires_at FROM oauth_tokens WHERE user_id = ? AND provider = 'gmail'",
                    rusqlite::params![uid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|_| "Gmail not connected. Use /api/financial/gmail/connect first.".to_string())?;

            // Check if expired
            if let Some(exp) = expires_at {
                let now = chrono::Utc::now().timestamp();
                if now >= exp - 60 {
                    return Err("Gmail access token expired. Re-authorize via /api/financial/gmail/connect.".to_string());
                }
            }
            Ok(token)
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
    };

    // Search for receipt emails
    let query = "subject:(order confirmation OR receipt OR invoice OR payment confirmation) newer_than:90d";
    let max = req.max_results.min(100);

    let search_url = format!(
        "https://gmail.googleapis.com/gmail/v1/users/me/messages?q={}&maxResults={}",
        urlencoding::encode(query),
        max
    );

    let search_resp = state
        .client
        .get(&search_url)
        .bearer_auth(&access_token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            error!("[financial] Gmail search failed: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Gmail API error: {}", e))
        })?;

    let search_status = search_resp.status();
    let search_text = search_resp.text().await.unwrap_or_default();

    if !search_status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "Gmail search failed ({}): {}",
                search_status,
                &search_text[..search_text.len().min(200)]
            ),
        ));
    }

    let search_data: serde_json::Value =
        serde_json::from_str(&search_text).unwrap_or_default();
    let messages = search_data["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut receipts_found: Vec<serde_json::Value> = Vec::new();

    // Fetch each message and extract receipt info
    for msg_ref in messages.iter().take(max as usize) {
        let msg_id = match msg_ref["id"].as_str() {
            Some(id) => id,
            None => continue,
        };

        let msg_url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=From&metadataHeaders=Date",
            msg_id
        );

        let msg_resp = state
            .client
            .get(&msg_url)
            .bearer_auth(&access_token)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;

        let msg_resp = match msg_resp {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let msg_text = msg_resp.text().await.unwrap_or_default();
        let msg_data: serde_json::Value =
            serde_json::from_str(&msg_text).unwrap_or_default();

        let headers = msg_data["payload"]["headers"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut subject = String::new();
        let mut from = String::new();
        let mut date = String::new();

        for header in &headers {
            let name = header["name"].as_str().unwrap_or("");
            let value = header["value"].as_str().unwrap_or("");
            match name {
                "Subject" => subject = value.to_string(),
                "From" => from = value.to_string(),
                "Date" => date = value.to_string(),
                _ => {}
            }
        }

        let snippet = msg_data["snippet"].as_str().unwrap_or("");

        // Try to extract amount from snippet (look for dollar amounts)
        let amount = extract_dollar_amount(snippet);

        receipts_found.push(serde_json::json!({
            "message_id": msg_id,
            "subject": subject,
            "from": from,
            "date": date,
            "snippet": &snippet[..snippet.len().min(200)],
            "amount_cents": amount,
            "amount_display": amount.map(cents_to_display),
        }));
    }

    info!(
        "[financial] Gmail scan for user {}: found {} potential receipts from {} messages",
        uid,
        receipts_found.iter().filter(|r| r["amount_cents"].is_number()).count(),
        messages.len()
    );

    Ok(Json(serde_json::json!({
        "messages_searched": messages.len(),
        "receipts": receipts_found,
    })))
}

/// Extract a dollar amount from text (e.g. "$49.99", "$1,234.56").
/// Returns amount in cents if found.
fn extract_dollar_amount(text: &str) -> Option<i64> {
    // Find pattern like $XX.XX or $X,XXX.XX
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_digit() || bytes[end] == b',' || bytes[end] == b'.')
            {
                end += 1;
            }
            if end > start {
                let num_str: String = text[start..end]
                    .chars()
                    .filter(|c| *c != ',')
                    .collect();
                if let Some(cents) = parse_cents(&num_str) {
                    if cents > 0 && cents < 100_000_00 {
                        // sanity: < $100k
                        return Some(cents);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Simple URL encoding for query parameters.
mod urlencoding {
    pub fn encode(input: &str) -> String {
        let mut result = String::with_capacity(input.len() * 3);
        for b in input.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push('%');
                    result.push(char::from(HEX_DIGITS[(b >> 4) as usize]));
                    result.push(char::from(HEX_DIGITS[(b & 0xf) as usize]));
                }
            }
        }
        result
    }

    const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
}

// ════════════════════════════════════════════════════════════════════════════
//  TESTS
// ════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_dollar_amount() {
        assert_eq!(extract_dollar_amount("Your order total: $49.99"), Some(4999));
        assert_eq!(
            extract_dollar_amount("Payment of $1,234.56 received"),
            Some(123456)
        );
        assert_eq!(extract_dollar_amount("$0.99 charge"), Some(99));
        assert_eq!(extract_dollar_amount("no amount here"), None);
        assert_eq!(extract_dollar_amount("$0.00 free"), None); // zero excluded
    }

    #[test]
    fn test_base64_decode() {
        let encoded = "aHR0cHM6Ly9leGFtcGxlLmNvbQ==";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "https://example.com");
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(
            urlencoding::encode("hello world"),
            "hello%20world"
        );
        assert_eq!(
            urlencoding::encode("subject:(receipt OR invoice)"),
            "subject%3A%28receipt%20OR%20invoice%29"
        );
    }

    #[test]
    fn test_hmac_sha256() {
        // RFC 4231 test vector 2
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let expected = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        let result = hex::encode(hmac_sha256(key, data));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_integrations_config_deserialize() {
        let json = r#"{"plaid": {"client_id": "abc", "secret": "def"}}"#;
        let config: IntegrationsConfig = serde_json::from_str(json).unwrap();
        assert!(config.plaid.is_some());
        assert_eq!(config.plaid.unwrap().client_id, "abc");
        assert!(config.stripe.is_none());
    }

    #[test]
    fn test_plaid_base_url() {
        let sandbox = PlaidConfig {
            client_id: "x".into(),
            secret: "y".into(),
            environment: "sandbox".into(),
        };
        assert_eq!(sandbox.base_url(), "https://sandbox.plaid.com");

        let prod = PlaidConfig {
            client_id: "x".into(),
            secret: "y".into(),
            environment: "production".into(),
        };
        assert_eq!(prod.base_url(), "https://production.plaid.com");
    }

    #[test]
    fn test_alpaca_base_url() {
        let paper = AlpacaConfig {
            api_key: "x".into(),
            api_secret: "y".into(),
            environment: "paper".into(),
        };
        assert_eq!(paper.base_url(), "https://paper-api.alpaca.markets");

        let live = AlpacaConfig {
            api_key: "x".into(),
            api_secret: "y".into(),
            environment: "live".into(),
        };
        assert_eq!(live.base_url(), "https://api.alpaca.markets");
    }
}
