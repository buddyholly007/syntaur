#!/usr/bin/env bash
# Install syntaur-ble-shim as a systemd service on Debian/Ubuntu.
#
# Idempotent: re-running upgrades the binary + unit in place.
# Usage:
#   sudo ./install.sh            # uses ./syntaur-ble-shim binary in this dir
#   sudo BIN=/path/to/binary ./install.sh

set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
    echo "error: run as root (sudo $0)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
BIN="${BIN:-${SCRIPT_DIR}/syntaur-ble-shim}"
UNIT_SRC="${SCRIPT_DIR}/syntaur-ble-shim.service"

if [[ ! -x "${BIN}" ]]; then
    echo "error: ${BIN} is not executable. Build it first:" >&2
    echo "       cargo build --release -p syntaur-ble-shim" >&2
    echo "       cp target/release/syntaur-ble-shim ${SCRIPT_DIR}/" >&2
    exit 1
fi

# Required runtime deps.
echo ">>> Installing runtime dependencies (libdbus, bluez, libsystemd)…"
apt-get update -qq
apt-get install -y --no-install-recommends \
    libdbus-1-3 \
    bluez \
    libsystemd0 \
    ca-certificates

# Service account.
if ! id syntaur-ble-shim &> /dev/null; then
    echo ">>> Creating syntaur-ble-shim user…"
    useradd \
        --system \
        --no-create-home \
        --home /var/lib/syntaur-ble-shim \
        --shell /usr/sbin/nologin \
        --groups bluetooth \
        syntaur-ble-shim
else
    # Ensure group membership is current (bluetooth group may not have existed before).
    usermod -aG bluetooth syntaur-ble-shim
fi

# Binary + state + config dirs.
echo ">>> Installing binary to /usr/local/bin/syntaur-ble-shim"
install -m 0755 -o root -g root "${BIN}" /usr/local/bin/syntaur-ble-shim

mkdir -p /etc/syntaur-ble-shim
chmod 0755 /etc/syntaur-ble-shim
mkdir -p /var/lib/syntaur-ble-shim
chown syntaur-ble-shim:syntaur-ble-shim /var/lib/syntaur-ble-shim
chmod 0750 /var/lib/syntaur-ble-shim

# Default config (only if absent — we don't clobber on re-install).
if [[ ! -f /etc/syntaur-ble-shim/config.toml ]]; then
    cat > /etc/syntaur-ble-shim/config.toml <<'EOF'
# syntaur-ble-shim runtime config. CLI flags + env vars override anything here.
# bind = "0.0.0.0:6053"
# name = "ha-mini-ble-shim"
# suggested_area = "Office"
EOF
    chmod 0644 /etc/syntaur-ble-shim/config.toml
fi

echo ">>> Installing systemd unit"
install -m 0644 -o root -g root "${UNIT_SRC}" /etc/systemd/system/syntaur-ble-shim.service
systemctl daemon-reload
systemctl enable --now syntaur-ble-shim.service

# Health probe.
sleep 2
if systemctl is-active --quiet syntaur-ble-shim.service; then
    echo
    echo "✓ syntaur-ble-shim is active."
    echo "  status:  systemctl status syntaur-ble-shim"
    echo "  logs:    journalctl -u syntaur-ble-shim -f"
    echo "  config:  /etc/syntaur-ble-shim/config.toml"
    echo "  port:    tcp/6053  (mDNS _esphomelib._tcp)"
else
    echo
    echo "✗ syntaur-ble-shim failed to start. Last 20 log lines:" >&2
    journalctl -u syntaur-ble-shim --no-pager -n 20 >&2
    exit 1
fi
