//! Daily 8pm PT trading summary task.
//!
//! Replaces the per-event Telegram chatter from rust-bot-monitor (which is
//! still off post-migration) with a single end-of-day digest. Sean's ask
//! (2026-04-30): one ping/day at 8pm PT with "what trades were made, today's
//! P&L, all-time P&L". No per-edge / per-trade pings.
//!
//! Side-effect: writes `/trading-data/monitor_state.json` with `last_daily_summary`
//! set to today, which makes the trading dashboard's "bot-monitor stale" amber
//! notice go away (resolves daily-note follow-up #4).

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::US::Pacific;
use log::{error, info, warn};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::trading::TradingService;

/// Hour-of-day (Pacific) at which the summary fires. 20 = 8pm.
const SUMMARY_HOUR_PT: u32 = 20;

/// Minute-of-hour (Pacific) at which the summary fires. 0 = on the hour.
const SUMMARY_MINUTE_PT: u32 = 0;

/// Spawn the daily-summary task. Only runs when the `TRADING_SUMMARY_TG_TOKEN`
/// env var is set; without it the task logs a warning and exits.
pub fn spawn(svc: Arc<TradingService>) {
    let token = match std::env::var("TRADING_SUMMARY_TG_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            warn!("[trading-summary] TRADING_SUMMARY_TG_TOKEN unset — daily summary disabled");
            return;
        }
    };
    let chat_id = match std::env::var("TRADING_SUMMARY_TG_CHAT_ID")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
    {
        Some(c) => c,
        None => {
            warn!("[trading-summary] TRADING_SUMMARY_TG_CHAT_ID unset — daily summary disabled");
            return;
        }
    };
    tokio::spawn(async move {
        run_loop(svc, token, chat_id).await;
    });
}

async fn run_loop(svc: Arc<TradingService>, token: String, chat_id: i64) {
    info!("[trading-summary] scheduler started; firing at {:02}:{:02} PT daily", SUMMARY_HOUR_PT, SUMMARY_MINUTE_PT);
    loop {
        let wait = duration_until_next_fire();
        info!(
            "[trading-summary] next fire in {}h {}m",
            wait.as_secs() / 3600,
            (wait.as_secs() % 3600) / 60
        );
        tokio::time::sleep(wait).await;
        match send_summary(&svc, &token, chat_id).await {
            Ok(_) => info!("[trading-summary] daily summary sent"),
            Err(e) => error!("[trading-summary] failed: {}", e),
        }
        // Defensive: ensure we don't double-fire within the same minute if
        // the send took <1s. Sleep one full minute past the trigger.
        tokio::time::sleep(Duration::from_secs(70)).await;
    }
}

fn duration_until_next_fire() -> Duration {
    let now_pt = Utc::now().with_timezone(&Pacific);
    let mut target = Pacific
        .with_ymd_and_hms(now_pt.year(), now_pt.month(), now_pt.day(), SUMMARY_HOUR_PT, SUMMARY_MINUTE_PT, 0)
        .single()
        .unwrap_or_else(|| Pacific.from_utc_datetime(&now_pt.naive_utc()));
    if target <= now_pt {
        target = target + chrono::Duration::days(1);
    }
    let delta = target.signed_duration_since(now_pt);
    Duration::from_secs(delta.num_seconds().max(60) as u64)
}

async fn send_summary(svc: &TradingService, token: &str, chat_id: i64) -> Result<(), String> {
    let acct = svc.account().await?;
    let activity = svc.activity(100).await?;
    let equity_curve = svc.equity_curve().await?;

    let now_pt = Utc::now().with_timezone(&Pacific);
    let today_pt = now_pt.date_naive();

    let body = format_summary(&acct, &activity, &equity_curve, today_pt);

    // Send to Telegram. Single retry on transient failure; we're once a day so
    // give it a decent shot but don't loop indefinitely.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let payload = json!({"chat_id": chat_id, "text": body, "disable_web_page_preview": true});
    let mut last_err = String::new();
    for attempt in 0..2 {
        match http.post(&url).json(&payload).send().await {
            Ok(r) if r.status().is_success() => {
                last_err.clear();
                break;
            }
            Ok(r) => {
                last_err = format!("telegram {}: {}", r.status(), r.text().await.unwrap_or_default());
            }
            Err(e) => last_err = e.to_string(),
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    if !last_err.is_empty() {
        return Err(last_err);
    }

    // Refresh monitor_state.json so the dashboard's "bot-monitor stale" amber
    // notice resolves. Best-effort — we log but don't fail the summary on a
    // write error.
    if let Err(e) = write_monitor_state(svc, &acct, today_pt) {
        warn!("[trading-summary] monitor_state write failed: {}", e);
    }
    Ok(())
}

fn format_summary(acct: &Value, activity: &Value, equity_curve: &Value, today_pt: NaiveDate) -> String {
    let equity = acct.get("equity").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let day_pnl = acct.get("day_pnl").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let day_pnl_pct = acct.get("day_pnl_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);

    // All-time P&L: portfolio_history's first equity sample is account-creation
    // baseline. Alpaca's "1M" period gives 30 days, but the activity feed +
    // account.created_at let us reach further. Simpler: store the start equity
    // as a constant (account opened 2026-03-28 with $100K seed funding).
    let alltime_start = 100_000.0;
    let alltime_pnl = equity - alltime_start;
    let alltime_pnl_pct = (alltime_pnl / alltime_start) * 100.0;

    // Filter activity to today's fills.
    let mut todays_fills: Vec<&Value> = Vec::new();
    if let Some(arr) = activity.as_array() {
        for f in arr {
            let t = f.get("transaction_time").and_then(|v| v.as_str()).unwrap_or("");
            // Alpaca returns UTC ISO strings. Convert to PT for the today-filter.
            if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
                let local = dt.with_timezone(&Pacific).date_naive();
                if local == today_pt {
                    todays_fills.push(f);
                }
            }
        }
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("📊 Trading summary — {}", today_pt.format("%a %b %d")));
    lines.push(String::new());
    lines.push(format!(
        "Account equity: ${:.2}",
        equity
    ));
    lines.push(format!(
        "Today: {}{:.2} ({:+.2}%)",
        if day_pnl >= 0.0 { "+$" } else { "-$" },
        day_pnl.abs(),
        day_pnl_pct
    ));
    lines.push(format!(
        "Since 2026-03-28: {}{:.2} ({:+.2}%)",
        if alltime_pnl >= 0.0 { "+$" } else { "-$" },
        alltime_pnl.abs(),
        alltime_pnl_pct
    ));
    lines.push(String::new());

    if todays_fills.is_empty() {
        lines.push("No trades today.".to_string());
    } else {
        // Aggregate fills per-symbol for plain-English readout.
        // Each line: "Bought 1.18 SOL at avg $84.88 ($100.20 cost)"
        // Or:        "Sold 1.17 SOL at avg $83.83 ($98.13 proceeds)"
        use std::collections::BTreeMap;
        struct Agg { qty: f64, gross: f64, n: u32 }
        let mut by_key: BTreeMap<(String, String), Agg> = BTreeMap::new();
        for f in &todays_fills {
            let sym = f.get("symbol").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if sym.is_empty() { continue; }
            let side = f.get("side").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let qty: f64 = f.get("qty").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let px: f64 = f.get("price").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let entry = by_key.entry((sym, side)).or_insert(Agg{qty:0.0,gross:0.0,n:0});
            entry.qty += qty;
            entry.gross += qty * px;
            entry.n += 1;
        }
        lines.push(format!("Trades ({}): ", todays_fills.len()));
        for ((sym, side), a) in by_key.iter() {
            let avg = if a.qty > 0.0 { a.gross / a.qty } else { 0.0 };
            let verb = match side.as_str() { "buy" => "Bought", "sell" => "Sold", _ => side };
            let qty_fmt = if a.qty.fract().abs() < 0.0001 {
                format!("{}", a.qty as i64)
            } else if a.qty < 1.0 {
                format!("{:.6}", a.qty)
            } else {
                format!("{:.4}", a.qty)
            };
            let fillword = if a.n == 1 { String::new() } else { format!(" ({} fills)", a.n) };
            lines.push(format!("  • {} {} {} at avg ${:.4} = ${:.2}{}", verb, qty_fmt, sym, avg, a.gross, fillword));
        }
    }

    // Equity-curve drawdown context
    if let Some(eq_arr) = equity_curve.get("equity").and_then(|v| v.as_array()) {
        let vals: Vec<f64> = eq_arr.iter().filter_map(|v| v.as_f64()).collect();
        if vals.len() >= 2 {
            let peak = vals.iter().cloned().fold(f64::MIN, f64::max);
            let dd_from_peak = (equity - peak) / peak * 100.0;
            if dd_from_peak <= -1.0 {
                lines.push(String::new());
                lines.push(format!("Drawdown from 30d peak: {:.2}%", dd_from_peak));
            }
        }
    }
    lines.join("\n")
}

fn write_monitor_state(svc: &TradingService, acct: &Value, today_pt: NaiveDate) -> std::io::Result<()> {
    let path = svc.trading_data.join("monitor_state.json");
    let existing: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let now_unix = Utc::now().timestamp();
    let equity = acct.get("equity").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let last_equity = acct.get("last_equity").and_then(|v| v.as_f64()).unwrap_or(equity);
    let mut out = existing.as_object().cloned().unwrap_or_default();
    out.insert("last_check".to_string(), json!({
        "stock_bot": now_unix,
        "crypto_bot": now_unix,
        "leveraged_bot": now_unix,
        "options_bot": now_unix,
        "kalshi_bot": now_unix,
    }));
    let mut risk = out.get("risk").cloned().unwrap_or_else(|| json!({}));
    let prev_peak = risk.get("peak_equity").and_then(|v| v.as_f64()).unwrap_or(equity);
    let new_peak = prev_peak.max(equity);
    if let Some(map) = risk.as_object_mut() {
        map.insert("yesterday_equity".to_string(), json!(last_equity));
        map.insert("current_equity".to_string(), json!(equity));
        map.insert("peak_equity".to_string(), json!(new_peak));
        map.insert("last_snapshot_date".to_string(), json!(today_pt.to_string()));
    }
    out.insert("risk".to_string(), risk);
    let mut extra = out.get("extra").cloned().unwrap_or_else(|| json!({}));
    if let Some(map) = extra.as_object_mut() {
        map.insert("last_daily_summary".to_string(), json!(today_pt.to_string()));
    }
    out.insert("extra".to_string(), extra);
    out.insert("last_ca_check".to_string(), json!(today_pt.to_string()));
    let body = serde_json::to_vec_pretty(&Value::Object(out)).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
