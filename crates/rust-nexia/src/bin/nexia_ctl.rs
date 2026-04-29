//! `nexia-ctl list | show | setpoint | mode | fan | hold | resume | em_heat`
//!
//! Credentials via env: NEXIA_EMAIL, NEXIA_PASSWORD, NEXIA_BRAND (trane|nexia|asair).

use std::env;

use anyhow::{anyhow, Context};
use rust_nexia::{Brand, FanMode, HvacMode, NexiaClient, RunMode};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let email = env::var("NEXIA_EMAIL").context("NEXIA_EMAIL env required")?;
    let password = env::var("NEXIA_PASSWORD").context("NEXIA_PASSWORD env required")?;
    let brand = match env::var("NEXIA_BRAND").unwrap_or_else(|_| "trane".into()).as_str() {
        "trane" => Brand::Trane,
        "nexia" => Brand::Nexia,
        "asair" => Brand::Asair,
        other => return Err(anyhow!("unknown brand: {other}")),
    };
    let mut client = NexiaClient::new(brand);
    client.login(&email, &password).await.context("login")?;

    let therms = client.list_thermostats().await.context("list_thermostats")?;
    if therms.is_empty() {
        println!("(no thermostats on this account)");
        return Ok(());
    }

    match argv.as_slice() {
        ["list"] | [] => {
            for t in &therms {
                println!("{} ({})", t.name, t.model.as_deref().unwrap_or("?"));
                println!("  manufacturer   = {}", t.manufacturer.as_deref().unwrap_or("?"));
                println!("  firmware       = {}", t.firmware_version.as_deref().unwrap_or("?"));
                println!("  system_status  = {}", t.system_status);
                println!("  indoor_hum     = {}", fmt_opt(t.indoor_humidity));
                println!("  outdoor_temp   = {}", fmt_opt(t.outdoor_temperature));
                println!(
                    "  compressor     = {}  (0.0..=1.0; float = variable-speed)",
                    fmt_opt(t.compressor_speed)
                );
                println!("  mode           = {}", t.mode.as_deref().unwrap_or("?"));
                println!("  fan_mode       = {}", t.fan_mode.as_deref().unwrap_or("?"));
                for (i, z) in t.zones.iter().enumerate() {
                    println!("  zone[{i}] id={}: temp={} heat={} cool={} state={}",
                        z.id,
                        fmt_opt(z.temperature),
                        fmt_opt(z.heat_setpoint),
                        fmt_opt(z.cool_setpoint),
                        z.operating_state.as_deref().unwrap_or("?"));
                }
            }
        }
        ["show", zone_id_str] => {
            let zid: u64 = zone_id_str.parse().context("zone id must be u64")?;
            let z = find_zone(&therms, zid)?;
            println!("{:#?}", z);
        }
        ["setpoint", zone_id_str, "heat", v] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            client.set_setpoint(&z, Some(v.parse()?), None).await?;
            println!("heat setpoint -> {v}");
        }
        ["setpoint", zone_id_str, "cool", v] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            client.set_setpoint(&z, None, Some(v.parse()?)).await?;
            println!("cool setpoint -> {v}");
        }
        ["mode", zone_id_str, m] => {
            let zid: u64 = zone_id_str.parse()?;
            let z = find_zone(&therms, zid).cloned()?;
            let mode = match m.to_ascii_uppercase().as_str() {
                "HEAT" => HvacMode::Heat,
                "COOL" => HvacMode::Cool,
                "AUTO" => HvacMode::Auto,
                "OFF" => HvacMode::Off,
                other => return Err(anyhow!("unknown mode: {other}")),
            };
            client.set_mode(&z, mode).await?;
            println!("mode -> {}", mode.as_str());
        }
        ["fan", zone_id_str, f] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            let fm = match f.to_ascii_lowercase().as_str() {
                "auto" => FanMode::Auto,
                "on" => FanMode::On,
                "circulate" | "circ" => FanMode::Circulate,
                other => return Err(anyhow!("unknown fan mode: {other}")),
            };
            client.set_fan_mode(&z, fm).await?;
            println!("fan_mode -> {}", fm.as_str());
        }
        ["hold", zone_id_str] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            client.set_run_mode(&z, RunMode::PermanentHold).await?;
            println!("run_mode -> permanent_hold");
        }
        ["resume", zone_id_str] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            client.set_run_mode(&z, RunMode::Schedule).await?;
            println!("run_mode -> run_schedule");
        }
        ["em_heat", zone_id_str, onoff] => {
            let z = find_zone(&therms, zone_id_str.parse()?).cloned()?;
            let on = matches!(onoff.to_ascii_lowercase().as_str(), "on" | "true" | "1");
            client.set_emergency_heat(&z, on).await?;
            println!("emergency_heat -> {}", if on { "on" } else { "off" });
        }
        _ => {
            eprintln!(
                "usage: nexia-ctl [list|show <zid>|setpoint <zid> (heat|cool) <val>|mode <zid> (HEAT|COOL|AUTO|OFF)|fan <zid> (auto|on|circulate)|hold <zid>|resume <zid>|em_heat <zid> (on|off)]"
            );
            std::process::exit(1);
        }
    }

    // Nudge — for any write op, swallow the reply so the caller sees a clean exit.
    std::mem::drop(therms);
    Ok(())
}

fn fmt_opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
}

fn find_zone<'a>(
    therms: &'a [rust_nexia::Thermostat],
    zid: u64,
) -> anyhow::Result<&'a rust_nexia::Zone> {
    for t in therms {
        for z in &t.zones {
            if z.id == zid {
                return Ok(z);
            }
        }
    }
    Err(anyhow!("zone {zid} not found in any thermostat"))
}
