//! matter-op — drive a commissioned Matter device (on/off/state/power/energy).
//!
//! Usage:
//!   matter-op <subcommand> --fabric-label LABEL --node-id N [--endpoint EP]
//!
//! Subcommands:
//!   on        invoke OnOff::On (cluster 0x0006, cmd 0x01) on EP (default 1)
//!   off       invoke OnOff::Off  (cluster 0x0006, cmd 0x00)
//!   toggle    invoke OnOff::Toggle (cluster 0x0006, cmd 0x02)
//!   state     read   OnOff::OnOff (attr 0x0000) bool
//!   power     read   ElectricalPowerMeasurement::ActivePower (cluster 0x0090, attr 0x0008) i64 mW
//!   energy    read   ElectricalEnergyMeasurement::CumulativeEnergyImported (cluster 0x0091, attr 0x0001) struct
//!
//! Discovers device IPv6 via mDNS, runs CASE-over-UDP, performs the op,
//! prints result. Same fabric-loading + UDP handshake machinery the BLE
//! commissioner uses for step 8.

use std::time::Duration;

use syntaur_matter_ble::case_udp::{discover_operational, with_case_op_persisted};

#[derive(Debug)]
struct Args {
    op: Op,
    fabric_label: String,
    node_id: u64,
    endpoint: u16,
}

#[derive(Debug, Clone, Copy)]
enum Op {
    On,
    Off,
    Toggle,
    State,
    Power,
    Energy,
    /// CommissioningComplete on the GeneralCommissioning cluster. Used to
    /// rescue a device whose commissioning's final CASE handshake timed
    /// out: AddNOC + AddOrUpdateNetwork + ConnectNetwork have all
    /// succeeded server-side, but the failsafe is still armed and will
    /// roll those changes back at expiry. Running this within the
    /// failsafe window finalizes commissioning.
    Complete,
}

const CLUSTER_ON_OFF: u32 = 0x0006;
const CLUSTER_GENERAL_COMMISSIONING: u32 = 0x0030;
const CLUSTER_ELEC_POWER: u32 = 0x0090;
const CLUSTER_ELEC_ENERGY: u32 = 0x0091;
const CMD_OFF: u32 = 0x00;
const CMD_ON: u32 = 0x01;
const CMD_TOGGLE: u32 = 0x02;
const CMD_COMMISSIONING_COMPLETE: u32 = 0x04;
const ATTR_ONOFF: u32 = 0x0000;
const ATTR_ACTIVE_POWER: u32 = 0x0008;
const ATTR_CUM_ENERGY_IMPORTED: u32 = 0x0001;

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        return Err(usage());
    }
    let op = match argv[0].as_str() {
        "on" => Op::On,
        "off" => Op::Off,
        "toggle" => Op::Toggle,
        "state" => Op::State,
        "power" => Op::Power,
        "energy" => Op::Energy,
        "complete" => Op::Complete,
        "-h" | "--help" => return Err(usage()),
        other => return Err(format!("unknown subcommand: {other}\n{}", usage())),
    };
    let mut fabric_label: Option<String> = None;
    let mut node_id: Option<u64> = None;
    let mut endpoint: u16 = 1;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--fabric-label" => {
                fabric_label = Some(argv.get(i + 1).cloned().ok_or("--fabric-label needs a value")?);
                i += 2;
            }
            "--node-id" => {
                let v = argv.get(i + 1).ok_or("--node-id needs a value")?;
                let n = if let Some(hex) = v.strip_prefix("0x") {
                    u64::from_str_radix(hex, 16).map_err(|e| format!("--node-id hex: {e}"))?
                } else {
                    v.parse::<u64>().map_err(|e| format!("--node-id: {e}"))?
                };
                node_id = Some(n);
                i += 2;
            }
            "--endpoint" => {
                endpoint = argv.get(i + 1).ok_or("--endpoint needs a value")?
                    .parse().map_err(|e| format!("--endpoint: {e}"))?;
                i += 2;
            }
            other => return Err(format!("unknown flag: {other}\n{}", usage())),
        }
    }
    Ok(Args {
        op,
        fabric_label: fabric_label.ok_or("--fabric-label is required")?,
        node_id: node_id.ok_or("--node-id is required")?,
        endpoint,
    })
}

fn usage() -> String {
    "usage: matter-op <on|off|toggle|state|power|energy|complete> \
        --fabric-label LABEL --node-id N [--endpoint EP]".to_string()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = parse_args().map_err(|e| { eprintln!("{e}"); e })?;

    let fabric = syntaur_matter::persist::load_fabric(&args.fabric_label)
        .map_err(|e| format!("load fabric {}: {e:?}", args.fabric_label))?;
    log::info!("loaded fabric {} (fabric_id {:#x})", fabric.label, fabric.fabric_id);

    log::info!("discovering operational mDNS for node {:#x} ({:016X})", args.node_id, args.node_id);
    let addr = discover_operational(args.node_id, Duration::from_secs(15))
        .map_err(|e| format!("mdns: {e:?}"))?;
    log::info!("resolved {addr}");

    let rcac = hex::decode(&fabric.root_cert_hex)
        .map_err(|e| format!("rcac hex decode: {e}"))?;
    let mut ca_secret_key_scalar = [0u8; 32];
    let ca_decoded = hex::decode(&fabric.ca_secret_key_hex)
        .map_err(|e| format!("ca_secret hex decode: {e}"))?;
    if ca_decoded.len() != 32 { return Err(format!("ca_secret wrong length: {}", ca_decoded.len())); }
    ca_secret_key_scalar.copy_from_slice(&ca_decoded);
    let mut ipk = [0u8; 16];
    let ipk_decoded = hex::decode(&fabric.ipk_hex).map_err(|e| format!("ipk hex decode: {e}"))?;
    if ipk_decoded.len() != 16 { return Err(format!("ipk wrong length: {}", ipk_decoded.len())); }
    ipk.copy_from_slice(&ipk_decoded);

    let controller_noc_hex = fabric.controller_noc_hex.as_ref().ok_or_else(|| {
        format!(
            "fabric {} has no persisted controller NOC. Re-mint the fabric              (matter-fabric delete {} && matter-fabric new {}) and re-commission              devices. The post-commissioning CASE handshake needs a stable              controller identity that this fabric format provides.",
            fabric.label, fabric.label, fabric.label
        )
    })?;
    let controller_secret_key_hex = fabric.controller_secret_key_hex.as_ref().ok_or_else(|| {
        format!("fabric {} missing controller_secret_key_hex (paired field)", fabric.label)
    })?;
    let controller_noc = hex::decode(controller_noc_hex)
        .map_err(|e| format!("controller_noc hex decode: {e}"))?;
    let controller_secret_decoded = hex::decode(controller_secret_key_hex)
        .map_err(|e| format!("controller_secret hex decode: {e}"))?;
    if controller_secret_decoded.len() != 32 {
        return Err(format!("controller_secret wrong length: {}", controller_secret_decoded.len()));
    }
    let mut controller_secret_scalar = [0u8; 32];
    controller_secret_scalar.copy_from_slice(&controller_secret_decoded);

    let op = args.op;
    let endpoint = args.endpoint;
    let node_id = args.node_id;

    let _ = ca_secret_key_scalar; // not needed for persisted-NOC path
    with_case_op_persisted(
        addr,
        fabric.fabric_id,
        node_id,
        rcac,
        controller_noc,
        controller_secret_scalar,
        ipk,
        fabric.vendor_id,
        move |ex| {
            Box::pin(async move {
                use rs_matter::im::client::ImClient;
                use rs_matter::im::{AttrResp, CmdResp};
                use rs_matter::tlv::TLVElement;
                use syntaur_matter::error::MatterFabricError;

                match op {
                    Op::Complete => {
                        // CommissioningComplete is empty payload per Matter
                        // Core 11.10.6.6 — same TLV bytes we send during
                        // BTP-side commissioning's step 8.
                        let empty_payload: Vec<u8> = vec![0x15, 0x18];
                        let tlv = TLVElement::new(&empty_payload);
                        let resp = ImClient::invoke_single_cmd(
                            ex,
                            0,
                            CLUSTER_GENERAL_COMMISSIONING,
                            CMD_COMMISSIONING_COMPLETE,
                            tlv,
                            None,
                        )
                        .await
                        .map_err(|e| MatterFabricError::Matter(format!(
                            "CommissioningComplete: {e:?}"
                        )))?;
                        match resp {
                            CmdResp::Cmd(_) => {
                                println!("✓ CommissioningComplete (InvokeResponse) — failsafe disarmed");
                            }
                            CmdResp::Status(s) => {
                                if s.status.status == rs_matter::im::IMStatusCode::Success {
                                    println!("✓ CommissioningComplete (Success) — failsafe disarmed");
                                } else {
                                    println!("✗ CommissioningComplete returned {:?}", s.status);
                                }
                            }
                        }
                    }
                    Op::On | Op::Off | Op::Toggle => {
                        let (cluster, cmd) = match op {
                            Op::On => (CLUSTER_ON_OFF, CMD_ON),
                            Op::Off => (CLUSTER_ON_OFF, CMD_OFF),
                            Op::Toggle => (CLUSTER_ON_OFF, CMD_TOGGLE),
                            _ => unreachable!(),
                        };
                        let empty_payload: Vec<u8> = vec![0x15, 0x18];
                        let tlv = TLVElement::new(&empty_payload);
                        let resp = ImClient::invoke_single_cmd(ex, endpoint, cluster, cmd, tlv, None)
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!(
                                "invoke ep={endpoint} cluster={cluster:#x} cmd={cmd:#x}: {e:?}"
                            )))?;
                        let label = match op {
                            Op::On => "ON", Op::Off => "OFF", Op::Toggle => "TOGGLE",
                            _ => "?",
                        };
                        match resp {
                            CmdResp::Cmd(_) => println!("✓ {label} (InvokeResponse)"),
                            CmdResp::Status(s) => {
                                if s.status.status == rs_matter::im::IMStatusCode::Success {
                                    println!("✓ {label} (Success)");
                                } else {
                                    println!("✗ {label} returned {:?}", s.status);
                                }
                            }
                        }
                    }
                    Op::State => {
                        let resp = ImClient::read_single_attr(ex, endpoint, CLUSTER_ON_OFF, ATTR_ONOFF, false)
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!("read OnOff: {e:?}")))?;
                        match resp {
                            AttrResp::Data(d) => {
                                let on = d.data.bool().map_err(|e| MatterFabricError::Matter(format!(
                                    "OnOff bool decode: {e:?}"
                                )))?;
                                println!("state: {}", if on { "ON" } else { "OFF" });
                            }
                            AttrResp::Status(s) => println!("state read returned status: {:?}", s),
                        }
                    }
                    Op::Power => {
                        let resp = ImClient::read_single_attr(ex, endpoint, CLUSTER_ELEC_POWER, ATTR_ACTIVE_POWER, false)
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!("read ActivePower: {e:?}")))?;
                        match resp {
                            AttrResp::Data(d) => {
                                let mw = d.data.i64().map_err(|e| MatterFabricError::Matter(format!(
                                    "ActivePower i64 decode: {e:?}"
                                )))?;
                                println!("power: {} mW ({:.3} W)", mw, mw as f64 / 1000.0);
                            }
                            AttrResp::Status(s) => println!("power read returned status: {:?}", s),
                        }
                    }
                    Op::Energy => {
                        let resp = ImClient::read_single_attr(ex, endpoint, CLUSTER_ELEC_ENERGY, ATTR_CUM_ENERGY_IMPORTED, false)
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!("read CumEnergy: {e:?}")))?;
                        match resp {
                            AttrResp::Data(d) => {
                                println!("energy raw TLV: {} bytes", d.data.raw_data().len());
                                println!("  hex: {}", d.data.raw_data().iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(""));
                            }
                            AttrResp::Status(s) => println!("energy read returned status: {:?}", s),
                        }
                    }
                }
                Ok::<(), MatterFabricError>(())
            })
        },
    )
    .await
    .map_err(|e| format!("CASE op: {e:?}"))?;

    Ok(())
}
