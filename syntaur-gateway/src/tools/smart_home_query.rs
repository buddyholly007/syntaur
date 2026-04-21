//! Agent tool: read-only smart-home state queries.
//!
//! Lets agents answer "is the garage door open?", "is anyone home?", or
//! "what's on in the kitchen?" without side-effects. The dedicated
//! persona enforces the read-only contract — see the plan for the rule.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolContext};

pub struct SmartHomeQueryTool;

#[async_trait]
impl Tool for SmartHomeQueryTool {
    fn name(&self) -> &str {
        "smart_home_query"
    }

    fn description(&self) -> &str {
        "Query smart-home state in plain language. Read-only — never \
         mutates a device. Examples: \"is the front door locked?\", \
         \"who's home?\", \"what's the thermostat set to?\"."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "Plain-English read-only question."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        Ok(RichToolResult::text(
            "smart_home_query is scaffolded — device state cache lands with \
             the driver bring-up in weeks 3-8."
                .to_string(),
        ))
    }
}
