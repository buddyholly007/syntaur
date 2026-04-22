//! `kasa-ctl list|state|on|off|brightness|harvest` — thin wrapper that
//! reads `~/.syntaur/kasa_inventory.json`, resolves a device by friendly
//! alias, does the KLAP handshake, runs one operation.
//!
//! Setup: `kasa-ctl harvest <email> <password> <ip1> <ip2> ...` probes each
//! IP, records alias/model/MAC, and writes `~/.syntaur/kasa_inventory.json`.

use std::env;

use anyhow::{anyhow, Context};
use rust_kasa::{Device, Inventory, InventoryDevice};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // `harvest` runs BEFORE the inventory exists.
    if let ["harvest", email, password, ips @ ..] = argv.as_slice() {
        if ips.is_empty() {
            eprintln!("usage: kasa-ctl harvest <email> <password> <ip1> [ip2 ...]");
            std::process::exit(1);
        }
        let ip_vec: Vec<String> = ips.iter().map(|s| s.to_string()).collect();
        eprintln!("[kasa] probing {} IP(s)", ip_vec.len());
        let inv = rust_kasa::harvest_from_ips(email, password, &ip_vec).await?;
        let path = inventory_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let bytes = serde_json::to_vec_pretty(&inv)?;
        std::fs::write(&path, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        eprintln!(
            "[kasa] wrote {} devices to {}",
            inv.devices.len(),
            path.display()
        );
        return Ok(());
    }

    let inv = Inventory::load_default().context("loading kasa inventory")?;

    match argv.as_slice() {
        ["list"] => {
            for d in &inv.devices {
                println!(
                    "{:35} model={:6}  ip={:15}  mac={}",
                    d.alias.trim(),
                    d.model,
                    d.ip,
                    d.mac
                );
            }
        }
        ["state", alias] => {
            let dev = find(&inv, alias)?;
            let mut client = Device::connect(&dev.ip, &inv.username, &inv.password)
                .await
                .context("connect")?;
            let info = client.get_device_info().await.context("get_device_info")?;
            println!(
                "{} [{}]  device_on={:?}  fw={:?}  ip={:?}",
                dev.alias.trim(),
                dev.model,
                info.device_on,
                info.fw_ver,
                info.ip.as_deref().unwrap_or(&dev.ip)
            );
        }
        ["on", alias] => {
            do_action(&inv, alias, |d| Box::pin(async move { d.turn_on().await })).await?
        }
        ["off", alias] => {
            do_action(&inv, alias, |d| Box::pin(async move { d.turn_off().await })).await?
        }
        ["brightness", alias, pct] => {
            let p: u8 = pct.parse().context("brightness must be 0..=100")?;
            do_action(&inv, alias, move |d| {
                Box::pin(async move { d.set_brightness(p).await })
            })
            .await?
        }
        _ => {
            eprintln!("usage: kasa-ctl list|state|on|off|brightness <alias> [pct]");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn inventory_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("KASA_INVENTORY") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home)
        .join(".syntaur")
        .join("kasa_inventory.json")
}

fn find<'a>(inv: &'a Inventory, alias: &str) -> anyhow::Result<&'a InventoryDevice> {
    inv.find_by_alias(alias)
        .ok_or_else(|| anyhow!("no device named {alias:?} — try `kasa-ctl list`"))
}

async fn do_action<F>(inv: &Inventory, alias: &str, f: F) -> anyhow::Result<()>
where
    F: for<'c> FnOnce(
        &'c mut Device,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), rust_kasa::KasaError>> + 'c>,
    >,
{
    let dev = find(inv, alias)?;
    let mut client = Device::connect(&dev.ip, &inv.username, &inv.password)
        .await
        .context("connect")?;
    f(&mut client).await.context("action")?;
    println!("{} -> ok", dev.alias.trim());
    Ok(())
}
