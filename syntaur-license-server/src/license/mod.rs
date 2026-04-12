//! License server module — Stripe checkout, webhook handling, Ed25519 license generation.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use log::{error, info};
use rand::rngs::OsRng;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::config::LicenseConfig;

const PRODUCT_NAME: &str = "Syntaur Pro";
const PRICE_AMOUNT: u64 = 4900;

pub struct LicenseState {
    pub db: Mutex<Connection>,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub config: LicenseConfig,
    pub server_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct LicensePayload {
    email: String,
    tier: String,
    expires_at: u64,
    modules: Vec<String>,
    issued_at: u64,
    purchase_id: String,
}

#[derive(Serialize, Deserialize)]
struct SignedLicense {
    #[serde(flatten)]
    payload: LicensePayload,
    signature: String,
}

impl LicenseState {
    pub fn new(config: LicenseConfig, server_url: String) -> Arc<Self> {
        let data_dir = dirs_data_dir();
        std::fs::create_dir_all(&data_dir).ok();

        let key_path = format!("{}/signing_key.bin", data_dir);
        let signing_key = load_or_generate_key(&key_path);
        let verifying_key = signing_key.verifying_key();

        let pub_key_path = format!("{}/public_key.txt", data_dir);
        let pub_key_hex = hex::encode(verifying_key.as_bytes());
        std::fs::write(&pub_key_path, &pub_key_hex).ok();
        info!("Public key (embed in Syntaur binary): {}", pub_key_hex);

        let db_path = format!("{}/licenses.db", data_dir);
        let db = Connection::open(&db_path).expect("Failed to open licenses.db");
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS purchases (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL,
                stripe_session_id TEXT,
                stripe_customer_id TEXT,
                license_key TEXT NOT NULL,
                tier TEXT NOT NULL DEFAULT 'professional',
                amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL DEFAULT 'usd',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_email ON purchases(email);
            CREATE INDEX IF NOT EXISTS idx_session ON purchases(stripe_session_id);",
        )
        .expect("Failed to create tables");

        Arc::new(Self {
            db: Mutex::new(db),
            signing_key,
            verifying_key,
            config,
            server_url,
        })
    }
}

pub fn license_routes() -> Router<Arc<LicenseState>> {
    Router::new()
        .route("/", get(handle_home))
        .route("/checkout", get(handle_checkout))
        .route("/success", get(handle_success))
        .route("/stripe/webhook", post(handle_webhook))
        .route("/verify", post(handle_verify))
}

async fn handle_home() -> Html<&'static str> {
    Html(r#"<!DOCTYPE html>
<html><head><title>Syntaur Pro</title>
<style>body{font-family:system-ui;max-width:600px;margin:80px auto;padding:0 20px;background:#0a0a0a;color:#e5e5e5}
h1{color:#0ea5e9}a{color:#0ea5e9}.btn{display:inline-block;background:#0284c7;color:white;padding:12px 32px;border-radius:8px;text-decoration:none;font-weight:600;margin-top:20px}</style></head>
<body>
<h1>Syntaur Pro</h1>
<p>Unlock premium modules: voice assistant, smart home control, social media automation, finance tools, and browser automation.</p>
<p><strong>$49</strong> — one-time purchase, perpetual license.</p>
<a href="/checkout" class="btn">Buy Syntaur Pro →</a>
<p style="margin-top:40px;color:#666;font-size:14px">Already have a key? Paste it into Syntaur → Settings → License.</p>
</body></html>"#)
}

async fn handle_checkout(
    State(state): State<Arc<LicenseState>>,
) -> Result<Redirect, (StatusCode, String)> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .basic_auth(&state.config.stripe_secret_key, None::<&str>)
        .form(&[
            ("mode", "payment"),
            ("line_items[0][price]", &state.config.stripe_price_id),
            ("line_items[0][quantity]", "1"),
            (
                "success_url",
                &format!(
                    "{}/success?session_id={{CHECKOUT_SESSION_ID}}",
                    state.server_url
                ),
            ),
            ("cancel_url", &format!("{}/", state.server_url)),
        ])
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Stripe error: {}", e),
            )
        })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Parse error: {}", e),
        )
    })?;

    let checkout_url = body
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("Stripe response missing URL: {:?}", body);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stripe error".to_string(),
            )
        })?;

    Ok(Redirect::temporary(checkout_url))
}

async fn handle_success(
    State(state): State<Arc<LicenseState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Html<String> {
    let session_id = params.get("session_id").cloned().unwrap_or_default();

    let db = state.db.lock().await;
    let license_key: Option<String> = db
        .query_row(
            "SELECT license_key FROM purchases WHERE stripe_session_id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .ok();

    let content = if let Some(key) = license_key {
        format!(
            r#"<h1>Thank you!</h1>
<p>Your Syntaur Pro license key:</p>
<textarea readonly style="width:100%;height:120px;background:#1a1a1a;color:#0ea5e9;border:1px solid #333;border-radius:8px;padding:12px;font-family:monospace;font-size:12px">{}</textarea>
<p style="margin-top:12px"><button onclick="navigator.clipboard.writeText(document.querySelector('textarea').value)" style="background:#0284c7;color:white;border:none;padding:8px 20px;border-radius:6px;cursor:pointer">Copy Key</button></p>
<p style="color:#888;font-size:14px;margin-top:20px">Paste this key into Syntaur → Settings → License to unlock Pro features.</p>
<p style="color:#888;font-size:14px">A copy has also been sent to your email.</p>"#,
            key
        )
    } else {
        r#"<h1>Processing...</h1>
<p>Your payment is being processed. Your license key will appear here in a moment.</p>
<p><a href="javascript:location.reload()">Refresh</a></p>
<p style="color:#888;font-size:14px">If this takes more than a minute, check your email or contact support.</p>"#.to_string()
    };

    Html(format!(
        r#"<!DOCTYPE html><html><head><title>Syntaur Pro — Thank You</title>
<style>body{{font-family:system-ui;max-width:600px;margin:80px auto;padding:0 20px;background:#0a0a0a;color:#e5e5e5}}
h1{{color:#0ea5e9}}a{{color:#0ea5e9}}</style></head><body>{}</body></html>"#,
        content
    ))
}

async fn handle_webhook(
    State(state): State<Arc<LicenseState>>,
    headers: HeaderMap,
    body: String,
) -> Result<StatusCode, (StatusCode, String)> {
    let sig_header = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !verify_stripe_signature(&body, sig_header, &state.config.stripe_webhook_secret) {
        return Err((StatusCode::BAD_REQUEST, "Invalid signature".to_string()));
    }

    let event: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Bad JSON: {}", e)))?;

    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

    if event_type == "checkout.session.completed" {
        let session = event
            .get("data")
            .and_then(|d| d.get("object"))
            .unwrap_or(&serde_json::Value::Null);
        let session_id = session.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let email = session
            .get("customer_details")
            .and_then(|c| c.get("email"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let customer_id = session
            .get("customer")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let amount = session
            .get("amount_total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let currency = session
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("usd");

        let purchase_id = uuid::Uuid::new_v4().to_string();
        let payload = LicensePayload {
            email: email.to_string(),
            tier: "professional".to_string(),
            expires_at: 0,
            modules: vec![],
            issued_at: Utc::now().timestamp() as u64,
            purchase_id: purchase_id.clone(),
        };

        let payload_json = serde_json::to_string(&payload).unwrap_or_default();
        let signature = state.signing_key.sign(payload_json.as_bytes());
        let sig_hex = hex::encode(signature.to_bytes());

        let signed = SignedLicense {
            payload: payload.clone(),
            signature: sig_hex,
        };

        let license_key = serde_json::to_string(&signed).unwrap_or_default();

        let db = state.db.lock().await;
        db.execute(
            "INSERT OR REPLACE INTO purchases (id, email, stripe_session_id, stripe_customer_id, license_key, tier, amount_cents, currency, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                purchase_id,
                email,
                session_id,
                customer_id,
                license_key,
                "professional",
                amount as i64,
                currency,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?;

        info!(
            "[license] Generated key for {} (session: {})",
            email, session_id
        );
    }

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct VerifyRequest {
    key: String,
}

async fn handle_verify(
    State(state): State<Arc<LicenseState>>,
    axum::Json(req): axum::Json<VerifyRequest>,
) -> axum::Json<serde_json::Value> {
    match verify_license_key(&req.key, &state.verifying_key) {
        Ok(payload) => axum::Json(serde_json::json!({
            "valid": true,
            "email": payload.email,
            "tier": payload.tier,
            "expires_at": payload.expires_at,
        })),
        Err(e) => axum::Json(serde_json::json!({
            "valid": false,
            "error": e,
        })),
    }
}

// ── Crypto ──────────────────────────────────────────────────────────────

fn load_or_generate_key(path: &str) -> SigningKey {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut key_bytes = [0u8; 32];
            key_bytes.copy_from_slice(&bytes);
            info!("Loaded existing signing key");
            return SigningKey::from_bytes(&key_bytes);
        }
    }
    let key = SigningKey::generate(&mut OsRng);
    std::fs::write(path, key.to_bytes()).ok();
    info!("Generated new signing key");
    key
}

fn verify_license_key(
    key_json: &str,
    verifying_key: &VerifyingKey,
) -> Result<LicensePayload, String> {
    let signed: SignedLicense =
        serde_json::from_str(key_json).map_err(|_| "Invalid key format".to_string())?;

    let payload_json =
        serde_json::to_string(&signed.payload).map_err(|_| "Serialization error".to_string())?;

    let sig_bytes =
        hex::decode(&signed.signature).map_err(|_| "Invalid signature encoding".to_string())?;

    let signature = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|_| "Invalid signature".to_string())?;

    verifying_key
        .verify_strict(payload_json.as_bytes(), &signature)
        .map_err(|_| "Signature verification failed".to_string())?;

    if signed.payload.expires_at != 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > signed.payload.expires_at {
            return Err("License expired".to_string());
        }
    }

    Ok(signed.payload)
}

fn verify_stripe_signature(payload: &str, sig_header: &str, secret: &str) -> bool {
    use hmac::{Hmac, Mac};

    let parts: std::collections::HashMap<&str, &str> = sig_header
        .split(',')
        .filter_map(|p| {
            let mut kv = p.splitn(2, '=');
            Some((kv.next()?, kv.next()?))
        })
        .collect();

    let timestamp = match parts.get("t") {
        Some(t) => *t,
        None => return false,
    };
    let expected_sig = match parts.get("v1") {
        Some(s) => *s,
        None => return false,
    };

    let signed_payload = format!("{}.{}", timestamp, payload);
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(signed_payload.as_bytes());
    let computed = hex::encode(mac.finalize().into_bytes());

    computed == expected_sig
}

fn dirs_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    format!("{}/.syntaur-license-server", home)
}
