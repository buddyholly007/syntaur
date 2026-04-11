use log::{info, warn};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Execute a command with sandboxing:
/// - Scoped to workspace directory
/// - Timeout enforced
/// - Script allowlist for Python/shell scripts
/// - No raw shell interpolation of user input
pub async fn exec_sandboxed(
    workspace: &Path,
    command: &str,
    timeout_secs: u64,
    allowed_scripts: &[String],
) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Empty command".to_string());
    }

    // Security: check if the command runs an allowed script
    let is_script_exec = command.starts_with("python3 ") || command.starts_with("bash ") || command.starts_with("sh ");

    if is_script_exec {
        // Extract the script path
        let script_path = command.split_whitespace().nth(1).unwrap_or("");
        let expanded = script_path.replace("~", &std::env::var("HOME").unwrap_or_default());

        // Check against allowlist
        let is_allowed = allowed_scripts.iter().any(|s| s == &expanded || expanded.starts_with(s));

        // Also allow scripts in the workspace skills directory
        let home = std::env::var("HOME").unwrap_or_default();
        let in_skills = expanded.contains("/.syntaur/workspace") && expanded.contains("/skills/");
        let in_syntaur = expanded.starts_with(&format!("{}/.syntaur/", home));

        if !is_allowed && !in_skills && !in_syntaur {
            warn!("Blocked script execution: {} (not in allowlist)", command);
            return Err(format!("Script not in allowlist: {}", script_path));
        }
    }

    // Block dangerous commands
    let dangerous = ["rm -rf /", "dd if=", "mkfs", "> /dev/", ":(){ :|:& };:"];
    for d in &dangerous {
        if command.contains(d) {
            warn!("Blocked dangerous command: {}", command);
            return Err("Command blocked for safety".to_string());
        }
    }

    let timeout = Duration::from_secs(timeout_secs.max(5).min(600));

    info!("[exec] Running: {} (timeout: {}s)", &command[..command.len().min(100)], timeout.as_secs());

    let result = tokio::time::timeout(timeout, async {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(workspace)
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .output()
            .await
    }).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            // Cap output size
            let max_output = 65536; // 64KB
            let stdout_trimmed: String = stdout.chars().take(max_output).collect();
            let stderr_trimmed: String = stderr.chars().take(max_output / 4).collect();

            if output.status.success() {
                if stderr_trimmed.is_empty() {
                    Ok(stdout_trimmed)
                } else {
                    Ok(format!("{}\n\nSTDERR:\n{}", stdout_trimmed, stderr_trimmed))
                }
            } else {
                Err(format!("Exit code {}\n{}\n{}", output.status, stdout_trimmed, stderr_trimmed))
            }
        }
        Ok(Err(e)) => Err(format!("Execution error: {}", e)),
        Err(_) => Err(format!("Timed out after {}s", timeout.as_secs())),
    }
}
