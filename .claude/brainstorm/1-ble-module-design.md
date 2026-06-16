# Brainstorm: BLE Telemetry Design (ESP32-H2 on-chip)

- **ID:** 1
- **Category:** Architecture
- **Date:** 2026-06-14 (revised 2026-06-16 for the ESP32-H2 port)
- **Status:** Active

## Context

Design of the BLE telemetry link for the weather station. An app must be able to
read the sensor values (and more) the device reports. Sampling is 1 Hz, so the
link has no throughput pressure. The device should **only push** data (or answer
queries — direction doesn't matter), must be **always advertising when not
connected**, and must run **unattended for hours-to-days with no way to trigger
an external reset**.

> **Revision note (2026-06-16) — STM32 → ESP32-H2 port.** This brainstorm was
> originally written as an _external-module_ design: an STM32H753ZI host driving a
> **RN4871** BLE module over USART2, with an `RST_N` GPIO wedge-watchdog. The
> firmware has since been ported to the **ESP32-H2-DevKitM-1**, whose **BLE 5.3 LE
> radio is on-chip**. That single hardware change deletes the entire external-module
> layer — RN4871, USART2, `RST_N`, the ASCII command driver, and the wedge
> watchdog all disappear. The peripheral is now native on-chip BLE driven in Rust
> via **esp-radio + trouble-host (TrouBLE)** on the existing esp-hal / esp-rtos /
> embassy stack. The sections below are rewritten for that target. The
> **data contract** (one custom service, one Notify characteristic, a
> self-describing framed packet, the 8-value roster) carries over almost intact;
> only the RN4871-specific 20-byte hard cap is relaxed (see Data Contract).

## Current State

**Device side** (`crates/meteo-firmware`, ESP32-H2 `riscv32imac`, `no_std`):

- BMP388 sampled over **I2C0** (SDA = GPIO10, SCL = GPIO11; `bmp.rs`).
- Status LED on **GPIO8** (external LED + onboard WS2812 share the line; driven as
  a plain GPIO, so the external LED blinks and the WS2812 stays dark).
- esp-hal 1.1 + esp-rtos 0.3 (thread-mode executor + embassy time driver) +
  embassy-executor/-time. `#[esp_rtos::main]` brings up the scheduler.
- **No BLE code exists.** `main.rs` initialises the BMP388 task and the LED blink
  loop and nothing else. Native BLE is the work this brainstorm scopes.

**`meteo-lib`** (hardware-agnostic, `embedded-hal-async` based):

- BMP388 driver, `utils`. Builds on host + target.
- ⚠️ **Leftover from the RN4871 era:** `meteo-lib/src/ble/rn4871.rs` (1053 lines),
  re-exported via `lib.rs` → `pub mod ble`, with host unit tests. It is the
  RN4871 ASCII command/event parser — command-mode entry, `STA`/`SHW`,
  `%CONNECT%`/`%DISCONNECT%` parsing. **None of it applies to the on-chip
  trouble-host path.** It is not compiled into firmware, only host-tested. See
  Findings → "BLE mess to clean up."

**Consumer side:** the former `crates/meteo-tui` (a tokio + ratatui central using
`bluer`) was **purged** in an earlier attempt and is not in the tree today. The
central is re-included in scope (see Scope) and will be rebuilt.

**Test host:** the dev machine has no Bluetooth radio. **gaia** does — controller
`D8:F3:BC:63:2E:56`, powered, **BlueZ 5.86**, reachable over SSH (no password),
running `blueman-manager` (standing discovery). gaia is the **current** acceptance
test host. Nothing on gaia is to be rebooted.

## Findings

### Topology — two halves (rewritten for on-chip BLE)

1. **Peripheral:** the ESP32-H2 runs the BLE stack **itself** — no external
   module. **esp-radio** provides the controller/driver (it enables BLE + 802.15.4
   on the H2 and requires esp-hal's `unstable` feature, already on) and
   **trouble-host (TrouBLE)** is the host-side GATT/GAP stack, the recommended
   pairing with esp-radio and embassy. The firmware defines the GATT server,
   advertises connectably, accepts a central, and pushes 1 Hz telemetry via
   Notify. esp-rtos is the task scheduler (already in use).
2. **Central:** a Linux app (revived `meteo-tui` or a fresh consumer) is the BLE
   central, using **`bluer`** (the official Linux-only BlueZ binding, tokio-based).
   It scans, connects, discovers the custom service, subscribes to the Notify
   characteristic, decodes frames, and feeds the UI. Validated against gaia's
   radio. `bluer` is chosen over `btleplug` because the consumer only ever runs on
   Linux (btleplug's cross-platform support buys nothing) and btleplug's BlueZ
   backend has documented reliability bugs for this exact use case:
   `adapter.events()` going silent on long runs (#332) and flaky
   notifications/reconnect (#165).

### Robustness for an always-on, unattended on-chip peripheral

The failure modes change completely versus the RN4871 design. There is **no
external module to wedge and no `RST_N` line to pulse** — the radio is on-chip and
managed by esp-radio inside the same firmware. The robustness mechanisms become:

- **Always-connectable advertising:** the trouble-host GAP loop re-enters
  advertising immediately whenever it is not connected (i.e. after every
  disconnect). This replaces the RN4871 `STA,00A0,0000,00A0` + `%DISCONNECT%`
  re-advertise dance — it is now just the peripheral's normal control flow.
- **Firmware-hang recovery:** if the firmware itself hangs, the recovery is the
  **ESP32-H2 RWDT (reset watchdog timer)** resetting the whole chip — not an
  external reset pin. This is the new "no external reset needed" guarantee; the
  chip is its own supervisor.
- **No firmware-version gate, no UART flow control, no command-mode guards.** All
  of that was RN4871 module plumbing and is gone. The relevant tuning knobs are
  now BLE connection parameters (interval / supervision timeout / PHY), set by the
  peripheral's preferred-connection-parameters and/or accepted from the central.
- **Single connection** (one central at a time) still matches the consumer model;
  re-advertise on disconnect.

### Data contract — custom GATT, one framed Notify characteristic (carried over)

The device exposes **one custom GATT service** with a **single Notify
characteristic** carrying a **self-describing, extensible framed packet** of
sensor readings. The rationale is unchanged by the port:

- vs. one-characteristic-per-sensor: avoids GATT/handle churn every time a sensor
  is added; the central subscribes once.
- The Notify push model fits "device only pushes" 1 Hz telemetry.

On trouble-host the characteristic is a server attribute; pushing a value is a
`notify()` on it. Each frame carries a sensor identifier + value so it maps onto a
clean consumer seam and a presentation registry on the central.

**Values the frame carries** (the contract; per-sensor hardware/drivers are out of
scope here):

1. Temperature (°C) — BMP388 / BME280
2. Pressure (hPa) — BMP388 / BME280
3. Humidity (%RH) — BME280
4. Sky / IR temperature (°C) — MLX90614, used as a cloud-coverage proxy
5. Luminosity (lux) — VEML7700
6. Wind speed — weather meter
7. Wind direction — weather meter
8. Battery level — ADC

**MTU / size — the RN4871 hard cap is GONE.** The original design was forced into
a tight 17-byte packet because a RN4871 custom characteristic was **hard-capped at
20 bytes** with no MTU command. On the ESP32-H2 the ATT MTU is **negotiable** (BLE
5.3, DLE), so a single notification can carry far more than 20 bytes when the
central negotiates a larger MTU. The constraint is relaxed from a hard ceiling to a
soft default: with no MTU negotiation the usable Notify payload is the default
`ATT_MTU 23 − 3 = 20 bytes`, so a packet that fits 20 bytes still works
everywhere with zero negotiation.

**Frame encoding (kept as the default, no longer forced):** the versioned,
fixed-schema, scaled-integer packet still fits the 20-byte default and stays the
recommended layout — it is compact, self-describing, and dependency-free:

| Field          | Encoding                       | Bytes  |
| -------------- | ------------------------------ | ------ |
| header         | `u8` schema/version            | 1      |
| temperature    | `i16` centi-°C                 | 2      |
| pressure       | `u16` deci-hPa (3000–11000)    | 2      |
| humidity       | `u16` centi-%RH                | 2      |
| sky / IR temp  | `i16` centi-°C                 | 2      |
| luminosity     | `u16` mantissa + `u8` exponent | 3      |
| wind speed     | `u16` cm/s                     | 2      |
| wind direction | `u16` deci-degree              | 2      |
| battery        | `u8` percent                   | 1      |
| **total**      |                                | **17** |

≈ 17 bytes — inside the 20-byte default with headroom, and now also extensible
_upward_ (negotiate a larger MTU) if the roster or precision grows, instead of
being forced into fragmentation. The version byte lets the central detect and
reject a schema it doesn't understand.

### Direction & security (resolved, unchanged)

- **Push via Notify.** Beats periodic read for 1 Hz telemetry (half the packets,
  lower latency, deterministic cadence) and is the natural "device only pushes"
  model.
- **Open link, no pairing.** Weather telemetry isn't sensitive; skip
  bonding/encryption for now to keep the design simple. (On the H2, native
  pairing/bonding via trouble-host is available later if wanted.)

### Hardware decision (resolved by the port)

**On-chip BLE on the ESP32-H2 — settled.** The original brainstorm re-evaluated
and _declined_ "nRF52840 as the sole MCU" (native on-chip BLE, deleting the whole
external-module layer). It declined it for **one** reason: the planned sensor
roster needed more peripherals and I/O than a small nRF board could host, so BLE
was offloaded to the RN4871 to keep the STM32H7 as the sensor host.

The ESP32-H2 port **dissolves that exact trade-off.** It has on-chip BLE 5.3 _and_
enough I/O for the full roster: 2× I2C, 2× UART, SPI, a 5-channel ADC, and 19
GPIOs — the complete weather-station wiring (BMP388 I2C, anemometer/rain-gauge
pulse inputs, wind-vane and battery ADC) already fits its pin map
(`datasheets/esp32_h2_devkitm.md`). So the on-chip path the original brainstorm
wanted but couldn't afford is now the chosen design, with no I/O penalty. The
RN4871, USART2, and `RST_N` are retired hardware.

### BLE mess to clean up (the leftovers from the port)

The STM32+RN4871 era left artifacts that no longer belong:

- **`meteo-lib/src/ble/rn4871.rs` (1053 lines) + its host tests + the `ble` module
  re-export** — recommend **removing** them as part of this BLE rework. The driver
  is RN4871-specific (ASCII command mode, `STA`/`SHW`, module status events) and
  has no role in the trouble-host design; keeping it as host-tested dead code is
  misleading. (Reversible from git history if ever needed.)
- **`CLAUDE.md` still documents the RN4871 path** (USART2 wiring, `RST_N`, the
  module-supervisor model, "RN4871 parser kept for host tests") and the gaia soak
  framed around the module. Those notes should be updated to the on-chip model
  once this design lands.
- **Brainstorm 2 (`2-ble-link-soak-test.md`) is also RN4871-based** — its `STA`
  advertising mechanism, `RST_N` wedge recovery, and module wiring are obsolete on
  the H2. Its _soak-harness methodology_ (the gaia connect/hold/disconnect/
  reconnect loop, the discovery-cache traps) stays valid and is the acceptance
  gate here (see Scope). Updating B2 itself is out of scope for this revision.

## Scope

**In scope**

- A **firmware BLE peripheral** on the ESP32-H2 using **esp-radio + trouble-host**:
  GATT server with one custom service + one Notify characteristic, connectable
  advertising that re-advertises on disconnect, and 1 Hz telemetry push. Fed sensor
  readings from the sampling task via an `embassy_sync` channel. RWDT as the
  firmware-hang backstop.
- A **self-describing wire frame** carrying sensor id + value, extensible to the
  full sensor set (BMP388 today; BME280, MLX90614, VEML7700, weather meter, battery
  later). Default = the 17-byte scaled-integer packet; MTU negotiable upward.
- A **central transport** (revived `meteo-tui` or a fresh Linux consumer using
  `bluer`) that scans, connects, discovers the service, subscribes, decodes frames,
  drives the UI, and reconnects on disconnect — validated on gaia.
- **Acceptance gate:** the **gaia BlueZ soak** (connect → hold 6 min → disconnect →
  90 s gap → reconnect, self-validating, fail-loud), now exercising the on-chip
  radio. gaia (BlueZ 5.86) is the current test host.
- **Cleanup:** remove the vestigial RN4871 driver/tests from `meteo-lib`; update
  `CLAUDE.md`'s BLE section to the on-chip model.

**Out of scope**

- Pairing/bonding/encryption, multi-central support, BLE throughput beyond 1 Hz.
- The per-sensor **hardware bring-up and drivers** for the roster (the list of
  values the frame _carries_ is in scope — see the data contract — but acquiring
  them is separate work).
- Reading or reviving any prior BLE _attempt_ beyond salvaging the gaia soak
  methodology.
- Updating brainstorm 2 (tracked separately).
- 802.15.4 / Thread / Zigbee (the H2's other radio) — BLE only here.

## Open Questions

Implementation-specific (for the planner). The data contract — characteristic
topology, value roster, default frame layout — is **resolved above**, not here.

1. **Service & characteristic UUIDs** — pick the custom 128-bit service/char UUIDs
   and the characteristic properties (Notify; optional Read for last value).
2. **esp-radio + trouble-host integration** — exact crate versions compatible with
   esp-hal 1.1 / esp-rtos 0.3 / embassy-executor 0.10; controller init, BLE
   feature flags on esp-radio, memory/heap allocation for the BLE stack, and how the
   trouble-host runner is scheduled alongside the esp-rtos executor.
3. **GATT server definition** — trouble-host's attribute-table / server macro
   layout for the one service + Notify characteristic, and how a sample becomes a
   `notify()`.
4. **Advertising & connection parameters** — adv interval, connectable adv payload
   (name + service UUID), preferred connection interval / supervision timeout / PHY
   for a stable hold on a possibly-marginal link; re-advertise-on-disconnect control
   flow.
5. **Embassy task structure** — channel type/capacity between the sampling task and
   the BLE task; behaviour when no central is connected (drop vs latest-wins).
6. **RWDT configuration** — feeding cadence and timeout for the firmware-hang
   backstop, and where the feed lives so a wedged BLE task actually trips it.
7. **Central transport** — revive `meteo-tui` vs fresh consumer; `bluer` reconnect
   state machine (detect disconnect, rescan, fresh device handle, rediscover,
   resubscribe); frame decode → presentation mapping. (Crate choice resolved:
   `bluer`.)
8. **Frame schema versioning** — initial version byte value and the
   forward/backward-compatibility rule the central applies on an unknown version.

## Next Steps

Run `/tyrex:code:plan-light 1` to turn this into an implementation plan.
