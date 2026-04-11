//! Pure-Rust shopping/todo list tool.
//!
//! Persists items to a JSON file. Replaces HA's in-memory shopping_list
//! integration (which loses data on restart and has no clean voice API).
//!
//! Supports multiple named lists: "shopping" (default), "todo", "grocery",
//! or any custom name. Items are timestamped for ordering.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

const STATE_FILE: &str = "/tmp/syntaur/lists.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListItem {
    text: String,
    added_at: String,
    checked: bool,
}

type Lists = HashMap<String, Vec<ListItem>>;

fn load_lists() -> Lists {
    std::fs::read_to_string(STATE_FILE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_lists(lists: &Lists) {
    let _ = std::fs::create_dir_all("/tmp/syntaur");
    if let Ok(json) = serde_json::to_string_pretty(lists) {
        let _ = std::fs::write(STATE_FILE, json);
    }
}

pub struct ShoppingListTool;

#[async_trait]
impl Tool for ShoppingListTool {
    fn name(&self) -> &str {
        "shopping_list"
    }

    fn description(&self) -> &str {
        "Manage shopping lists and todo lists. Add items, read the current list, \
         remove items, or clear the list. Supports named lists: 'shopping' (default), \
         'todo', 'grocery', or any custom name."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "read", "remove", "clear"],
                    "description": "What to do: add an item, read the list, remove an item, or clear all items."
                },
                "item": {
                    "type": "string",
                    "description": "Item text. Required for add/remove. For remove, matches by substring (case-insensitive)."
                },
                "list_name": {
                    "type": "string",
                    "description": "Which list. Default: 'shopping'. Examples: 'shopping', 'todo', 'grocery', 'hardware store'."
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            idempotent: false,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
        let list_name = args
            .get("list_name")
            .and_then(|v| v.as_str())
            .unwrap_or("shopping")
            .trim()
            .to_lowercase();
        let list_name = if list_name.is_empty() { "shopping".to_string() } else { list_name };
        let item = args.get("item").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let mut lists = load_lists();

        match action {
            "add" => {
                if item.is_empty() {
                    return Err("shopping_list: 'item' is required for add".to_string());
                }
                {
                    let list = lists.entry(list_name.clone()).or_default();
                    // Don't add duplicates
                    if list.iter().any(|i| i.text.eq_ignore_ascii_case(&item)) {
                        return Ok(RichToolResult::text(format!(
                            "'{}' is already on the {} list.",
                            item, list_name
                        )));
                    }
                    list.push(ListItem {
                        text: item.clone(),
                        added_at: Utc::now().to_rfc3339(),
                        checked: false,
                    });
                }
                save_lists(&lists);
                let count = lists.get(&list_name).map(|l| l.len()).unwrap_or(0);
                info!("[shopping_list] added '{}' to {}", item, list_name);
                Ok(RichToolResult::text(format!(
                    "Added '{}' to the {} list ({} item{} total).",
                    item,
                    list_name,
                    count,
                    if count == 1 { "" } else { "s" }
                )))
            }
            "read" => {
                let list = lists.get(&list_name);
                match list {
                    Some(items) if !items.is_empty() => {
                        let unchecked: Vec<&ListItem> =
                            items.iter().filter(|i| !i.checked).collect();
                        if unchecked.is_empty() {
                            return Ok(RichToolResult::text(format!(
                                "The {} list is empty (all items checked off).",
                                list_name
                            )));
                        }
                        let text: Vec<String> = unchecked
                            .iter()
                            .enumerate()
                            .map(|(i, item)| format!("{}. {}", i + 1, item.text))
                            .collect();
                        Ok(RichToolResult::text(format!(
                            "{} list ({} item{}):\n{}",
                            list_name,
                            unchecked.len(),
                            if unchecked.len() == 1 { "" } else { "s" },
                            text.join("\n")
                        )))
                    }
                    _ => Ok(RichToolResult::text(format!(
                        "The {} list is empty.",
                        list_name
                    ))),
                }
            }
            "remove" => {
                if item.is_empty() {
                    return Err("shopping_list: 'item' is required for remove".to_string());
                }
                let list = lists.entry(list_name.clone()).or_default();
                let before = list.len();
                list.retain(|i| !i.text.to_lowercase().contains(&item.to_lowercase()));
                let removed = before - list.len();
                save_lists(&lists);
                if removed > 0 {
                    info!("[shopping_list] removed {} item(s) matching '{}' from {}", removed, item, list_name);
                    Ok(RichToolResult::text(format!(
                        "Removed '{}' from the {} list.",
                        item, list_name
                    )))
                } else {
                    Ok(RichToolResult::text(format!(
                        "'{}' not found on the {} list.",
                        item, list_name
                    )))
                }
            }
            "clear" => {
                lists.remove(&list_name);
                save_lists(&lists);
                info!("[shopping_list] cleared {}", list_name);
                Ok(RichToolResult::text(format!(
                    "Cleared the {} list.",
                    list_name
                )))
            }
            other => Err(format!("shopping_list: unknown action '{}'", other)),
        }
    }
}
