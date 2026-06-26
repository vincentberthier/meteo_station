#!/usr/bin/env bash
# ble_broadcast_check.sh — BLE broadcast data-flow check for gaia (BlueZ 5.86)
#
# Purpose:
#   Listens for extended advertising packets from the ESP32-H2 MeteoStation,
#   counts ManufacturerData updates whose company-id 0xFFFF entry is FRAME_LEN
#   bytes with byte[0] == FRAME_VERSION, and asserts at least MIN_FRAMES
#   well-formed frames arrived in WINDOW_SECS. Exits 0 on PASS, non-zero on FAIL.
#
# Why ManufacturerData polling (not AcquireNotify):
#   Telemetry is now BROADCAST: the firmware encodes sensor readings into the
#   ManufacturerData field of extended connectable advertising packets
#   (company-id 0xFFFF).  No GATT connection or subscription is required.
#   AcquireNotify was the correct tool for the old notify-characteristic model
#   and is no longer relevant.  Because the uptime_s field increments every
#   second, the manufacturer data payload changes with every advertisement —
#   this defeats BlueZ's ManufacturerData property dedup (which only suppresses
#   re-emitting unchanged values) so each second's frame is distinct and
#   countable via per-second property polling.
#
# Environment knobs (all optional, shown with defaults):
#   DEVICE           — BLE address of the peripheral          (F0:CA:FE:00:00:01)
#   ADAPTER          — local HCI adapter name                 (hci0)
#   WINDOW_SECS      — advertisement capture window in seconds (15)
#   MIN_FRAMES       — minimum frames required to PASS        (5)
#   FRAME_LEN        — expected manufacturer-data payload length in bytes (38)
#   FRAME_VERSION    — expected byte[0] frame-version tag     (5)
#   COMPANY_ID       — BLE company identifier, decimal        (65535)  # 0xFFFF
#
# Requires on gaia:
#   bluetoothctl, python3 with the `dbus` bindings (python-dbus), date
#
# Deploy and run:
#   scp scripts/ble_broadcast_check.sh gaia:
#   ssh gaia ./ble_broadcast_check.sh
#
# Discovery / the no-scan rule:
#   One bounded, self-terminating `bluetoothctl --timeout WINDOW_SECS scan on`
#   is started in the background.  The `--timeout` flag causes bluetoothctl to
#   issue StopDiscovery itself when it exits — it NEVER wedges the adapter in
#   "Discovering: yes".  Do NOT use `timeout … btmgmt find` or a bare `scan on`
#   without `--timeout` — both leave the adapter stuck discovering.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (env-overridable)
# ---------------------------------------------------------------------------
DEVICE="${DEVICE:-F0:CA:FE:00:00:01}"
ADAPTER="${ADAPTER:-hci0}"
WINDOW_SECS="${WINDOW_SECS:-15}"
MIN_FRAMES="${MIN_FRAMES:-5}"
FRAME_LEN="${FRAME_LEN:-38}"
FRAME_VERSION="${FRAME_VERSION:-5}"
COMPANY_ID="${COMPANY_ID:-65535}"

export DEVICE ADAPTER WINDOW_SECS MIN_FRAMES FRAME_LEN FRAME_VERSION COMPANY_ID

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# log MSG — timestamped line to stdout
log() {
    printf '%s %s\n' "$(date -Is)" "$*"
}

# fail REASON — log failure and exit non-zero
fail() {
    log "FAIL: $1"
    exit 1
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

trap 'log "interrupted"; exit 0' INT

log "Starting bounded ${WINDOW_SECS}s scan for ManufacturerData from ${DEVICE} …"

# Start a bounded, self-terminating scan in the background.
# bluetoothctl --timeout exits cleanly and calls StopDiscovery itself.
bluetoothctl --timeout "$WINDOW_SECS" scan on >/dev/null 2>&1 &
SCAN_PID=$!

# Run the frame counter for the full window.  The reader handles the
# "device not yet visible" case internally via per-iteration exception handling.
rc=0
python3 - <<'PYEOF' || rc=$?
import os, sys, time
import dbus

dev_addr      = os.environ["DEVICE"]
adapter       = os.environ["ADAPTER"]
window        = float(os.environ["WINDOW_SECS"])
min_frames    = int(os.environ["MIN_FRAMES"])
frame_len     = int(os.environ["FRAME_LEN"])
frame_version = int(os.environ["FRAME_VERSION"])
company_id    = int(os.environ["COMPANY_ID"])

bus = dbus.SystemBus()
dev_path = "/org/bluez/%s/dev_%s" % (adapter, dev_addr.replace(":", "_"))
props = dbus.Interface(bus.get_object("org.bluez", dev_path),
                       "org.freedesktop.DBus.Properties")

good = bad = 0
last_data = None


def check_mfr(mfr_data):
    """Count a ManufacturerData update if it carries a new, valid v5 frame."""
    global good, bad, last_data
    data = None
    for k, v in mfr_data.items():
        if int(k) == company_id:
            data = bytes(v)
            break
    if data is None:
        return
    if data == last_data:
        return  # unchanged — not a new frame
    last_data = data
    if len(data) == frame_len and data[0] == frame_version:
        good += 1
    else:
        bad += 1
        print("  malformed: len=%d byte[0]=0x%02x" %
              (len(data), data[0] if data else 0), file=sys.stderr)


end = time.time() + window

# Seed from the initial ManufacturerData property so a device already in the
# BlueZ cache contributes its cached frame before the poll loop begins.
try:
    check_mfr(props.Get("org.bluez.Device1", "ManufacturerData"))
except dbus.exceptions.DBusException:
    pass  # device not yet in cache; the background scan will populate it

# Poll every 0.3 s for the duration of the window.  Each iteration reads the
# current property value; check_mfr only counts a frame when the value
# changes (i.e. each distinct uptime_s tick = one new broadcast frame).
while time.time() < end:
    time.sleep(0.3)
    try:
        check_mfr(props.Get("org.bluez.Device1", "ManufacturerData"))
    except dbus.exceptions.DBusException:
        pass  # device transiently absent; keep polling

print("frames: %d valid, %d malformed in %.0fs (need %d)" %
      (good, bad, window, min_frames))
if bad:
    sys.exit(2)
sys.exit(0 if good >= min_frames else 3)
PYEOF

# Wait for the bounded scan to self-terminate (it runs for WINDOW_SECS).
wait "$SCAN_PID" 2>/dev/null || true

case "$rc" in
    0) log "PASS: broadcast frames flow correctly"; exit 0 ;;
    2) fail "malformed frame(s) received — wrong length or byte[0] != 0x05" ;;
    3) fail "too few valid frames in ${WINDOW_SECS}s; need ${MIN_FRAMES}" ;;
    *) fail "frame reader error (device not visible / ManufacturerData unavailable)" ;;
esac
