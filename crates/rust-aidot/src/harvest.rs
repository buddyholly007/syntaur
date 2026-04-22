//! **One-time** vendor-cloud harvest. Runs at setup, writes an
//! [`Inventory`] to disk, and is never called again at runtime.
//!
//! Takes the user's aidot email + password, calls
//! `prod-us-api.arnoo.com/v17`, and persists per-device AES keys +
//! passwords for use by [`DeviceClient`][crate::DeviceClient].
//!
//! Ported from `python-aidot`'s `client.py` (login + houses + devices
//! endpoints). RSA-PKCS1v15 encrypt the password with the vendor's
//! published public key, POST JSON, capture bearer token, enumerate
//! houses, enumerate devices per house.
//!
//! Vendor can rotate keys / change the API. Both are rare but possible.
//! If login fails after a firmware update or vendor API change, the
//! user re-runs harvest with current creds. Runtime LAN control keeps
//! working until the per-device AES key rotates (only on factory-reset
//! or cloud unlink).

use base64::Engine as _;
use rand::rngs::OsRng;
use rsa::pkcs1v15::Pkcs1v15Encrypt;
use rsa::pkcs8::DecodePublicKey;
use rsa::RsaPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Inventory, InventoryDevice};

/// Base URL of aidot's US API. The region prefix is `us` for North
/// American accounts; an EU-hosted aidot account would use
/// `prod-eu-api.arnoo.com`. Hard-coding US is fine for now; make this a
/// parameter if/when we see a non-US Syntaur user.
const BASE_URL: &str = "https://prod-us-api.arnoo.com/v17";
/// App identifier aidot's app sends. The server checks it; we echo the
/// value from the vendor Python library.
const APP_ID: &str = "1383974540041977857";
/// Stable "device installation" id — any 20-char alnum works. The
/// vendor library uses `"gvz3gjae10l4zii00t7y0"` as a fallback; we
/// reuse it so the server recognizes our client shape. Syntaur may
/// generate + persist its own per-install ID in a later refinement.
const DEFAULT_TERMINAL_ID: &str = "gvz3gjae10l4zii00t7y0";
/// Vendor's RSA public key for the password-encryption field on login.
/// 1024-bit RSA, PKCS1 v1.5 padding. Extracted from `aidot/login_const.py`.
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
    /// Everything else ignored.
    #[serde(flatten)]
    _extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct House {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct RawDevice {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}

/// One-time harvest: log in, fetch houses, fetch devices per house,
/// fold into an [`Inventory`] ready to persist.
///
/// The country name is used in `countryKey: "region:<country>"`; aidot
/// supports a fixed set (`"United States"`, `"Canada"`, …). Wrong value
/// produces a 400 at the server with a clear code.
pub async fn harvest(
    email: &str,
    password: &str,
    country_name: &str,
) -> Result<Inventory, HarvestError> {
    let http = reqwest::Client::builder()
        .use_rustls_tls()
        .build()?;

    // Step 1 — login. Encrypt the password with vendor's RSA public key.
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

    // Step 2 — houses.
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

    // Step 3 — devices per house.
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
        country_code: "US".into(), // harvest always runs against the US backend for now
        devices,
    })
}
