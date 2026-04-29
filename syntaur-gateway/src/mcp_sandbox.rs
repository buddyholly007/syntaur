//! MCP process sandboxing — Phase 4.6 of the security plan.
//!
//! Every MCP server Syntaur spawns runs as a child process with full
//! access to the gateway's filesystem + network. A prompt-injection pivot
//! that tricks the agent into calling a compromised MCP server could read
//! `~/.syntaur/master.key`, exfiltrate the vault, or reach internal LAN
//! services. None of those belong to the MCP tool's legitimate surface.
//!
//! This module wraps the child command with `bubblewrap` (bwrap) when it's
//! available on the host. bwrap is the same sandbox technology Flatpak
//! uses — battle-tested, unprivileged, and ships in nearly every Linux
//! distro including TrueNAS's Electric Eel base image.
//!
//! Default sandbox profile for MCP servers:
//!   - Read-only root filesystem view (`--ro-bind / /`)
//!   - Writable ephemeral /tmp (`--tmpfs /tmp`)
//!   - Minimal /dev + /proc (`--dev /dev --proc /proc`)
//!   - New user namespace (`--unshare-user`)
//!   - New PID namespace (`--unshare-pid`)
//!   - Die with parent (`--die-with-parent`) so a crashed gateway doesn't
//!     leave child processes behind
//!   - Explicit RW mounts only for declared data directories
//!
//! Network is NOT unshared by default — most MCP servers (search, web,
//! fetch) legitimately need outbound HTTP. Operators who want tighter
//! isolation can set `sandbox.unshare_net = true` per-server in the mcp
//! config.
//!
//! Default behavior on Linux when bwrap is missing: **fail closed**. The
//! returned Command is `/bin/false`, so the MCP server is unusable rather
//! than unsandboxed. Operators who genuinely cannot install bubblewrap
//! (custom minimal images, weird distros) can opt out with one of two
//! env flags:
//!
//!   - `SYNTAUR_ALLOW_UNSANDBOXED_MCP=1` — explicit opt-in to the old
//!     fail-open behavior. Recommended for nothing.
//!   - `SYNTAUR_STRICT_MCP_SANDBOX=0` — legacy spelling kept for backward
//!     compat with deployments that already set the inverse flag.
//!
//! Non-Linux platforms (macOS, Windows) keep the old fail-open default
//! because bubblewrap is Linux-only and there's no equivalent backend
//! ready. They get a `warn!` on every spawn so it's still visible.

use tokio::process::Command;

/// Sandbox policy resolved from config + host capabilities. Built once
/// per MCP spawn and consumed by `wrap_command`.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Extra writable bind-mounts. Each is `(source, dest)` — both paths
    /// on the host; bwrap binds `source` into the sandbox at `dest`.
    pub rw_mounts: Vec<(String, String)>,
    /// Whether to drop the process into a fresh network namespace. Off by
    /// default — many MCP servers need outbound HTTP.
    pub unshare_net: bool,
}

/// Whether bubblewrap is available on this host. Cached after first probe.
pub fn bwrap_available() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| std::path::Path::new("/usr/bin/bwrap").exists())
}

/// Wrap an MCP command with `bwrap` if available. Returns a new `Command`
/// whose program is `bwrap` and whose arguments encapsulate the original
/// command as a sandboxed child. If bwrap isn't available the original
/// command is returned unchanged and a warning is logged.
///
/// The caller MUST still configure stdio / env / kill_on_drop on the
/// returned Command — this fn only rewrites the program + argv.
pub fn wrap_command(
    server_name: &str,
    program: &str,
    args: &[String],
    policy: &Policy,
) -> Command {
    if !bwrap_available() {
        // Default on Linux is FAIL CLOSED — bubblewrap is required. The
        // Dockerfile installs it; install.sh installs it alongside the
        // gstreamer deps. If it's missing on Linux it's a real operator
        // gap, not a platform limitation.
        //
        // Two env flags can opt back into fail-open:
        //   - `SYNTAUR_ALLOW_UNSANDBOXED_MCP=1` (current preferred name)
        //   - `SYNTAUR_STRICT_MCP_SANDBOX=0`    (legacy inverse spelling)
        //
        // Non-Linux (macOS, Windows) keeps fail-open because there's no
        // bubblewrap backend on those platforms yet.
        let opt_out_explicit = std::env::var("SYNTAUR_ALLOW_UNSANDBOXED_MCP")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let opt_out_legacy = std::env::var("SYNTAUR_STRICT_MCP_SANDBOX")
            .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
            .unwrap_or(false);
        let allow_unsandboxed = opt_out_explicit || opt_out_legacy;

        #[cfg(target_os = "linux")]
        let fail_closed = !allow_unsandboxed;
        #[cfg(not(target_os = "linux"))]
        let fail_closed = false;

        if fail_closed {
            log::error!(
                "[mcp:{server_name}] REFUSING to spawn '{program}': bubblewrap is \
                 missing and fail-closed sandboxing is the default on Linux. \
                 Install bubblewrap (apt/dnf/pacman install bubblewrap), or set \
                 SYNTAUR_ALLOW_UNSANDBOXED_MCP=1 if you accept the risk."
            );
            let mut c = Command::new("/bin/false");
            c.arg("--syntaur-mcp-sandbox-required");
            return c;
        }

        #[cfg(target_os = "linux")]
        log::error!(
            "[mcp:{server_name}] bubblewrap NOT installed and \
             SYNTAUR_ALLOW_UNSANDBOXED_MCP=1 — spawning '{program}' UNSANDBOXED. \
             Fix the underlying issue: apt/dnf/pacman install bubblewrap."
        );
        #[cfg(not(target_os = "linux"))]
        log::warn!(
            "[mcp:{server_name}] bubblewrap not available on this OS — spawning '{program}' \
             unsandboxed. Linux is the only platform with a sandbox backend today."
        );

        let mut c = Command::new(program);
        c.args(args);
        return c;
    }

    let mut bwrap_args: Vec<String> = vec![
        "--ro-bind".into(), "/".into(), "/".into(),
        "--dev".into(), "/dev".into(),
        "--proc".into(), "/proc".into(),
        "--tmpfs".into(), "/tmp".into(),
        "--unshare-user".into(),
        "--unshare-pid".into(),
        "--die-with-parent".into(),
        // Keep new-session on so the child can't read the parent's TTY
        // (it'd otherwise inherit any terminal lines leaking via /dev/tty).
        "--new-session".into(),
    ];
    if policy.unshare_net {
        bwrap_args.push("--unshare-net".into());
    }
    for (src, dst) in &policy.rw_mounts {
        bwrap_args.push("--bind".into());
        bwrap_args.push(src.clone());
        bwrap_args.push(dst.clone());
    }
    // Separator between bwrap flags and the inner command.
    bwrap_args.push("--".into());
    bwrap_args.push(program.to_string());
    bwrap_args.extend(args.iter().cloned());

    log::info!(
        "[mcp:{server_name}] sandbox: bwrap with {} extra RW mount(s), unshare_net={}",
        policy.rw_mounts.len(),
        policy.unshare_net
    );

    let mut c = Command::new("/usr/bin/bwrap");
    c.args(bwrap_args);
    c
}
