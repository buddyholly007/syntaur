//! Escalation rule engine: detects when a main-agent conversation drifts into
//! a specialist's domain and offers a handoff.
//!
//! Flow:
//! 1. Each user message is classified into a module tag (tax, music, etc.)
//! 2. A rolling window per conversation tracks the last 4 tags
//! 3. When 3 of 4 tags match the same specialist → return an escalation offer
//! 4. The offer is included as a JSON field in the chat response
//! 5. Frontend renders it as "[Open Tax module] [Stay here]" buttons

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

const WINDOW_SIZE: usize = 4;
const THRESHOLD: usize = 3;

/// Classify a user message into a module tag via keyword matching.
/// Returns "other" for messages that don't clearly map to a specialist.
pub fn classify(message: &str) -> &'static str {
    let lower = message.to_lowercase();

    static TAX_KEYWORDS: &[&str] = &[
        "tax", "deduct", "receipt", "expense", "irs", "1040", "w-2", "w2",
        "refund", "filing", "quarterly", "estimated tax", "depreciation",
        "write-off", "write off", "schedule c", "1099", "capital gain",
    ];
    static MUSIC_KEYWORDS: &[&str] = &[
        "song", "music", "play ", "track", "playlist", "album", "artist",
        "spotify", "listen", "genre", "vibe", "dj", "beats",
    ];
    static RESEARCH_KEYWORDS: &[&str] = &[
        "research", "document", "search my", "source", "citation", "paper",
        "knowledge", "uploaded", "find in my", "look up", "what does my",
    ];
    static SCHEDULER_KEYWORDS: &[&str] = &[
        "calendar", "schedule", "meeting", "appointment", "deadline",
        "remind", "todo", "to-do", "to do", "tomorrow", "next week",
        "free time", "busy", "reschedule", "cancel meeting",
    ];
    static CODERS_KEYWORDS: &[&str] = &[
        "code", "bug", "compile", "deploy", "git ", "commit", "push",
        "rust ", "python ", "cargo", "npm", "server", "ssh", "terminal",
        "function", "error in", "stack trace", "debug",
    ];
    static JOURNAL_KEYWORDS: &[&str] = &[
        "journal", "reflect", "feeling", "diary", "how i feel",
        "anxious", "grateful", "stressed", "emotion", "vent",
    ];

    if TAX_KEYWORDS.iter().any(|k| lower.contains(k)) { return "tax"; }
    if MUSIC_KEYWORDS.iter().any(|k| lower.contains(k)) { return "music"; }
    if RESEARCH_KEYWORDS.iter().any(|k| lower.contains(k)) { return "research"; }
    if SCHEDULER_KEYWORDS.iter().any(|k| lower.contains(k)) { return "scheduler"; }
    if CODERS_KEYWORDS.iter().any(|k| lower.contains(k)) { return "coders"; }
    if JOURNAL_KEYWORDS.iter().any(|k| lower.contains(k)) { return "journal"; }
    "other"
}

/// Display name for a module's specialist agent.
fn agent_display_name(module: &str) -> &'static str {
    match module {
        "tax" => "Positron",
        "research" => "Cortex",
        "music" => "Silvr",
        "scheduler" => "Thaddeus",
        "coders" => "Maurice",
        "journal" => "Mushi",
        _ => "a specialist",
    }
}

/// Human-readable description of what the specialist offers.
fn module_pitch(module: &str) -> &'static str {
    match module {
        "tax" => "has direct access to your receipts and expense data",
        "research" => "can search your documents and cite sources",
        "music" => "reads the vibe and controls your playback",
        "scheduler" => "sees your full calendar and manages your time",
        "coders" => "can pair-program and run commands on your hosts",
        "journal" => "offers a private, reflective space",
        _ => "specializes in this area",
    }
}

pub struct EscalationTracker {
    windows: RwLock<HashMap<String, VecDeque<String>>>,
    suppressed: RwLock<HashMap<(String, String), usize>>,
}

impl EscalationTracker {
    pub fn new() -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            suppressed: RwLock::new(HashMap::new()),
        }
    }

    /// Record a classified module tag for a conversation. Call on every
    /// user message in main-agent chats.
    pub fn record(&self, conv_id: &str, module_tag: &str) {
        let mut windows = self.windows.write().unwrap();
        let window = windows
            .entry(conv_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(WINDOW_SIZE + 1));
        window.push_back(module_tag.to_string());
        if window.len() > WINDOW_SIZE {
            window.pop_front();
        }
    }

    /// Check if an escalation should be offered. Returns the module name
    /// if 3 of the last 4 messages are tagged to the same specialist,
    /// and we haven't suppressed that module for this conversation recently.
    pub fn should_offer(&self, conv_id: &str) -> Option<String> {
        let windows = self.windows.read().unwrap();
        let window = windows.get(conv_id)?;
        if window.len() < THRESHOLD {
            return None;
        }

        // Count tags in the window
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for tag in window.iter() {
            if tag != "other" {
                *counts.entry(tag.as_str()).or_default() += 1;
            }
        }

        // Find a module that meets threshold
        let candidate = counts
            .into_iter()
            .find(|(_, count)| *count >= THRESHOLD)
            .map(|(module, _)| module.to_string())?;

        // Check suppression
        let suppressed = self.suppressed.read().unwrap();
        let key = (conv_id.to_string(), candidate.clone());
        if let Some(&suppressed_at) = suppressed.get(&key) {
            let windows = self.windows.read().unwrap();
            let current_len = windows
                .get(conv_id)
                .map(|w| w.len())
                .unwrap_or(0);
            // Don't re-offer for 10 messages after dismissal
            if current_len < suppressed_at + 10 {
                return None;
            }
        }

        Some(candidate)
    }

    /// Suppress escalation for a module in a conversation (user clicked "Stay here").
    pub fn dismiss(&self, conv_id: &str, module: &str) {
        let mut suppressed = self.suppressed.write().unwrap();
        let windows = self.windows.read().unwrap();
        let msg_count = windows.get(conv_id).map(|w| w.len()).unwrap_or(0);
        suppressed.insert((conv_id.to_string(), module.to_string()), msg_count);
    }

    /// Build the escalation offer JSON for a triggered module.
    pub fn build_offer(module: &str) -> serde_json::Value {
        let agent = agent_display_name(module);
        let pitch = module_pitch(module);
        serde_json::json!({
            "module": module,
            "agent_name": agent,
            "message": format!(
                "This is feeling like {} territory \u{2014} {} {}. Want me to hand you off?",
                agent, agent, pitch
            ),
            "actions": [
                {"label": format!("Open {} module", agent), "action": "handoff", "target": format!("/{}", module)},
                {"label": "Keep going here", "action": "dismiss"},
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_tax_messages() {
        assert_eq!(classify("How much can I deduct for my home office?"), "tax");
        assert_eq!(classify("Show me my receipts from October"), "tax");
        assert_eq!(classify("What's the IRS deadline for Q3 estimated taxes?"), "tax");
    }

    #[test]
    fn classify_music_messages() {
        assert_eq!(classify("Play something chill"), "music");
        assert_eq!(classify("Add this song to my playlist"), "music");
    }

    #[test]
    fn classify_other() {
        assert_eq!(classify("What's the weather like?"), "other");
        assert_eq!(classify("Hello"), "other");
    }

    #[test]
    fn escalation_triggers_at_threshold() {
        let tracker = EscalationTracker::new();
        tracker.record("c1", "tax");
        assert!(tracker.should_offer("c1").is_none()); // only 1
        tracker.record("c1", "tax");
        assert!(tracker.should_offer("c1").is_none()); // only 2
        tracker.record("c1", "tax");
        assert_eq!(tracker.should_offer("c1"), Some("tax".to_string())); // 3 of 3
    }

    #[test]
    fn escalation_with_one_other() {
        let tracker = EscalationTracker::new();
        tracker.record("c1", "tax");
        tracker.record("c1", "other");
        tracker.record("c1", "tax");
        tracker.record("c1", "tax");
        assert_eq!(tracker.should_offer("c1"), Some("tax".to_string())); // 3 of 4
    }

    #[test]
    fn dismissal_suppresses() {
        let tracker = EscalationTracker::new();
        for _ in 0..3 { tracker.record("c1", "tax"); }
        assert!(tracker.should_offer("c1").is_some());
        tracker.dismiss("c1", "tax");
        assert!(tracker.should_offer("c1").is_none()); // suppressed
    }

    #[test]
    fn mixed_topics_no_trigger() {
        let tracker = EscalationTracker::new();
        tracker.record("c1", "tax");
        tracker.record("c1", "music");
        tracker.record("c1", "research");
        tracker.record("c1", "scheduler");
        assert!(tracker.should_offer("c1").is_none());
    }
}
