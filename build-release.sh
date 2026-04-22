#!/bin/bash
# Syntaur release build script
# Builds the gateway binary for Linux x86_64
# For macOS/Windows, cross-compilation or CI is needed
set -e

# Source of truth for the release version. `scripts/sync-version.sh`
# keeps Cargo.toml / install.sh / install.ps1 / landing badge aligned
# with this same VERSION file; CI version-check.yml fails the build if
# anything drifted.
WORKSPACE="$(cd "$(dirname "$0")" && pwd)"
cd "$WORKSPACE"
if [ ! -f VERSION ]; then
  echo "error: VERSION file missing at repo root — run scripts/sync-version.sh or create it" >&2
  exit 1
fi
VERSION="$(tr -d '[:space:]' < VERSION)"
OUTPUT="$WORKSPACE/release"

echo "Building Syntaur v$VERSION..."

# Build gateway
echo "  Building gateway..."
cargo build --release -p syntaur-gateway 2>&1 | tail -1

# Build module manager
echo "  Building syntaur-mod..."
cargo build --release -p syntaur-mod 2>&1 | tail -1

# Build isolation harness (Phase 4.4). Ships alongside the gateway so
# operators can re-verify cross-user isolation after every deploy.
echo "  Building syntaur-isolation-tests..."
cargo build --release -p syntaur-isolation-tests 2>&1 | tail -1

# Collect artifacts
mkdir -p "$OUTPUT"
cp target/release/syntaur-gateway "$OUTPUT/syntaur"
cp target/release/syntaur-mod "$OUTPUT/syntaur-mod"
cp target/release/syntaur-isolation-tests "$OUTPUT/syntaur-isolation-tests"
cp README-syntaur.md "$OUTPUT/README.md"
cp SECURITY.md "$OUTPUT/SECURITY.md"
cp install.sh "$OUTPUT/install.sh"

# Security docs + operator playbooks ship in the release bundle so
# offline installs still surface guidance.
mkdir -p "$OUTPUT/docs/security"
cp docs/security/threat-model.md "$OUTPUT/docs/security/" 2>/dev/null || true
cp docs/security/operator-hardening.md "$OUTPUT/docs/security/" 2>/dev/null || true

# TrueNAS sidecar template. Operators running the Docker app can drop
# these into their compose path to enable Tailscale Serve (Phase 4.1).
mkdir -p "$OUTPUT/truenas-infra"
cp truenas-infra/docker-compose-prod.yml "$OUTPUT/truenas-infra/" 2>/dev/null || true
cp truenas-infra/tailscale-sidecar-entrypoint.sh "$OUTPUT/truenas-infra/" 2>/dev/null || true

# Copy extension module binaries
for bin in mcp-server-filesystem-rs mcp-server-search-rs; do
  if [ -f "target/release/$bin" ]; then
    cp "target/release/$bin" "$OUTPUT/$bin"
  fi
done

# Size report
echo ""
echo "Release artifacts in $OUTPUT:"
ls -lh "$OUTPUT/"
echo ""
TOTAL=$(du -sh "$OUTPUT" | cut -f1)
echo "Total: $TOTAL"
echo ""
echo "Done! To install: ./syntaur"
