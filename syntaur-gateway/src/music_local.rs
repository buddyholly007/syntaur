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
/// local_music_tracks. Returns (tracks_indexed, errors). Extracts
/// embedded album art (ID3v2 APIC / FLAC picture block / MP4 covr) and
/// caches it under /config/art/<key>.<ext> so the /art endpoint can
/// serve it without re-reading the file.
fn scan_one_folder(conn: &mut rusqlite::Connection, uid: i64, folder_id: i64, folder_path: &str) -> (usize, usize) {
    let now = chrono::Utc::now().timestamp();
    let mut found = 0usize;
    let mut errs = 0usize;
    let art_dir = art_cache_dir();
    let _ = std::fs::create_dir_all(&art_dir);

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
                // Extract + cache embedded art. Key by (artist|album)
                // so the same album only stores one cover regardless of
                // how many tracks carry it embedded. Folder-level
                // fallback (cover.jpg / folder.jpg) kicks in at serve
                // time for tracks without embedded art.
                let art_key: Option<String> = {
                    let pic = extract_embedded_picture(p);
                    if let Some((bytes, mime)) = pic {
                        let key = art_key_for_album(meta.artist.as_deref(), meta.album.as_deref(), &bytes);
                        let ext = match mime.as_str() {
                            "image/png" => "png",
                            "image/webp" => "webp",
                            _ => "jpg",
                        };
                        let dest = art_dir.join(format!("{}.{}", key, ext));
                        if !dest.exists() {
                            let _ = std::fs::write(&dest, &bytes);
                        }
                        Some(format!("{}.{}", key, ext))
                    } else {
                        None
                    }
                };

                let res = tx.execute(
                    "INSERT INTO local_music_tracks
                        (user_id, folder_id, path, title, artist, album, duration_ms, track_no, year, indexed_at,
                         original_title, original_artist, original_album, art_cache_key, bit_depth, sample_rate)
                     VALUES (?,?,?,?,?,?,?,?,?,?, ?,?,?,?,?,?)
                     ON CONFLICT (user_id, path) DO UPDATE SET
                        folder_id = excluded.folder_id,
                        duration_ms = excluded.duration_ms,
                        track_no = excluded.track_no,
                        year = excluded.year,
                        indexed_at = excluded.indexed_at,
                        art_cache_key = COALESCE(excluded.art_cache_key, local_music_tracks.art_cache_key),
                        bit_depth = excluded.bit_depth,
                        sample_rate = excluded.sample_rate,
                        -- Preserve user edits: only refresh title/artist/album from the file
                        -- when metadata_source is 'file_tags' (never user_edit / llm / MB).
                        title  = CASE WHEN local_music_tracks.metadata_source = 'file_tags' THEN excluded.title  ELSE local_music_tracks.title  END,
                        artist = CASE WHEN local_music_tracks.metadata_source = 'file_tags' THEN excluded.artist ELSE local_music_tracks.artist END,
                        album  = CASE WHEN local_music_tracks.metadata_source = 'file_tags' THEN excluded.album  ELSE local_music_tracks.album  END,
                        original_title  = excluded.original_title,
                        original_artist = excluded.original_artist,
                        original_album  = excluded.original_album",
                    rusqlite::params![
                        uid, folder_id, p.to_string_lossy(),
                        meta.title, meta.artist, meta.album,
                        meta.duration_ms, meta.track_no, meta.year, now,
                        meta.title, meta.artist, meta.album,   // original_*
                        art_key, meta.bit_depth, meta.sample_rate,
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

/// Where we keep extracted album-art images. Lives under the Syntaur
/// data dir so it's persisted with the DB, backed up together, and
/// survives container restarts.
fn art_cache_dir() -> PathBuf {
    crate::resolve_data_dir().join("art")
}

/// Build a stable filename for a cached album image. Keyed by
/// (artist|album) when available so tracks from the same album share
/// one file. Falls back to a content hash when tags are missing.
fn art_key_for_album(artist: Option<&str>, album: Option<&str>, bytes: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let (Some(a), Some(b)) = (artist, album) {
        a.trim().to_lowercase().hash(&mut h);
        b.trim().to_lowercase().hash(&mut h);
        return format!("alb_{:016x}", h.finish());
    }
    // No tags — hash the picture bytes so identical images dedupe.
    bytes[..bytes.len().min(4096)].hash(&mut h);
    format!("img_{:016x}", h.finish())
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
                "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source, art_cache_key, bit_depth, sample_rate, favorite, play_count
                 FROM local_music_tracks
                 WHERE user_id = ? AND folder_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?3 OR LOWER(COALESCE(artist,'')) LIKE ?3 OR LOWER(COALESCE(album,'')) LIKE ?3)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?4 OFFSET ?5"
            } else {
                "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source, art_cache_key, bit_depth, sample_rate, favorite, play_count
                 FROM local_music_tracks
                 WHERE user_id = ? AND (LOWER(COALESCE(title,'')) LIKE ?2 OR LOWER(COALESCE(artist,'')) LIKE ?2 OR LOWER(COALESCE(album,'')) LIKE ?2)
                 ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
                 LIMIT ?3 OFFSET ?4"
            }
        } else if folder_filter.is_some() {
            "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source, art_cache_key, bit_depth, sample_rate, favorite, play_count
             FROM local_music_tracks
             WHERE user_id = ? AND folder_id = ?
             ORDER BY COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0), COALESCE(title,'')
             LIMIT ? OFFSET ?"
        } else {
            "SELECT id, title, artist, album, duration_ms, track_no, year, metadata_source, art_cache_key, bit_depth, sample_rate, favorite, play_count
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
                "has_art":  r.get::<_, Option<String>>(8)?.is_some(),
                "bit_depth": r.get::<_, Option<i64>>(9)?,
                "sample_rate": r.get::<_, Option<i64>>(10)?,
                "favorite": r.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
                "play_count": r.get::<_, Option<i64>>(12)?.unwrap_or(0),
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

// ── Album art serving ────────────────────────────────────────────────
// Three-tier fallback chain:
//   1. art_cache_key → /config/art/<key>.<ext>  (embedded at scan time)
//   2. folder.jpg / cover.jpg / front.jpg / albumart.jpg / album.jpg
//      in the track's directory (case-insensitive, JPG/PNG)
//   3. MusicBrainz Cover Art Archive via the track's mbid
//      → https://coverartarchive.org/release/<mbid>/front-500
// Response cached 24h so the viewer doesn't re-pull on every render.

const FOLDER_ART_NAMES: &[&str] = &[
    "folder.jpg","folder.jpeg","folder.png",
    "cover.jpg","cover.jpeg","cover.png","cover.webp",
    "front.jpg","front.jpeg","front.png",
    "albumart.jpg","albumart.jpeg","albumart.png",
    "album.jpg","album.jpeg","album.png",
    "artwork.jpg","artwork.jpeg","artwork.png",
];

pub async fn serve_art(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Response<Body>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let row: Option<(Option<String>, String, Option<String>)> =
        tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
            let conn = rusqlite::Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT art_cache_key, path, mbid \
                 FROM local_music_tracks WHERE id = ? AND user_id = ?"
            )?;
            Ok(stmt.query_row(rusqlite::params![id, uid], |r| Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))).ok())
        }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
          .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (art_cache_key, track_path, mbid) = row.ok_or(StatusCode::NOT_FOUND)?;

    // Tier 1: cached file.
    if let Some(key) = art_cache_key {
        let cached = art_cache_dir().join(&key);
        if let Ok(bytes) = tokio::fs::read(&cached).await {
            let ct = match key.rsplit('.').next().unwrap_or("jpg") {
                "png" => "image/png",
                "webp" => "image/webp",
                _ => "image/jpeg",
            };
            return image_response(bytes, ct);
        }
    }

    // Tier 2: folder scan.
    if let Some(folder) = PathBuf::from(&track_path).parent() {
        if let Ok(entries) = std::fs::read_dir(folder) {
            for e in entries.flatten() {
                let Some(n) = e.file_name().to_str().map(|s| s.to_lowercase()) else { continue };
                if FOLDER_ART_NAMES.contains(&n.as_str()) {
                    if let Ok(bytes) = tokio::fs::read(e.path()).await {
                        let ct = if n.ends_with(".png") { "image/png" }
                                 else if n.ends_with(".webp") { "image/webp" }
                                 else { "image/jpeg" };
                        // Cache for next time.
                        let key = art_key_for_album(None, None, &bytes);
                        let dest = art_cache_dir().join(format!("{}.{}", key,
                            ct.strip_prefix("image/").unwrap_or("jpg")));
                        let _ = tokio::fs::create_dir_all(art_cache_dir()).await;
                        let _ = tokio::fs::write(&dest, &bytes).await;
                        let dbp = state.db_path.clone();
                        let kfile = dest.file_name().unwrap().to_string_lossy().to_string();
                        let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
                            let conn = rusqlite::Connection::open(&dbp)?;
                            conn.execute(
                                "UPDATE local_music_tracks SET art_cache_key = ? WHERE id = ? AND user_id = ?",
                                rusqlite::params![kfile, id, uid],
                            )?;
                            Ok(())
                        }).await;
                        return image_response(bytes, ct);
                    }
                }
            }
        }
    }

    // Tier 3: MusicBrainz Cover Art Archive (needs mbid from user
    // confirming a match).
    if let Some(mbid_val) = mbid.filter(|s| !s.is_empty()) {
        let url = format!("https://coverartarchive.org/release/{}/front-500", mbid_val);
        if let Ok(resp) = state.client.get(&url)
            .timeout(std::time::Duration::from_secs(8))
            .send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await {
                    let cached_key = cache_fetched_art(state.db_path.clone(), id, uid, &bytes, "image/jpeg").await;
                    let _ = cached_key;
                    return image_response(bytes.to_vec(), "image/jpeg");
                }
            }
        }
    }

    // Tier 4: iTunes Search API. Free, no key, high hit rate for
    // commercial music. Query by "artist album" and take the first
    // result's artworkUrl100 → swap in 600x600. Covers the case where
    // a track has no embedded image AND no folder.jpg AND no mbid.
    let (title_opt, artist_opt, album_opt): (Option<String>, Option<String>, Option<String>) = {
        let db2 = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<(Option<String>, Option<String>, Option<String>)> {
            let conn = rusqlite::Connection::open(&db2)?;
            let mut stmt = conn.prepare(
                "SELECT title, artist, album FROM local_music_tracks WHERE id = ? AND user_id = ?"
            )?;
            Ok(stmt.query_row(rusqlite::params![id, uid], |r| Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))).unwrap_or((None, None, None)))
        }).await.unwrap_or(Ok((None, None, None))).unwrap_or((None, None, None))
    };

    let artist = artist_opt.as_deref().unwrap_or("");
    let album_or_title = album_opt.as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| title_opt.as_deref())
        .unwrap_or("");
    if !artist.is_empty() || !album_or_title.is_empty() {
        let term = format!("{} {}", artist, album_or_title).trim().to_string();
        let entity = if album_opt.as_deref().filter(|s| !s.is_empty()).is_some() { "album" } else { "song" };
        if !term.is_empty() {
            if let Ok(resp) = state.client.get("https://itunes.apple.com/search")
                .query(&[("term", term.as_str()), ("entity", entity), ("limit", "1"), ("media", "music")])
                .timeout(std::time::Duration::from_secs(6))
                .send().await {
                if resp.status().is_success() {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let art_url = j.get("results")
                            .and_then(|v| v.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|first| first.get("artworkUrl100").or_else(|| first.get("artworkUrl60")))
                            .and_then(|v| v.as_str())
                            // Upgrade 100×100 → 600×600 — iTunes serves it from the same URL pattern.
                            .map(|s| s.replace("100x100bb", "600x600bb").replace("100x100-75", "600x600-75"));
                        if let Some(url) = art_url {
                            if let Ok(imgresp) = state.client.get(&url)
                                .timeout(std::time::Duration::from_secs(6))
                                .send().await {
                                if imgresp.status().is_success() {
                                    if let Ok(bytes) = imgresp.bytes().await {
                                        let _ = cache_fetched_art(state.db_path.clone(), id, uid, &bytes, "image/jpeg").await;
                                        return image_response(bytes.to_vec(), "image/jpeg");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
}

/// After fetching art from a remote source (MB / iTunes), persist it
/// under /config/art/<key>.jpg and point this track's art_cache_key
/// at it so future requests don't re-hit the network. Keyed by
/// (artist|album) so the whole album dedupes, not per-track.
async fn cache_fetched_art(db: PathBuf, track_id: i64, uid: i64, bytes: &[u8], mime: &str) -> Option<String> {
    let dir = art_cache_dir();
    let _ = tokio::fs::create_dir_all(&dir).await;
    let (artist, album): (Option<String>, Option<String>) = {
        let dbc = db.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<(Option<String>, Option<String>)> {
            let conn = rusqlite::Connection::open(&dbc)?;
            let mut stmt = conn.prepare(
                "SELECT artist, album FROM local_music_tracks WHERE id = ? AND user_id = ?"
            )?;
            Ok(stmt.query_row(rusqlite::params![track_id, uid], |r| Ok((
                r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
            ))).unwrap_or((None, None)))
        }).await.ok().and_then(|r| r.ok()).unwrap_or((None, None))
    };
    let key = art_key_for_album(artist.as_deref(), album.as_deref(), bytes);
    let ext = match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let file = format!("{}.{}", key, ext);
    let dest = dir.join(&file);
    if tokio::fs::write(&dest, bytes).await.is_err() {
        return None;
    }
    // Point THIS track + every same-album track at the cached file.
    let dbp = db.clone();
    let k = file.clone();
    let ar = artist.clone();
    let al = album.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&dbp)?;
        if let (Some(a), Some(b)) = (ar.as_deref(), al.as_deref()) {
            conn.execute(
                "UPDATE local_music_tracks SET art_cache_key = ? \
                 WHERE user_id = ? AND COALESCE(artist,'') = ? AND COALESCE(album,'') = ? AND art_cache_key IS NULL",
                rusqlite::params![k, uid, a, b],
            )?;
        } else {
            conn.execute(
                "UPDATE local_music_tracks SET art_cache_key = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![k, track_id, uid],
            )?;
        }
        Ok(())
    }).await;
    Some(file)
}

fn image_response(bytes: Vec<u8>, content_type: &'static str) -> Result<Response<Body>, StatusCode> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// ── Revert + favorite + record-play ──────────────────────────────────

pub async fn revert_to_original(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let n: usize = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "UPDATE local_music_tracks \
             SET title = original_title, \
                 artist = original_artist, \
                 album = original_album, \
                 metadata_source = 'file_tags', \
                 mbid = NULL \
             WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
        )
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if n == 0 { return Err(StatusCode::NOT_FOUND); }
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct FavoriteBody {
    pub token: Option<String>,
    pub favorite: bool,
}

pub async fn set_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<FavoriteBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let fav = if body.favorite { 1 } else { 0 };
    let n: usize = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "UPDATE local_music_tracks SET favorite = ? WHERE id = ? AND user_id = ?",
            rusqlite::params![fav, id, uid],
        )
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if n == 0 { return Err(StatusCode::NOT_FOUND); }
    Ok(Json(json!({"ok": true, "favorite": body.favorite})))
}

pub async fn record_play(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "UPDATE local_music_tracks \
             SET play_count = play_count + 1, last_played_at = ? \
             WHERE id = ? AND user_id = ?",
            rusqlite::params![now, id, uid],
        )?;
        Ok(())
    }).await;
    Ok(Json(json!({"ok": true})))
}

// ── Albums / Artists aggregates (T2 views) ───────────────────────────

pub async fn list_albums(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT \
               COALESCE(album,'')       AS album_name, \
               COALESCE(artist,'')      AS artist_name, \
               COUNT(*)                 AS track_count, \
               MIN(year)                AS year, \
               (SELECT id FROM local_music_tracks t2 \
                WHERE t2.user_id = ? AND COALESCE(t2.album,'') = COALESCE(t.album,'') \
                  AND COALESCE(t2.artist,'') = COALESCE(t.artist,'') \
                  AND t2.art_cache_key IS NOT NULL LIMIT 1) AS art_track_id, \
               (SELECT id FROM local_music_tracks t2 \
                WHERE t2.user_id = ? AND COALESCE(t2.album,'') = COALESCE(t.album,'') \
                  AND COALESCE(t2.artist,'') = COALESCE(t.artist,'') LIMIT 1) AS any_track_id \
             FROM local_music_tracks t \
             WHERE t.user_id = ? AND COALESCE(t.album,'') <> '' \
             GROUP BY COALESCE(t.album,''), COALESCE(t.artist,'') \
             ORDER BY COALESCE(t.artist,''), COALESCE(t.album,'')"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![uid, uid, uid], |r| Ok(json!({
            "album":       r.get::<_, String>(0)?,
            "artist":      r.get::<_, String>(1)?,
            "track_count": r.get::<_, i64>(2)?,
            "year":        r.get::<_, Option<i64>>(3)?,
            "art_track_id": r.get::<_, Option<i64>>(4)?.or(r.get::<_, Option<i64>>(5)?),
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"albums": rows})))
}

pub async fn list_artists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT \
               COALESCE(artist,'') AS name, \
               COUNT(DISTINCT COALESCE(album,'')) AS album_count, \
               COUNT(*) AS track_count \
             FROM local_music_tracks \
             WHERE user_id = ? AND COALESCE(artist,'') <> '' \
             GROUP BY COALESCE(artist,'') \
             ORDER BY COALESCE(artist,'')"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![uid], |r| Ok(json!({
            "name":        r.get::<_, String>(0)?,
            "album_count": r.get::<_, i64>(1)?,
            "track_count": r.get::<_, i64>(2)?,
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"artists": rows})))
}

// ── Playlists (manual) ──────────────────────────────────────────────

pub async fn list_playlists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.kind, \
                    (SELECT COUNT(*) FROM local_playlist_tracks pt WHERE pt.playlist_id = p.id) AS track_count, \
                    p.updated_at \
             FROM local_playlists p WHERE p.user_id = ? ORDER BY p.updated_at DESC"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![uid], |r| Ok(json!({
            "id":          r.get::<_, i64>(0)?,
            "name":        r.get::<_, String>(1)?,
            "kind":        r.get::<_, String>(2)?,
            "track_count": r.get::<_, i64>(3)?,
            "updated_at":  r.get::<_, i64>(4)?,
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"playlists": rows})))
}

#[derive(Deserialize)]
pub struct PlaylistCreateBody {
    pub token: Option<String>,
    pub name: String,
}

pub async fn create_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PlaylistCreateBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let name = body.name.trim().to_string();
    if name.is_empty() { return Err(StatusCode::BAD_REQUEST); }
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let id: i64 = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "INSERT INTO local_playlists (user_id, name, kind, created_at, updated_at) VALUES (?, ?, 'manual', ?, ?)",
            rusqlite::params![uid, name, now, now],
        )?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"id": id})))
}

#[derive(Deserialize)]
pub struct PlaylistAddBody {
    pub token: Option<String>,
    pub track_id: i64,
}

pub async fn playlist_add_track(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(pl_id): AxumPath<i64>,
    Json(body): Json<PlaylistAddBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let now = chrono::Utc::now().timestamp();
    let tid = body.track_id;
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db)?;
        // Verify playlist belongs to user
        let owned: i64 = conn.query_row(
            "SELECT COUNT(*) FROM local_playlists WHERE id = ? AND user_id = ?",
            rusqlite::params![pl_id, uid], |r| r.get(0),
        ).unwrap_or(0);
        if owned == 0 { return Ok(()); }
        let pos: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM local_playlist_tracks WHERE playlist_id = ?",
            rusqlite::params![pl_id], |r| r.get(0),
        ).unwrap_or(1);
        conn.execute(
            "INSERT INTO local_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?,?,?,?) \
             ON CONFLICT(playlist_id, track_id) DO NOTHING",
            rusqlite::params![pl_id, tid, pos, now],
        )?;
        conn.execute(
            "UPDATE local_playlists SET updated_at = ? WHERE id = ?",
            rusqlite::params![now, pl_id],
        )?;
        Ok(())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"ok": true})))
}

pub async fn get_playlist_tracks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(pl_id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title, t.artist, t.album, t.duration_ms, t.art_cache_key, t.bit_depth, t.sample_rate \
             FROM local_playlist_tracks pt \
             JOIN local_music_tracks t ON t.id = pt.track_id \
             JOIN local_playlists p ON p.id = pt.playlist_id \
             WHERE pt.playlist_id = ? AND p.user_id = ? AND t.user_id = ? \
             ORDER BY pt.position"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![pl_id, uid, uid], |r| Ok(json!({
            "id":          r.get::<_, i64>(0)?,
            "title":       r.get::<_, Option<String>>(1)?,
            "artist":      r.get::<_, Option<String>>(2)?,
            "album":       r.get::<_, Option<String>>(3)?,
            "duration_ms": r.get::<_, Option<i64>>(4)?,
            "has_art":     r.get::<_, Option<String>>(5)?.is_some(),
            "bit_depth":   r.get::<_, Option<i64>>(6)?,
            "sample_rate": r.get::<_, Option<i64>>(7)?,
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"tracks": rows})))
}

pub async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(pl_id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let n: usize = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute("DELETE FROM local_playlists WHERE id = ? AND user_id = ?", rusqlite::params![pl_id, uid])
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if n == 0 { return Err(StatusCode::NOT_FOUND); }
    Ok(Json(json!({"ok": true})))
}

// ── Lyrics via LRCLIB (T3) ───────────────────────────────────────────

pub async fn fetch_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();

    let db = state.db_path.clone();
    let row: Option<(Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>, Option<String>)> =
        tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
            let conn = rusqlite::Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT t.title, t.artist, t.album, t.duration_ms, l.plain_text, l.synced_lrc \
                 FROM local_music_tracks t \
                 LEFT JOIN local_lyrics_cache l ON l.track_id = t.id \
                 WHERE t.id = ? AND t.user_id = ?"
            )?;
            Ok(stmt.query_row(rusqlite::params![id, uid], |r| Ok((
                r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?,
            ))).ok())
        }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
          .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (title, artist, album, duration_ms, cached_plain, cached_synced) = row.ok_or(StatusCode::NOT_FOUND)?;

    if cached_plain.is_some() || cached_synced.is_some() {
        return Ok(Json(json!({
            "plain_text": cached_plain,
            "synced_lrc": cached_synced,
            "source": "cache",
        })));
    }

    let (t, a) = match (title.as_deref(), artist.as_deref()) {
        (Some(t), Some(a)) if !t.is_empty() && !a.is_empty() => (t, a),
        _ => return Ok(Json(json!({"plain_text": null, "synced_lrc": null, "reason": "no title+artist to query LRCLIB"}))),
    };

    let mut qb = vec![
        ("track_name", t.to_string()),
        ("artist_name", a.to_string()),
    ];
    if let Some(al) = album.as_deref() { if !al.is_empty() { qb.push(("album_name", al.to_string())); } }
    if let Some(d) = duration_ms { qb.push(("duration", (d / 1000).to_string())); }

    let resp = state.client.get("https://lrclib.net/api/get")
        .query(&qb)
        .header("User-Agent", format!("Syntaur/{} (music-lyrics)", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(8))
        .send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        let now = chrono::Utc::now().timestamp();
        let dbp = state.db_path.clone();
        let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let conn = rusqlite::Connection::open(&dbp)?;
            conn.execute(
                "INSERT INTO local_lyrics_cache (track_id, plain_text, synced_lrc, fetched_at) VALUES (?, NULL, NULL, ?)",
                rusqlite::params![id, now],
            )?;
            Ok(())
        }).await;
        return Ok(Json(json!({"plain_text": null, "synced_lrc": null, "reason": "LRCLIB had no match"})));
    }
    if !resp.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let j: Value = resp.json().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let plain = j.get("plainLyrics").and_then(|v| v.as_str()).map(|s| s.to_string());
    let synced = j.get("syncedLyrics").and_then(|v| v.as_str()).map(|s| s.to_string());
    let now = chrono::Utc::now().timestamp();
    let dbp = state.db_path.clone();
    let p2 = plain.clone();
    let s2 = synced.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&dbp)?;
        conn.execute(
            "INSERT INTO local_lyrics_cache (track_id, plain_text, synced_lrc, fetched_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(track_id) DO UPDATE SET plain_text = excluded.plain_text, synced_lrc = excluded.synced_lrc, fetched_at = excluded.fetched_at",
            rusqlite::params![id, p2, s2, now],
        )?;
        Ok(())
    }).await;
    let _ = uid; // quiet unused-var warnings from above closure env
    Ok(Json(json!({
        "plain_text": plain,
        "synced_lrc": synced,
        "source": "lrclib",
    })))
}

// ── Duplicates view (T3) ─────────────────────────────────────────────
// Same (title, artist) combination across multiple files. SQL does
// the heavy lift; the client renders the groups.

pub async fn list_duplicates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT LOWER(COALESCE(title,'')) AS t, LOWER(COALESCE(artist,'')) AS a, COUNT(*) AS n, \
                    GROUP_CONCAT(id) AS ids \
             FROM local_music_tracks \
             WHERE user_id = ? AND COALESCE(title,'') <> '' AND COALESCE(artist,'') <> '' \
             GROUP BY t, a HAVING COUNT(*) > 1 ORDER BY n DESC, a, t LIMIT 500"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![uid], |r| Ok(json!({
            "title":  r.get::<_, String>(0)?,
            "artist": r.get::<_, String>(1)?,
            "count":  r.get::<_, i64>(2)?,
            "ids":    r.get::<_, String>(3)?.split(',').filter_map(|s| s.parse::<i64>().ok()).collect::<Vec<_>>(),
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"groups": rows})))
}

// ── Natural-language library search (T4) ─────────────────────────────

#[derive(Deserialize)]
pub struct NLSearchBody {
    pub token: Option<String>,
    pub query: String,
}

pub async fn nl_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NLSearchBody>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, body.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let user_query = body.query.trim().to_string();
    if user_query.is_empty() { return Err(StatusCode::BAD_REQUEST); }

    // Grab a compact library summary for the LLM: top 40 artists + top 20 genres via inferred.
    let db = state.db_path.clone();
    let summary: String = tokio::task::spawn_blocking(move || -> rusqlite::Result<String> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut s = String::new();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(artist,'(unknown)'), COUNT(*) FROM local_music_tracks WHERE user_id = ? GROUP BY COALESCE(artist,'(unknown)') ORDER BY 2 DESC LIMIT 40"
        )?;
        s.push_str("Top artists (name, track count):\n");
        for r in stmt.query_map(rusqlite::params![uid], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?)))?.flatten() {
            s.push_str(&format!("  {} ({})\n", r.0, r.1));
        }
        Ok(s)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let sys = "You translate a user's natural-language music query into a strict JSON filter the server can execute. Output ONLY a JSON object, no prose, no markdown. Shape: {\"artist_contains\":\"...\",\"title_contains\":\"...\",\"album_contains\":\"...\",\"favorites_only\":bool,\"recently_played\":bool,\"most_played\":bool,\"limit\":integer}. Leave fields empty/false when the user didn't specify them. `limit` defaults to 30.";
    let user_msg = format!(
"User query: {:?}\n\nLibrary summary for context:\n{}\n\nReturn the JSON filter now.",
        user_query, summary);
    let messages = vec![
        crate::llm::ChatMessage::system(sys),
        crate::llm::ChatMessage::user(&user_msg),
    ];
    let reply = chain.call(&messages).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let cleaned = reply.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let filter: Value = serde_json::from_str(cleaned).unwrap_or_else(|_| json!({}));

    // Execute the filter against the DB — conservative clauses.
    let artist_q = filter.get("artist_contains").and_then(|v| v.as_str()).map(|s| s.to_lowercase());
    let title_q = filter.get("title_contains").and_then(|v| v.as_str()).map(|s| s.to_lowercase());
    let album_q = filter.get("album_contains").and_then(|v| v.as_str()).map(|s| s.to_lowercase());
    let favs_only = filter.get("favorites_only").and_then(|v| v.as_bool()).unwrap_or(false);
    let recent = filter.get("recently_played").and_then(|v| v.as_bool()).unwrap_or(false);
    let most = filter.get("most_played").and_then(|v| v.as_bool()).unwrap_or(false);
    let limit = filter.get("limit").and_then(|v| v.as_i64()).unwrap_or(30).clamp(1, 200);

    let db = state.db_path.clone();
    let rows: Vec<Value> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let order = if most { "play_count DESC" }
                    else if recent { "last_played_at DESC" }
                    else { "COALESCE(artist,''), COALESCE(album,''), COALESCE(track_no,0)" };
        let sql = format!(
            "SELECT id, title, artist, album, duration_ms, art_cache_key FROM local_music_tracks \
             WHERE user_id = ? \
               AND (?1 IS NULL OR LOWER(COALESCE(artist,'')) LIKE ?1) \
               AND (?2 IS NULL OR LOWER(COALESCE(title,'')) LIKE ?2) \
               AND (?3 IS NULL OR LOWER(COALESCE(album,'')) LIKE ?3) \
               AND (?4 = 0 OR favorite = 1) \
               AND (?5 = 0 OR last_played_at IS NOT NULL) \
             ORDER BY {} LIMIT ?6",
            order);
        let _ = sql;
        // Use simpler binding since placeholders ?1..?6 vs ? mix badly in rusqlite.
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, album, duration_ms, art_cache_key FROM local_music_tracks \
             WHERE user_id = ?1 \
               AND (?2 = '' OR LOWER(COALESCE(artist,'')) LIKE '%' || ?2 || '%') \
               AND (?3 = '' OR LOWER(COALESCE(title,'')) LIKE '%' || ?3 || '%') \
               AND (?4 = '' OR LOWER(COALESCE(album,'')) LIKE '%' || ?4 || '%') \
               AND (?5 = 0 OR favorite = 1) \
               AND (?6 = 0 OR last_played_at IS NOT NULL) \
             LIMIT ?7"
        )?;
        let out: Vec<Value> = stmt.query_map(rusqlite::params![
            uid,
            artist_q.as_deref().unwrap_or(""),
            title_q.as_deref().unwrap_or(""),
            album_q.as_deref().unwrap_or(""),
            if favs_only { 1 } else { 0 },
            if recent { 1 } else { 0 },
            limit,
        ], |r| Ok(json!({
            "id":          r.get::<_, i64>(0)?,
            "title":       r.get::<_, Option<String>>(1)?,
            "artist":      r.get::<_, Option<String>>(2)?,
            "album":       r.get::<_, Option<String>>(3)?,
            "duration_ms": r.get::<_, Option<i64>>(4)?,
            "has_art":     r.get::<_, Option<String>>(5)?.is_some(),
        })))?.filter_map(|x| x.ok()).collect();
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "interpretation": filter,
        "tracks": rows,
    })))
}

// ── Album liner notes (T4) ───────────────────────────────────────────

#[derive(Deserialize)]
pub struct LinerNotesQuery {
    pub token: Option<String>,
    pub artist: String,
    pub album: String,
    pub force: Option<bool>,
}

pub async fn album_liner_notes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LinerNotesQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let artist = q.artist.trim().to_string();
    let album = q.album.trim().to_string();
    if artist.is_empty() || album.is_empty() { return Err(StatusCode::BAD_REQUEST); }

    let db = state.db_path.clone();
    let a = artist.clone(); let al = album.clone();
    let cached: Option<String> = if !q.force.unwrap_or(false) {
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<String>> {
            let conn = rusqlite::Connection::open(&db)?;
            Ok(conn.query_row(
                "SELECT body FROM local_album_notes WHERE user_id = ? AND artist = ? AND album = ?",
                rusqlite::params![uid, a, al],
                |r| r.get::<_, String>(0)
            ).ok())
        }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
          .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else { None };
    if let Some(body) = cached { return Ok(Json(json!({"body": body, "source":"cache"}))); }

    // Pull the user's related artists so the "if you like this, try"
    // suggests real tracks the user actually owns.
    let db = state.db_path.clone();
    let user_artists: Vec<String> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<String>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(artist,'') FROM local_music_tracks WHERE user_id = ? AND COALESCE(artist,'') <> '' GROUP BY COALESCE(artist,'') ORDER BY COUNT(*) DESC LIMIT 80"
        )?;
        let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| r.get::<_, String>(0))?.filter_map(|x| x.ok()).collect();
        Ok(rows)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let user_lib = user_artists.join(", ");

    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let sys = "You are a music writer. Given an album + artist, write a 2-paragraph appreciation: (1) what makes this album matter — era, sound, influence; (2) a brief 'if you like this, try' suggestion that picks 2-3 artists from the user's own library that share a musical DNA with this one. Plain prose, no markdown headers, no hype. If the album is obscure or you're unsure, say so explicitly instead of inventing facts.";
    let user_msg = format!(
        "Album: {}\nArtist: {}\n\nArtists the user actually owns (pick from these for the if-you-like section):\n{}",
        album, artist, user_lib);
    let messages = vec![
        crate::llm::ChatMessage::system(sys),
        crate::llm::ChatMessage::user(&user_msg),
    ];
    let body = chain.call(&messages).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let body2 = body.clone();
    let a2 = artist.clone(); let al2 = album.clone();
    let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "INSERT INTO local_album_notes (user_id, artist, album, body, generated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, artist, album) DO UPDATE SET body = excluded.body, generated_at = excluded.generated_at",
            rusqlite::params![uid, a2, al2, body2, now],
        )?;
        Ok(())
    }).await;
    Ok(Json(json!({"body": body, "source": "llm"})))
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

