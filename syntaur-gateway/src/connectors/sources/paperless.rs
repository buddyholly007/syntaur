//! Paperless-ngx document connector.
//!
//! Pulls documents from a Paperless instance via its REST API. Each
//! Paperless document becomes one indexed document with title, OCR'd
//! content, tags, and creation date.
//!
//! Pagination follows the `next` link in the JSON response. Page size 100.
//!
//! Auth: token in Authorization header. Configure in syntaur.json under
//! `connectors.paperless.{base_url, token}`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 50; // safety cap — 5000 docs max per sync

#[derive(Deserialize)]
struct PaperlessListResponse {
    results: Vec<PaperlessDoc>,
    next: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct PaperlessDoc {
    id: i64,
    title: String,
    content: Option<String>,
    created: Option<String>,
    modified: Option<String>,
    #[serde(default)]
    tags: Vec<i64>,
    #[serde(default)]
    correspondent: Option<i64>,
    #[serde(default)]
    document_type: Option<i64>,
    #[serde(default)]
    archive_serial_number: Option<String>,
}

pub struct PaperlessConnector {
    name: String,
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl PaperlessConnector {
    pub fn new(base_url: String, token: String, http: reqwest::Client) -> Self {
        Self {
            name: "paperless".to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    async fn fetch_all(&self) -> Result<Vec<ExternalDoc>, String> {
        let mut docs = Vec::new();
        let mut url = format!(
            "{}/api/documents/?page_size={}&ordering=-modified",
            self.base_url, PAGE_SIZE
        );
        for _ in 0..MAX_PAGES {
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("Token {}", self.token))
                .send()
                .await
                .map_err(|e| format!("fetch {}: {}", url, e))?;
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("paperless API: {}", body));
            }
            let parsed: PaperlessListResponse = resp
                .json()
                .await
                .map_err(|e| format!("parse paperless json: {}", e))?;
            for d in parsed.results {
                let updated_at = d
                    .modified
                    .as_deref()
                    .or(d.created.as_deref())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);
                let body_text = d.content.unwrap_or_default();
                if body_text.trim().is_empty() {
                    continue;
                }
                let title = if d.title.is_empty() {
                    format!("Paperless doc #{}", d.id)
                } else {
                    d.title.clone()
                };
                docs.push(ExternalDoc {
                    source: "paperless".to_string(),
                    external_id: d.id.to_string(),
                    title,
                    body: body_text,
                    updated_at,
                    metadata: json!({
                        "tags": d.tags,
                        "correspondent": d.correspondent,
                        "document_type": d.document_type,
                        "archive_serial_number": d.archive_serial_number,
                    }),
                    agent_id: "shared".to_string(),
                });
            }
            match parsed.next {
                Some(next_url) => url = next_url,
                None => break,
            }
        }
        debug!("[paperless] fetched {} documents", docs.len());
        Ok(docs)
    }
}

impl Connector for PaperlessConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for PaperlessConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        match self.fetch_all().await {
            Ok(docs) => Ok(docs),
            Err(e) => {
                warn!("[paperless] load failed: {}", e);
                Err(e)
            }
        }
    }
}

#[async_trait]
impl SlimConnector for PaperlessConnector {
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
