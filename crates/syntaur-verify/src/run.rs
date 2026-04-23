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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
