//! Event types used on the WebSocket broadcast channel. Every state change
//! the bridge observes (metadata update, position tick, volume change,
//! ducking state change, session error) becomes a BridgeEvent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// Emitted on WS connect so the client has a known baseline.
    Hello {
        bridge_version: String,
        chromium_ready: bool,
        authed_providers: Vec<String>,
    },
    NowPlaying {
        provider: String,
        track_id: String,
        name: String,
        artist: String,
        album: Option<String>,
        artwork_url: Option<String>,
        duration_ms: Option<u64>,
    },
    Position {
        position_ms: u64,
        duration_ms: Option<u64>,
        playing: bool,
    },
    Volume {
        level: f32,
    },
    Ducking {
        active: bool,
    },
    Ended {
        provider: String,
        track_id: String,
    },
    Error {
        code: String,
        message: String,
    },
    /// Auth state changed — e.g., cookies expired, user needs to re-auth.
    AuthExpired {
        provider: String,
    },
}

/// HTTP request payload for POST /play.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayRequest {
    pub provider: String,      // "apple_music" | "spotify" | "tidal" | "youtube_music"
    pub track_id: String,      // provider-specific ID (Apple: song id, Spotify: uri/id, YT: videoId)
    pub name: Option<String>,  // display name — purely for UI feedback
    pub artist: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VolumeRequest {
    /// 0.0 - 1.0
    pub level: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeekRequest {
    pub position_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DuckRequest {
    pub active: bool,
    /// Optional target duck-level (0.0 to 1.0). Default: 0.2
    pub level: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusResponse {
    pub running: bool,
    pub version: String,
    pub chromium_ready: bool,
    pub authed_providers: Vec<String>,
    pub audio_backend: String,
    pub now_playing: Option<NowPlaying>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NowPlaying {
    pub provider: String,
    pub track_id: String,
    pub name: String,
    pub artist: String,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub playing: bool,
}
