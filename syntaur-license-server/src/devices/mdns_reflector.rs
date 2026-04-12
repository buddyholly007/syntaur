//! Embedded mDNS reflector for cross-VLAN device discovery.
//!
//! Only activates when multiple network interfaces are detected (VLAN setup).
//! Bounces mDNS packets between interfaces so devices on different subnets
//! can discover each other. Based on the standalone rust mdns-reflector.
//!
//! Enable with: MDNS_REFLECT=true (or auto-detected if >1 non-loopback interface)

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{debug, info, warn};

const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

/// Stats for the mDNS reflector.
#[derive(Default)]
pub struct ReflectorStats {
    pub packets_received: AtomicU64,
    pub packets_reflected: AtomicU64,
    pub packets_filtered: AtomicU64,
    pub duplicates_suppressed: AtomicU64,
}

/// Start the mDNS reflector in the background.
/// Returns None if not needed (single interface) or if disabled.
pub fn start_if_needed() -> Option<Arc<ReflectorStats>> {
    let enabled = std::env::var("MDNS_REFLECT")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Auto-detect: check if there are multiple non-loopback interfaces
    let interfaces = list_interfaces();
    let multi_interface = interfaces.len() > 1;

    if !enabled && !multi_interface {
        debug!("[mdns-reflect] single interface, reflector not needed");
        return None;
    }

    if interfaces.is_empty() {
        warn!("[mdns-reflect] no usable interfaces found");
        return None;
    }

    let stats = Arc::new(ReflectorStats::default());
    let stats_clone = stats.clone();

    warn!(
        "[mdns-reflect] IMPORTANT: If your router has a built-in mDNS relay/reflector \
         (UniFi 'mDNS', Mikrotik 'IP > DNS > mDNS Repeater', etc.), disable it before \
         using Syntaur's reflector. Running two reflectors causes packet storms, duplicate \
         devices, and discovery failures. Set MDNS_REFLECT=false to disable Syntaur's."
    );
    info!(
        "[mdns-reflect] starting on {} interface(s): {}",
        interfaces.len(),
        interfaces
            .iter()
            .map(|(name, ip)| format!("{}({})", name, ip))
            .collect::<Vec<_>>()
            .join(", ")
    );

    std::thread::spawn(move || {
        if let Err(e) = run_reflector(&interfaces, &stats_clone) {
            warn!("[mdns-reflect] reflector stopped: {}", e);
        }
    });

    Some(stats)
}

/// List non-loopback network interfaces with their IPv4 addresses.
fn list_interfaces() -> Vec<(String, Ipv4Addr)> {
    let mut interfaces = Vec::new();

    // Read from /proc/net/if_inet6 and /proc/net/fib_trie for interface enumeration
    // Simpler: parse `ip -4 addr show` output
    if let Ok(output) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let name = parts[1].trim_end_matches(':');
                if name == "lo" {
                    continue;
                }
                if let Some(ip_str) = parts[3].split('/').next() {
                    if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                        if !ip.is_loopback() {
                            interfaces.push((name.to_string(), ip));
                        }
                    }
                }
            }
        }
    }

    interfaces
}

/// Run the mDNS reflector loop (blocking, runs in its own thread).
fn run_reflector(
    interfaces: &[(String, Ipv4Addr)],
    stats: &ReflectorStats,
) -> Result<(), String> {
    let mdns_addr = SocketAddrV4::new(MDNS_ADDR, MDNS_PORT);

    // Create one socket per interface, bind to its IP
    let mut sockets: Vec<(String, UdpSocket)> = Vec::new();

    for (name, ip) in interfaces {
        let socket = UdpSocket::bind(SocketAddrV4::new(*ip, 0))
            .map_err(|e| format!("bind {}: {}", name, e))?;

        // Join multicast group on this interface
        socket
            .join_multicast_v4(&MDNS_ADDR, ip)
            .map_err(|e| format!("join multicast on {}: {}", name, e))?;

        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok();
        socket.set_nonblocking(false).ok();

        sockets.push((name.clone(), socket));
    }

    // Also create a listening socket on 0.0.0.0:5353 for receiving multicasts
    let listener = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT))
        .map_err(|e| format!("bind listener: {}", e))?;
    for (_, ip) in interfaces {
        listener.join_multicast_v4(&MDNS_ADDR, ip).ok();
    }
    listener
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();

    // Dedup cache: hash → timestamp
    let mut dedup: HashMap<u64, Instant> = HashMap::new();
    let mut buf = [0u8; 4096];

    info!("[mdns-reflect] reflector running");

    loop {
        // Receive from the shared listener
        match listener.recv_from(&mut buf) {
            Ok((len, src)) => {
                stats.packets_received.fetch_add(1, Ordering::Relaxed);

                let data = &buf[..len];

                // Simple hash for dedup
                let hash = simple_hash(data);
                let now = Instant::now();
                if let Some(prev) = dedup.get(&hash) {
                    if now.duration_since(*prev) < Duration::from_secs(1) {
                        stats.duplicates_suppressed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }
                dedup.insert(hash, now);

                // Evict old dedup entries
                if dedup.len() > 2048 {
                    dedup.retain(|_, t| now.duration_since(*t) < Duration::from_secs(2));
                }

                // Filter: only reflect smart home service types
                let data_str = String::from_utf8_lossy(data).to_lowercase();
                let dominated = ["matter", "homekit", "hap", "esphome", "shelly",
                                  "wled", "tasmota", "hue", "wiz", "airplay",
                                  "chromecast", "spotify", "mqtt"];
                let dominated_match = dominated.iter().any(|s| data_str.contains(s));
                if !dominated_match && len < 100 {
                    // Skip tiny non-matching packets (likely queries for unrelated services)
                    stats.packets_filtered.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // Reflect to multicast on all interfaces
                let src_ip = match src.ip() {
                    std::net::IpAddr::V4(ip) => ip,
                    _ => continue,
                };

                for (name, socket) in &sockets {
                    // Don't reflect back to the source interface
                    if interfaces.iter().any(|(n, ip)| n == name && *ip == src_ip) {
                        continue;
                    }
                    match socket.send_to(data, mdns_addr) {
                        Ok(_) => {
                            stats.packets_reflected.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            debug!("[mdns-reflect] send to {}: {}", name, e);
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Timeout, normal
            }
            Err(e) => {
                debug!("[mdns-reflect] recv: {}", e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn simple_hash(data: &[u8]) -> u64 {
    // FNV-1a hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
