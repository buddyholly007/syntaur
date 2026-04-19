#!/bin/sh
# Tailscale sidecar entrypoint for Syntaur.
#
# Polls /config/authkey for changes and (re-)authenticates the node on
# change. After a successful login, applies `tailscale serve` pointing at
# the gateway's HTTP listener on the TrueNAS host.
#
# Expected bind-mounts:
#   /config/authkey  (0600, written by the Syntaur gateway)
#   /state           (persistent Tailscale state — node identity)
#
# Environment:
#   TS_HOSTNAME   (optional, defaults to `syntaur`)
#
# Requires /dev/net/tun + CAP_NET_ADMIN — Tailscale Serve binds port 443
# on the tailnet interface and that needs a real TUN device. Userspace
# networking doesn't support inbound ports.

set -e

TS_HOSTNAME="${TS_HOSTNAME:-syntaur}"
KEY_FILE="/config/authkey"
BACKEND_URL="http://host.docker.internal:18789"
SOCKET="/state/tailscaled.sock"
LAST_KEY_HASH=""
SERVE_APPLIED=0

/usr/local/bin/tailscaled \
    --statedir=/state \
    --socket="$SOCKET" &
TAILSCALED_PID=$!
trap "kill $TAILSCALED_PID 2>/dev/null; exit 0" TERM INT

# Give tailscaled a moment to create its socket.
for _ in 1 2 3 4 5; do
    [ -S "$SOCKET" ] && break
    sleep 1
done

apply_serve() {
    # Apply the serve config. `serve reset` is cheap and idempotent.
    /usr/local/bin/tailscale --socket="$SOCKET" serve reset 2>/dev/null || true
    out=$(/usr/local/bin/tailscale --socket="$SOCKET" serve --bg "$BACKEND_URL" 2>&1) || true
    # If Tailscale tells us Serve isn't enabled on the tailnet, surface the
    # exact URL in the sidecar logs so the admin can click it once.
    echo "$out" | grep -q "Serve is not enabled" && {
        echo "[sidecar] >>>>>> Tailscale Serve not enabled for this tailnet."
        echo "$out" | grep -oE 'https://login\.tailscale\.com/f/serve[^ ]*'
        SERVE_APPLIED=0
        return 1
    }
    echo "[sidecar] serve applied: $BACKEND_URL"
    SERVE_APPLIED=1
}

while true; do
    # 1. Auth key rotation: if the file changed, re-up Tailscale.
    if [ -s "$KEY_FILE" ]; then
        HASH=$(md5sum "$KEY_FILE" 2>/dev/null | awk '{print $1}')
        if [ "$HASH" != "$LAST_KEY_HASH" ]; then
            echo "[sidecar] auth key changed, (re)logging in"
            /usr/local/bin/tailscale --socket="$SOCKET" up \
                --authkey="$(cat "$KEY_FILE")" \
                --hostname="$TS_HOSTNAME" \
                --accept-dns=false \
                --reset || echo "[sidecar] tailscale up failed (will retry)"
            LAST_KEY_HASH="$HASH"
            SERVE_APPLIED=0
        fi
    fi

    # 2. Serve: apply once after each successful up. Don't re-reset blindly;
    #    a periodic reset would kick off TLS-cert renegotiation unnecessarily
    #    and might clear a working config during a transient error.
    if [ "$SERVE_APPLIED" = "0" ] && /usr/local/bin/tailscale --socket="$SOCKET" status >/dev/null 2>&1; then
        apply_serve || true
    fi

    sleep 5
done
