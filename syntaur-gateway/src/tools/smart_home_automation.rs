//! Agent tool: create / list / disable automations via natural-language.
//!
//! Drives the nl_automation compiler; the agent can draft an automation,
//! preview the AST, and save it after the user confirms. Persona never
//! saves without explicit approval.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolContext};

pub struct SmartHomeAutomationTool;

#[async_trait]
impl Tool for SmartHomeAutomationTool {
    fn name(&self) -> &str {
        "smart_home_automation"
    }

    fn description(&self) -> &str {
        "Draft, list, or disable a smart-home automation. Drafting accepts \
         a plain-English description and returns a preview (triggers / \
         conditions / actions) that the user must explicitly save."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["draft", "list", "disable", "enable"],
                    "description": "What to do with automations."
                },
                "prompt": {
                    "type": "string",
                    "description": "For \"draft\": the plain-English description."
                },
                "automation_id": {
                    "type": "integer",
                    "description": "For \"disable\"/\"enable\"."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        Ok(RichToolResult::text(
            "smart_home_automation is scaffolded. Draft/preview lands in week 7; \
             persistence + enable/disable in week 10."
                .to_string(),
        ))
    }
}
