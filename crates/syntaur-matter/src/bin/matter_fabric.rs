//! `matter-fabric` — Phase 1 CLI for Syntaur-owned Matter fabrics.
//!
//! Subcommands:
//!   new <label>        Generate + save a fresh fabric
//!   list               Show all saved fabrics (summary only, no secrets)
//!   show <label>       Print one fabric's summary
//!   delete <label>     Remove a fabric (no undo — secrets are gone)

use std::env;

use anyhow::{anyhow, Context};
use syntaur_matter::{default_dir, list_fabrics, load_fabric, save_fabric, FabricHandle};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    match argv.as_slice() {
        ["new", label] => {
            // Refuse to clobber an existing fabric silently.
            if load_fabric(label).is_ok() {
                return Err(anyhow!(
                    "fabric {label:?} already exists in {} — delete it first if that's intentional",
                    default_dir().display()
                ));
            }
            let handle = FabricHandle::new(label.to_string())?;
            let path = save_fabric(&handle).context("persisting fabric")?;
            let sum = handle.summary();
            println!("created fabric {path}", path = path.display());
            println!("  label              = {}", sum.label);
            println!("  fabric_id          = {:#018x}", sum.fabric_id);
            println!("  controller_node_id = {}", sum.controller_node_id);
            println!("  vendor_id          = {:#06x}", sum.vendor_id);
            println!("  rcac_fingerprint   = {}", sum.rcac_fingerprint);
            println!("  created_at         = {}", sum.created_at);
        }
        ["list"] => {
            let all = list_fabrics()?;
            if all.is_empty() {
                println!("(no fabrics; create one with: matter-fabric new <label>)");
                return Ok(());
            }
            println!(
                "{:<16} {:>18} {:>6} {:<16} {}",
                "label", "fabric_id", "vendor", "fingerprint", "created"
            );
            for s in &all {
                println!(
                    "{:<16} {:>#18x} {:>#06x} {:<16} {}",
                    s.label,
                    s.fabric_id,
                    s.vendor_id,
                    s.rcac_fingerprint,
                    s.created_at.format("%Y-%m-%d %H:%M"),
                );
            }
        }
        ["show", label] => {
            let handle = load_fabric(label)?;
            let sum = handle.summary();
            println!("{}", serde_json::to_string_pretty(&sum)?);
        }
        ["delete", label] => {
            let dir = default_dir();
            let path = dir.join(format!("{label}.enc"));
            if !path.exists() {
                return Err(anyhow!("no fabric {label:?} at {}", path.display()));
            }
            std::fs::remove_file(&path)?;
            println!("deleted {}", path.display());
        }
        _ => {
            eprintln!("usage: matter-fabric new <label> | list | show <label> | delete <label>");
            std::process::exit(1);
        }
    }
    Ok(())
}
