//! Dyson adopter (Recipe R1 — cloud-bootstrapped local MQTT).
//!
//! ## Why R1, not the existing MQTT subsystem
//!
//! Each Dyson device runs **its own** embedded MQTT broker on
//! TCP :1883. Authentication is per-device:
//!
//! - **Username** — the device's serial number (e.g. `A9S-US-KJA2249A`).
//! - **Password** — the `apPasswordHash` field, derived from the
//!   `LocalCredentials` blob harvested once from the Dyson cloud account
//!   (the same flow `libdyson` and Home Assistant's `dyson_local`
//!   integration use).
//!
//! After bootstrap, all traffic is LAN-only. The cloud is dormant.
//!
//! Topic schema (per device):
//! - State (device → us): `<productType>/<serial>/status/current`
//! - Faults (device → us): `<productType>/<serial>/status/faults`
//! - Commands (us → device): `<productType>/<serial>/command`
//!
//! Because the broker is per-device, the existing
//! [MQTT subsystem][mqtt] (which assumes a shared broker bus) is the
//! wrong abstraction. Dyson lives here and opens its own per-device
//! rumqttc client when control ships.
//!
//! [mqtt]: super::super::mqtt
//!
//! ## Discovery
//!
//! Two complementary paths so this works for every Syntaur install,
//! not just same-L2-segment setups:
//!
//! 1. **mDNS** (preferred when the gateway is on the same L2 as the
//!    devices, or a reflector forwards multicast across VLANs). Dyson
//!    devices advertise `_dyson_mqtt._tcp.local.` — the existing
//!    `wifi_lan::mdns_sweep` is wired to surface those candidates as
//!    `lan_dyson`. No probe needed.
//! 2. **Direct-IP fingerprint probe** (works across VLAN boundaries
//!    and inside Docker bridges where multicast is filtered). Probes
//!    a list of IPs, sends an MQTT CONNECT with no credentials, and
//!    treats a CONNACK reason 4 (`bad creds`) or 5 (`not authorized`)
//!    as the Dyson signature.
//!
//! The probe IP list comes from `SYNTAUR_DYSON_PROBE_IPS` (env
//! override, useful for early bring-up) and falls through to a
//! per-user DB-stored list once the settings UI lands (see
//! `smart_home::credentials` for the credential-storage pattern the
//! MQTT subsystem already established). End users will see a
//! "Add Dyson IP" field in Settings → Smart Home → Devices.
//!
//! ## Per-user cloud bootstrap (R1 contract)
//!
//! Each user authenticates against **their own** Dyson cloud account
//! once. Syntaur harvests the `LocalCredentials` blob for every device
//! on that account, decrypts the `apPasswordHash`, and stores
//! `(serial, password, productType)` triples encrypted via
//! `smart_home::credentials`. From then on, all Dyson traffic is
//! LAN-only. Per-user namespacing is critical: one user's Dyson
//! credentials must never reach another user's device list on a
//! multi-tenant install.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::{LanAdopter, LanCandidate, LanCommand, LanCommandOutcome, Recipe};

const MQTT_PORT: u16 = 1883;
const PROBE_TIMEOUT_MS: u64 = 1_500;

#[derive(Default, Clone)]
pub struct DysonAdopter;

#[async_trait]
impl LanAdopter for DysonAdopter {
    fn slug(&self) -> &'static str {
        "dyson"
    }

    fn recipe(&self) -> Recipe {
        Recipe::R1CloudBootstrap
    }

    async fn discover(&self) -> Vec<LanCandidate> {
        let ips = match std::env::var("SYNTAUR_DYSON_PROBE_IPS") {
            Ok(s) if !s.trim().is_empty() => s
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
            _ => return Vec::new(),
        };
        log::info!("[lan::dyson] probing {} configured IP(s)", ips.len());

        let mut tasks = Vec::with_capacity(ips.len());
        for ip in ips {
            tasks.push(tokio::spawn(async move {
                let result = probe(&ip).await;
                (ip, result)
            }));
        }
        let mut out = Vec::new();
        for t in tasks {
            if let Ok((ip, Ok(true))) = t.await {
                out.push(LanCandidate {
                    adopter: "dyson".to_string(),
                    external_id: ip.clone(),
                    name: "Dyson air-treatment device".to_string(),
                    kind: "air_purifier".to_string(),
                    vendor: "Dyson".to_string(),
                    ip,
                    mac: None,
                    model: None,
                    details: json!({
                        "needs_cloud_bootstrap": true,
                        "transport": "mqtt",
                        "transport_port": MQTT_PORT,
                    }),
                });
            }
        }
        out
    }

    async fn dispatch(
        &self,
        external_id: &str,
        _cmd: &LanCommand,
    ) -> LanCommandOutcome {
        // Dispatch needs the device serial + apPasswordHash from the
        // cloud-bootstrap step, neither of which is wired today. This
        // returns a structured "needs bootstrap" outcome the API layer
        // can render into a setup CTA.
        LanCommandOutcome {
            adopter: "dyson".to_string(),
            external_id: external_id.to_string(),
            ok: false,
            message: Some(
                "dyson dispatch requires cloud bootstrap (LocalCredentials + serial); see settings".to_string(),
            ),
        }
    }
}

/// Send a no-auth MQTT-3.1.1 CONNECT to `<ip>:1883` and return Ok(true)
/// if the device responds with a CONNACK whose reason code matches the
/// Dyson fingerprint (reason ∈ {4, 5}). Anything else (timeout, refused,
/// non-MQTT response, reason 0 = accepted) is Ok(false).
pub(crate) async fn probe(ip: &str) -> Result<bool, String> {
    let addr = format!("{ip}:{MQTT_PORT}");
    let connect = build_no_auth_connect("syntaur-probe");
    let res = timeout(Duration::from_millis(PROBE_TIMEOUT_MS), async {
        let mut stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        stream
            .write_all(&connect)
            .await
            .map_err(|e| format!("write: {e}"))?;
        let mut buf = [0u8; 64];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        Ok::<_, String>((n, buf))
    })
    .await;

    match res {
        Ok(Ok((n, buf))) => Ok(parse_connack(&buf[..n])
            .map(is_dyson_reason)
            .unwrap_or(false)),
        Ok(Err(e)) => {
            log::debug!("[lan::dyson::probe] {ip}: {e}");
            Ok(false)
        }
        Err(_) => {
            log::debug!("[lan::dyson::probe] {ip}: timeout");
            Ok(false)
        }
    }
}

/// Hand-rolled MQTT-3.1.1 CONNECT packet without username/password.
/// Keeping this dep-free (no rumqttc) keeps discovery cheap.
fn build_no_auth_connect(client_id: &str) -> Vec<u8> {
    // Variable header: protocol name + level + flags + keep-alive
    let mut var = Vec::new();
    var.extend_from_slice(&[0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x3C]);

    // Payload: client identifier (length-prefixed UTF-8)
    let mut payload = Vec::new();
    let cid = client_id.as_bytes();
    payload.extend_from_slice(&(cid.len() as u16).to_be_bytes());
    payload.extend_from_slice(cid);

    let body_len = var.len() + payload.len();
    let mut packet = Vec::with_capacity(2 + body_len);
    packet.push(0x10); // CONNECT control packet type
    encode_remaining_length(&mut packet, body_len);
    packet.extend_from_slice(&var);
    packet.extend_from_slice(&payload);
    packet
}

fn encode_remaining_length(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value % 128) as u8;
        value /= 128;
        if value > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Returns the CONNACK reason code if `buf` starts with a valid CONNACK
/// fixed header, else `None`.
fn parse_connack(buf: &[u8]) -> Option<u8> {
    if buf.len() < 4 {
        return None;
    }
    if buf[0] != 0x20 {
        return None;
    }
    if buf[1] != 0x02 {
        return None;
    }
    Some(buf[3])
}

fn is_dyson_reason(reason: u8) -> bool {
    // 4 = Connection refused: bad user name or password
    // 5 = Connection refused: not authorized
    matches!(reason, 4 | 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_connect_packet_layout() {
        let pkt = build_no_auth_connect("syntaur-probe");
        // Fixed header: 0x10
        assert_eq!(pkt[0], 0x10);
        // Variable header begins at byte 2 (after 1-byte rem length for
        // small packets) with protocol name length 0x00 0x04 + "MQTT".
        assert_eq!(&pkt[2..8], &[0x00, 0x04, b'M', b'Q', b'T', b'T']);
        // Protocol level 4 (3.1.1).
        assert_eq!(pkt[8], 0x04);
        // Connect flags: clean session only (0x02).
        assert_eq!(pkt[9], 0x02);
    }

    #[test]
    fn parse_connack_accepted() {
        let r = parse_connack(&[0x20, 0x02, 0x00, 0x00]).unwrap();
        assert_eq!(r, 0);
    }

    #[test]
    fn parse_connack_bad_creds() {
        let r = parse_connack(&[0x20, 0x02, 0x00, 0x04]).unwrap();
        assert_eq!(r, 4);
    }

    #[test]
    fn parse_connack_rejects_garbage() {
        assert_eq!(parse_connack(b""), None);
        assert_eq!(parse_connack(&[0x20]), None);
        assert_eq!(parse_connack(&[0x10, 0x02, 0x00, 0x00]), None);
        assert_eq!(parse_connack(&[0x20, 0x05, 0x00, 0x00]), None);
    }

    #[test]
    fn dyson_fingerprint_reasons() {
        assert!(is_dyson_reason(4));
        assert!(is_dyson_reason(5));
        assert!(!is_dyson_reason(0));
        assert!(!is_dyson_reason(1));
        assert!(!is_dyson_reason(2));
        assert!(!is_dyson_reason(3));
    }

    #[test]
    fn remaining_length_small() {
        let mut out = Vec::new();
        encode_remaining_length(&mut out, 12);
        assert_eq!(out, vec![12]);
    }

    #[test]
    fn remaining_length_multi_byte() {
        let mut out = Vec::new();
        encode_remaining_length(&mut out, 200);
        assert_eq!(out, vec![0xC8, 0x01]);
    }
}
