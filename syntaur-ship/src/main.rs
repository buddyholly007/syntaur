//! `syntaur-ship` — the canonical deploy pipeline for Syntaur.
//!
//! Replaces the bash `~/deploy.sh` with a Rust implementation that:
//!
//! - Enforces a single ordered pipeline (claudevm → Mac Mini → GitHub →
//!   TrueNAS → viewer relaunch). Every stage is either a `--dry-run`
//!   no-op or a real run, never partial.
//! - Snapshots the TrueNAS pool before any writes, retains 20 snapshots
//!   + last 3 `.prev` copies of every binary (Phase 2).
//! - Verifies version consistency across all public surfaces (Phase 3).
//! - Coordinates with other Claude Code sessions via the claude-coord
//!   broker (Phase 5).
//! - Auto-rolls-back on prod `/health` failure (Phase 2).
//!
//! Phase 1 (this file): scaffold + functional parity with deploy.sh.
//! Each subsequent phase adds a stage without breaking the pipeline.
//!
//! Typical invocation:
//!
//!     syntaur-ship                         # full pipeline, no flags = no shortcuts
//!     syntaur-ship --dry-run               # show plan, touch nothing
//!     syntaur-ship status                  # prod + Mac Mini + Win11 VM + stamp state
//!     syntaur-ship rollback                # revert TrueNAS to .prev-<latest>
//!     syntaur-ship release v0.5.1          # full release flow (Phase 6)
//!
//! The vault docs — projects/syntaur_release_story.md + feedback/
//! syntaur_deploy_pipeline.md — are the canonical policy this tool
//! operationalizes. If the tool ever diverges from those docs, the docs
//! win; fix the tool.

use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod ci_audit;
mod config;
mod coord;
mod guards;
mod journal;
mod pipeline;
mod release;
mod stages;
mod stamp_sign;
mod state;

#[derive(Parser, Debug)]
#[command(
    name = "syntaur-ship",
    version,
    about = "Canonical Syntaur deploy pipeline",
    long_about = None,
)]
struct Cli {
    /// Show the plan without executing any commands.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Deploy only `rust-social-manager`, leaving the gateway running.
    /// This is a partial-deploy scope, NOT a quality bypass — every
    /// stage that runs still runs in full.
    #[arg(long, global = true)]
    social_only: bool,

    /// Let the verify stage attempt an Opus-driven auto-fix if it
    /// catches a regression. Capped at 2 iters + 150 LoC/module. When
    /// off (default), regressions just abort the pipeline and the
    /// operator fixes manually.
    #[arg(long, global = true)]
    auto_fix: bool,

    // NOTE: --skip-build / --skip-mac / --skip-git / --skip-verify /
    // --force-ci-drift were removed in v0.6.5. Those flags existed
    // as "emergency overrides" but every prior emergency turned out
    // to be either a bug we should have fixed inline OR a transient
    // we should have retried. Cumulative cost over v0.5.x: a 90-min
    // hardware-debug session caused by --skip-build hiding a stale
    // binary, three weeks of broken smart-home masked by --skip-verify
    // bypassing the visual audit, and four publication gaps caused
    // by --force-ci-drift letting the pipeline declare success past
    // failing CI. Every "emergency" flag was a security regression
    // we shipped to ourselves. The pipeline is now fix-or-block.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show current state across all hosts (prod / Mac Mini / Win11 /
    /// stamp / GitHub Releases).
    Status,
    /// Dry-run verification; same as `--dry-run` with no subcommand.
    Check,
    /// Revert TrueNAS to the previous binary (.prev-<latest>) and
    /// restart the container. Phase 2 feature.
    Rollback {
        /// Use a ZFS snapshot restore instead of binary-only rollback.
        /// Restores user data — requires confirmation prompt.
        #[arg(long)]
        zfs: Option<String>,
    },
    /// Full 6-step release flow — edit VERSION → sync → commit → tag →
    /// push → CI → deploy. Phase 6.
    Release {
        version: String,
    },
    /// Push a fresh Windows binary into the Win11 VM. Phase 6.
    RefreshWindows,
    /// Audit version strings across every public surface. Phase 3.
    VersionSweep,
    /// List available recovery points (ZFS snapshots + .prev binaries).
    SnapshotList,
    /// Show recent deploy records from the journal.
    Journal {
        #[arg(long, default_value = "10")]
        last: usize,
    },
    /// Verify the cosign signature chain on a deploy stamp. Phase 7.
    VerifyStamp {
        stamp_path: Option<String>,
    },
    /// Install git pre-commit + pre-push hooks in the workspace. Phase 8.
    HooksInstall,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_secs()
    .init();

    let cli = Cli::parse();
    let cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(2);
        }
    };

    let run_opts = pipeline::RunOptions {
        dry_run: cli.dry_run,
        social_only: cli.social_only,
        auto_fix: cli.auto_fix,
    };

    let result = match cli.command {
        None => pipeline::run_full(&cfg, &run_opts),
        Some(Command::Status) => pipeline::run_status(&cfg),
        Some(Command::Check) => pipeline::run_full(
            &cfg,
            &pipeline::RunOptions { dry_run: true, ..run_opts },
        ),
        Some(Command::Rollback { zfs }) => pipeline::run_rollback(&cfg, zfs.as_deref()),
        Some(Command::Release { version }) => pipeline::run_release(&cfg, &version),
        Some(Command::RefreshWindows) => pipeline::run_refresh_windows(&cfg),
        Some(Command::VersionSweep) => pipeline::run_version_sweep(&cfg),
        Some(Command::SnapshotList) => pipeline::run_snapshot_list(&cfg),
        Some(Command::Journal { last }) => pipeline::run_journal(&cfg, last),
        Some(Command::VerifyStamp { stamp_path }) => {
            pipeline::run_verify_stamp(&cfg, stamp_path.as_deref())
        }
        Some(Command::HooksInstall) => pipeline::run_hooks_install(&cfg),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("syntaur-ship failed: {e:#}");
            ExitCode::from(1)
        }
    }
}
