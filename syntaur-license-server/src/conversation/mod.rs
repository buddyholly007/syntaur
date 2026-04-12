//! Server-side conversation persistence with SQLite.
//!
//! Key lessons from Peter voice pipeline:
//! - Store ONLY final user+assistant text, never tool intermediates
//! - Max turns prevents context bloat (10 for voice, 50 for chat)
//! - Auto-expire inactive conversations (5 min voice, 30 min chat)
//! - Token budget enforcement: truncate oldest turns when over budget

pub mod profile;

use std::time::{Duration, Instant};

use log::{debug, info};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::task::{Message, MessageRole, TaskCategory};

/// Per-category context budget defaults (from Peter pipeline analysis).
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Max generation tokens for the LLM response.
    pub max_gen_tokens: u32,
    /// Max number of user+assistant turn pairs to keep.
    pub max_turns: usize,
    /// Conversation auto-expire timeout.
    pub timeout: Duration,
    /// Approximate max tokens for conversation history.
    pub history_token_budget: u32,
}

impl ContextBudget {
    pub fn for_category(category: &TaskCategory) -> Self {
        match category {
            TaskCategory::Conversation => Self {
                max_gen_tokens: 4096,
                max_turns: 25,
                timeout: Duration::from_secs(1800), // 30 min
                history_token_budget: 4000,
            },
            TaskCategory::Search => Self {
                max_gen_tokens: 2048,
                max_turns: 10,
                timeout: Duration::from_secs(600), // 10 min
                history_token_budget: 1000,
            },
            TaskCategory::Coding => Self {
                max_gen_tokens: 8192,
                max_turns: 15,
                timeout: Duration::from_secs(1800),
                history_token_budget: 4000,
            },
            TaskCategory::Research => Self {
                max_gen_tokens: 8192,
                max_turns: 15,
                timeout: Duration::from_secs(1800),
                history_token_budget: 4000,
            },
            _ => Self {
                max_gen_tokens: 4096,
                max_turns: 20,
                timeout: Duration::from_secs(1800),
                history_token_budget: 4000,
            },
        }
    }

    /// Voice-specific budget: lean, fast, short responses.
    pub fn voice() -> Self {
        Self {
            max_gen_tokens: 200,
            max_turns: 5,
            timeout: Duration::from_secs(300), // 5 min
            history_token_budget: 500,
        }
    }
}

/// Rough token estimate: ~4 chars per token for English text.
fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32 + 3) / 4
}

/// Trim messages to fit within a token budget, keeping the most recent.
pub fn trim_to_budget(messages: &[Message], budget: &ContextBudget) -> Vec<Message> {
    let mut result: Vec<Message> = messages.to_vec();

    // Enforce max turns (count user+assistant pairs)
    let turn_count = result
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .count();
    if turn_count > budget.max_turns {
        let excess = turn_count - budget.max_turns;
        let mut removed = 0;
        result.retain(|m| {
            if removed >= excess * 2 {
                return true;
            }
            if m.role == MessageRole::System {
                return true; // never remove system messages
            }
            removed += 1;
            false
        });
    }

    // Enforce token budget — drop oldest non-system messages
    loop {
        let total: u32 = result.iter().map(|m| estimate_tokens(&m.content)).sum();
        if total <= budget.history_token_budget || result.len() <= 1 {
            break;
        }
        // Find first non-system message and remove it
        if let Some(pos) = result.iter().position(|m| m.role != MessageRole::System) {
            result.remove(pos);
        } else {
            break;
        }
    }

    result
}

/// SQLite-backed conversation store.
pub struct ConversationStore {
    db: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub category: String,
    pub title: Option<String>,
    pub created_at: String,
    pub last_active: String,
    pub message_count: u32,
}

impl ConversationStore {
    pub fn open(data_dir: &str) -> Self {
        let db_path = format!("{}/conversations.db", data_dir);
        let db = Connection::open(&db_path).expect("Failed to open conversations.db");
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL DEFAULT 'conversation',
                title TEXT,
                created_at TEXT NOT NULL,
                last_active TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id)
            );
            CREATE INDEX IF NOT EXISTS idx_msg_conv ON messages(conversation_id);
            -- Add title column if upgrading from older schema
            CREATE INDEX IF NOT EXISTS idx_conv_title ON conversations(title);",
        )
        .expect("Failed to create conversation tables");
        // Schema migration: add title column if missing
        db.execute("ALTER TABLE conversations ADD COLUMN title TEXT", []).ok();

        info!("[conversations] opened {}", db_path);
        Self {
            db: Mutex::new(db),
        }
    }

    /// Create a new conversation, return its ID.
    pub async fn create(&self, category: &str) -> String {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO conversations (id, category, created_at, last_active) VALUES (?, ?, ?, ?)",
            params![id, category, now, now],
        )
        .ok();
        debug!("[conversations] created {} ({})", id, category);
        id
    }

    /// Set a conversation's title.
    pub async fn set_title(&self, conversation_id: &str, title: &str) {
        let db = self.db.lock().await;
        db.execute(
            "UPDATE conversations SET title = ? WHERE id = ?",
            params![title, conversation_id],
        )
        .ok();
    }

    /// Auto-generate a title from the first user message (truncated, cleaned).
    pub fn auto_title(first_message: &str) -> String {
        let cleaned = first_message
            .lines()
            .next()
            .unwrap_or(first_message)
            .trim();
        if cleaned.len() <= 60 {
            cleaned.to_string()
        } else {
            format!("{}...", &cleaned[..57])
        }
    }

    /// Search conversations by title or message content.
    pub async fn search(&self, query: &str, limit: u32) -> Vec<Conversation> {
        let db = self.db.lock().await;
        let pattern = format!("%{}%", query.to_lowercase());
        let mut stmt = db
            .prepare(
                "SELECT DISTINCT c.id, c.category, c.title, c.created_at, c.last_active,
                 (SELECT COUNT(*) FROM messages WHERE conversation_id = c.id)
                 FROM conversations c
                 LEFT JOIN messages m ON m.conversation_id = c.id
                 WHERE LOWER(c.title) LIKE ? OR LOWER(m.content) LIKE ?
                 ORDER BY c.last_active DESC LIMIT ?",
            )
            .unwrap();
        stmt.query_map(params![pattern, pattern, limit], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                category: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                last_active: row.get(4)?,
                message_count: row.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Append a message (user or assistant text only — never tool intermediates).
    pub async fn append(&self, conversation_id: &str, role: &str, content: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO messages (conversation_id, role, content, created_at) VALUES (?, ?, ?, ?)",
            params![conversation_id, role, content, now],
        )
        .ok();
        db.execute(
            "UPDATE conversations SET last_active = ? WHERE id = ?",
            params![now, conversation_id],
        )
        .ok();
    }

    /// Load conversation messages (only user + assistant, never system/tool).
    pub async fn messages(&self, conversation_id: &str) -> Vec<Message> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare("SELECT role, content FROM messages WHERE conversation_id = ? ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map(params![conversation_id], |row| {
                let role: String = row.get(0)?;
                let content: String = row.get(1)?;
                Ok((role, content))
            })
            .unwrap();

        rows.filter_map(|r| r.ok())
            .map(|(role, content)| match role.as_str() {
                "assistant" => Message::assistant(content),
                _ => Message::user(content),
            })
            .collect()
    }

    /// Get conversation metadata.
    pub async fn get(&self, conversation_id: &str) -> Option<Conversation> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT c.id, c.category, c.title, c.created_at, c.last_active,
                 (SELECT COUNT(*) FROM messages WHERE conversation_id = c.id)
                 FROM conversations c WHERE c.id = ?",
            )
            .ok()?;
        stmt.query_row(params![conversation_id], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                category: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                last_active: row.get(4)?,
                message_count: row.get(5)?,
            })
        })
        .ok()
    }

    /// List recent conversations.
    pub async fn list(&self, limit: u32) -> Vec<Conversation> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT c.id, c.category, c.title, c.created_at, c.last_active,
                 (SELECT COUNT(*) FROM messages WHERE conversation_id = c.id)
                 FROM conversations c ORDER BY c.last_active DESC LIMIT ?",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                    last_active: row.get(4)?,
                    message_count: row.get(5)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Purge conversations older than the given duration.
    pub async fn purge_expired(&self, max_age: Duration) {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age).unwrap_or_default();
        let cutoff_str = cutoff.to_rfc3339();
        let db = self.db.lock().await;
        let deleted: usize = db
            .execute(
                "DELETE FROM messages WHERE conversation_id IN
                 (SELECT id FROM conversations WHERE last_active < ?)",
                params![cutoff_str],
            )
            .unwrap_or(0);
        let convs: usize = db
            .execute(
                "DELETE FROM conversations WHERE last_active < ?",
                params![cutoff_str],
            )
            .unwrap_or(0);
        if convs > 0 {
            info!(
                "[conversations] purged {} conversations ({} messages)",
                convs, deleted
            );
        }
    }
}
