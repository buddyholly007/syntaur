//! `matter-ble-scan` — quick-and-dirty BLE scan for commissionable
//! Matter devices. Useful for verifying your HCI adapter is working
//! AND for cross-checking a QR/manual-code discriminator against what
//! the device is actually advertising.
//!
//! Usage:
//!   matter-ble-scan                      # 15 s, list everything
//!   matter-ble-scan --disc 0xF00         # list only matching discriminator
//!   matter-ble-scan --timeout 30         # extend the scan window

use std::env;
use std::time::Duration;

use anyhow::{anyhow, Context};
use syntaur_matter_ble::scan_for_discriminator;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut want: Option<u16> = None;
    let mut timeout_s: u64 = 15;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--disc" => {
                let v = args.get(i + 1).ok_or_else(|| anyhow!("--disc needs value"))?;
                want = Some(parse_u16(v)?);
                i += 2;
            }
            "--timeout" => {
                let v = args.get(i + 1).ok_or_else(|| anyhow!("--timeout needs value"))?;
                timeout_s = v.parse().context("--timeout must be seconds")?;
                i += 2;
            }
            other => return Err(anyhow!("unknown arg {other}")),
        }
    }
    let devs = scan_for_discriminator(want, Duration::from_secs(timeout_s))
        .await
        .context("BLE scan — does the host have a working HCI adapter?")?;
    if devs.is_empty() {
        println!("(no commissionable Matter devices in range)");
        return Ok(());
    }
    println!(
        "{:<20} {:>6} {:>6} {:>6} {:>5}  name",
        "address", "disc", "vid", "pid", "rssi"
    );
    for d in devs {
        println!(
            "{:<20} {:#05x} {:#06x} {:#06x} {:>5}  {}",
            d.address,
            d.discriminator,
            d.vendor_id,
            d.product_id,
            d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "?".into()),
            d.local_name.as_deref().unwrap_or("(unnamed)")
        );
    }
    Ok(())
}

fn parse_u16(s: &str) -> anyhow::Result<u16> {
    if let Some(hex) = s.strip_prefix("0x") {
        Ok(u16::from_str_radix(hex, 16)?)
    } else {
        Ok(s.parse()?)
    }
}
