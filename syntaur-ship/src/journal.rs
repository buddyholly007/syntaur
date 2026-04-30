//! Append-only deploy journal at `~/vault/deploys/YYYY-MM.jsonl`.
//!
//! Every deploy (success OR failure) appends one JSON line with full
//! context. Immutable audit trail — useful for:
//! - Post-mortems ("when did v0.5.0 actually go live?")
//! - Security audits ("show me every deploy in April that bypassed
//!   the Mac Mini smoke")
//! - Statusline ("latest deploy at HH:MM was OK / FAIL")
//!
//! One file per calendar month keeps each file small + indexable.
//! Atomic append via O_APPEND + single `write()` per record.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub timestamp: DateTime<Utc>,
    /// "success" | "partial" | "aborted" | "rolled-back".
    /// `partial` = a stage failed AFTER the TrueNAS docker-restart
    /// already swung prod onto the new binary (viewer / version_audit /
    /// win11 are post-truenas). The deploy didn't reach a clean finish,
    /// but prod IS live with the new version. `aborted` is reserved for
    /// pre-truenas failures where prod is still on the previous version.
    pub outcome: String,
    pub version: String,
    pub git_head: String,
    pub gateway_sha256: Option<String>,
    pub pre_deploy_snapshot: Option<String>,
    pub deploy_session: String,
    pub skip_flags: Vec<String>,
    /// If outcome != success, the stage that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_stage: Option<String>,
    /// If outcome != success, human-readable reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// True only when outcome == "partial". Surfaces to the operator
    /// that prod is actually serving the new version even though the
    /// deploy line says non-success — so they don't roll back something
    /// that was already mostly succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prod_live: Option<bool>,
    /// How long the pipeline took wall-clock.
    pub duration_ms: u128,
}

pub fn append(vault_dir: &std::path::Path, entry: &JournalEntry) -> Result<()> {
    let deploys_dir = vault_dir.join("deploys");
    std::fs::create_dir_all(&deploys_dir)?;
    let path = deploys_dir.join(format!("{}.jsonl", entry.timestamp.format("%Y-%m")));

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    Ok(())
}

pub fn read_recent(vault_dir: &std::path::Path, last: usize) -> Result<Vec<JournalEntry>> {
    let deploys_dir = vault_dir.join("deploys");
    if !deploys_dir.exists() {
        return Ok(Vec::new());
    }
    // Read all .jsonl files in reverse-chronological order.
    let mut files: Vec<PathBuf> = std::fs::read_dir(&deploys_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|s| s == "jsonl").unwrap_or(false))
        .collect();
    files.sort();
    files.reverse();

    let mut out: Vec<JournalEntry> = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f)?;
        let mut entries: Vec<JournalEntry> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        entries.reverse();
        out.extend(entries);
        if out.len() >= last {
            break;
        }
    }
    out.truncate(last);
    Ok(out)
}
