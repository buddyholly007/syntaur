//! Runtime config — CLI flags + optional TOML file. Order of precedence:
//! CLI flag > ENV > TOML > built-in default.

use clap::Parser;
use serde::Deserialize;

#[derive(Parser, Debug, Clone)]
#[command(version, about = "ESPHome bluetooth_proxy-compatible shim that fans out adverts to multiple subscribers")]
pub struct Cli {
    /// Path to optional TOML config file.
    #[arg(long, env = "SYNTAUR_BLE_SHIM_CONFIG")]
    pub config: Option<String>,

    /// Bind address for the ESPHome native API (default 0.0.0.0:6053).
    #[arg(long, env = "SYNTAUR_BLE_SHIM_BIND")]
    pub bind: Option<String>,

    /// Friendly name advertised over mDNS and shown in HA's UI.
    #[arg(long, env = "SYNTAUR_BLE_SHIM_NAME")]
    pub name: Option<String>,

    /// Suggested area for the proxy (HA shows this on adoption).
    #[arg(long, env = "SYNTAUR_BLE_SHIM_AREA")]
    pub suggested_area: Option<String>,

    /// Increase log level (-v info, -vv debug, -vvv trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Deserialize, Default)]
pub struct FileConfig {
    pub bind: Option<String>,
    pub name: Option<String>,
    pub suggested_area: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub name: String,
    pub suggested_area: String,
}

const DEFAULT_BIND: &str = "0.0.0.0:6053";

impl Config {
    pub fn resolve(cli: Cli) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let file: FileConfig = match cli.config.as_deref() {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("read config {path}: {e}"))?;
                toml::from_str(&text).map_err(|e| format!("parse config {path}: {e}"))?
            }
            None => FileConfig::default(),
        };

        let default_name = derive_default_name();

        let bind = cli
            .bind
            .or(file.bind)
            .unwrap_or_else(|| DEFAULT_BIND.into());
        let name = cli.name.or(file.name).unwrap_or(default_name);
        let suggested_area = cli
            .suggested_area
            .or(file.suggested_area)
            .unwrap_or_default();

        Ok(Config {
            bind,
            name,
            suggested_area,
        })
    }
}

/// Build a sensible default friendly name from the host's first label, with
/// non-alphanumeric chars (except `-`) replaced by `-`.
fn derive_default_name() -> String {
    let raw = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "syntaur".into());
    let label = raw.split('.').next().unwrap_or(&raw);
    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "syntaur-ble-shim".into()
    } else {
        format!("{trimmed}-ble-shim")
    }
}
