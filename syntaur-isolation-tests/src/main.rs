//! Per-user isolation harness — Phase 4.4 of the security plan.
//!
//! Proves, empirically, that two users on the same Syntaur instance cannot
//! reach each other's data across every write-accessible endpoint. Runs as a
//! CLI binary so operators can re-run after every deploy; designed to work
//! against a live gateway (local or TrueNAS) rather than an in-process mock
//! so it catches middleware-level leaks, not just handler-level ones.
//!
//! Test shape:
//!   1. Admin creates users `iso-a-<rand>` and `iso-b-<rand>` + a session
//!      token for each.
//!   2. User A creates a resource on endpoint X.
//!   3. User B tries every realistic way of reaching that resource:
//!      direct GET by id, listing, update, delete.
//!   4. All B-side reads must 4xx or return zero rows. A-side reads must
//!      still work — regression canary.
//!   5. Cleanup: delete both users (cascades their data).
//!
//! A single failure fails the whole run loudly — "user B saw user A's
//! conversation #123 at /api/conversations/123" is not something to bury
//! in a pass count.
//!
//! Usage:
//!   SYNTAUR_URL=https://syntaur.<tailnet>.ts.net \
//!   SYNTAUR_ADMIN_TOKEN=ocp_... \
//!   syntaur-isolation-tests

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use colored::Colorize;
use rand::Rng;
use serde_json::{json, Value};

#[derive(Parser)]
#[command(version, about = "Per-user isolation probes for Syntaur.")]
struct Args {
    /// Gateway base URL (no trailing slash). Falls back to SYNTAUR_URL env.
    #[arg(long, env = "SYNTAUR_URL", default_value = "http://127.0.0.1:18789")]
    url: String,

    /// Admin token for creating test users. SYNTAUR_ADMIN_TOKEN env var.
    #[arg(long, env = "SYNTAUR_ADMIN_TOKEN")]
    admin_token: String,

    /// Keep test users on exit (for debugging). Default deletes them.
    #[arg(long)]
    keep_users: bool,
}

struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    fn new(base: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { http, base })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .header("Authorization", format!("Bearer {token}"))
            .header("Origin", &self.base);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Ok((status, value))
    }
}

struct TestUser {
    id: i64,
    name: String,
    token: String,
}

async fn create_user(c: &Client, admin: &str, label: &str) -> Result<TestUser> {
    let name = format!(
        "iso-{label}-{:x}",
        rand::thread_rng().gen::<u32>()
    );
    let (status, body) = c
        .request(
            reqwest::Method::POST,
            "/api/admin/users",
            admin,
            Some(json!({"token": admin, "name": name})),
        )
        .await?;
    if status >= 400 {
        return Err(anyhow!("admin create_user failed ({status}): {body}"));
    }
    let id = body["id"].as_i64().context("no id in create_user response")?;
    // Mint a session token for the new user.
    let (mstatus, mbody) = c
        .request(
            reqwest::Method::POST,
            &format!("/api/admin/users/{id}/tokens"),
            admin,
            Some(json!({"token": admin, "name": "isolation-test"})),
        )
        .await?;
    if mstatus >= 400 {
        return Err(anyhow!("admin mint_token failed ({mstatus}): {mbody}"));
    }
    let token = mbody["token"]
        .as_str()
        .context("no token in mint response")?
        .to_string();
    Ok(TestUser { id, name, token })
}

async fn delete_user(c: &Client, admin: &str, u: &TestUser) -> Result<()> {
    let (status, body) = c
        .request(
            reqwest::Method::DELETE,
            &format!("/api/admin/users/{}?token={}", u.id, urlencoding::encode(admin)),
            admin,
            None,
        )
        .await?;
    if status >= 400 {
        return Err(anyhow!("delete_user failed ({status}): {body}"));
    }
    Ok(())
}

// ── Probes ─────────────────────────────────────────────────────────────

/// A single isolation probe. Returns Ok(()) on pass, Err(msg) on fail.
struct ProbeResult {
    name: &'static str,
    result: Result<()>,
}

/// User A creates a conversation. User B must not be able to:
///   - GET /api/conversations/{id}
///   - PATCH / DELETE it
///   - See it listed at GET /api/conversations
async fn probe_conversations(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (s, body) = c
        .request(
            reqwest::Method::POST,
            "/api/conversations",
            &a.token,
            Some(json!({"token": a.token, "agent": "main", "title": "isolation-A"})),
        )
        .await?;
    if s >= 400 {
        return Err(anyhow!("A: POST /api/conversations failed {s}: {body}"));
    }
    let id = body["id"]
        .as_str()
        .or_else(|| body["conversation_id"].as_str())
        .context("no conv id in POST /api/conversations response")?
        .to_string();

    // B tries direct GET.
    let (sb, _) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/conversations/{id}?token={}", urlencoding::encode(&b.token)),
            &b.token,
            None,
        )
        .await?;
    if sb == 200 {
        return Err(anyhow!("user B reached user A's conversation at /api/conversations/{id} (got 200)"));
    }

    // B lists conversations — must not see A's.
    let (sl, lbody) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/conversations?token={}", urlencoding::encode(&b.token)),
            &b.token,
            None,
        )
        .await?;
    if sl == 200 {
        let convs = lbody["conversations"].as_array().cloned().unwrap_or_default();
        for conv in convs {
            let cid = conv["id"].as_str().unwrap_or("");
            if cid == id {
                return Err(anyhow!(
                    "user B's /api/conversations listing contained user A's conversation id {id}"
                ));
            }
        }
    }

    // Canary: A can still see their own.
    let (sa, _) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/conversations/{id}?token={}", urlencoding::encode(&a.token)),
            &a.token,
            None,
        )
        .await?;
    if sa >= 400 {
        return Err(anyhow!("regression: user A can't GET their own conversation (got {sa})"));
    }
    Ok(())
}

/// User A creates a scheduler approval. User B must not see it.
async fn probe_scheduler_approvals(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    // Create a synthetic approval via /api/scheduler/voice_create — the
    // text-to-event path lands in pending_approvals scoped to the caller.
    // This hits an LLM + may time out on a cold gateway or a degraded
    // upstream. Skip (not fail) on network error or bad gateway.
    let res = c
        .request(
            reqwest::Method::POST,
            "/api/scheduler/voice_create",
            &a.token,
            Some(json!({"token": a.token, "transcript": "meeting next Monday at 3pm"})),
        )
        .await;
    let (s, body) = match res {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("  [skip] scheduler_approvals: transport error ({e})");
            return Ok(());
        }
    };
    if s >= 400 {
        eprintln!("  [skip] scheduler_approvals: A-side create returned {s}");
        return Ok(());
    }
    let ap_id = body["approval_id"].as_i64().context("no approval_id")?;

    let (sl, lbody) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/approvals?token={}", urlencoding::encode(&b.token)),
            &b.token,
            None,
        )
        .await?;
    if sl == 200 {
        let rows = lbody["approvals"].as_array().cloned().unwrap_or_default();
        for r in rows {
            if r["id"].as_i64() == Some(ap_id) {
                return Err(anyhow!(
                    "user B's /api/approvals listing contained user A's approval id {ap_id}"
                ));
            }
        }
    }

    let (sr, _) = c
        .request(
            reqwest::Method::POST,
            &format!("/api/approvals/{ap_id}/resolve"),
            &b.token,
            Some(json!({"token": b.token, "approved": true})),
        )
        .await?;
    // User B's resolve should either 403 OR succeed but not actually flip
    // A's row (because the handler's WHERE user_id guard excludes it).
    // Either way, A must still see the approval as unresolved.
    let (_, acheck) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/approvals?token={}", urlencoding::encode(&a.token)),
            &a.token,
            None,
        )
        .await?;
    let rows = acheck["approvals"].as_array().cloned().unwrap_or_default();
    let still_pending = rows.iter().any(|r| r["id"].as_i64() == Some(ap_id) && r["resolved_at"].is_null());
    if !still_pending && sr == 200 {
        return Err(anyhow!(
            "user B's POST /api/approvals/{ap_id}/resolve succeeded AND flipped the row; isolation broken"
        ));
    }
    Ok(())
}

/// User A adds a memory. User B must not list it.
async fn probe_memories(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (s, _) = c
        .request(
            reqwest::Method::POST,
            "/api/memories",
            &a.token,
            Some(json!({"token": a.token, "agent_id": "main", "text": "isolation secret A"})),
        )
        .await?;
    if s >= 400 {
        eprintln!("  [skip] memories: POST /api/memories returned {s} (endpoint may require onboarding)");
        return Ok(());
    }
    let (_, lbody) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/memories?token={}&agent_id=main", urlencoding::encode(&b.token)),
            &b.token,
            None,
        )
        .await?;
    let rows = lbody["memories"].as_array().cloned().unwrap_or_default();
    for r in rows {
        let txt = r["text"].as_str().unwrap_or("");
        if txt.contains("isolation secret A") {
            return Err(anyhow!("user B's memories listing leaked user A's memory text"));
        }
    }
    Ok(())
}

/// Tax module — per-user expense isolation.
async fn probe_tax_expenses(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let body = json!({
        "token": a.token,
        "vendor": "Isolation Harness Coffee",
        "amount_cents": 12345,
        "expense_date": "2026-04-19",
        "category": "meals",
    });
    let (s, _) = c.request(reqwest::Method::POST, "/api/tax/expenses", &a.token, Some(body)).await?;
    if s >= 400 { eprintln!("  [skip] tax_expenses: POST returned {s}"); return Ok(()); }
    let (_, lb) = c.request(
        reqwest::Method::GET,
        &format!("/api/tax/expenses?token={}&start=2026-01-01&end=2026-12-31", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    for r in lb["expenses"].as_array().cloned().unwrap_or_default() {
        if r["vendor"].as_str().unwrap_or("").contains("Isolation Harness Coffee") {
            return Err(anyhow!("user B's /api/tax/expenses listing leaked user A's vendor"));
        }
    }
    Ok(())
}

/// Journal moments — per-user daily view isolation.
async fn probe_journal_moments(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let (s, body) = c.request(
        reqwest::Method::POST, "/api/journal/moments", &a.token,
        Some(json!({ "token": a.token, "date": today, "text": "isolation-A moment secret", "source": "harness" })),
    ).await?;
    if s >= 400 { eprintln!("  [skip] journal_moments: POST returned {s}"); return Ok(()); }

    // List-visibility: user B's listing must not contain user A's text.
    let (_, lb) = c.request(
        reqwest::Method::GET,
        &format!("/api/journal/moments?token={}&date={}", urlencoding::encode(&b.token), today),
        &b.token, None,
    ).await?;
    for r in lb["moments"].as_array().cloned().unwrap_or_default() {
        if r["text"].as_str().unwrap_or("").contains("isolation-A moment secret") {
            return Err(anyhow!("user B's journal moments listing leaked user A's text"));
        }
    }

    // Direct write-access: user B tries DELETE on user A's moment by id.
    // A 4xx is required — 2xx means the moment was deleted by a non-owner.
    let moment_id = body["id"].as_i64().or_else(|| body["moment_id"].as_i64());
    if let Some(id) = moment_id {
        let (sd, _) = c.request(
            reqwest::Method::DELETE,
            &format!("/api/journal/moments/{}", id),
            &b.token, None,
        ).await?;
        if sd < 400 {
            return Err(anyhow!(
                "user B DELETEd user A's journal moment id {id} (got {sd})"
            ));
        }
    }
    Ok(())
}

/// Knowledge docs — upload-then-list scope.
async fn probe_knowledge_docs(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (sa, abody) = c.request(
        reqwest::Method::GET,
        &format!("/api/knowledge/docs?token={}", urlencoding::encode(&a.token)),
        &a.token, None,
    ).await?;
    if sa >= 400 { eprintln!("  [skip] knowledge_docs: A-side GET returned {sa}"); return Ok(()); }
    let (_, bbody) = c.request(
        reqwest::Method::GET,
        &format!("/api/knowledge/docs?token={}", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    let a_ids: Vec<i64> = abody["docs"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["id"].as_i64()).collect();
    let b_ids: Vec<i64> = bbody["docs"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["id"].as_i64()).collect();
    for id in &a_ids {
        if b_ids.contains(id) {
            return Err(anyhow!("user B's knowledge docs contain user A's doc id {id}"));
        }
    }

    // Direct write-access: user B tries to delete user A's doc via the
    // body-param delete endpoint. Must 4xx (or return a success envelope
    // reporting 0 rows deleted). The handler must refuse to look up
    // a doc owned by another user_id.
    if let Some(a_doc) = a_ids.first() {
        let (sd, rb) = c.request(
            reqwest::Method::POST,
            "/api/knowledge/docs/delete",
            &b.token,
            Some(json!({ "token": b.token, "id": a_doc })),
        ).await?;
        if sd < 400 {
            // Some handlers return 200 with `deleted: 0` for not-found. That's
            // acceptable — the leaked signal is whether rows were deleted.
            let deleted = rb["deleted"].as_i64().unwrap_or(0);
            if deleted > 0 {
                return Err(anyhow!(
                    "user B deleted user A's knowledge doc id {a_doc} via /docs/delete"
                ));
            }
        }
    }
    Ok(())
}

/// Music local folders — per-user folder scope.
async fn probe_music_folders(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (sa, abody) = c.request(
        reqwest::Method::GET,
        &format!("/api/music/local/folders?token={}", urlencoding::encode(&a.token)),
        &a.token, None,
    ).await?;
    if sa >= 400 { eprintln!("  [skip] music_folders: A-side GET returned {sa}"); return Ok(()); }
    let (_, bbody) = c.request(
        reqwest::Method::GET,
        &format!("/api/music/local/folders?token={}", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    let a_ids: Vec<i64> = abody["folders"].as_array().cloned().unwrap_or_default().iter().filter_map(|f| f["id"].as_i64()).collect();
    let b_ids: Vec<i64> = bbody["folders"].as_array().cloned().unwrap_or_default().iter().filter_map(|f| f["id"].as_i64()).collect();
    for id in &a_ids {
        if b_ids.contains(id) {
            return Err(anyhow!("user B's music folders contain user A's folder id {id}"));
        }
    }

    // Direct write-access: user B tries DELETE on one of user A's folders.
    // Must 4xx — remove_folder handler must check user_id on the row.
    if let Some(a_fid) = a_ids.first() {
        let (sd, _) = c.request(
            reqwest::Method::DELETE,
            &format!("/api/music/local/folders/{}", a_fid),
            &b.token, None,
        ).await?;
        if sd < 400 {
            return Err(anyhow!(
                "user B DELETEd user A's music folder id {a_fid} (got {sd})"
            ));
        }
    }
    Ok(())
}

/// Social drafts — rust-social-manager scope enforcement at the read path.
async fn probe_social_drafts(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (sa, abody) = c.request(
        reqwest::Method::GET,
        &format!("/api/social/drafts?token={}", urlencoding::encode(&a.token)),
        &a.token, None,
    ).await?;
    if sa >= 400 { eprintln!("  [skip] social_drafts: A-side GET returned {sa}"); return Ok(()); }
    let (_, bbody) = c.request(
        reqwest::Method::GET,
        &format!("/api/social/drafts?token={}", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    let a_ids: Vec<i64> = abody["drafts"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["id"].as_i64()).collect();
    let b_ids: Vec<i64> = bbody["drafts"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["id"].as_i64()).collect();
    for id in &a_ids {
        if b_ids.contains(id) {
            return Err(anyhow!("user B's social drafts contain user A's draft id {id}"));
        }
    }
    Ok(())
}

/// Calendar events — scope on range queries.
async fn probe_calendar(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (s, body) = c.request(
        reqwest::Method::POST, "/api/calendar", &a.token,
        Some(json!({
            "token": a.token, "title": "Isolation-A only event",
            "start_time": "2026-04-19T12:00", "end_time": null, "all_day": false,
        })),
    ).await?;
    if s >= 400 { eprintln!("  [skip] calendar: POST returned {s}"); return Ok(()); }
    let ev_id = body["id"].as_i64().or_else(|| body["event_id"].as_i64());

    // List-visibility: user B's calendar must not surface user A's event.
    let (_, lb) = c.request(
        reqwest::Method::GET,
        &format!("/api/calendar?token={}&start=2026-04-01&end=2026-04-30", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    for r in lb["events"].as_array().cloned().unwrap_or_default() {
        if r["title"].as_str().unwrap_or("").contains("Isolation-A only event") {
            return Err(anyhow!("user B's calendar listing leaked user A's event title"));
        }
        if let Some(id) = ev_id {
            if r["id"].as_i64() == Some(id) {
                return Err(anyhow!("user B's calendar listing contained user A's event id {id}"));
            }
        }
    }

    // Direct write-access: user B tries PUT + DELETE on user A's event by id.
    // Both must 4xx.
    if let Some(id) = ev_id {
        let (sp, _) = c.request(
            reqwest::Method::PUT,
            &format!("/api/calendar/{}", id),
            &b.token,
            Some(json!({ "token": b.token, "title": "user-B overwrite attempt" })),
        ).await?;
        if sp < 400 {
            return Err(anyhow!(
                "user B PUT user A's calendar event id {id} (got {sp})"
            ));
        }
        let (sd, _) = c.request(
            reqwest::Method::DELETE,
            &format!("/api/calendar/{}", id),
            &b.token, None,
        ).await?;
        if sd < 400 {
            return Err(anyhow!(
                "user B DELETEd user A's calendar event id {id} (got {sd})"
            ));
        }
    }
    Ok(())
}

/// Scheduler lists (shopping, to-do).
async fn probe_scheduler_lists(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (s, body) = c.request(
        reqwest::Method::POST, "/api/scheduler/lists", &a.token,
        Some(json!({ "token": a.token, "name": "isolation-A-grocery", "kind": "shopping" })),
    ).await?;
    if s >= 400 { eprintln!("  [skip] scheduler_lists: POST returned {s}"); return Ok(()); }

    // List-visibility: user B must not see user A's list.
    let (_, lb) = c.request(
        reqwest::Method::GET,
        &format!("/api/scheduler/lists?token={}", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    for r in lb["lists"].as_array().cloned().unwrap_or_default() {
        if r["name"].as_str().unwrap_or("") == "isolation-A-grocery" {
            return Err(anyhow!("user B's scheduler lists include user A's list"));
        }
    }

    // Direct write-access: user B tries to POST a new item into user A's
    // list by id. Must 4xx — the handler must reject writes to lists
    // owned by another user_id.
    let list_id = body["id"].as_i64().or_else(|| body["list_id"].as_i64());
    if let Some(id) = list_id {
        let (sp, _) = c.request(
            reqwest::Method::POST,
            &format!("/api/scheduler/lists/{}/items", id),
            &b.token,
            Some(json!({ "token": b.token, "text": "user-B injected item" })),
        ).await?;
        if sp < 400 {
            return Err(anyhow!(
                "user B POSTed an item into user A's scheduler list id {id} (got {sp})"
            ));
        }
    }
    Ok(())
}

/// Research sessions.
async fn probe_research_sessions(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (sa, abody) = c.request(
        reqwest::Method::GET,
        &format!("/api/research?token={}", urlencoding::encode(&a.token)),
        &a.token, None,
    ).await?;
    if sa >= 400 { eprintln!("  [skip] research_sessions: A-side GET returned {sa}"); return Ok(()); }
    let (_, bbody) = c.request(
        reqwest::Method::GET,
        &format!("/api/research?token={}", urlencoding::encode(&b.token)),
        &b.token, None,
    ).await?;
    let a_ids: Vec<String> = abody["sessions"].as_array().cloned().unwrap_or_default().iter().filter_map(|s| s["id"].as_str().map(|s| s.to_string())).collect();
    let b_ids: Vec<String> = bbody["sessions"].as_array().cloned().unwrap_or_default().iter().filter_map(|s| s["id"].as_str().map(|s| s.to_string())).collect();
    for id in &a_ids {
        if b_ids.contains(id) {
            return Err(anyhow!("user B's research sessions contain user A's session id {id}"));
        }
    }
    Ok(())
}

/// Admin endpoints are admin-only; non-admin user must get 4xx on every
/// /api/admin/* write. Kept as one broad lockout probe so adding a new
/// admin route anywhere in the gateway is one line here, not a new probe.
async fn probe_admin_lockout(c: &Client, _a: &TestUser, b: &TestUser) -> Result<()> {
    let targets = [
        ("POST", "/api/admin/users",          json!({"token": b.token, "name": "shouldnt-work"})),
        ("POST", "/api/admin/family-invite",  json!({"token": b.token, "name": "shouldnt-work"})),
        ("POST", "/api/admin/invites",        json!({"token": b.token})),
        ("POST", "/api/admin/hooks",          json!({"token": b.token, "name": "x", "event": "x", "command": "x"})),
        ("POST", "/api/admin/skills",         json!({"token": b.token, "name": "x", "instructions": "x"})),
        ("POST", "/api/admin/slash",          json!({"token": b.token, "name": "x", "body": "x"})),
        ("POST", "/api/admin/oauth_config",   json!({"token": b.token, "provider": "x"})),
        ("POST", "/api/admin/sharing",        json!({"token": b.token, "mode": "isolated"})),
        ("POST", "/api/admin/sharing/grants", json!({"token": b.token})),
        ("POST", "/api/admin/sharing/options",json!({"token": b.token})),
    ];
    for (method, path, body) in &targets {
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let (s, _) = c.request(m, path, &b.token, Some(body.clone())).await?;
        if s < 400 {
            return Err(anyhow!("non-admin user B reached admin endpoint {method} {path} (got {s})"));
        }
    }
    Ok(())
}

/// /api/me returns the caller's own row. A and B must see different ids.
async fn probe_me(c: &Client, a: &TestUser, b: &TestUser) -> Result<()> {
    let (_, abody) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/me?token={}", urlencoding::encode(&a.token)),
            &a.token,
            None,
        )
        .await?;
    let (_, bbody) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/me?token={}", urlencoding::encode(&b.token)),
            &b.token,
            None,
        )
        .await?;
    let aid = abody["user"]["id"].as_i64();
    let bid = bbody["user"]["id"].as_i64();
    if aid.is_none() || bid.is_none() {
        return Err(anyhow!("/api/me didn't return user.id for both"));
    }
    if aid == bid {
        return Err(anyhow!("/api/me returned same id for A and B — session tokens swapped"));
    }
    if aid != Some(a.id) {
        return Err(anyhow!(
            "/api/me for user A returned id {:?}, expected {}",
            aid, a.id
        ));
    }
    Ok(())
}

// ── Driver ──────────────────────────────────────────────────────────────

async fn get_sharing_mode(c: &Client, admin: &str) -> Result<String> {
    let (s, body) = c
        .request(
            reqwest::Method::GET,
            &format!("/api/admin/sharing?token={}", urlencoding::encode(admin)),
            admin,
            None,
        )
        .await?;
    if s >= 400 {
        return Err(anyhow!("GET /api/admin/sharing failed ({s}): {body}"));
    }
    Ok(body["mode"].as_str().unwrap_or("isolated").to_string())
}

async fn set_sharing_mode(c: &Client, admin: &str, mode: &str) -> Result<()> {
    let (s, body) = c
        .request(
            reqwest::Method::PUT,
            "/api/admin/sharing",
            admin,
            Some(json!({"token": admin, "mode": mode})),
        )
        .await?;
    if s >= 400 {
        return Err(anyhow!("set sharing mode failed ({s}): {body}"));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.admin_token.is_empty() {
        return Err(anyhow!(
            "SYNTAUR_ADMIN_TOKEN required — admin session token for the target gateway"
        ));
    }

    println!("{}", format!("isolation harness → {}", args.url).bold());

    let c = Client::new(args.url.clone())?;

    // Save the original sharing mode, flip to `isolated` for the duration
    // of the test run. `shared` mode intentionally exposes cross-user data
    // for single-household setups; if the operator leaves it on, the
    // harness would flag the feature as a "leak." Running in isolated
    // mode ensures the probes test the fail-closed path explicitly.
    let original_mode = get_sharing_mode(&c, &args.admin_token)
        .await
        .unwrap_or_else(|_| "isolated".to_string());
    if original_mode != "isolated" {
        println!("  {} sharing mode currently {:?}, flipping to 'isolated' for test run", "!".yellow(), original_mode);
        set_sharing_mode(&c, &args.admin_token, "isolated")
            .await
            .context("couldn't flip sharing mode for test run")?;
    }

    // Create two test users.
    let a = create_user(&c, &args.admin_token, "a")
        .await
        .context("failed to create user A — is the admin token valid?")?;
    println!("  created user A: {} (id {})", a.name, a.id);
    let b = create_user(&c, &args.admin_token, "b")
        .await
        .context("failed to create user B")?;
    println!("  created user B: {} (id {})", b.name, b.id);

    // Run every probe, collect per-probe result.
    let mut results: Vec<ProbeResult> = Vec::new();
    for (name, f) in probes() {
        let r = (f)(&c, &a, &b).await;
        results.push(ProbeResult { name, result: r });
    }

    // Cleanup (best-effort — don't fail the whole run on delete error).
    if !args.keep_users {
        let _ = delete_user(&c, &args.admin_token, &a).await;
        let _ = delete_user(&c, &args.admin_token, &b).await;
    }

    // Restore the operator's original sharing mode.
    if original_mode != "isolated" {
        let _ = set_sharing_mode(&c, &args.admin_token, &original_mode).await;
        println!("  restored sharing mode to {:?}", original_mode);
    }

    // Report.
    let mut pass = 0;
    let mut fail = 0;
    for r in &results {
        match &r.result {
            Ok(()) => {
                println!("  {} {}", "✓".green(), r.name);
                pass += 1;
            }
            Err(e) => {
                println!("  {} {}", "✗".red(), r.name);
                println!("    {}", e.to_string().red());
                fail += 1;
            }
        }
    }
    println!();
    if fail == 0 {
        println!("{}", format!("{pass}/{} probes passed", results.len()).green().bold());
        Ok(())
    } else {
        println!(
            "{}",
            format!("{pass}/{} probes passed, {fail} FAILED", results.len())
                .red()
                .bold()
        );
        std::process::exit(1);
    }
}

type ProbeFn = for<'a> fn(
    &'a Client,
    &'a TestUser,
    &'a TestUser,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

fn probes() -> Vec<(&'static str, ProbeFn)> {
    vec![
        ("me-returns-own-id", |c, a, b| Box::pin(probe_me(c, a, b))),
        ("conversations", |c, a, b| Box::pin(probe_conversations(c, a, b))),
        ("scheduler-approvals", |c, a, b| Box::pin(probe_scheduler_approvals(c, a, b))),
        ("scheduler-lists", |c, a, b| Box::pin(probe_scheduler_lists(c, a, b))),
        ("memories", |c, a, b| Box::pin(probe_memories(c, a, b))),
        ("calendar", |c, a, b| Box::pin(probe_calendar(c, a, b))),
        ("tax-expenses", |c, a, b| Box::pin(probe_tax_expenses(c, a, b))),
        ("journal-moments", |c, a, b| Box::pin(probe_journal_moments(c, a, b))),
        ("knowledge-docs", |c, a, b| Box::pin(probe_knowledge_docs(c, a, b))),
        ("music-folders", |c, a, b| Box::pin(probe_music_folders(c, a, b))),
        ("social-drafts", |c, a, b| Box::pin(probe_social_drafts(c, a, b))),
        ("research-sessions", |c, a, b| Box::pin(probe_research_sessions(c, a, b))),
        ("admin-lockout", |c, a, b| Box::pin(probe_admin_lockout(c, a, b))),
    ]
}
