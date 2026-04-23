//! Stage modules. Each exports a `pub fn run(ctx: &StageContext)`.

pub mod build;
pub mod git_push;
pub mod mac_mini;
pub mod preflight;
pub mod snapshot;
pub mod truenas;
pub mod version_audit;
pub mod version_sweep;
pub mod viewer;
pub mod win11;
