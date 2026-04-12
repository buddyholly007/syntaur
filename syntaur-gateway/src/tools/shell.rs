use log::{info, warn};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Execute a command with sandboxing:
/// - Scoped to workspace directory
/// - Timeout enforced
/// - Script allowlist for Python/shell scripts
/// - `mode`: `"argv"` = split with shell_words + exec directly (no shell);
///           `"shell"` = pass through `sh -c` (legacy, less safe).
pub async fn exec_sandboxed(
    workspace: &Path,
    command: &str,
    timeout_secs: u64,
    allowed_scripts: &[String],
    mode: &str,
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

    let timeout = Duration::from_secs(timeout_secs.max(5).min(600));

    info!("[exec] Running (mode={}): {} (timeout: {}s)", mode, &command[..command.len().min(100)], timeout.as_secs());

    let result = tokio::time::timeout(timeout, async {
        match mode {
            "argv" => {
                let parts = shell_words::split(command)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                if parts.is_empty() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"));
                }
                Command::new(&parts[0])
                    .args(&parts[1..])
                    .current_dir(workspace)
                    .env("HOME", std::env::var("HOME").unwrap_or_default())
                    .output()
                    .await
            }
            _ => {
                // "shell" mode — legacy sh -c pass-through
                Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(workspace)
                    .env("HOME", std::env::var("HOME").unwrap_or_default())
                    .output()
                    .await
            }
        }
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
