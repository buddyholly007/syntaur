//! Voice Journal module — record, transcribe, and search spoken entries.
//!
//! Part of the Syntaur Voice module (`mod-voice-journal`). Provides tools
//! for searching daily transcripts, summarizing conversations, and listing
//! recording sessions.
//!
//! Audio sources feed into a shared pipeline:
//!   - BLE wearables (Limitless pendant, Omi necklace) via TCP relay
//!   - Phone/desktop mic via WebSocket PWA
//!   - ESPHome satellites (existing voice pipeline)
//!
//! Each recording session is:
//!   1. VAD → strip silence → identify speech segments
//!   2. STT → transcribe speech segments
//!   3. Journal → append to daily markdown file
//!   4. Training → export clean clips with quality metadata
//!
//! ## Storage layout
//!
//! ```text
//! ~/.syntaur/voice-data/
//!   journal/          — daily transcript markdown (YYYY-MM-DD.md)
//!   wav/              — raw WAV recordings (auto-cleaned after processing)
//!   training/         — clean speech clips + metadata JSON
//!   wake-word/        — labeled wake word recordings for training
//!   sessions.json     — session index (source, duration, timestamps)
//! ```

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

// ── Module configuration ─────────────────────────────────────────────

/// Configuration for the Voice Journal module.
/// Parsed from `modules.entries["mod-voice-journal"].config` in syntaur.json.
///
/// Example config:
/// ```json
/// {
///   "modules": {
///     "entries": {
///       "mod-voice-journal": {
///         "enabled": true,
///         "config": {
///           "storage_path": "~/.syntaur/voice-data",
///           "wearable_port": 18800,
///           "wake_word": "Hey Atlas",
///           "consent_mode": "all_party",
///           "auto_cleanup_days": 7,
///           "training_clips": true
///         }
///       }
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceJournalConfig {
    /// Root directory for all voice data (journal, wav, training clips).
    #[serde(default = "default_storage_path")]
    pub storage_path: String,

    /// TCP port for BLE wearable relay connections.
    #[serde(default = "default_wearable_port")]
    pub wearable_port: u16,

    /// User's chosen wake word for their assistant.
    /// Used to label training recordings. Empty = not yet configured.
    #[serde(default)]
    pub wake_word: String,

    /// Recording consent mode: "one_party" or "all_party" (default).
    /// Displayed in the PWA before recording starts.
    #[serde(default = "default_consent_mode")]
    pub consent_mode: String,

    /// Days to keep raw WAV files before auto-cleanup. 0 = keep forever.
    #[serde(default = "default_cleanup_days")]
    pub auto_cleanup_days: u32,

    /// Whether to export clean speech clips for voice model training.
    #[serde(default = "default_training_clips")]
    pub training_clips: bool,

    /// Minimum number of wake word recordings needed before training can start.
    #[serde(default = "default_wake_word_min")]
    pub wake_word_min_clips: u32,
}

fn default_storage_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/home/sean"));
    format!("{}/.syntaur/voice-data", home)
}
fn default_wearable_port() -> u16 { 18800 }
fn default_consent_mode() -> String { "all_party".to_string() }
fn default_cleanup_days() -> u32 { 7 }
fn default_training_clips() -> bool { true }
fn default_wake_word_min() -> u32 { 5 }

impl Default for VoiceJournalConfig {
    fn default() -> Self {
        Self {
            storage_path: default_storage_path(),
            wearable_port: default_wearable_port(),
            wake_word: String::new(),
            consent_mode: default_consent_mode(),
            auto_cleanup_days: default_cleanup_days(),
            training_clips: true,
            wake_word_min_clips: default_wake_word_min(),
        }
    }
}

impl VoiceJournalConfig {
    /// Parse from a serde_json::Value (the module's config blob).
    pub fn from_value(v: &Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// Resolved storage path (expands ~).
    pub fn data_dir(&self) -> PathBuf {
        let expanded = if self.storage_path.starts_with('~') {
            let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/home/sean"));
            self.storage_path.replacen('~', &home, 1)
        } else {
            self.storage_path.clone()
        };
        PathBuf::from(expanded)
    }
}

/// Global config — set once at startup from the parsed config file.
static CONFIG: OnceLock<VoiceJournalConfig> = OnceLock::new();

/// Initialize the module config. Call once at gateway startup.
pub fn init_config(config: VoiceJournalConfig) {
    let _ = CONFIG.set(config);
}

/// Get the active config (falls back to defaults if not initialized).
pub fn config() -> &'static VoiceJournalConfig {
    CONFIG.get_or_init(VoiceJournalConfig::default)
}

// ── Path helpers (use config) ────────────────────────────────────────

fn voice_data_dir() -> PathBuf {
    config().data_dir()
}

fn journal_dir() -> PathBuf {
    voice_data_dir().join("journal")
}

// ── Session index ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSession {
    pub id: String,
    pub source: String,       // "wearable", "phone", "desktop", "satellite"
    pub started_at: String,   // ISO 8601
    pub duration_secs: f64,
    pub speech_ratio: f64,
    pub transcript_len: usize,
    pub training_clips: usize,
}

fn sessions_path() -> PathBuf {
    voice_data_dir().join("sessions.json")
}

pub fn load_sessions() -> Vec<RecordingSession> {
    std::fs::read_to_string(sessions_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_session(session: RecordingSession) {
    let mut sessions = load_sessions();
    sessions.push(session);
    // Keep last 1000 sessions
    if sessions.len() > 1000 {
        sessions = sessions.split_off(sessions.len() - 1000);
    }
    let _ = std::fs::create_dir_all(voice_data_dir());
    if let Ok(json) = serde_json::to_string_pretty(&sessions) {
        let _ = std::fs::write(sessions_path(), json);
    }
}

// ── Journal read/search ──────────────────────────────────────────────

fn read_journal(date: &str) -> Option<String> {
    let path = journal_dir().join(format!("{}.md", date));
    std::fs::read_to_string(path).ok()
}

fn list_journal_dates() -> Vec<String> {
    let dir = journal_dir();
    let mut dates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
    }
    dates.sort();
    dates.reverse(); // Most recent first
    dates
}

fn search_journals(query: &str, max_results: usize) -> Vec<(String, Vec<String>)> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for date in list_journal_dates() {
        if let Some(content) = read_journal(&date) {
            let matching_lines: Vec<String> = content
                .lines()
                .filter(|line| line.to_lowercase().contains(&query_lower))
                .map(|s| s.to_string())
                .collect();
            if !matching_lines.is_empty() {
                results.push((date, matching_lines));
            }
            if results.len() >= max_results {
                break;
            }
        }
    }
    results
}

// ── Journal append (called by audio processor) ───────────────────────

/// Append a timestamped transcript entry to the daily journal.
/// Called by the audio processing pipeline after STT.
pub fn append_journal_entry(
    timestamp: &chrono::DateTime<Utc>,
    text: &str,
    source: &str,
) -> std::io::Result<()> {
    let dir = journal_dir();
    std::fs::create_dir_all(&dir)?;

    let date_str = timestamp.format("%Y-%m-%d");
    let time_str = timestamp.format("%H:%M");
    let path = dir.join(format!("{}.md", date_str));

    let source_tag = match source {
        "assistant" => "",
        s => &format!(" [{}]", s),
    };

    let entry = if path.exists() {
        format!("\n**{}**{} {}\n", time_str, source_tag, text)
    } else {
        format!(
            "# Journal — {}\n\n**{}**{} {}\n",
            date_str, time_str, source_tag, text
        )
    };

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(entry.as_bytes())?;
    Ok(())
}

// ── Tools ────────────────────────────────────────────────────────────

pub struct SearchJournalTool;

#[async_trait]
impl Tool for SearchJournalTool {
    fn name(&self) -> &str {
        "search_journal"
    }

    fn description(&self) -> &str {
        "Search the user's voice journal transcripts. Find conversations by keyword, \
         topic, or date. Use this when the user asks what they talked about, discussed, \
         mentioned, or said on a specific day or about a specific topic."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search term or topic to find in transcripts"
                },
                "date": {
                    "type": "string",
                    "description": "Specific date to search (YYYY-MM-DD). Omit to search all dates."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of days to return (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let date = args.get("date").and_then(|v| v.as_str());
        let max = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        if let Some(date) = date {
            // Search specific date
            match read_journal(date) {
                Some(content) => {
                    if query.is_empty() {
                        Ok(RichToolResult::text(content))
                    } else {
                        let matches: Vec<&str> = content
                            .lines()
                            .filter(|l| l.to_lowercase().contains(&query.to_lowercase()))
                            .collect();
                        if matches.is_empty() {
                            Ok(RichToolResult::text(format!(
                                "No mentions of '{}' on {}.",
                                query, date
                            )))
                        } else {
                            Ok(RichToolResult::text(format!(
                                "Matches for '{}' on {}:\n{}",
                                query,
                                date,
                                matches.join("\n")
                            )))
                        }
                    }
                }
                None => Ok(RichToolResult::text(format!(
                    "No journal entry for {}.",
                    date
                ))),
            }
        } else {
            // Search all dates
            let results = search_journals(query, max);
            if results.is_empty() {
                return Ok(RichToolResult::text(format!(
                    "No journal entries matching '{}'.",
                    query
                )));
            }

            let mut output = format!("Found '{}' in {} day(s):\n\n", query, results.len());
            for (date, lines) in &results {
                output.push_str(&format!("**{}**:\n", date));
                for line in lines.iter().take(3) {
                    output.push_str(&format!("  {}\n", line));
                }
                if lines.len() > 3 {
                    output.push_str(&format!("  ...and {} more matches\n", lines.len() - 3));
                }
                output.push('\n');
            }
            Ok(RichToolResult::text(output))
        }
    }
}

pub struct JournalSummaryTool;

#[async_trait]
impl Tool for JournalSummaryTool {
    fn name(&self) -> &str {
        "journal_summary"
    }

    fn description(&self) -> &str {
        "Read VOICE PENDANT recordings only — NOT a summary of work activity. Returns raw \
         transcripts from the Limitless/satellite voice wearable for a specific date. \
         Most days have no voice journal. For 'what did I do' or 'recent activity' queries \
         use `memory_list`, `list_todos`, `execution_log`, or `search_memory` instead."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "date": {
                    "type": "string",
                    "description": "Date to retrieve (YYYY-MM-DD). Defaults to today."
                }
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let date = args
            .get("date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());

        match read_journal(&date) {
            Some(content) => {
                let line_count = content.lines().filter(|l| l.starts_with("**")).count();
                Ok(RichToolResult::text(format!(
                    "{}\n\n({} entries)",
                    content, line_count
                )))
            }
            None => {
                let available = list_journal_dates();
                let avail_str = if available.is_empty() {
                    "none — no voice pendant recordings exist yet".to_string()
                } else {
                    available.iter().take(10).cloned().collect::<Vec<_>>().join(", ")
                };
                Ok(RichToolResult::text(format!(
                    "No voice pendant recordings for {}. Available dates: {}.\n\n\
                     Note: this tool only returns raw voice transcripts. For general \
                     'recent activity' questions, call `memory_list` or `list_todos` instead — \
                     do NOT fabricate a summary from this empty result.",
                    date, avail_str
                )))
            }
        }
    }
}

pub struct ListRecordingsTool;

#[async_trait]
impl Tool for ListRecordingsTool {
    fn name(&self) -> &str {
        "list_recordings"
    }

    fn description(&self) -> &str {
        "List recent voice recording sessions with source device, duration, and speech percentage."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of sessions to return (default 10)"
                }
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let sessions = load_sessions();

        if sessions.is_empty() {
            return Ok(RichToolResult::text("No recording sessions yet."));
        }

        let recent: Vec<_> = sessions.iter().rev().take(limit).collect();
        let mut output = format!("Recent {} of {} sessions:\n\n", recent.len(), sessions.len());

        for s in &recent {
            output.push_str(&format!(
                "- {} | {} | {:.0}s | {:.0}% speech | {} clips\n",
                &s.started_at[..16],
                s.source,
                s.duration_secs,
                s.speech_ratio * 100.0,
                s.training_clips,
            ));
        }

        // Summary stats
        let total_duration: f64 = sessions.iter().map(|s| s.duration_secs).sum();
        let total_clips: usize = sessions.iter().map(|s| s.training_clips).sum();
        output.push_str(&format!(
            "\nTotal: {:.1} hours recorded, {} training clips",
            total_duration / 3600.0,
            total_clips
        ));

        Ok(RichToolResult::text(output))
    }
}

// ── Minimal WAV writer (avoids hound dependency) ─────────────────────

/// Write 16kHz mono 16-bit PCM to a WAV file.
fn write_wav_16k_mono(path: &Path, pcm: &[i16]) -> std::io::Result<()> {
    use std::io::Write;
    let data_len = (pcm.len() * 2) as u32;
    let file_len = 36 + data_len;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&file_len.to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;    // chunk size
    f.write_all(&1u16.to_le_bytes())?;     // PCM format
    f.write_all(&1u16.to_le_bytes())?;     // mono
    f.write_all(&16000u32.to_le_bytes())?;  // sample rate
    f.write_all(&32000u32.to_le_bytes())?;  // byte rate (16000 * 2)
    f.write_all(&2u16.to_le_bytes())?;     // block align
    f.write_all(&16u16.to_le_bytes())?;    // bits per sample
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for &sample in pcm {
        f.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

// ── Wake word training data ──────────────────────────────────────────

/// Save a labeled wake word recording for training.
pub fn save_wake_word_clip(pcm: &[i16], wake_word: &str, clip_index: u32) -> std::io::Result<PathBuf> {
    let dir = voice_data_dir().join("wake-word");
    std::fs::create_dir_all(&dir)?;

    let safe_name: String = wake_word
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let filename = format!("{}_{:03}.wav", safe_name, clip_index);
    let path = dir.join(&filename);

    write_wav_16k_mono(&path, pcm)?;

    // Update metadata
    let meta_path = dir.join("metadata.json");
    let mut meta: Value = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "wake_word": wake_word, "clips": [] }));

    if let Some(clips) = meta.get_mut("clips").and_then(|c| c.as_array_mut()) {
        clips.push(json!({
            "file": filename,
            "recorded_at": Utc::now().to_rfc3339(),
        }));
    }
    let _ = std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default());

    info!("[voice-journal] wake word clip saved: {}", path.display());
    Ok(path)
}

/// Get the number of wake word training clips recorded.
pub fn wake_word_clip_count() -> usize {
    let meta_path = voice_data_dir().join("wake-word").join("metadata.json");
    std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("clips").and_then(|c| c.as_array()).map(|a| a.len()))
        .unwrap_or(0)
}
