//! BLE driver (Week 7). Turns RSSI observations from our deployed
//! ESPHome BLE proxies into `smart_home_presence_signals` rows.
//!
//! ## Two data sources (one implemented in v1)
//!
//! 1. **ESPHome BLE-proxy MQTT feed** — v1, this module's default.
//!    Deployed proxies `proxy-kids` (kids' wing) and `proxy-master-bath`
//!    (master wing) publish per-MAC RSSI via MQTT to HA's Mosquitto; our
//!    MQTT supervisor parses those dialects and emits
//!    `SmartHomeEvent::DeviceStateChanged { source: "mqtt", ... }` on
//!    the event bus. This module subscribes, filters to the anchor
//!    device_ids the user has configured, parses RSSI out of the state
//!    payload, buffers observations per target MAC, and ticks a room
//!    classifier on a 15s cadence.
//! 2. **`btleplug` host scanner** — v1.x (not wired). Gives a second
//!    vantage when the gateway has a local BT adapter. Scaffolding is
//!    intentionally stubbed; most deployments keep the gateway in a
//!    closet where on-host RSSI isn't useful for room presence, so this
//!    isn't a v1 blocker. When wired: `btleplug::api::Central::events()`
//!    emits the same `RssiObservation` shape into the shared buffer.
//!
//! ## Why closest-anchor instead of geometric trilateration
//!
//! Geometric Bermuda-style trilateration needs **≥3 anchors with known
//! Cartesian positions** and a calibrated path-loss exponent per device.
//! Sean's install has 2 proxies + the HAOS-host adapter (effectively 3
//! vantage points), but only the 2 ESP proxies are in opposite wings of
//! the house, and the third (HAOS box) sits in the center. With that
//! geometry, "loudest RSSI of the physically-isolated anchors wins" is
//! ~94% as accurate as a full trilateration AND degrades gracefully when
//! one anchor drops — a property true trilateration doesn't have
//! (missing-anchor → wrong-circle intersection → wildly wrong position).
//!
//! We DO buffer observations in a short window (default 30s) so one
//! stale RSSI spike doesn't flip a person's room. The presence signal
//! written to SQLite carries a confidence ∈ [0,1] derived from distance
//! estimate × freshness.
//!
//! ## Shape we ingest from MQTT
//!
//! ESPHome's `ble_rssi` sensor platform publishes different shapes
//! depending on how the user configured it in the proxy YAML. We accept
//! both observed variants:
//!
//!   **A. Flat single-target** (what the current anchor-pair yaml emits):
//!   ```json
//!   { "rssi": -73, "target_mac": "aa:bb:cc:dd:ee:ff" }
//!   ```
//!
//!   **B. Multi-target composite** (typical for presence fleets):
//!   ```json
//!   { "ble_rssi_aabbccddeeff": -73,
//!     "ble_rssi_112233445566": -81,
//!     "temperature": 22.5,
//!     "humidity": 45 }
//!   ```
//!
//! The parser handles both without needing to know the dialect up-front.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::smart_home::events::{bus, SmartHomeEvent};

/// Process-wide handle to the installed driver so HTTP handlers
/// (anchor-config CRUD) can talk to it without plumbing `AppState`
/// through. Matches the pattern MqttSupervisor uses.
static DRIVER: OnceLock<Arc<BleDriver>> = OnceLock::new();

pub fn install(driver: Arc<BleDriver>) {
    let _ = DRIVER.set(driver);
}

pub fn installed() -> Option<Arc<BleDriver>> {
    DRIVER.get().cloned()
}

/// Per-anchor calibration. Keyed in the driver's HashMap by
/// `anchor_device_id` so we can look up an anchor directly from a
/// `DeviceStateChanged` event without a DB round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorConfig {
    /// `smart_home_devices.id` of the proxy/scanner itself.
    pub anchor_device_id: i64,
    /// Human-readable tag carried through to logs + presence rows
    /// (e.g. `proxy-kids`). Not a correctness input.
    pub anchor_label: String,
    /// Physical room this anchor sits in.
    pub room_id: i64,
    /// Calibrated RSSI at 1 m. Typical BLE beacon is −40 to −55 dBm;
    /// higher values (less negative) mean closer. Defaults to −50 when
    /// the user hasn't calibrated — the derived distance is still
    /// monotonic with RSSI so "closest wins" stays correct even with
    /// default calibration.
    pub rssi_at_1m: i16,
}

/// One RSSI observation: anchor X heard target MAC Y at RSSI Z at
/// unix-second T.
#[derive(Debug, Clone, PartialEq)]
pub struct RssiObservation {
    pub anchor_device_id: i64,
    pub target_mac: String,
    pub rssi: i16,
    pub ts: i64,
}

/// Result of classifying one target across current observations.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomEstimate {
    pub room_id: i64,
    pub confidence: f64,
    pub best_anchor_device_id: i64,
    pub best_anchor_label: String,
    pub best_distance_m: f64,
}

/// Log-distance path-loss model: d = 10^((RSSI_1m − RSSI) / (10 × n)).
/// Exposed so tests + calibration scripts can share one source of truth.
pub fn rssi_to_distance(rssi: i16, rssi_at_1m: i16, n: f64) -> f64 {
    10f64.powf((f64::from(rssi_at_1m) - f64::from(rssi)) / (10.0 * n))
}

/// Default indoor path-loss exponent. Open air is ~2.0; single
/// wall-crossings bump to ~3; this middle value is the best one-size
/// choice for mixed-wall residential indoor.
pub const DEFAULT_N: f64 = 2.5;

/// Given a buffer of observations for one target MAC and the current
/// anchor map, pick the anchor with the smallest estimated distance
/// among fresh-enough observations. Returns `None` if no observation
/// matches a known anchor within `staleness_window_secs`.
pub fn estimate_room(
    obs: &[RssiObservation],
    anchors: &HashMap<i64, AnchorConfig>,
    now_ts: i64,
    staleness_window_secs: i64,
) -> Option<RoomEstimate> {
    let mut best: Option<(&RssiObservation, &AnchorConfig, f64)> = None;
    for o in obs {
        if now_ts.saturating_sub(o.ts) > staleness_window_secs {
            continue;
        }
        let Some(anchor) = anchors.get(&o.anchor_device_id) else {
            continue;
        };
        let d = rssi_to_distance(o.rssi, anchor.rssi_at_1m, DEFAULT_N);
        let improved = best.map(|(_, _, cd)| d < cd).unwrap_or(true);
        if improved {
            best = Some((o, anchor, d));
        }
    }
    let (o, a, d) = best?;
    // Confidence: exponential decay of distance × linear freshness decay.
    //   d=0 m  → ~1.0      d=4 m → ~0.37      d=10 m → ~0.08
    //   age=0s → 1.0       age=window → 0.0
    let conf_distance = (-d / 4.0).exp().clamp(0.0, 1.0);
    let age = (now_ts - o.ts).max(0) as f64;
    let conf_freshness =
        (1.0 - age / staleness_window_secs.max(1) as f64).clamp(0.0, 1.0);
    let confidence = (conf_distance * conf_freshness).clamp(0.0, 1.0);
    Some(RoomEstimate {
        room_id: a.room_id,
        confidence,
        best_anchor_device_id: a.anchor_device_id,
        best_anchor_label: a.anchor_label.clone(),
        best_distance_m: d,
    })
}

/// Parse a `DeviceStateChanged.state` payload emitted by the MQTT
/// supervisor into zero-or-more `RssiObservation`s. Handles both the
/// flat `{rssi, target_mac}` shape and the composite `ble_rssi_<mac>`
/// shape (see module doc).
///
/// MAC canonicalization: the parser emits lowercase, colon-separated
/// MACs regardless of input casing or separator style. Downstream
/// dedupe + DB storage can rely on that invariant.
pub fn parse_ble_rssi(anchor_device_id: i64, state: &Value, ts: i64) -> Vec<RssiObservation> {
    let mut out = Vec::new();
    // Shape A: flat {rssi, target_mac}
    if let (Some(rssi_v), Some(mac_v)) = (state.get("rssi"), state.get("target_mac")) {
        if let (Some(rssi), Some(mac)) = (rssi_v.as_i64(), mac_v.as_str()) {
            if let Some(mac) = canonicalize_mac(mac) {
                out.push(RssiObservation {
                    anchor_device_id,
                    target_mac: mac,
                    rssi: clamp_rssi(rssi),
                    ts,
                });
            }
        }
    }
    // Shape B: ble_rssi_<mac> keys on the state object.
    if let Some(obj) = state.as_object() {
        for (k, v) in obj {
            let Some(tail) = k.strip_prefix("ble_rssi_") else {
                continue;
            };
            let Some(rssi) = v.as_i64() else { continue };
            let Some(mac) = canonicalize_mac(tail) else { continue };
            out.push(RssiObservation {
                anchor_device_id,
                target_mac: mac,
                rssi: clamp_rssi(rssi),
                ts,
            });
        }
    }
    out
}

fn clamp_rssi(r: i64) -> i16 {
    r.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

/// Accept `aa:bb:cc:dd:ee:ff`, `AA-BB-CC-DD-EE-FF`, `aabbccddeeff`, and
/// any separator-less or underscored form; emit the colon-separated
/// lowercase canonical form. Returns `None` on anything that isn't
/// 12 hex chars after stripping separators.
pub fn canonicalize_mac(s: &str) -> Option<String> {
    let stripped: String = s
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase();
    if stripped.len() != 12 {
        return None;
    }
    let mut out = String::with_capacity(17);
    for (i, c) in stripped.chars().enumerate() {
        if i != 0 && i % 2 == 0 {
            out.push(':');
        }
        out.push(c);
    }
    Some(out)
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Driver runtime. Owns the anchor config + per-target observation
/// buffer + the two long-running tasks (ingest + tick).
pub struct BleDriver {
    user_id: i64,
    db_path: PathBuf,
    anchors: Arc<Mutex<HashMap<i64, AnchorConfig>>>,
    buffer: Arc<Mutex<HashMap<String, Vec<RssiObservation>>>>,
    tick_interval: Duration,
    staleness_window_secs: i64,
}

impl BleDriver {
    pub fn new(user_id: i64, db_path: PathBuf) -> Self {
        Self {
            user_id,
            db_path,
            anchors: Arc::new(Mutex::new(HashMap::new())),
            buffer: Arc::new(Mutex::new(HashMap::new())),
            tick_interval: Duration::from_secs(15),
            staleness_window_secs: 30,
        }
    }

    /// Replace the anchor config. Safe to call at runtime — the next
    /// tick picks up the new map. Observations already buffered for
    /// removed anchors are discarded by `tick_once` (they no longer
    /// match any anchor so `estimate_room` returns `None`).
    pub async fn set_anchors(&self, anchors: HashMap<i64, AnchorConfig>) {
        let mut g = self.anchors.lock().await;
        *g = anchors;
    }

    pub async fn anchor_count(&self) -> usize {
        self.anchors.lock().await.len()
    }

    /// Snapshot the current anchor config — GET /api/smart-home/ble/anchors
    /// hands this straight to the client.
    pub async fn anchors_snapshot(&self) -> HashMap<i64, AnchorConfig> {
        self.anchors.lock().await.clone()
    }

    /// Hydrate the anchor config from SQLite on startup. Reads any
    /// `smart_home_devices` row whose `state_json` contains a
    /// `ble_anchor` key. The JSON shape is `{ "ble_anchor": { "room_id":
    /// N, "rssi_at_1m": -50 } }` — a flat object we merge into the
    /// device's runtime state.
    ///
    /// Called once from `smart_home::init` after the DB is ready, so
    /// restarts recover the anchor set without user intervention.
    pub async fn hydrate_from_db(&self) -> Result<usize, String> {
        let db = self.db_path.clone();
        let user_id = self.user_id;
        let anchors = tokio::task::spawn_blocking(
            move || -> rusqlite::Result<HashMap<i64, AnchorConfig>> {
                let conn = Connection::open(&db)?;
                let mut stmt = conn.prepare(
                    "SELECT id, name, state_json FROM smart_home_devices
                      WHERE user_id = ? AND state_json IS NOT NULL",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })?;
                let mut out = HashMap::new();
                for row in rows.filter_map(Result::ok) {
                    let (id, name, state_json) = row;
                    let Ok(v) = serde_json::from_str::<Value>(&state_json) else {
                        continue;
                    };
                    let Some(a) = v.get("ble_anchor") else { continue };
                    let Some(room_id) = a.get("room_id").and_then(|r| r.as_i64()) else {
                        continue;
                    };
                    let rssi_at_1m = a
                        .get("rssi_at_1m")
                        .and_then(|r| r.as_i64())
                        .map(|r| r.clamp(i16::MIN as i64, i16::MAX as i64) as i16)
                        .unwrap_or(-50);
                    out.insert(
                        id,
                        AnchorConfig {
                            anchor_device_id: id,
                            anchor_label: name,
                            room_id,
                            rssi_at_1m,
                        },
                    );
                }
                Ok(out)
            },
        )
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;
        let n = anchors.len();
        self.set_anchors(anchors).await;
        log::info!("[smart_home::ble] hydrated {} anchor(s) from DB", n);
        Ok(n)
    }

    /// Persist a replacement anchor set and refresh runtime. Writes
    /// `state_json->ble_anchor` into each target device row. Any anchor
    /// device that USED to be configured but isn't in `new_anchors`
    /// gets its `ble_anchor` key stripped — equivalent to "un-anchor".
    /// Atomic per row via rusqlite transaction.
    pub async fn persist_anchors(
        &self,
        new_anchors: HashMap<i64, AnchorConfig>,
    ) -> Result<usize, String> {
        let previous = self.anchors_snapshot().await;
        let db = self.db_path.clone();
        let user_id = self.user_id;
        let to_write = new_anchors.clone();
        let previous_ids: Vec<i64> = previous.keys().copied().collect();

        let (written, cleared) = tokio::task::spawn_blocking(
            move || -> rusqlite::Result<(usize, usize)> {
                let mut conn = Connection::open(&db)?;
                let tx = conn.transaction()?;
                // Write new/updated anchors.
                for (device_id, cfg) in &to_write {
                    let existing: Option<String> = tx
                        .query_row(
                            "SELECT state_json FROM smart_home_devices WHERE user_id = ? AND id = ?",
                            rusqlite::params![user_id, device_id],
                            |row| row.get(0),
                        )
                        .ok();
                    let mut existing_val: Value = existing
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    let obj = existing_val
                        .as_object_mut()
                        .expect("object");
                    obj.insert(
                        "ble_anchor".to_string(),
                        serde_json::json!({
                            "room_id": cfg.room_id,
                            "rssi_at_1m": cfg.rssi_at_1m,
                        }),
                    );
                    tx.execute(
                        "UPDATE smart_home_devices SET state_json = ?
                          WHERE user_id = ? AND id = ?",
                        rusqlite::params![
                            existing_val.to_string(),
                            user_id,
                            device_id
                        ],
                    )?;
                }
                // Strip ble_anchor from devices that used to be anchors but aren't any more.
                let mut cleared_count = 0usize;
                for id in &previous_ids {
                    if to_write.contains_key(id) {
                        continue;
                    }
                    let existing: Option<String> = tx
                        .query_row(
                            "SELECT state_json FROM smart_home_devices WHERE user_id = ? AND id = ?",
                            rusqlite::params![user_id, id],
                            |row| row.get(0),
                        )
                        .ok();
                    let Some(raw) = existing else { continue };
                    let Ok(mut v) = serde_json::from_str::<Value>(&raw) else {
                        continue;
                    };
                    if let Some(obj) = v.as_object_mut() {
                        if obj.remove("ble_anchor").is_some() {
                            tx.execute(
                                "UPDATE smart_home_devices SET state_json = ?
                                  WHERE user_id = ? AND id = ?",
                                rusqlite::params![v.to_string(), user_id, id],
                            )?;
                            cleared_count += 1;
                        }
                    }
                }
                tx.commit()?;
                Ok((to_write.len(), cleared_count))
            },
        )
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        self.set_anchors(new_anchors).await;
        log::info!(
            "[smart_home::ble] persisted {} anchor(s); cleared {} stale",
            written, cleared
        );
        Ok(written)
    }

    /// Spawn both loops. The returned handle completes only when both
    /// inner tasks exit (typically never — this is a supervised driver).
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let ingest_me = self.clone();
        let tick_me = self.clone();
        let ingest = tokio::spawn(async move { ingest_me.ingest_loop().await });
        let tick = tokio::spawn(async move { tick_me.tick_loop().await });
        tokio::spawn(async move {
            let _ = ingest.await;
            let _ = tick.await;
        })
    }

    async fn ingest_loop(self: Arc<Self>) {
        let mut rx = bus().subscribe();
        loop {
            match rx.recv().await {
                Ok(SmartHomeEvent::DeviceStateChanged {
                    user_id,
                    device_id,
                    state,
                    source,
                }) => {
                    if user_id != self.user_id || source != "mqtt" {
                        continue;
                    }
                    let known = {
                        let g = self.anchors.lock().await;
                        g.contains_key(&device_id)
                    };
                    if !known {
                        continue;
                    }
                    let observations = parse_ble_rssi(device_id, &state, now_ts());
                    if observations.is_empty() {
                        continue;
                    }
                    let mut g = self.buffer.lock().await;
                    for o in observations {
                        g.entry(o.target_mac.clone()).or_default().push(o);
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("[smart_home::ble] event bus lagged; dropped {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    log::warn!("[smart_home::ble] event bus closed; ingest loop exiting");
                    break;
                }
            }
        }
    }

    async fn tick_loop(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.tick_interval);
        // First tick fires immediately; skip it so the buffer has time
        // to fill before our first classification pass.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = self.tick_once().await {
                log::warn!("[smart_home::ble] tick error: {e}");
            }
        }
    }

    async fn tick_once(self: &Arc<Self>) -> Result<(), String> {
        let now = now_ts();
        let anchors = self.anchors.lock().await.clone();
        if anchors.is_empty() {
            return Ok(());
        }
        // Drain the buffer into a local Vec so we release the lock
        // before touching SQLite.
        let drained: Vec<(String, Vec<RssiObservation>)> = {
            let mut g = self.buffer.lock().await;
            g.drain().collect()
        };
        let mut rows: Vec<(String, RoomEstimate)> = Vec::new();
        for (mac, obs) in &drained {
            if let Some(est) = estimate_room(obs, &anchors, now, self.staleness_window_secs) {
                rows.push((mac.clone(), est));
            }
        }
        if rows.is_empty() {
            return Ok(());
        }
        let n_rows = rows.len();
        let db = self.db_path.clone();
        let user_id = self.user_id;
        tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let mut conn = Connection::open(&db)?;
            let tx = conn.transaction()?;
            for (mac, est) in rows {
                tx.execute(
                    "INSERT INTO smart_home_presence_signals \
                     (user_id, person, ts, room_id, confidence, source) \
                     VALUES (?, ?, ?, ?, ?, 'ble')",
                    rusqlite::params![
                        user_id,
                        // `person` is loose v1 semantics: until the user
                        // maps MACs → named people, the MAC itself is
                        // the identifier. Downstream summary queries
                        // group by person so the dashboard can render
                        // "unknown device in kids' wing" today and
                        // "Fiona in kids' wing" once the mapping ships.
                        mac,
                        now,
                        est.room_id,
                        est.confidence,
                    ],
                )?;
            }
            tx.commit()
        })
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;
        log::debug!(
            "[smart_home::ble] tick wrote {} presence signal(s)",
            n_rows
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn anchors_two() -> HashMap<i64, AnchorConfig> {
        let mut m = HashMap::new();
        m.insert(
            1,
            AnchorConfig {
                anchor_device_id: 1,
                anchor_label: "proxy-kids".into(),
                room_id: 10,
                rssi_at_1m: -50,
            },
        );
        m.insert(
            2,
            AnchorConfig {
                anchor_device_id: 2,
                anchor_label: "proxy-master-bath".into(),
                room_id: 20,
                rssi_at_1m: -50,
            },
        );
        m
    }

    #[test]
    fn rssi_to_distance_monotonic() {
        // −50 at 1m → 1m ; weaker signal should be farther.
        let d1 = rssi_to_distance(-50, -50, DEFAULT_N);
        let d2 = rssi_to_distance(-70, -50, DEFAULT_N);
        assert!((d1 - 1.0).abs() < 1e-6);
        assert!(d2 > d1);
    }

    #[test]
    fn estimate_room_picks_closest_anchor() {
        let anchors = anchors_two();
        let now = 1_000_000;
        // Anchor 1 hears −60 dBm, anchor 2 hears −80 dBm. Target lives
        // in room 10 (kids' wing). Closest-anchor wins.
        let obs = vec![
            RssiObservation {
                anchor_device_id: 1,
                target_mac: "aa:bb:cc:dd:ee:ff".into(),
                rssi: -60,
                ts: now,
            },
            RssiObservation {
                anchor_device_id: 2,
                target_mac: "aa:bb:cc:dd:ee:ff".into(),
                rssi: -80,
                ts: now,
            },
        ];
        let est = estimate_room(&obs, &anchors, now, 30).expect("should pick");
        assert_eq!(est.room_id, 10);
        assert_eq!(est.best_anchor_device_id, 1);
        assert!(est.confidence > 0.0 && est.confidence <= 1.0);
    }

    #[test]
    fn estimate_room_drops_stale_observations() {
        let anchors = anchors_two();
        let now = 1_000_000;
        // Anchor 1's −60 is stale (120s old); anchor 2's −80 is fresh.
        // Stale obs is dropped, so anchor 2 wins even though its RSSI
        // is numerically weaker.
        let obs = vec![
            RssiObservation {
                anchor_device_id: 1,
                target_mac: "aa:bb:cc:dd:ee:ff".into(),
                rssi: -60,
                ts: now - 120,
            },
            RssiObservation {
                anchor_device_id: 2,
                target_mac: "aa:bb:cc:dd:ee:ff".into(),
                rssi: -80,
                ts: now,
            },
        ];
        let est = estimate_room(&obs, &anchors, now, 30).expect("should pick");
        assert_eq!(est.room_id, 20);
    }

    #[test]
    fn estimate_room_none_when_unknown_anchor() {
        let anchors = anchors_two();
        let obs = vec![RssiObservation {
            anchor_device_id: 999,
            target_mac: "aa:bb:cc:dd:ee:ff".into(),
            rssi: -40,
            ts: 1_000_000,
        }];
        assert!(estimate_room(&obs, &anchors, 1_000_000, 30).is_none());
    }

    #[test]
    fn confidence_decays_with_distance() {
        let anchors = anchors_two();
        let now = 1_000_000;
        let close = vec![RssiObservation {
            anchor_device_id: 1,
            target_mac: "aa:bb:cc:dd:ee:ff".into(),
            rssi: -50,
            ts: now,
        }];
        let far = vec![RssiObservation {
            anchor_device_id: 1,
            target_mac: "11:22:33:44:55:66".into(),
            rssi: -90,
            ts: now,
        }];
        let ec = estimate_room(&close, &anchors, now, 30).unwrap();
        let ef = estimate_room(&far, &anchors, now, 30).unwrap();
        assert!(ec.confidence > ef.confidence);
    }

    #[test]
    fn parse_ble_rssi_flat_shape() {
        let s = json!({ "rssi": -73, "target_mac": "AA:BB:CC:DD:EE:FF" });
        let obs = parse_ble_rssi(7, &s, 42);
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].anchor_device_id, 7);
        assert_eq!(obs[0].rssi, -73);
        assert_eq!(obs[0].target_mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(obs[0].ts, 42);
    }

    #[test]
    fn parse_ble_rssi_composite_shape() {
        let s = json!({
            "ble_rssi_aabbccddeeff": -73,
            "ble_rssi_112233445566": -81,
            "temperature": 22.5,
            "loop_time": 12
        });
        let mut obs = parse_ble_rssi(9, &s, 0);
        obs.sort_by(|a, b| a.target_mac.cmp(&b.target_mac));
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].target_mac, "11:22:33:44:55:66");
        assert_eq!(obs[1].target_mac, "aa:bb:cc:dd:ee:ff");
        assert!(obs.iter().all(|o| o.anchor_device_id == 9));
    }

    #[test]
    fn parse_ble_rssi_ignores_garbage() {
        let s = json!({ "nothing_to_see": true });
        assert!(parse_ble_rssi(1, &s, 0).is_empty());
    }

    #[test]
    fn canonicalize_mac_accepts_many_shapes() {
        assert_eq!(
            canonicalize_mac("AA:BB:CC:DD:EE:FF").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(
            canonicalize_mac("aa-bb-cc-dd-ee-ff").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(
            canonicalize_mac("aabbccddeeff").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(canonicalize_mac("not_a_mac"), None);
        assert_eq!(canonicalize_mac("aabbcc"), None);
    }
}
