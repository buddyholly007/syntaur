//! Stage 1 (Verify Plan v2 Layer A): Cache-Control gate.
//!
//! Walks a static list of user-mutable asset endpoints and asserts
//! every response has a Cache-Control header that's safe for content
//! that can change between deploys. Catches the navigate-away avatar
//! regression class — `/agent-avatar/{id}` shipped with `max-age=300`,
//! WebKitGTK pinned the stale response on persistent disk cache, and
//! voice-mode TTS + chat avatars showed pre-deploy state for hours
//! after a successful ship.
//!
//! Same class as [[feedback/html_cache_must_be_no_store]] (Cache-Control
//! no-store rule for HTML pages) — this is the structural enforcement
//! of the same rule for assets that the rule didn't reach.
//!
//! Safe Cache-Control responses (any one suffices):
//!   - `no-store` (never cache)
//!   - `no-cache` (revalidate every time)
//!   - `max-age=N` where N ≤ 60 (short TTL; bounded staleness)
//!
//! `etag` / `last-modified` ALONE are NOT sufficient — they only
//! trigger revalidation AFTER the freshness window expires. A
//! response with `Cache-Control: max-age=3600` plus `ETag` will be
//! served from cache without any network request for the first hour
//! after a deploy, which matches the regression class we're trying
//! to prevent.
//!
//! `etag` / `last-modified` PAIRED WITH `must-revalidate` is also
//! not enough on its own — `must-revalidate` only forces
//! revalidation once stale, same trap as above.
//!
//! Anything else fails the stage with a Regression Finding so the
//! deploy doesn't ship a cache-stale risk.

use anyhow::Result;
use chrono::Utc;
use std::time::Duration;

use crate::run::{Finding, FindingKind, Severity};

/// Endpoints that serve user-mutable content. Add to this list whenever
/// a new endpoint is created where the response body can change between
/// deploys (or between user actions) but a stale cached copy would
/// confuse the user.
///
/// Curated, not exhaustive — the goal is a deploy-time tripwire on the
/// surfaces Sean has hit cache-stale bugs on, not a comprehensive crawl.
const MUTABLE_ASSETS: &[&str] = &[
    // Avatars per persona — most-trodden path. The 2026-04-30 voice
    // regression chain was this exact endpoint with max-age=300.
    "/agent-avatar/main",
    "/agent-avatar/peter",
    "/agent-avatar/kyron",
    "/agent-avatar/silvr",
    "/agent-avatar/thaddeus",
    "/agent-avatar/mushi",
    "/agent-avatar/nyota",
    "/agent-avatar/cortex",
    "/agent-avatar/positron",
    "/agent-avatar/maurice",
    "/agent-avatar/crimson-lantern",
    // Per-agent icon endpoints — the upload write side; fetch side
    // backs /agent-avatar/{id}.
    "/api/agents/main/icon",
    "/api/agents/peter/icon",
    // Smart-home backdrops — Sean drops PNGs in
    // ~/.syntaur/smart-home/backdrops/ and expects the next page load
    // to reflect them.
    "/assets/smart-home/backdrop/morning.png",
    "/assets/smart-home/backdrop/midday.png",
    "/assets/smart-home/backdrop/evening.png",
    "/assets/smart-home/backdrop/night.png",
];

/// Parse a `max-age=N` directive out of an already-tokenized
/// directive list. Returns None if absent or unparseable. Tolerates
/// optional whitespace around the `=` (RFC 7234 permits it: some
/// upstream proxies emit `max-age = 60`).
fn parse_max_age(dirs: &[String]) -> Option<u64> {
    for d in dirs {
        let stripped = d
            .strip_prefix("max-age")
            .map(|s| s.trim_start())
            .and_then(|s| s.strip_prefix('='))
            .map(|s| s.trim_start());
        if let Some(v) = stripped {
            if let Ok(n) = v.parse::<u64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Tokenize a Cache-Control header value into normalized lowercase
/// directives. Splits on commas, trims whitespace, and lowercases
/// each token. Avoids the substring-matching trap (e.g. so a
/// hypothetical `x-no-store-requested=true` extension can't
/// accidentally satisfy a `no-store` check).
fn directives(cc: &str) -> Vec<String> {
    cc.split(',')
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}

/// Whether the directives contain `no-store` as an exact token.
fn has_directive(dirs: &[String], name: &str) -> bool {
    dirs.iter().any(|d| d == name)
}

/// Decide whether a response with this Cache-Control header is safe
/// for a user-mutable asset. Returns Ok if safe, Err(reason) if not.
///
/// Validators (etag / last-modified) are intentionally NOT inputs —
/// they only revalidate AFTER a freshness window expires; during the
/// window the cache serves the stale copy with zero network. Mutable
/// assets need a freshness window of ≤60s, no validator-gating.
fn classify(cache_control: Option<&str>) -> Result<(), String> {
    let raw = cache_control.unwrap_or("");
    let dirs = directives(raw);

    if has_directive(&dirs, "no-store") {
        return Ok(());
    }
    if has_directive(&dirs, "no-cache") {
        return Ok(());
    }

    if let Some(max_age) = parse_max_age(&dirs) {
        if max_age <= 60 {
            return Ok(());
        }
        return Err(format!(
            "Cache-Control max-age={max_age} (>60s) — stale content will be served \
             for up to {max_age}s after deploy. ETag/Last-Modified do NOT save you \
             during the freshness window — only after expiry."
        ));
    }

    // No max-age, no no-store, no no-cache → heuristic caching applies
    // (browsers commonly default to 10% of Last-Modified age, which can
    // be days for stable resources). Treat as unsafe.
    if raw.trim().is_empty() {
        return Err(
            "no Cache-Control header — browser-default heuristic caching can serve \
             stale content for an unbounded period"
                .into(),
        );
    }
    Err(format!(
        "Cache-Control `{raw}` has no max-age, no-store, or no-cache — \
         heuristic caching applies and content can be served stale indefinitely"
    ))
}

/// Walk every MUTABLE_ASSETS path against `target_url`, return one
/// Finding per violation. Finding `kind` is `Other` (no existing kind
/// is a perfect fit; `Other` matches the bespoke-check intent).
pub async fn check_cache_control(
    target_url: &str,
    auth_token: Option<&str>,
) -> Result<Vec<Finding>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut findings = Vec::new();
    for path in MUTABLE_ASSETS {
        let url = format!("{}{}", target_url.trim_end_matches('/'), path);
        let mut req = client.get(&url);
        if let Some(tok) = auth_token {
            req = req.bearer_auth(tok);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[cache-gate] {url}: request failed: {e:#} — skipping");
                continue;
            }
        };
        // Skip 404/410 — endpoint may not be live in this environment
        // (e.g., persona avatar that hasn't been seeded yet). The gate
        // only assesses endpoints that actually serve content.
        let status = resp.status().as_u16();
        if status == 404 || status == 410 {
            continue;
        }
        if !resp.status().is_success() {
            log::info!("[cache-gate] {url}: status {status} — skipping (not a 2xx body)");
            continue;
        }

        let headers = resp.headers();
        let cc = headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        if let Err(reason) = classify(cc.as_deref()) {
            findings.push(Finding {
                module_slug: "cache-control".into(),
                kind: FindingKind::Other,
                severity: Severity::Regression,
                title: format!("Cache-Control unsafe: {path}"),
                detail: format!(
                    "{url}: {reason}. Mutable assets must respond with one of: \
                     `no-store`, `no-cache`, or `max-age≤60`. ETag/Last-Modified \
                     alone are NOT sufficient — they only revalidate after expiry. \
                     See feedback/html_cache_must_be_no_store.md."
                ),
                artifact: None,
                captured_at: Utc::now(),
                edits: None,
                persona: None,
            });
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_store_passes() {
        assert!(classify(Some("no-store")).is_ok());
        assert!(classify(Some("public, no-store")).is_ok());
    }

    #[test]
    fn no_cache_passes() {
        assert!(classify(Some("no-cache, public")).is_ok());
    }

    #[test]
    fn short_max_age_passes() {
        assert!(classify(Some("max-age=30")).is_ok());
        assert!(classify(Some("public, max-age=60")).is_ok());
    }

    #[test]
    fn max_age_boundary() {
        // 60s is the cutoff; 61s fails.
        assert!(classify(Some("max-age=60")).is_ok());
        assert!(classify(Some("max-age=61")).is_err());
    }

    #[test]
    fn long_max_age_fails_regardless_of_etag() {
        // max-age > 60 is unsafe even when an ETag is present.
        // (ETag only revalidates AFTER expiry; during freshness it
        // serves stale.) The classifier ignores validators by design;
        // this test exists to lock in that semantic.
        assert!(classify(Some("max-age=3600")).is_err());
        assert!(classify(Some("max-age=300, must-revalidate")).is_err());
    }

    #[test]
    fn no_cache_control_fails() {
        assert!(classify(None).is_err());
        assert!(classify(Some("")).is_err());
        assert!(classify(Some("public")).is_err());
    }

    #[test]
    fn whitespace_in_max_age_parses() {
        // Some upstream proxies emit "max-age = 60" with whitespace.
        // RFC 7234 permits OWS around the `=`.
        assert!(classify(Some("max-age = 30")).is_ok());
        assert!(classify(Some("max-age =3600")).is_err());
    }

    #[test]
    fn substring_attack_does_not_satisfy_no_store() {
        // A hypothetical extension directive containing the substring
        // "no-store" must NOT satisfy the no-store check.
        assert!(classify(Some("x-no-store-requested=true, max-age=3600")).is_err());
    }

    #[test]
    fn parse_max_age_extracts_directive() {
        assert_eq!(parse_max_age(&directives("max-age=300")), Some(300));
        assert_eq!(
            parse_max_age(&directives("public, max-age=60, must-revalidate")),
            Some(60)
        );
        assert_eq!(parse_max_age(&directives("no-store")), None);
        assert_eq!(parse_max_age(&directives("max-age=foo")), None);
        assert_eq!(parse_max_age(&directives("max-age = 90")), Some(90));
    }
}
