//! Agent tool: natural-language device operations.
//!
//! Exposed to Peter (voice satellite) and the module's dedicated persona.
//! Scaffolded in week 1 — execution path returns a "driver not wired"
//! error so the LLM surfaces a clean message until the control plane
//! lands in weeks 3–8 (per protocol).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolContext};

pub struct SmartHomeControlTool;

#[async_trait]
impl Tool for SmartHomeControlTool {
    fn name(&self) -> &str {
        "smart_home_control"
    }

    fn description(&self) -> &str {
        "Control a smart-home device. Accepts a natural-language instruction \
         (\"turn off the kitchen lights\") and routes it to the appropriate \
         driver (Matter / Zigbee / Z-Wave / Wi-Fi / BLE / MQTT / cloud)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "instruction": {
                    "type": "string",
                    "description": "Plain-English operation, e.g. \"dim the bedroom lights to 40%\"."
                },
                "room": {
                    "type": "string",
                    "description": "Optional room hint if the instruction is ambiguous."
                }
            },
            "required": ["instruction"]
        })
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        Ok(RichToolResult::text(
            "smart_home_control is scaffolded but no driver is wired yet (Track A week 1). \
             Device control lands in weeks 3-8 per the plan calendar."
                .to_string(),
        ))
    }
}
