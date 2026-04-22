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
/// device seen.
pub async fn scan_for_discriminator(
    want_discriminator: Option<u16>,
    timeout: Duration,
) -> Result<Vec<CommissionableDevice>, btleplug::Error> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters
        .into_iter()
        .next()
        .ok_or_else(|| btleplug::Error::DeviceNotFound)?;

    central
        .start_scan(ScanFilter {
            services: vec![MATTER_SERVICE_UUID],
        })
        .await?;

    let mut events = central.events().await?;
    let mut hits: Vec<CommissionableDevice> = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
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
        if !hits.iter().any(|h| h.address == dev.address) {
            hits.push(dev.clone());
        }
        if want_discriminator.is_some() && !hits.is_empty() {
            break;
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
