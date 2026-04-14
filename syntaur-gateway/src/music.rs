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
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
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

    Ok(Json(serde_json::json!({
        "state": "off",
        "source": "none",
        "hint": "Nothing playing. Ask Peter to play something, or connect Apple Music / pair your phone in Sync.",
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
    let principal = crate::resolve_principal(&state, &req.token).await?;
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
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let mut speakers: Vec<serde_json::Value> = Vec::new();

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
    let principal = crate::resolve_principal(&state, &req.token).await?;
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
    let principal = crate::resolve_principal(&state, &req.token).await?;
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
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let Some((dev_token, mut_token, storefront)) = load_apple_music(&state, uid).await else {
        return Ok(Json(serde_json::json!({
            "error":"Apple Music not connected",
            "hint":"Connect Apple Music in Sync settings for DJ mode."
        })));
    };

    let count = req.count.unwrap_or(15).min(30);

    // Step 1: ask the LLM for track ideas matching the prompt
    let llm_prompt = format!(
        "You are a DJ. Build a playlist based on this request: \"{}\"\n\n         Return ONLY a JSON array of {} strings, each a \"SONG - ARTIST\" search query. \
         No markdown fences, no prose, just the JSON array. Example:\n         [\"Kind of Blue - Miles Davis\", \"Take Five - Dave Brubeck\"]",
        req.prompt, count
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

    // Step 2: search Apple Music for each idea in parallel
    let client = state.client.clone();
    let dev = dev_token.clone();
    let mut_ = mut_token.clone();
    let sf = storefront.clone();
    let mut search_handles = Vec::new();
    for idea in &ideas {
        let cli = client.clone();
        let d = dev.clone();
        let u = mut_.clone();
        let s = sf.clone();
        let q = idea.clone();
        search_handles.push(tokio::spawn(async move {
            let url = format!(
                "https://api.music.apple.com/v1/catalog/{}/search?types=songs&limit=1&term={}",
                s, url_encode_local(&q)
            );
            let resp = cli.get(&url)
                .header("Authorization", format!("Bearer {}", d))
                .header("Music-User-Token", u)
                .header("Origin", "https://music.apple.com")
                .timeout(Duration::from_secs(10))
                .send().await.ok()?;
            if !resp.status().is_success() { return None; }
            let j: serde_json::Value = resp.json().await.ok()?;
            let song = j.get("results")?
                .get("songs")?.get("data")?
                .as_array()?.first()?.clone();
            Some((q, song))
        }));
    }

    let mut tracks: Vec<serde_json::Value> = Vec::new();
    for h in search_handles {
        if let Ok(Some((query, song))) = h.await {
            let attrs = song.get("attributes").cloned().unwrap_or(serde_json::Value::Null);
            tracks.push(serde_json::json!({
                "query": query,
                "id": song.get("id"),
                "name": attrs.get("name"),
                "artist": attrs.get("artistName"),
                "album": attrs.get("albumName"),
                "url": attrs.get("url"),
                "artwork": attrs.get("artwork").and_then(|a| a.get("url")),
            }));
        }
    }

    // Step 3 (optional): create an actual Apple Music playlist
    let mut playlist_id: Option<String> = None;
    if req.create_playlist.unwrap_or(false) && !tracks.is_empty() {
        let track_data: Vec<serde_json::Value> = tracks.iter().filter_map(|t| {
            let id = t.get("id")?.as_str()?;
            Some(serde_json::json!({"id": id, "type": "songs"}))
        }).collect();
        let body = serde_json::json!({
            "attributes": {
                "name": format!("Syntaur DJ: {}", req.prompt.chars().take(40).collect::<String>()),
                "description": format!("Generated by Syntaur from prompt: {}", req.prompt),
            },
            "relationships": {
                "tracks": {"data": track_data}
            }
        });
        let url = format!("https://api.music.apple.com/v1/me/library/playlists");
        let resp = state.client.post(&url)
            .header("Authorization", format!("Bearer {}", dev_token))
            .header("Music-User-Token", mut_token)
            .header("Origin", "https://music.apple.com")
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send().await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    playlist_id = j.get("data").and_then(|d| d.as_array())
                        .and_then(|a| a.first())
                        .and_then(|p| p.get("id"))
                        .and_then(|i| i.as_str())
                        .map(|s| s.to_string());
                }
            } else {
                warn!("[music-dj] playlist create failed: {}", r.status());
            }
        }
    }

    info!("[music-dj] prompt=\"{}\" ideas={} tracks={} playlist={:?}",
        req.prompt.chars().take(50).collect::<String>(),
        ideas.len(), tracks.len(), playlist_id);

    Ok(Json(serde_json::json!({
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
    let _ = crate::resolve_principal(&state, &req.token).await?;
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
    let principal = crate::resolve_principal(&state, &req.token).await?;
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

