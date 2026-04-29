//! Music module — aggregated now-playing, playback control, speaker management, AI DJ.
//!
//! Pulls state from:
//!   - sync_connections for home_assistant credential
//!   - HA media_player.* entity states (most reliable for HomePod/Apple TV/Sonos)
//!   - Apple Music API recent-played (as metadata source)
//!   - PWA-reported state via a shared in-memory cache (posted by the PWA when it plays)
//!
//! Actions routed to:
//!   - HA service calls (pause/play/skip/seek/volume/join/unjoin/set_sound_mode)
//!   - Phone PWA via bridge command channel (launch URL with music:// scheme)
//!
//! AI DJ uses the existing LLM provider config to generate track ideas, then
//! hits api.music.apple.com/v1/catalog/{storefront}/search for each, and
//! POSTs to /v1/me/library/playlists to build a real Apple Music playlist.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, response::Json};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::AppState;

// ── Shared state for PWA-reported playback ──────────────────────────────────
// When the PWA launches music:// URL it also posts {playing_now: {...}} back.
// The bridge relays to gateway; we cache here in memory (single-user only).

pub struct PwaNowPlaying {
    pub song: String,
    pub artist: String,
    pub device: String,
    pub updated_at: i64,
}

static PWA_NOW_PLAYING: tokio::sync::OnceCell<tokio::sync::RwLock<Option<PwaNowPlaying>>> =
    tokio::sync::OnceCell::const_new();

async fn get_pwa_now() -> Option<PwaNowPlaying> {
    let cell = PWA_NOW_PLAYING.get_or_init(|| async { tokio::sync::RwLock::new(None) }).await;
    let g = cell.read().await;
    g.as_ref().map(|p| PwaNowPlaying {
        song: p.song.clone(), artist: p.artist.clone(),
        device: p.device.clone(), updated_at: p.updated_at,
    })
}

async fn set_pwa_now(val: PwaNowPlaying) {
    let cell = PWA_NOW_PLAYING.get_or_init(|| async { tokio::sync::RwLock::new(None) }).await;
    let mut g = cell.write().await;
    *g = Some(val);
}

// ── Helpers: load HA creds ──────────────────────────────────────────────────

async fn load_ha(state: &Arc<AppState>, uid: i64) -> Option<(String, String)> {
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Option<(String, String)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        let cred_s: String = conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'home_assistant' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).ok()?;
        let c: serde_json::Value = serde_json::from_str(&cred_s).ok()?;
        let url = c.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tok = c.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if url.is_empty() || tok.is_empty() { None } else { Some((url, tok)) }
    }).await.ok().flatten()
}

async fn load_apple_music(state: &Arc<AppState>, uid: i64) -> Option<(String, String, String)> {
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Option<(String, String, String)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        let cred_s: String = conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'apple_music' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).ok()?;
        let c: serde_json::Value = serde_json::from_str(&cred_s).ok()?;
        let d = c.get("developer_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let u = c.get("music_user_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let s = c.get("storefront").and_then(|v| v.as_str()).unwrap_or("us").to_string();
        if d.is_empty() || u.is_empty() { None } else { Some((d, u, s)) }
    }).await.ok().flatten()
}

// ── /api/music/now_playing ──────────────────────────────────────────────────

#[derive(Serialize)]
struct NowPlaying {
    song: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    art_url: Option<String>,
    device: Option<String>,
    entity_id: Option<String>,
    state: String, // "playing" | "paused" | "idle" | "off"
    source: String, // "homepod" | "appletv" | "sonos" | "phone" | "none"
    position: Option<f64>,
    duration: Option<f64>,
}

pub async fn handle_music_now_playing(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal_scoped(&state, token, "music").await?;
    let uid = principal.user_id();

    // PRIMARY: PWA-reported state. The phone (via the Syntaur Voice PWA) is
    // the default music client — no HA dependency. It reports what Music.app is
    // playing after Peter fires a play_music command.
    if let Some(pwa) = get_pwa_now().await {
        let age = chrono::Utc::now().timestamp() - pwa.updated_at;
        if age < 300 { // within 5 min — fresh enough to trust
            return Ok(Json(serde_json::json!({
                "song": pwa.song,
                "artist": pwa.artist,
                "device": pwa.device,
                "state": "playing",
                "source": "phone",
                "age_seconds": age,
            })));
        }
    }

    // SECONDARY: Apple Music recently-played — shows what the user was listening
    // to on ANY Apple Music client (phone, HomePod via their account, web).
    // Works with just Apple Music connected — no HA needed.
    if let Some((dev, mut_, sf)) = load_apple_music(&state, uid).await {
        let url = format!("https://api.music.apple.com/v1/me/recent/played/tracks?limit=1");
        if let Ok(resp) = state.client.get(&url)
            .header("Authorization", format!("Bearer {}", dev))
            .header("Music-User-Token", mut_)
            .header("Origin", "https://music.apple.com")
            .timeout(Duration::from_secs(8))
            .send().await {
            let _ = sf;
            if resp.status().is_success() {
                if let Ok(j) = resp.json::<serde_json::Value>().await {
                    if let Some(track) = j.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()) {
                        let attrs = track.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
                        return Ok(Json(serde_json::json!({
                            "song": attrs.get("name"),
                            "artist": attrs.get("artistName"),
                            "album": attrs.get("albumName"),
                            "art_url": attrs.get("artwork").and_then(|a| a.get("url")),
                            "state": "recent",
                            "source": "apple_music_recent",
                            "note": "Most recently played on Apple Music. Not live.",
                        })));
                    }
                }
            }
        }
    }

    // OPTIONAL: Home Assistant media_player states (power-user path).
    // Only queried if HA is actually connected — never required.
    if let Some(ha) = load_ha(&state, uid).await {
        let states_url = format!("{}/api/states", ha.0.trim_end_matches('/'));
        if let Ok(resp) = state.client.get(&states_url)
            .header("Authorization", format!("Bearer {}", ha.1))
            .timeout(Duration::from_secs(8))
            .send().await {
            if resp.status().is_success() {
                if let Ok(arr) = resp.json::<serde_json::Value>().await {
                    if let Some(states) = arr.as_array() {
                        // Pick the first media_player that is playing (or paused with title)
                        let mut best: Option<&serde_json::Value> = None;
                        let mut best_score = 0;
                        for s in states {
                            let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
                            if !eid.starts_with("media_player.") { continue; }
                            let st = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
                            let score = match st {
                                "playing" => 100,
                                "paused" => 50,
                                _ => 0,
                            };
                            if score > best_score {
                                best_score = score;
                                best = Some(s);
                            }
                        }
                        if let Some(mp) = best {
                            let eid = mp.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
                            let st = mp.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
                            let attrs = mp.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
                            let name = attrs.get("friendly_name").and_then(|v| v.as_str()).unwrap_or(eid).to_string();
                            let lower = name.to_ascii_lowercase();
                            let source = if lower.contains("homepod") { "homepod" }
                                else if lower.contains("apple tv") || eid.contains("apple_tv") { "appletv" }
                                else if lower.contains("sonos") { "sonos" }
                                else { "media_player" };
                            let np = NowPlaying {
                                song: attrs.get("media_title").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                artist: attrs.get("media_artist").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                album: attrs.get("media_album_name").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                art_url: attrs.get("entity_picture").and_then(|v| v.as_str()).map(|p| {
                                    if p.starts_with("http") { p.to_string() }
                                    else { format!("{}{}", ha.0.trim_end_matches('/'), p) }
                                }),
                                device: Some(name),
                                entity_id: Some(eid.to_string()),
                                state: st.to_string(),
                                source: source.to_string(),
                                position: attrs.get("media_position").and_then(|v| v.as_f64()),
                                duration: attrs.get("media_duration").and_then(|v| v.as_f64()),
                            };
                            return Ok(Json(serde_json::to_value(np).unwrap_or_default()));
                        }
                    }
                }
            }
        }
    }

    let ducking = get_duck_state().await;
    Ok(Json(serde_json::json!({
        "state": "off",
        "source": "none",
        "ducking": ducking.active,
        "hint": "Nothing playing. Ask to play something, or connect Apple Music / pair your phone in Sync.",
    })))
}

// ── /api/music/control ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MusicControlRequest {
    pub token: String,
    pub action: String,      // "play" | "pause" | "play_pause" | "next" | "prev" | "volume"
    pub entity_id: Option<String>,
    pub value: Option<f64>,  // volume 0.0-1.0
}

pub async fn handle_music_control(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MusicControlRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let uid = principal.user_id();

    // PRIMARY: if the user is playing via PWA (phone), route the control there
    // via the bridge command channel. The PWA handles pause/skip by navigating
    // to music:// URLs that Music.app interprets.
    let using_pwa = get_pwa_now().await.is_some();
    if using_pwa {
        let cmd = match req.action.as_str() {
            "pause" | "play" | "play_pause" => serde_json::json!({"type":"pause","message":"Pause requested"}),
            "next" | "skip" => serde_json::json!({"type":"next","message":"Skip requested"}),
            "prev" | "previous" => serde_json::json!({"type":"prev","message":"Previous requested"}),
            _ => serde_json::json!({"type":"info","message":format!("{} not supported on phone playback", req.action)}),
        };
        let resp = state.client.post("http://127.0.0.1:18804/command")
            .json(&cmd)
            .timeout(Duration::from_secs(3))
            .send().await;
        match resp {
            Ok(r) if r.status().is_success() => {
                return Ok(Json(serde_json::json!({
                    "ok": true,
                    "action": req.action,
                    "routed_via": "phone_pwa",
                    "note": "Phone's Music.app may need a manual tap for some controls due to iOS restrictions.",
                })));
            }
            _ => {} // fall through to HA
        }
    }

    // OPTIONAL: Home Assistant — power-user path for HomePod / Sonos / etc.
    let Some(ha) = load_ha(&state, uid).await else {
        return Ok(Json(serde_json::json!({
            "error": "No playback target available",
            "hint": "Pair your phone in Sync, or connect Home Assistant for speaker control."
        })));
    };

    // If no entity_id given, find the currently playing one
    let entity_id = match req.entity_id.clone() {
        Some(id) => id,
        None => {
            // Re-query states for a playing/paused media_player
            let states_url = format!("{}/api/states", ha.0.trim_end_matches('/'));
            let resp = state.client.get(&states_url)
                .header("Authorization", format!("Bearer {}", ha.1))
                .timeout(Duration::from_secs(8))
                .send().await
                .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
            if !resp.status().is_success() {
                return Err(axum::http::StatusCode::BAD_GATEWAY);
            }
            let arr: serde_json::Value = resp.json().await.unwrap_or_default();
            let found = arr.as_array()
                .and_then(|states| states.iter().find(|s| {
                    s.get("entity_id").and_then(|v| v.as_str()).map(|e| e.starts_with("media_player.")).unwrap_or(false)
                    && matches!(s.get("state").and_then(|v| v.as_str()), Some("playing") | Some("paused"))
                }).cloned())
                .and_then(|s| s.get("entity_id").and_then(|v| v.as_str()).map(|e| e.to_string()));
            match found {
                Some(e) => e,
                None => return Ok(Json(serde_json::json!({"error":"no active media_player"}))),
            }
        }
    };

    let (svc, body) = match req.action.as_str() {
        "play" => ("media_play", serde_json::json!({"entity_id": entity_id})),
        "pause" => ("media_pause", serde_json::json!({"entity_id": entity_id})),
        "play_pause" => ("media_play_pause", serde_json::json!({"entity_id": entity_id})),
        "next" | "skip" => ("media_next_track", serde_json::json!({"entity_id": entity_id})),
        "prev" | "previous" => ("media_previous_track", serde_json::json!({"entity_id": entity_id})),
        "volume" => {
            let vol = req.value.unwrap_or(0.5).max(0.0).min(1.0);
            ("volume_set", serde_json::json!({"entity_id": entity_id, "volume_level": vol}))
        }
        _ => return Err(axum::http::StatusCode::BAD_REQUEST),
    };
    let url = format!("{}/api/services/media_player/{}", ha.0.trim_end_matches('/'), svc);
    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", ha.1))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send().await
        .map_err(|e| { warn!("[music-control] {}", e); axum::http::StatusCode::BAD_GATEWAY })?;
    if !resp.status().is_success() {
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    Ok(Json(serde_json::json!({"ok": true, "action": req.action, "entity_id": entity_id})))
}

// ── /api/music/speakers ─────────────────────────────────────────────────────

pub async fn handle_music_speakers(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal_scoped(&state, token, "music").await?;
    let uid = principal.user_id();
    let mut speakers: Vec<serde_json::Value> = Vec::new();

    // "This computer" is a playback target when a /music tab is subscribed
    // to the local events stream (Spotify Web Playback SDK / YouTube IFrame
    // Player are ready to play on that tab's audio output)
    if this_computer_available().await {
        speakers.push(serde_json::json!({
            "id": "this_computer",
            "entity_id": "this_computer",
            "name": "This computer",
            "kind": "this_computer",
            "state": "available",
            "can_control": true,
            "hint": "Plays through this browser\'s audio output (laptop speakers, headphones, whatever your OS has selected). Requires the /music page to be open.",
        }));
    }

    // Phone is always a playback target (if PWA paired)
    let db = state.db_path.clone();
    let pwa_connected = tokio::task::spawn_blocking(move || -> bool {
        let Ok(conn) = rusqlite::Connection::open(&db) else { return false; };
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_connections WHERE user_id = ? AND provider = 'phone_music_pwa' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).unwrap_or(0);
        count > 0
    }).await.unwrap_or(false);
    if pwa_connected {
        speakers.push(serde_json::json!({
            "id": "phone",
            "entity_id": "phone",
            "name": "My Phone",
            "kind": "phone",
            "state": "available",
            "can_control": true,
            "hint": "Plays wherever your phone is connected (speakers, AirPods, or AirPlayed to HomePod).",
        }));
    }

    // AirPlay discovered speakers (cached in sync_connections when user connects airplay provider)
    let db2 = state.db_path.clone();
    let airplay_devices: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let Ok(conn) = rusqlite::Connection::open(&db2) else { return vec![]; };
        let cred_s: Option<String> = conn.query_row(
            "SELECT credential FROM sync_connections WHERE user_id = ? AND provider = 'airplay' AND status = 'active'",
            rusqlite::params![uid], |r| r.get(0),
        ).ok();
        let Some(s) = cred_s else { return vec![]; };
        let cred: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        cred.get("devices").and_then(|d| d.as_array()).cloned().unwrap_or_default()
    }).await.unwrap_or_default();
    for dev in airplay_devices {
        let name = dev.get("name").and_then(|v| v.as_str()).unwrap_or("AirPlay speaker").to_string();
        let lower = name.to_ascii_lowercase();
        let kind = if lower.contains("homepod") { "homepod" }
                   else if lower.contains("apple tv") { "appletv" }
                   else { "airplay" };
        speakers.push(serde_json::json!({
            "id": format!("airplay:{}", dev.get("hostname").and_then(|v| v.as_str()).unwrap_or(&name)),
            "entity_id": null,
            "name": name,
            "kind": kind,
            "state": "available",
            "can_control": false, // would need direct AirPlay sender; for now info-only
            "hint": "Use iOS Control Center on your phone to AirPlay to this speaker, then Peter will play through it.",
        }));
    }

    // OPTIONAL: Home Assistant media_players — if HA is connected, also list those
    let Some(ha) = load_ha(&state, uid).await else {
        return Ok(Json(serde_json::json!({
            "speakers": speakers,
            "count": speakers.len(),
            "note": if speakers.is_empty() { "Pair your phone in Sync to enable music playback." } else { "Showing phone + AirPlay speakers. Connect Home Assistant for direct speaker control and grouping." },
        })));
    };
    let states_url = format!("{}/api/states", ha.0.trim_end_matches('/'));
    let resp = state.client.get(&states_url)
        .header("Authorization", format!("Bearer {}", ha.1))
        .timeout(Duration::from_secs(8))
        .send().await.map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        // HA unreachable — still return phone + AirPlay
        return Ok(Json(serde_json::json!({"speakers": speakers, "count": speakers.len()})));
    }
    let arr: serde_json::Value = resp.json().await.unwrap_or_default();
    let Some(states) = arr.as_array() else {
        return Ok(Json(serde_json::json!({"speakers": speakers, "count": speakers.len()})));
    };

    let ha_speakers: Vec<serde_json::Value> = states.iter().filter_map(|s| {
        let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
        if !eid.starts_with("media_player.") { return None; }
        let st = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
        if st == "unavailable" || st == "unknown" { return None; }
        let attrs = s.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
        let name = attrs.get("friendly_name").and_then(|v| v.as_str()).unwrap_or(eid);
        let lower = name.to_ascii_lowercase();
        let kind = if lower.contains("homepod") { "homepod" }
                   else if lower.contains("apple tv") || eid.contains("apple_tv") { "appletv" }
                   else if lower.contains("sonos") { "sonos" }
                   else { "other" };
        let group_members = attrs.get("group_members").cloned().unwrap_or(serde_json::Value::Null);
        let sound_mode_list = attrs.get("sound_mode_list").cloned().unwrap_or(serde_json::Value::Null);
        Some(serde_json::json!({
            "entity_id": eid,
            "name": name,
            "state": st,
            "kind": kind,
            "volume": attrs.get("volume_level"),
            "muted": attrs.get("is_volume_muted"),
            "source": attrs.get("source"),
            "sound_mode": attrs.get("sound_mode"),
            "sound_mode_list": sound_mode_list,
            "group_members": group_members,
            "supports_grouping": !attrs.get("group_members").is_none(),
        }))
    }).collect();
    // Merge HA speakers with the phone/airplay list
    speakers.extend(ha_speakers);
    Ok(Json(serde_json::json!({
        "speakers": speakers,
        "count": speakers.len(),
        "has_ha": true,
        "can_group": true,
    })))
}

// ── /api/music/group — join/unjoin speakers (HA-only feature) ───────────────

#[derive(Deserialize)]
pub struct MusicGroupRequest {
    pub token: String,
    pub action: String, // "join" or "unjoin"
    pub entity_id: String, // leader (for join) or member (for unjoin)
    pub group_members: Option<Vec<String>>, // only for join
}

pub async fn handle_music_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MusicGroupRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let Some(ha) = load_ha(&state, principal.user_id()).await else {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    };
    let (svc, body) = match req.action.as_str() {
        "join" => {
            let members = req.group_members.unwrap_or_default();
            ("join", serde_json::json!({
                "entity_id": req.entity_id,
                "group_members": members,
            }))
        }
        "unjoin" => ("unjoin", serde_json::json!({"entity_id": req.entity_id})),
        _ => return Err(axum::http::StatusCode::BAD_REQUEST),
    };
    let url = format!("{}/api/services/media_player/{}", ha.0.trim_end_matches('/'), svc);
    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", ha.1))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    Ok(Json(serde_json::json!({"ok": true, "action": req.action})))
}

// ── /api/music/eq — set sound mode (EQ preset) ──────────────────────────────

#[derive(Deserialize)]
pub struct MusicEqRequest {
    pub token: String,
    pub entity_id: String,
    pub sound_mode: String,
}

pub async fn handle_music_eq(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MusicEqRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let Some(ha) = load_ha(&state, principal.user_id()).await else {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    };
    let url = format!("{}/api/services/media_player/select_sound_mode", ha.0.trim_end_matches('/'));
    let body = serde_json::json!({
        "entity_id": req.entity_id,
        "sound_mode": req.sound_mode,
    });
    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", ha.1))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send().await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if !resp.status().is_success() {
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    Ok(Json(serde_json::json!({"ok": true, "sound_mode": req.sound_mode})))
}

// ── /api/music/dj — AI-generated playlist ───────────────────────────────────

#[derive(Deserialize)]
pub struct MusicDjRequest {
    pub token: String,
    pub prompt: String,
    pub count: Option<usize>, // default 15
    pub target: Option<String>, // HA entity_id, phone, etc. If none, just returns list.
    pub create_playlist: Option<bool>,
}

pub async fn handle_music_dj(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MusicDjRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let uid = principal.user_id();
    // Pick which music provider to search through. preferred_music_provider
    // now returns "local" first when the user has any local tracks indexed,
    // so the DJ works without a streaming subscription. Cloud providers are
    // used as a fallback if local has no matches for the prompt (see the
    // local search loop below).
    let provider = match preferred_music_provider(&state, uid).await {
        Some(p) => p,
        None => return Ok(Json(serde_json::json!({
            "error": "No music source yet",
            "hint": "Add a folder in Local library, or connect Apple Music / Spotify / YouTube Music / Tidal in Sync."
        }))),
    };

    // Load auth credentials for whichever cloud provider might be used —
    // either as the primary (if no local tracks) or as a fallback after
    // local runs dry. Cheap no-ops when not needed.
    let cloud_fallback = preferred_cloud_music_provider(&state, uid).await;
    let cloud_for_load = if provider == "local" { cloud_fallback.clone() } else { Some(provider.clone()) };
    let apple_creds = if cloud_for_load.as_deref() == Some("apple_music") { load_apple_music(&state, uid).await } else { None };
    let spotify_token = if cloud_for_load.as_deref() == Some("spotify") { load_oauth_access_token(&state, uid, "spotify").await } else { None };
    let ytm_token = if cloud_for_load.as_deref() == Some("youtube_music") { load_oauth_access_token(&state, uid, "youtube_music").await } else { None };

    // Cloud-auth checks only apply when cloud is the primary provider.
    if provider == "youtube_music" && ytm_token.is_none() {
        return Ok(Json(serde_json::json!({"error":"YouTube Music not authorized","hint":"Complete the OAuth flow in Sync. Requires Google OAuth with the youtube scope."})));
    }
    if provider == "apple_music" && apple_creds.is_none() {
        return Ok(Json(serde_json::json!({"error":"Apple Music credentials expired","hint":"Reconnect in Sync"})));
    }
    if provider == "spotify" && spotify_token.is_none() {
        return Ok(Json(serde_json::json!({"error":"Spotify not authorized","hint":"Complete the OAuth flow in Sync"})));
    }

    let count = req.count.unwrap_or(15).min(30);

    // Step 1: ask the LLM for track ideas matching the prompt
    // Pull recent preferences for personalization
    let prefs_context = load_prefs_context(&state, uid, 30).await;
    let prefs_preamble = if prefs_context.is_empty() {
        String::new()
    } else {
        format!("\n\nUser preference context (use to bias selection):\n{}\n", prefs_context)
    };
    let llm_prompt = format!(
        "You are a music DJ building a playlist. Request: \"{}\"\n{}\n         Return ONLY a JSON array of {} strings, each a \"SONG - ARTIST\" search query. \
         Favor artists/genres the user likes; avoid what they dislike. \
         No markdown fences, no prose, just the JSON array. Example:\n         [\"Kind of Blue - Miles Davis\", \"Take Five - Dave Brubeck\"]",
        req.prompt, prefs_preamble, count
    );

    let chain = crate::llm::LlmChain::from_config_fast(&state.config, "main", state.client.clone());
    let messages = vec![
        crate::llm::ChatMessage::system("You are a music curator. Respond ONLY with a JSON array of track search queries — no markdown, no prose."),
        crate::llm::ChatMessage::user(&llm_prompt),
    ];
    let llm_response = chain.call(&messages).await;
    let ideas: Vec<String> = match llm_response {
        Ok(text) => {
            // Strip any markdown fences
            let cleaned = text.trim()
                .trim_start_matches("```json").trim_start_matches("```")
                .trim_end_matches("```").trim();
            serde_json::from_str::<Vec<String>>(cleaned).unwrap_or_else(|_| {
                // Fallback: split by newlines
                text.lines().filter_map(|l| {
                    let t = l.trim().trim_start_matches('-').trim_start_matches('*').trim_start_matches('"').trim_end_matches(',').trim_end_matches('"').trim();
                    if t.is_empty() || t.starts_with('[') || t.starts_with(']') { None } else { Some(t.to_string()) }
                }).take(count).collect()
            })
        }
        Err(e) => {
            warn!("[music-dj] LLM failed: {}", e);
            return Err(axum::http::StatusCode::BAD_GATEWAY);
        }
    };

    if ideas.is_empty() {
        return Ok(Json(serde_json::json!({"error":"LLM returned no tracks"})));
    }

    // Step 2: search the chosen provider for each idea in parallel
    let client = state.client.clone();
    let mut tracks: Vec<serde_json::Value> = Vec::new();
    let mut search_handles = Vec::new();
    for idea in &ideas {
        let cli = client.clone();
        let q = idea.clone();
        let prov = provider.clone();
        let apple = apple_creds.clone();
        let sp_tok = spotify_token.clone();
        let ytm_tok = ytm_token.clone();
        let state_clone = Arc::clone(&state);
        let cloud_fb = cloud_fallback.clone();
        search_handles.push(tokio::spawn(async move {
            // When the primary is local, try the library first. Fall
            // through to whichever cloud provider the user has configured
            // so a local miss doesn't leave the playlist short.
            if prov == "local" {
                if let Some(track) = search_local_track(&state_clone, uid, &q).await {
                    return Some((q.clone(), track));
                }
                match cloud_fb.as_deref() {
                    Some("apple_music") => {
                        let (dev, mut_, sf) = apple?;
                        return do_apple_search(&cli, &dev, &mut_, &sf, &q).await.map(|t| (q.clone(), t));
                    }
                    Some("spotify") => {
                        let tok = sp_tok?;
                        let results = spotify_search(&cli, &tok, &q, 1).await.ok()?;
                        return results.into_iter().next().map(|t| (q.clone(), t));
                    }
                    Some("youtube_music") => {
                        let tok = ytm_tok?;
                        let results = youtube_music_search(&cli, &tok, &q, 1).await.ok()?;
                        return results.into_iter().next().map(|t| (q.clone(), t));
                    }
                    _ => return None,
                }
            }
            match prov.as_str() {
                "apple_music" => {
                    let (dev, mut_, sf) = apple?;
                    do_apple_search(&cli, &dev, &mut_, &sf, &q).await.map(|t| (q.clone(), t))
                }
                "spotify" => {
                    let tok = sp_tok?;
                    let results = spotify_search(&cli, &tok, &q, 1).await.ok()?;
                    let first = results.into_iter().next()?;
                    Some((q.clone(), first))
                }
                "youtube_music" => {
                    let tok = ytm_tok?;
                    let results = youtube_music_search(&cli, &tok, &q, 1).await.ok()?;
                    let first = results.into_iter().next()?;
                    Some((q.clone(), first))
                }
                _ => None,
            }
        }));
    }
    for h in search_handles {
        if let Ok(Some((_, track))) = h.await {
            tracks.push(track);
        }
    }

    // Step 3 (optional): create a playlist on the chosen provider
    let mut playlist_id: Option<String> = None;
    if req.create_playlist.unwrap_or(false) && !tracks.is_empty() {
        match provider.as_str() {
            "apple_music" => {
                if let Some((dev, mut_, _)) = &apple_creds {
                    let track_data: Vec<serde_json::Value> = tracks.iter().filter_map(|t| {
                        let id = t.get("id")?.as_str()?;
                        Some(serde_json::json!({"id": id, "type": "songs"}))
                    }).collect();
                    let body = serde_json::json!({
                        "attributes": {
                            "name": format!("Syntaur DJ: {}", req.prompt.chars().take(40).collect::<String>()),
                            "description": format!("Generated by Syntaur from prompt: {}", req.prompt),
                        },
                        "relationships": {"tracks": {"data": track_data}}
                    });
                    let url = "https://api.music.apple.com/v1/me/library/playlists";
                    if let Ok(r) = state.client.post(url)
                        .header("Authorization", format!("Bearer {}", dev))
                        .header("Music-User-Token", mut_)
                        .header("Origin", "https://music.apple.com")
                        .json(&body).timeout(Duration::from_secs(15)).send().await {
                        if r.status().is_success() {
                            if let Ok(j) = r.json::<serde_json::Value>().await {
                                playlist_id = j.get("data").and_then(|d| d.as_array())
                                    .and_then(|a| a.first()).and_then(|p| p.get("id"))
                                    .and_then(|i| i.as_str()).map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
            "youtube_music" => {
                if let Some(tok) = ytm_token.as_ref() {
                    let video_ids: Vec<String> = tracks.iter()
                        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .collect();
                    let name = format!("Syntaur DJ: {}", req.prompt.chars().take(40).collect::<String>());
                    let desc = format!("Generated by Syntaur from prompt: {}", req.prompt);
                    match youtube_music_create_playlist(&state.client, tok, &name, &desc, &video_ids).await {
                        Ok(pid) => { playlist_id = Some(pid); }
                        Err(e) => warn!("[music-dj] YT Music playlist create failed: {}", e),
                    }
                }
            }
            "spotify" => {
                if let Some(tok) = spotify_token.as_ref() {
                    // Step 3a: get user id
                    let sp_user_id: Option<String> = {
                        let resp = state.client.get("https://api.spotify.com/v1/me")
                            .header("Authorization", format!("Bearer {}", tok))
                            .send().await;
                        match resp {
                            Ok(r) if r.status().is_success() => {
                                r.json::<serde_json::Value>().await.ok()
                                    .and_then(|b| b.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            }
                            _ => None,
                        }
                    };
                    if let Some(user_id) = sp_user_id {
                        let url = format!("https://api.spotify.com/v1/users/{}/playlists", user_id);
                        let body = serde_json::json!({
                            "name": format!("Syntaur DJ: {}", req.prompt.chars().take(40).collect::<String>()),
                            "description": format!("Generated by Syntaur from prompt: {}", req.prompt),
                            "public": false,
                        });
                        if let Ok(r) = state.client.post(&url)
                            .header("Authorization", format!("Bearer {}", tok))
                            .json(&body).timeout(Duration::from_secs(15)).send().await {
                            if r.status().is_success() {
                                if let Ok(j) = r.json::<serde_json::Value>().await {
                                    if let Some(pid) = j.get("id").and_then(|v| v.as_str()) {
                                        playlist_id = Some(pid.to_string());
                                        // Add tracks
                                        let uris: Vec<String> = tracks.iter()
                                            .filter_map(|t| t.get("uri").and_then(|v| v.as_str()).map(|s| s.to_string()))
                                            .collect();
                                        if !uris.is_empty() {
                                            let add_url = format!("https://api.spotify.com/v1/playlists/{}/tracks", pid);
                                            let add_body = serde_json::json!({"uris": uris});
                                            let _ = state.client.post(&add_url)
                                                .header("Authorization", format!("Bearer {}", tok))
                                                .json(&add_body).send().await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    info!("[music-dj] provider={} prompt=\"{}\" ideas={} tracks={} playlist={:?}",
        provider,
        req.prompt.chars().take(50).collect::<String>(),
        ideas.len(), tracks.len(), playlist_id);

    Ok(Json(serde_json::json!({
        "provider": provider,
        "prompt": req.prompt,
        "ideas": ideas,
        "tracks": tracks,
        "playlist_id": playlist_id,
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn url_encode_local(s: &str) -> String {
    s.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            (b as char).to_string()
        } else {
            format!("%{:02X}", b)
        }
    }).collect()
}

// ── /api/music/pwa_state — PWA reports its current playback ────────────────

#[derive(Deserialize)]
pub struct PwaStateRequest {
    pub token: String,
    pub song: String,
    pub artist: String,
    pub device: Option<String>,
}

pub async fn handle_pwa_state(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PwaStateRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _ = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    set_pwa_now(PwaNowPlaying {
        song: req.song,
        artist: req.artist,
        device: req.device.unwrap_or_else(|| "Phone".to_string()),
        updated_at: chrono::Utc::now().timestamp(),
    }).await;
    Ok(Json(serde_json::json!({"ok": true})))
}
// ── Preferred playback target (persisted per-user) ──────────────────────────

#[derive(serde::Deserialize)]
pub struct PreferredTargetRequest {
    pub token: String,
    pub entity_id: String,
    pub name: Option<String>,
}

pub async fn handle_set_preferred_target(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PreferredTargetRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let target = req.entity_id.clone();
    let name = req.name.unwrap_or_else(|| target.clone());
    let now = chrono::Utc::now().timestamp();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Store as metadata on a dedicated row (or patch existing if phone_music_pwa is connected)
        let meta = serde_json::json!({"preferred_target": target, "preferred_name": name});
        let meta_json = serde_json::to_string(&meta).unwrap_or_default();
        conn.execute(
            "INSERT INTO sync_connections (user_id, provider, display_name, credential, metadata, status, created_at, updated_at, last_check_at)
             VALUES (?, 'music_preferences', 'Preferences', '{}', ?, 'active', ?, ?, ?)
             ON CONFLICT(user_id, provider) DO UPDATE SET
               metadata = excluded.metadata,
               updated_at = excluded.updated_at",
            rusqlite::params![uid, meta_json, now, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"ok": true, "target": req.entity_id})))
}

pub async fn load_preferred_target(state: &Arc<AppState>, uid: i64) -> Option<(String, String)> {
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Option<(String, String)> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        let meta: String = conn.query_row(
            "SELECT metadata FROM sync_connections WHERE user_id = ? AND provider = 'music_preferences'",
            rusqlite::params![uid], |r| r.get(0)
        ).ok()?;
        let v: serde_json::Value = serde_json::from_str(&meta).ok()?;
        let target = v.get("preferred_target")?.as_str()?.to_string();
        let name = v.get("preferred_name").and_then(|n| n.as_str()).unwrap_or(&target).to_string();
        Some((target, name))
    }).await.ok().flatten()
}


// ── Multi-provider music catalog helpers ────────────────────────────────────

/// Returns the preferred music provider id for this user. Priority: local
/// library first (if the user has scanned any tracks), then Apple Music →
/// Spotify → YouTube Music → Tidal from sync_connections. The DJ endpoint
/// treats "local" as a first-class provider and falls through to cloud
/// only if no local tracks match an LLM-generated query.
pub async fn preferred_music_provider(state: &Arc<AppState>, uid: i64) -> Option<String> {
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        // Local library takes priority — free, zero-latency, no auth.
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM local_music_tracks WHERE user_id = ?",
            rusqlite::params![uid],
            |r| r.get(0)
        ).unwrap_or(0);
        if local_count > 0 {
            return Some("local".to_string());
        }
        for pid in &["apple_music", "spotify", "youtube_music", "tidal"] {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sync_connections WHERE user_id = ? AND provider = ? AND status = 'active'",
                rusqlite::params![uid, pid],
                |r| r.get(0)
            ).unwrap_or(0);
            if count > 0 {
                return Some(pid.to_string());
            }
        }
        None
    }).await.ok().flatten()
}

/// Returns the FIRST cloud provider fallback for this user (ignoring
/// local). Used when local has no matches for the DJ prompt and we want
/// to reach into a streaming service before giving up.
pub async fn preferred_cloud_music_provider(state: &Arc<AppState>, uid: i64) -> Option<String> {
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        for pid in &["apple_music", "spotify", "youtube_music", "tidal"] {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sync_connections WHERE user_id = ? AND provider = ? AND status = 'active'",
                rusqlite::params![uid, pid],
                |r| r.get(0)
            ).unwrap_or(0);
            if count > 0 {
                return Some(pid.to_string());
            }
        }
        None
    }).await.ok().flatten()
}

/// Fuzzy-match one "Song - Artist" query against this user's local
/// library. Returns a single best-scoring track in the same JSON shape
/// as the cloud search results so the DJ endpoint can hand them back
/// uniformly. Scoring: title-match > artist-match > album-match, with
/// bonus points when BOTH halves of the query appear in the row.
pub async fn search_local_track(state: &Arc<AppState>, uid: i64, query: &str) -> Option<serde_json::Value> {
    let db = state.db_path.clone();
    let q = query.to_string();

    // Split "Song - Artist" or "Artist - Song" — try both halves as soft filters.
    let (q_song, q_artist) = match q.split_once(" - ") {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (q.clone(), String::new()),
    };
    let q_full_low = q.to_lowercase();
    let q_song_low = q_song.to_lowercase();
    let q_artist_low = q_artist.to_lowercase();

    tokio::task::spawn_blocking(move || -> Option<serde_json::Value> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        // Broad candidate fetch: any row that contains either half of the
        // query in title / artist / album. Scoring happens in Rust.
        let like_song = format!("%{}%", q_song_low);
        let like_artist = if q_artist_low.is_empty() { "%".to_string() } else { format!("%{}%", q_artist_low) };
        let like_full = format!("%{}%", q_full_low);
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, album, duration_ms \
             FROM local_music_tracks \
             WHERE user_id = ? AND ( \
               LOWER(COALESCE(title,'')) LIKE ?2 OR \
               LOWER(COALESCE(artist,'')) LIKE ?3 OR \
               LOWER(COALESCE(album,'')) LIKE ?2 OR \
               LOWER(COALESCE(title,'')) LIKE ?4 \
             ) LIMIT 40"
        ).ok()?;
        let rows: Vec<(i64, Option<String>, Option<String>, Option<String>, Option<i64>)> =
            stmt.query_map(
                rusqlite::params![uid, like_song, like_artist, like_full],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            ).ok()?
             .filter_map(|x| x.ok())
             .collect();

        // Score each candidate — favor exact token hits, reward both halves
        // matching, penalize near-empty titles.
        let mut best: Option<(i32, (i64, Option<String>, Option<String>, Option<String>, Option<i64>))> = None;
        for row in rows {
            let t = row.1.clone().unwrap_or_default().to_lowercase();
            let a = row.2.clone().unwrap_or_default().to_lowercase();
            let al = row.3.clone().unwrap_or_default().to_lowercase();
            let mut score: i32 = 0;
            if !q_song_low.is_empty() && t.contains(&q_song_low) { score += 10; }
            if !q_artist_low.is_empty() && a.contains(&q_artist_low) { score += 8; }
            if !q_song_low.is_empty() && al.contains(&q_song_low) { score += 3; }
            if t == q_song_low { score += 5; }
            if a == q_artist_low && !q_artist_low.is_empty() { score += 5; }
            if t.is_empty() { score -= 5; }
            if score <= 0 { continue; }
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, row));
            }
        }
        let (_, (id, title, artist, album, dur)) = best?;
        Some(serde_json::json!({
            "query": q,
            "id": id,
            "name": title,
            "artist": artist,
            "album": album,
            "duration_ms": dur,
            "play_url": format!("/api/music/local/file/{}", id),
            "provider": "local",
        }))
    }).await.ok().flatten()
}

/// Load OAuth access_token for a user+provider from the oauth_tokens table.
async fn load_oauth_access_token(state: &Arc<AppState>, uid: i64, provider: &str) -> Option<String> {
    let db = state.db_path.clone();
    let prov = provider.to_string();
    tokio::task::spawn_blocking(move || -> Option<String> {
        let conn = rusqlite::Connection::open(&db).ok()?;
        conn.query_row(
            "SELECT access_token FROM oauth_tokens WHERE user_id = ? AND provider = ?",
            rusqlite::params![uid, prov],
            |r| r.get::<_, String>(0),
        ).ok().filter(|s| !s.is_empty())
    }).await.ok().flatten()
}

/// Apple Music single-hit search — extracted from the DJ inline loop so
/// both the "primary apple_music" path and the "local → apple fallback"
/// path share the same request shape.
async fn do_apple_search(
    client: &reqwest::Client,
    developer_token: &str,
    music_user_token: &str,
    storefront: &str,
    query: &str,
) -> Option<serde_json::Value> {
    let url = format!(
        "https://api.music.apple.com/v1/catalog/{}/search?types=songs&limit=1&term={}",
        storefront, url_encode_local(query)
    );
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", developer_token))
        .header("Music-User-Token", music_user_token)
        .header("Origin", "https://music.apple.com")
        .timeout(Duration::from_secs(10))
        .send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let j: serde_json::Value = resp.json().await.ok()?;
    let song = j.get("results")?.get("songs")?.get("data")?
        .as_array()?.first()?.clone();
    let attrs = song.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
    let id = song.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(serde_json::json!({
        "query": query,
        "id": id,
        "name": attrs.get("name"),
        "artist": attrs.get("artistName"),
        "album": attrs.get("albumName"),
        "url": attrs.get("url"),
        "play_url": build_play_url("apple_music", song.get("id").and_then(|v| v.as_str()).unwrap_or(""), attrs.get("url").and_then(|u| u.as_str())),
        "artwork": attrs.get("artwork").and_then(|a| a.get("url")),
        "provider": "apple_music",
    }))
}

/// Spotify search — returns list of Track objects similar to Apple Music's format.
/// Each has: id, name, artist, album, url, artwork (spotify://track/ID usable as play URL).
async fn spotify_search(
    client: &reqwest::Client,
    access_token: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "https://api.spotify.com/v1/search?q={}&type=track&limit={}",
        url_encode_local(query), limit.min(50)
    );
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(15))
        .send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Spotify {}", resp.status()));
    }
    let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let tracks: Vec<serde_json::Value> = j.get("tracks")
        .and_then(|t| t.get("items"))
        .and_then(|i| i.as_array()).cloned().unwrap_or_default();
    Ok(tracks.into_iter().map(|t| {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        serde_json::json!({
            "id": id,
            "name": t.get("name"),
            "artist": t.get("artists").and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("name")),
            "album": t.get("album").and_then(|a| a.get("name")),
            "url": t.get("external_urls").and_then(|u| u.get("spotify")),
            "uri": format!("spotify:track:{}", id),
            "play_url": format!("spotify:track:{}", id),
            "artwork": t.get("album").and_then(|a| a.get("images"))
                .and_then(|i| i.as_array()).and_then(|imgs| imgs.first())
                .and_then(|img| img.get("url")),
            "provider": "spotify",
        })
    }).collect())
}

/// Build a provider-appropriate play URL that the PWA / browser can launch.
pub fn build_play_url(provider: &str, track_id: &str, fallback_url: Option<&str>) -> String {
    match provider {
        "apple_music" => format!("music://music.apple.com/us/song/{}", track_id),
        "spotify" => format!("spotify:track:{}", track_id),
        "youtube_music" => format!("https://music.youtube.com/watch?v={}", track_id),
        "tidal" => format!("tidal://track/{}", track_id),
        _ => fallback_url.map(|s| s.to_string()).unwrap_or_default(),
    }
}

/// YouTube Music search via YouTube Data API v3.
/// Quota cost: 100 units per search. Default daily quota is 10,000 — so ~100 searches/day.
/// A 15-track DJ run = 1500 units, so ~6 runs/day at default quota.
async fn youtube_music_search(
    client: &reqwest::Client,
    access_token: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    // Restrict to videoCategoryId=10 (Music) for better relevance.
    let url = format!(
        "https://www.googleapis.com/youtube/v3/search?part=snippet&q={}&type=video&videoCategoryId=10&maxResults={}",
        url_encode_local(query), limit.min(10)
    );
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(15))
        .send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("YouTube Music API {}: {}", status, body.chars().take(200).collect::<String>()));
    }
    let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let items: Vec<serde_json::Value> = j.get("items")
        .and_then(|i| i.as_array()).cloned().unwrap_or_default();
    Ok(items.into_iter().filter_map(|item| {
        let video_id = item.get("id")?.get("videoId")?.as_str()?.to_string();
        let snip = item.get("snippet").cloned().unwrap_or_default();
        let title = snip.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let channel = snip.get("channelTitle").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let thumb = snip.get("thumbnails").and_then(|t| t.get("medium"))
            .and_then(|m| m.get("url")).and_then(|u| u.as_str()).unwrap_or("").to_string();
        Some(serde_json::json!({
            "id": video_id,
            "name": title,
            "artist": channel,
            "album": null,
            "url": format!("https://music.youtube.com/watch?v={}", video_id),
            "play_url": format!("https://music.youtube.com/watch?v={}", video_id),
            "artwork": thumb,
            "provider": "youtube_music",
        }))
    }).collect())
}

/// Create a YouTube playlist and add the given videos to it.
/// Returns the playlist ID on success.
/// Quota cost: 50 (insert) + 50*N (add each item). 15 tracks = ~800 units.
async fn youtube_music_create_playlist(
    client: &reqwest::Client,
    access_token: &str,
    name: &str,
    description: &str,
    video_ids: &[String],
) -> Result<String, String> {
    // Step 1: create the playlist
    let body = serde_json::json!({
        "snippet": {
            "title": name,
            "description": description,
        },
        "status": { "privacyStatus": "private" },
    });
    let url = "https://www.googleapis.com/youtube/v3/playlists?part=snippet,status";
    let resp = client.post(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let s = resp.status();
        let b = resp.text().await.unwrap_or_default();
        return Err(format!("YouTube playlist create {}: {}", s, b.chars().take(200).collect::<String>()));
    }
    let j: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let playlist_id = j.get("id").and_then(|v| v.as_str())
        .ok_or("no playlist id in response")?.to_string();

    // Step 2: add each video as a playlistItem
    let add_url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet";
    for vid in video_ids {
        let item_body = serde_json::json!({
            "snippet": {
                "playlistId": playlist_id,
                "resourceId": {"kind": "youtube#video", "videoId": vid}
            }
        });
        let _ = client.post(add_url)
            .header("Authorization", format!("Bearer {}", access_token))
            .json(&item_body)
            .timeout(std::time::Duration::from_secs(10))
            .send().await;
    }
    Ok(playlist_id)
}

// ── Spotify Connect playback ────────────────────────────────────────────────
//
// Uses the Spotify Web API's /v1/me/player/play endpoint. Spotify Connect
// finds whichever device the user is signed into — the phone app, desktop
// app, a Web Player tab, Sonos with Spotify, etc — and routes playback
// there. No Spotify Premium required for Connect itself, though most
// playback transfer features require Premium.

#[derive(serde::Deserialize)]
pub struct SpotifyPlayRequest {
    pub token: String,
    pub uri: String,          // "spotify:track:ID" or "spotify:playlist:ID"
    pub device_id: Option<String>,
}

pub async fn handle_spotify_play(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SpotifyPlayRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let uid = principal.user_id();
    let Some(tok) = load_oauth_access_token(&state, uid, "spotify").await else {
        return Ok(Json(serde_json::json!({
            "error": "Spotify not authorized",
            "hint": "Complete the OAuth flow in Sync settings."
        })));
    };

    // If no device_id given, list devices and pick the active one (or first available)
    let target_device = match req.device_id.clone() {
        Some(d) => Some(d),
        None => {
            let resp = state.client.get("https://api.spotify.com/v1/me/player/devices")
                .header("Authorization", format!("Bearer {}", tok))
                .timeout(std::time::Duration::from_secs(10))
                .send().await;
            let mut picked: Option<String> = None;
            if let Ok(r) = resp {
                if r.status().is_success() {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if let Some(devs) = j.get("devices").and_then(|d| d.as_array()) {
                            // Prefer the active device
                            let active = devs.iter().find(|d| d.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false));
                            let first = devs.first();
                            if let Some(d) = active.or(first) {
                                picked = d.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
            picked
        }
    };

    // Build request body
    let body = if req.uri.starts_with("spotify:track:") {
        serde_json::json!({"uris": [&req.uri]})
    } else {
        // context_uri for playlist/album/artist
        serde_json::json!({"context_uri": &req.uri})
    };

    let url = match target_device {
        Some(d) => format!("https://api.spotify.com/v1/me/player/play?device_id={}", d),
        None => "https://api.spotify.com/v1/me/player/play".to_string(),
    };

    let resp = state.client.put(&url)
        .header("Authorization", format!("Bearer {}", tok))
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send().await
        .map_err(|e| {
            warn!("[spotify-play] {}", e);
            axum::http::StatusCode::BAD_GATEWAY
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(Json(serde_json::json!({
            "error": "No active Spotify device",
            "hint": "Open Spotify on your phone, desktop, or web player and start playing anything. Then try again.",
        })));
    }
    if !status.is_success() {
        let b = resp.text().await.unwrap_or_default();
        return Ok(Json(serde_json::json!({
            "error": format!("Spotify returned {}", status),
            "detail": b.chars().take(200).collect::<String>(),
            "hint": if status == reqwest::StatusCode::FORBIDDEN {
                "Playback transfer requires Spotify Premium. Free users can still use 'Open in Spotify' links."
            } else { "" },
        })));
    }

    info!("[spotify-play] started: uri={}", req.uri);
    Ok(Json(serde_json::json!({"success": true, "uri": req.uri})))
}

/// Return the user's Spotify access token so the Web Playback SDK can
/// initialize. Same-origin only — the token is scoped to the user's
/// authenticated Syntaur session.
pub async fn handle_spotify_token(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal_scoped(&state, token, "music").await?;
    let tok = load_oauth_access_token(&state, principal.user_id(), "spotify").await;
    match tok {
        Some(t) => Ok(Json(serde_json::json!({"access_token": t}))),
        None => Ok(Json(serde_json::json!({"error": "spotify not authorized"}))),
    }
}

// ── User music preferences (persistent DJ memory) ──────────────────────────

#[derive(serde::Deserialize)]
pub struct MusicPrefSaveRequest {
    pub token: String,
    pub category: String,  // "like" | "dislike" | "note"
    pub kind: Option<String>,
    pub value: String,
    pub track_id: Option<String>,
    pub provider: Option<String>,
    pub source: Option<String>,
}

pub async fn handle_music_pref_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MusicPrefSaveRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let uid = principal.user_id();
    if req.value.trim().is_empty() { return Err(axum::http::StatusCode::BAD_REQUEST); }
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let cat = req.category.clone();
    let kind = req.kind.clone();
    let val = req.value.clone();
    let tid = req.track_id.clone();
    let prov = req.provider.clone();
    let src = req.source.unwrap_or_else(|| "manual".to_string());
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO user_music_preferences (user_id, category, kind, value, track_id, provider, source, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, cat, kind, val, tid, prov, src, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn handle_music_prefs_list(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal_scoped(&state, token, "music").await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let limit: i64 = params.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    let db = state.db_path.clone();
    let rows: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        if let Ok(conn) = rusqlite::Connection::open(&db) {
            let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
                ("SELECT id, category, kind, value, track_id, provider, source, created_at \
                  FROM user_music_preferences WHERE user_id = ? ORDER BY created_at DESC LIMIT ?".to_string(),
                 vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(limit)])
            } else {
                ("SELECT id, category, kind, value, track_id, provider, source, created_at \
                  FROM user_music_preferences ORDER BY created_at DESC LIMIT ?".to_string(),
                 vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql>])
            };
            if let Ok(mut stmt) = conn.prepare(&sql) {
                let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
                if let Ok(rs) = stmt.query_map(params_refs.as_slice(), |r| Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?,
                    "category": r.get::<_, String>(1)?,
                    "kind": r.get::<_, Option<String>>(2)?,
                    "value": r.get::<_, String>(3)?,
                    "track_id": r.get::<_, Option<String>>(4)?,
                    "provider": r.get::<_, Option<String>>(5)?,
                    "source": r.get::<_, Option<String>>(6)?,
                    "created_at": r.get::<_, i64>(7)?,
                }))) {
                    for row in rs.flatten() { out.push(row); }
                }
            }
        }
        out
    }).await.unwrap_or_default();
    Ok(Json(serde_json::json!({"preferences": rows})))
}

pub async fn handle_music_pref_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(pref_id): axum::extract::Path<i64>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal_scoped(&state, token, "music").await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        if let Some(uid) = scope {
            conn.execute("DELETE FROM user_music_preferences WHERE id = ? AND user_id = ?",
                rusqlite::params![pref_id, uid]).map_err(|e| e.to_string())?;
        } else {
            conn.execute("DELETE FROM user_music_preferences WHERE id = ?",
                rusqlite::params![pref_id]).map_err(|e| e.to_string())?;
        }
        Ok(())
    }).await;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// Fetch the user's recent preferences formatted as a context string
/// for the DJ LLM prompt.
async fn load_prefs_context(state: &Arc<AppState>, uid: i64, limit: i64) -> String {
    let db = state.db_path.clone();
    let rows: Vec<(String, String)> = tokio::task::spawn_blocking(move || -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Ok(conn) = rusqlite::Connection::open(&db) {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT category, value FROM user_music_preferences                  WHERE user_id = ? ORDER BY created_at DESC LIMIT ?"
            ) {
                if let Ok(rs) = stmt.query_map(rusqlite::params![uid, limit], |r| Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?
                ))) {
                    for row in rs.flatten() { out.push(row); }
                }
            }
        }
        out
    }).await.unwrap_or_default();
    if rows.is_empty() { return String::new(); }
    let mut likes = Vec::new();
    let mut dislikes = Vec::new();
    let mut notes = Vec::new();
    for (cat, val) in rows {
        match cat.as_str() {
            "like" => likes.push(val),
            "dislike" => dislikes.push(val),
            _ => notes.push(val),
        }
    }
    let mut out = String::new();
    if !likes.is_empty() { out.push_str(&format!("User likes: {}. ", likes.join("; "))); }
    if !dislikes.is_empty() { out.push_str(&format!("User dislikes: {}. ", dislikes.join("; "))); }
    if !notes.is_empty() { out.push_str(&format!("Notes: {}. ", notes.join("; "))); }
    out
}

// ── Music ducking during TTS ───────────────────────────────────────────────
// Shared ducking state: when TTS is speaking, music player clients attenuate
// their volume. Set via /api/music/duck {state}, read via /api/music/duck_state
// or as the `ducking` field on /api/music/now_playing.

static DUCKING_STATE: tokio::sync::OnceCell<tokio::sync::RwLock<DuckingState>> = tokio::sync::OnceCell::const_new();

#[derive(Clone, Debug, Default)]
pub struct DuckingState {
    pub active: bool,
    pub until_ts: i64,  // unix epoch; auto-unduck after this time
}

async fn get_duck_state() -> DuckingState {
    let cell = DUCKING_STATE.get_or_init(|| async { tokio::sync::RwLock::new(DuckingState::default()) }).await;
    let s = cell.read().await;
    // Auto-expire if past until_ts
    let now = chrono::Utc::now().timestamp();
    if s.active && s.until_ts > 0 && now > s.until_ts {
        return DuckingState::default();
    }
    DuckingState { active: s.active, until_ts: s.until_ts }
}

async fn set_duck_state(active: bool, duration_secs: i64) {
    let cell = DUCKING_STATE.get_or_init(|| async { tokio::sync::RwLock::new(DuckingState::default()) }).await;
    let mut w = cell.write().await;
    w.active = active;
    w.until_ts = if active && duration_secs > 0 {
        chrono::Utc::now().timestamp() + duration_secs
    } else { 0 };
}

#[derive(serde::Deserialize)]
pub struct DuckRequest {
    pub token: String,
    pub state: String,  // "on" | "off"
    pub duration_secs: Option<i64>,
}

pub async fn handle_music_duck(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DuckRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let _ = crate::resolve_principal_scoped(&state, &req.token, "music").await?;
    let active = req.state == "on";
    let duration = req.duration_secs.unwrap_or(if active { 30 } else { 0 });
    // This actually lowers volume on Spotify Connect + HA media_players
    trigger_duck_with_state(&state, active, duration).await;
    Ok(Json(serde_json::json!({"ok": true, "state": req.state})))
}

pub async fn handle_music_duck_state(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let _ = crate::resolve_principal_scoped(&state, token, "music").await?;
    let ds = get_duck_state().await;
    Ok(Json(serde_json::json!({
        "ducking": ds.active,
        "until_ts": ds.until_ts,
    })))
}

/// Public helper for in-process callers (e.g. voice_api TTS handler) to
/// trigger ducking without going through the HTTP endpoint.
pub async fn trigger_duck(active: bool, duration_secs: i64) {
    set_duck_state(active, duration_secs).await;
    info!("[music-duck] (in-process) {} for {}s", if active {"duck"} else {"unduck"}, duration_secs);
    // Best-effort broadcast to bridge so phone PWA also attenuates
    let client = reqwest::Client::new();
    let event_type = if active { "duck" } else { "unduck" };
    let cmd = serde_json::json!({"type": event_type, "message": "TTS"});
    let _ = client.post("http://127.0.0.1:18804/command")
        .json(&cmd)
        .timeout(std::time::Duration::from_secs(2))
        .send().await;
}

/// Like trigger_duck but ALSO calls music-service volume APIs to actually
/// lower the audio. Use this from gateway code paths that have AppState.
pub async fn trigger_duck_with_state(state: &Arc<AppState>, active: bool, duration_secs: i64) {
    trigger_duck(active, duration_secs).await;
    apply_ducking_to_all_users(state, active).await;
    // Also emit local duck/unduck for /music tabs playing through Spotify SDK / YT IFrame
    emit_local_event(LocalEvent {
        event_type: if active { "duck".to_string() } else { "unduck".to_string() },
        provider: None,
        track_id: None,
        uri: None,
        name: None,
        artist: None,
        volume: Some(if active { 0.2 } else { 1.0 }),
        message: None,
    }).await;
    if active && duration_secs > 0 {
        // Schedule auto-unduck in case no explicit unduck arrives
        let state_clone = state.clone();
        let dur = duration_secs;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(dur as u64)).await;
            let now_ds = get_duck_state().await;
            // Only unduck if state hasn't been refreshed by another call
            if now_ds.until_ts > 0 && chrono::Utc::now().timestamp() >= now_ds.until_ts {
                set_duck_state(false, 0).await;
                apply_ducking_to_all_users(&state_clone, false).await;
                info!("[music-duck] auto-restored after {}s", dur);
            }
        });
    }
}

// ── iOS Shortcut integration for music ducking ──────────────────────────────
//
// iOS doesn't let web pages programmatically lower another app's volume.
// The closest workaround: a one-time-installed Shortcut with the "Set Music
// Volume" action (iOS 17+). The PWA fires the Shortcut via URL scheme
// (shortcuts://run-shortcut?name=...) on every duck/unduck event.

/// Returns simple {volume: 20|100} JSON suitable for an iOS Shortcut
/// to consume via "Get Contents of URL" → "Get Dictionary Value".
/// No auth required so the Shortcut can poll without managing tokens.
pub async fn handle_duck_volume_simple() -> Json<serde_json::Value> {
    let ds = get_duck_state().await;
    let volume = if ds.active { 20 } else { 100 };
    Json(serde_json::json!({
        "volume": volume,
        "ducking": ds.active,
    }))
}

/// Returns the Shortcut setup guide — text steps for building or
/// installing the Syntaur Music Volume Shortcut.
pub async fn handle_shortcut_setup_guide(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let _ = crate::resolve_principal_scoped(&state, token, "music").await?;
    // The "host" from request would be ideal but we infer from config
    let host = std::env::var("SYNTAUR_PUBLIC_HOST").unwrap_or_else(|_| "your-syntaur-host".to_string());
    Ok(Json(serde_json::json!({
        "shortcut_name": "Syntaur Music Volume",
        "trigger_url_scheme": "shortcuts://run-shortcut?name=Syntaur+Music+Volume&input=text&text=duck",
        "duck_state_url": format!("https://{}/api/music/duck/v", host),
        "icloud_template_url": null,
        "manual_steps": [
            "Open the Shortcuts app on your iPhone.",
            "Tap + (top-right) to create a new Shortcut.",
            "Name it exactly: Syntaur Music Volume",
            "Add action: Get Contents of URL — set URL to your Syntaur duck-state URL (shown above).",
            "Add action: Get Dictionary Value — Get [volume] from [Contents of URL].",
            "Add action: Set Music Volume — set to [Dictionary Value]/100 (e.g. drag the variable into the volume slider position).",
            "Save the Shortcut.",
            "(Optional) Open Settings → Shortcuts → toggle ON 'Allow Untrusted Shortcuts' if installing from a share link.",
            "Test it: in the Shortcuts app, tap your new Shortcut. Music volume should drop to 20% if Syntaur is currently ducking.",
            "Once installed and named exactly 'Syntaur Music Volume', the Syntaur PWA will automatically run it whenever the AI voice speaks — your music drops, then restores when the voice ends."
        ],
        "note": "iOS 17+ required for the Set Music Volume action. The PWA fires the Shortcut via URL scheme on every duck/unduck event from the gateway. After this one-time setup, ducking is fully automatic.",
    })))
}

// ── Gateway-side automatic volume control during ducking ───────────────────
//
// When trigger_duck() fires, we proactively call music-service APIs to lower
// the actual volume on whatever device is playing. No client involvement, no
// iOS Shortcut needed for Spotify or HA-controlled speakers. Apple Music on
// phone is the one provider with no remote-volume API — for that niche case
// the user needs the Shortcut documented in /sync.

static PREVIOUS_VOLUMES: tokio::sync::OnceCell<tokio::sync::RwLock<std::collections::HashMap<String, i64>>> = tokio::sync::OnceCell::const_new();

async fn store_previous_volume(key: &str, vol: i64) {
    let cell = PREVIOUS_VOLUMES.get_or_init(|| async { tokio::sync::RwLock::new(std::collections::HashMap::new()) }).await;
    let mut w = cell.write().await;
    w.insert(key.to_string(), vol);
}

async fn pop_previous_volume(key: &str) -> Option<i64> {
    let cell = PREVIOUS_VOLUMES.get_or_init(|| async { tokio::sync::RwLock::new(std::collections::HashMap::new()) }).await;
    let mut w = cell.write().await;
    w.remove(key)
}

/// Try to lower (or restore) volume on every connected music service for
/// every user with active credentials. Best-effort; failures logged not raised.
pub async fn apply_ducking_to_all_users(state: &Arc<AppState>, ducking: bool) {
    // Find all users with active sync_connections for any music provider
    let db = state.db_path.clone();
    let users: Vec<i64> = tokio::task::spawn_blocking(move || -> Vec<i64> {
        let mut out = Vec::new();
        if let Ok(conn) = rusqlite::Connection::open(&db) {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT DISTINCT user_id FROM sync_connections                  WHERE provider IN ('spotify','home_assistant','apple_music','phone_music_pwa')                  AND status='active'"
            ) {
                if let Ok(rs) = stmt.query_map([], |r| r.get::<_, i64>(0)) {
                    for u in rs.flatten() { out.push(u); }
                }
            }
        }
        out
    }).await.unwrap_or_default();

    for uid in users {
        // Spotify ducking
        if let Some(tok) = load_oauth_access_token(state, uid, "spotify").await {
            apply_spotify_ducking(state, uid, &tok, ducking).await;
        }
        // HA ducking
        if let Some((url, ha_tok)) = load_ha(state, uid).await {
            apply_ha_ducking(state, uid, &url, &ha_tok, ducking).await;
        }
    }
}

async fn apply_spotify_ducking(state: &Arc<AppState>, uid: i64, token: &str, ducking: bool) {
    let key = format!("spotify:{}", uid);
    if ducking {
        // Capture current volume, then drop to 20
        let resp = state.client.get("https://api.spotify.com/v1/me/player")
            .header("Authorization", format!("Bearer {}", token))
            .timeout(Duration::from_secs(5))
            .send().await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    let prev = j.get("device").and_then(|d| d.get("volume_percent"))
                        .and_then(|v| v.as_i64()).unwrap_or(75);
                    store_previous_volume(&key, prev).await;
                    let _ = state.client.put("https://api.spotify.com/v1/me/player/volume?volume_percent=20")
                        .header("Authorization", format!("Bearer {}", token))
                        .timeout(Duration::from_secs(5))
                        .send().await;
                    info!("[duck] Spotify user {} 20% (was {}%)", uid, prev);
                }
            }
        }
    } else {
        let prev = pop_previous_volume(&key).await.unwrap_or(75);
        let url = format!("https://api.spotify.com/v1/me/player/volume?volume_percent={}", prev);
        let _ = state.client.put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .timeout(Duration::from_secs(5))
            .send().await;
        info!("[duck] Spotify user {} restored to {}%", uid, prev);
    }
}

async fn apply_ha_ducking(state: &Arc<AppState>, uid: i64, ha_url: &str, ha_token: &str, ducking: bool) {
    // Find an actively-playing media_player
    let states_url = format!("{}/api/states", ha_url.trim_end_matches('/'));
    let resp = state.client.get(&states_url)
        .header("Authorization", format!("Bearer {}", ha_token))
        .timeout(Duration::from_secs(5))
        .send().await;
    let arr: serde_json::Value = match resp {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => return,
    };
    let states = match arr.as_array() { Some(s) => s, None => return };
    for s in states {
        let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
        if !eid.starts_with("media_player.") { continue; }
        let st = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
        if st != "playing" { continue; }
        let attrs = s.get("attributes").cloned().unwrap_or_default();
        let key = format!("ha:{}:{}", uid, eid);
        if ducking {
            let prev_vol = attrs.get("volume_level").and_then(|v| v.as_f64()).unwrap_or(0.5);
            store_previous_volume(&key, (prev_vol * 100.0) as i64).await;
            let svc_url = format!("{}/api/services/media_player/volume_set", ha_url.trim_end_matches('/'));
            let _ = state.client.post(&svc_url)
                .header("Authorization", format!("Bearer {}", ha_token))
                .json(&serde_json::json!({"entity_id": eid, "volume_level": 0.2}))
                .timeout(Duration::from_secs(5))
                .send().await;
            info!("[duck] HA {} 0.2 (was {})", eid, prev_vol);
        } else {
            let prev = pop_previous_volume(&key).await.unwrap_or(50);
            let svc_url = format!("{}/api/services/media_player/volume_set", ha_url.trim_end_matches('/'));
            let _ = state.client.post(&svc_url)
                .header("Authorization", format!("Bearer {}", ha_token))
                .json(&serde_json::json!({"entity_id": eid, "volume_level": (prev as f64) / 100.0}))
                .timeout(Duration::from_secs(5))
                .send().await;
            info!("[duck] HA {} restored to {}", eid, prev as f64 / 100.0);
        }
    }
}

// ── Local playback SSE channel ──────────────────────────────────────────────
// /music tabs subscribe to /api/music/local_events. When the music tool
// targets "this_computer" or ducking fires, we broadcast JSON events for
// the client-side audio players (Spotify Web Playback SDK, YouTube IFrame,
// HTML5 audio) to handle. Zero external API round trip for the play command
// itself — the /music tab IS the player.

#[derive(Clone, Debug, serde::Serialize)]
pub struct LocalEvent {
    #[serde(rename = "type")]
    pub event_type: String,   // "play" | "pause" | "next" | "duck" | "unduck" | "volume"
    pub provider: Option<String>,   // "spotify" | "youtube_music" | "apple_music"
    pub track_id: Option<String>,
    pub uri: Option<String>,        // e.g. "spotify:track:ID" or YouTube videoId
    pub name: Option<String>,
    pub artist: Option<String>,
    pub volume: Option<f64>,        // 0.0-1.0
    pub message: Option<String>,
}

static LOCAL_EVENT_TX: tokio::sync::OnceCell<tokio::sync::broadcast::Sender<LocalEvent>> = tokio::sync::OnceCell::const_new();

async fn get_local_event_tx() -> tokio::sync::broadcast::Sender<LocalEvent> {
    LOCAL_EVENT_TX.get_or_init(|| async {
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        tx
    }).await.clone()
}

/// Broadcast a local playback event to all /music tabs.
pub async fn emit_local_event(ev: LocalEvent) {
    let tx = get_local_event_tx().await;
    let _ = tx.send(ev);
}

/// SSE stream of local playback events for /music tabs.
pub async fn handle_local_events(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use axum::response::IntoResponse;

    // SSE EventSource can't attach Authorization; accept stream_token,
    // Bearer header, or legacy ?token= (deprecated) via the shared
    // helper. Scope check enforced after.
    let principal = match crate::resolve_principal_for_stream(&state, &headers, &params, uri.path()).await {
        Ok((p, _via_stream)) => p,
        Err(s) => return (s, "unauthorized").into_response(),
    };
    if principal.require_scope("music").is_err() {
        return (axum::http::StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    let tx = get_local_event_tx().await;
    let mut rx = tx.subscribe();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(Event::default().data(r#"{"type":"connected"}"#));
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let json = serde_json::to_string(&ev).unwrap_or_default();
                    yield Ok(Event::default().data(json));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(20)))
        .into_response()
}

/// Is there at least one /music tab subscribed (i.e. "this computer" available)?
pub async fn this_computer_available() -> bool {
    let tx = get_local_event_tx().await;
    tx.receiver_count() > 0
}

/// Play a track on the active /music tab via the Web Playback / IFrame SDKs.
/// Called by the music tool when target == "this_computer".
pub async fn play_on_this_computer(
    provider: &str,
    track_id: &str,
    name: &str,
    artist: &str,
) -> bool {
    let tx = get_local_event_tx().await;
    if tx.receiver_count() == 0 { return false; }
    let uri = match provider {
        "spotify" => format!("spotify:track:{}", track_id),
        "youtube_music" => track_id.to_string(),  // raw videoId
        _ => track_id.to_string(),
    };
    let _ = tx.send(LocalEvent {
        event_type: "play".to_string(),
        provider: Some(provider.to_string()),
        track_id: Some(track_id.to_string()),
        uri: Some(uri),
        name: Some(name.to_string()),
        artist: Some(artist.to_string()),
        volume: None,
        message: None,
    });
    true
}

/// Check if the user has set "this_computer" as their default playback target.
pub async fn preferred_target_is_this_computer(db_path: &std::path::Path, user_id: i64) -> bool {
    let db = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> bool {
        let Ok(conn) = rusqlite::Connection::open(&db) else { return false; };
        let cred: Option<String> = conn.query_row(
            "SELECT metadata FROM sync_connections WHERE user_id = ? AND provider = 'music_preferences'",
            rusqlite::params![user_id], |r| r.get(0)
        ).ok();
        if let Some(s) = cred {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                return v.get("preferred_target").and_then(|x| x.as_str()) == Some("this_computer");
            }
        }
        false
    }).await.unwrap_or(false)
}

