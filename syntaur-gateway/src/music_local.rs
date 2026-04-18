//! Local music library — folders the user has added as sources, the
//! tracks indexed from them, and a range-supporting stream endpoint so
//! HTML5 `<audio>` can scrub.
//!
//! Storage: `local_music_folders` + `local_music_tracks` tables (schema
//! v43). Every row is `user_id`-scoped; a user can't see or stream
//! another user's folder even if they guess track ids.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, Response, StatusCode};
use axum::response::Json;
use log::info;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::AppState;

// Supported extensions. Aligned with what `lofty` reliably decodes.
const AUDIO_EXT: &[&str] = &["mp3", "flac", "m4a", "aac", "ogg", "oga", "opus", "wav", "wma", "aiff", "aif"];
const MAX_FOLDER_DEPTH: usize = 12;

fn extract_token(h: &HeaderMap, q: Option<&str>) -> String {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.to_string())
        .or_else(|| q.map(|s| s.to_string()))
        .unwrap_or_default()
}

#[derive(Deserialize)]
pub struct TokenQuery { pub token: Option<String> }

// ── Folder CRUD ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddFolderReq {
    pub token: String,
    pub path: String,
    pub label: Option<String>,
}

/// POST /api/music/local/folders — register a new local music source.
/// The path is canonicalized and must exist on the gateway host. A scan
/// is NOT run immediately; call POST /scan or trigger from the UI.
pub async fn add_folder(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddFolderReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();

    let raw = req.path.trim().to_string();
    if raw.is_empty() { return Err((StatusCode::BAD_REQUEST, "path is empty".into())); }

    // Expand leading ~ so users can paste ~/Music naturally.
    let expanded = if let Some(stripped) = raw.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = PathBuf::from(home);
            if !stripped.is_empty() { p.push(stripped.trim_start_matches('/')); }
            p
        } else { PathBuf::from(raw.clone()) }
    } else {
        PathBuf::from(raw.clone())
    };

    let canonical = match std::fs::canonicalize(&expanded) {
        Ok(p) => p,
        Err(e) => return Err((StatusCode::BAD_REQUEST, format!("cannot access {}: {}", expanded.display(), e))),
    };
    if !canonical.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("{} is not a directory", canonical.display())));
    }
    let canonical_str = canonical.to_string_lossy().to_string();
    let label = req.label.unwrap_or_else(|| {
        canonical.file_name().and_then(|n| n.to_str()).unwrap_or("Music").to_string()
    });
    let now = chrono::Utc::now().timestamp();

    let db = state.db_path.clone();
    let id = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "INSERT INTO local_music_folders (user_id, path, label, added_at) VALUES (?,?,?,?)
             ON CONFLICT (user_id, path) DO UPDATE SET label = excluded.label",
            rusqlite::params![uid, canonical_str, label, now],
        )?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
      .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!("[music-local] user {} added folder {}", uid, canonical.display());
    Ok(Json(json!({"ok": true, "id": id, "path": canonical.to_string_lossy()})))
}

/// GET /api/music/local/folders — list this user's folders with track counts.
pub async fn list_folders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let rows = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT id, path, label, added_at, last_scan_at, track_count
             FROM local_music_folders WHERE user_id = ? ORDER BY added_at DESC"
        )?;
        let rows = stmt.query_map(rusqlite::params![uid], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "path": r.get::<_, String>(1)?,
                "label": r.get::<_, Option<String>>(2)?,
                "added_at": r.get::<_, i64>(3)?,
                "last_scan_at": r.get::<_, Option<i64>>(4)?,
                "track_count": r.get::<_, i64>(5)?,
            }))
        })?;
        rows.collect()
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({"folders": rows})))
}

/// DELETE /api/music/local/folders/:id — drop the folder and all its tracks.
pub async fn remove_folder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let deleted = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        // user_id guard prevents cross-user deletion
        conn.execute(
            "DELETE FROM local_music_folders WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
        )
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if deleted == 0 { return Err(StatusCode::NOT_FOUND); }
    info!("[music-local] user {} removed folder {}", uid, id);
    Ok(Json(json!({"ok": true})))
}

// ── Scan ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ScanReq {
    pub token: String,
    /// When set, scan only that folder; otherwise scan all the user's folders.
    pub folder_id: Option<i64>,
}

/// POST /api/music/local/scan — walk one or all folders and (re-)index tracks.
pub async fn scan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScanReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let folder_id = req.folder_id;

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let mut conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Pull folders to scan
        let folders: Vec<(i64, String)> = if let Some(fid) = folder_id {
            let mut stmt = conn.prepare(
                "SELECT id, path FROM local_music_folders WHERE user_id = ? AND id = ?"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(
                rusqlite::params![uid, fid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            ).map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, path FROM local_music_folders WHERE user_id = ?"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(
                rusqlite::params![uid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            ).map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        };

        let mut summary = Vec::new();
        for (fid, path) in folders {
            let (found, errs) = scan_one_folder(&mut conn, uid, fid, &path);
            summary.push(json!({
                "folder_id": fid, "path": path, "tracks": found, "errors": errs,
            }));
        }
        Ok(summary)
    }).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(summary) => Ok(Json(json!({"ok": true, "results": summary}))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// Walk `folder_path`, extract metadata via lofty, upsert into
/// local_music_tracks. Returns (tracks_indexed, errors).
fn scan_one_folder(conn: &mut rusqlite::Connection, uid: i64, folder_id: i64, folder_path: &str) -> (usize, usize) {
    let now = chrono::Utc::now().timestamp();
    let mut found = 0usize;
    let mut errs = 0usize;
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(_) => return (0, 1),
    };

    // Clear tracks for this folder before re-indexing (simpler than diffing)
    let _ = tx.execute(
        "DELETE FROM local_music_tracks WHERE folder_id = ? AND user_id = ?",
        rusqlite::params![folder_id, uid],
    );

    // Recursive walk
    let root = PathBuf::from(folder_path);
    walk_audio_files(&root, 0, &mut |p| {
        match extract_metadata(p) {
            Ok(meta) => {
                let res = tx.execute(
                    "INSERT INTO local_music_tracks
                        (user_id, folder_id, path, title, artist, album, duration_ms, track_no, year, indexed_at)
                     VALUES (?,?,?,?,?,?,?,?,?,?)
                     ON CONFLICT (user_id, path) DO UPDATE SET
                        folder_id = excluded.folder_id,
                        title = excluded.title,
                        artist = excluded.artist,
                        album = excluded.album,
                        duration_ms = excluded.duration_ms,
                        track_no = excluded.track_no,
                        year = excluded.year,
                        indexed_at = excluded.indexed_at",
                    rusqlite::params![
                        uid, folder_id, p.to_string_lossy(),
                        meta.title, meta.artist, meta.album,
                        meta.duration_ms, meta.track_no, meta.year, now,
                    ],
                );
                if res.is_ok() { found += 1; } else { errs += 1; }
            }
            Err(_) => errs += 1,
        }
    });

    let _ = tx.execute(
        "UPDATE local_music_folders SET last_scan_at = ?, track_count = ? WHERE id = ?",
        rusqlite::params![now, found as i64, folder_id],
    );
    let _ = tx.commit();
    (found, errs)
}

fn walk_audio_files(root: &Path, depth: usize, visit: &mut dyn FnMut(&Path)) {
    if depth > MAX_FOLDER_DEPTH { return; }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let path = e.path();
        // Skip hidden files/folders (.DS_Store, iTunes metadata junk, etc.)
        if let Some(n) = path.file_name().and_then(|n| n.to_str()) {
            if n.starts_with('.') { continue; }
        }
        let ft = match e.file_type() { Ok(ft) => ft, Err(_) => continue };
        if ft.is_dir() {
            walk_audio_files(&path, depth + 1, visit);
        } else if ft.is_file() {
            let ext = path.extension()
                .and_then(|x| x.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if AUDIO_EXT.contains(&ext.as_str()) {
                visit(&path);
            }
        }
    }
}

struct TrackMeta {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
    track_no: Option<i64>,
    year: Option<i64>,
}

fn extract_metadata(path: &Path) -> Result<TrackMeta, String> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::Accessor;

    let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let duration_ms = tagged.properties().duration().as_millis() as i64;
    let (title, artist, album, track_no, year) = if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        (
            tag.title().map(|s| s.into_owned()),
            tag.artist().map(|s| s.into_owned()),
            tag.album().map(|s| s.into_owned()),
            tag.track().map(|n| n as i64),
            tag.year().map(|y| y as i64),
        )
    } else { (None, None, None, None, None) };

    // Fallback title = file stem when tag missing (common on loose WAVs)
    let title = title.or_else(|| {
        path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string())
    });

    Ok(TrackMeta { title, artist, album, duration_ms: Some(duration_ms), track_no, year })
}

// ── Track list / search ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TracksQuery {
    pub token: Option<String>,
    pub q: Option<String>,
    pub folder_id: Option<i64>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// GET /api/music/local/tracks — list/search the user's local tracks.
pub async fn list_tracks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TracksQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let offset = q.offset.unwrap_or(0).max(0);
    let search = q.q.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let folder_filter = q.folder_id;

    let db = state.db_path.clone();
    let (rows, total) = tokio::task::spawn_blocking(move || -> rusqlite::Result<(Vec<Value>, i64)> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut where_clause = String::from("user_id = ?");
        if folder_filter.is_some() { where_clause.push_str(" AND folder_id = ?"); }
        if search.is_some() {
            where_clause.push_str(" AND (LOWER(COALESCE(title,'')) LIKE ?1_pat OR LOWER(COALESCE(artist,'')) LIKE ?1_pat OR LOWER(COALESCE(album,'')) LIKE ?1_pat)");
        }

        // Build WHERE inline (rusqlite placeholders are positional so we build params in order).
        let search_pat = search.as_ref().map(|s| format!("%{}%", s.to_lowercase()));

        // Count
        let count_sql = if search.is_some() {
            if folder_filter.is_some() {
                "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ? AND folder_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?3 OR LOWER(COALESCE(artist,'')) LIKE ?3 OR LOWER(COALESCE(album,'')) LIKE ?3)"
            } else {
                "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?2 OR LOWER(COALESCE(artist,'')) LIKE ?2 OR LOWER(COALESCE(album,'')) LIKE ?2)"
            }
        } else if folder_filter.is_some() {
            "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ? AND folder_id = ?"
        } else {
            "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ?"
        };

        let total: i64 = match (folder_filter, &search_pat) {
            (Some(fid), Some(pat)) => conn.query_row(count_sql, rusqlite::params![uid, fid, pat], |r| r.get(0))?,
            (Some(fid), None)      => conn.query_row(count_sql, rusqlite::params![uid, fid], |r| r.get(0))?,
            (None, Some(pat))      => conn.query_row(count_sql, rusqlite::params![uid, pat], |r| r.get(0))?,
            (None, None)           => conn.query_row(count_sql, rusqlite::params![uid], |r| r.get(0))?,
        };

        let list_sql = if search.is_some() {
            if folder_filter.is_some() {
                "SELECT id, title, artist, album, duration_ms, track_no, year
                 FROM local_music_tracks
                 WHERE user_id = ? AND folder_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?3 OR LOWER(COALESCE(artist,'')) LIKE ?3 OR LOWER(COALESCE(album,'')) LIKE ?3)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?4 OFFSET ?5"
            } else {
                "SELECT id, title, artist, album, duration_ms, track_no, year
                 FROM local_music_tracks
                 WHERE user_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?2 OR LOWER(COALESCE(artist,'')) LIKE ?2 OR LOWER(COALESCE(album,'')) LIKE ?2)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?3 OFFSET ?4"
            }
        } else if folder_filter.is_some() {
            "SELECT id, title, artist, album, duration_ms, track_no, year
             FROM local_music_tracks
             WHERE user_id = ? AND folder_id = ?
             ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
             LIMIT ? OFFSET ?"
        } else {
            "SELECT id, title, artist, album, duration_ms, track_no, year
             FROM local_music_tracks
             WHERE user_id = ?
             ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
             LIMIT ? OFFSET ?"
        };

        let mut stmt = conn.prepare(list_sql)?;
        let mut rows_out = Vec::new();
        let map = |r: &rusqlite::Row| -> rusqlite::Result<Value> {
            Ok(json!({
                "id":       r.get::<_, i64>(0)?,
                "title":    r.get::<_, Option<String>>(1)?,
                "artist":   r.get::<_, Option<String>>(2)?,
                "album":    r.get::<_, Option<String>>(3)?,
                "duration_ms": r.get::<_, Option<i64>>(4)?,
                "track_no": r.get::<_, Option<i64>>(5)?,
                "year":     r.get::<_, Option<i64>>(6)?,
            }))
        };
        match (folder_filter, &search_pat) {
            (Some(fid), Some(pat)) => {
                let iter = stmt.query_map(rusqlite::params![uid, fid, pat, limit, offset], map)?;
                for r in iter { rows_out.push(r?); }
            }
            (Some(fid), None) => {
                let iter = stmt.query_map(rusqlite::params![uid, fid, limit, offset], map)?;
                for r in iter { rows_out.push(r?); }
            }
            (None, Some(pat)) => {
                let iter = stmt.query_map(rusqlite::params![uid, pat, limit, offset], map)?;
                for r in iter { rows_out.push(r?); }
            }
            (None, None) => {
                let iter = stmt.query_map(rusqlite::params![uid, limit, offset], map)?;
                for r in iter { rows_out.push(r?); }
            }
        }
        Ok((rows_out, total))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({"tracks": rows, "total": total})))
}

// ── File streaming (HTTP Range) ─────────────────────────────────────────

/// GET /api/music/local/file/:id — stream track audio bytes.
/// Supports HTTP Range so HTML5 `<audio>` can scrub. Ownership gate:
/// the track's user_id must match the authenticated caller.
pub async fn stream_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Response<Body>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let path: PathBuf = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<String>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT path FROM local_music_tracks WHERE id = ? AND user_id = ?"
        )?;
        let r: Option<String> = stmt.query_row(rusqlite::params![id, uid], |r| r.get(0)).ok();
        Ok(r)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map(PathBuf::from)
      .ok_or(StatusCode::NOT_FOUND)?;

    let mut file = tokio::fs::File::open(&path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let meta = file.metadata().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = meta.len();
    let ct = content_type_for_path(&path);

    let range_hdr = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let (start, end, status) = match parse_range(&range_hdr, total) {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0u64, total.saturating_sub(1), StatusCode::OK),
    };
    if start > end || end >= total {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    file.seek(std::io::SeekFrom::Start(start)).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let len = end - start + 1;
    // Read limit: stream up to `len` bytes from the seeked file.
    let limited = file.take(len);
    let stream = tokio_util::io::ReaderStream::new(limited);
    let body = Body::from_stream(stream);

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ct)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::CACHE_CONTROL, "private, max-age=3600");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, total));
    }
    builder.body(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn parse_range(header_val: &Option<String>, total: u64) -> Option<(u64, u64)> {
    let s = header_val.as_ref()?;
    let spec = s.strip_prefix("bytes=")?;
    let mut parts = spec.splitn(2, '-');
    let start_s = parts.next()?;
    let end_s = parts.next()?;
    if start_s.is_empty() {
        // suffix range: bytes=-N → last N bytes
        let n: u64 = end_s.parse().ok()?;
        if n == 0 { return None; }
        let start = total.saturating_sub(n);
        Some((start, total - 1))
    } else {
        let start: u64 = start_s.parse().ok()?;
        let end: u64 = if end_s.is_empty() { total - 1 } else { end_s.parse().ok()? };
        Some((start, end.min(total - 1)))
    }
}

fn content_type_for_path(p: &Path) -> &'static str {
    match p.extension().and_then(|x| x.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("m4a") | Some("aac") => "audio/mp4",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("wav") => "audio/wav",
        Some("wma") => "audio/x-ms-wma",
        Some("aiff") | Some("aif") => "audio/aiff",
        _ => "application/octet-stream",
    }
}

