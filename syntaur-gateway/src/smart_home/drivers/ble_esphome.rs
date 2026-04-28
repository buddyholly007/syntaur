//! ESPHome native-API BLE-advertisement ingest. Phase 4.
//!
//! For every `smart_home_devices` row of kind=`esphome_proxy` whose
//! `state_json.esphome.mode` is `tracking` (the default), open one TCP
//! connection on the proxy's API port (typically 6053), complete the
//! ESPHome handshake, subscribe to raw BLE adverts (msg_type=66 with
//! flags=1), and pump every `BluetoothLERawAdvertisementsResponse`
//! (msg_type=93) batch into the BleDriver attributed to that proxy's
//! anchor row.
//!
//! ## Why this isn't gated behind `ble-host`
//!
//! Despite the name, this module needs no Bluetooth radio. The shim
//! and ESPHome proxies expose adverts over TCP; the gateway is just a
//! native-API client. The feature flag `ble-host` gates `btleplug`
//! (used by the local-host scanner in `ble_host.rs`); we only depend
//! on `tokio` + `snow` + the in-tree `voice::esphome_api` codec, which
//! ship in every build profile.
//!
//! ## Encryption mode
//!
//! Looked up from `smart_home_credentials`: a row with provider
//! `"esphome_native_api"` and label = `<device.name>` carrying
//! `{"psk_b64": "<32-byte base64>"}` triggers a Noise_NNpsk0 handshake.
//! Otherwise we attempt plaintext. The shim ships plaintext; flashed
//! ESP32 proxies use Noise (their YAML embeds the same key the
//! firmware-role wizard wrote into `smart_home_credentials`).
//!
//! A connection that fails in either mode is retried with exponential
//! backoff (2 s → 60 s) so a missing PSK never bricks the supervisor —
//! it just spins on reconnect attempts in the background.
//!
//! ## Per-proxy mode toggle
//!
//! `state_json.esphome.mode` controls whether this supervisor
//! subscribes to a given proxy. Values:
//!   * `"tracking"` — open a connection and ingest adverts (default
//!     for newly-adopted proxies).
//!   * anything else — leave the proxy alone (used during Matter
//!     commissioning over `bluetooth_proxy.connect`, where a parallel
//!     advert subscription would just compete for the radio).
//!
//! Toggling the mode in the device row takes effect on the next
//! refresh tick (every 60 s).
//!
//! ## Lifecycle
//!
//! - One supervisor task hydrates the proxy list every 60 s.
//! - One worker task per proxy, started on first appearance, aborted
//!   when the proxy disappears or its mode flips off `"tracking"`.
//! - Both layers exit cleanly when the BleDriver's owning Arc drops.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::{interval, timeout};

use super::ble::{canonicalize_mac, BleDriver, RssiObservation};
use crate::voice::esphome_api::{
    self as eapi, build_hello_request, NoiseTransport, ProtoEncoder, RawMessage,
    MSG_HELLO_REQUEST, MSG_HELLO_RESPONSE, MSG_PING_REQUEST, MSG_PING_RESPONSE,
};

// ESPHome message-type IDs not exported by voice/esphome_api.rs.
const MSG_CONNECT_REQUEST: u32 = 3;
const MSG_CONNECT_RESPONSE: u32 = 4;
const MSG_GET_TIME_REQUEST: u32 = 36;
const MSG_GET_TIME_RESPONSE: u32 = 37;
const MSG_SUBSCRIBE_BLE_ADVERTS: u32 = 66;
const MSG_BLE_RAW_ADVERTS_RESPONSE: u32 = 93;

/// Refresh interval for the proxy list. New rows from
/// `POST /api/smart-home/scan/confirm` come online within this window
/// without a gateway restart.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Initial reconnect delay; doubles on each error up to RECONNECT_MAX.
const RECONNECT_MIN: Duration = Duration::from_secs(2);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
/// TCP connect timeout. ESPHome devices on Wi-Fi answer in <300 ms when
/// up; 5 s tolerates a busy AP without holding the worker.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-message read timeout. Forces a reconnect if the proxy stops
/// emitting messages entirely (radio failure, firmware crash) rather
/// than holding the worker on a silently-dead socket. Sized so the
/// proxy's own heartbeat (~60 s ping cycle) is well within the window.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawn the ESPHome ingest supervisor. Returns immediately; the
/// supervisor loop runs detached until the JoinHandle is dropped.
pub fn start_esphome_ingest(
    driver: Arc<BleDriver>,
    db_path: PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { run(driver, db_path).await })
}

async fn run(driver: Arc<BleDriver>, db_path: PathBuf) {
    log::info!(
        "[smart_home::ble_esphome] supervisor up; refreshing proxy list every {}s",
        REFRESH_INTERVAL.as_secs()
    );

    let mut workers: HashMap<i64, tokio::task::AbortHandle> = HashMap::new();
    let mut tick = interval(REFRESH_INTERVAL);
    // Don't burst-tick on a missed deadline; one refresh per interval
    // even after a long pause.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;
        let proxies = match load_active_proxies(&db_path).await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("[smart_home::ble_esphome] proxy list reload failed: {e}");
                continue;
            }
        };
        let live: HashSet<i64> = proxies.iter().map(|p| p.device_id).collect();

        // Stop workers for proxies that disappeared or switched off tracking.
        workers.retain(|id, h| {
            if live.contains(id) {
                true
            } else {
                log::info!(
                    "[smart_home::ble_esphome] device id={id} no longer tracking — stopping worker"
                );
                h.abort();
                false
            }
        });

        // Spawn workers for newly-tracking proxies.
        for p in proxies {
            if workers.contains_key(&p.device_id) {
                continue;
            }
            log::info!(
                "[smart_home::ble_esphome] starting worker device_id={} ({} @ {}:{})",
                p.device_id, p.name, p.host, p.port
            );
            let driver_c = driver.clone();
            let db_c = db_path.clone();
            let row = p.clone();
            let handle =
                tokio::spawn(async move { worker_loop(driver_c, db_c, row).await });
            workers.insert(p.device_id, handle.abort_handle());
        }
    }
}

/// One row in the proxy list.
#[derive(Debug, Clone)]
struct ProxyRow {
    device_id: i64,
    user_id: i64,
    name: String,
    host: String,
    port: u16,
}

/// Read every esphome_proxy device row and return the subset whose
/// `state_json.esphome.mode` is "tracking" (or unset — default tracking).
async fn load_active_proxies(db_path: &PathBuf) -> Result<Vec<ProxyRow>, String> {
    let db = db_path.clone();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<ProxyRow>> {
        let conn = Connection::open(&db)?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                   WHERE type='table' AND name='smart_home_devices'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, external_id, metadata_json, state_json
               FROM smart_home_devices
              WHERE kind = 'esphome_proxy' AND driver = 'esphome'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows.filter_map(Result::ok) {
            let (device_id, user_id, name, external_id, meta_raw, state_raw) = row;

            // Mode gate. Default = tracking when state_json is missing/empty.
            let mode = serde_json::from_str::<Value>(&state_raw)
                .ok()
                .and_then(|v| {
                    v.get("esphome")
                        .and_then(|e| e.get("mode"))
                        .and_then(|m| m.as_str().map(str::to_string))
                })
                .unwrap_or_else(|| "tracking".to_string());
            if mode != "tracking" {
                continue;
            }

            // Resolve host/port. Prefer metadata_json.host/port, fall
            // back to parsing external_id ("ip:port"). The shim's
            // adopt path puts host/port inside metadata.scan_details
            // rather than at the top level; check that too.
            let meta: Value = serde_json::from_str(&meta_raw).unwrap_or(Value::Null);
            let host = first_string(&[
                meta.get("host"),
                meta.get("ip"),
                meta.pointer("/scan_details/host"),
            ]);
            let port = first_u16(&[
                meta.get("port"),
                meta.pointer("/scan_details/port"),
            ]);
            let (host, port) = match (host, port) {
                (Some(h), Some(p)) => (h, p),
                _ => match parse_host_port(&external_id) {
                    Some(hp) => hp,
                    None => {
                        log::warn!(
                            "[smart_home::ble_esphome] device id={device_id} has no host/port; skipping"
                        );
                        continue;
                    }
                },
            };

            out.push(ProxyRow {
                device_id,
                user_id,
                name,
                host,
                port,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
    .map_err(|e| format!("db: {e}"))
}

fn first_string(candidates: &[Option<&Value>]) -> Option<String> {
    for c in candidates.iter().flatten() {
        if let Some(s) = c.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn first_u16(candidates: &[Option<&Value>]) -> Option<u16> {
    for c in candidates.iter().flatten() {
        if let Some(n) = c.as_u64() {
            if n <= u16::MAX as u64 {
                return Some(n as u16);
            }
        }
    }
    None
}

fn parse_host_port(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    let port: u16 = p.parse().ok()?;
    Some((h.to_string(), port))
}

/// Look up the Noise PSK for a given device label. Returns `None`
/// when the credential is missing OR fails to decrypt — caller then
/// attempts plaintext mode.
async fn load_noise_psk(db_path: &PathBuf, user_id: i64, label: &str) -> Option<[u8; 32]> {
    let db = db_path.clone();
    let label_owned = label.to_string();
    tokio::task::spawn_blocking(move || -> Option<[u8; 32]> {
        // Master key lives under data_dir(db)/master.key.
        let data_dir = db
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let key = crate::crypto::load_or_create_key(&data_dir).ok()?;
        let conn = Connection::open(&db).ok()?;
        // Skip silently if the credentials table doesn't exist yet
        // (fresh installs hit init before all migrations run).
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                   WHERE type='table' AND name='smart_home_credentials'",
                [],
                |r| r.get(0),
            )
            .ok()?;
        if exists == 0 {
            return None;
        }
        let creds = crate::smart_home::credentials::load(
            &conn,
            &key,
            user_id,
            "esphome_native_api",
        )
        .ok()?;
        let cred = creds.into_iter().find(|c| c.label == label_owned)?;
        let psk_b64 = cred.secret.get("psk_b64")?.as_str()?.to_string();
        decode_psk(&psk_b64)
    })
    .await
    .ok()
    .flatten()
}

fn decode_psk(b64: &str) -> Option<[u8; 32]> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Per-proxy worker: connect, subscribe, ingest, reconnect on error.
/// Runs forever (until aborted by the supervisor).
async fn worker_loop(driver: Arc<BleDriver>, db_path: PathBuf, row: ProxyRow) {
    let mut backoff = RECONNECT_MIN;
    loop {
        let psk = load_noise_psk(&db_path, row.user_id, &row.name).await;
        let label = format!(
            "device_id={} {}@{}:{}",
            row.device_id, row.name, row.host, row.port
        );

        match connect_and_pump(&driver, &row, psk).await {
            Ok(()) => {
                log::info!("[smart_home::ble_esphome] {label}: connection closed cleanly");
                backoff = RECONNECT_MIN;
            }
            Err(e) => {
                log::warn!(
                    "[smart_home::ble_esphome] {label}: {e}; reconnect in {}s",
                    backoff.as_secs()
                );
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// One connection lifecycle: handshake, subscribe, pump messages,
/// return on disconnect or error.
async fn connect_and_pump(
    driver: &Arc<BleDriver>,
    row: &ProxyRow,
    psk: Option<[u8; 32]>,
) -> Result<(), String> {
    let addr = format!("{}:{}", row.host, row.port);
    let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("connect timeout to {addr}"))?
        .map_err(|e| format!("connect {addr}: {e}"))?;
    // Disable Nagle so PingResponses + keepalives don't pile up.
    let _ = stream.set_nodelay(true);

    let mut conn = match psk {
        Some(p) => {
            let t = NoiseTransport::handshake(stream, &p)
                .await
                .map_err(|e| format!("noise handshake: {e}"))?;
            Conn::Noise(t)
        }
        None => Conn::Plain(stream),
    };

    // 1. Hello → expect HelloResponse.
    let hello = build_hello_request("syntaur-ble-esphome/0.1");
    conn.write_message(MSG_HELLO_REQUEST, &hello).await?;
    let resp = timeout(READ_TIMEOUT, conn.read_message())
        .await
        .map_err(|_| "hello read timeout".to_string())?
        .ok_or("hello: connection closed")?;
    if resp.msg_type != MSG_HELLO_RESPONSE {
        return Err(format!("expected HelloResponse, got type {}", resp.msg_type));
    }

    // 2. Connect (no password — the Noise PSK is the auth boundary
    //    for encrypted devices; plaintext-only devices have no auth).
    conn.write_message(MSG_CONNECT_REQUEST, &[]).await?;
    let resp = timeout(READ_TIMEOUT, conn.read_message())
        .await
        .map_err(|_| "connect read timeout".to_string())?
        .ok_or("connect: connection closed")?;
    if resp.msg_type != MSG_CONNECT_RESPONSE {
        return Err(format!(
            "expected ConnectResponse, got type {}",
            resp.msg_type
        ));
    }

    // 3. Subscribe to BluetoothLE raw adverts. flags=1 selects the raw
    //    stream; without it the proxy emits the (legacy) decoded form
    //    which loses MAC + ad-data fidelity.
    let mut sub = ProtoEncoder::new();
    sub.encode_uint32(1, 1); // flags
    conn.write_message(MSG_SUBSCRIBE_BLE_ADVERTS, &sub.finish())
        .await?;

    log::info!(
        "[smart_home::ble_esphome] device_id={} subscribed (mode={})",
        row.device_id,
        if matches!(conn, Conn::Noise(_)) {
            "noise"
        } else {
            "plain"
        },
    );

    // 4. Pump until error.
    loop {
        let msg = timeout(READ_TIMEOUT, conn.read_message())
            .await
            .map_err(|_| "read timeout (no traffic)".to_string())?
            .ok_or("connection closed".to_string())?;
        match msg.msg_type {
            MSG_BLE_RAW_ADVERTS_RESPONSE => {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let n = decode_and_push_adverts(driver, row, &msg.payload, ts).await;
                log::trace!(
                    "[smart_home::ble_esphome] device_id={} +{n} adverts",
                    row.device_id
                );
            }
            MSG_PING_REQUEST => {
                conn.write_message(MSG_PING_RESPONSE, &[]).await?;
            }
            MSG_GET_TIME_REQUEST => {
                // GetTimeResponse: epoch_seconds (1, fixed32). Wire
                // type 5 = 4-byte little-endian — emit the bytes raw
                // since voice::esphome_api's encoder doesn't expose
                // a fixed32 helper.
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as u32)
                    .unwrap_or(0);
                let mut payload = Vec::with_capacity(5);
                payload.push((1u8 << 3) | 5); // tag: field 1, wire type 5
                payload.extend_from_slice(&now.to_le_bytes());
                conn.write_message(MSG_GET_TIME_RESPONSE, &payload).await?;
            }
            _ => {
                // Ignore all other message types — list_entities,
                // device_info, etc. The proxy emits them eagerly and
                // we don't act on them here.
            }
        }
    }
}

/// Decode a `BluetoothLERawAdvertisementsResponse` payload and push
/// one observation per advert. Returns the number pushed.
///
/// Wire shape:
///   message BluetoothLERawAdvertisementsResponse {
///     repeated BluetoothLERawAdvertisement advertisements = 1;
///   }
///   message BluetoothLERawAdvertisement {
///     fixed64 address     = 1;  // 48-bit BD_ADDR in low 48 bits
///     sint32  rssi        = 2;
///     uint32  address_type = 3;
///     bytes   data        = 4;
///   }
async fn decode_and_push_adverts(
    driver: &Arc<BleDriver>,
    row: &ProxyRow,
    payload: &[u8],
    ts: i64,
) -> usize {
    let mut pushed = 0usize;
    let mut p = 0;
    while p < payload.len() {
        // Outer field: tag for advertisements (field 1, wire type 2).
        let Some((tag, t_used)) = read_varint(&payload[p..]) else {
            break;
        };
        p += t_used;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x07) as u8;
        if field != 1 || wire != 2 {
            // Skip unknown outer fields.
            if !skip_field(wire, payload, &mut p) {
                break;
            }
            continue;
        }
        let Some((len, l_used)) = read_varint(&payload[p..]) else {
            break;
        };
        p += l_used;
        let len = len as usize;
        if p + len > payload.len() {
            break;
        }
        let inner = &payload[p..p + len];
        p += len;

        if let Some(obs) = parse_one_advert(row, inner, ts) {
            driver.push_observation(obs).await;
            pushed += 1;
        }
    }
    pushed
}

fn parse_one_advert(row: &ProxyRow, inner: &[u8], ts: i64) -> Option<RssiObservation> {
    let mut address_lo: u64 = 0;
    let mut have_address = false;
    let mut rssi: i32 = 0;
    let mut have_rssi = false;
    let mut p = 0;
    while p < inner.len() {
        let (tag, t_used) = read_varint(&inner[p..])?;
        p += t_used;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x07) as u8;
        match (field, wire) {
            (1, 1) => {
                // fixed64 address — little-endian 8 bytes.
                if p + 8 > inner.len() {
                    return None;
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&inner[p..p + 8]);
                address_lo = u64::from_le_bytes(buf);
                p += 8;
                have_address = true;
            }
            (2, 0) => {
                // sint32 rssi (zigzag).
                let (z, used) = read_varint(&inner[p..])?;
                p += used;
                rssi = zigzag_decode(z);
                have_rssi = true;
            }
            _ => {
                if !skip_field(wire, inner, &mut p) {
                    return None;
                }
            }
        }
    }
    if !have_address || !have_rssi {
        return None;
    }
    // 48-bit BD_ADDR in the low 48 bits — render as colon-separated MAC.
    let mac_raw = format_bd_addr(address_lo);
    let mac = canonicalize_mac(&mac_raw)?;
    let rssi = rssi.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    Some(RssiObservation {
        user_id: row.user_id,
        anchor_device_id: row.device_id,
        target_mac: mac,
        rssi,
        ts,
    })
}

fn format_bd_addr(addr: u64) -> String {
    // Top byte → lowest. ESPHome's bluetooth_proxy emits the address
    // packed big-endian into the low 6 bytes of the fixed64 (i.e.,
    // byte[5] is the MSB of the BD_ADDR).
    let bytes = [
        ((addr >> 40) & 0xFF) as u8,
        ((addr >> 32) & 0xFF) as u8,
        ((addr >> 24) & 0xFF) as u8,
        ((addr >> 16) & 0xFF) as u8,
        ((addr >> 8) & 0xFF) as u8,
        (addr & 0xFF) as u8,
    ];
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

fn zigzag_decode(v: u64) -> i32 {
    ((v >> 1) as i64 ^ -((v & 1) as i64)) as i32
}

/// Skip an unknown protobuf field given its wire type. Returns true
/// on success, false on malformed input.
fn skip_field(wire: u8, data: &[u8], p: &mut usize) -> bool {
    match wire {
        0 => match read_varint(&data[*p..]) {
            Some((_, used)) => {
                *p += used;
                true
            }
            None => false,
        },
        1 => {
            // fixed64
            if *p + 8 > data.len() {
                return false;
            }
            *p += 8;
            true
        }
        2 => match read_varint(&data[*p..]) {
            Some((len, used)) => {
                let len = len as usize;
                if *p + used + len > data.len() {
                    return false;
                }
                *p += used + len;
                true
            }
            None => false,
        },
        5 => {
            // fixed32
            if *p + 4 > data.len() {
                return false;
            }
            *p += 4;
            true
        }
        _ => false,
    }
}

fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    eapi::decode_varint(data)
}

/// Tiny enum so plaintext + Noise transports share a call surface.
enum Conn {
    Plain(TcpStream),
    Noise(NoiseTransport<TcpStream>),
}

impl Conn {
    async fn read_message(&mut self) -> Option<RawMessage> {
        match self {
            Conn::Plain(s) => eapi::read_message(s).await,
            Conn::Noise(t) => t.read_message().await,
        }
    }
    async fn write_message(&mut self, t: u32, p: &[u8]) -> Result<(), String> {
        match self {
            Conn::Plain(s) => eapi::write_message(s, t, p).await,
            Conn::Noise(n) => n.write_message(t, p).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_advert_field_layout() {
        // Hand-built BluetoothLERawAdvertisement payload:
        //   field 1 (fixed64): address packed BE in low 6 bytes
        //   field 2 (sint32):  -73  → zigzag = 145
        //   field 3 (uint32):  0
        let mut inner = Vec::new();
        // tag for field 1 wire 1: (1<<3)|1 = 0x09
        inner.push(0x09);
        // address bytes in BE pack of low 48 bits — produces MAC 01:02:03:04:05:06.
        inner.extend_from_slice(&0x0000_0102_0304_0506u64.to_le_bytes());
        // tag for field 2 wire 0: (2<<3)|0 = 0x10
        inner.push(0x10);
        inner.extend_from_slice(&[0x91, 0x01]); // varint 145 (-73 zigzag)
        // tag for field 3 wire 0: 0x18, value 0.
        inner.push(0x18);
        inner.push(0x00);

        let row = ProxyRow {
            device_id: 42,
            user_id: 1,
            name: "p".into(),
            host: "h".into(),
            port: 6053,
        };
        let obs = parse_one_advert(&row, &inner, 1_700_000_000).unwrap();
        assert_eq!(obs.target_mac, "01:02:03:04:05:06");
        assert_eq!(obs.rssi, -73);
        assert_eq!(obs.anchor_device_id, 42);
        assert_eq!(obs.user_id, 1);
    }

    #[test]
    fn zigzag_round_trips() {
        assert_eq!(zigzag_decode(0), 0);
        assert_eq!(zigzag_decode(1), -1);
        assert_eq!(zigzag_decode(2), 1);
        assert_eq!(zigzag_decode(145), -73);
    }

    #[test]
    fn parse_host_port_handles_ipv4() {
        assert_eq!(
            parse_host_port("192.168.1.3:6053"),
            Some(("192.168.1.3".to_string(), 6053))
        );
    }

    #[test]
    fn parse_host_port_rejects_missing_port() {
        assert!(parse_host_port("192.168.1.3").is_none());
    }

    #[test]
    fn decodes_outer_response_with_two_adverts() {
        // Outer response wraps two advertisements as
        //   field 1 (length-delimited): inner1
        //   field 1 (length-delimited): inner2
        let mut inner1 = Vec::new();
        inner1.push(0x09);
        inner1.extend_from_slice(&0x0000_aabb_ccdd_eeffu64.to_le_bytes());
        inner1.push(0x10);
        inner1.extend_from_slice(&[0x5b]); // sint32 -46

        let mut inner2 = Vec::new();
        inner2.push(0x09);
        inner2.extend_from_slice(&0x0000_1122_3344_aabbu64.to_le_bytes());
        inner2.push(0x10);
        inner2.extend_from_slice(&[0x91, 0x01]); // -73

        let mut payload = Vec::new();
        payload.push(0x0a); // tag field 1 wire 2
        payload.push(inner1.len() as u8);
        payload.extend_from_slice(&inner1);
        payload.push(0x0a);
        payload.push(inner2.len() as u8);
        payload.extend_from_slice(&inner2);

        let row = ProxyRow {
            device_id: 7,
            user_id: 1,
            name: "p".into(),
            host: "h".into(),
            port: 6053,
        };
        let mut found = 0;
        let mut p = 0;
        while p < payload.len() {
            let (tag, used) = read_varint(&payload[p..]).unwrap();
            p += used;
            assert_eq!(tag & 0x07, 2);
            let (len, used) = read_varint(&payload[p..]).unwrap();
            p += used;
            let len = len as usize;
            let one = &payload[p..p + len];
            p += len;
            if parse_one_advert(&row, one, 0).is_some() {
                found += 1;
            }
        }
        assert_eq!(found, 2);
    }
}
