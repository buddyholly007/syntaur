//! Core data types shared between the gateway and modules.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Citation produced by a tool that grounds its output in a source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// Connector / source name (e.g. "workspace_files", "paperless").
    pub source: String,
    /// Stable identifier within the source (e.g. file path, document id).
    pub external_id: String,
    /// Human-readable title (e.g. "SOUL.md (felix)").
    pub title: String,
    /// Excerpted text relevant to the query.
    pub snippet: String,
    /// Search rank score (higher = more relevant).
    pub rank: f64,
}

/// File-like artifact produced by a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: usize,
    /// Inline base64 content. Present iff size_bytes <= inline limit.
    pub content_base64: Option<String>,
    /// Path on the host where the artifact was persisted.
    pub stored_path: Option<String>,
}

/// Rich result returned by tools. Carries structured data alongside
/// the human-readable text.
#[derive(Debug, Clone, Serialize)]
pub struct RichToolResult {
    pub content: String,
    pub citations: Vec<Citation>,
    pub artifacts: Vec<Artifact>,
    pub structured: Option<Value>,
}

impl RichToolResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            citations: Vec::new(),
            artifacts: Vec::new(),
            structured: None,
        }
    }

    /// Format as a single string. Citations and artifacts are appended
    /// as numbered lists at the end.
    pub fn to_text(&self) -> String {
        if self.citations.is_empty() && self.artifacts.is_empty() {
            return self.content.clone();
        }
        let mut out = self.content.clone();
        if !self.citations.is_empty() {
            out.push_str("\n\nSources:\n");
            for (i, c) in self.citations.iter().enumerate() {
                out.push_str(&format!(
                    "  [{}] {} — {}\n",
                    i + 1,
                    c.title,
                    c.external_id
                ));
            }
        }
        if !self.artifacts.is_empty() {
            out.push_str("\nArtifacts:\n");
            for (i, a) in self.artifacts.iter().enumerate() {
                let size_kb = (a.size_bytes as f64) / 1024.0;
                let storage = if a.content_base64.is_some() {
                    "inline"
                } else {
                    "stored"
                };
                out.push_str(&format!(
                    "  [{}] {} ({}, {:.1} KB, {})\n",
                    i + 1,
                    a.filename,
                    a.mime_type,
                    size_kb,
                    storage
                ));
                if let Some(path) = &a.stored_path {
                    out.push_str(&format!("      → {}\n", path));
                }
            }
        }
        out
    }
}
