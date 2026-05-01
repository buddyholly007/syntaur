# Syntaur BLE Shim — systemd packaging

This is the path forward for the day HAOS comes off the box. Same binary, same protocol, same mDNS advert — different supervisor.

## Install

```bash
cd ~/openclaw-workspace
cargo build --release -p syntaur-ble-shim

# Drop the binary next to the install script (or set BIN= when calling install.sh)
cp target/release/syntaur-ble-shim syntaur-ble-shim/packaging/systemd/

# Copy this directory to the target host
scp -r syntaur-ble-shim/packaging/systemd/ user@target-host:/tmp/

# On the target host
ssh user@target-host
cd /tmp/systemd
sudo ./install.sh
```

What it does, in order:
1. `apt-get install` libdbus-1-3, bluez, libsystemd0 (idempotent)
2. Create `syntaur-ble-shim` system user in the `bluetooth` group
3. Install binary to `/usr/local/bin/`
4. Drop a default config to `/etc/syntaur-ble-shim/config.toml` (only on first install)
5. Install + enable + start the systemd unit
6. Verify it's running, dump last 20 log lines on failure

## Operate

```bash
systemctl status syntaur-ble-shim
journalctl -u syntaur-ble-shim -f
```

Config: `/etc/syntaur-ble-shim/config.toml`. Restart after editing:

```bash
sudo systemctl restart syntaur-ble-shim
```

## Sandbox

The unit hardens the service:
- runs as a non-root system user
- only `CAP_NET_RAW` + `CAP_NET_ADMIN` (no DAC override)
- `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`
- write access only to its own state dir at `/var/lib/syntaur-ble-shim/`

If BlueZ glitches, `Restart=on-failure` brings it back up to 10× per minute.
