//! Minimal hand-rolled DNS-SD responder for `_esphomelib._tcp.local.`.
//!
//! Why not `mdns-sd`? On HAOS Add-on hosts (host_network: true) with multiple
//! veth/docker/hassio bridges, mdns-sd 0.13 ships **empty 12-byte DNS frames**
//! on the LAN interface — the daemon's send-side buffer never gets the
//! announcement records even though `register()` succeeds and the records
//! show up on the local loopback. We hit this on 2026-04-28 (HA mini PC,
//! HAOS 6.12.67) and `tcpdump` confirmed it: every outbound packet from
//! 192.168.1.3:5353 was a bare DNS header.
//!
//! Since the protocol is small and stable (RFC 6762 + 6763) and we only need
//! the announce path (not querying / browsing), it's faster to write 120 lines
//! of well-tested DNS-SD encoder than to keep fighting the library. We send
//! gratuitous unsolicited responses every 30s on every LAN interface.
//!
//! Both HA and Syntaur are passive listeners — they cache our records on the
//! first announcement they see and use them for service discovery. They never
//! need us to answer a probe.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

/// Same exclusion list as before — these interfaces face into containers,
/// not out to the LAN.
const BRIDGE_PREFIXES: &[&str] = &[
    "veth", "docker", "br-", "hassio", "cni", "flannel", "weave",
];

fn is_bridge_iface(name: &str) -> bool {
    BRIDGE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Handle keeping the responder thread alive. Drop to stop announcing.
pub struct AnnounceHandle {
    _stop_tx: std::sync::mpsc::Sender<()>,
    _join: std::thread::JoinHandle<()>,
}

pub fn announce(
    name: &str,
    port: u16,
    mac_address: &str,
    version: &str,
) -> Result<AnnounceHandle, Box<dyn std::error::Error + Send + Sync>> {
    let lan_ips: Vec<Ipv4Addr> = if_addrs::get_if_addrs()?
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(a) if !a.is_loopback() && !is_bridge_iface(&i.name) => {
                log::info!("[mdns] LAN interface: {} ({})", i.name, a);
                Some(a)
            }
            IpAddr::V4(a) if a.is_loopback() => None,
            IpAddr::V4(_) => {
                log::info!("[mdns] skipped bridge interface: {}", i.name);
                None
            }
            _ => None,
        })
        .collect();

    if lan_ips.is_empty() {
        return Err("no LAN-facing IPv4 interfaces found".into());
    }

    let raw = hostname::get()?
        .into_string()
        .map_err(|_| "hostname not utf-8")?;
    let label = raw.split('.').next().unwrap_or(&raw);
    let host_local = format!("{}.local.", label);

    // TXT records — same set HA's ESPHome integration looks for.
    let mut props: HashMap<String, String> = HashMap::new();
    let mac_compact: String = mac_address
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    props.insert("mac".into(), mac_compact.clone());
    props.insert("version".into(), version.into());
    props.insert("platform".into(), "linux".into());
    props.insert("board".into(), "syntaur-ble-shim".into());
    props.insert("network".into(), "ethernet".into());
    props.insert("friendly_name".into(), name.into());
    props.insert("project_name".into(), "syntaur.ble-shim".into());
    props.insert("project_version".into(), version.into());
    props.insert("bluetooth_mac".into(), mac_compact);

    let svc_type = "_esphomelib._tcp.local.".to_string();
    let instance = format!("{}.{}", name, svc_type);

    let payload = build_announce(&svc_type, &instance, &host_local, port, &props, &lan_ips)?;

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    // Spawn each LAN interface's send loop on its own thread. Plain blocking
    // sockets — way simpler than fighting tokio multicast joins.
    let mut sockets = Vec::new();
    for ip in &lan_ips {
        match make_send_socket(*ip) {
            Ok(s) => sockets.push((*ip, s)),
            Err(e) => log::warn!("[mdns] cannot bind send socket on {ip}: {e}"),
        }
    }
    if sockets.is_empty() {
        return Err("no usable multicast sockets".into());
    }

    let join = std::thread::Builder::new()
        .name("mdns-announce".into())
        .spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            // Send three quick announces (RFC 6762 §8.3) then every 30s.
            for _ in 0..3 {
                blast(&sockets, &payload);
                std::thread::sleep(Duration::from_millis(1000));
            }
            loop {
                match stop_rx.recv_timeout(Duration::from_secs(30)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                        log::info!("[mdns] stopping announcer");
                        return;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        blast(&sockets, &payload);
                    }
                }
            }
        })?;

    log::info!(
        "[mdns] advertising {} on {} addr(s), port {}",
        instance.trim_end_matches('.'),
        lan_ips.len(),
        port
    );

    Ok(AnnounceHandle {
        _stop_tx: stop_tx,
        _join: join,
    })
}

fn make_send_socket(local: Ipv4Addr) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // SO_REUSEADDR + SO_REUSEPORT lets us coexist with whatever other mDNS
    // responder may already own 5353 on this host (HAOS frequently has 2+).
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    // RFC 6762 §11 mandates mDNS responses use UDP source port 5353. Strict
    // clients (notably Bonjour) drop responses from ephemeral ports. Try 5353
    // first (with SO_REUSEPORT so we coexist with whatever HAOS already has
    // bound); fall back to ephemeral if even that fails.
    let primary = SocketAddrV4::new(local, MDNS_PORT);
    if let Err(e) = socket.bind(&socket2::SockAddr::from(primary)) {
        log::warn!(
            "[mdns] could not bind {local}:{MDNS_PORT} ({e}); falling back to ephemeral"
        );
        socket.bind(&socket2::SockAddr::from(SocketAddrV4::new(local, 0)))?;
    }
    socket.set_multicast_if_v4(&local)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_ttl_v4(255)?;
    Ok(socket.into())
}

fn blast(sockets: &[(Ipv4Addr, UdpSocket)], payload: &[u8]) {
    let dest = SocketAddrV4::new(MDNS_GROUP, MDNS_PORT);
    for (ip, sock) in sockets {
        match sock.send_to(payload, dest) {
            Ok(n) => log::debug!("[mdns] announced {} bytes via {ip}", n),
            Err(e) => log::warn!("[mdns] send via {ip} failed: {e}"),
        }
    }
}

// ── DNS-SD packet construction ──────────────────────────────────────────────
//
// We build a single response packet with:
//   PTR  _esphomelib._tcp.local. → instance
//   SRV  instance → host:port
//   TXT  instance → key/value pairs
//   A    host → each LAN IPv4 we can advertise
//
// Header flags = 0x8400 (response + AA = authoritative answer).
// All TTLs = 4500 (75 min) for service records, 120 for A.

const TTL_SERVICE: u32 = 4500;
const TTL_HOST: u32 = 120;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;

const CLASS_IN: u16 = 1;
const CLASS_FLUSH: u16 = 0x8001; // CLASS_IN | cache-flush bit

fn build_announce(
    service_type: &str,
    instance: &str,
    host: &str,
    port: u16,
    txt: &HashMap<String, String>,
    addrs: &[Ipv4Addr],
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = Vec::with_capacity(512);

    // Header: id=0, flags=0x8400 (response, authoritative), counts filled in below.
    buf.extend_from_slice(&[0, 0]); // id
    buf.extend_from_slice(&[0x84, 0x00]); // flags

    let answers = (1 + 1 + 1 + addrs.len()) as u16; // PTR + SRV + TXT + N×A
    buf.extend_from_slice(&[0, 0]); // QDCOUNT
    buf.extend_from_slice(&answers.to_be_bytes()); // ANCOUNT
    buf.extend_from_slice(&[0, 0]); // NSCOUNT
    buf.extend_from_slice(&[0, 0]); // ARCOUNT

    // 1. PTR  service_type → instance  (shared, not cache-flush)
    write_name(&mut buf, service_type);
    buf.extend_from_slice(&TYPE_PTR.to_be_bytes());
    buf.extend_from_slice(&CLASS_IN.to_be_bytes());
    buf.extend_from_slice(&TTL_SERVICE.to_be_bytes());
    let ptr_rdata = encode_name(instance);
    buf.extend_from_slice(&(ptr_rdata.len() as u16).to_be_bytes());
    buf.extend_from_slice(&ptr_rdata);

    // 2. SRV  instance → host:port  (cache-flush)
    write_name(&mut buf, instance);
    buf.extend_from_slice(&TYPE_SRV.to_be_bytes());
    buf.extend_from_slice(&CLASS_FLUSH.to_be_bytes());
    buf.extend_from_slice(&TTL_HOST.to_be_bytes());
    let mut srv_rdata = Vec::with_capacity(8 + host.len());
    srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // priority
    srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // weight
    srv_rdata.extend_from_slice(&port.to_be_bytes());
    srv_rdata.extend_from_slice(&encode_name(host));
    buf.extend_from_slice(&(srv_rdata.len() as u16).to_be_bytes());
    buf.extend_from_slice(&srv_rdata);

    // 3. TXT  instance → key=value pairs  (cache-flush)
    write_name(&mut buf, instance);
    buf.extend_from_slice(&TYPE_TXT.to_be_bytes());
    buf.extend_from_slice(&CLASS_FLUSH.to_be_bytes());
    buf.extend_from_slice(&TTL_SERVICE.to_be_bytes());
    let txt_rdata = encode_txt(txt);
    buf.extend_from_slice(&(txt_rdata.len() as u16).to_be_bytes());
    buf.extend_from_slice(&txt_rdata);

    // 4. A records  host → ipv4 (one per LAN IP)  (cache-flush)
    for ip in addrs {
        write_name(&mut buf, host);
        buf.extend_from_slice(&TYPE_A.to_be_bytes());
        buf.extend_from_slice(&CLASS_FLUSH.to_be_bytes());
        buf.extend_from_slice(&TTL_HOST.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&ip.octets());
    }

    Ok(buf)
}

/// Append a DNS name (no compression) to buf.
fn write_name(buf: &mut Vec<u8>, name: &str) {
    buf.extend_from_slice(&encode_name(name));
}

/// Encode a DNS name as a sequence of length-prefixed labels terminated by 0.
/// `_esphomelib._tcp.local.` → 0B "_esphomelib" 04 "_tcp" 05 "local" 00
fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 2);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
    out
}

fn encode_txt(props: &HashMap<String, String>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort(); // stable ordering for cache consistency
    for k in keys {
        let v = &props[k];
        let entry = format!("{k}={v}");
        let bytes = entry.as_bytes();
        let len = bytes.len().min(255);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    if out.is_empty() {
        // Per RFC 6763 §6.1, empty TXT is a single zero-length string.
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_name_basic() {
        let bytes = encode_name("_esphomelib._tcp.local.");
        // 11 + "_esphomelib" + 4 + "_tcp" + 5 + "local" + 0 = 11+11+1+4+1+5+1 = 34? let's compute:
        //   1 (len) + 11 chars + 1 (len) + 4 chars + 1 (len) + 5 chars + 1 (terminator) = 24
        assert_eq!(bytes.len(), 24);
        assert_eq!(bytes[0], 11);
        assert_eq!(&bytes[1..12], b"_esphomelib");
        assert_eq!(bytes[12], 4);
        assert_eq!(bytes[23], 0);
    }

    #[test]
    fn announce_packet_is_nontrivial() {
        let mut props = HashMap::new();
        props.insert("k1".into(), "v1".into());
        let pkt = build_announce(
            "_esphomelib._tcp.local.",
            "shim._esphomelib._tcp.local.",
            "host.local.",
            6053,
            &props,
            &[Ipv4Addr::new(192, 168, 1, 3)],
        )
        .unwrap();
        // header (12) + much more than that. Real packets are 100-300 bytes.
        assert!(pkt.len() > 80, "got {} bytes", pkt.len());
        // Header: response flag bit set
        assert_eq!(pkt[2] & 0x80, 0x80, "QR bit must be set on response");
    }

    #[test]
    fn txt_encoding_is_sorted() {
        let mut props = HashMap::new();
        props.insert("zzz".into(), "1".into());
        props.insert("aaa".into(), "2".into());
        let bytes = encode_txt(&props);
        // First label after the length byte should start with "aaa="
        assert_eq!(&bytes[1..5], b"aaa=");
    }
}
