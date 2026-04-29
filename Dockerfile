# Syntaur gateway — production runtime image.
#
# Two deploy modes use this same image:
#   1. Fast binary-swap deploy (deploy.sh): binaries live on a bind-mounted
#      volume, image only provides the runtime environment.
#   2. Full image build (nightly GHCR push): COPY-in the target/release
#      artifacts so `docker run` works without a bind-mount.
#
# Keep this image thin — Phase 4.6 MCP sandboxing via bubblewrap is the
# only security-critical addition past the base runtime.

# ─── Builder stage ────────────────────────────────────────────────────────
# Match claudevm's toolchain (1.94.x). Earlier bumps 1.84 → 1.88
# unblocked edition2024 parsing but then hit `round_char_boundary`
# (stabilized >=1.89) in syntaur-gateway. Rather than chase each
# transitive MSRV bump one-commit-at-a-time, pin to what the
# interactive dev environment actually uses so CI matches local.
FROM rust:1.94-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    libdbus-1-dev \
    g++ \
  && rm -rf /var/lib/apt/lists/*
# libdbus-1-dev is needed by libdbus-sys (pulled in transitively via
# rs-matter → bluer → dbus for BLE commissioning). Claudevm has it
# pre-installed; rust:*-slim-bookworm doesn't.
#
# g++ is needed at LINK time — `-lstdc++` is required by the C++
# bits in aws-lc-sys + onnxruntime (via ort/fastembed). The image's
# default `gcc` doesn't include libstdc++-dev; `g++` pulls it in.
# Symptom: "rust-lld: error: unable to find library -lstdc++".

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Cargo resolves the workspace manifest before building any subset of
# members, so even though we only `cargo build -p syntaur-gateway ...`
# below, every member listed in Cargo.toml's `[workspace] members`
# block must be present on disk. Missing directories -> "failed to read
# <member>/Cargo.toml" build error. Copy every member; the ones we
# don't build stay harmlessly in /src as source only.
COPY syntaur-gateway syntaur-gateway
COPY syntaur-viewer syntaur-viewer
COPY syntaur-voice-pipeline syntaur-voice-pipeline
COPY syntaur-hwscan syntaur-hwscan
COPY syntaur-setup syntaur-setup
COPY syntaur-license-server syntaur-license-server
COPY syntaur-capability-shim syntaur-capability-shim
COPY syntaur-sdk syntaur-sdk
COPY syntaur-mod syntaur-mod
COPY syntaur-isolation-tests syntaur-isolation-tests
COPY rust-media-bridge rust-media-bridge
COPY mcp-protocol mcp-protocol
COPY mcp-server-filesystem-rs mcp-server-filesystem-rs
COPY mcp-server-search-rs mcp-server-search-rs
COPY mace mace
COPY syntaur-ship syntaur-ship
# All workspace-member sub-crates under `crates/`. The workspace root
# Cargo.toml lists each of syntaur-zwave, rust-aidot, rust-kasa,
# rust-nexia, syntaur-matter*, and cargo has to be able to read every
# member's manifest before it can resolve the gateway build. The
# `rust-aidot-harvest` crate is `workspace.exclude`d (keeps `rsa` out
# of the lockfile — see its Cargo.toml), so this bulk COPY is safe:
# the resolver skips excluded paths, the builder never touches them.
COPY crates crates
# syntaur-defaults is NOT a crate — it's the agent-template data dir
# that syntaur-gateway reads at compile time via `include_str!()`.
# Missing → "couldn't read syntaur-defaults/agent-template/AGENTS.md".
COPY syntaur-defaults syntaur-defaults

RUN cargo build --release \
    -p syntaur-gateway \
    -p syntaur-isolation-tests \
    -p mace \
  && strip target/release/syntaur-gateway \
  && strip target/release/syntaur-isolation-tests \
  && strip target/release/mace

# ─── Runtime stage ────────────────────────────────────────────────────────
FROM ubuntu:24.04

# Runtime packages:
#   - ca-certificates: outbound HTTPS to LLM providers, Tailscale, etc.
#   - curl: healthcheck + deploy-sh-side verification
#   - bubblewrap: Phase 4.6 MCP process sandboxing (mcp_sandbox.rs)
#   - xdg-utils: `xdg-open` fallback used by some tool paths
#   - sqlite3: operator debugging of the index.db from inside the container
#   - tzdata: TZ env variable support for scheduler time-zone math
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl bubblewrap xdg-utils sqlite3 tzdata \
  && rm -rf /var/lib/apt/lists/*

# Non-root runtime user. UID 568 lines up with TrueNAS SCALE's reserved
# `apps` user (TrueNAS assigns 568 to every containerized app by default),
# so the host-side files created by Syntaur have the right ownership for
# the platform's app-management tooling. On other platforms (plain Docker
# on Linux, macOS, Windows) 568 is also almost certainly unused — it
# avoids accidental collision with any pre-existing human user since
# standard user accounts start at 1000.
RUN useradd --create-home --uid 568 --user-group syntaur

# Binaries. In the bind-mount deploy path these get shadowed by the host's
# compiled binaries; the COPY lines below ensure a standalone `docker run`
# of this image still works.
COPY --from=builder /src/target/release/syntaur-gateway /usr/local/bin/rust-openclaw
COPY --from=builder /src/target/release/syntaur-isolation-tests /usr/local/bin/syntaur-isolation-tests
COPY --from=builder /src/target/release/mace /usr/local/bin/mace

# Default config path. The production compose overrides `command` to point
# at a bind-mounted syntaur.json; this baked-in path only matters for
# `docker run` smoke tests.
ENV HOME=/home/syntaur
# Fail-closed sandboxing is now the in-code default (mcp_sandbox.rs); this
# env line is no longer required for prod safety, but kept commented for
# documentation. Operators who genuinely need fail-open can flip with:
#   ENV SYNTAUR_ALLOW_UNSANDBOXED_MCP=1
# bwrap is installed above so this image stays sandboxed regardless.
USER syntaur
WORKDIR /home/syntaur

EXPOSE 18789

# Fail-fast on startup misconfiguration — `security::assert_startup_permissions`
# in the gateway refuses to boot if master.key or vault.json aren't 0600.
ENTRYPOINT ["/usr/local/bin/rust-openclaw"]
CMD ["/config/syntaur.json"]

LABEL org.opencontainers.image.title="Syntaur" \
      org.opencontainers.image.description="Household AI platform — voice, scheduler, tax, music, knowledge, journal" \
      org.opencontainers.image.source="https://github.com/buddyholly007/syntaur" \
      org.opencontainers.image.licenses="MIT"
