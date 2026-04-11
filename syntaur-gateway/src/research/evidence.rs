//! In-memory evidence store + plan/result types for deep research.
//!
//! v1 keeps evidence in memory per research session — no persistence.
//! Each subtask produces an `EvidenceItem`; the orchestrator collects them
//! all before handing off to the report phase.

use serde::{Deserialize, Serialize};

use crate::tools::extension::Citation;

/// One step in the research plan, produced by the planner LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub plan: Vec<PlanStep>,
}

impl Plan {
    pub fn len(&self) -> usize {
        self.plan.len()
    }
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }
    pub fn truncate_to_max(&mut self, max: usize) {
        self.plan.truncate(max);
    }
}

/// One evidence item produced by a research subtask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// 1-indexed step number from the plan.
    pub step_index: usize,
    /// The original task description (for traceability).
    pub task: String,
    /// Compact text answer produced by the subtask agent.
    pub summary: String,
    /// Citations bubbled up from internal_search tool calls during the subtask.
    pub citations: Vec<Citation>,
    /// Names of tools the subtask actually called.
    pub tools_used: Vec<String>,
    /// Wall-clock duration of the subtask execution.
    pub duration_ms: u64,
    /// LLM rounds the subtask consumed.
    pub rounds_used: usize,
    /// Set if the subtask failed; summary will be empty.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceStore {
    pub items: Vec<EvidenceItem>,
}

impl EvidenceStore {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, item: EvidenceItem) {
        self.items.push(item);
    }

    /// Sort items by step_index so the report sees them in plan order
    /// regardless of the order they completed in.
    pub fn sort_by_step(&mut self) {
        self.items.sort_by_key(|i| i.step_index);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Return a deduplicated list of (source, external_id, title) tuples
    /// across all evidence items, in first-seen order. Used by the report
    /// renderer to attach a cumulative Sources section.
    pub fn cumulative_citations(&self) -> Vec<(String, String, String)> {
        use std::collections::HashSet;
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut out: Vec<(String, String, String)> = Vec::new();
        for item in &self.items {
            for c in &item.citations {
                let key = (c.source.clone(), c.external_id.clone());
                if seen.insert(key) {
                    out.push((c.source.clone(), c.external_id.clone(), c.title.clone()));
                }
            }
        }
        out
    }

    /// Render the evidence as a numbered markdown list for the report LLM.
    /// Citations are flattened so the LLM sees them with the same numbers
    /// it should use in the final report. A "Cumulative sources"
    /// section is appended at the end with deduplicated citations across
    /// all subtasks.
    pub fn render_for_report(&self) -> String {
        let mut out = String::new();
        for item in &self.items {
            out.push_str(&format!(
                "## Evidence [{}] — {}\n",
                item.step_index, item.task
            ));
            out.push_str(&format!(
                "_tools used: {} • duration: {}ms • rounds: {}_\n\n",
                if item.tools_used.is_empty() {
                    "(none)".to_string()
                } else {
                    item.tools_used.join(", ")
                },
                item.duration_ms,
                item.rounds_used
            ));
            // ALWAYS include the summary if non-empty, even when an error
            // occurred. The error field is metadata about HOW the subtask ran
            // (LLM provider failed, round budget hit, etc.) — it does not
            // invalidate the summary text. The reporter prompt is told to use
            // summaries regardless of error state.
            if !item.summary.trim().is_empty() {
                out.push_str(&item.summary);
                out.push_str("\n\n");
            } else if let Some(err) = &item.error {
                out.push_str(&format!(
                    "_(no summary produced; subtask error: {})_\n\n",
                    err
                ));
            } else {
                out.push_str("_(no summary produced)_\n\n");
            }
            // Surface the error as metadata only if we DID have a summary, so
            // the reporter knows the subtask was partial.
            if let Some(err) = &item.error {
                if !item.summary.trim().is_empty() {
                    out.push_str(&format!("_(note: subtask was partial — {})_\n\n", err));
                }
            }
            if !item.citations.is_empty() {
                out.push_str("Citations:\n");
                for (i, c) in item.citations.iter().enumerate() {
                    out.push_str(&format!(
                        "  - [{}.{}] {} ({})\n",
                        item.step_index,
                        i + 1,
                        c.title,
                        c.source
                    ));
                }
                out.push('\n');
            }
        }
        // Cumulative deduplicated sources for the report agent
        let cumulative = self.cumulative_citations();
        if !cumulative.is_empty() {
            out.push_str("## Cumulative sources (deduplicated)\n");
            for (i, (source, external_id, title)) in cumulative.iter().enumerate() {
                out.push_str(&format!(
                    "  [s{}] {} — {} ({})\n",
                    i + 1,
                    title,
                    external_id,
                    source
                ));
            }
            out.push('\n');
        }
        out
    }
}

impl Default for EvidenceStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Final research report returned to the API caller.
#[derive(Debug, Clone, Serialize)]
pub struct ResearchReport {
    /// The user's original query, echoed for traceability.
    pub query: String,
    /// The plan that was executed.
    pub plan: Vec<PlanStep>,
    /// All evidence items in plan order.
    pub evidence: Vec<EvidenceItem>,
    /// Final synthesized markdown answer from the report phase.
    pub report: String,
    /// Total wall-clock duration including all phases.
    pub total_duration_ms: u64,
    /// Set if any phase failed irrecoverably; report may still contain partial output.
    pub error: Option<String>,
}
