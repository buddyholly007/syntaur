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
    AuthFlow, ConnectInput, Notification, PlatformDescriptor, PlatformStats, PostRef,
    RecentPost, RefreshedCredentials, SocialError, SocialPlatform, StoredCredentials,
    VerifiedIdentity, WizardStep,
};

const BSKY_API: &str = "https://bsky.social/xrpc";

pub struct Bluesky;

// ── Internal helpers ────────────────────────────────────────────────────────

fn require_access(creds: &serde_json::Value) -> Result<String, SocialError> {
    let a = creds.get("access_jwt").and_then(|v| v.as_str()).unwrap_or("");
    if a.is_empty() {
        return Err(SocialError::AuthExpired { hint: "No active session. Reconnect or refresh first." });
    }
    Ok(a.to_string())
}

/// Shape returned by `com.atproto.repo.getRecord` (or pieces we care about).
struct BskyPost {
    uri: String,
    cid: String,
    root: Option<(String, String)>,
}

async fn bsky_get_post(
    http: &reqwest::Client,
    access: &str,
    at_uri: &str,
) -> Result<BskyPost, SocialError> {
    // at_uri shape: at://<did>/<collection>/<rkey>
    let stripped = at_uri.strip_prefix("at://").ok_or_else(|| SocialError::InvalidInput {
        field: "parent_uri", reason: "expected at://... URI".to_string(),
    })?;
    let parts: Vec<&str> = stripped.splitn(3, '/').collect();
    if parts.len() != 3 {
        return Err(SocialError::InvalidInput { field: "parent_uri", reason: "malformed AT URI".to_string() });
    }
    let url = format!("{}/com.atproto.repo.getRecord", BSKY_API);
    let resp = http.get(&url)
        .bearer_auth(access)
        .query(&[("repo", parts[0]), ("collection", parts[1]), ("rkey", parts[2])])
        .timeout(Duration::from_secs(10))
        .send().await
        .map_err(|e| SocialError::Network(e.to_string()))?;
    if resp.status().as_u16() != 200 {
        return Err(SocialError::Unknown(format!("getRecord HTTP {}", resp.status().as_u16())));
    }
    #[derive(Deserialize)]
    struct GetRec {
        uri: String,
        cid: String,
        value: serde_json::Value,
    }
    let body: GetRec = resp.json().await.map_err(|e| SocialError::Unknown(format!("getRecord JSON: {}", e)))?;
    // If the parent is itself a reply, pull its thread root so our reply lives in the same thread.
    let root = body.value.get("reply")
        .and_then(|r| r.get("root"))
        .and_then(|r| {
            let uri = r.get("uri").and_then(|v| v.as_str())?;
            let cid = r.get("cid").and_then(|v| v.as_str())?;
            Some((uri.to_string(), cid.to_string()))
        });
    Ok(BskyPost { uri: body.uri, cid: body.cid, root })
}

async fn bsky_create_record(
    http: &reqwest::Client,
    creds: &serde_json::Value,
    collection: &str,
    record: serde_json::Value,
) -> Result<PostRef, SocialError> {
    let access = require_access(creds)?;
    let repo = creds.get("did").and_then(|v| v.as_str()).unwrap_or("");
    if repo.is_empty() {
        return Err(SocialError::AuthInvalid { detail: "Credential blob missing DID — reconnect.".to_string() });
    }
    let url = format!("{}/com.atproto.repo.createRecord", BSKY_API);
    let resp = http.post(&url)
        .bearer_auth(&access)
        .json(&serde_json::json!({
            "repo": repo,
            "collection": collection,
            "record": record,
        }))
        .timeout(Duration::from_secs(15))
        .send().await
        .map_err(|e| SocialError::Network(e.to_string()))?;
    match resp.status().as_u16() {
        200 => {
            #[derive(Deserialize)]
            struct CreatedRef { uri: String, cid: Option<String> }
            let parsed: CreatedRef = resp.json().await
                .map_err(|e| SocialError::Unknown(format!("createRecord JSON: {}", e)))?;
            Ok(PostRef { uri: parsed.uri, cid: parsed.cid, posted_at: chrono::Utc::now().timestamp() })
        }
        401 | 400 => Err(SocialError::AuthExpired { hint: "Session expired. Refresh Bluesky first." }),
        429       => Err(SocialError::RateLimit { retry_after_secs: None }),
        500..=599 => Err(SocialError::PlatformDown { status_url: "https://status.bsky.app/" }),
        code      => Err(SocialError::Unknown(format!("createRecord HTTP {}", code))),
    }
}

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

    async fn post(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        text: &str,
    ) -> Result<PostRef, SocialError> {
        bsky_create_record(http, creds, "app.bsky.feed.post", serde_json::json!({
            "$type": "app.bsky.feed.post",
            "text": text,
            "createdAt": chrono::Utc::now().to_rfc3339(),
        })).await
    }

    async fn reply(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        parent_uri: &str,
        text: &str,
    ) -> Result<PostRef, SocialError> {
        // Need the parent's CID to reply. Fetch it.
        let access = require_access(creds)?;
        let parent = bsky_get_post(http, &access, parent_uri).await?;
        // Find the root of the thread — for a top-level reply the root == parent.
        let (root_uri, root_cid) = parent.root.clone().unwrap_or_else(|| (parent.uri.clone(), parent.cid.clone()));
        bsky_create_record(http, creds, "app.bsky.feed.post", serde_json::json!({
            "$type": "app.bsky.feed.post",
            "text": text,
            "createdAt": chrono::Utc::now().to_rfc3339(),
            "reply": {
                "root":   { "uri": root_uri, "cid": root_cid },
                "parent": { "uri": parent.uri, "cid": parent.cid },
            },
        })).await
    }

    async fn like(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        target_uri: &str,
    ) -> Result<(), SocialError> {
        let access = require_access(creds)?;
        let target = bsky_get_post(http, &access, target_uri).await?;
        let _ = bsky_create_record(http, creds, "app.bsky.feed.like", serde_json::json!({
            "$type": "app.bsky.feed.like",
            "subject": { "uri": target.uri, "cid": target.cid },
            "createdAt": chrono::Utc::now().to_rfc3339(),
        })).await?;
        Ok(())
    }

    async fn follow(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        target_did: &str,
    ) -> Result<String, SocialError> {
        let r = bsky_create_record(http, creds, "app.bsky.graph.follow", serde_json::json!({
            "$type": "app.bsky.graph.follow",
            "subject": target_did,
            "createdAt": chrono::Utc::now().to_rfc3339(),
        })).await?;
        Ok(r.uri)
    }

    async fn unfollow(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        follow_uri: &str,
    ) -> Result<(), SocialError> {
        let access = require_access(creds)?;
        // follow_uri shape: at://did:plc:xxx/app.bsky.graph.follow/<rkey>
        let parts: Vec<&str> = follow_uri.strip_prefix("at://").unwrap_or("").splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(SocialError::InvalidInput { field: "follow_uri", reason: "expected at://did/collection/rkey".to_string() });
        }
        let (repo, collection, rkey) = (parts[0], parts[1], parts[2]);
        let url = format!("{}/com.atproto.repo.deleteRecord", BSKY_API);
        let resp = http.post(&url)
            .bearer_auth(&access)
            .json(&serde_json::json!({ "repo": repo, "collection": collection, "rkey": rkey }))
            .timeout(Duration::from_secs(10))
            .send().await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            200 => Ok(()),
            401 | 400 => Err(SocialError::AuthExpired { hint: "Session expired during unfollow. Try again after a refresh." }),
            code => Err(SocialError::Unknown(format!("deleteRecord returned HTTP {}", code))),
        }
    }

    async fn notifications(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        since: Option<i64>,
    ) -> Result<Vec<Notification>, SocialError> {
        let access = require_access(creds)?;
        let url = format!("{}/app.bsky.notification.listNotifications?limit=50", BSKY_API);
        let resp = http.get(&url)
            .bearer_auth(&access)
            .timeout(Duration::from_secs(10))
            .send().await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        if resp.status().as_u16() != 200 {
            return Err(SocialError::Unknown(format!("listNotifications HTTP {}", resp.status().as_u16())));
        }
        #[derive(Deserialize)]
        struct NotifList { notifications: Vec<NotifItem> }
        #[derive(Deserialize)]
        struct NotifItem {
            uri: String,
            #[serde(rename = "cid")] _cid: Option<String>,
            author: NotifAuthor,
            reason: String,
            #[serde(rename = "reasonSubject")] reason_subject: Option<String>,
            record: Option<serde_json::Value>,
            #[serde(rename = "indexedAt")] indexed_at: String,
        }
        #[derive(Deserialize)]
        struct NotifAuthor { handle: String, #[serde(rename = "displayName")] _display: Option<String> }

        let body: NotifList = resp.json().await
            .map_err(|e| SocialError::Unknown(format!("notifications JSON: {}", e)))?;
        let mut out = Vec::new();
        for n in body.notifications {
            let at = chrono::DateTime::parse_from_rfc3339(&n.indexed_at)
                .map(|d| d.timestamp()).unwrap_or(0);
            if let Some(s) = since { if at <= s { continue; } }
            // We care about reasons that warrant a drafted reply — reply/mention primarily.
            let include = matches!(n.reason.as_str(), "reply" | "mention" | "quote");
            if !include { continue; }
            let parent_text = n.record.as_ref()
                .and_then(|r| r.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("").to_string();
            out.push(Notification {
                id: n.uri.clone(),
                parent_uri: n.uri,
                parent_author: n.author.handle,
                parent_text,
                kind: n.reason,
                at,
            });
            let _ = n.reason_subject;
        }
        Ok(out)
    }

    async fn stats(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
    ) -> Result<PlatformStats, SocialError> {
        let handle = creds.get("handle").and_then(|v| v.as_str()).unwrap_or("");
        if handle.is_empty() {
            return Ok(PlatformStats::default());
        }
        let url = format!("{}/app.bsky.actor.getProfile", BSKY_API);
        let access = require_access(creds)?;
        let resp = http.get(&url)
            .bearer_auth(&access)
            .query(&[("actor", handle)])
            .timeout(Duration::from_secs(10))
            .send().await
            .map_err(|e| SocialError::Network(e.to_string()))?;
        if resp.status().as_u16() != 200 {
            return Err(SocialError::Unknown(format!("getProfile HTTP {}", resp.status().as_u16())));
        }
        #[derive(Deserialize)]
        struct Profile {
            #[serde(rename = "followersCount")] followers: Option<i64>,
            #[serde(rename = "followsCount")]   follows: Option<i64>,
            #[serde(rename = "postsCount")]     posts: Option<i64>,
        }
        let p: Profile = resp.json().await.map_err(|e| SocialError::Unknown(format!("profile JSON: {}", e)))?;
        Ok(PlatformStats {
            followers: p.followers,
            following: p.follows,
            posts_count: p.posts,
            ..Default::default()
        })
    }

    async fn recent_posts(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        limit: u32,
    ) -> Result<Vec<RecentPost>, SocialError> {
        let handle = creds.get("handle").and_then(|v| v.as_str()).unwrap_or("");
        if handle.is_empty() { return Ok(vec![]); }
        let access = require_access(creds)?;
        let url = format!("{}/app.bsky.feed.getAuthorFeed", BSKY_API);
        let lim = limit.clamp(1, 100).to_string();
        let resp = http.get(&url).bearer_auth(&access)
            .query(&[("actor", handle), ("limit", lim.as_str())])
            .timeout(Duration::from_secs(10))
            .send().await.map_err(|e| SocialError::Network(e.to_string()))?;
        if resp.status().as_u16() != 200 {
            return Err(SocialError::Unknown(format!("getAuthorFeed HTTP {}", resp.status().as_u16())));
        }
        let body: serde_json::Value = resp.json().await
            .map_err(|e| SocialError::Unknown(format!("feed JSON: {}", e)))?;
        let feed = body.get("feed").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for item in feed {
            if let Some(post) = item.get("post") {
                let uri = post.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let text = post.get("record").and_then(|r| r.get("text")).and_then(|t| t.as_str()).unwrap_or("").to_string();
                let created_at = post.get("record")
                    .and_then(|r| r.get("createdAt"))
                    .and_then(|c| c.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.timestamp()).unwrap_or(0);
                if !uri.is_empty() { out.push(RecentPost { uri, text, created_at }); }
            }
        }
        Ok(out)
    }

    async fn search_hashtag(
        &self,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        hashtag: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, SocialError> {
        let access = require_access(creds)?;
        let q = hashtag.trim_start_matches('#').to_string();
        let url = format!("{}/app.bsky.feed.searchPosts", BSKY_API);
        let lim = limit.clamp(1, 100).to_string();
        let resp = http.get(&url).bearer_auth(&access)
            .query(&[("q", q.as_str()), ("limit", lim.as_str())])
            .timeout(Duration::from_secs(10))
            .send().await.map_err(|e| SocialError::Network(e.to_string()))?;
        if resp.status().as_u16() != 200 {
            return Err(SocialError::Unknown(format!("searchPosts HTTP {}", resp.status().as_u16())));
        }
        let body: serde_json::Value = resp.json().await
            .map_err(|e| SocialError::Unknown(format!("search JSON: {}", e)))?;
        let posts = body.get("posts").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for post in posts {
            let uri = post.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let did = post.get("author").and_then(|a| a.get("did")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !uri.is_empty() && !did.is_empty() {
                out.push((uri, did));
            }
        }
        Ok(out)
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
