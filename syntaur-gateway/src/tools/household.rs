//! Household status tools — read-only views into Sean's infrastructure.
//!
//! These tools query existing Rust services and state files to give Peter
//! visibility into the trading bots, Crimson Lantern social accounts,
//! system health, and financial ledger. All run locally on syntaur-server
//! (where the services live) — no SSH or remote calls needed.
//!
//! ## Tools
//!
//! - `bot_status` — trading bot health from rust-bot-monitor's state
//! - `system_health` — LLM endpoint + service health from rust-health-check logs
//! - `ledger_query` — account balances and recent transactions from rust-ledger API

use async_trait::async_trait;
use log::info;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

// ── Bot Status ─────────────────────────────────────────────────────────────

pub struct BotStatusTool;

#[async_trait]
impl Tool for BotStatusTool {
    fn name(&self) -> &str {
        "bot_status"
    }

    fn description(&self) -> &str {
        "Check the status of the trading bots (stock, crypto, leveraged, options). \
         Shows which bots are running, their last trade, PnL, and any alerts from \
         the bot monitor."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bot": {
                    "type": "string",
                    "enum": ["all", "stock", "crypto", "leveraged", "options"],
                    "description": "Which bot to check. Default: all."
                }
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::default() // read-only, local
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let bot = args
            .get("bot")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        // Read the health state from rust-bot-monitor's output
        let health_path = "/tmp/syntaur/health_state.json";
        let health: Value = std::fs::read_to_string(health_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        // Read the health check log for recent entries
        let log_path = "/tmp/syntaur/health_check.log";
        let log_tail = std::fs::read_to_string(log_path)
            .ok()
            .map(|s| {
                let lines: Vec<&str> = s.lines().collect();
                let start = lines.len().saturating_sub(15);
                lines[start..].join("\n")
            })
            .unwrap_or_else(|| "(no health check log found)".to_string());

        // Also check which tmux sessions exist
        let tmux_output = tokio::process::Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name} #{session_activity}"])
            .output()
            .await
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        let bot_sessions: Vec<&str> = tmux_output
            .lines()
            .filter(|l| {
                let name = l.split_whitespace().next().unwrap_or("");
                match bot {
                    "all" => {
                        name == "stock_bot"
                            || name == "crypto_bot"
                            || name == "leveraged_bot"
                            || name == "options_bot"
                    }
                    "stock" => name == "stock_bot",
                    "crypto" => name == "crypto_bot",
                    "leveraged" => name == "leveraged_bot",
                    "options" => name == "options_bot",
                    _ => false,
                }
            })
            .collect();

        let mut parts = Vec::new();

        if bot_sessions.is_empty() {
            parts.push(format!(
                "No {} bot tmux sessions found.",
                if bot == "all" { "trading" } else { bot }
            ));
        } else {
            parts.push(format!(
                "Running bot sessions: {}",
                bot_sessions
                    .iter()
                    .map(|s| s.split_whitespace().next().unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // Add health state summary if available
        if let Some(obj) = health.as_object() {
            if !obj.is_empty() {
                let status_str = serde_json::to_string_pretty(&health)
                    .unwrap_or_default();
                parts.push(format!("Health state:\n{}", status_str.chars().take(500).collect::<String>()));
            }
        }

        // Add recent log entries
        if !log_tail.trim().is_empty() {
            parts.push(format!(
                "Recent health check log:\n{}",
                log_tail.chars().take(800).collect::<String>()
            ));
        }

        info!("[bot_status] queried for bot={}", bot);
        Ok(RichToolResult::text(parts.join("\n\n")))
    }
}

// ── System Health ──────────────────────────────────────────────────────────

pub struct SystemHealthTool;

#[async_trait]
impl Tool for SystemHealthTool {
    fn name(&self) -> &str {
        "system_health"
    }

    fn description(&self) -> &str {
        "Check the health of the home infrastructure: LLM endpoints (TurboQuant, \
         OpenRouter), services (syntaur, bot monitor, telegram gateway), and \
         system resources."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::default()
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let mut parts = Vec::new();

        // Read rust-health-check's latest output
        let health_log = "/tmp/syntaur/health_check.log";
        if let Ok(content) = std::fs::read_to_string(health_log) {
            let lines: Vec<&str> = content.lines().collect();
            let recent = &lines[lines.len().saturating_sub(10)..];
            parts.push(format!("Health check:\n{}", recent.join("\n")));
        } else {
            parts.push("Health check log not found.".to_string());
        }

        // Check key systemd services
        let services = [
            "syntaur",
            "rust-bot-monitor",
            "syntaur-watchdog",
            "rust-stream-hub",
        ];
        let mut service_status = Vec::new();
        for svc in &services {
            let output = tokio::process::Command::new("systemctl")
                .args(["--user", "is-active", svc])
                .output()
                .await;
            let status = output
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            service_status.push(format!("{}: {}", svc, status));
        }
        parts.push(format!("Services:\n{}", service_status.join("\n")));

        // System uptime + load
        if let Ok(uptime) = std::fs::read_to_string("/proc/uptime") {
            let secs: f64 = uptime
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let days = (secs / 86400.0) as u64;
            let hours = ((secs % 86400.0) / 3600.0) as u64;
            parts.push(format!("Uptime: {}d {}h", days, hours));
        }

        if let Ok(loadavg) = std::fs::read_to_string("/proc/loadavg") {
            let loads: Vec<&str> = loadavg.split_whitespace().take(3).collect();
            parts.push(format!("Load: {}", loads.join(" ")));
        }

        info!("[system_health] queried");
        Ok(RichToolResult::text(parts.join("\n\n")))
    }
}

// ── Ledger Query ───────────────────────────────────────────────────────────

pub struct LedgerQueryTool;

#[async_trait]
impl Tool for LedgerQueryTool {
    fn name(&self) -> &str {
        "ledger_query"
    }

    fn description(&self) -> &str {
        "Query the financial ledger (rust-ledger) for account balances, recent \
         transactions, or expense summaries. The ledger tracks personal + business \
         finances (Cherry Woodworks), receipts, and tax categories."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "enum": ["balances", "recent_transactions", "expense_summary"],
                    "description": "What to query: current balances, recent transactions, or expense summary by category."
                },
                "entity": {
                    "type": "string",
                    "enum": ["personal", "cherry_woodworks", "all"],
                    "description": "Which entity's data. Default: all."
                },
                "limit": {
                    "type": "integer",
                    "description": "For recent_transactions: how many to show. Default: 10."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("balances");
        let entity = args.get("entity").and_then(|v| v.as_str()).unwrap_or("all");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);

        let client = ctx.http.as_ref().ok_or("no HTTP client")?;
        let base = "http://127.0.0.1:18790"; // rust-ledger on syntaur-server

        match query {
            "balances" => {
                let url = format!("{}/api/accounts?entity={}", base, entity);
                let resp = client
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                    .map_err(|e| format!("ledger API: {}", e))?;

                if !resp.status().is_success() {
                    return Err(format!("ledger API: HTTP {}", resp.status()));
                }

                let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
                let accounts = body.as_array().cloned().unwrap_or_default();

                if accounts.is_empty() {
                    return Ok(RichToolResult::text("No accounts found in ledger."));
                }

                let lines: Vec<String> = accounts
                    .iter()
                    .take(20)
                    .filter_map(|a| {
                        let name = a.get("name").and_then(|v| v.as_str())?;
                        let balance = a.get("balance").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        if balance.abs() < 0.01 {
                            return None;
                        }
                        Some(format!("  {}: ${:.2}", name, balance))
                    })
                    .collect();

                info!("[ledger] queried balances for {}", entity);
                Ok(RichToolResult::text(format!(
                    "Account balances ({}):\n{}",
                    entity,
                    if lines.is_empty() {
                        "(all zero)".to_string()
                    } else {
                        lines.join("\n")
                    }
                )))
            }
            "recent_transactions" => {
                let url = format!(
                    "{}/api/transactions?entity={}&limit={}",
                    base, entity, limit
                );
                let resp = client
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                    .map_err(|e| format!("ledger API: {}", e))?;

                if !resp.status().is_success() {
                    return Err(format!("ledger API: HTTP {}", resp.status()));
                }

                let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
                let txns = body.as_array().cloned().unwrap_or_default();

                if txns.is_empty() {
                    return Ok(RichToolResult::text("No recent transactions."));
                }

                let lines: Vec<String> = txns
                    .iter()
                    .map(|t| {
                        let date = t.get("date").and_then(|v| v.as_str()).unwrap_or("?");
                        let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("?");
                        let amount = t.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        format!("  {} — {} ${:.2}", date, desc, amount.abs())
                    })
                    .collect();

                Ok(RichToolResult::text(format!(
                    "Recent transactions ({}):\n{}",
                    entity,
                    lines.join("\n")
                )))
            }
            "expense_summary" => {
                let url = format!("{}/api/expense-summary?entity={}", base, entity);
                let resp = client
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await;

                match resp {
                    Ok(r) if r.status().is_success() => {
                        let body: Value =
                            r.json().await.map_err(|e| format!("parse: {}", e))?;
                        Ok(RichToolResult::text(format!(
                            "Expense summary ({}):\n{}",
                            entity,
                            serde_json::to_string_pretty(&body)
                                .unwrap_or_default()
                                .chars()
                                .take(1500)
                                .collect::<String>()
                        )))
                    }
                    Ok(r) => Err(format!("ledger API: HTTP {}", r.status())),
                    Err(e) => Err(format!(
                        "rust-ledger not reachable at {} — is the service running? Error: {}",
                        base, e
                    )),
                }
            }
            other => Err(format!("ledger_query: unknown query '{}'", other)),
        }
    }
}
