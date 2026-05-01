//! Autonomous post-build audit for the Syntaur gateway.
//!
//! Phase 1 scope (shipped): scaffold + change→module impact map +
//! chromiumoxide harness that can render a module, screenshot it,
//! and collect the console log. CLI that takes a source change-set
//! and figures out which modules need re-verifying.
//!
//! Later phases (queued as tasks #42–#46):
//!   2. Opus API client + auto-fix loop (safety rails: iteration
//!      budget, blast-radius cap, regression-on-fix check)
//!   3. Persistent baselines + cross-device viewports + regression
//!      corpus that grows from caught bugs
//!   4. Persona POV coverage (all 9) + user-flow YAML interpreter
//!   5. Security co-sweep + diff narrative + fix-log ledger
//!   6. syntaur-ship pipeline integration (verify stage between
//!      build and Mac Mini rsync, auto-fix applies before canary)
//!
//! Design invariants preserved from Phase 1 so later phases slot
//! in cleanly:
//!   - Every verified surface produces a `Finding` (structured;
//!     auto-fix later just reads the same shape).
//!   - Screenshots are written to a run-scoped dir that the
//!     baseline system (Phase 3) will consume unmodified.
//!   - Module identity is a slug + URL — never a file path — so
//!     re-routing the gateway doesn't break the catalog.

pub mod baseline;
pub mod browser;
pub mod cache_control;
pub mod changeset;
pub mod corpus;
pub mod fix;
pub mod flow;
pub mod module_map;
pub mod opus;
pub mod persona;
pub mod persona_identity;
pub mod persona_tone;
pub mod run;
pub mod visual_diff;

pub use baseline::BaselineStore;
pub use browser::{Browser, PageCapture, Viewport};
pub use changeset::{ChangeSet, resolve_against};
pub use corpus::{Corpus, CorpusEntry, CorpusMeta};
pub use fix::{apply_edits, count_loc_delta, AppliedEdit, Budgets, FixAttempt};
pub use flow::{run_flow, FlowFile, FlowRunOutcome, Step};
pub use module_map::{Module, ModuleMap};
pub use opus::OpusClient;
pub use persona::{AuthSource, Persona, PersonaCatalog};
pub use run::{Finding, FindingEdit, FindingKind, Severity, VerifyRun};
pub use visual_diff::{diff_pngs, DiffResult};
