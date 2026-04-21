//! Address-cache populator for the rs-matter direct backend.
//!
//! Queries the legacy python-matter-server bridge over WebSocket, reads
//! its live node state, and extracts whatever IP address the bridge
//! currently holds for each paired device. Output is a JSON map
//! `{ node_id (string) -> "addr:port" }` suitable for loading into a
//! `MatterDirectClient`'s `address_cache`.
//!
//! ## Why this exists
//!
//! rs-matter upstream hasn't landed operational mDNS (`_matter._tcp`)
//! yet — tracked as rs-matter issue #370. The direct backend's 4 public
//! methods return `DirectError::OperationalMdnsMissing` when they can't
//! resolve a node_id to an address. Until #370 merges, this bridge
//! shim provides the addresses: python-matter-server already runs a
//! Chip mDNS client internally and holds the resolved address per
//! node. We just ask it.
//!
//! Once upstream ships operational mDNS, this module becomes optional /
//! deletable. The data flow stays the same — `address_cache` gets
//! populated, just from a different source.
//!
//! ## What gets extracted
//!
//! python-matter-server's `get_nodes` response doesn't have a stable
//! "last_known_address" field (see
//! `home-assistant-libs/python-matter-server/matter_server/common/models.py`
//! — `MatterNodeData` omits it). But in practice, recent versions
//! surface address info via nested attributes or a sibling field. We
//! probe several plausible locations and return whatever we find:
//!
//! 1. Top-level `address` / `transport_address` / `last_known_address`
//!    strings on each node dict
//! 2. Nested `attributes["0/0x34/4"]` (General Diagnostics cluster,
//!    `NetworkInterfaces` attribute) — each element has `IPv6Addresses`
//!    + `IPv4Addresses` arrays
//! 3. Top-level `available` + a separate `ping` call if needed (TODO)
//!
//! Per-node extraction is best-effort — a device we can't resolve just
//! gets skipped. The CLI reports counts so Sean can see coverage.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use serde_json::Value;

/// Default Matter port. Operational mDNS advertises on this port +
/// `_matter._tcp`. Commissioning uses `_matterc._udp` on a different
/// port. For paired-device operation we always talk to 5540 unless
/// the bridge tells us otherwise.
const DEFAULT_MATTER_PORT: u16 = 5540;

/// python-matter-server WebSocket URL. Matches `tools/matter.rs`'s
/// `MATTER_WS_URL` — keep in sync. SSH tunnel from syntaur-server
/// to HAOS's internal Docker bridge brings this to localhost.
const BRIDGE_WS_URL: &str = "ws://127.0.0.1:5580/ws";

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("bridge websocket: {0}")]
    Ws(String),

    #[error("bridge response: {0}")]
    Response(String),

    #[error("no addresses extracted — bridge returned {0} nodes, none had resolvable addresses")]
    NoAddresses(usize),
}

/// Fetch the full node list from the bridge. One connect, one send,
/// one response, disconnect. Matches `tools/matter.rs::matter_command`
/// but we keep this self-contained so the direct backend doesn't
/// depend on the bridge's module layout.
async fn fetch_raw_nodes(ws_url: &str) -> Result<Vec<Value>, BridgeError> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (mut ws, _) = connect_async(ws_url)
        .await
        .map_err(|e| BridgeError::Ws(format!("connect: {e}")))?;

    // Discard ServerInfo handshake.
    if let Some(Ok(_)) = ws.next().await {}

    let req = serde_json::json!({
        "message_id": "1",
        "command": "get_nodes",
        "args": {},
    });
    ws.send(Message::Text(req.to_string()))
        .await
        .map_err(|e| BridgeError::Ws(format!("send: {e}")))?;

    let resp_msg = ws
        .next()
        .await
        .ok_or_else(|| BridgeError::Ws("closed before response".into()))?
        .map_err(|e| BridgeError::Ws(format!("recv: {e}")))?;

    let resp_text = resp_msg
        .to_text()
        .map_err(|e| BridgeError::Ws(format!("non-text: {e}")))?;

    let resp: Value = serde_json::from_str(resp_text)
        .map_err(|e| BridgeError::Response(format!("parse: {e}")))?;

    if let Some(err_code) = resp.get("error_code") {
        let details = resp
            .get("details")
            .and_then(|v| v.as_str())
            .unwrap_or("(no details)");
        return Err(BridgeError::Response(format!("{err_code}: {details}")));
    }

    let _ = ws.close(None).await;

    let arr = resp
        .get("result")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr)
}

/// Scan a single node's JSON for an IP address. Returns the first
/// extraction that parses successfully.
fn extract_address_from_node(node: &Value) -> Option<SocketAddr> {
    let candidates = [
        node.get("transport_address").and_then(|v| v.as_str()),
        node.get("address").and_then(|v| v.as_str()),
        node.get("last_known_address").and_then(|v| v.as_str()),
        node.get("ip").and_then(|v| v.as_str()),
    ];
    for c in candidates.iter().flatten() {
        if let Some(sa) = parse_addr_hint(c) {
            return Some(sa);
        }
    }

    // Fallback: General Diagnostics NetworkInterfaces attribute.
    // python-matter-server attribute keys are strings of
    // "endpoint/cluster_hex/attribute" — the GeneralDiagnostics cluster
    // is 0x33 (decimal 51), NetworkInterfaces attribute is 0x0000. On
    // endpoint 0, the key is typically "0/51/0" or "0/0x33/0" depending
    // on version.
    if let Some(attrs) = node.get("attributes").and_then(|v| v.as_object()) {
        for key in ["0/51/0", "0/0x33/0", "0/0033/0"] {
            if let Some(nets) = attrs.get(key).and_then(|v| v.as_array()) {
                for iface in nets {
                    if let Some(addr) = extract_from_network_interface(iface) {
                        return Some(addr);
                    }
                }
            }
        }
    }
    None
}

/// Parse an address hint that may be "1.2.3.4", "1.2.3.4:5540",
/// "[fd00::1]", "[fd00::1]:5540", or "fd00::1". Returns the SocketAddr
/// using DEFAULT_MATTER_PORT if the port isn't embedded.
fn parse_addr_hint(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // With port already attached
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    // Bare IPv4 or IPv6 with no port
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, DEFAULT_MATTER_PORT));
    }
    // Bracketed IPv6 without port: "[fd00::1]"
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if let Ok(ip) = inner.parse::<IpAddr>() {
            return Some(SocketAddr::new(ip, DEFAULT_MATTER_PORT));
        }
    }
    None
}

/// Extract from a NetworkInterfaces TLV element (already decoded to
/// JSON by python-matter-server). Per Matter spec, the struct has
/// IPv6Addresses (list of octet-strings) and IPv4Addresses (same). We
/// take the first non-link-local address.
fn extract_from_network_interface(iface: &Value) -> Option<SocketAddr> {
    // Prefer IPv6 global over IPv4 for Matter (Thread devices are
    // IPv6-only; Wi-Fi devices usually have both).
    for key in ["IPv6Addresses", "ipv6_addresses", "IPv4Addresses", "ipv4_addresses"] {
        if let Some(arr) = iface.get(key).and_then(|v| v.as_array()) {
            for entry in arr {
                let candidate = entry.as_str().or_else(|| {
                    // TLV octet-strings sometimes arrive as hex-encoded strings
                    // or as { "hex": "..." } objects; the python-matter-server
                    // JSON emitter usually stringifies them. Try the direct str
                    // path first and give up if it's nested.
                    None
                });
                if let Some(s) = candidate {
                    if let Some(sa) = parse_addr_hint(s) {
                        if !is_link_local(sa.ip()) {
                            return Some(sa);
                        }
                    }
                }
            }
        }
    }
    None
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Public entry — hit the bridge, return `{ node_id -> SocketAddr }`.
/// Uses DEFAULT_BRIDGE_WS_URL; pass `None` for the default.
pub async fn fetch_node_addresses(
    ws_url: Option<&str>,
) -> Result<HashMap<u64, SocketAddr>, BridgeError> {
    let url = ws_url.unwrap_or(BRIDGE_WS_URL);
    let nodes = fetch_raw_nodes(url).await?;
    let total = nodes.len();
    let mut out = HashMap::new();
    for node in &nodes {
        let Some(nid) = node.get("node_id").and_then(|v| v.as_u64()) else {
            continue;
        };
        if let Some(sa) = extract_address_from_node(node) {
            out.insert(nid, sa);
        }
    }
    if out.is_empty() && total > 0 {
        return Err(BridgeError::NoAddresses(total));
    }
    Ok(out)
}

/// Persist an address map to JSON on disk. The file is keyed by
/// stringified node_id (for JSON compatibility) and values are
/// `"addr:port"` strings that parse via `SocketAddr::from_str`.
///
/// Used by the CLI's `populate-from-bridge --save PATH` flow + the
/// complementary `load_from_file` used at `MatterDirectClient::new`.
/// Atomic write: writes to a sibling `.tmp` then renames, so a crash
/// mid-write can't leave a torn file.
pub fn save_addresses_to_file(
    path: &std::path::Path,
    addrs: &HashMap<u64, SocketAddr>,
) -> std::io::Result<()> {
    let map: std::collections::BTreeMap<String, String> = addrs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let json = serde_json::to_vec_pretty(&map)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("serialize: {e}")))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Load an address map that was previously written by
/// `save_addresses_to_file`. Returns Ok(empty) if the file doesn't
/// exist — missing is not an error, we just have no addresses yet.
/// Any JSON/parse failures surface as `io::Error` with a descriptive
/// message.
pub fn load_addresses_from_file(
    path: &std::path::Path,
) -> std::io::Result<HashMap<u64, SocketAddr>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let bytes = std::fs::read(path)?;
    let map: std::collections::BTreeMap<String, String> = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse: {e}")))?;
    let mut out = HashMap::new();
    for (k, v) in map {
        let node_id: u64 = k.parse().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad node_id {k}: {e}"))
        })?;
        let sa: SocketAddr = v.parse().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad addr {v}: {e}"))
        })?;
        out.insert(node_id, sa);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v4_with_port() {
        assert_eq!(
            parse_addr_hint("192.168.1.42:5540"),
            Some("192.168.1.42:5540".parse().unwrap())
        );
    }

    #[test]
    fn parses_v4_without_port_uses_matter_default() {
        let sa = parse_addr_hint("192.168.1.42").unwrap();
        assert_eq!(sa.port(), 5540);
    }

    #[test]
    fn parses_v6_bracketed_no_port() {
        let sa = parse_addr_hint("[fd00::1]").unwrap();
        assert_eq!(sa.port(), 5540);
        assert!(matches!(sa.ip(), IpAddr::V6(_)));
    }

    #[test]
    fn parses_v6_bare() {
        let sa = parse_addr_hint("fd00::1").unwrap();
        assert_eq!(sa.port(), 5540);
    }

    #[test]
    fn empty_and_garbage_return_none() {
        assert!(parse_addr_hint("").is_none());
        assert!(parse_addr_hint("not-an-address").is_none());
    }

    #[test]
    fn extracts_from_top_level_string() {
        let node = serde_json::json!({
            "node_id": 42,
            "address": "192.168.1.42",
        });
        let sa = extract_address_from_node(&node).unwrap();
        assert_eq!(sa.ip(), "192.168.1.42".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn extracts_from_general_diagnostics_attribute() {
        let node = serde_json::json!({
            "node_id": 7,
            "attributes": {
                "0/51/0": [
                    {
                        "IPv6Addresses": ["fe80::1234", "fd00::42"],
                        "IPv4Addresses": ["192.168.1.50"],
                    }
                ]
            }
        });
        let sa = extract_address_from_node(&node).unwrap();
        // Prefer non-link-local IPv6 over IPv4; fe80:: is skipped.
        assert!(matches!(sa.ip(), IpAddr::V6(_)));
    }

    #[test]
    fn skips_nodes_without_address() {
        let node = serde_json::json!({
            "node_id": 99,
            "available": false,
        });
        assert!(extract_address_from_node(&node).is_none());
    }

    #[test]
    fn link_local_is_skipped() {
        let node = serde_json::json!({
            "attributes": {
                "0/51/0": [
                    { "IPv6Addresses": ["fe80::1"] }
                ]
            }
        });
        assert!(extract_address_from_node(&node).is_none());
    }
}
