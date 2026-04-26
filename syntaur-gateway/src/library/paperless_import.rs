//! Phase 9 — Paperless-ngx → library importer.
//!
//! One-shot migration of Sean's existing Paperless documents into the
//! library. Drives off the Paperless REST API:
//!
//!   GET /api/documents/?ordering=-created&page=N  — paginated list
//!   GET /api/documents/{id}/download/             — raw bytes
//!   GET /api/documents/{id}/                      — metadata (tags, type, dates)
//!
//! Each document gets `POST /api/library/ingest?source=paperless-import`
//! with the Paperless tags forwarded as a meta hint, then we cross-link
//! the resulting library_file to the Paperless ID in `library_links`
//! so duplicates can be detected on a second pass.
//!
//! Triggered manually via `POST /api/library/import/paperless` — not
//! a background sweep. Sean has the Paperless instance off after migration
//! so this only needs to run once per install.

use anyhow::{anyhow, Result};
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    /// Paperless base URL (e.g. http://192.168.1.35:8000).
    pub base_url: String,
    /// Paperless API token (from /api/token/ on the Paperless side).
    pub api_token: String,
    /// Cap so a partial run doesn't pull 50k docs at once.
    pub max_documents: Option<i64>,
    /// Dry-run: list what would be imported, don't actually ingest.
    pub dry_run: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub total_seen: i64,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct PaperlessDoc {
    id: i64,
    title: String,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    document_type: Option<i64>,
    #[serde(default)]
    correspondent: Option<i64>,
    #[serde(default)]
    tags: Vec<i64>,
    #[serde(default)]
    archive_serial_number: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaperlessPage {
    count: i64,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    results: Vec<PaperlessDoc>,
}

pub async fn handle_import(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportResult>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    if !principal.is_admin() { return Err(StatusCode::FORBIDDEN); }
    let user_id = principal.user_id();
    let dry = req.dry_run.unwrap_or(false);
    let cap = req.max_documents.unwrap_or(10_000);

    let result = run_import(state.clone(), user_id, req, cap, dry).await
        .map_err(|e| { log::error!("[library/paperless] import failed: {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;
    Ok(Json(result))
}

async fn run_import(
    state: Arc<AppState>,
    user_id: i64,
    req: ImportRequest,
    cap: i64,
    dry: bool,
) -> Result<ImportResult> {
    let base = req.base_url.trim_end_matches('/').to_string();
    let auth = format!("Token {}", req.api_token);
    let mut total_seen = 0i64;
    let mut imported = 0i64;
    let mut duplicates = 0i64;
    let mut failed = 0i64;
    let mut next_url = Some(format!("{base}/api/documents/?ordering=-created&page=1"));

    while let Some(url) = next_url.take() {
        if total_seen >= cap { break; }
        let page: PaperlessPage = state.client
            .get(&url)
            .header("Authorization", &auth)
            .send().await.map_err(|e| anyhow!("paperless GET: {e}"))?
            .json().await.map_err(|e| anyhow!("paperless decode: {e}"))?;
        next_url = page.next.clone();
        for doc in page.results {
            if total_seen >= cap { break; }
            total_seen += 1;
            if dry { continue; }
            let dl_url = format!("{base}/api/documents/{}/download/", doc.id);
            let bytes = match state.client.get(&dl_url).header("Authorization", &auth).send().await {
                Ok(r) => match r.bytes().await { Ok(b) => b.to_vec(), Err(_) => { failed += 1; continue; } },
                Err(_) => { failed += 1; continue; },
            };
            let outcome = ingest_via_self(&state, user_id, &doc, bytes).await;
            match outcome {
                Ok(ingest_kind) if ingest_kind == "duplicate" => duplicates += 1,
                Ok(_) => imported += 1,
                Err(e) => { log::warn!("[paperless] ingest doc {} failed: {e}", doc.id); failed += 1; }
            }
        }
    }
    Ok(ImportResult { total_seen, imported, duplicates, failed, dry_run: dry })
}

/// Internal: bypass the HTTP layer and call the ingest pipeline directly.
/// Saves the round-trip + lets us pre-populate the meta with Paperless tags.
async fn ingest_via_self(
    state: &Arc<AppState>,
    user_id: i64,
    doc: &PaperlessDoc,
    bytes: Vec<u8>,
) -> Result<String> {
    use rusqlite::params;

    // Dedup
    let sha = crate::library::sha256_bytes(&bytes);
    let db_path = state.db_path.clone();
    let sha_for_dedup = sha.clone();
    let dup: Option<i64> = tokio::task::spawn_blocking(move || -> Option<i64> {
        let conn = rusqlite::Connection::open(&db_path).ok()?;
        conn.query_row(
            "SELECT id FROM library_files WHERE user_id = ? AND sha256 = ?",
            params![user_id, &sha_for_dedup], |r| r.get(0),
        ).ok()
    }).await.ok().flatten();
    if dup.is_some() { return Ok("duplicate".into()); }

    // Heuristic kind from Paperless: title-based fallback if no
    // document_type is set. The classifier still runs; this is just a
    // hint to bias the routing.
    let kind_hint = guess_kind_from_title(&doc.title);
    let classification = crate::library::Classification {
        kind: kind_hint.clone(),
        confidence: 0.9, // imported = trusted
        alternatives: vec![],
        doc_date: doc.created.clone().map(|s| s.split('T').next().unwrap_or(&s).to_string()),
        notes: Some(format!("imported from paperless id={}", doc.id)),
        vendor: None,
        form_type: None,
        entity: None,
        year: doc.created.as_deref()
            .and_then(|s| s.split('-').next())
            .and_then(|y| y.parse::<i32>().ok()),
    };
    let scan_year = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2026);
    let relative_path = crate::library::target_relative_path(&classification, &doc.title, scan_year);
    let abs_path = crate::library::library_root().join(&relative_path);
    if let Some(p) = abs_path.parent() { let _ = std::fs::create_dir_all(p); }
    let to_write = crate::library::encryption::maybe_encrypt(state.master_key.as_ref(), &classification.kind, &bytes)
        .map_err(|e| anyhow!("encrypt: {e}"))?;
    std::fs::write(&abs_path, &to_write).map_err(|e| anyhow!("write: {e}"))?;

    let now = chrono::Utc::now().timestamp();
    let size = bytes.len() as i64;
    let kind_db = classification.kind.clone();
    let conf_db = classification.confidence;
    let rel_db = relative_path.clone();
    let title_db = doc.title.clone();
    let doc_date_db = classification.doc_date.clone();
    let meta_db = serde_json::json!({
        "imported_from": "paperless",
        "paperless_id": doc.id,
        "paperless_tags": doc.tags,
        "paperless_correspondent": doc.correspondent,
        "paperless_document_type": doc.document_type,
        "year": classification.year,
        "doc_date": doc_date_db,
    }).to_string();
    let db_path = state.db_path.clone();
    let _ = tokio::task::spawn_blocking(move || -> Result<()> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        conn.execute(
            "INSERT INTO library_files
             (user_id, sha256, relative_path, original_filename, content_type, size_bytes,
              kind, classifier_confidence, status, doc_date, scan_date, meta_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'filed', ?, ?, ?)",
            params![user_id, &sha, &rel_db, &title_db, "application/pdf", size,
                    &kind_db, conf_db, &doc_date_db, now, &meta_db],
        ).map_err(|e| anyhow!("insert: {e}"))?;
        // Link mesh row so a future run sees this is already imported.
        let fid = conn.last_insert_rowid();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO library_links
             (user_id, src_kind, src_id, dst_kind, dst_id, relation, confidence, created_at, created_by)
             VALUES (?, 'library', ?, 'paperless', ?, 'imported_from', 1.0, ?, 'paperless-import')",
            params![user_id, fid, doc.id, now],
        );
        Ok(())
    }).await.map_err(|e| anyhow!("join: {e}"))?;
    Ok(kind_hint)
}

fn guess_kind_from_title(title: &str) -> String {
    let t = title.to_lowercase();
    if t.contains("receipt") || t.contains("rcpt") { return "receipt".into(); }
    if t.contains("statement") || t.contains("stmt") { return "statement".into(); }
    if t.contains("1099") || t.contains("w-2") || t.contains("w2") || t.contains("1098") { return "tax_form".into(); }
    if t.contains("manual") || t.contains("guide") { return "manual".into(); }
    "personal_doc".into()
}
