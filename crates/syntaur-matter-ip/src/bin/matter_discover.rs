//! matter-discover — standalone Rust mDNS for Matter services.
//!
//! Browses both `_matterc._udp` (commissionable) and `_matter._tcp`
//! (operational). Args:
//!   matter-discover                     # commissionable only (default)
//!   matter-discover operational         # operational only
//!   matter-discover both                # both
//!   matter-discover both --node <hex>   # both, only print entries whose
//!                                       # operational name ends with -<hex>
//!   matter-discover --timeout-secs N    # browse window (default 8)
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Commissionable,
    Operational,
    Both,
}

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = Mode::Commissionable;
    let mut node_filter: Option<String> = None;
    let mut timeout_secs: u64 = 8;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "commissionable" | "_matterc" => { mode = Mode::Commissionable; i += 1; }
            "operational" | "_matter"     => { mode = Mode::Operational;    i += 1; }
            "both"                         => { mode = Mode::Both;           i += 1; }
            "--node" => {
                node_filter = Some(argv.get(i+1).cloned().ok_or("--node needs a value (16 hex chars)")?.to_uppercase());
                i += 2;
            }
            "--timeout-secs" => {
                timeout_secs = argv.get(i+1).ok_or("--timeout-secs needs a value")?
                    .parse().map_err(|e| format!("--timeout-secs: {e}"))?;
                i += 2;
            }
            "-h" | "--help" => {
                eprintln!("usage: matter-discover [commissionable|operational|both] [--node HEX16] [--timeout-secs N]");
                return Ok(());
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }

    let mdns = ServiceDaemon::new().map_err(|e| format!("mdns daemon: {e}"))?;
    let mut receivers = Vec::new();
    if matches!(mode, Mode::Commissionable | Mode::Both) {
        receivers.push(("_matterc._udp", mdns.browse("_matterc._udp.local.")
            .map_err(|e| format!("browse _matterc._udp: {e}"))?));
    }
    if matches!(mode, Mode::Operational | Mode::Both) {
        receivers.push(("_matter._tcp", mdns.browse("_matter._tcp.local.")
            .map_err(|e| format!("browse _matter._tcp: {e}"))?));
    }
    println!("browsing {} ({}s)...",
        receivers.iter().map(|(n,_)| *n).collect::<Vec<_>>().join(" + "),
        timeout_secs);

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut seen: std::collections::BTreeMap<String, (u16, Vec<IpAddr>, String, &'static str)> =
        std::collections::BTreeMap::new();

    'outer: loop {
        let now = std::time::Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else { break; };
        // Poll each receiver round-robin with a small slice so we don't
        // starve one when the other is busy.
        let slice = remaining / (receivers.len() as u32).max(1);
        let slice = slice.max(Duration::from_millis(50));
        let mut any_event = false;
        for (svc, rx) in receivers.iter() {
            match rx.recv_timeout(slice) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    any_event = true;
                    let name = info.get_fullname().to_string();
                    if let Some(filter) = &node_filter {
                        let upper = name.to_uppercase();
                        if !upper.contains(&format!("-{filter}.")) && !upper.contains(&format!("-{filter}_")) {
                            continue;
                        }
                    }
                    let port = info.get_port();
                    let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    let summary = if *svc == "_matterc._udp" {
                        let cm = info.get_property_val_str("CM").unwrap_or("?");
                        let d  = info.get_property_val_str("D").unwrap_or("?");
                        let vp = info.get_property_val_str("VP").unwrap_or("?");
                        let dn = info.get_property_val_str("DN").unwrap_or("?");
                        format!("CM={cm} D={d} VP={vp} DN={dn}")
                    } else {
                        // operational: SII/SAI/T are common txt records
                        let sii = info.get_property_val_str("SII").unwrap_or("?");
                        let sai = info.get_property_val_str("SAI").unwrap_or("?");
                        let t   = info.get_property_val_str("T").unwrap_or("?");
                        format!("SII={sii} SAI={sai} T={t}")
                    };
                    seen.insert(name, (port, addrs, summary, *svc));
                }
                Err(_) => continue,
                _ => continue,
            }
        }
        if !any_event && receivers.iter().all(|(_,r)| r.is_empty()) {
            // Nothing to do this slice; loop until deadline.
        }
        if std::time::Instant::now() >= deadline {
            break 'outer;
        }
    }
    mdns.shutdown().map_err(|e| format!("shutdown: {e}"))?;

    if seen.is_empty() {
        println!("(no Matter devices found)");
    } else {
        for (name, (port, addrs, summary, svc)) in seen {
            let suffix = match svc {
                "_matterc._udp" => "._matterc._udp.local.",
                "_matter._tcp"  => "._matter._tcp.local.",
                _ => "",
            };
            let short = name.strip_suffix(suffix).unwrap_or(&name);
            println!("  [{svc}] {short} port={port} addrs={addrs:?} {summary}");
        }
    }
    Ok(())
}
