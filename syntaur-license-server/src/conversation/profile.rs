//! Persistent user profile — remembered across all conversations.
//!
//! Stores name, preferences, location, timezone, and freeform notes.
//! Injected into the system prompt so the assistant knows who it's talking to.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub preferences: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl UserProfile {
    /// Render as context for the system prompt.
    pub fn as_context(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(ref name) = self.name {
            parts.push(format!("The user's name is {}.", name));
        }
        if let Some(ref tz) = self.timezone {
            parts.push(format!("Their timezone is {}.", tz));
        }
        if let Some(ref loc) = self.location {
            parts.push(format!("They are located in {}.", loc));
        }
        for pref in &self.preferences {
            parts.push(pref.clone());
        }
        for note in &self.notes {
            parts.push(note.clone());
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.timezone.is_none()
            && self.location.is_none()
            && self.preferences.is_empty()
            && self.notes.is_empty()
    }
}

pub struct ProfileStore {
    db: Mutex<Connection>,
}

impl ProfileStore {
    pub fn open(data_dir: &str) -> Self {
        let db_path = format!("{}/conversations.db", data_dir);
        let db = Connection::open(&db_path).expect("Failed to open conversations.db for profiles");
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_profile (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .expect("Failed to create profile table");

        Self {
            db: Mutex::new(db),
        }
    }

    pub async fn get(&self) -> UserProfile {
        let db = self.db.lock().await;
        let mut profile = UserProfile::default();

        let mut stmt = db
            .prepare("SELECT key, value FROM user_profile")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        for (key, value) in rows {
            match key.as_str() {
                "name" => profile.name = Some(value),
                "timezone" => profile.timezone = Some(value),
                "location" => profile.location = Some(value),
                k if k.starts_with("pref:") => profile.preferences.push(value),
                k if k.starts_with("note:") => profile.notes.push(value),
                _ => {}
            }
        }

        profile
    }

    pub async fn set_field(&self, key: &str, value: &str) {
        let db = self.db.lock().await;
        db.execute(
            "INSERT OR REPLACE INTO user_profile (key, value) VALUES (?, ?)",
            params![key, value],
        )
        .ok();
        info!("[profile] set {}={}", key, value);
    }

    pub async fn remove_field(&self, key: &str) {
        let db = self.db.lock().await;
        db.execute("DELETE FROM user_profile WHERE key = ?", params![key])
            .ok();
    }

    pub async fn update(&self, profile: &UserProfile) {
        if let Some(ref name) = profile.name {
            self.set_field("name", name).await;
        }
        if let Some(ref tz) = profile.timezone {
            self.set_field("timezone", tz).await;
        }
        if let Some(ref loc) = profile.location {
            self.set_field("location", loc).await;
        }
        // Clear and re-add preferences/notes
        {
            let db = self.db.lock().await;
            db.execute("DELETE FROM user_profile WHERE key LIKE 'pref:%'", [])
                .ok();
            db.execute("DELETE FROM user_profile WHERE key LIKE 'note:%'", [])
                .ok();
        }
        for (i, pref) in profile.preferences.iter().enumerate() {
            self.set_field(&format!("pref:{}", i), pref).await;
        }
        for (i, note) in profile.notes.iter().enumerate() {
            self.set_field(&format!("note:{}", i), note).await;
        }
    }
}
