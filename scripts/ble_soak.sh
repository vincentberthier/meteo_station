#!/usr/bin/env bash
# ble_soak.sh — BLE link soak-test harness for gaia (BlueZ 5.86)
#
# Purpose:
#   Continuously exercises the BLE link to the RN4871 weather-station peripheral:
#   connect → hold HOLD_SECS → disconnect → gap GAP_SECS → reconnect → …
#   Any mid-window drop or failed reconnect produces a loud non-zero exit.
#
# Environment knobs (all optional, shown with defaults):
#   DEVICE          — BLE address of the peripheral          (80:1F:12:B6:60:BF)
#   ADAPTER         — local HCI adapter name                 (hci0)
#   HOLD_SECS       — seconds the link must stay up per cycle (360)
#   GAP_SECS        — seconds between disconnect and reconnect (90)
#   CONNECT_TIMEOUT — per-step deadline in seconds           (30)
#   CONN_MIN        — BlueZ debugfs conn_min_interval value  (6)
#   CONN_MAX        — BlueZ debugfs conn_max_interval value  (12)
#   SUPERVISION     — BlueZ debugfs supervision_timeout value (600)
#
# Requires on gaia:
#   bluetoothctl, busctl, doas (for debugfs writes), date
#
# The script NEVER starts a scan.  It connects by address off blueman's
# standing discovery cache — this avoids the "Discovering: yes" wedge trap
# where a concurrent scan blocks connection establishment.
#
# The debugfs connection-parameter files reset on every
# "systemctl restart bluetooth", so apply_conn_params() is called once at
# startup and re-applies them unconditionally each run.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (env-overridable)
# ---------------------------------------------------------------------------
DEVICE="${DEVICE:-80:1F:12:B6:60:BF}"
ADAPTER="${ADAPTER:-hci0}"
HOLD_SECS="${HOLD_SECS:-360}"
GAP_SECS="${GAP_SECS:-90}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-30}"
CONN_MIN="${CONN_MIN:-6}"
CONN_MAX="${CONN_MAX:-12}"
SUPERVISION="${SUPERVISION:-600}"

DBUS_PATH="/org/bluez/${ADAPTER}/dev_${DEVICE//:/_}"
DEBUGFS="/sys/kernel/debug/bluetooth/${ADAPTER}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# log LEVEL MSG — timestamped line to stdout
log() {
    printf '%s %s\n' "$(date -Is)" "$*"
}

# cleanup — best-effort disconnect; safe to call from a trap
cleanup() {
    bluetoothctl disconnect "$DEVICE" >/dev/null 2>&1 || true
}

# fail CYCLE REASON — log failure, disconnect, exit non-zero
fail() {
    log "FAIL(cycle=$1): $2"
    cleanup
    exit 1
}

# apply_conn_params — write BlueZ debugfs connection parameters via doas.
# These are reset by "systemctl restart bluetooth"; reapply each run.
apply_conn_params() {
    # The kernel enforces conn_min_interval <= conn_max_interval on every write,
    # so a naive min-then-max order fails with "Invalid argument" whenever the
    # new min exceeds the *current* max (e.g. widening 6/12 -> 24/40). Drop min
    # to the floor (6) first so any max write is valid, set max, then set min.
    printf '%s' "6"            | doas tee "${DEBUGFS}/conn_min_interval"    >/dev/null
    printf '%s' "$CONN_MAX"    | doas tee "${DEBUGFS}/conn_max_interval"    >/dev/null
    printf '%s' "$CONN_MIN"    | doas tee "${DEBUGFS}/conn_min_interval"    >/dev/null
    printf '%s' "$SUPERVISION" | doas tee "${DEBUGFS}/supervision_timeout"  >/dev/null
    log "conn params applied: min=${CONN_MIN} max=${CONN_MAX} supervision=${SUPERVISION}"
}

# is_connected — returns 0 if the device D-Bus property Connected is true.
# busctl prints "b true" / "b false"; a D-Bus error (device absent) is
# suppressed to stderr and grep returns 1 => "not connected".
# The leading 'if' form is used so grep's non-zero does not trip set -e.
is_connected() {
    busctl get-property org.bluez "$DBUS_PATH" org.bluez.Device1 Connected \
        2>/dev/null | grep -q 'b true'
}

# device_known — preflight cache check.
# The script never scans; blueman's standing discovery must populate the cache.
device_known() {
    bluetoothctl info "$DEVICE" 2>/dev/null | grep -q 'Device '
}

# wait_known — bounded wait for device to appear in the BlueZ cache.
# Called once before the first cycle.  Returns non-zero after CONNECT_TIMEOUT
# iterations.  Each iteration checks the real cache state (poll-with-check).
wait_known() {
    local n=0
    until device_known; do
        sleep 1
        n=$((n + 1))
        [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 1
    done
}

# wait_connected — issue connect, then bounded poll until Connected == true.
# "|| true" keeps a failed bluetoothctl connect from tripping set -e;
# the poll loop is the real arbiter of success.
wait_connected() {
    bluetoothctl connect "$DEVICE" >/dev/null 2>&1 || true
    local n=0
    until is_connected; do
        sleep 1
        n=$((n + 1))
        [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 1
    done
}

# hold — keep the link up for HOLD_SECS seconds.
# Each of the HOLD_SECS iterations reads the real link signal via
# is_connected — the wait ends immediately on a drop.  HOLD_SECS is the
# TEST DEFINITION, not a readiness guess.
hold() {
    local n=0
    while [ "$n" -lt "$HOLD_SECS" ]; do
        sleep 1
        n=$((n + 1))
        is_connected || return 1
    done
}

# disconnect — request disconnect, bounded poll until down.
# Best-effort: if the peer does not acknowledge within CONNECT_TIMEOUT seconds
# we move on (the next cycle's wait_connected re-checks the real state).
disconnect() {
    bluetoothctl disconnect "$DEVICE" >/dev/null 2>&1 || true
    local n=0
    while is_connected; do
        sleep 1
        n=$((n + 1))
        [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 0
    done
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

trap 'log "interrupted"; cleanup; exit 0' INT

apply_conn_params

if ! wait_known; then
    log "FATAL: $DEVICE not in BlueZ cache within ${CONNECT_TIMEOUT}s."
    log "  -> ensure the device is powered/advertising and blueman discovery is running."
    log "  -> after a 'systemctl restart bluetooth', let blueman re-discover before re-running."
    exit 1
fi

cycle=0
while true; do
    cycle=$((cycle + 1))
    log "cycle=$cycle: connecting"
    wait_connected || fail "$cycle" "connect/re-advertise within ${CONNECT_TIMEOUT}s"
    log "cycle=$cycle: connected, holding ${HOLD_SECS}s"
    hold || fail "$cycle" "link dropped before ${HOLD_SECS}s hold completed"
    log "cycle=$cycle: PASS (held ${HOLD_SECS}s)"
    disconnect
    log "cycle=$cycle: disconnected, gap ${GAP_SECS}s"
    sleep "$GAP_SECS"
done
