//! Run orchestration + Finding data model.
//!
//! A VerifyRun is one invocation of the tool — covers N modules,
//! each producing zero or more Findings. Findings have a severity
//! (`regression` fails the run; `suggestion` is advisory). Phase 2
//! auto-fix consumes this same shape.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Hard fail — the build shouldn't ship until this is fixed.
    Regression,
    /// Advisory — improvement suggestion. Auto-fix eligible per
    /// Phase 2 policy but doesn't fail the run on its own.
    Suggestion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    /// Expected module URL didn't reach 200 / timed out.
    BootFailure,
    /// Console error during page load.
    ConsoleError,
    /// Visual difference against baseline beyond threshold.
    VisualDiff,
    /// Accessibility/contrast/a11y heuristic violation.
    Accessibility,
    /// UX improvement opportunity (Opus-identified in Phase 2).
    Improvement,
    /// Security invariant violation (Phase 5).
    Security,
    /// Catch-all for bespoke checks.
    Other,
}

/// A precise source-code edit suggested by Opus, consumable by the
/// Phase 2b auto-fix loop. Shape deliberately mirrors Claude Code's
/// `Edit` tool — `old_string` must match the file EXACTLY and be
/// unique, preventing silent multi-replace. File paths are relative
/// to the workspace root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingEdit {
    pub file: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub module_slug: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Path to a supporting artifact (screenshot, diff image, log).
    pub artifact: Option<PathBuf>,
    pub captured_at: DateTime<Utc>,
    /// Structured edits Opus proposes for auto-fix (Phase 2b).
    /// Empty/None for heuristic-only findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<FindingEdit>>,
    /// Phase 4b — persona slug whose session was active when the
    /// finding was captured. `None` for anonymous / default-session
    /// runs, which is the only shape pre-Phase-4b reports had, so
    /// `#[serde(default)]` keeps old `report.json` blobs parseable.
    /// `skip_serializing_if = "Option::is_none"` keeps new runs that
    /// don't use personas looking identical to old runs on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyRun {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Git revision the run was verifying against (deploy-stamp head).
    pub against_rev: String,
    /// Git HEAD being verified.
    pub head_rev: String,
    /// Working-tree paths the run analysed.
    pub changed_paths: Vec<String>,
    /// Module slugs the run actually visited.
    pub modules_covered: Vec<String>,
    /// All findings from this run.
    pub findings: Vec<Finding>,
    /// Directory under ~/.syntaur-verify/runs/<run_id>/ containing
    /// every screenshot + artifact for this run.
    pub run_dir: PathBuf,
}

impl VerifyRun {
    pub fn regressions(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Regression)
            .count()
    }

    pub fn suggestions(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Suggestion)
            .count()
    }

    pub fn is_clean(&self) -> bool {
        self.regressions() == 0
    }
}
