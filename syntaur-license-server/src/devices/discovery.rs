//! mDNS device discovery — scans the local network for smart home devices.
//!
//! Detects:
//! - Shelly (_http._tcp with "shelly" in name, or _shelly._tcp)
//! - WLED (_http._tcp with "wled" in name, or _wled._tcp)
//! - ESPHome (_esphomelib._tcp)
//! - Tasmota (_http._tcp with "tasmota" in name)
//! - Matter (_matter._tcp, _matterc._udp for commissioning)
//! - Hue Bridge (_hue._tcp)
//! - Zigbee2MQTT (MQTT broker at _mqtt._tcp)

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use super::{DeviceCapability, DevicePlatform, DeviceProtocol};

/// A device found during network scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub platform: DevicePlatform,
    pub protocol: DeviceProtocol,
    pub service_type: String,
    pub suggested_capabilities: Vec<DeviceCapability>,
    /// Extra info from the mDNS TXT records or HTTP probe.
    pub info: HashMap<String, String>,
}

/// Scan the local network for smart home devices.
/// Sends mDNS queries and listens for responses, then probes HTTP endpoints.
pub async fn scan_network(timeout_secs: u64) -> Vec<DiscoveredDevice> {
    let mut found = Vec::new();

    // Phase 1: mDNS scan
    info!("[discovery] starting mDNS scan ({}s timeout)", timeout_secs);
    let mdns_results = mdns_scan(Duration::from_secs(timeout_secs.min(10))).await;
    found.extend(mdns_results);

    // Phase 2: HTTP probe known device ports on discovered IPs
    // Also probe common smart home ports on the local subnet
    let http_results = http_probe(&found, Duration::from_secs(timeout_secs.min(5))).await;
    found.extend(http_results);

    // Deduplicate by IP
    found.sort_by(|a, b| a.ip.cmp(&b.ip));
    found.dedup_by(|a, b| a.ip == b.ip && a.port == b.port);

    info!("[discovery] found {} device(s)", found.len());
    found
}

/// Send mDNS queries for known smart home service types.
async fn mdns_scan(timeout: Duration) -> Vec<DiscoveredDevice> {
    let services = [
        ("_http._tcp.local", None),
        ("_shelly._tcp.local", Some(DevicePlatform::Shelly)),
        ("_wled._tcp.local", Some(DevicePlatform::Wled)),
        ("_esphomelib._tcp.local", Some(DevicePlatform::Esphome)),
        ("_hue._tcp.local", Some(DevicePlatform::HueBridge)),
        ("_matter._tcp.local", Some(DevicePlatform::Matter)),
        ("_matterc._udp.local", Some(DevicePlatform::Matter)),
        ("_mqtt._tcp.local", None), // MQTT broker (Zigbee2MQTT, Z-Wave JS)
    ];

    // mDNS multicast address
    let mdns_addr: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            warn!("[discovery] failed to bind UDP socket: {}", e);
            return Vec::new();
        }
    };
    socket.set_read_timeout(Some(Duration::from_millis(500))).ok();
    socket.set_nonblocking(false).ok();

    // Send queries for each service type
    for (service, _) in &services {
        if let Some(query) = build_mdns_query(service) {
            socket.send_to(&query, mdns_addr).ok();
        }
    }

    // Collect responses
    let mut found = Vec::new();
    let start = Instant::now();
    let mut buf = [0u8; 4096];

    while start.elapsed() < timeout {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                let ip = src.ip().to_string();
                if let Some(device) = parse_mdns_response(&buf[..len], &ip, &services) {
                    debug!("[discovery] mDNS: {} at {} ({})", device.name, device.ip, device.platform);
                    found.push(device);
                }
            }
            Err(_) => {
                // Timeout or error, keep going until deadline
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    found
}

/// Build a simple mDNS query packet for a service type.
fn build_mdns_query(service: &str) -> Option<Vec<u8>> {
    let mut packet = Vec::new();

    // Header: ID=0, Flags=0 (standard query), QDCOUNT=1
    packet.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);

    // Question: service name in DNS label format
    for label in service.split('.') {
        if label.is_empty() {
            continue;
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0); // null terminator

    // Type: PTR (12), Class: IN (1)
    packet.extend_from_slice(&[0, 12, 0, 1]);

    Some(packet)
}

/// Parse an mDNS response and extract device info.
fn parse_mdns_response(
    data: &[u8],
    ip: &str,
    services: &[(&str, Option<DevicePlatform>)],
) -> Option<DiscoveredDevice> {
    // Simple heuristic: look for known strings in the response
    let response_str = String::from_utf8_lossy(data).to_lowercase();

    let (platform, service_type) = if response_str.contains("shelly") {
        (DevicePlatform::Shelly, "_shelly._tcp")
    } else if response_str.contains("wled") {
        (DevicePlatform::Wled, "_wled._tcp")
    } else if response_str.contains("esphome") || response_str.contains("_esphomelib") {
        (DevicePlatform::Esphome, "_esphomelib._tcp")
    } else if response_str.contains("tasmota") {
        (DevicePlatform::Tasmota, "_http._tcp")
    } else if response_str.contains("hue") {
        (DevicePlatform::HueBridge, "_hue._tcp")
    } else if response_str.contains("matter") {
        (DevicePlatform::Matter, "_matter._tcp")
    } else if response_str.contains("mqtt") {
        return Some(DiscoveredDevice {
            name: format!("MQTT Broker at {}", ip),
            ip: ip.to_string(),
            port: 1883,
            platform: DevicePlatform::Generic,
            protocol: DeviceProtocol::Mqtt,
            service_type: "_mqtt._tcp".into(),
            suggested_capabilities: vec![],
            info: [("type".into(), "mqtt_broker".into())].into(),
        });
    } else {
        return None;
    };

    let protocol = match platform {
        DevicePlatform::Matter => DeviceProtocol::Matter,
        _ => DeviceProtocol::Http,
    };

    let capabilities = match platform {
        DevicePlatform::Shelly => vec![DeviceCapability::OnOff, DeviceCapability::Power, DeviceCapability::Energy],
        DevicePlatform::Wled => vec![DeviceCapability::OnOff, DeviceCapability::Brightness, DeviceCapability::Color],
        DevicePlatform::Esphome => vec![DeviceCapability::OnOff, DeviceCapability::Brightness],
        DevicePlatform::HueBridge => vec![DeviceCapability::OnOff, DeviceCapability::Brightness, DeviceCapability::ColorTemp, DeviceCapability::Color],
        DevicePlatform::Matter => vec![DeviceCapability::OnOff, DeviceCapability::Brightness],
        _ => vec![DeviceCapability::OnOff],
    };

    // Try to extract a name from the response
    let name = extract_name_from_mdns(&response_str, &platform, ip);

    Some(DiscoveredDevice {
        name,
        ip: ip.to_string(),
        port: match platform {
            DevicePlatform::Wled => 80,
            DevicePlatform::Shelly => 80,
            DevicePlatform::Esphome => 80,
            DevicePlatform::HueBridge => 80,
            DevicePlatform::Matter => 5540,
            _ => 80,
        },
        platform,
        protocol,
        service_type: service_type.into(),
        suggested_capabilities: capabilities,
        info: HashMap::new(),
    })
}

fn extract_name_from_mdns(response: &str, platform: &DevicePlatform, ip: &str) -> String {
    // Try to find a human-readable name in the mDNS payload
    // This is a heuristic — proper DNS name parsing would be more robust
    let fallback = format!("{} at {}", platform, ip);
    fallback
}

/// Probe known HTTP endpoints to identify devices that didn't respond to mDNS.
async fn http_probe(
    already_found: &[DiscoveredDevice],
    timeout: Duration,
) -> Vec<DiscoveredDevice> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    let mut found = Vec::new();
    let already_ips: Vec<&str> = already_found.iter().map(|d| d.ip.as_str()).collect();

    // Probe specific known IPs that responded to mDNS for more device info
    for device in already_found {
        if device.platform == DevicePlatform::Shelly {
            // Probe Shelly for device info
            if let Ok(resp) = client.get(&format!("http://{}/shelly", device.ip)).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    debug!("[discovery] Shelly probe {}: {:?}", device.ip, json.get("type"));
                }
            }
        }
    }

    found
}

/// Detect MQTT brokers and Zigbee2MQTT/Z-Wave JS instances.
pub async fn detect_mqtt_services(broker_url: &str) -> Vec<DiscoveredDevice> {
    let mut found = Vec::new();

    // Try connecting to the MQTT broker and checking for Zigbee2MQTT bridge state
    let (client, mut eventloop) = match rumqttc::MqttOptions::new("syntaur-discovery", broker_url, 1883)
        .try_into()
        .ok()
        .map(|opts: rumqttc::MqttOptions| rumqttc::AsyncClient::new(opts, 32))
    {
        Some((c, e)) => (c, e),
        None => return found,
    };

    // Subscribe to Zigbee2MQTT bridge info
    let _ = client.subscribe("zigbee2mqtt/bridge/devices", rumqttc::QoS::AtMostOnce).await;
    let _ = client.subscribe("zwave-js-ui/#", rumqttc::QoS::AtMostOnce).await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(1), eventloop.poll()).await {
            Ok(Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(msg)))) => {
                if msg.topic.starts_with("zigbee2mqtt/bridge/devices") {
                    if let Ok(devices) = serde_json::from_slice::<Vec<serde_json::Value>>(&msg.payload) {
                        for dev in devices {
                            let friendly = dev.get("friendly_name").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let ieee = dev.get("ieee_address").and_then(|v| v.as_str()).unwrap_or("");
                            let dev_type = dev.get("type").and_then(|v| v.as_str()).unwrap_or("");

                            if dev_type == "Coordinator" {
                                continue;
                            }

                            let caps = if let Some(def) = dev.get("definition") {
                                let mut c = vec![DeviceCapability::OnOff];
                                let desc = def.get("description").and_then(|v| v.as_str()).unwrap_or("");
                                if desc.contains("light") || desc.contains("bulb") || desc.contains("dimm") {
                                    c.push(DeviceCapability::Brightness);
                                }
                                if desc.contains("color") {
                                    c.push(DeviceCapability::Color);
                                }
                                if desc.contains("temperature") || desc.contains("sensor") {
                                    c.push(DeviceCapability::Temperature);
                                }
                                c
                            } else {
                                vec![DeviceCapability::OnOff]
                            };

                            found.push(DiscoveredDevice {
                                name: friendly.to_string(),
                                ip: broker_url.to_string(),
                                port: 1883,
                                platform: DevicePlatform::Zigbee2mqtt,
                                protocol: DeviceProtocol::Mqtt,
                                service_type: "zigbee2mqtt".into(),
                                suggested_capabilities: caps,
                                info: [
                                    ("ieee".into(), ieee.to_string()),
                                    ("topic".into(), format!("zigbee2mqtt/{}", friendly)),
                                ].into(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let _ = client.disconnect().await;
    info!("[discovery] found {} Zigbee/Z-Wave device(s) via MQTT", found.len());
    found
}
