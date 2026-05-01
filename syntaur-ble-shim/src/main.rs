//! syntaur-ble-shim — claim a Linux BlueZ Bluetooth adapter, scan for BLE
//! advertisements, and expose them over the ESPHome native-API protocol so
//! multiple consumers (Home Assistant + Syntaur, today; whatever lives on the
//! same box tomorrow) can read the same adapter without fighting over it.
//!
//! Same binary ships in two packagings:
//!   - HAOS Add-on (Alpine container, BlueZ via host_dbus)
//!   - systemd unit on a vanilla Linux host (BlueZ via system DBus)

use std::sync::Arc;

use clap::Parser;
use tokio::sync::broadcast;

mod codec;
mod config;
mod mdns;
mod protocol;
mod scanner;
mod server;

use config::{Cli, Config};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let cfg = Config::resolve(cli)?;
    log::info!(
        "[main] starting syntaur-ble-shim {} (bind={}, name=\"{}\", area=\"{}\")",
        VERSION,
        cfg.bind,
        cfg.name,
        cfg.suggested_area
    );

    // Single broadcast channel; every connection subscribes its own receiver.
    // Capacity 1024 absorbs short bursts in dense BLE environments without
    // pinning real memory (each item is small).
    let (advert_tx, _) = broadcast::channel(1024);

    let scanner_info = scanner::start(advert_tx.clone()).await?;
    let bt_mac = scanner_info.mac.clone();
    log::info!("[main] scanner up; bluetooth mac = {bt_mac}");

    let mdns_handle = mdns::announce(&cfg.name, parse_port(&cfg.bind)?, &bt_mac, VERSION)?;

    let server_cfg = Arc::new(server::ServerConfig {
        bind_addr: cfg.bind.clone(),
        friendly_name: cfg.name.clone(),
        mac_address: bt_mac.clone(),
        bluetooth_mac_address: bt_mac.clone(),
        suggested_area: cfg.suggested_area.clone(),
        version: VERSION.to_string(),
    });

    let server_task = tokio::spawn(server::run(server_cfg, advert_tx.clone()));

    // Graceful shutdown on SIGINT/SIGTERM.
    let shutdown = wait_for_shutdown();

    tokio::select! {
        res = server_task => {
            match res {
                Ok(Ok(())) => log::info!("[main] server exited cleanly"),
                Ok(Err(e)) => log::error!("[main] server error: {e}"),
                Err(e) => log::error!("[main] server task join error: {e}"),
            }
        }
        _ = shutdown => {
            log::info!("[main] shutdown signal received, stopping mDNS + closing");
        }
    }

    drop(mdns_handle);
    Ok(())
}

fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    // Include mdns_sd at the same verbosity so we can tell whether it's announcing
    // (silent failures from interface autodetection are otherwise invisible).
    let filter = format!("syntaur_ble_shim={lvl},btleplug=info", lvl = level);
    let env = env_logger::Env::default().default_filter_or(filter);
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        .init();
}

fn parse_port(bind: &str) -> Result<u16, String> {
    bind.rsplit(':')
        .next()
        .ok_or_else(|| format!("bind addr missing port: {bind}"))?
        .parse::<u16>()
        .map_err(|e| format!("bind port: {e}"))
}

#[cfg(unix)]
async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[main] cannot install SIGTERM handler: {e} — falling back to SIGINT only");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = sigterm.recv() => log::info!("[main] SIGTERM"),
        _ = sigint.recv() => log::info!("[main] SIGINT"),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
