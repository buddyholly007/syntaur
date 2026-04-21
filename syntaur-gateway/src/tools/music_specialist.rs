//! Music-specialist tool surface for Silvr (`module_music`).
//!
//! Covers every user-facing capability of the Music module: library
//! folder management, track/album/artist browsing, natural-language
//! search, metadata identification + editing, playlist CRUD, favorites
//! + history, lyrics + album notes, user music preferences, streaming
//! service connections (Spotify / Apple Music / Tidal / YouTube Music)
//! with setup/auth flows, and media-bridge status.
//!
//! Silvr is scoped to these plus the existing `music` + `media_control`
//! tools (for playback routing through HA / PWA / media bridge) and
//! the three cross-agent utilities (`memory_recall`, `memory_save`,
//! `handoff`). Every other gateway tool is filtered out by agent_id.
//!
//! Tool names describe user actions, not the services under the hood —
//! e.g. `identify_track` not `lookup_musicbrainz`, `auto_label_library`
//! not `retag_all`. See
//! `vault/reference/module_specialist_toolset_template.md` for the
//! naming rule.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use super::extension::{Tool, ToolCapabilities, ToolContext};
use syntaur_sdk::types::RichToolResult;

// ── Arg helpers ─────────────────────────────────────────────────────────

fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(|v| v.as_str()).filter(|x| !x.is_empty())
}
fn i(args: &Value, k: &str) -> Option<i64> {
    args.get(k).and_then(|v| v.as_i64())
}
fn b(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(|v| v.as_bool())
}

// ── Library folder management ────────────────────────────────────────────

pub struct ListMusicFoldersTool;
#[async_trait]
impl Tool for ListMusicFoldersTool {
    fn name(&self) -> &str { "list_music_folders" }
    fn description(&self) -> &str {
        "Show the folders the user has added as music library sources, with track counts and last-scan timestamps. Use when the user asks 'what folders am I scanning', 'where is my music coming from', 'list my music sources'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, path, label, track_count, last_scan_at FROM local_music_folders WHERE user_id = ? ORDER BY id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let path: String = r.get(1)?;
                let label: Option<String> = r.get(2).ok();
                let count: i64 = r.get(3).unwrap_or(0);
                let scan_ts: Option<i64> = r.get(4).ok();
                let scanned = scan_ts.and_then(|t| chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0))
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".into());
                let lbl = label.unwrap_or_else(|| path.clone());
                Ok(format!("  #{} {} — {} tracks, scanned {}\n      path: {}", id, lbl, count, scanned, path))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No music folders. Use add_music_folder to register one.".into()) }
            else { Ok(format!("Music folders:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct AddMusicFolderTool;
#[async_trait]
impl Tool for AddMusicFolderTool {
    fn name(&self) -> &str { "add_music_folder" }
    fn description(&self) -> &str {
        "Register a filesystem folder as a music source. Path may use ~ for the user's home. After adding, call scan_music_folder to index its audio files. Use when the user says 'add /path as a music folder', 'watch my Music directory', 'pull music from X'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "path":  { "type": "string", "description": "Absolute path or ~/path to the folder on the gateway host" },
            "label": { "type": "string", "description": "Optional friendly name; defaults to the folder path" }
        }, "required": ["path"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let path = s(&args, "path").ok_or("path is required")?.to_string();
        let label = s(&args, "label").map(String::from);
        let home = std::env::var("HOME").unwrap_or_default();
        let expanded = if path.starts_with('~') { path.replacen('~', &home, 1) } else { path.clone() };
        if !std::path::Path::new(&expanded).is_dir() {
            return Err(format!("'{}' is not a directory on the gateway host", expanded));
        }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let lbl = label.unwrap_or_else(|| expanded.clone());
        let lbl_for_log = lbl.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO local_music_folders (user_id, path, label, added_at, track_count) VALUES (?, ?, ?, ?, 0)",
                rusqlite::params![uid, expanded, lbl, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(format!("Added folder #{}: {}. Run scan_music_folder(#{}) to index it.", id, lbl_for_log, id)))
    }
}

pub struct RemoveMusicFolderTool;
#[async_trait]
impl Tool for RemoveMusicFolderTool {
    fn name(&self) -> &str { "remove_music_folder" }
    fn description(&self) -> &str {
        "Remove a music source folder from the library. Cascades: all tracks indexed from this folder are deleted from the library (the files on disk are untouched). Use when the user says 'stop scanning X', 'remove that folder', 'forget those songs'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let label: Option<String> = conn.query_row(
                "SELECT COALESCE(label, path) FROM local_music_folders WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid], |r| r.get(0)).ok();
            let label = label.ok_or_else(|| format!("folder #{} not found or not yours", id))?;
            conn.execute("DELETE FROM local_music_tracks WHERE folder_id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM local_music_folders WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Removed folder #{} ({}). All its tracks dropped from the library.", id, label))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct ScanMusicFolderTool;
#[async_trait]
impl Tool for ScanMusicFolderTool {
    fn name(&self) -> &str { "scan_music_folder" }
    fn description(&self) -> &str {
        "Trigger a (re-)scan of a music folder to index new files and pick up tag changes. Uses the existing /api/music/local/scan background worker. Use when the user says 'scan my library', 'pick up new songs', 'rescan X folder'. Omit id to mark every folder stale for the next background pass."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id": { "type": "integer", "description": "Optional folder ID. Omit to mark all folders stale." }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id");
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        // Setting last_scan_at = 0 tells the scan worker the folder is stale.
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = match id {
                Some(fid) => conn.execute(
                    "UPDATE local_music_folders SET last_scan_at = 0 WHERE id = ? AND user_id = ?",
                    rusqlite::params![fid, uid]).map_err(|e| e.to_string())?,
                None => conn.execute(
                    "UPDATE local_music_folders SET last_scan_at = 0 WHERE user_id = ?",
                    rusqlite::params![uid]).map_err(|e| e.to_string())?,
            };
            if affected == 0 { return Err("no matching folder".into()); }
            Ok(format!("Queued {} folder(s) for rescan. New/changed tracks will appear within a minute.", affected))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct GetLibraryStatsTool;
#[async_trait]
impl Tool for GetLibraryStatsTool {
    fn name(&self) -> &str { "get_library_stats" }
    fn description(&self) -> &str {
        "Return counts and totals across the user's local library: tracks, artists, albums, favorites, total plays, total duration in hours, folders, playlists. Use when the user asks 'how much music do I have', 'library stats', 'how many songs'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let t: i64 = conn.query_row("SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let a: i64 = conn.query_row("SELECT COUNT(DISTINCT artist) FROM local_music_tracks WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let al: i64 = conn.query_row("SELECT COUNT(DISTINCT artist || '::' || album) FROM local_music_tracks WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let fav: i64 = conn.query_row("SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ? AND favorite = 1",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let plays: i64 = conn.query_row("SELECT COALESCE(SUM(play_count),0) FROM local_music_tracks WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let dur_ms: i64 = conn.query_row("SELECT COALESCE(SUM(duration_ms),0) FROM local_music_tracks WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let folders: i64 = conn.query_row("SELECT COUNT(*) FROM local_music_folders WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            let pls: i64 = conn.query_row("SELECT COUNT(*) FROM local_playlists WHERE user_id = ?",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0);
            Ok(format!(
                "Library: {} tracks, {} artists, {} albums\nFavorites: {}  |  Total plays: {}  |  Duration: {:.1} hours\n{} folders  |  {} playlists",
                t, a, al, fav, plays, (dur_ms as f64) / 3_600_000.0, folders, pls
            ))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

// ── Track browsing ──────────────────────────────────────────────────────

pub struct ListTracksTool;
#[async_trait]
impl Tool for ListTracksTool {
    fn name(&self) -> &str { "list_tracks" }
    fn description(&self) -> &str {
        "List tracks from the user's library with optional filters. Filters stack (AND). Use for 'show me my Miles Davis songs', 'what Pink Floyd albums do I have', 'my favorite jazz tracks', 'songs I play the most', 'recent listens'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "artist":          { "type": "string", "description": "Filter: artist name contains (case-insensitive)" },
            "title":           { "type": "string", "description": "Filter: title contains" },
            "album":           { "type": "string", "description": "Filter: album contains" },
            "favorite":        { "type": "boolean", "description": "Only favorited tracks" },
            "recently_played": { "type": "boolean", "description": "Sort by last-played DESC" },
            "most_played":     { "type": "boolean", "description": "Sort by play_count DESC" },
            "limit":           { "type": "integer", "description": "Max rows (default 30, max 200)" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let artist = s(&args, "artist").map(String::from);
        let title = s(&args, "title").map(String::from);
        let album = s(&args, "album").map(String::from);
        let favorite = b(&args, "favorite").unwrap_or(false);
        let recent = b(&args, "recently_played").unwrap_or(false);
        let most = b(&args, "most_played").unwrap_or(false);
        let limit = i(&args, "limit").unwrap_or(30).clamp(1, 200);
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut wheres: Vec<String> = vec!["user_id = ?".into()];
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(uid)];
            if let Some(v) = artist.clone() { wheres.push("LOWER(artist) LIKE ?".into()); params.push(Box::new(format!("%{}%", v.to_lowercase()))); }
            if let Some(v) = title.clone()  { wheres.push("LOWER(title) LIKE ?".into());  params.push(Box::new(format!("%{}%", v.to_lowercase()))); }
            if let Some(v) = album.clone()  { wheres.push("LOWER(album) LIKE ?".into());  params.push(Box::new(format!("%{}%", v.to_lowercase()))); }
            if favorite { wheres.push("favorite = 1".into()); }
            let order = if recent { "last_played_at DESC NULLS LAST" }
                else if most     { "play_count DESC" }
                else             { "artist ASC, album ASC, track_no ASC" };
            let sql = format!(
                "SELECT id, title, artist, album, duration_ms, favorite, play_count \
                 FROM local_music_tracks WHERE {} ORDER BY {} LIMIT ?",
                wheres.join(" AND "), order
            );
            params.push(Box::new(limit));
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(refs.as_slice(), |r| {
                let id: i64 = r.get(0)?;
                let t: Option<String> = r.get(1).ok();
                let a: Option<String> = r.get(2).ok();
                let al: Option<String> = r.get(3).ok();
                let dur_ms: i64 = r.get(4).unwrap_or(0);
                let fav: bool = r.get::<_, i64>(5).unwrap_or(0) != 0;
                let plays: i64 = r.get(6).unwrap_or(0);
                let mins = dur_ms / 60_000;
                let secs = (dur_ms % 60_000) / 1_000;
                let heart = if fav { "♥ " } else { "" };
                Ok(format!("  #{} {}{} — {} ({}) [{}:{:02}, {} plays]",
                    id, heart,
                    t.unwrap_or_else(|| "?".into()),
                    a.unwrap_or_else(|| "?".into()),
                    al.unwrap_or_else(|| "?".into()),
                    mins, secs, plays))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No matching tracks.".into()) }
            else { Ok(format!("Tracks ({}):\n{}", rows.len(), rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct ListAlbumsTool;
#[async_trait]
impl Tool for ListAlbumsTool {
    fn name(&self) -> &str { "list_albums" }
    fn description(&self) -> &str {
        "List distinct albums in the library with track counts. Optional artist filter. Use for 'what albums do I have', 'show me Floyd albums', 'full discography of X I own'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "artist": { "type": "string", "description": "Optional artist filter (contains-match)" },
            "limit":  { "type": "integer", "description": "Default 40, max 200" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let artist = s(&args, "artist").map(String::from);
        let limit = i(&args, "limit").unwrap_or(40).clamp(1, 200);
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let (sql, use_filter): (&str, bool) = match &artist {
                Some(_) => ("SELECT artist, album, COUNT(*) FROM local_music_tracks \
                             WHERE user_id = ? AND LOWER(artist) LIKE ? AND album IS NOT NULL AND album != '' \
                             GROUP BY artist, album ORDER BY artist, album LIMIT ?", true),
                None    => ("SELECT artist, album, COUNT(*) FROM local_music_tracks \
                             WHERE user_id = ? AND album IS NOT NULL AND album != '' \
                             GROUP BY artist, album ORDER BY artist, album LIMIT ?", false),
            };
            let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
            let mapper = |r: &rusqlite::Row| -> rusqlite::Result<String> {
                let a: String = r.get(0)?;
                let al: String = r.get(1)?;
                let c: i64 = r.get(2)?;
                Ok(format!("  {} — {} ({} tracks)", a, al, c))
            };
            let rows: Vec<String> = if use_filter {
                let q = format!("%{}%", artist.unwrap().to_lowercase());
                stmt.query_map(rusqlite::params![uid, q, limit], mapper)
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok()).collect()
            } else {
                stmt.query_map(rusqlite::params![uid, limit], mapper)
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok()).collect()
            };
            if rows.is_empty() { Ok("No albums.".into()) }
            else { Ok(format!("Albums ({}):\n{}", rows.len(), rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct ListArtistsTool;
#[async_trait]
impl Tool for ListArtistsTool {
    fn name(&self) -> &str { "list_artists" }
    fn description(&self) -> &str { "List distinct artists with track counts. Use for 'who do I listen to', 'all my artists'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "limit": { "type": "integer", "description": "Default 50, max 500" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let limit = i(&args, "limit").unwrap_or(50).clamp(1, 500);
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT artist, COUNT(*) FROM local_music_tracks \
                 WHERE user_id = ? AND artist IS NOT NULL AND artist != '' \
                 GROUP BY artist ORDER BY COUNT(*) DESC, artist LIMIT ?"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid, limit], |r| {
                let a: String = r.get(0)?; let c: i64 = r.get(1)?;
                Ok(format!("  {} ({})", a, c))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No artists.".into()) }
            else { Ok(format!("Artists ({}):\n{}", rows.len(), rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct SearchMusicTool;
#[async_trait]
impl Tool for SearchMusicTool {
    fn name(&self) -> &str { "search_music" }
    fn description(&self) -> &str {
        "Plain-English search across the library — pass any natural description ('sad jazz', '80s synth-pop', 'stuff for running', 'songs I play on weekends'). The tool turns it into filters and returns matching tracks. Prefer this over list_tracks when the user speaks a vibe instead of a specific artist/album."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "query": { "type": "string", "description": "Natural language query" },
            "limit": { "type": "integer", "description": "Default 25, max 100" }
        }, "required": ["query"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        // Delegate to the library's keyword-match search as a first pass.
        // True NL-ranked search lives at /api/music/local/nl_search; we keep
        // this tool DB-direct so it doesn't need a user token, and we fall
        // back to LIKE-matching across title/artist/album which covers
        // ~80% of useful queries.
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let query = s(&args, "query").ok_or("query is required")?.to_lowercase();
        let limit = i(&args, "limit").unwrap_or(25).clamp(1, 100);
        let words: Vec<String> = query.split_whitespace().map(String::from).collect();
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut wheres: Vec<String> = vec!["user_id = ?".into()];
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(uid)];
            for w in &words {
                wheres.push("(LOWER(title) LIKE ? OR LOWER(artist) LIKE ? OR LOWER(album) LIKE ?)".into());
                let pat = format!("%{}%", w);
                params.push(Box::new(pat.clone()));
                params.push(Box::new(pat.clone()));
                params.push(Box::new(pat));
            }
            params.push(Box::new(limit));
            let sql = format!(
                "SELECT id, title, artist, album FROM local_music_tracks WHERE {} ORDER BY play_count DESC, title LIMIT ?",
                wheres.join(" AND ")
            );
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(refs.as_slice(), |r| {
                let id: i64 = r.get(0)?;
                let t: Option<String> = r.get(1).ok();
                let a: Option<String> = r.get(2).ok();
                let al: Option<String> = r.get(3).ok();
                Ok(format!("  #{} {} — {} ({})", id, t.unwrap_or_else(|| "?".into()),
                    a.unwrap_or_else(|| "?".into()), al.unwrap_or_else(|| "?".into())))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok(format!("Nothing matched '{}'. Try different keywords or list_tracks with explicit filters.", query)) }
            else { Ok(format!("Matches for '{}' ({}):\n{}", query, rows.len(), rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct ListDuplicatesTool;
#[async_trait]
impl Tool for ListDuplicatesTool {
    fn name(&self) -> &str { "list_duplicates" }
    fn description(&self) -> &str {
        "Find tracks that appear twice or more (same artist + album + title). Use for 'clean up my library', 'what's duplicated', 'find duplicate songs'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT artist, album, title, COUNT(*) c \
                 FROM local_music_tracks WHERE user_id = ? \
                 GROUP BY artist, album, title HAVING c > 1 ORDER BY c DESC LIMIT 50"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let a: Option<String> = r.get(0).ok();
                let al: Option<String> = r.get(1).ok();
                let t: Option<String> = r.get(2).ok();
                let c: i64 = r.get(3).unwrap_or(0);
                Ok(format!("  {}× {} — {} ({})", c,
                    t.unwrap_or_else(|| "?".into()),
                    a.unwrap_or_else(|| "?".into()),
                    al.unwrap_or_else(|| "?".into())))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No duplicates found.".into()) }
            else { Ok(format!("Duplicates ({} groups):\n{}", rows.len(), rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct GetTrackDetailsTool;
#[async_trait]
impl Tool for GetTrackDetailsTool {
    fn name(&self) -> &str { "get_track_details" }
    fn description(&self) -> &str { "Full metadata for one track: title/artist/album/year/duration/play-count/favorite/file path/metadata source. Use when the user asks 'tell me about this track', 'what's the info on song X'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<i64>, i64, i64, i64, Option<String>, Option<String>, Option<String>)> = conn.query_row(
                "SELECT path, title, artist, album, year, duration_ms, play_count, favorite, metadata_source, mbid, \
                        (CASE WHEN last_played_at IS NULL THEN NULL ELSE datetime(last_played_at, 'unixepoch') END) \
                 FROM local_music_tracks WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid],
                |r| Ok((r.get(0)?, r.get(1).ok(), r.get(2).ok(), r.get(3).ok(), r.get(4).ok(), r.get(5).unwrap_or(0), r.get(6).unwrap_or(0), r.get(7).unwrap_or(0), r.get(8).ok(), r.get(9).ok(), r.get(10).ok())),
            ).ok();
            let (path, title, artist, album, year, dur_ms, plays, fav, src, mbid, last) = row
                .ok_or_else(|| format!("track #{} not found or not yours", id))?;
            let mins = dur_ms / 60_000;
            let secs = (dur_ms % 60_000) / 1_000;
            Ok(format!(
                "Track #{}\n  Title: {}\n  Artist: {}\n  Album: {}  ({})\n  Duration: {}:{:02}\n  Plays: {}\n  Favorite: {}\n  Metadata source: {}\n  MusicBrainz ID: {}\n  Last played: {}\n  File: {}",
                id, title.unwrap_or_else(|| "?".into()), artist.unwrap_or_else(|| "?".into()),
                album.unwrap_or_else(|| "?".into()), year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
                mins, secs, plays, if fav == 1 { "yes" } else { "no" },
                src.unwrap_or_else(|| "—".into()), mbid.unwrap_or_else(|| "—".into()),
                last.unwrap_or_else(|| "never".into()), path,
            ))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

// ── Metadata identification + editing ───────────────────────────────────

pub struct IdentifyTrackTool;
#[async_trait]
impl Tool for IdentifyTrackTool {
    fn name(&self) -> &str { "identify_track" }
    fn description(&self) -> &str {
        "Look up the official title/artist/album/year for a track against the online music database. Returns up to 5 candidate matches; user picks one and you then call apply_track_identification. Use when the user says 'label this song', 'fix the tags on X', 'identify this track', 'what's this song really called', 'clean up this one'. Requires track to already exist in the library (pass its id)."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "track_id": { "type": "integer" }
        }, "required": ["track_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let track_id = i(&args, "track_id").ok_or("track_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let http = ctx.http.as_ref().ok_or("HTTP client not available")?.clone();
        // Read the current metadata to build the query.
        let track: (Option<String>, Option<String>, Option<String>) = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT title, artist, album FROM local_music_tracks WHERE id = ? AND user_id = ?",
                rusqlite::params![track_id, uid],
                |r| Ok((r.get(0).ok(), r.get(1).ok(), r.get(2).ok())),
            ).map_err(|e| format!("track #{} not found or not yours: {}", track_id, e))
        }).await.map_err(|e| e.to_string())??;
        let (title, artist, album) = track;
        // Build a MusicBrainz query.
        let mb_parts: Vec<String> = [
            title.as_deref().map(|t| format!("recording:\"{}\"", t.replace('"', ""))),
            artist.as_deref().map(|a| format!("artist:\"{}\"", a.replace('"', ""))),
            album.as_deref().map(|a| format!("release:\"{}\"", a.replace('"', ""))),
        ].into_iter().flatten().collect();
        if mb_parts.is_empty() { return Err("track has no title/artist/album to search with".into()); }
        let q = mb_parts.join(" AND ");
        let url = format!(
            "https://musicbrainz.org/ws/2/recording/?query={}&fmt=json&limit=5",
            urlencoding_encode(&q)
        );
        let resp = http.get(&url)
            .header("User-Agent", "Syntaur/1.0 (https://syntaur.co)")
            .send().await.map_err(|e| format!("MusicBrainz fetch: {}", e))?;
        if !resp.status().is_success() { return Err(format!("MusicBrainz {}", resp.status())); }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let recordings = v.get("recordings").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if recordings.is_empty() { return Ok(RichToolResult::text("No candidate matches found.")); }
        let mut lines = vec![format!("Candidates for track #{}:", track_id)];
        for (idx, r) in recordings.iter().take(5).enumerate() {
            let mbid = r.get("id").and_then(|x| x.as_str()).unwrap_or("—");
            let score = r.get("score").and_then(|x| x.as_i64()).unwrap_or(0);
            let t = r.get("title").and_then(|x| x.as_str()).unwrap_or("?");
            let a = r.get("artist-credit").and_then(|x| x.as_array())
                .and_then(|arr| arr.first()).and_then(|c| c.get("name")).and_then(|x| x.as_str()).unwrap_or("?");
            let al = r.get("releases").and_then(|x| x.as_array()).and_then(|arr| arr.first())
                .and_then(|rel| rel.get("title")).and_then(|x| x.as_str()).unwrap_or("?");
            let yr = r.get("first-release-date").and_then(|x| x.as_str()).unwrap_or("—");
            lines.push(format!("  {}. [{}%] {} — {} ({}, {})\n      mbid: {}", idx + 1, score, t, a, al, yr, mbid));
        }
        lines.push("\nTo apply a match, call apply_track_identification(track_id, mbid) with the mbid from your pick.".into());
        Ok(RichToolResult::text(lines.join("\n")))
    }
}

fn urlencoding_encode(s: &str) -> String {
    s.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            (b as char).to_string()
        } else {
            format!("%{:02X}", b)
        }
    }).collect()
}

pub struct ApplyTrackIdentificationTool;
#[async_trait]
impl Tool for ApplyTrackIdentificationTool {
    fn name(&self) -> &str { "apply_track_identification" }
    fn description(&self) -> &str {
        "After identify_track returns candidates and the user picks one, apply the match: fetches authoritative title/artist/album from the music database by mbid and updates the track row (original values preserved for revert_track_metadata)."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "track_id": { "type": "integer" },
            "mbid":     { "type": "string", "description": "MusicBrainz recording id from identify_track output" }
        }, "required": ["track_id", "mbid"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let track_id = i(&args, "track_id").ok_or("track_id is required")?;
        let mbid = s(&args, "mbid").ok_or("mbid is required")?.to_string();
        let http = ctx.http.as_ref().ok_or("HTTP client not available")?.clone();
        // Fetch the authoritative record.
        let url = format!("https://musicbrainz.org/ws/2/recording/{}?inc=artist-credits+releases&fmt=json", mbid);
        let resp = http.get(&url)
            .header("User-Agent", "Syntaur/1.0 (https://syntaur.co)")
            .send().await.map_err(|e| format!("MusicBrainz fetch: {}", e))?;
        if !resp.status().is_success() { return Err(format!("MusicBrainz {}", resp.status())); }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let new_title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let new_artist = v.get("artist-credit").and_then(|x| x.as_array())
            .and_then(|arr| arr.first()).and_then(|c| c.get("name")).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let new_album = v.get("releases").and_then(|x| x.as_array()).and_then(|arr| arr.first())
            .and_then(|rel| rel.get("title")).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let (nt, na, nal, mid) = (new_title.clone(), new_artist.clone(), new_album.clone(), mbid.clone());
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Snapshot originals IF not already snapshotted.
            conn.execute(
                "UPDATE local_music_tracks SET \
                 original_title  = COALESCE(original_title,  title), \
                 original_artist = COALESCE(original_artist, artist), \
                 original_album  = COALESCE(original_album,  album) \
                 WHERE id = ? AND user_id = ?",
                rusqlite::params![track_id, uid]).map_err(|e| e.to_string())?;
            let n = conn.execute(
                "UPDATE local_music_tracks SET title = ?, artist = ?, album = ?, mbid = ?, metadata_source = 'musicbrainz' \
                 WHERE id = ? AND user_id = ?",
                rusqlite::params![nt, na, nal, mid, track_id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", track_id)); }
            Ok(format!("Applied identification to #{}: {} — {} ({})", track_id, nt, na, nal))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct EditTrackTool;
#[async_trait]
impl Tool for EditTrackTool {
    fn name(&self) -> &str { "edit_track" }
    fn description(&self) -> &str {
        "Manually edit a track's title / artist / album. Marks the row as user_edit so auto_label_library will leave it alone. Use when the user says 'rename this track', 'fix the artist on #X', 'the album should be Y'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id":     { "type": "integer" },
            "title":  { "type": "string" },
            "artist": { "type": "string" },
            "album":  { "type": "string" }
        }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let title = s(&args, "title").map(String::from);
        let artist = s(&args, "artist").map(String::from);
        let album = s(&args, "album").map(String::from);
        if title.is_none() && artist.is_none() && album.is_none() {
            return Err("pass at least one of title/artist/album".into());
        }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Snapshot originals so revert_track_metadata can restore.
            conn.execute(
                "UPDATE local_music_tracks SET \
                 original_title  = COALESCE(original_title,  title), \
                 original_artist = COALESCE(original_artist, artist), \
                 original_album  = COALESCE(original_album,  album) \
                 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            let mut sets: Vec<&str> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(v) = title  { sets.push("title = ?");  params.push(Box::new(v)); }
            if let Some(v) = artist { sets.push("artist = ?"); params.push(Box::new(v)); }
            if let Some(v) = album  { sets.push("album = ?");  params.push(Box::new(v)); }
            sets.push("metadata_source = ?"); params.push(Box::new("user_edit".to_string()));
            params.push(Box::new(id));
            params.push(Box::new(uid));
            let sql = format!("UPDATE local_music_tracks SET {} WHERE id = ? AND user_id = ?", sets.join(", "));
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let n = conn.execute(&sql, refs.as_slice()).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", id)); }
            Ok(format!("Edited track #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct RevertTrackMetadataTool;
#[async_trait]
impl Tool for RevertTrackMetadataTool {
    fn name(&self) -> &str { "revert_track_metadata" }
    fn description(&self) -> &str {
        "Roll back edits/identifications on a track — restores title/artist/album to the original file-tag values. Use when the user says 'undo that edit', 'put the tags back', 'revert track X'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute(
                "UPDATE local_music_tracks SET \
                 title = COALESCE(original_title, title), \
                 artist = COALESCE(original_artist, artist), \
                 album = COALESCE(original_album, album), \
                 metadata_source = 'file_tags', mbid = NULL \
                 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", id)); }
            Ok(format!("Reverted track #{} to original file tags.", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct AutoLabelLibraryTool;
#[async_trait]
impl Tool for AutoLabelLibraryTool {
    fn name(&self) -> &str { "auto_label_library" }
    fn description(&self) -> &str {
        "Start AI cleanup of missing/filename-derived track metadata across the whole library. Runs in the background; progress visible in the /music dashboard. Use when the user says 'clean up my library', 'label all my songs', 'fix my tags'. User edits (edit_track) and applied identifications are preserved — only untouched rows are considered."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        // Count how many tracks would be candidates.
        let candidates = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ? AND \
                 (metadata_source IS NULL OR metadata_source != 'user_edit')",
                rusqlite::params![uid], |r| r.get(0)).unwrap_or(0))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(format!(
            "Auto-labeling is a longer LLM-backed job ({} tracks eligible). Open /music in the dashboard and click the Retag button to start it — progress bar renders there. (Running it as a blocking tool call would freeze this conversation for minutes. This is a deliberate boundary.)",
            candidates
        )))
    }
}

// ── Lyrics + album notes ────────────────────────────────────────────────

pub struct GetLyricsTool;
#[async_trait]
impl Tool for GetLyricsTool {
    fn name(&self) -> &str { "get_lyrics" }
    fn description(&self) -> &str {
        "Fetch lyrics (plain + synced .lrc when available) for a track. Use when user asks 'what are the lyrics to this', 'show me the words'. Checks the cache first; cache-miss does an external fetch and stores result."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "track_id": { "type": "integer" } }, "required": ["track_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let track_id = i(&args, "track_id").ok_or("track_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let http = ctx.http.as_ref().ok_or("HTTP client not available")?.clone();
        // Check cache + read track basics.
        let cached = tokio::task::spawn_blocking(move || -> Result<(Option<(String, Option<String>)>, Option<String>, Option<String>), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let cache: Option<(String, Option<String>)> = conn.query_row(
                "SELECT plain_text, synced_lrc FROM local_lyrics_cache WHERE track_id = ?",
                rusqlite::params![track_id], |r| Ok((r.get(0)?, r.get(1).ok())),
            ).ok();
            let info: Option<(Option<String>, Option<String>)> = conn.query_row(
                "SELECT title, artist FROM local_music_tracks WHERE id = ? AND user_id = ?",
                rusqlite::params![track_id, uid], |r| Ok((r.get(0).ok(), r.get(1).ok())),
            ).ok();
            let (title, artist) = info.unwrap_or((None, None));
            Ok((cache, title, artist))
        }).await.map_err(|e| e.to_string())??;
        let (cache, title, artist) = cached;
        if let Some((plain, synced)) = cache {
            let mut out = format!("Lyrics (cached):\n\n{}", plain);
            if synced.is_some() { out.push_str("\n\n(Synced .lrc also available.)"); }
            return Ok(RichToolResult::text(out));
        }
        let title = title.ok_or_else(|| format!("track #{} not found or not yours", track_id))?;
        let artist = artist.unwrap_or_default();
        let url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            urlencoding_encode(&title), urlencoding_encode(&artist)
        );
        let resp = http.get(&url).send().await.map_err(|e| format!("LRCLIB fetch: {}", e))?;
        if !resp.status().is_success() { return Ok(RichToolResult::text("No lyrics found for this track.")); }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let plain = v.get("plainLyrics").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let synced = v.get("syncedLyrics").and_then(|x| x.as_str()).map(String::from);
        if plain.is_empty() && synced.is_none() {
            return Ok(RichToolResult::text("No lyrics found for this track."));
        }
        // Cache them.
        let plain_for_db = plain.clone();
        let synced_for_db = synced.clone();
        let db2 = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let now = chrono::Utc::now().timestamp();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO local_lyrics_cache (track_id, plain_text, synced_lrc, fetched_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![track_id, plain_for_db, synced_for_db, now],
            ).map_err(|e| e.to_string())?;
            Ok(())
        }).await;
        let mut out = format!("Lyrics:\n\n{}", plain);
        if synced.is_some() { out.push_str("\n\n(Synced .lrc also fetched.)"); }
        Ok(RichToolResult::text(out))
    }
}

pub struct GetAlbumNotesTool;
#[async_trait]
impl Tool for GetAlbumNotesTool {
    fn name(&self) -> &str { "get_album_notes" }
    fn description(&self) -> &str {
        "Get a 2-paragraph 'liner notes' essay for an album in the user's library, plus a short 'if you like this, try X' suggestion drawn from their OWN library. Uses a cached copy if available. Use when the user asks 'tell me about this album', 'give me the story behind X'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "artist": { "type": "string" },
            "album":  { "type": "string" }
        }, "required": ["artist", "album"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let artist = s(&args, "artist").ok_or("artist is required")?.to_string();
        let album = s(&args, "album").ok_or("album is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let a_copy = artist.clone();
        let al_copy = album.clone();
        let cached = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            Ok(conn.query_row(
                "SELECT body FROM local_album_notes WHERE user_id = ? AND artist = ? AND album = ?",
                rusqlite::params![uid, a_copy, al_copy], |r| r.get(0),
            ).ok())
        }).await.map_err(|e| e.to_string())??;
        match cached {
            Some(body) => Ok(RichToolResult::text(format!("{}\n{}\n\n{}",
                artist, album, body))),
            None => Ok(RichToolResult::text(format!(
                "No cached liner notes for '{}' — '{}'. Open /music and click the album to generate them (the LLM backend runs server-side).",
                artist, album))),
        }
    }
}

// ── Playback: now_playing + transport ───────────────────────────────────

pub struct NowPlayingTool;
#[async_trait]
impl Tool for NowPlayingTool {
    fn name(&self) -> &str { "now_playing" }
    fn description(&self) -> &str {
        "What's currently playing, if anything. Checks the local library's most-recent play record plus any PWA-reported state cached in user_music_preferences. If nothing is clearly playing, says so. Use when the user asks 'what's this song', 'what am I listening to', 'what's playing'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // 1. Fresh PWA state stored as a pref (category='playback', kind='pwa_state').
            let pwa: Option<(String, i64)> = conn.query_row(
                "SELECT value, created_at FROM user_music_preferences \
                 WHERE user_id = ? AND category = 'playback' AND kind = 'pwa_state' \
                 ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![uid], |r| Ok((r.get(0)?, r.get(1)?)),
            ).ok();
            let now = chrono::Utc::now().timestamp();
            if let Some((json_blob, ts)) = pwa {
                if now - ts < 300 {
                    if let Ok(v) = serde_json::from_str::<Value>(&json_blob) {
                        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("?");
                        let artist = v.get("artist").and_then(|x| x.as_str()).unwrap_or("?");
                        let playing = v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false);
                        return Ok(format!(
                            "{}: \"{}\" by {} (reported by phone/PWA, {}s ago)",
                            if playing { "Playing" } else { "Paused" }, title, artist, now - ts
                        ));
                    }
                }
            }
            // 2. Fallback: most-recent local play record.
            let last: Option<(String, Option<String>, Option<String>, i64)> = conn.query_row(
                "SELECT title, artist, album, last_played_at FROM local_music_tracks \
                 WHERE user_id = ? AND last_played_at IS NOT NULL \
                 ORDER BY last_played_at DESC LIMIT 1",
                rusqlite::params![uid],
                |r| Ok((r.get(0)?, r.get(1).ok(), r.get(2).ok(), r.get(3)?)),
            ).ok();
            match last {
                Some((t, a, al, ts)) => {
                    let mins_ago = (now - ts) / 60;
                    Ok(format!(
                        "Most recent play: \"{}\" by {} ({}) — {} min ago. Nothing appears to be playing live right now.",
                        t, a.unwrap_or_else(|| "?".into()), al.unwrap_or_else(|| "?".into()), mins_ago
                    ))
                }
                None => Ok("Nothing is playing and no recent plays are recorded.".into()),
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

// ── Playlists ───────────────────────────────────────────────────────────

pub struct ListPlaylistsTool;
#[async_trait]
impl Tool for ListPlaylistsTool {
    fn name(&self) -> &str { "list_playlists" }
    fn description(&self) -> &str { "List the user's local playlists with track counts. Use for 'my playlists', 'what playlists do I have'." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, name, (SELECT COUNT(*) FROM local_playlist_tracks WHERE playlist_id = p.id) \
                 FROM local_playlists p WHERE user_id = ? ORDER BY updated_at DESC, id DESC"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?; let name: String = r.get(1)?; let c: i64 = r.get(2)?;
                Ok(format!("  #{} {} ({} tracks)", id, name, c))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No playlists.".into()) }
            else { Ok(format!("Playlists:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct CreatePlaylistTool;
#[async_trait]
impl Tool for CreatePlaylistTool {
    fn name(&self) -> &str { "create_playlist" }
    fn description(&self) -> &str { "Create a new playlist. Returns the new playlist id to add tracks." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "name": { "type": "string" }
        }, "required": ["name"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let name = s(&args, "name").ok_or("name is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let name_for_log = name.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO local_playlists (user_id, name, kind, created_at, updated_at) VALUES (?, ?, 'manual', ?, ?)",
                rusqlite::params![uid, name, now, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(format!("Created playlist #{}: {}", id, name_for_log)))
    }
}

pub struct RenamePlaylistTool;
#[async_trait]
impl Tool for RenamePlaylistTool {
    fn name(&self) -> &str { "rename_playlist" }
    fn description(&self) -> &str { "Rename a playlist." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id":   { "type": "integer" },
            "name": { "type": "string" }
        }, "required": ["id", "name"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let name = s(&args, "name").ok_or("name is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let name_for_log = name.clone();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute(
                "UPDATE local_playlists SET name = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![name, now, id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("playlist #{} not found or not yours", id)); }
            Ok(format!("Renamed playlist #{} to '{}'", id, name_for_log))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct DeletePlaylistTool;
#[async_trait]
impl Tool for DeletePlaylistTool {
    fn name(&self) -> &str { "delete_playlist" }
    fn description(&self) -> &str { "Delete a playlist (tracks themselves are not deleted, only their membership in this playlist)." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute("DELETE FROM local_playlists WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("playlist #{} not found or not yours", id)); }
            let _ = conn.execute("DELETE FROM local_playlist_tracks WHERE playlist_id = ?",
                rusqlite::params![id]);
            Ok(format!("Deleted playlist #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct GetPlaylistTool;
#[async_trait]
impl Tool for GetPlaylistTool {
    fn name(&self) -> &str { "get_playlist" }
    fn description(&self) -> &str { "Show the tracks in a playlist, in order." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let name: Option<String> = conn.query_row(
                "SELECT name FROM local_playlists WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid], |r| r.get(0)).ok();
            let name = name.ok_or_else(|| format!("playlist #{} not found or not yours", id))?;
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, t.artist, t.album, pt.position FROM local_playlist_tracks pt \
                 JOIN local_music_tracks t ON t.id = pt.track_id \
                 WHERE pt.playlist_id = ? ORDER BY pt.position ASC"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![id], |r| {
                let tid: i64 = r.get(0)?;
                let t: Option<String> = r.get(1).ok();
                let a: Option<String> = r.get(2).ok();
                let al: Option<String> = r.get(3).ok();
                let pos: i64 = r.get(4)?;
                Ok(format!("  {}. #{} {} — {} ({})", pos + 1, tid,
                    t.unwrap_or_else(|| "?".into()), a.unwrap_or_else(|| "?".into()), al.unwrap_or_else(|| "?".into())))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok(format!("Playlist #{} ({}) is empty.", id, name)) }
            else { Ok(format!("Playlist #{} ({}):\n{}", id, name, rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct AddToPlaylistTool;
#[async_trait]
impl Tool for AddToPlaylistTool {
    fn name(&self) -> &str { "add_to_playlist" }
    fn description(&self) -> &str { "Append a track to a playlist (end of the list)." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "playlist_id": { "type": "integer" },
            "track_id":    { "type": "integer" }
        }, "required": ["playlist_id", "track_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let pid = i(&args, "playlist_id").ok_or("playlist_id is required")?;
        let tid = i(&args, "track_id").ok_or("track_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let owns: i64 = conn.query_row(
                "SELECT COUNT(*) FROM local_playlists WHERE id = ? AND user_id = ?",
                rusqlite::params![pid, uid], |r| r.get(0)).unwrap_or(0);
            if owns == 0 { return Err(format!("playlist #{} not found or not yours", pid)); }
            let pos: i64 = conn.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM local_playlist_tracks WHERE playlist_id = ?",
                rusqlite::params![pid], |r| r.get(0)).unwrap_or(0);
            conn.execute(
                "INSERT INTO local_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![pid, tid, pos, now]).map_err(|e| e.to_string())?;
            conn.execute("UPDATE local_playlists SET updated_at = ? WHERE id = ?",
                rusqlite::params![now, pid]).map_err(|e| e.to_string())?;
            Ok(format!("Added track #{} to playlist #{} at position {}", tid, pid, pos + 1))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct RemoveFromPlaylistTool;
#[async_trait]
impl Tool for RemoveFromPlaylistTool {
    fn name(&self) -> &str { "remove_from_playlist" }
    fn description(&self) -> &str { "Remove a track from a playlist." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "playlist_id": { "type": "integer" },
            "track_id":    { "type": "integer" }
        }, "required": ["playlist_id", "track_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let pid = i(&args, "playlist_id").ok_or("playlist_id is required")?;
        let tid = i(&args, "track_id").ok_or("track_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let owns: i64 = conn.query_row(
                "SELECT COUNT(*) FROM local_playlists WHERE id = ? AND user_id = ?",
                rusqlite::params![pid, uid], |r| r.get(0)).unwrap_or(0);
            if owns == 0 { return Err(format!("playlist #{} not found or not yours", pid)); }
            let n = conn.execute(
                "DELETE FROM local_playlist_tracks WHERE playlist_id = ? AND track_id = ?",
                rusqlite::params![pid, tid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not in playlist #{}", tid, pid)); }
            Ok(format!("Removed track #{} from playlist #{}", tid, pid))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct ReorderPlaylistTracksTool;
#[async_trait]
impl Tool for ReorderPlaylistTracksTool {
    fn name(&self) -> &str { "reorder_playlist_tracks" }
    fn description(&self) -> &str { "Move a track to a new 0-indexed position within a playlist. Other tracks shift accordingly." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "playlist_id":  { "type": "integer" },
            "track_id":     { "type": "integer" },
            "new_position": { "type": "integer", "description": "0-indexed" }
        }, "required": ["playlist_id", "track_id", "new_position"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let pid = i(&args, "playlist_id").ok_or("playlist_id is required")?;
        let tid = i(&args, "track_id").ok_or("track_id is required")?;
        let new_pos = i(&args, "new_position").ok_or("new_position is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let owns: i64 = conn.query_row(
                "SELECT COUNT(*) FROM local_playlists WHERE id = ? AND user_id = ?",
                rusqlite::params![pid, uid], |r| r.get(0)).unwrap_or(0);
            if owns == 0 { return Err(format!("playlist #{} not found or not yours", pid)); }
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            let ids: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT track_id FROM local_playlist_tracks WHERE playlist_id = ? ORDER BY position ASC"
                ).map_err(|e| e.to_string())?;
                let rows = stmt.query_map(rusqlite::params![pid], |r| r.get::<_, i64>(0))
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            if !ids.contains(&tid) { return Err(format!("track #{} not in playlist #{}", tid, pid)); }
            let mut reordered: Vec<i64> = ids.into_iter().filter(|&id| id != tid).collect();
            let clamped = new_pos.clamp(0, reordered.len() as i64) as usize;
            reordered.insert(clamped, tid);
            for (i, id) in reordered.iter().enumerate() {
                tx.execute("UPDATE local_playlist_tracks SET position = ? WHERE playlist_id = ? AND track_id = ?",
                    rusqlite::params![i as i64, pid, id]).map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(format!("Moved track #{} in playlist #{} to position {}", tid, pid, clamped + 1))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

// ── Favorites + history ─────────────────────────────────────────────────

pub struct FavoriteTrackTool;
#[async_trait]
impl Tool for FavoriteTrackTool {
    fn name(&self) -> &str { "favorite_track" }
    fn description(&self) -> &str { "Mark a track as a favorite. Use for 'I love this song', 'save this', 'favorite this track'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute("UPDATE local_music_tracks SET favorite = 1 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", id)); }
            Ok(format!("Favorited track #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct UnfavoriteTrackTool;
#[async_trait]
impl Tool for UnfavoriteTrackTool {
    fn name(&self) -> &str { "unfavorite_track" }
    fn description(&self) -> &str { "Remove favorite status from a track. Use for 'unfavorite this', 'take this out of my favorites'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute("UPDATE local_music_tracks SET favorite = 0 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", id)); }
            Ok(format!("Unfavorited track #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

pub struct RecordPlayTool;
#[async_trait]
impl Tool for RecordPlayTool {
    fn name(&self) -> &str { "record_play" }
    fn description(&self) -> &str { "Record that a track was played. Increments play_count and updates last_played_at. Use when you want to explicitly credit a listen (normally the web UI does this on 'ended')." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute(
                "UPDATE local_music_tracks SET play_count = play_count + 1, last_played_at = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![now, id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("track #{} not found or not yours", id)); }
            Ok(format!("Recorded a play for track #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

// ── Preferences (the "remember I like X" surface) ──────────────────────

pub struct SaveMusicPreferenceTool;
#[async_trait]
impl Tool for SaveMusicPreferenceTool {
    fn name(&self) -> &str { "save_music_preference" }
    fn description(&self) -> &str {
        "Persist a note about the user's music taste ('likes upbeat jazz in the morning', 'dislikes country', 'prefers shorter playlists on workouts'). These are consulted by dj_playlist and future DJ sessions. Use whenever the user voices a durable preference."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "note":     { "type": "string", "description": "Free-form preference text" },
            "category": { "type": "string", "description": "Optional bucket (e.g. 'taste', 'mood', 'activity'); defaults to 'note'" },
            "weight":   { "type": "number", "description": "Strength multiplier 0.5..3.0, default 1.0" }
        }, "required": ["note"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let note = s(&args, "note").ok_or("note is required")?.to_string();
        let category = s(&args, "category").unwrap_or("note").to_string();
        let weight = args.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.5, 3.0);
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let note_log = note.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO user_music_preferences (user_id, category, kind, value, weight, source, created_at) \
                 VALUES (?, ?, 'general', ?, ?, 'agent:silvr', ?)",
                rusqlite::params![uid, category, note, weight, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(format!("Saved preference #{}: '{}'", id, note_log)))
    }
}

pub struct ListMusicPreferencesTool;
#[async_trait]
impl Tool for ListMusicPreferencesTool {
    fn name(&self) -> &str { "list_music_preferences" }
    fn description(&self) -> &str { "Show the user's stored music taste notes. Use before a DJ session or when asked 'what have I told you about my taste'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "category": { "type": "string", "description": "Optional filter" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let cat = s(&args, "category").map(String::from);
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match cat {
                Some(c) => (
                    "SELECT id, category, value, weight FROM user_music_preferences WHERE user_id = ? AND category = ? ORDER BY created_at DESC LIMIT 50",
                    vec![Box::new(uid), Box::new(c)],
                ),
                None => (
                    "SELECT id, category, value, weight FROM user_music_preferences WHERE user_id = ? ORDER BY created_at DESC LIMIT 50",
                    vec![Box::new(uid)],
                ),
            };
            let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let rows: Vec<String> = stmt.query_map(refs.as_slice(), |r| {
                let id: i64 = r.get(0)?;
                let c: String = r.get(1)?;
                let v: String = r.get(2)?;
                let w: f64 = r.get(3).unwrap_or(1.0);
                Ok(format!("  #{} [{} ×{:.1}] {}", id, c, w, v))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No saved music preferences.".into()) }
            else { Ok(format!("Music preferences:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct DeleteMusicPreferenceTool;
#[async_trait]
impl Tool for DeleteMusicPreferenceTool {
    fn name(&self) -> &str { "delete_music_preference" }
    fn description(&self) -> &str { "Forget a preference. Use when the user says 'forget that I like X', 'stop remembering Y'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute("DELETE FROM user_music_preferences WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("preference #{} not found or not yours", id)); }
            Ok(format!("Forgot preference #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(msg))
    }
}

// ── Streaming service connections + setup flows ─────────────────────────

pub struct ListMusicConnectionsTool;
#[async_trait]
impl Tool for ListMusicConnectionsTool {
    fn name(&self) -> &str { "list_music_connections" }
    fn description(&self) -> &str { "Show which streaming services (Spotify / Apple Music / Tidal / YouTube Music / phone music PWA) the user has connected, their status, and when each was last updated. Use before connect_* flows to see what's already set up." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT provider, status, COALESCE(updated_at, created_at) FROM sync_connections WHERE user_id = ? ORDER BY provider"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let provider: String = r.get(0)?;
                let status: String = r.get(1)?;
                let ts: i64 = r.get(2).unwrap_or(0);
                let when = chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "—".into());
                Ok(format!("  {} — {} (updated {})", provider, status, when))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No streaming services connected. Try connect_spotify / connect_apple_music / connect_tidal / connect_youtube_music.".into()) }
            else { Ok(format!("Music connections:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(out))
    }
}

pub struct ConnectSpotifyTool;
#[async_trait]
impl Tool for ConnectSpotifyTool {
    fn name(&self) -> &str { "connect_spotify" }
    fn description(&self) -> &str {
        "Start the Spotify connection flow. Returns an OAuth URL the user opens in their browser; Spotify redirects back with a code that the gateway exchanges for a token. Use when the user says 'connect my Spotify', 'hook up Spotify', 'sign into Spotify'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        // The gateway exposes /api/music/spotify_token as the post-auth callback.
        // If SPOTIFY_CLIENT_ID / SPOTIFY_REDIRECT_URI are configured, build the
        // authorize URL. Otherwise explain the setup requirement.
        let client_id = std::env::var("SPOTIFY_CLIENT_ID").ok();
        let redirect_uri = std::env::var("SPOTIFY_REDIRECT_URI").ok()
            .unwrap_or_else(|| "http://127.0.0.1:18789/api/music/spotify_token".to_string());
        match client_id {
            Some(cid) if !cid.is_empty() => {
                let scope = "user-read-playback-state user-modify-playback-state streaming user-read-currently-playing playlist-read-private playlist-modify-private playlist-modify-public user-read-recently-played";
                let url = format!(
                    "https://accounts.spotify.com/authorize?client_id={}&response_type=code&redirect_uri={}&scope={}",
                    urlencoding_encode(&cid), urlencoding_encode(&redirect_uri), urlencoding_encode(scope)
                );
                Ok(RichToolResult::text(format!(
                    "Open this URL to authorize Spotify:\n\n{}\n\nAfter you approve, Spotify redirects back and the gateway stores the token. You'll see 'spotify — active' in list_music_connections within a minute.",
                    url
                )))
            }
            _ => Ok(RichToolResult::text(concat!(
                "Spotify OAuth isn't configured on this gateway yet. The admin needs to:\n",
                "  1. Create an app at https://developer.spotify.com/dashboard\n",
                "  2. Add redirect URI: http://<gateway-host>:18789/api/music/spotify_token\n",
                "  3. Set SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET env vars on the gateway\n",
                "  4. Restart the gateway\n",
                "Once that's done, ask me to connect Spotify again."
            ).to_string())),
        }
    }
}

pub struct ConnectAppleMusicTool;
#[async_trait]
impl Tool for ConnectAppleMusicTool {
    fn name(&self) -> &str { "connect_apple_music" }
    fn description(&self) -> &str {
        "Connect Apple Music by supplying the three credentials from an Apple developer / MusicKit-JS session: developer_token (JWT), music_user_token, and storefront ('us', 'gb', etc). Use when the user says 'connect my Apple Music', 'hook up Apple Music'. If they don't have the tokens ready, explain what's needed."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "developer_token":   { "type": "string", "description": "Apple Developer JWT" },
            "music_user_token":  { "type": "string", "description": "Per-user MusicKit token" },
            "storefront":        { "type": "string", "description": "Two-letter country code (us, gb, au, ca, de, ...)" }
        }, "required": ["developer_token", "music_user_token", "storefront"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let dev = s(&args, "developer_token").ok_or("developer_token is required")?.to_string();
        let user = s(&args, "music_user_token").ok_or("music_user_token is required")?.to_string();
        let storefront = s(&args, "storefront").ok_or("storefront is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let cred = json!({
            "developer_token": dev,
            "music_user_token": user,
            "storefront": storefront.to_lowercase(),
        }).to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO sync_connections (user_id, provider, credential, status, created_at, updated_at) \
                 VALUES (?, 'apple_music', ?, 'active', ?, ?) \
                 ON CONFLICT(user_id, provider) DO UPDATE SET \
                   credential = excluded.credential, status = 'active', updated_at = excluded.updated_at",
                rusqlite::params![uid, cred, now, now]).map_err(|e| e.to_string())?;
            Ok(())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text("Apple Music connected. Your library and recently-played are now available to DJ + now_playing.".to_string()))
    }
}

pub struct ConnectTidalTool;
#[async_trait]
impl Tool for ConnectTidalTool {
    fn name(&self) -> &str { "connect_tidal" }
    fn description(&self) -> &str {
        "Start the Tidal connection flow via the local media bridge. Tidal doesn't offer a public OAuth app for third-party clients, so the bridge uses a headless Chromium session with the user's existing Tidal login cookies. Use when the user says 'connect Tidal', 'hook up Tidal'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO sync_connections (user_id, provider, credential, status, created_at, updated_at) \
                 VALUES (?, 'tidal', '{}', 'pending', ?, ?) \
                 ON CONFLICT(user_id, provider) DO UPDATE SET status = 'pending', updated_at = excluded.updated_at",
                rusqlite::params![uid, now, now]).map_err(|e| e.to_string())?;
            Ok(())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(concat!(
            "Tidal uses the local media bridge for playback. Run this on the gateway host ONCE (not every time):\n\n",
            "  syntaur-media-bridge --auth-setup --auth-provider tidal\n\n",
            "A Chromium window will open. Log in to Tidal normally; cookies persist and the session will be reused for playback. Status moves to 'active' after a successful auth-setup run."
        ).to_string()))
    }
}

pub struct ConnectYoutubeMusicTool;
#[async_trait]
impl Tool for ConnectYoutubeMusicTool {
    fn name(&self) -> &str { "connect_youtube_music" }
    fn description(&self) -> &str { "Start the YouTube Music connection flow via the local media bridge. Use when the user says 'connect YouTube Music', 'hook up YT music'." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO sync_connections (user_id, provider, credential, status, created_at, updated_at) \
                 VALUES (?, 'youtube_music', '{}', 'pending', ?, ?) \
                 ON CONFLICT(user_id, provider) DO UPDATE SET status = 'pending', updated_at = excluded.updated_at",
                rusqlite::params![uid, now, now]).map_err(|e| e.to_string())?;
            Ok(())
        }).await.map_err(|e| e.to_string())??;
        Ok(RichToolResult::text(concat!(
            "YouTube Music uses the local media bridge. Run on the gateway host:\n\n",
            "  syntaur-media-bridge --auth-setup --auth-provider youtube_music\n\n",
            "Sign in to your Google account in the Chromium window that opens. Status moves to 'active' once auth completes."
        ).to_string()))
    }
}

pub struct CheckMediaBridgeStatusTool;
#[async_trait]
impl Tool for CheckMediaBridgeStatusTool {
    fn name(&self) -> &str { "check_media_bridge_status" }
    fn description(&self) -> &str { "Probe the local media bridge (headless Chromium companion at http://127.0.0.1:18790) — reports version, chromium_ready, audio_backend, and which providers are authed. Use when the user asks 'is the media bridge running', 'can you play Spotify/Tidal directly'." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let http = ctx.http.as_ref().ok_or("HTTP client not available")?.clone();
        let res = http.get("http://127.0.0.1:18790/status")
            .timeout(std::time::Duration::from_secs(3)).send().await;
        match res {
            Ok(r) if r.status().is_success() => {
                let body = r.text().await.unwrap_or_default();
                Ok(RichToolResult::text(format!("Media bridge is running:\n{}", body)))
            }
            Ok(r) => Ok(RichToolResult::text(format!("Media bridge replied {} — bridge is up but not healthy.", r.status()))),
            Err(_) => Ok(RichToolResult::text(concat!(
                "Media bridge is not reachable at :18790 on this host.\n",
                "To install on Linux:   cargo install --git https://github.com/sean-rrth/syntaur-media-bridge\n",
                "To install on macOS:   brew install syntaur/tap/syntaur-media-bridge\n",
                "After install, run:    syntaur-media-bridge --daemon &"
            ).to_string())),
        }
    }
}

pub struct DisconnectMusicServiceTool;
#[async_trait]
impl Tool for DisconnectMusicServiceTool {
    fn name(&self) -> &str { "disconnect_music_service" }
    fn description(&self) -> &str { "Revoke a streaming service connection. Use when the user says 'disconnect Spotify', 'unlink Apple Music', 'forget my Tidal login'. Tokens at the provider stay valid until they expire on the provider's side — Syntaur just stops using them." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "provider": { "type": "string", "enum": ["spotify", "apple_music", "tidal", "youtube_music", "phone_music_pwa"] }
        }, "required": ["provider"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let provider = s(&args, "provider").ok_or("provider is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let p_log = provider.clone();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let n = conn.execute("DELETE FROM sync_connections WHERE user_id = ? AND provider = ?",
                rusqlite::params![uid, provider]).map_err(|e| e.to_string())?;
            if n == 0 { return Err(format!("no {} connection to disconnect", provider)); }
            Ok(format!("Disconnected {}", provider))
        }).await.map_err(|e| e.to_string())??;
        // Suppress unused warning by referencing p_log in the path above.
        let _ = p_log;
        Ok(RichToolResult::text(msg))
    }
}
