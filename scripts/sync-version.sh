#!/bin/sh
# sync-version.sh — propagate the VERSION file into every place that can't
# read it at runtime. Run before tagging a release; CI can invoke this in
# a pre-release job to fail the build if anything drifted.
#
# Authoritative source: ./VERSION
# Targets:
#   - install.sh      VERSION="x.y.z"
#   - install.ps1     $Version = "x.y.z"
#   - Cargo.toml      [workspace.package] version = "x.y.z"
#
# Crate Cargo.tomls inherit from the workspace (`version.workspace = true`)
# so they don't need a per-file bump — cargo reads from [workspace.package].
set -eu

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [ ! -f VERSION ]; then
  echo "error: no VERSION file at repo root" >&2
  exit 1
fi

version="$(cat VERSION | tr -d '[:space:]')"
if [ -z "$version" ]; then
  echo "error: VERSION file is empty" >&2
  exit 1
fi

echo "Syncing version $version into install.sh, install.ps1, Cargo.toml..."

# install.sh
sed -i.bak -E "s/^VERSION=\"[^\"]+\"/VERSION=\"$version\"/" install.sh
rm -f install.sh.bak

# install.ps1
sed -i.bak -E "s/^\\\$Version = \"[^\"]+\"/\$Version = \"$version\"/" install.ps1
rm -f install.ps1.bak

# workspace Cargo.toml (single source for every crate that inherits)
sed -i.bak -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+\"/version = \"$version\"/" Cargo.toml
rm -f Cargo.toml.bak

# landing page version badge — matched by HTML comment markers so the sed
# is not fooled by CDN URLs or other version-shaped strings elsewhere in
# the file.
sed -i.bak -E "s#<!-- VERSION-BADGE -->[^<]*<!-- /VERSION-BADGE -->#<!-- VERSION-BADGE -->v$version<!-- /VERSION-BADGE -->#" landing/index.html
rm -f landing/index.html.bak

echo "Done. Diff these before committing:"
echo "  git diff VERSION install.sh install.ps1 Cargo.toml"
