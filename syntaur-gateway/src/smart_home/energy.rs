//! Energy accounting — ingestion worker + roll-up queries.
//!
//! **Ingestion**: `EnergyEngine` runs as a detached background task.
//! Every 5 minutes it walks devices with a metering capability and
//! pulls the current watts / kWh from whichever driver owns the device.
//! Matter bridge-backed devices get polled via `tools::matter::get_node_state`
//! (which reads the ElectricalMeasurement cluster); other driver sources
//! will plug in as they land. Each sample lands in
//! `smart_home_energy_samples` with `source = 'device'`.
//!
//! **Roll-ups**: computed on demand via SQL at query time. `today_kwh`
//! for a user is the max `kwh_cumulative` minus the earliest
//! `kwh_cumulative` observed today per device, summed across devices.
//! Falls back to integrating `watts × Δt` when cumulative isn't
//! available — estimated samples get a clearly-labeled `source =
//! 'derived'`.
//!
//! Week 11 scope: Matter ingestion only. Shelly Wi-Fi meters +
//! Z-Wave Meter CC (already encoded, task #18) wire in as their
//! drivers gain state reconciliation in Week 12.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone, Timelike};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_INGEST_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyRollup {
    pub device_id: Option<i64>,
    pub period_start: i64,
    pub period_end: i64,
    pub kwh: f64,
    pub cost: Option<f64>,
    pub carbon_grams: Option<f64>,
}

/// Query shape returned by `GET /api/smart-home/energy/summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergySummary {
    pub today_kwh: f64,
    pub today_cost: Option<f64>,
    pub today_carbon_grams: Option<f64>,
    pub devices: Vec<DeviceEnergyEntry>,
    pub last_sample_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEnergyEntry {
    pub device_id: i64,
    pub device_name: String,
    pub current_watts: Option<f64>,
    pub today_kwh: Option<f64>,
}

// ── Ingestion engine ────────────────────────────────────────────────────

pub struct EnergyEngine {
    db_path: PathBuf,
    interval: Duration,
}

impl EnergyEngine {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            interval: DEFAULT_INGEST_INTERVAL,
        }
    }

    pub fn with_interval(mut self, d: Duration) -> Self {
        self.interval = d;
        self
    }

    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            log::info!(
                "[smart_home::energy] ingest engine started (interval = {:?})",
                self.interval
            );
            // Offset 45s past the diagnostics sweep so the two workers
            // don't try to network-poll the same devices at the same tick.
            tokio::time::sleep(Duration::from_secs(45)).await;
            loop {
                if let Err(e) = self.ingest_once().await {
                    log::warn!("[smart_home::energy] ingest failed: {}", e);
                }
                tokio::time::sleep(self.interval).await;
            }
        })
    }

    /// Poll every metering-capable device once. Public so the UI's
    /// "Refresh energy" button can force a sync without waiting.
    pub async fn ingest_once(&self) -> Result<usize, String> {
        let db = self.db_path.clone();
        let targets = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<IngestTarget>> {
            let conn = Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT id, user_id, driver, external_id, kind
                   FROM smart_home_devices
                  WHERE kind IN ('plug', 'switch', 'light', 'thermostat')",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(IngestTarget {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    driver: row.get(2)?,
                    external_id: row.get(3)?,
                    kind: row.get(4)?,
                })
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        let mut stored = 0usize;
        let ts = chrono::Utc::now().timestamp();
        for target in &targets {
            if let Some(watts) = read_watts_for(target).await {
                if insert_sample(&self.db_path, target, ts, watts).await.is_ok() {
                    stored += 1;
                    crate::smart_home::events::publish(
                        crate::smart_home::events::SmartHomeEvent::EnergySample {
                            user_id: target.user_id,
                            device_id: target.id,
                            watts,
                        },
                    );
                }
            }
        }
        log::debug!(
            "[smart_home::energy] ingest pass stored {}/{} samples",
            stored,
            targets.len()
        );
        Ok(stored)
    }
}

#[derive(Debug, Clone)]
struct IngestTarget {
    id: i64,
    user_id: i64,
    driver: String,
    external_id: String,
    #[allow(dead_code)]
    kind: String,
}

/// Ask the owning driver for the device's instantaneous watts. Returns
/// None if the driver can't report (device not metered, driver not
/// wired, or bridge unreachable).
async fn read_watts_for(target: &IngestTarget) -> Option<f64> {
    match target.driver.as_str() {
        "matter" => {
            let node_id: u64 = target
                .external_id
                .strip_prefix("node:")
                .and_then(|s| s.parse().ok())?;
            let state = crate::tools::matter::get_node_state(node_id).await.ok()?;
            state
                .get("watts")
                .and_then(|v| v.as_f64())
                .or_else(|| {
                    // Fallback: some Matter plugs expose ActivePower in mW
                    // at cluster 0x0B04 attribute 0x050B (ElectricalMeasurement).
                    // If the bridge surfaces that elsewhere, adjust here.
                    state.get("active_power_mw").and_then(|v| v.as_f64()).map(|mw| mw / 1000.0)
                })
        }
        // Shelly Wi-Fi meters, Z-Wave COMMAND_CLASS_METER, cloud Tesla
        // readings all plug in here as their driver wiring lands.
        _ => None,
    }
}

async fn insert_sample(
    db: &PathBuf,
    target: &IngestTarget,
    ts: i64,
    watts: f64,
) -> Result<(), String> {
    let db = db.clone();
    let user_id = target.user_id;
    let device_id = target.id;
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = Connection::open(&db)?;
        conn.execute(
            "INSERT INTO smart_home_energy_samples
                (user_id, device_id, ts, watts, kwh_cumulative, source)
             VALUES (?, ?, ?, ?, NULL, 'device')",
            rusqlite::params![user_id, device_id, ts, watts],
        )?;
        // Keep smart_home_devices.last_seen_at fresh too.
        conn.execute(
            "UPDATE smart_home_devices SET last_seen_at = ? WHERE id = ?",
            rusqlite::params![ts, device_id],
        )?;
        Ok(())
    })
    .await
    .map_err(|e| format!("join: {e}"))?
    .map_err(|e| format!("db: {e}"))
}

// ── Roll-up queries ─────────────────────────────────────────────────────

// ── Phase 2I: arbitrary-window roll-ups for the Energy drawer ──────────

/// One day in the calendar heatmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayKwh {
    pub day: u32,
    pub kwh: f64,
}

/// Per-device kWh for the day-detail leaderboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceKwh {
    pub device_id: i64,
    pub device_name: String,
    pub kwh: f64,
}

/// Day-detail payload: 24 hourly buckets + per-device leaderboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayPayload {
    pub date: String,
    pub hourly: Vec<f64>,
    pub leaderboard: Vec<DeviceKwh>,
}

/// kWh consumed in [start, end). Sums cumulative-style devices
/// (max − min per device) plus trapezoidal watts × Δt for devices
/// without cumulative readings, mirroring summary_for_user.
fn integrate_window(
    conn: &Connection,
    user_id: i64,
    start: i64,
    end: i64,
) -> rusqlite::Result<f64> {
    let mut stmt = conn.prepare(
        "SELECT MIN(kwh_cumulative), MAX(kwh_cumulative)
           FROM smart_home_energy_samples
          WHERE user_id = ? AND ts >= ? AND ts < ? AND kwh_cumulative IS NOT NULL
          GROUP BY device_id",
    )?;
    let cumulative_kwh: f64 = stmt
        .query_map(rusqlite::params![user_id, start, end], |row| {
            let lo: Option<f64> = row.get(0)?;
            let hi: Option<f64> = row.get(1)?;
            Ok(match (hi, lo) {
                (Some(h), Some(l)) if h >= l => h - l,
                _ => 0.0,
            })
        })?
        .filter_map(Result::ok)
        .sum();
    let integrated = integrated_kwh_today(conn, user_id, start, end)?;
    Ok(cumulative_kwh + integrated)
}

/// Local-midnight unix timestamp for (y, m, d). Falls back to UTC if
/// the calendar tuple lands in a DST gap.
fn local_midnight_ts(year: i32, month: u32, day: u32) -> i64 {
    Local
        .with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|| {
            chrono::Utc
                .with_ymd_and_hms(year, month, day, 0, 0, 0)
                .single()
                .map(|dt| dt.timestamp())
                .unwrap_or(0)
        })
}

/// Number of days in (year, month). Returns 28..=31.
fn days_in_month(year: i32, month: u32) -> u32 {
    let first_next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let first_this = chrono::NaiveDate::from_ymd_opt(year, month, 1);
    match (first_next, first_this) {
        (Some(n), Some(t)) => (n - t).num_days() as u32,
        _ => 30,
    }
}

/// Daily kWh for every day in (year, month). Days with no samples
/// return 0.0 — empty calendar cells render with the same baseline
/// styling.
pub fn calendar_for_user(
    conn: &Connection,
    user_id: i64,
    year: i32,
    month: u32,
) -> rusqlite::Result<Vec<DayKwh>> {
    let n = days_in_month(year, month);
    let mut out = Vec::with_capacity(n as usize);
    for d in 1..=n {
        let start = local_midnight_ts(year, month, d);
        let end = start + 24 * 3600;
        let kwh = integrate_window(conn, user_id, start, end)?;
        out.push(DayKwh { day: d, kwh });
    }
    Ok(out)
}

/// Hour-by-hour kWh for one day + per-device leaderboard for that day.
pub fn day_for_user(
    conn: &Connection,
    user_id: i64,
    year: i32,
    month: u32,
    day: u32,
) -> rusqlite::Result<DayPayload> {
    let start = local_midnight_ts(year, month, day);
    let end = start + 24 * 3600;

    let mut hourly = vec![0.0_f64; 24];
    for h in 0..24 {
        let h_start = start + (h as i64) * 3600;
        let h_end = h_start + 3600;
        hourly[h] = integrate_window(conn, user_id, h_start, h_end)?;
    }

    // Leaderboard: per-device kWh for the day, cumulative-style first
    // (max−min) joined with the integrated fallback for devices without
    // a kwh_cumulative track. Top 10 by kWh, descending.
    let mut stmt = conn.prepare(
        "SELECT s.device_id, d.name,
                MAX(s.kwh_cumulative) AS hi, MIN(s.kwh_cumulative) AS lo
           FROM smart_home_energy_samples s
           JOIN smart_home_devices d ON d.id = s.device_id
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ?
          GROUP BY s.device_id",
    )?;
    let mut by_device: std::collections::HashMap<i64, (String, f64)> = std::collections::HashMap::new();
    for row in stmt
        .query_map(rusqlite::params![user_id, start, end], |row| {
            let device_id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let hi: Option<f64> = row.get(2)?;
            let lo: Option<f64> = row.get(3)?;
            let kwh = match (hi, lo) {
                (Some(h), Some(l)) if h >= l => h - l,
                _ => 0.0,
            };
            Ok((device_id, name, kwh))
        })?
        .filter_map(Result::ok)
    {
        by_device.insert(row.0, (row.1, row.2));
    }

    // Add integrated-watts kWh for devices without cumulative readings.
    let mut watts_stmt = conn.prepare(
        "SELECT s.device_id, d.name, s.ts, s.watts
           FROM smart_home_energy_samples s
           JOIN smart_home_devices d ON d.id = s.device_id
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ?
            AND s.watts IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM smart_home_energy_samples k
                 WHERE k.device_id = s.device_id AND k.user_id = s.user_id
                   AND k.ts >= ? AND k.ts < ? AND k.kwh_cumulative IS NOT NULL
            )
          ORDER BY s.device_id, s.ts",
    )?;
    let mut current_dev: Option<i64> = None;
    let mut current_name = String::new();
    let mut prev: Option<(i64, f64)> = None;
    let mut accum = 0.0_f64;
    let flush = |dev: Option<i64>, name: &str, accum: f64, by_device: &mut std::collections::HashMap<i64, (String, f64)>| {
        if let Some(id) = dev {
            if accum > 0.0 {
                let entry = by_device.entry(id).or_insert_with(|| (name.to_string(), 0.0));
                entry.1 += accum;
                if entry.0.is_empty() { entry.0 = name.to_string(); }
            }
        }
    };
    for row in watts_stmt
        .query_map(
            rusqlite::params![user_id, start, end, start, end],
            |row| {
                let dev_id: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                let ts: i64 = row.get(2)?;
                let w: f64 = row.get(3)?;
                Ok((dev_id, name, ts, w))
            },
        )?
        .filter_map(Result::ok)
    {
        let (dev_id, name, ts, w) = row;
        if Some(dev_id) != current_dev {
            flush(current_dev, &current_name, accum, &mut by_device);
            current_dev = Some(dev_id);
            current_name = name;
            prev = None;
            accum = 0.0;
        }
        if let Some((t0, w0)) = prev {
            if ts > t0 {
                let dt_h = (ts - t0) as f64 / 3600.0;
                accum += (w0 + w) / 2.0 * dt_h / 1000.0;
            }
        }
        prev = Some((ts, w));
    }
    flush(current_dev, &current_name, accum, &mut by_device);

    let mut leaderboard: Vec<DeviceKwh> = by_device
        .into_iter()
        .filter(|(_, (_, kwh))| *kwh > 0.0)
        .map(|(device_id, (device_name, kwh))| DeviceKwh { device_id, device_name, kwh })
        .collect();
    leaderboard.sort_by(|a, b| b.kwh.partial_cmp(&a.kwh).unwrap_or(std::cmp::Ordering::Equal));
    leaderboard.truncate(10);

    let date = format!("{:04}-{:02}-{:02}", year, month, day);
    Ok(DayPayload { date, hourly, leaderboard })
}

/// Currently active $/kWh rate for a user, if one is configured.
/// Returns the cost_per_kwh for the rate row whose [starts_at, ends_at) covers "now".
/// Powers Settings -> Smart Home -> Energy (Phase 2J).
/// Per-device kWh totals over a [start, end) window. Mirrors integrate_window
/// but bucketed by device. Used by Phase 2K anomaly detection.
fn device_kwh_for_window(
    conn: &Connection,
    user_id: i64,
    start: i64,
    end: i64,
) -> rusqlite::Result<std::collections::HashMap<i64, (String, f64)>> {
    let mut by_dev: std::collections::HashMap<i64, (String, f64)> = std::collections::HashMap::new();

    // Cumulative meters: max - min per (device).
    let mut cstmt = conn.prepare(
        "SELECT s.device_id, COALESCE(d.name, CAST(s.device_id AS TEXT)),
                MIN(s.kwh_cumulative), MAX(s.kwh_cumulative)
           FROM smart_home_energy_samples s
           LEFT JOIN smart_home_devices d ON d.id = s.device_id
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ? AND s.kwh_cumulative IS NOT NULL
          GROUP BY s.device_id, d.name",
    )?;
    for row in cstmt
        .query_map(rusqlite::params![user_id, start, end], |row| {
            let dev_id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let lo: Option<f64> = row.get(2)?;
            let hi: Option<f64> = row.get(3)?;
            Ok((dev_id, name, lo, hi))
        })?
        .filter_map(Result::ok)
    {
        let (dev_id, name, lo, hi) = row;
        let kwh = match (hi, lo) {
            (Some(h), Some(l)) if h >= l => h - l,
            _ => 0.0,
        };
        if kwh > 0.0 {
            by_dev.entry(dev_id).or_insert_with(|| (name, 0.0)).1 += kwh;
        }
    }

    // Watts integration (only for device-windows that have NO cumulative reading).
    let mut wstmt = conn.prepare(
        "SELECT s.device_id, COALESCE(d.name, CAST(s.device_id AS TEXT)), s.ts, s.watts
           FROM smart_home_energy_samples s
           LEFT JOIN smart_home_devices d ON d.id = s.device_id
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ? AND s.watts IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM smart_home_energy_samples k
                 WHERE k.device_id = s.device_id AND k.user_id = s.user_id
                   AND k.ts >= ? AND k.ts < ? AND k.kwh_cumulative IS NOT NULL
            )
          ORDER BY s.device_id, s.ts",
    )?;
    let mut current_dev: Option<i64> = None;
    let mut current_name = String::new();
    let mut prev: Option<(i64, f64)> = None;
    let mut accum = 0.0_f64;
    for row in wstmt
        .query_map(
            rusqlite::params![user_id, start, end, start, end],
            |row| {
                let dev_id: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                let ts: i64 = row.get(2)?;
                let w: f64 = row.get(3)?;
                Ok((dev_id, name, ts, w))
            },
        )?
        .filter_map(Result::ok)
    {
        let (dev_id, name, ts, w) = row;
        if Some(dev_id) != current_dev {
            if let Some(id) = current_dev {
                if accum > 0.0 {
                    by_dev.entry(id).or_insert_with(|| (current_name.clone(), 0.0)).1 += accum;
                }
            }
            current_dev = Some(dev_id);
            current_name = name;
            prev = None;
            accum = 0.0;
        }
        if let Some((t0, w0)) = prev {
            if ts > t0 {
                let dt_h = (ts - t0) as f64 / 3600.0;
                accum += (w0 + w) / 2.0 * dt_h / 1000.0;
            }
        }
        prev = Some((ts, w));
    }
    if let Some(id) = current_dev {
        if accum > 0.0 {
            by_dev.entry(id).or_insert_with(|| (current_name, 0.0)).1 += accum;
        }
    }

    Ok(by_dev)
}

/// One device flagged as using significantly more energy today than its
/// 30-day rolling average. Powers Phase 2K anomaly badges.
#[derive(serde::Serialize, Debug, Clone)]
pub struct Anomaly {
    pub device_id: i64,
    pub device_name: String,
    /// Average daily kWh over the prior 30 days (not including today).
    pub baseline_kwh_per_day: f64,
    /// Projected total kWh for today, extrapolated from current pace.
    pub projected_kwh: f64,
    /// projected_kwh / baseline_kwh_per_day.
    pub ratio: f64,
    /// "medium" (1.5x..2x) or "high" (>=2x baseline).
    pub severity: &'static str,
}

/// Returns devices whose projected today's kWh exceeds 1.5x their 30-day
/// rolling baseline. Skips devices with baseline < 0.05 kWh/day (noise
/// floor) and skips when fewer than 2 hours have elapsed today (projection
/// is too noisy in the early morning).
pub fn anomalies_for_user(
    conn: &Connection,
    user_id: i64,
) -> rusqlite::Result<Vec<Anomaly>> {
    let now = chrono::Utc::now().timestamp();
    let today_start = midnight_local_ts();
    let today_end = today_start + 24 * 3600;
    let elapsed = (now - today_start).max(0) as f64;
    if elapsed < 2.0 * 3600.0 {
        return Ok(Vec::new());
    }
    let baseline_start = today_start - 30 * 24 * 3600;
    let baseline_end = today_start;

    let baseline = device_kwh_for_window(conn, user_id, baseline_start, baseline_end)?;
    let today = device_kwh_for_window(conn, user_id, today_start, today_end)?;

    let projection_factor = (24.0 * 3600.0) / elapsed;

    let mut out: Vec<Anomaly> = Vec::new();
    for (device_id, (today_name, today_kwh)) in today.iter() {
        let projected = today_kwh * projection_factor;
        let (base_name, base_total) = match baseline.get(device_id) {
            Some(v) => (v.0.clone(), v.1),
            None => continue,
        };
        let baseline_per_day = base_total / 30.0;
        if baseline_per_day < 0.05 {
            continue;
        }
        let ratio = projected / baseline_per_day;
        let severity = if ratio >= 2.0 {
            "high"
        } else if ratio >= 1.5 {
            "medium"
        } else {
            continue;
        };
        let device_name = if !today_name.is_empty() { today_name.clone() } else { base_name };
        out.push(Anomaly {
            device_id: *device_id,
            device_name,
            baseline_kwh_per_day: baseline_per_day,
            projected_kwh: projected,
            ratio,
            severity,
        });
    }
    out.sort_by(|a, b| b.ratio.partial_cmp(&a.ratio).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

pub fn current_rate_for_user(
    conn: &Connection,
    user_id: i64,
) -> rusqlite::Result<Option<f64>> {
    let now = chrono::Utc::now().timestamp();
    let row: rusqlite::Result<f64> = conn.query_row(
        "SELECT cost_per_kwh FROM smart_home_energy_rates
          WHERE user_id = ? AND starts_at <= ? AND (ends_at IS NULL OR ends_at > ?)
          ORDER BY starts_at DESC LIMIT 1",
        rusqlite::params![user_id, now, now],
        |r| r.get(0),
    );
    match row {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Set the user's flat $/kWh rate. Closes any open rate (ends_at = now)
/// and inserts a new active rate. Pass  to clear (just close current
/// without inserting a new one).
pub fn set_rate_for_user(
    conn: &Connection,
    user_id: i64,
    cost_per_kwh: Option<f64>,
) -> rusqlite::Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE smart_home_energy_rates SET ends_at = ?
          WHERE user_id = ? AND ends_at IS NULL",
        rusqlite::params![now, user_id],
    )?;
    if let Some(cost) = cost_per_kwh {
        conn.execute(
            "INSERT INTO smart_home_energy_rates
                (user_id, starts_at, ends_at, cost_per_kwh, carbon_g_per_kwh)
             VALUES (?, ?, NULL, ?, NULL)",
            rusqlite::params![user_id, now, cost],
        )?;
    }
    Ok(())
}

/// Today's roll-up for a user. If cumulative readings exist, uses the
/// max−min per device; otherwise integrates watts×Δt (trapezoidal).
pub fn summary_for_user(conn: &Connection, user_id: i64) -> rusqlite::Result<EnergySummary> {
    let start = midnight_local_ts();
    let end = start + 24 * 3600;

    // Per-device kWh today.
    let mut stmt = conn.prepare(
        "SELECT s.device_id, d.name, MAX(s.watts) AS cur_w,
                MIN(s.kwh_cumulative) AS k_min, MAX(s.kwh_cumulative) AS k_max
           FROM smart_home_energy_samples s
           JOIN smart_home_devices d ON d.id = s.device_id
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ?
          GROUP BY s.device_id",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, start, end], |row| {
        let device_id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let cur_w: Option<f64> = row.get(2)?;
        let k_min: Option<f64> = row.get(3)?;
        let k_max: Option<f64> = row.get(4)?;
        let device_kwh = match (k_max, k_min) {
            (Some(hi), Some(lo)) if hi >= lo => Some(hi - lo),
            _ => None,
        };
        Ok(DeviceEnergyEntry {
            device_id,
            device_name: name,
            current_watts: cur_w,
            today_kwh: device_kwh,
        })
    })?;
    let devices: Vec<DeviceEnergyEntry> = rows.filter_map(Result::ok).collect();

    // Overall today_kwh: sum cumulative-style first, then add integrated
    // watts for devices without a kwh_cumulative track.
    let cumulative_kwh: f64 = devices
        .iter()
        .filter_map(|d| d.today_kwh)
        .sum();

    let integrated_kwh =
        integrated_kwh_today(conn, user_id, start, end)?;

    let today_kwh = cumulative_kwh + integrated_kwh;

    // Active rate (cost + carbon) for the current ts, if configured.
    let now = chrono::Utc::now().timestamp();
    let rate: Option<(f64, Option<f64>)> = conn
        .query_row(
            "SELECT cost_per_kwh, carbon_g_per_kwh FROM smart_home_energy_rates
              WHERE user_id = ? AND starts_at <= ? AND (ends_at IS NULL OR ends_at > ?)
              ORDER BY starts_at DESC LIMIT 1",
            rusqlite::params![user_id, now, now],
            |row| {
                let cost: f64 = row.get(0)?;
                let carbon: Option<f64> = row.get(1)?;
                Ok((cost, carbon))
            },
        )
        .ok();

    let today_cost = rate.map(|(c, _)| today_kwh * c);
    let today_carbon_grams = rate
        .and_then(|(_, c)| c)
        .map(|c| today_kwh * c);

    let last_sample_at: Option<i64> = conn
        .query_row(
            "SELECT MAX(ts) FROM smart_home_energy_samples WHERE user_id = ?",
            rusqlite::params![user_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    Ok(EnergySummary {
        today_kwh,
        today_cost,
        today_carbon_grams,
        devices,
        last_sample_at,
    })
}

/// Integrate watts samples for devices that never reported
/// kwh_cumulative today. Uses trapezoidal area between consecutive
/// samples within the [start, end) window.
fn integrated_kwh_today(
    conn: &Connection,
    user_id: i64,
    start: i64,
    end: i64,
) -> rusqlite::Result<f64> {
    // Only devices without ANY cumulative reading in the window need
    // integration.
    let mut dev_stmt = conn.prepare(
        "SELECT DISTINCT s.device_id FROM smart_home_energy_samples s
          WHERE s.user_id = ? AND s.ts >= ? AND s.ts < ?
            AND NOT EXISTS (
                SELECT 1 FROM smart_home_energy_samples k
                 WHERE k.device_id = s.device_id
                   AND k.user_id = s.user_id
                   AND k.ts >= ? AND k.ts < ?
                   AND k.kwh_cumulative IS NOT NULL
            )",
    )?;
    let device_ids: Vec<i64> = dev_stmt
        .query_map(rusqlite::params![user_id, start, end, start, end], |r| {
            r.get::<_, i64>(0)
        })?
        .filter_map(Result::ok)
        .collect();

    let mut total_kwh = 0f64;
    for device_id in device_ids {
        let mut stmt = conn.prepare(
            "SELECT ts, watts FROM smart_home_energy_samples
              WHERE user_id = ? AND device_id = ? AND ts >= ? AND ts < ? AND watts IS NOT NULL
              ORDER BY ts ASC",
        )?;
        let samples: Vec<(i64, f64)> = stmt
            .query_map(
                rusqlite::params![user_id, device_id, start, end],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?
            .filter_map(Result::ok)
            .collect();
        for pair in samples.windows(2) {
            let (t0, w0) = pair[0];
            let (t1, w1) = pair[1];
            if t1 > t0 {
                let dt_hours = (t1 - t0) as f64 / 3600.0;
                let avg_w = (w0 + w1) / 2.0;
                total_kwh += avg_w * dt_hours / 1000.0;
            }
        }
    }
    Ok(total_kwh)
}

fn midnight_local_ts() -> i64 {
    let now = Local::now();
    let local_midnight = Local
        .with_ymd_and_hms(now.year_ce().1 as i32, now.month(), now.day(), 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now);
    local_midnight.timestamp()
}

// chrono re-imports used in midnight_local_ts
use chrono::Datelike;

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE users (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 disabled INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE smart_home_devices (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 room_id INTEGER, driver TEXT NOT NULL, external_id TEXT NOT NULL,
                 name TEXT NOT NULL, kind TEXT NOT NULL,
                 capabilities_json TEXT NOT NULL DEFAULT '{}',
                 state_json TEXT NOT NULL DEFAULT '{}',
                 metadata_json TEXT NOT NULL DEFAULT '{}',
                 last_seen_at INTEGER, created_at INTEGER NOT NULL
             );
             CREATE TABLE smart_home_energy_samples (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 device_id INTEGER, ts INTEGER NOT NULL,
                 watts REAL, kwh_cumulative REAL, source TEXT
             );
             CREATE TABLE smart_home_energy_rates (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 starts_at INTEGER NOT NULL, ends_at INTEGER,
                 cost_per_kwh REAL NOT NULL, carbon_g_per_kwh REAL, utility TEXT
             );
             INSERT INTO users (name, created_at) VALUES ('alice', 0);
             INSERT INTO smart_home_devices
                (user_id, driver, external_id, name, kind, created_at)
                VALUES (1, 'matter', 'node:1', 'Porch Light', 'light', 0);
             INSERT INTO smart_home_devices
                (user_id, driver, external_id, name, kind, created_at)
                VALUES (1, 'matter', 'node:2', 'Kitchen Plug', 'plug', 0);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn summary_empty_when_no_samples() {
        let conn = fresh_db();
        let s = summary_for_user(&conn, 1).unwrap();
        assert_eq!(s.today_kwh, 0.0);
        assert!(s.last_sample_at.is_none());
        assert!(s.today_cost.is_none());
    }

    #[test]
    fn integrated_kwh_trapezoidal() {
        let conn = fresh_db();
        // 60W for 1 hour = 0.060 kWh. Two samples: t=start, 60W; t=start+3600, 60W.
        let start = midnight_local_ts();
        let mid = start + 1800;
        let end = start + 3600;
        for (ts, w) in [(start, 60.0), (mid, 60.0), (end, 60.0)] {
            conn.execute(
                "INSERT INTO smart_home_energy_samples
                    (user_id, device_id, ts, watts, source)
                 VALUES (1, 1, ?, ?, 'device')",
                rusqlite::params![ts, w],
            )
            .unwrap();
        }
        let s = summary_for_user(&conn, 1).unwrap();
        assert!(
            (s.today_kwh - 0.060).abs() < 1e-6,
            "expected 0.060 kWh got {}",
            s.today_kwh
        );
    }

    #[test]
    fn summary_uses_cumulative_when_available() {
        let conn = fresh_db();
        let start = midnight_local_ts() + 60;
        // Two cumulative readings: 100.0 kWh then 101.5 kWh → 1.5 kWh today.
        conn.execute(
            "INSERT INTO smart_home_energy_samples
                (user_id, device_id, ts, watts, kwh_cumulative, source)
             VALUES (1, 1, ?, NULL, 100.0, 'device')",
            rusqlite::params![start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO smart_home_energy_samples
                (user_id, device_id, ts, watts, kwh_cumulative, source)
             VALUES (1, 1, ?, NULL, 101.5, 'device')",
            rusqlite::params![start + 3600],
        )
        .unwrap();
        let s = summary_for_user(&conn, 1).unwrap();
        assert!(
            (s.today_kwh - 1.5).abs() < 1e-9,
            "expected 1.5 kWh got {}",
            s.today_kwh
        );
        // Per-device list should include Porch Light with its 1.5 kWh.
        let porch = s
            .devices
            .iter()
            .find(|d| d.device_name == "Porch Light")
            .unwrap();
        assert!((porch.today_kwh.unwrap() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn summary_applies_cost_and_carbon_rate() {
        let conn = fresh_db();
        let start = midnight_local_ts() + 60;
        conn.execute(
            "INSERT INTO smart_home_energy_samples
                (user_id, device_id, ts, watts, kwh_cumulative, source)
             VALUES (1, 1, ?, NULL, 0.0, 'device')",
            rusqlite::params![start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO smart_home_energy_samples
                (user_id, device_id, ts, watts, kwh_cumulative, source)
             VALUES (1, 1, ?, NULL, 10.0, 'device')",
            rusqlite::params![start + 3600],
        )
        .unwrap();
        // Flat $0.15/kWh, 300 g CO₂/kWh, currently active.
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO smart_home_energy_rates
                (user_id, starts_at, ends_at, cost_per_kwh, carbon_g_per_kwh)
             VALUES (1, ?, NULL, 0.15, 300.0)",
            rusqlite::params![now - 86400],
        )
        .unwrap();
        let s = summary_for_user(&conn, 1).unwrap();
        assert!((s.today_kwh - 10.0).abs() < 1e-9);
        assert!((s.today_cost.unwrap() - 1.5).abs() < 1e-9);
        assert!((s.today_carbon_grams.unwrap() - 3000.0).abs() < 1e-6);
    }
}
