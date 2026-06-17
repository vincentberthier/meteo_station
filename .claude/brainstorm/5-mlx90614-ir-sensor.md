# Brainstorm: MLX90614 IR (Sky) Temperature Sensor Integration

- **ID:** 5
- **Category:** Feature
- **Date:** 2026-06-17
- **Status:** Active

## Context

The BMP388 barometer and the on-chip BLE telemetry path both work (advertises as
`MeteoStation`, 1 Hz notify, link holds). Time to grow the sensor set. Two parts
were on the table — BME280 (humidity) and VEML7700 (light) — but their breakouts
still need ordering. The **MLX90614 IR non-contact thermometer is in hand and
fully plugged into the devboard**, so it goes in now.

The MLX90614 reads the temperature of whatever it points at. Aimed at the sky it
gives a sky/cloud IR temperature — the basis for cloud detection in a weather
station. The wire frame already reserves a field for exactly this.

## Current State

- **One working sensor, owning the bus by value.** `bmp.rs::read_barometer`
  takes `I2c<'static, Async>` **by move**, reads the BMP388 at 1 Hz, builds a
  `Telemetry` via `Telemetry::from_bmp388` (temp + pressure only), and pushes it
  with `crate::ble::TELEMETRY.signal(telem)`. Nothing else can touch I2C0 today.
- **`TELEMETRY` is latest-wins.** A `Signal<…, Telemetry>` carrying one _whole_
  frame; the BLE notify task sends whatever was signaled last. Two sensor tasks
  each signaling their own partial frame would **clobber** each other.
- **The frame already has the sky-IR slot.** `meteo-lib::ble::frame` v1
  (17 bytes, `FRAME_VERSION = 1`) reserves bytes 7–8 for `sky_temp_c` (`i16` LE,
  centi-°C, sentinel `i16::MIN` → `None`). Encode/decode + round-trip tests for
  it already pass. The IR **object** temperature maps straight onto this field.
- **No second-sensor infrastructure yet.** No bus sharing, no aggregator. The
  multi-sensor merge problem was identified and a direction chosen in
  **brainstorm 4** (BME280) but **not yet built** — that work is hardware-blocked
  on a BME280 breakout. The MLX90614 is the first extra sensor actually in hand,
  so **this work carries that refactor.**
- **Driver style:** sensors are hand-rolled in `meteo-lib` against
  `embedded-hal-async` traits with pure, host-tested logic (see `bmp388.rs`).
  `libm` and `embassy-sync 0.8` are available; `embassy-embedded-hal` /
  `embedded-hal-bus` are **not** in the tree yet.

## Findings

### Sensor & protocol (from `datasheets/mlx90614.md`)

- **Address `0x5A`** on the shared I2C0 bus (GPIO10 SDA / GPIO11 SCL), distinct
  from BMP388 `0x77`; existing 4.7 kΩ pull-ups. **No new GPIO.** The pin doc
  treats this as "one bus, four sensors."
- **SMBus, not plain I2C.** 16-bit words **LSByte first** + an 8-bit **PEC
  (CRC-8, poly X⁸+X²+X¹+1)**. Read-Word only for our use. RAM `0x07` = TOBJ1
  (object), `0x06` = TA (ambient). Conversion: **`°C = raw · 0.02 − 273.15`**.
  **MSB bit 15 = error flag** → reading invalid.
- **PWM-vs-SMBus power-up risk.** The repo pin notes warn a GY-906 breakout _can_
  power up in PWM mode and must be nudged to SMBus. Most GY-906 modules ship in
  SMBus mode (a plain read works), but if the first reads fail this is the first
  thing to check (driving SCL low ≥1.44 ms, or an EEPROM config-register write to
  disable PWM). Treated as a **bring-up risk**, not designed-for up front.
- **Emissivity** left at factory default 1.0 (decided) — driver is read-only this
  pass, no EEPROM writes.

### The IR-ambient occlusion diagnostic (new requirement)

The IR sensor points **up at the sky**, so it can fill with snow / rain / ice.
The MLX's own **ambient (TA)** reading is used as a health proxy: it should track
the BMP388 air temperature closely. A wide divergence means something is wrong
(occlusion, icing, self-heating). This is surfaced as a **new diagnostics byte in
the frame** (frame v2). The MLX ambient itself is **not** reported as a
temperature — only the derived diagnostic bit — and the BMP388 stays the
authoritative ambient/air temperature.

### Merge architecture — settled by brainstorm 4

Brainstorm 4 already chose **aggregator + channels** over a shared
`Mutex<Telemetry>` and over folding sensors into one combined task. Each sensor
task sends its reading over an `embassy_sync` channel; an aggregator task owns
`TELEMETRY`, merges incoming readings into a single running `Telemetry`, and
publishes a merged frame at ~1 Hz (decoupled from individual read rates). This
work **builds that infrastructure for real**, with the MLX90614 as its first
client; the BMP388 task is reworked to send on the channel instead of signaling
`TELEMETRY` directly. The aggregator is the natural home for the occlusion
diagnostic, since it holds both the BMP388 air temperature and the MLX ambient.

### Frame v2 (new — brainstorm 4 needed no frame change)

Adding the diagnostics byte changes the wire format: **`FRAME_VERSION = 2`,
`FRAME_LEN = 18`**, byte 17 = diagnostics bitfield. Consumers to update:
`meteo-tui` decodes via `meteo-lib::frame::decode` (gets v2 for free once the lib
changes), and the acceptance scripts (`ble_notify_check.sh`) assert byte[0]==0x01
and a 17-byte length — both need bumping to 0x02 / 18 bytes.

## Scope

**In scope:**

- **`meteo-lib/src/sensors/mlx90614.rs`** — hand-rolled `embedded-hal-async`,
  `no_std` driver: Read-Word of TOBJ1 (`0x07`) and TA (`0x06`), **PEC (CRC-8)
  verification** of each read, error-flag (bit 15) check, and the
  `raw·0.02 − 273.15` conversion. Pure CRC-8 and conversion logic with host
  tests, matching the `bmp388.rs` pattern. Re-export from `sensors/mod.rs` and
  `lib.rs`.
- **Aggregator + channel refactor** (the infrastructure brainstorm 4 described):
  an aggregator task owning `TELEMETRY`; sensor tasks send readings over an
  `embassy_sync` channel; merged frame published at ~1 Hz. The **BMP388 task is
  reworked** to send on the channel.
- **I2C0 bus sharing** so both the BMP388 and MLX90614 tasks use the one bus
  (currently moved by value into the BMP task).
- **New MLX firmware task** (e.g. `mlx::read_sky`) spawned in `main.rs` against
  `MLX90614_ADDR = 0x5A`; sends object temp **and** ambient temp on the channel.
- **Frame v2 + diagnostics byte:** `FRAME_VERSION = 2`, `FRAME_LEN = 18`, a new
  diagnostics bitfield byte. Bit 0 = "sky IR sensor ambient diverges from
  barometer air temperature beyond threshold (possible occlusion / icing)";
  remaining bits reserved (0) for future per-sensor health flags. Update
  `encode`/`decode`, the layout doc comment, and round-trip tests.
- **Occlusion diagnostic in the aggregator:** compute `|TA_mlx − T_bmp|` and set
  the diagnostics bit when it exceeds the threshold. Default threshold **5 °C**
  (a field-tunable calibration value, not a design choice — revisit during
  real-sky testing).
- **Update frame consumers:** `meteo-tui` (display the new diagnostic;
  `sky_temp_c` already rendered) and the acceptance scripts' version/length
  assertions.
- **Host tests:** MLX conversion + PEC, aggregator merge + occlusion-bit logic,
  frame v2 round-trip.

**Resolved behaviour decisions:**

- MLX role: **object/IR temperature → `sky_temp_c`**; ambient (TA) used **only**
  as the occlusion diagnostic, never reported as a temperature. BMP388 stays
  authoritative for air temperature.
- Diagnostics surfaced as **one bitfield byte** (frame v2), occlusion computed
  **on-device** in the aggregator (chosen over shipping raw MLX ambient and
  deciding centrally).
- Emissivity: **factory default 1.0**, no EEPROM writes this pass.
- Merge model: **aggregator + channels** (consistent with brainstorm 4).
- Missing/failed MLX read, error flag set, or PEC mismatch: `sky_temp_c` stays
  **`None`** for that frame (graceful degradation, matching the BMP388 pattern).
- Publish cadence: aggregator publishes a **merged frame at ~1 Hz**, decoupled
  from individual sensor read rates.

**Out of scope:**

- BME280 (humidity) and VEML7700 (light) drivers — breakouts not yet ordered.
  The aggregator and bus sharing are designed to accommodate them.
- MLX emissivity tuning / any EEPROM writes.
- Reporting MLX ambient as a temperature field.
- PWM-mode auto-recovery — handled as a bring-up contingency, not built up front.

**Relationship to brainstorm 4:** this work **absorbs brainstorm 4's aggregator +
bus-sharing refactor** (same infrastructure, built here because the MLX is the
first sensor in hand). Brainstorm 4's remaining BME280-specific scope (the
`bme280.rs` humidity driver, `0x76` task) stays future and hardware-blocked. The
planner must not double-build the aggregator.

## Open Questions

Implementation-specific (for the planner):

- Channel topology: one shared multi-producer channel of a tagged
  `SensorReading` enum, vs. per-sensor signals the aggregator selects over.
- Bus-sharing mechanism: add `embassy-embedded-hal` `I2cDevice` over a
  `Mutex<RawMutex, I2c>`, vs. another approach; which `RawMutex`.
- Where the aggregator's running `Telemetry` lives and how the ~1 Hz publish
  tick is driven (separate from each sensor's read loop).
- Watchdog wiring: today `BMP_BEAT` bumps per BMP read; decide the heartbeat
  story for the MLX task and/or the aggregator (a stalled aggregator must still
  trip the RWDT).
- Whether `decode` hard-rejects v1 or accepts both v1 and v2 (firmware and
  central are both in-repo, so a hard v2 bump is acceptable).
- Exact diagnostics-byte bit assignments beyond bit 0 (reserved-bit policy).
- MLX read pacing: the datasheet warns continuous reads add noise — confirm the
  1 Hz forced cadence is gentle enough or whether to read less often.

## Next Steps

- Run `/tyrex:code:plan-light 5` to turn this into an implementation plan.
- Coordinate with brainstorm 4: the plan here should build the shared aggregator
  - bus-sharing so the later BME280 work only adds its driver + task.
