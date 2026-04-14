#!/usr/bin/env bash
# Installs syntaur-media-bridge as a user-scope systemd service on Linux,
# or launchd agent on macOS. Run after scp'ing the binary:
#   scp syntaur-media-bridge <user>@<host>:/usr/local/bin/
#   ssh <user>@<host> 'bash install.sh'
set -euo pipefail

BIN_NAME="syntaur-media-bridge"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BIN_PATH="${INSTALL_DIR}/${BIN_NAME}"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "error: $BIN_PATH not found or not executable"
  echo "  scp target/release/${BIN_NAME} to ${INSTALL_DIR} first"
  exit 1
fi

OS="$(uname -s)"
case "$OS" in
  Linux)
    UNIT_DIR="${HOME}/.config/systemd/user"
    mkdir -p "$UNIT_DIR"
    cat > "${UNIT_DIR}/${BIN_NAME}.service" <<EOF
[Unit]
Description=Syntaur Media Bridge (local audio companion)
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=${BIN_PATH}
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable "${BIN_NAME}.service"
    systemctl --user restart "${BIN_NAME}.service"
    echo "✓ ${BIN_NAME} installed as systemd user service"
    echo "  status: systemctl --user status ${BIN_NAME}"
    echo "  logs:   journalctl --user -u ${BIN_NAME} -f"
    ;;
  Darwin)
    AGENT_DIR="${HOME}/Library/LaunchAgents"
    mkdir -p "$AGENT_DIR"
    PLIST="${AGENT_DIR}/com.syntaur.media-bridge.plist"
    cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.syntaur.media-bridge</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BIN_PATH}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${HOME}/Library/Logs/syntaur-media-bridge.log</string>
  <key>StandardErrorPath</key><string>${HOME}/Library/Logs/syntaur-media-bridge.log</string>
  <key>EnvironmentVariables</key>
  <dict><key>RUST_LOG</key><string>info</string></dict>
</dict>
</plist>
EOF
    launchctl unload "$PLIST" 2>/dev/null || true
    launchctl load "$PLIST"
    echo "✓ ${BIN_NAME} installed as launchd agent"
    echo "  logs: ~/Library/Logs/syntaur-media-bridge.log"
    ;;
  *)
    echo "error: unsupported OS: $OS"
    echo "  run manually: ${BIN_PATH}"
    exit 1
    ;;
esac

echo
echo "Next: log in to each music service once so cookies get cached:"
echo "  ${BIN_NAME} --auth-setup --auth-provider apple_music"
echo "  ${BIN_NAME} --auth-setup --auth-provider spotify"
echo "  ${BIN_NAME} --auth-setup --auth-provider tidal"
echo "  ${BIN_NAME} --auth-setup --auth-provider youtube_music"
