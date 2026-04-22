//! `nexia-probe` — reconnaissance CLI for the Trane/Nexia cloud API.
//!
//! Logs in, then walks the device tree looking for LAN-adjacent fields
//! (local IP, MAC, Z-Wave info, Matter capability, OTA status) that
//! could let us bypass the cloud for read/control paths.
//!
//! Usage:
//!   nexia-probe                        # full recon report → stdout
//!   nexia-probe --raw                  # dump full device JSON instead
//!   nexia-probe --endpoint /mobile/... # GET arbitrary path
//!
//! Credentials via env: NEXIA_EMAIL, NEXIA_PASSWORD, NEXIA_BRAND (trane|nexia|asair).

use std::env;

use anyhow::{anyhow, Context};
use rust_nexia::{Brand, NexiaClient};
use serde_json::Value;

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
    eprintln!("[probe] logging in to {}...", brand.root_url());
    client.login(&email, &password).await.context("login")?;
    eprintln!(
        "[probe] login OK: mobile_id={:?}, api_key acquired={}",
        client.mobile_id(),
        client.api_key_present()
    );

    // Match --endpoint for arbitrary probes
    if let ["--endpoint", path] = argv.as_slice() {
        let v = client.get_raw(path).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    // POST /mobile/session to discover the house_id (per python-nexia's
    // `_find_house_id` flow — Nexia's API is hypermedia-ish and the
    // houses list is behind a session-bootstrap POST, not a plain GET).
    let session_body = serde_json::json!({
        "app_version": rust_nexia::APP_VERSION,
        "device_uuid": uuid::Uuid::new_v4().to_string(),
    });
    let session = client.post_raw("/mobile/session", session_body).await?;
    let house = session
        .pointer("/result/_links/child/0/data")
        .ok_or_else(|| anyhow!("session response missing /result/_links/child/0/data"))?;
    let house_id = house
        .get("id")
        .and_then(|v| v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(String::from)))
        .ok_or_else(|| anyhow!("house data missing id"))?;
    let house_name = house.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    eprintln!("[probe] house: id={house_id}  name={house_name:?}");

    let house_full = client
        .get_raw(&format!("/mobile/houses/{house_id}"))
        .await?;
    if argv == ["--raw"] {
        println!("{}", serde_json::to_string_pretty(&house_full)?);
        return Ok(());
    }

    recon_report(&client, &house_full).await?;
    Ok(())
}

/// Walk the device tree + probe for LAN-adjacent fields.
async fn recon_report(client: &NexiaClient, house_full: &Value) -> anyhow::Result<()> {
    println!("\n=== top-level house metadata ===");
    for k in ["id", "name", "timezone", "postal_code", "state", "country"] {
        if let Some(v) = house_full.pointer(&format!("/result/{k}")) {
            println!("  {k} = {}", preview(v));
        }
    }

    // The full device tree hangs off /result/_links/child
    let children = house_full
        .pointer("/result/_links/child")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    eprintln!(
        "\n[probe] walking {} top-level child links of the house response",
        children.len()
    );
    for (i, child) in children.iter().enumerate() {
        let href = child.pointer("/href").and_then(|v| v.as_str()).unwrap_or("(none)");
        eprintln!("  [child {i}] href = {href}");
        walk_device_tree(child, 1);
    }

    // Known probe endpoints to test on a live account — worth checking
    // whether any of these still exist and what they expose.
    eprintln!("\n[probe] probing known / speculative endpoints");
    for (label, path) in [
        ("mobile/phones", "/mobile/phones"),
        ("mobile/tiers", "/mobile/tiers"),
        ("mobile/account", "/mobile/account"),
        ("mobile/accounts", "/mobile/accounts"),
        ("mobile/contractors", "/mobile/contractors"),
        ("mobile/messages", "/mobile/messages"),
        ("mobile/zones", "/mobile/zones"),
        ("mobile/zwave", "/mobile/zwave"),
        ("mobile/zwave/devices", "/mobile/zwave/devices"),
        ("mobile/devices", "/mobile/devices"),
    ] {
        match client.get_raw(path).await {
            Ok(v) => {
                let kind = categorize(&v);
                let short = preview(&v);
                eprintln!("  {label:<28} -> {kind:<10} {short}");
            }
            Err(e) => {
                let msg = format!("{e}");
                let oneline = msg.lines().next().unwrap_or("").chars().take(80).collect::<String>();
                eprintln!("  {label:<28} -> ERR       {oneline}");
            }
        }
    }

    Ok(())
}

/// Recursively scan nodes, printing LAN-relevant fields as we find them.
fn walk_device_tree(node: &Value, depth: usize) {
    let indent = "  ".repeat(depth);
    match node {
        Value::Object(map) => {
            // Surface standard thermostat fields
            let id = map.get("id").and_then(|v| v.as_str()).or_else(|| {
                map.get("id").and_then(|v| v.as_u64()).map(|_| "(int)")
            });
            let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let typ = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !typ.is_empty() || !name.is_empty() || id.is_some() {
                println!(
                    "{indent}{type} {name:?} id={id:?}",
                    type = typ,
                );
            }

            // Scan for LAN-adjacent keys we'd care about
            const INTERESTING_KEYS: &[&str] = &[
                "ip", "ip_address", "lan_ip", "local_ip",
                "mac", "mac_address",
                "zwave", "z_wave", "zwave_id", "z_wave_id", "zwave_node_id",
                "matter", "matter_enabled", "matter_capable", "matter_id",
                "model", "model_number", "serial_number",
                "firmware", "firmware_version", "fw_version",
                "ota", "ota_status", "ota_available",
                "connection_type", "radio", "protocol",
            ];
            for key in INTERESTING_KEYS {
                if let Some(v) = map.get(*key) {
                    println!("{indent}  * {key} = {}", preview(v));
                }
            }
            // Keys containing interesting substrings (catches case variants)
            for (k, v) in map.iter() {
                let lk = k.to_ascii_lowercase();
                if INTERESTING_KEYS.contains(&lk.as_str()) {
                    continue; // already printed
                }
                if lk.contains("zwave")
                    || lk.contains("z_wave")
                    || lk.contains("matter")
                    || lk.contains("radio")
                    || lk.contains("firmware")
                    || (lk.contains("ip") && lk.len() < 20)
                    || lk.contains("mac_address")
                    || lk.contains("ota")
                {
                    println!("{indent}  ~ {k} = {}", preview(v));
                }
            }

            // Follow _links.child arrays into children
            if let Some(links) = map.get("_links") {
                if let Some(children) = links.pointer("/child").and_then(|v| v.as_array()) {
                    for child in children {
                        walk_device_tree(child, depth + 1);
                    }
                }
            }
            // Also follow any nested "features", "zones", "systems" arrays
            for nested_key in ["features", "zones", "systems", "data"] {
                if let Some(child) = map.get(nested_key) {
                    if child.is_array() || child.is_object() {
                        walk_device_tree(child, depth + 1);
                    }
                }
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                if i < 5 || depth < 3 {
                    walk_device_tree(v, depth);
                }
            }
        }
        _ => {}
    }
}

fn categorize(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn preview(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    let s: String = s.chars().take(80).collect();
    if s.len() == 80 {
        format!("{s}…")
    } else {
        s
    }
}
