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

use crate::run::{Finding, FindingEdit, FindingKind, Severity};

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "anthropic/claude-opus-4";

pub struct OpusClient {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl OpusClient {
    /// Fetch the `openrouter` key from the local syntaur-vault agent,
    /// falling back to the `OPENROUTER_API_KEY` env var. Env-var path
    /// is used by CI and by syntaur-ship Phase 6 where the vault agent
    /// isn't running. Errors loudly if both are missing.
    pub fn from_vault() -> Result<Self> {
        let api_key = Self::resolve_api_key()?;
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

    /// Try vault socket first, fall back to `OPENROUTER_API_KEY` env.
    fn resolve_api_key() -> Result<String> {
        use syntaur_vault_core::{
            agent::{request, AgentRequest, AgentResponse},
            default_socket_path,
        };
        let socket = default_socket_path();
        if socket.exists() {
            let resp = request(
                &socket,
                &AgentRequest::Get {
                    name: "openrouter".to_string(),
                },
            )
            .context("asking vault for openrouter key")?;
            match resp {
                AgentResponse::Value { value } => return Ok(value),
                AgentResponse::Error { message } => {
                    log::warn!(
                        "[opus] vault has no openrouter entry ({message}); trying env var"
                    );
                }
                other => anyhow::bail!("unexpected vault response: {other:?}"),
            }
        }
        // Env fallback — used by CI, syntaur-ship Phase 6, and any
        // headless run where the vault agent isn't started.
        if let Ok(k) = std::env::var("OPENROUTER_API_KEY") {
            if !k.is_empty() {
                return Ok(k);
            }
        }
        anyhow::bail!(
            "no openrouter key: vault agent not running at {} and OPENROUTER_API_KEY env is unset. \
             Run `syntaur-vault unlock` OR `export OPENROUTER_API_KEY=sk-or-…`",
            socket.display()
        )
    }

    /// Send one screenshot + module context to Opus and parse the
    /// structured findings response. Errors bubble up; the caller
    /// decides whether to fail the run or continue with heuristic-
    /// only Findings.
    ///
    /// Phase 2a shim — delegates to `analyze_module_with_source` with
    /// no attached source context. Prefer the `_with_source` form for
    /// Phase 2b auto-fix since Opus needs code to propose edits.
    pub async fn analyze_module(
        &self,
        module_slug: &str,
        url: &str,
        screenshot_path: &Path,
        changed_paths: &[String],
    ) -> Result<Vec<Finding>> {
        self.analyze_module_with_source(
            module_slug,
            url,
            screenshot_path,
            changed_paths,
            &[],
            /* request_edits = */ false,
        )
        .await
    }

    /// Phase 2b entry point. In addition to the screenshot + changed
    /// paths, attaches module-relevant source files (already truncated
    /// to a byte budget by the caller — we just embed them verbatim)
    /// and — if `request_edits` is set — asks Opus to return
    /// `{file, old_string, new_string}` edits for each regression so
    /// the auto-fix loop can apply them precisely.
    ///
    /// Keeping the two modes behind one function avoids duplicating
    /// the HTTP + parse plumbing; the prompt branches on
    /// `request_edits` and whether `source_files` is non-empty.
    pub async fn analyze_module_with_source(
        &self,
        module_slug: &str,
        url: &str,
        screenshot_path: &Path,
        changed_paths: &[String],
        source_files: &[(String, String)],
        request_edits: bool,
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

        let source_section = if source_files.is_empty() {
            String::new()
        } else {
            let mut s = String::from("\n\nRelevant source files (workspace-relative paths):\n");
            for (path, body) in source_files {
                s.push_str(&format!("\n===== {} =====\n", path));
                s.push_str(body);
                if !body.ends_with('\n') {
                    s.push('\n');
                }
            }
            s
        };

        let edits_clause = if request_edits {
            ",\n      \"edits\": [ /* optional: precise source edits to fix this regression */\n        {\n          \"file\": \"workspace-relative path from the list above\",\n          \"old_string\": \"EXACT current text from the file (must be unique in the file; include enough surrounding lines to make it unique)\",\n          \"new_string\": \"replacement text\"\n        }\n      ]"
        } else {
            ""
        };

        let edits_rules = if request_edits {
            "\n\nWhen you return `edits` for a regression:\n\
- Only propose edits for files shown above. Do NOT invent new files.\n\
- `old_string` must match EXACTLY (bytes, whitespace, indentation) and appear ONLY ONCE in the file. If you can't make it unique with 1-3 surrounding lines, widen the window until you can.\n\
- Prefer the smallest edit that fixes the regression. Multiple small edits beat one large one.\n\
- Keep total new code under ~40 lines per finding — large rewrites belong in a manual review, not auto-fix.\n\
- Do NOT propose edits for `improvement` findings — only for `regression`.\n\
- If you can see the regression on screen but the code context doesn't let you fix it precisely, omit `edits` (set null or leave out). A regression without edits is still useful — it tells the human what to look at."
        } else {
            ""
        };

        let prompt = format!(
            "You are auditing one module of the Syntaur web application.

Module slug: {module_slug}
URL path: {url}
Source paths changed since last successful deploy:
{changes_summary}{source_section}

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
      \"suggested_fix\": \"optional: natural-language description of how to fix\"{edits_clause}
    }}
  ]
}}

If the module looks clean with no suggestions, output {{\"findings\": []}}.
Be strict about regressions — a first-time user would not forgive them.
Be practical about improvements — only surface things with clear user value.{edits_rules}"
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
                // Policy: only regressions carry edits. If Opus
                // returned edits on an improvement we drop them —
                // auto-fix is for breakage, not style tweaks.
                let edits = match severity {
                    Severity::Regression => f.edits.filter(|v| !v.is_empty()),
                    Severity::Suggestion => None,
                };
                Finding {
                    module_slug: module_slug.to_string(),
                    kind,
                    severity,
                    title: f.title,
                    detail,
                    artifact: Some(screenshot_path.to_path_buf()),
                    captured_at: now,
                    edits,
                    // Phase 4b — Opus returns a persona-agnostic
                    // finding; the CLI stamps the active POV's slug
                    // after it returns so the persona-tagging policy
                    // lives in one place.
                    persona: None,
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
    #[serde(default)]
    edits: Option<Vec<FindingEdit>>,
}
