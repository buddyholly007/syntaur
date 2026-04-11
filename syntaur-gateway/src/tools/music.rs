//! Music playback tool — Apple Music + Plex.
//!
//! ## Apple Music
//! Requires Apple Developer Program enrollment ($99/yr) for MusicKit
//! REST API access. Credentials needed in `~/.syntaur/syntaur.json`:
//! ```json
//! "connectors": {
//!   "apple_music": {
//!     "team_id": "<Apple Developer Team ID>",
//!     "key_id": "<MusicKit key ID>",
//!     "private_key_path": "<path to .p8 file>",
//!     "enabled": true
//!   }
//! }
//! ```
//!
//! ## Plex
//! Requires a Plex auth token. Get from Plex app: Settings → Account →
//! view XML → copy X-Plex-Token. Config:
//! ```json
//! "connectors": {
//!   "plex": {
//!     "base_url": "http://<plex-server>:32400",
//!     "token": "<X-Plex-Token>",
//!     "enabled": true
//!   }
//! }
//! ```
//!
//! ## Playback target
//! Music plays on HA media_player entities (Apple TV, Sonos, Snapcast,
//! etc.) via HA's `media_player.play_media` service. Alternatively, once
//! Matter or AirPlay Rust integration is done, playback can be direct.

use async_trait::async_trait;
use log::info;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct MusicTool;

#[async_trait]
impl Tool for MusicTool {
    fn name(&self) -> &str {
        "music"
    }

    fn description(&self) -> &str {
        "Control music playback. Play songs, albums, or playlists from Apple Music \
         or Plex. Pause, skip, or adjust volume. Supports playing by artist name, \
         song title, genre, or mood."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play", "pause", "skip", "volume", "search", "status"],
                    "description": "What to do: play music, pause, skip track, set volume, search, or get current status."
                },
                "query": {
                    "type": "string",
                    "description": "For play/search: artist, song, album, playlist, or genre to play/find."
                },
                "provider": {
                    "type": "string",
                    "enum": ["apple_music", "plex"],
                    "description": "Music source. Default: apple_music."
                },
                "volume": {
                    "type": "integer",
                    "description": "For action=volume: volume level 0-100."
                }
            },
            "required": ["action"]
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
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("play");
        let provider = args
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("apple_music");

        match provider {
            "apple_music" => {
                // Check if Apple Music is configured
                let config_path = format!(
                    "{}/.syntaur/syntaur.json"/* legacy path; new installs use ~/.syntaur/ */,
                    std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string())
                );
                let has_apple = std::fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|c| c.get("connectors")?.get("apple_music")?.get("enabled")?.as_bool())
                    .unwrap_or(false);

                if !has_apple {
                    return Ok(RichToolResult::text(
                        "Apple Music is not configured yet. To set up:\n\
                         1. Enroll in Apple Developer Program ($99/yr) if not already\n\
                         2. Create a MusicKit key at developer.apple.com\n\
                         3. Add connectors.apple_music to ~/.syntaur/syntaur.json \
                            with team_id, key_id, private_key_path\n\
                         4. Restart syntaur.\n\
                         Sean will provide the credentials later."
                    ));
                }

                // TODO: Implement Apple Music API calls
                // - Generate JWT developer token (team_id + key_id + .p8 private key)
                // - Search: GET https://api.music.apple.com/v1/catalog/us/search?term=...
                // - Play: requires AirPlay or HA media_player integration to target a speaker
                match action {
                    "play" => {
                        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        info!("[music:apple] play request: {}", query);
                        Ok(RichToolResult::text(format!(
                            "Apple Music play '{}' — API integration not yet implemented. \
                             The credentials are configured but the playback pipeline \
                             (JWT token generation → catalog search → AirPlay/HA media_player) \
                             needs to be built.",
                            query
                        )))
                    }
                    "search" => {
                        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        Ok(RichToolResult::text(format!(
                            "Apple Music search '{}' — not yet implemented.", query
                        )))
                    }
                    _ => Ok(RichToolResult::text(format!(
                        "Apple Music action '{}' — not yet implemented.", action
                    ))),
                }
            }
            "plex" => {
                let config_path = format!(
                    "{}/.syntaur/syntaur.json"/* legacy path; new installs use ~/.syntaur/ */,
                    std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string())
                );
                let plex_config = std::fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|c| c.get("connectors")?.get("plex").cloned());

                let (base_url, token) = match plex_config {
                    Some(pc) if pc.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) => {
                        let url = pc.get("base_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let tok = pc.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if url.is_empty() || tok.is_empty() {
                            return Err("Plex base_url or token is empty in config".to_string());
                        }
                        (url, tok)
                    }
                    _ => {
                        return Ok(RichToolResult::text(
                            "Plex is not configured yet. To set up:\n\
                             1. Get your X-Plex-Token from Plex app Settings → Account\n\
                             2. Add connectors.plex to ~/.syntaur/syntaur.json with \
                                base_url and token\n\
                             3. Restart syntaur.\n\
                             Sean will provide the credentials later."
                        ));
                    }
                };

                let client = ctx.http.as_ref().ok_or("no HTTP client")?;

                match action {
                    "search" | "play" => {
                        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        if query.is_empty() {
                            return Err("music: 'query' is required for play/search".to_string());
                        }

                        // Search Plex library
                        let search_url = format!(
                            "{}/search?query={}&type=10&X-Plex-Token={}",
                            base_url,
                            urlencoded(query),
                            token
                        );
                        let resp = client
                            .get(&search_url)
                            .header("Accept", "application/json")
                            .timeout(std::time::Duration::from_secs(10))
                            .send()
                            .await
                            .map_err(|e| format!("Plex search: {}", e))?;

                        if !resp.status().is_success() {
                            return Err(format!("Plex search: HTTP {}", resp.status()));
                        }

                        let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
                        let tracks = body
                            .get("MediaContainer")
                            .and_then(|mc| mc.get("Metadata"))
                            .and_then(|m| m.as_array())
                            .cloned()
                            .unwrap_or_default();

                        if tracks.is_empty() {
                            return Ok(RichToolResult::text(format!(
                                "No results for '{}' in Plex library.", query
                            )));
                        }

                        let results: Vec<String> = tracks
                            .iter()
                            .take(5)
                            .map(|t| {
                                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                                let artist = t
                                    .get("grandparentTitle")
                                    .or(t.get("parentTitle"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                format!("{} — {}", artist, title)
                            })
                            .collect();

                        if action == "search" {
                            Ok(RichToolResult::text(format!(
                                "Found {} result{} for '{}':\n{}",
                                results.len(),
                                if results.len() == 1 { "" } else { "s" },
                                query,
                                results.join("\n")
                            )))
                        } else {
                            // For play: would need to send to a media_player via HA or
                            // directly via Plex's playback API. Placeholder for now.
                            info!("[music:plex] found {} tracks for '{}'", tracks.len(), query);
                            Ok(RichToolResult::text(format!(
                                "Found '{}' in Plex. Playback routing to a speaker is not yet \
                                 wired — need to set up a media_player target (Apple TV, Sonos, \
                                 or Snapcast). Top result: {}",
                                query,
                                results.first().unwrap_or(&"?".to_string())
                            )))
                        }
                    }
                    "status" => {
                        // Check what's currently playing on Plex
                        let status_url = format!(
                            "{}/status/sessions?X-Plex-Token={}",
                            base_url, token
                        );
                        let resp = client
                            .get(&status_url)
                            .header("Accept", "application/json")
                            .timeout(std::time::Duration::from_secs(10))
                            .send()
                            .await
                            .map_err(|e| format!("Plex status: {}", e))?;

                        let body: Value = resp.json().await.unwrap_or_default();
                        let sessions = body
                            .get("MediaContainer")
                            .and_then(|mc| mc.get("Metadata"))
                            .and_then(|m| m.as_array())
                            .cloned()
                            .unwrap_or_default();

                        if sessions.is_empty() {
                            Ok(RichToolResult::text("Nothing playing on Plex right now."))
                        } else {
                            let lines: Vec<String> = sessions
                                .iter()
                                .map(|s| {
                                    let title = s.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                                    let artist = s
                                        .get("grandparentTitle")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let player = s
                                        .get("Player")
                                        .and_then(|p| p.get("title"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?");
                                    format!("{} — {} (on {})", artist, title, player)
                                })
                                .collect();
                            Ok(RichToolResult::text(format!(
                                "Currently playing on Plex:\n{}",
                                lines.join("\n")
                            )))
                        }
                    }
                    _ => Ok(RichToolResult::text(format!(
                        "Plex action '{}' not yet implemented.", action
                    ))),
                }
            }
            other => Err(format!("music: unknown provider '{}'", other)),
        }
    }
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('#', "%23")
}
