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
       read-on-off <node_id>      OnOff cluster attribute 0\n\n\
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
