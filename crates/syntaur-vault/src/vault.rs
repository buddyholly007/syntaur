//! Plaintext vault data model — what lives inside the encryption boundary.
//!
//! The whole `Vault` is serialized to JSON then encrypted in one shot;
//! we don't do per-entry encryption. Simpler, no metadata leaks, and
//! the blob is tiny (hundreds of KB at most for any realistic number
//! of secrets).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::FORMAT_VERSION;

#[allow(dead_code)]
fn _assert_entry_zeroize_on_drop() {
    fn needs<T: ZeroizeOnDrop>() {}
    needs::<Entry>();
}

/// A single secret. `value` is zeroized on drop so we don't leave
/// copies in freed heap pages.
#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Entry {
    /// The actual secret bytes (API key / token / passphrase).
    pub value: String,
    /// Human-readable description. Never contains secret material.
    #[zeroize(skip)]
    #[serde(default)]
    pub description: String,
    /// Longer notes (URL, rotation schedule, etc.). Never secret.
    #[zeroize(skip)]
    #[serde(default)]
    pub notes: String,
    /// Free-form tags for filtering — e.g. `["api", "llm", "bot"]`.
    #[zeroize(skip)]
    #[serde(default)]
    pub tags: Vec<String>,
    /// First time this entry was written.
    #[zeroize(skip)]
    pub created_at: DateTime<Utc>,
    /// Most recent `set` against this name.
    #[zeroize(skip)]
    pub updated_at: DateTime<Utc>,
}

impl Entry {
    pub fn new(value: String) -> Self {
        let now = Utc::now();
        Self {
            value,
            description: String::new(),
            notes: String::new(),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// The full encrypted payload.
///
/// Deliberately NOT `Zeroize`d at the Vault level — BTreeMap isn't
/// Zeroize-able as a whole, but each `Entry` zeros its own `value` on
/// drop, so when the map drops every entry-value gets cleared
/// individually. Description/notes/tags never contain secret
/// material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    /// Matches `crate::FORMAT_VERSION`. Future format bumps migrate
    /// on read.
    pub version: u8,
    /// Secret store keyed by name. BTreeMap so `list` is stable-sorted.
    pub entries: BTreeMap<String, Entry>,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, entry: Entry) {
        self.entries.insert(name.to_string(), entry);
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Entry> {
        self.entries.remove(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

/// Public metadata for `list` — deliberately scrubs `value` + `notes`
/// so the agent can hand this back to any caller without leaking
/// secret material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMeta {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Length of the plaintext secret in bytes — useful feedback
    /// ("it's there, 48 chars") without exposing the value.
    pub value_len: usize,
}

impl EntryMeta {
    pub fn from_entry(name: &str, e: &Entry) -> Self {
        Self {
            name: name.to_string(),
            description: e.description.clone(),
            tags: e.tags.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            value_len: e.value.len(),
        }
    }
}
