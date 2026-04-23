//! **One-time** vendor-cloud harvest for aidot smart bulbs. Runs at
//! setup, writes an [`Inventory`] to disk, and is never called again at
//! runtime.
//!
//! Split out of `rust-aidot` (2026-04-22) to keep `rsa` (RUSTSEC-2023-0071,
//! no upstream fix) off the main workspace Cargo.lock. Our RSA usage is
//! encryption-only against the vendor's published public key, so the
//! Marvin timing attack (a server-side decryption-timing sidechannel)
//! isn't reachable — but cargo-audit can't reason about that, and we'd
//! rather not suppress a warning when we can architecturally fix it.
//!
//! Ported from `python-aidot`'s `client.py` (login + houses + devices
//! endpoints). RSA-PKCS1v15 encrypt the password with the vendor's
//! published public key, POST JSON, capture bearer token, enumerate
//! houses, enumerate devices per house.

use base64::Engine as _;
use rand::rngs::OsRng;
use rsa::pkcs1v15::Pkcs1v15Encrypt;
use rsa::pkcs8::DecodePublicKey;
use rsa::RsaPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rust_aidot::{Inventory, InventoryDevice};

const BASE_URL: &str = "https://prod-us-api.arnoo.com/v17";
const APP_ID: &str = "1383974540041977857";
const DEFAULT_TERMINAL_ID: &str = "gvz3gjae10l4zii00t7y0";
const RSA_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCtQAnPCi8ksPnS1Du6z96PsKfN
p2Gp/f/bHwlrAdplbX3p7/TnGpnbJGkLq8uRxf6cw+vOthTsZjkPCF7CatRvRnTj
c9fcy7yE0oXa5TloYyXD6GkxgftBbN/movkJJGQCc7gFavuYoAdTRBOyQoXBtm0m
kXMSjXOldI/290b9BQIDAQAB
-----END PUBLIC KEY-----";

#[derive(Debug, thiserror::Error)]
pub enum HarvestError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RSA: {0}")]
    Rsa(#[from] rsa::Error),
    #[error("RSA key parse: {0}")]
    RsaKey(#[from] rsa::pkcs8::spki::Error),
    #[error("login failed: {body}")]
    LoginFailed { body: String },
    #[error("unexpected response shape: missing field {0:?}")]
    MissingField(&'static str),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize)]
struct LoginResp {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    id: Option<String>,
    code: Option<i64>,
    msg: Option<String>,
    #[serde(flatten)]
    _extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct House {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct RawDevice {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}

pub async fn harvest(
    email: &str,
    password: &str,
    country_name: &str,
) -> Result<Inventory, HarvestError> {
    let http = reqwest::Client::builder().use_rustls_tls().build()?;

    let pk = RsaPublicKey::from_public_key_pem(RSA_PUBLIC_KEY_PEM)?;
    let mut rng = OsRng;
    let encrypted = pk.encrypt(&mut rng, Pkcs1v15Encrypt, password.as_bytes())?;
    let encrypted_b64 = base64::engine::general_purpose::STANDARD.encode(encrypted);

    let login_body = json!({
        "countryKey": format!("region:{country_name}"),
        "username": email,
        "password": encrypted_b64,
        "terminalId": DEFAULT_TERMINAL_ID,
        "webVersion": "0.5.0",
        "area": "Asia/Shanghai",
        "UTC": "UTC+8",
    });
    let resp = http
        .post(format!("{BASE_URL}/users/loginWithFreeVerification"))
        .header("Appid", APP_ID)
        .header("Terminal", "app")
        .json(&login_body)
        .send()
        .await?;
    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        return Err(HarvestError::LoginFailed { body: body_text });
    }
    let login: LoginResp = serde_json::from_str(&body_text)?;
    if login.access_token.is_none() {
        return Err(HarvestError::LoginFailed {
            body: format!(
                "code={:?} msg={:?} body={body_text}",
                login.code, login.msg
            ),
        });
    }
    let token = login.access_token.unwrap();
    let user_id = login.id.ok_or(HarvestError::MissingField("id"))?;

    let houses: Vec<House> = http
        .get(format!("{BASE_URL}/houses"))
        .header("Appid", APP_ID)
        .header("Terminal", "app")
        .header("Token", &token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut devices: Vec<InventoryDevice> = Vec::new();
    for house in &houses {
        let raw: Vec<RawDevice> = http
            .get(format!("{BASE_URL}/devices?houseId={}", house.id))
            .header("Appid", APP_ID)
            .header("Terminal", "app")
            .header("Token", &token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for rd in raw {
            let d: InventoryDevice = serde_json::from_value(Value::Object(rd.fields))?;
            devices.push(d);
        }
    }

    Ok(Inventory {
        user_id,
        country_code: "US".into(),
        devices,
    })
}
