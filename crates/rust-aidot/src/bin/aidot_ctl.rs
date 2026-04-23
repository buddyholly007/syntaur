//! Thin CLI around rust-aidot — reads the inventory JSON + runs one action.
//!
//! Examples:
//!   aidot-ctl list
//!   aidot-ctl on  "Office Light 2"
//!   aidot-ctl off "Office Light 2"
//!   aidot-ctl dim "Office Light 2" 50
//!   aidot-ctl rgbw "Office Light 2" 255 0 0 0
//!   aidot-ctl status "Office Light 2"
//!
//! The one-time cloud harvest lives in the separate `rust-aidot-harvest`
//! crate (workspace-excluded to keep `rsa` out of the main lockfile):
//!
//!   cd crates/rust-aidot-harvest && cargo run --release -- <email> <password>
//!
//! Looks up the device's IP from the harvested inventory's `properties.ipAddress`.
//! Inventory path override: `$AIDOT_INVENTORY`; default `~/.syntaur/aidot_inventory.json`.

use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Context};
use rust_aidot::{DeviceClient, Inventory, InventoryDevice};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    if matches!(argv.as_slice(), ["harvest", ..]) {
        eprintln!(
            "[aidot-ctl] `harvest` was moved to a separate binary to keep the \
             runtime build free of the `rsa` crate (RUSTSEC-2023-0071).\n\
             \n\
             Build + run it with:\n\
               cd crates/rust-aidot-harvest && cargo run --release -- <email> <password>"
        );
        std::process::exit(2);
    }

    let inventory = load_inventory()?;

    match argv.as_slice() {
        ["list"] => {
            for d in &inventory.devices {
                println!(
                    "{:30} mac={} model={} online={} ip={}",
                    d.name,
                    d.mac,
                    d.model_id,
                    d.online,
                    d.last_known_ip().unwrap_or_else(|| "-".into()),
                );
            }
        }
        ["on", name] => run(&inventory, name, |c| Box::pin(async move { c.turn_on().await })).await?,
        ["off", name] => run(&inventory, name, |c| Box::pin(async move { c.turn_off().await })).await?,
        ["dim", name, pct] => {
            let p: u8 = pct.parse().context("dim level must be 0..=100")?;
            run(&inventory, name, move |c| {
                Box::pin(async move { c.set_dimming(p).await })
            })
            .await?
        }
        ["rgbw", name, r, g, b, w] => {
            let r: u8 = r.parse()?;
            let g: u8 = g.parse()?;
            let b: u8 = b.parse()?;
            let w: u8 = w.parse()?;
            run(&inventory, name, move |c| {
                Box::pin(async move { c.set_rgbw(r, g, b, w).await })
            })
            .await?
        }
        _ => {
            eprintln!("usage: aidot-ctl list|on|off|dim|rgbw  <device_name> [args...]");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn inventory_path() -> PathBuf {
    if let Ok(p) = env::var("AIDOT_INVENTORY") {
        PathBuf::from(p)
    } else {
        let home = env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
        PathBuf::from(home).join(".syntaur").join("aidot_inventory.json")
    }
}

fn load_inventory() -> anyhow::Result<Inventory> {
    let path = inventory_path();
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading inventory {}", path.display()))?;
    let inv: Inventory = serde_json::from_slice(&bytes).context("parsing inventory JSON")?;
    Ok(inv)
}

fn find_device<'a>(inv: &'a Inventory, name: &str) -> anyhow::Result<&'a InventoryDevice> {
    inv.devices
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| anyhow!("no device named {name:?} — try `aidot-ctl list`"))
}

async fn run<F>(inv: &Inventory, name: &str, f: F) -> anyhow::Result<()>
where
    F: for<'c> FnOnce(
        &'c mut DeviceClient,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), rust_aidot::AidotError>> + 'c>,
    >,
{
    let dev = find_device(inv, name)?;
    let ip = dev
        .last_known_ip()
        .ok_or_else(|| anyhow!("device has no cached IP in properties.ipAddress"))?;
    let mut client = DeviceClient::connect(dev.clone(), inv.user_id.clone(), &ip)
        .await
        .context("connect + login")?;
    f(&mut client).await.context("action")?;
    println!("{name} -> ok");
    Ok(())
}
