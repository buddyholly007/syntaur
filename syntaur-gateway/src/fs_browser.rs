//! `/api/fs/list` — server-driven folder browser.
//!
//! Powers the "Add music folder" (and other) pickers in the UI. The server
//! is the one with filesystem access, so the browser just asks it to
//! enumerate directories. Read-only; only returns names + is_dir flags.
//!
//! Scope limits: only reads under the caller's home directory and a
//! curated set of common network-mount roots (`/mnt`, `/media`, `/Volumes`).
//! Anything outside those prefixes returns 403 so a user-facing picker
//! can't be coerced into browsing /etc or similar.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::auth::Principal;
use crate::AppState;

#[derive(Deserialize)]
pub struct FsListQuery {
    pub token: String,
    /// Absolute path to list. If omitted or "", returns the list of
    /// allowed root shortcuts (home + /mnt + /media + /Volumes) without
    /// descending anywhere.
    pub path: Option<String>,
    /// When true, include hidden entries (dotfiles/dirs). Default false.
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Serialize)]
pub struct FsListResponse {
    /// The resolved absolute path the listing is for ("" when at root shortcuts).
    pub path: String,
    /// Parent path for breadcrumb navigation (None when at the root shortcut view).
    pub parent: Option<String>,
    /// Quick shortcuts: Home, Network (/mnt, /media), macOS /Volumes.
    /// Always present so the user can jump back regardless of where they are.
    pub roots: Vec<FsRoot>,
    /// Directory entries, alphabetized. Only `is_dir` entries when `dirs_only`
    /// is the caller's intent (the frontend filters visually; backend always
    /// returns both so we can show non-dir count as a hint).
    pub entries: Vec<FsEntry>,
    /// Best-effort human-readable display name (for breadcrumbs).
    pub display: String,
}

#[derive(Serialize)]
pub struct FsRoot {
    pub label: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    /// Best-effort last-modified timestamp (seconds since epoch), 0 if unknown.
    pub modified: i64,
}

pub async fn handle_fs_list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsListQuery>,
) -> Result<Json<FsListResponse>, StatusCode> {
    // Authenticate — this endpoint exposes directory structure, so it
    // must be gated even though it's read-only.
    let _principal = crate::resolve_principal(&state, &q.token).await?;

    let roots = build_root_shortcuts();

    // No path → just return shortcuts so the UI can show a landing view.
    let requested = q.path.as_deref().map(str::trim).unwrap_or("");
    if requested.is_empty() {
        return Ok(Json(FsListResponse {
            path: String::new(),
            parent: None,
            roots,
            entries: Vec::new(),
            display: "Pick a location".to_string(),
        }));
    }

    // Resolve ~ and canonicalize so we can compare cleanly against the
    // allowed-roots list. Non-existent paths error out with 404 so the UI
    // can show "folder doesn't exist".
    let expanded = expand_tilde(requested);
    let canonical = match std::fs::canonicalize(&expanded) {
        Ok(c) => c,
        Err(_) => return Err(StatusCode::NOT_FOUND),
    };

    if !is_path_allowed(&canonical, &roots) {
        return Err(StatusCode::FORBIDDEN);
    }

    let meta = match std::fs::metadata(&canonical) {
        Ok(m) => m,
        Err(_) => return Err(StatusCode::NOT_FOUND),
    };
    if !meta.is_dir() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let read = match std::fs::read_dir(&canonical) {
        Ok(r) => r,
        Err(_) => return Err(StatusCode::FORBIDDEN),
    };

    let mut entries: Vec<FsEntry> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !q.show_hidden && name.starts_with('.') {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some(FsEntry {
                name,
                is_dir: meta.is_dir(),
                modified,
            })
        })
        .collect();

    // Directories first, then files; both alphabetized case-insensitively
    // so the picker feels native.
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let parent = canonical
        .parent()
        .filter(|p| !p.as_os_str().is_empty() && is_path_allowed(p, &roots))
        .map(|p| p.to_string_lossy().to_string());

    let display = canonical
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| canonical.to_string_lossy().to_string());

    Ok(Json(FsListResponse {
        path: canonical.to_string_lossy().to_string(),
        parent,
        roots,
        entries,
        display,
    }))
}

fn build_root_shortcuts() -> Vec<FsRoot> {
    let mut out = Vec::new();

    if let Some(home) = dirs_home() {
        out.push(FsRoot {
            label: "Home".to_string(),
            path: home.to_string_lossy().to_string(),
            exists: home.is_dir(),
        });
        // Common music subdirs as one-click shortcuts
        for sub in ["Music", "Downloads", "Documents"] {
            let p = home.join(sub);
            if p.is_dir() {
                out.push(FsRoot {
                    label: sub.to_string(),
                    path: p.to_string_lossy().to_string(),
                    exists: true,
                });
            }
        }
    }

    // Network / mounted drive conventions
    for (label, path) in [
        ("Network (/mnt)", "/mnt"),
        ("Removable (/media)", "/media"),
        ("Volumes (macOS)", "/Volumes"),
    ] {
        let p = Path::new(path);
        if p.is_dir() {
            out.push(FsRoot {
                label: label.to_string(),
                path: path.to_string(),
                exists: true,
            });
        }
    }

    out
}

/// Best-effort current-user home resolution without an extra crate.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return dirs_home().unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Only allow paths that descend from a known-safe root. Also reject any
/// path containing `..` after canonicalization as a belt-and-suspenders
/// check (canonicalize should resolve those, but this catches edge cases
/// like parent-of-symlink).
fn is_path_allowed(path: &Path, roots: &[FsRoot]) -> bool {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    for r in roots {
        let root_path = Path::new(&r.path);
        // Canonicalize root too so we compare apples-to-apples (handles
        // /home/sean vs /home/sean/ vs symlinked paths).
        let canon_root = std::fs::canonicalize(root_path).unwrap_or_else(|_| root_path.to_path_buf());
        if path.starts_with(&canon_root) {
            return true;
        }
    }
    false
}
