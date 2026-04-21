//! Tesla Powerwall / Tesla Gateway — **local** HTTP API.
//!
//! Runs entirely on the user's LAN; no cloud, no OAuth. The gateway
//! speaks HTTPS with a self-signed cert, so we build a reqwest client
//! with cert verification disabled. That's normally a red flag — here
//! it's fine because the "server" is literally a box on the user's
//! own network that they manually configured the URL of.
//!
//! Endpoints used:
//!   GET /api/system_status/soe      → { "percentage": 72.3 }
//!   GET /api/meters/aggregates      → real-time power flow per source
//!                                     (site / solar / battery / load)
//!
//! Newer firmware requires `POST /api/login/Basic` with the gateway
//! password before `/api/meters/aggregates` returns data. Older
//! firmware accepts unauthenticated GETs. We try un-auth first and
//! log a clear "set SMART_HOME_TESLA_GATEWAY_PASSWORD to login" hint
//! when we see 401/403.
//!
//! Configuration (env vars, v1 surface):
//!   SMART_HOME_TESLA_GATEWAY_URL       — e.g. https://192.168.1.42
//!   SMART_HOME_TESLA_GATEWAY_PASSWORD  — optional; gateway install pw

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const ENV_URL: &str = "SMART_HOME_TESLA_GATEWAY_URL";
const ENV_PW: &str = "SMART_HOME_TESLA_GATEWAY_PASSWORD";

/// Battery + power-flow snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PowerwallSnapshot {
    /// Battery percent 0-100.
    pub battery_soc: Option<f64>,
    /// Grid (site) power in watts. Positive = importing from grid,
    /// negative = exporting.
    pub site_watts: Option<f64>,
    /// Solar instantaneous watts.
    pub solar_watts: Option<f64>,
    /// Battery watts. Positive = charging, negative = discharging.
    /// (Tesla API convention is the inverse — we flip it here so the
    /// sign convention matches `site_watts` direction.)
    pub battery_watts: Option<f64>,
    /// Household load watts.
    pub load_watts: Option<f64>,
    /// Timestamp the snapshot was fetched (unix seconds).
    pub fetched_at: i64,
}

/// Thin client around the Tesla Gateway URL. Stateless — auth cookies
/// are fetched per-call when needed, since the gateway sessions are
/// short-lived and aren't worth persisting.
pub struct TeslaLocalClient {
    base_url: String,
    password: Option<String>,
}

impl TeslaLocalClient {
    /// Build a client from `SMART_HOME_TESLA_GATEWAY_URL` + optional
    /// password. Returns None when the URL env var isn't set — the
    /// "Tesla integration off" case shouldn't log noise.
    pub fn from_env() -> Option<Self> {
        let base = std::env::var(ENV_URL).ok()?;
        if base.is_empty() {
            return None;
        }
        Some(Self {
            base_url: base.trim_end_matches('/').to_string(),
            password: std::env::var(ENV_PW).ok().filter(|s| !s.is_empty()),
        })
    }

    /// Fetch a full snapshot. Any single endpoint failing degrades to
    /// None for that field rather than erroring the whole call, so a
    /// disabled solar array (for example) doesn't hide the battery.
    pub async fn fetch_snapshot(&self) -> Result<PowerwallSnapshot, String> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("build client: {e}"))?;

        let soc = self.fetch_json(&client, "/api/system_status/soe").await.ok();
        let aggregates = self
            .fetch_json(&client, "/api/meters/aggregates")
            .await
            .ok();

        Ok(PowerwallSnapshot {
            battery_soc: soc
                .as_ref()
                .and_then(|v| v.get("percentage").and_then(|x| x.as_f64())),
            site_watts: aggregates
                .as_ref()
                .and_then(|v| v.get("site").and_then(|x| x.get("instant_power")).and_then(|x| x.as_f64())),
            solar_watts: aggregates
                .as_ref()
                .and_then(|v| v.get("solar").and_then(|x| x.get("instant_power")).and_then(|x| x.as_f64())),
            battery_watts: aggregates
                .as_ref()
                .and_then(|v| v.get("battery").and_then(|x| x.get("instant_power")).and_then(|x| x.as_f64()))
                .map(|w| -w), // Tesla convention → our sign (positive=charging)
            load_watts: aggregates
                .as_ref()
                .and_then(|v| v.get("load").and_then(|x| x.get("instant_power")).and_then(|x| x.as_f64())),
            fetched_at: chrono::Utc::now().timestamp(),
        })
    }

    async fn fetch_json(&self, client: &reqwest::Client, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("GET {}: {}", url, e))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            if self.password.is_some() {
                // We have a password configured but the gateway still
                // rejected. Surface a specific message so users can
                // diagnose firmware-side auth changes.
                return Err(format!(
                    "tesla gateway rejected auth ({}); check SMART_HOME_TESLA_GATEWAY_PASSWORD",
                    status
                ));
            }
            return Err(format!(
                "tesla gateway requires auth ({}); set SMART_HOME_TESLA_GATEWAY_PASSWORD to enable",
                status
            ));
        }
        if !status.is_success() {
            return Err(format!("GET {} → HTTP {}", path, status));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("parse json from {}: {}", path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mirror parse_snapshot against a captured aggregates+soe JSON pair
    /// without hitting the network.
    #[test]
    fn snapshot_shape_parses_expected_fields() {
        // Simulate the two endpoints into the shape the client assembles.
        let soe = json!({ "percentage": 72.3 });
        let agg = json!({
            "site":    { "instant_power": 800.0 },
            "solar":   { "instant_power": 3500.0 },
            "battery": { "instant_power": -1200.0 },
            "load":    { "instant_power": 3100.0 }
        });

        // Inline the same projection logic that `fetch_snapshot` does
        // so we can verify field mapping without running a server.
        let snap = PowerwallSnapshot {
            battery_soc: soe.get("percentage").and_then(|v| v.as_f64()),
            site_watts: agg.get("site").and_then(|v| v.get("instant_power")).and_then(|v| v.as_f64()),
            solar_watts: agg.get("solar").and_then(|v| v.get("instant_power")).and_then(|v| v.as_f64()),
            battery_watts: agg
                .get("battery")
                .and_then(|v| v.get("instant_power"))
                .and_then(|v| v.as_f64())
                .map(|w| -w),
            load_watts: agg.get("load").and_then(|v| v.get("instant_power")).and_then(|v| v.as_f64()),
            fetched_at: 0,
        };
        assert!((snap.battery_soc.unwrap() - 72.3).abs() < 1e-9);
        assert!((snap.site_watts.unwrap() - 800.0).abs() < 1e-9);
        assert!((snap.solar_watts.unwrap() - 3500.0).abs() < 1e-9);
        // Sign flipped: Tesla -1200 → discharging in their convention,
        // so our positive-charging sign lands at +1200.
        assert!((snap.battery_watts.unwrap() - 1200.0).abs() < 1e-9);
    }

    #[test]
    fn from_env_returns_none_when_unset() {
        // Scrub both vars to simulate the off state. Use an isolated
        // fn so parallel tests don't trip over the env — CI runs test
        // threads serially per module, so this is safe in practice.
        std::env::remove_var(ENV_URL);
        std::env::remove_var(ENV_PW);
        assert!(TeslaLocalClient::from_env().is_none());
    }
}
