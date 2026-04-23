//! Pre-flight version sweep — verify the 5 public version surfaces
//! agree with `/VERSION` BEFORE we build + deploy. The repo-level
//! `version-check.yml` CI workflow already catches drift on PR, but
//! CI runs AFTER push. Running the same check locally before build
//! catches drift at authoring time — avoids a failed CI + forced
//! re-push.
//!
//! Addresses the security audit finding that version strings across
//! user-facing surfaces drift from the actual binary version.
//!
//! Surfaces checked:
//!   1. /VERSION              (authoritative — single source of truth)
//!   2. Cargo.toml [workspace.package] version
//!   3. install.sh   VERSION="..."
//!   4. install.ps1  $Version = "..."
//!   5. landing/index.html  <!-- VERSION-BADGE -->vX.Y.Z<!-- /VERSION-BADGE -->
//!
//! If any disagree, abort the deploy with a concrete message +
//! suggest running `scripts/sync-version.sh` to repair.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::pipeline::StageContext;

#[derive(Debug)]
struct Surface {
    name: &'static str,
    path: PathBuf,
    version: String,
}

pub fn run(ctx: &StageContext) -> Result<()> {
    let ws = &ctx.cfg.workspace;
    log::info!("[version-sweep] checking 5 public version surfaces against /VERSION");
    let surfaces = collect(ws)?;

    // /VERSION is the reference.
    let reference = surfaces
        .iter()
        .find(|s| s.name == "VERSION")
        .ok_or_else(|| anyhow::anyhow!("/VERSION missing; cannot run version sweep"))?
        .version
        .clone();

    let mut mismatches = Vec::new();
    for s in &surfaces {
        if s.version != reference {
            mismatches.push(format!(
                "  {:<20} {} != {} ({})",
                s.name,
                s.version,
                reference,
                s.path.display()
            ));
        } else {
            log::debug!("  {:<20} {} ✓", s.name, s.version);
        }
    }

    if !mismatches.is_empty() {
        let mut msg =
            format!("version drift detected — {} surface(s) disagree with /VERSION={reference}:\n", mismatches.len());
        for m in &mismatches {
            msg.push('\n');
            msg.push_str(m);
        }
        msg.push_str("\n\nFix by running `scripts/sync-version.sh` in the workspace, or edit the surface(s) above.");
        anyhow::bail!(msg);
    }

    log::info!(
        "[version-sweep] ✓ all 5 surfaces agree on v{reference}"
    );
    Ok(())
}

fn collect(ws: &Path) -> Result<Vec<Surface>> {
    let mut out = Vec::new();

    // 1. /VERSION — plain text, one line.
    let p = ws.join("VERSION");
    out.push(Surface {
        name: "VERSION",
        version: std::fs::read_to_string(&p)
            .with_context(|| p.display().to_string())?
            .trim()
            .to_string(),
        path: p,
    });

    // 2. Cargo.toml [workspace.package] version.
    let p = ws.join("Cargo.toml");
    let text = std::fs::read_to_string(&p).with_context(|| p.display().to_string())?;
    let v = extract_between(&text, r#"[workspace.package]"#, "version", '"', '"')
        .ok_or_else(|| anyhow::anyhow!("Cargo.toml [workspace.package] version not found"))?;
    out.push(Surface { name: "Cargo.toml", version: v, path: p });

    // 3. install.sh VERSION="..."
    let p = ws.join("install.sh");
    let text = std::fs::read_to_string(&p).with_context(|| p.display().to_string())?;
    let v = scan_for(&text, r#"VERSION=""#, '"')
        .ok_or_else(|| anyhow::anyhow!("install.sh VERSION=... not found"))?;
    out.push(Surface { name: "install.sh", version: v, path: p });

    // 4. install.ps1 $Version = "..."
    let p = ws.join("install.ps1");
    let text = std::fs::read_to_string(&p).with_context(|| p.display().to_string())?;
    let v = scan_for(&text, r#"$Version = ""#, '"')
        .ok_or_else(|| anyhow::anyhow!("install.ps1 $Version = \"...\" not found"))?;
    out.push(Surface { name: "install.ps1", version: v, path: p });

    // 5. landing/index.html <!-- VERSION-BADGE -->vX.Y.Z<!-- /VERSION-BADGE -->
    let p = ws.join("landing/index.html");
    let text = std::fs::read_to_string(&p).with_context(|| p.display().to_string())?;
    let marker_start = "<!-- VERSION-BADGE -->v";
    let marker_end = "<!-- /VERSION-BADGE -->";
    let Some(i) = text.find(marker_start) else {
        anyhow::bail!("landing/index.html VERSION-BADGE marker missing");
    };
    let after = &text[i + marker_start.len()..];
    let Some(j) = after.find(marker_end) else {
        anyhow::bail!("landing/index.html VERSION-BADGE close marker missing");
    };
    out.push(Surface {
        name: "landing/VERSION-BADGE",
        version: after[..j].trim().to_string(),
        path: p,
    });

    Ok(out)
}

/// Find `needle` inside `section` scope, then extract value between
/// next occurrence of `open` and `close` after it.
fn extract_between(text: &str, section: &str, needle: &str, open: char, close: char) -> Option<String> {
    let i = text.find(section)?;
    let rest = &text[i..];
    let j = rest.find(needle)?;
    let after = &rest[j + needle.len()..];
    let k = after.find(open)? + 1;
    let after = &after[k..];
    let end = after.find(close)?;
    Some(after[..end].to_string())
}

/// Find `needle` and return the value up to next `close`.
fn scan_for(text: &str, needle: &str, close: char) -> Option<String> {
    let i = text.find(needle)?;
    let after = &text[i + needle.len()..];
    let end = after.find(close)?;
    Some(after[..end].to_string())
}
