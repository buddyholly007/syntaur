//! SQLite device registry — persistent store of all known smart devices.

use log::{info, warn};
use rusqlite::{params, Connection};
use tokio::sync::Mutex;

use super::{Device, DeviceCapability, DeviceCommand, DevicePlatform, DeviceProtocol, DeviceState};

pub struct DeviceRegistry {
    db: Mutex<Connection>,
    client: reqwest::Client,
    mqtt: Option<super::mqtt::MqttController>,
    matter: Option<super::matter::MatterController>,
}

impl DeviceRegistry {
    pub fn open(data_dir: &str) -> Self {
        let db_path = format!("{}/devices.db", data_dir);
        let db = Connection::open(&db_path).expect("Failed to open devices.db");
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                room TEXT NOT NULL DEFAULT '',
                protocol TEXT NOT NULL,
                platform TEXT NOT NULL DEFAULT 'generic',
                endpoint TEXT NOT NULL,
                capabilities TEXT NOT NULL DEFAULT '[]',
                auth TEXT,
                metadata TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_device_room ON devices(room);",
        )
        .expect("Failed to create devices table");

        // Auto-detect MQTT broker from env
        let mqtt = std::env::var("MQTT_URL").ok().map(|url| {
            let port: u16 = std::env::var("MQTT_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(1883);
            info!("[devices] MQTT broker configured at {}:{}", url, port);
            super::mqtt::MqttController::new(&url, port)
        });

        // Auto-detect python-matter-server from env
        let matter = std::env::var("MATTER_WS_URL").ok().map(|url| {
            info!("[devices] Matter controller configured at {}", url);
            super::matter::MatterController::new(&url)
        });

        info!(
            "[devices] opened {} (mqtt={} matter={})",
            db_path,
            if mqtt.is_some() { "yes" } else { "no" },
            if matter.is_some() { "yes" } else { "no" },
        );
        Self {
            db: Mutex::new(db),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            mqtt,
            matter,
        }
    }

    /// Add or update a device.
    pub async fn register(&self, device: &Device) {
        let db = self.db.lock().await;
        let caps = serde_json::to_string(&device.capabilities).unwrap_or_default();
        let meta = device.metadata.to_string();
        db.execute(
            "INSERT OR REPLACE INTO devices (id, name, room, protocol, platform, endpoint, capabilities, auth, metadata)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                device.id,
                device.name,
                device.room,
                serde_json::to_string(&device.protocol).unwrap_or_default().trim_matches('"'),
                serde_json::to_string(&device.platform).unwrap_or_default().trim_matches('"'),
                device.endpoint,
                caps,
                device.auth,
                meta,
            ],
        )
        .ok();
        info!("[devices] registered {} ({}, {})", device.id, device.platform, device.room);
    }

    /// Remove a device.
    pub async fn remove(&self, device_id: &str) -> bool {
        let db = self.db.lock().await;
        db.execute("DELETE FROM devices WHERE id = ?", params![device_id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Get a device by ID.
    pub async fn get(&self, device_id: &str) -> Option<Device> {
        let db = self.db.lock().await;
        Self::query_one(&db, "SELECT * FROM devices WHERE id = ?", params![device_id])
    }

    /// Find devices by room name (fuzzy match).
    pub async fn by_room(&self, room: &str) -> Vec<Device> {
        let db = self.db.lock().await;
        let pattern = format!("%{}%", room.to_lowercase());
        Self::query_many(
            &db,
            "SELECT * FROM devices WHERE LOWER(room) LIKE ?",
            params![pattern],
        )
    }

    /// Find devices by name (fuzzy match).
    pub async fn by_name(&self, name: &str) -> Vec<Device> {
        let db = self.db.lock().await;
        let pattern = format!("%{}%", name.to_lowercase());
        Self::query_many(
            &db,
            "SELECT * FROM devices WHERE LOWER(name) LIKE ?",
            params![pattern],
        )
    }

    /// List all devices.
    pub async fn list(&self) -> Vec<Device> {
        let db = self.db.lock().await;
        Self::query_many(&db, "SELECT * FROM devices ORDER BY room, name", params![])
    }

    /// List all Matter nodes from python-matter-server (live query).
    pub async fn list_matter_nodes(&self) -> Result<Vec<super::matter::MatterNode>, String> {
        if let Some(ref matter) = self.matter {
            matter.list_nodes().await
        } else {
            Err("Matter not configured (set MATTER_WS_URL)".into())
        }
    }

    /// Auto-import Matter nodes into the registry.
    pub async fn import_matter_nodes(&self) -> Vec<String> {
        let nodes = match self.list_matter_nodes().await {
            Ok(n) => n,
            Err(e) => {
                warn!("[devices] matter import failed: {}", e);
                return vec![format!("error: {}", e)];
            }
        };

        let mut imported = Vec::new();
        for node in &nodes {
            if !node.available {
                continue;
            }
            let id = format!("matter-{}", node.node_id);
            let device = Device {
                id: id.clone(),
                name: node.name.clone(),
                room: String::new(), // User assigns rooms after import
                protocol: super::DeviceProtocol::Matter,
                platform: super::DevicePlatform::Matter,
                endpoint: self.matter.as_ref().map(|_| "matter-server").unwrap_or("").to_string(),
                capabilities: {
                    let mut caps = vec![super::DeviceCapability::OnOff];
                    if node.brightness.is_some() {
                        caps.push(super::DeviceCapability::Brightness);
                    }
                    caps
                },
                auth: None,
                metadata: serde_json::json!({"node_id": node.node_id, "product": node.product}),
            };
            self.register(&device).await;
            imported.push(format!("{}: {} (node {})", id, node.name, node.node_id));
        }

        info!("[devices] imported {} Matter node(s)", imported.len());
        imported
    }

    /// List all rooms.
    pub async fn rooms(&self) -> Vec<String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare("SELECT DISTINCT room FROM devices WHERE room != '' ORDER BY room")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Execute a command on a device (routes to the right protocol handler).
    pub async fn execute(
        &self,
        device_id: &str,
        command: &DeviceCommand,
    ) -> DeviceState {
        let device = match self.get(device_id).await {
            Some(d) => d,
            None => return DeviceState::err(device_id, "device not found"),
        };

        match device.protocol {
            DeviceProtocol::Http => {
                super::http::execute(&self.client, &device, command).await
            }
            DeviceProtocol::Matter => {
                if let Some(ref matter) = self.matter {
                    matter.execute(&device, command).await
                } else {
                    DeviceState::err(device_id, "Matter not configured (set MATTER_WS_URL=ws://host:5580/ws)")
                }
            }
            DeviceProtocol::Mqtt => {
                if let Some(ref mqtt) = self.mqtt {
                    mqtt.execute(&device, command).await
                } else {
                    DeviceState::err(device_id, "MQTT not configured (set MQTT_URL env var)")
                }
            }
        }
    }

    /// Execute a command on all devices in a room.
    pub async fn execute_room(
        &self,
        room: &str,
        command: &DeviceCommand,
    ) -> Vec<DeviceState> {
        let devices = self.by_room(room).await;
        if devices.is_empty() {
            return vec![DeviceState::err("", &format!("no devices found in room '{}'", room))];
        }

        let mut results = Vec::new();
        for device in &devices {
            // Only send on/off/brightness to devices that support it
            let supported = match command {
                DeviceCommand::TurnOn | DeviceCommand::TurnOff | DeviceCommand::Toggle => {
                    device.capabilities.contains(&DeviceCapability::OnOff)
                }
                DeviceCommand::SetBrightness { .. } => {
                    device.capabilities.contains(&DeviceCapability::Brightness)
                }
                DeviceCommand::SetColorTemp { .. } => {
                    device.capabilities.contains(&DeviceCapability::ColorTemp)
                }
                DeviceCommand::SetColor { .. } => {
                    device.capabilities.contains(&DeviceCapability::Color)
                }
                DeviceCommand::Status => true,
            };

            if supported {
                results.push(self.execute(&device.id, command).await);
            }
        }
        results
    }

    fn query_one(db: &Connection, sql: &str, params: impl rusqlite::Params) -> Option<Device> {
        db.query_row(sql, params, |row| Self::row_to_device(row)).ok()
    }

    fn query_many(db: &Connection, sql: &str, params: impl rusqlite::Params) -> Vec<Device> {
        let mut stmt = db.prepare(sql).unwrap();
        stmt.query_map(params, |row| Self::row_to_device(row))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    fn row_to_device(row: &rusqlite::Row) -> rusqlite::Result<Device> {
        let protocol_str: String = row.get(3)?;
        let platform_str: String = row.get(4)?;
        let caps_str: String = row.get(6)?;
        let auth: Option<String> = row.get(7)?;
        let meta_str: String = row.get(8)?;

        Ok(Device {
            id: row.get(0)?,
            name: row.get(1)?,
            room: row.get(2)?,
            protocol: serde_json::from_str(&format!("\"{}\"", protocol_str))
                .unwrap_or(DeviceProtocol::Http),
            platform: serde_json::from_str(&format!("\"{}\"", platform_str))
                .unwrap_or(DevicePlatform::Generic),
            endpoint: row.get(5)?,
            capabilities: serde_json::from_str(&caps_str).unwrap_or_default(),
            auth,
            metadata: serde_json::from_str(&meta_str).unwrap_or_default(),
        })
    }
}
