//! Social runtime engine — drafts, replies, generator, publisher,
//! notifications poller, engagement loop, stats snapshot, post monitor.
//!
//! All the CRUD + behavior the /social module needs beyond connection
//! management. This module is intentionally stateful: it reads/writes
//! social_drafts, social_replies, social_engagement_log,
//! social_stats_snapshots, social_alerts, social_pillar_cursor.
//!
//! Entry points:
//!   - `POST /api/social/drafts`            generate or insert a manual draft
//!   - `GET  /api/social/drafts`            list current drafts (Queue pane)
//!   - `POST /api/social/drafts/:id/approve`  publish via adapter
//!   - `POST /api/social/drafts/:id/redraft` regenerate via LLM
//!   - `DELETE /api/social/drafts/:id`      reject
//!   - `GET  /api/social/replies`           list pending replies (Inbox)
//!   - `POST /api/social/replies/:id/approve` publish reply
//!   - `DELETE /api/social/replies/:id`     reject reply
//!   - `GET  /api/social/stats`             latest snapshot per platform
//!   - `GET  /api/social/alerts`            open alerts
//!
//! Cron-facing helpers (no HTTP route — called from Syntaur's cron runner):
//!   - `run_draft_tick(state)`    fires for users whose cadence matches now
//!   - `run_notify_poll(state)`   drains platform notifications into social_replies
//!   - `run_engagement_tick(s)`   likes/follows/unfollows per preset
//!   - `run_stats_snapshot(s)`    weekly stats per user+platform
//!   - `run_post_monitor(s)`      scans recent posts, flags blocklist hits
//!
//! These are invoked by named skills which a cron job runs (see
//! migrate_cron_jobs() which installs the Syntaur-native replacements
//! for the rust-social-manager entries).

use std::sync::Arc;

use axum::{extract::{Path, Query, State}, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

use crate::AppState;
use super::platforms::{self, SocialPlatform};

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Draft {
    pub id: i64,
    pub platform: String,
    pub agent_id: Option<String>,
    pub text: String,
    pub pillar: Option<String>,
    pub source: String,
    pub status: String,
    pub scheduled_for: Option<i64>,
    pub posted_uri: Option<String>,
    pub posted_at: Option<i64>,
    pub error_detail: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReplyDraft {
    pub id: i64,
    pub platform: String,
    pub agent_id: Option<String>,
    pub parent_uri: String,
    pub parent_author: Option<String>,
    pub parent_text: Option<String>,
    pub draft_text: Option<String>,
    pub status: String,
    pub posted_uri: Option<String>,
    pub posted_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Deserialize)]
pub struct CreateDraftRequest {
    pub token: String,
    pub platform: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub pillar: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub scheduled_for: Option<i64>,
    #[serde(default)]
    pub generate: bool,
}

#[derive(Deserialize)]
pub struct TokenOnly { pub token: String }

#[derive(Deserialize)]
pub struct ApproveRequest {
    pub token: String,
    #[serde(default)]
    pub edited_text: Option<String>,
}

#[derive(Deserialize)]
pub struct RedraftRequest {
    pub token: String,
    #[serde(default)]
    pub hint: Option<String>,
}

// ── Draft CRUD handlers ─────────────────────────────────────────────────────

pub async fn handle_drafts_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<Draft>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let uid = principal.user_id();
    let filter_status = params.get("status").cloned();
    let db = state.db_path.clone();

    let drafts = tokio::task::spawn_blocking(move || -> Vec<Draft> {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(s) = filter_status {
            ("SELECT id, platform, agent_id, text, pillar, source, status, scheduled_for, \
              posted_uri, posted_at, error_detail, created_at, updated_at \
              FROM social_drafts WHERE user_id = ? AND status = ? \
              ORDER BY created_at DESC LIMIT 200".to_string(),
              vec![Box::new(uid), Box::new(s)])
        } else {
            ("SELECT id, platform, agent_id, text, pillar, source, status, scheduled_for, \
              posted_uri, posted_at, error_detail, created_at, updated_at \
              FROM social_drafts WHERE user_id = ? \
              ORDER BY created_at DESC LIMIT 200".to_string(),
              vec![Box::new(uid)])
        };
        let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return vec![] };
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let iter = stmt.query_map(refs.as_slice(), |r| Ok(Draft {
            id: r.get(0)?, platform: r.get(1)?, agent_id: r.get(2)?, text: r.get(3)?,
            pillar: r.get(4)?, source: r.get(5)?, status: r.get(6)?, scheduled_for: r.get(7)?,
            posted_uri: r.get(8)?, posted_at: r.get(9)?, error_detail: r.get(10)?,
            created_at: r.get(11)?, updated_at: r.get(12)?,
        }));
        match iter {
            Ok(i) => i.filter_map(Result::ok).collect(),
            Err(_) => vec![],
        }
    })
    .await
    .unwrap_or_default();

    Ok(Json(drafts))
}

pub async fn handle_drafts_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateDraftRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, msg: String| (code, Json(serde_json::json!({ "ok": false, "error": msg })));
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err(s, "Sign in again.".to_string()))?;
    let uid = principal.user_id();

    // Generate or use supplied text
    let (text, pillar) = if req.generate {
        match generate_draft_text(&state, uid, &req.platform, req.agent_id.as_deref()).await {
            Ok((t, p)) => (t, p),
            Err(e) => return Err(err(StatusCode::BAD_GATEWAY, e)),
        }
    } else {
        (req.text.unwrap_or_default(), req.pillar.clone())
    };
    if text.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Draft text is empty.".to_string()));
    }

    let source = req.source.clone().unwrap_or_else(|| "manual".to_string());
    let source_for_insert = source.clone();
    let agent_id = req.agent_id.clone();
    let platform = req.platform.clone();
    let scheduled_for = req.scheduled_for;
    let now = chrono::Utc::now().timestamp();
    let text_clone = text.clone();
    let pillar_clone = pillar.clone();
    let db = state.db_path.clone();

    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let conn_id: Option<i64> = conn.query_row(
            "SELECT id FROM social_connections WHERE user_id = ? AND platform = ? \
             ORDER BY COALESCE(last_verified_at, updated_at) DESC LIMIT 1",
            rusqlite::params![uid, platform],
            |r| r.get(0),
        ).ok();
        conn.execute(
            "INSERT INTO social_drafts (user_id, platform, agent_id, connection_id, text, pillar, \
                source, status, scheduled_for, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)",
            rusqlite::params![uid, platform, agent_id, conn_id, text_clone, pillar_clone, source_for_insert, scheduled_for, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "server".to_string()))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Send Telegram mirror (best-effort)
    telegram_send_draft(&state, uid, id).await;

    // Approval-mode enforcement: if user has auto_post_all, publish immediately.
    // auto_post_routine only fires from the generator path (see generate_and_maybe_publish).
    let approval_mode = read_pref(&state, uid, "social.approval_mode").await
        .unwrap_or_else(|| "always_review".to_string());
    if source == "auto" && approval_mode == "auto_post_all" {
        let _ = publish_draft_now(&state, uid, id).await;
    }

    Ok(Json(serde_json::json!({ "ok": true, "id": id, "text": text, "pillar": pillar })))
}

pub async fn handle_draft_approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<ApproveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, msg: String| (code, Json(serde_json::json!({ "ok": false, "error": msg })));
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err(s, "Sign in again.".to_string()))?;
    let uid = principal.user_id();

    // If user edited the text before approving, persist the edit first.
    if let Some(edited) = req.edited_text.as_ref() {
        if !edited.trim().is_empty() {
            let db = state.db_path.clone();
            let new_text = edited.clone();
            let now = chrono::Utc::now().timestamp();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_drafts SET text = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                    rusqlite::params![new_text, now, id, uid],
                ).ok()
            }).await;
        }
    }

    match publish_draft_now(&state, uid, id).await {
        Ok(post_ref) => Ok(Json(serde_json::json!({ "ok": true, "uri": post_ref.uri, "posted_at": post_ref.posted_at }))),
        Err(e) => Err(err(StatusCode::UNPROCESSABLE_ENTITY, e)),
    }
}

pub async fn handle_draft_redraft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<RedraftRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, msg: String| (code, Json(serde_json::json!({ "ok": false, "error": msg })));
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err(s, "Sign in again.".to_string()))?;
    let uid = principal.user_id();

    // Fetch current draft
    let db = state.db_path.clone();
    let row: Option<(String, Option<String>, String)> = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT platform, agent_id, text FROM social_drafts WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)),
        ).ok()
    }).await.ok().flatten();
    let (platform, agent_id, prev_text) = match row {
        Some(r) => r,
        None => return Err(err(StatusCode::NOT_FOUND, "Draft not found.".to_string())),
    };

    let (new_text, pillar) = generate_draft_text_with_hint(
        &state, uid, &platform, agent_id.as_deref(), Some(&prev_text), req.hint.as_deref(),
    ).await.map_err(|e| err(StatusCode::BAD_GATEWAY, e))?;

    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let new_text_clone = new_text.clone();
    let pillar_clone = pillar.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.execute(
            "UPDATE social_drafts SET text = ?, pillar = COALESCE(?, pillar), updated_at = ? WHERE id = ? AND user_id = ?",
            rusqlite::params![new_text_clone, pillar_clone, now, id, uid],
        ).ok()
    }).await;

    Ok(Json(serde_json::json!({ "ok": true, "text": new_text, "pillar": pillar })))
}

pub async fn handle_draft_reject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<TokenOnly>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> usize {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return 0 };
        conn.execute(
            "UPDATE social_drafts SET status = 'rejected', updated_at = ? WHERE id = ? AND user_id = ? AND status = 'pending'",
            rusqlite::params![chrono::Utc::now().timestamp(), id, uid],
        ).unwrap_or(0)
    }).await.unwrap_or(0);
    if n == 0 { return Err(StatusCode::NOT_FOUND); }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Replies (Inbox) ─────────────────────────────────────────────────────────

pub async fn handle_replies_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<ReplyDraft>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let out = tokio::task::spawn_blocking(move || -> Vec<ReplyDraft> {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT id, platform, agent_id, parent_uri, parent_author, parent_text, draft_text, \
                    status, posted_uri, posted_at, created_at, updated_at \
             FROM social_replies WHERE user_id = ? AND status IN ('pending','posted') \
             ORDER BY created_at DESC LIMIT 100") { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([uid], |r| Ok(ReplyDraft {
            id: r.get(0)?, platform: r.get(1)?, agent_id: r.get(2)?, parent_uri: r.get(3)?,
            parent_author: r.get(4)?, parent_text: r.get(5)?, draft_text: r.get(6)?,
            status: r.get(7)?, posted_uri: r.get(8)?, posted_at: r.get(9)?,
            created_at: r.get(10)?, updated_at: r.get(11)?,
        })).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
    }).await.unwrap_or_default();
    Ok(Json(out))
}

pub async fn handle_reply_approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<ApproveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, msg: String| (code, Json(serde_json::json!({ "ok": false, "error": msg })));
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err(s, "Sign in again.".to_string()))?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let row: Option<(String, String, String)> = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT platform, parent_uri, COALESCE(draft_text, '') FROM social_replies \
             WHERE id = ? AND user_id = ? AND status = 'pending'",
            rusqlite::params![id, uid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        ).ok()
    }).await.ok().flatten();
    let (platform, parent_uri, current_text) = match row {
        Some(r) => r,
        None => return Err(err(StatusCode::NOT_FOUND, "Reply not found or already actioned.".to_string())),
    };

    let text = req.edited_text.filter(|s| !s.trim().is_empty()).unwrap_or(current_text);
    if text.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Reply text is empty.".to_string()));
    }

    let adapters = platforms::registry();
    let adapter = match adapters.get(platform.as_str()) {
        Some(a) => a.clone(),
        None => return Err(err(StatusCode::BAD_REQUEST, format!("No adapter for {}.", platform))),
    };
    let creds = match load_creds_for_platform(&state, uid, &platform).await {
        Some(c) => c,
        None => return Err(err(StatusCode::BAD_REQUEST, "No active connection for that platform.".to_string())),
    };

    match adapter.reply(&state.client, &creds, &parent_uri, &text).await {
        Ok(post_ref) => {
            let now = chrono::Utc::now().timestamp();
            let db = state.db_path.clone();
            let uri = post_ref.uri.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_replies SET status = 'posted', draft_text = ?, posted_uri = ?, posted_at = ?, updated_at = ? WHERE id = ?",
                    rusqlite::params![text, uri, now, now, id],
                ).ok()
            }).await;
            Ok(Json(serde_json::json!({ "ok": true, "uri": post_ref.uri })))
        }
        Err(e) => Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.user_message())),
    }
}

pub async fn handle_reply_reject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<TokenOnly>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> usize {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return 0 };
        conn.execute(
            "UPDATE social_replies SET status = 'rejected', updated_at = ? WHERE id = ? AND user_id = ? AND status = 'pending'",
            rusqlite::params![chrono::Utc::now().timestamp(), id, uid],
        ).unwrap_or(0)
    }).await.unwrap_or(0);
    if n == 0 { return Err(StatusCode::NOT_FOUND); }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Stats + alerts ──────────────────────────────────────────────────────────

pub async fn handle_stats_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let out = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT platform, as_of, followers, following, posts_count, likes_received \
             FROM social_stats_snapshots WHERE user_id = ? ORDER BY as_of DESC LIMIT 30"
        ) { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([uid], |r| Ok(serde_json::json!({
            "platform": r.get::<_, String>(0)?,
            "as_of": r.get::<_, i64>(1)?,
            "followers": r.get::<_, Option<i64>>(2)?,
            "following": r.get::<_, Option<i64>>(3)?,
            "posts_count": r.get::<_, Option<i64>>(4)?,
            "likes_received": r.get::<_, Option<i64>>(5)?,
        }))).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
    }).await.unwrap_or_default();
    Ok(Json(serde_json::json!({ "snapshots": out })))
}

pub async fn handle_alerts_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let out = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT id, platform, alert_type, target_uri, detail, created_at \
             FROM social_alerts WHERE user_id = ? AND acknowledged = 0 ORDER BY created_at DESC LIMIT 50"
        ) { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([uid], |r| Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?,
            "platform": r.get::<_, String>(1)?,
            "alert_type": r.get::<_, String>(2)?,
            "target_uri": r.get::<_, Option<String>>(3)?,
            "detail": r.get::<_, String>(4)?,
            "created_at": r.get::<_, i64>(5)?,
        }))).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
    }).await.unwrap_or_default();
    Ok(Json(serde_json::json!({ "alerts": out })))
}

// ── Internals: pref reader ──────────────────────────────────────────────────

async fn read_pref(state: &AppState, user_id: i64, key: &str) -> Option<String> {
    let db = state.db_path.clone();
    let k = key.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT value FROM user_preferences WHERE user_id = ? AND key = ?",
            rusqlite::params![user_id, k], |r| r.get::<_, Option<String>>(0),
        ).ok().flatten().filter(|s| !s.trim().is_empty())
    }).await.ok().flatten()
}

// ── Internals: draft generator ──────────────────────────────────────────────

fn pick_next_pillar(
    conn: &rusqlite::Connection,
    user_id: i64,
    platform: &str,
    agent_id: &str,
    pillars: &[String],
) -> Option<String> {
    if pillars.is_empty() { return None; }
    let cur_idx: Option<i64> = conn.query_row(
        "SELECT last_idx FROM social_pillar_cursor WHERE user_id = ? AND platform = ? AND agent_id = ?",
        rusqlite::params![user_id, platform, agent_id], |r| r.get(0),
    ).ok();
    let next = ((cur_idx.unwrap_or(-1) + 1) as usize) % pillars.len();
    let now = chrono::Utc::now().timestamp();
    let _ = conn.execute(
        "INSERT INTO social_pillar_cursor (user_id, platform, agent_id, last_idx, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(user_id, platform, agent_id) DO UPDATE SET last_idx = excluded.last_idx, updated_at = excluded.updated_at",
        rusqlite::params![user_id, platform, agent_id, next as i64, now],
    );
    pillars.get(next).cloned()
}

async fn generate_draft_text(
    state: &AppState,
    user_id: i64,
    platform: &str,
    agent_id: Option<&str>,
) -> Result<(String, Option<String>), String> {
    generate_draft_text_with_hint(state, user_id, platform, agent_id, None, None).await
}

async fn generate_draft_text_with_hint(
    state: &AppState,
    user_id: i64,
    platform: &str,
    agent_id: Option<&str>,
    previous_draft: Option<&str>,
    user_hint: Option<&str>,
) -> Result<(String, Option<String>), String> {
    // Pull prefs we need
    let brand     = read_pref(state, user_id, "social.brand_voice").await.unwrap_or_default();
    let audience  = read_pref(state, user_id, "social.audience").await.unwrap_or_default();
    let blocklist = read_pref(state, user_id, "social.blocklist.words").await.unwrap_or_default();
    let humor     = read_pref(state, user_id, "social.tone.humor").await.unwrap_or_else(|| "4".to_string());
    let formality = read_pref(state, user_id, "social.tone.formality").await.unwrap_or_else(|| "4".to_string());

    // Per-platform overrides
    let platform_voice = read_pref(state, user_id, &format!("social.platform.{}.voice_override", platform)).await;
    let platform_signature = read_pref(state, user_id, &format!("social.platform.{}.signature", platform)).await.unwrap_or_default();
    let pillars_json = read_pref(state, user_id, &format!("social.platform.{}.pillars", platform)).await.unwrap_or_else(|| "[]".to_string());
    let pillars: Vec<String> = serde_json::from_str(&pillars_json).unwrap_or_default();

    // Pillar rotation (via blocking DB call, short)
    let db = state.db_path.clone();
    let platform_q = platform.to_string();
    let agent_q = agent_id.unwrap_or("").to_string();
    let pillars_q = pillars.clone();
    let pillar = tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        pick_next_pillar(&conn, user_id, &platform_q, &agent_q, &pillars_q)
    }).await.ok().flatten();

    let voice_effective = platform_voice.filter(|s| !s.trim().is_empty()).unwrap_or(brand.clone());

    let mut sys = String::new();
    sys.push_str("You are Nyota, a calm editor writing on behalf of the user. Draft ONE social-media post ready to ship. Output ONLY the post text — no preamble, no commentary, no hashtags unless the user's voice uses them.\n\n");
    sys.push_str(&format!("Platform: {}.\n", platform));
    if platform == "bluesky" { sys.push_str("Character limit: 300. Keep it under. Be specific, not salesy.\n"); }
    if platform == "youtube" { sys.push_str("This is a YouTube community post. 1-2 short paragraphs, inviting, not promotional.\n"); }
    if platform == "threads"  { sys.push_str("Threads-native vibe. Conversational. No Twitter-isms.\n"); }
    if !voice_effective.trim().is_empty() {
        sys.push_str(&format!("\nBrand voice (follow closely): {}\n", voice_effective.trim()));
    }
    if !audience.trim().is_empty() {
        sys.push_str(&format!("Audience: {}\n", audience.trim()));
    }
    sys.push_str(&format!("Tone dials — humor {}/10, formality {}/10.\n", humor, formality));
    if !blocklist.trim().is_empty() {
        sys.push_str(&format!("Avoid these words/phrases: {}.\n", blocklist.trim()));
    }
    if let Some(p) = &pillar {
        sys.push_str(&format!("\nContent pillar for today: {}\n", p.trim()));
    }
    if !platform_signature.trim().is_empty() {
        sys.push_str(&format!("End with this signature (if it fits naturally): {}\n", platform_signature.trim()));
    }

    let mut user_msg = String::new();
    if let Some(prev) = previous_draft {
        user_msg.push_str("Previous draft that didn't land:\n");
        user_msg.push_str(prev);
        user_msg.push_str("\n\nWrite a fresh one. Different angle, same voice.\n");
    }
    if let Some(hint) = user_hint {
        if !hint.trim().is_empty() {
            user_msg.push_str("User's redraft hint: ");
            user_msg.push_str(hint);
            user_msg.push('\n');
        }
    }
    if user_msg.is_empty() {
        user_msg = "Draft today's post.".to_string();
    }

    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let messages = vec![
        crate::llm::ChatMessage::system(&sys),
        crate::llm::ChatMessage::user(&user_msg),
    ];
    let reply = match tokio::time::timeout(std::time::Duration::from_secs(45), chain.call(&messages)).await {
        Ok(Ok(text)) => text.trim().to_string(),
        Ok(Err(e)) => return Err(format!("LLM error: {}", e)),
        Err(_) => return Err("LLM timed out.".to_string()),
    };
    if reply.is_empty() { return Err("LLM returned empty output.".to_string()); }
    Ok((reply, pillar))
}

// ── Internals: publisher ────────────────────────────────────────────────────

async fn load_creds_for_platform(state: &AppState, user_id: i64, platform: &str) -> Option<serde_json::Value> {
    let db = state.db_path.clone();
    let p = platform.to_string();
    tokio::task::spawn_blocking(move || -> Option<serde_json::Value> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        let json: String = conn.query_row(
            "SELECT credentials_json FROM social_connections WHERE user_id = ? AND platform = ? \
             ORDER BY COALESCE(last_verified_at, updated_at) DESC LIMIT 1",
            rusqlite::params![user_id, p], |r| r.get(0),
        ).ok()?;
        serde_json::from_str(&json).ok()
    }).await.ok().flatten()
}

async fn publish_draft_now(
    state: &AppState,
    user_id: i64,
    draft_id: i64,
) -> Result<platforms::PostRef, String> {
    // Load the draft
    let db = state.db_path.clone();
    let row = tokio::task::spawn_blocking(move || -> Option<(String, String, String)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT platform, text, status FROM social_drafts WHERE id = ? AND user_id = ?",
            rusqlite::params![draft_id, user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        ).ok()
    }).await.ok().flatten();
    let (platform, text, status) = row.ok_or_else(|| "Draft not found.".to_string())?;
    if status != "pending" { return Err(format!("Draft is already {}.", status)); }

    let adapters = platforms::registry();
    let adapter = adapters.get(platform.as_str())
        .ok_or_else(|| format!("No adapter for {}.", platform))?
        .clone();
    let creds = load_creds_for_platform(state, user_id, &platform).await
        .ok_or_else(|| "No active connection for that platform.".to_string())?;

    // Refresh creds first for OAuth platforms (access tokens tend to expire).
    // If refresh fails with a non-fatal reason, fall through to post anyway.
    if let Ok(refreshed) = adapter.refresh(&state.client, &creds).await {
        let merged = merge_json(&creds, &refreshed.credentials);
        persist_refreshed_creds(state, user_id, &platform, &merged, refreshed.expires_at).await;
    }
    let creds = load_creds_for_platform(state, user_id, &platform).await.unwrap_or(creds);

    match adapter.post(&state.client, &creds, &text).await {
        Ok(post_ref) => {
            let now = chrono::Utc::now().timestamp();
            let db = state.db_path.clone();
            let uri = post_ref.uri.clone();
            let posted_at = post_ref.posted_at;
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_drafts SET status = 'posted', posted_uri = ?, posted_at = ?, error_detail = NULL, updated_at = ? WHERE id = ?",
                    rusqlite::params![uri, posted_at, now, draft_id],
                ).ok()
            }).await;
            telegram_update_draft(state, user_id, draft_id, "posted", Some(&post_ref.uri)).await;
            Ok(post_ref)
        }
        Err(e) => {
            let msg = e.user_message();
            let now = chrono::Utc::now().timestamp();
            let db = state.db_path.clone();
            let msg_clone = msg.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_drafts SET status = 'failed', error_detail = ?, updated_at = ? WHERE id = ?",
                    rusqlite::params![msg_clone, now, draft_id],
                ).ok()
            }).await;
            telegram_update_draft(state, user_id, draft_id, "failed", Some(&msg)).await;
            Err(msg)
        }
    }
}

fn merge_json(base: &serde_json::Value, overlay: &serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            let mut out = a.clone();
            for (k, v) in b { out.insert(k.clone(), v.clone()); }
            serde_json::Value::Object(out)
        }
        _ => overlay.clone(),
    }
}

async fn persist_refreshed_creds(state: &AppState, user_id: i64, platform: &str, creds: &serde_json::Value, expires_at: Option<i64>) {
    let db = state.db_path.clone();
    let p = platform.to_string();
    let creds_str = creds.to_string();
    let now = chrono::Utc::now().timestamp();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.execute(
            "UPDATE social_connections SET credentials_json = ?, expires_at = ?, last_verified_at = ?, updated_at = ? WHERE user_id = ? AND platform = ?",
            rusqlite::params![creds_str, expires_at, now, now, user_id, p],
        ).ok()
    }).await;
}

// ── Telegram mirror ─────────────────────────────────────────────────────────

/// Send a draft-approval card to Telegram. Uses the outbound-notification
/// bot token (`channels.telegram.botToken` in config) which is what the
/// rest of Syntaur's notification system uses. Callback verbs:
///   social-draft:approve:<id>
///   social-draft:redraft:<id>
///   social-draft:reject:<id>
/// The telegram poller handles these in telegram.rs::callback dispatch.
async fn telegram_send_draft(state: &AppState, user_id: i64, draft_id: i64) {
    // Respect user notification pref
    if read_pref(state, user_id, "social.notify.telegram").await.as_deref() == Some("false") {
        return;
    }
    let bot_token = state.config.channels.telegram.bot_token.clone();
    let chat_id = state.config.channels.telegram.extra.get("chatId")
        .and_then(|v| v.as_i64())
        .or_else(|| state.config.channels.telegram.accounts.values()
            .next().and_then(|a| a.extra.get("chatId")).and_then(|v| v.as_i64()))
        .unwrap_or(0);
    if bot_token.is_empty() || chat_id == 0 { return; }

    // Load the draft so we can show its text
    let db = state.db_path.clone();
    let row = tokio::task::spawn_blocking(move || -> Option<(String, String, Option<String>)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT platform, text, pillar FROM social_drafts WHERE id = ? AND user_id = ?",
            rusqlite::params![draft_id, user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        ).ok()
    }).await.ok().flatten();
    let (platform, text, pillar) = match row { Some(r) => r, None => return };

    let pillar_line = pillar.as_ref().map(|p| format!("\n<i>pillar: {}</i>", html_escape(p))).unwrap_or_default();
    let msg_text = format!(
        "<b>Nyota drafted a post for {}</b>{}\n\n{}",
        html_escape(&platform), pillar_line, html_escape(&text)
    );
    let reply_markup = serde_json::json!({
        "inline_keyboard": [
            [
                { "text": "✓ Approve & post", "callback_data": format!("social-draft:approve:{}", draft_id) },
                { "text": "↻ Redraft",        "callback_data": format!("social-draft:redraft:{}", draft_id) },
                { "text": "✕ Reject",         "callback_data": format!("social-draft:reject:{}",  draft_id) },
            ]
        ]
    });
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let resp = state.client.post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": msg_text,
            "parse_mode": "HTML",
            "reply_markup": reply_markup,
        }))
        .timeout(std::time::Duration::from_secs(8))
        .send().await;
    if let Ok(r) = resp {
        if let Ok(body) = r.json::<serde_json::Value>().await {
            if let Some(mid) = body.get("result").and_then(|v| v.get("message_id")).and_then(|v| v.as_i64()) {
                let db = state.db_path.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let conn = rusqlite::Connection::open(&db).ok()?;
                    conn.execute(
                        "UPDATE social_drafts SET telegram_message_id = ?, telegram_chat_id = ?, updated_at = ? WHERE id = ?",
                        rusqlite::params![mid, chat_id, chrono::Utc::now().timestamp(), draft_id],
                    ).ok()
                }).await;
            }
        }
    }
}

/// Edit the Telegram message for a draft when its status changes (posted / failed).
async fn telegram_update_draft(state: &AppState, user_id: i64, draft_id: i64, new_status: &str, note: Option<&str>) {
    let bot_token = state.config.channels.telegram.bot_token.clone();
    if bot_token.is_empty() { return; }

    let db = state.db_path.clone();
    let row = tokio::task::spawn_blocking(move || -> Option<(Option<i64>, Option<i64>, String, String)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT telegram_message_id, telegram_chat_id, platform, text FROM social_drafts WHERE id = ? AND user_id = ?",
            rusqlite::params![draft_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        ).ok()
    }).await.ok().flatten();
    let (mid, chat_id, platform, text) = match row {
        Some((Some(m), Some(c), p, t)) => (m, c, p, t),
        _ => return,
    };

    let prefix = match new_status {
        "posted" => "✓ Posted",
        "failed" => "✕ Failed",
        "rejected" => "✕ Rejected",
        _ => new_status,
    };
    let note_line = note.map(|n| format!("\n<i>{}</i>", html_escape(n))).unwrap_or_default();
    let new_text = format!(
        "<b>{} on {}</b>{}\n\n{}",
        prefix, html_escape(&platform), note_line, html_escape(&text)
    );
    let url = format!("https://api.telegram.org/bot{}/editMessageText", bot_token);
    let _ = state.client.post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id, "message_id": mid, "text": new_text, "parse_mode": "HTML",
        }))
        .timeout(std::time::Duration::from_secs(8))
        .send().await;
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Called from telegram.rs when a callback with prefix "social-draft:" lands.
/// Parses verb + id and dispatches. Non-async-aware callers can spawn this.
pub async fn telegram_callback_dispatch(state: Arc<AppState>, user_id: i64, data: &str) -> Result<String, String> {
    // shape: social-draft:<verb>:<id>
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 || parts[0] != "social-draft" {
        return Err("not a social-draft callback".to_string());
    }
    let verb = parts[1];
    let id: i64 = parts[2].parse().map_err(|_| "bad id".to_string())?;
    match verb {
        "approve" => match publish_draft_now(&state, user_id, id).await {
            Ok(r) => Ok(format!("Posted: {}", r.uri)),
            Err(e) => Err(e),
        },
        "redraft" => {
            let db = state.db_path.clone();
            let row = tokio::task::spawn_blocking(move || -> Option<(String, Option<String>, String)> {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.query_row(
                    "SELECT platform, agent_id, text FROM social_drafts WHERE id = ? AND user_id = ?",
                    rusqlite::params![id, user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                ).ok()
            }).await.ok().flatten();
            let (platform, agent_id, prev) = row.ok_or_else(|| "Draft not found".to_string())?;
            let (new_text, pillar) = generate_draft_text_with_hint(&state, user_id, &platform, agent_id.as_deref(), Some(&prev), None).await?;
            let db = state.db_path.clone();
            let new_text_clone = new_text.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_drafts SET text = ?, pillar = COALESCE(?, pillar), updated_at = ? WHERE id = ?",
                    rusqlite::params![new_text_clone, pillar, chrono::Utc::now().timestamp(), id]
                ).ok()
            }).await;
            telegram_update_draft(&state, user_id, id, "pending", Some("Redrafted — new version above.")).await;
            // Re-send as a fresh message
            telegram_send_draft(&state, user_id, id).await;
            Ok(new_text)
        },
        "reject" => {
            let db = state.db_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "UPDATE social_drafts SET status = 'rejected', updated_at = ? WHERE id = ? AND user_id = ?",
                    rusqlite::params![chrono::Utc::now().timestamp(), id, user_id]
                ).ok()
            }).await;
            telegram_update_draft(&state, user_id, id, "rejected", None).await;
            Ok("Rejected".to_string())
        },
        _ => Err("unknown verb".to_string()),
    }
}

// ── Cron-facing tick functions ──────────────────────────────────────────────

/// Spawn the full set of Syntaur-native social-module background tasks.
/// Replaces the rust-social-manager CL cron jobs:
///   - draft tick           (every 60s)  → daily per-platform drafts on cadence
///   - credential refresh    (every 5min) → YouTube + others; replaces broken yt-token cron
///   - notify poll           (every 15min) → Bluesky notifications → Inbox
///   - engagement tick       (every 4h)   → hashtag likes/follows/unfollows
///   - stats snapshot        (every 24h)  → per-platform profile snapshot
///   - post monitor          (every 4h)   → blocklist scanner over recent posts
///
/// Each tick is its own tokio::spawn so failures stay isolated. All
/// respect per-user pause prefs.
pub fn spawn_background_tasks(state: Arc<AppState>) {
    use std::time::Duration;
    // draft tick — cadence-aligned
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] draft tick started (60s)");
            let mut i = tokio::time::interval(Duration::from_secs(60));
            i.tick().await;
            loop { i.tick().await; run_draft_tick(&state).await; }
        });
    }
    // Credential refresh — catches access tokens nearing expiry
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] credential refresh started (5min)");
            let mut i = tokio::time::interval(Duration::from_secs(300));
            i.tick().await;
            loop { i.tick().await; run_credential_refresh(&state).await; }
        });
    }
    // Notifications poll — builds the Inbox
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] notify poll started (15min)");
            let mut i = tokio::time::interval(Duration::from_secs(900));
            i.tick().await;
            loop { i.tick().await; run_notify_poll(&state).await; }
        });
    }
    // Engagement — likes/follows/unfollows
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] engagement tick started (4h)");
            let mut i = tokio::time::interval(Duration::from_secs(4 * 3600));
            i.tick().await;
            loop { i.tick().await; run_engagement_tick(&state).await; }
        });
    }
    // Stats snapshot — daily
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] stats snapshot started (24h)");
            let mut i = tokio::time::interval(Duration::from_secs(24 * 3600));
            i.tick().await;
            loop { i.tick().await; run_stats_snapshot(&state).await; }
        });
    }
    // Post monitor — scans recent posts for blocklist hits
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            log::info!("[social] post monitor started (4h)");
            let mut i = tokio::time::interval(Duration::from_secs(4 * 3600));
            i.tick().await;
            loop { i.tick().await; run_post_monitor(&state).await; }
        });
    }
}

/// Invoked once per minute by Syntaur's cron runner. Scans user prefs
/// for per-platform cadence matching "now" and inserts a draft.
///
/// Matching rule: schedule matches if today's weekday abbrev is in
/// social.platform.<id>.cadence.days AND the current local HH:MM equals
/// social.platform.<id>.cadence.time. We also allow a fallback path for
/// users who set social.schedule.enabled=true but no per-platform days
/// — in that case we use their primary connected platform and the
/// global social.schedule.time.
pub async fn run_draft_tick(state: &AppState) {
    let users: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT id FROM users WHERE disabled = 0") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([], |r| r.get::<_, i64>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };

    use chrono::Datelike;
    let now = chrono::Local::now();
    let hhmm = now.format("%H:%M").to_string();
    let weekday = match now.date_naive().weekday() {
        chrono::Weekday::Mon => "mon", chrono::Weekday::Tue => "tue", chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu", chrono::Weekday::Fri => "fri", chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    };

    for uid in users {
        // Skip if posting is paused globally.
        if read_pref(state, uid, "social.pause.posting").await.as_deref() == Some("true") { continue; }

        // Find connected platforms this user has.
        let db = state.db_path.clone();
        let platforms: Vec<String> = tokio::task::spawn_blocking(move || -> Vec<String> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare(
                "SELECT DISTINCT platform FROM social_connections WHERE user_id = ? AND status IN ('connected','degraded')"
            ) { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([uid], |r| r.get::<_, String>(0))
                .map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default();

        for platform in platforms {
            // Skip if platform-level paused.
            if read_pref(state, uid, &format!("social.platform.{}.pause", platform)).await.as_deref() == Some("true") { continue; }

            let days_pref = read_pref(state, uid, &format!("social.platform.{}.cadence.days", platform)).await.unwrap_or_default();
            let time_pref = read_pref(state, uid, &format!("social.platform.{}.cadence.time", platform)).await.unwrap_or_else(|| "09:00".to_string());
            if days_pref.is_empty() { continue; }
            if !days_pref.split(',').any(|d| d.trim() == weekday) { continue; }
            if time_pref.trim() != hhmm { continue; }

            // Cadence matches. Generate draft.
            match generate_draft_text(state, uid, &platform, None).await {
                Ok((text, pillar)) => {
                    let db = state.db_path.clone();
                    let platform_clone = platform.clone();
                    let now_ts = chrono::Utc::now().timestamp();
                    let text_clone = text.clone();
                    let pillar_clone = pillar.clone();
                    let id = tokio::task::spawn_blocking(move || -> Option<i64> {
                        let conn = rusqlite::Connection::open(&db).ok()?;
                        let conn_id: Option<i64> = conn.query_row(
                            "SELECT id FROM social_connections WHERE user_id = ? AND platform = ? ORDER BY COALESCE(last_verified_at, updated_at) DESC LIMIT 1",
                            rusqlite::params![uid, &platform_clone], |r| r.get(0),
                        ).ok();
                        conn.execute(
                            "INSERT INTO social_drafts (user_id, platform, connection_id, text, pillar, source, status, created_at, updated_at) \
                             VALUES (?, ?, ?, ?, ?, 'auto', 'pending', ?, ?)",
                            rusqlite::params![uid, &platform_clone, conn_id, text_clone, pillar_clone, now_ts, now_ts],
                        ).ok()?;
                        Some(conn.last_insert_rowid())
                    }).await.ok().flatten();

                    if let Some(did) = id {
                        log::info!("[social] generated draft for user={} platform={} id={}", uid, platform, did);
                        telegram_send_draft(state, uid, did).await;

                        // Approval-mode enforcement on auto-generated drafts
                        let mode = read_pref(state, uid, "social.approval_mode").await.unwrap_or_else(|| "always_review".to_string());
                        let plat_override = read_pref(state, uid, &format!("social.platform.{}.approval_override", platform)).await.unwrap_or_else(|| "inherit".to_string());
                        let effective = if plat_override != "inherit" { plat_override } else { mode };
                        if effective == "auto_post_all" || effective == "auto_post_routine" {
                            let _ = publish_draft_now(state, uid, did).await;
                        }
                    }
                }
                Err(e) => log::warn!("[social] draft-tick gen failed user={} platform={}: {}", uid, platform, e),
            }
        }
    }
}

/// Scan each connected platform for new notifications, draft replies,
/// insert into social_replies for user review.
pub async fn run_notify_poll(state: &AppState) {
    let users: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT id FROM users WHERE disabled = 0") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([], |r| r.get::<_, i64>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };

    let adapters = platforms::registry();
    for uid in users {
        if read_pref(state, uid, "social.pause.posting").await.as_deref() == Some("true") { continue; }
        let db = state.db_path.clone();
        let conns: Vec<String> = tokio::task::spawn_blocking(move || -> Vec<String> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT DISTINCT platform FROM social_connections WHERE user_id = ? AND status IN ('connected','degraded')") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([uid], |r| r.get::<_, String>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default();
        for platform in conns {
            let adapter = match adapters.get(platform.as_str()) { Some(a) => a.clone(), None => continue };
            let creds = match load_creds_for_platform(state, uid, &platform).await { Some(c) => c, None => continue };
            // Since = last notification we stored for this user+platform, to avoid re-drafting.
            let db = state.db_path.clone();
            let platform_q = platform.clone();
            let since: Option<i64> = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.query_row(
                    "SELECT MAX(created_at) FROM social_replies WHERE user_id = ? AND platform = ?",
                    rusqlite::params![uid, platform_q], |r| r.get::<_, Option<i64>>(0),
                ).ok().flatten()
            }).await.ok().flatten();

            let notifs = match adapter.notifications(&state.client, &creds, since).await {
                Ok(n) => n, Err(e) => { log::warn!("[social] notify poll user={} {}: {:?}", uid, platform, e); continue; }
            };
            for n in notifs {
                // Draft a reply via LLM using user prefs.
                let draft = draft_reply_text(state, uid, &platform, &n.parent_text).await.ok();
                let db = state.db_path.clone();
                let platform_c = platform.clone();
                let parent_uri = n.parent_uri.clone();
                let parent_author = n.parent_author.clone();
                let parent_text = n.parent_text.clone();
                let draft_clone = draft.clone();
                let now = chrono::Utc::now().timestamp();
                let _ = tokio::task::spawn_blocking(move || {
                    let conn = rusqlite::Connection::open(&db).ok()?;
                    conn.execute(
                        "INSERT OR IGNORE INTO social_replies (user_id, platform, parent_uri, parent_author, parent_text, draft_text, status, created_at, updated_at) \
                         VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
                        rusqlite::params![uid, platform_c, parent_uri, parent_author, parent_text, draft_clone, now, now],
                    ).ok()
                }).await;
            }
        }
    }
}

async fn draft_reply_text(state: &AppState, user_id: i64, platform: &str, parent_text: &str) -> Result<String, String> {
    let brand = read_pref(state, user_id, "social.brand_voice").await.unwrap_or_default();
    let sys = format!(
        "You are Nyota, drafting a short reply on behalf of the user. Keep it warm, specific, and under 280 chars. Match the user's voice: {}. Output only the reply text.",
        brand.trim()
    );
    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let _ = platform; // silence for now — later adapters may tailor
    let messages = vec![
        crate::llm::ChatMessage::system(&sys),
        crate::llm::ChatMessage::user(&format!("Parent post:\n{}\n\nDraft the reply.", parent_text)),
    ];
    match tokio::time::timeout(std::time::Duration::from_secs(20), chain.call(&messages)).await {
        Ok(Ok(t)) => Ok(t.trim().to_string()),
        Ok(Err(e)) => Err(format!("LLM error: {}", e)),
        Err(_) => Err("LLM timeout".to_string()),
    }
}

/// Per-user engagement tick: likes + follows + unfollows per preset.
pub async fn run_engagement_tick(state: &AppState) {
    let users: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT id FROM users WHERE disabled = 0") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([], |r| r.get::<_, i64>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };
    let adapters = platforms::registry();
    for uid in users {
        if read_pref(state, uid, "social.pause.engagement").await.as_deref() == Some("true") { continue; }
        let preset = read_pref(state, uid, "social.engage.preset").await.unwrap_or_else(|| "off".to_string());
        if preset == "off" { continue; }
        let likes_per_day  = read_pref(state, uid, "social.engage.likes_per_day").await.and_then(|s| s.parse::<u32>().ok()).unwrap_or(20);
        let follows_per_day= read_pref(state, uid, "social.engage.follows_per_day").await.and_then(|s| s.parse::<u32>().ok()).unwrap_or(15);
        let unfollow_days  = read_pref(state, uid, "social.engage.unfollow_after_days").await.and_then(|s| s.parse::<i64>().ok()).unwrap_or(7);

        // For v1 we focus on bluesky; other platforms fall through
        let platform = "bluesky";
        let adapter = match adapters.get(platform) { Some(a) => a.clone(), None => continue };
        let creds = match load_creds_for_platform(state, uid, platform).await { Some(c) => c, None => continue };

        // Pull per-platform hashtags; fall back to a preset default
        let tags = read_pref(state, uid, &format!("social.platform.{}.engage.hashtags", platform)).await.unwrap_or_else(|| {
            match preset.as_str() {
                "artist" => "indiefolk,songwriter,indiemusic,acoustic".to_string(),
                "podcaster" => "podcast,podcastlife,podcasting".to_string(),
                "small_business" => "smallbusiness,supportlocal".to_string(),
                _ => "".to_string(),
            }
        });
        if tags.trim().is_empty() { continue; }

        let mut liked = 0u32;
        let mut followed = 0u32;
        for tag in tags.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()) {
            if liked >= likes_per_day && followed >= follows_per_day { break; }
            let hits = match adapter.search_hashtag(&state.client, &creds, tag, 20).await {
                Ok(h) => h, Err(_) => continue,
            };
            for (post_uri, author_did) in hits {
                if liked < likes_per_day {
                    if adapter.like(&state.client, &creds, &post_uri).await.is_ok() {
                        record_engagement(state, uid, platform, "like", &post_uri, None).await;
                        liked += 1;
                    }
                }
                if followed < follows_per_day {
                    if let Ok(uri) = adapter.follow(&state.client, &creds, &author_did).await {
                        record_engagement(state, uid, platform, "follow", &uri, Some(&author_did)).await;
                        followed += 1;
                    }
                }
                if liked >= likes_per_day && followed >= follows_per_day { break; }
            }
        }

        // Unfollow stale follows
        if unfollow_days > 0 {
            let cutoff = chrono::Utc::now().timestamp() - unfollow_days * 86400;
            let db = state.db_path.clone();
            let rows: Vec<String> = tokio::task::spawn_blocking(move || -> Vec<String> {
                let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
                let mut stmt = match conn.prepare(
                    "SELECT target_uri FROM social_engagement_log WHERE user_id = ? AND platform = ? AND action = 'follow' AND created_at < ? \
                     AND NOT EXISTS (SELECT 1 FROM social_engagement_log e2 WHERE e2.user_id = social_engagement_log.user_id AND e2.platform = social_engagement_log.platform AND e2.target_uri = social_engagement_log.target_uri AND e2.action = 'unfollow') LIMIT 50"
                ) { Ok(s) => s, Err(_) => return vec![] };
                stmt.query_map(rusqlite::params![uid, platform, cutoff], |r| r.get::<_, String>(0))
                    .map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
            }).await.unwrap_or_default();
            for follow_uri in rows {
                if adapter.unfollow(&state.client, &creds, &follow_uri).await.is_ok() {
                    record_engagement(state, uid, platform, "unfollow", &follow_uri, None).await;
                }
            }
        }

        log::info!("[social] engagement user={} platform={} liked={} followed={}", uid, platform, liked, followed);
    }
}

async fn record_engagement(state: &AppState, user_id: i64, platform: &str, action: &str, target_uri: &str, info: Option<&str>) {
    let db = state.db_path.clone();
    let p = platform.to_string();
    let a = action.to_string();
    let t = target_uri.to_string();
    let i = info.map(|s| s.to_string());
    let now = chrono::Utc::now().timestamp();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.execute(
            "INSERT INTO social_engagement_log (user_id, platform, action, target_uri, target_info, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![user_id, p, a, t, i, now],
        ).ok()
    }).await;
}

/// Pull current stats per platform and store a snapshot.
pub async fn run_stats_snapshot(state: &AppState) {
    let users: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT id FROM users WHERE disabled = 0") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([], |r| r.get::<_, i64>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };
    let adapters = platforms::registry();
    for uid in users {
        let db = state.db_path.clone();
        let conns: Vec<String> = tokio::task::spawn_blocking(move || -> Vec<String> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT DISTINCT platform FROM social_connections WHERE user_id = ? AND status IN ('connected','degraded')") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([uid], |r| r.get::<_, String>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default();
        for platform in conns {
            let adapter = match adapters.get(platform.as_str()) { Some(a) => a.clone(), None => continue };
            let creds = match load_creds_for_platform(state, uid, &platform).await { Some(c) => c, None => continue };
            let stats = match adapter.stats(&state.client, &creds).await {
                Ok(s) => s, Err(_) => continue,
            };
            let now = chrono::Utc::now().timestamp();
            let db = state.db_path.clone();
            let p = platform.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db).ok()?;
                conn.execute(
                    "INSERT INTO social_stats_snapshots (user_id, platform, as_of, followers, following, posts_count, likes_received, reposts_received, replies_received) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![uid, p, now, stats.followers, stats.following, stats.posts_count, stats.likes_received, stats.reposts_received, stats.replies_received],
                ).ok()
            }).await;
        }
    }
}

/// Refresh OAuth credentials for platforms whose access token is
/// nearing expiry (next 10 minutes) so posting never hits a stale token.
/// Replaces the external rust-social-manager youtube-token-refresh cron
/// which had 14 consecutive errors.
pub async fn run_credential_refresh(state: &AppState) {
    let adapters = platforms::registry();
    let db = state.db_path.clone();
    let targets: Vec<(i64, String, i64)> = tokio::task::spawn_blocking(move || -> Vec<(i64, String, i64)> {
        let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
        let now = chrono::Utc::now().timestamp();
        let soon = now + 600;
        let mut stmt = match conn.prepare(
            "SELECT user_id, platform, id FROM social_connections \
             WHERE status IN ('connected','degraded') AND expires_at IS NOT NULL AND expires_at < ?"
        ) { Ok(s) => s, Err(_) => return vec![] };
        stmt.query_map([soon], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
    }).await.unwrap_or_default();

    for (uid, platform, _id) in targets {
        let adapter = match adapters.get(platform.as_str()) { Some(a) => a.clone(), None => continue };
        let creds = match load_creds_for_platform(state, uid, &platform).await { Some(c) => c, None => continue };
        match adapter.refresh(&state.client, &creds).await {
            Ok(refreshed) => {
                let merged = merge_json(&creds, &refreshed.credentials);
                persist_refreshed_creds(state, uid, &platform, &merged, refreshed.expires_at).await;
                log::info!("[social] refreshed {} creds for user={}", platform, uid);
            }
            Err(e) => log::warn!("[social] refresh failed user={} platform={}: {:?}", uid, platform, e),
        }
    }
}

/// Scan recent posts; flag any that contain blocklist terms as an alert.
pub async fn run_post_monitor(state: &AppState) {
    let users: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT id FROM users WHERE disabled = 0") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([], |r| r.get::<_, i64>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };
    let adapters = platforms::registry();
    for uid in users {
        let blocklist = read_pref(state, uid, "social.blocklist.words").await.unwrap_or_default();
        let terms: Vec<String> = blocklist.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        if terms.is_empty() { continue; }

        let db = state.db_path.clone();
        let conns: Vec<String> = tokio::task::spawn_blocking(move || -> Vec<String> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare("SELECT DISTINCT platform FROM social_connections WHERE user_id = ? AND status IN ('connected','degraded')") { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map([uid], |r| r.get::<_, String>(0)).map(|i| i.filter_map(Result::ok).collect()).unwrap_or_default()
        }).await.unwrap_or_default();
        for platform in conns {
            let adapter = match adapters.get(platform.as_str()) { Some(a) => a.clone(), None => continue };
            let creds = match load_creds_for_platform(state, uid, &platform).await { Some(c) => c, None => continue };
            let recents = match adapter.recent_posts(&state.client, &creds, 20).await { Ok(p) => p, Err(_) => continue };
            for post in recents {
                let lower = post.text.to_lowercase();
                let hit = terms.iter().find(|t| lower.contains(t.as_str()));
                if let Some(term) = hit {
                    let detail = format!("Post contains blocklisted term '{}': {}", term, post.text.chars().take(120).collect::<String>());
                    let db = state.db_path.clone();
                    let p = platform.clone();
                    let uri = post.uri.clone();
                    let d = detail.clone();
                    let now = chrono::Utc::now().timestamp();
                    let _ = tokio::task::spawn_blocking(move || {
                        let conn = rusqlite::Connection::open(&db).ok()?;
                        // Dedup per (user, target_uri)
                        let exists: Option<i64> = conn.query_row(
                            "SELECT id FROM social_alerts WHERE user_id = ? AND target_uri = ? LIMIT 1",
                            rusqlite::params![uid, uri], |r| r.get(0),
                        ).ok();
                        if exists.is_none() {
                            let _ = conn.execute(
                                "INSERT INTO social_alerts (user_id, platform, alert_type, target_uri, detail, created_at) VALUES (?, ?, 'blocklist_hit', ?, ?, ?)",
                                rusqlite::params![uid, p, uri, d, now],
                            );
                        }
                        Some(())
                    }).await;
                }
            }
        }
    }
}
