//! Media player control — play/pause/volume/skip on HA media players.
//!
//! Controls Apple TV, Samsung TV, satellite speakers, and any other
//! HA media_player entity. Separate from the `music` tool which handles
//! searching/queueing content from Apple Music / Plex.
//!
//! This tool is the "remote control" — play/pause what's already playing,
//! change volume, skip tracks, select source. The music tool is the
//! "jukebox" — find and start playing something new.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};
use crate::tools::home_assistant;

pub struct MediaControlTool;

#[async_trait]
impl Tool for MediaControlTool {
    fn name(&self) -> &str { "media_control" }

    fn description(&self) -> &str {
        "Control media playback on TVs and speakers. Play, pause, stop, skip, \
         or set volume on the Apple TV, Samsung TV, or satellite speaker. \
         Use this for transport controls on whatever is currently playing."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play", "pause", "stop", "next", "previous", "volume", "mute", "status"],
                    "description": "Transport control action."
                },
                "target": {
                    "type": "string",
                    "description": "Which device: 'apple_tv', 'samsung_tv', 'satellite', or 'all'. Default: auto-detect playing device."
                },
                "volume": {
                    "type": "number",
                    "description": "For action=volume: level 0.0 to 1.0 (0 = mute, 1 = max)."
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

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let ha = home_assistant::ha()?;
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("status");
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        // Map target names to entity IDs (owned String to avoid lifetime issues)
        let entity_id: String = match target.to_lowercase().as_str() {
            "apple_tv" | "apple tv" | "appletv" | "living room" => {
                "media_player.living_room".to_string()
            }
            "samsung_tv" | "samsung" | "tv" | "the frame" => {
                "media_player.65_the_frame_qn65ls03fwfxza".to_string()
            }
            "satellite" | "speaker" | "office" => {
                "media_player.satellite1_918358_sat1_media_player".to_string()
            }
            _ => {
                // Auto: try to find any currently playing media player
                let states = ha.list_states().await?;
                states.iter()
                    .find(|s| {
                        let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
                        eid.starts_with("media_player.")
                            && s.get("state").and_then(|v| v.as_str()) == Some("playing")
                    })
                    .and_then(|p| p.get("entity_id").and_then(|v| v.as_str()))
                    .unwrap_or("media_player.living_room")
                    .to_string()
            }
        };
        let eid = entity_id.as_str();

        let (service_domain, service_name, body) = match action {
            "play" => ("media_player", "media_play", json!({"entity_id": eid})),
            "pause" => ("media_player", "media_pause", json!({"entity_id": eid})),
            "stop" => ("media_player", "media_stop", json!({"entity_id": eid})),
            "next" => ("media_player", "media_next_track", json!({"entity_id": eid})),
            "previous" => (
                "media_player",
                "media_previous_track",
                json!({"entity_id": eid}),
            ),
            "volume" => {
                let vol = args
                    .get("volume")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5);
                (
                    "media_player",
                    "volume_set",
                    json!({"entity_id": eid, "volume_level": vol}),
                )
            }
            "mute" => (
                "media_player",
                "volume_mute",
                json!({"entity_id": eid, "is_volume_muted": true}),
            ),
            "status" => {
                let state = ha.get_state(eid).await?;
                let player_state = state
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let attrs = state.get("attributes").cloned().unwrap_or_default();
                let title = attrs
                    .get("media_title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("nothing");
                let artist = attrs
                    .get("media_artist")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let volume = attrs
                    .get("volume_level")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let friendly = attrs
                    .get("friendly_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(eid);

                let status = if player_state == "playing" {
                    if artist.is_empty() {
                        format!(
                            "{}: playing '{}', volume {:.0}%.",
                            friendly, title, volume * 100.0
                        )
                    } else {
                        format!(
                            "{}: playing {} — '{}', volume {:.0}%.",
                            friendly, artist, title, volume * 100.0
                        )
                    }
                } else {
                    format!("{}: {} (volume {:.0}%).", friendly, player_state, volume * 100.0)
                };
                return Ok(RichToolResult::text(status));
            }
            other => return Err(format!("media_control: unknown action '{}'", other)),
        };

        ha.call_service(service_domain, service_name, body).await?;

        let friendly_target = match target.to_lowercase().as_str() {
            "apple_tv" | "apple tv" | "appletv" | "living room" => "Apple TV",
            "samsung_tv" | "samsung" | "tv" | "the frame" => "Samsung TV",
            "satellite" | "speaker" | "office" => "satellite speaker",
            _ => "media player",
        };

        log::info!("[media_control] {} on {}", action, eid);
        Ok(RichToolResult::text(format!(
            "{} on {}.",
            match action {
                "play" => "Playing",
                "pause" => "Paused",
                "stop" => "Stopped",
                "next" => "Skipped to next",
                "previous" => "Back to previous",
                "volume" => "Volume set",
                "mute" => "Muted",
                _ => action,
            },
            friendly_target
        )))
    }
}
