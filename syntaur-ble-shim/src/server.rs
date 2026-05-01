//! ESPHome native-API TCP server. One connection per subscriber (HA, Syntaur,
//! whatever). Multi-subscriber by construction: each connection's task owns
//! its own broadcast::Receiver and forwards adverts independently.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Semaphore};

/// Soft cap on concurrent client connections. ESPHome bluetooth_proxy
/// firmware caps at 4. We're more generous — the realistic load is
/// 2 (HA + Syntaur) plus the occasional discovery probe — but bound
/// it to keep a runaway scanner from exhausting fds.
const MAX_CONCURRENT_CLIENTS: usize = 16;

use crate::codec::{read_message, write_message, ProtoEncoder};
use crate::protocol::{self as p, RawAdvert};
use crate::scanner::ScannerEvent;

#[derive(Clone)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub friendly_name: String,
    pub mac_address: String,
    pub bluetooth_mac_address: String,
    pub suggested_area: String,
    pub version: String,
}

pub async fn run(
    cfg: Arc<ServerConfig>,
    advert_tx: broadcast::Sender<ScannerEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(&cfg.bind_addr).await?;
    log::info!("[server] listening on {}", cfg.bind_addr);
    let conn_limit = Arc::new(Semaphore::new(MAX_CONCURRENT_CLIENTS));

    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("[server] accept failed: {e}");
                continue;
            }
        };
        let permit = match conn_limit.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                log::warn!(
                    "[server] connection cap reached ({}); rejecting {peer}",
                    MAX_CONCURRENT_CLIENTS
                );
                drop(sock);
                continue;
            }
        };
        let cfg = cfg.clone();
        let rx = advert_tx.subscribe();
        tokio::spawn(async move {
            log::info!("[server] new client: {peer}");
            if let Err(e) = handle_client(sock, cfg, rx).await {
                log::info!("[server] client {peer} disconnected: {e}");
            } else {
                log::info!("[server] client {peer} closed cleanly");
            }
            drop(permit);
        });
    }
}

async fn handle_client(
    sock: TcpStream,
    cfg: Arc<ServerConfig>,
    mut rx: broadcast::Receiver<ScannerEvent>,
) -> Result<(), String> {
    sock.set_nodelay(true).map_err(|e| format!("nodelay: {e}"))?;
    let (read_half, mut write_half) = sock.into_split();
    let mut reader = BufReader::new(read_half);

    // Per-connection state
    let mut completed_hello = false;
    let mut subscribed_to_adverts = false;

    loop {
        // Wait for either inbound message or outbound advert.
        tokio::select! {
            biased;

            msg = read_message(&mut reader) => {
                let msg = match msg {
                    Some(m) => m,
                    None => return Ok(()), // clean EOF or framing error
                };

                match msg.msg_type {
                    p::MSG_HELLO_REQUEST => {
                        let hello = p::parse_hello_request(&msg.payload);
                        log::info!(
                            "[server] hello from \"{}\" (api {}.{})",
                            hello.client_info,
                            hello.api_version_major,
                            hello.api_version_minor
                        );
                        let resp = p::build_hello_response(
                            &format!("syntaur-ble-shim {}", cfg.version),
                            &cfg.friendly_name,
                        );
                        write_message(&mut write_half, p::MSG_HELLO_RESPONSE, &resp)
                            .await
                            .map_err(|e| format!("hello write: {e}"))?;
                        completed_hello = true;
                    }

                    p::MSG_CONNECT_REQUEST => {
                        // We don't enforce a password; respond invalid_password=false.
                        let resp = p::build_connect_response(false);
                        write_message(&mut write_half, p::MSG_CONNECT_RESPONSE, &resp)
                            .await
                            .map_err(|e| format!("connect write: {e}"))?;
                        log::debug!("[server] connect ok");
                    }

                    p::MSG_DEVICE_INFO_REQUEST => {
                        if !completed_hello {
                            return Err("DeviceInfoRequest before Hello".into());
                        }
                        let info = p::DeviceInfo {
                            name: &cfg.friendly_name,
                            mac_address: &cfg.mac_address,
                            bluetooth_mac_address: &cfg.bluetooth_mac_address,
                            esphome_version: "2026.4.0",
                            model: "syntaur-ble-shim",
                            manufacturer: "Syntaur",
                            friendly_name: &cfg.friendly_name,
                            suggested_area: &cfg.suggested_area,
                            feature_flags: p::FEATURE_RAW_ADVERTISEMENTS
                                | p::FEATURE_STATE_AND_MODE,
                        };
                        let resp = p::build_device_info_response(&info);
                        write_message(&mut write_half, p::MSG_DEVICE_INFO_RESPONSE, &resp)
                            .await
                            .map_err(|e| format!("devinfo write: {e}"))?;
                    }

                    p::MSG_LIST_ENTITIES_REQUEST => {
                        // We expose no entities (no sensors / lights / etc.) — just
                        // close out the listing.
                        write_message(
                            &mut write_half,
                            p::MSG_LIST_ENTITIES_DONE_RESPONSE,
                            &[],
                        )
                        .await
                        .map_err(|e| format!("list-entities-done write: {e}"))?;
                    }

                    p::MSG_PING_REQUEST => {
                        write_message(&mut write_half, p::MSG_PING_RESPONSE, &[])
                            .await
                            .map_err(|e| format!("ping write: {e}"))?;
                    }

                    p::MSG_GET_TIME_REQUEST => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs() as u32)
                            .unwrap_or(0);
                        let mut e = ProtoEncoder::new();
                        e.encode_uint32(1, now); // GetTimeResponse.epoch_seconds = 1
                        let body = e.finish();
                        write_message(&mut write_half, p::MSG_GET_TIME_RESPONSE, &body)
                            .await
                            .map_err(|e| format!("get-time write: {e}"))?;
                    }

                    p::MSG_DISCONNECT_REQUEST => {
                        write_message(&mut write_half, p::MSG_DISCONNECT_RESPONSE, &[])
                            .await
                            .map_err(|e| format!("disc write: {e}"))?;
                        let _ = write_half.shutdown().await;
                        return Ok(());
                    }

                    p::MSG_SUBSCRIBE_BLUETOOTH_LE_ADVERTISEMENTS_REQUEST => {
                        let flags = p::parse_subscribe_ble_request(&msg.payload);
                        if flags & 1 == 0 {
                            log::warn!(
                                "[server] client requested legacy parsed-advert mode (flags=0); we only emit raw mode"
                            );
                        }
                        subscribed_to_adverts = true;
                        log::info!("[server] client subscribed to BLE adverts (flags={flags})");

                        // Always echo current scanner state on subscribe — clients
                        // re-subscribing after a reconnect expect a fresh status.
                        let body = p::build_scanner_state_response(
                            p::SCANNER_STATE_RUNNING,
                            p::SCANNER_MODE_ACTIVE,
                        );
                        write_message(
                            &mut write_half,
                            p::MSG_BLUETOOTH_SCANNER_STATE_RESPONSE,
                            &body,
                        )
                        .await
                        .map_err(|e| format!("scanner-state write: {e}"))?;
                    }

                    p::MSG_BLUETOOTH_SCANNER_SET_MODE_REQUEST => {
                        // Echo back current mode; we don't actually change scan mode
                        // (btleplug + BlueZ keeps active scan running).
                        let body = p::build_scanner_state_response(
                            p::SCANNER_STATE_RUNNING,
                            p::SCANNER_MODE_ACTIVE,
                        );
                        write_message(
                            &mut write_half,
                            p::MSG_BLUETOOTH_SCANNER_STATE_RESPONSE,
                            &body,
                        )
                        .await
                        .map_err(|e| format!("scanner-state write: {e}"))?;
                    }

                    // Quietly accept and ignore — these are common HA subscriptions
                    // we don't have anything to send for.
                    p::MSG_SUBSCRIBE_STATES_REQUEST
                    | p::MSG_SUBSCRIBE_LOGS_REQUEST
                    | p::MSG_SUBSCRIBE_HOMEASSISTANT_SERVICES_REQUEST
                    | p::MSG_SUBSCRIBE_HOME_ASSISTANT_STATES_REQUEST => {
                        log::debug!("[server] subscribed to passive stream type={}", msg.msg_type);
                    }

                    other => {
                        log::debug!(
                            "[server] unhandled message type {} ({} bytes) — ignoring",
                            other,
                            msg.payload.len()
                        );
                    }
                }
            }

            advert = rx.recv(), if subscribed_to_adverts => {
                match advert {
                    Ok(ev) => {
                        // Forward as a single-element raw advert response. We could
                        // batch, but per-frame latency matters more than CPU here
                        // (~30 adverts/sec total across all devices).
                        let body = p::build_raw_advertisements_response(std::slice::from_ref::<RawAdvert>(&ev.advert));
                        if let Err(e) = write_message(
                            &mut write_half,
                            p::MSG_BLUETOOTH_LE_RAW_ADVERTISEMENTS_RESPONSE,
                            &body,
                        )
                        .await
                        {
                            return Err(format!("advert write: {e}"));
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Slow subscriber. We dropped n adverts. Not fatal — Bermuda
                        // tolerates gaps.
                        log::warn!("[server] subscriber lagged, dropped {n} adverts");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err("scanner channel closed".into());
                    }
                }
            }
        }
    }
}
