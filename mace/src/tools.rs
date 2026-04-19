//! MACE tool registry. All tools execute locally on the host where the
//! binary is running — that's the whole point of keeping the tool loop out
//! of the gateway. Filesystem ops touch this host's disk, shell commands
//! run in this host's shell.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Result of a tool call. Normal tool outputs feed back into the LLM as
/// `role: tool` messages. `RequestExit` is a signal from `handoff_return`
/// that the session should wrap up — the REPL catches it, posts the summary
/// to the return conversation, and exits cleanly.
pub enum ToolOutput {
    Normal(String),
    RequestExit { summary: String },
}

/// OpenAI-style tool specs. Each `name` must match a case in `run()` below.
pub fn specs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": "Run a shell command on the local host and return combined stdout+stderr. Use for builds, tests, git, grep, system inspection. The shell is bash -lc.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The command to run."},
                        "timeout_secs": {"type": "integer", "description": "Maximum seconds before the command is killed.", "default": 60}
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file from the local filesystem.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute or relative path."}
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a UTF-8 text file on the local filesystem. Creates parent directories if needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace an exact substring in a local file. `old_string` must appear exactly once in the file, or the edit is rejected. Cheaper than write_file for small, targeted changes to large files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_string": {"type": "string", "description": "Exact text to find. Must be unique in the file."},
                        "new_string": {"type": "string", "description": "Text to replace it with."}
                    },
                    "required": ["path", "old_string", "new_string"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List directory entries on the local filesystem. Returns one entry per line (name, size for files).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ask_user",
                "description": "Ask the user a clarifying question and wait for their reply. Use when you genuinely need input to continue — don't use for rhetorical questions.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "description": "One short, specific question."}
                    },
                    "required": ["question"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "handoff_return",
                "description": "Finish the coding session and hand the user back to the main agent. Call when the user's task is resolved (or when they say they're done). Post a concise 2-4 sentence summary that the main agent can pick up from.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "summary": {"type": "string", "description": "What was asked, what you did, and the current state. Clear enough that the main agent understands without re-reading the code."}
                    },
                    "required": ["summary"]
                }
            }
        }),
    ]
}

pub async fn run(name: &str, args: &Value) -> Result<ToolOutput> {
    match name {
        "run_shell" => {
            let cmd = str_arg(args, "command").unwrap_or_default();
            let preview = format!("run_shell: {}", if cmd.len() > 140 { format!("{}…", &cmd[..139]) } else { cmd });
            if !confirm_destructive(&preview).await? {
                return Ok(ToolOutput::Normal("(cancelled by user — did not run)".into()));
            }
            run_shell(args).await.map(ToolOutput::Normal)
        }
        "read_file"     => read_file(args).await.map(ToolOutput::Normal),
        "write_file"    => {
            let path = str_arg(args, "path").unwrap_or_default();
            let len = args.get("content").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
            let preview = format!("write_file: {path}  ({len} bytes, overwrites existing)");
            if !confirm_destructive(&preview).await? {
                return Ok(ToolOutput::Normal("(cancelled by user — did not write)".into()));
            }
            write_file(args).await.map(ToolOutput::Normal)
        }
        "edit_file"     => {
            let path = str_arg(args, "path").unwrap_or_default();
            let preview = format!("edit_file: {path}  (single-match replacement)");
            if !confirm_destructive(&preview).await? {
                return Ok(ToolOutput::Normal("(cancelled by user — did not edit)".into()));
            }
            edit_file(args).await.map(ToolOutput::Normal)
        }
        "list_dir"      => list_dir(args).await.map(ToolOutput::Normal),
        "ask_user"      => ask_user(args).await.map(ToolOutput::Normal),
        "handoff_return" => {
            let summary = str_arg(args, "summary").unwrap_or_else(|_| "Maurice ended the session.".into());
            Ok(ToolOutput::RequestExit { summary })
        }
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

/// Inline confirmation prompt for tools that can change state. Skipped when
/// `MACE_CONFIRM_DESTRUCTIVE` isn't set to a truthy value — the sandboxed
/// container is the default safe environment, so the web-terminal launcher
/// only sets this on real hosts (openclawprod, gaming PC, claudevm). Reads
/// the user's reply from stdin via the same pattern as `ask_user`, so it
/// interleaves cleanly with the rest of the tool output in the TTY.
async fn confirm_destructive(preview: &str) -> Result<bool> {
    let enabled = match std::env::var("MACE_CONFIRM_DESTRUCTIVE").ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => true,
        _ => false,
    };
    if !enabled { return Ok(true); }
    let host = std::env::var("MAURICE_HOSTNAME").unwrap_or_else(|_| {
        std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "this host".into()).trim().to_string()
    });
    println!();
    if std::env::var("NO_COLOR").is_err() {
        println!("  \x1b[38;5;215m!\x1b[0m about to run on \x1b[38;5;215m{host}\x1b[0m:");
        println!("    {preview}");
        print!("  approve? [y/N] ");
    } else {
        println!("  ! about to run on {host}:");
        println!("    {preview}");
        print!("  approve? [y/N] ");
    }
    use std::io::Write;
    std::io::stdout().flush().ok();
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        line.trim().to_lowercase()
    }).await.context("read stdin")?;
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn str_arg(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .with_context(|| format!("missing required argument `{key}`"))
}

async fn run_shell(args: &Value) -> Result<String> {
    let command = str_arg(args, "command")?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60)
        .min(300);

    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn bash")?;

    let mut stdout = child.stdout.take().context("no stdout pipe")?;
    let mut stderr = child.stderr.take().context("no stderr pipe")?;

    let run = async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        let (a, b) = tokio::join!(stdout.read_to_end(&mut so), stderr.read_to_end(&mut se));
        a?; b?;
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status, so, se))
    };

    let result = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(timeout_secs), run) => r,
        _ = tokio::signal::ctrl_c() => anyhow::bail!("cancelled by Ctrl-C"),
    };
    let (status, so, se) = match result {
        Ok(r) => r?,
        Err(_) => anyhow::bail!("command exceeded {timeout_secs}s timeout"),
    };

    let mut out = String::new();
    if !so.is_empty() {
        out.push_str(&String::from_utf8_lossy(&so));
    }
    if !se.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
        out.push_str("--- stderr ---\n");
        out.push_str(&String::from_utf8_lossy(&se));
    }
    if let Some(code) = status.code() {
        if code != 0 {
            if !out.ends_with('\n') { out.push('\n'); }
            out.push_str(&format!("--- exit code: {code} ---"));
        }
    }
    Ok(truncate(out))
}

async fn read_file(args: &Value) -> Result<String> {
    let path = str_arg(args, "path")?;
    let bytes = tokio::fs::read(&path).await.with_context(|| format!("read {path}"))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(truncate(text))
}

async fn write_file(args: &Value) -> Result<String> {
    let path = str_arg(args, "path")?;
    let content = str_arg(args, "content")?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
    }
    tokio::fs::write(&path, &content).await.with_context(|| format!("write {path}"))?;
    Ok(format!("wrote {} bytes to {path}", content.len()))
}

async fn edit_file(args: &Value) -> Result<String> {
    let path = str_arg(args, "path")?;
    let old = str_arg(args, "old_string")?;
    let new = str_arg(args, "new_string")?;
    let current = tokio::fs::read_to_string(&path).await.with_context(|| format!("read {path}"))?;
    let matches: Vec<_> = current.match_indices(&old).collect();
    if matches.is_empty() {
        anyhow::bail!("old_string not found in {path} — either the file changed or the match string is off by a character");
    }
    if matches.len() > 1 {
        anyhow::bail!("old_string appears {} times in {path} — make it more specific so it matches exactly once", matches.len());
    }
    let updated = current.replacen(&old, &new, 1);
    tokio::fs::write(&path, &updated).await.with_context(|| format!("write {path}"))?;
    Ok(format!("replaced {} bytes → {} bytes in {path}", old.len(), new.len()))
}

async fn list_dir(args: &Value) -> Result<String> {
    let path = str_arg(args, "path")?;
    let mut rd = tokio::fs::read_dir(&path).await.with_context(|| format!("list {path}"))?;
    let mut lines = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = entry.file_type().await.ok();
        let is_dir = ft.map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            lines.push(format!("{name}/"));
        } else {
            let size = entry.metadata().await.ok().map(|m| m.len()).unwrap_or(0);
            lines.push(format!("{name}  {size}"));
        }
    }
    lines.sort();
    Ok(lines.join("\n"))
}

async fn ask_user(args: &Value) -> Result<String> {
    let question = str_arg(args, "question")?;
    println!();
    if std::env::var("NO_COLOR").is_err() {
        println!("  \x1b[38;5;215m?\x1b[0m {question}");
    } else {
        println!("  ? {question}");
    }
    print!("  › ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        line.trim().to_string()
    })
    .await
    .context("read stdin")?;
    if answer.is_empty() {
        Ok("(user gave no answer)".to_string())
    } else {
        Ok(answer)
    }
}

fn truncate(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        s
    } else {
        let mut out = s[..MAX_OUTPUT_BYTES].to_string();
        out.push_str(&format!("\n… truncated ({} bytes total)", s.len()));
        out
    }
}
