//! Post-deploy version audit — hit the live prod container + public
//! GitHub Releases + install.sh CDN and confirm all the user-visible
//! version surfaces agree with what we just deployed.
//!
//! Runs AFTER TrueNAS docker-restart + /health OK. If any surface
//! disagrees, logs a WARNING (doesn't fail the deploy — prod is
//! already live at this point). Future Phase 4 will append the audit
//! result to the deploy journal for historical tracking.
//!
//! Surfaces:
//!   1. GET /health                     (version field — what prod actually identifies as)
//!   2. GET /                           (HTML <!-- VERSION-BADGE -->vX.Y.Z or landing embed)
//!   3. GET github.com/.../releases/latest  (tag name via GH API)
//!   4. GET <install.sh URL from README>    (VERSION= line in the published shell)
//!   5. Phase 3b: GET /api/version-proof (structured version+commit+sha)

use anyhow::Result;
use serde_json::Value;
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    let cfg = ctx.cfg;
    if ctx.opts.dry_run {
        log::info!("[version-audit] dry-run, skipping live surface hits");
        return Ok(());
    }
    log::info!("[version-audit] checking public surfaces on live prod");

    let expected = read_file_string(&cfg.workspace.join("VERSION"))?;
    let expected = expected.trim();

    let mut results: Vec<(String, Result<String>)> = Vec::new();

    results.push(("prod /health version".into(), health_version(&cfg.health_url)));
    results.push(("prod landing VERSION-BADGE".into(), landing_badge(cfg)));
    results.push(("GitHub Releases latest tag".into(), github_latest_tag()));
    results.push(("install.sh raw (from GH main)".into(), install_sh_version()));

    let mut mismatches = Vec::new();
    for (name, got) in &results {
        match got {
            Ok(v) if v == expected => log::info!("  {name:<32} = {v} ✓"),
            Ok(v) => {
                log::warn!("  {name:<32} = {v} (expected {expected})");
                mismatches.push(format!("{name}: got {v}, expected {expected}"));
            }
            Err(e) => log::warn!("  {name:<32} probe failed: {e}"),
        }
    }

    if !mismatches.is_empty() {
        log::warn!(
            "[version-audit] ⚠ {} surface(s) drift from v{expected} — deploy already landed, fix at source + redeploy:\n  {}",
            mismatches.len(),
            mismatches.join("\n  ")
        );
    } else {
        log::info!("[version-audit] ✓ all user-visible surfaces report v{expected}");
    }
    Ok(())
}

fn read_file_string(p: &std::path::Path) -> Result<String> {
    Ok(std::fs::read_to_string(p)?)
}

fn health_version(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args(["-sf", "--max-time", "5", url])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("curl {url}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    Ok(v["version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no .version in /health"))?
        .to_string())
}

fn landing_badge(cfg: &crate::config::Config) -> Result<String> {
    // Prod's root responds with the gateway dashboard or landing — the
    // landing is typically served at the marketing site, not prod. For
    // our internal prod we check the prod dashboard HTML for the
    // VERSION-BADGE comment marker, which the gateway embeds if the
    // landing assets are bundled. If the marker isn't there, report
    // the finding rather than fail.
    let out = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "5",
            &cfg.health_url.replace("/health", "/"),
        ])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("curl prod /");
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let marker = "<!-- VERSION-BADGE -->v";
    let end = "<!-- /VERSION-BADGE -->";
    let Some(i) = body.find(marker) else {
        anyhow::bail!("VERSION-BADGE marker not in prod / response");
    };
    let after = &body[i + marker.len()..];
    let Some(j) = after.find(end) else {
        anyhow::bail!("VERSION-BADGE close marker missing");
    };
    Ok(after[..j].trim().to_string())
}

fn github_latest_tag() -> Result<String> {
    let out = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            "https://api.github.com/repos/buddyholly007/syntaur/releases/latest",
        ])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("gh api /releases/latest");
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no tag_name in gh api response"))?;
    Ok(tag.trim_start_matches('v').to_string())
}

fn install_sh_version() -> Result<String> {
    // Fetch the install.sh that users actually curl, check its VERSION=.
    let out = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            "https://raw.githubusercontent.com/buddyholly007/syntaur/main/install.sh",
        ])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("fetch raw install.sh");
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let needle = r#"VERSION=""#;
    let Some(i) = body.find(needle) else {
        anyhow::bail!("install.sh VERSION= not found");
    };
    let after = &body[i + needle.len()..];
    let end = after
        .find('"')
        .ok_or_else(|| anyhow::anyhow!("install.sh VERSION close quote"))?;
    Ok(after[..end].to_string())
}
