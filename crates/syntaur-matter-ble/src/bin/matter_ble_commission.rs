//! `matter-ble-commission` — CLI driver for end-to-end BLE Matter
//! commissioning. Runs the full pipeline: scan → BTP handshake → PASE
//! → IM state machine → device lands in fabric.
//!
//! Designed to be scp'd to a host with a working HCI adapter (HAOS SSH
//! add-on is the primary target) and run against one factory-fresh
//! device at a time.
//!
//! ## Usage
//!
//! ```text
//! matter-ble-commission \
//!     --fabric-label primary \
//!     --code 0341-091-2217 \
//!     --assigned-node-id 100 \
//!     [--wifi-ssid NAME --wifi-psk PASSWORD] \
//!     [--timeout-secs 60]
//! ```
//!
//! Fabric must already exist in `~/.syntaur/matter_fabrics/<label>.enc`
//! (create one via `matter-fabric new <label>`).

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use syntaur_matter::commission::{
    Commissioner, NetworkCredentials, ThreadCredentials, WifiCredentials,
};
use syntaur_matter::{load_fabric, parse_manual_code, parse_qr};
use syntaur_matter_ble::{btp::BleCommissionExchange, scan_for_discriminator};

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
    qr: Option<String>,
    code: Option<String>,
    assigned_node_id: u64,
    wifi_ssid: Option<String>,
    wifi_psk: Option<String>,
    thread_dataset_hex: Option<String>,
    thread_dataset_file: Option<String>,
    scan_timeout: Duration,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut fabric_label = None;
    let mut qr = None;
    let mut code = None;
    let mut assigned_node_id: Option<u64> = None;
    let mut wifi_ssid = None;
    let mut wifi_psk = None;
    let mut thread_dataset_hex = None;
    let mut thread_dataset_file = None;
    let mut scan_timeout = 15u64;

    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--fabric-label" => {
                fabric_label = Some(argv.get(i + 1).cloned().ok_or("--fabric-label needs a value")?);
                i += 2;
            }
            "--qr" => {
                qr = Some(argv.get(i + 1).cloned().ok_or("--qr needs a value")?);
                i += 2;
            }
            "--code" => {
                code = Some(argv.get(i + 1).cloned().ok_or("--code needs a value")?);
                i += 2;
            }
            "--assigned-node-id" => {
                assigned_node_id = Some(
                    argv.get(i + 1)
                        .ok_or("--assigned-node-id needs a value")?
                        .parse()
                        .map_err(|e| format!("--assigned-node-id: {e}"))?,
                );
                i += 2;
            }
            "--wifi-ssid" => {
                wifi_ssid = Some(argv.get(i + 1).cloned().ok_or("--wifi-ssid needs a value")?);
                i += 2;
            }
            "--wifi-psk" => {
                wifi_psk = Some(argv.get(i + 1).cloned().ok_or("--wifi-psk needs a value")?);
                i += 2;
            }
            "--thread-dataset-hex" => {
                thread_dataset_hex =
                    Some(argv.get(i + 1).cloned().ok_or("--thread-dataset-hex needs a value")?);
                i += 2;
            }
            "--thread-dataset-file" => {
                thread_dataset_file =
                    Some(argv.get(i + 1).cloned().ok_or("--thread-dataset-file needs a value")?);
                i += 2;
            }
            "--timeout-secs" => {
                scan_timeout = argv
                    .get(i + 1)
                    .ok_or("--timeout-secs needs a value")?
                    .parse()
                    .map_err(|e| format!("--timeout-secs: {e}"))?;
                i += 2;
            }
            "-h" | "--help" => return Err("help requested".into()),
            other => return Err(format!("unknown flag {other}")),
        }
    }

    Ok(Args {
        fabric_label: fabric_label.ok_or("--fabric-label is required")?,
        qr,
        code,
        assigned_node_id: assigned_node_id.ok_or("--assigned-node-id is required")?,
        wifi_ssid,
        wifi_psk,
        thread_dataset_hex,
        thread_dataset_file,
        scan_timeout: Duration::from_secs(scan_timeout),
    })
}

fn print_usage() {
    eprintln!(
        "\n  matter-ble-commission --fabric-label <LABEL> (--qr MT:...|--code XXXX-XXX-XXXX) \\
                        --assigned-node-id <N> \\
                        [--wifi-ssid S --wifi-psk P] \\
                        [--thread-dataset-hex HEX | --thread-dataset-file PATH] \\
                        [--timeout-secs N]\n"
    );
}

async fn run(args: Args) -> Result<(), String> {
    log::info!("loading fabric {:?}", args.fabric_label);
    let fabric =
        load_fabric(&args.fabric_label).map_err(|e| format!("load fabric: {e}"))?;

    let payload = match (args.qr.as_deref(), args.code.as_deref()) {
        (Some(q), _) => parse_qr(q).map_err(|e| format!("parse qr: {e}"))?,
        (_, Some(c)) => parse_manual_code(c).map_err(|e| format!("parse code: {e}"))?,
        _ => return Err("supply either --qr or --code".into()),
    };
    log::info!(
        "pairing code decoded — discriminator {:#x}, passcode ******",
        payload.discriminator
    );

    // 11-digit manual codes only carry the upper-4-bit discriminator,
    // so we scan unfiltered + match by upper 4 bits. 21-digit codes +
    // QR codes carry the full 12 bits — exact match then.
    let want_upper = (payload.discriminator >> 8) & 0x0F;
    let exact = (payload.discriminator & 0x0FF) != 0;
    log::info!(
        "scanning BLE for discriminator upper={:#x} (exact_full={}, up to {}s)",
        want_upper,
        exact,
        args.scan_timeout.as_secs()
    );
    let candidates = scan_for_discriminator(None, args.scan_timeout)
        .await
        .map_err(|e| format!("BLE scan: {e}"))?;
    // Sort by RSSI descending (strongest first) to prefer the closest bulb.
    let mut candidates = candidates;
    candidates.sort_by_key(|d| std::cmp::Reverse(d.rssi.unwrap_or(-127)));
    let device = if exact {
        candidates.into_iter().find(|d| d.discriminator == payload.discriminator)
    } else {
        candidates.into_iter().find(|d| ((d.discriminator >> 8) & 0x0F) == want_upper)
    }
    .ok_or("no commissionable device found in range matching discriminator. Is the bulb in commissioning mode (blinking)?")?;
    log::info!(
        "found {} vendor={:#06x} product={:#06x} rssi={:?}",
        device.address,
        device.vendor_id,
        device.product_id,
        device.rssi
    );

    let device_address = device.address.clone();
    let device_vendor_id = device.vendor_id;
    let device_product_id = device.product_id;
    let device_discriminator = device.discriminator;
    let device_local_name = device.local_name.clone();
    let mut exchange = BleCommissionExchange::connect(device, payload.passcode)
        .await
        .map_err(|e| format!("BTP session open: {e}"))?;

    let network = resolve_network(&args)?;
    match &network {
        Some(NetworkCredentials::Wifi(w)) => {
            log::info!("WiFi network handoff — ssid={:?} ({} B PSK)", String::from_utf8_lossy(&w.ssid), w.psk.len());
        }
        Some(NetworkCredentials::Thread(t)) => {
            log::info!("Thread network handoff — {}-byte operational dataset", t.operational_dataset.len());
        }
        None => {
            log::info!("no network creds — device must be already on a network (IP/OCW path)");
        }
    }

    log::info!(
        "running Commissioner state machine (8 steps) against assigned_node_id {}",
        args.assigned_node_id
    );
    let commissioner = Commissioner::new(&fabric);
    let commissioned = commissioner
        .commission(&mut exchange, args.assigned_node_id, network)
        .await
        .map_err(|e| format!("commissioning: {e}"))?;

    println!(
        "\n✓ COMMISSIONED\n  node_id: {}\n  fabric: {}\n  add_noc_response: {} bytes\n",
        commissioned.node_id,
        commissioned.fabric_label,
        commissioned.add_noc_response.len()
    );

    let captured_device = CapturedDevice {
        address: device_address,
        vendor_id: device_vendor_id,
        product_id: device_product_id,
        discriminator: device_discriminator,
        local_name: device_local_name,
    };
    if let Err(e) = persist_pairing_record(&args, &fabric, &commissioned, &captured_device) {
        log::warn!("[commission] failed to persist pairing record: {e}. Phase 7 auto-recommission won\'t catch this device until next manual commission.");
    } else {
        log::info!("[commission] pairing record saved to ~/.syntaur/matter_pairings/{:016x}.json", commissioned.node_id);
    }

    Ok(())
}

/// Write a minimal pairing record so the Phase 7 auto-recommission daemon
/// (and any future "list my commissioned devices" UX) has a stable on-disk
/// inventory keyed by node_id.
///
/// Stored at `~/.syntaur/matter_pairings/<node_id_hex16>.json`. Schema is
/// intentionally small + JSON for easy reading by other tools (HA bridge,
/// dashboard inventory, etc.).
struct CapturedDevice {
    address: String,
    vendor_id: u16,
    product_id: u16,
    discriminator: u16,
    local_name: Option<String>,
}

fn persist_pairing_record(
    args: &Args,
    fabric: &syntaur_matter::FabricHandle,
    commissioned: &syntaur_matter::commission::CommissionedDevice,
    device: &CapturedDevice,
) -> Result<(), String> {
    use serde_json::json;
    use std::fs;

    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let dir = std::path::PathBuf::from(home).join(".syntaur").join("matter_pairings");
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let network_kind = if args.thread_dataset_hex.is_some() || args.thread_dataset_file.is_some() {
        "thread"
    } else if args.wifi_ssid.is_some() {
        "wifi"
    } else {
        "unknown"
    };

    let record = json!({
        "schema_version": 1,
        "node_id": commissioned.node_id,
        "node_id_hex": format!("{:016x}", commissioned.node_id),
        "fabric_label": commissioned.fabric_label.clone(),
        "fabric_id": fabric.fabric_id,
        "fabric_id_hex": format!("{:016x}", fabric.fabric_id),
        "controller_node_id": fabric.controller_node_id,
        "vendor_id": device.vendor_id,
        "product_id": device.product_id,
        "discriminator": device.discriminator,
        "ble_name": device.local_name.clone(),
        "ble_address": device.address.clone(),
        "setup_code": args.code.clone(),
        "qr_string": args.qr.clone(),
        "network_kind": network_kind,
        "assigned_at": chrono::Utc::now().to_rfc3339(),
        "last_commissioned_at": chrono::Utc::now().to_rfc3339(),
    });

    let path = dir.join(format!("{:016x}.json", commissioned.node_id));
    let body = serde_json::to_string_pretty(&record).map_err(|e| format!("json: {e}"))?;
    fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn resolve_network(args: &Args) -> Result<Option<NetworkCredentials>, String> {
    let wifi = match (args.wifi_ssid.as_ref(), args.wifi_psk.as_ref()) {
        (Some(s), Some(p)) => Some(WifiCredentials {
            ssid: s.as_bytes().to_vec(),
            psk: p.as_bytes().to_vec(),
        }),
        _ => None,
    };
    let thread = match (args.thread_dataset_hex.as_deref(), args.thread_dataset_file.as_deref()) {
        (Some(hex), _) => {
            let bytes = hex::decode(hex.trim()).map_err(|e| format!("--thread-dataset-hex decode: {e}"))?;
            Some(ThreadCredentials { operational_dataset: bytes })
        }
        (_, Some(path)) => {
            let raw = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
            let bytes = hex::decode(raw.trim()).map_err(|e| format!("{path} hex decode: {e}"))?;
            Some(ThreadCredentials { operational_dataset: bytes })
        }
        _ => None,
    };
    match (wifi, thread) {
        (Some(_), Some(_)) => Err("specify only one of --wifi-* or --thread-dataset-*".into()),
        (Some(w), None) => Ok(Some(NetworkCredentials::Wifi(w))),
        (None, Some(t)) => Ok(Some(NetworkCredentials::Thread(t))),
        (None, None) => Ok(None),
    }
}
