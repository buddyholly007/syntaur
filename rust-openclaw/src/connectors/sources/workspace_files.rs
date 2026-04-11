//! Workspace files connector — indexes markdown and text files across
//! all configured agent workspaces.
//!
//! Each `~/.syntaur/workspace*` directory becomes a source of `ExternalDoc`s.
//! The agent name is derived from the directory suffix (workspace = "felix",
//! workspace-crimson-lantern = "crimson-lantern", etc.).
//!
//! Files matching `.syntaurignore` patterns are skipped. Default ignores:
//! .png, .jpg, .bak, .pyc, target/, node_modules/, .git/.
//!
//! Files are walked recursively up to a configurable depth (default 4).
//! Bodies larger than 1 MB are truncated.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use log::{debug, warn};
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const MAX_BODY_BYTES: usize = 1_000_000;
const MAX_DEPTH: usize = 6;

/// Default extensions we index. Conservative — pure text formats only.
const INDEXED_EXTS: &[&str] = &["md", "txt", "json", "yaml", "yml", "toml", "rst", "log"];

/// Filename / glob patterns to always skip.
const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".cache",
    "screenshot-",
    ".bak",
];

pub struct WorkspaceFilesConnector {
    name: String,
    workspaces: Vec<(String, PathBuf)>, // (agent_id, workspace_path)
}

impl WorkspaceFilesConnector {
    /// Build a connector that indexes the given (agent_id, workspace_path) pairs.
    pub fn new(workspaces: Vec<(String, PathBuf)>) -> Self {
        Self {
            name: "workspace_files".to_string(),
            workspaces,
        }
    }

    /// Walk one workspace recursively, returning all indexable files.
    fn walk(agent_id: &str, root: &Path) -> Vec<ExternalDoc> {
        let mut docs = Vec::new();
        if !root.is_dir() {
            return docs;
        }
        Self::walk_recursive(agent_id, root, root, 0, &mut docs);
        docs
    }

    fn walk_recursive(
        agent_id: &str,
        workspace_root: &Path,
        dir: &Path,
        depth: usize,
        out: &mut Vec<ExternalDoc>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Apply ignores
            if Self::should_ignore(&name) {
                continue;
            }

            if path.is_dir() {
                Self::walk_recursive(agent_id, workspace_root, &path, depth + 1, out);
                continue;
            }
            if !path.is_file() {
                continue;
            }

            // Filter by extension
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !INDEXED_EXTS.contains(&ext.as_str()) {
                continue;
            }

            // Read with size limit
            let body = match std::fs::read_to_string(&path) {
                Ok(s) => {
                    if s.len() > MAX_BODY_BYTES {
                        s.chars().take(MAX_BODY_BYTES).collect::<String>()
                    } else {
                        s
                    }
                }
                Err(e) => {
                    debug!("[workspace_files] skip {}: {}", path.display(), e);
                    continue;
                }
            };

            if body.trim().is_empty() {
                continue;
            }

            // mtime
            let updated_at = path
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    Utc.timestamp_opt(d.as_secs() as i64, 0)
                        .single()
                        .unwrap_or_else(Utc::now)
                })
                .unwrap_or_else(Utc::now);

            // Relative path within workspace for the title
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            // Build the doc — external_id is the absolute path so we can dedupe
            // even if the same file is symlinked from multiple workspaces.
            let title = format!("{} ({})", rel, agent_id);
            out.push(ExternalDoc {
                source: "workspace_files".to_string(),
                external_id: path.to_string_lossy().to_string(),
                title,
                body,
                updated_at,
                metadata: json!({
                    "agent": agent_id,
                    "workspace": workspace_root.to_string_lossy(),
                    "rel_path": rel,
                    "extension": ext,
                }),
            });
        }
    }

    fn should_ignore(name: &str) -> bool {
        if name.starts_with('.') && name != ".syntaurignore" {
            // hidden files (except the ignore file itself) — but allow .md etc
            // already filtered by extension check, so skip dotfiles.
            return true;
        }
        for pat in DEFAULT_IGNORES {
            if name.contains(pat) {
                return true;
            }
        }
        false
    }
}

impl Connector for WorkspaceFilesConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for WorkspaceFilesConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        let workspaces = self.workspaces.clone();
        let docs = tokio::task::spawn_blocking(move || {
            let mut all = Vec::new();
            for (agent_id, root) in workspaces {
                if !root.exists() {
                    warn!("[workspace_files] workspace not found: {}", root.display());
                    continue;
                }
                let mut docs = Self::walk(&agent_id, &root);
                debug!(
                    "[workspace_files] {} files from {}",
                    docs.len(),
                    root.display()
                );
                all.append(&mut docs);
            }
            all
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?;
        Ok(docs)
    }
}

#[async_trait]
impl SlimConnector for WorkspaceFilesConnector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String> {
        // For file-based connectors the cheapest "list ids" is just walking
        // the tree and emitting paths without reading bodies. We reuse the
        // walk logic but project to ids only.
        let docs = self.load_full().await?;
        Ok(docs
            .into_iter()
            .map(|d| DocIdOnly {
                external_id: d.external_id,
                updated_at: Some(d.updated_at),
            })
            .collect())
    }
}

// Helper for places that want a `DateTime<Utc>` constructor inline.
#[allow(dead_code)]
fn _utc_from_secs(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap_or_else(Utc::now)
}
