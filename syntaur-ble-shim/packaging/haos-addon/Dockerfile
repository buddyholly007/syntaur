# syntaur-ble-shim HAOS Add-on
# Debian-slim base — btleplug links libdbus-1.so.3 + libsystemd.so.0, both
# trivially available on Debian. Alpine would force a musl rebuild + alpine
# eudev/elogind shimming; not worth the churn for a tool that runs once
# per house.
ARG BUILD_FROM=ghcr.io/hassio-addons/debian-base:7.2.0
FROM ${BUILD_FROM}

ENV LANG C.UTF-8

# Runtime deps for btleplug:
#   dbus, libdbus-1-3      → BlueZ adapter D-Bus access
#   libsystemd0            → dbus-rs's sd_bus client transitively links it
#   bluez                  → the actual BlueZ daemon (HAOS hands one through host_dbus,
#                            but having the client tools here helps debug)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        dbus \
        libdbus-1-3 \
        libsystemd0 \
        bluez \
        jq \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Pre-built x86-64 binary copied in by the build pipeline:
#   cp ~/openclaw-workspace/target/release/syntaur-ble-shim \
#      ~/openclaw-workspace/syntaur-ble-shim/packaging/haos-addon/syntaur-ble-shim
COPY syntaur-ble-shim /usr/local/bin/syntaur-ble-shim
COPY run.sh /run.sh
RUN chmod +x /usr/local/bin/syntaur-ble-shim /run.sh

LABEL \
    io.hass.name="Syntaur BLE Shim" \
    io.hass.description="Multi-subscriber BLE adapter shim (ESPHome native API)" \
    io.hass.arch="amd64|aarch64" \
    io.hass.type="addon" \
    io.hass.version="0.5.1"

ENTRYPOINT [ "/run.sh" ]
