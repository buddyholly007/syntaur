//! play_music — routes music playback across connected sync providers.
//!
//! Apple Music audio is DRM-protected; we can't decrypt it server-side.
//! This tool instead routes commands to whichever client CAN decrypt:
//!
//! Priority order (uses first available):
//!   1. Home Assistant media_player (HomePod, Apple TV) — invisible/seamless
//!   2. Browser dashboard player (if user has dashboard open) via SSE event
//!   3. iOS Shortcut webhook — plays on user's phone
//!   4. Fallback: returns a music.apple.com URL and status=needs_client
//!
//! Search is always done server-side against api.music.apple.com so we
//! know which Apple Music ID to route. If Apple Music isn't connected,
//! we fall back to passing the raw query to the routing target (HA's
//! media_player.play_media, Shortcut webhook, etc.) and let it handle.

use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct MusicTool;

#[derive(Debug, Clone)]
struct SyncCreds {
    apple_music: Option<(String, String, String)>, // (dev_token, music_user_token, storefront)
    home_assistant: Option<(String, String)>,      // (url, token)
    ios_shortcut: Option<String>,                  // webhook url
    has_music_assistant: bool,
    has_phone_pwa: bool,
}

fn load_sync_creds(db_path: &std::path::Path, user_id: i64) -> SyncCreds {
    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(_) => return SyncCreds {
            apple_music: None, home_assistant: None, ios_shortcut: None,
            has_music_assistant: false,
        has_phone_pwa: false,
        },
    };
    let mut stmt = match conn.prepare(
        "SELECT provider, credential FROM sync_connections \
         WHERE user_id = ? AND status = 'active'"
    ) {
        Ok(s) => s,
        Err(_) => return SyncCreds {
            apple_music: None, home_assistant: None, ios_shortcut: None,
            has_music_assistant: false,
        has_phone_pwa: false,
        },
    };
    let rows = stmt.query_map(rusqlite::params![user_id], |r| Ok((
        r.get::<_, String>(0)?, r.get::<_, String>(1)?,
    ))).ok();

    let mut apple_music = None;
    let mut home_assistant = None;
    let mut ios_shortcut = None;
    let mut has_music_assistant = false;
    let mut has_phone_pwa = false;

    if let Some(rs) = rows {
        for row in rs.flatten() {
            let (provider, cred_json) = row;
            let cred: Value = serde_json::from_str(&cred_json).unwrap_or(Value::Null);
            match provider.as_str() {
                "apple_music" => {
                    let dev = cred.get("developer_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let mut_ = cred.get("music_user_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let sf = cred.get("storefront").and_then(|v| v.as_str()).unwrap_or("us").to_string();
                    if !dev.is_empty() && !mut_.is_empty() {
                        apple_music = Some((dev, mut_, sf));
                    }
                }
                "home_assistant" => {
                    let url = cred.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let tok = cred.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !url.is_empty() && !tok.is_empty() {
                        home_assistant = Some((url, tok));
                    }
                }
                "ios_shortcut_music" => {
                    let url = cred.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !url.is_empty() { ios_shortcut = Some(url); }
                }
                "music_assistant" => { has_music_assistant = true; }
                "phone_music_pwa" => { has_phone_pwa = true; }
                _ => {}
            }
        }
    }

    SyncCreds { apple_music, home_assistant, ios_shortcut, has_music_assistant, has_phone_pwa }
}

async fn apple_music_search_first(
    client: &Arc<reqwest::Client>,
    creds: &(String, String, String),
    query: &str,
) -> Result<Option<Value>, String> {
    let (dev, mut_, sf) = creds;
    let url = format!(
        "https://api.music.apple.com/v1/catalog/{}/search?types=songs&limit=1&term={}",
        sf,
        url_encode(query)
    );
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", dev))
        .header("Music-User-Token", mut_)
        .header("Origin", "https://music.apple.com")
        .timeout(std::time::Duration::from_secs(15))
        .send().await.map_err(|e| format!("Apple Music search: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Apple Music returned {}", resp.status()));
    }
    let j: Value = resp.json().await.map_err(|e| e.to_string())?;
    // results.songs.data[0]
    let song = j.get("results")
        .and_then(|r| r.get("songs"))
        .and_then(|s| s.get("data"))
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .cloned();
    Ok(song)
}

async fn find_ha_media_player(
    client: &Arc<reqwest::Client>,
    ha: &(String, String),
) -> Result<Option<String>, String> {
    // Query HA states, filter for media_player domain, prefer apple_tv / homepod
    let (url, token) = ha;
    let states_url = format!("{}/api/states", url.trim_end_matches('/'));
    let resp = client.get(&states_url)
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send().await.map_err(|e| format!("HA states: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HA returned {}", resp.status()));
    }
    let arr: Value = resp.json().await.map_err(|e| e.to_string())?;
    let Some(states) = arr.as_array() else { return Ok(None); };

    let mut candidates: Vec<(i32, String)> = Vec::new();
    for s in states {
        let entity_id = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
        if !entity_id.starts_with("media_player.") { continue; }
        let state = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
        if state == "unavailable" || state == "unknown" { continue; }
        let attrs = s.get("attributes").cloned().unwrap_or(Value::Null);
        let name = attrs.get("friendly_name").and_then(|v| v.as_str()).unwrap_or(entity_id);
        let lower = name.to_ascii_lowercase();
        let eid_lower = entity_id.to_ascii_lowercase();
        // Scoring: HomePod > Apple TV > Sonos > any other media_player
        let score: i32 =
            if lower.contains("homepod") || eid_lower.contains("homepod") { 100 }
            else if lower.contains("apple tv") || eid_lower.contains("apple_tv") { 90 }
            else if lower.contains("sonos") || eid_lower.contains("sonos") { 70 }
            else if attrs.get("supported_features").and_then(|v| v.as_i64()).unwrap_or(0) > 0 { 50 }
            else { 10 };
        candidates.push((score, entity_id.to_string()));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates.first().map(|c| c.1.clone()))
}

async fn ha_play_media(
    client: &Arc<reqwest::Client>,
    ha: &(String, String),
    entity_id: &str,
    media_content_id: &str,
    media_content_type: &str,
) -> Result<(), String> {
    let (url, token) = ha;
    let svc_url = format!("{}/api/services/media_player/play_media", url.trim_end_matches('/'));
    let body = json!({
        "entity_id": entity_id,
        "media_content_id": media_content_id,
        "media_content_type": media_content_type,
    });
    let resp = client.post(&svc_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send().await.map_err(|e| format!("HA play_media: {}", e))?;
    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HA play_media {}: {}", s, body.chars().take(200).collect::<String>()));
    }
    Ok(())
}

async fn trigger_ios_shortcut(
    client: &Arc<reqwest::Client>,
    url: &str,
    query: &str,
) -> Result<(), String> {
    // Append query as URL parameter — iOS Shortcut's "Get Contents of URL"
    // receives it as the Shortcut input
    let full_url = if url.contains('?') {
        format!("{}&input={}", url, url_encode(query))
    } else {
        format!("{}?input={}", url, url_encode(query))
    };
    let resp = client.get(&full_url)
        .timeout(std::time::Duration::from_secs(10))
        .send().await.map_err(|e| format!("Shortcut trigger: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Shortcut returned {}", resp.status()));
    }
    Ok(())
}

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            (b as char).to_string()
        } else {
            format!("%{:02X}", b)
        }
    }).collect()
}

#[async_trait]
impl Tool for MusicTool {
    fn name(&self) -> &str { "music" }

    fn description(&self) -> &str {
        "Play music on any connected speaker. Searches Apple Music for the requested \
         song/artist/playlist, then routes playback automatically through whichever \
         device is available: HomePod/Apple TV (via Home Assistant), browser dashboard \
         player, or iPhone (via Shortcut). Peter picks the best available path."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Song, artist, album, playlist, genre, or mood to play. E.g. 'jazz', 'Miles Davis Kind of Blue', 'workout playlist', 'something relaxing'."
                },
                "target": {
                    "type": "string",
                    "description": "Optional HA media_player entity_id to target (e.g. 'media_player.homepod_living_room'). If omitted, picks the best available speaker automatically."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            network: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
        if query.is_empty() {
            return Ok(RichToolResult::text("What should I play? Tell me a song, artist, or mood."));
        }
        let target_override = args.get("target").and_then(|v| v.as_str()).map(|s| s.to_string());
        let client = ctx.http.as_ref().ok_or("no HTTP client")?.clone();
        let db_path = ctx.db_path.ok_or("no db_path in context")?;
        let creds = load_sync_creds(db_path, ctx.user_id);

        // Step 1: search Apple Music if connected
        let song = if let Some(ref am) = creds.apple_music {
            match apple_music_search_first(&client, am, query).await {
                Ok(Some(s)) => Some(s),
                Ok(None) => None,
                Err(e) => { warn!("[play_music] AM search failed: {}", e); None }
            }
        } else { None };

        let (song_name, artist_name, apple_music_url) = match &song {
            Some(s) => {
                let attrs = s.get("attributes").cloned().unwrap_or(Value::Null);
                let name = attrs.get("name").and_then(|v| v.as_str()).unwrap_or(query).to_string();
                let artist = attrs.get("artistName").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let url = attrs.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                (name, artist, url)
            }
            None => (query.to_string(), String::new(), String::new()),
        };

        // Step 2: route playback

        // 2a. Phone PWA — TOP priority for mobile users. Sends music:// URL
        // through the bridge SSE channel; phone's Music.app opens and plays.
        // No DRM workaround needed — phone has its own Apple Music subscription.
        if creds.has_phone_pwa && !apple_music_url.is_empty() {
            let pwa_url = if apple_music_url.starts_with("https://music.apple.com") {
                apple_music_url.replacen("https://", "music://", 1)
            } else {
                apple_music_url.clone()
            };
            let cmd = serde_json::json!({
                "type": "play_music",
                "url": pwa_url,
                "song": song_name,
                "artist": artist_name,
            });
            let resp = client.post("http://127.0.0.1:18804/command")
                .json(&cmd)
                .timeout(std::time::Duration::from_secs(3))
                .send().await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    let body: serde_json::Value = r.json().await.unwrap_or_default();
                    let count = body.get("sent_to").and_then(|v| v.as_u64()).unwrap_or(0);
                    let phone_count = count.saturating_sub(1);
                    if phone_count > 0 {
                        info!("[play_music] sent to PWA ({} subscribers): {}", phone_count, song_name);
                        return Ok(RichToolResult::text(format!(
                            "Sending {}{} to your phone — Music app should open and play.",
                            song_name,
                            if artist_name.is_empty() { "".to_string() } else { format!(" by {}", artist_name) },
                        )));
                    } else {
                        warn!("[play_music] PWA registered but no live subscribers — skipping");
                    }
                }
                Ok(r) => warn!("[play_music] bridge returned {}", r.status()),
                Err(e) => warn!("[play_music] bridge unreachable: {}", e),
            }
        }

        // 2b. Home Assistant — preferred for whole-home playback (HomePod/Apple TV seamless)
        if let Some(ref ha) = creds.home_assistant {
            let target = match target_override.clone() {
                Some(t) => Some(t),
                None => find_ha_media_player(&client, ha).await.ok().flatten(),
            };
            if let Some(entity_id) = target {
                let (content_id, content_type) = if !apple_music_url.is_empty() {
                    (apple_music_url.clone(), "music".to_string())
                } else {
                    (query.to_string(), "music".to_string())
                };
                match ha_play_media(&client, ha, &entity_id, &content_id, &content_type).await {
                    Ok(_) => {
                        info!("[play_music] routed via HA {} → {}", entity_id, song_name);
                        return Ok(RichToolResult::text(format!(
                            "Playing {}{}{} on {}.",
                            song_name,
                            if artist_name.is_empty() { "".to_string() } else { format!(" by {}", artist_name) },
                            if apple_music_url.is_empty() { " (search query)".to_string() } else { "".to_string() },
                            entity_id.strip_prefix("media_player.").unwrap_or(&entity_id).replace('_', " ")
                        )));
                    }
                    Err(e) => warn!("[play_music] HA play_media failed: {}", e),
                }
            }
        }

        // 2c. iOS Shortcut fallback — plays on phone
        if let Some(ref shortcut_url) = creds.ios_shortcut {
            match trigger_ios_shortcut(&client, shortcut_url, query).await {
                Ok(_) => {
                    info!("[play_music] triggered iOS Shortcut: {}", query);
                    return Ok(RichToolResult::text(format!(
                        "Sent '{}' to your iPhone Shortcut. Check your phone for playback.",
                        song_name
                    )));
                }
                Err(e) => warn!("[play_music] iOS Shortcut failed: {}", e),
            }
        }

        // 2d. Browser fallback — return URL
        if !apple_music_url.is_empty() {
            return Ok(RichToolResult::text(format!(
                "Found {}{}. Open this link on any Apple device to play: {}\n\
                 (To play automatically, connect Home Assistant with a HomePod or Apple TV, or set up the iOS Shortcut provider in Sync settings.)",
                song_name,
                if artist_name.is_empty() { "".to_string() } else { format!(" by {}", artist_name) },
                apple_music_url,
            )));
        }

        // Nothing connected at all
        Ok(RichToolResult::text(
            "No music playback target available. To enable music playback:\n\
             • Connect Apple Music in Sync settings (gives metadata + search)\n\
             • Connect Home Assistant + pair a HomePod/Apple TV (best — speakers play automatically)\n\
             • OR set up an iOS Shortcut (plays on your phone)\n\
             Go to Settings → Sync to add any of these."
        ))
    }
}
