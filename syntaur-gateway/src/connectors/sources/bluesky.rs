//! Bluesky (ATProto) public posts connector.
//!
//! Pulls a configured account's authored posts via the public XRPC endpoint
//! `app.bsky.feed.getAuthorFeed`. No auth required for read access on
//! public accounts. Each post becomes one indexed document.
//!
//! Defaults: pulls up to 200 most recent posts. Replies and reposts are
//! included. Quote posts are flattened by including the quoted record's
//! text in the body.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const APPVIEW_HOST: &str = "https://public.api.bsky.app";
const PAGE_LIMIT: usize = 100;
const MAX_PAGES: usize = 4; // 400 posts max

#[derive(Deserialize)]
struct AuthorFeedResponse {
    feed: Vec<FeedItem>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct FeedItem {
    post: Post,
    #[serde(default)]
    reply: Option<serde_json::Value>,
    #[serde(default)]
    reason: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Post {
    uri: String,
    cid: String,
    author: Author,
    record: PostRecord,
    #[serde(default)]
    indexed_at: Option<String>,
    #[serde(default)]
    like_count: Option<i64>,
    #[serde(default)]
    repost_count: Option<i64>,
    #[serde(default)]
    reply_count: Option<i64>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Author {
    did: String,
    handle: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct PostRecord {
    text: String,
    #[serde(rename = "createdAt", default)]
    created_at: Option<String>,
}

pub struct BlueskyConnector {
    name: String,
    actor: String,
    http: reqwest::Client,
}

impl BlueskyConnector {
    /// `actor` is a handle (e.g. `crimsonlantern.bsky.social`) or DID.
    pub fn new(actor: String, http: reqwest::Client) -> Self {
        Self {
            name: "bluesky".to_string(),
            actor,
            http,
        }
    }

    async fn fetch_all(&self) -> Result<Vec<ExternalDoc>, String> {
        let mut docs = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = format!(
                "{}/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit={}&filter=posts_with_replies",
                APPVIEW_HOST, self.actor, PAGE_LIMIT
            );
            if let Some(c) = &cursor {
                url.push_str(&format!("&cursor={}", c));
            }
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("fetch {}: {}", url, e))?;
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("bsky API: {}", body));
            }
            let parsed: AuthorFeedResponse = resp
                .json()
                .await
                .map_err(|e| format!("parse bsky json: {}", e))?;
            for item in parsed.feed {
                let post = item.post;
                let text = post.record.text;
                if text.trim().is_empty() {
                    continue;
                }
                let updated_at = post
                    .record
                    .created_at
                    .as_deref()
                    .or(post.indexed_at.as_deref())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);
                let title = format!(
                    "{} @{} — {}",
                    post.author
                        .display_name
                        .as_deref()
                        .unwrap_or(&post.author.handle),
                    post.author.handle,
                    updated_at.format("%Y-%m-%d %H:%M")
                );
                let body = format!(
                    "{}\n\n👍 {} 🔁 {} 💬 {}\n\nURI: {}",
                    text,
                    post.like_count.unwrap_or(0),
                    post.repost_count.unwrap_or(0),
                    post.reply_count.unwrap_or(0),
                    post.uri
                );
                docs.push(ExternalDoc {
                    source: "bluesky".to_string(),
                    external_id: post.uri.clone(),
                    title,
                    body,
                    updated_at,
                    metadata: json!({
                        "author_handle": post.author.handle,
                        "author_did": post.author.did,
                        "cid": post.cid,
                        "is_reply": item.reply.is_some(),
                        "is_repost": item.reason.is_some(),
                    }),
                    agent_id: "shared".to_string(),
                });
            }
            cursor = parsed.cursor;
            if cursor.is_none() {
                break;
            }
        }
        debug!("[bluesky] fetched {} posts for {}", docs.len(), self.actor);
        Ok(docs)
    }
}

impl Connector for BlueskyConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for BlueskyConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        match self.fetch_all().await {
            Ok(d) => Ok(d),
            Err(e) => {
                warn!("[bluesky] load failed: {}", e);
                Err(e)
            }
        }
    }
}

#[async_trait]
impl SlimConnector for BlueskyConnector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String> {
        let docs = self.load_full().await?;
        Ok(docs
            .into_iter()
            .map(|d| DocIdOnly {
                external_id: d.external_id,
                updated_at: Some(d.updated_at),
            })
            .collect())
    }
}
