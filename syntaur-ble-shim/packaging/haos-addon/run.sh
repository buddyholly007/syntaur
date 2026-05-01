#!/usr/bin/env bash
# Add-on entrypoint — reads /data/options.json (filled by HA from the schema in
# config.yaml) and execs the binary with matching CLI flags.
#
# We avoid bashio so the Add-on works with `init: false` (no s6-overlay).

set -euo pipefail

OPTS=/data/options.json
get() {
    if [[ -f "${OPTS}" ]]; then
        jq -r --arg key "$1" '.[$key] // empty' "${OPTS}" 2>/dev/null || true
    fi
}

NAME="$(get name)"
AREA="$(get suggested_area)"
BIND="$(get bind)"
[[ -z "${BIND}" ]] && BIND="0.0.0.0:6053"
VERBOSITY="$(get verbosity)"
[[ -z "${VERBOSITY}" ]] && VERBOSITY="1"

VFLAGS=""
case "${VERBOSITY}" in
    0) VFLAGS="" ;;
    1) VFLAGS="-v" ;;
    2) VFLAGS="-vv" ;;
    *) VFLAGS="-vvv" ;;
esac

ARGS=(--bind "${BIND}")
if [[ -n "${NAME}" ]]; then
    ARGS+=(--name "${NAME}")
fi
if [[ -n "${AREA}" ]]; then
    ARGS+=(--suggested-area "${AREA}")
fi
if [[ -n "${VFLAGS}" ]]; then
    ARGS+=("${VFLAGS}")
fi

echo "[run.sh] starting syntaur-ble-shim with: ${ARGS[*]}"
exec /usr/local/bin/syntaur-ble-shim "${ARGS[@]}"
