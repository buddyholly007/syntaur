//! Opus vision client — sends a screenshot + module context to Claude
//! Opus via OpenRouter and gets back structured Findings (regressions
//! + improvements).
//!
//! Why OpenRouter: Sean already has an `openrouter` API key in the
//! vault; routing via OpenRouter means no new credential to manage.
//! Cost overhead vs. direct Anthropic is ~5% — negligible compared
//! to the cost of maintaining a second credential.
//!
//! Model: `anthropic/claude-opus-4` by default; override via
//! SYNTAUR_VERIFY_MODEL env var when a newer Opus ships.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::run::{Finding, FindingKind, Severity};

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "anthropic/claude-opus-4";

pub struct OpusClient {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl OpusClient {
    /// Fetch the `openrouter` key from the local syntaur-vault agent.
    /// Errors loudly if the vault is locked — we want that surfaced,
    /// not silently skipped.
    pub fn from_vault() -> Result<Self> {
        use syntaur_vault_core::{
            agent::{request, AgentRequest, AgentResponse},
            default_socket_path,
        };
        let socket = default_socket_path();
        if !socket.exists() {
            anyhow::bail!(
                "vault agent not running at {} — run `syntaur-vault unlock` first. \
                 syntaur-verify needs the `openrouter` entry to call Opus.",
                socket.display()
            );
        }
        let resp = request(
            &socket,
            &AgentRequest::Get {
                name: "openrouter".to_string(),
            },
        )
        .context("asking vault for openrouter key")?;
        let api_key = match resp {
            AgentResponse::Value { value } => value,
            AgentResponse::Error { message } => {
                anyhow::bail!(
                    "vault refused openrouter: {message} — run `syntaur-vault list` + \
                     `syntaur-vault set openrouter` if missing"
                )
            }
            other => anyhow::bail!("unexpected vault response: {other:?}"),
        };
        let model =
            std::env::var("SYNTAUR_VERIFY_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Ok(Self {
            api_key,
            model,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
        })
    }

    /// Send one screenshot + module context to Opus and parse the
    /// structured findings response. Errors bubble up; the caller
    /// decides whether to fail the run or continue with heuristic-
    /// only Findings.
    pub async fn analyze_module(
        &self,
        module_slug: &str,
        url: &str,
        screenshot_path: &Path,
        changed_paths: &[String],
    ) -> Result<Vec<Finding>> {
        let png = std::fs::read(screenshot_path)
            .with_context(|| format!("reading screenshot {}", screenshot_path.display()))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let data_url = format!("data:image/png;base64,{}", b64);

        let changes_summary = if changed_paths.is_empty() {
            "(no changed paths — full-sweep run)".to_string()
        } else {
            changed_paths
                .iter()
                .map(|p| format!("  - {}", p))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prompt = format!(
            "You are auditing one module of the Syntaur web application.

Module slug: {module_slug}
URL path: {url}
Source paths changed since last successful deploy:
{changes_summary}

Look at the attached screenshot and identify TWO categories of issues:

1. REGRESSIONS — things that look broken or wrong: UI elements cut off, \
overlapping text, unreadable contrast, missing affordances, obvious alignment \
bugs, broken layouts, mystery floating elements with no label, text that reads \
as placeholder/TODO/Lorem ipsum.

2. IMPROVEMENTS — changes a user would appreciate: accessibility issues \
(missing alt, small tap targets, poor contrast approaching WCAG AA cutoff), \
missing loading/empty/error states, unclear interaction patterns, \
likely-broken mobile layouts.

Output ONLY a JSON object with this exact shape — NO prose, no code fences:

{{
  \"findings\": [
    {{
      \"kind\": \"regression\" | \"improvement\",
      \"title\": \"short noun phrase (5-10 words)\",
      \"detail\": \"one-sentence explanation of what is wrong + where on the page\",
      \"suggested_fix\": \"optional: natural-language description of how to fix\"
    }}
  ]
}}

If the module looks clean with no suggestions, output {{\"findings\": []}}.
Be strict about regressions — a first-time user would not forgive them.
Be practical about improvements — only surface things with clear user value."
        );

        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": data_url}}
                ]
            }],
            "response_format": {"type": "json_object"},
            "max_tokens": 2000,
        });

        let resp = self
            .http
            .post(OPENROUTER_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://github.com/buddyholly007/syntaur")
            .header("X-Title", "syntaur-verify")
            .json(&body)
            .send()
            .await
            .context("POST openrouter")?;

        let status = resp.status();
        let text = resp.text().await.context("read openrouter response")?;
        if !status.is_success() {
            anyhow::bail!("openrouter {}: {}", status, text);
        }

        let api: OpenAiResponse = serde_json::from_str(&text)
            .with_context(|| format!("parse openrouter response: {text}"))?;
        let content = api
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("openrouter returned no choices"))?
            .message
            .content;

        // Model sometimes wraps JSON in code fences despite instructions.
        // Strip them if present.
        let json_body = strip_code_fence(&content);
        let parsed: OpusFindings = serde_json::from_str(json_body).with_context(|| {
            format!("parse Opus findings JSON: {}", json_body.chars().take(400).collect::<String>())
        })?;

        let now = chrono::Utc::now();
        let findings = parsed
            .findings
            .into_iter()
            .map(|f| {
                let (severity, kind) = match f.kind.as_str() {
                    "regression" => (Severity::Regression, FindingKind::Other),
                    "improvement" => (Severity::Suggestion, FindingKind::Improvement),
                    _ => (Severity::Suggestion, FindingKind::Other),
                };
                let mut detail = f.detail;
                if let Some(fix) = f.suggested_fix {
                    if !fix.is_empty() {
                        detail.push_str("\n  suggested fix: ");
                        detail.push_str(&fix);
                    }
                }
                Finding {
                    module_slug: module_slug.to_string(),
                    kind,
                    severity,
                    title: f.title,
                    detail,
                    artifact: Some(screenshot_path.to_path_buf()),
                    captured_at: now,
                }
            })
            .collect();

        Ok(findings)
    }
}

fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_start_matches('\n')
            .trim_end_matches('\n')
            .trim_end_matches("```")
            .trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_start_matches('\n')
            .trim_end_matches('\n')
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    }
}

// ── OpenRouter wire types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpusFindings {
    findings: Vec<RawFinding>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawFinding {
    kind: String,
    title: String,
    detail: String,
    #[serde(default)]
    suggested_fix: Option<String>,
}
