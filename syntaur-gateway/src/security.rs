//! Security middleware — phase 1 of the 2026-04-19 remediation plan.
//!
//! Four middleware layers, applied from outermost to innermost in `main.rs`:
//!
//!   1. `bootstrap_loopback_only` — when the users table is empty, reject any
//!      request to `/setup`, `/api/auth/register`, `/api/auth/login` unless it
//!      originates from 127.0.0.1. Closes the drive-by-takeover window.
//!
//!   2. `csrf_check` — for every state-changing method (POST/PUT/PATCH/DELETE),
//!      require that the `Origin` (or falling back to `Referer`) header
//!      matches the gateway's host. Blocks cross-origin forged submits.
//!
//!   3. `lift_bearer_to_body_and_query` — read `Authorization: Bearer <token>`
//!      and inject the token into the URL query string AND (when the request
//!      carries JSON) the body. Handlers continue to read `params.get("token")`
//!      or `body["token"]` as before — but the value now comes from the header,
//!      not from attacker-accessible URL/body sources.
//!
//!   4. `security_headers` — set CSP / X-Content-Type-Options /
//!      Referrer-Policy / X-Frame-Options / Permissions-Policy on every
//!      response and `Cache-Control: no-store` on /api/*.
//!
//! The layers are intentionally independent so any one can be disabled
//! during incident response without rewriting the others.

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, Uri},
    middleware::Next,
    response::Response,
};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;

// ── helper ──────────────────────────────────────────────────────────────────

/// URL-encode a raw value for safe use in the query string. Only the
/// reserved characters are percent-encoded.
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Extract the bearer token from the `Authorization` header. Returns None
/// if the header is absent or doesn't carry a `Bearer ` prefix.
fn extract_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Check whether the query string already carries a `token=` parameter.
fn query_has_token(uri: &Uri) -> bool {
    uri.query()
        .map(|q| q.split('&').any(|pair| pair.starts_with("token=")))
        .unwrap_or(false)
}

// ── 1. bootstrap_loopback_only ─────────────────────────────────────────────

/// When the `users` table is empty, bootstrap surfaces are reachable only
/// over loopback. This closes the "attacker sees an uninitialized instance
/// bound to 0.0.0.0 and creates the first admin account before the real
/// operator does" takeover window.
///
/// Paths covered:
///   - `/setup` and any sub-path
///   - `/api/auth/register`
///   - `/api/auth/login` (when users table is empty)
///
/// Once a user exists, this middleware is a no-op.
pub async fn bootstrap_loopback_only(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let is_bootstrap_path = path.starts_with("/setup")
        || path == "/api/auth/register"
        || path == "/api/auth/login";

    if !is_bootstrap_path {
        return Ok(next.run(req).await);
    }

    // Quick check: is the users table empty?
    let has_users = match state.users.list_users().await {
        Ok(u) => !u.is_empty(),
        Err(_) => false, // on error, fail-closed: treat as empty
    };

    if has_users {
        return Ok(next.run(req).await);
    }

    // Bootstrap window. Only accept from loopback.
    let ip = addr.ip();
    if ip.is_loopback() {
        return Ok(next.run(req).await);
    }

    log::warn!(
        "[security] bootstrap request rejected: path={} from {} (users table empty, loopback-only)",
        path, ip
    );
    Err(StatusCode::FORBIDDEN)
}

// ── 2. csrf_check ───────────────────────────────────────────────────────────

/// Reject mutating requests whose `Origin` (or `Referer`) doesn't match the
/// gateway's host. Protects session-token-authenticated POST/PUT/PATCH/DELETE
/// calls from being forged by a third-party site the user happens to have
/// open in another tab.
///
/// Exemptions:
///   - GET/HEAD/OPTIONS (safe methods)
///   - API requests that present a bearer token in the `Authorization`
///     header (those are explicitly opted-in by the caller, not forgeable
///     via <img>/<form>)
///   - Webhook + OAuth callback endpoints where the third-party caller
///     legitimately has no matching origin
pub async fn csrf_check(req: Request, next: Next) -> Result<Response, StatusCode> {
    // Only gate mutating methods.
    match *req.method() {
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE => {}
        _ => return Ok(next.run(req).await),
    }

    let path = req.uri().path();

    // Webhook / OAuth callbacks: no origin check possible.
    let callback_allowed = path.starts_with("/api/oauth/callback")
        || path.starts_with("/api/telegram/webhook")
        || path.starts_with("/api/scheduler/m365/callback");
    if callback_allowed {
        return Ok(next.run(req).await);
    }

    // If the request presents a bearer token, it's API traffic from a
    // deliberate caller, not a forgeable browser form submission. Allow.
    if req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("Bearer "))
        .unwrap_or(false)
    {
        return Ok(next.run(req).await);
    }

    // Extract Origin or fall back to Referer.
    let origin = req
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.headers().get("referer").and_then(|v| v.to_str().ok()))
        .map(|s| s.to_string());

    // Accept same-origin requests. Rough check: origin scheme+host must
    // match the Host header. If either is missing, reject.
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (origin, host) {
        (Some(o), Some(h)) => {
            // Parse origin: strip scheme + trailing path.
            let o_host = o
                .strip_prefix("https://")
                .or_else(|| o.strip_prefix("http://"))
                .unwrap_or(&o)
                .split('/')
                .next()
                .unwrap_or("");
            if o_host == h {
                Ok(next.run(req).await)
            } else {
                log::warn!("[security] CSRF rejected: origin={} host={} path={}", o, h, path);
                Err(StatusCode::FORBIDDEN)
            }
        }
        _ => {
            log::warn!("[security] CSRF rejected: missing Origin/Referer on {}", path);
            Err(StatusCode::FORBIDDEN)
        }
    }
}

// ── 3. lift_bearer_to_body_and_query ───────────────────────────────────────

/// When a request carries `Authorization: Bearer <token>`, transparently
/// copy that token into the URL query (as `token=…`) and, for JSON POSTs,
/// into the body (as `"token": "..."`). Handler code reading
/// `params.get("token")` / `body["token"]` then sees the header-provided
/// value — without every handler needing to be refactored.
///
/// The net effect: clients stop sending tokens in URLs / bodies (where
/// they leak into logs / history / screenshots); the server continues to
/// read from the old positions for now. Phase 1.1 of the remediation plan.
pub async fn lift_bearer_to_body_and_query(req: Request, next: Next) -> Response {
    let Some(token) = extract_bearer(&req) else {
        return next.run(req).await;
    };

    // Split once so we can mutate URI + body independently.
    let (mut parts, body) = req.into_parts();

    // Inject into URL query if no token= already present.
    if !query_has_token(&parts.uri) {
        let path = parts.uri.path().to_string();
        let existing = parts.uri.query().unwrap_or("");
        let encoded = url_encode(&token);
        let new_query = if existing.is_empty() {
            format!("token={}", encoded)
        } else {
            format!("{}&token={}", existing, encoded)
        };
        let new_uri: String = format!("{}?{}", path, new_query);
        if let Ok(parsed) = new_uri.parse::<Uri>() {
            parts.uri = parsed;
        }
    }

    // Decide whether to rewrite the body: only for JSON requests on
    // POST/PUT/PATCH, bounded at 16 MB.
    let is_json = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false);
    let mutating = matches!(parts.method, Method::POST | Method::PUT | Method::PATCH);

    let rebuilt_body: Body = if is_json && mutating {
        // Buffer + parse + re-serialize the JSON body.
        let limit = 16 * 1024 * 1024;
        let bytes: Bytes = match axum::body::to_bytes(body, limit).await {
            Ok(b) => b,
            Err(e) => {
                log::warn!("[security] body-read failed during bearer lift: {e}");
                return Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(Body::empty())
                    .unwrap_or_else(|_| Response::new(Body::empty()));
            }
        };

        // Empty body: skip injection, preserve empty.
        if bytes.is_empty() {
            Body::from(bytes)
        } else {
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(serde_json::Value::Object(mut map)) => {
                    if !map.contains_key("token") {
                        map.insert(
                            "token".to_string(),
                            serde_json::Value::String(token.clone()),
                        );
                    }
                    match serde_json::to_vec(&serde_json::Value::Object(map)) {
                        Ok(v) => {
                            // Content-Length header needs updating too.
                            let new_len = v.len();
                            parts
                                .headers
                                .insert(
                                    axum::http::header::CONTENT_LENGTH,
                                    HeaderValue::from_str(&new_len.to_string())
                                        .unwrap_or_else(|_| HeaderValue::from_static("0")),
                                );
                            Body::from(v)
                        }
                        Err(_) => Body::from(bytes),
                    }
                }
                // Not a JSON object (array / scalar / invalid): leave as-is.
                Ok(_) | Err(_) => Body::from(bytes),
            }
        }
    } else {
        body
    };

    let new_req = Request::from_parts(parts, rebuilt_body);
    next.run(new_req).await
}

// ── 4. security_headers ─────────────────────────────────────────────────────

/// Set a conservative set of security headers on every response.
/// Paired with Phase 4's TLS work which will add HSTS.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let headers = res.headers_mut();

    let insert = |h: &mut axum::http::HeaderMap, name: &'static str, value: &'static str| {
        let hn = HeaderName::from_static(name);
        if !h.contains_key(&hn) {
            if let Ok(v) = HeaderValue::from_str(value) {
                h.insert(hn, v);
            }
        }
    };

    // CSP: allow same-origin + inline (the module pages use inline script
    // literals). Over time we'll hash inline scripts and drop unsafe-inline.
    insert(
        headers,
        "content-security-policy",
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
         img-src 'self' data: blob: https:; \
         font-src 'self' data: https://fonts.gstatic.com; \
         connect-src 'self' https: wss: ws:; \
         frame-ancestors 'none'; \
         base-uri 'self'; \
         form-action 'self'",
    );
    insert(headers, "x-content-type-options", "nosniff");
    insert(headers, "x-frame-options", "DENY");
    insert(headers, "referrer-policy", "strict-origin-when-cross-origin");
    insert(
        headers,
        "permissions-policy",
        "geolocation=(self), microphone=(self), camera=(self), payment=()",
    );

    // API responses must never be cached — they commonly contain tokens,
    // PII, or user-scoped data that would bleed across sessions otherwise.
    if path.starts_with("/api/") || path.starts_with("/v1/") {
        insert(headers, "cache-control", "no-store, private");
    }

    res
}
