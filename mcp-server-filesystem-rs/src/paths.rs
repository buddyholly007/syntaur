//! Path validation and normalization.
//!
//! All file operations go through `validate_path` first, which:
//!   1. Expands `~/...` to the caller's home directory.
//!   2. Resolves to an absolute, normalized path.
//!   3. Verifies the path is inside one of the allowed directories.
//!   4. If the path exists, follows symlinks via `canonicalize` and verifies
//!      the *real* target is also inside the allowed set (prevents symlink
//!      escape attacks).
//!   5. If the path does not exist (e.g. for `write_file`), verifies the
//!      *parent* directory's real path is inside the allowed set, so we
//!      can't be tricked into creating a file outside the sandbox.

use std::path::{Component, Path, PathBuf};

/// Expand a leading `~` to the user's home directory. No-op for any other
/// path. We do this manually rather than depending on a crate so we don't
/// pull in shellexpand or dirs for one function.
pub fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    } else if input == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(input)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Normalize a path: collapse `.` and `..` components without touching the
/// filesystem. Used as a fallback when `canonicalize` fails (file doesn't
/// exist yet) so we still get a stable, comparable absolute path.
pub fn normalize_absolute(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };

    let mut out = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// True if `path` is inside one of `allowed_dirs`. Comparison is purely
/// path-string based; the caller is responsible for canonicalizing first.
pub fn is_within_allowed(path: &Path, allowed_dirs: &[PathBuf]) -> bool {
    allowed_dirs.iter().any(|d| path.starts_with(d))
}

/// Result of `validate_path`: the canonicalized path that file operations
/// should actually use.
#[derive(Debug, Clone)]
pub struct ValidatedPath {
    pub real: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("Access denied - path outside allowed directories: {path} not in {allowed}")]
    OutsideAllowed { path: String, allowed: String },
    #[error("Access denied - symlink target outside allowed directories: {real} not in {allowed}")]
    SymlinkOutsideAllowed { real: String, allowed: String },
    #[error("Access denied - parent directory outside allowed directories: {parent} not in {allowed}")]
    ParentOutsideAllowed { parent: String, allowed: String },
    #[error("Parent directory does not exist: {0}")]
    ParentMissing(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Validate a requested path against the allowed directory list. Returns the
/// canonicalized real path on success.
///
/// This is a blocking helper because all callers run inside `spawn_blocking`
/// already; making it async would just add a layer of indirection.
pub fn validate_path(requested: &str, allowed_dirs: &[PathBuf]) -> Result<ValidatedPath, PathError> {
    let expanded = expand_home(requested);
    let absolute = if expanded.is_absolute() {
        expanded.clone()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or_else(|_| expanded.clone())
    };
    let normalized = normalize_absolute(&absolute);

    if !is_within_allowed(&normalized, allowed_dirs) {
        return Err(PathError::OutsideAllowed {
            path: normalized.display().to_string(),
            allowed: allowed_join(allowed_dirs),
        });
    }

    match std::fs::canonicalize(&normalized) {
        Ok(real) => {
            if !is_within_allowed(&real, allowed_dirs) {
                return Err(PathError::SymlinkOutsideAllowed {
                    real: real.display().to_string(),
                    allowed: allowed_join(allowed_dirs),
                });
            }
            Ok(ValidatedPath { real })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // For new files: verify parent dir is allowed.
            let parent = normalized
                .parent()
                .ok_or_else(|| PathError::ParentMissing(normalized.display().to_string()))?;
            match std::fs::canonicalize(parent) {
                Ok(real_parent) => {
                    if !is_within_allowed(&real_parent, allowed_dirs) {
                        return Err(PathError::ParentOutsideAllowed {
                            parent: real_parent.display().to_string(),
                            allowed: allowed_join(allowed_dirs),
                        });
                    }
                    // Use the unresolved absolute path so the caller writes
                    // exactly where they asked, not somewhere reachable
                    // through a different symlink.
                    Ok(ValidatedPath { real: normalized })
                }
                Err(_) => Err(PathError::ParentMissing(parent.display().to_string())),
            }
        }
        Err(e) => Err(PathError::Io(e)),
    }
}

fn allowed_join(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
