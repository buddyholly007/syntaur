//! YouTube adapter — Google OAuth2.
//!
//! Phase 3 scope: silent `refresh()` using a stored refresh_token.
//! First-time `reconnect()` (full OAuth2 PKCE dance) is scaffolded but
//! returns a clear "run the OAuth wizard" error for now; the wizard UI
//! is wired in Phase 4.
//!
//! Stored credential shape:
//! ```json
//! {
//!   "access_token":  "ya29...",
//!   "refresh_token": "1//...",   // the durable one — weeks-to-months
//!   "expires_in":    3599,
//!   "issued_at":     1729000000,
//!   "client_id":     "xxxx.apps.googleusercontent.com",
//!   "client_secret": "GOCSPX-...",
//!   "scope":         "https://www.googleapis.com/auth/youtube ..."
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::{
    AuthFlow, ConnectInput, PlatformDescriptor, RefreshedCredentials, SocialError,
    SocialPlatform, StoredCredentials, VerifiedIdentity, WizardStep,
};

const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const YT_CHANNELS_URL: &str = "https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true";

pub struct YouTube;

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct ChannelsResponse {
    items: Option<Vec<ChannelItem>>,
}
#[derive(Deserialize)]
struct ChannelItem {
    id: Option<String>,
    snippet: Option<ChannelSnippet>,
}
#[derive(Deserialize)]
struct ChannelSnippet {
    title: Option<String>,
}

fn s(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

#[async_trait]
impl SocialPlatform for YouTube {
    fn id(&self) -> &'static str { "youtube" }

    fn descriptor(&self) -> PlatformDescriptor {
        PlatformDescriptor {
            id: "youtube",
            display_name: "YouTube",
            tagline: "Google OAuth2 — community posts, comments, and channel stats",
            tone: "crimson",
            auth_flow: AuthFlow::OAuth2 {
                authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
                token_url: GOOGLE_TOKEN_URL,
                scopes: vec![
                    "https://www.googleapis.com/auth/youtube",
                    "https://www.googleapis.com/auth/youtube.force-ssl",
                ],
                requires_user_app: true,
                pkce: true,
                setup_steps: vec![
                    WizardStep {
                        title: "Create (or open) a Google Cloud project",
                        body_md: "Go to **Google Cloud Console**, create a project named `Syntaur` (or reuse an existing one). You'll manage your OAuth app here.",
                        deep_link: Some("https://console.cloud.google.com/"),
                        copy_target: None,
                    },
                    WizardStep {
                        title: "Enable the YouTube Data API v3",
                        body_md: "Under **APIs & Services → Library**, search for `YouTube Data API v3` and enable it for your project.",
                        deep_link: Some("https://console.cloud.google.com/apis/library/youtube.googleapis.com"),
                        copy_target: None,
                    },
                    WizardStep {
                        title: "Configure the OAuth consent screen",
                        body_md: "Under **APIs & Services → OAuth consent screen**, set user type to **External**, add yourself as a test user, and add the two `youtube` scopes Syntaur needs (listed above).",
                        deep_link: Some("https://console.cloud.google.com/apis/credentials/consent"),
                        copy_target: None,
                    },
                    WizardStep {
                        title: "Create OAuth client credentials",
                        body_md: "Under **APIs & Services → Credentials**, click **Create Credentials → OAuth client ID → Desktop app** (or Web app if you want the redirect flow). Download the JSON and keep the `client_id` + `client_secret` handy.",
                        deep_link: Some("https://console.cloud.google.com/apis/credentials"),
                        copy_target: Some("client_id"),
                    },
                    WizardStep {
                        title: "Paste your OAuth credentials",
                        body_md: "In the next step Syntaur will open a Google consent page. Sign in as the channel owner and grant the listed scopes. After consent, Google redirects back with a code that Syntaur exchanges for tokens. **You don't copy the code manually — Syntaur catches it.**",
                        deep_link: None,
                        copy_target: None,
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
        let access = s(creds, "access_token");
        if access.is_empty() {
            return Err(SocialError::AuthInvalid {
                detail: "No access token on file.".to_string(),
            });
        }
        let resp = http.get(YT_CHANNELS_URL)
            .bearer_auth(&access)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let body: ChannelsResponse = resp.json().await
                    .map_err(|e| SocialError::Unknown(format!("bad channels JSON: {}", e)))?;
                let first = body.items.unwrap_or_default().into_iter().next();
                let (title, id) = first
                    .map(|c| (c.snippet.and_then(|s| s.title).unwrap_or_default(), c.id.unwrap_or_default()))
                    .unwrap_or_default();
                Ok(VerifiedIdentity {
                    display_name: if title.is_empty() { "YouTube Channel".to_string() } else { title },
                    details: if id.is_empty() { None } else { Some(id) },
                })
            }
            401 | 403 => Err(SocialError::AuthExpired {
                hint: "Token expired. Reconnect to refresh — your refresh_token should still be valid.",
            }),
            429 => Err(SocialError::RateLimit { retry_after_secs: None }),
            500..=599 => Err(SocialError::PlatformDown {
                status_url: "https://status.cloud.google.com/",
            }),
            code => Err(SocialError::Unknown(format!("channels.list returned HTTP {}", code))),
        }
    }

    /// Full first-time OAuth dance. Phase-4 territory — the wizard UI +
    /// callback plumbing lands in a later session. Today this returns a
    /// clear error pointing to the refresh path if credentials exist.
    async fn reconnect(
        &self,
        _http: &reqwest::Client,
        _input: &ConnectInput,
    ) -> Result<StoredCredentials, SocialError> {
        Err(SocialError::Unknown(
            "Google OAuth first-time connect arrives in an upcoming release. For now: if you already have tokens from an existing integration, import them and use the Reconnect (refresh) path.".to_string()
        ))
    }

    async fn refresh(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<RefreshedCredentials, SocialError> {
        let refresh_token = s(creds, "refresh_token");
        let client_id     = s(creds, "client_id");
        let client_secret = s(creds, "client_secret");
        if refresh_token.is_empty() {
            return Err(SocialError::AuthInvalid {
                detail: "No refresh token on file — run the full Google OAuth connect flow.".to_string(),
            });
        }
        if client_id.is_empty() || client_secret.is_empty() {
            return Err(SocialError::AuthInvalid {
                detail: "OAuth client_id/client_secret missing. Paste them from your Google Cloud Console credentials.".to_string(),
            });
        }

        let form = [
            ("grant_type",    "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id",     client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        let resp = http.post(GOOGLE_TOKEN_URL)
            .form(&form)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| SocialError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        let body: RefreshResponse = resp.json().await
            .map_err(|e| SocialError::Unknown(format!("bad token-refresh JSON: {}", e)))?;

        if let Some(err) = body.error.as_deref() {
            let detail = body.error_description.unwrap_or_default();
            return match err {
                "invalid_grant" => Err(SocialError::AuthExpired {
                    hint: "Refresh token was revoked or expired. You'll need to run the full Google OAuth connect flow again.",
                }),
                "invalid_client" => Err(SocialError::AuthInvalid {
                    detail: "Google rejected your client_id or client_secret. Check them against your OAuth client in Google Cloud Console.".to_string(),
                }),
                _ => Err(SocialError::Unknown(format!("Google returned error '{}'{}",
                    err,
                    if detail.is_empty() { "".to_string() } else { format!(": {}", detail) })
                )),
            };
        }
        if status != 200 {
            return Err(SocialError::Unknown(format!("token endpoint returned HTTP {}", status)));
        }
        let new_access = body.access_token.ok_or_else(||
            SocialError::Unknown("Google response missing access_token".to_string()))?;
        let expires_in = body.expires_in.unwrap_or(3600);
        let now = chrono::Utc::now().timestamp();

        let mut updated = creds.clone();
        updated["access_token"] = serde_json::Value::String(new_access);
        updated["expires_in"]   = serde_json::Value::Number(expires_in.into());
        updated["issued_at"]    = serde_json::Value::Number(now.into());
        if let Some(scope) = body.scope {
            updated["scope"] = serde_json::Value::String(scope);
        }

        Ok(RefreshedCredentials {
            credentials: updated,
            expires_at: Some(now + expires_in),
        })
    }
}
