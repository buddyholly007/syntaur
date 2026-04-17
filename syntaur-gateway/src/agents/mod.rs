//! Default persona configurations shipped with Syntaur.
//!
//! Syntaur ships eight canonical personas — one main agent + seven module
//! specialists — plus Peter for Sean's local-only personal deployment.
//! Metadata and system prompt templates live in `defaults.rs` and are
//! upserted into `module_agent_defaults` on gateway startup.
//!
//! The full design rationale (inspirations, tone dials, memory sharing,
//! escalation rules) lives in vault/projects/syntaur_personas.md.

pub mod defaults;
pub mod escalation;
pub mod handoff;
pub mod tasks;
pub mod context_budget;
pub mod templates;
pub mod import;
