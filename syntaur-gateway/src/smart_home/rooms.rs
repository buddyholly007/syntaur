//! Room + Zone model. One row per physical room per user.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub zone: Option<String>,
    pub sort_order: i64,
    pub background_image: Option<String>,
    pub created_at: i64,
}

/// List all rooms for a user, ordered by (sort_order, name).
pub fn list_for_user(conn: &Connection, user_id: i64) -> rusqlite::Result<Vec<Room>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, name, zone, sort_order, background_image, created_at
           FROM smart_home_rooms
          WHERE user_id = ?
          ORDER BY sort_order ASC, name ASC",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(Room {
            id: row.get(0)?,
            user_id: row.get(1)?,
            name: row.get(2)?,
            zone: row.get(3)?,
            sort_order: row.get(4)?,
            background_image: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn get(conn: &Connection, user_id: i64, id: i64) -> rusqlite::Result<Option<Room>> {
    conn.query_row(
        "SELECT id, user_id, name, zone, sort_order, background_image, created_at
           FROM smart_home_rooms
          WHERE user_id = ? AND id = ?",
        params![user_id, id],
        |row| {
            Ok(Room {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                zone: row.get(3)?,
                sort_order: row.get(4)?,
                background_image: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()
}

pub fn create(
    conn: &Connection,
    user_id: i64,
    name: &str,
    zone: Option<&str>,
) -> rusqlite::Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO smart_home_rooms (user_id, name, zone, sort_order, background_image, created_at)
         VALUES (?, ?, ?, COALESCE((SELECT MAX(sort_order)+1 FROM smart_home_rooms WHERE user_id = ?), 0), NULL, ?)",
        params![user_id, name, zone, user_id, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, user_id: i64, id: i64, name: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE smart_home_rooms SET name = ? WHERE user_id = ? AND id = ?",
        params![name, user_id, id],
    )
}

pub fn set_sort_order(
    conn: &Connection,
    user_id: i64,
    id: i64,
    sort_order: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE smart_home_rooms SET sort_order = ? WHERE user_id = ? AND id = ?",
        params![sort_order, user_id, id],
    )
}

pub fn delete(conn: &Connection, user_id: i64, id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM smart_home_rooms WHERE user_id = ? AND id = ?",
        params![user_id, id],
    )
}
