//! BLE scanner: claims the BlueZ adapter via btleplug, listens for advert
//! events, reconstructs raw AD-structure bytes, and broadcasts them to all
//! connected ESPHome-API subscribers.

use std::time::Duration;

use btleplug::api::{BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use futures::stream::StreamExt;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::protocol::RawAdvert;

/// One advert as observed by the scanner. We send these on a broadcast channel.
/// Subscribers (TCP connections) decide on their own whether to forward
/// (depends on whether the peer has subscribed to BLE events yet).
#[derive(Debug, Clone)]
pub struct ScannerEvent {
    pub advert: RawAdvert,
}

/// What the scanner publishes once on startup so we can fill DeviceInfoResponse.
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub mac: String,
}

/// Bring the adapter up, start scanning, and return the broadcast receiver +
/// adapter MAC. Buffer size 1024 = a generous burst window; lagging subscribers
/// get a `Lagged` error which we treat as non-fatal.
pub async fn start(
    tx: broadcast::Sender<ScannerEvent>,
) -> Result<AdapterInfo, Box<dyn std::error::Error + Send + Sync>> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let adapter = adapters
        .into_iter()
        .next()
        .ok_or("no Bluetooth adapter present (btleplug found 0)")?;
    let info = adapter.adapter_info().await?;
    log::info!("[scanner] using adapter: {}", info);

    // Best-effort MAC discovery. btleplug doesn't expose adapter MAC directly,
    // so we use the adapter info string and fall back to a derived MAC.
    let mac = derive_adapter_mac(&info);
    log::info!("[scanner] adapter mac: {}", mac);

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| format!("start_scan: {e}"))?;

    let mut events = adapter.events().await?;
    let tx_for_loop = tx.clone();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if let Some(advert) = handle_event(&adapter, event).await {
                // send returns Err only when there are zero subscribers — that's
                // fine, it just means nobody's connected yet.
                let _ = tx_for_loop.send(ScannerEvent { advert });
            }
        }
        log::warn!("[scanner] event stream ended unexpectedly");
    });

    // Periodic health log.
    let tx_for_health = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            log::info!(
                "[scanner] running — {} subscriber(s)",
                tx_for_health.receiver_count()
            );
        }
    });

    Ok(AdapterInfo { mac })
}

fn derive_adapter_mac(info: &str) -> String {
    // 1) btleplug `adapter_info()` on some BlueZ versions returns
    //    "hci0 (00:1A:7D:DA:71:13)" — extract the MAC.
    if let (Some(open), Some(close)) = (info.find('('), info.find(')')) {
        if open + 1 < close {
            let candidate = &info[open + 1..close];
            if candidate.len() == 17 && candidate.chars().filter(|c| *c == ':').count() == 5 {
                return candidate.to_lowercase();
            }
        }
    }

    // 2) Other BlueZ versions return "hci0 (usb:v1D6Bp0246d0555)" — fall back
    //    to /sys/class/bluetooth/<hciN>/address, which is always the real MAC.
    let hci = info.split_whitespace().next().unwrap_or("hci0");
    let path = format!("/sys/class/bluetooth/{}/address", hci);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim().to_lowercase();
        if trimmed.len() == 17 && trimmed.chars().filter(|c| *c == ':').count() == 5 {
            return trimmed;
        }
    }
    // 3) Last resort — derived from hostname so HA can still dedupe.
    let hn = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "syntaur-shim".into());
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in hn.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let bytes = hash.to_be_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

async fn handle_event(adapter: &Adapter, event: CentralEvent) -> Option<RawAdvert> {
    let id = match event {
        CentralEvent::DeviceDiscovered(id)
        | CentralEvent::DeviceUpdated(id)
        | CentralEvent::ManufacturerDataAdvertisement { id, .. }
        | CentralEvent::ServiceDataAdvertisement { id, .. }
        | CentralEvent::ServicesAdvertisement { id, .. } => id,
        _ => return None,
    };

    let peripheral = adapter.peripheral(&id).await.ok()?;
    let props = peripheral.properties().await.ok()??;
    let address: BDAddr = props.address;
    let rssi = props.rssi.unwrap_or(-127) as i32;

    // address_type: 0=public, 1=random. btleplug exposes Option<AddressType>.
    // Default to Public if unknown — almost all real devices are one of the two
    // and Bermuda math doesn't care which.
    let address_type = match props.address_type {
        Some(btleplug::api::AddressType::Random) => 1,
        _ => 0,
    };

    let mac_u64 = bdaddr_to_u64(&address);
    let data = reconstruct_advert(
        props.local_name.as_deref(),
        props.tx_power_level,
        &props.manufacturer_data,
        &props.service_data,
        &props.services,
    );

    Some(RawAdvert {
        address: mac_u64,
        rssi,
        address_type,
        data,
    })
}

fn bdaddr_to_u64(a: &BDAddr) -> u64 {
    // ESPHome's bluetooth_proxy serializes MAC big-endian into a u64:
    //   addr[0] << 40 | addr[1] << 32 | ... | addr[5]
    // BDAddr's `into_inner()` returns [u8; 6] in the same order ("aa:bb:..." => [aa, bb, ...]).
    let bytes = a.into_inner();
    ((bytes[0] as u64) << 40)
        | ((bytes[1] as u64) << 32)
        | ((bytes[2] as u64) << 24)
        | ((bytes[3] as u64) << 16)
        | ((bytes[4] as u64) << 8)
        | (bytes[5] as u64)
}

/// Reassemble GAP AD structures from btleplug's parsed properties. Output is
/// the same byte sequence an ESPHome bluetooth_proxy would forward in
/// `BluetoothLERawAdvertisement.data`. We're conservative — only emit fields
/// we have data for — but cover what real consumers (Bermuda, BTHome, iBeacon)
/// rely on.
fn reconstruct_advert(
    name: Option<&str>,
    tx_power: Option<i16>,
    manufacturer_data: &std::collections::HashMap<u16, Vec<u8>>,
    service_data: &std::collections::HashMap<Uuid, Vec<u8>>,
    services: &[Uuid],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);

    // --- Flags (AD type 0x01) — we don't actually know flags from BlueZ;
    // emit a sensible default ("LE General Discoverable + BR/EDR Not Supported")
    // only when we have *some* other content to emit.
    let mut have_content = name.is_some()
        || tx_power.is_some()
        || !manufacturer_data.is_empty()
        || !service_data.is_empty()
        || !services.is_empty();

    if have_content {
        push_ad(&mut out, 0x01, &[0x06]);
    }

    if let Some(p) = tx_power {
        push_ad(&mut out, 0x0A, &[(p as i8) as u8]);
    }

    // 16-bit and 128-bit Service UUIDs split.
    let mut uuids16 = Vec::new();
    let mut uuids128 = Vec::new();
    for u in services {
        if let Some(short) = uuid_to_u16(u) {
            uuids16.extend_from_slice(&short.to_le_bytes());
        } else {
            let mut bytes = u.as_bytes().to_vec();
            bytes.reverse(); // GAP wire order is LE/reversed
            uuids128.extend_from_slice(&bytes);
        }
    }
    if !uuids16.is_empty() {
        push_ad(&mut out, 0x03, &uuids16); // complete list, 16-bit
    }
    if !uuids128.is_empty() {
        push_ad(&mut out, 0x07, &uuids128); // complete list, 128-bit
    }

    // Service Data
    for (uuid, payload) in service_data {
        if let Some(short) = uuid_to_u16(uuid) {
            let mut buf: Vec<u8> = Vec::with_capacity(2 + payload.len());
            buf.extend_from_slice(&short.to_le_bytes());
            buf.extend_from_slice(payload);
            push_ad(&mut out, 0x16, &buf); // service data, 16-bit UUID
        } else {
            let mut buf: Vec<u8> = Vec::with_capacity(16 + payload.len());
            let mut bytes = uuid.as_bytes().to_vec();
            bytes.reverse();
            buf.extend_from_slice(&bytes);
            buf.extend_from_slice(payload);
            push_ad(&mut out, 0x21, &buf); // service data, 128-bit UUID
        }
    }

    // Manufacturer-specific data
    for (company_id, payload) in manufacturer_data {
        let mut buf = Vec::with_capacity(2 + payload.len());
        buf.extend_from_slice(&company_id.to_le_bytes());
        buf.extend_from_slice(payload);
        push_ad(&mut out, 0xFF, &buf);
    }

    if let Some(n) = name {
        if !n.is_empty() {
            push_ad(&mut out, 0x09, n.as_bytes()); // complete local name
        }
    }

    out
}

fn push_ad(out: &mut Vec<u8>, ad_type: u8, payload: &[u8]) {
    // GAP AD struct: [length: u8] [type: u8] [data...]
    // length covers the type byte too. Cap at 255 bytes to avoid bad output;
    // BlueZ won't produce single AD entries that large in practice.
    let len = payload.len().min(254) + 1;
    out.push(len as u8);
    out.push(ad_type);
    out.extend_from_slice(&payload[..(len - 1)]);
}

fn uuid_to_u16(u: &Uuid) -> Option<u16> {
    // Bluetooth-SIG short UUIDs sit inside the
    //   0000xxxx-0000-1000-8000-00805f9b34fb
    // base. If bytes 4..16 of the candidate match the base and bytes 0..2 are
    // zero, return the embedded u16; otherwise None.
    const BASE: [u8; 16] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34,
        0xfb,
    ];
    let b = u.as_bytes();
    if b[0] == 0 && b[1] == 0 && b[4..] == BASE[4..] {
        Some(u16::from_be_bytes([b[2], b[3]]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bdaddr_to_u64_known() {
        let a = BDAddr::from([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(bdaddr_to_u64(&a), 0xaabbccddeeff);
    }

    #[test]
    fn uuid_short_form_detected() {
        // BTHome v2 service UUID: 0xFCD2
        let u = Uuid::parse_str("0000fcd2-0000-1000-8000-00805f9b34fb").unwrap();
        assert_eq!(uuid_to_u16(&u), Some(0xFCD2));
    }

    #[test]
    fn uuid_random_full_not_short() {
        let u = Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();
        assert_eq!(uuid_to_u16(&u), None);
    }

    #[test]
    fn reconstruct_emits_manufacturer_and_name() {
        let mut mfg = std::collections::HashMap::new();
        mfg.insert(0x004C, vec![0x02, 0x15]); // Apple iBeacon header
        let bytes = reconstruct_advert(Some("Beacon"), Some(-12), &mfg, &Default::default(), &[]);
        // Should contain AD type 0xFF (manufacturer) and 0x09 (complete name)
        assert!(bytes.windows(1).any(|w| w == [0xFF]));
        assert!(bytes.windows(1).any(|w| w == [0x09]));
    }
}
