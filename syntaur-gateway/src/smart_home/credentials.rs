//! Shared encrypted-credentials helper for smart_home drivers.
//!
//! Backed by `smart_home_credentials` (schema v67): provider + label
//! scope a row; the secret JSON is AES-256-GCM encrypted via
//! `crate::crypto` before write and decrypted on load. Callers pass in
//! the master key loaded once at startup from `~/.syntaur/master.key`.
//!
//! MQTT uses provider="mqtt"; label distinguishes multiple brokers for
//! the same user (e.g. "default" vs "garage-broker"). Future Matter
//! fabric storage (v1.1 cutover), Ring/Nest/Ecobee tokens, and Z-Wave
//! network keys funnel through the same surface with different
//! provider values.
//!
//! MQTT secret shape (illustrative — the helper is JSON-opaque):
//! ```json
//! {
//!   "url": "mqtt://user:pass@host:1883",
//!   "client_id": "syntaur-<user>",
//!   "ca_pem": null,
//!   "dialects": ["ha","z2m","tasmota","shelly_gen1","shelly_gen2","esphome"],
//!   "bridge_to": null
//! }
//! ```

use aes_gcm::{Aes256Gcm, Key};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::crypto;

/// One decrypted row from smart_home_credentials.
#[derive(Debug, Clone)]
pub struct Credential {
    pub id: i64,
    pub user_id: i64,
    pub provider: String,
    pub label: String,
    pub secret: Value,
    pub metadata: Value,
    pub created_at: i64,
}

/// Load every credential for `(user_id, provider)`. Rows that fail to
/// decrypt are logged and skipped so a single corrupt row doesn't
/// poison the list.
pub fn load(
    conn: &Connection,
    key: &Key<Aes256Gcm>,
    user_id: i64,
    provider: &str,
) -> Result<Vec<Credential>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, provider, label, secret_encrypted, metadata_json, created_at
               FROM smart_home_credentials
              WHERE user_id = ? AND provider = ?
              ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, provider], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        let (id, user_id, provider, label, enc, meta, created_at) =
            row.map_err(|e| e.to_string())?;
        let plain = match crypto::decrypt(key, &enc) {
            Ok(p) => p,
            Err(e) => {
                log::warn!(
                    "[smart_home::credentials] decrypt failed for id={} provider={} label={}: {}",
                    id,
                    provider,
                    label,
                    e
                );
                continue;
            }
        };
        let secret: Value = serde_json::from_str(&plain).unwrap_or(Value::Null);
        let metadata: Value =
            serde_json::from_str(&meta).unwrap_or_else(|_| Value::Object(Default::default()));
        out.push(Credential {
            id,
            user_id,
            provider,
            label,
            secret,
            metadata,
            created_at,
        });
    }
    Ok(out)
}

/// Insert or update by `(user_id, provider, label)`. Returns the row id.
/// Encrypts `secret` before writing; `metadata` is stored as plaintext
/// JSON (used for non-sensitive hints like friendly names, last-seen
/// timestamps, etc. — nothing that leaks a token).
pub fn upsert(
    conn: &Connection,
    key: &Key<Aes256Gcm>,
    user_id: i64,
    provider: &str,
    label: &str,
    secret: &Value,
    metadata: Option<&Value>,
) -> Result<i64, String> {
    let plaintext = serde_json::to_string(secret).map_err(|e| e.to_string())?;
    let encrypted = crypto::encrypt(key, &plaintext)?;
    let meta_s = match metadata {
        Some(m) => serde_json::to_string(m).map_err(|e| e.to_string())?,
        None => "{}".to_string(),
    };

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM smart_home_credentials
              WHERE user_id = ? AND provider = ? AND label = ?",
            params![user_id, provider, label],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(id) = existing {
        conn.execute(
            "UPDATE smart_home_credentials
                SET secret_encrypted = ?, metadata_json = ?
              WHERE id = ?",
            params![encrypted, meta_s, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    } else {
        conn.execute(
            "INSERT INTO smart_home_credentials
               (user_id, provider, label, secret_encrypted, metadata_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                user_id,
                provider,
                label,
                encrypted,
                meta_s,
                chrono::Utc::now().timestamp()
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }
}

/// Delete by `(user_id, provider, label)`. No-op if missing.
pub fn delete(
    conn: &Connection,
    user_id: i64,
    provider: &str,
    label: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM smart_home_credentials
          WHERE user_id = ? AND provider = ? AND label = ?",
        params![user_id, provider, label],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn test_db() -> (Connection, TempDir, Key<Aes256Gcm>) {
        let tmp = TempDir::new().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        // Only create the one table we care about — full migration
        // pulls in unrelated schema we don't need for this isolate.
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY);
             INSERT INTO users (id) VALUES (1);
             CREATE TABLE smart_home_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                provider TEXT NOT NULL,
                label TEXT NOT NULL,
                secret_encrypted TEXT NOT NULL,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        let key = crypto::load_or_create_key(tmp.path()).unwrap();
        (conn, tmp, key)
    }

    #[test]
    fn roundtrip_encrypts_and_decrypts() {
        let (conn, _tmp, key) = test_db();
        let secret = json!({"url": "mqtt://u:p@host:1883", "client_id": "test"});
        let id = upsert(&conn, &key, 1, "mqtt", "default", &secret, None).unwrap();
        assert!(id > 0);
        let rows = load(&conn, &key, 1, "mqtt").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "mqtt");
        assert_eq!(rows[0].label, "default");
        assert_eq!(rows[0].secret, secret);
    }

    #[test]
    fn upsert_updates_existing_keeps_same_id() {
        let (conn, _tmp, key) = test_db();
        let id1 = upsert(&conn, &key, 1, "mqtt", "default", &json!({"v": 1}), None).unwrap();
        let id2 = upsert(&conn, &key, 1, "mqtt", "default", &json!({"v": 2}), None).unwrap();
        assert_eq!(id1, id2);
        let rows = load(&conn, &key, 1, "mqtt").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].secret, json!({"v": 2}));
    }

    #[test]
    fn load_empty_returns_empty_vec() {
        let (conn, _tmp, key) = test_db();
        let rows = load(&conn, &key, 1, "nonexistent").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn delete_removes_row() {
        let (conn, _tmp, key) = test_db();
        upsert(&conn, &key, 1, "mqtt", "default", &json!({}), None).unwrap();
        delete(&conn, 1, "mqtt", "default").unwrap();
        let rows = load(&conn, &key, 1, "mqtt").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn metadata_roundtrips_as_json() {
        let (conn, _tmp, key) = test_db();
        let meta = json!({"label_friendly": "Main Mosquitto", "last_seen": 1700000000});
        upsert(
            &conn,
            &key,
            1,
            "mqtt",
            "default",
            &json!({"url": "mqtt://h"}),
            Some(&meta),
        )
        .unwrap();
        let rows = load(&conn, &key, 1, "mqtt").unwrap();
        assert_eq!(rows[0].metadata, meta);
    }

    #[test]
    fn separate_labels_stay_distinct() {
        let (conn, _tmp, key) = test_db();
        upsert(&conn, &key, 1, "mqtt", "default", &json!({"n": 1}), None).unwrap();
        upsert(&conn, &key, 1, "mqtt", "garage", &json!({"n": 2}), None).unwrap();
        let rows = load(&conn, &key, 1, "mqtt").unwrap();
        assert_eq!(rows.len(), 2);
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"garage"));
    }
}
