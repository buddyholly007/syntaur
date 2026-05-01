# Syntaur BLE Shim — HAOS Add-on

A local-only Add-on that claims the host's Bluetooth adapter via BlueZ and exposes the live advertisement stream over the ESPHome native API protocol on TCP port 6053.

## Why
ESPHome's `bluetooth_proxy` firmware only allows ONE subscriber. The Add-on lifts that wall, so Home Assistant and Syntaur (and anyone else) can each subscribe to the same physical radio in parallel.

It's the same code path as the firmware C++ patch we ship for ESP32-S3 proxies, but in pure Rust on the host instead of on the chip.

## Install (local Add-on)

1. Copy this directory to `/addons/syntaur-ble-shim/` on the HAOS box (e.g. via Samba/SSH).
2. Copy the prebuilt `syntaur-ble-shim` binary alongside the Dockerfile (the build pipeline does this automatically).
3. Settings → Add-ons → ⋮ → Reload (HA picks up the new local Add-on).
4. Open the Add-on, set a friendly `name` and `suggested_area`, hit **Start**.

## After Start

- HA's ESPHome integration should auto-discover the new "device" via mDNS within ~30 s. Click **Configure** → **Submit** (no password needed); it adopts as a regular `bluetooth_proxy`.
- Syntaur's BLE setup wizard (`/smart-home/ble`) should also auto-discover it. Adopt + register as an anchor for the room the box lives in.
- **Disable HA's local Bluetooth integration on this same host.** Two consumers cannot both talk to BlueZ directly; the shim must be the only thing that does. HA continues to get its data — through the shim now — over the ESPHome integration.

## Build

The Dockerfile copies a prebuilt binary, so you must produce one first:

```bash
cd ~/openclaw-workspace
cargo build --release -p syntaur-ble-shim
cp target/release/syntaur-ble-shim syntaur-ble-shim/packaging/haos-addon/
docker build -t syntaur-ble-shim:latest syntaur-ble-shim/packaging/haos-addon
```

## Forward compatibility

The same binary also runs as a vanilla systemd service on Debian/Ubuntu — see `../systemd/`. When you uninstall HAOS from this box, run `install.sh` from there and BLE stays available without losing the Add-on's behaviour.
