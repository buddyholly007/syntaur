//! Crate error surface. Stage 1 scope — extended in Phase 3 with
//! commissioning-specific variants.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MatterFabricError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rs-matter internal: {0}")]
    Matter(String),
    #[error("master key {path} missing or wrong shape (want 32 bytes)")]
    MasterKey { path: String },
    #[error("encryption envelope: {0}")]
    Envelope(String),
    #[error("fabric {label:?} already exists at {path}")]
    AlreadyExists { label: String, path: String },
    #[error("fabric {label:?} not found in {dir}")]
    NotFound { label: String, dir: String },
    #[error("fabric label {label:?} contains unsafe chars (only [a-zA-Z0-9_-] allowed)")]
    BadLabel { label: String },
}
