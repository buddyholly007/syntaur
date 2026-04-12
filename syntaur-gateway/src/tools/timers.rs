//! In-process timer/alarm tool for the voice pipeline.
//!
//! Timers are held in a global `TimerStore` and tick via a background
//! tokio task. When a timer expires, the callback sends a TTS
//! announcement to the satellite via HA's `tts.speak` service (using
//! the existing `HomeAssistantClient`).
//!
//! ## Why in-process instead of HA timer entities
//!
//! HA has `timer.*` entities but they require YAML config per timer and
//! don't cleanly expose a "create on the fly from voice" API. The HA
//! Assist satellite firmware HAS a native timer surface, but using it
//! requires passing `device_id` through the conversation API which
//! extended_openai_conversation doesn't support. Building it in Rust
//! gives us full control, persistence, and clean integration with the
//! voice pipeline.
//!
//! ## Persistence
//!
//! Timers survive restarts via a JSON file at
//! `/tmp/syntaur/voice_timers.json`. The background tick task reloads
//! the file on startup. Expired timers are pruned on load.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

const STATE_FILE: &str = "/tmp/syntaur/voice_timers.json";

// ── Timer state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub name: String,
    pub duration_seconds: u64,
    pub expires_at: u64, // unix epoch seconds
    pub fired: bool,
}

#[derive(Default)]
pub struct TimerStore {
    timers: HashMap<String, Timer>,
}

impl TimerStore {
    pub fn load() -> Self {
        let timers: HashMap<String, Timer> = std::fs::read_to_string(STATE_FILE)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Prune already-fired timers on load
        let now = epoch_secs();
        let timers = timers
            .into_iter()
            .filter(|(_, t)| !t.fired && t.expires_at > now)
            .collect();
        Self { timers }
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all("/tmp/syntaur");
        if let Ok(json) = serde_json::to_string_pretty(&self.timers) {
            let _ = std::fs::write(STATE_FILE, json);
        }
    }

    pub fn add(&mut self, name: &str, duration_seconds: u64) -> Timer {
        let now = epoch_secs();
        let id = format!("t{}", now % 100000);
        let timer = Timer {
            id: id.clone(),
            name: name.to_string(),
            duration_seconds,
            expires_at: now + duration_seconds,
            fired: false,
        };
        self.timers.insert(id, timer.clone());
        self.save();
        timer
    }

    pub fn list_active(&self) -> Vec<&Timer> {
        let now = epoch_secs();
        let mut active: Vec<&Timer> = self
            .timers
            .values()
            .filter(|t| !t.fired && t.expires_at > now)
            .collect();
        active.sort_by_key(|t| t.expires_at);
        active
    }

    pub fn cancel(&mut self, name_or_id: &str) -> Option<Timer> {
        // Try exact id match first
        if let Some(t) = self.timers.remove(name_or_id) {
            self.save();
            return Some(t);
        }
        // Try name match (case-insensitive)
        let key = self
            .timers
            .iter()
            .find(|(_, t)| t.name.eq_ignore_ascii_case(name_or_id))
            .map(|(k, _)| k.clone());
        if let Some(k) = key {
            let t = self.timers.remove(&k);
            self.save();
            return t;
        }
        None
    }

    pub fn mark_fired(&mut self, id: &str) {
        if let Some(t) = self.timers.get_mut(id) {
            t.fired = true;
        }
        self.save();
    }

    /// Return timers that have expired but not yet fired.
    pub fn expired_unfired(&self) -> Vec<Timer> {
        let now = epoch_secs();
        self.timers
            .values()
            .filter(|t| !t.fired && t.expires_at <= now)
            .cloned()
            .collect()
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{} second{}", secs, if secs == 1 { "" } else { "s" })
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{} minute{}", m, if m == 1 { "" } else { "s" })
        } else {
            format!("{} minute{} {} seconds", m, if m == 1 { "" } else { "s" }, s)
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{} hour{}", h, if h == 1 { "" } else { "s" })
        } else {
            format!("{} hour{} {} minutes", h, if h == 1 { "" } else { "s" }, m)
        }
    }
}

// ── Background tick task ───────────────────────────────────────────────────

/// Spawn a background task that checks for expired timers every second and
/// fires TTS announcements via HA. Call once at startup from main.rs.
pub fn spawn_timer_tick(store: Arc<Mutex<TimerStore>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let expired = {
                let s = store.lock().await;
                s.expired_unfired()
            };
            for timer in expired {
                info!("[timers] timer '{}' expired, announcing", timer.name);
                let announcement = format!("Hey, your {} timer is up!", timer.name);
                // Route through satellite connection (not HA)
                crate::voice::satellite_client::announce(&announcement);
                info!("[timers] announcement sent for '{}'", timer.name);
                // Mark as fired regardless
                store.lock().await.mark_fired(&timer.id);
            }
        }
    });
}

// ── Tool impl ──────────────────────────────────────────────────────────────

/// Global timer store. Initialized once at startup in main.rs.
static TIMER_STORE: std::sync::OnceLock<Arc<Mutex<TimerStore>>> = std::sync::OnceLock::new();

pub fn init_timer_store() -> Arc<Mutex<TimerStore>> {
    let store = Arc::new(Mutex::new(TimerStore::load()));
    TIMER_STORE.get_or_init(|| Arc::clone(&store));
    store
}

fn get_store() -> Option<Arc<Mutex<TimerStore>>> {
    TIMER_STORE.get().cloned()
}

pub struct TimerTool;

#[async_trait]
impl Tool for TimerTool {
    fn name(&self) -> &str {
        "timer"
    }

    fn description(&self) -> &str {
        "Start, list, or cancel countdown timers. When a timer expires, \
         Peter announces it via TTS on the satellite speaker. Supports \
         multiple concurrent timers with names."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "list", "cancel"],
                    "description": "What to do: start a new timer, list active timers, or cancel one."
                },
                "duration_seconds": {
                    "type": "integer",
                    "description": "Timer duration in seconds. Required for action=start."
                },
                "name": {
                    "type": "string",
                    "description": "Timer name (e.g. 'chicken', 'laundry', 'eggs'). Default: 'timer'."
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
        let store = get_store().ok_or_else(|| "timer store not initialized".to_string())?;
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("start");
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("timer")
            .trim();
        let name = if name.is_empty() { "timer" } else { name };

        match action {
            "start" => {
                let secs = args
                    .get("duration_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if secs == 0 {
                    return Err("timer: duration_seconds is required and must be > 0".to_string());
                }
                let timer = store.lock().await.add(name, secs);
                info!("[timers] started '{}' for {}s (id={})", name, secs, timer.id);
                Ok(RichToolResult::text(format!(
                    "Timer '{}' set for {}.",
                    name,
                    format_duration(secs)
                )))
            }
            "list" => {
                let s = store.lock().await;
                let active = s.list_active();
                if active.is_empty() {
                    return Ok(RichToolResult::text("No active timers."));
                }
                let now = epoch_secs();
                let lines: Vec<String> = active
                    .iter()
                    .map(|t| {
                        let remaining = t.expires_at.saturating_sub(now);
                        format!("{}: {} remaining", t.name, format_duration(remaining))
                    })
                    .collect();
                Ok(RichToolResult::text(format!(
                    "{} active timer{}:\n{}",
                    lines.len(),
                    if lines.len() == 1 { "" } else { "s" },
                    lines.join("\n")
                )))
            }
            "cancel" => {
                let cancelled = store.lock().await.cancel(name);
                match cancelled {
                    Some(t) => {
                        info!("[timers] cancelled '{}'", t.name);
                        Ok(RichToolResult::text(format!(
                            "Cancelled timer '{}'.",
                            t.name
                        )))
                    }
                    None => Ok(RichToolResult::text(format!(
                        "No timer named '{}' found.",
                        name
                    ))),
                }
            }
            other => Err(format!("timer: unknown action '{}'", other)),
        }
    }
}
