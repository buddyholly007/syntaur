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

# Download viewer (lightweight dashboard window — no full browser needed)
VIEWER_BINARY="syntaur-viewer"
VIEWER_URL="https://github.com/buddyholly007/syntaur/releases/download/v${VERSION}/syntaur-viewer-${PLATFORM}-${ARCH}"
echo "  Downloading dashboard viewer..."
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$VIEWER_URL" -o "$INSTALL_DIR/$VIEWER_BINARY" 2>/dev/null || true
elif command -v wget >/dev/null 2>&1; then
  wget -q "$VIEWER_URL" -O "$INSTALL_DIR/$VIEWER_BINARY" 2>/dev/null || true
fi
if [ -f "$INSTALL_DIR/$VIEWER_BINARY" ]; then
  chmod +x "$INSTALL_DIR/$VIEWER_BINARY"
fi

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

# Install desktop shortcut
DASHBOARD_URL="http://localhost:18789"

if [ "$PLATFORM" = "linux" ]; then
  # Install icon
  ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
  mkdir -p "$ICON_DIR"
  cat > "$ICON_DIR/syntaur.svg" << 'ICON'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#0ea5e9"/>
      <stop offset="100%" stop-color="#0369a1"/>
    </linearGradient>
  </defs>
  <rect width="64" height="64" rx="12" fill="#0a0a0a"/>
  <path d="M16 44 C16 38 20 34 26 34 L38 34 C44 34 48 38 48 44 L48 48 C48 52 44 52 42 48 L40 44 L38 48 C36 52 32 52 30 48 L28 44 L26 48 C24 52 20 52 18 48 L16 44Z" fill="url(#g)"/>
  <path d="M30 34 L30 20 C30 16 32 14 34 14 L34 14 C36 14 38 16 38 20 L38 34" fill="url(#g)"/>
  <circle cx="34" cy="11" r="5" fill="url(#g)"/>
  <path d="M36 20 L46 16 L48 14" stroke="url(#g)" stroke-width="2.5" stroke-linecap="round" fill="none"/>
  <path d="M16 38 C12 36 10 32 12 28" stroke="url(#g)" stroke-width="2" stroke-linecap="round" fill="none"/>
</svg>
ICON

  # Install .desktop file (shows in app launcher / start menu)
  APP_DIR="$HOME/.local/share/applications"
  mkdir -p "$APP_DIR"
  # Use syntaur-viewer if available, fall back to xdg-open
  if [ -x "$INSTALL_DIR/syntaur-viewer" ]; then
    SHORTCUT_EXEC="$INSTALL_DIR/syntaur-viewer"
  else
    SHORTCUT_EXEC="xdg-open $DASHBOARD_URL"
  fi

  cat > "$APP_DIR/syntaur.desktop" << DESKTOP
[Desktop Entry]
Name=Syntaur
Comment=Your personal AI platform
Exec=$SHORTCUT_EXEC
Icon=syntaur
Type=Application
Categories=Utility;Development;
StartupNotify=false
DESKTOP

  # Validate the desktop file if desktop-file-validate is available
  if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$APP_DIR/syntaur.desktop" 2>/dev/null || true
  fi

  # Update icon cache if possible
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
  fi

  echo "  Application shortcut installed (find 'Syntaur' in your app launcher)"
fi

if [ "$PLATFORM" = "macos" ]; then
  # Create a lightweight .app that opens the dashboard in the default browser
  APP_PATH="$HOME/Applications/Syntaur.app"
  mkdir -p "$APP_PATH/Contents/MacOS"
  mkdir -p "$APP_PATH/Contents/Resources"

  cat > "$APP_PATH/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Syntaur</string>
  <key>CFBundleDisplayName</key><string>Syntaur</string>
  <key>CFBundleIdentifier</key><string>dev.syntaur.app</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>syntaur-open</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
PLIST

  cat > "$APP_PATH/Contents/MacOS/syntaur-open" << LAUNCHER
#!/bin/sh
if [ -x "$INSTALL_DIR/syntaur-viewer" ]; then
  exec "$INSTALL_DIR/syntaur-viewer"
else
  open "http://localhost:18789"
fi
LAUNCHER
  chmod +x "$APP_PATH/Contents/MacOS/syntaur-open"

  # Copy SVG as a resource (macOS Spotlight can index it; for a proper icon
  # you'd convert to .icns, but the .app still works without one)
  cat > "$APP_PATH/Contents/Resources/icon.svg" << 'ICON'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#0ea5e9"/>
      <stop offset="100%" stop-color="#0369a1"/>
    </linearGradient>
  </defs>
  <rect width="64" height="64" rx="12" fill="#0a0a0a"/>
  <path d="M16 44 C16 38 20 34 26 34 L38 34 C44 34 48 38 48 44 L48 48 C48 52 44 52 42 48 L40 44 L38 48 C36 52 32 52 30 48 L28 44 L26 48 C24 52 20 52 18 48 L16 44Z" fill="url(#g)"/>
  <path d="M30 34 L30 20 C30 16 32 14 34 14 L34 14 C36 14 38 16 38 20 L38 34" fill="url(#g)"/>
  <circle cx="34" cy="11" r="5" fill="url(#g)"/>
  <path d="M36 20 L46 16 L48 14" stroke="url(#g)" stroke-width="2.5" stroke-linecap="round" fill="none"/>
  <path d="M16 38 C12 36 10 32 12 28" stroke="url(#g)" stroke-width="2" stroke-linecap="round" fill="none"/>
</svg>
ICON

  echo "  Application shortcut installed (find 'Syntaur' in ~/Applications or Spotlight)"
fi

echo ""
echo "  ✓ $BRAND installed to $INSTALL_DIR/$BINARY"
echo ""
echo "  To start:"
if [ "$PLATFORM" = "linux" ]; then
  echo "    systemctl --user start syntaur"
  echo ""
  echo "  Then open Syntaur from your app launcher, or go to:"
else
  echo "    $BINARY"
  echo ""
  echo "  Then open Syntaur from ~/Applications or Spotlight, or go to:"
fi
echo "    $DASHBOARD_URL"
echo ""
