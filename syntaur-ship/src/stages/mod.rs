//! Stage modules. Each exports a `pub fn run(ctx: &StageContext)`.
//!
//! Phase 2 additions: snapshot (ZFS preservation).
//! Phase 3a additions: version_sweep (pre-flight, aborts on drift)
//!                     + version_audit (post-deploy, warns on drift).

pub mod build;
pub mod git_push;
pub mod mac_mini;
pub mod preflight;
pub mod snapshot;
pub mod truenas;
pub mod version_audit;
pub mod version_sweep;
pub mod viewer;
