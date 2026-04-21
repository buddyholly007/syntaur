//! Device model — one row per commissioned device per user. Per-driver
//! specifics live in `state_json` / `capabilities_json` / `metadata_json`
//! so adding a new protocol doesn't force a schema migration.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    pub user_id: i64,
    pub room_id: Option<i64>,
    pub driver: String,
    pub external_id: String,
    pub name: String,
    pub kind: String,
    pub capabilities_json: String,
    pub state_json: String,
    pub metadata_json: String,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

pub fn list_for_user(conn: &Connection, user_id: i64) -> rusqlite::Result<Vec<Device>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, room_id, driver, external_id, name, kind,
                capabilities_json, state_json, metadata_json, last_seen_at, created_at
           FROM smart_home_devices
          WHERE user_id = ?
          ORDER BY kind ASC, name ASC",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(Device {
            id: row.get(0)?,
            user_id: row.get(1)?,
            room_id: row.get(2)?,
            driver: row.get(3)?,
            external_id: row.get(4)?,
            name: row.get(5)?,
            kind: row.get(6)?,
            capabilities_json: row.get(7)?,
            state_json: row.get(8)?,
            metadata_json: row.get(9)?,
            last_seen_at: row.get(10)?,
            created_at: row.get(11)?,
        })
    })?;
    rows.collect()
}

pub fn list_for_room(
    conn: &Connection,
    user_id: i64,
    room_id: i64,
) -> rusqlite::Result<Vec<Device>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, room_id, driver, external_id, name, kind,
                capabilities_json, state_json, metadata_json, last_seen_at, created_at
           FROM smart_home_devices
          WHERE user_id = ? AND room_id = ?
          ORDER BY kind ASC, name ASC",
    )?;
    let rows = stmt.query_map(params![user_id, room_id], |row| {
        Ok(Device {
            id: row.get(0)?,
            user_id: row.get(1)?,
            room_id: row.get(2)?,
            driver: row.get(3)?,
            external_id: row.get(4)?,
            name: row.get(5)?,
            kind: row.get(6)?,
            capabilities_json: row.get(7)?,
            state_json: row.get(8)?,
            metadata_json: row.get(9)?,
            last_seen_at: row.get(10)?,
            created_at: row.get(11)?,
        })
    })?;
    rows.collect()
}

/// Insert or refresh a device discovered via scan. Keyed on
/// `(user_id, driver, external_id)` so repeated scans don't duplicate.
pub fn upsert_from_scan(
    conn: &Connection,
    user_id: i64,
    driver: &str,
    external_id: &str,
    name: &str,
    kind: &str,
    capabilities_json: &str,
    metadata_json: &str,
) -> rusqlite::Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO smart_home_devices
            (user_id, room_id, driver, external_id, name, kind,
             capabilities_json, state_json, metadata_json, last_seen_at, created_at)
         VALUES (?, NULL, ?, ?, ?, ?, ?, '{}', ?, ?, ?)
         ON CONFLICT(user_id, driver, external_id) DO UPDATE SET
            name              = excluded.name,
            kind              = excluded.kind,
            capabilities_json = excluded.capabilities_json,
            metadata_json     = excluded.metadata_json,
            last_seen_at      = excluded.last_seen_at",
        params![
            user_id,
            driver,
            external_id,
            name,
            kind,
            capabilities_json,
            metadata_json,
            now,
            now
        ],
    )?;
    // Prefer the canonical id for the (user, driver, external_id) key.
    let id: i64 = conn.query_row(
        "SELECT id FROM smart_home_devices WHERE user_id = ? AND driver = ? AND external_id = ?",
        params![user_id, driver, external_id],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn assign_room(
    conn: &Connection,
    user_id: i64,
    device_id: i64,
    room_id: Option<i64>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE smart_home_devices SET room_id = ? WHERE user_id = ? AND id = ?",
        params![room_id, user_id, device_id],
    )
}

pub fn get(conn: &Connection, user_id: i64, id: i64) -> rusqlite::Result<Option<Device>> {
    conn.query_row(
        "SELECT id, user_id, room_id, driver, external_id, name, kind,
                capabilities_json, state_json, metadata_json, last_seen_at, created_at
           FROM smart_home_devices
          WHERE user_id = ? AND id = ?",
        params![user_id, id],
        |row| {
            Ok(Device {
                id: row.get(0)?,
                user_id: row.get(1)?,
                room_id: row.get(2)?,
                driver: row.get(3)?,
                external_id: row.get(4)?,
                name: row.get(5)?,
                kind: row.get(6)?,
                capabilities_json: row.get(7)?,
                state_json: row.get(8)?,
                metadata_json: row.get(9)?,
                last_seen_at: row.get(10)?,
                created_at: row.get(11)?,
            })
        },
    )
    .optional()
}
