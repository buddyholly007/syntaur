//! Hash-diff cache for the long-running MQTT subscriber.
//!
//! Why this exists: retained-message catch-up and chatty sensors (energy
//! plugs that republish identical wattage every 10s) would saturate the
//! 256-capacity `SmartHomeEvent` broadcast channel. The cache keeps the
//! hash of the last state we emitted per `(driver, external_id)` and
//! only re-publishes when the incoming frame differs. The same gate
//! applies to `Availability` transitions.
//!
//! Side effects on emit:
//!   1. Shallow-merge the update into `smart_home_devices.state_json`
//!      (so `tele/.../SENSOR` and `tele/.../STATE` both land without
//!      clobbering each other) and refresh `last_seen_at`.
//!   2. Return `EmittedChange` so the caller can fire a
//!      `SmartHomeEvent::DeviceStateChanged` on the module-wide bus.
//!
//! If the device isn't commissioned yet (no row in `smart_home_devices`
//! for the `(driver, external_id)` key), we silently drop the frame —
//! a scan is expected to land first. That keeps the subscriber from
//! fighting with the discovery pipeline during reconnect storms.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use tokio::sync::RwLock;

use super::dialects::DeviceStateUpdate;

/// Result of an apply that changed cached state. The caller publishes a
/// `SmartHomeEvent::DeviceStateChanged` with these fields.
#[derive(Debug, Clone)]
pub struct EmittedChange {
    pub user_id: i64,
    pub device_id: i64,
    pub state: Value,
    pub source: String,
}

pub struct StateCache {
    db_path: PathBuf,
    /// `(driver, external_id) -> last emitted state hash`.
    state_hashes: Arc<RwLock<HashMap<(String, String), u64>>>,
    /// `(driver, external_id) -> last availability flag`. Kept separate
    /// so an Availability change doesn't invalidate the state hash.
    availability: Arc<RwLock<HashMap<(String, String), bool>>>,
}

impl StateCache {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            state_hashes: Arc::new(RwLock::new(HashMap::new())),
            availability: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Apply one state update. Returns `Ok(Some(change))` when the
    /// merged result differs from the last emission, `Ok(None)` when
    /// suppressed (no diff / device not commissioned). Never panics on
    /// DB errors — they return `Err` but don't poison the cache.
    pub async fn apply_state(
        &self,
        driver: &str,
        update: DeviceStateUpdate,
    ) -> Result<Option<EmittedChange>, String> {
        let key = (driver.to_string(), update.external_id.clone());
        let incoming_hash = hash_value(&update.state);
        {
            let hashes = self.state_hashes.read().await;
            if hashes.get(&key).copied() == Some(incoming_hash) {
                return Ok(None);
            }
        }

        let db = self.db_path.clone();
        let driver_s = driver.to_string();
        let external_id = update.external_id.clone();
        let incoming = update.state.clone();
        let merged = tokio::task::spawn_blocking(
            move || -> Result<Option<(i64, i64, Value)>, String> {
                let conn = Connection::open(&db).map_err(|e| e.to_string())?;
                let row: Option<(i64, i64, String)> = conn
                    .query_row(
                        "SELECT id, user_id, state_json
                           FROM smart_home_devices
                          WHERE driver = ? AND external_id = ?",
                        params![driver_s, external_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let Some((id, user_id, current_json)) = row else {
                    return Ok(None);
                };
                let current: Value =
                    serde_json::from_str(&current_json).unwrap_or_else(|_| Value::Null);
                let merged = shallow_merge(&current, &incoming);
                let merged_s =
                    serde_json::to_string(&merged).map_err(|e| e.to_string())?;
                let ts = chrono::Utc::now().timestamp();
                conn.execute(
                    "UPDATE smart_home_devices
                        SET state_json = ?, last_seen_at = ?
                      WHERE id = ?",
                    params![merged_s, ts, id],
                )
                .map_err(|e| e.to_string())?;
                Ok(Some((user_id, id, merged)))
            },
        )
        .await
        .map_err(|e| format!("join: {e}"))??;

        let Some((user_id, device_id, merged)) = merged else {
            // Not commissioned yet — drop without marking the hash so
            // the next frame after scan can fire cleanly.
            return Ok(None);
        };

        {
            let mut hashes = self.state_hashes.write().await;
            hashes.insert(key, incoming_hash);
        }

        Ok(Some(EmittedChange {
            user_id,
            device_id,
            state: merged,
            source: update.source,
        }))
    }

    /// Apply one availability flag. Returns `Some` only on transition
    /// (including first-ever observation). Persists online=false under
    /// `_availability.online = false` on the state so dashboards can
    /// filter without a second query.
    pub async fn apply_availability(
        &self,
        driver: &str,
        external_id: String,
        online: bool,
    ) -> Result<Option<EmittedChange>, String> {
        let key = (driver.to_string(), external_id.clone());
        {
            let avail = self.availability.read().await;
            if avail.get(&key).copied() == Some(online) {
                return Ok(None);
            }
        }

        let db = self.db_path.clone();
        let driver_s = driver.to_string();
        let ext = external_id.clone();
        let merged = tokio::task::spawn_blocking(
            move || -> Result<Option<(i64, i64, Value)>, String> {
                let conn = Connection::open(&db).map_err(|e| e.to_string())?;
                let row: Option<(i64, i64, String)> = conn
                    .query_row(
                        "SELECT id, user_id, state_json
                           FROM smart_home_devices
                          WHERE driver = ? AND external_id = ?",
                        params![driver_s, ext],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let Some((id, user_id, current_json)) = row else {
                    return Ok(None);
                };
                let current: Value =
                    serde_json::from_str(&current_json).unwrap_or_else(|_| Value::Null);
                let availability = serde_json::json!({
                    "_availability": { "online": online }
                });
                let merged = shallow_merge(&current, &availability);
                let merged_s =
                    serde_json::to_string(&merged).map_err(|e| e.to_string())?;
                let ts = chrono::Utc::now().timestamp();
                conn.execute(
                    "UPDATE smart_home_devices
                        SET state_json = ?, last_seen_at = ?
                      WHERE id = ?",
                    params![merged_s, ts, id],
                )
                .map_err(|e| e.to_string())?;
                Ok(Some((user_id, id, merged)))
            },
        )
        .await
        .map_err(|e| format!("join: {e}"))??;

        let Some((user_id, device_id, merged)) = merged else {
            return Ok(None);
        };

        {
            let mut avail = self.availability.write().await;
            avail.insert(key, online);
        }

        Ok(Some(EmittedChange {
            user_id,
            device_id,
            state: merged,
            source: driver.to_string(),
        }))
    }
}

/// Stable hash for `serde_json::Value`. Not cryptographic — only used
/// to detect "frame contents identical to what we just emitted."
fn hash_value(v: &Value) -> u64 {
    let s = v.to_string();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Shallow merge: for object values, `incoming`'s top-level keys
/// overwrite `current`'s keys; non-object incoming replaces entirely.
/// Chosen over deep merge because dialect payloads (Tasmota STATE,
/// Shelly RPC) are flat by convention — deep merging would hide sensor
/// removal.
fn shallow_merge(current: &Value, incoming: &Value) -> Value {
    match (current, incoming) {
        (Value::Object(c), Value::Object(i)) => {
            let mut out = c.clone();
            for (k, v) in i {
                out.insert(k.clone(), v.clone());
            }
            Value::Object(out)
        }
        _ => incoming.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;
    use tempfile::TempDir;

    fn test_db_with_device(external_id: &str, initial_state: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE smart_home_devices (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                room_id INTEGER,
                driver TEXT NOT NULL,
                external_id TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                capabilities_json TEXT NOT NULL DEFAULT '{{}}',
                state_json TEXT NOT NULL DEFAULT '{{}}',
                metadata_json TEXT NOT NULL DEFAULT '{{}}',
                last_seen_at INTEGER,
                created_at INTEGER NOT NULL,
                UNIQUE(user_id, driver, external_id)
            );
            INSERT INTO smart_home_devices
                (user_id, driver, external_id, name, kind, state_json, created_at)
              VALUES
                (1, 'mqtt', '{ext}', 'test', 'switch', '{state}', 0);",
            ext = external_id,
            state = initial_state,
        ))
        .unwrap();
        (tmp, path)
    }

    #[tokio::test]
    async fn diff_emits_only_on_change() {
        let (_tmp, db) = test_db_with_device("tasmota_topic:plug", "{}");
        let cache = StateCache::new(db);
        let u = DeviceStateUpdate {
            external_id: "tasmota_topic:plug".into(),
            state: json!({"POWER": "ON"}),
            source: "tasmota".into(),
        };

        let first = cache.apply_state("mqtt", u.clone()).await.unwrap();
        assert!(first.is_some(), "first apply should emit");
        let second = cache.apply_state("mqtt", u.clone()).await.unwrap();
        assert!(second.is_none(), "identical apply should suppress");

        let changed = DeviceStateUpdate {
            external_id: "tasmota_topic:plug".into(),
            state: json!({"POWER": "OFF"}),
            source: "tasmota".into(),
        };
        let third = cache.apply_state("mqtt", changed).await.unwrap();
        assert!(third.is_some(), "changed apply should emit");
    }

    #[tokio::test]
    async fn unknown_device_is_dropped() {
        let (_tmp, db) = test_db_with_device("tasmota_topic:known", "{}");
        let cache = StateCache::new(db);
        let u = DeviceStateUpdate {
            external_id: "tasmota_topic:ghost".into(),
            state: json!({"x": 1}),
            source: "tasmota".into(),
        };
        let out = cache.apply_state("mqtt", u).await.unwrap();
        assert!(out.is_none(), "unknown device emits nothing");
    }

    #[tokio::test]
    async fn availability_emits_only_on_transition() {
        let (_tmp, db) = test_db_with_device("tasmota_topic:plug", "{}");
        let cache = StateCache::new(db);
        let a = cache
            .apply_availability("mqtt", "tasmota_topic:plug".into(), true)
            .await
            .unwrap();
        assert!(a.is_some());
        let b = cache
            .apply_availability("mqtt", "tasmota_topic:plug".into(), true)
            .await
            .unwrap();
        assert!(b.is_none(), "same flag suppresses");
        let c = cache
            .apply_availability("mqtt", "tasmota_topic:plug".into(), false)
            .await
            .unwrap();
        assert!(c.is_some(), "transition emits");
    }

    #[tokio::test]
    async fn shallow_merge_preserves_prior_keys() {
        let (_tmp, db) =
            test_db_with_device("tasmota_topic:plug", "{\"POWER\":\"ON\",\"Dimmer\":50}");
        let cache = StateCache::new(db);
        let u = DeviceStateUpdate {
            external_id: "tasmota_topic:plug".into(),
            state: json!({"Dimmer": 80}),
            source: "tasmota".into(),
        };
        let out = cache.apply_state("mqtt", u).await.unwrap().unwrap();
        assert_eq!(out.state["POWER"], "ON");
        assert_eq!(out.state["Dimmer"], 80);
    }
}
