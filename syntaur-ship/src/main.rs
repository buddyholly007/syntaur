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

mod config;
mod coord;
mod guards;
mod journal;
mod pipeline;
mod stages;
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

    /// Reuse the existing target/release binary instead of rebuilding.
    /// Emergency flag — only safe when the binary was just built and
    /// nothing changed since. See feedback/never_skip_pipeline_steps.
    #[arg(long, global = true)]
    skip_build: bool,

    /// Skip the Mac Mini smoke step. Emergency flag only; disables the
    /// sole guard that catches HSTS-on-HTTP regressions before prod.
    /// Defaults to false.
    #[arg(long, global = true)]
    skip_mac: bool,

    /// Skip the `git push` step. Emergency flag only; use when a
    /// concurrent session has uncommitted drift. Not a substitute for
    /// --skip-mac (the smoke is independent of git state).
    #[arg(long, global = true)]
    skip_git: bool,

    /// Deploy only `rust-social-manager`, leaving the gateway running.
    #[arg(long, global = true)]
    social_only: bool,

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
        skip_build: cli.skip_build,
        skip_mac: cli.skip_mac,
        skip_git: cli.skip_git,
        social_only: cli.social_only,
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
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("syntaur-ship failed: {e:#}");
            ExitCode::from(1)
        }
    }
}
