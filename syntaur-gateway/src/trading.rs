//! Trading module — read-only snapshot of the sibling `syntaur-trading`
//! container's 5-bot stack (stock, crypto, leveraged, options, kalshi).
//!
//! - Alpaca: live account / positions / activity / portfolio history
//! - Local: per-bot state-file mtime as heartbeat, kill-switch state
//!
//! Credentials and per-bot state come from a read-only bind mount of the
//! trading container's data dir at `/trading-data`. When the mount or the
//! `stock-bot/.env` is missing, the service is `None` and `/api/trading/*`
//! returns 503.

use axum::{extract::State, http::StatusCode, response::Json};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone)]
pub struct TradingService {
    pub trading_data: PathBuf,
    alpaca_key: String,
    alpaca_secret: String,
    alpaca_base: String,
    http: reqwest::Client,
}

impl TradingService {
    pub fn new(trading_data: PathBuf) -> Result<Self, String> {
        let env_path = trading_data.join("stock-bot").join(".env");
        let creds = parse_env_file(&env_path)
            .map_err(|e| format!("read {}: {}", env_path.display(), e))?;
        let alpaca_key = creds
            .get("APCA_API_KEY_ID")
            .cloned()
            .ok_or_else(|| "APCA_API_KEY_ID missing".to_string())?;
        let alpaca_secret = creds
            .get("APCA_API_SECRET_KEY")
            .cloned()
            .ok_or_else(|| "APCA_API_SECRET_KEY missing".to_string())?;
        let alpaca_base = creds
            .get("APCA_BASE_URL")
            .cloned()
            .ok_or_else(|| "APCA_BASE_URL missing".to_string())?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { trading_data, alpaca_key, alpaca_secret, alpaca_base, http })
    }

    async fn alpaca_get(&self, path: &str) -> Result<Value, String> {
        let r = self
            .http
            .get(format!("{}{}", self.alpaca_base, path))
            .header("APCA-API-KEY-ID", &self.alpaca_key)
            .header("APCA-API-SECRET-KEY", &self.alpaca_secret)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("alpaca {} {}", path, r.status()));
        }
        r.json::<Value>().await.map_err(|e| e.to_string())
    }

    pub async fn account(&self) -> Result<Value, String> {
        let a = self.alpaca_get("/v2/account").await?;
        let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
        let eq = s("equity");
        let last = s("last_equity");
        Ok(json!({
            "equity": eq,
            "last_equity": last,
            "day_pnl": eq - last,
            "day_pnl_pct": if last > 0.0 { (eq - last) / last * 100.0 } else { 0.0 },
            "cash": s("cash"),
            "buying_power": s("buying_power"),
            "long_market_value": s("long_market_value"),
            "status": a.get("status").and_then(|v| v.as_str()).unwrap_or("?"),
            "trading_blocked": a.get("trading_blocked").and_then(|v| v.as_bool()).unwrap_or(false),
            "account_blocked": a.get("account_blocked").and_then(|v| v.as_bool()).unwrap_or(false),
            "daytrade_count": a.get("daytrade_count"),
            "created_at": a.get("created_at"),
        }))
    }

    pub async fn positions(&self) -> Result<Value, String> {
        self.alpaca_get("/v2/positions").await
    }

    pub async fn activity(&self, limit: usize) -> Result<Value, String> {
        self.alpaca_get(&format!(
            "/v2/account/activities?activity_types=FILL&page_size={}",
            limit.min(100)
        ))
        .await
    }

    pub async fn equity_curve(&self) -> Result<Value, String> {
        self.alpaca_get("/v2/account/portfolio/history?period=1M&timeframe=1D").await
    }

    /// Per-bot heartbeat from state-file mtime + kill switch state. Pure
    /// filesystem read; no network calls.
    pub fn bots_health(&self) -> Value {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let bots: &[(&str, &str, bool)] = &[
            // (name, heartbeat file, expected_24_7)
            // crypto: oms_state.json bumps on every 60s heartbeat cycle.
            // kalshi: audit_log.jsonl appends per-market (~70s); predictions
            //         only rewrite per ~2h cycle so they look stale even
            //         when the bot is healthy.
            // equity bots: bot_state.json only mutates on signal/fill, so
            //         "stale" is gated by us_market_open_now() (4h thresh).
            ("stock_bot", "stock-bot/bot_state.json", false),
            ("crypto_bot", "crypto-bot/oms_state.json", true),
            ("leveraged_bot", "leveraged-bot/leveraged_oms_state.json", false),
            ("options_bot", "options-bot/bot_state.json", false),
            ("kalshi_bot", "kalshi/audit_log.jsonl", true),
        ];
        let mut out = Vec::new();
        for (name, rel, full_time) in bots {
            let path = self.trading_data.join(rel);
            let mtime: Option<i64> = path
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            let age = mtime.map(|t| now - t);
            // 24/7 bots: stale > 10 min. Equity bots: stale > 4h during US
            // market hours, else "idle" (not stale).
            let stale = match age {
                Some(a) if *full_time => a > 600,
                Some(a) => a > 14_400 && us_market_open_now(),
                None => true,
            };
            out.push(json!({
                "name": name,
                "state_file": rel,
                "mtime": mtime,
                "age_secs": age,
                "stale": stale,
                "exists": mtime.is_some(),
            }));
        }
        let ks_path = self.trading_data.join("kill_switch.json");
        let kill_switch: Value = std::fs::read_to_string(&ks_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({"halted": false}));
        let monitor_path = self.trading_data.join("monitor_state.json");
        let monitor_state: Value = std::fs::read_to_string(&monitor_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        json!({
            "bots": out,
            "kill_switch": kill_switch,
            "monitor_state": monitor_state,
            "now": now,
        })
    }
}

fn parse_env_file(path: &Path) -> std::io::Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)?;
    let mut out = HashMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v.strip_prefix('"').unwrap_or(v);
            let v = v.strip_suffix('"').unwrap_or(v);
            out.insert(k.trim().to_string(), v.to_string());
        }
    }
    Ok(out)
}

fn us_market_open_now() -> bool {
    use chrono::{Datelike, Timelike, Utc};
    let now = Utc::now();
    let wd = now.weekday().num_days_from_monday();
    if wd >= 5 {
        return false;
    }
    let h = now.hour();
    (13..=21).contains(&h)
}

pub mod api {
    use super::*;
    use crate::AppState;

    pub async fn handle_account(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<Value>, StatusCode> {
        let svc = state.trading.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        svc.account().await.map(Json).map_err(|e| {
            log::warn!("[trading] account: {}", e);
            StatusCode::BAD_GATEWAY
        })
    }

    pub async fn handle_positions(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<Value>, StatusCode> {
        let svc = state.trading.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        svc.positions().await.map(Json).map_err(|e| {
            log::warn!("[trading] positions: {}", e);
            StatusCode::BAD_GATEWAY
        })
    }

    pub async fn handle_activity(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<Value>, StatusCode> {
        let svc = state.trading.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        svc.activity(50).await.map(Json).map_err(|e| {
            log::warn!("[trading] activity: {}", e);
            StatusCode::BAD_GATEWAY
        })
    }

    pub async fn handle_equity(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<Value>, StatusCode> {
        let svc = state.trading.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        svc.equity_curve().await.map(Json).map_err(|e| {
            log::warn!("[trading] equity: {}", e);
            StatusCode::BAD_GATEWAY
        })
    }

    pub async fn handle_bots(State(state): State<Arc<AppState>>) -> Json<Value> {
        match state.trading.as_ref() {
            Some(s) => Json(s.bots_health()),
            None => Json(json!({
                "bots": [],
                "kill_switch": {"halted": false},
                "unavailable": true,
            })),
        }
    }
}
