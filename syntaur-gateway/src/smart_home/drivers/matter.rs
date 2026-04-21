//! Matter driver — **v1 legacy bridge wrapper**.
//!
//! The existing `crate::tools::matter` module already speaks Matter via
//! a WebSocket connection to `python-matter-server` on HAOS. For v1 this
//! driver just wraps those helpers so the new Smart Home dashboard can
//! see already-commissioned Matter nodes alongside Wi-Fi / Zigbee / etc.
//!
//! The v1.1 milestone replaces this entirely with the pure-Rust
//! `crates/syntaur-matter-controller` crate; nothing outside this file
//! (or the `tools/matter.rs` surface) needs to change when that swap
//! happens.

use serde_json::Value;

use crate::smart_home::scan::ScanCandidate;

const BRIDGE_LABEL: &str = "Matter (legacy bridge)";

/// Scan for Matter nodes by asking the python-matter-server bridge for
/// its inventory. Silently returns an empty list if the bridge is
/// unreachable — Matter failures must never block the rest of the scan.
pub async fn scan() -> Vec<ScanCandidate> {
    let nodes = match crate::tools::matter::list_nodes().await {
        Ok(n) => n,
        Err(e) => {
            log::info!(
                "[smart_home::matter] bridge unreachable, skipping Matter scan: {}",
                e
            );
            return Vec::new();
        }
    };
    nodes.iter().filter_map(candidate_from_node).collect()
}

/// Build a ScanCandidate from one node JSON object returned by the
/// bridge. Returns None for nodes without a usable `node_id` (shouldn't
/// happen in practice, but defensive).
fn candidate_from_node(node: &Value) -> Option<ScanCandidate> {
    let node_id = node.get("node_id").and_then(|v| v.as_u64())?;
    let available = node
        .get("available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let (friendly_device, room_area_id, room_friendly) =
        crate::tools::matter::lookup_node_metadata(node_id);

    // Infer kind from the node's endpoint/cluster/attribute layout.
    // Matter clusters live under "attributes" keyed by "<endpoint>/<cluster>/<attr>".
    let kind = infer_kind(node);

    let vendor = node
        .get("attributes")
        .and_then(|a| a.get("0/40/1"))  // BasicInformation::VendorName (0x0028/0x0001)
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let product_name = node
        .get("attributes")
        .and_then(|a| a.get("0/40/3"))  // BasicInformation::ProductName
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let name = friendly_device
        .clone()
        .or_else(|| product_name.clone())
        .unwrap_or_else(|| format!("Matter node {}", node_id));

    let mut details = serde_json::Map::new();
    details.insert("source".into(), Value::String("matter_bridge".into()));
    details.insert("node_id".into(), Value::from(node_id));
    details.insert("available".into(), Value::from(available));
    details.insert("bridge".into(), Value::String(BRIDGE_LABEL.into()));
    if let Some(product) = product_name {
        details.insert("product_name".into(), Value::String(product));
    }
    if let Some(area) = room_area_id {
        details.insert("suggested_room_area_id".into(), Value::String(area));
    }
    if let Some(room) = room_friendly {
        details.insert("suggested_room".into(), Value::String(room));
    }

    Some(ScanCandidate {
        driver: "matter".to_string(),
        external_id: format!("node:{}", node_id),
        name,
        kind,
        vendor,
        ip: None,
        mac: None,
        details: Value::Object(details),
    })
}

/// Inspect the node's `attributes` map and guess a coarse `kind` from
/// which clusters are present on endpoint 1+.
///
/// Matter cluster ids of interest:
///   0x0006 (6)   OnOff              → switch/light/plug
///   0x0008 (8)   LevelControl       → dimmable → light
///   0x0300 (768) ColorControl       → colored light
///   0x0201 (513) Thermostat         → thermostat
///   0x0101 (257) DoorLock           → lock
///   0x0406 (1030) OccupancySensing  → sensor_motion
///   0x0402 (1026) TemperatureMeasurement → sensor_climate
fn infer_kind(node: &Value) -> String {
    let Some(attrs) = node.get("attributes").and_then(|v| v.as_object()) else {
        return "unknown".to_string();
    };
    let has_cluster = |cluster_hex: u32| -> bool {
        attrs.keys().any(|k| {
            // key format: "<endpoint>/<cluster>/<attr>"
            let mut parts = k.split('/');
            parts.next(); // endpoint
            parts
                .next()
                .and_then(|c| c.parse::<u32>().ok())
                .map(|c| c == cluster_hex)
                .unwrap_or(false)
        })
    };

    if has_cluster(0x0101) {
        return "lock".into();
    }
    if has_cluster(0x0201) {
        return "thermostat".into();
    }
    if has_cluster(0x0406) {
        return "sensor_motion".into();
    }
    if has_cluster(0x0402) {
        return "sensor_climate".into();
    }
    if has_cluster(0x0300) || has_cluster(0x0008) {
        return "light".into();
    }
    if has_cluster(0x0006) {
        // On/Off without Level/Color — ambiguous. Call it "switch" so the
        // user can rename to "plug" or "light" in the confirm step.
        return "switch".into();
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infer_kind_recognises_colored_light() {
        let node = json!({
            "node_id": 42,
            "attributes": {
                "1/6/0": true,         // OnOff
                "1/8/0": 200,          // LevelControl
                "1/768/7": 0,          // ColorControl
            }
        });
        assert_eq!(infer_kind(&node), "light");
    }

    #[test]
    fn infer_kind_recognises_lock() {
        let node = json!({
            "node_id": 1,
            "attributes": { "1/257/0": 0 }
        });
        assert_eq!(infer_kind(&node), "lock");
    }

    #[test]
    fn infer_kind_plain_switch() {
        let node = json!({ "node_id": 1, "attributes": { "1/6/0": false } });
        assert_eq!(infer_kind(&node), "switch");
    }

    #[test]
    fn candidate_from_node_builds_expected_shape() {
        let node = json!({
            "node_id": 42,
            "available": true,
            "attributes": {
                "0/40/1": "Eve",
                "0/40/3": "Eve Energy",
                "1/6/0": false,
            }
        });
        let c = candidate_from_node(&node).expect("candidate");
        assert_eq!(c.driver, "matter");
        assert_eq!(c.external_id, "node:42");
        assert_eq!(c.kind, "switch");
        assert_eq!(c.vendor.as_deref(), Some("Eve"));
        assert!(c.details.get("bridge").is_some());
    }

    #[test]
    fn candidate_from_node_skips_nodes_without_id() {
        let node = json!({ "attributes": {} });
        assert!(candidate_from_node(&node).is_none());
    }
}
