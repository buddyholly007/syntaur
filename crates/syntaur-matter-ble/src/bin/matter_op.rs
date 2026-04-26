//! matter-op — drive a commissioned Matter device (on/off/state/power/energy).
//!
//! Usage:
//!   matter-op <subcommand> --fabric-label LABEL --node-id N [--endpoint EP]
//!
//! Subcommands:
//!   on          invoke OnOff::On (cluster 0x0006, cmd 0x01) on EP (default 1)
//!   off         invoke OnOff::Off  (cluster 0x0006, cmd 0x00)
//!   toggle      invoke OnOff::Toggle (cluster 0x0006, cmd 0x02)
//!   state       read   OnOff::OnOff (attr 0x0000) bool
//!   power       read   ElectricalPowerMeasurement::ActivePower (cluster 0x0090, attr 0x0008) i64 mW
//!   energy      read   ElectricalEnergyMeasurement::CumulativeEnergyImported (cluster 0x0091, attr 0x0001) struct
//!   info        read   BasicInformation cluster (0x0028) ep0 — vendor/product/HW/SW versions
//!   ota-status  read   OtaSoftwareUpdateRequestor (0x0029) ep0 — UpdatePossible + UpdateState
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
    /// Number of full off→on cycles to send (rapid-cycle only).
    cycles: u32,
    /// Time to hold OFF between off-command and on-command, in milliseconds.
    off_ms: u64,
    /// Time to hold ON between on-command and the next off-command, in milliseconds.
    on_ms: u64,
    /// Optional `IP:PORT` override that skips operational mDNS lookup.
    /// Useful for cross-VLAN devices where mDNS doesn't reflect across
    /// subnet boundaries — e.g., a bulb on the IoT VLAN viewed from
    /// the Default VLAN. Standard Matter operational port is 5540.
    addr: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum Op {
    On,
    Off,
    Toggle,
    State,
    Power,
    Energy,
    /// Issue N rapid off/on cycles over a single CASE session. Used to
    /// trigger AiDot-class smart-bulb factory-reset (typically 5+ cycles
    /// each <2 s) without paying CASE handshake overhead per command —
    /// matter-op's normal one-command-per-invocation costs ~5 s of CASE
    /// every shot, far too slow to drive a vendor-defined reset pattern.
    RapidCycle,
    /// CommissioningComplete on the GeneralCommissioning cluster. Used to
    /// rescue a device whose commissioning's final CASE handshake timed
    /// out: AddNOC + AddOrUpdateNetwork + ConnectNetwork have all
    /// succeeded server-side, but the failsafe is still armed and will
    /// roll those changes back at expiry. Running this within the
    /// failsafe window finalizes commissioning.
    Complete,
    /// Read BasicInformation cluster on endpoint 0 — the canonical
    /// nameplate that identifies a Matter device. Used by the firmware
    /// advisor to map (VendorID, ProductID) → known-latest version and
    /// surface "your plug needs an update" UX.
    Info,
    /// Read OtaSoftwareUpdateRequestor cluster on endpoint 0. Tells us
    /// whether the device implements standard Matter OTA at all
    /// (UpdatePossible) and what state its updater is in. If it
    /// reports UnsupportedCluster, the vendor uses a proprietary
    /// channel (Meross app, Tapo app, etc.) and Syntaur cannot push
    /// firmware directly.
    OtaStatus,
    /// Walk the Descriptor cluster on every endpoint and render a
    /// human-readable capability summary: device-type label, supported
    /// controls (Power / Brightness / Color temp / Color), and ranges
    /// in human units (brightness 0-100%, color temp Warm←→Cool with
    /// Kelvin in parens). Drives the per-device tile UI and the agent
    /// tool surface — we only show controls that the device actually
    /// supports.
    Caps,
}

const CLUSTER_ON_OFF: u32 = 0x0006;
const CLUSTER_BASIC_INFO: u32 = 0x0028;
const CLUSTER_OTA_REQUESTOR: u32 = 0x0029;
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

// BasicInformation cluster (0x0028) attribute IDs — Matter Core 1.5 §11.1.5.
const ATTR_BASIC_VENDOR_NAME: u32 = 0x0001;
const ATTR_BASIC_VENDOR_ID: u32 = 0x0002;
const ATTR_BASIC_PRODUCT_NAME: u32 = 0x0003;
const ATTR_BASIC_PRODUCT_ID: u32 = 0x0004;
const ATTR_BASIC_HARDWARE_VERSION: u32 = 0x0007;
const ATTR_BASIC_HARDWARE_VERSION_STRING: u32 = 0x0008;
const ATTR_BASIC_SOFTWARE_VERSION: u32 = 0x0009;
const ATTR_BASIC_SOFTWARE_VERSION_STRING: u32 = 0x000A;
const ATTR_BASIC_SERIAL_NUMBER: u32 = 0x000F;

// OtaSoftwareUpdateRequestor cluster (0x0029) attribute IDs.
const ATTR_OTA_UPDATE_POSSIBLE: u32 = 0x0001;
const ATTR_OTA_UPDATE_STATE: u32 = 0x0002;
const ATTR_OTA_UPDATE_STATE_PROGRESS: u32 = 0x0003;

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
        "info" => Op::Info,
        "ota-status" => Op::OtaStatus,
        "caps" => Op::Caps,
        "rapid-cycle" => Op::RapidCycle,
        "-h" | "--help" => return Err(usage()),
        other => return Err(format!("unknown subcommand: {other}\n{}", usage())),
    };
    let mut fabric_label: Option<String> = None;
    let mut node_id: Option<u64> = None;
    let mut endpoint: u16 = 1;
    let mut cycles: u32 = 5;
    let mut off_ms: u64 = 600;
    let mut on_ms: u64 = 600;
    let mut addr: Option<String> = None;
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
                    if hex.is_empty() {
                        return Err("--node-id 0x needs hex digits after the prefix".into());
                    }
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
            "--cycles" => {
                cycles = argv.get(i + 1).ok_or("--cycles needs a value")?
                    .parse().map_err(|e| format!("--cycles: {e}"))?;
                i += 2;
            }
            "--off-ms" => {
                off_ms = argv.get(i + 1).ok_or("--off-ms needs a value")?
                    .parse().map_err(|e| format!("--off-ms: {e}"))?;
                i += 2;
            }
            "--on-ms" => {
                on_ms = argv.get(i + 1).ok_or("--on-ms needs a value")?
                    .parse().map_err(|e| format!("--on-ms: {e}"))?;
                i += 2;
            }
            "--addr" => {
                addr = Some(argv.get(i + 1).cloned().ok_or("--addr needs IP:PORT")?);
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
        cycles,
        off_ms,
        on_ms,
        addr,
    })
}

fn usage() -> String {
    "usage: matter-op <on|off|toggle|state|power|energy|complete|info|ota-status|caps|rapid-cycle> \
        --fabric-label LABEL --node-id N [--endpoint EP] \
        [--addr IP:PORT  (skip mDNS — useful cross-VLAN)] \
        [--cycles N --off-ms MS --on-ms MS  (rapid-cycle only)]".to_string()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Don't double-print: main's Err is reported by the runtime.
    let args = parse_args()?;

    let fabric = syntaur_matter::persist::load_fabric(&args.fabric_label)
        .map_err(|e| format!("load fabric {}: {e:?}", args.fabric_label))?;
    log::info!("loaded fabric {} (fabric_id {:#x})", fabric.label, fabric.fabric_id);

    let addr = if let Some(a) = args.addr.as_deref() {
        log::info!("[matter-op] using --addr override {a} (skipping mDNS)");
        a.parse::<std::net::SocketAddr>()
            .map_err(|e| format!("--addr {a:?} parse: {e}"))?
    } else {
        log::info!("discovering operational mDNS for node {:#x} ({:016X})", args.node_id, args.node_id);
        let a = discover_operational(args.node_id, Duration::from_secs(15))
            .map_err(|e| format!("mdns: {e:?}"))?;
        log::info!("resolved {a}");
        a
    };

    let rcac = hex::decode(&fabric.root_cert_hex)
        .map_err(|e| format!("rcac hex decode: {e}"))?;
    // CA secret key is not needed once a controller NOC is persisted in the
    // fabric file (the persisted-NOC code path uses the controller's own
    // private key for CASE Sigma3). Only validate fabric integrity.
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
    let rapid_cycles = args.cycles;
    let rapid_off_ms = args.off_ms;
    let rapid_on_ms = args.on_ms;

    // Auto-retry on CASE Sigma1→2 hang. Meross MSS315 over WiFi shows
    // ~1-in-5 timeouts where Sigma1 is acked (MRPStandAloneAck) but no
    // Sigma2 follows; subsequent attempts succeed with a fresh
    // InitiatorRandom. Eve over Thread is solid, but the retry costs
    // nothing on a successful first try and turns the Meross flake
    // from a user-visible error into a silent retry. Since the closure
    // only prints on success and bubbles MatterFabricError on failure,
    // a timeout never produces output — safe to wrap in a retry loop.
    let max_attempts: u32 = 3;
    let fabric_vendor_id = fabric.vendor_id;
    let fabric_fabric_id = fabric.fabric_id;
    let mut last_err: Option<String> = None;
    for attempt in 1..=max_attempts {
        if attempt > 1 {
            log::info!("[matter-op] CASE retry {attempt}/{max_attempts} after timeout");
        }
        let rcac_attempt = rcac.clone();
        let controller_noc_attempt = controller_noc.clone();
        let result = with_case_op_persisted(
            addr,
            fabric_fabric_id,
            node_id,
            rcac_attempt,
            controller_noc_attempt,
            controller_secret_scalar,
            ipk,
            fabric_vendor_id,
            move |ex| {
                Box::pin(async move {
                    use rs_matter::im::client::ImClient;
                    use rs_matter::im::{AttrResp, CmdResp, IMStatusCode};
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
                                if s.status.status == IMStatusCode::Success {
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
                                if s.status.status == IMStatusCode::Success {
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
                        // Walk plausible endpoints — Matter 1.3 Energy
                        // devices place ElectricalPowerMeasurement on the
                        // application endpoint, but Eve Energy places it
                        // on a different endpoint than its OnOff cluster.
                        // Try the user-specified endpoint first, then the
                        // common alternates. Stop on first non-Unsupported
                        // response.
                        let candidates: Vec<u16> = {
                            let mut v = vec![endpoint];
                            for e in [1u16, 2, 3] {
                                if !v.contains(&e) { v.push(e); }
                            }
                            v
                        };
                        let mut printed = false;
                        for try_ep in &candidates {
                            let resp = ImClient::read_single_attr(ex, *try_ep, CLUSTER_ELEC_POWER, ATTR_ACTIVE_POWER, false)
                                .await
                                .map_err(|e| MatterFabricError::Matter(format!("read ActivePower ep{try_ep}: {e:?}")))?;
                            match resp {
                                AttrResp::Data(d) => {
                                    let mw = d.data.i64().map_err(|e| MatterFabricError::Matter(format!(
                                        "ActivePower i64 decode: {e:?}"
                                    )))?;
                                    println!("power: {} mW ({:.3} W) [ep {}]", mw, mw as f64 / 1000.0, try_ep);
                                    printed = true;
                                    break;
                                }
                                AttrResp::Status(s) => {
                                    if matches!(
                                        s.status.status,
                                        IMStatusCode::UnsupportedCluster | IMStatusCode::UnsupportedEndpoint
                                    ) {
                                        continue;
                                    }
                                    println!("power read returned status: {:?} [ep {}]", s, try_ep);
                                    printed = true;
                                    break;
                                }
                            }
                        }
                        if !printed {
                            println!("power: no endpoint exposes ElectricalPowerMeasurement (tried {:?})", candidates);
                        }
                    }
                    Op::Energy => {
                        let candidates: Vec<u16> = {
                            let mut v = vec![endpoint];
                            for e in [1u16, 2, 3] {
                                if !v.contains(&e) { v.push(e); }
                            }
                            v
                        };
                        let mut printed = false;
                        for try_ep in &candidates {
                            let resp = ImClient::read_single_attr(ex, *try_ep, CLUSTER_ELEC_ENERGY, ATTR_CUM_ENERGY_IMPORTED, false)
                                .await
                                .map_err(|e| MatterFabricError::Matter(format!("read CumEnergy ep{try_ep}: {e:?}")))?;
                            match resp {
                                AttrResp::Data(d) => {
                                    // EnergyMeasurementStruct (Matter 1.3, cluster 0x0091):
                                    //   tag 0: Energy (int64u, mWh)
                                    //   tag 1: StartTimestamp (optional epoch-s)
                                    //   tag 2: EndTimestamp (optional epoch-s)
                                    //   tag 3: StartSystime (optional system-ms)
                                    //   tag 4: EndSystime (optional system-ms)
                                    // Decode the Energy field; print Wh/kWh for human use.
                                    //
                                    // rs-matter's `.u64()` only accepts wire-type
                                    // U8/U16/U32/U64. Eve Energy encodes 0 as S8
                                    // (smallest-fit signed) so we fall back to
                                    // `.i64()` and cast — cumulative energy can
                                    // never be negative per the spec.
                                    let mwh_result = d.data.r#struct()
                                        .and_then(|seq| seq.find_ctx(0))
                                        .and_then(|e| e.u64().or_else(|_| e.i64().map(|v| v.max(0) as u64)));
                                    match mwh_result {
                                        Ok(mwh) => {
                                            let wh = mwh as f64 / 1000.0;
                                            let kwh = wh / 1000.0;
                                            println!(
                                                "energy: {} mWh ({:.3} Wh / {:.6} kWh) [ep {}]",
                                                mwh, wh, kwh, try_ep
                                            );
                                        }
                                        Err(e) => {
                                            println!("energy: struct decode failed ({e:?}) [ep {}]; raw {} bytes",
                                                try_ep, d.data.raw_data().len());
                                            println!("  hex: {}",
                                                d.data.raw_data().iter()
                                                    .map(|b| format!("{:02x}", b))
                                                    .collect::<Vec<_>>().join(""));
                                        }
                                    }
                                    printed = true;
                                    break;
                                }
                                AttrResp::Status(s) => {
                                    if matches!(
                                        s.status.status,
                                        IMStatusCode::UnsupportedCluster | IMStatusCode::UnsupportedEndpoint
                                    ) {
                                        continue;
                                    }
                                    println!("energy read returned status: {:?} [ep {}]", s, try_ep);
                                    printed = true;
                                    break;
                                }
                            }
                        }
                        if !printed {
                            println!("energy: no endpoint exposes ElectricalEnergyMeasurement (tried {:?})", candidates);
                        }
                    }
                    Op::Info => {
                        // BasicInformation lives on endpoint 0 only.
                        let _ = endpoint;
                        async fn read_str(
                            ex: &mut rs_matter::transport::exchange::Exchange<'_>,
                            attr: u32,
                            label: &str,
                        ) {
                            match ImClient::read_single_attr(ex, 0, CLUSTER_BASIC_INFO, attr, false).await {
                                Ok(AttrResp::Data(d)) => match d.data.utf8() {
                                    Ok(s) => println!("  {label}: {s}"),
                                    Err(e) => println!("  {label}: <decode error {e:?}>"),
                                },
                                Ok(AttrResp::Status(s)) => println!("  {label}: <status {:?}>", s.status.status),
                                Err(e) => println!("  {label}: <read error {e:?}>"),
                            }
                        }
                        async fn read_u16(
                            ex: &mut rs_matter::transport::exchange::Exchange<'_>,
                            attr: u32,
                            label: &str,
                        ) {
                            match ImClient::read_single_attr(ex, 0, CLUSTER_BASIC_INFO, attr, false).await {
                                Ok(AttrResp::Data(d)) => match d.data.u16() {
                                    Ok(v) => println!("  {label}: {v} ({v:#06x})"),
                                    Err(e) => println!("  {label}: <decode error {e:?}>"),
                                },
                                Ok(AttrResp::Status(s)) => println!("  {label}: <status {:?}>", s.status.status),
                                Err(e) => println!("  {label}: <read error {e:?}>"),
                            }
                        }
                        async fn read_u32(
                            ex: &mut rs_matter::transport::exchange::Exchange<'_>,
                            attr: u32,
                            label: &str,
                        ) {
                            match ImClient::read_single_attr(ex, 0, CLUSTER_BASIC_INFO, attr, false).await {
                                Ok(AttrResp::Data(d)) => match d.data.u32() {
                                    Ok(v) => println!("  {label}: {v} ({v:#010x})"),
                                    Err(e) => println!("  {label}: <decode error {e:?}>"),
                                },
                                Ok(AttrResp::Status(s)) => println!("  {label}: <status {:?}>", s.status.status),
                                Err(e) => println!("  {label}: <read error {e:?}>"),
                            }
                        }

                        println!("device info (BasicInformation cluster, ep 0):");
                        read_str(ex, ATTR_BASIC_VENDOR_NAME, "VendorName").await;
                        read_u16(ex, ATTR_BASIC_VENDOR_ID, "VendorID").await;
                        read_str(ex, ATTR_BASIC_PRODUCT_NAME, "ProductName").await;
                        read_u16(ex, ATTR_BASIC_PRODUCT_ID, "ProductID").await;
                        read_u16(ex, ATTR_BASIC_HARDWARE_VERSION, "HardwareVersion").await;
                        read_str(ex, ATTR_BASIC_HARDWARE_VERSION_STRING, "HardwareVersionString").await;
                        read_u32(ex, ATTR_BASIC_SOFTWARE_VERSION, "SoftwareVersion").await;
                        read_str(ex, ATTR_BASIC_SOFTWARE_VERSION_STRING, "SoftwareVersionString").await;
                        read_str(ex, ATTR_BASIC_SERIAL_NUMBER, "SerialNumber").await;
                    }
                    Op::OtaStatus => {
                        // OtaSoftwareUpdateRequestor lives on endpoint 0.
                        let _ = endpoint;

                        // UpdatePossible (bool, attr 1)
                        match ImClient::read_single_attr(ex, 0, CLUSTER_OTA_REQUESTOR, ATTR_OTA_UPDATE_POSSIBLE, false).await {
                            Ok(AttrResp::Data(d)) => match d.data.bool() {
                                Ok(b) => println!("UpdatePossible: {b}"),
                                Err(e) => println!("UpdatePossible: <decode error {e:?}>"),
                            },
                            Ok(AttrResp::Status(s)) => {
                                if matches!(
                                    s.status.status,
                                    IMStatusCode::UnsupportedCluster | IMStatusCode::UnsupportedEndpoint
                                ) {
                                    println!("OTA cluster not supported by device — vendor uses a proprietary update channel (no Matter OTA).");
                                    return Ok::<(), MatterFabricError>(());
                                }
                                println!("UpdatePossible: <status {:?}>", s.status.status);
                            }
                            Err(e) => println!("UpdatePossible: <read error {e:?}>"),
                        }

                        // UpdateState (enum8, attr 2)
                        match ImClient::read_single_attr(ex, 0, CLUSTER_OTA_REQUESTOR, ATTR_OTA_UPDATE_STATE, false).await {
                            Ok(AttrResp::Data(d)) => match d.data.u8() {
                                Ok(v) => {
                                    let label = match v {
                                        0 => "Unknown",
                                        1 => "Idle",
                                        2 => "Querying",
                                        3 => "DelayedOnQuery",
                                        4 => "Downloading",
                                        5 => "Applying",
                                        6 => "DelayedOnApply",
                                        7 => "RollingBack",
                                        8 => "DelayedOnUserConsent",
                                        _ => "?",
                                    };
                                    println!("UpdateState: {v} ({label})");
                                }
                                Err(e) => println!("UpdateState: <decode error {e:?}>"),
                            },
                            Ok(AttrResp::Status(s)) => println!("UpdateState: <status {:?}>", s.status.status),
                            Err(e) => println!("UpdateState: <read error {e:?}>"),
                        }

                        // UpdateStateProgress (u8 percent, attr 3, optional)
                        match ImClient::read_single_attr(ex, 0, CLUSTER_OTA_REQUESTOR, ATTR_OTA_UPDATE_STATE_PROGRESS, false).await {
                            Ok(AttrResp::Data(d)) => match d.data.u8() {
                                Ok(v) => println!("UpdateStateProgress: {v}%"),
                                Err(_) => println!("UpdateStateProgress: <null/unset>"),
                            },
                            Ok(AttrResp::Status(s)) => {
                                if matches!(s.status.status, IMStatusCode::UnsupportedAttribute) {
                                    println!("UpdateStateProgress: <attr not supported>");
                                } else {
                                    println!("UpdateStateProgress: <status {:?}>", s.status.status);
                                }
                            }
                            Err(e) => println!("UpdateStateProgress: <read error {e:?}>"),
                        }
                    }
                    Op::RapidCycle => {
                        // Drive an AiDot-class smart bulb's reset pattern by
                        // toggling the upstream plug rapidly over a single
                        // CASE session. Each invoke_single_cmd reuses the
                        // same secure session, so the only round-trip cost
                        // per command is the IM exchange — no fresh CASE.
                        // Default 5 cycles × 600 ms off / 600 ms on matches
                        // the AiDot factory-reset window most closely.
                        let empty_payload: Vec<u8> = vec![0x15, 0x18];
                        for cycle in 1..=rapid_cycles {
                            // OFF
                            let resp = ImClient::invoke_single_cmd(
                                ex,
                                endpoint,
                                CLUSTER_ON_OFF,
                                CMD_OFF,
                                TLVElement::new(&empty_payload),
                                None,
                            )
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!(
                                "rapid-cycle off ({cycle}/{rapid_cycles}): {e:?}"
                            )))?;
                            match resp {
                                CmdResp::Status(s) if s.status.status != IMStatusCode::Success => {
                                    println!("✗ cycle {cycle} OFF returned {:?}", s.status);
                                }
                                _ => {}
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(rapid_off_ms)).await;

                            // ON
                            let resp = ImClient::invoke_single_cmd(
                                ex,
                                endpoint,
                                CLUSTER_ON_OFF,
                                CMD_ON,
                                TLVElement::new(&empty_payload),
                                None,
                            )
                            .await
                            .map_err(|e| MatterFabricError::Matter(format!(
                                "rapid-cycle on ({cycle}/{rapid_cycles}): {e:?}"
                            )))?;
                            match resp {
                                CmdResp::Status(s) if s.status.status != IMStatusCode::Success => {
                                    println!("✗ cycle {cycle} ON returned {:?}", s.status);
                                }
                                _ => {}
                            }
                            println!("✓ cycle {cycle}/{rapid_cycles} (off {rapid_off_ms}ms, on {rapid_on_ms}ms)");
                            // Hold the ON state before next OFF (skip on the
                            // last cycle — caller wants the device powered on).
                            if cycle < rapid_cycles {
                                tokio::time::sleep(std::time::Duration::from_millis(rapid_on_ms)).await;
                            }
                        }
                    }
                    Op::Caps => {
                        // Walks Descriptor on every endpoint and renders
                        // a human-friendly capability summary. The
                        // discovery + rendering live in
                        // syntaur_matter_ble::caps so the gateway driver
                        // can call the same code on commission and
                        // persist the resulting `DeviceCapabilities` as
                        // JSON for the Smart Home tile UI + agent tool
                        // surface filtering.
                        let caps = syntaur_matter_ble::discover_capabilities(ex).await;
                        print!("{}", caps.render_human());
                    }
                }
                    Ok::<(), MatterFabricError>(())
                })
            },
        )
        .await;

        match result {
            Ok(()) => { last_err = None; break; }
            Err(syntaur_matter::error::MatterFabricError::Matter(ref msg))
                if (msg.contains("timed out") || msg.contains("Error::Invalid"))
                    && attempt < max_attempts =>
            {
                // Retry on:
                //   - "timed out": pre-9.5.33 Meross Sigma1→2 silent-drop
                //     bug. Acked Sigma1, never sends Sigma2.
                //   - "Error::Invalid": post-9.5.33 Meross transient where
                //     a delayed Sigma3 ack triggers our retransmit, then
                //     Meross's CASE responder replies with a Status Report
                //     code 2 (NoSharedTrustRoots). Looks like a session-
                //     table eviction collision while Meross is also
                //     handling its cloud session. Recoverable on retry.
                last_err = Some(format!("CASE op: {msg}"));
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => {
                last_err = Some(format!("CASE op: {e:?}"));
                break;
            }
        }
    }
    if let Some(msg) = last_err {
        return Err(msg);
    }

    Ok(())
}
