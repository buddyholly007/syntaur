//! Deep research orchestrator (Phase 5).
//!
//! Workflow: Plan → Orchestrate (parallel subtasks) → Report.
//!
//! The orchestrator is plain Rust workflow code, not an LLM-driven loop.
//! Each phase makes a discrete LLM call:
//!   1. **Plan**: one LLM call to produce a JSON list of subtasks
//!   2. **Orchestrate**: spawns one tokio task per subtask, bounded by
//!      `Semaphore(3)`. Each subtask is an isolated mini agent loop with
//!      a restricted tool set (internal_search, web_search, web_fetch,
//!      code_execute) and an 8-round budget.
//!   3. **Report**: one LLM call to synthesize collected evidence into a
//!      final cited markdown answer.
//!
//! Patterns ported from Onyx's `orchestration_layer.py` (the policies,
//! not the Python code):
//!   * Plan and execution are isolated
//!   * Subtasks are context-isolated — each only sees its own task text
//!   * Maximum 3 parallel subtasks
//!   * Maximum 6 plan steps
//!   * Hard wall-clock cap on the whole session
//!
//! v1 has no clarification phase (assumes the user provides a detailed
//! query) and no persistence (evidence lives in memory). Both are
//! deferred to later phases.

mod agent;
mod clarify;
mod evidence;
mod events;
mod orchestrate;
mod prompts;
mod store;

pub use evidence::{EvidenceItem, Plan, PlanStep, ResearchReport};
pub use orchestrate::{run_research, validate_query, ResearchRequest};
pub use store::SessionStore;
pub use events::ResearchEvent;
pub use clarify::{run_clarify, ClarifyResult};
