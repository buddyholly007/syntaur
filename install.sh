#!/bin/sh
# Syntaur installer — https://syntaur.dev
#
# Recommended install (verified):
#   wget https://github.com/buddyholly007/syntaur/releases/latest/download/install.sh
#   sh install.sh --server           # reads signed checksums + cosign bundle from release
#
# Alternative (developer convenience, *not* verified — warns you loudly):
#   curl -sSL https://get.syntaur.dev | sh
#
# Flags:
#   --server          install the gateway + service unit
#   --connect         install only the viewer (points at an existing server)
#   --skip-verify     bypass checksum + signature verification (developer use; WARNS)
#   --no-sudo         skip the package-manager step that installs GStreamer plugins
set -e

BRAND="Syntaur"
# MUST match the VERSION file at repo root. Run `scripts/sync-version.sh`
# before tagging a release so this string and install.ps1 stay in sync with
# the workspace version in Cargo.toml. install.sh ships standalone (users
# curl|sh it), so it can't read the VERSION file at runtime.
VERSION="0.5.0"
BINARY="syntaur"
INSTALL_DIR="$HOME/.local/bin"
MODE=""
VERIFY="1"
AUTOSUDO="1"

# Parse flags
for arg in "$@"; do
  case "$arg" in
    --server)       MODE="server" ;;
    --connect)      MODE="connect" ;;
    --skip-verify)  VERIFY="0" ;;
    --no-sudo)      AUTOSUDO="0" ;;
  esac
done

# ── Release-integrity helpers (Phase 2.2 of the security plan) ──────────────
#
# download_verified <binary-url> <target-path>
#   Downloads the binary, alongside its sibling `checksums.txt` +
#   `checksums.txt.cosign.bundle` from the same release directory.
#   Verifies:
#     1. sha256 against checksums.txt
#     2. cosign signature of checksums.txt (if cosign is installed)
#   Aborts with a loud error on any mismatch. `--skip-verify` bypasses
#   everything and prints a RED WARNING so it can't happen silently.
#
# The release signing workflow at .github/workflows/release-sign.yml
# publishes checksums.txt + *.cosign.bundle alongside every tagged
# release; this function assumes that layout.

REPO_URL="https://github.com/buddyholly007/syntaur"
COSIGN_IDENT="https://github.com/buddyholly007/syntaur/.github/workflows/release-sign.yml@refs/tags/v${VERSION}"
COSIGN_ISSUER="https://token.actions.githubusercontent.com"

_fetch() {
  # $1 url, $2 out
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$1" -O "$2"
  else
    echo "Error: need curl or wget"
    return 1
  fi
}

download_verified() {
  BIN_URL="$1"
  OUT="$2"
  BASE_URL=$(dirname "$BIN_URL")
  BIN_NAME=$(basename "$BIN_URL")
  TMP=$(mktemp -d)
  trap "rm -rf '$TMP'" EXIT INT TERM

  echo "  → Downloading $BIN_NAME"
  _fetch "$BIN_URL" "$TMP/$BIN_NAME" || return 1

  if [ "$VERIFY" = "0" ]; then
    printf "  \033[1;31m!!!\033[0m \033[1m--skip-verify:\033[0m installing unverified binary. DO NOT use in production.\n"
    mv "$TMP/$BIN_NAME" "$OUT"
    chmod +x "$OUT"
    return 0
  fi

  # Fetch checksums.txt + its cosign bundle.
  if ! _fetch "$BASE_URL/checksums.txt" "$TMP/checksums.txt" 2>/dev/null; then
    echo ""
    echo "  ✗ checksums.txt missing on this release — cannot verify."
    echo "    Re-run with --skip-verify only if you accept the risk."
    return 1
  fi
  _fetch "$BASE_URL/checksums.txt.cosign.bundle" "$TMP/checksums.txt.cosign.bundle" 2>/dev/null || true

  # Step 1: SHA-256 match.
  (cd "$TMP" && grep " $BIN_NAME\$" checksums.txt | sha256sum -c --quiet) || {
    echo ""
    echo "  ✗ SHA-256 mismatch for $BIN_NAME — ABORT."
    echo "    Expected hash in checksums.txt did not match the downloaded binary."
    echo "    This could mean: a corrupted download, a malicious proxy, OR a"
    echo "    compromised release asset. Report to security@syntaur.dev."
    return 1
  }
  echo "  ✓ SHA-256 verified"

  # Step 2: cosign signature (best-effort — install cosign if missing).
  if ! command -v cosign >/dev/null 2>&1; then
    echo "  ⚠ cosign not installed — signature not verified (hash OK)."
    echo "    For strongest verification: https://docs.sigstore.dev/system_config/installation/"
  else
    if [ -f "$TMP/checksums.txt.cosign.bundle" ]; then
      cosign verify-blob \
        --bundle "$TMP/checksums.txt.cosign.bundle" \
        --certificate-identity "$COSIGN_IDENT" \
        --certificate-oidc-issuer "$COSIGN_ISSUER" \
        "$TMP/checksums.txt" >/dev/null 2>&1 && echo "  ✓ cosign signature verified" || {
        echo ""
        echo "  ✗ cosign signature verification FAILED — ABORT."
        echo "    The release artifact could not be cryptographically tied to the"
        echo "    Syntaur GitHub Actions signing identity. DO NOT run this binary."
        return 1
      }
    else
      echo "  ⚠ No cosign bundle on this release — signature not verified."
    fi
  fi

  mv "$TMP/$BIN_NAME" "$OUT"
  chmod +x "$OUT"
}

echo ""
echo "  ♞ $BRAND v$VERSION"
echo "  Your personal AI platform"
echo ""

# If no mode specified, ask
if [ -z "$MODE" ]; then
  echo "  How would you like to use Syntaur?"
  echo ""
  echo "  1) Run the server on this computer"
  echo "     Your AI runs here. Access from phone, laptop, any device."
  echo "     (This computer needs to stay on.)"
  echo ""
  echo "  2) Connect to my Syntaur server"
  echo "     Syntaur is already running elsewhere. Just install the viewer."
  echo ""
  printf "  Choose [1/2]: "
  read -r CHOICE
  case "$CHOICE" in
    2) MODE="connect" ;;
    *) MODE="server" ;;
  esac
  echo ""
fi

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

# Download gateway binary (server mode only) — verified by default.
if [ "$MODE" = "server" ]; then
  DOWNLOAD_URL="${REPO_URL}/releases/download/v${VERSION}/syntaur-${PLATFORM}-${ARCH}"
  echo "  Installing $BRAND server..."

  download_verified "$DOWNLOAD_URL" "$INSTALL_DIR/$BINARY" || {
    echo ""
    echo "  ✗ Could not install a verified server binary."
    echo "    If the release doesn't carry signed checksums yet, re-run with"
    echo "    --skip-verify to proceed at your own risk."
    exit 1
  }
fi

# Download viewer — verified too when available.
VIEWER_BINARY="syntaur-viewer"
VIEWER_URL="${REPO_URL}/releases/download/v${VERSION}/syntaur-viewer-${PLATFORM}-${ARCH}"
echo "  Installing dashboard viewer..."
download_verified "$VIEWER_URL" "$INSTALL_DIR/$VIEWER_BINARY" 2>/dev/null || {
  echo "  (viewer download skipped — not yet published for this platform)"
}

# Ensure the WebKitGTK viewer can actually decode audio. On many distros
# the base `webkit2gtk` / `webkitgtk-6.0` package does NOT pull in
# `gst-plugins-good`, which is where autoaudiosink / pulsesink live.
# Without them, the first <audio>.play() call segfaults the WebKit
# render process (confirmed on CachyOS 2026-04-19). Detect the package
# manager and install the known-required GStreamer plugins.
#
# We treat three packages as the baseline:
#   gst-plugins-good  — audiosinks + mp3/wav/flac parsers
#   gst-plugins-bad   — extra decoders (aac, more)
#   gst-libav         — ffmpeg-backed decoders, covers everything else
#
# All three are in the standard repos of the distros Syntaur supports.
if [ "$PLATFORM" = "linux" ]; then
  echo "  Checking GStreamer audio plugins…"
  NEED_INSTALL=""
  NEED_PKGS=""
  MGR=""

  if command -v pacman >/dev/null 2>&1; then
    # Arch / CachyOS / Manjaro / EndeavourOS
    MGR="pacman"
    for pkg in gst-plugins-good gst-plugins-bad gst-libav bubblewrap; do
      if ! pacman -Q "$pkg" >/dev/null 2>&1; then
        NEED_PKGS="$NEED_PKGS $pkg"
      fi
    done
  elif command -v apt-get >/dev/null 2>&1; then
    # Debian / Ubuntu / Linux Mint
    MGR="apt"
    for pkg in gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav bubblewrap; do
      if ! dpkg -s "$pkg" >/dev/null 2>&1; then
        NEED_PKGS="$NEED_PKGS $pkg"
      fi
    done
  elif command -v dnf >/dev/null 2>&1; then
    # Fedora / RHEL / Rocky / Alma
    MGR="dnf"
    for pkg in gstreamer1-plugins-good gstreamer1-plugins-bad-free gstreamer1-libav bubblewrap; do
      if ! rpm -q "$pkg" >/dev/null 2>&1; then
        NEED_PKGS="$NEED_PKGS $pkg"
      fi
    done
  elif command -v zypper >/dev/null 2>&1; then
    # openSUSE
    MGR="zypper"
    for pkg in gstreamer-plugins-good gstreamer-plugins-bad gstreamer-plugins-libav bubblewrap; do
      if ! rpm -q "$pkg" >/dev/null 2>&1; then
        NEED_PKGS="$NEED_PKGS $pkg"
      fi
    done
  fi

  if [ -n "$NEED_PKGS" ] && [ -n "$MGR" ]; then
    echo "  Missing audio plugins:$NEED_PKGS"
    if [ "$AUTOSUDO" = "0" ]; then
      echo "  --no-sudo was passed; skipping installation. Run this manually:"
      case "$MGR" in
        pacman) echo "    sudo pacman -S $NEED_PKGS" ;;
        apt)    echo "    sudo apt-get install $NEED_PKGS" ;;
        dnf)    echo "    sudo dnf install $NEED_PKGS" ;;
        zypper) echo "    sudo zypper install $NEED_PKGS" ;;
      esac
      INSTALL_FAILED=1
    else
      printf "  Install now with sudo? [Y/n] "
      read -r CONFIRM
      case "$CONFIRM" in
        n|N|no|No)
          echo "  Skipped. Install manually if music playback crashes the viewer."
          INSTALL_FAILED=1
          ;;
        *)
          echo "  Installing so music playback works out of the box…"
          case "$MGR" in
            pacman) sudo pacman -S --needed --noconfirm $NEED_PKGS 2>&1 | tail -3 || INSTALL_FAILED=1 ;;
            apt)    sudo apt-get install -y $NEED_PKGS 2>&1 | tail -3 || INSTALL_FAILED=1 ;;
            dnf)    sudo dnf install -y $NEED_PKGS 2>&1 | tail -3 || INSTALL_FAILED=1 ;;
            zypper) sudo zypper install -y $NEED_PKGS 2>&1 | tail -3 || INSTALL_FAILED=1 ;;
          esac
          ;;
      esac
    fi
    if [ -n "$INSTALL_FAILED" ]; then
      echo ""
      echo "  Couldn't install audio plugins automatically."
      echo "  Music playback will crash the viewer until these are installed:"
      case "$MGR" in
        pacman) echo "    sudo pacman -S $NEED_PKGS" ;;
        apt)    echo "    sudo apt-get install $NEED_PKGS" ;;
        dnf)    echo "    sudo dnf install $NEED_PKGS" ;;
        zypper) echo "    sudo zypper install $NEED_PKGS" ;;
      esac
      echo ""
    else
      echo "  ✓ Audio plugins ready"
    fi
  elif [ -z "$MGR" ]; then
    echo "  (unfamiliar package manager — if audio playback crashes, install"
    echo "   gst-plugins-good / gst-plugins-bad / gst-libav via your distro)"
  else
    echo "  ✓ Audio plugins already installed"
  fi
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

# Install systemd service (server mode, Linux only)
if [ "$MODE" = "server" ] && [ "$PLATFORM" = "linux" ] && command -v systemctl >/dev/null 2>&1; then
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

# macOS launchd plist (server mode only)
if [ "$MODE" = "server" ] && [ "$PLATFORM" = "macos" ]; then
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
  # Install SVG icon
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

  # Generate PNG icons for KDE/XFCE (SVG alone is not enough)
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "
import struct, zlib, os
def create_png(size, path):
    raw = b''
    for y in range(size):
        raw += b'\x00'
        for x in range(size):
            t = y / size
            r = int(14 + t * (3 - 14))
            g = int(165 + t * (105 - 165))
            b = int(233 + t * (161 - 233))
            cx, cy = abs(x - size//2), abs(y - size//2)
            corner = size//2 - size//8
            if cx > corner and cy > corner:
                if ((cx - corner)**2 + (cy - corner)**2)**0.5 > size//8:
                    raw += bytes([0, 0, 0, 0]); continue
            raw += bytes([r, g, b, 255])
    def chunk(ct, d):
        c = ct + d
        return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    hdr = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', hdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
    with open(path, 'wb') as f: f.write(png)
for s in [48, 64, 128, 256]:
    d = os.path.expanduser('~/.local/share/icons/hicolor/%dx%d/apps' % (s, s))
    os.makedirs(d, exist_ok=True)
    create_png(s, d + '/syntaur.png')
" 2>/dev/null && echo "  PNG icons generated"
  fi

  # Determine exec command: prefer syntaur-viewer, fall back to xdg-open
  if [ -x "$INSTALL_DIR/syntaur-viewer" ]; then
    SHORTCUT_EXEC="$INSTALL_DIR/syntaur-viewer"
  else
    SHORTCUT_EXEC="xdg-open $DASHBOARD_URL"
  fi

  # Install .desktop file in app launcher
  APP_DIR="$HOME/.local/share/applications"
  mkdir -p "$APP_DIR"
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

  # Install .desktop file on desktop
  DESKTOP_DIR="${XDG_DESKTOP_DIR:-$HOME/Desktop}"
  if [ -d "$DESKTOP_DIR" ]; then
    cp "$APP_DIR/syntaur.desktop" "$DESKTOP_DIR/syntaur.desktop"
    chmod +x "$DESKTOP_DIR/syntaur.desktop"

    # GNOME trust
    gio set "$DESKTOP_DIR/syntaur.desktop" metadata::trusted true 2>/dev/null

    # KDE Plasma trust
    if command -v kwriteconfig5 >/dev/null 2>&1; then
      kwriteconfig5 --file "$DESKTOP_DIR/syntaur.desktop" --group "Desktop Entry" --key "X-KDE-RunOnDisconnect" "false" 2>/dev/null
    fi
  fi

  # Update icon caches
  gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
  kbuildsycoca5 2>/dev/null || true

  echo "  Application shortcut installed (find 'Syntaur' in your app launcher and on your desktop)"
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

  # Desktop .webloc — double-click opens in default browser. Only
  # created when SYNTAUR_URL is set (remote gateway case); for
  # local-only installs the Applications bundle is enough.
  if [ -n "${SYNTAUR_URL:-}" ] && [ -d "$HOME/Desktop" ]; then
    cat > "$HOME/Desktop/Syntaur.webloc" << WEBLOC
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>URL</key>
  <string>${SYNTAUR_URL}</string>
</dict>
</plist>
WEBLOC
    echo "  Desktop shortcut created (Syntaur.webloc)"
  fi
fi

# ── Tailscale auto-setup (Tier 2 onboarding) ────────────────────────────────
#
# If the caller passed a Tailscale pre-auth key via SYNTAUR_TS_AUTHKEY
# (the personalized-invite path mints one and bakes it into the install
# command you send to a family member), bring Tailscale up to the
# household tailnet so the viewer reaches the remote gateway the moment
# it launches. Silently skipped when the key is absent — local-only
# installs don't need it.
#
# Uses the official Tailscale installer. We never ship the daemon
# ourselves. Users who already have Tailscale just get the `up` call.

if [ -n "${SYNTAUR_TS_AUTHKEY:-}" ]; then
  echo ""
  echo "  Setting up Tailscale…"

  if ! command -v tailscale >/dev/null 2>&1; then
    if [ "$PLATFORM" = "macos" ]; then
      # Mac installer is a signed .pkg — needs a user click-through.
      echo "  Tailscale isn't installed yet. Opening the download page…"
      open "https://tailscale.com/download/mac" 2>/dev/null || true
      echo ""
      echo "  When Tailscale finishes installing (icon appears in the menu bar),"
      echo "  run this command to finish joining the household network:"
      echo ""
      echo "    /Applications/Tailscale.app/Contents/MacOS/Tailscale up \\"
      echo "      --authkey=\"\$SYNTAUR_TS_AUTHKEY\" --accept-routes"
    elif [ "$PLATFORM" = "linux" ]; then
      echo "  Installing Tailscale…"
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL https://tailscale.com/install.sh | sh
      else
        echo "  ! curl required for Tailscale install; please install curl and re-run."
      fi
    fi
  fi

  if command -v tailscale >/dev/null 2>&1; then
    TS_UP="tailscale up --authkey=${SYNTAUR_TS_AUTHKEY} --accept-routes"
    if [ "$PLATFORM" = "linux" ] && [ "$(id -u)" != "0" ]; then
      sudo $TS_UP || echo "  ! tailscale up failed — retry with: sudo $TS_UP"
    else
      $TS_UP || echo "  ! tailscale up failed — you can retry with the command above."
    fi
    echo "  ✓ Tailscale connected to your household tailnet"
  fi
fi

echo ""
if [ "$MODE" = "server" ]; then
  echo "  ✓ $BRAND server installed"
  echo ""
  echo "  To start:"
  if [ "$PLATFORM" = "linux" ]; then
    echo "    systemctl --user start syntaur"
  else
    echo "    $BINARY"
  fi
  echo ""
  echo "  Open Syntaur from your app launcher or go to:"
  echo "    $DASHBOARD_URL"
  echo ""
  echo "  To access from your phone or other computers:"
  echo "    1. Install Tailscale on this computer and your other devices"
  echo "    2. Open the Tailscale URL shown in the Syntaur dashboard"
  echo "    3. Or use the local network address shown after setup"
else
  echo "  ✓ $BRAND viewer installed"
  echo ""
  echo "  Open Syntaur from your app launcher to connect to your server."
  echo "  The setup wizard will ask for your server address."
fi
echo ""
