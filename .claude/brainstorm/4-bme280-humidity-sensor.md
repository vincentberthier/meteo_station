# Brainstorm: BME280 Humidity Sensor Integration

- **ID:** 4
- **Category:** Feature
- **Date:** 2026-06-17
- **Status:** Active

## Context

The on-chip BLE telemetry path is working (advertises as `MeteoStation`, 1 Hz
notify, link holds). Time to add the second sensor. The natural next step is the
**BME280**, used **strictly for humidity** — the BMP388 is the better barometer
and thermometer, so it stays authoritative for pressure and temperature.

## Current State

- **One sensor today.** `bmp.rs::read_barometer` owns the `I2c<'static, Async>`
  bus by value, reads the BMP388 at 1 Hz, builds a `Telemetry` via
  `Telemetry::from_bmp388` (temp + pressure only, all else `None`), and pushes
  it with `crate::ble::TELEMETRY.signal(telem)`.
- **`TELEMETRY` is latest-wins.** It's a
  `Signal<CriticalSectionRawMutex, Telemetry>` (`ble.rs:137`). The BLE notify
  task does `TELEMETRY.wait().await` and notifies the central with whatever
  whole frame was signaled last.
- **The frame already has humidity.** `meteo-lib::ble::frame` v1 reserves
  bytes 5–6 for humidity (`u16` LE, centi-%RH, sentinel `u16::MAX` → `None`).
  Encode/decode and round-trip tests for the humidity field already pass. No
  wire-format change is needed.
- **BMP388 driver is hand-rolled** in `meteo-lib/src/sensors/bmp388.rs` against
  `embedded-hal-async` traits, with pure compensation logic and host tests.

## Findings

### Datasheet rationale (confirmed)

| Metric                       | BMP388            | BME280      | Authoritative |
| ---------------------------- | ----------------- | ----------- | ------------- |
| Pressure absolute accuracy   | ±0.50 hPa         | ±1.0 hPa    | BMP388 (2×)   |
| Pressure relative accuracy   | ±0.08 hPa (±66cm) | unspecified | BMP388        |
| Temperature accuracy (0–65C) | ±0.30 °C @25C     | ±0.50 °C    | BMP388        |
| Humidity                     | —                 | ±3 %RH      | BME280 (only) |

The BME280's pressure and temperature are redundant and less precise. Humidity
is its only unique capability. **Decision: humidity only.**

**Compensation nuance (constraint, not a choice):** the BME280 humidity
compensation formula depends on `t_fine`, which comes from the BME280's _own_
temperature reading. So the BME280 must still sample temperature internally
(Bosch "Humidity sensing" mode: `osrs_p`=skip, `osrs_t`=x1, `osrs_h`=x1, IIR
off, forced mode). That internal temperature is used only to compute humidity;
it is **not** reported in the frame.

### Wiring & address (provisioned, no conflict)

- BMP388 is at **`0x77`** (`BMP388_ADDR`, SDO tied high). **`0x76` is reserved
  for the BME280** — comment in `main.rs:38` and the pin doc both say so.
- BME280 at `0x76`: SDO → GND, CSB → VDDIO (selects I2C), VDD/VDDIO → 3V3.
- Same **I2C0 bus** (GPIO10 SDA / GPIO11 SCL), existing 4.7 kΩ pull-ups. **No
  new GPIO.** Pin doc treats this as "one bus, four sensors" with distinct
  addresses.

### Hardware gap (blocks on-device acceptance)

The BME280 in hand is the **bare 2.5×2.5 mm LGA chip** (solder-down), not a
breakout — nothing to put on the bus yet. This work is **firmware-first**; a
solderable board (e.g. a GY-BME280 breakout, which exposes header pins + the
SDO/CSB selects + a regulator) is needed before on-device acceptance. Until
then humidity simply reads as `None` on a station with no BME280 present.

### The real integration problem: merging multiple partial frames

The latest-wins `Signal` carries one _whole_ `Telemetry`. Two sensor tasks each
signaling their own partial frame would **clobber** each other — the central
would see humidity _or_ temp+pressure, never both. This is the same problem all
**6 future sensors** (humidity, sky-IR, light, wind speed/dir, rain, battery)
will hit. **Resolved direction: aggregator + channels** (see Scope).

## Scope

**In scope:**

- Hand-rolled `meteo-lib/src/sensors/bme280.rs` driver: `embedded-hal-async`,
  `no_std`, BME280 init (chip-id `0x60`, calibration read incl. the split
  H4/H5 packing, "Humidity sensing" forced-mode config), and the integer
  humidity compensation (with the required internal temperature/`t_fine` step).
  Pure compensation logic with host tests, matching the bmp388.rs pattern.
- A `Telemetry::from_bme280` (or equivalent) that populates **only**
  `humidity_pct`.
- **Aggregator refactor of the telemetry path:** introduce an aggregator task
  that owns the `TELEMETRY` signal. Each sensor task (BMP388, BME280, and future
  ones) sends its reading over an `embassy_sync` channel; the aggregator merges
  incoming readings into a single running `Telemetry` and publishes a merged
  frame. **The existing BMP388 task is reworked** to send on the channel instead
  of signaling `TELEMETRY` directly.
- **I2C0 bus sharing** so both sensor tasks use the one bus (the bus is
  currently moved by value into the BMP task).
- A new `bme::read_humidity` (or similar) Embassy task in the firmware, spawned
  in `main.rs` against `BME280_ADDR = 0x76`.
- Host tests for the BME280 compensation and the aggregator merge logic.

**Resolved behaviour decisions:**

- BME280 role: **humidity only** (internal temp used solely for `t_fine`).
- Driver: **hand-rolled in `meteo-lib`** (consistency with BMP388; no new dep).
- Merge model: **aggregator + channels** (scales to all future sensors; chosen
  over a shared `Mutex<Telemetry>` and over folding both Bosch parts into one
  task).
- Publish cadence: aggregator publishes a **merged frame at ~1 Hz**, decoupled
  from individual sensor read rates, so the BLE notify stays 1 Hz regardless of
  sensor count. (Default chosen to match the existing 1 Hz design; revisit if a
  different rate is wanted.)
- Missing/failed BME280: humidity stays **`None`** (frame sentinel), so a
  station with no BME280 soldered, or a transient read error, degrades cleanly
  rather than stalling.

**Out of scope:**

- The other 5 sensors (light, sky-IR, wind, rain, battery) — but the aggregator
  is designed to accommodate them.
- Wire-format / frame changes (humidity field already exists).
- On-device acceptance (deferred — no breakout hardware yet).
- BME280 pressure/temperature in the frame (redundant; BMP388 owns those).

## Open Questions

Implementation-specific (for the planner):

- Channel topology: one shared multi-producer channel of a tagged
  `SensorReading` enum, vs. per-sensor signals the aggregator selects over.
- Bus-sharing mechanism: `embassy-embedded-hal` `I2cDevice` over a
  `Mutex<RawMutex, I2c>`, vs. another approach. Which `RawMutex`.
- Where the aggregator's merged `Telemetry` lives (the running state it updates
  per incoming reading) and how the ~1 Hz publish tick is driven.
- Exact watchdog wiring: today `BMP_BEAT` is bumped per BMP read; decide the
  heartbeat story for the BME280 task and/or the aggregator.
- BME280 calibration parsing layout (the H4/H5 12-bit packed coefficients) and
  fixed-point vs. float compensation in `no_std`.

## Next Steps

- Run `/tyrex:code:plan-light 4` to turn this into an implementation plan.
- Procure a BME280 breakout (e.g. GY-BME280) to unblock on-device acceptance
  once the firmware lands.
