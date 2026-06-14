#!/usr/bin/env bash
#
# Cross-machine BLE debug capture for the MeteoStation firmware.
#
# The debug probe (ST-Link) is on this machine; the Bluetooth adapter is on the
# Gaia host. This script coordinates three concurrent log streams and pulls
# everything back to ./logs:
#
#   1. RTT  — firmware defmt output, captured locally via probe-rs.
#   2. HCI  — btmon's decoded trace, captured on Gaia and streamed back as text
#             over ssh stdout (no file is written on Gaia).
#   3. CLI  — the meteo-cli BLE central, run on Gaia (it has the adapter).
#
# The session ends when meteo-cli exits (the ~30s disconnect ends its
# notification stream), at which point the captures are stopped cleanly.
#
# Usage:   scripts/ble-debug.sh
# Env:     GAIA_HOST (default: gaia)
#          GAIA_REPO (default: ~/code/meteo_station)
#          MAX_FIND_ATTEMPTS (default: 6)   device-ready circuit breaker
#          SESSION_TIMEOUT   (default: 180) hung-central circuit breaker (s)
set -euo pipefail

GAIA="${GAIA_HOST:-gaia}"
GAIA_REPO="${GAIA_REPO:-~/code/meteo_station}"
CHIP="STM32H753ZITx"
BIN="target/thumbv7em-none-eabihf/release/meteo-firmware"
HOST_TARGET="x86_64-unknown-linux-gnu"
MAX_FIND_ATTEMPTS="${MAX_FIND_ATTEMPTS:-6}"
SESSION_TIMEOUT="${SESSION_TIMEOUT:-180}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOGDIR="$REPO_ROOT/logs"
TS="$(date +%Y%m%d-%H%M%S)"
RTT_LOG="$LOGDIR/rtt-$TS.log"
CLI_LOG="$LOGDIR/cli-$TS.log"
HCI_LOG="$LOGDIR/btmon-$TS.log"

mkdir -p "$LOGDIR"
cd "$REPO_ROOT"

rtt_pid=""
btmon_pid=""

cleanup() {
    # Stop the HCI trace: closing the local ssh sends SIGHUP to the remote
    # btmon; the explicit pkill is a belt-and-suspenders for an idle capture
    # that hasn't noticed the closed pipe yet (avoids a stray root btmon).
    if [[ -n "$btmon_pid" ]] && kill -0 "$btmon_pid" 2>/dev/null; then
        kill "$btmon_pid" 2>/dev/null || true
        wait "$btmon_pid" 2>/dev/null || true
    fi
    ssh "$GAIA" "doas pkill -INT -x btmon" 2>/dev/null || true
    # SIGINT (never SIGTERM) so probe-rs cleanly detaches the debug probe —
    # SIGTERM/timeout leaves the probe locked and the chip halted.
    if [[ -n "$rtt_pid" ]] && kill -0 "$rtt_pid" 2>/dev/null; then
        kill -INT "$rtt_pid" 2>/dev/null || true
        wait "$rtt_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

echo ">> building firmware"
cargo build --release -p meteo-firmware

echo ">> flashing + capturing RTT -> $RTT_LOG"
probe-rs run --chip "$CHIP" "$BIN" >"$RTT_LOG" 2>&1 &
rtt_pid=$!

echo ">> starting HCI trace on $GAIA -> $HCI_LOG"
# Plain btmon streams a decoded, human-readable trace to stdout (no btsnoop file
# on Gaia's disk). stdbuf -o0: btmon is glibc-buffered, so force unbuffered
# output or the pipe loses everything not yet flushed when the capture stops.
ssh "$GAIA" doas stdbuf -o0 btmon >"$HCI_LOG" 2>"$LOGDIR/btmon-stderr-$TS.log" &
btmon_pid=$!

echo ">> running meteo-cli central on $GAIA (retries until device advertises)"
cli_rc=1
for ((i = 1; i <= MAX_FIND_ATTEMPTS; i++)); do
    echo "   attempt $i/$MAX_FIND_ATTEMPTS"
    attempt_log="$(mktemp)"
    set +e
    timeout "$SESSION_TIMEOUT" ssh "$GAIA" \
        "bash -c 'cd $GAIA_REPO && cargo run -q -p meteo-cli --target $HOST_TARGET'" \
        2>&1 | tee "$attempt_log"
    cli_rc=${PIPESTATUS[0]}
    set -e
    cat "$attempt_log" >>"$CLI_LOG"
    # Retry transient pre-session failures, but not a genuine error (e.g. a
    # missing characteristic), which should surface immediately:
    #   - "MeteoStation not found": device not advertising yet.
    #   - "le-connection-abort-by-local": BlueZ aborts the connection attempt
    #     locally (HCI 0x3e, Connection Failed to be Established) — a recurring
    #     Gaia controller establishment flake, unrelated to the firmware.
    #   - "Error.Timeout"/"Timeout waiting for reply": BlueZ D-Bus call times
    #     out mid-connect, another face of the same establishment flake.
    if [[ $cli_rc -ne 0 ]] &&
        grep -Eq "MeteoStation not found|le-connection-abort-by-local|Timeout waiting for reply" "$attempt_log"; then
        rm -f "$attempt_log"
        continue
    fi
    rm -f "$attempt_log"
    break
done

echo ">> session ended (cli rc=$cli_rc); stopping captures"
cleanup
trap - EXIT INT TERM

echo
echo "Capture complete:"
echo "  RTT : $RTT_LOG"
echo "  CLI : $CLI_LOG"
echo "  HCI : $HCI_LOG"
