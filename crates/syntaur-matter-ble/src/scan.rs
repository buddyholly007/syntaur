//! BLE scan for Matter-commissionable devices.
//!
//! Matter devices in commissioning mode advertise the 16-bit service
//! UUID `0xFFF6` (expanded to the 128-bit base UUID) with a 22-byte
//! "Matter service data" payload containing the discriminator, vendor
//! ID, product ID, and commissioning flags.
//!
//! Service data layout (Matter spec §5.4.2):
//!   Byte 0:      Matter BLE OpCode (0x00 = Commissionable)
//!   Bytes 1-2:   Version[4] + Discriminator[12] (LE)
//!   Bytes 3-4:   Vendor ID (LE)
//!   Bytes 5-6:   Product ID (LE)
//!   Bytes 7+:    Additional data (optional)

use std::time::Duration;

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use futures::StreamExt;
use uuid::Uuid;

/// Matter's 16-bit service UUID, expanded into the Bluetooth base UUID.
pub const MATTER_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FFF6_0000_1000_8000_00805F9B34FB);

/// One hit from a BLE scan — enough for the commissioner to connect.
#[derive(Debug, Clone)]
pub struct CommissionableDevice {
    /// BLE address (MAC on Linux; opaque UUID on macOS).
    pub address: String,
    /// Full 12-bit discriminator from the advertisement.
    pub discriminator: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Advertised device name, if present in the AD record.
    pub local_name: Option<String>,
    /// Most recent RSSI from the scan.
    pub rssi: Option<i16>,
}

/// Scan for up to `timeout` and return every commissionable device
/// whose discriminator (12-bit) matches `want_discriminator`, OR if
/// `want_discriminator` is `None` return every Matter-commissionable
/// device seen. Passes the returned devices' `want_upper_nibble`
/// match is the caller's responsibility (the scan here exits as
/// soon as it has collected at least one Matter device, to avoid
/// burning the peer's commissioning window).
pub async fn scan_for_discriminator(
    want_discriminator: Option<u16>,
    timeout: Duration,
) -> Result<Vec<CommissionableDevice>, btleplug::Error> {
    scan_for_discriminator_ext(want_discriminator, timeout, Duration::from_millis(1500)).await
}

/// Like [`scan_for_discriminator`], but explicitly controls the
/// "grace window" — how long we keep scanning AFTER the first
/// matching Matter-advertising device is seen, to collect duplicates
/// / RPAs of the same device / neighbors. Short grace window (~1.5s)
/// minimizes the time spent scanning once we have something to connect
/// to, which matters when the peer's BLE commissioning window is
/// counting down.
pub async fn scan_for_discriminator_ext(
    want_discriminator: Option<u16>,
    timeout: Duration,
    post_first_hit_grace: Duration,
) -> Result<Vec<CommissionableDevice>, btleplug::Error> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters
        .into_iter()
        .next()
        .ok_or_else(|| btleplug::Error::DeviceNotFound)?;

    // 2026-04-26: Drop BlueZ UUID filter — BlueZ"s SetDiscoveryFilter UUID list
    // checks only the device"s Service Class UUID advertising field, NOT entries
    // in ServiceData. Eve Energy advertises 0xFFF6 only via ServiceData, so the
    // filtered scan never returns it. We post-filter on service_data.get below.
    central.start_scan(ScanFilter::default()).await?;

    let mut events = central.events().await?;
    let mut hits: Vec<CommissionableDevice> = Vec::new();
    // 2026-04-25: stale-cache guard. BlueZ replays cached "known device"
    // entries when scan starts, including peripherals whose RPA has rotated
    // since. Accepting on first-event causes connect() to hang on a MAC the
    // peer no longer responds to. Require >= 2 events for the same id within
    // the scan session to confirm it is actively advertising NOW.
    use std::collections::HashMap;
    let mut sighting_counts: HashMap<btleplug::platform::PeripheralId, u32> = HashMap::new();
    let hard_deadline = tokio::time::Instant::now() + timeout;
    // Set once we see our first Matter-service advertisement — stops
    // the loop `post_first_hit_grace` later instead of after the full
    // `timeout`. Keeps the peer's commissioning window intact.
    let mut soft_deadline: Option<tokio::time::Instant> = None;

    loop {
        let now = tokio::time::Instant::now();
        let remaining_hard = hard_deadline.saturating_duration_since(now);
        let remaining = match soft_deadline {
            Some(soft) => {
                let remaining_soft = soft.saturating_duration_since(now);
                remaining_soft.min(remaining_hard)
            }
            None => remaining_hard,
        };
        if remaining.is_zero() {
            break;
        }
        let evt = match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(evt)) => evt,
            _ => break,
        };
        let id = match &evt {
            CentralEvent::DeviceDiscovered(id)
            | CentralEvent::DeviceUpdated(id)
            | CentralEvent::ServiceDataAdvertisement { id, .. } => id.clone(),
            _ => continue,
        };
        let Ok(periph) = central.peripheral(&id).await else {
            continue;
        };
        let Ok(Some(props)) = periph.properties().await else {
            continue;
        };
        let Some(service_data) = props.service_data.get(&MATTER_SERVICE_UUID) else {
            continue;
        };
        let Some(dev) = parse_matter_service_data(service_data, &props.address.to_string()) else {
            continue;
        };
        let dev = CommissionableDevice {
            local_name: props.local_name.clone(),
            rssi: props.rssi,
            ..dev
        };
        if let Some(want) = want_discriminator {
            if dev.discriminator != want {
                continue;
            }
        }
        let count = sighting_counts.entry(id.clone()).or_insert(0);
        *count += 1;
        if *count < 2 {
            // Discard the first sighting — could be a cache replay.
            continue;
        }
        if !hits.iter().any(|h| h.address == dev.address) {
            hits.push(dev.clone());
        }
        if want_discriminator.is_some() {
            // Exact match requested and confirmed (>=2 sightings) — stop.
            break;
        }
        // No exact filter: start a short grace window after first hit
        // so duplicates of the same device + any RPA variants + any
        // neighbors in range get collected, but we don't burn the full
        // `timeout` on a peer whose commissioning window is ticking.
        if soft_deadline.is_none() && !hits.is_empty() {
            soft_deadline = Some(tokio::time::Instant::now() + post_first_hit_grace);
        }
    }

    let _ = central.stop_scan().await;
    Ok(hits)
}

fn parse_matter_service_data(data: &[u8], address: &str) -> Option<CommissionableDevice> {
    if data.len() < 7 {
        return None;
    }
    if data[0] != 0x00 {
        // Only opcode 0 (commissionable) is interesting for us.
        return None;
    }
    let version_and_disc = u16::from_le_bytes([data[1], data[2]]);
    let discriminator = version_and_disc & 0x0FFF; // low 12 bits
    let vendor_id = u16::from_le_bytes([data[3], data[4]]);
    let product_id = u16::from_le_bytes([data[5], data[6]]);
    Some(CommissionableDevice {
        address: address.to_string(),
        discriminator,
        vendor_id,
        product_id,
        local_name: None,
        rssi: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_service_data_golden() {
        // version=0, disc=0xF00, vid=0x2ECE, pid=0x42D3 (same chandelier QR as syntaur-matter tests).
        // version|disc little-endian = 0x0F00 → bytes [0x00, 0x0F]
        let data = [
            0x00, // opcode
            0x00, 0x0F, // version(4)|disc(12) LE = 0x0F00
            0xCE, 0x2E, // vid LE
            0xD3, 0x42, // pid LE
        ];
        let d = parse_matter_service_data(&data, "AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(d.discriminator, 0xF00);
        assert_eq!(d.vendor_id, 0x2ECE);
        assert_eq!(d.product_id, 0x42D3);
    }
}
