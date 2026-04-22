//! `matter-pair` — decode a Matter QR code or manual pairing code.
//!
//! Subcommands:
//!   qr <MT:...>         Decode a QR text payload
//!   code <digits>       Decode an 11- or 21-digit manual pairing code (dashes OK)

use std::env;

use anyhow::{anyhow, Context};
use syntaur_matter::{parse_manual_code, parse_qr, PairingPayload};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let payload: PairingPayload = match argv.as_slice() {
        ["qr", s] => parse_qr(s).context("QR parse")?,
        ["code", s] => parse_manual_code(s).context("manual pairing code parse")?,
        _ => {
            eprintln!("usage: matter-pair qr <MT:...> | code <digits>");
            return Err(anyhow!("bad args"));
        }
    };

    println!("version              = {}", payload.version);
    println!(
        "vendor_id            = {}",
        payload
            .vendor_id
            .map(|v| format!("{v:#06x}"))
            .unwrap_or_else(|| "(not in payload)".into())
    );
    println!(
        "product_id           = {}",
        payload
            .product_id
            .map(|v| format!("{v:#06x}"))
            .unwrap_or_else(|| "(not in payload)".into())
    );
    println!("commissioning_flow   = {:?}", payload.commissioning_flow);
    println!(
        "rendezvous           = {:#04x} {}",
        payload.rendezvous,
        describe_rendezvous(payload.rendezvous)
    );
    if payload.discriminator_short {
        println!(
            "discriminator        = {:#05x} (short form; match upper 4 bits only)",
            payload.discriminator
        );
    } else {
        println!("discriminator        = {:#05x} (12-bit, exact match)", payload.discriminator);
    }
    println!("passcode             = {}", payload.passcode);
    Ok(())
}

fn describe_rendezvous(r: u8) -> String {
    let mut parts = Vec::new();
    if r & 0x1 != 0 {
        parts.push("SoftAP");
    }
    if r & 0x2 != 0 {
        parts.push("BLE");
    }
    if r & 0x4 != 0 {
        parts.push("OnNetwork");
    }
    if r & 0x8 != 0 {
        parts.push("WiFi-PAF");
    }
    if r & 0x10 != 0 {
        parts.push("NFC");
    }
    if parts.is_empty() {
        "(none set)".into()
    } else {
        format!("({})", parts.join("|"))
    }
}
