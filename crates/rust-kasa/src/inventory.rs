//! `~/.syntaur/kasa_inventory.json` — list of TP-Link LAN devices + a
//! single TP-Link cloud login pair (one-time harvest).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::KasaError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub username: String,
    pub password: String,
    pub devices: Vec<InventoryDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryDevice {
    pub ip: String,
    pub mac: String,
    pub alias: String,
    pub model: String,
    pub device_id: String,
}

impl Inventory {
    pub fn load_default() -> Result<Self, KasaError> {
        let p = default_path();
        Self::load(&p)
    }

    pub fn load(path: &PathBuf) -> Result<Self, KasaError> {
        let bytes = std::fs::read(path).map_err(KasaError::Io)?;
        serde_json::from_slice(&bytes).map_err(KasaError::Json)
    }

    pub fn find_by_alias(&self, alias: &str) -> Option<&InventoryDevice> {
        // Tolerate trailing whitespace in friendly names ("Office Switch ").
        self.devices
            .iter()
            .find(|d| d.alias.trim() == alias.trim())
    }
}

fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    if let Ok(p) = std::env::var("KASA_INVENTORY") {
        return PathBuf::from(p);
    }
    PathBuf::from(home).join(".syntaur").join("kasa_inventory.json")
}
