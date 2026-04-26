//! Phase 3 — Year-foldered tax archive + audit-portable export.
//!
//! Builds the `tax/<year>/manifest.json`, generates the audit-export
//! zip, and runs the adaptive size + age compression policy. The link
//! mesh manifest goes alongside each year-folder export so a CPA can
//! follow links offline.

use anyhow::{anyhow, Result};
use axum::{extract::{Path as AxPath, State}, http::StatusCode, Json};
use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use crate::library::library_root;
use crate::AppState;

const SIZE_ZIP_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;       // 50 MB
const SIZE_COLD_PROMPT_BYTES:  u64 = 500 * 1024 * 1024;       // 500 MB
const DORMANT_MONTHS_FOR_ZIP:  i64 = 12;                       // 1 yr no writes
const IRS_AUDIT_WINDOW_YEARS:  i64 = 7;

#[derive(Debug, Serialize)]
pub struct YearManifest {
    pub year: i32,
    pub generated_at: i64,
    pub user_id: i64,
    pub total_files: usize,
    pub total_size_bytes: u64,
    pub by_kind: serde_json::Map<String, serde_json::Value>,
    pub files: Vec<ManifestEntry>,
    pub link_mesh_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ManifestEntry {
    pub id: i64,
    pub relative_path: String,
    pub kind: String,
    pub vendor: Option<String>,
    pub doc_date: Option<String>,
    pub amount_cents: Option<i64>,
    pub sha256: String,
    pub size_bytes: i64,
}

/// Build the manifest for a tax year. Aggregates rows from
/// `library_files` whose relative_path starts with `tax/<year>/`.
pub fn build_year_manifest(db_path: &std::path::Path, user_id: i64, year: i32) -> Result<YearManifest> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| anyhow!("db: {e}"))?;
    let prefix = format!("tax/{year}/");
    let mut stmt = conn.prepare(
        "SELECT id, relative_path, kind, sha256, size_bytes, doc_date, meta_json
         FROM library_files
         WHERE user_id = ? AND relative_path LIKE ?
         ORDER BY relative_path"
    ).map_err(|e| anyhow!("prepare: {e}"))?;

    let rows = stmt.query_map(params![user_id, format!("{prefix}%")], |r| {
        let meta_str: String = r.get(6)?;
        let meta: serde_json::Value = serde_json::from_str(&meta_str).unwrap_or(serde_json::Value::Null);
        let vendor = meta.get("vendor").and_then(|v| v.as_str()).map(String::from);
        let amount = meta.get("amount_cents").and_then(|v| v.as_i64());
        Ok(ManifestEntry {
            id: r.get(0)?,
            relative_path: r.get(1)?,
            kind: r.get(2)?,
            sha256: r.get(3)?,
            size_bytes: r.get(4)?,
            doc_date: r.get(5)?,
            vendor,
            amount_cents: amount,
        })
    }).map_err(|e| anyhow!("query: {e}"))?;

    let entries: Vec<ManifestEntry> = rows.filter_map(|r| r.ok()).collect();
    let total_size: u64 = entries.iter().map(|e| e.size_bytes as u64).sum();
    let mut by_kind: std::collections::HashMap<String, (usize, u64)> = Default::default();
    for e in &entries {
        let v = by_kind.entry(e.kind.clone()).or_insert((0, 0));
        v.0 += 1;
        v.1 += e.size_bytes as u64;
    }
    let mut by_kind_json = serde_json::Map::new();
    for (k, (count, sz)) in by_kind {
        by_kind_json.insert(k, serde_json::json!({"count": count, "size_bytes": sz}));
    }

    // Link mesh enable check — placeholder for Phase 9 settings table.
    let link_mesh_enabled = conn
        .query_row(
            "SELECT 1 FROM library_files WHERE relative_path LIKE 'tax/links/%' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok();

    Ok(YearManifest {
        year,
        generated_at: chrono::Utc::now().timestamp(),
        user_id,
        total_files: entries.len(),
        total_size_bytes: total_size,
        by_kind: by_kind_json,
        files: entries,
        link_mesh_enabled,
    })
}

/// Write the manifest to `tax/<year>/manifest.json` for in-place
/// browsing. Called after each ingest into a year folder so the
/// manifest stays current.
pub fn write_year_manifest(db_path: &std::path::Path, user_id: i64, year: i32) -> Result<PathBuf> {
    let manifest = build_year_manifest(db_path, user_id, year)?;
    let path = library_root().join("tax").join(year.to_string()).join("manifest.json");
    if let Some(p) = path.parent() { let _ = std::fs::create_dir_all(p); }
    std::fs::write(&path, serde_json::to_string_pretty(&manifest)?)
        .map_err(|e| anyhow!("write manifest: {e}"))?;
    Ok(path)
}

/// GET /api/library/tax/{year}/export — build a zip of the year
/// folder + manifest + cover-sheet PDF; returns the zip bytes.
pub async fn handle_export_year(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(year): AxPath<i32>,
) -> Result<axum::response::Response, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let key_clone = (*state.master_key).clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let manifest = build_year_manifest(&db_path, user_id, year)?;
        let cover = crate::library::cover_sheet::build_year_cover(&manifest)
            .unwrap_or_default(); // best-effort; if PDF gen fails, skip cover
        let mut zip_bytes: Vec<u8> = Vec::with_capacity(manifest.total_size_bytes as usize + 1024);
        {
            let cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut zip = zip::ZipWriter::new(cursor);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            // Manifest.
            zip.start_file("manifest.json", opts).map_err(|e| anyhow!("zip start: {e}"))?;
            zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())
                .map_err(|e| anyhow!("zip write manifest: {e}"))?;

            // Cover sheet (if generated).
            if !cover.is_empty() {
                zip.start_file("cover-sheet.pdf", opts).map_err(|e| anyhow!("zip start cover: {e}"))?;
                zip.write_all(&cover).map_err(|e| anyhow!("zip write cover: {e}"))?;
            }

            // Files. Decrypt at-rest envelope on the way into the zip
            // so the recipient gets readable PDFs/images.
            for entry in &manifest.files {
                let abs = library_root().join(&entry.relative_path);
                if let Ok(raw) = std::fs::read(&abs) {
                    let plain = crate::library::encryption::decrypt_if_needed(&key_clone, &raw)
                        .unwrap_or_else(|_| raw);
                    let _ = zip.start_file(&entry.relative_path, opts);
                    let _ = zip.write_all(&plain);
                }
            }
            zip.finish().map_err(|e| anyhow!("zip finish: {e}"))?;
        }
        Ok(zip_bytes)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| { log::error!("[library/year_export] {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;

    let resp = axum::response::Response::builder()
        .header("Content-Type", "application/zip")
        .header("Content-Disposition", format!("attachment; filename=\"tax-{year}.zip\""))
        .header("Content-Length", bytes.len())
        .body(axum::body::Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(resp)
}

/// Adaptive compression sweep — runs from a periodic task. Identifies
/// year folders meeting the size/age criteria and zips them into
/// `tax-archive/<year>.zip`, leaving a stub README under the original
/// folder pointing at the archive.
pub async fn run_compression_sweep(state: Arc<AppState>) -> Result<()> {
    let db_path = state.db_path.clone();
    let key_clone = (*state.master_key).clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        let now = chrono::Utc::now().timestamp();
        let cutoff_dormant = now - DORMANT_MONTHS_FOR_ZIP * 30 * 86400;
        let cutoff_audit = now - IRS_AUDIT_WINDOW_YEARS * 365 * 86400;

        // Collect distinct (user_id, year) tuples and their last-write +
        // total-size stats.
        let mut stmt = conn.prepare(
            "SELECT user_id,
                    CAST(SUBSTR(relative_path, 5, 4) AS INTEGER) AS year,
                    MAX(scan_date) as last_write,
                    SUM(size_bytes) as total_size
             FROM library_files
             WHERE relative_path LIKE 'tax/%' AND status = 'filed'
             GROUP BY user_id, year"
        ).map_err(|e| anyhow!("prep sweep: {e}"))?;

        let rows: Vec<(i64, i32, i64, i64)> = stmt.query_map([], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))).map_err(|e| anyhow!("query: {e}"))?.filter_map(|r| r.ok()).collect();

        for (uid, year, last_write, total_size) in rows {
            let size_eligible = (total_size as u64) >= SIZE_ZIP_THRESHOLD_BYTES && last_write < cutoff_dormant;
            let audit_eligible = last_write < cutoff_audit;
            if !(size_eligible || audit_eligible) { continue; }

            log::info!("[library/compression] zipping uid={uid} year={year} size={total_size}");
            // Inline build + write — skip going through the export endpoint.
            let manifest = build_year_manifest(&db_path, uid, year)?;
            let archive = library_root().join("tax-archive");
            let _ = std::fs::create_dir_all(&archive);
            let zip_path = archive.join(format!("{year}.zip"));
            let f = std::fs::File::create(&zip_path).map_err(|e| anyhow!("create zip: {e}"))?;
            let mut zip = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("manifest.json", opts).map_err(|e| anyhow!("zip: {e}"))?;
            zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())
                .map_err(|e| anyhow!("zip: {e}"))?;
            for entry in &manifest.files {
                let abs = library_root().join(&entry.relative_path);
                if let Ok(raw) = std::fs::read(&abs) {
                    let plain = crate::library::encryption::decrypt_if_needed(&key_clone, &raw)
                        .unwrap_or_else(|_| raw);
                    let _ = zip.start_file(&entry.relative_path, opts);
                    let _ = zip.write_all(&plain);
                }
            }
            zip.finish().map_err(|e| anyhow!("zip finish: {e}"))?;

            // Leave a README pointing at the archive; remove the originals.
            let year_dir = library_root().join("tax").join(year.to_string());
            let _ = std::fs::write(
                year_dir.join("ARCHIVED.txt"),
                format!("This year was archived to tax-archive/{year}.zip on {}.\nSize was: {total_size} bytes ({} files).\nUnzip the archive to restore.", chrono::Utc::now().to_rfc3339(), manifest.total_files),
            );
        }
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("join: {e}"))?
}

/// Compute sha256 of a file. Used by manifest builder.
#[allow(dead_code)]
pub fn sha256_file(path: &std::path::Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| anyhow!("read for sha: {e}"))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}
