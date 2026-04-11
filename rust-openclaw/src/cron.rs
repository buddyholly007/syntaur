use chrono::{DateTime, Utc};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

use crate::config::Config;
use crate::llm::{ChatMessage, LlmChain};

// ── Cron Job Schema (matches Syntaur's jobs.json) ──────────────────────────

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CronFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

fn default_version() -> u32 { 1 }

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CronDelivery {
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub mode: String,
    pub to: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct CronJob {
    pub id: String,
    pub schedule: CronSchedule,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub description: String,
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "wakeMode")]
    pub wake_mode: String,
    pub payload: CronPayload,
    pub delivery: Option<CronDelivery>,
    pub state: CronJobState,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for CronJob {
    fn default() -> Self {
        Self {
            id: String::new(),
            schedule: CronSchedule::default(),
            agent_id: String::new(),
            description: String::new(),
            name: String::new(),
            enabled: true,
            wake_mode: "now".to_string(),
            payload: CronPayload::default(),
            delivery: None,
            state: CronJobState::default(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CronSchedule {
    pub kind: String,
    pub expr: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CronPayload {
    pub kind: String,
    pub message: String,
    pub text: String,
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: u64,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CronJobState {
    #[serde(rename = "nextRunAtMs")]
    pub next_run_at_ms: u64,
    #[serde(rename = "lastRunAtMs")]
    pub last_run_at_ms: u64,
    #[serde(rename = "lastRunStatus")]
    pub last_run_status: String,
    #[serde(rename = "lastStatus")]
    pub last_status: String,
    #[serde(rename = "lastDurationMs")]
    pub last_duration_ms: u64,
    #[serde(rename = "consecutiveErrors")]
    pub consecutive_errors: u32,
    #[serde(rename = "lastDeliveryStatus")]
    pub last_delivery_status: String,
    #[serde(rename = "lastDelivered")]
    pub last_delivered: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ── Cron Expression Parser ──────────────────────────────────────────────────

/// Simple cron expression matcher (minute hour dom month dow)
pub fn cron_matches_now(expr: &str) -> bool {
    let now = Utc::now();
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }

    let minute = now.format("%M").to_string().parse::<u32>().unwrap_or(0);
    let hour = now.format("%H").to_string().parse::<u32>().unwrap_or(0);
    let dom = now.format("%d").to_string().parse::<u32>().unwrap_or(0);
    let month = now.format("%m").to_string().parse::<u32>().unwrap_or(0);
    let dow = now.format("%u").to_string().parse::<u32>().unwrap_or(0); // 1=Mon, 7=Sun
    // Cron uses 0=Sun, 1=Mon..6=Sat
    let dow_cron = if dow == 7 { 0 } else { dow };

    field_matches(fields[0], minute)
        && field_matches(fields[1], hour)
        && field_matches(fields[2], dom)
        && field_matches(fields[3], month)
        && field_matches(fields[4], dow_cron)
}

fn field_matches(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }

    // Handle comma-separated values: "16,20"
    for part in field.split(',') {
        let part = part.trim();

        // Handle step: "*/15"
        if let Some(step_str) = part.strip_prefix("*/") {
            if let Ok(step) = step_str.parse::<u32>() {
                if step > 0 && value % step == 0 {
                    return true;
                }
            }
            continue;
        }

        // Handle range: "1-5"
        if part.contains('-') {
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() == 2 {
                if let (Ok(start), Ok(end)) = (range_parts[0].parse::<u32>(), range_parts[1].parse::<u32>()) {
                    if value >= start && value <= end {
                        return true;
                    }
                }
            }
            continue;
        }

        // Simple number
        if let Ok(num) = part.parse::<u32>() {
            if value == num {
                return true;
            }
        }
    }

    false
}

// ── Script Extractor ────────────────────────────────────────────────────────

/// Extract the Python script command from the job's message/text
pub fn extract_script_command(job: &CronJob) -> Option<String> {
    let msg = if !job.payload.message.is_empty() {
        &job.payload.message
    } else if !job.payload.text.is_empty() {
        &job.payload.text
    } else {
        return None;
    };

    // Look for "python3 /path/to/script.py" pattern
    let re = regex::Regex::new(r"python3\s+(~?/\S+\.py)").ok()?;
    if let Some(caps) = re.captures(msg) {
        let path = caps[1].replace("~", &std::env::var("HOME").unwrap_or_default());
        return Some(format!("python3 {}", path));
    }

    // Look for "Execute: command" pattern
    let re2 = regex::Regex::new(r"(?i)execute:\s*(.+?)(?:\n|$)").ok()?;
    if let Some(caps) = re2.captures(msg) {
        return Some(caps[1].trim().to_string());
    }

    None
}

// ── Agent Turn Check ───────────────────────────────────────────────────────

/// Check if this job should use agent-turn mode (LLM + tools) instead of script execution.
/// Returns the prompt to send to the LLM, or None if script execution should be used.
fn agent_turn_prompt(job: &CronJob) -> Option<String> {
    let kind = job.payload.kind.as_str();
    if kind != "agentTurn" && kind != "systemEvent" {
        return None;
    }

    // If we can extract a script command, prefer script execution (faster, no LLM cost)
    if extract_script_command(job).is_some() {
        return None;
    }

    // Use message field first, then text field as the prompt
    let prompt = if !job.payload.message.is_empty() {
        job.payload.message.clone()
    } else if !job.payload.text.is_empty() {
        job.payload.text.clone()
    } else {
        return None;
    };

    Some(prompt)
}

// ── Agent Turn Execution ───────────────────────────────────────────────────

/// Execute a cron job in agent-turn mode: send the prompt to the LLM with tools
/// and let it work through tool calls to complete the task.
async fn execute_agent_turn(
    job: &CronJob,
    prompt: &str,
    config: &Config,
    mcp: Option<Arc<crate::mcp::McpRegistry>>,
    rate_limiter: Option<Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>>,
    circuit_breakers: Option<
        Arc<tokio::sync::Mutex<HashMap<String, crate::circuit_breaker::CircuitBreaker>>>,
    >,
) -> JobResult {
    let start = Instant::now();
    let agent_id = &job.agent_id;

    let timeout_secs = if job.payload.timeout_seconds > 0 {
        job.payload.timeout_seconds
    } else {
        300
    };

    info!("[cron:{}] Agent-turn mode for agent '{}' (timeout: {}s)", job.id, agent_id, timeout_secs);
    info!("[cron:{}] Prompt: {}...", job.id, &prompt[..prompt.len().min(120)]);

    // Build LLM chain for this agent
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();
    let llm_chain = LlmChain::from_config(config, agent_id, client);

    // Load system prompt from workspace files
    let workspace = config.agent_workspace(agent_id);
    let mut context_parts = Vec::new();

    for file in &["SOUL.md", "IDENTITY.md", "TOOLS.md", "USER.md", "BRIEF.md", "PLAN.md", "MEMORY.md"] {
        if let Ok(content) = std::fs::read_to_string(workspace.join(file)) {
            if !content.trim().is_empty() {
                context_parts.push(content);
            }
        }
    }

    // Load today's memory
    let today = Utc::now().format("%Y-%m-%d").to_string();
    if let Ok(memory) = std::fs::read_to_string(workspace.join("memory").join(format!("{}.md", today))) {
        if !memory.trim().is_empty() {
            context_parts.push(format!("[Today's memory]\n{}", memory));
        }
    }

    // Load PENDING_TASKS.md
    if let Ok(tasks) = std::fs::read_to_string(workspace.join("PENDING_TASKS.md")) {
        if !tasks.trim().is_empty() {
            context_parts.push(format!("[Pending tasks]\n{}", tasks));
        }
    }

    let system_prompt = if context_parts.is_empty() {
        format!("You are agent '{}'. You are running as an automated cron job. Complete the requested task using your tools.", agent_id)
    } else {
        let mut parts = context_parts;
        parts.push(format!("[Context: You are running as an automated cron job '{}'. Complete the task and report results concisely.]", job.id));
        parts.join("\n\n---\n\n")
    };

    // Build messages
    let mut messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(prompt),
    ];

    // Get tool definitions and registry
    let mut tool_registry = crate::tools::ToolRegistry::with_mcp(workspace.clone(), mcp);
    if let (Some(rl), Some(cbs)) = (rate_limiter, circuit_breakers) {
        tool_registry.set_infra(rl, cbs);
    }
    let tools = tool_registry.tool_definitions();
    let max_rounds = 30;

    // Run with timeout
    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        async {
            for round in 0..max_rounds {
                let result = match llm_chain.call_raw(&messages, Some(&tools)).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("[cron:{}] LLM error (round {}): {}", job.id, round, e);
                        return JobResult {
                            success: false,
                            output: format!("LLM error: {}", e),
                            duration_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                };

                match result {
                    crate::llm::LlmResult::Text(text) => {
                        info!("[cron:{}] Agent-turn completed in round {} ({} chars)", job.id, round, text.len());
                        return JobResult {
                            success: true,
                            output: text.chars().take(2000).collect(),
                            duration_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                    crate::llm::LlmResult::ToolCalls { content, tool_calls } => {
                        info!("[cron:{}] Round {}: {} tool call(s)", job.id, round, tool_calls.len());

                        messages.push(ChatMessage::assistant_with_tools(&content, tool_calls.clone()));

                        for tc in &tool_calls {
                            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let func = tc.get("function").cloned().unwrap_or_default();
                            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let args_str = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                            let args: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));

                            info!("[cron:{}] Tool: {}({})", job.id, name, &args_str[..args_str.len().min(100)]);

                            let tool_call = crate::tools::ToolCall { id: id.clone(), name: name.clone(), arguments: args };
                            let result = tool_registry.execute(&tool_call).await;

                            info!("[cron:{}] Tool {} result: success={}, {} chars", job.id, name, result.success, result.output.len());

                            let mut output = result.output;
                            if output.len() > 1500 {
                                let truncate_at = output.floor_char_boundary(1200);
                                output = format!("{}...\n[truncated — {} chars total]", &output[..truncate_at], output.len());
                            }

                            let remaining = max_rounds - round - 1;
                            if remaining <= 8 && remaining > 0 {
                                output.push_str(&format!("\n\n[Round {}/{} — {} remaining. Finish your task or report status.]", round + 1, max_rounds, remaining));
                            }

                            messages.push(ChatMessage::tool_result(&id, &output));
                        }
                    }
                }
            }

            // Hit max rounds — force text response
            warn!("[cron:{}] Max tool rounds reached, forcing text", job.id);
            messages.push(ChatMessage::system("Respond with text now. No more tools. Summarize what you accomplished."));
            match llm_chain.call(&messages).await {
                Ok(text) => JobResult {
                    success: true,
                    output: text.chars().take(2000).collect(),
                    duration_ms: start.elapsed().as_millis() as u64,
                },
                Err(e) => JobResult {
                    success: false,
                    output: format!("Final LLM call failed: {}", e),
                    duration_ms: start.elapsed().as_millis() as u64,
                },
            }
        }
    ).await;

    match result {
        Ok(job_result) => job_result,
        Err(_) => {
            error!("[cron:{}] Agent-turn timed out after {}s", job.id, timeout_secs);
            JobResult {
                success: false,
                output: format!("Agent-turn timed out after {}s", timeout_secs),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
    }
}

// ── Job Execution ───────────────────────────────────────────────────────────

pub struct JobResult {
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

pub async fn execute_job(job: &CronJob) -> JobResult {
    let start = Instant::now();

    let cmd = match extract_script_command(job) {
        Some(c) => c,
        None => {
            // No extractable command — check if this is an agent-turn job
            // (caller should have handled agent-turn before reaching here)
            warn!("[cron:{}] No script command found in payload (use agent-turn mode)", job.id);
            return JobResult {
                success: false,
                output: "No script command found and agent-turn not handled".to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let timeout = Duration::from_secs(if job.payload.timeout_seconds > 0 {
        job.payload.timeout_seconds
    } else {
        300 // default 5 min
    });

    info!("[cron:{}] Executing: {} (timeout: {}s)", job.id, cmd, timeout.as_secs());

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let result = tokio::time::timeout(timeout, async {
        Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("HOME", &home)
            .current_dir(&home)
            .output()
            .await
    })
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let success = output.status.success();

            if success {
                info!("[cron:{}] Completed in {}ms", job.id, duration_ms);
            } else {
                warn!("[cron:{}] Failed (exit {}): {}", job.id, output.status, stderr.chars().take(200).collect::<String>());
            }

            JobResult {
                success,
                output: if success {
                    stdout.chars().take(1000).collect()
                } else {
                    format!("STDERR: {}", stderr.chars().take(500).collect::<String>())
                },
                duration_ms,
            }
        }
        Ok(Err(e)) => {
            error!("[cron:{}] Exec error: {}", job.id, e);
            JobResult {
                success: false,
                output: format!("Exec error: {}", e),
                duration_ms,
            }
        }
        Err(_) => {
            error!("[cron:{}] Timed out after {}s", job.id, timeout.as_secs());
            JobResult {
                success: false,
                output: format!("Timed out after {}s", timeout.as_secs()),
                duration_ms,
            }
        }
    }
}

// ── Scheduler ───────────────────────────────────────────────────────────────

pub struct CronScheduler {
    jobs_path: PathBuf,
    running: HashMap<String, bool>,
    telegram_tokens: HashMap<String, String>, // accountId -> bot token
    delivery_client: reqwest::Client,
    config: Option<Arc<Config>>,
    mcp: Option<Arc<crate::mcp::McpRegistry>>,
    rate_limiter: Option<Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>>,
    circuit_breakers: Option<
        Arc<tokio::sync::Mutex<HashMap<String, crate::circuit_breaker::CircuitBreaker>>>,
    >,
}

impl CronScheduler {
    pub fn new(jobs_path: PathBuf) -> Self {
        Self {
            jobs_path,
            running: HashMap::new(),
            telegram_tokens: HashMap::new(),
            delivery_client: reqwest::Client::new(),
            config: None,
            mcp: None,
            rate_limiter: None,
            circuit_breakers: None,
        }
    }

    pub fn set_mcp(&mut self, mcp: Arc<crate::mcp::McpRegistry>) {
        self.mcp = Some(mcp);
    }

    /// Wire shared tool infrastructure (rate limiter + circuit breakers) so
    /// agent-turn cron jobs participate in the same uniform funnel as
    /// /api/message and Telegram-driven calls. v5 Item 1 Stage 4.
    pub fn set_tool_infra(
        &mut self,
        rate_limiter: Arc<tokio::sync::Mutex<crate::rate_limit::RateLimiter>>,
        circuit_breakers: Arc<
            tokio::sync::Mutex<HashMap<String, crate::circuit_breaker::CircuitBreaker>>,
        >,
    ) {
        self.rate_limiter = Some(rate_limiter);
        self.circuit_breakers = Some(circuit_breakers);
    }

    pub fn set_config(&mut self, config: Arc<Config>) {
        self.config = Some(config);
    }

    pub fn set_telegram_tokens(&mut self, tokens: HashMap<String, String>) {
        self.telegram_tokens = tokens;
    }

    pub fn load_jobs(&self) -> Vec<CronJob> {
        match std::fs::read_to_string(&self.jobs_path) {
            Ok(content) => {
                match serde_json::from_str::<CronFile>(&content) {
                    Ok(f) => f.jobs,
                    Err(e) => {
                        error!("Failed to parse cron jobs: {} — keeping file unchanged", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                warn!("Cannot read cron jobs from {}: {}", self.jobs_path.display(), e);
                Vec::new()
            }
        }
    }

    fn save_jobs(&self, jobs: &[CronJob]) {
        if jobs.is_empty() {
            // Never overwrite jobs.json with an empty list — likely a parse failure
            warn!("Skipping save_jobs — refusing to overwrite with empty job list");
            return;
        }
        let file = CronFile {
            version: 1,
            jobs: jobs.to_vec(),
        };
        match serde_json::to_string_pretty(&file) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.jobs_path, &json) {
                    error!("Failed to save cron jobs: {}", e);
                }
            }
            Err(e) => error!("Failed to serialize cron jobs: {}", e),
        }
    }

    /// Check and run any jobs that are due
    pub async fn tick(&mut self) {
        let mut jobs = self.load_jobs();
        let now_ms = Utc::now().timestamp_millis() as u64;

        for job in jobs.iter_mut() {
            if !job.enabled {
                continue;
            }

            // Check if cron expression matches current time
            if !cron_matches_now(&job.schedule.expr) {
                continue;
            }

            // Prevent re-running within the same minute
            let one_min_ago = now_ms.saturating_sub(60_000);
            if job.state.last_run_at_ms > one_min_ago {
                continue;
            }

            // Check if already running
            if *self.running.get(&job.id).unwrap_or(&false) {
                debug!("[cron:{}] Skipping — still running", job.id);
                continue;
            }

            // Check backoff for consecutive errors
            if job.state.consecutive_errors > 0 {
                let backoff_ms = backoff_duration(job.state.consecutive_errors);
                let since_last = now_ms.saturating_sub(job.state.last_run_at_ms);
                if since_last < backoff_ms {
                    debug!("[cron:{}] In backoff ({} errors, wait {}ms)", job.id, job.state.consecutive_errors, backoff_ms - since_last);
                    continue;
                }
            }

            // Run the job
            self.running.insert(job.id.clone(), true);
            let job_id = job.id.clone();

            // Check if this is an agent-turn job (no script, just a prompt for the LLM)
            let result = if let Some(prompt) = agent_turn_prompt(job) {
                if let Some(ref config) = self.config {
                    execute_agent_turn(
                        job,
                        &prompt,
                        config,
                        self.mcp.clone(),
                        self.rate_limiter.clone(),
                        self.circuit_breakers.clone(),
                    )
                    .await
                } else {
                    warn!("[cron:{}] Agent-turn job but no config available", job.id);
                    JobResult {
                        success: false,
                        output: "Agent-turn mode requires config (not set)".to_string(),
                        duration_ms: 0,
                    }
                }
            } else {
                execute_job(job).await
            };

            // Update state
            job.state.last_run_at_ms = Utc::now().timestamp_millis() as u64;
            job.state.last_duration_ms = result.duration_ms;

            if result.success {
                job.state.last_run_status = "ok".to_string();
                job.state.last_status = "ok".to_string();
                job.state.consecutive_errors = 0;
            } else {
                job.state.last_run_status = "error".to_string();
                job.state.last_status = format!("error: {}", result.output.chars().take(100).collect::<String>());
                job.state.consecutive_errors += 1;
            }

            self.running.insert(job_id.clone(), false);

            // Deliver result if configured
            if let Some(ref delivery) = job.delivery {
                if delivery.mode == "announce" && result.success && !delivery.to.is_empty() {
                    if let Some(token) = self.telegram_tokens.get(&delivery.account_id) {
                        let chat_id: i64 = delivery.to.parse().unwrap_or(0);
                        if chat_id != 0 {
                            let msg = format!("[Cron: {}]\n{}", job_id, &result.output[..result.output.len().min(3000)]);
                            let payload = serde_json::json!({"chat_id": chat_id, "text": msg});
                            let _ = self.delivery_client
                                .post(format!("https://api.telegram.org/bot{}/sendMessage", token))
                                .json(&payload)
                                .timeout(Duration::from_secs(15))
                                .send()
                                .await;
                            info!("[cron:{}] Delivered to {}", job_id, delivery.to);
                            job.state.last_delivery_status = "delivered".to_string();
                            job.state.last_delivered = true;
                        }
                    }
                }
            }
        }

        // Save updated state
        self.save_jobs(&jobs);
    }
}

/// Exponential backoff: 1min, 2min, 4min, 8min, 16min, cap at 30min
fn backoff_duration(consecutive_errors: u32) -> u64 {
    let base_ms = 60_000u64; // 1 minute
    let max_ms = 1_800_000u64; // 30 minutes
    let backoff = base_ms * 2u64.pow(consecutive_errors.saturating_sub(1).min(5));
    backoff.min(max_ms)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_matches() {
        assert!(field_matches("*", 5));
        assert!(field_matches("5", 5));
        assert!(!field_matches("5", 6));
        assert!(field_matches("*/15", 0));
        assert!(field_matches("*/15", 15));
        assert!(field_matches("*/15", 30));
        assert!(!field_matches("*/15", 7));
        assert!(field_matches("16,20", 16));
        assert!(field_matches("16,20", 20));
        assert!(!field_matches("16,20", 17));
        assert!(field_matches("1-5", 3));
        assert!(!field_matches("1-5", 6));
    }

    #[test]
    fn test_backoff() {
        assert_eq!(backoff_duration(1), 60_000);
        assert_eq!(backoff_duration(2), 120_000);
        assert_eq!(backoff_duration(3), 240_000);
        assert_eq!(backoff_duration(6), 1_800_000); // capped
    }
}
