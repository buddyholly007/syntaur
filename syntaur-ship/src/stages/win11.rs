//! Win11 nightly-tester refresh stage.
//!
//! Today (Apr 2026) the Win11 VM's `$env:LOCALAPPDATA\Syntaur\syntaur.exe`
//! only updates when someone manually SSHs in + runs `Invoke-WebRequest`.
//! Result: the nightly tester silently drifts versions behind prod.
//!
//! This stage pushes a fresh Windows binary to the VM via WinRM ->
//! PowerShell Invoke-WebRequest pulling from GitHub's latest release
//! (since tag+release is the published artifact surface).
//!
//! Fallback path: if the GitHub release tag doesn't match /VERSION
//! (e.g., CI failed like v0.5.0), skip the refresh + warn. The
//! alternative — cross-compiling Windows binaries from claudevm —
//! requires the MSVC toolchain and is out of scope for Phase 6.
//!
//! This stage runs at the END of the pipeline, after prod is confirmed
//! healthy. Failure is non-fatal — prod is already live.

use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

use crate::pipeline::StageContext;

const WIN11_WINRM: &str = "192.168.1.58";
const WIN11_USER: &str = "Sean";
const WIN11_PASS: &str = "1928";

pub fn run(ctx: &StageContext) -> Result<()> {
    if ctx.opts.dry_run {
        log::info!("[win11] dry-run — skipping refresh");
        return Ok(());
    }
    // Check GH for current "latest" release. If it matches /VERSION,
    // proceed. If it doesn't (e.g. CI failed), skip with a clear warn.
    let expected = std::fs::read_to_string(ctx.cfg.workspace.join("VERSION"))?
        .trim()
        .to_string();
    let gh_latest = fetch_gh_latest()?;
    if gh_latest != expected {
        log::warn!(
            "[win11] skipping nightly-tester refresh: GH Releases \"latest\" is v{gh_latest} but /VERSION is v{expected}. \
             Fix release-sign CI + re-dispatch, then re-run `syntaur-ship refresh-windows`."
        );
        return Ok(());
    }
    log::info!(">> [win11] refreshing nightly-tester binary to v{expected} via WinRM+Invoke-WebRequest");

    // PowerShell script run remotely via pywinrm (Mac Mini → Win11 VM).
    // Sourced from feedback/winrm_chunked_upload_avoid: have Windows
    // pull from GH; don't push chunked base64.
    let ps_script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Get-Process syntaur -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 3

$dir = "$env:LOCALAPPDATA\Syntaur"
if (-not (Test-Path $dir)) {{ New-Item -ItemType Directory -Force -Path $dir | Out-Null }}

if (Test-Path "$dir\syntaur.exe") {{
    Copy-Item "$dir\syntaur.exe" "$dir\syntaur.exe.prev" -Force
}}

$url = "https://github.com/buddyholly007/syntaur/releases/download/v{expected}/syntaur-gateway-windows-x86_64.exe"
$dst = "$dir\syntaur.exe"
$pp = $ProgressPreference
$ProgressPreference = "SilentlyContinue"
Invoke-WebRequest -Uri $url -OutFile $dst -UseBasicParsing
$ProgressPreference = $pp

$fi = Get-Item $dst
"size=" + $fi.Length
"sha256=" + (Get-FileHash $dst -Algorithm SHA256).Hash
"#,
    );

    // The WinRM bridge: we SSH to Mac Mini (which hosts the VM) and
    // drive pywinrm from there. Mac Mini already has pywinrm installed
    // per projects/win11_test_vm.md.
    //
    // CRITICAL — don't use `ssh … python3 -c "$CODE"`. That joins all
    // ssh args with spaces before handing to the remote shell, which
    // mangles newlines + triple-quoted strings. Instead we invoke
    // `python3 -` remotely and pipe the whole script in on stdin, so
    // no shell quoting rules apply. The earlier `-c` path silently
    // broke with `SyntaxError: invalid syntax` after the maud migration
    // shifted the raw-string layout — pipe-via-stdin fixes the class.
    let python_code = String::new()
        + "import winrm\n"
        + &format!(
            "s = winrm.Session('http://127.0.0.1:5985/wsman', auth=('{WIN11_USER}','{WIN11_PASS}'), transport='basic', read_timeout_sec=300, operation_timeout_sec=240)\n"
        )
        + "ps = r'''"
        + &ps_script
        + "'''\n"
        + "r = s.run_ps(ps)\n"
        + "print(r.std_out.decode('cp1252', errors='replace'))\n";

    let mut child = Command::new("ssh")
        .args(["sean@192.168.1.58", "python3", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn ssh → mac-mini python3")?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("stdin handle missing"))?
        .write_all(python_code.as_bytes())
        .context("write python to ssh stdin")?;
    let output = child.wait_with_output().context("wait on ssh child")?;
    if !output.status.success() {
        log::warn!(
            "[win11] refresh failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(300)
                .collect::<String>()
        );
        return Ok(());
    }
    let out = String::from_utf8_lossy(&output.stdout);
    log::info!("[win11] refresh output: {}", out.trim());
    Ok(())
}

fn fetch_gh_latest() -> Result<String> {
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    Ok(v["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no tag_name"))?
        .trim_start_matches('v')
        .to_string())
}
