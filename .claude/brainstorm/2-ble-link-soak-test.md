# Brainstorm: BLE Link Soak Test (no GATT)

- **ID:** 2
- **Category:** Feature
- **Date:** 2026-06-15
- **Status:** Active

## Context

Fourth attempt at the BLE link. The previous three were all purged. This one is
deliberately stripped to the bone: **no GATT, no services, no telemetry, no
framing.** The only goal is a link that behaves:

1. The device (RN4871 peripheral) **always advertises connectably when it is not
   connected**, and re-advertises immediately after any disconnect.
2. The link **stays up, uninterrupted, for a full 6-minute hold** — a drop before
   the 6-minute mark is a failure.
3. A **gaia-side bash script** drives the cycle: connect → hold 6 min →
   disconnect → wait 90 s → reconnect → repeat, indefinitely, until Ctrl-C.
4. The script is **self-validating**: it polls the link every second during the
   hold and treats any mid-window drop, or a failed reconnect, as a hard FAIL
   (logged, non-zero exit). If the device fails to re-advertise during the 90 s
   gap, the next connect fails and that is reported as FAIL.

Hard rule from the user, adopted here: **do not blame the hardware or the hosts.**
The RN4871, gaia, and the dev machine all work. Where the link is RF-marginal,
that is a constraint the design accommodates with connection parameters — not an
excuse.

## Current State

Post-purge, the tree has **no BLE code**. `crates/meteo-firmware/src/main.rs`
initialises only LEDs (PB0/PE1/PB14/PG2) and the BMP388 on I2C1; the main loop
just idles on a 5 s timer. `meteo-lib` has only the BMP388 driver and utils.
`docs/` and `scripts/` are empty.

The wiring and module facts survive in git history (revision `snlwmrollztk`,
recoverable):

- **RN4871 on USART2:** PD6 = RX, PD5 = TX, 115200 8N1. **RST_N on PA4**
  (active-low, deasserted high at init).
- **Device BLE address:** `80:1F:12:B6:60:BF`. **Module firmware:** v1.30.
- A full 1253-line RN4871 ASCII driver, a firmware BLE supervisor task, and a
  `bluer` central transport all exist in history. They are reference material —
  the parts worth salvaging are the command-mode entry, status-event parsing
  (`%CONNECT%` / `%DISCONNECT%` / `%REBOOT%`), and the `STA` / reset logic. The
  GATT, frame, and `SHW` push machinery is explicitly **not** wanted this time.

**gaia (the central):** BlueZ 5.86, controller `D8:F3:BC:63:2E:56`, reachable
over SSH (no password), `doas` available. Runs `blueman-manager`, which holds a
continuous discovery. Link to the station measured at **~-89/-91 dBm** — marginal
but workable with the right connection parameters. Never to be rebooted.

## Findings

### Why the previous three attempts kept failing (inferred from artifacts)

The git log shows five consecutive `fix(ble)` commits all fighting the same two
problems:

1. **Advertising silently dying.** On v1.30 firmware, `A,<interval>` stops
   advertising after ~30 s, leaving the device undiscoverable. The fix found
   late was `STA,00A0,0000,00A0` (100 ms fast interval, no fast→slow timeout) =
   advertise connectably forever, plus an idle keepalive re-arm and a
   `%DISCONNECT%`-triggered re-advertise.
2. **Service resolution aborting on the marginal link** (`le-connection-abort-by-
local`). The overbuilt GATT design needed the central to complete service
   discovery inside a tight window; at -89 dBm that window was too short unless
   gaia used a fast connection interval. This was the single most fragile part.

**The decisive simplification for attempt 4:** dropping GATT removes failure
mode #2 almost entirely. There is no custom service to resolve, no
characteristic to subscribe, no frame to decode. What remains is a pure
link-layer hold — far easier to make robust.

### Two halves of the deliverable

**Half A — minimal firmware (peripheral, `meteo-firmware` + small `meteo-lib`
driver):**

- Bring up the RN4871 over USART2: reset via RST_N, enter command mode,
  provision **`STA,00A0,0000,00A0`** for continuous connectable advertising,
  start advertising.
- Parse status events; on **`%DISCONNECT%`** re-advertise immediately; an idle
  keepalive periodically re-arms advertising as a belt-and-suspenders.
- On a wedged module (UART errors / no response), pulse **RST_N** and bring up
  again — this is the "no external reset needed" guarantee.
- **No** GATT service definition, **no** characteristic discovery, **no** `SHW`
  push, **no** sensor channel, **no** frame codec. None of it.

**Half B — gaia bash script (central, self-validating soak harness):**

- Before connecting, set gaia's fast connection parameters via `doas` (debugfs
  `conn_min_interval` / `conn_max_interval` / `supervision_timeout`). These reset
  on every `systemctl restart bluetooth` and on reboot, so the script reapplies
  them on each run. Even with no app GATT, BlueZ still auto-resolves the module's
  built-in GAP services once on connect; the fast interval is what lets that one
  resolution complete on the marginal link before the link is dropped. A
  multi-second supervision timeout is what lets the empty-PDU link survive bursts
  of packet loss at -89 dBm through the whole 6-minute hold.
- Drive one cycle: `bluetoothctl connect 80:1F:12:B6:60:BF`, confirm
  `Connected: yes`, then hold 6 min, polling `bluetoothctl info` (or the D-Bus
  `Connected` property) **every second**. Any drop before 6 min → FAIL.
- `bluetoothctl disconnect`, wait 90 s, reconnect. A reconnect that does not
  succeed within a bounded timeout → FAIL (device did not re-advertise / link
  unrecoverable).
- Loop indefinitely, printing a per-cycle PASS line, until Ctrl-C; clean up the
  connection on exit.

### Test-methodology traps (recorded so they are not re-hit)

- Do **not** probe discovery with `timeout N btmgmt find` — a SIGKILL mid-scan
  leaves `org.bluez` stuck `Discovering: yes` and wedges every later scan, which
  looks like "the station stopped radiating."
- `blueman-manager` on gaia already holds a continuous discovery, so a second
  `btmgmt find` cannot start. To check presence, query the cache:
  `bluetoothctl info 80:1F:12:B6:60:BF` and look for a live `RSSI:`.
- When testing the firmware with `probe-rs run`, never let it be SIGKILLed/timed
  out — send SIGINT so it detaches cleanly (per project CLAUDE.md).

### "No GATT" is honored on both sides

The user wants no GATT and no services. Firmware side: we define no custom
service or characteristic (the module keeps only its mandatory GAP). Script side:
it only opens and holds the LE link — it never browses, reads, or subscribes to
characteristics. BlueZ's one automatic resolution of the built-in GAP services on
connect is unavoidable plumbing, not an application GATT contract.

## Scope

**In scope**

- A **minimal RN4871 driver** in `meteo-lib` (or small module): reset, command
  mode, `STA` provisioning, start/restart advertising, status-event parsing.
  Salvage the reusable parts from history; drop everything GATT/frame related.
- A **firmware BLE task** wiring USART2 (PD6/PD5) + RST_N (PA4): bring-up,
  re-advertise on disconnect, idle keepalive, RST_N wedge recovery. No sensor
  data flows over the link.
- A committed **gaia bash script** (under `scripts/`) implementing the
  self-validating connect/hold/disconnect/reconnect soak loop, including the
  debugfs conn-param setup.
- Restoring the **pin/wiring documentation** (USART2 + RST_N + RN4871 addr) and
  the gaia test procedure in `CLAUDE.md`.

**Out of scope**

- GATT services/characteristics, Notify, frame codecs, sensor telemetry over BLE.
- Pairing/bonding/encryption, multi-central support.
- Switching away from the RN4871 hardware.
- Reviving the purged `meteo-tui` viewer or the `bluer` transport.
- Per-sensor data acquisition (the link carries no sensor data at all here).

## Resolved decisions (the "what" and "why")

- **Deliverable:** minimal advertising firmware **+** gaia bash script. A script
  alone cannot make the device advertise; the peripheral behavior is firmware.
- **Continuous advertising mechanism:** `STA,00A0,0000,00A0` + re-advertise on
  `%DISCONNECT%` + idle keepalive (proven approach from history; v1.30 `A,<iv>`
  alone goes silent).
- **Stability mechanism:** gaia fast conn interval + multi-second supervision
  timeout, set by the script before connect.
- **Harness behavior:** self-validating, fail-loud, non-zero exit on any
  mid-window drop or failed reconnect.
- **Run mode:** loop indefinitely until Ctrl-C, one PASS/FAIL line per cycle.
- **Script form:** bash + `bluetoothctl` (lightest; tools already on gaia).
- **Timings:** 6-minute hold, 90-second disconnect gap (per user).

## Open Questions

Implementation-specific only — for the planner:

1. **Exact debugfs conn-param values.** History used `6 / 12 / 500`
   (7.5 ms / 15 ms / 5 s). Confirm these are right for a no-GATT hold, or whether
   a longer supervision timeout buys more margin for the 6-min window without
   masking a genuine drop. (Behavior is settled; only the numbers are open.)
2. **Link-state polling source in bash.** `bluetoothctl info <addr>` parsed for
   `Connected: yes`, vs `dbus-send`/`busctl` reading the `Connected` property
   directly. Pick the one that is least racy under `blueman-manager`'s ongoing
   discovery.
3. **RN4871 driver internals.** Async command/response parser, `$$$`/`---` mode
   timing and guards, status-event delimiter parsing — how much to salvage
   verbatim from the history driver vs rewrite lean.
4. **Provisioning model.** Configure-on-every-boot vs one-time `WR`-persisted
   provisioning with a verify-and-repair check at startup.
5. **Wedge-detection thresholds.** When to declare the module wedged and pulse
   RST_N (UART error vs response timeout), and the recovery sequence timing.
6. **Embassy task / UART setup.** `BufferedUart` vs DMA UART, buffer sizes, the
   RST_N GPIO, and whether the BLE task needs any channel at all (it carries no
   data, so likely none).
7. **Reconnect/connect timeouts in the script.** Bounded wait for `Connected:
yes` after a connect, and how long to allow for re-advertise before declaring
   a failed reconnect.

## Next Steps

Run `/tyrex:code:plan-light 2` to turn this into an implementation plan.
