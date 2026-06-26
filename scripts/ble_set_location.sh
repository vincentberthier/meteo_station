#!/usr/bin/env bash
# ble_set_location.sh — Write coarse location to the MeteoStation BLE peripheral
#
# Usage:  ble_set_location.sh LAT LON [ALT_M]
#
#   LAT   — latitude in decimal degrees, e.g. 48.8566 (range -90..90)
#   LON   — longitude in decimal degrees, e.g. 2.3522 (range -180..180)
#   ALT_M — altitude in metres, e.g. 35 (signed, default 0)
#
# Purpose:
#   Computes the 10-byte PIN-authenticated write payload, connects to the
#   MeteoStation GATT configuration service, finds the location characteristic
#   (UUID 7e700011-…), and issues a write-with-response via python3+dbus.
#   The firmware stores the location in flash.  Exits 0 on confirmed write,
#   non-zero on any error (wrong PIN, GATT error, connection failure, etc.).
#
# Wire-frame layout (10 bytes, all fields little-endian):
#   bytes 0-3  PIN  u32 LE  — config PIN; firmware default 911
#   bytes 4-5  lat  i16 LE  — latitude  × 100, rounded to nearest integer
#   bytes 6-7  lon  i16 LE  — longitude × 100, rounded to nearest integer
#   bytes 8-9  alt  i16 LE  — altitude in whole metres
#
# Security caveat:
#   The PIN is transmitted in cleartext over BLE during the one-time
#   configuration connection.  The link is unencrypted (unbonded peripheral).
#   Use this script only in a physically secure environment.  The PIN exists
#   to prevent accidental writes, not to provide cryptographic protection.
#
# Wrong PIN behaviour:
#   The firmware rejects the ATT write with an application error code.  BlueZ
#   surfaces this as a D-Bus "org.bluez.Error.Failed" exception; the python
#   reader catches it, prints a "wrong PIN?" hint, and exits non-zero.
#
# Environment knobs (all optional, shown with defaults):
#   DEVICE           — BLE address of the peripheral          (F0:CA:FE:00:00:01)
#   ADAPTER          — local HCI adapter name                 (hci0)
#   CHAR_UUID        — location characteristic UUID           (7e700011-b1df-42a1-bb5f-6a1028c793b0)
#   CONNECT_TIMEOUT  — per-step deadline in seconds           (30)
#   PIN              — configuration PIN (decimal)            (911)
#
# Requires on gaia:
#   bluetoothctl, busctl, python3 with the `dbus` bindings (python-dbus),
#   awk, date
#
# Deploy and run:
#   scp scripts/ble_set_location.sh gaia:
#   ssh gaia ./ble_set_location.sh 48.8566 2.3522 35

set -euo pipefail

# ---------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------
if [ $# -lt 2 ]; then
    printf 'Usage: %s LAT LON [ALT_M]\n' "$(basename "$0")" >&2
    exit 1
fi

LAT="$1"
LON="$2"
ALT="${3:-0}"

# ---------------------------------------------------------------------------
# Configuration (env-overridable)
# ---------------------------------------------------------------------------
DEVICE="${DEVICE:-F0:CA:FE:00:00:01}"
ADAPTER="${ADAPTER:-hci0}"
CHAR_UUID="${CHAR_UUID:-7e700011-b1df-42a1-bb5f-6a1028c793b0}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-30}"
PIN="${PIN:-911}"

DBUS_PATH="/org/bluez/${ADAPTER}/dev_${DEVICE//:/_}"

# ---------------------------------------------------------------------------
# Compute and validate the coordinate payload
# ---------------------------------------------------------------------------
# awk rounds the values, validates ranges, converts to two's-complement i16 / u32
# LE, and prints a 20-hex-digit string (10 bytes).
#
# Range constraints enforced here:
#   |lat_c| ≤ 9000   (90.00° × 100)
#   |lon_c| ≤ 18000  (180.00° × 100)
#   |alt_m| ≤ 32767  (i16 positive max; negative altitudes are valid too)
HEX_PAYLOAD=$(awk -v lat="$LAT" -v lon="$LON" -v alt="$ALT" -v pin="$PIN" '
BEGIN {
    # Round to nearest integer with correct sign handling.
    lat_c = int(lat * 100 + (lat >= 0 ? 0.5 : -0.5))
    lon_c = int(lon * 100 + (lon >= 0 ? 0.5 : -0.5))
    alt_m = int(alt        + (alt >= 0 ? 0.5 : -0.5))
    pin_v = int(pin)

    if (lat_c < -9000 || lat_c > 9000) {
        printf "error: lat_c=%d out of range [-9000,9000] (lat must be in [-90,90])\n",
               lat_c > "/dev/stderr"
        exit 1
    }
    if (lon_c < -18000 || lon_c > 18000) {
        printf "error: lon_c=%d out of range [-18000,18000] (lon must be in [-180,180])\n",
               lon_c > "/dev/stderr"
        exit 1
    }
    if (alt_m < -32768 || alt_m > 32767) {
        printf "error: alt_m=%d out of i16 range [-32768,32767]\n",
               alt_m > "/dev/stderr"
        exit 1
    }

    # Two'"'"'s-complement unsigned representation for negative i16 values.
    lat_u = (lat_c < 0) ? lat_c + 65536 : lat_c
    lon_u = (lon_c < 0) ? lon_c + 65536 : lon_c
    alt_u = (alt_m < 0) ? alt_m + 65536 : alt_m

    # Pack: PIN u32 LE | lat i16 LE | lon i16 LE | alt i16 LE  (10 bytes total)
    printf "%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x",
        pin_v               % 256,
        int(pin_v / 256)    % 256,
        int(pin_v / 65536)  % 256,
        int(pin_v / 16777216) % 256,
        lat_u % 256,
        int(lat_u / 256) % 256,
        lon_u % 256,
        int(lon_u / 256) % 256,
        alt_u % 256,
        int(alt_u / 256) % 256
}
')

export HEX_PAYLOAD CHAR_UUID DEVICE ADAPTER CONNECT_TIMEOUT

# ---------------------------------------------------------------------------
# Helpers (same pattern as ble_soak.sh)
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

log "Payload: ${HEX_PAYLOAD}  (PIN=${PIN}, lat=${LAT}°, lon=${LON}°, alt=${ALT}m)"
log "Ensuring ${DEVICE} is in the BlueZ cache …"
ensure_cached || fail "${DEVICE} not in BlueZ cache within ${CONNECT_TIMEOUT}s (powered/advertising?)"

log "Connecting to ${DEVICE} …"
wait_connected || fail "could not connect within ${CONNECT_TIMEOUT}s"
log "Connected.  Writing location to ${CHAR_UUID} …"

# Write the payload via python3+dbus: wait for GATT services to resolve,
# find the characteristic by UUID, and issue a WriteValue (write-with-response).
rc=0
python3 - <<'PYEOF' || rc=$?
import os, sys, time
import dbus

dev_addr    = os.environ["DEVICE"]
adapter     = os.environ["ADAPTER"]
char_uuid   = os.environ["CHAR_UUID"].lower()
hex_payload = os.environ["HEX_PAYLOAD"]
timeout     = int(os.environ["CONNECT_TIMEOUT"])

payload = bytes.fromhex(hex_payload)

bus = dbus.SystemBus()
dev_path = "/org/bluez/%s/dev_%s" % (adapter, dev_addr.replace(":", "_"))
props = dbus.Interface(bus.get_object("org.bluez", dev_path),
                       "org.freedesktop.DBus.Properties")

# Wait for GATT service discovery to finish.
deadline = time.time() + timeout
while True:
    try:
        if bool(props.Get("org.bluez.Device1", "ServicesResolved")):
            break
    except dbus.exceptions.DBusException:
        pass
    if time.time() > deadline:
        print("FAIL: GATT services did not resolve in %ds" % timeout, file=sys.stderr)
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

# WriteValue with an empty options dict defaults to write-with-response for
# characteristics that advertise the Write property.
try:
    char.WriteValue([dbus.Byte(b) for b in payload], {})
    print("OK: location written (%d bytes)" % len(payload))
    sys.exit(0)
except dbus.exceptions.DBusException as exc:
    print("FAIL: WriteValue error — %s" % str(exc), file=sys.stderr)
    err = str(exc)
    if "AuthenticationFailed" in err or "NotPermitted" in err or "Failed" in err:
        print("  -> wrong PIN?  The firmware rejects writes with an incorrect PIN.",
              file=sys.stderr)
    sys.exit(1)
PYEOF

cleanup

case "$rc" in
    0) log "PASS: location written successfully"; exit 0 ;;
    *) fail "GATT write failed — check PIN, connection, and firmware location-write support" ;;
esac
