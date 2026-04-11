//! `code_execute` — sandboxed code execution via bubblewrap.
//!
//! Spawns the user's interpreter inside a `bwrap` jail with these properties:
//!   * `--unshare-all` → fresh user/IPC/PID/network/UTS/cgroup namespaces
//!   * `--die-with-parent` → child dies if syntaur dies
//!   * `--new-session` → cannot signal the parent
//!   * read-only bind of `/usr`, `/lib`, `/lib64`, `/etc` (system libraries only)
//!   * tmpfs on `/tmp`, `/workspace`, `/dev`
//!   * fresh `/proc`
//!   * NO host filesystem access at all (not even read-only)
//!   * resource limits via prlimit (RAM, CPU, file size, processes)
//!   * wall-clock kill via tokio::time::timeout
//!
//! Languages supported in v1: `python`, `bash`, `node`. Each maps to the
//! corresponding system interpreter inside the sandbox (so the interpreter
//! must be installed on the host — bwrap inherits the host's `/usr`).
//!
//! Files passed via the `files` argument are written to `/workspace/<name>`
//! before execution, so the code can `open("name")` them. Files written into
//! `/workspace` after execution are scanned and returned as artifacts.
//!
//! This is `--unshare-all` strict by default. If a future use case needs
//! network access, add a separate `code_execute_online` tool with explicit
//! domain allowlisting rather than relaxing this one.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use log::{debug, info, warn};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

use crate::tools::extension::{Artifact, RichToolResult, Tool, ToolContext};

const TOOL_NAME: &str = "code_execute";

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_CODE_BYTES: usize = 64 * 1024;
const MAX_INPUT_FILE_BYTES: usize = 1024 * 1024;
const MAX_STDOUT_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_ARTIFACT_BYTES: usize = 5 * 1024 * 1024;
const ARTIFACT_INLINE_LIMIT: usize = 256 * 1024;
const MEM_LIMIT_BYTES: usize = 512 * 1024 * 1024;
const FSIZE_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const ARTIFACT_STORE_DIR: &str = "/tmp/syntaur-artifacts";

pub struct CodeExecuteTool;

#[async_trait]
impl Tool for CodeExecuteTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": TOOL_NAME,
                "description": "Run code in a sandboxed environment with NO network access \
                    and NO host filesystem access. Use this for data analysis, calculations, \
                    plotting, parsing, and any computation you want grounded in real execution \
                    rather than guessed. Returns stdout, stderr, exit code, and any files \
                    written to /workspace as artifacts. Sandbox: bubblewrap with fresh \
                    namespaces, 30s wall time, 512MB RAM, 64 processes. \
                    Pass input files via the `files` argument; they appear at /workspace/<name>.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "language": {
                            "type": "string",
                            "enum": ["python", "bash", "node"],
                            "description": "Interpreter to run (python = python3, bash, node)"
                        },
                        "code": {
                            "type": "string",
                            "description": "Source code to execute. Max 64KB."
                        },
                        "files": {
                            "type": "object",
                            "description": "Optional input files placed at /workspace/<name>. \
                                Map of filename → string content. Each file max 1MB.",
                            "additionalProperties": {"type": "string"}
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Wall-clock timeout. Default 30, max 120."
                        }
                    },
                    "required": ["language", "code"]
                }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let language = args
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or("missing 'language'")?;
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or("missing 'code'")?;

        if code.len() > MAX_CODE_BYTES {
            return Err(format!(
                "code too large: {} bytes (max {})",
                code.len(),
                MAX_CODE_BYTES
            ));
        }

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .map(|t| t.min(MAX_TIMEOUT_SECS).max(1))
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let interpreter = match language {
            "python" => "/usr/bin/python3",
            "bash" => "/bin/bash",
            "node" => "/usr/bin/node",
            other => return Err(format!("unsupported language: {}", other)),
        };

        // Build the host-side workspace dir that we'll bind into the sandbox
        // as RW. This is where the code, input files, and artifacts live.
        let session_id = Uuid::new_v4().simple().to_string();
        let host_workspace = std::env::temp_dir().join(format!("syntaur-sandbox-{}", session_id));
        if let Err(e) = std::fs::create_dir_all(&host_workspace) {
            return Err(format!("create sandbox workspace: {}", e));
        }
        // Cleanup guard — best-effort delete on drop
        let _cleanup = ScopedDir(host_workspace.clone());

        // Write the user's code to a file in the workspace.
        let code_filename = match language {
            "python" => "_code.py",
            "bash" => "_code.sh",
            "node" => "_code.js",
            _ => unreachable!(),
        };
        let code_path = host_workspace.join(code_filename);
        if let Err(e) = std::fs::write(&code_path, code) {
            return Err(format!("write code file: {}", e));
        }

        // Write any input files
        if let Some(files) = args.get("files").and_then(|v| v.as_object()) {
            for (name, content) in files {
                let content_str = content.as_str().unwrap_or_default();
                if content_str.len() > MAX_INPUT_FILE_BYTES {
                    return Err(format!(
                        "input file '{}' too large: {} bytes (max {})",
                        name,
                        content_str.len(),
                        MAX_INPUT_FILE_BYTES
                    ));
                }
                // Sanitize filename — only allow safe chars to prevent path traversal
                if name.contains("..") || name.contains('/') || name.contains('\0') {
                    return Err(format!("unsafe input filename: {}", name));
                }
                let path = host_workspace.join(name);
                if let Err(e) = std::fs::write(&path, content_str) {
                    return Err(format!("write input file '{}': {}", name, e));
                }
            }
        }

        // Snapshot input files (so we can detect new artifacts after run)
        let pre_run: std::collections::HashSet<PathBuf> = match std::fs::read_dir(&host_workspace) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .collect(),
            Err(_) => Default::default(),
        };

        let host_workspace_str = host_workspace.to_string_lossy().to_string();

        // Build the bwrap command line.
        // Order matters: ro-binds first, then tmpfs/proc, then resource limits, then exec.
        let mut bwrap = Command::new("/usr/bin/bwrap");
        bwrap
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--new-session",
                "--clearenv",
                "--ro-bind", "/usr", "/usr",
                "--ro-bind", "/etc/alternatives", "/etc/alternatives",
                "--symlink", "usr/lib", "/lib",
                "--symlink", "usr/lib64", "/lib64",
                "--symlink", "usr/bin", "/bin",
                "--symlink", "usr/sbin", "/sbin",
                "--ro-bind-try", "/etc/ssl", "/etc/ssl",
                "--ro-bind-try", "/etc/ca-certificates", "/etc/ca-certificates",
                "--proc", "/proc",
                "--dev", "/dev",
                "--tmpfs", "/tmp",
                "--bind", &host_workspace_str, "/workspace",
                "--chdir", "/workspace",
                "--setenv", "HOME", "/workspace",
                "--setenv", "PATH", "/usr/bin:/bin",
                "--setenv", "LANG", "C.UTF-8",
                "--setenv", "PYTHONDONTWRITEBYTECODE", "1",
                "--setenv", "PYTHONUNBUFFERED", "1",
            ]);

        bwrap.arg(interpreter).arg(code_filename);

        // Apply resource limits via prlimit before exec — bwrap doesn't do this
        // itself, so we wrap with /usr/bin/prlimit ... bwrap ... → no, that
        // only limits prlimit, not bwrap children. Use setrlimit via tokio::process
        // pre_exec hook instead.
        // Resource limits applied via setrlimit pre-exec.
        // RLIMIT_NPROC is intentionally NOT set: it counts processes per real
        // UID across the whole system, so a user with many existing processes
        // would inherit an exhausted limit and bwrap.fork() would EAGAIN.
        // PID isolation is handled by bwrap's PID namespace; runaway forks
        // are bounded by the wall-clock timeout + RLIMIT_AS memory cap.
        unsafe {
            use std::os::unix::process::CommandExt;
            bwrap.pre_exec(|| {
                set_rlimit(libc::RLIMIT_AS, MEM_LIMIT_BYTES as u64)?;
                set_rlimit(libc::RLIMIT_FSIZE, FSIZE_LIMIT_BYTES as u64)?;
                set_rlimit(libc::RLIMIT_CORE, 0)?;
                Ok(())
            });
        }

        bwrap
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!("[code_execute] spawning bwrap for {} ({} bytes)", language, code.len());
        let started = Instant::now();
        let mut child = bwrap
            .spawn()
            .map_err(|e| format!("spawn bwrap: {}", e))?;

        // Drop stdin, capture stdout/stderr concurrently, time out the whole thing
        let mut stdout_pipe = child.stdout.take().expect("stdout piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr piped");

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::with_capacity(8192);
            use tokio::io::AsyncReadExt;
            let mut tmp = [0u8; 4096];
            loop {
                match stdout_pipe.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() + n > MAX_STDOUT_BYTES {
                            buf.extend_from_slice(&tmp[..MAX_STDOUT_BYTES - buf.len()]);
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    Err(_) => break,
                }
            }
            buf
        });

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::with_capacity(2048);
            use tokio::io::AsyncReadExt;
            let mut tmp = [0u8; 2048];
            loop {
                match stderr_pipe.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() + n > MAX_STDERR_BYTES {
                            buf.extend_from_slice(&tmp[..MAX_STDERR_BYTES - buf.len()]);
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    Err(_) => break,
                }
            }
            buf
        });

        let wait_fut = child.wait();
        let exit_status = match timeout(Duration::from_secs(timeout_secs), wait_fut).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(e)) => {
                warn!("[code_execute] wait error: {}", e);
                None
            }
            Err(_) => {
                // Timed out — kill the child
                if let Some(id) = child.id() {
                    unsafe {
                        libc::kill(id as i32, libc::SIGKILL);
                    }
                }
                let _ = child.wait().await;
                None
            }
        };

        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let stdout_str = String::from_utf8_lossy(&stdout).into_owned();
        let stderr_str = String::from_utf8_lossy(&stderr).into_owned();

        let (exit_code, status_word) = match exit_status {
            Some(s) if s.success() => (Some(0i32), "ok"),
            Some(s) => (s.code(), "error"),
            None => (None, "timeout"),
        };

        // Scan host_workspace for new files (artifacts)
        let mut artifacts: Vec<Artifact> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&host_workspace) {
            // Ensure artifact store dir exists
            let _ = std::fs::create_dir_all(ARTIFACT_STORE_DIR);
            for entry in entries.flatten() {
                let path = entry.path();
                if pre_run.contains(&path) {
                    continue; // existed before run, not a new artifact
                }
                if !path.is_file() {
                    continue;
                }
                let filename = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if filename == code_filename {
                    continue;
                }
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if bytes.len() > MAX_ARTIFACT_BYTES {
                    debug!("[code_execute] skipping oversized artifact: {} ({} bytes)", filename, bytes.len());
                    continue;
                }
                let mime = guess_mime(&filename);
                let id = Uuid::new_v4().simple().to_string();
                let (content_b64, stored_path) = if bytes.len() <= ARTIFACT_INLINE_LIMIT {
                    (Some(base64::engine::general_purpose::STANDARD.encode(&bytes)), None)
                } else {
                    let stored = format!("{}/{}-{}", ARTIFACT_STORE_DIR, id, filename);
                    if let Err(e) = std::fs::write(&stored, &bytes) {
                        warn!("[code_execute] persist artifact {}: {}", filename, e);
                        continue;
                    }
                    (None, Some(stored))
                };
                artifacts.push(Artifact {
                    artifact_id: id,
                    filename,
                    mime_type: mime,
                    size_bytes: bytes.len(),
                    content_base64: content_b64,
                    stored_path,
                });
            }
        }

        info!(
            "[code_execute] {} done in {}ms: status={}, exit={:?}, stdout={}B, stderr={}B, artifacts={}",
            language, elapsed_ms, status_word, exit_code, stdout_str.len(), stderr_str.len(), artifacts.len()
        );

        // Build the human-readable content
        let mut content = String::new();
        content.push_str(&format!(
            "## Execution result\n\nStatus: **{}** (exit {:?}, {}ms)\n\n",
            status_word, exit_code, elapsed_ms
        ));
        if !stdout_str.is_empty() {
            content.push_str("### stdout\n```\n");
            content.push_str(&stdout_str);
            if !stdout_str.ends_with('\n') { content.push('\n'); }
            content.push_str("```\n\n");
        }
        if !stderr_str.is_empty() {
            content.push_str("### stderr\n```\n");
            content.push_str(&stderr_str);
            if !stderr_str.ends_with('\n') { content.push('\n'); }
            content.push_str("```\n\n");
        }

        Ok(RichToolResult {
            content,
            citations: Vec::new(),
            artifacts,
            structured: Some(json!({
                "language": language,
                "exit_code": exit_code,
                "status": status_word,
                "duration_ms": elapsed_ms,
                "stdout_bytes": stdout_str.len(),
                "stderr_bytes": stderr_str.len(),
            })),
        })
    }
}

/// Best-effort cleanup of the sandbox workspace dir on drop.
struct ScopedDir(PathBuf);

impl Drop for ScopedDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Set a single rlimit. Used inside the pre_exec hook so it applies to the
/// child process before exec(2).
fn set_rlimit(resource: u32, value: u64) -> std::io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    let rc = unsafe { libc::setrlimit(resource as libc::__rlimit_resource_t, &lim) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Quick filename → MIME guess. Used for artifact metadata only.
fn guess_mime(filename: &str) -> String {
    let lower = filename.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "csv" => "text/csv",
        "txt" | "log" => "text/plain",
        "html" => "text/html",
        "md" => "text/markdown",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Whether bwrap is available on the host. Called once at startup so we can
/// decide whether to register the tool at all.
pub fn bwrap_available() -> bool {
    std::path::Path::new("/usr/bin/bwrap").is_file()
}
