#!/usr/bin/env bash
# ble_notify_check.sh — GATT notify data-flow check for gaia (BlueZ 5.86)
#
# Purpose:
#   Connects to the on-chip ESP32-H2 BLE peripheral (MeteoStation), subscribes to
#   the telemetry characteristic, captures notifications for WINDOW_SECS, and
#   asserts at least MIN_FRAMES well-formed frames arrived (each FRAME_LEN bytes
#   with byte[0] == 0x02, the frame-version sentinel). Exits 0 on PASS, non-zero
#   on FAIL.
#
# Why BlueZ AcquireNotify (not bluetoothctl "notify on" output parsing):
#   bluetoothctl surfaces notifications via the org.bluez Value property, which
#   BlueZ only re-emits when the value CHANGES. The weather telemetry is nearly
#   constant at rest (temperature/pressure barely move), so BlueZ dedupes the
#   PropertiesChanged signals and the displayed value stream goes silent even
#   though notifications are flowing on-air. AcquireNotify hands back a raw socket
#   that delivers EVERY notification PDU with no value-dedup — the only reliable
#   way to count frames. (btmon would also work but needs root; AcquireNotify does
#   not.) This was confirmed on-device: 10/10 valid frames in a 10 s window.
#
# Environment knobs (all optional, shown with defaults):
#   DEVICE           — BLE address of the peripheral          (F0:CA:FE:00:00:01)
#   ADAPTER          — local HCI adapter name                 (hci0)
#   CHAR_UUID        — telemetry characteristic UUID          (7e700002-b1df-42a1-bb5f-6a1028c793b0)
#   CONNECT_TIMEOUT  — per-step deadline in seconds           (30)
#   WINDOW_SECS      — notification capture window in seconds (15)
#   MIN_FRAMES       — minimum notifications required to PASS (5)
#   FRAME_LEN        — expected payload length in bytes       (18)
#
# Requires on gaia:
#   bluetoothctl, busctl, python3 with the `dbus` bindings (python-dbus), date
#
# Deploy and run (same pattern as ble_soak.sh):
#   scp scripts/ble_notify_check.sh gaia:
#   ssh gaia ./ble_notify_check.sh
#
# Discovery / the no-scan rule:
#   The script prefers the device already being in the BlueZ cache via blueman's
#   standing discovery. If it is not cached, the script falls back to a single
#   bounded, self-terminating `bluetoothctl --timeout` discovery — this exits
#   cleanly (StopDiscovery on its own), unlike a `timeout … btmgmt find` SIGKILL
#   or a left-running `scan on`, both of which wedge the adapter in
#   "Discovering: yes". The adapter state is verified back to not-discovering
#   after the fallback.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (env-overridable)
# ---------------------------------------------------------------------------
DEVICE="${DEVICE:-F0:CA:FE:00:00:01}"
ADAPTER="${ADAPTER:-hci0}"
CHAR_UUID="${CHAR_UUID:-7e700002-b1df-42a1-bb5f-6a1028c793b0}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-30}"
WINDOW_SECS="${WINDOW_SECS:-15}"
MIN_FRAMES="${MIN_FRAMES:-5}"
FRAME_LEN="${FRAME_LEN:-18}"

export DEVICE ADAPTER CHAR_UUID WINDOW_SECS MIN_FRAMES FRAME_LEN

DBUS_PATH="/org/bluez/${ADAPTER}/dev_${DEVICE//:/_}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# log MSG — timestamped line to stdout
log() {
    printf '%s %s\n' "$(date -Is)" "$*"
}

# cleanup — best-effort disconnect; safe to call from a trap
cleanup() {
    bluetoothctl disconnect "$DEVICE" >/dev/null 2>&1 || true
}

# fail REASON — log failure, disconnect, exit non-zero
fail() {
    log "FAIL: $1"
    cleanup
    exit 1
}

# is_connected — returns 0 if the device D-Bus property Connected is true.
is_connected() {
    busctl get-property org.bluez "$DBUS_PATH" org.bluez.Device1 Connected \
        2>/dev/null | grep -q 'b true'
}

# device_known — preflight cache check (no scan).
device_known() {
    bluetoothctl info "$DEVICE" 2>/dev/null | grep -q 'Device '
}

# adapter_discovering — 0 if the adapter is currently scanning.
adapter_discovering() {
    bluetoothctl show "$ADAPTER" 2>/dev/null | grep -q 'Discovering: yes'
}

# ensure_cached — make sure the device object exists in BlueZ.
# Prefer blueman's standing discovery; if absent, run ONE bounded,
# self-terminating discovery, then verify the adapter stopped scanning.
ensure_cached() {
    if device_known; then
        return 0
    fi
    log "Device not cached; running a bounded ${CONNECT_TIMEOUT}s discovery …"
    bluetoothctl --timeout "$CONNECT_TIMEOUT" scan on >/dev/null 2>&1 &
    local n=0
    until device_known; do
        sleep 1
        n=$((n + 1))
        [ "$n" -ge "$CONNECT_TIMEOUT" ] && break
    done
    # Let the bounded scan self-terminate and confirm the adapter is idle again.
    local w=0
    while adapter_discovering; do
        sleep 1
        w=$((w + 1))
        [ "$w" -ge 5 ] && break
    done
    device_known
}

# wait_connected — issue connect, then bounded poll until Connected == true.
wait_connected() {
    bluetoothctl connect "$DEVICE" >/dev/null 2>&1 || true
    local n=0
    until is_connected; do
        sleep 1
        n=$((n + 1))
        [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 1
    done
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

trap 'log "interrupted"; cleanup; exit 0' INT

log "Ensuring $DEVICE is in the BlueZ cache …"
ensure_cached || fail "$DEVICE not in BlueZ cache within ${CONNECT_TIMEOUT}s (powered/advertising? blueman discovery running?)"

log "Connecting to $DEVICE …"
wait_connected || fail "could not connect within ${CONNECT_TIMEOUT}s"
log "Connected. Subscribing to $CHAR_UUID and capturing ${WINDOW_SECS}s …"

# The notification reader runs in python3: it waits for GATT services to resolve,
# finds the characteristic by UUID, calls AcquireNotify for a raw (un-deduped)
# notification socket, and counts well-formed frames. Exit code is the verdict.
python3 - <<'PYEOF'
import os, select, sys, time
import dbus

dev_addr   = os.environ["DEVICE"]
adapter    = os.environ["ADAPTER"]
char_uuid  = os.environ["CHAR_UUID"].lower()
window     = float(os.environ["WINDOW_SECS"])
min_frames = int(os.environ["MIN_FRAMES"])
frame_len  = int(os.environ["FRAME_LEN"])

bus = dbus.SystemBus()
dev_path = "/org/bluez/%s/dev_%s" % (adapter, dev_addr.replace(":", "_"))
props = dbus.Interface(bus.get_object("org.bluez", dev_path),
                       "org.freedesktop.DBus.Properties")

# Wait for GATT service discovery to finish.
deadline = time.time() + 15
while True:
    try:
        if bool(props.Get("org.bluez.Device1", "ServicesResolved")):
            break
    except dbus.exceptions.DBusException:
        pass
    if time.time() > deadline:
        print("FAIL: GATT services did not resolve in 15s", file=sys.stderr)
        sys.exit(1)
    time.sleep(0.2)

# Find the characteristic object path by UUID.
om = dbus.Interface(bus.get_object("org.bluez", "/"),
                    "org.freedesktop.DBus.ObjectManager")
char_path = None
for path, ifaces in om.GetManagedObjects().items():
    c = ifaces.get("org.bluez.GattCharacteristic1")
    if c and str(c.get("UUID", "")).lower() == char_uuid and path.startswith(dev_path):
        char_path = path
        break
if char_path is None:
    print("FAIL: characteristic %s not found under %s" % (char_uuid, dev_addr),
          file=sys.stderr)
    sys.exit(1)

char = dbus.Interface(bus.get_object("org.bluez", char_path),
                      "org.bluez.GattCharacteristic1")
fd_obj, mtu = char.AcquireNotify({})
fd = fd_obj.take()
mtu = int(mtu) or 64

good = bad = 0
poller = select.poll()
poller.register(fd, select.POLLIN)
end = time.time() + window
while time.time() < end:
    remaining_ms = max(0, (end - time.time()) * 1000)
    if not poller.poll(remaining_ms):
        continue
    data = os.read(fd, mtu)
    if len(data) == frame_len and data[0] == 0x02:
        good += 1
    elif data:
        bad += 1
        print("  malformed frame: len=%d first=0x%02x" %
              (len(data), data[0] if data else 0), file=sys.stderr)
os.close(fd)

print("frames: %d valid, %d malformed in %.0fs (need %d)" %
      (good, bad, window, min_frames))
if bad:
    sys.exit(2)
sys.exit(0 if good >= min_frames else 3)
PYEOF
rc=$?

cleanup

case "$rc" in
    0) log "PASS: telemetry notifications flow correctly"; exit 0 ;;
    2) fail "malformed frame(s) received — wrong length or byte[0] != 0x02" ;;
    3) fail "too few valid frames in ${WINDOW_SECS}s; need ${MIN_FRAMES}" ;;
    *) fail "notify reader error (GATT not ready / characteristic missing)" ;;
esac
