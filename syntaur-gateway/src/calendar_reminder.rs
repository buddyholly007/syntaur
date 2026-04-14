//! Background task that scans for upcoming calendar events with
//! `reminder_minutes` set, and sends a Telegram message when the
//! reminder window is reached. Uses `calendar_reminders_sent` to
//! dedupe per-occurrence.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};

use crate::AppState;

/// Spawn the reminder tick task. Runs once every 60 seconds.
pub fn spawn_reminder_task(state: Arc<AppState>) {
    tokio::spawn(async move {
        info!("[calendar-reminder] background task started (60s interval)");
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        // Skip the immediate first tick — wait a minute before first run
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(e) = tick(&state).await {
                warn!("[calendar-reminder] tick failed: {}", e);
            }
        }
    });
}

async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let now = chrono::Utc::now();
    let now_ts = now.timestamp();

    // Look up bot token from config
    let bot_token = state.config.channels.telegram.bot_token.clone();
    if bot_token.is_empty() {
        return Ok(()); // No bot configured — silently skip
    }

    let db_path: PathBuf = state.db_path.clone();

    // Collect (user_id, chat_id, event_id, title, description, start_time, occurrence_date, reminder_minutes, recurrence_rule, recurrence_end_date)
    let db = db_path.clone();
    let jobs: Vec<(i64, String, i64, String, Option<String>, String, String, i64, Option<String>, Option<String>)> =
        tokio::task::spawn_blocking(move || -> Result<Vec<_>, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Join events with telegram links. Only users who linked a chat get reminders.
            let mut stmt = conn.prepare(
                "SELECT e.user_id, l.chat_id, e.id, e.title, e.description, e.start_time, \
                        e.reminder_minutes, e.recurrence_rule, e.recurrence_end_date \
                 FROM calendar_events e \
                 JOIN user_telegram_links l ON l.user_id = e.user_id \
                 WHERE e.reminder_minutes IS NOT NULL AND e.reminder_minutes > 0"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| Ok((
                r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?, r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?,
            ))).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for r in rows.flatten() {
                let (user_id, chat_id, event_id, title, desc, start_time, rmins, rrule, rend) = r;
                let occurrences = next_occurrences(&start_time, rrule.as_deref(), rend.as_deref(), rmins, now_ts);
                for occ_date in occurrences {
                    out.push((user_id, chat_id.clone(), event_id, title.clone(), desc.clone(),
                              start_time.clone(), occ_date, rmins, rrule.clone(), rend.clone()));
                }
            }
            Ok(out)
        }).await.map_err(|e| e.to_string())??;

    if jobs.is_empty() { return Ok(()); }

    let client = reqwest::Client::new();
    for (_user_id, chat_id, event_id, title, desc, _start_time, occ_date, _rmins, _rrule, _rend) in jobs {
        // Check if already sent
        let db = db_path.clone();
        let occ_clone = occ_date.clone();
        let already: bool = tokio::task::spawn_blocking(move || -> Result<bool, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM calendar_reminders_sent WHERE event_id = ? AND occurrence_date = ?",
                rusqlite::params![event_id, &occ_clone],
                |r| r.get(0),
            ).unwrap_or(0);
            Ok(count > 0)
        }).await.map_err(|e| e.to_string())??;

        if already { continue; }

        // Build message
        let desc_str = desc.as_deref().unwrap_or("");
        let msg = if desc_str.is_empty() {
            format!("📅 Reminder: {}\n\n🕐 {}", title, occ_date)
        } else {
            format!("📅 Reminder: {}\n\n🕐 {}\n\n{}", title, occ_date, desc_str)
        };

        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
        let res = client
            .post(&url)
            .json(&serde_json::json!({"chat_id": chat_id, "text": msg}))
            .timeout(Duration::from_secs(15))
            .send()
            .await;

        match res {
            Ok(r) if r.status().is_success() => {
                info!("[calendar-reminder] sent reminder for event {} occurrence {}", event_id, occ_date);
                let db = db_path.clone();
                let occ_clone = occ_date.clone();
                let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO calendar_reminders_sent (event_id, occurrence_date, sent_at) VALUES (?, ?, ?)",
                        rusqlite::params![event_id, &occ_clone, now_ts],
                    );
                    Ok(())
                }).await;
            }
            Ok(r) => warn!("[calendar-reminder] telegram API returned {}", r.status()),
            Err(e) => warn!("[calendar-reminder] send failed: {}", e),
        }
    }

    Ok(())
}

/// For a given event, find all occurrences whose start_time minus reminder_minutes
/// falls in the window [now-60s, now+60s]. Returns occurrence_date strings (YYYY-MM-DD).
fn next_occurrences(
    start_time: &str,
    rrule: Option<&str>,
    rend: Option<&str>,
    reminder_mins: i64,
    now_ts: i64,
) -> Vec<String> {
    let mut out = Vec::new();
    let window_start = now_ts - 60;
    let window_end = now_ts + 60;

    let parse_evt_time = |date_str: &str, time_part: &str| -> Option<i64> {
        let dt_str = if time_part.is_empty() {
            format!("{}T00:00:00", date_str)
        } else if time_part.starts_with('T') {
            if time_part.len() >= 9 {
                format!("{}{}", date_str, time_part)
            } else {
                format!("{}{}:00", date_str, time_part)
            }
        } else {
            format!("{}T{}", date_str, time_part.trim_start())
        };
        let dt_str = if dt_str.len() == 16 { format!("{}:00", dt_str) } else { dt_str };
        chrono::NaiveDateTime::parse_from_str(&dt_str, "%Y-%m-%dT%H:%M:%S").ok()
            .map(|dt| dt.and_utc().timestamp())
    };

    if start_time.len() < 10 { return out; }
    let base_date_str = &start_time[..10];
    let time_part = if start_time.len() > 10 { &start_time[10..] } else { "" };
    let base_date = match chrono::NaiveDate::parse_from_str(base_date_str, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return out,
    };

    let rec_end = rend.and_then(|s| chrono::NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d").ok());
    let reminder_offset = reminder_mins * 60;

    let event_window_start = window_start + reminder_offset;
    let event_window_end = window_end + reminder_offset;

    if rrule.is_none() || rrule == Some("") || rrule == Some("none") {
        if let Some(ts) = parse_evt_time(base_date_str, time_part) {
            if ts >= event_window_start && ts <= event_window_end {
                out.push(base_date_str.to_string());
            }
        }
        return out;
    }

    let mut cur = base_date;
    let today = chrono::Utc::now().date_naive();
    if cur < today {
        match rrule.unwrap_or("") {
            "daily" => cur = today,
            "weekly" => {
                let diff = (today - cur).num_days();
                let weeks = diff / 7;
                cur = cur.checked_add_days(chrono::Days::new((weeks * 7) as u64)).unwrap_or(cur);
            }
            _ => {}
        }
    }

    let mut safety = 0;
    while safety < 400 {
        safety += 1;
        if let Some(end_d) = rec_end { if cur > end_d { break; } }
        let date_str = cur.format("%Y-%m-%d").to_string();
        if let Some(ts) = parse_evt_time(&date_str, time_part) {
            if ts > event_window_end { break; }
            if ts >= event_window_start {
                out.push(date_str.clone());
            }
        }
        cur = match rrule.unwrap_or("") {
            "daily" => cur.succ_opt().unwrap_or(cur),
            "weekly" => cur.checked_add_days(chrono::Days::new(7)).unwrap_or(cur),
            "monthly" => {
                let m = cur.month();
                let y = cur.year();
                let (ny, nm) = if m == 12 { (y+1, 1) } else { (y, m+1) };
                let target_day = cur.day();
                let max_day = match nm {
                    1|3|5|7|8|10|12 => 31,
                    4|6|9|11 => 30,
                    2 => if (ny % 4 == 0 && ny % 100 != 0) || ny % 400 == 0 { 29 } else { 28 },
                    _ => 28,
                };
                chrono::NaiveDate::from_ymd_opt(ny, nm, target_day.min(max_day)).unwrap_or(cur)
            }
            "yearly" => chrono::NaiveDate::from_ymd_opt(cur.year()+1, cur.month(), cur.day()).unwrap_or(cur),
            _ => break,
        };
    }
    out
}

/// Need this trait for chrono NaiveDate component access
use chrono::Datelike;
