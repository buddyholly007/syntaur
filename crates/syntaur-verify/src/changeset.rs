//! Resolve "what changed since the last successful deploy" via git.
//!
//! Inputs: workspace dir + a git revision to diff against (typically
//! the `git_head` from `~/.syntaur/ship/deploy-stamp.json`).
//! Output: a list of modified/added paths relative to workspace root.
//!
//! Intentionally shells out to `git` rather than using libgit2 — the
//! workspace is already a working copy of a real repo, and `git
//! diff --name-only` is three lines of shell with predictable
//! semantics.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct ChangeSet {
    /// Revision we diff'd against (base).
    pub against: String,
    /// Revision or worktree state on the other side.
    pub head: String,
    /// Paths changed between base and head. Relative to workspace root.
    pub paths: Vec<String>,
}

/// Read the deploy stamp and return its `git_head`. Used as the
/// default `against` when the CLI isn't told otherwise.
pub fn deploy_stamp_head(stamp_path: &Path) -> Result<String> {
    let body = std::fs::read_to_string(stamp_path)
        .with_context(|| format!("reading deploy stamp {}", stamp_path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&body).context("parsing deploy stamp JSON")?;
    let head = v
        .get("git_head")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("deploy stamp missing git_head"))?;
    Ok(head.to_string())
}

pub fn resolve_against(workspace: &Path, against: &str) -> Result<ChangeSet> {
    // HEAD of the current working tree (including uncommitted
    // changes captured as "HEAD"; we diff against the stamp commit
    // to catch in-flight edits too).
    let head = run_git(workspace, &["rev-parse", "HEAD"])?;

    // Show both committed AND uncommitted changes since `against`.
    // The `diff $against` (no `--cached`) includes worktree state.
    let out = run_git(workspace, &["diff", "--name-only", against])?;
    let committed_and_unstaged: Vec<String> = out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Also include UNTRACKED files (new files not yet `git add`-ed).
    // `ls-files --others --exclude-standard` is the canonical way.
    let out2 = run_git(workspace, &["ls-files", "--others", "--exclude-standard"])?;
    let untracked: Vec<String> = out2
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut all: Vec<String> = committed_and_unstaged;
    all.extend(untracked);
    all.sort();
    all.dedup();

    Ok(ChangeSet {
        against: against.to_string(),
        head: head.trim().to_string(),
        paths: all,
    })
}

fn run_git(workspace: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
