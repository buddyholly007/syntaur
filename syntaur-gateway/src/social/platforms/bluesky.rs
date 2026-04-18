//! Bluesky adapter. AT Protocol, app-password auth model.
//!
//! No OAuth — the user generates an "app password" from bsky.app's
//! settings and pastes it into the wizard along with their handle. We
//! call `com.atproto.server.createSession` to exchange those for a JWT
//! pair (access + refresh) that can be used on behalf of the account.
//!
//! Stored credential shape:
//! ```json
//! {
//!   "handle":     "crimsonlanternsong.bsky.social",
//!   "password":   "xxxx-xxxx-xxxx-xxxx",           // app password, NOT the main one
//!   "did":        "did:plc:xxxx",
//!   "access_jwt":  "...",
//!   "refresh_jwt": "...",
//!   "last_session_at": 1734567890
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::{
    AuthFlow, ConnectInput, PlatformDescriptor, RefreshedCredentials, SocialError,
    SocialPlatform, StoredCredentials, VerifiedIdentity, WizardStep,
};

const BSKY_API: &str = "https://bsky.social/xrpc";

pub struct Bluesky;

#[derive(Deserialize)]
struct CreateSessionResponse {
    #[serde(default)]
    access_jwt:  Option<String>,
    #[serde(rename = "accessJwt", default)]
    access_jwt_camel:  Option<String>,
    #[serde(default)]
    refresh_jwt: Option<String>,
    #[serde(rename = "refreshJwt", default)]
    refresh_jwt_camel: Option<String>,
    #[serde(default)]
    did: Option<String>,
    #[serde(default)]
    handle: Option<String>,
}

impl CreateSessionResponse {
    fn access(&self) -> Option<&str> {
        self.access_jwt.as_deref().or(self.access_jwt_camel.as_deref())
    }
    fn refresh(&self) -> Option<&str> {
        self.refresh_jwt.as_deref().or(self.refresh_jwt_camel.as_deref())
    }
}

#[derive(Deserialize)]
struct GetSessionResponse {
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    did: Option<String>,
}

#[async_trait]
impl SocialPlatform for Bluesky {
    fn id(&self) -> &'static str { "bluesky" }

    fn descriptor(&self) -> PlatformDescriptor {
        PlatformDescriptor {
            id: "bluesky",
            display_name: "Bluesky",
            tagline: "AT Protocol app password",
            tone: "skyblue",
            auth_flow: AuthFlow::AppPassword {
                field_labels: ["Handle", "App password"],
                field_helps:  [
                    "Your full handle — e.g. yourname.bsky.social",
                    "An app password, NOT your main Bluesky password. Create one under Settings → Privacy & Security → App Passwords.",
                ],
                setup_steps: vec![
                    WizardStep {
                        title: "Open Bluesky app-password settings",
                        body_md: "Sign in to the Bluesky web app, then go to **Settings → Privacy and Security → App Passwords**.",
                        deep_link: Some("https://bsky.app/settings/app-passwords"),
                        copy_target: None,
                    },
                    WizardStep {
                        title: "Create a new app password",
                        body_md: "Click **Add App Password**. Give it a name like `Syntaur` so you know where it's used. Bluesky will show you the password once — copy it before closing the dialog.",
                        deep_link: None,
                        copy_target: None,
                    },
                    WizardStep {
                        title: "Paste into the wizard",
                        body_md: "Back here, paste your handle (including the `.bsky.social` suffix, or your custom domain) and the app password. Syntaur exchanges them for a session token and never sends the app password anywhere else.",
                        deep_link: None,
                        copy_target: Some("App password"),
                    },
                ],
            },
        }
    }

    async fn verify(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<VerifiedIdentity, SocialError> {
        let access = creds.get("access_jwt").and_then(|v| v.as_str()).unwrap_or("");
        if access.is_empty() {
            // No session yet — fall back to presence of handle+password.
            // A full `refresh` is the right answer, but for a bare verify
            // we just confirm the saved creds look usable.
            let handle = creds.get("handle").and_then(|v| v.as_str()).unwrap_or("");
            let password = creds.get("password").and_then(|v| v.as_str()).unwrap_or("");
            if handle.is_empty() || password.is_empty() {
                return Err(SocialError::AuthInvalid {
                    detail: "No session and no app password on file.".to_string(),
                });
            }
            return Ok(VerifiedIdentity {
                display_name: handle.to_string(),
                details: Some("No active session yet — will create one on next use.".to_string()),
            });
        }
        let url = format!("{}/com.atproto.server.getSession", BSKY_API);
        let resp = http.get(&url)
            .bearer_auth(access)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let body: GetSessionResponse = resp.json().await
                    .map_err(|e| SocialError::Unknown(format!("bad JSON from getSession: {}", e)))?;
                Ok(VerifiedIdentity {
                    display_name: body.handle.unwrap_or_default(),
                    details: body.did,
                })
            }
            401 | 400 => Err(SocialError::AuthExpired {
                hint: "Your Bluesky session timed out. Reconnect to generate a new one.",
            }),
            429 => Err(SocialError::RateLimit { retry_after_secs: None }),
            500..=599 => Err(SocialError::PlatformDown {
                status_url: "https://status.bsky.app/",
            }),
            code => Err(SocialError::Unknown(format!("getSession returned HTTP {}", code))),
        }
    }

    async fn reconnect(
        &self,
        http: &reqwest::Client,
        input: &ConnectInput,
    ) -> Result<StoredCredentials, SocialError> {
        let handle = input.fields.get("handle").and_then(|v| v.as_str())
            .ok_or_else(|| SocialError::InvalidInput {
                field: "handle",
                reason: "missing".to_string(),
            })?
            .trim()
            .trim_start_matches('@');
        if handle.is_empty() {
            return Err(SocialError::InvalidInput {
                field: "handle",
                reason: "can't be blank — use something like yourname.bsky.social".to_string(),
            });
        }
        let password = input.fields.get("app_password")
            .or_else(|| input.fields.get("password"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| SocialError::InvalidInput {
                field: "app_password",
                reason: "missing".to_string(),
            })?
            .trim();
        if password.is_empty() {
            return Err(SocialError::InvalidInput {
                field: "app_password",
                reason: "generate one on bsky.app under Settings → App Passwords".to_string(),
            });
        }

        let url = format!("{}/com.atproto.server.createSession", BSKY_API);
        let body = serde_json::json!({ "identifier": handle, "password": password });
        let resp = http.post(&url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let parsed: CreateSessionResponse = resp.json().await
                    .map_err(|e| SocialError::Unknown(format!("bad JSON from createSession: {}", e)))?;
                let access  = parsed.access().ok_or_else(|| SocialError::Unknown("response missing accessJwt".to_string()))?.to_string();
                let refresh = parsed.refresh().ok_or_else(|| SocialError::Unknown("response missing refreshJwt".to_string()))?.to_string();
                let did = parsed.did.clone().unwrap_or_default();
                let resolved_handle = parsed.handle.clone().unwrap_or_else(|| handle.to_string());
                let now = chrono::Utc::now().timestamp();
                let stored = serde_json::json!({
                    "handle": resolved_handle,
                    "password": password,   // kept so we can re-createSession when refresh_jwt ages out
                    "did": did,
                    "access_jwt": access,
                    "refresh_jwt": refresh,
                    "last_session_at": now,
                });
                Ok(StoredCredentials {
                    display_name: resolved_handle,
                    credentials: stored,
                    expires_at: None,  // Bluesky JWTs refresh-on-use, not time-bound at our layer
                })
            }
            400 | 401 => Err(SocialError::AuthInvalid {
                detail: "Handle or app password didn't match. Double-check for stray spaces, or generate a fresh app password.".to_string(),
            }),
            429 => Err(SocialError::RateLimit { retry_after_secs: None }),
            500..=599 => Err(SocialError::PlatformDown {
                status_url: "https://status.bsky.app/",
            }),
            code => Err(SocialError::Unknown(format!("createSession returned HTTP {}", code))),
        }
    }

    async fn refresh(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<RefreshedCredentials, SocialError> {
        // Bluesky's session tokens expire; we exchange the refresh_jwt for
        // a new pair. If refresh_jwt is itself stale, fall back to
        // re-createSession with the stored app password.
        let refresh_jwt = creds.get("refresh_jwt").and_then(|v| v.as_str()).unwrap_or("");
        if !refresh_jwt.is_empty() {
            let url = format!("{}/com.atproto.server.refreshSession", BSKY_API);
            let resp = http.post(&url)
                .bearer_auth(refresh_jwt)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| SocialError::Network(e.to_string()))?;
            if resp.status().as_u16() == 200 {
                let parsed: CreateSessionResponse = resp.json().await
                    .map_err(|e| SocialError::Unknown(format!("bad JSON from refreshSession: {}", e)))?;
                let access  = parsed.access().unwrap_or("").to_string();
                let refresh = parsed.refresh().unwrap_or(refresh_jwt).to_string();
                let mut updated = creds.clone();
                updated["access_jwt"]  = serde_json::Value::String(access);
                updated["refresh_jwt"] = serde_json::Value::String(refresh);
                updated["last_session_at"] = serde_json::Value::Number(chrono::Utc::now().timestamp().into());
                return Ok(RefreshedCredentials { credentials: updated, expires_at: None });
            }
            // fall through to createSession below
        }
        // refresh_jwt is missing or stale — use stored handle+password.
        let fields = serde_json::json!({
            "handle": creds.get("handle").cloned().unwrap_or(serde_json::Value::Null),
            "app_password": creds.get("password").cloned().unwrap_or(serde_json::Value::Null),
        });
        let fresh = self.reconnect(http, &ConnectInput { fields }).await?;
        Ok(RefreshedCredentials { credentials: fresh.credentials, expires_at: fresh.expires_at })
    }
}
