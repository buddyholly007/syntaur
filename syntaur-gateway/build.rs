//! Build-time metadata capture.
//!
//! Embeds `GIT_COMMIT` (full SHA of HEAD), `GIT_COMMIT_SHORT` (7 chars),
//! and `BUILD_TIMESTAMP` (ISO-8601 UTC) into the binary so the runtime
//! `/api/version-proof` endpoint can report provenance without relying
//! on filesystem lookups.
//!
//! If git isn't available (e.g. source tarball build), falls back to
//! "unknown" + the current time — never fails the build.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let short = commit.chars().take(7).collect::<String>();

    // Avoid pulling chrono as a build-dependency — shell out to `date`
    // for RFC-3339 UTC. Fallback: seconds since epoch if date missing.
    let timestamp = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| format!("epoch-{}", d.as_secs()))
                .unwrap_or_else(|_| "unknown".into())
        });

    println!("cargo:rustc-env=SYNTAUR_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=SYNTAUR_GIT_COMMIT_SHORT={short}");
    println!("cargo:rustc-env=SYNTAUR_BUILD_TIMESTAMP={timestamp}");

    // Rerun if HEAD changes.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads/main");
}
