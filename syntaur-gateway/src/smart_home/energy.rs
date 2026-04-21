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
