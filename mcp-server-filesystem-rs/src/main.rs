//! Pure-Rust replacement for `@modelcontextprotocol/server-filesystem`.
//!
//! Drop-in replacement: same tool names, same input schemas, same text
//! output format. Driven from `rust-openclaw`'s MCP client.
//!
//! Usage:
//!     mcp-server-filesystem-rs <allowed-dir> [<allowed-dir>...]
//!
//! All file operations are constrained to the allowed directory list. Symlink
//! targets that escape the allowed set are rejected. Writes are atomic via
//! temp-file + rename.

use std::env;
use std::process::ExitCode;
use std::sync::Arc;

mod ops;
mod paths;
mod tools;

use tools::FilesystemHandler;

#[tokio::main]
async fn main() -> ExitCode {
    // Logging goes to stderr — stdout is reserved for the JSON-RPC stream.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .try_init();

    let raw_args: Vec<String> = env::args().skip(1).collect();
    if raw_args.is_empty() {
        eprintln!("Usage: mcp-server-filesystem-rs <allowed-directory> [<allowed-directory>...]");
        eprintln!("At least one directory is required.");
        return ExitCode::from(2);
    }

    let allowed = match resolve_allowed(&raw_args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {:#}", e);
            return ExitCode::from(2);
        }
    };

    log::info!(
        "starting filesystem MCP server with {} allowed dir(s):",
        allowed.len()
    );
    for d in &allowed {
        log::info!("  - {}", d.display());
    }

    let handler = Arc::new(FilesystemHandler::new(allowed));

    if let Err(e) = mcp_protocol::run_stdio_server(handler).await {
        log::error!("server exited with error: {}", e);
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

/// Resolve each requested allowed directory: expand `~`, canonicalize via
/// `realpath`, and verify it's actually a directory. Mirrors what the Node
/// reference does on startup so symlink-pointing-to-symlink dirs work.
fn resolve_allowed(raw: &[String]) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        let expanded = paths::expand_home(r);
        // We allow non-existent dirs through with their normalized absolute
        // path, matching the Node reference's behavior. validatePath will
        // still reject them on first use, but the server stays runnable.
        let normalized = match std::fs::canonicalize(&expanded) {
            Ok(p) => p,
            Err(_) => paths::normalize_absolute(&expanded),
        };
        if normalized.exists() && !normalized.is_dir() {
            anyhow::bail!("not a directory: {}", normalized.display());
        }
        out.push(normalized);
    }
    if out.is_empty() {
        anyhow::bail!("no allowed directories provided");
    }
    Ok(out)
}

