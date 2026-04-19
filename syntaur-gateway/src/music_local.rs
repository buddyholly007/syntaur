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

// Supported extensions — anything that's plausibly an audio file gets
// indexed. Browser-native HTML5 <audio> plays mp3/m4a/aac/mp4/ogg/oga/
// opus/wav/flac/webm/weba directly. Formats like WMA/APE/DSD get
// indexed with correct Content-Type; playback depends on the browser —
// WebKitGTK will reject those and surface a play error (which the UI
// now shows plainly instead of freezing).
const AUDIO_EXT: &[&str] = &[
    // Everyday lossy
    "mp3", "aac", "m4a", "m4b", "m4p", "m4r", "mp4", "3gp", "3gpp",
    // Ogg family
    "ogg", "oga", "opus",
    // WebM / Matroska
    "webm", "weba", "mka",
    // Lossless / PCM
    "flac", "alac", "wav", "wave", "aiff", "aif", "aifc",
    // Windows / older
    "wma",
    // Specialty lossless
    "ape", "tak", "shn", "tta",
    // DSD (SACD rips)
    "dsf", "dff",
    // Other
    "amr", "awb", "ac3", "dts",
];
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
    bit_depth: Option<i64>,
    sample_rate: Option<i64>,
}

fn extract_metadata(path: &Path) -> Result<TrackMeta, String> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::Accessor;

    let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let props = tagged.properties();
    let duration_ms = props.duration().as_millis() as i64;
    let bit_depth = props.bit_depth().map(|v| v as i64);
    let sample_rate = props.sample_rate().map(|v| v as i64);

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

    Ok(TrackMeta { title, artist, album, duration_ms: Some(duration_ms), track_no, year, bit_depth, sample_rate })
}

/// Extract the first `Front` picture (or any picture, as a fallback)
/// from a tagged audio file. Returns (bytes, mime_type). Used at scan
/// time to populate the per-user art cache.
fn extract_embedded_picture(path: &Path) -> Option<(Vec<u8>, String)> {
    use lofty::file::TaggedFileExt;
    use lofty::picture::PictureType;

    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pictures = tag.pictures();
    if pictures.is_empty() { return None; }

    // Prefer CoverFront, fall back to first picture.
    let pic = pictures.iter().find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    let mime = pic.mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "image/jpeg".to_string());
    Some((pic.data().to_vec(), mime))
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
                "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source
                 FROM local_music_tracks
                 WHERE user_id = ? AND folder_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?3 OR LOWER(COALESCE(artist,'')) LIKE ?3 OR LOWER(COALESCE(album,'')) LIKE ?3)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?4 OFFSET ?5"
            } else {
                "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source
                 FROM local_music_tracks
                 WHERE user_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?2 OR LOWER(COALESCE(artist,'')) LIKE ?2 OR LOWER(COALESCE(album,'')) LIKE ?2)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?3 OFFSET ?4"
            }
        } else if folder_filter.is_some() {
            "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source
             FROM local_music_tracks
             WHERE user_id = ? AND folder_id = ?
             ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
             LIMIT ? OFFSET ?"
        } else {
            "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source
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
                "metadata_source": r.get::<_, Option<String>>(7)?,
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
        Some("mp3")                              => "audio/mpeg",
        Some("flac")                             => "audio/flac",
        Some("m4a") | Some("m4b") | Some("m4p")
            | Some("m4r") | Some("mp4")          => "audio/mp4",
        Some("aac")                              => "audio/aac",
        Some("3gp") | Some("3gpp")               => "audio/3gpp",
        Some("ogg") | Some("oga")                => "audio/ogg",
        Some("opus")                             => "audio/opus",
        Some("webm") | Some("weba")              => "audio/webm",
        Some("mka")                              => "audio/x-matroska",
        Some("wav") | Some("wave")               => "audio/wav",
        Some("aiff") | Some("aif") | Some("aifc") => "audio/aiff",
        Some("alac")                             => "audio/alac",
        Some("wma")                              => "audio/x-ms-wma",
        Some("ape")                              => "audio/x-ape",
        Some("tak")                              => "audio/x-tak",
        Some("shn")                              => "audio/x-shn",
        Some("tta")                              => "audio/x-tta",
        Some("dsf")                              => "audio/dsf",
        Some("dff")                              => "audio/dff",
        Some("amr")                              => "audio/amr",
        Some("awb")                              => "audio/amr-wb",
        Some("ac3")                              => "audio/ac3",
        Some("dts")                              => "audio/vnd.dts",
        _ => "application/octet-stream",
    }
}

// ── MusicBrainz lookup + apply-match ─────────────────────────────────
// Sean's ask: "see information that perhaps is labeled in a different
// way" and "auto name and categorize music tool for items not labeled
// correctly." MusicBrainz is the community's canonical metadata
// registry — free, no API key, generous rate limit (1 req/sec).
//
// The lookup endpoint (GET /api/music/local/lookup/:id) takes a track,
// reads its current artist + title, and returns the top MB recording
// matches with canonical title, artist-credit, release name, year, and
// MBID. The UI shows those side-by-side with the file's current tags
// and lets the user pick the right one.
//
// The apply endpoint (POST /api/music/local/match/:id) writes the
// chosen canonical values back into the row + sets metadata_source =
// 'musicbrainz' so we can badge the entry as canonically-tagged.
//
// Rate limiting: MusicBrainz policy is 1 req/sec across the whole
// application. We serialize calls with a global mutex + a sleep. That's
// fine for a small-family deployment; if this scales, swap for a
// proper leaky-bucket.

static MB_LAST_CALL: tokio::sync::Mutex<Option<std::time::Instant>> =
    tokio::sync::Mutex::const_new(None);

/// User-Agent string required by MusicBrainz. Includes project name,
/// version, and a contact path (their policy page specifically asks
/// for a contact). Keep under ~80 chars.
fn mb_user_agent() -> String {
    format!("Syntaur/{} ( https://github.com/buddyholly007/syntaur )",
            env!("CARGO_PKG_VERSION"))
}

/// Sleep just long enough that no two MB calls are made within 1 second.
async fn mb_rate_limit() {
    let mut last = MB_LAST_CALL.lock().await;
    if let Some(prev) = *last {
        let since = prev.elapsed();
        if since < std::time::Duration::from_millis(1100) {
            let wait = std::time::Duration::from_millis(1100) - since;
            tokio::time::sleep(wait).await;
        }
    }
    *last = Some(std::time::Instant::now());
}

#[derive(Deserialize)]
pub struct TrackIdPath {
    #[serde(rename = "id")]
    pub _id: i64,
}

/// GET /api/music/local/lookup/:id — ask MusicBrainz for canonical
/// metadata matching this track's current artist + title.
pub async fn lookup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    // Read current tags for this user's track.
    let db = state.db_path.clone();
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
            let conn = rusqlite::Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT title, artist, album FROM local_music_tracks WHERE id = ? AND user_id = ?"
            )?;
            Ok(stmt.query_row(rusqlite::params![id, uid], |r| Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))).ok())
        }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
          .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (title, artist, album) = row.ok_or(StatusCode::NOT_FOUND)?;

    // Build MB search. Unquoted queries are less precise; Lucene-style
    // field queries (`recording:"x" AND artist:"y"`) work best.
    let mut terms: Vec<String> = Vec::new();
    if let Some(t) = title.as_ref().filter(|s| !s.is_empty()) {
        terms.push(format!(r#"recording:"{}""#, t.replace('"', "")));
    }
    if let Some(a) = artist.as_ref().filter(|s| !s.is_empty()) {
        terms.push(format!(r#"artist:"{}""#, a.replace('"', "")));
    }
    if terms.is_empty() {
        return Ok(Json(json!({
            "current": { "title": title, "artist": artist, "album": album },
            "matches": [],
            "reason": "This track has no title or artist tag to look up. Use Clean up tags to let the AI infer them first."
        })));
    }
    let query = terms.join(" AND ");

    mb_rate_limit().await;
    let resp = state.client.get("https://musicbrainz.org/ws/2/recording/")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .header("User-Agent", mb_user_agent())
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let j: Value = resp.json().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

    // Shape the MB response into something the UI can render without
    // knowing MB's schema.
    let recordings = j.get("recordings").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let matches: Vec<Value> = recordings.into_iter().take(5).map(|rec| {
        let mbid = rec.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let title = rec.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let score = rec.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let length_ms = rec.get("length").and_then(|v| v.as_i64());
        // artist-credit is an array of {name, artist: {...}} — join by space.
        let artist_credit = rec.get("artist-credit")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|ac| {
                ac.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
            }).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        // Try to pull a release name + year from the first official release.
        let (release, year) = rec.get("releases")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .map(|rel| {
                let r_name = rel.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let r_date = rel.get("date").and_then(|v| v.as_str()).unwrap_or("");
                let y = r_date.split('-').next().unwrap_or("").to_string();
                (r_name, y)
            })
            .unwrap_or_default();
        json!({
            "mbid": mbid,
            "title": title,
            "artist": artist_credit,
            "album": release,
            "year": year,
            "duration_ms": length_ms,
            "score": score,
        })
    }).collect();

    Ok(Json(json!({
        "current": { "title": title, "artist": artist, "album": album },
        "matches": matches,
    })))
}

#[derive(Deserialize)]
pub struct ApplyMatchBody {
    pub token: Option<String>,
    pub mbid: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub source: Option<String>,   // defaults to "musicbrainz"
}

/// POST /api/music/local/match/:id — write the user-chosen canonical
/// metadata back to the row. Also accepts plain user-edits when `source`
/// is explicitly "user_edit".
pub async fn apply_match(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<ApplyMatchBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let source = body.source.clone().unwrap_or_else(|| "musicbrainz".to_string());
    let source_for_sql = source.clone();
    let db = state.db_path.clone();
    let mbid = body.mbid.clone();
    let title = body.title.clone();
    let artist = body.artist.clone();
    let album = body.album.clone();
    let updated: usize = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "UPDATE local_music_tracks \
             SET title = ?, artist = ?, album = ?, mbid = ?, metadata_source = ? \
             WHERE id = ? AND user_id = ?",
            rusqlite::params![title, artist, album, mbid, source_for_sql, id, uid],
        )
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(json!({ "ok": true, "source": source })))
}

// ── LLM bulk auto-tag ────────────────────────────────────────────────
// Finds tracks with empty or suspicious metadata (no title, or title
// that looks like a filename: "track01", "01 - audio", etc.) and asks
// the LLM to parse artist/title/album from the file path. Fast,
// dependency-free, handles the common "wrong casing / missing album
// tag / foreign characters" case. Acoustic-fingerprint identification
// is the follow-up for tracks with no recoverable text at all.

#[derive(Deserialize)]
pub struct RetagBody {
    pub token: Option<String>,
    pub limit: Option<usize>,   // cap how many rows to process (default 50)
}

/// Returns true if a title looks like it came from a filename instead
/// of a real tag. These are the rows we want the LLM to clean up.
fn looks_like_filename_title(t: &str) -> bool {
    let lower = t.trim().to_lowercase();
    if lower.is_empty() { return true; }
    if lower.starts_with("track") && lower.len() < 10 { return true; }
    // "01", "01 - audio", "audio 01", etc.
    if lower.chars().filter(|c| c.is_ascii_digit()).count() >= lower.chars().count() / 2 { return true; }
    // Contains a file extension — real tags never do
    if lower.ends_with(".mp3") || lower.ends_with(".flac") || lower.ends_with(".m4a")
       || lower.ends_with(".wav") || lower.ends_with(".ogg") || lower.ends_with(".opus") {
        return true;
    }
    false
}

/// POST /api/music/local/retag_all — bulk auto-tag untagged or
/// mistagged tracks. Batches 20 rows per LLM call for throughput.
pub async fn retag_all(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RetagBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let limit = body.limit.unwrap_or(50).min(200);

    // Find candidates — either NULL/empty title or looks-like-filename title.
    let db = state.db_path.clone();
    let candidates: Vec<(i64, String, Option<String>, Option<String>, Option<String>)> =
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<_>> {
            let conn = rusqlite::Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT id, path, title, artist, album \
                 FROM local_music_tracks \
                 WHERE user_id = ? AND metadata_source != 'user_edit' \
                 ORDER BY id"
            )?;
            let rows: Vec<_> = stmt.query_map(rusqlite::params![uid], |r| Ok((
                r.get::<_, i64>(0)?, r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            )))?.filter_map(|x| x.ok()).collect();
            Ok(rows)
        }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
          .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let filtered: Vec<_> = candidates.into_iter().filter(|(_, _path, title, artist, _album)| {
        let t_suspicious = title.as_ref().map(|t| looks_like_filename_title(t)).unwrap_or(true);
        let a_missing = artist.as_ref().map(|a| a.trim().is_empty()).unwrap_or(true);
        t_suspicious || a_missing
    }).take(limit).collect();

    if filtered.is_empty() {
        return Ok(Json(json!({
            "ok": true, "updated": 0, "scanned": 0,
            "message": "Nothing to clean up — all tags look good."
        })));
    }

    // LLM batches of 20.
    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let mut updated = 0usize;
    let total = filtered.len();

    for batch in filtered.chunks(20) {
        let numbered: String = batch.iter().enumerate().map(|(i, (_, path, t, a, al))| {
            format!("{}. path={} | current_title={} | current_artist={} | current_album={}",
                i + 1,
                path,
                t.as_deref().unwrap_or(""),
                a.as_deref().unwrap_or(""),
                al.as_deref().unwrap_or(""),
            )
        }).collect::<Vec<_>>().join("\n");
        let user_msg = format!(
"Clean up song tags from file paths + existing tags. Return ONLY a JSON \
array in the same order, shape: \
[{{\"title\":\"...\",\"artist\":\"...\",\"album\":\"...\"}}].\n\n\
RULES — read carefully, these are critical:\n\
1. Per-field empty-string means \"I cannot confidently parse this\". \
Syntaur will keep the existing tag unchanged when you return empty — \
DO NOT GUESS. Empty is always safer than wrong.\n\
2. Only parse a field when the file path, folder name, or existing tag \
contains clear evidence. Example: path \
\"Arctic Monkeys/Favourite Worst Nightmare/01-Brianstorm.flac\" → \
{{\"title\":\"Brianstorm\",\"artist\":\"Arctic Monkeys\",\"album\":\"Favourite Worst Nightmare\"}}.\n\
3. When the path contains no title info — e.g. \"29 - Prince -.flac\" \
or \"Track 05.mp3\" — return \"\" for title. Do NOT write the filename \
back as the title.\n\
4. Strip these from parsed values: leading track numbers (01-, A1., \
\"Track 5 - \"), file extensions, disc markers (\"cd1\", \"disc 2\"), \
scene release tags ([FLAC], [24bit], [1080p]), URLs.\n\
5. Normalise capitalization: Title Case for titles; artist names as \
commonly written (\"Arctic Monkeys\", not \"ARCTIC MONKEYS\").\n\
6. Respect the language of the original. Don't translate or transliterate.\n\n\
Tracks:\n{}",
            numbered,
        );
        let messages = vec![
            crate::llm::ChatMessage::system("You are a music-metadata parser. Respond ONLY with a JSON array — no prose, no markdown fences, no explanations."),
            crate::llm::ChatMessage::user(&user_msg),
        ];
        let llm_reply = match chain.call(&messages).await {
            Ok(t) => t,
            Err(_) => continue,
        };
        let cleaned = llm_reply.trim()
            .trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```").trim();
        let parsed: Vec<Value> = serde_json::from_str(cleaned).unwrap_or_default();
        if parsed.is_empty() { continue; }

        // Write each parsed row back — **per-field COALESCE**: when the
        // LLM returns an empty string, we keep the existing value
        // instead of clobbering it. The earlier bug was the opposite:
        // empty title from the LLM destroyed a usable file_tags title
        // (see projects/syntaur_music_module_plan for the incident).
        for (input, out) in batch.iter().zip(parsed.iter()) {
            let (track_id, _path, cur_title, cur_artist, cur_album) = input;
            let new_title = out.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let new_artist = out.get("artist").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let new_album = out.get("album").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

            // If the LLM gave us nothing for every field, this row was
            // a waste — don't touch the DB.
            if new_title.is_empty() && new_artist.is_empty() && new_album.is_empty() {
                continue;
            }

            // Per-field merge. An empty LLM field means "I don't know"
            // — keep what we have.
            let merged_title  = if new_title.is_empty()  { cur_title.clone()  } else { Some(new_title) };
            let merged_artist = if new_artist.is_empty() { cur_artist.clone() } else { Some(new_artist) };
            let merged_album  = if new_album.is_empty()  { cur_album.clone()  } else { Some(new_album) };

            // If the merge changed nothing, skip.
            if &merged_title == cur_title && &merged_artist == cur_artist && &merged_album == cur_album {
                continue;
            }

            let db = state.db_path.clone();
            let id = *track_id;
            let u = uid;
            let ok = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
                let conn = rusqlite::Connection::open(&db)?;
                conn.execute(
                    "UPDATE local_music_tracks \
                     SET title = ?, artist = ?, album = ?, metadata_source = 'llm' \
                     WHERE id = ? AND user_id = ?",
                    rusqlite::params![merged_title, merged_artist, merged_album, id, u],
                )
            }).await.ok().and_then(|r| r.ok()).unwrap_or(0);
            if ok > 0 { updated += 1; }
        }
    }

    info!("[music-local] retag_all uid={} scanned={} updated={}", uid, total, updated);
    Ok(Json(json!({
        "ok": true,
        "scanned": total,
        "updated": updated,
        "message": format!("Cleaned up {} of {} flagged tracks.", updated, total),
    })))
}

