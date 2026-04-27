//! Local Bluetooth host scanner — third RSSI source for the BLE driver
//! (alongside MQTT and ESPHome native-API ingest).
//!
//! Compiled only when the `ble-host` cargo feature is enabled. Headless
//! deployments without a local BT adapter (claudevm, GitHub CI, most
//! servers) build the no-op `start_host_scanner` shim below and the
//! driver continues to ingest exclusively from the MQTT bus.
//!
//! ## Behavior
//!
//! At startup the scanner snapshots which anchors are flagged
//! `host_scanner = true` (one per tenant). It opens the system's first
//! BT adapter, starts a passive scan, and for every advertisement
//! received fans the (mac, rssi) pair into one observation per
//! configured host anchor — so each tenant gets a parallel observation
//! stream attributed to their own anchor.
//!
//! No-adapter handling: if `Manager::new()` returns no adapters, or the
//! first adapter rejects `start_scan`, the task logs and exits cleanly.
//! The driver itself stays running on the MQTT path.
//!
//! Anchor reload: the scanner refreshes its host-anchor snapshot every
//! 60 seconds so settings-page changes take effect without a restart.

#![cfg_attr(not(feature = "ble-host"), allow(dead_code))]

use std::sync::Arc;
use std::time::Duration;

use super::ble::{BleDriver, RssiObservation};

/// Spawn the host scanner. Returns a JoinHandle that completes only
/// when the inner task exits (typically never on a real adapter, or
/// immediately on a feature-disabled / no-adapter build).
pub fn start_host_scanner(driver: Arc<BleDriver>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run(driver).await;
    })
}

#[cfg(not(feature = "ble-host"))]
async fn run(_driver: Arc<BleDriver>) {
    log::info!(
        "[smart_home::ble_host] feature `ble-host` disabled — host scanner is a no-op"
    );
}

#[cfg(feature = "ble-host")]
async fn run(driver: Arc<BleDriver>) {
    use btleplug::api::{Central, CentralEvent, Manager as _, ScanFilter};
    use btleplug::platform::Manager;
    use futures_util::stream::StreamExt;

    // Open the first available adapter. Most laptops/desktops with an
    // internal BT chip have exactly one; servers commonly have zero.
    let manager = match Manager::new().await {
        Ok(m) => m,
        Err(e) => {
            log::warn!(
                "[smart_home::ble_host] btleplug Manager::new failed ({e}); host scanner disabled"
            );
            return;
        }
    };
    let adapters = match manager.adapters().await {
        Ok(a) => a,
        Err(e) => {
            log::warn!(
                "[smart_home::ble_host] adapters() failed ({e}); host scanner disabled"
            );
            return;
        }
    };
    let Some(central) = adapters.into_iter().next() else {
        log::info!(
            "[smart_home::ble_host] no Bluetooth adapter found on host; scanner disabled"
        );
        return;
    };

    let mut events = match central.events().await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[smart_home::ble_host] events() failed ({e}); host scanner disabled");
            return;
        }
    };
    if let Err(e) = central.start_scan(ScanFilter::default()).await {
        log::warn!(
            "[smart_home::ble_host] start_scan failed ({e}); host scanner disabled"
        );
        return;
    }
    log::info!("[smart_home::ble_host] host scanner started on local adapter");

    // Refresh the host-anchor snapshot once per minute so settings-page
    // edits take effect without restarting the gateway.
    let mut refresh = tokio::time::interval(Duration::from_secs(60));
    let mut anchors = driver.host_anchors_snapshot().await;
    log::info!(
        "[smart_home::ble_host] {} host-anchor(s) configured at startup",
        anchors.len()
    );

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                anchors = driver.host_anchors_snapshot().await;
            }
            ev = events.next() => {
                let Some(ev) = ev else { break };
                let (mac_raw, rssi) = match ev {
                    CentralEvent::DeviceDiscovered(id)
                    | CentralEvent::DeviceUpdated(id) => {
                        let Ok(props) = central.peripheral(&id).await else { continue };
                        let Ok(Some(props)) = props.properties().await else { continue };
                        let Some(rssi) = props.rssi else { continue };
                        (props.address.to_string(), rssi as i64)
                    }
                    CentralEvent::ManufacturerDataAdvertisement { id, .. }
                    | CentralEvent::ServiceDataAdvertisement { id, .. }
                    | CentralEvent::ServicesAdvertisement { id, .. } => {
                        let Ok(props) = central.peripheral(&id).await else { continue };
                        let Ok(Some(props)) = props.properties().await else { continue };
                        let Some(rssi) = props.rssi else { continue };
                        (props.address.to_string(), rssi as i64)
                    }
                    _ => continue,
                };
                let Some(mac) = super::ble::canonicalize_mac(&mac_raw) else {
                    continue;
                };
                if anchors.is_empty() {
                    continue;
                }
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let rssi = rssi.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                for a in &anchors {
                    driver
                        .push_observation(RssiObservation {
                            user_id: a.user_id,
                            anchor_device_id: a.anchor_device_id,
                            target_mac: mac.clone(),
                            rssi,
                            ts,
                        })
                        .await;
                }
            }
        }
    }

    log::info!("[smart_home::ble_host] host scanner event stream closed; exiting");
}
