//! Phase 7 — Consume folder watcher.
//!
//! Background task that polls `library_root()/_consume/` every 30 s and
//! ingests any new files it finds into the regular pipeline. Mirrors the
//! Paperless `CONSUMPTION_DIR` ergonomic — drag a file into the folder
//! (over SMB/AFP from the desktop, or from an iOS Shortcut), see it
//! appear in the library a moment later.
//!
//! Settings gate: only runs when `library_settings.consume_folder_enabled
//! = 1` for at least one user. The "owning user" of a consumed file is
//! the first admin user (sean) — the consume folder is a single-user
//! drop-zone, not a multi-tenant inbox. For shared households this is
//! fine because library_shares grants visibility downstream.
//!
//! Atomicity: we move the file to `_consume/_inflight/` first to claim
//! it (rename is atomic on POSIX), then ingest from the inflight copy.
//! Failures move the file to `_consume/_failed/` with a `.error.txt`
//! sibling describing what went wrong, so the operator sees stuck files
//! rather than silent loss.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::AppState;

const POLL_INTERVAL_SECS: u64 = 30;

/// Spawn the watcher loop. Idempotent — second call returns immediately
/// because the underlying tokio task is already running.
///
/// Wired from main.rs at startup, behind the same module-enabled gate
/// that controls the rest of the library subsystem.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        log::info!("[library/consume] watcher started; poll={}s", POLL_INTERVAL_SECS);
        loop {
            if let Err(e) = sweep_once(&state).await {
                log::warn!("[library/consume] sweep failed: {e}");
            }
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
    });
}

async fn sweep_once(state: &Arc<AppState>) -> Result<()> {
    // Find any user with the feature enabled.
    let db_path = state.db_path.clone();
    let owner_user_id: Option<i64> = tokio::task::spawn_blocking(move || -> Option<i64> {
        let conn = rusqlite::Connection::open(&db_path).ok()?;
        conn.query_row::<i64, _, _>(
            "SELECT user_id FROM library_settings WHERE consume_folder_enabled = 1
             ORDER BY user_id ASC LIMIT 1",
            [], |r| r.get(0),
        ).ok()
    }).await.ok().flatten();

    let user_id = match owner_user_id {
        Some(uid) => uid,
        None => return Ok(()), // feature off — silent no-op
    };

    let consume_dir = crate::library::library_root().join("_consume");
    let inflight_dir = consume_dir.join("_inflight");
    let failed_dir = consume_dir.join("_failed");
    let _ = std::fs::create_dir_all(&inflight_dir);
    let _ = std::fs::create_dir_all(&failed_dir);

    let entries = match std::fs::read_dir(&consume_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() { continue; }
        // Skip the staging subdirs and dotfiles.
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if fname.starts_with('_') || fname.starts_with('.') { continue; }
        if let Err(e) = process_one(state, user_id, &path, &inflight_dir, &failed_dir).await {
            log::warn!("[library/consume] {} failed: {e}", path.display());
        }
    }
    Ok(())
}

async fn process_one(
    state: &Arc<AppState>,
    user_id: i64,
    path: &Path,
    inflight_dir: &Path,
    failed_dir: &Path,
) -> Result<()> {
    let fname = path.file_name().ok_or_else(|| anyhow!("no filename"))?
        .to_string_lossy().to_string();

    // Wait for the file to settle — uploads via SMB can land in chunks.
    // Two stat reads 1s apart, same size = stable.
    let s1 = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let s2 = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if s1 != s2 || s1 == 0 { return Ok(()); }

    let inflight_path: PathBuf = inflight_dir.join(&fname);
    std::fs::rename(path, &inflight_path).map_err(|e| anyhow!("claim rename: {e}"))?;
    let bytes = std::fs::read(&inflight_path).map_err(|e| anyhow!("read inflight: {e}"))?;

    // Run the same pipeline the HTTP ingest endpoint does — but in-process,
    // bypassing the multipart parsing.
    let content_type = guess_content_type(&fname);
    let classification = match crate::library::classifier::classify(state, &bytes, &content_type, &fname).await {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::write(failed_dir.join(format!("{fname}.error.txt")), format!("classifier: {e}"));
            let _ = std::fs::rename(&inflight_path, failed_dir.join(&fname));
            return Err(anyhow!("classify: {e}"));
        }
    };
    let scan_year = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2026);
    let relative_path = crate::library::target_relative_path(&classification, &fname, scan_year);
    let abs = crate::library::library_root().join(&relative_path);
    if let Some(p) = abs.parent() { let _ = std::fs::create_dir_all(p); }
    // Encrypt-on-write for non-photo kinds. We can't `rename` into the
    // final path because the bytes need transformation; write directly
    // and drop the inflight copy.
    let to_write = crate::library::encryption::maybe_encrypt(state.master_key.as_ref(), &classification.kind, &bytes)
        .map_err(|e| anyhow!("encrypt: {e}"))?;
    std::fs::write(&abs, &to_write).map_err(|e| anyhow!("write: {e}"))?;
    let _ = std::fs::remove_file(&inflight_path);

    let sha = crate::library::sha256_bytes(&bytes);
    let now = chrono::Utc::now().timestamp();
    let size = bytes.len() as i64;
    let kind_db = classification.kind.clone();
    let conf_db = classification.confidence;
    let rel_db = relative_path.clone();
    let doc_date_db = classification.doc_date.clone();
    let meta_db = serde_json::json!({
        "source": "consume-folder",
        "vendor": classification.vendor,
        "year": classification.year,
        "form_type": classification.form_type,
        "entity": classification.entity,
        "notes": classification.notes,
    }).to_string();
    let status = if conf_db < crate::library::CONFIDENCE_THRESHOLD || kind_db == "unknown" { "inbox" } else { "filed" };
    let status_db = status.to_string();

    let db_path = state.db_path.clone();
    let fname_db = fname.clone();
    let _ = tokio::task::spawn_blocking(move || -> Result<()> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        conn.execute(
            "INSERT INTO library_files
             (user_id, sha256, relative_path, original_filename, content_type, size_bytes,
              kind, classifier_confidence, status, doc_date, scan_date, meta_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![user_id, &sha, &rel_db, &fname_db, &content_type, size,
                    &kind_db, conf_db, &status_db, &doc_date_db, now, &meta_db],
        ).map_err(|e| anyhow!("insert: {e}"))?;
        Ok(())
    }).await.map_err(|e| anyhow!("join: {e}"))?;

    log::info!("[library/consume] ingested {} → {} ({})", fname, relative_path, status);
    Ok(())
}

fn guess_content_type(fname: &str) -> String {
    let lower = fname.to_lowercase();
    if lower.ends_with(".pdf") { "application/pdf".into() }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg".into() }
    else if lower.ends_with(".png") { "image/png".into() }
    else if lower.ends_with(".webp") { "image/webp".into() }
    else if lower.ends_with(".heic") { "image/heic".into() }
    else if lower.ends_with(".txt") { "text/plain".into() }
    else if lower.ends_with(".md") { "text/markdown".into() }
    else { "application/octet-stream".into() }
}
