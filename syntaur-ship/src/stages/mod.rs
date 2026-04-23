//! Stage modules. Each exports a `pub fn run(ctx: &StageContext) -> Result<()>`.
//!
//! Phase 1 ships the core pipeline stages from deploy.sh. Subsequent
//! phases add additional modules (snapshot, canary, version_audit,
//! win11, journal). The pipeline.rs orchestrator decides which stages
//! run in which order.

pub mod build;
pub mod git_push;
pub mod mac_mini;
pub mod preflight;
pub mod truenas;
pub mod viewer;
