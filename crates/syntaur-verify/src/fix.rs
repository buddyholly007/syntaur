//! Phase 2b — apply / revert / track Opus-proposed source edits.
//!
//! The auto-fix loop lives in the CLI binary (it owns the browser
//! handle and the build/reload machinery). This module is the pure
//! file-ops substrate: given a list of `FindingEdit`s, apply them
//! atomically-per-attempt with exact pre-images captured so a revert
//! is a single pass of `fs::write(path, pre_image)`.
//!
//! Invariants:
//!   - `old_string` must appear EXACTLY ONCE in the target file.
//!     This is a hard guard — we refuse to guess which occurrence
//!     the model meant.
//!   - `file` paths are workspace-relative. Any edit whose resolved
//!     absolute path escapes the workspace is rejected.
//!   - Every successful `apply_edits` returns a `FixAttempt` whose
//!     `revert()` restores the pre-images. If apply aborts partway
//!     through (e.g. a later edit's old_string isn't unique), the
//!     already-applied edits are immediately reverted before
//!     returning — the workspace is left as it was.
//!   - `count_loc_delta` is a cheap line-count proxy used as a blast-
//!     radius check. It counts "lines in new_string that weren't in
//!     old_string" + "lines in old_string that aren't in new_string"
//!     — not a proper LCS diff, but enough to catch 200-line
//!     rewrites masquerading as tweaks.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::run::FindingEdit;

/// Caps for one auto-fix pass — Sean's call ("3 iterations, 200 LoC").
#[derive(Debug, Clone, Copy)]
pub struct Budgets {
    pub max_iterations: usize,
    pub max_loc: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            max_loc: 200,
        }
    }
}

/// One successfully applied edit — everything needed to undo it.
#[derive(Debug, Clone)]
pub struct AppliedEdit {
    /// Absolute path as written to.
    pub abs_path: PathBuf,
    /// Workspace-relative (for logs + reports).
    pub rel_path: String,
    /// File contents BEFORE this edit (for revert).
    pub pre_image: String,
    /// File contents AFTER this edit (what we wrote).
    pub post_image: String,
    /// Cheap line-delta count used against the budget.
    pub loc_delta: usize,
}

/// The result of applying one batch of edits. Owns the pre-images
/// needed for revert — drop it after a successful verify, or call
/// `revert()` on it to roll back.
#[derive(Debug, Clone, Default)]
pub struct FixAttempt {
    pub iteration: usize,
    pub applied: Vec<AppliedEdit>,
    pub loc_applied: usize,
}

impl FixAttempt {
    /// Restore every file we touched to its pre-image. Best-effort:
    /// keeps going even if one write fails (logs the failure). A
    /// half-reverted workspace is a worse outcome than full-reverted,
    /// even if one file ends up wedged — the user will see the
    /// errors and can inspect.
    pub fn revert(&self) -> Result<()> {
        let mut first_err: Option<anyhow::Error> = None;
        for edit in &self.applied {
            if let Err(e) = std::fs::write(&edit.abs_path, &edit.pre_image) {
                log::error!(
                    "[fix] revert failed for {}: {e}",
                    edit.abs_path.display()
                );
                if first_err.is_none() {
                    first_err = Some(anyhow::Error::from(e).context(format!(
                        "reverting {}",
                        edit.abs_path.display()
                    )));
                }
            } else {
                log::info!("[fix] reverted {}", edit.rel_path);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(())
    }

    /// Files touched (deduped). For re-verify scoping in Phase 3+.
    #[allow(dead_code)]
    pub fn files_touched(&self) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for e in &self.applied {
            if seen.insert(e.rel_path.clone()) {
                out.push(e.rel_path.clone());
            }
        }
        out
    }
}

/// Apply a batch of edits. All-or-nothing: if any single edit fails
/// validation or the unique-old_string guard, every edit already
/// applied in THIS call is reverted before returning. Caller gets
/// either a complete `FixAttempt` or an error — never a partial one.
pub fn apply_edits(
    workspace: &Path,
    iteration: usize,
    edits: &[FindingEdit],
) -> Result<FixAttempt> {
    let mut attempt = FixAttempt {
        iteration,
        applied: Vec::new(),
        loc_applied: 0,
    };

    for edit in edits {
        match try_apply_one(workspace, edit) {
            Ok(applied) => {
                attempt.loc_applied = attempt.loc_applied.saturating_add(applied.loc_delta);
                attempt.applied.push(applied);
            }
            Err(e) => {
                // Roll back whatever we did so far before bubbling.
                if !attempt.applied.is_empty() {
                    log::warn!(
                        "[fix] aborting batch after {} applied edit(s); reverting: {e:#}",
                        attempt.applied.len()
                    );
                    if let Err(re) = attempt.revert() {
                        log::error!("[fix] revert during abort hit an error: {re:#}");
                    }
                }
                return Err(e.context(format!(
                    "applying edit to {} (iteration {iteration})",
                    edit.file
                )));
            }
        }
    }
    Ok(attempt)
}

fn try_apply_one(workspace: &Path, edit: &FindingEdit) -> Result<AppliedEdit> {
    if edit.old_string == edit.new_string {
        anyhow::bail!("old_string == new_string (no-op edit for {})", edit.file);
    }
    if edit.old_string.is_empty() {
        anyhow::bail!("old_string empty for {} (would insert unbounded text)", edit.file);
    }

    let abs_path = resolve_inside_workspace(workspace, &edit.file)?;
    let pre_image = std::fs::read_to_string(&abs_path)
        .with_context(|| format!("reading {}", abs_path.display()))?;

    let matches = count_non_overlapping(&pre_image, &edit.old_string);
    if matches == 0 {
        anyhow::bail!(
            "old_string not found in {} — Opus hallucinated the match, \
             or the file drifted since the screenshot",
            edit.file
        );
    }
    if matches > 1 {
        anyhow::bail!(
            "old_string appears {matches} times in {} — refusing to guess \
             which one; widen old_string with more surrounding lines",
            edit.file
        );
    }

    let post_image = pre_image.replacen(&edit.old_string, &edit.new_string, 1);
    std::fs::write(&abs_path, &post_image)
        .with_context(|| format!("writing {}", abs_path.display()))?;

    let loc_delta = count_loc_delta(&edit.old_string, &edit.new_string);
    log::info!(
        "[fix] applied edit to {} (+{} LoC)",
        edit.file, loc_delta
    );

    Ok(AppliedEdit {
        abs_path,
        rel_path: edit.file.clone(),
        pre_image,
        post_image,
        loc_delta,
    })
}

fn resolve_inside_workspace(workspace: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        anyhow::bail!("empty file path");
    }
    if rel.contains("..") {
        anyhow::bail!("path traversal rejected in `{rel}`");
    }
    let abs = workspace.join(rel);
    // Canonicalize the workspace once, but resolve `abs` without
    // canonicalize() so missing files still give a clear error — we
    // only want to ensure the *logical* prefix is correct.
    let ws_canon = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    if let Ok(abs_canon) = abs.canonicalize() {
        if !abs_canon.starts_with(&ws_canon) {
            anyhow::bail!("{} escapes workspace {}", abs.display(), workspace.display());
        }
    } else {
        // File doesn't exist yet — that's fine for path-escape check
        // (read_to_string will give the real error), but still refuse
        // any rel that doesn't start inside the ws prefix textually.
        if !abs.starts_with(workspace) {
            anyhow::bail!("{} not under workspace {}", abs.display(), workspace.display());
        }
    }
    Ok(abs)
}

fn count_non_overlapping(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(idx) = haystack[start..].find(needle) {
        count += 1;
        start += idx + needle.len();
    }
    count
}

/// Cheap line-delta proxy: sum of lines in `new` not present in `old`
/// plus lines in `old` not present in `new`. Over-reports compared
/// to a real LCS diff, which is fine — we're using it as a blast-
/// radius cap, and over-reporting errs toward conservative.
pub fn count_loc_delta(old: &str, new: &str) -> usize {
    let old_lines: HashSet<&str> = old.lines().collect();
    let new_lines: HashSet<&str> = new.lines().collect();
    let added = new.lines().filter(|l| !old_lines.contains(l)).count();
    let removed = old.lines().filter(|l| !new_lines.contains(l)).count();
    added + removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_ws() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("syntaur-gateway/src/pages");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("dashboard.rs"),
            "fn render() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn apply_and_revert_single_edit() {
        let ws = setup_ws();
        let edits = vec![FindingEdit {
            file: "syntaur-gateway/src/pages/dashboard.rs".into(),
            old_string: "let y = 2;".into(),
            new_string: "let y = 42;".into(),
        }];

        let attempt = apply_edits(ws.path(), 1, &edits).expect("apply");
        assert_eq!(attempt.applied.len(), 1);
        let after =
            fs::read_to_string(ws.path().join("syntaur-gateway/src/pages/dashboard.rs")).unwrap();
        assert!(after.contains("let y = 42;"));

        attempt.revert().expect("revert");
        let restored =
            fs::read_to_string(ws.path().join("syntaur-gateway/src/pages/dashboard.rs")).unwrap();
        assert!(restored.contains("let y = 2;"));
        assert!(!restored.contains("let y = 42;"));
    }

    #[test]
    fn reject_ambiguous_old_string() {
        let ws = setup_ws();
        let edits = vec![FindingEdit {
            file: "syntaur-gateway/src/pages/dashboard.rs".into(),
            // `let ` is not unique — refuses.
            old_string: "let ".into(),
            new_string: "let mut ".into(),
        }];
        let err = apply_edits(ws.path(), 1, &edits).expect_err("should reject");
        assert!(format!("{err:#}").contains("appears"));
    }

    #[test]
    fn reject_path_traversal() {
        let ws = setup_ws();
        let edits = vec![FindingEdit {
            file: "../../etc/passwd".into(),
            old_string: "root".into(),
            new_string: "xxx".into(),
        }];
        let err = apply_edits(ws.path(), 1, &edits).expect_err("should reject");
        assert!(format!("{err:#}").contains("traversal"));
    }

    #[test]
    fn atomic_batch_revert_on_second_edit_failure() {
        let ws = setup_ws();
        let edits = vec![
            FindingEdit {
                file: "syntaur-gateway/src/pages/dashboard.rs".into(),
                old_string: "let y = 2;".into(),
                new_string: "let y = 42;".into(),
            },
            // Second edit fails (old_string missing) — first must be
            // reverted before apply_edits returns.
            FindingEdit {
                file: "syntaur-gateway/src/pages/dashboard.rs".into(),
                old_string: "this string is not in the file".into(),
                new_string: "anything".into(),
            },
        ];
        let err = apply_edits(ws.path(), 1, &edits).expect_err("should fail");
        assert!(format!("{err:#}").contains("not found"));
        let restored =
            fs::read_to_string(ws.path().join("syntaur-gateway/src/pages/dashboard.rs")).unwrap();
        assert!(
            restored.contains("let y = 2;"),
            "first edit was not reverted after second failed"
        );
    }

    #[test]
    fn loc_delta_cheap_count() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        // B replaces b (1 added, 1 removed), d added (1 added) => 3
        assert_eq!(count_loc_delta(old, new), 3);
    }
}
