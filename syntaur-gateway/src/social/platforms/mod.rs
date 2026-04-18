//! Per-platform adapters — one file per platform, all implement
//! `SocialPlatform`. The registry at the bottom of this file is the only
//! place the rest of the gateway looks up an adapter by id.
//!
//! Adding a new platform:
//!   1. Create `src/social/platforms/<id>.rs` implementing `SocialPlatform`.
//!   2. Register it in `registry()` below.
//!   3. The `/social` Connections UI + the reconnect endpoint pick it up
//!      automatically via the descriptor.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub mod bluesky;
pub mod youtube;

// ── Error taxonomy ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SocialError {
    AuthExpired { hint: &'static str },
    AuthInvalid { detail: String },
    RateLimit { retry_after_secs: Option<u64> },
    InvalidInput { field: &'static str, reason: String },
    PlatformDown { status_url: &'static str },
    Network(String),
    Unknown(String),
}

impl std::fmt::Display for SocialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for SocialError {}

impl SocialError {
    /// Human-readable message the UI shows verbatim. Per
    /// feedback/human_readable_errors — no HTTP codes, no raw JSON, just
    /// what the user needs to do next.
    pub fn user_message(&self) -> String {
        match self {
            Self::AuthExpired { hint } =>
                format!("Looks like that session timed out. {}", hint),
            Self::AuthInvalid { detail } =>
                format!("Those credentials didn't work. {}", detail),
            Self::RateLimit { retry_after_secs } => match retry_after_secs {
                Some(s) => format!("Platform is rate-limiting us — try again in ~{} seconds.", s),
                None => "Platform is rate-limiting us — try again in a minute.".to_string(),
            },
            Self::InvalidInput { field, reason } =>
                format!("The {} field: {}", field, reason),
            Self::PlatformDown { status_url } =>
                format!("Platform looks down. Check {} for incidents.", status_url),
            Self::Network(detail) =>
                format!("Couldn't reach the platform — {}", detail),
            Self::Unknown(detail) =>
                format!("Something unexpected happened: {}", detail),
        }
    }

    /// Status-pill class the UI maps this error to.
    pub fn status(&self) -> &'static str {
        match self {
            Self::AuthExpired { .. } | Self::AuthInvalid { .. } => "error",
            Self::RateLimit { .. } | Self::PlatformDown { .. } | Self::Network(_) => "degraded",
            Self::InvalidInput { .. } => "error",
            Self::Unknown(_) => "error",
        }
    }
}

// ── Auth flow descriptor ────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthFlow {
    /// e.g. Bluesky: user enters a handle + app password, we call createSession.
    AppPassword {
        field_labels: [&'static str; 2],
        field_helps:  [&'static str; 2],
        setup_steps: Vec<WizardStep>,
    },
    /// OAuth2 with PKCE. Filled in phases 4+.
    /// Explicit rename because serde's snake_case mangler turns `OAuth2`
    /// into `o_auth2`, which the UI's `kind === 'oauth2'` check would miss.
    #[serde(rename = "oauth2")]
    OAuth2 {
        authorize_url: &'static str,
        token_url: &'static str,
        scopes: Vec<&'static str>,
        requires_user_app: bool,
        pkce: bool,
        setup_steps: Vec<WizardStep>,
    },
    /// User must sign up for a paid API tier before we can connect.
    Paid {
        signup_url: &'static str,
        setup_steps: Vec<WizardStep>,
    },
    /// Placeholder for platforms whose adapter isn't written yet.
    NotImplemented,
}

#[derive(Serialize, Clone, Debug)]
pub struct WizardStep {
    pub title: &'static str,
    pub body_md: &'static str,
    pub deep_link: Option<&'static str>,
    pub copy_target: Option<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlatformDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub tagline: &'static str,
    pub tone: &'static str,  // ui card accent
    pub auth_flow: AuthFlow,
}

// ── Inputs + outputs ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConnectInput {
    /// Platform-supplied fields. For AppPassword flows: {"handle": ..., "app_password": ...}.
    /// For OAuth2 flows: {"code": ..., "state": ...}.
    pub fields: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    pub display_name: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredCredentials {
    pub display_name: String,
    pub credentials: serde_json::Value,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RefreshedCredentials {
    pub credentials: serde_json::Value,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostRef {
    pub uri: String,
    pub cid: Option<String>,
    pub posted_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub id: String,            // unique within platform (URI / notification id)
    pub parent_uri: String,    // post being replied to
    pub parent_author: String,
    pub parent_text: String,
    pub kind: String,          // "reply" | "mention" | "like" | "repost" | ...
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PlatformStats {
    pub followers: Option<i64>,
    pub following: Option<i64>,
    pub posts_count: Option<i64>,
    pub likes_received: Option<i64>,
    pub reposts_received: Option<i64>,
    pub replies_received: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentPost {
    pub uri: String,
    pub text: String,
    pub created_at: i64,
}

// ── Adapter trait ───────────────────────────────────────────────────────────

#[async_trait]
pub trait SocialPlatform: Send + Sync {
    fn id(&self) -> &'static str;
    fn descriptor(&self) -> PlatformDescriptor;

    /// Hit the platform with the stored credentials and confirm they work
    /// right now. Used by health checks + on-demand "verify" buttons.
    async fn verify(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<VerifiedIdentity, SocialError>;

    /// Establish a fresh session from whatever the wizard collected
    /// (password, OAuth code, etc.).
    async fn reconnect(
        &self,
        http: &reqwest::Client,
        input: &ConnectInput,
    ) -> Result<StoredCredentials, SocialError>;

    /// For OAuth platforms: use the stored refresh_token to mint a fresh
    /// access_token without user interaction. For AppPassword: default impl
    /// re-verifies (since app passwords don't expire on a schedule).
    async fn refresh(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<RefreshedCredentials, SocialError> {
        let _ = self.verify(http, creds).await?;
        Ok(RefreshedCredentials { credentials: creds.clone(), expires_at: None })
    }

    // ── Action surface — default unimplemented so adapters opt in ──────────

    async fn post(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _text: &str,
    ) -> Result<PostRef, SocialError> {
        Err(SocialError::Unknown("Posting not implemented for this platform yet.".to_string()))
    }

    async fn reply(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _parent_uri: &str,
        _text: &str,
    ) -> Result<PostRef, SocialError> {
        Err(SocialError::Unknown("Replies not implemented for this platform yet.".to_string()))
    }

    async fn like(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _target_uri: &str,
    ) -> Result<(), SocialError> {
        Err(SocialError::Unknown("Likes not implemented for this platform yet.".to_string()))
    }

    async fn follow(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _target_did: &str,
    ) -> Result<String, SocialError> {
        Err(SocialError::Unknown("Follow not implemented for this platform yet.".to_string()))
    }

    async fn unfollow(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _follow_uri: &str,
    ) -> Result<(), SocialError> {
        Err(SocialError::Unknown("Unfollow not implemented for this platform yet.".to_string()))
    }

    async fn notifications(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _since: Option<i64>,
    ) -> Result<Vec<Notification>, SocialError> {
        Ok(vec![])
    }

    async fn stats(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
    ) -> Result<PlatformStats, SocialError> {
        Ok(PlatformStats::default())
    }

    async fn recent_posts(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _limit: u32,
    ) -> Result<Vec<RecentPost>, SocialError> {
        Ok(vec![])
    }

    async fn search_hashtag(
        &self,
        _http: &reqwest::Client,
        _creds: &serde_json::Value,
        _hashtag: &str,
        _limit: u32,
    ) -> Result<Vec<(String, String)>, SocialError> {
        // Returns (post_uri, author_did) pairs
        Ok(vec![])
    }
}

// ── Registry ────────────────────────────────────────────────────────────────

/// All adapters compiled into the gateway. Order here determines the
/// order cards render on the Connections pane.
pub fn registry() -> HashMap<&'static str, Arc<dyn SocialPlatform>> {
    let mut m: HashMap<&'static str, Arc<dyn SocialPlatform>> = HashMap::new();
    let adapters: Vec<Arc<dyn SocialPlatform>> = vec![
        Arc::new(bluesky::Bluesky),
        Arc::new(youtube::YouTube),
        // Phase 5: Arc::new(threads::Threads), ...
    ];
    for a in adapters { m.insert(a.id(), a); }
    m
}

/// Descriptors for every platform the UI renders a card for, INCLUDING
/// ones without a live adapter yet. `auth_flow = NotImplemented` marks
/// the stubbed ones so the UI can disable the Connect button.
pub fn all_descriptors() -> Vec<PlatformDescriptor> {
    let adapters = registry();
    let mut out = Vec::new();

    // Live adapters first (in registry order would be nice, but HashMap
    // isn't ordered — list them explicitly so UI stays deterministic).
    for id in &["bluesky", "youtube"] {
        if let Some(a) = adapters.get(id) { out.push(a.descriptor()); }
    }

    // Stubbed platforms — will be replaced by live adapters as phases land.
    let stubbed: &[(&str, &str, &str, &str)] = &[
        ("threads",   "Threads",     "Meta OAuth2 (via Facebook Graph)",         "graphite"),
        ("instagram", "Instagram",   "Meta OAuth2 (Business or Creator)",         "rose"),
        ("linkedin",  "LinkedIn",    "LinkedIn OAuth2 (Member / Page)",           "steel"),
        ("tiktok",    "TikTok",      "TikTok for Developers",                      "graphite"),
        ("facebook",  "Facebook",    "Meta OAuth2 (Page posts)",                   "navy"),
        ("twitter",   "X / Twitter", "Paid API tier required",                     "graphite"),
    ];
    for (id, name, tagline, tone) in stubbed {
        out.push(PlatformDescriptor {
            id,
            display_name: name,
            tagline,
            tone,
            auth_flow: AuthFlow::NotImplemented,
        });
    }
    out
}
