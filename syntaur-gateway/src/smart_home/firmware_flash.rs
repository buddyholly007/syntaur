//! ESPHome OTA flash — Phase 6b companion to `firmware_role.rs`.
//!
//! Pipeline:
//!   1. `firmware_role::render_yaml(&req)` → final config string.
//!   2. `write_yaml_to_disk(&req, &build_dir)` → writes
//!      `<build_dir>/<name>/<name>.yaml`, returning the path. Idempotent;
//!      regenerates on every call.
//!   3. `flash_via_esphome(&yaml_path, target)` → shells out to
//!      `esphome run <yaml> --device <target>` and captures stdout +
//!      stderr. Returns `FlashResult` regardless of outcome so the
//!      caller can surface compile errors to the operator without
//!      re-running.
//!
//! The split exists so the YAML-write step is unit-testable
//! (filesystem only, deterministic) without invoking the long-running
//! `esphome` toolchain. Integration tests for the flash itself live in
//! the wizard CI job — they need real ESPHome installed.
//!
//! ## Where this runs
//!
//! `esphome` ships as a Python tool. It is *not* installed on the
//! production gateway host (TrueNAS Custom App, debian-slim image).
//! For a stock Sean install, the wizard endpoint:
//!   * accepts the request on the gateway,
//!   * SSHes to the build host (`gaming-pc` per
//!     `vault/projects/gaming_pc_hardware.md`) where esphome lives,
//!   * proxies the `flash_via_esphome` call there.
//!
//! The remote-shell layer is a follow-up; this module is the local
//! primitive. When invoked on a host without `esphome` on PATH, the
//! shell-out fails and the wizard surfaces "esphome not installed —
//! run from a build host".

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use super::firmware_role::{render_yaml, FirmwareRequest};

/// Maximum time to wait for `esphome run` to complete. Compile +
/// upload is dominated by the ESP-IDF build (~3-5 min cold,
/// ~30 s warm) plus OTA upload (~60 s). 15 minutes covers cold
/// builds with margin and forces a clean failure on a wedged
/// toolchain instead of holding the wizard request indefinitely.
const FLASH_TIMEOUT: Duration = Duration::from_secs(900);

/// Default build root if the caller doesn't override. Matches the
/// convention `firmware_role.rs` documents.
pub fn default_build_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join("esphome-builds");
    }
    PathBuf::from("/tmp/esphome-builds")
}

/// Render `req` and write the YAML to `<build_dir>/<name>/<name>.yaml`.
/// Creates the per-device subdirectory if missing. Returns the
/// absolute path of the written file. Existing files are overwritten
/// so the renderer is the source of truth — operators who hand-edit
/// the YAML will see their changes wiped on the next wizard run,
/// which matches `firmware_role.rs`'s contract.
///
/// The output file is created with mode `0600` on Unix because the
/// rendered config embeds the Wi-Fi password, OTA password, and Noise
/// API encryption key in plaintext. A default `0644` would expose
/// those secrets to any other local account on the build host.
pub fn write_yaml_to_disk(
    req: &FirmwareRequest,
    build_dir: &Path,
) -> Result<PathBuf, String> {
    // Defense-in-depth: re-check the name even though render_yaml does.
    // Stops a path-traversal attempt cold if the renderer's validation
    // is ever loosened.
    if req.name.is_empty()
        || !req
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("invalid device name `{}`", req.name));
    }

    let yaml = render_yaml(req)?;
    let dev_dir = build_dir.join(&req.name);
    std::fs::create_dir_all(&dev_dir)
        .map_err(|e| format!("create_dir_all({}): {e}", dev_dir.display()))?;
    let yaml_path = dev_dir.join(format!("{}.yaml", req.name));

    write_secret_file(&yaml_path, yaml.as_bytes())
        .map_err(|e| format!("write({}): {e}", yaml_path.display()))?;
    Ok(yaml_path)
}

#[cfg(unix)]
fn write_secret_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents)
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

/// Where to send the upload. `Ota("ip-or-hostname")` runs `esphome
/// run --device <ip>`; `CompileOnly` skips the upload step (used for
/// CI smoke tests + the "validate before adopting" wizard path).
#[derive(Debug, Clone)]
pub enum FlashTarget {
    Ota(String),
    CompileOnly,
}

/// Outcome of an `esphome` invocation. `success` is the conservative
/// "compile + upload both succeeded" signal; the operator sees `log`
/// regardless so a failed compile is debuggable from the wizard.
#[derive(Debug, Clone, Serialize)]
pub struct FlashResult {
    pub success: bool,
    pub log: String,
    pub elapsed_secs: u64,
    pub yaml_path: String,
}

/// Run `esphome` against the rendered YAML. Captures merged stdout +
/// stderr — esphome interleaves both to the same TTY in normal use,
/// and operators expect to see both.
pub async fn flash_via_esphome(
    yaml_path: &Path,
    target: FlashTarget,
) -> Result<FlashResult, String> {
    let started = std::time::Instant::now();
    let mut cmd = Command::new("esphome");
    match &target {
        FlashTarget::Ota(host) => {
            cmd.arg("run").arg(yaml_path).arg("--device").arg(host);
        }
        FlashTarget::CompileOnly => {
            cmd.arg("compile").arg(yaml_path);
        }
    }
    // Hide the interactive OTA-source prompt that esphome shows when
    // both serial + Wi-Fi are reachable. `--no-logs` skips the
    // post-upload monitor; the wizard reconnects via the gateway's
    // own ESPHome client to verify, and there's no operator at the
    // CLI to read the log stream anyway.
    cmd.arg("--no-logs")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        // Most common case: PATH doesn't have `esphome`. Surface the
        // "install esphome" hint up-front so the wizard's error UI
        // can show it verbatim.
        format!(
            "spawn esphome: {e} (is `esphome` installed and on PATH? \
             try `pip install esphome` on the build host)"
        )
    })?;

    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let stderr = child.stderr.take().ok_or("no stderr pipe")?;

    // Drain both pipes concurrently into a shared log buffer so we
    // capture the full transcript even on a slow compile. Timeline
    // ordering is approximate (we interleave by line not by byte
    // arrival) but matches what an operator sees on a live terminal.
    let log_collect = tokio::spawn(async move {
        let mut log = String::new();
        let mut so = BufReader::new(stdout).lines();
        let mut se = BufReader::new(stderr).lines();
        loop {
            tokio::select! {
                line = so.next_line() => match line {
                    Ok(Some(l)) => { log.push_str(&l); log.push('\n'); }
                    _ => break,
                },
                line = se.next_line() => match line {
                    Ok(Some(l)) => { log.push_str(&l); log.push('\n'); }
                    _ => break,
                },
            }
        }
        // Drain whichever pipe outlives the other.
        while let Ok(Some(l)) = so.next_line().await {
            log.push_str(&l);
            log.push('\n');
        }
        while let Ok(Some(l)) = se.next_line().await {
            log.push_str(&l);
            log.push('\n');
        }
        log
    });

    let status = match timeout(FLASH_TIMEOUT, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("wait esphome: {e}")),
        Err(_) => {
            // Timeout — kill_on_drop fires when `child` drops.
            return Err(format!(
                "esphome timed out after {}s",
                FLASH_TIMEOUT.as_secs()
            ));
        }
    };
    let log = log_collect.await.unwrap_or_default();

    let elapsed = started.elapsed().as_secs();
    Ok(FlashResult {
        success: status.success(),
        log,
        elapsed_secs: elapsed,
        yaml_path: yaml_path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::esphome_discovery::SuggestedRole;
    use super::super::firmware_role::{FirmwareRequest, HardwareVariant};
    use super::*;
    use tempfile::TempDir;

    fn req() -> FirmwareRequest {
        FirmwareRequest {
            name: "test-proxy".into(),
            friendly_name: Some("Test Proxy".into()),
            variant: HardwareVariant::Esp32C3,
            role: SuggestedRole::BtProxyPassive,
            api_encryption_key: Some(
                "0Q/SMOlxnQQU6dIKGFm+7Lgusp0Ke4eU3tKfj3eHNbo=".into(),
            ),
            ota_password: Some("ota-secret".into()),
            wifi_ssid: "IOT".into(),
            wifi_password: "pw".into(),
            ap_fallback_password: None,
        }
    }

    #[test]
    fn write_yaml_creates_subdir_and_file() {
        let tmp = TempDir::new().unwrap();
        let path = write_yaml_to_disk(&req(), tmp.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "test-proxy.yaml");
        assert!(path.starts_with(tmp.path().join("test-proxy")));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("name: test-proxy"));
        assert!(body.contains("bluetooth_proxy:"));
    }

    #[test]
    fn write_yaml_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let path = write_yaml_to_disk(&req(), tmp.path()).unwrap();
        std::fs::write(&path, "stale: true\n").unwrap();
        let path2 = write_yaml_to_disk(&req(), tmp.path()).unwrap();
        let body = std::fs::read_to_string(&path2).unwrap();
        assert!(!body.starts_with("stale:"));
        assert!(body.contains("name: test-proxy"));
    }

    #[test]
    fn write_yaml_rejects_bad_request_via_renderer() {
        let mut bad = req();
        bad.name = "Bad Name".into(); // invalid
        let tmp = TempDir::new().unwrap();
        assert!(write_yaml_to_disk(&bad, tmp.path()).is_err());
    }

    #[test]
    fn default_build_dir_uses_home() {
        std::env::set_var("HOME", "/tmp/fake-home");
        let d = default_build_dir();
        assert_eq!(d, PathBuf::from("/tmp/fake-home/esphome-builds"));
    }

    #[tokio::test]
    async fn flash_via_esphome_surfaces_missing_binary() {
        // Override PATH so `esphome` definitely isn't found, regardless
        // of the host's setup. Spawn should fail with our actionable
        // hint, not panic or block.
        let prev = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", "/nonexistent");
        let tmp = TempDir::new().unwrap();
        let yaml = tmp.path().join("x.yaml");
        std::fs::write(&yaml, "esphome: { name: x }\n").unwrap();
        let res = flash_via_esphome(&yaml, FlashTarget::CompileOnly).await;
        std::env::set_var("PATH", prev);
        let err = res.expect_err("missing esphome should error");
        assert!(err.contains("esphome"), "error mentions esphome: {err}");
    }
}
