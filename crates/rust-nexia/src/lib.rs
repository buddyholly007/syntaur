//! Rust client for Trane / Nexia / Asair residential-HVAC cloud API
//! (`mynexia.com`, `tranehome.com`, `asairhome.com`).
//!
//! **Runtime reality**: Nexia thermostats expose NO local LAN API.
//! All control flows through the vendor's cloud via a reverse tunnel
//! the thermostat opens on boot. Unlike aidot/Tapo, this crate is
//! cloud-bound at runtime, not just at setup. See
//! [[projects/trane_nexia_thermostat]] in the vault for the honest
//! tradeoff analysis.
//!
//! Primary purpose of this crate: **replace Home Assistant's `nexia`
//! integration + python-nexia**, so Syntaur is the sole platform even
//! when the underlying protocol is vendor cloud. Secondary purpose
//! (the immediate motivation): **probe the cloud API surface** for
//! undocumented LAN-adjacent fields — Z-Wave controller info, local
//! IP, Matter capability, OTA status.
//!
//! Wire protocol (ported from `hvaclibs/nexia` Python lib):
//! 1. `POST /mobile/accounts/sign_in` `{login, password, device_uuid, ...}`
//!    → `{result: {mobile_id, api_key}}`
//! 2. Auth headers on every subsequent request:
//!    `X-AppVersion: 6.0.0`, `X-AssociatedBrand: trane`,
//!    `X-MobileId: <id>`, `X-ApiKey: <key>`
//! 3. `GET /mobile/houses/{house_id}` → device tree
//! 4. Arbitrary JSON REST for every other operation (hypermedia-ish).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

pub mod thermostat;
pub use thermostat::{FanMode, HvacMode, RunMode, Thermostat, Zone};

/// Brand selector — determines the root URL.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Brand {
    Nexia,
    Trane,
    Asair,
}

impl Brand {
    pub fn root_url(self) -> &'static str {
        match self {
            Self::Nexia => "https://www.mynexia.com",
            Self::Trane => "https://www.tranehome.com",
            Self::Asair => "https://asairhome.com",
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Self::Nexia => "nexia",
            Self::Trane => "trane",
            Self::Asair => "asair",
        }
    }
}

/// Vendor's expected app-version header. Tracks python-nexia; bumped
/// occasionally when the backend deprecates older clients.
pub const APP_VERSION: &str = "6.0.0";

#[derive(Debug, Error)]
pub enum NexiaError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("login failed: {0}")]
    LoginFailed(String),
    #[error("unexpected response shape (missing {0})")]
    MissingField(&'static str),
    #[error("API error {status}: {body}")]
    ApiError { status: u16, body: String },
}

/// Authenticated client — holds mobile_id + api_key after login.
pub struct NexiaClient {
    http: reqwest::Client,
    brand: Brand,
    device_uuid: Uuid,
    device_name: String,
    mobile_id: Option<u64>,
    api_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SignInPayload<'a> {
    login: &'a str,
    password: &'a str,
    children: Vec<Value>,
    #[serde(rename = "childSchemas")]
    child_schemas: Vec<Value>,
    #[serde(rename = "commitModel")]
    commit_model: Option<Value>,
    #[serde(rename = "nextHref")]
    next_href: Option<Value>,
    device_uuid: String,
    device_name: &'a str,
    app_version: &'a str,
    is_commercial: bool,
}

#[derive(Debug, Deserialize)]
struct SignInResp {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    result: Option<SignInResult>,
}

#[derive(Debug, Deserialize)]
struct SignInResult {
    mobile_id: u64,
    api_key: String,
}

impl NexiaClient {
    pub fn new(brand: Brand) -> Self {
        Self {
            http: reqwest::Client::builder()
                .use_rustls_tls()
                .cookie_store(true)
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .expect("build client"),
            brand,
            device_uuid: Uuid::new_v4(),
            device_name: "Syntaur".into(),
            mobile_id: None,
            api_key: None,
        }
    }

    /// Log in. On success, `mobile_id` + `api_key` are set for the
    /// duration of this client. The credentials are short-lived
    /// per-device tokens — Nexia can invalidate them if you sign in
    /// from too many different device_uuids, so reuse the same UUID
    /// where possible (persist the client across calls, or save the
    /// uuid to disk).
    pub async fn login(&mut self, email: &str, password: &str) -> Result<(), NexiaError> {
        let url = format!("{}/mobile/accounts/sign_in", self.brand.root_url());
        let payload = SignInPayload {
            login: email,
            password,
            children: vec![],
            child_schemas: vec![],
            commit_model: None,
            next_href: None,
            device_uuid: self.device_uuid.to_string(),
            device_name: &self.device_name,
            app_version: APP_VERSION,
            is_commercial: false,
        };
        let resp = self
            .http
            .post(&url)
            .header("X-AppVersion", APP_VERSION)
            .header("X-AssociatedBrand", self.brand.tag())
            .json(&payload)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(NexiaError::LoginFailed(format!(
                "HTTP {status}: {body}"
            )));
        }
        let r: SignInResp = serde_json::from_str(&body)?;
        if !r.success {
            return Err(NexiaError::LoginFailed(
                r.error.unwrap_or_else(|| "unknown error".into()),
            ));
        }
        let res = r.result.ok_or(NexiaError::MissingField("result"))?;
        self.mobile_id = Some(res.mobile_id);
        self.api_key = Some(res.api_key);
        Ok(())
    }

    /// Generic authenticated GET — returns raw JSON. Use for probing
    /// / development; wrap with typed helpers when shapes stabilize.
    pub async fn get_raw(&self, path: &str) -> Result<Value, NexiaError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.brand.root_url(), path)
        };
        let resp = self.http.get(&url).headers(self.auth_headers()).send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(NexiaError::ApiError {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// Generic authenticated POST — returns raw JSON.
    pub async fn post_raw(&self, path: &str, body: Value) -> Result<Value, NexiaError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.brand.root_url(), path)
        };
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(NexiaError::ApiError {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        use reqwest::header::HeaderValue;
        h.insert("X-AppVersion", HeaderValue::from_static(APP_VERSION));
        h.insert(
            "X-AssociatedBrand",
            HeaderValue::from_str(self.brand.tag()).unwrap(),
        );
        if let Some(id) = self.mobile_id {
            if let Ok(v) = HeaderValue::from_str(&id.to_string()) {
                h.insert("X-MobileId", v);
            }
        }
        if let Some(ref k) = self.api_key {
            if let Ok(v) = HeaderValue::from_str(k) {
                h.insert("X-ApiKey", v);
            }
        }
        h
    }

    pub fn mobile_id(&self) -> Option<u64> {
        self.mobile_id
    }
    pub fn api_key_present(&self) -> bool {
        self.api_key.is_some()
    }
}
