//! TrueNAS ZFS snapshot stage.
//!
//! Creates a recursive pool-wide snapshot of `cherry_family_nas` before
//! any rsync/binary swap occurs. If any later stage fails, or if the
//! user runs `syntaur-ship rollback --zfs <name>`, we can restore the
//! entire pool state (including user data — photos, documents, ledger,
//! etc.) by rolling back to this snapshot.
//!
//! Retention: keep last 20 `syntaur-ship-pre-deploy-*` snapshots,
//! prune the rest. 20 × (pool-delta-per-deploy) is small because ZFS
//! snapshots are copy-on-write — disk usage grows only with data
//! that actually changes between deploys.
//!
//! Uses TrueNAS's `midclt call zfs.snapshot.{create,query,delete}`
//! JSON-RPC interface — no sudo required, no /sbin/zfs shelling,
//! runs as the standard truenas_admin user.

use anyhow::{Context, Result};
use chrono::Utc;
use std::process::Command;

use crate::pipeline::StageContext;

pub const POOL: &str = "cherry_family_nas";
pub const SNAPSHOT_PREFIX: &str = "syntaur-ship-pre-deploy-";
pub const RETAIN_SNAPSHOTS: usize = 20;

pub fn run(ctx: &StageContext) -> Result<String> {
    let name = format!(
        "{}{}",
        SNAPSHOT_PREFIX,
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let full = format!("{POOL}@{name}");

    log::info!(">> ZFS recursive snapshot {full} (TrueNAS preservation)");
    if ctx.opts.dry_run {
        return Ok(full);
    }

    // zfs.snapshot.create signature per TrueNAS API:
    //   { "dataset": "cherry_family_nas",
    //     "name": "<snapshot-name>",
    //     "recursive": true,
    //     "suspend_vms": false,
    //     "vmware_sync": false }
    let payload = format!(
        r#"{{"dataset":"{POOL}","name":"{name}","recursive":true,"suspend_vms":false,"vmware_sync":false}}"#
    );
    let args = ssh_args(ctx, "zfs.snapshot.create", &payload);
    let output = Command::new("ssh")
        .args(&args)
        .output()
        .context("midclt zfs.snapshot.create")?;
    if !output.status.success() {
        anyhow::bail!(
            "zfs.snapshot.create {full} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    log::info!("   ✓ {full}");
    prune_old_snapshots(ctx)?;
    Ok(full)
}

pub fn list(ctx: &StageContext) -> Result<Vec<String>> {
    let filter = format!(
        r#"[["pool", "=", "{POOL}"], ["name", "^", "{POOL}@{SNAPSHOT_PREFIX}"]]"#
    );
    let args = ssh_args(ctx, "zfs.snapshot.query", &filter);
    let out = Command::new("ssh").args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "zfs.snapshot.query: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let arr: serde_json::Value = serde_json::from_str(&text)
        .context("parse zfs.snapshot.query json")?;
    let mut names: Vec<String> = arr
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v["name"].as_str().map(str::to_string))
        .filter(|n| n.starts_with(&format!("{POOL}@{SNAPSHOT_PREFIX}")))
        .collect();
    names.sort();
    Ok(names)
}

fn prune_old_snapshots(ctx: &StageContext) -> Result<()> {
    let names = list(ctx)?;
    if names.len() <= RETAIN_SNAPSHOTS {
        return Ok(());
    }
    let to_delete = &names[..names.len() - RETAIN_SNAPSHOTS];
    for snap in to_delete {
        log::info!("   prune old snapshot: {snap}");
        // zfs.snapshot.delete takes [id, options]. id is full "pool@name".
        let payload = format!(r#"{snap:?}, {{"recursive":true}}"#);
        let args = ssh_args(ctx, "zfs.snapshot.delete", &payload);
        let status = Command::new("ssh").args(&args).status();
        if let Err(e) = status {
            log::warn!("   prune failed for {snap}: {e}");
        }
    }
    Ok(())
}

pub fn rollback(ctx: &StageContext, snapshot: &str) -> Result<()> {
    let full = if snapshot.contains('@') {
        snapshot.to_string()
    } else {
        format!("{POOL}@{snapshot}")
    };
    log::warn!(
        "⚠ ZFS rollback: {full} — this restores ALL user data to pre-deploy state. Irreversible."
    );
    if ctx.opts.dry_run {
        log::info!("   (dry-run, skipped)");
        return Ok(());
    }
    let payload = format!(r#"{full:?}, {{"recursive":true,"force":true}}"#);
    let args = ssh_args(ctx, "zfs.snapshot.rollback", &payload);
    let output = Command::new("ssh").args(&args).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "rollback {full}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    log::info!("   ✓ pool rolled back to {full}");
    Ok(())
}

fn ssh_args(ctx: &StageContext, method: &str, payload_json: &str) -> Vec<String> {
    let cfg = ctx.cfg;
    vec![
        "-J".into(),
        cfg.truenas_jump.clone(),
        format!("{}@{}", cfg.truenas_user, cfg.truenas_ip),
        format!("midclt call {method} '{payload_json}'"),
    ]
}
