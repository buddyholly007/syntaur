//! ESPHome native-API message-type IDs and typed builders for the messages
//! a bluetooth_proxy server (this shim) needs to send + parse.
//!
//! Only the subset required for the BLE-advert flow is implemented:
//! Hello, Connect, Disconnect, Ping, GetTime, DeviceInfo, ListEntities,
//! SubscribeBluetoothLEAdvertisements, BluetoothLERawAdvertisementsResponse.
//! Everything else gets a no-op acknowledgement or is ignored.

use crate::codec::{ProtoDecoder, ProtoEncoder, ProtoField};

// ── Message type IDs ────────────────────────────────────────────────────────
//
// Source: esphome/components/api/api.proto
//
// These IDs are stable across ESPHome firmware versions; the protocol uses
// the on-the-wire u32 to dispatch, not the field numbers.

pub const MSG_HELLO_REQUEST: u32 = 1;
pub const MSG_HELLO_RESPONSE: u32 = 2;
pub const MSG_CONNECT_REQUEST: u32 = 3;
pub const MSG_CONNECT_RESPONSE: u32 = 4;
pub const MSG_DISCONNECT_REQUEST: u32 = 5;
pub const MSG_DISCONNECT_RESPONSE: u32 = 6;
pub const MSG_PING_REQUEST: u32 = 7;
pub const MSG_PING_RESPONSE: u32 = 8;
pub const MSG_DEVICE_INFO_REQUEST: u32 = 9;
pub const MSG_DEVICE_INFO_RESPONSE: u32 = 10;
pub const MSG_LIST_ENTITIES_REQUEST: u32 = 11;
pub const MSG_LIST_ENTITIES_DONE_RESPONSE: u32 = 19;
pub const MSG_SUBSCRIBE_STATES_REQUEST: u32 = 20;
pub const MSG_SUBSCRIBE_LOGS_REQUEST: u32 = 28;
pub const MSG_SUBSCRIBE_HOMEASSISTANT_SERVICES_REQUEST: u32 = 34;
pub const MSG_SUBSCRIBE_HOME_ASSISTANT_STATES_REQUEST: u32 = 38;
pub const MSG_GET_TIME_REQUEST: u32 = 36;
pub const MSG_GET_TIME_RESPONSE: u32 = 37;
pub const MSG_SUBSCRIBE_BLUETOOTH_LE_ADVERTISEMENTS_REQUEST: u32 = 66;
pub const MSG_BLUETOOTH_LE_ADVERTISEMENT_RESPONSE: u32 = 67;
pub const MSG_BLUETOOTH_CONNECTIONS_FREE_RESPONSE: u32 = 81;
pub const MSG_BLUETOOTH_GATT_ERROR_RESPONSE: u32 = 82;
pub const MSG_BLUETOOTH_LE_RAW_ADVERTISEMENTS_RESPONSE: u32 = 93;
pub const MSG_BLUETOOTH_SCANNER_STATE_RESPONSE: u32 = 126;
pub const MSG_BLUETOOTH_SCANNER_SET_MODE_REQUEST: u32 = 127;

// ── Bluetooth-proxy feature flag bits ───────────────────────────────────────
//
// Sent in DeviceInfoResponse.bluetooth_proxy_feature_flags so the client
// (HA, Syntaur) knows what the proxy can do.

#[allow(dead_code)]
pub const FEATURE_PASSIVE_SCAN: u32 = 1 << 0;
#[allow(dead_code)]
pub const FEATURE_ACTIVE_CONNECTIONS: u32 = 1 << 1;
#[allow(dead_code)]
pub const FEATURE_REMOTE_CACHING: u32 = 1 << 2;
#[allow(dead_code)]
pub const FEATURE_PAIRING: u32 = 1 << 3;
#[allow(dead_code)]
pub const FEATURE_CACHE_CLEARING: u32 = 1 << 4;
pub const FEATURE_RAW_ADVERTISEMENTS: u32 = 1 << 5;
pub const FEATURE_STATE_AND_MODE: u32 = 1 << 6;

// Scanner state enum (BluetoothScannerStateResponse.state)
#[allow(dead_code)]
pub const SCANNER_STATE_IDLE: u32 = 0;
#[allow(dead_code)]
pub const SCANNER_STATE_STARTING: u32 = 1;
pub const SCANNER_STATE_RUNNING: u32 = 2;
#[allow(dead_code)]
pub const SCANNER_STATE_FAILED: u32 = 3;
#[allow(dead_code)]
pub const SCANNER_STATE_STOPPING: u32 = 4;
#[allow(dead_code)]
pub const SCANNER_STATE_STOPPED: u32 = 5;

// Scanner mode enum (BluetoothScannerStateResponse.mode)
#[allow(dead_code)]
pub const SCANNER_MODE_PASSIVE: u32 = 0;
pub const SCANNER_MODE_ACTIVE: u32 = 1;

// ── HelloRequest parser (client_info, api_version_major, api_version_minor) ──

pub struct HelloRequestData {
    pub client_info: String,
    pub api_version_major: u32,
    pub api_version_minor: u32,
}

pub fn parse_hello_request(payload: &[u8]) -> HelloRequestData {
    let mut r = HelloRequestData {
        client_info: String::new(),
        api_version_major: 0,
        api_version_minor: 0,
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(f) = dec.next_field() {
        match f {
            ProtoField::Bytes(1, b) => r.client_info = String::from_utf8_lossy(b).to_string(),
            ProtoField::Varint(2, v) => r.api_version_major = v as u32,
            ProtoField::Varint(3, v) => r.api_version_minor = v as u32,
            _ => {}
        }
    }
    r
}

// ── HelloResponse builder ───────────────────────────────────────────────────
//
// HelloResponse {
//   uint32 api_version_major = 1;
//   uint32 api_version_minor = 2;
//   string server_info = 3;
//   string name = 4;
// }

pub fn build_hello_response(server_info: &str, name: &str) -> Vec<u8> {
    let mut e = ProtoEncoder::new();
    e.encode_uint32(1, 1);
    e.encode_uint32(2, 10);
    e.encode_string(3, server_info);
    e.encode_string(4, name);
    e.finish()
}

// ── ConnectRequest / ConnectResponse ────────────────────────────────────────
//
// ConnectRequest { string password = 1; }
// ConnectResponse { bool invalid_password = 1; }
//
// We don't enforce a password — first iteration is plaintext, LAN-only.
// HA's ESPHome integration accepts an empty-password proxy without prompting.

pub fn build_connect_response(invalid_password: bool) -> Vec<u8> {
    let mut e = ProtoEncoder::new();
    e.encode_bool(1, invalid_password);
    e.finish()
}

// ── DeviceInfoResponse ──────────────────────────────────────────────────────
//
// Field numbers verified against esphome/components/api/api.proto (DeviceInfoResponse):
//   1  bool   uses_password
//   2  string name
//   3  string mac_address
//   4  string esphome_version
//   5  string compilation_time
//   6  string model
//   7  bool   has_deep_sleep
//   8  string project_name
//   9  string project_version
//   10 uint32 webserver_port
//   11 uint32 legacy_bluetooth_proxy_version (deprecated)
//   12 string manufacturer
//   13 string friendly_name
//   14 uint32 legacy_voice_assistant_version (deprecated)
//   15 uint32 bluetooth_proxy_feature_flags
//   16 string suggested_area
//   17 uint32 voice_assistant_feature_flags
//   18 string bluetooth_mac_address
//   19 bool   api_encryption_supported

pub struct DeviceInfo<'a> {
    pub name: &'a str,
    pub mac_address: &'a str,
    pub bluetooth_mac_address: &'a str,
    pub esphome_version: &'a str,
    pub model: &'a str,
    pub manufacturer: &'a str,
    pub friendly_name: &'a str,
    pub suggested_area: &'a str,
    pub feature_flags: u32,
}

pub fn build_device_info_response(d: &DeviceInfo) -> Vec<u8> {
    let mut e = ProtoEncoder::new();
    e.encode_bool(1, false); // uses_password
    e.encode_string(2, d.name);
    e.encode_string(3, d.mac_address);
    e.encode_string(4, d.esphome_version);
    // compilation_time (5) intentionally empty
    e.encode_string(6, d.model);
    e.encode_string(12, d.manufacturer);
    e.encode_string(13, d.friendly_name);
    e.encode_uint32(15, d.feature_flags);
    e.encode_string(16, d.suggested_area);
    e.encode_string(18, d.bluetooth_mac_address);
    e.finish()
}

// ── BluetoothScannerStateResponse ────────────────────────────────────────────
//
// BluetoothScannerStateResponse {
//   BluetoothScannerState state = 1;
//   BluetoothScannerMode mode = 2;
// }

pub fn build_scanner_state_response(state: u32, mode: u32) -> Vec<u8> {
    let mut e = ProtoEncoder::new();
    e.encode_uint32(1, state);
    e.encode_uint32(2, mode);
    e.finish()
}

// ── BluetoothLERawAdvertisement / Response ──────────────────────────────────
//
// BluetoothLERawAdvertisement {
//   uint64 address = 1;
//   sint32 rssi = 2;
//   uint32 address_type = 3;
//   bytes data = 4;
// }
// BluetoothLERawAdvertisementsResponse {
//   repeated BluetoothLERawAdvertisement advertisements = 1;
// }

#[derive(Debug, Clone)]
pub struct RawAdvert {
    pub address: u64,
    pub rssi: i32,
    pub address_type: u32,
    pub data: Vec<u8>,
}

fn build_one_raw_advert(a: &RawAdvert) -> Vec<u8> {
    let mut e = ProtoEncoder::new();
    e.encode_uint64(1, a.address);
    e.encode_sint32(2, a.rssi);
    e.encode_uint32(3, a.address_type);
    e.encode_bytes(4, &a.data);
    e.finish()
}

pub fn build_raw_advertisements_response(adverts: &[RawAdvert]) -> Vec<u8> {
    let mut out = ProtoEncoder::new();
    for a in adverts {
        let bytes = build_one_raw_advert(a);
        out.encode_message(1, &bytes);
    }
    out.finish()
}

// ── SubscribeBluetoothLEAdvertisementsRequest parser ─────────────────────────
//
// SubscribeBluetoothLEAdvertisementsRequest { uint32 flags = 1; }
//   flags & 1 == raw mode (BluetoothLERawAdvertisementsResponse, mt=93)
//   flags == 0     == legacy parsed mode (mt=67) — we always emit raw.

pub fn parse_subscribe_ble_request(payload: &[u8]) -> u32 {
    let mut dec = ProtoDecoder::new(payload);
    while let Some(f) = dec.next_field() {
        if let ProtoField::Varint(1, v) = f {
            return v as u32;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_request_roundtrip() {
        let mut e = ProtoEncoder::new();
        e.encode_string(1, "Home Assistant 2026.4.0 (esphome)");
        e.encode_uint32(2, 1);
        e.encode_uint32(3, 10);
        let data = e.finish();
        let parsed = parse_hello_request(&data);
        assert_eq!(parsed.client_info, "Home Assistant 2026.4.0 (esphome)");
        assert_eq!(parsed.api_version_major, 1);
        assert_eq!(parsed.api_version_minor, 10);
    }

    #[test]
    fn raw_advert_response_nonempty() {
        let advert = RawAdvert {
            address: 0xaabbccddeeff,
            rssi: -67,
            address_type: 1,
            data: vec![0x02, 0x01, 0x06, 0x05, 0x09, b'F', b'O', b'O'],
        };
        let resp = build_raw_advertisements_response(&[advert]);
        assert!(!resp.is_empty());
        // First field should be field 1 (advertisements), wire type 2 (length-delimited)
        // tag = (1 << 3) | 2 = 0x0A
        assert_eq!(resp[0], 0x0A);
    }

    #[test]
    fn subscribe_ble_request_parses_flags() {
        let mut e = ProtoEncoder::new();
        e.encode_uint32(1, 1);
        let buf = e.finish();
        assert_eq!(parse_subscribe_ble_request(&buf), 1);
    }
}
