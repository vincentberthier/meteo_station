# Brainstorm: BLE Module Design (RN4871)

- **ID:** 1
- **Category:** Architecture
- **Date:** 2026-06-14
- **Status:** Active

## Context

Third attempt at designing the BLE link for the weather station. An app must be
able to read the sensor values (and more) the device reports. Sampling is 1 Hz,
so the link has no throughput pressure. The device should **only push** data (or
answer queries — the direction doesn't matter), must be **always advertising
when not connected**, and must run **unattended for hours-to-days with no way to
trigger an external reset**. `meteo-tui` is a throwaway proof-of-concept consumer
and may be reworked freely.

Design was driven by current research (btleplug, RN4871 user guide + forums,
embedded-Rust BLE ecosystem), **not** by prior attempts — which were explicitly
out of scope and not consulted. Everything below refers only to the current
(post-purge) changeset.

## Current State

**Device side** (`crates/meteo-firmware`, STM32H753ZI + Embassy, `no_std`):

- BMP388 sampled at 1 Hz over I2C1 (`bmp.rs`, forced mode).
- LEDs (`leds.rs`).
- `CLAUDE.md` reserves **USART2** (PD5 TX, PD6 RX, PD3 CTS, PD4 RTS) and a
  **`RST_N` GPIO on PA4** for the RN4871. No BLE code currently exists.

**Consumer side** (`crates/meteo-tui`, tokio + ratatui):

- Clean transport seam in `feed.rs`: it emits `ClientEvent::{Connected,
Disconnected, Reading { index, raw }}` to the UI, but **no transport is
  wired** — it idles until shutdown.
- `sensors.rs` is a presentation registry (`SENSORS`: Temperature `°C`,
  Pressure `hPa` via `pa_to_hpa`). The feed maps incoming readings onto registry
  indices. Adding a sensor is one entry.
- `app.rs` keeps rolling per-sensor history and connection status. Explicitly
  disposable.

**Test host:** the dev machine has no Bluetooth radio. **gaia** does — controller
`D8:F3:BC:63:2E:56`, powered, **BlueZ 5.86**, **Rust 1.93** — reachable over SSH
(no password). Nothing on gaia is to be rebooted.

## Findings

### Topology — two halves

1. **Peripheral:** STM32H753 host drives the **RN4871** over USART2 using its
   ASCII command set (115200 8N1). The RN4871 owns the BLE stack/radio; the host
   configures it, feeds it 1 Hz telemetry, and supervises it.
2. **Central:** `meteo-tui` (or any app) is the BLE central, using **`bluer`**
   (the official Linux-only BlueZ binding, tokio-based). It scans, connects,
   discovers the GATT service, subscribes to the Notify characteristic, decodes
   frames, and feeds the existing `ClientEvent` seam. Tested against gaia's
   radio. `bluer` is chosen over `btleplug` because `meteo-tui` only ever runs
   on Linux (so btleplug's cross-platform support buys nothing), and btleplug's
   BlueZ backend has documented reliability bugs that hit this exact use case:
   `adapter.events()` going silent on long runs (#332) and flaky
   notifications/reconnect (#165). `bluer` is more complete and better-behaved
   for a long-running, reconnect-heavy consumer.

### RN4871 robustness for an always-on, unattended peripheral

- **Re-advertise (primary):** host watches the UART for `%DISCONNECT%` and
  re-issues the `A` advertise command. Zero reboot latency.
- **Reboot-after-disconnect (fallback):** feature bit `SR` 0x0080 makes the
  module reboot (and thus re-advertise) on its own after a drop.
- **Wedge recovery:** if the module stops emitting events / stops answering
  commands, the host pulses **`RST_N` (PA4)** low, waits for `%REBOOT%`, and
  reconfigures. This is the "no external reset needed" guarantee — the STM32 is
  the supervisor.
- **Firmware:** target **≥ v1.40** (earlier versions have Android
  private-addressing bugs and command-mode-entry reboots; v1.28 is the practical
  minimum). Check at bring-up with `V`.
- **Config persistence:** `SR/SN/SS/...` set-commands need `WR` (or a reboot) to
  survive power loss.
- **Flow control:** HW CTS/RTS (`SR` 0x8000) is **not** required at 115200 for
  1 Hz traffic; the CTS/RTS pins are wired and can be enabled later if RX
  overruns ever appear.
- **Single connection:** the RN4871 accepts one central; re-advertise after it
  disconnects. Matches the chosen consumer model.

### Data contract — custom GATT, one framed Notify characteristic

The device exposes **one custom GATT service** with a **single Notify
characteristic** carrying a **self-describing, extensible framed packet** of
sensor readings. Decided over the alternatives:

- vs. one-characteristic-per-sensor: avoids GATT/handle churn every time a sensor
  is added; the central subscribes once.
- vs. RN4871 Transparent UART pipe: gives real GATT structure and a stable,
  documented contract instead of an opaque proprietary-UUID byte pipe.

On the firmware the characteristic is defined with `PS`/`PC` and updated with
`SHW,<handle>,<hex>` (a local-characteristic write triggers the Notify). Each
frame carries a sensor identifier + value so it maps directly onto the existing
`ClientEvent::Reading { index, raw }` seam and the `SENSORS` registry.

**Values the frame carries** (the contract; per-sensor hardware/drivers are out
of scope here):

1. Temperature (°C) — BMP388 / BME280
2. Pressure (hPa) — BMP388 / BME280
3. Humidity (%RH) — BME280
4. Sky / IR temperature (°C) — MLX90614, used as a cloud-coverage proxy
5. Luminosity (lux) — VEML7700
6. Wind speed — weather meter
7. Wind direction — weather meter
8. Battery level — ADC

**Hard RN4871 constraint (verified):** a custom characteristic defined with `PC`
is **capped at 20 bytes** (1–20 octets, explicit in the User Guide DS50002466).
The module exposes **no ATT-MTU command**; its partial DLE (151 bytes, v1.28+)
benefits only the built-in Transparent UART stream — it does **not** lift the
20-byte cap on custom characteristics. So a single custom-characteristic
notification carries **at most 20 bytes**, full stop.

**Frame encoding (RESOLVED — not deferred):** a naïve `1-byte id + 4-byte f32`
per value would be ~40 bytes (≈ 2× the cap) and is rejected. Instead the frame is
a **versioned, fixed-schema, scaled-integer packet sized to fit one 20-byte
notification**:

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

≈ 17 bytes — inside the 20-byte cap with ~3 bytes of headroom. **No
fragmentation, no DLE dependency, no Transparent UART.** Self-description is at
the frame level (the version byte); extend by bumping the schema version.

**Ceiling (recorded risk):** the 20-byte cap is a real ceiling, and the current
frame already uses ~17 of it. A materially larger roster or higher precision
would exceed 20 bytes and force a transport change — either multiple
characteristics, or switching this one characteristic to the Transparent UART
stream (151 bytes with DLE). The version byte lets the central detect and reject
a schema it doesn't understand if that day comes.

### Direction & security (resolved)

- **Push via Notify.** Notify beats periodic read for 1 Hz telemetry (half the
  packets, lower latency, deterministic cadence) and is the natural "device only
  pushes" model.
- **Open link, no pairing.** Weather telemetry isn't sensitive; skip
  bonding/encryption for now to keep the PoC simple.

### Hardware decision (resolved)

Stay with the **RN4871** — it is physically wired and in the pin map. Noted
trade-off accepted: it has no maintained Rust driver (the firmware ASCII layer is
ours to write) and documented firmware interop quirks.

A switch to **nRF52840 as the sole MCU** (native BLE via `nrf-softdevice`,
replacing the STM32H753 entirely) was explicitly re-evaluated and **declined**.
It would be materially better _for the BLE goal_ — it deletes the entire
ASCII-driver + UART-supervisor + `RST_N` wedge-watchdog layer in favour of a
Bluetooth-qualified on-chip stack driven by the most production-proven Rust BLE
library — and the cost is bounded (the `embedded-hal-async` sensor drivers and
`meteo-tui` port unchanged; only `meteo-firmware`'s `embassy-stm32` →
`embassy-nrf` init is rewritten; same `thumbv7em` target). It was declined
because the planned sensor roster needs more peripherals and I/O than a small
nRF52840 board can host — the STM32H753's peripheral and pin count is required,
so offloading BLE to a dedicated module (the RN4871) and keeping the H7 as the
sensor host is the right split. If the RN4871 path proves unstable again,
nRF52840-as-sole-MCU remains the documented BLE fallback (at the cost of
re-solving the sensor-I/O budget). Other alternatives
(`STM32 + HCI controller + trouble-host`, ESP32 Rust BLE) were considered and
set aside as less mature.

## Scope

**In scope**

- A hardware-agnostic **RN4871 driver in `meteo-lib`** over `embedded-io-async`
  UART traits: command/response handling, command-vs-data mode, status-event
  parsing (`%CONNECT%`, `%DISCONNECT%`, `%REBOOT%`, `%STREAM_OPEN%`), one-time/
  boot-time provisioning, custom GATT definition, and characteristic push.
- A **firmware BLE task** wiring USART2 + `RST_N`, fed sensor readings from the
  sampling task via an `embassy_sync` channel; owns advertising supervision,
  disconnect re-advertise, and the `RST_N` wedge watchdog.
- A **self-describing wire frame** carrying sensor id + value, extensible to the
  future sensor set (BME280, MLX90614, VEML7700, weather meter, …).
- A **central transport for `meteo-tui`** (btleplug or bluer) that scans,
  connects, subscribes, decodes frames, drives `ClientEvent`, and reconnects on
  disconnect — validated on gaia.

**Out of scope**

- Pairing/bonding/encryption, multi-central support, BLE throughput beyond 1 Hz.
- Switching away from the RN4871 hardware.
- Reading or reviving any prior BLE attempt.
- The per-sensor **hardware bring-up and drivers** for the roster (the list of
  values the frame _carries_ is in scope — see the data contract — but acquiring
  them is separate work).

## Open Questions

Implementation-specific (for the planner). The data contract — characteristic
topology, value roster, and the exact frame layout — is **resolved above**, not
here.

1. **Service & characteristic UUIDs** — pick the custom 128-bit UUIDs and the
   `PC` property bitmap (Notify; `PC` size = 20 / `0x14`).
2. **RN4871 driver internals** — async command/response parser, `$$$` / `---`
   timing and guards (`SR` bits 0x4000 "No Prompt", 0x0008 "Command Mode Guard"),
   status-event delimiter parsing, and how readings are serialized to `SHW`.
3. **Provisioning model** — configure-on-every-boot vs one-time `WR` provisioning
   plus a verify-and-repair check at startup; firmware-version gate via `V`.
4. **Supervisor/watchdog state machine** — re-advertise on `%DISCONNECT%`,
   timing for declaring the module wedged, `RST_N` pulse + recovery sequence.
5. **Embassy task structure** — channel type/capacity between the sampling task
   and the BLE task; behaviour when no central is connected (drop vs latest-wins).
6. **Central reconnect** — `bluer` reconnect state machine (detect disconnect,
   rescan, fresh device handle, rediscover, resubscribe); frame decode →
   `SENSORS` index mapping. (Crate choice resolved: `bluer`.)
7. **USART2 config** — confirm 115200 8N1, flow control initially off; Embassy
   `Uart`/buffered-UART setup and the `RST_N` GPIO.

## Next Steps

Run `/tyrex:code:plan-light 1` to turn this into an implementation plan.
