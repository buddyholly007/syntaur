//! matter-discover — standalone Rust mDNS for _matterc._udp.
//!
//! Uses the `syntaur_matter_ip::mdns` resolver (mdns-sd crate) to find
//! all Matter devices currently advertising commissioning mode, with
//! full detail (address, port, discriminator, CM, vendor/product).
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mdns = ServiceDaemon::new().map_err(|e| format!("mdns daemon: {e}"))?;
    let receiver = mdns
        .browse("_matterc._udp.local.")
        .map_err(|e| format!("mdns browse: {e}"))?;
    println!("browsing _matterc._udp.local. (8s)...");

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut seen: std::collections::HashMap<String, (u16, Vec<IpAddr>, String)> =
        std::collections::HashMap::new();
    while let Some(remaining) =
        deadline.checked_duration_since(std::time::Instant::now())
    {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let name = info.get_fullname().to_string();
                let port = info.get_port();
                let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                let cm = info.get_property_val_str("CM").unwrap_or("?").to_string();
                let d = info.get_property_val_str("D").unwrap_or("?").to_string();
                let vp = info.get_property_val_str("VP").unwrap_or("?").to_string();
                let dn = info.get_property_val_str("DN").unwrap_or("?").to_string();
                let summary = format!(
                    "CM={cm} D={d} VP={vp} DN={dn}"
                );
                seen.insert(name, (port, addrs, summary));
            }
            _ => continue,
        }
    }
    mdns.shutdown().map_err(|e| format!("shutdown: {e}"))?;

    if seen.is_empty() {
        println!("(no _matterc._udp devices advertising)");
    } else {
        for (name, (port, addrs, summary)) in seen {
            let short = name.split("._matterc").next().unwrap_or(&name);
            println!("  {short} port={port} addrs={addrs:?} {summary}");
        }
    }
    Ok(())
}
