//! Uploaded files connector — indexes documents the user has uploaded
//! through the /knowledge page.
//!
//! Each file lives under `<data_dir>/uploads/knowledge/`. The external_id
//! is the file name (unique because uploads are timestamped + UUID-suffixed
//! in `handle_knowledge_upload`), the title is the original filename
//! recorded in a `.meta.json` sidecar when available, otherwise the file
//! stem.
//!
//! Extraction strategy:
//!   - `.pdf` → `pdf_extract::extract_text`
//!   - everything else → `std::fs::read_to_string` (UTF-8, skip on error)
//!
//! Binary files with no extractor are skipped silently.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use log::{debug, warn};
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

pub const SOURCE_NAME: &str = "uploaded_files";

const MAX_BODY_BYTES: usize = 4_000_000;
/// Extensions we handle as plain text (UTF-8). Anything else needs a
/// dedicated extractor branch below.
const TEXT_EXTS: &[&str] = &[
    "md", "txt", "rst", "log", "csv", "tsv",
    "json", "yaml", "yml", "toml", "ini", "conf",
    "html", "htm", "xml",
    "rs", "py", "go", "js", "ts", "tsx", "jsx", "c", "cpp", "h", "hpp",
    "java", "rb", "sh", "fish", "zsh", "sql",
];

pub struct UploadedFilesConnector {
    name: String,
    root: PathBuf,
}

impl UploadedFilesConnector {
    pub fn new(root: PathBuf) -> Self {
        Self {
            name: SOURCE_NAME.to_string(),
            root,
        }
    }

    /// Ensure the storage directory exists. Called once at boot.
    pub fn ensure_root(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.root) {
            warn!(
                "[uploaded_files] could not create {}: {}",
                self.root.display(),
                e
            );
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Walk the uploads dir once and return one ExternalDoc per extractable file.
    /// Errors on individual files are logged and skipped, not propagated.
    pub fn scan(&self) -> Vec<ExternalDoc> {
        let mut out = Vec::new();
        if !self.root.is_dir() {
            return out;
        }
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    "[uploaded_files] read_dir {} failed: {}",
                    self.root.display(),
                    e
                );
                return out;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Skip sidecar metadata files
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext == "meta" {
                    continue;
                }
            }
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.ends_with(".meta.json") {
                    continue;
                }
            }
            match file_to_doc(&path) {
                Ok(Some(doc)) => out.push(doc),
                Ok(None) => debug!(
                    "[uploaded_files] skipped {}: no extractor",
                    path.display()
                ),
                Err(e) => warn!(
                    "[uploaded_files] extract {} failed: {}",
                    path.display(),
                    e
                ),
            }
        }
        out
    }

    /// Remove a stored file by its external_id (== file stem produced by the
    /// upload handler). Returns true if a file was actually removed.
    pub fn delete_by_external_id(&self, external_id: &str) -> bool {
        // external_id is the filename relative to self.root (exact match,
        // no path traversal via ..).
        if external_id.contains('/') || external_id.contains("..") {
            return false;
        }
        let path = self.root.join(external_id);
        let mut removed = false;
        if path.is_file() {
            if std::fs::remove_file(&path).is_ok() {
                removed = true;
            }
        }
        // Also remove matching sidecar if present.
        let meta = self.root.join(format!("{}.meta.json", external_id));
        let _ = std::fs::remove_file(meta);
        removed
    }
}

impl Connector for UploadedFilesConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for UploadedFilesConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            UploadedFilesConnector {
                name: SOURCE_NAME.to_string(),
                root,
            }
            .scan()
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))
    }
}

#[async_trait]
impl SlimConnector for UploadedFilesConnector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&root) {
                for e in entries.flatten() {
                    if !e.path().is_file() {
                        continue;
                    }
                    let name = match e.file_name().into_string() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if name.ends_with(".meta.json") {
                        continue;
                    }
                    let updated = e
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| Utc.timestamp_opt(d.as_secs() as i64, 0).single());
                    out.push(DocIdOnly {
                        external_id: name,
                        updated_at: updated,
                    });
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }
}

/// Extract text from a single file. Returns `Ok(None)` if the extension
/// isn't handled; `Err` only for I/O or extractor failures on a file we
/// *tried* to read.
pub fn file_to_doc(path: &Path) -> Result<Option<ExternalDoc>, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let external_id = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "bad filename".to_string())?
        .to_string();

    let body = extract_text(path, &ext)?;
    let body = if body.is_none() {
        return Ok(None);
    } else {
        body.unwrap()
    };

    // Truncate overly large bodies — the chunker is fine with it but this
    // keeps the FTS table from ballooning on a single dump file.
    let body = if body.len() > MAX_BODY_BYTES {
        let mut truncated = body[..MAX_BODY_BYTES].to_string();
        truncated.push_str("\n\n[truncated]");
        truncated
    } else {
        body
    };

    // Title: sidecar override → original filename → file stem
    let sidecar_path = path.with_extension(format!(
        "{}.meta.json",
        ext
    ));
    let title = load_sidecar_title(&sidecar_path)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&external_id)
                .to_string()
        });

    let updated_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| Utc.timestamp_opt(d.as_secs() as i64, 0).single())
        .unwrap_or_else(Utc::now);

    Ok(Some(ExternalDoc {
        source: SOURCE_NAME.to_string(),
        external_id,
        title,
        body,
        updated_at,
        metadata: json!({
            "ext": ext,
            "path": path.display().to_string(),
        }),
    }))
}

fn extract_text(path: &Path, ext: &str) -> Result<Option<String>, String> {
    if ext == "pdf" {
        return pdf_extract::extract_text(path)
            .map(Some)
            .map_err(|e| format!("pdf_extract: {}", e));
    }
    if TEXT_EXTS.iter().any(|e| *e == ext) || ext.is_empty() {
        return std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| format!("read_to_string: {}", e));
    }
    Ok(None)
}

fn load_sidecar_title(sidecar: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(sidecar).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("original_filename")
        .and_then(|t| t.as_str())
        .map(String::from)
}
