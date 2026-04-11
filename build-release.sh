#!/bin/bash
# Syntaur release build script
# Builds the gateway binary for Linux x86_64
# For macOS/Windows, cross-compilation or CI is needed
set -e

VERSION="0.1.0"
WORKSPACE="/home/sean/openclaw-workspace"
OUTPUT="$WORKSPACE/release"

echo "Building Syntaur v$VERSION..."

cd "$WORKSPACE"

# Build gateway
echo "  Building gateway..."
cargo build --release -p rust-openclaw 2>&1 | tail -1

# Build module manager
echo "  Building ocmod..."
cargo build --release -p openclaw-mod 2>&1 | tail -1

# Collect artifacts
mkdir -p "$OUTPUT"
cp target/release/rust-openclaw "$OUTPUT/syntaur"
cp target/release/ocmod "$OUTPUT/ocmod"
cp README-syntaur.md "$OUTPUT/README.md"
cp install.sh "$OUTPUT/install.sh"

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
