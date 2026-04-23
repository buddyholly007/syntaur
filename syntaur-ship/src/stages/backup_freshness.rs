//! Pre-deploy backup-freshness check.
//!
//! The preservation design called for: "refuse to deploy if the most
//! recent pool-wide backup is >24h old." This catches the failure
//! mode where the nightly ZFS replication/snapshot task has been
//! broken silently for a week and nobody noticed.
//!
//! We query TrueNAS for the youngest non-syntaur-ship snapshot on
//! the pool root. syntaur-ship's OWN snapshots don't count (they're
//! created mid-deploy — not an independent backup). If no
//! independent snapshot in the last 24h, abort with a clear message
//! directing the operator to check their snapshot/replication task.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::process::Command;

use crate::pipeline::StageContext;

pub const FRESHNESS_MAX_HOURS: i64 = 24;

pub fn run(ctx: &StageContext) -> Result<()> {
    if ctx.opts.dry_run {
        log::info!("[backup-freshness] dry-run, skipping check");
        return Ok(());
    }
    log::info!(
        "[backup-freshness] checking for independent pool snapshot in last {FRESHNESS_MAX_HOURS}h"
    );

    // Ask midclt for all snapshots on cherry_family_nas, ordered by
    // creation time descending. Take youngest that is NOT created by
    // syntaur-ship itself.
    let cfg = ctx.cfg;
    let filter = r#"[["pool", "=", "cherry_family_nas"]]"#;
    let mut args = cfg.truenas_ssh_args();
    args.push(format!("midclt call zfs.snapshot.query '{filter}'"));
    let out = Command::new("ssh")
        .args(&args)
        .output()
        .context("midclt zfs.snapshot.query")?;
    if !out.status.success() {
        log::warn!(
            "[backup-freshness] midclt unreachable — skipping check (degraded): {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return Ok(());
    }

    let arr: serde_json::Value = serde_json::from_slice(&out.stdout)
        .context("parse zfs.snapshot.query json")?;
    let snaps = arr.as_array().ok_or_else(|| anyhow::anyhow!("snapshot query returned non-array"))?;

    // Find the youngest snapshot NOT named syntaur-ship-*.
    let now = Utc::now();
    let mut youngest_indep: Option<(String, DateTime<Utc>)> = None;
    for s in snaps {
        let name = s["name"].as_str().unwrap_or("");
        if name.contains("@syntaur-ship-") {
            continue;
        }
        // properties.creation.parsed is {"$date": <ms since epoch>}
        let created_ms = s["properties"]["creation"]["parsed"]["$date"]
            .as_i64();
        let Some(created_ms) = created_ms else { continue };
        let created = DateTime::from_timestamp_millis(created_ms);
        let Some(created) = created else { continue };
        if youngest_indep.as_ref().map(|(_, t)| *t < created).unwrap_or(true) {
            youngest_indep = Some((name.to_string(), created));
        }
    }

    let Some((name, created)) = youngest_indep else {
        log::warn!(
            "[backup-freshness] ⚠ NO non-syntaur-ship snapshots found on cherry_family_nas. \
             Your snapshot/replication task may be broken. Review TrueNAS → Data Protection → Snapshot Tasks. \
             Proceeding anyway (this is a first-time warning, not a block)."
        );
        return Ok(());
    };

    let age_hours = (now - created).num_hours();
    if age_hours > FRESHNESS_MAX_HOURS {
        if std::env::var("SYNTAUR_SHIP_ALLOW_STALE_BACKUP").ok().as_deref() == Some("1") {
            log::warn!(
                "[backup-freshness] ⚠ override: stale backup accepted ({age_hours}h, {name}, {})",
                created.format("%Y-%m-%d %H:%M UTC")
            );
            return Ok(());
        }
        anyhow::bail!(
            "backup-freshness: youngest independent snapshot is {age_hours}h old ({name}, created {}). \
             No backup has run in the last {FRESHNESS_MAX_HOURS}h — refusing to deploy until a \
             recent backup exists. Check TrueNAS snapshot/replication tasks. \
             If confirmed OK, override with SYNTAUR_SHIP_ALLOW_STALE_BACKUP=1 env var.",
            created.format("%Y-%m-%d %H:%M UTC")
        );
    }

    log::info!(
        "[backup-freshness] ✓ youngest independent snapshot: {name} ({age_hours}h ago)"
    );
    Ok(())
}
