//! CLI for the rs-matter direct backend.
//!
//! `syntaur-gateway matter-direct <subcommand>` — exercises the
//! `MatterDirectClient` API against a fabric loaded from the path in
//! `SYNTAUR_MATTER_FABRIC_FILE` (or `--fabric <path>`).
//!
//! Designed for hardware smoke-testing the upstream-rs-matter cutover:
//! Sean runs this against a single bulb to validate end-to-end pure-Rust
//! Matter operation, side-by-side with the existing python-matter-server
//! bridge (which stays running for everything else).
//!
//! ## Subcommands
//!
//! ```text
//! syntaur-gateway matter-direct list                        # enumerate paired devices
//! syntaur-gateway matter-direct on <node_id>                # OnOff cluster, command 0x01
//! syntaur-gateway matter-direct off <node_id>               # OnOff cluster, command 0x00
//! syntaur-gateway matter-direct level <node_id> <0..=254>   # LevelControl, MoveToLevel
//! syntaur-gateway matter-direct read-on-off <node_id>       # OnOff cluster, attr 0
//! ```
//!
//! ## Flags
//!
//! ```text
//! --fabric <path>     # override SYNTAUR_MATTER_FABRIC_FILE
//! --json              # machine-readable output
//! ```
//!
//! Exit codes: 0 = success, 1 = usage error, 2 = backend error
//! (`DirectError`), 3 = device unreachable.

use crate::tools::matter_direct::{DirectError, MatterDirectClient};

#[derive(Debug)]
struct CliArgs {
    fabric_override: Option<String>,
    json: bool,
    subcommand: Subcommand,
}

#[derive(Debug)]
enum Subcommand {
    List,
    On(u64),
    Off(u64),
    Level(u64, u8),
    ReadOnOff(u64),
    /// import-pms <pms-storage-dir>
    /// Read python-matter-server storage, dump a SyntaurFabricFile JSON
    /// to stdout. Does NOT talk to the network — pure file parsing.
    ImportPms(String),
    /// validate-fabric <syntaur-fabric-file-path>
    /// Load a SyntaurFabricFile from disk and print a summary of the
    /// cert + key lengths. No network — catches format errors before
    /// you try to use the fabric for real ops.
    ValidateFabric(String),
}

fn parse_args(argv: &[String]) -> Result<CliArgs, String> {
    let mut fabric_override = None;
    let mut json = false;
    let mut positional: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--fabric" => {
                i += 1;
                fabric_override = Some(
                    argv.get(i)
                        .ok_or_else(|| "--fabric needs a value".to_string())?
                        .clone(),
                );
            }
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}"));
            }
            other => positional.push(other),
        }
        i += 1;
    }
    let sub = match positional.as_slice() {
        ["list"] => Subcommand::List,
        ["on", id] => Subcommand::On(parse_node_id(id)?),
        ["off", id] => Subcommand::Off(parse_node_id(id)?),
        ["level", id, lvl] => {
            let n = parse_node_id(id)?;
            let l: u8 = lvl
                .parse()
                .map_err(|_| format!("level must be 0..=254, got {lvl}"))?;
            if l > 254 {
                return Err(format!("level must be 0..=254, got {l}"));
            }
            Subcommand::Level(n, l)
        }
        ["read-on-off", id] => Subcommand::ReadOnOff(parse_node_id(id)?),
        ["import-pms", path] => Subcommand::ImportPms(path.to_string()),
        ["validate-fabric", path] => Subcommand::ValidateFabric(path.to_string()),
        [] => return Err(usage()),
        _ => return Err(format!("unknown subcommand: {}\n\n{}", positional.join(" "), usage())),
    };
    Ok(CliArgs { fabric_override, json, subcommand: sub })
}

fn parse_node_id(s: &str) -> Result<u64, String> {
    if let Some(hex) = s.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|_| format!("invalid hex node_id: {s}"))
    } else {
        s.parse::<u64>()
            .map_err(|_| format!("invalid node_id (decimal or 0x-prefix hex): {s}"))
    }
}

fn usage() -> String {
    "usage: syntaur-gateway matter-direct [--fabric PATH] [--json] <subcommand> [args]\n\n\
     subcommands:\n\
       list                       enumerate paired devices\n\
       on   <node_id>             OnOff cluster, command 0x01\n\
       off  <node_id>             OnOff cluster, command 0x00\n\
       level <node_id> <0..=254>  LevelControl MoveToLevel\n\
       read-on-off <node_id>      OnOff cluster attribute 0\n\
       import-pms <pms-dir>       dump SyntaurFabricFile from python-matter-server storage\n\
       validate-fabric <path>     load + summarize a SyntaurFabricFile\n\n\
     fabric file resolution: --fabric flag, then SYNTAUR_MATTER_FABRIC_FILE env"
        .into()
}

/// Entry point. Called from main.rs when argv[0] == \"matter-direct\".
/// Skips the first element of `raw_args` (\"matter-direct\") before parsing.
pub async fn run(raw_args: &[String]) -> ! {
    let args = match parse_args(&raw_args[1..]) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    if let Some(path) = &args.fabric_override {
        // The MatterDirectClient reads SYNTAUR_MATTER_FABRIC_FILE on
        // construction; honor --fabric by setting the env first.
        std::env::set_var("SYNTAUR_MATTER_FABRIC_FILE", path);
    }
    let client = MatterDirectClient::new();

    let result = match args.subcommand {
        Subcommand::List => list_cmd(&client, args.json).await,
        Subcommand::On(id) => set_on_off_cmd(&client, id, true, args.json).await,
        Subcommand::Off(id) => set_on_off_cmd(&client, id, false, args.json).await,
        Subcommand::Level(id, lvl) => set_level_cmd(&client, id, lvl, args.json).await,
        Subcommand::ReadOnOff(id) => read_on_off_cmd(&client, id, args.json).await,
        Subcommand::ImportPms(path) => import_pms_cmd(&path, args.json),
        Subcommand::ValidateFabric(path) => validate_fabric_cmd(&path, args.json),
    };

    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": false, "error": format!("{e}") })
                );
            } else {
                eprintln!("error: {e}");
            }
            // Distinguish address-cache misses (likely operational mDNS
            // gap) from generic backend errors so scripts can act on it.
            let code = if matches!(
                e,
                DirectError::OperationalMdnsMissing { .. }
            ) {
                3
            } else {
                2
            };
            std::process::exit(code);
        }
    }
}

async fn list_cmd(client: &MatterDirectClient, json: bool) -> Result<(), DirectError> {
    let nodes = client.list_nodes().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
    } else if nodes.is_empty() {
        println!("(no paired devices)");
    } else {
        println!("{:>10}  {:>6}  {:>6}  {:>4}  {:>5}  {}", "node_id", "vendor", "product", "on", "level", "label");
        for n in &nodes {
            println!(
                "{:>10}  {:>6}  {:>6}  {:>4}  {:>5}  {}",
                n.node_id,
                n.vendor_id.map(|v| format!("{v:#06x}")).unwrap_or_else(|| "?".into()),
                n.product_id.map(|v| format!("{v:#06x}")).unwrap_or_else(|| "?".into()),
                match n.on_off { Some(true) => "on", Some(false) => "off", None => "?" },
                n.level.map(|l| l.to_string()).unwrap_or_else(|| "?".into()),
                n.label.as_deref().unwrap_or("(no label)"),
            );
        }
    }
    Ok(())
}

async fn set_on_off_cmd(
    client: &MatterDirectClient,
    node_id: u64,
    on: bool,
    json: bool,
) -> Result<(), DirectError> {
    client.set_on_off(node_id, on).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": true, "node_id": node_id, "on": on })
        );
    } else {
        println!("node {node_id} -> {}", if on { "on" } else { "off" });
    }
    Ok(())
}

async fn set_level_cmd(
    client: &MatterDirectClient,
    node_id: u64,
    level: u8,
    json: bool,
) -> Result<(), DirectError> {
    client.set_level(node_id, level).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": true, "node_id": node_id, "level": level })
        );
    } else {
        println!("node {node_id} -> level {level}");
    }
    Ok(())
}

async fn read_on_off_cmd(
    client: &MatterDirectClient,
    node_id: u64,
    json: bool,
) -> Result<(), DirectError> {
    let on = client.read_on_off(node_id).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": true, "node_id": node_id, "on": on })
        );
    } else {
        println!("node {node_id}: {}", if on { "on" } else { "off" });
    }
    Ok(())
}


/// `import-pms <pms-storage-dir>` — parse python-matter-server's
/// `chip.json` + compressed-fabric-id.json, dump a SyntaurFabricFile.
/// Lossy: we drop fabric_label, commissioned_devices, compressed_fabric_id
/// because SyntaurFabricFile is the minimum rs-matter needs to build a
/// FabricMgr. Run with `--output /path/out.json` or redirect stdout.
fn import_pms_cmd(pms_dir: &str, json_output: bool) -> Result<(), DirectError> {
    use crate::tools::matter_fabric_import::{import_from_storage_dir, ImportError};

    let pms_path = std::path::Path::new(pms_dir);
    let imported = import_from_storage_dir(pms_path).map_err(|e: ImportError| {
        DirectError::FabricParseError {
            path: pms_dir.to_string(),
            reason: format!("python-matter-server import: {e}"),
        }
    })?;

    // Controller signing key is stored as a full serialized P-256 keypair
    // in python-matter-server; the trailing 32 bytes are the raw scalar.
    // If the blob isn't a standard 97-byte keypair, caller has to fix up.
    let secret_key = if imported.noc_signing_key_serialized.len() >= 32 {
        let n = imported.noc_signing_key_serialized.len();
        &imported.noc_signing_key_serialized[n - 32..]
    } else {
        return Err(DirectError::FabricParseError {
            path: pms_dir.to_string(),
            reason: format!(
                "noc_signing_key too short: {} bytes (expected >= 32)",
                imported.noc_signing_key_serialized.len()
            ),
        });
    };

    // Required TLV certs — error if absent, can't build a fabric without them.
    let root_tlv = imported.root_ca_cert.tlv.as_ref().ok_or_else(|| {
        DirectError::FabricParseError {
            path: pms_dir.to_string(),
            reason: "root_ca_cert.tlv missing from python-matter-server storage".into(),
        }
    })?;
    let icac_tlv = match imported.icac.as_ref() {
        Some(cb) => cb.tlv.as_ref().map(|v| hex_encode(v)),
        None => None,
    };
    let syntaur_file = serde_json::json!({
        "fabric_id": imported.fabric_id,
        "vendor_id": imported.vendor_id,
        "controller_node_id": imported.node_id,
        "root_cert_hex": hex_encode(root_tlv),
        "noc_hex": hex_encode(&imported.noc),
        "icac_hex": icac_tlv,
        "secret_key_hex": hex_encode(secret_key),
        "ipk_hex": hex_encode(&imported.ipk),
    });

    if json_output {
        println!("{}", syntaur_file);
    } else {
        println!("{}", serde_json::to_string_pretty(&syntaur_file).unwrap());
    }
    Ok(())
}

/// `validate-fabric <path>` — load SyntaurFabricFile, print summary.
/// Does NOT establish any sessions. Catches hex decode + structural
/// errors before they blow up in Stage 2b CASE/IM paths.
fn validate_fabric_cmd(path: &str, json_output: bool) -> Result<(), DirectError> {
    #[derive(serde::Deserialize)]
    struct FabricFile {
        fabric_id: u64,
        vendor_id: u16,
        controller_node_id: u64,
        root_cert_hex: String,
        noc_hex: String,
        icac_hex: Option<String>,
        secret_key_hex: String,
        ipk_hex: String,
    }

    let bytes = std::fs::read(path).map_err(|e| DirectError::FabricParseError {
        path: path.to_string(),
        reason: format!("read: {e}"),
    })?;
    let ff: FabricFile = serde_json::from_slice(&bytes).map_err(|e| {
        DirectError::FabricParseError {
            path: path.to_string(),
            reason: format!("parse: {e}"),
        }
    })?;

    // Decode every hex field, surface exact location on failure.
    let root = hex_decode(&ff.root_cert_hex, "root_cert_hex", path)?;
    let noc = hex_decode(&ff.noc_hex, "noc_hex", path)?;
    let icac = match ff.icac_hex.as_deref() {
        Some(s) => Some(hex_decode(s, "icac_hex", path)?),
        None => None,
    };
    let key = hex_decode(&ff.secret_key_hex, "secret_key_hex", path)?;
    let ipk = hex_decode(&ff.ipk_hex, "ipk_hex", path)?;

    if key.len() != 32 {
        return Err(DirectError::FabricParseError {
            path: path.to_string(),
            reason: format!(
                "secret_key_hex decoded to {} bytes; expected 32 for P-256 scalar",
                key.len()
            ),
        });
    }
    if ipk.len() != 16 {
        return Err(DirectError::FabricParseError {
            path: path.to_string(),
            reason: format!("ipk_hex decoded to {} bytes; expected 16", ipk.len()),
        });
    }

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "fabric_id": ff.fabric_id,
                "vendor_id": ff.vendor_id,
                "controller_node_id": ff.controller_node_id,
                "root_cert_bytes": root.len(),
                "noc_bytes": noc.len(),
                "icac_bytes": icac.as_ref().map(|v| v.len()),
                "secret_key_bytes": key.len(),
                "ipk_bytes": ipk.len(),
            })
        );
    } else {
        println!("Fabric file: {}", path);
        println!("  fabric_id           = {}", ff.fabric_id);
        println!("  vendor_id           = {:#06x}", ff.vendor_id);
        println!("  controller_node_id  = {} ({:#x})", ff.controller_node_id, ff.controller_node_id);
        println!("  root_cert           = {} bytes (TLV)", root.len());
        println!("  noc                 = {} bytes (TLV)", noc.len());
        println!("  icac                = {}", icac.as_ref().map(|v| format!("{} bytes", v.len())).unwrap_or_else(|| "(none)".into()));
        println!("  secret_key          = {} bytes (P-256 scalar)", key.len());
        println!("  ipk                 = {} bytes", ipk.len());
        println!("OK");
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hex_decode(s: &str, field: &str, path: &str) -> Result<Vec<u8>, DirectError> {
    if s.len() % 2 != 0 {
        return Err(DirectError::FabricParseError {
            path: path.to_string(),
            reason: format!("{field}: odd-length hex ({})", s.len()),
        });
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| {
            DirectError::FabricParseError {
                path: path.to_string(),
                reason: format!("{field}: invalid hex at byte {}", i / 2),
            }
        })?;
        out.push(byte);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_list() {
        let a = parse_args(&argv(&["list"])).unwrap();
        assert!(matches!(a.subcommand, Subcommand::List));
        assert!(!a.json);
    }

    #[test]
    fn parses_on_decimal() {
        let a = parse_args(&argv(&["on", "42"])).unwrap();
        assert!(matches!(a.subcommand, Subcommand::On(42)));
    }

    #[test]
    fn parses_on_hex() {
        let a = parse_args(&argv(&["on", "0xDEADBEEF"])).unwrap();
        assert!(matches!(a.subcommand, Subcommand::On(0xDEADBEEF)));
    }

    #[test]
    fn parses_level_with_flags() {
        let a = parse_args(&argv(&["--fabric", "/tmp/f.json", "--json", "level", "7", "200"]))
            .unwrap();
        assert!(matches!(a.subcommand, Subcommand::Level(7, 200)));
        assert!(a.json);
        assert_eq!(a.fabric_override.as_deref(), Some("/tmp/f.json"));
    }

    #[test]
    fn rejects_bad_level() {
        // u8 already caps at 255 — this exercises the >254 check
        let res = parse_args(&argv(&["level", "1", "255"]));
        assert!(res.is_err());
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(parse_args(&argv(&["fly"])).is_err());
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_args(&argv(&["--bogus", "list"])).is_err());
    }
}
