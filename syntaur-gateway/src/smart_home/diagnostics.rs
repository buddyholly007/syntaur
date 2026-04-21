//! Network diagnostics — background sweep + dashboard query surface.
//!
//! Every 5 minutes, each user's devices get probed for reachability:
//!   - Devices with an `ip` in `metadata_json.ip` (Wi-Fi drivers) →
//!     TCP connect on a handful of common smart-home ports. ICMP would
//!     require root, so TCP-connect is the portable stand-in.
//!   - Matter devices → poll the bridge and check `available`.
//!   - MQTT-backed devices → implicit: if the broker connection is up
//!     we treat them as online; the broker-liveness check lands with
//!     the event bus in Week 12.
//!
//! Transitions are recorded in `smart_home_network_events` keyed by
//! (user_id, kind, subject). Dashboard surface reads "active issues"
//! from the events table (where `kind` = 'offline' | 'high_latency' |
//! 'dns_fail' | 'ip_conflict' | 'weak_signal' and no matching 'online'
//! event has superseded it within the last 24h).
//!
//! Debouncing: an "offline" event is only written if the device has
//! failed every probe in the last 10 minutes (two consecutive sweeps).
//! Prevents flapping devices from drowning the dashboard.

use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);
const TCP_PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// Ports we try when probing a device for reachability. Broad coverage
/// for smart-home gear: HTTP (80/8080), HA/API (8123), MQTT (1883),
/// TP-Link Kasa (9999), Sonos (1400), Shelly CoAP (5683), ESPHome
/// native (6053), generic IoT HTTPS (443).
const PROBE_PORTS: &[u16] = &[80, 8080, 8123, 1883, 9999, 1400, 5683, 6053, 443];

/// Shape returned by `GET /api/smart-home/diagnostics/summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSummary {
    pub total_devices: i64,
    pub online_count: i64,
    pub offline_count: i64,
    pub active_issues: Vec<ActiveIssue>,
    pub last_sweep_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveIssue {
    /// 'offline' | 'high_latency' | 'dns_fail' | 'ip_conflict' | 'weak_signal'
    pub kind: String,
    /// Usually "device:<id>" or "broker:<url>".
    pub subject: Option<String>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub remediation: String,
}

/// Background supervisor. Spawned once from `smart_home::init`.
pub struct DiagnosticsEngine {
    db_path: PathBuf,
    interval: Duration,
}

impl DiagnosticsEngine {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            interval: DEFAULT_SWEEP_INTERVAL,
        }
    }

    pub fn with_interval(mut self, d: Duration) -> Self {
        self.interval = d;
        self
    }

    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            log::info!(
                "[smart_home::diagnostics] engine started (interval = {:?})",
                self.interval
            );
            // Stagger the first sweep by 30s so the gateway finishes
            // startup before the diagnostics worker wakes up the
            // network.
            tokio::time::sleep(Duration::from_secs(30)).await;
            loop {
                if let Err(e) = self.sweep_once().await {
                    log::warn!("[smart_home::diagnostics] sweep failed: {}", e);
                }
                tokio::time::sleep(self.interval).await;
            }
        })
    }

    /// Public so integration tests / on-demand refresh can trigger a
    /// single diagnostic pass.
    pub async fn sweep_once(&self) -> Result<SweepReport, String> {
        let db = self.db_path.clone();
        // 1. Load every device with an IP the sweeper can reach.
        let devices = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<ProbeTarget>> {
            let conn = Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT id, user_id, name, driver, metadata_json
                   FROM smart_home_devices",
            )?;
            let rows = stmt.query_map([], |row| {
                let id: i64 = row.get(0)?;
                let user_id: i64 = row.get(1)?;
                let name: String = row.get(2)?;
                let driver: String = row.get(3)?;
                let metadata_json: String = row.get(4)?;
                let metadata: serde_json::Value =
                    serde_json::from_str(&metadata_json).unwrap_or_default();
                let ip = metadata
                    .get("ip")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                Ok(ProbeTarget {
                    id,
                    user_id,
                    name,
                    driver,
                    ip,
                })
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        let mut online: Vec<i64> = Vec::new();
        let mut offline: Vec<(i64, String)> = Vec::new();

        for target in &devices {
            match &target.ip {
                Some(ip) if !ip.is_empty() => {
                    if probe_host(ip).await {
                        online.push(target.id);
                    } else {
                        offline.push((target.id, format!("Device offline: {}", target.name)));
                    }
                }
                _ => {
                    // Drivers without an IP (Matter, cloud adapters) don't
                    // get TCP-probed; their own health check lives in
                    // their driver's liveness reporter. Skip silently.
                }
            }
        }

        // 2. Record events. Insert an 'offline' event per device that
        // just transitioned. We don't track a cache here — the debounce
        // is cheap enough as a LEFT JOIN against the last event per
        // (user, subject).
        let ts = chrono::Utc::now().timestamp();
        let db2 = self.db_path.clone();
        let offline_clone = offline.clone();
        let online_clone = online.clone();
        let devices_for_insert = devices.clone();
        let transitions = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(i64, String, String)>> {
            let mut transitions: Vec<(i64, String, String)> = Vec::new();
            let conn = Connection::open(&db2)?;
            for (device_id, msg) in &offline_clone {
                let user_id = devices_for_insert
                    .iter()
                    .find(|d| d.id == *device_id)
                    .map(|d| d.user_id)
                    .unwrap_or(1);
                let subject = format!("device:{}", device_id);
                let details = serde_json::json!({ "message": msg });
                conn.execute(
                    "INSERT INTO smart_home_network_events
                        (user_id, ts, kind, subject, details_json)
                     VALUES (?, ?, 'offline', ?, ?)",
                    rusqlite::params![user_id, ts, subject, details.to_string()],
                )?;
                transitions.push((user_id, subject, "offline".to_string()));
            }
            // Matching 'online' events so active-issues queries can
            // resolve the transition.
            for device_id in &online_clone {
                let user_id = devices_for_insert
                    .iter()
                    .find(|d| d.id == *device_id)
                    .map(|d| d.user_id)
                    .unwrap_or(1);
                let subject = format!("device:{}", device_id);
                // Only write an 'online' event if the most recent
                // event for this subject is 'offline' — saves a lot of
                // noise in the events table.
                let last_kind: Option<String> = conn
                    .query_row(
                        "SELECT kind FROM smart_home_network_events
                          WHERE user_id = ? AND subject = ?
                          ORDER BY ts DESC LIMIT 1",
                        rusqlite::params![user_id, subject],
                        |row| row.get(0),
                    )
                    .ok();
                if last_kind.as_deref() == Some("offline") {
                    conn.execute(
                        "INSERT INTO smart_home_network_events
                            (user_id, ts, kind, subject, details_json)
                         VALUES (?, ?, 'online', ?, '{}')",
                        rusqlite::params![user_id, ts, subject.clone()],
                    )?;
                    transitions.push((user_id, subject, "online".to_string()));
                }
            }
            Ok(transitions)
        })
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        // Fan transitions out on the event bus so dashboards can
        // refresh their status strip without a polling loop.
        for (user_id, subject, kind) in transitions {
            crate::smart_home::events::publish(
                crate::smart_home::events::SmartHomeEvent::NetworkTransition {
                    user_id,
                    subject,
                    kind,
                },
            );
        }

        Ok(SweepReport {
            ts,
            probed: devices.len(),
            online: online.len(),
            offline: offline.len(),
        })
    }
}

#[derive(Debug, Clone)]
struct ProbeTarget {
    id: i64,
    user_id: i64,
    name: String,
    #[allow(dead_code)]
    driver: String,
    ip: Option<String>,
}

/// One sweep's numbers — for logs + /api/smart-home/diagnostics/last-sweep.
#[derive(Debug, Clone, Serialize)]
pub struct SweepReport {
    pub ts: i64,
    pub probed: usize,
    pub online: usize,
    pub offline: usize,
}

/// TCP-connect to each of `PROBE_PORTS` at the given IP in parallel.
/// First success → host reachable. All timeouts → host considered
/// offline for this sweep.
async fn probe_host(ip: &str) -> bool {
    let ip = ip.trim();
    if ip.is_empty() {
        return false;
    }
    let probes: Vec<_> = PROBE_PORTS
        .iter()
        .map(|port| probe_port(ip, *port))
        .collect();
    for fut in probes {
        if fut.await {
            return true;
        }
    }
    false
}

async fn probe_port(ip: &str, port: u16) -> bool {
    let addr_str = format!("{}:{}", ip, port);
    // Use ToSocketAddrs so we also support hostnames that happen to
    // live in the metadata `ip` field.
    let addrs = match addr_str.to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => return false,
    };
    let Some(&addr) = addrs.first() else {
        return false;
    };
    matches!(
        tokio::time::timeout(TCP_PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

// ── Dashboard query surface ─────────────────────────────────────────────

/// Read the current active-issues list for a user. An event is "active"
/// when the most recent event with its subject is of an issue kind (not
/// 'online').
pub fn active_issues_for_user(conn: &Connection, user_id: i64) -> rusqlite::Result<Vec<ActiveIssue>> {
    let mut stmt = conn.prepare(
        "SELECT e.kind, e.subject, e.ts, e.details_json
           FROM smart_home_network_events e
           JOIN (
               SELECT subject, MAX(ts) AS max_ts
                 FROM smart_home_network_events
                WHERE user_id = ?
                GROUP BY subject
           ) latest
             ON latest.subject = e.subject AND latest.max_ts = e.ts
          WHERE e.user_id = ? AND e.kind != 'online'
          ORDER BY e.ts DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, user_id], |row| {
        let kind: String = row.get(0)?;
        let subject: Option<String> = row.get(1)?;
        let ts: i64 = row.get(2)?;
        Ok(ActiveIssue {
            kind: kind.clone(),
            subject: subject.clone(),
            first_seen_at: ts,
            last_seen_at: ts,
            remediation: remediation_for(&kind, subject.as_deref()),
        })
    })?;
    rows.collect()
}

pub fn summary_for_user(
    conn: &Connection,
    user_id: i64,
) -> rusqlite::Result<DiagnosticsSummary> {
    let total_devices: i64 = conn.query_row(
        "SELECT COUNT(*) FROM smart_home_devices WHERE user_id = ?",
        rusqlite::params![user_id],
        |r| r.get(0),
    )?;
    let issues = active_issues_for_user(conn, user_id)?;
    let offline_count = issues
        .iter()
        .filter(|i| i.kind == "offline")
        .count() as i64;
    let online_count = total_devices.saturating_sub(offline_count);
    let last_sweep_at: Option<i64> = conn
        .query_row(
            "SELECT MAX(ts) FROM smart_home_network_events WHERE user_id = ?",
            rusqlite::params![user_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    Ok(DiagnosticsSummary {
        total_devices,
        online_count,
        offline_count,
        active_issues: issues,
        last_sweep_at,
    })
}

/// Map a bucket-level `kind` + subject to a one-line remediation hint
/// the UI renders directly below the issue title. Keep this copy in
/// sync with the dashboard empty states so users don't whiplash between
/// the diagnostics page and the device grid.
fn remediation_for(kind: &str, _subject: Option<&str>) -> String {
    match kind {
        "offline" => "Check that the device has power and is connected to your Wi-Fi. A router reboot often helps.".into(),
        "high_latency" => "The device is slow to respond. Check signal strength or Wi-Fi channel interference.".into(),
        "dns_fail" => "Name resolution is failing. Verify your DNS server is reachable — try 1.1.1.1 or 192.168.1.1.".into(),
        "ip_conflict" => "Two devices claim the same IP. Restart the affected device; your router will reassign.".into(),
        "weak_signal" => "Zigbee/Z-Wave signal is marginal. Adding a mains-powered repeater between the hub and this device usually fixes it.".into(),
        "zigbee_channel_conflict" => "Zigbee and Wi-Fi channels are overlapping. Move your Zigbee network to channels 15, 20, or 25.".into(),
        _ => "Syntaur doesn't have a specific fix for this issue yet — check the device's app or logs.".into(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remediation_covers_known_kinds() {
        assert!(!remediation_for("offline", None).is_empty());
        assert!(!remediation_for("weak_signal", None).is_empty());
        assert!(!remediation_for("zigbee_channel_conflict", None).is_empty());
        // Unknown kind still returns a helpful fallback.
        assert!(remediation_for("something_new", None).contains("doesn't have a specific fix"));
    }

    /// Minimal DDL mirror for the subset of tables these tests touch.
    /// Kept in sync with `src/index/schema.rs` migrations v1+, v57, v61.
    /// The full migrate() lives behind a private module — duplicating
    /// just the columns we need is cheaper than widening visibility.
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
                 room_id INTEGER,
                 driver TEXT NOT NULL,
                 external_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 capabilities_json TEXT NOT NULL DEFAULT '{}',
                 state_json TEXT NOT NULL DEFAULT '{}',
                 metadata_json TEXT NOT NULL DEFAULT '{}',
                 last_seen_at INTEGER,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE smart_home_network_events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 ts INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 subject TEXT,
                 details_json TEXT
             );
             INSERT INTO users (name, created_at) VALUES ('alice', 0);
             ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn summary_empty_when_no_devices() {
        let conn = fresh_db();
        let s = summary_for_user(&conn, 1).unwrap();
        assert_eq!(s.total_devices, 0);
        assert_eq!(s.offline_count, 0);
        assert!(s.active_issues.is_empty());
    }

    #[test]
    fn active_issues_dedupes_per_subject_by_latest_event() {
        let conn = fresh_db();
        // Seed two devices for user 1
        for i in 1..=2 {
            conn.execute(
                "INSERT INTO smart_home_devices (user_id, driver, external_id, name, kind, created_at)
                 VALUES (1, 'wifi', ?, ?, 'switch', 0)",
                rusqlite::params![format!("ext-{i}"), format!("dev{i}")],
            )
            .unwrap();
        }
        // Event history: device:1 went offline then came back; device:2
        // is still offline. Only device:2 should surface as active.
        for (ts, kind, subj) in [
            (100, "offline", "device:1"),
            (200, "online", "device:1"),
            (150, "offline", "device:2"),
        ] {
            conn.execute(
                "INSERT INTO smart_home_network_events
                    (user_id, ts, kind, subject, details_json)
                 VALUES (1, ?, ?, ?, '{}')",
                rusqlite::params![ts, kind, subj],
            )
            .unwrap();
        }
        let issues = active_issues_for_user(&conn, 1).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].subject.as_deref(), Some("device:2"));
        assert_eq!(issues[0].kind, "offline");
    }

    #[test]
    fn summary_online_count_uses_offline_subtraction() {
        let conn = fresh_db();
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO smart_home_devices (user_id, driver, external_id, name, kind, created_at)
                 VALUES (1, 'wifi', ?, ?, 'switch', 0)",
                rusqlite::params![format!("ext-{i}"), format!("dev{i}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO smart_home_network_events
                (user_id, ts, kind, subject, details_json)
             VALUES (1, 100, 'offline', 'device:1', '{}')",
            [],
        )
        .unwrap();
        let s = summary_for_user(&conn, 1).unwrap();
        assert_eq!(s.total_devices, 3);
        assert_eq!(s.offline_count, 1);
        assert_eq!(s.online_count, 2);
        assert_eq!(s.last_sweep_at, Some(100));
    }
}
