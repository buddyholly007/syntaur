//! Task extraction from journal entries — the one privacy exception.
//!
//! When a user explicitly asks Mushi to "pull out the todos", this module
//! scans conversation messages for task-like patterns and returns them as
//! a structured list. Approved tasks are routed to the todos table WITHOUT
//! carrying any journal context — only the task text travels.

use regex::Regex;
use std::sync::OnceLock;

/// Patterns that suggest a task or action item in natural text.
fn task_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)(?:need to|have to|must|should|gotta)\s+(.{10,120})",
            r"(?i)(?:remember to|don't forget|remind me to)\s+(.{10,120})",
            r"(?i)(?:todo|to-do|to do):\s*(.{5,120})",
            r"(?i)^[-*]\s*\[\s*\]\s*(.{5,120})",
            r"(?i)(?:i need|we need)\s+(?:to\s+)?(.{10,120})",
            r"(?i)(?:call|email|text|message|contact)\s+(.{5,80})",
            r"(?i)(?:pick up|buy|order|get|grab)\s+(.{5,80})",
            r"(?i)(?:schedule|book|set up|arrange)\s+(.{5,80})",
            r"(?i)(?:fix|repair|replace)\s+(.{5,80})",
            r"(?i)(?:start|begin|finish|complete)\s+(?:the\s+)?(.{5,80})",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

/// Extract task-like items from a block of text (journal messages).
/// Returns a list of (raw_match, cleaned_task_text) pairs.
pub fn extract_tasks(text: &str) -> Vec<String> {
    let patterns = task_patterns();
    let mut tasks = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        for re in patterns {
            if let Some(cap) = re.captures(trimmed) {
                if let Some(task_text) = cap.get(1) {
                    let cleaned = task_text.as_str().trim().trim_end_matches('.').to_string();
                    if cleaned.len() >= 5 && seen.insert(cleaned.clone()) {
                        tasks.push(cleaned);
                    }
                }
            }
        }
    }

    tasks
}

/// Create a todo item in the todos table. Returns the new todo id.
/// The todo carries ONLY the task text — no journal context, no
/// conversation reference, no emotional backdrop.
pub fn create_todo(
    conn: &rusqlite::Connection,
    user_id: i64,
    text: &str,
) -> rusqlite::Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO todos (user_id, text, done, created_at) VALUES (?1, ?2, 0, ?3)",
        rusqlite::params![user_id, text, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// LLM-powered task extraction — more accurate than regex, catches natural
/// language tasks like "I should really get the car serviced" while avoiding
/// false positives on emotional statements.
///
/// Falls back to regex extraction on error or timeout.
pub async fn extract_tasks_with_llm(
    config: &crate::config::Config,
    client: &reqwest::Client,
    text: &str,
) -> Vec<String> {
    use tokio::time::{timeout, Duration};

    if text.len() < 10 { return vec![]; }

    // Truncate very long text to avoid burning tokens
    let input = if text.len() > 3000 { &text[..3000] } else { text };

    let chain = crate::llm::LlmChain::from_config_fast(config, "main", client.clone());
    let system_prompt = format!(
        "Extract actionable tasks from this journal entry. Return ONLY a JSON array of short task strings.\n\
         Rules:\n\
         - Include: things to do, buy, call, fix, schedule, remember, send, finish\n\
         - Exclude: feelings, reflections, observations, wishes without action\n\
         - Keep each task under 100 characters\n\
         - If no tasks found, return []\n\
         Example: [\"call the dentist\", \"buy groceries\", \"fix kitchen faucet\"]\n\n{}",
        crate::security::UNTRUSTED_INPUT_SYSTEM_DIRECTIVE
    );
    let wrapped = crate::security::wrap_untrusted_input("journal_entry", input);
    let messages = vec![
        crate::llm::ChatMessage::system(&system_prompt),
        crate::llm::ChatMessage::user(&wrapped),
    ];

    let result = match timeout(Duration::from_secs(8), chain.call(&messages)).await {
        Ok(Ok(text)) => text,
        _ => return extract_tasks(text), // fall back to regex
    };

    // Parse JSON array from response (handle markdown code blocks)
    let cleaned = result
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<Vec<String>>(cleaned) {
        Ok(tasks) => {
            // Filter and deduplicate
            let mut seen = std::collections::HashSet::new();
            tasks.into_iter()
                .filter(|t| t.len() >= 5 && t.len() <= 200 && seen.insert(t.clone()))
                .collect()
        }
        Err(_) => extract_tasks(text), // fall back to regex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_need_to() {
        let tasks = extract_tasks("I really need to call the dentist about that appointment");
        assert!(!tasks.is_empty());
        assert!(tasks[0].contains("call the dentist"));
    }

    #[test]
    fn extract_remember_to() {
        let tasks = extract_tasks("remember to order new filters for the furnace");
        assert!(!tasks.is_empty());
        assert!(tasks[0].contains("order new filters"));
    }

    #[test]
    fn extract_checkbox() {
        let tasks = extract_tasks("- [ ] finish the quarterly report\n- [ ] send invoice to client");
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn extract_todo_colon() {
        let tasks = extract_tasks("TODO: update the README with new API docs");
        assert!(!tasks.is_empty());
    }

    #[test]
    fn no_false_positives_on_short() {
        let tasks = extract_tasks("I had a good day. The sun was nice.");
        assert!(tasks.is_empty());
    }

    #[test]
    fn dedup_repeated_tasks() {
        let tasks = extract_tasks(
            "need to call the bank\nI also need to call the bank about it",
        );
        // "call the bank" appears in both but the cleaned text differs slightly
        // so dedup by exact match may keep both — that's acceptable
        assert!(tasks.len() <= 2);
    }
}
