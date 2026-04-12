//! Wrapper around the `rust-captcha-bridge` binary so any agent can solve a
//! captcha-protected login flow without writing site-specific code in
//! syntaur itself.
//!
//! The bridge binary lives at `/home/sean/rust-captcha-bridge` on
//! syntaur-server and reads its config (2Captcha API key + per-site
//! credentials) from `~/.captcha-bridge/config.toml`. Site flows are
//! defined in the bridge crate at `/home/sean/rust-captcha-bridge/src/sites/`.
//!
//! Tools exposed:
//!  - `captcha_bridge_solve {site, label?}`  → runs the flow and returns the key
//!  - `captcha_bridge_balance`               → 2Captcha account balance
//!  - `captcha_bridge_list_sites`            → registered site names

use log::{info, warn};
use std::time::Duration;
use tokio::process::Command;

const BRIDGE_BIN: &str = "/home/sean/rust-captcha-bridge";
/// Maximum time we let a single solve run. Includes browser nav + 2Captcha
/// solve (~1-3 min) + post-login navigation. 8 min is generous.
const SOLVE_TIMEOUT: Duration = Duration::from_secs(8 * 60);

fn ensure_bridge_present() -> Result<(), String> {
    if !std::path::Path::new(BRIDGE_BIN).exists() {
        return Err(format!(
            "captcha bridge binary not found at {BRIDGE_BIN} — install with `cargo build --release` and scp"
        ));
    }
    Ok(())
}

/// Solve a captcha-protected login flow and return the extracted credential
/// (e.g. an API key). Site must be one of the names returned by
/// `captcha_bridge_list_sites`.
pub async fn solve(site: &str, label: Option<&str>) -> Result<String, String> {
    ensure_bridge_present()?;
    if site.is_empty() {
        return Err("site is required".to_string());
    }
    // Reject anything that isn't a sane identifier — protects against shell
    // injection even though we use process args (defense in depth).
    if !site
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid site name: {site}"));
    }

    let mut cmd = Command::new(BRIDGE_BIN);
    cmd.arg("solve").arg(site);
    if let Some(l) = label {
        if !l.is_empty() {
            cmd.arg("--label").arg(l);
        }
    }
    // Force info-level logging from the bridge so the tool output captures the
    // flow trace, but silence chromiumoxide spam.
    cmd.env("RUST_LOG", "info,chromiumoxide=off");
    info!(
        "[captcha_bridge_solve] running {} solve {}",
        BRIDGE_BIN, site
    );

    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn captcha bridge: {e}"))?;

    let output = match tokio::time::timeout(SOLVE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("captcha bridge wait failed: {e}")),
        Err(_) => {
            return Err(format!(
                "captcha bridge timed out after {}s",
                SOLVE_TIMEOUT.as_secs()
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        warn!(
            "[captcha_bridge_solve] failed (status={}): {}",
            output.status,
            stderr.lines().last().unwrap_or("(no stderr)")
        );
        return Err(format!(
            "captcha bridge exited {}: {}",
            output.status,
            stderr.lines().rev().take(5).collect::<Vec<_>>().join(" | ")
        ));
    }

    // The bridge prints the extracted key as the LAST non-empty stdout line.
    let key = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "captcha bridge produced no output".to_string())?;

    info!(
        "[captcha_bridge_solve] success site={} key_prefix={}",
        site,
        &key.chars().take(8).collect::<String>()
    );
    Ok(format!(
        "Captcha bridge solved '{}'.\nExtracted credential:\n{}\n\nFull stderr trace:\n{}",
        site,
        key,
        stderr.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    ))
}

/// Return the current 2Captcha account balance in USD.
pub async fn balance() -> Result<String, String> {
    ensure_bridge_present()?;
    let output = Command::new(BRIDGE_BIN)
        .arg("balance")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run captcha bridge balance: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "captcha bridge balance exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List all sites the bridge knows how to solve. Each line is a site name
/// passable to `captcha_bridge_solve`.
pub async fn list_sites() -> Result<String, String> {
    ensure_bridge_present()?;
    let output = Command::new(BRIDGE_BIN)
        .arg("list")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run captcha bridge list: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "captcha bridge list exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
