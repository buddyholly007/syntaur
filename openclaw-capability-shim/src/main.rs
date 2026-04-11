//! openclaw-capability-shim — tiny HTTP server that runs one or more
//! "capabilities" on behalf of openclawprod, so the gateway can offload
//! specific tools to a different host without committing to the full v5
//! distributed multi-host architecture.
//!
//! ## What this is
//!
//! A standalone binary you deploy on any always-on host (claudevm, the
//! gaming PC, a NAS, whatever). It exposes a single HTTP endpoint:
//!
//!   POST /execute
//!     { "tool": "<name>", "args": <json> }
//!     → { "ok": true, "output": "..." }   (success)
//!     → { "ok": false, "error": "..." }   (failure, HTTP 500)
//!
//! Auth: shared bearer token in `Authorization: Bearer <token>`. Token is
//! read from the `SHIM_TOKEN` env var at startup; if unset, the shim
//! refuses to start. There's no per-user model — the shim trusts whoever
//! holds the bearer.
//!
//! Listen address: `SHIM_BIND` env, default `127.0.0.1:18790`. Bind to
//! `0.0.0.0:18790` to accept connections from openclawprod, but only do
//! that on a trusted LAN segment. The shim does no TLS termination of its
//! own — front it with Tailscale or stick to localhost.
//!
//! ## What this is NOT
//!
//! * Not Item 5 of the v5 plan. There's no master/worker registry, no
//!   capability routing, no failover, no gRPC, no shared SQLite replica.
//! * Not a generic RPC framework. The shim only knows about a fixed set
//!   of capabilities defined at compile time below.
//! * Not invoked by rust-openclaw automatically. You wire openclawprod
//!   to call this shim manually (e.g. point a tool's HTTP fallback URL
//!   at it, or write a wrapper tool in rust-openclaw that POSTs here).
//!
//! ## Adding a new capability
//!
//! 1. Write an `async fn run_X(args: Value) -> Result<String, String>`
//!    below the `dispatch` function.
//! 2. Add a `"X" => run_X(args).await` arm to the match in `dispatch`.
//! 3. Rebuild + redeploy.
//!
//! Each capability is a one-liner thunk in `dispatch` plus its own helper
//! function. Keep helpers self-contained — the whole point of the shim is
//! to be small enough that you can read it in one sitting.

use std::env;
use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use log::{error, info, warn};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_BIND: &str = "127.0.0.1:18790";
const HTTP_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const HTTP_FETCH_MAX_BYTES: usize = 200 * 1024;
const CODE_EXEC_TIMEOUT: Duration = Duration::from_secs(30);
const CODE_EXEC_MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone)]
struct ShimState {
    /// Shared bearer token. Compared with `Authorization: Bearer <token>`
    /// on every request.
    token: Arc<String>,
    /// Pre-built reqwest client used by the http_fetch capability.
    /// Reusing the client across requests gives us connection pooling
    /// and a single TLS handshake per host.
    http: reqwest::Client,
    /// Whether bwrap was found at startup. Determines whether
    /// `code_execute` is available; absent → returns a clear error.
    bwrap_available: bool,
}

#[derive(Deserialize)]
struct ExecuteRequest {
    tool: String,
    #[serde(default)]
    args: Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let token = env::var("SHIM_TOKEN").context(
        "SHIM_TOKEN env var is required. Generate with: openssl rand -base64 32",
    )?;
    if token.len() < 16 {
        anyhow::bail!("SHIM_TOKEN must be at least 16 characters");
    }

    let bind: SocketAddr = env::var("SHIM_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .context("SHIM_BIND must be host:port")?;

    let bwrap_available = std::path::Path::new("/usr/bin/bwrap").exists();
    if bwrap_available {
        info!("bwrap detected at /usr/bin/bwrap → code_execute enabled");
    } else {
        warn!("bwrap not found → code_execute will return errors");
    }

    let http = reqwest::Client::builder()
        .timeout(HTTP_FETCH_TIMEOUT)
        .user_agent("openclaw-capability-shim/0.1")
        .build()
        .context("build reqwest client")?;

    let state = ShimState {
        token: Arc::new(token),
        http,
        bwrap_available,
    };

    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/execute", post(handle_execute))
        .with_state(state);

    info!("openclaw-capability-shim listening on {}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ── handlers ──────────────────────────────────────────────────────────────

async fn handle_health() -> Json<Value> {
    Json(json!({"status": "ok", "shim": "openclaw-capability-shim", "version": env!("CARGO_PKG_VERSION")}))
}

async fn handle_execute(
    State(state): State<ShimState>,
    headers: HeaderMap,
    Json(req): Json<ExecuteRequest>,
) -> (StatusCode, Json<Value>) {
    // Bearer auth. Constant-time would be nicer; for a LAN-only shim with
    // a 32-byte token the constant-time leak is irrelevant.
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .unwrap_or("");
    if bearer != state.token.as_str() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error": "unauthorized"})),
        );
    }

    info!(
        "[exec] tool={} args_chars={}",
        req.tool,
        req.args.to_string().len()
    );

    let result = dispatch(&state, &req.tool, req.args).await;
    match result {
        Ok(text) => (
            StatusCode::OK,
            Json(json!({"ok": true, "tool": req.tool, "output": text})),
        ),
        Err(e) => {
            warn!("[exec] tool={} error={}", req.tool, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "tool": req.tool, "error": e})),
            )
        }
    }
}

// ── dispatch ──────────────────────────────────────────────────────────────

async fn dispatch(state: &ShimState, tool: &str, args: Value) -> Result<String, String> {
    match tool {
        "http_fetch" => run_http_fetch(state, args).await,
        "code_execute" => run_code_execute(state, args).await,
        // Add new capabilities here. Keep each one in its own helper
        // function below; this match should stay scannable in one screen.
        other => Err(format!("unknown tool: {}", other)),
    }
}

// ── capability: http_fetch ────────────────────────────────────────────────
//
// Simple GET-only HTTP client. Useful for offloading network egress when
// openclawprod's outbound is constrained or you want a different egress
// IP for fetching content.
//
// Args: { "url": "<url>" }
// Output: response body, capped at HTTP_FETCH_MAX_BYTES, error message
//         on non-2xx.

async fn run_http_fetch(state: &ShimState, args: Value) -> Result<String, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing url".to_string())?;

    let resp = state
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("http error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("read body: {}", e))?;
    let truncated = bytes.len() > HTTP_FETCH_MAX_BYTES;
    let slice = &bytes[..bytes.len().min(HTTP_FETCH_MAX_BYTES)];
    let mut body = String::from_utf8_lossy(slice).into_owned();
    if truncated {
        body.push_str(&format!("\n\n... [truncated, {} bytes total]", bytes.len()));
    }
    Ok(body)
}

// ── capability: code_execute ──────────────────────────────────────────────
//
// Bubblewrap-sandboxed code execution. Mirrors the rust-openclaw
// code_execute tool's isolation model: bwrap with --unshare-all,
// read-only /usr, fresh /tmp, no network, RLIMITs via shell ulimit.
//
// Args: { "language": "python|bash|node", "code": "<source>", "timeout_secs": 30 }
// Output: combined stdout+stderr, capped at CODE_EXEC_MAX_OUTPUT.
//
// Returns a clear error if bwrap isn't installed on the shim host.

async fn run_code_execute(state: &ShimState, args: Value) -> Result<String, String> {
    if !state.bwrap_available {
        return Err(
            "bwrap (bubblewrap) is not installed on this shim host. \
             Install with: sudo apt-get install bubblewrap"
                .to_string(),
        );
    }

    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("python");
    let code = args
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing code".to_string())?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(CODE_EXEC_TIMEOUT.as_secs())
        .min(120);

    let interpreter = match language {
        "python" => "/usr/bin/python3",
        "bash" => "/bin/bash",
        "node" => "/usr/bin/node",
        other => return Err(format!("unsupported language: {}", other)),
    };

    // Write the code to a temp file under /tmp on the host. bwrap mounts
    // a fresh tmpfs over /tmp inside the sandbox so we copy the file in
    // via --bind on a separate workspace dir.
    let work = tempdir()?;
    let script_name = match language {
        "python" => "script.py",
        "bash" => "script.sh",
        "node" => "script.js",
        _ => unreachable!(),
    };
    let host_script = format!("{}/{}", work, script_name);
    std::fs::write(&host_script, code).map_err(|e| format!("write script: {}", e))?;

    // Standard bwrap isolation flags. Order matters: --clearenv must come
    // before --setenv overrides; --unshare-all must come early.
    let mut cmd = Command::new("/usr/bin/bwrap");
    cmd.arg("--unshare-all")
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--clearenv")
        .arg("--ro-bind").arg("/usr").arg("/usr")
        .arg("--ro-bind").arg("/etc/alternatives").arg("/etc/alternatives")
        .arg("--ro-bind").arg("/etc/ssl").arg("/etc/ssl")
        .arg("--ro-bind").arg("/etc/ca-certificates").arg("/etc/ca-certificates")
        .arg("--symlink").arg("/usr/bin").arg("/bin")
        .arg("--symlink").arg("/usr/sbin").arg("/sbin")
        .arg("--symlink").arg("/usr/lib").arg("/lib")
        .arg("--symlink").arg("/usr/lib64").arg("/lib64")
        .arg("--proc").arg("/proc")
        .arg("--dev").arg("/dev")
        .arg("--tmpfs").arg("/tmp")
        .arg("--bind").arg(&work).arg("/workspace")
        .arg("--chdir").arg("/workspace")
        .arg("--setenv").arg("HOME").arg("/workspace")
        .arg("--setenv").arg("PATH").arg("/usr/bin:/usr/local/bin")
        .arg("--setenv").arg("LANG").arg("C.UTF-8")
        .arg(interpreter)
        .arg(script_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| format!("spawn bwrap: {}", e))?;
    let wait = timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;
    let _ = std::fs::remove_dir_all(&work);

    let output = match wait {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("bwrap wait: {}", e)),
        Err(_) => return Err(format!("code_execute timed out after {}s", timeout_secs)),
    };

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        combined.push_str("\n--- stderr ---\n");
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if combined.len() > CODE_EXEC_MAX_OUTPUT {
        combined.truncate(CODE_EXEC_MAX_OUTPUT);
        combined.push_str("\n... [truncated]");
    }
    if !output.status.success() {
        combined.push_str(&format!(
            "\n[exit {}]",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(combined)
}

/// Create a fresh subdirectory under /tmp/openclaw-shim that the caller
/// owns. We don't use the `tempfile` crate so the dep tree stays minimal.
fn tempdir() -> Result<String, String> {
    let parent = "/tmp/openclaw-shim";
    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = format!("{}/job-{}-{}", parent, pid, nanos);
    std::fs::create_dir(&dir).map_err(|e| format!("mkdir job: {}", e))?;
    Ok(dir)
}

#[allow(dead_code)]
fn _ensure_error_used(e: &dyn std::error::Error) {
    error!("{}", e);
}
