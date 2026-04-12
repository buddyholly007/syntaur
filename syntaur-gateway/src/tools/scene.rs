//! Scene tool — activate HA scenes by name.
//!
//! Wraps HA's `scene.turn_on` service with a voice-friendly interface.
//! Can also list available scenes. Better than making the LLM figure out
//! the exact entity_id via call_ha_service.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};
use crate::tools::home_assistant;

pub struct SceneTool;

#[async_trait]
impl Tool for SceneTool {
    fn name(&self) -> &str { "scene" }

    fn description(&self) -> &str {
        "Activate a Home Assistant scene by name. Scenes are preconfigured \
         states for groups of devices (e.g. 'movie night', 'good morning', \
         'bedtime'). Can also list all available scenes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["activate", "list"],
                    "description": "Activate a scene or list available scenes."
                },
                "name": {
                    "type": "string",
                    "description": "For activate: the scene name (partial match ok, case-insensitive)."
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
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("activate");

        match action {
            "list" => {
                let states = ha.list_states().await?;
                let scenes: Vec<String> = states
                    .iter()
                    .filter_map(|s| {
                        let eid = s.get("entity_id").and_then(|v| v.as_str())?;
                        if eid.starts_with("scene.") {
                            let name = s
                                .get("attributes")
                                .and_then(|a| a.get("friendly_name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(eid);
                            Some(format!("- {}", name))
                        } else {
                            None
                        }
                    })
                    .collect();

                if scenes.is_empty() {
                    Ok(RichToolResult::text("No scenes configured in Home Assistant."))
                } else {
                    Ok(RichToolResult::text(format!(
                        "{} scene{}:\n{}",
                        scenes.len(),
                        if scenes.len() == 1 { "" } else { "s" },
                        scenes.join("\n")
                    )))
                }
            }
            "activate" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("scene: 'name' is required for activate")?
                    .trim();

                // Find the scene entity by fuzzy name match
                let states = ha.list_states().await?;
                let scene = states.iter().find(|s| {
                    let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
                    if !eid.starts_with("scene.") {
                        return false;
                    }
                    let friendly = s
                        .get("attributes")
                        .and_then(|a| a.get("friendly_name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    friendly.to_lowercase().contains(&name.to_lowercase())
                        || eid.to_lowercase().contains(&name.to_lowercase())
                });

                match scene {
                    Some(s) => {
                        let eid = s.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");
                        let friendly = s
                            .get("attributes")
                            .and_then(|a| a.get("friendly_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(eid);
                        ha.call_service("scene", "turn_on", json!({"entity_id": eid}))
                            .await?;
                        log::info!("[scene] activated {}", eid);
                        Ok(RichToolResult::text(format!("Activated scene '{}'.", friendly)))
                    }
                    None => Ok(RichToolResult::text(format!(
                        "No scene matching '{}' found. Use action=list to see available scenes.",
                        name
                    ))),
                }
            }
            other => Err(format!("scene: unknown action '{}'", other)),
        }
    }
}
