//! Library — auto-classified document intake + storage.
//!
//! Implements the document scanning protocol per
//! `vault/projects/syntaur_doc_intake_storage.md`. This module owns the
//! `~/.syntaur/library/` tree on disk and the `library_files` /
//! `library_inbox_items` tables in `index.db` (schema v70).
//!
//! Phase 1 (this file): single-file ingest endpoint + classifier + disk
//! routing + low-confidence inbox.
//! Later phases bolt on: receipt image cleanup (P2), year-folder
//! migration + audit export (P3), photos + face-rec (P4), tag system
//! (P5), encryption (P6), consume folder + iOS Shortcut + Apple Photos
//! sync (P7), faceted search + audit bundle (P8), cross-user sharing
//! ACL (P9).

use anyhow::{anyhow, Result};
use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

pub mod acl;
pub mod classifier;
pub mod cleanup;
pub mod consume_watcher;
pub mod encryption;
pub mod cover_sheet;
pub mod faces;
pub mod paperless_import;
pub mod photos_sync;
pub mod shares;
pub mod tags;
pub mod year_archive;

use crate::AppState;

/// Library root — lives under the data dir alongside `uploads/` and
/// `receipts/`. Created on first ingest if absent.
pub fn library_root() -> PathBuf {
    crate::resolve_data_dir().join("library")
}

/// Pre-decrypted cache (Phase 6). For MVP we don't encrypt yet; this
/// returns the same path as `library_root()`. Once Phase 6 lands the
/// cache lives at `~/.syntaur/library-unlocked/` and the encrypted
/// store moves to `~/.syntaur/library-vault/`.
pub fn library_cache_root() -> PathBuf {
    library_root()
}

/// Make sure all the canonical subdirectories exist. Called on startup
/// + before first write. Idempotent.
pub fn ensure_layout() -> std::io::Result<()> {
    let root = library_root();
    let dirs = [
        "_inbox",
        "_trash",
        "_originals",
        "photos",
        "tax",
        "personal/identity",
        "personal/property",
        "personal/medical",
        "personal/legal",
        "personal/household",
        "manuals/appliances",
        "manuals/electronics",
        "manuals/tools",
        "manuals/vehicles",
        "tax-archive",
    ];
    for d in dirs {
        std::fs::create_dir_all(root.join(d))?;
    }
    Ok(())
}

/// Compute sha256 of a byte slice (synchronous, fine for ingest paths
/// where we have the bytes in memory anyway).
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Classification result from the vision LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub kind: String,                // photo | receipt | tax_form | personal_doc | manual | unknown
    pub confidence: f64,             // 0.0 .. 1.0
    pub alternatives: Vec<(String, f64)>,
    pub doc_date: Option<String>,    // YYYY-MM-DD if extractable
    pub notes: Option<String>,
    pub vendor: Option<String>,      // for receipts
    pub form_type: Option<String>,   // W-2, 1099-NEC, etc., for tax_form
    pub entity: Option<String>,      // personal | business
    pub year: Option<i32>,           // tax year for tax docs
}

impl Classification {
    pub fn unknown() -> Self {
        Self {
            kind: "unknown".into(),
            confidence: 0.0,
            alternatives: vec![],
            doc_date: None,
            notes: Some("Could not classify".into()),
            vendor: None,
            form_type: None,
            entity: None,
            year: None,
        }
    }
}

pub(crate) const CONFIDENCE_THRESHOLD: f64 = 0.85;

/// Decide the disk path under `library/` for a classified file.
/// Returns the relative path (under library root) to file at.
pub fn target_relative_path(c: &Classification, original_filename: &str, scan_year: i32) -> String {
    let now = chrono::Utc::now();
    let stamp = now.format("%Y%m%d-%H%M%S");
    let short_uuid = &uuid::Uuid::new_v4().to_string()[..8];
    let ext = std::path::Path::new(original_filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    // Below confidence threshold → inbox for human triage
    if c.confidence < CONFIDENCE_THRESHOLD || c.kind == "unknown" {
        return format!("_inbox/{stamp}-{short_uuid}.{ext}");
    }

    match c.kind.as_str() {
        "photo" => {
            let year = c.year.unwrap_or(scan_year);
            let month = c.doc_date
                .as_ref()
                .and_then(|d| d.split('-').nth(1).and_then(|m| m.parse::<u32>().ok()))
                .unwrap_or_else(|| now.format("%m").to_string().parse().unwrap_or(1));
            format!("photos/{:04}/{:02}/{stamp}-{short_uuid}.{ext}", year, month)
        }
        "receipt" => {
            let year = c.year.unwrap_or(scan_year);
            let entity = c.entity.as_deref().unwrap_or("personal");
            let vendor = c.vendor
                .as_deref()
                .map(sanitize_filename_segment)
                .unwrap_or_else(|| "unknown-vendor".into());
            let date = c.doc_date.as_deref().unwrap_or(&stamp.to_string()).replace(':', "");
            format!("tax/{year}/{entity}/receipts/{vendor}-{date}-{short_uuid}.{ext}")
        }
        "tax_form" => {
            let year = c.year.unwrap_or(scan_year);
            let entity = c.entity.as_deref().unwrap_or("personal");
            let form = c.form_type.as_deref().map(sanitize_filename_segment).unwrap_or_else(|| "form".into());
            let issuer = c.vendor.as_deref().map(sanitize_filename_segment).unwrap_or_else(|| "unknown".into());
            format!("tax/{year}/{entity}/forms/{form}-{issuer}-{short_uuid}.{ext}")
        }
        "personal_doc" => {
            let category = c.notes.as_deref().and_then(|n| {
                let n_lower = n.to_lowercase();
                if n_lower.contains("birth") || n_lower.contains("ssn") || n_lower.contains("passport") || n_lower.contains("license") {
                    Some("identity")
                } else if n_lower.contains("deed") || n_lower.contains("title") || n_lower.contains("survey") {
                    Some("property")
                } else if n_lower.contains("medical") || n_lower.contains("doctor") || n_lower.contains("prescription") {
                    Some("medical")
                } else if n_lower.contains("contract") || n_lower.contains("will") || n_lower.contains("legal") {
                    Some("legal")
                } else {
                    Some("household")
                }
            }).unwrap_or("household");
            format!("personal/{category}/{stamp}-{short_uuid}.{ext}")
        }
        "manual" => {
            let category = c.notes.as_deref().and_then(|n| {
                let n_lower = n.to_lowercase();
                if n_lower.contains("appliance") || n_lower.contains("dishwasher") || n_lower.contains("refrigerator") || n_lower.contains("washer") {
                    Some("appliances")
                } else if n_lower.contains("electronic") || n_lower.contains("tv") || n_lower.contains("stereo") {
                    Some("electronics")
                } else if n_lower.contains("vehicle") || n_lower.contains("car") || n_lower.contains("truck") {
                    Some("vehicles")
                } else {
                    Some("tools")
                }
            }).unwrap_or("tools");
            let product = c.vendor.as_deref().map(sanitize_filename_segment).unwrap_or_else(|| "unknown-product".into());
            format!("manuals/{category}/{product}-{short_uuid}.{ext}")
        }
        _ => format!("_inbox/{stamp}-{short_uuid}.{ext}"),
    }
}

/// Strip path separators, control chars, and shell-special chars from
/// a filename segment so it's safe to use in disk paths.
pub fn sanitize_filename_segment(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == ' ' || c == '.' || c == ',' {
            out.push('-');
        }
        // drop everything else
    }
    let trimmed: String = out.chars().take(60).collect();
    if trimmed.is_empty() { "item".into() } else { trimmed }
}

// ───────────────────────────────────────────────────────────────────────
// Ingest endpoint
// ───────────────────────────────────────────────────────────────────────

/// Response from `/api/library/ingest`.
#[derive(Debug, Serialize)]
pub struct IngestResponse {
    pub success: bool,
    pub file_id: Option<i64>,
    pub kind: String,
    pub confidence: f64,
    pub status: String,                   // filed | inbox | duplicate
    pub relative_path: String,
    pub original_filename: String,
    pub size_bytes: usize,
    pub doc_date: Option<String>,
    pub error: Option<String>,
    pub duplicate_of: Option<i64>,        // file_id of existing dup
}

/// Query parameters for `/api/library/ingest`. Hint pre-classifies
/// (e.g. tax module sends `hint=receipt` so we skip the LLM pass).
#[derive(Debug, Deserialize)]
pub struct IngestQuery {
    pub hint: Option<String>,
    pub source: Option<String>, // 'apple-photos' | 'consume-folder' | 'paperless-import' | 'web' | 'shortcut'
}

/// POST /api/library/ingest — multipart upload, classify, route.
pub async fn handle_ingest(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<IngestQuery>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<IngestResponse>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();

    ensure_layout().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Read the file bytes from the multipart body.
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut original_filename = String::from("upload.bin");
    let mut content_type = String::from("application/octet-stream");
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || name == "image" || name == "document" {
            if let Some(fname) = field.file_name() {
                original_filename = fname.to_string();
            }
            if let Some(ct) = field.content_type() {
                content_type = ct.to_string();
            }
            match field.bytes().await {
                Ok(b) => file_bytes = Some(b.to_vec()),
                Err(_) => return Err(StatusCode::BAD_REQUEST),
            }
        }
    }
    let bytes = match file_bytes {
        Some(b) => b,
        None => {
            return Ok(Json(IngestResponse {
                success: false,
                file_id: None,
                kind: "unknown".into(),
                confidence: 0.0,
                status: "error".into(),
                relative_path: String::new(),
                original_filename,
                size_bytes: 0,
                doc_date: None,
                error: Some("No file in request".into()),
                duplicate_of: None,
            }));
        }
    };
    let size = bytes.len();

    // Size cap (20MB) — same as /api/voice/transcribe.
    if size > 20 * 1024 * 1024 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    // Sha256 + dedup check.
    let sha = sha256_bytes(&bytes);
    let db_path = state.db_path.clone();
    let sha_for_dedup = sha.clone();
    let dup: Option<(i64, String)> = tokio::task::spawn_blocking(move || -> Option<(i64, String)> {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(_) => return None,
        };
        conn.query_row(
            "SELECT id, relative_path FROM library_files WHERE user_id = ? AND sha256 = ? LIMIT 1",
            params![user_id, &sha_for_dedup],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()
    })
    .await
    .unwrap_or(None);

    if let Some((dup_id, dup_path)) = dup {
        return Ok(Json(IngestResponse {
            success: true,
            file_id: Some(dup_id),
            kind: "duplicate".into(),
            confidence: 1.0,
            status: "duplicate".into(),
            relative_path: dup_path,
            original_filename,
            size_bytes: size,
            doc_date: None,
            error: None,
            duplicate_of: Some(dup_id),
        }));
    }

    // Classify.
    let classification = if let Some(hint) = q.hint.as_deref() {
        // Pre-classified by caller (tax module, Apple Photos sync, etc.).
        // Trust the hint with high confidence — caller knows the source.
        Classification {
            kind: hint.to_string(),
            confidence: 0.99,
            alternatives: vec![],
            doc_date: None,
            notes: Some(format!("hinted by source={}", q.source.as_deref().unwrap_or("api"))),
            vendor: None,
            form_type: None,
            entity: None,
            year: None,
        }
    } else {
        match classifier::classify(&state, &bytes, &content_type, &original_filename).await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[library] classifier failed: {e} — routing to inbox");
                Classification {
                    notes: Some(format!("classifier error: {e}")),
                    ..Classification::unknown()
                }
            }
        }
    };

    let scan_year: i32 = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2026);
    let relative_path = target_relative_path(&classification, &original_filename, scan_year);
    let abs_path = library_root().join(&relative_path);

    if let Some(parent) = abs_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Phase 2 hook: if it's a receipt, run the cleanup pipeline now.
    // Cleanup needs to operate on raw image bytes, so it runs BEFORE
    // encryption. Cleanup writes back to disk plaintext, then we
    // re-read + re-encrypt below if the kind warrants it.
    if classification.kind == "receipt" {
        // Write plaintext first so cleanup_receipt has something to mutate.
        if let Err(e) = std::fs::write(&abs_path, &bytes) {
            log::error!("[library] failed to write {}: {}", abs_path.display(), e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        if let Err(e) = cleanup::cleanup_receipt(&abs_path).await {
            log::warn!("[library] receipt cleanup failed (kept original): {e}");
        }
        // Re-read + encrypt the cleaned bytes if encryption applies.
        if encryption::should_encrypt_kind(&classification.kind) {
            match std::fs::read(&abs_path) {
                Ok(cleaned) => {
                    match encryption::maybe_encrypt(&state.master_key, &classification.kind, &cleaned) {
                        Ok(enc) => { let _ = std::fs::write(&abs_path, &enc); }
                        Err(e) => log::warn!("[library] encrypt-after-cleanup failed: {e}"),
                    }
                }
                Err(e) => log::warn!("[library] re-read for encryption failed: {e}"),
            }
        }
    } else {
        // Non-receipt path: encrypt-then-write in one shot.
        let to_write = match encryption::maybe_encrypt(&state.master_key, &classification.kind, &bytes) {
            Ok(b) => b,
            Err(e) => {
                log::error!("[library] encrypt failed for {}: {}", abs_path.display(), e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        if let Err(e) = std::fs::write(&abs_path, &to_write) {
            log::error!("[library] failed to write {}: {}", abs_path.display(), e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let now = chrono::Utc::now().timestamp();
    let status = if classification.confidence < CONFIDENCE_THRESHOLD || classification.kind == "unknown" {
        "inbox"
    } else {
        "filed"
    };

    let meta_json = serde_json::json!({
        "alternatives": classification.alternatives,
        "vendor": classification.vendor,
        "form_type": classification.form_type,
        "entity": classification.entity,
        "year": classification.year,
        "notes": classification.notes,
        "source": q.source,
    })
    .to_string();

    let kind_db = classification.kind.clone();
    let conf_db = classification.confidence;
    let status_db = status.to_string();
    let rel_db = relative_path.clone();
    let orig_db = original_filename.clone();
    let ct_db = content_type.clone();
    let doc_date_db = classification.doc_date.clone();
    let meta_db = meta_json.clone();
    let alt_json = serde_json::to_string(&classification.alternatives).unwrap_or_else(|_| "[]".into());
    let notes_db = classification.notes.clone();

    let db_path2 = state.db_path.clone();
    let file_id: Result<i64, String> = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db_path2).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO library_files
             (user_id, sha256, relative_path, original_filename, content_type, size_bytes,
              kind, classifier_confidence, status, doc_date, scan_date, meta_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![user_id, &sha, &rel_db, &orig_db, &ct_db, size as i64,
                    &kind_db, conf_db, &status_db, &doc_date_db, now, &meta_db],
        )
        .map_err(|e| e.to_string())?;
        let fid = conn.last_insert_rowid();
        if status_db == "inbox" {
            conn.execute(
                "INSERT INTO library_inbox_items
                 (file_id, suggested_kind, suggested_confidence, alternatives_json, classifier_notes, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![fid, &kind_db, conf_db, &alt_json, &notes_db, now],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(fid)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    match file_id {
        Ok(fid) => {
            log::info!(
                "[library] ingest fid={} kind={} conf={:.2} status={} path={}",
                fid, classification.kind, classification.confidence, status, relative_path
            );

            // Post-insert hooks: tags + (tax) manifest + (photo) face detect.
            // Each one is best-effort — a failure logs but doesn't fail the ingest.
            //
            // Tags: synchronous because they should be visible the moment
            // the file_id is returned (UI immediately filters by tag).
            let tags_db_path = state.db_path.clone();
            let tags_classification = classification.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = match rusqlite::Connection::open(&tags_db_path) { Ok(c) => c, Err(_) => return };
                if let Err(e) = tags::apply_system_tags(&conn, user_id, fid, &tags_classification) {
                    log::warn!("[library] apply_system_tags failed: {e}");
                }
            }).await;

            // Year manifest: regenerate `tax/<year>/manifest.json` whenever
            // a tax-folder ingest lands. Cheap (just rebuilds from DB rows).
            if relative_path.starts_with("tax/") {
                let mf_db_path = state.db_path.clone();
                let mf_year = classification.year.unwrap_or(scan_year);
                tokio::task::spawn(async move {
                    let r = tokio::task::spawn_blocking(move || {
                        year_archive::write_year_manifest(&mf_db_path, user_id, mf_year)
                    }).await;
                    if let Ok(Err(e)) = r {
                        log::warn!("[library] write_year_manifest({mf_year}) failed: {e}");
                    }
                });
            }

            // Face detect: only photos, fire-and-forget. The inference
            // service may be unreachable (no GPU model loaded) — in that
            // case the call short-circuits with a warn and the photo is
            // still filed normally.
            if classification.kind == "photo" {
                let face_state = state.clone();
                let face_bytes = bytes.clone();
                tokio::task::spawn(async move {
                    match faces::detect_and_embed_for_file(&face_state, fid, user_id, face_bytes).await {
                        Ok(n) => log::info!("[library] face-detect fid={fid} n={n}"),
                        Err(e) => log::debug!("[library] face-detect skipped fid={fid}: {e}"),
                    }
                });
            }

            // Phase 9 hook (deferred): if user opted into the link mesh,
            // fire auto-link triggers here. For MVP we just persist.

            Ok(Json(IngestResponse {
                success: true,
                file_id: Some(fid),
                kind: classification.kind,
                confidence: classification.confidence,
                status: status.to_string(),
                relative_path,
                original_filename,
                size_bytes: size,
                doc_date: classification.doc_date,
                error: None,
                duplicate_of: None,
            }))
        }
        Err(e) => {
            log::error!("[library] DB insert failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ───────────────────────────────────────────────────────────────────────
// File listing / inbox / serve / delete endpoints
// ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct FileSummary {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub original_filename: String,
    pub relative_path: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub doc_date: Option<String>,
    pub scan_date: i64,
    pub confidence: f64,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let db_path = state.db_path.clone();
    let kind_filter = q.kind.clone();
    let status_filter = q.status.clone();

    let rows: Vec<FileSummary> = tokio::task::spawn_blocking(move || -> Vec<FileSummary> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut sql = String::from(
            "SELECT id, kind, status, original_filename, relative_path, content_type,
                    size_bytes, doc_date, scan_date, classifier_confidence
             FROM library_files WHERE user_id = ?"
        );
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
        if let Some(k) = &kind_filter {
            sql.push_str(" AND kind = ?");
            p.push(Box::new(k.clone()));
        }
        if let Some(s) = &status_filter {
            sql.push_str(" AND status = ?");
            p.push(Box::new(s.clone()));
        }
        sql.push_str(" ORDER BY scan_date DESC LIMIT ?");
        p.push(Box::new(limit));
        let params_refs: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params_refs.as_slice(), |r| {
            Ok(FileSummary {
                id: r.get(0)?,
                kind: r.get(1)?,
                status: r.get(2)?,
                original_filename: r.get(3)?,
                relative_path: r.get(4)?,
                content_type: r.get(5)?,
                size_bytes: r.get(6)?,
                doc_date: r.get(7)?,
                scan_date: r.get(8)?,
                confidence: r.get(9)?,
            })
        });
        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        }
    })
    .await
    .unwrap_or_default();

    Ok(Json(serde_json::json!({ "files": rows })))
}

/// Resolve a (file_id, user_id) to bytes + content-type WITHOUT auth —
/// used by the share-token redeem path. The caller has already verified
/// the share is valid and attached `?shr=<token>` for downstream auditing.
pub async fn serve_file_for_share(
    state: &Arc<AppState>,
    file_id: i64,
    owner_user_id: i64,
    share_token: &str,
) -> Result<axum::response::Response, StatusCode> {
    let db_path = state.db_path.clone();
    let row: Option<(String, String, String)> = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db_path).ok()?;
        conn.query_row(
            "SELECT relative_path, content_type, original_filename
             FROM library_files WHERE id = ? AND user_id = ?",
            params![file_id, owner_user_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .ok()
    })
    .await
    .unwrap_or(None);
    let (rel_path, ct, fname) = row.ok_or(StatusCode::NOT_FOUND)?;
    let abs = library_cache_root().join(&rel_path);
    let raw = std::fs::read(&abs).map_err(|_| StatusCode::NOT_FOUND)?;
    let bytes = encryption::decrypt_if_needed(&state.master_key, &raw)
        .map_err(|e| { log::warn!("[library] decrypt for share fid={file_id}: {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;
    axum::response::Response::builder()
        .header("Content-Type", ct)
        .header("Content-Length", bytes.len())
        .header("Content-Disposition", format!("inline; filename=\"{fname}\""))
        .header("X-Syntaur-Share-Token", share_token)
        .body(axum::body::Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Year-folder share: build the same zip the export endpoint produces.
pub async fn serve_year_zip_for_share(
    state: &Arc<AppState>,
    owner_user_id: i64,
    year: i32,
    share_token: &str,
) -> Result<axum::response::Response, StatusCode> {
    use std::io::Write as _;
    let db_path = state.db_path.clone();
    let key_clone = (*state.master_key).clone();
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let manifest = year_archive::build_year_manifest(&db_path, owner_user_id, year)?;
        let cover = cover_sheet::build_year_cover(&manifest).unwrap_or_default();
        let mut zip_bytes: Vec<u8> = Vec::with_capacity(manifest.total_size_bytes as usize + 1024);
        {
            let cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut zip = zip::ZipWriter::new(cursor);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let _ = zip.start_file("manifest.json", opts);
            let _ = zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes());
            if !cover.is_empty() {
                let _ = zip.start_file("cover-sheet.pdf", opts);
                let _ = zip.write_all(&cover);
            }
            for entry in &manifest.files {
                let abs = library_root().join(&entry.relative_path);
                if let Ok(raw) = std::fs::read(&abs) {
                    let plain = encryption::decrypt_if_needed(&key_clone, &raw)
                        .unwrap_or_else(|_| raw);
                    let _ = zip.start_file(&entry.relative_path, opts);
                    let _ = zip.write_all(&plain);
                }
            }
            let _ = zip.finish();
        }
        Ok(zip_bytes)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    axum::response::Response::builder()
        .header("Content-Type", "application/zip")
        .header("Content-Disposition", format!("attachment; filename=\"tax-{year}.zip\""))
        .header("Content-Length", bytes.len())
        .header("X-Syntaur-Share-Token", share_token)
        .body(axum::body::Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn handle_get_content(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(file_id): AxPath<i64>,
) -> Result<axum::response::Response, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let row: Option<(String, String)> = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db_path).ok()?;
        conn.query_row(
            "SELECT relative_path, content_type FROM library_files WHERE id = ? AND user_id = ?",
            params![file_id, user_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()
    })
    .await
    .unwrap_or(None);

    let (rel_path, ct) = match row {
        Some(r) => r,
        None => return Err(StatusCode::NOT_FOUND),
    };
    let abs = library_cache_root().join(&rel_path);
    let raw = std::fs::read(&abs).map_err(|_| StatusCode::NOT_FOUND)?;
    let bytes = encryption::decrypt_if_needed(&state.master_key, &raw)
        .map_err(|e| { log::warn!("[library] decrypt fid={file_id}: {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;
    let resp = axum::response::Response::builder()
        .header("Content-Type", ct)
        .header("Content-Length", bytes.len())
        .body(axum::body::Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct InboxConfirmBody {
    pub kind: String,                    // user-chosen final kind
    pub vendor: Option<String>,
    pub doc_date: Option<String>,
    pub entity: Option<String>,
    pub year: Option<i32>,
    pub form_type: Option<String>,
    pub action: Option<String>,          // 'confirm' | 'reject'
}

/// POST /api/library/inbox/{file_id}/confirm — user confirms (or
/// rejects) a low-confidence classification. On confirm: re-route the
/// file to its proper folder + flip status='filed'. On reject:
/// status='trash' and the file moves to _trash/.
pub async fn handle_inbox_confirm(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(file_id): AxPath<i64>,
    Json(body): Json<InboxConfirmBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let action = body.action.as_deref().unwrap_or("confirm");
    let db_path = state.db_path.clone();

    let existing: Option<(String, String)> = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        move || {
            let conn = rusqlite::Connection::open(&db_path).ok()?;
            conn.query_row(
                "SELECT relative_path, original_filename FROM library_files WHERE id = ? AND user_id = ?",
                params![file_id, user_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
        }
    })
    .await
    .unwrap_or(None);

    let (old_rel, original_filename) = existing.ok_or(StatusCode::NOT_FOUND)?;
    let old_abs = library_root().join(&old_rel);

    if action == "reject" {
        let trash_rel = format!("_trash/{}", old_rel.replace('/', "_"));
        let trash_abs = library_root().join(&trash_rel);
        let _ = std::fs::create_dir_all(trash_abs.parent().unwrap_or(&library_root()));
        let _ = std::fs::rename(&old_abs, &trash_abs);
        let _ = tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&db_path).ok()?;
            let _ = conn.execute(
                "UPDATE library_files SET relative_path = ?, status = 'trash' WHERE id = ?",
                params![&trash_rel, file_id],
            );
            let _ = conn.execute("DELETE FROM library_inbox_items WHERE file_id = ?", params![file_id]);
            Some(())
        })
        .await;
        return Ok(Json(serde_json::json!({ "success": true, "status": "trashed" })));
    }

    // Confirm path — re-classify by the user's chosen kind, route, move file.
    let scan_year: i32 = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2026);
    let manual_class = Classification {
        kind: body.kind.clone(),
        confidence: 1.0,
        alternatives: vec![],
        doc_date: body.doc_date.clone(),
        notes: Some("user-confirmed from inbox".into()),
        vendor: body.vendor.clone(),
        form_type: body.form_type.clone(),
        entity: body.entity.clone(),
        year: body.year,
    };
    let new_rel = target_relative_path(&manual_class, &original_filename, scan_year);
    let new_abs = library_root().join(&new_rel);
    if let Some(parent) = new_abs.parent() { let _ = std::fs::create_dir_all(parent); }
    let _ = std::fs::rename(&old_abs, &new_abs);

    let new_rel_db = new_rel.clone();
    let kind_db = body.kind.clone();
    let doc_date_db = body.doc_date.clone();
    let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE library_files
             SET relative_path = ?, kind = ?, status = 'filed',
                 doc_date = COALESCE(?, doc_date), classifier_confidence = 1.0
             WHERE id = ?",
            params![&new_rel_db, &kind_db, &doc_date_db, file_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM library_inbox_items WHERE file_id = ?", params![file_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "status": "filed",
        "relative_path": new_rel,
    })))
}

pub async fn handle_inbox_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let rows: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT f.id, f.original_filename, f.content_type, f.size_bytes,
                    f.relative_path, f.scan_date,
                    i.suggested_kind, i.suggested_confidence, i.alternatives_json, i.classifier_notes
             FROM library_files f
             JOIN library_inbox_items i ON i.file_id = f.id
             WHERE f.user_id = ? AND f.status = 'inbox'
             ORDER BY f.scan_date DESC LIMIT 200"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id], |r| {
            Ok(serde_json::json!({
                "file_id": r.get::<_, i64>(0)?,
                "original_filename": r.get::<_, String>(1)?,
                "content_type": r.get::<_, String>(2)?,
                "size_bytes": r.get::<_, i64>(3)?,
                "relative_path": r.get::<_, String>(4)?,
                "scan_date": r.get::<_, i64>(5)?,
                "suggested_kind": r.get::<_, String>(6)?,
                "suggested_confidence": r.get::<_, f64>(7)?,
                "alternatives": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(8)?).unwrap_or(serde_json::json!([])),
                "notes": r.get::<_, Option<String>>(9)?,
            }))
        });
        match mapped { Ok(iter) => iter.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "items": rows })))
}
