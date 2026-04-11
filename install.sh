#!/bin/sh
# Syntaur installer — https://syntaur.dev
# Usage: curl -sSL https://get.syntaur.dev | sh
set -e

BRAND="Syntaur"
VERSION="0.1.0"
BINARY="syntaur"
INSTALL_DIR="$HOME/.local/bin"

echo ""
echo "  ♞ $BRAND v$VERSION"
echo "  Your personal AI platform"
echo ""

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
  linux)  PLATFORM="linux" ;;
  darwin) PLATFORM="macos" ;;
  *)      echo "Error: Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64|amd64)  ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *)             echo "Error: Unsupported architecture: $ARCH"; exit 1 ;;
esac

echo "  Platform: $PLATFORM-$ARCH"
echo ""

# Create install directory
mkdir -p "$INSTALL_DIR"

# Download binary
DOWNLOAD_URL="https://github.com/buddyholly007/syntaur/releases/download/v${VERSION}/syntaur-${PLATFORM}-${ARCH}"
echo "  Downloading $BRAND..."

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$DOWNLOAD_URL" -o "$INSTALL_DIR/$BINARY" 2>/dev/null || {
    echo ""
    echo "  Note: Download server not yet available."
    echo "  For now, copy the binary manually to $INSTALL_DIR/$BINARY"
    echo "  Then run: $BINARY"
    echo ""
    exit 0
  }
elif command -v wget >/dev/null 2>&1; then
  wget -q "$DOWNLOAD_URL" -O "$INSTALL_DIR/$BINARY" 2>/dev/null || {
    echo "  Note: Download server not yet available."
    exit 0
  }
else
  echo "Error: curl or wget required"
  exit 1
fi

chmod +x "$INSTALL_DIR/$BINARY"

# Check if install dir is in PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "  Adding $INSTALL_DIR to PATH..."
    SHELL_NAME=$(basename "$SHELL")
    case "$SHELL_NAME" in
      bash) echo "export PATH=\"\$HOME/.local/bin:\$PATH\"" >> "$HOME/.bashrc" ;;
      zsh)  echo "export PATH=\"\$HOME/.local/bin:\$PATH\"" >> "$HOME/.zshrc" ;;
      fish) echo "fish_add_path $HOME/.local/bin" >> "$HOME/.config/fish/config.fish" 2>/dev/null ;;
    esac
    export PATH="$INSTALL_DIR:$PATH"
    ;;
esac

# Install systemd service (Linux only)
if [ "$PLATFORM" = "linux" ] && command -v systemctl >/dev/null 2>&1; then
  UNIT_DIR="$HOME/.config/systemd/user"
  mkdir -p "$UNIT_DIR"

  cat > "$UNIT_DIR/syntaur.service" << UNIT
[Unit]
Description=Syntaur AI Platform
After=network-online.target

[Service]
ExecStart=$INSTALL_DIR/$BINARY
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
UNIT

  systemctl --user daemon-reload
  systemctl --user enable syntaur.service
  echo "  Systemd service installed (syntaur.service)"
fi

# macOS launchd plist
if [ "$PLATFORM" = "macos" ]; then
  PLIST_DIR="$HOME/Library/LaunchAgents"
  mkdir -p "$PLIST_DIR"

  cat > "$PLIST_DIR/dev.syntaur.gateway.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.syntaur.gateway</string>
  <key>ProgramArguments</key>
  <array><string>$INSTALL_DIR/$BINARY</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
PLIST

  echo "  LaunchAgent installed (dev.syntaur.gateway)"
fi

echo ""
echo "  ✓ $BRAND installed to $INSTALL_DIR/$BINARY"
echo ""
echo "  To start:"
if [ "$PLATFORM" = "linux" ]; then
  echo "    systemctl --user start syntaur"
else
  echo "    $BINARY"
fi
echo ""
echo "  Your browser will open to the setup wizard automatically."
echo "  Dashboard: http://localhost:18789"
echo ""
