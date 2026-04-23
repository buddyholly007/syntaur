//! Stage modules. Each exports a `pub fn run(...)` that Pipeline invokes.
//!
//! Phase 2 additions: snapshot (ZFS pool-wide before deploy).

pub mod build;
pub mod git_push;
pub mod mac_mini;
pub mod preflight;
pub mod snapshot;
pub mod truenas;
pub mod viewer;
