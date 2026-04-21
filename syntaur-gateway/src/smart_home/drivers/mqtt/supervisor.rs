//! MQTT supervisor — the entry point hooked into `smart_home::init`.
//!
//! Responsibilities:
//!   1. Iterate `smart_home_credentials` rows with `provider = 'mqtt'`
//!      and spawn one long-running `MqttSession` per row.
//!   2. Honor the legacy `SMART_HOME_MQTT_URL` env var as a dev-only
//!      fallback when no credentials are configured. Logs `DEPRECATED`
//!      so operators migrate.
//!   3. Own a shared `DialectRouter` + `StateCache` used by every
//!      session.
//!   4. Expose a discovery-cache snapshot so the one-shot `scan()` free
//!      function can return a fresh inventory without spinning its own
//!      broker connection when the supervisor is running.
//!   5. Orderly shutdown via one-shot channels per session.
//!
//! On DB load failure (missing master key / decrypt error / disk
//! error), the supervisor logs a warning and either falls through to
//! the env-var fallback or spawns nothing. It never panics — v1
//! `smart_home::init` must not fail if MQTT is misconfigured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use super::client::{MqttSession, SessionCommand, SessionConfig};
use super::command::{commands_from_state_patch, MqttCommand};
use super::dialects::DialectRouter;
use super::state::StateCache;
use crate::crypto;
use crate::smart_home::scan::ScanCandidate;

/// One dispatch handle per running session. The supervisor uses these
/// to route command publishes to the right broker without holding a
/// reference to the (reconnect-recreated) AsyncClient.
#[derive(Clone)]
struct SessionHandle {
    user_id: i64,
    label: String,
    cmd_tx: mpsc::Sender<SessionCommand>,
}

pub struct MqttSupervisor {
    db_path: PathBuf,
    router: Arc<DialectRouter>,
    state_cache: Arc<StateCache>,
    discovery: Arc<RwLock<HashMap<String, ScanCandidate>>>,
    shutdowns: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    handles: Arc<RwLock<Vec<SessionHandle>>>,
}

impl MqttSupervisor {
    pub fn new(db_path: PathBuf) -> Self {
        let state_cache = Arc::new(StateCache::new(db_path.clone()));
        Self {
            db_path,
            router: Arc::new(DialectRouter::v1()),
            state_cache,
            discovery: Arc::new(RwLock::new(HashMap::new())),
            shutdowns: Arc::new(Mutex::new(Vec::new())),
            handles: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Constructor + spawn rolled into one. Returns the supervisor for
    /// callers that want to peek at the discovery cache later.
    pub async fn spawn(db_path: PathBuf) -> Arc<Self> {
        let me = Arc::new(Self::new(db_path));
        me.clone().start().await;
        me
    }

    /// Read-only snapshot of every device surfaced by any currently
    /// running session. The one-shot `scan()` free fn can hand this
    /// back without creating a second broker connection.
    pub async fn scan_snapshot(&self) -> Vec<ScanCandidate> {
        self.discovery.read().await.values().cloned().collect()
    }

    async fn start(self: Arc<Self>) {
        let configs = match self.load_session_configs() {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => match env_fallback_config() {
                Some(c) => {
                    log::warn!(
                        "[smart_home::mqtt] no credentials rows, falling back to \
                         DEPRECATED env var SMART_HOME_MQTT_URL"
                    );
                    vec![c]
                }
                None => {
                    log::info!(
                        "[smart_home::mqtt] supervisor has no brokers configured \
                         (no smart_home_credentials rows, no SMART_HOME_MQTT_URL)"
                    );
                    return;
                }
            },
            Err(e) => {
                log::warn!(
                    "[smart_home::mqtt] credentials load failed ({}); attempting env fallback",
                    e
                );
                match env_fallback_config() {
                    Some(c) => vec![c],
                    None => return,
                }
            }
        };

        log::info!(
            "[smart_home::mqtt] supervisor starting {} session(s)",
            configs.len()
        );

        let (discovery_tx, mut discovery_rx) =
            tokio::sync::mpsc::unbounded_channel::<ScanCandidate>();

        // Discovery aggregator — folds every candidate into the cache
        // keyed by external_id so the scan snapshot stays deduped.
        let cache = self.discovery.clone();
        tokio::spawn(async move {
            while let Some(c) = discovery_rx.recv().await {
                let mut guard = cache.write().await;
                guard.insert(c.external_id.clone(), c);
            }
        });

        for cfg in configs {
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>(32);
            {
                let mut guard = self.shutdowns.lock().await;
                guard.push(shutdown_tx);
            }
            {
                let mut handles = self.handles.write().await;
                handles.push(SessionHandle {
                    user_id: cfg.user_id,
                    label: cfg.label.clone(),
                    cmd_tx,
                });
            }
            let session = MqttSession::new(
                cfg,
                self.router.clone(),
                self.state_cache.clone(),
                discovery_tx.clone(),
                cmd_rx,
                shutdown_rx,
            );
            tokio::spawn(session.run());
        }
    }

    /// Look up device `(driver, external_id)` in SQLite, translate the
    /// state patch through `commands_from_state_patch`, route each
    /// encoded command to the user's session(s). Returns the number of
    /// commands enqueued for publish.
    ///
    /// Errors:
    ///   - device row missing           → Err("device <id> not found")
    ///   - driver column isn't "mqtt"    → Err("device driver is <x>, not mqtt")
    ///   - state patch emits no commands → Ok(0) (caller handles)
    ///   - no session matches user_id    → Err("no mqtt session for user <id>")
    pub async fn dispatch_command(
        &self,
        user_id: i64,
        device_id: i64,
        state: &serde_json::Value,
    ) -> Result<usize, String> {
        let db = self.db_path.clone();
        let row = tokio::task::spawn_blocking(
            move || -> Result<(String, String), String> {
                let conn = Connection::open(&db).map_err(|e| e.to_string())?;
                conn.query_row(
                    "SELECT driver, external_id
                       FROM smart_home_devices
                      WHERE user_id = ? AND id = ?",
                    params![user_id, device_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| format!("device {} not found: {}", device_id, e))
            },
        )
        .await
        .map_err(|e| format!("join: {e}"))??;
        let (driver, external_id) = row;

        if driver != "mqtt" {
            return Err(format!("device driver is {}, not mqtt", driver));
        }

        let cmds: Vec<MqttCommand> = commands_from_state_patch(state);
        if cmds.is_empty() {
            return Ok(0);
        }

        let handles = self.handles.read().await;
        let targets: Vec<&SessionHandle> =
            handles.iter().filter(|h| h.user_id == user_id).collect();
        if targets.is_empty() {
            return Err(format!("no mqtt session for user {}", user_id));
        }

        let mut dispatched = 0usize;
        for cmd in &cmds {
            let Some(enc) = self.router.encode_command(&external_id, cmd) else {
                log::info!(
                    "[smart_home::mqtt] no dialect accepted external_id={} cmd={:?}",
                    external_id,
                    cmd
                );
                continue;
            };
            for h in &targets {
                if let Err(e) = h
                    .cmd_tx
                    .send(SessionCommand::Publish(enc.clone()))
                    .await
                {
                    log::warn!(
                        "[smart_home::mqtt] session {} dispatch channel closed: {}",
                        h.label,
                        e
                    );
                    continue;
                }
                dispatched += 1;
            }
        }
        Ok(dispatched)
    }

    fn load_session_configs(&self) -> Result<Vec<SessionConfig>, String> {
        let data_dir = data_dir_for(&self.db_path);
        let key = crypto::load_or_create_key(&data_dir)
            .map_err(|e| format!("master key: {e}"))?;

        let conn = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
        // Silently skip brokers if the table doesn't exist yet. Fresh
        // installs hit `init` before migrations have run in rare boot
        // orderings — don't panic, just log and proceed.
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                   WHERE type='table' AND name='smart_home_credentials'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists == 0 {
            log::warn!(
                "[smart_home::mqtt] smart_home_credentials table missing — \
                 supervisor will attempt env-var fallback only"
            );
            return Ok(Vec::new());
        }

        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, label, secret_encrypted
                   FROM smart_home_credentials
                  WHERE provider = 'mqtt'
                  ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            let (id, user_id, label, enc) = row.map_err(|e| e.to_string())?;
            let plain = match crypto::decrypt(&key, &enc) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "[smart_home::mqtt] credential id={} decrypt failed: {} — skipping",
                        id,
                        e
                    );
                    continue;
                }
            };
            let secret: serde_json::Value = match serde_json::from_str(&plain) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!(
                        "[smart_home::mqtt] credential id={} secret JSON malformed: {}",
                        id,
                        e
                    );
                    continue;
                }
            };
            match SessionConfig::from_credential(user_id, &label, &secret) {
                Some(cfg) => out.push(cfg),
                None => log::warn!(
                    "[smart_home::mqtt] credential id={} missing url field — skipping",
                    id
                ),
            }
        }

        Ok(out)
    }

    /// Fire every stored shutdown channel. Sessions detect it on the
    /// next `tokio::select!` branch.
    pub async fn shutdown_all(&self) {
        let mut guard = self.shutdowns.lock().await;
        for tx in guard.drain(..) {
            let _ = tx.send(());
        }
    }

    /// Publish an arbitrary retained frame to every active session.
    /// Used by the HA Discovery publisher (Phase F) which needs to
    /// reach whatever broker the user's session is connected to —
    /// embedded 1884 or their upstream Mosquitto. Returns the number
    /// of sessions the publish was enqueued on.
    pub async fn publish_retained(
        &self,
        topic: impl Into<String>,
        payload: impl Into<Vec<u8>>,
    ) -> usize {
        let topic = topic.into();
        let payload = payload.into();
        let enc = super::command::EncodedCommand {
            topic,
            payload,
            qos: rumqttc::QoS::AtLeastOnce,
            retain: true,
        };
        let handles = self.handles.read().await;
        let mut count = 0usize;
        for h in handles.iter() {
            if h.cmd_tx
                .send(SessionCommand::Publish(enc.clone()))
                .await
                .is_ok()
            {
                count += 1;
            }
        }
        count
    }

    /// Snapshot of the device rows the publisher should emit HA
    /// Discovery configs for. Run off a spawn_blocking path so the
    /// caller (an async init task) doesn't block on SQLite.
    pub async fn list_commissioned_devices(
        &self,
    ) -> Result<Vec<(i64, i64, String, String, String, String)>, String> {
        let db = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let conn = Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, name, kind, driver, external_id
                       FROM smart_home_devices",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| format!("join: {e}"))?
    }
}

fn data_dir_for(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn env_fallback_config() -> Option<SessionConfig> {
    let url = std::env::var("SMART_HOME_MQTT_URL").ok()?;
    let secret = serde_json::json!({ "url": url });
    SessionConfig::from_credential(0, "env", &secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_db_with_master_key(tmp: &TempDir) -> PathBuf {
        let data_dir = tmp.path().to_path_buf();
        let _key = crypto::load_or_create_key(&data_dir).unwrap();
        let db_path = data_dir.join("index.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE smart_home_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                provider TEXT NOT NULL,
                label TEXT NOT NULL,
                secret_encrypted TEXT NOT NULL,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        db_path
    }

    #[tokio::test]
    async fn load_session_configs_returns_empty_without_rows() {
        let tmp = TempDir::new().unwrap();
        let db_path = init_db_with_master_key(&tmp);
        let sup = MqttSupervisor::new(db_path);
        let out = sup.load_session_configs().unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn load_session_configs_decrypts_valid_row() {
        let tmp = TempDir::new().unwrap();
        let db_path = init_db_with_master_key(&tmp);
        let data_dir = tmp.path().to_path_buf();
        let key = crypto::load_or_create_key(&data_dir).unwrap();
        let secret = serde_json::json!({"url": "mqtt://broker.lan:1883"});
        {
            let conn = Connection::open(&db_path).unwrap();
            crate::smart_home::credentials::upsert(
                &conn, &key, 1, "mqtt", "default", &secret, None,
            )
            .unwrap();
        }
        let sup = MqttSupervisor::new(db_path);
        let out = sup.load_session_configs().unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, "default");
        assert!(out[0].url.starts_with("mqtt://"));
    }

    #[tokio::test]
    async fn scan_snapshot_starts_empty() {
        let tmp = TempDir::new().unwrap();
        let db_path = init_db_with_master_key(&tmp);
        let sup = MqttSupervisor::new(db_path);
        let snap = sup.scan_snapshot().await;
        assert!(snap.is_empty());
    }
}
