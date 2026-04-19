//! Social module backend.
//!
//! Phase 1 scope: connection CRUD for the `/social` → Connections pane.
//! Platform adapters (posting, replying, engagement) land in subsequent
//! phases once each platform's auth flow is wired.
//!
//! Storage: `social_connections` table (schema v44). Per-user, scoped
//! via `resolve_principal` + `user_id` on every query.
//!
//! Credentials are stored as plaintext JSON for v1, matching the rest of
//! the SQLite storage posture. Encryption-at-rest is a cross-module
//! improvement (see `projects/syntaur_security_remediation` in the vault).

use axum::{extract::{Path, State}, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

pub mod platforms;
pub mod engine;

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SocialConnection {
    pub id: i64,
    pub platform: String,
    pub display_name: Option<String>,
    pub status: String,
    pub status_detail: Option<String>,
    pub agent_id: Option<String>,
    pub connected_at: i64,
    pub last_verified_at: Option<i64>,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct CreateConnectionRequest {
    pub token: String,
    pub platform: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub credentials: serde_json::Value,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_detail: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct DeleteConnectionRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct ReconnectRequest {
    pub token: String,
    pub fields: serde_json::Value,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Deserialize)]
pub struct NyotaAssistRequest {
    pub token: String,
    /// What Nyota should draft. One of:
    ///   "draft_voice"     — caller passes sample_posts, gets a brand-voice paragraph
    ///   "draft_pillars"   — caller passes a few topics/interests, gets 3-5 pillar lines
    ///   "draft_audience"  — caller passes a rough sketch, gets a tighter audience description
    ///   "draft_blocklist" — caller passes a short description of what to avoid, gets a word list
    pub intent: String,
    /// Platform-specific hint (for draft_pillars or draft_voice). Optional.
    #[serde(default)]
    pub platform: Option<String>,
    /// Raw content the user pasted or typed.
    #[serde(default)]
    pub content: String,
    /// Optional sample posts (array of strings) used by draft_voice.
    #[serde(default)]
    pub sample_posts: Vec<String>,
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/social/connections?token=...
///
/// Returns the caller's connections. Credentials are never returned —
/// only metadata the UI needs to render status pills.
pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<SocialConnection>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let uid = principal.user_id();
    let agent_filter = params.get("agent_id").cloned();
    let db = state.db_path.clone();

    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<SocialConnection>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Credentials are deliberately NOT selected — the UI never renders them,
        // and omitting them from the query reduces the chance of accidental leak.
        let sql = "SELECT id, platform, display_name, status, status_detail, agent_id, \
                          connected_at, last_verified_at, expires_at \
                   FROM social_connections WHERE user_id = ? \
                   ORDER BY platform";
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let iter = stmt.query_map([uid], |r| {
            Ok(SocialConnection {
                id: r.get(0)?,
                platform: r.get(1)?,
                display_name: r.get(2)?,
                status: r.get(3)?,
                status_detail: r.get(4)?,
                agent_id: r.get(5)?,
                connected_at: r.get(6)?,
                last_verified_at: r.get(7)?,
                expires_at: r.get(8)?,
            })
        }).map_err(|e| e.to_string())?;
        for row in iter {
            let row = row.map_err(|e| e.to_string())?;
            if let Some(ref f) = agent_filter {
                if row.agent_id.as_deref() != Some(f.as_str()) { continue; }
            }
            out.push(row);
        }
        Ok(out)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows))
}

/// POST /api/social/connections
///
/// Upsert a connection. Keyed by (user_id, platform, agent_id): repeated
/// POSTs for the same triple update the existing row rather than inserting
/// a second. This is what lets the Phase-1 import script be idempotent.
pub async fn handle_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateConnectionRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await?;
    let uid = principal.user_id();

    if req.platform.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let creds_str = req.credentials.to_string();
    let status = req.status.unwrap_or_else(|| "connected".to_string());
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let platform = req.platform.clone();
    let display_name = req.display_name.clone();
    let agent_id = req.agent_id.clone();
    let status_clone = status.clone();
    let status_detail = req.status_detail.clone();
    let expires_at = req.expires_at;

    let result = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Manual upsert on (user_id, platform, COALESCE(agent_id,'')) since
        // we want NULL agent_id to dedupe as the same logical row. SQLite
        // treats NULL as distinct in UNIQUE constraints, so we handle it
        // here explicitly instead of declaring one on the table.
        let existing: Option<i64> = conn.query_row(
            "SELECT id FROM social_connections \
             WHERE user_id = ? AND platform = ? AND COALESCE(agent_id,'') = COALESCE(?,'')",
            rusqlite::params![uid, platform, agent_id],
            |r| r.get(0),
        ).ok();
        if let Some(id) = existing {
            conn.execute(
                "UPDATE social_connections SET \
                   display_name = ?, credentials_json = ?, status = ?, status_detail = ?, \
                   expires_at = ?, last_verified_at = ?, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![
                    display_name, creds_str, status_clone, status_detail,
                    expires_at, now, now, id
                ],
            ).map_err(|e| e.to_string())?;
            Ok(id)
        } else {
            conn.execute(
                "INSERT INTO social_connections \
                   (user_id, platform, display_name, credentials_json, status, status_detail, \
                    agent_id, connected_at, last_verified_at, expires_at, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    uid, platform, display_name, creds_str, status_clone, status_detail,
                    agent_id, now, now, expires_at, now, now
                ],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    log::info!("[social] user={} upserted connection platform={} status={}", uid, req.platform, status);
    Ok(Json(serde_json::json!({ "ok": true, "id": result })))
}

/// DELETE /api/social/connections/:id
///
/// Remove a connection. Only the owning user can delete; admins without
/// a user_id cannot reach through.
pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<DeleteConnectionRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let deleted = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM social_connections WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
        ).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    log::warn!("[social] user={} deleted connection id={}", uid, id);
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Nyota-assisted setup ────────────────────────────────────────────────────

/// POST /api/social/nyota/assist
///
/// Small-bore LLM helper for Settings fields. Each `intent` has a tight
/// system prompt in Nyota's voice that asks the model to distill rather
/// than embellish. Output is always editable by the user before it saves.
pub async fn handle_nyota_assist(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NyotaAssistRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, msg: &str| (code, Json(serde_json::json!({ "ok": false, "error": msg })));
    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err(s, "Sign in again."))?;
    let _uid = principal.user_id();

    let (system_prompt, user_prompt) = match req.intent.as_str() {
        "draft_voice" => {
            let samples = if !req.sample_posts.is_empty() {
                req.sample_posts.iter()
                    .enumerate()
                    .map(|(i, p)| format!("SAMPLE {} —\n{}", i + 1, p.trim()))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            } else {
                req.content.trim().to_string()
            };
            if samples.is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Paste a few sample posts first so I have something to listen to."));
            }
            let sys = "You are Nyota, a calm editor. Read the user's past posts and distill a one-paragraph brand-voice description (80-140 words). Describe how they sound, what they care about, what they avoid. Use 'They' (third person) — this is a specification, not a letter to them. Do not invent traits not evidenced in the samples. Do not embellish. No emoji. No growth jargon. Output only the paragraph, nothing else.".to_string();
            let usr = format!("Here are the samples:\n\n{}", samples);
            (sys, usr)
        }
        "draft_pillars" => {
            if req.content.trim().is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Tell me a few things you post about — a sentence or two is enough."));
            }
            let platform_note = req.platform.as_deref()
                .filter(|s| !s.is_empty())
                .map(|p| format!(" Platform context: {}.", p))
                .unwrap_or_default();
            let sys = format!(
                "You are Nyota, a calm editor. Turn the user's rough interests into 3-5 content pillars for social media. Each pillar is one short sentence describing a recurring post type.{} Output one pillar per line, no numbering, no bullets, no prose before or after. Pillars should be distinct and actionable.",
                platform_note
            );
            (sys, req.content.trim().to_string())
        }
        "draft_audience" => {
            if req.content.trim().is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Sketch who you're writing for — a few words is enough."));
            }
            let sys = "You are Nyota, a calm editor. Turn the user's rough audience sketch into a one-sentence description of who they are writing for. Specific, concrete, no more than 25 words. No marketing language. Output only the sentence.".to_string();
            (sys, req.content.trim().to_string())
        }
        "draft_blocklist" => {
            if req.content.trim().is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Describe what you want to avoid — words, topics, vibes."));
            }
            let sys = "You are Nyota. The user wants a blocklist: words, phrases, hashtags, or topics to avoid in their posts. Read their description and output a comma-separated list of 5-15 terms. Lowercase. No leading # on hashtag-like terms (they'll be fuzzy-matched). Output ONLY the comma-separated list.".to_string();
            (sys, req.content.trim().to_string())
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unknown intent.")),
    };

    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let messages = vec![
        crate::llm::ChatMessage::system(&system_prompt),
        crate::llm::ChatMessage::user(&user_prompt),
    ];
    let reply = match tokio::time::timeout(std::time::Duration::from_secs(30), chain.call(&messages)).await {
        Ok(Ok(text)) => text.trim().to_string(),
        Ok(Err(e)) => return Err(err(StatusCode::BAD_GATEWAY, &format!("Model error: {}", e))),
        Err(_) => return Err(err(StatusCode::GATEWAY_TIMEOUT, "That took too long — try again in a moment.")),
    };

    Ok(Json(serde_json::json!({ "ok": true, "draft": reply })))
}

// ── Descriptors ─────────────────────────────────────────────────────────────

/// GET /api/social/platforms
///
/// Returns the full descriptor list the Connections pane renders — live
/// adapters + stubbed platforms. UI uses this to drive wizard content
/// and determine which Connect/Reconnect buttons are enabled.
pub async fn handle_platforms(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal_scoped(&state, token, "social").await?;
    let descriptors = platforms::all_descriptors();
    Ok(Json(serde_json::json!({ "platforms": descriptors })))
}

// ── Reconnect ───────────────────────────────────────────────────────────────

/// POST /api/social/connections/reconnect/:platform
///
/// Takes `{ token, fields, agent_id? }` and dispatches to the platform
/// adapter's `reconnect()`. On success, upserts the row in
/// `social_connections` with status=connected and the fresh credential
/// blob. Error paths map to SocialError variants so the UI renders a
/// consistent, human-readable message.
pub async fn handle_reconnect(
    State(state): State<Arc<AppState>>,
    Path(platform): Path<String>,
    Json(req): Json<ReconnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let err_json = |status: StatusCode, msg: String| {
        (status, Json(serde_json::json!({ "ok": false, "error": msg })))
    };

    let principal = crate::resolve_principal_scoped(&state, &req.token, "social").await
        .map_err(|s| err_json(s, "Sign in again to reconnect a platform.".to_string()))?;
    let uid = principal.user_id();

    let adapters = platforms::registry();
    let adapter = adapters.get(platform.as_str())
        .ok_or_else(|| err_json(
            StatusCode::BAD_REQUEST,
            format!("No live adapter for '{}' yet. That platform will light up in a later phase.", platform),
        ))?;

    // Detect refresh vs fresh-connect: if the user didn't hand us any
    // wizard fields AND there's an existing row with credentials we can
    // rotate, call adapter.refresh() with the stored blob. Otherwise
    // treat it as a fresh connect with whatever fields they supplied.
    let fields_empty = req.fields.as_object().map(|o| o.is_empty()).unwrap_or(true);

    // When agent_id is specified, match exact. When the client didn't
    // send one (typical — most UIs today don't know which persona's row
    // they're targeting), fall back to any row for (user, platform) and
    // take the most recently verified one. Sean's rows are tagged with
    // agent_id='crimson-lantern' from the workspace import; the modal
    // doesn't know that and shouldn't have to.
    let existing: Option<(i64, Option<String>, serde_json::Value)> = if fields_empty {
        let db = state.db_path.clone();
        let platform_q = platform.clone();
        let agent_q = req.agent_id.clone();
        tokio::task::spawn_blocking(move || -> Option<(i64, Option<String>, serde_json::Value)> {
            let conn = rusqlite::Connection::open(&db).ok()?;
            let row: Option<(i64, Option<String>, String)> = if let Some(agent) = agent_q.as_deref() {
                conn.query_row(
                    "SELECT id, agent_id, credentials_json FROM social_connections \
                     WHERE user_id = ? AND platform = ? AND agent_id = ?",
                    rusqlite::params![uid, platform_q, agent],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).ok()
            } else {
                conn.query_row(
                    "SELECT id, agent_id, credentials_json FROM social_connections \
                     WHERE user_id = ? AND platform = ? \
                     ORDER BY COALESCE(last_verified_at, updated_at) DESC LIMIT 1",
                    rusqlite::params![uid, platform_q],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).ok()
            };
            let (id, found_agent, json) = row?;
            let creds: serde_json::Value = serde_json::from_str(&json).ok()?;
            Some((id, found_agent, creds))
        })
        .await
        .ok()
        .flatten()
    } else { None };
    let existing_creds = existing.as_ref().map(|(_, _, c)| c.clone());
    // When the request didn't specify agent_id but we found one, use it
    // on the write path so we update the SAME row instead of inserting a
    // duplicate.
    let effective_agent_id = req.agent_id.clone()
        .or_else(|| existing.as_ref().and_then(|(_, a, _)| a.clone()));

    let stored = if let Some(creds) = existing_creds {
        // Silent refresh. On success we rebuild a StoredCredentials from
        // the refreshed blob + the adapter's verify() for display_name.
        match adapter.refresh(&state.client, &creds).await {
            Ok(refreshed) => {
                let display_name = adapter.verify(&state.client, &refreshed.credentials).await
                    .map(|v| v.display_name)
                    .unwrap_or_else(|_| creds.get("handle").or_else(|| creds.get("username"))
                        .and_then(|v| v.as_str()).unwrap_or("(refreshed)").to_string());
                platforms::StoredCredentials {
                    display_name,
                    credentials: refreshed.credentials,
                    expires_at: refreshed.expires_at,
                }
            }
            Err(e) => {
                log::warn!("[social] user={} platform={} refresh failed: {:?}", uid, platform, e);
                return Err(err_json(StatusCode::UNPROCESSABLE_ENTITY, e.user_message()));
            }
        }
    } else {
        let input = platforms::ConnectInput { fields: req.fields.clone() };
        match adapter.reconnect(&state.client, &input).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[social] user={} platform={} reconnect failed: {:?}", uid, platform, e);
                return Err(err_json(StatusCode::UNPROCESSABLE_ENTITY, e.user_message()));
            }
        }
    };

    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let platform_s = platform.clone();
    let agent_id = effective_agent_id.clone();
    let display_name = stored.display_name.clone();
    let creds_str = stored.credentials.to_string();
    let expires_at = stored.expires_at;
    let existing_id: Option<i64> = existing.as_ref().map(|(id, _, _)| *id);

    let row_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // If we already looked up the row (refresh path), reuse its id
        // directly so we can't miss it due to an agent_id mismatch.
        let target_id: Option<i64> = existing_id.or_else(|| {
            if let Some(agent) = agent_id.as_deref() {
                conn.query_row(
                    "SELECT id FROM social_connections \
                     WHERE user_id = ? AND platform = ? AND agent_id = ?",
                    rusqlite::params![uid, platform_s, agent],
                    |r| r.get(0),
                ).ok()
            } else {
                conn.query_row(
                    "SELECT id FROM social_connections \
                     WHERE user_id = ? AND platform = ? \
                     ORDER BY COALESCE(last_verified_at, updated_at) DESC LIMIT 1",
                    rusqlite::params![uid, platform_s],
                    |r| r.get(0),
                ).ok()
            }
        });
        if let Some(id) = target_id {
            conn.execute(
                "UPDATE social_connections SET \
                   display_name = ?, credentials_json = ?, status = 'connected', \
                   status_detail = NULL, expires_at = ?, last_verified_at = ?, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![display_name, creds_str, expires_at, now, now, id],
            ).map_err(|e| e.to_string())?;
            Ok(id)
        } else {
            conn.execute(
                "INSERT INTO social_connections \
                   (user_id, platform, display_name, credentials_json, status, \
                    agent_id, connected_at, last_verified_at, expires_at, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, 'connected', ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    uid, platform_s, display_name, creds_str, agent_id,
                    now, now, expires_at, now, now
                ],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    })
    .await
    .map_err(|_| err_json(StatusCode::INTERNAL_SERVER_ERROR, "Server error while saving the connection.".to_string()))?
    .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    log::info!("[social] user={} platform={} reconnected ok id={}", uid, platform, row_id);
    Ok(Json(serde_json::json!({
        "ok": true,
        "id": row_id,
        "display_name": stored.display_name,
    })))
}
