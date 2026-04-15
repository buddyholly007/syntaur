//! Trading bot execution log connector.
//!
//! Each `~/bots/data/{bot}/execution_log.jsonl` file is read line-by-line.
//! Each JSON line becomes one indexed document. Lines older than the
//! configured cutoff (default 30 days) are skipped.
//!
//! Document shape:
//!   external_id = "{bot}/{timestamp}/{correlation_id}"
//!   title       = "{bot} {action} {symbol} {iso_timestamp}"
//!   body        = original JSON line + a flat human-readable summary
//!   metadata    = {bot, symbol, action, side, ...}

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use log::{debug, warn};
use serde_json::{json, Value};

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const MAX_AGE_DAYS: i64 = 30;
const MAX_LINES_PER_FILE: usize = 5000;

pub struct ExecutionLogConnector {
    name: String,
    /// (bot_name, jsonl_path) pairs
    sources: Vec<(String, PathBuf)>,
}

impl ExecutionLogConnector {
    /// Auto-detect from a base bots data dir. Looks for
    /// `<base>/<bot>/execution_log.jsonl` for each common bot name.
    pub fn auto_detect(base: PathBuf) -> Self {
        let bots = ["stock-bot", "crypto-bot", "leveraged-bot", "options-bot"];
        let mut sources = Vec::new();
        for b in &bots {
            let path = base.join(b).join("execution_log.jsonl");
            if path.is_file() {
                sources.push((b.to_string(), path));
            }
        }
        Self {
            name: "execution_log".to_string(),
            sources,
        }
    }
}

impl Connector for ExecutionLogConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for ExecutionLogConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        let sources = self.sources.clone();
        tokio::task::spawn_blocking(move || -> Vec<ExternalDoc> {
            let cutoff = Utc::now() - Duration::days(MAX_AGE_DAYS);
            let mut docs = Vec::new();
            for (bot, path) in sources {
                let content = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "[execution_log] read {}: {}",
                            path.display(),
                            e
                        );
                        continue;
                    }
                };
                let lines: Vec<&str> = content.lines().rev().take(MAX_LINES_PER_FILE).collect();
                for line in lines {
                    let value: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    // Try to extract a timestamp
                    let ts_str = value
                        .get("ts")
                        .or_else(|| value.get("timestamp"))
                        .or_else(|| value.get("time"))
                        .and_then(|v| v.as_str());
                    let updated_at = ts_str
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);
                    if updated_at < cutoff {
                        continue;
                    }
                    let symbol = value.get("symbol").and_then(|v| v.as_str()).unwrap_or("?");
                    let action = value.get("event")
                        .or_else(|| value.get("action"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let side = value.get("side").and_then(|v| v.as_str()).unwrap_or("");
                    let corr = value
                        .get("correlation_id")
                        .or_else(|| value.get("corr_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| {
                            ts_str.unwrap_or("noid")
                        });
                    let external_id = format!("{}/{}", bot, corr);
                    let title = format!(
                        "{} {} {} {} {}",
                        bot,
                        action,
                        side,
                        symbol,
                        updated_at.format("%Y-%m-%d %H:%M")
                    );
                    let body = format!(
                        "{}\n\nFull record:\n{}",
                        title,
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                    docs.push(ExternalDoc {
                        source: "execution_log".to_string(),
                        external_id,
                        title,
                        body,
                        updated_at,
                        metadata: json!({
                            "bot": bot,
                            "symbol": symbol,
                            "action": action,
                            "side": side,
                        }),
                        agent_id: "shared".to_string(),
                    user_id: 0,
                    });
                }
                debug!(
                    "[execution_log] {} lines from {}",
                    docs.len(),
                    path.display()
                );
            }
            docs
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))
    }
}

#[async_trait]
impl SlimConnector for ExecutionLogConnector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String> {
        let docs = self.load_full().await?;
        Ok(docs
            .into_iter()
            .map(|d| DocIdOnly {
                external_id: d.external_id,
                updated_at: Some(d.updated_at),
            })
            .collect())
    }
}

/// Helper avoiding warnings on unused TimeZone import.
#[allow(dead_code)]
fn _utc_from_secs(s: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(s, 0).single().unwrap_or_else(Utc::now)
}
