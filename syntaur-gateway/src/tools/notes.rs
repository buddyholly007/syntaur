//! Quick voice notes — "Peter, remember that I need to call the plumber"
//!
//! Persists notes to `/tmp/syntaur/voice_notes.json`. Each note has a
//! timestamp. Supports add, read (list all), and clear.

use async_trait::async_trait;
use chrono::Utc;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

const STATE_FILE: &str = "/tmp/syntaur/voice_notes.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Note {
    text: String,
    added_at: String,
}

fn load_notes() -> Vec<Note> {
    std::fs::read_to_string(STATE_FILE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_notes(notes: &[Note]) {
    let _ = std::fs::create_dir_all("/tmp/syntaur");
    if let Ok(json) = serde_json::to_string_pretty(notes) {
        let _ = std::fs::write(STATE_FILE, json);
    }
}

pub struct NotesTool;

#[async_trait]
impl Tool for NotesTool {
    fn name(&self) -> &str {
        "notes"
    }

    fn description(&self) -> &str {
        "Save quick voice notes or reminders. Add a note to remember something, \
         read back all saved notes, or clear them. Notes persist across sessions."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "read", "clear"],
                    "description": "Add a note, read all notes, or clear all notes."
                },
                "text": {
                    "type": "string",
                    "description": "For action=add: the note text to save."
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

        match action {
            "add" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or("notes: 'text' is required for add")?
                    .trim();
                if text.is_empty() {
                    return Err("notes: text is empty".to_string());
                }
                let mut notes = load_notes();
                notes.push(Note {
                    text: text.to_string(),
                    added_at: Utc::now().to_rfc3339(),
                });
                save_notes(&notes);
                info!("[notes] added: {}", text.chars().take(60).collect::<String>());
                Ok(RichToolResult::text(format!(
                    "Noted: \"{}\" ({} note{} total).",
                    text,
                    notes.len(),
                    if notes.len() == 1 { "" } else { "s" }
                )))
            }
            "read" => {
                let notes = load_notes();
                if notes.is_empty() {
                    return Ok(RichToolResult::text("No saved notes."));
                }
                let lines: Vec<String> = notes
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let date = chrono::DateTime::parse_from_rfc3339(&n.added_at)
                            .ok()
                            .map(|d| d.format("%b %d %I:%M %p").to_string())
                            .unwrap_or_else(|| "?".to_string());
                        format!("{}. {} ({})", i + 1, n.text, date)
                    })
                    .collect();
                Ok(RichToolResult::text(format!(
                    "{} note{}:\n{}",
                    notes.len(),
                    if notes.len() == 1 { "" } else { "s" },
                    lines.join("\n")
                )))
            }
            "clear" => {
                save_notes(&[]);
                info!("[notes] cleared all");
                Ok(RichToolResult::text("All notes cleared."))
            }
            other => Err(format!("notes: unknown action '{}'", other)),
        }
    }
}
