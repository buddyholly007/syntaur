//! Announce tool — broadcasts a TTS message to the satellite speaker.
//!
//! Uses HA's `tts.speak` service to play text through the voice satellite.
//! This is the tool Peter uses when asked to "announce" something, "tell
//! everyone", or send a broadcast message. Also used internally by the
//! timer tick when a timer expires.
//!
//! Future: when multiple satellites exist, this tool should accept a
//! `room` or `target` parameter and route to the right media_player
//! entity. For now it always targets the single Satellite1 speaker.

use async_trait::async_trait;
use log::info;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};
use crate::tools::home_assistant;

/// Default satellite media player entity. Update when more satellites are added.
const DEFAULT_MEDIA_PLAYER: &str = "media_player.satellite1_918358_sat1_media_player";
/// TTS entity to use for speech synthesis.
const DEFAULT_TTS_ENTITY: &str = "tts.fish_audio";

pub struct AnnounceTool;

#[async_trait]
impl Tool for AnnounceTool {
    fn name(&self) -> &str {
        "announce"
    }

    fn description(&self) -> &str {
        "Speak a message out loud through the satellite speaker. Use when the user \
         asks you to announce something, say something out loud, or broadcast a message. \
         The message will be spoken via TTS on the office satellite."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The text to speak out loud."
                }
            },
            "required": ["message"]
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
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "announce: 'message' is required".to_string())?
            .trim();
        if message.is_empty() {
            return Err("announce: message is empty".to_string());
        }

        let ha = home_assistant::ha()?;
        ha.call_service(
            "tts",
            "speak",
            json!({
                "entity_id": DEFAULT_TTS_ENTITY,
                "media_player_entity_id": DEFAULT_MEDIA_PLAYER,
                "message": message,
            }),
        )
        .await?;

        info!("[announce] TTS sent: {}", message.chars().take(80).collect::<String>());
        Ok(RichToolResult::text(format!(
            "Announced: \"{}\"",
            message.chars().take(100).collect::<String>()
        )))
    }
}
