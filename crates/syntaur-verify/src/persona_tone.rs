//! Phase 4c — persona-tone audit.
//!
//! Per-persona, per-prompt register / voice check against the live
//! chat API. Complements the Phase 4b visual sweep: screenshots can't
//! catch a Thaddeus reply that's grammatically correct but drops the
//! butler "Pencilled in." ack phrase for a flat "Added event #339".
//!
//! Phase is entirely rule-based — we don't ask Opus to score "quality"
//! because that's expensive and drifts. We hand Opus the rule failure
//! (via the shared auto-fix loop) and it proposes a targeted edit to
//! that persona's prompt template in `syntaur-gateway/src/agents/
//! defaults.rs`.
//!
//! Shape:
//!   1. Load `persona-tone.yaml` into a `PersonaToneMatrix`.
//!   2. For each persona in the matrix with an auth token set:
//!        a. For each prompt, POST to `/api/message` with that persona
//!           scoped (Authorization header + persona in body).
//!        b. Apply the persona's rules to the reply text.
//!        c. Emit one `Finding` per failing rule (severity from rule,
//!           default Regression).
//!   3. Emit a summary Suggestion per persona even on clean runs, so
//!      the report has visibility that the phase ran.
//!
//! Design notes:
//!   * Single-turn prompts only in v1 (spec). Multi-turn adds state
//!     management that doesn't earn its keep for register-drift
//!     catches. Callers who need multi-turn can use Phase 4 flow
//!     YAML with `eval:` steps.
//!   * Retries: default 2 with 5s backoff, overridable. Timeouts are
//!     a hard-fail `Finding` — never waved off.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::run::{Finding, FindingKind, Severity};

/// Top-level YAML root. The file is a bare list of `PersonaTone`,
/// mirroring `personas.yaml` in the same crate.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct PersonaToneMatrix(pub Vec<PersonaTone>);

impl PersonaToneMatrix {
    /// Load from a YAML file. Missing file surfaces as a clear error
    /// pointing at the default location.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| {
            format!(
                "reading persona-tone matrix {}: pass --persona-tone-file to override, or \
                 drop a `persona-tone.yaml` next to `personas.yaml` in the crate root",
                path.display()
            )
        })?;
        let matrix: PersonaToneMatrix = serde_yaml::from_str(&raw).with_context(|| {
            format!("parsing persona-tone matrix {}", path.display())
        })?;
        Ok(matrix)
    }

    /// Entry for a specific persona slug (case-sensitive match on slug
    /// field). Returns None if the persona isn't present in the matrix —
    /// meaning "no tone rules for this persona" rather than an error.
    pub fn get(&self, slug: &str) -> Option<&PersonaTone> {
        self.0.iter().find(|p| p.slug == slug)
    }

    /// Every persona slug present in the matrix. Used when
    /// `--persona-tone` is set without a `--persona` filter.
    pub fn slugs(&self) -> Vec<&str> {
        self.0.iter().map(|p| p.slug.as_str()).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaTone {
    pub slug: String,
    #[serde(default)]
    pub prompts: Vec<TonePrompt>,
    #[serde(default)]
    pub rules: Vec<ToneRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonePrompt {
    pub kind: String,
    pub text: String,
}

/// A rule is a bag of optional predicates. `applies` scopes the rule
/// to certain prompt kinds (default = all). Every predicate is checked
/// independently; failing any one fails the rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneRule {
    pub id: String,
    #[serde(default)]
    pub applies: Vec<String>,
    #[serde(default = "default_severity")]
    pub severity: RuleSeverity,

    // Presence predicates
    #[serde(default)]
    pub any_of: Vec<String>,
    #[serde(default)]
    pub all_of: Vec<String>,
    #[serde(default)]
    pub require_phrase_any_of: Vec<String>,
    #[serde(default)]
    pub require_suffix: Option<String>,
    #[serde(default)]
    pub require_code_fence: bool,
    #[serde(default)]
    pub require_citation_marker: bool,
    #[serde(default)]
    pub require_absence_acknowledgment_if_empty_kb: bool,

    // Forbid predicates
    #[serde(default)]
    pub forbid_tokens: Vec<String>,
    #[serde(default)]
    pub forbid_chars: Vec<String>,
    #[serde(default)]
    pub forbid_ranges: Vec<String>,
    #[serde(default)]
    pub forbid_patterns: Vec<String>,
    #[serde(default)]
    pub forbid_prefix_regex: Option<String>,

    // Rate predicates
    #[serde(default)]
    pub rate_at_most: Option<f64>,
    #[serde(default)]
    pub token: Option<String>,

    // Count predicates
    #[serde(default)]
    pub max_lines: Option<usize>,
    #[serde(default)]
    pub max_questions: Option<usize>,
    #[serde(default)]
    pub if_sentences_gt: Option<usize>,
    #[serde(default)]
    pub length_between: Option<Vec<usize>>,

    // Conditional predicates
    #[serde(default)]
    pub if_request_is_code: bool,
    #[serde(default)]
    pub if_response_mentions_knowledge: bool,
    #[serde(default)]
    pub contractions_present: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
    Regression,
    Suggestion,
}

fn default_severity() -> RuleSeverity {
    RuleSeverity::Regression
}

/// Evaluation of a single rule against a single reply. Built for the
/// chat transport layer so unit tests can exercise the rule engine
/// without an HTTP mock.
#[derive(Debug, Clone)]
pub struct RuleOutcome {
    pub rule_id: String,
    pub severity: RuleSeverity,
    pub passed: bool,
    pub reason: Option<String>,
}

impl PersonaTone {
    /// Score one reply against this persona's rules, filtered by
    /// which rules `applies:` to this prompt kind.
    pub fn evaluate(&self, prompt: &TonePrompt, reply: &str) -> Vec<RuleOutcome> {
        let mut out = Vec::with_capacity(self.rules.len());
        for rule in &self.rules {
            if !rule.applies.is_empty() && !rule.applies.contains(&prompt.kind) {
                continue;
            }
            out.push(evaluate_rule(rule, prompt, reply));
        }
        out
    }
}

fn evaluate_rule(rule: &ToneRule, prompt: &TonePrompt, reply: &str) -> RuleOutcome {
    let reply_lc = reply.to_lowercase();

    // Conditional guards — if the conditional prefix doesn't apply,
    // the rule is vacuously satisfied.
    if rule.if_sentences_gt.is_some() {
        let sentences = sentence_count(reply);
        if sentences <= rule.if_sentences_gt.unwrap() {
            return passed(rule, "sentence count below if_sentences_gt threshold");
        }
    }
    if rule.if_request_is_code && !request_is_code(&prompt.text) {
        return passed(rule, "if_request_is_code: false — rule skipped");
    }
    if rule.if_response_mentions_knowledge && !response_mentions_knowledge(reply) {
        return passed(rule, "if_response_mentions_knowledge: false — rule skipped");
    }

    // Presence — any_of / require_phrase_any_of
    for (label, needles) in [
        ("any_of", &rule.any_of),
        ("require_phrase_any_of", &rule.require_phrase_any_of),
    ] {
        if !needles.is_empty() {
            let hit = needles.iter().any(|n| reply.contains(n));
            if !hit {
                return failed(
                    rule,
                    &format!("{label}: none of {:?} present in reply", needles),
                );
            }
        }
    }

    // Presence — all_of
    if !rule.all_of.is_empty() {
        for needle in &rule.all_of {
            if !reply.contains(needle) {
                return failed(rule, &format!("all_of missing `{needle}`"));
            }
        }
    }

    if let Some(suffix) = &rule.require_suffix {
        if !reply.trim_end().ends_with(suffix) {
            return failed(rule, &format!("require_suffix `{suffix}` missing"));
        }
    }
    if rule.require_code_fence && !reply.contains("```") {
        return failed(rule, "require_code_fence: no ``` block in reply");
    }
    if rule.require_citation_marker {
        // Accept either `[…]` or `(doc…)` per spec.
        let re = Regex::new(r"\[[^\]]+\]|\(doc[^)]+\)").unwrap();
        if !re.is_match(reply) {
            return failed(
                rule,
                "require_citation_marker: no [cite] or (doc…) marker found",
            );
        }
    }
    if rule.require_absence_acknowledgment_if_empty_kb {
        // Hand-rolled heuristic: reply must contain one of a handful
        // of "I don't have that" phrases if Cortex is asked about
        // content it can't source. KB emptiness isn't introspectable
        // from here, so we conservatively require the acknowledgment
        // phrase every time — fabricating content and NOT ack-ing is
        // the whole failure mode. If this is too strict in practice,
        // we can relax via a dedicated field.
        let acks = [
            "I don't have",
            "I do not have",
            "No record",
            "Nothing in my knowledge",
            "Not in my knowledge",
            "I can't find",
            "I cannot find",
        ];
        if !acks.iter().any(|a| reply.contains(a)) {
            return failed(
                rule,
                "require_absence_acknowledgment_if_empty_kb: missing I-don't-have phrase",
            );
        }
    }

    // Forbid — tokens (case-insensitive word match)
    for tok in &rule.forbid_tokens {
        if reply_lc.contains(&tok.to_lowercase()) {
            return failed(rule, &format!("forbid_tokens: `{tok}` present in reply"));
        }
    }

    // Forbid — chars
    for c in &rule.forbid_chars {
        if reply.contains(c.as_str()) {
            return failed(rule, &format!("forbid_chars: `{c}` present in reply"));
        }
    }

    // Forbid — ranges (expects `\u{HEX}-\u{HEX}` format)
    for range in &rule.forbid_ranges {
        if let Some((lo, hi)) = parse_codepoint_range(range) {
            if reply.chars().any(|c| (c as u32) >= lo && (c as u32) <= hi) {
                return failed(
                    rule,
                    &format!("forbid_ranges: code point in {range} present"),
                );
            }
        }
    }

    // Forbid — patterns (regex)
    for pat in &rule.forbid_patterns {
        match Regex::new(pat) {
            Ok(re) => {
                if re.is_match(reply) {
                    return failed(rule, &format!("forbid_patterns: `{pat}` matched"));
                }
            }
            Err(e) => {
                return failed(rule, &format!("forbid_patterns: invalid regex `{pat}`: {e}"));
            }
        }
    }

    // Forbid — prefix regex
    if let Some(pat) = &rule.forbid_prefix_regex {
        match Regex::new(pat) {
            Ok(re) => {
                if re.is_match(reply.trim_start()) {
                    return failed(
                        rule,
                        &format!("forbid_prefix_regex: `{pat}` matched at start of reply"),
                    );
                }
            }
            Err(e) => {
                return failed(
                    rule,
                    &format!("forbid_prefix_regex: invalid regex `{pat}`: {e}"),
                );
            }
        }
    }

    // Rate — token occurrences / reply-count (approximated via sentence count for stability)
    if let (Some(rate), Some(tok)) = (rule.rate_at_most, &rule.token) {
        let occurrences = count_occurrences(&reply_lc, &tok.to_lowercase());
        let denom = sentence_count(reply).max(1);
        let observed_rate = occurrences as f64 / denom as f64;
        if observed_rate > rate {
            return failed(
                rule,
                &format!(
                    "rate_at_most: `{tok}` rate {observed_rate:.2} exceeds {rate:.2} ({occurrences}/{denom})"
                ),
            );
        }
    }

    // Count — max_lines
    if let Some(maxl) = rule.max_lines {
        let n = reply.lines().filter(|l| !l.trim().is_empty()).count();
        if n > maxl {
            return failed(
                rule,
                &format!("max_lines: reply has {n} non-empty lines, max {maxl}"),
            );
        }
    }

    // Count — max_questions
    if let Some(maxq) = rule.max_questions {
        let n = reply.matches('?').count();
        if n > maxq {
            return failed(
                rule,
                &format!("max_questions: reply has {n} `?`s, max {maxq}"),
            );
        }
    }

    // Count — length_between
    if let Some(range) = &rule.length_between {
        if range.len() == 2 {
            let n = sentence_count(reply);
            if n < range[0] || n > range[1] {
                return failed(
                    rule,
                    &format!("length_between {:?}: reply has {n} sentences", range),
                );
            }
        }
    }

    // Contractions
    if rule.contractions_present {
        let has_contraction = reply
            .to_lowercase()
            .split(|c: char| !c.is_alphabetic() && c != '\'')
            .any(is_contraction);
        if !has_contraction {
            return failed(rule, "contractions_present: no contractions in reply");
        }
    }

    passed(rule, "all predicates satisfied")
}

fn passed(rule: &ToneRule, reason: &str) -> RuleOutcome {
    RuleOutcome {
        rule_id: rule.id.clone(),
        severity: rule.severity,
        passed: true,
        reason: Some(reason.into()),
    }
}

fn failed(rule: &ToneRule, reason: &str) -> RuleOutcome {
    RuleOutcome {
        rule_id: rule.id.clone(),
        severity: rule.severity,
        passed: false,
        reason: Some(reason.into()),
    }
}

fn sentence_count(text: &str) -> usize {
    text.split(|c: char| c == '.' || c == '!' || c == '?')
        .filter(|s| !s.trim().is_empty())
        .count()
}

fn request_is_code(prompt: &str) -> bool {
    let lc = prompt.to_lowercase();
    ["write me", "one-liner", "write a function", "code to", "rust", "python", "javascript", "script"]
        .iter()
        .any(|h| lc.contains(h))
}

fn response_mentions_knowledge(reply: &str) -> bool {
    let lc = reply.to_lowercase();
    ["knowledge base", "knowledge", "my notes", "the doc", "cited", "source", "according to"]
        .iter()
        .any(|h| lc.contains(h))
}

fn parse_codepoint_range(spec: &str) -> Option<(u32, u32)> {
    // Accepts either plain-hex `HEX-HEX` (the YAML-friendly format,
    // e.g. `1F300-1FAFF`) or Rust-style `\u{HEX}-\u{HEX}` (tolerated
    // for call sites that paste directly from Rust char code).
    let s = spec.trim();
    let (lo, hi) = s.split_once('-')?;
    let parse = |side: &str| -> Option<u32> {
        let side = side.trim();
        let hex = if let Some(rest) = side.strip_prefix("\\u{") {
            rest.strip_suffix('}')?
        } else {
            side
        };
        u32::from_str_radix(hex, 16).ok()
    };
    Some((parse(lo)?, parse(hi)?))
}

fn count_occurrences(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    hay.matches(needle).count()
}

fn is_contraction(word: &str) -> bool {
    let word = word.trim_matches('\'');
    word.contains('\'')
        && word
            .split('\'')
            .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_alphabetic()))
}

// ── Transport — POST /api/message and collect the reply text ────────

/// One tone-audit run against a live gateway. Created once, reused
/// across prompts so HTTP keepalive holds.
pub struct ToneClient {
    http: reqwest::Client,
    target_url: String,
}

#[derive(Debug, Clone, Serialize)]
struct MessagePostBody<'a> {
    persona: &'a str,
    text: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct MessagePostReply {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    reply: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

impl MessagePostReply {
    fn best_text(&self) -> Option<String> {
        self.text
            .clone()
            .or_else(|| self.reply.clone())
            .or_else(|| self.content.clone())
    }
}

pub struct ToneRunConfig {
    pub retries: u32,
    pub retry_backoff: Duration,
    pub timeout: Duration,
}

impl Default for ToneRunConfig {
    fn default() -> Self {
        Self {
            retries: 2,
            retry_backoff: Duration::from_secs(5),
            timeout: Duration::from_secs(45),
        }
    }
}

impl ToneClient {
    pub fn new(target_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("building tone client")?;
        Ok(Self { http, target_url })
    }

    /// Post one prompt as `persona`; retry on timeout. Returns the
    /// reply text or an error after exhausted retries.
    pub async fn ask(
        &self,
        persona_slug: &str,
        auth_token: &str,
        prompt: &TonePrompt,
        cfg: &ToneRunConfig,
    ) -> Result<String> {
        let url = format!("{}/api/message", self.target_url.trim_end_matches('/'));
        let body = MessagePostBody {
            persona: persona_slug,
            text: &prompt.text,
        };
        let mut attempt = 0u32;
        let max_attempts = cfg.retries.saturating_add(1);
        loop {
            attempt += 1;
            let req = self
                .http
                .post(&url)
                .bearer_auth(auth_token)
                .json(&body)
                .timeout(cfg.timeout);
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    let payload: MessagePostReply =
                        resp.json().await.context("parsing /api/message reply")?;
                    return payload
                        .best_text()
                        .ok_or_else(|| anyhow::anyhow!("reply had no text/reply/content field"));
                }
                Ok(resp) => {
                    let status = resp.status();
                    if attempt >= max_attempts {
                        let body_text = resp.text().await.unwrap_or_default();
                        anyhow::bail!(
                            "POST /api/message returned {status} after {attempt} attempts: {}",
                            body_text.chars().take(200).collect::<String>()
                        );
                    }
                }
                Err(e) if e.is_timeout() && attempt < max_attempts => {
                    log::warn!(
                        "[persona-tone] POST timed out ({persona_slug} / {}) — attempt {attempt}/{max_attempts}",
                        prompt.kind
                    );
                }
                Err(e) => {
                    if attempt >= max_attempts {
                        return Err(e).context("POST /api/message");
                    }
                }
            }
            tokio::time::sleep(cfg.retry_backoff).await;
        }
    }
}

/// Run a full tone audit: for every persona in `matrix` whose slug is
/// in `filter_slugs` (or all if `filter_slugs` is empty), POST every
/// prompt, evaluate rules, return Findings.
///
/// `token_lookup` maps persona slug → bearer token. Returns None
/// when no token is available for that persona (env var unset) — the
/// persona is skipped with a warn.
pub async fn run_tone_audit<F>(
    matrix: &PersonaToneMatrix,
    filter_slugs: &[String],
    target_url: &str,
    cfg: &ToneRunConfig,
    token_lookup: F,
) -> Result<Vec<Finding>>
where
    F: Fn(&str) -> Option<String>,
{
    let client = ToneClient::new(target_url.to_string())?;
    let mut findings = Vec::new();
    let personas: Vec<&PersonaTone> = if filter_slugs.is_empty() {
        matrix.0.iter().collect()
    } else {
        matrix
            .0
            .iter()
            .filter(|p| filter_slugs.iter().any(|s| s == &p.slug))
            .collect()
    };

    for persona in personas {
        let token = match token_lookup(&persona.slug) {
            Some(t) => t,
            None => {
                log::warn!(
                    "[persona-tone] no auth token for `{}` — skipping (set its env var in personas.yaml)",
                    persona.slug
                );
                continue;
            }
        };
        let mut persona_pass = 0usize;
        let mut persona_fail = 0usize;
        for prompt in &persona.prompts {
            let reply = match client.ask(&persona.slug, &token, prompt, cfg).await {
                Ok(r) => r,
                Err(e) => {
                    findings.push(Finding {
                        module_slug: "persona-tone".into(),
                        kind: FindingKind::BootFailure,
                        severity: Severity::Regression,
                        title: format!(
                            "Persona `{}` chat API failed on `{}`",
                            persona.slug, prompt.kind
                        ),
                        detail: format!("{e:#}"),
                        artifact: None,
                        captured_at: Utc::now(),
                        edits: None,
                        persona: Some(persona.slug.clone()),
                    });
                    persona_fail += 1;
                    continue;
                }
            };
            for outcome in persona.evaluate(prompt, &reply) {
                if outcome.passed {
                    persona_pass += 1;
                    continue;
                }
                persona_fail += 1;
                let severity = match outcome.severity {
                    RuleSeverity::Regression => Severity::Regression,
                    RuleSeverity::Suggestion => Severity::Suggestion,
                };
                findings.push(Finding {
                    module_slug: "persona-tone".into(),
                    kind: FindingKind::Other,
                    severity,
                    title: format!(
                        "Persona `{}` tone rule `{}` failed on `{}`",
                        persona.slug, outcome.rule_id, prompt.kind
                    ),
                    detail: format!(
                        "{}\n\nPrompt: {}\nReply snippet (first 400 chars):\n{}",
                        outcome.reason.unwrap_or_default(),
                        prompt.text,
                        reply.chars().take(400).collect::<String>()
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: Some(persona.slug.clone()),
                });
            }
        }
        // Per-persona summary Suggestion so the report shows the phase
        // ran even when everything passed.
        findings.push(Finding {
            module_slug: "persona-tone".into(),
            kind: FindingKind::Other,
            severity: Severity::Suggestion,
            title: format!("Persona-tone summary: `{}`", persona.slug),
            detail: format!(
                "{} rule-evaluation(s) passed, {} failed across {} prompt(s)",
                persona_pass,
                persona_fail,
                persona.prompts.len()
            ),
            artifact: None,
            captured_at: Utc::now(),
            edits: None,
            persona: Some(persona.slug.clone()),
        });
    }
    Ok(findings)
}

/// Default persona-tone matrix path inside the crate.
pub fn default_matrix_path(workspace: &Path) -> PathBuf {
    workspace.join("crates/syntaur-verify/persona-tone.yaml")
}

// ── Tests — pure evaluator, no HTTP ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(kind: &str, text: &str) -> TonePrompt {
        TonePrompt {
            kind: kind.into(),
            text: text.into(),
        }
    }

    fn make_rule(id: &str) -> ToneRule {
        ToneRule {
            id: id.into(),
            applies: vec![],
            severity: RuleSeverity::Regression,
            any_of: vec![],
            all_of: vec![],
            require_phrase_any_of: vec![],
            require_suffix: None,
            require_code_fence: false,
            require_citation_marker: false,
            require_absence_acknowledgment_if_empty_kb: false,
            forbid_tokens: vec![],
            forbid_chars: vec![],
            forbid_ranges: vec![],
            forbid_patterns: vec![],
            forbid_prefix_regex: None,
            rate_at_most: None,
            token: None,
            max_lines: None,
            max_questions: None,
            if_sentences_gt: None,
            length_between: None,
            if_request_is_code: false,
            if_response_mentions_knowledge: false,
            contractions_present: false,
        }
    }

    #[test]
    fn any_of_matches_first_phrase() {
        let mut rule = make_rule("ack");
        rule.any_of = vec!["Pencilled in.".into(), "Done.".into()];
        let out = evaluate_rule(&rule, &prompt("any", "any"), "Pencilled in. Event #339 — sync.");
        assert!(out.passed, "{:?}", out.reason);
    }

    #[test]
    fn any_of_fails_when_none_present() {
        let mut rule = make_rule("ack");
        rule.any_of = vec!["Pencilled in.".into(), "Done.".into()];
        let out = evaluate_rule(&rule, &prompt("any", "any"), "Added event #339: sync at 2pm");
        assert!(!out.passed);
    }

    #[test]
    fn forbid_tokens_case_insensitive() {
        let mut rule = make_rule("no_hype");
        rule.forbid_tokens = vec!["Amazing".into()];
        let out = evaluate_rule(&rule, &prompt("any", "any"), "That is amazing work");
        assert!(!out.passed);
    }

    #[test]
    fn forbid_chars_exclamation() {
        let mut rule = make_rule("no_exclaim");
        rule.forbid_chars = vec!["!".into()];
        let out = evaluate_rule(&rule, &prompt("any", "any"), "Done!");
        assert!(!out.passed);
    }

    #[test]
    fn forbid_ranges_blocks_emoji() {
        let mut rule = make_rule("no_emoji");
        rule.forbid_ranges = vec!["\\u{1F300}-\\u{1FAFF}".into()];
        let out = evaluate_rule(
            &rule,
            &prompt("any", "any"),
            "Set up the sync 🗓 for tomorrow",
        );
        assert!(!out.passed);
    }

    #[test]
    fn max_lines_cap() {
        let mut rule = make_rule("short");
        rule.max_lines = Some(2);
        let out = evaluate_rule(
            &rule,
            &prompt("any", "any"),
            "Line one\nLine two\nLine three",
        );
        assert!(!out.passed);
    }

    #[test]
    fn rate_at_most_sir_token() {
        let mut rule = make_rule("sir_sparingly");
        rule.rate_at_most = Some(0.34);
        rule.token = Some("sir".into());
        // 3 sentences, 3 "sir" → 1.0 rate, well above 0.34
        let out = evaluate_rule(
            &rule,
            &prompt("any", "any"),
            "Very good, sir. Noted, sir. Taken care of, sir.",
        );
        assert!(!out.passed);
        // 3 sentences, 1 "sir" → 0.33, just under threshold
        let out = evaluate_rule(
            &rule,
            &prompt("any", "any"),
            "Very good, sir. Noted. Taken care of.",
        );
        assert!(out.passed, "{:?}", out.reason);
    }

    #[test]
    fn contractions_present() {
        let mut rule = make_rule("contractions");
        rule.contractions_present = true;
        let out = evaluate_rule(&rule, &prompt("any", "any"), "I'm ready when you are.");
        assert!(out.passed);
        let out = evaluate_rule(&rule, &prompt("any", "any"), "I am ready when you are.");
        assert!(!out.passed);
    }

    #[test]
    fn require_suffix_signoff() {
        let mut rule = make_rule("signoff");
        rule.require_suffix = Some("—Nyota".into());
        let out = evaluate_rule(
            &rule,
            &prompt("any", "any"),
            "Draft posted to Bluesky.\n—Nyota",
        );
        assert!(out.passed);
        let out2 = evaluate_rule(&rule, &prompt("any", "any"), "Draft posted to Bluesky.");
        assert!(!out2.passed);
    }

    #[test]
    fn applies_filters_to_prompt_kind() {
        let mut rule = make_rule("code_fence");
        rule.applies = vec!["code_request".into()];
        rule.require_code_fence = true;
        let persona = PersonaTone {
            slug: "maurice".into(),
            prompts: vec![],
            rules: vec![rule],
        };
        // Code request without fence — rule fails
        let out = persona.evaluate(
            &prompt("code_request", "Write me a one-liner"),
            "Just use reversed()",
        );
        assert_eq!(out.len(), 1);
        assert!(!out[0].passed);
        // Non-code prompt kind — rule SKIPPED (not evaluated)
        let out = persona.evaluate(
            &prompt("conceptual", "explain memoization"),
            "Plain prose reply",
        );
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn loads_real_matrix_file() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("persona-tone.yaml");
        if !path.exists() {
            return; // skip if repo layout changes
        }
        let matrix = PersonaToneMatrix::load(&path).unwrap();
        assert!(matrix.0.len() >= 5, "expected >=5 personas in matrix");
        assert!(matrix.get("thaddeus").is_some());
    }
}
