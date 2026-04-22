//! `matter-ip-commission` — CLI driver for IP-side multi-admin commissioning.
//!
//! Prerequisite: the target device is already on another fabric (HA,
//! Apple Home, Google Home) and that admin has opened a commissioning
//! window. We receive the setup pin + manual code from the admin, then
//! discover the device via mDNS and drive the 8-step Commissioner
//! state machine over UDP.
//!
//! ## Usage
//!
//! ```text
//! matter-ip-commission \
//!     --fabric-label primary \
//!     --setup-pin 14596893 \
//!     --discriminator 0xe9 \
//!     --assigned-node-id 200 \
//!     [--peer-addr 192.168.20.52:5540]   # skip mDNS discovery
//!     [--wifi-ssid NAME --wifi-psk PASSWORD]
//!     [--timeout-secs 60]
//! ```
//!
//! If `--peer-addr` is provided, mDNS discovery is skipped. Otherwise
//! we listen for `_matterc._udp.local` broadcasts matching the
//! discriminator for up to 30s.

use std::env;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use syntaur_matter::commission::{Commissioner, WifiCredentials};
use syntaur_matter::load_fabric;
use syntaur_matter_ip::{mdns, IpCommissionExchange};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args: Vec<String> = env::args().collect();
    let args = match parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage error: {e}");
            print_usage();
            return ExitCode::from(1);
        }
    };

    if let Err(e) = run(args).await {
        eprintln!("commissioning failed: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

struct Args {
    fabric_label: String,
    setup_pin: u32,
    discriminator: Option<u16>,
    peer_addr: Option<SocketAddr>,
    assigned_node_id: u64,
    wifi_ssid: Option<String>,
    wifi_psk: Option<String>,
    timeout: Duration,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut fabric_label = None;
    let mut setup_pin: Option<u32> = None;
    let mut discriminator: Option<u16> = None;
    let mut peer_addr: Option<SocketAddr> = None;
    let mut assigned_node_id: Option<u64> = None;
    let mut wifi_ssid = None;
    let mut wifi_psk = None;
    let mut timeout = 60u64;

    let mut i = 1;
    while i < argv.len() {
        let take = |i: &mut usize, flag: &str| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match argv[i].as_str() {
            "--fabric-label" => fabric_label = Some(take(&mut i, "--fabric-label")?),
            "--setup-pin" => {
                let v = take(&mut i, "--setup-pin")?;
                setup_pin = Some(v.parse().map_err(|e| format!("--setup-pin: {e}"))?);
            }
            "--discriminator" => {
                let v = take(&mut i, "--discriminator")?;
                let v = v.strip_prefix("0x").unwrap_or(&v);
                discriminator = Some(
                    u16::from_str_radix(v, if v.contains(|c: char| !c.is_ascii_digit()) { 16 } else { 10 })
                        .or_else(|_| v.parse::<u16>())
                        .map_err(|e| format!("--discriminator: {e}"))?,
                );
            }
            "--peer-addr" => {
                let v = take(&mut i, "--peer-addr")?;
                peer_addr = Some(v.parse().map_err(|e| format!("--peer-addr: {e}"))?);
            }
            "--assigned-node-id" => {
                let v = take(&mut i, "--assigned-node-id")?;
                assigned_node_id = Some(v.parse().map_err(|e| format!("--assigned-node-id: {e}"))?);
            }
            "--wifi-ssid" => wifi_ssid = Some(take(&mut i, "--wifi-ssid")?),
            "--wifi-psk" => wifi_psk = Some(take(&mut i, "--wifi-psk")?),
            "--timeout-secs" => {
                let v = take(&mut i, "--timeout-secs")?;
                timeout = v.parse().map_err(|e| format!("--timeout-secs: {e}"))?;
            }
            "-h" | "--help" => return Err("help".into()),
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }

    Ok(Args {
        fabric_label: fabric_label.ok_or("--fabric-label required")?,
        setup_pin: setup_pin.ok_or("--setup-pin required")?,
        discriminator,
        peer_addr,
        assigned_node_id: assigned_node_id.ok_or("--assigned-node-id required")?,
        wifi_ssid,
        wifi_psk,
        timeout: Duration::from_secs(timeout),
    })
}

fn print_usage() {
    eprintln!(
        r#"
  matter-ip-commission --fabric-label LABEL --setup-pin N --assigned-node-id ID \
                       (--discriminator 0xXXX | --peer-addr IP:PORT) \
                       [--wifi-ssid S --wifi-psk P] [--timeout-secs N]

  Joins Syntaur fabric as a second admin on a device that another fabric
  has opened via AdministratorCommissioning.OpenCommissioningWindow.
"#
    );
}

async fn run(args: Args) -> Result<(), String> {
    log::info!("loading fabric {:?}", args.fabric_label);
    let fabric = load_fabric(&args.fabric_label).map_err(|e| format!("load fabric: {e}"))?;

    let peer_addr = match args.peer_addr {
        Some(a) => a,
        None => {
            let d = args
                .discriminator
                .ok_or("--peer-addr or --discriminator required")?;
            log::info!(
                "mDNS discovery for _matterc._udp with discriminator {:#x} (up to 30s)",
                d
            );
            tokio::task::spawn_blocking(move || mdns::discover(Some(d), Duration::from_secs(30)))
                .await
                .map_err(|e| format!("mdns spawn: {e}"))?
                .map_err(|e| format!("mdns discover: {e}"))?
        }
    };
    log::info!("target peer: {peer_addr}");

    let mut exchange = IpCommissionExchange::new(peer_addr, args.setup_pin);

    let wifi = match (args.wifi_ssid.as_ref(), args.wifi_psk.as_ref()) {
        (Some(s), Some(p)) => {
            log::info!("wifi creds supplied — ssid={:?}", s);
            Some(WifiCredentials {
                ssid: s.as_bytes().to_vec(),
                psk: p.as_bytes().to_vec(),
            })
        }
        _ => {
            log::info!(
                "no wifi creds — second-admin join on an already-networked device (typical)"
            );
            None
        }
    };

    log::info!(
        "running Commissioner (8 steps) as second-admin into fabric {:?} as node {}",
        args.fabric_label,
        args.assigned_node_id
    );
    let commissioner = Commissioner::new(&fabric);
    let commissioned = commissioner
        .commission(&mut exchange, args.assigned_node_id, wifi)
        .await
        .map_err(|e| format!("commission: {e}"))?;

    println!(
        "\n✓ COMMISSIONED\n  node_id: {}\n  fabric: {}\n  add_noc_response: {} bytes\n",
        commissioned.node_id,
        commissioned.fabric_label,
        commissioned.add_noc_response.len()
    );
    Ok(())
}
