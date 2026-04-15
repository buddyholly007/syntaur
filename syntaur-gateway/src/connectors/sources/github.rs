//! GitHub repos / issues / PRs connector.
//!
//! Lists the configured user's repos via the GitHub REST API and indexes:
//!   * each repo's README (if present)
//!   * each repo's most recent issues (open + closed, last 30)
//!   * each repo's most recent pull requests (last 30)
//!
//! Auth: a personal access token (classic or fine-grained) with `repo:read`.
//! Configure in `connectors.github.{user, token}` in syntaur.json.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const GH_API: &str = "https://api.github.com";
const PER_PAGE: usize = 30;
const REPO_LIMIT: usize = 50;

#[derive(Deserialize)]
#[allow(dead_code)]
struct Repo {
    id: i64,
    name: String,
    full_name: String,
    description: Option<String>,
    html_url: String,
    updated_at: String,
    fork: bool,
    archived: bool,
    private: bool,
    default_branch: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Issue {
    number: i64,
    title: String,
    body: Option<String>,
    state: String,
    html_url: String,
    updated_at: String,
    user: Option<UserRef>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct UserRef {
    login: String,
}

pub struct GithubConnector {
    name: String,
    user: String,
    token: String,
    http: reqwest::Client,
}

impl GithubConnector {
    pub fn new(user: String, token: String, http: reqwest::Client) -> Self {
        Self {
            name: "github".to_string(),
            user,
            token,
            http,
        }
    }

    fn auth_header(&self) -> (String, String) {
        ("Authorization".to_string(), format!("Bearer {}", self.token))
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, String> {
        let url = format!(
            "{}/users/{}/repos?per_page={}&sort=updated",
            GH_API, self.user, PER_PAGE
        );
        let (k, v) = self.auth_header();
        let resp = self
            .http
            .get(&url)
            .header(&k, v)
            .header("User-Agent", "syntaur")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("list repos: {}", e))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("github API: {}", body));
        }
        let mut repos: Vec<Repo> = resp
            .json()
            .await
            .map_err(|e| format!("parse repos: {}", e))?;
        repos.truncate(REPO_LIMIT);
        Ok(repos)
    }

    async fn fetch_readme(&self, full_name: &str) -> Option<String> {
        let url = format!("{}/repos/{}/readme", GH_API, full_name);
        let (k, v) = self.auth_header();
        let resp = self
            .http
            .get(&url)
            .header(&k, v)
            .header("User-Agent", "syntaur")
            .header("Accept", "application/vnd.github.raw")
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.text().await.ok()
    }

    async fn list_issues(&self, full_name: &str) -> Vec<Issue> {
        let url = format!(
            "{}/repos/{}/issues?state=all&per_page={}&sort=updated",
            GH_API, full_name, PER_PAGE
        );
        let (k, v) = self.auth_header();
        let resp = match self
            .http
            .get(&url)
            .header(&k, v)
            .header("User-Agent", "syntaur")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        if !resp.status().is_success() {
            return Vec::new();
        }
        resp.json().await.unwrap_or_default()
    }
}

impl Connector for GithubConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for GithubConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        let repos = match self.list_repos().await {
            Ok(r) => r,
            Err(e) => {
                warn!("[github] {}", e);
                return Err(e);
            }
        };
        let mut docs = Vec::new();
        for repo in repos {
            if repo.archived || repo.fork {
                continue;
            }
            let updated_at = DateTime::parse_from_rfc3339(&repo.updated_at)
                .ok()
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            // README as a doc
            if let Some(readme) = self.fetch_readme(&repo.full_name).await {
                if !readme.trim().is_empty() {
                    docs.push(ExternalDoc {
                        source: "github".to_string(),
                        external_id: format!("{}/README", repo.full_name),
                        title: format!(
                            "{} README — {}",
                            repo.full_name,
                            repo.description.clone().unwrap_or_default()
                        ),
                        body: readme,
                        updated_at,
                        metadata: json!({
                            "repo": repo.full_name,
                            "kind": "readme",
                            "url": repo.html_url,
                        }),
                        agent_id: "shared".to_string(),
                    });
                }
            }
            // Issues + PRs (the issues endpoint returns both; pull_request is set on PRs)
            for issue in self.list_issues(&repo.full_name).await {
                let kind = if issue.pull_request.is_some() {
                    "pr"
                } else {
                    "issue"
                };
                let issue_updated = DateTime::parse_from_rfc3339(&issue.updated_at)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);
                let body = format!(
                    "# {} #{}: {}\nState: {}\nURL: {}\n\n{}",
                    kind,
                    issue.number,
                    issue.title,
                    issue.state,
                    issue.html_url,
                    issue.body.unwrap_or_default()
                );
                docs.push(ExternalDoc {
                    source: "github".to_string(),
                    external_id: format!("{}/{}/{}", repo.full_name, kind, issue.number),
                    title: format!("{} {}#{}: {}", repo.full_name, kind, issue.number, issue.title),
                    body,
                    updated_at: issue_updated,
                    metadata: json!({
                        "repo": repo.full_name,
                        "kind": kind,
                        "number": issue.number,
                        "state": issue.state,
                    }),
                    agent_id: "shared".to_string(),
                });
            }
        }
        debug!("[github] {} docs total", docs.len());
        Ok(docs)
    }
}

#[async_trait]
impl SlimConnector for GithubConnector {
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
