use crate::config::LcmConfig;
use crate::llm::{ChatMessage, LlmChain};
use log::{debug, error, info, warn};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Arc;

/// Long Context Management — summarizes old messages to fit within context window
pub struct LcmManager {
    db_path: String,
    config: LcmConfig,
}

impl LcmManager {
    /// Expose the on-disk path so external modules (agent settings cog
    /// Maintenance section) can open their own connection for one-off
    /// queries without pushing every CRUD into LcmManager.
    pub fn db_path_str(&self) -> String {
        self.db_path.clone()
    }

    pub fn new(db_path: &str, config: LcmConfig) -> Self {
        // Ensure database exists with correct schema
        if let Ok(conn) = Connection::open(db_path) {
            let _ = conn.execute_batch("
                CREATE TABLE IF NOT EXISTS summaries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    depth INTEGER DEFAULT 0,
                    content TEXT NOT NULL,
                    token_count INTEGER DEFAULT 0,
                    source_count INTEGER DEFAULT 0,
                    created_at TEXT DEFAULT (datetime('now')),
                    UNIQUE(agent_id, session_id, depth, created_at)
                );
                CREATE INDEX IF NOT EXISTS idx_summaries_agent ON summaries(agent_id, session_id);
                CREATE TABLE IF NOT EXISTS conversations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    token_estimate INTEGER DEFAULT 0,
                    created_at TEXT DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_conversations_agent ON conversations(agent_id, session_id);
            ");
            info!("LCM database initialized at {}", db_path);
        } else {
            warn!("Cannot open LCM database at {}", db_path);
        }

        Self {
            db_path: db_path.to_string(),
            config,
        }
    }

    /// Store a message in the conversation log
    pub fn store_message(&self, agent_id: &str, session_id: &str, role: &str, content: &str) {
        let token_estimate = content.len() / 4; // rough estimate

        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute(
                "INSERT INTO conversations (agent_id, session_id, role, content, token_estimate) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![agent_id, session_id, role, content, token_estimate],
            );
        }
    }

    /// Check if context needs compaction and return summarized messages if so
    pub fn get_context(
        &self,
        agent_id: &str,
        session_id: &str,
        messages: &[ChatMessage],
        max_tokens: u64,
    ) -> Vec<ChatMessage> {
        let total_tokens: u64 = messages.iter()
            .map(|m| m.content.len() as u64 / 4)
            .sum();

        let threshold = (max_tokens as f64 * self.config.context_threshold) as u64;

        if total_tokens <= threshold {
            // Context is fine, return as-is
            return messages.to_vec();
        }

        info!("[lcm:{}] Context at {}% ({}/{} tokens), compacting",
            agent_id,
            (total_tokens * 100) / max_tokens,
            total_tokens, max_tokens
        );

        // Keep system message + fresh tail
        let fresh_count = self.config.fresh_tail_count;
        let mut result = Vec::new();

        // System message always first
        if let Some(sys) = messages.first() {
            if sys.role == "system" {
                result.push(sys.clone());
            }
        }

        // Try to load existing summary from DB
        if let Some(summary) = self.get_latest_summary(agent_id, session_id) {
            result.push(ChatMessage::system(&format!("[Previous conversation summary]\n{}", summary)));
        }

        // Add fresh tail (last N messages, excluding system)
        let non_system: Vec<&ChatMessage> = messages.iter()
            .filter(|m| m.role != "system")
            .collect();

        let tail_start = non_system.len().saturating_sub(fresh_count);
        for msg in &non_system[tail_start..] {
            result.push((*msg).clone());
        }

        result
    }

    /// Get the most recent summary for a session
    fn get_latest_summary(&self, agent_id: &str, session_id: &str) -> Option<String> {
        let conn = Connection::open(&self.db_path).ok()?;
        conn.query_row(
            "SELECT content FROM summaries WHERE agent_id = ?1 AND session_id = ?2 ORDER BY depth DESC, created_at DESC LIMIT 1",
            params![agent_id, session_id],
            |row| row.get(0),
        ).ok()
    }

    /// Create a summary of older messages using the LLM
    pub async fn summarize(
        &self,
        agent_id: &str,
        session_id: &str,
        messages: &[ChatMessage],
        llm: &LlmChain,
    ) {
        // Only summarize if we have enough messages
        if messages.len() < self.config.fresh_tail_count + 4 {
            return;
        }

        // Get messages to summarize (everything except system + fresh tail)
        let non_system: Vec<&ChatMessage> = messages.iter()
            .filter(|m| m.role != "system")
            .collect();

        let tail_start = non_system.len().saturating_sub(self.config.fresh_tail_count);
        let to_summarize: Vec<&ChatMessage> = non_system[..tail_start].to_vec();

        if to_summarize.is_empty() {
            return;
        }

        // Build summarization prompt
        let conversation = to_summarize.iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt_messages = vec![
            ChatMessage::system("You are a conversation summarizer. Produce a concise but comprehensive summary of the following conversation, preserving key facts, decisions, and context. The summary will be used to maintain context in a long-running conversation."),
            ChatMessage::user(&format!("Summarize this conversation:\n\n{}", conversation)),
        ];

        info!("[lcm:{}] Summarizing {} messages", agent_id, to_summarize.len());

        match llm.call(&prompt_messages).await {
            Ok(summary) => {
                // Store the summary
                if let Ok(conn) = Connection::open(&self.db_path) {
                    let token_count = summary.len() / 4;
                    let _ = conn.execute(
                        "INSERT INTO summaries (agent_id, session_id, depth, content, token_count, source_count) VALUES (?1, ?2, 0, ?3, ?4, ?5)",
                        params![agent_id, session_id, summary, token_count, to_summarize.len()],
                    );
                }
                info!("[lcm:{}] Summary stored ({} chars from {} messages)", agent_id, summary.len(), to_summarize.len());
            }
            Err(e) => {
                warn!("[lcm:{}] Summarization failed: {} (continuing without compaction)", agent_id, e);
            }
        }
    }

    /// Search summaries for relevant context
    pub fn search_summaries(&self, agent_id: &str, query: &str) -> Vec<String> {
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut results = Vec::new();
        let query_lower = format!("%{}%", query.to_lowercase());

        if let Ok(mut stmt) = conn.prepare(
            "SELECT content FROM summaries WHERE agent_id = ?1 AND LOWER(content) LIKE ?2 ORDER BY created_at DESC LIMIT 5"
        ) {
            if let Ok(rows) = stmt.query_map(params![agent_id, query_lower], |row| {
                row.get::<_, String>(0)
            }) {
                for row in rows.flatten() {
                    results.push(row);
                }
            }
        }

        results
    }
}
