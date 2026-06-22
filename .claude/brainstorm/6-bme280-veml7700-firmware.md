# Brainstorm: BME280 + VEML7700 firmware integration

- **ID:** 6
- **Category:** Feature
- **Date:** 2026-06-23
- **Status:** Active

## Context

Two breakout boards arrived and are being wired onto the shared I2C0 bus:

- **BME280** (humidity / pressure / temperature) — breakout at **`0x76`** (SDO →
  GND; CSB → 3V3 to select I2C; VCC → 3V3). `0x77` is already taken by the BMP388.
- **VEML7700** (ambient light) — breakout at the fixed **`0x10`** (plugged in
  without trouble).

Goal: update the firmware to drive both, feed their data through the existing
aggregator → telemetry → BLE path, and surface them on the dashboard.

This supersedes brainstorm **#4** (BME280, humidity-only), which predates the
aggregator/shared-bus refactor (now built) and predates the BME280 breakout
(its on-device acceptance was blocked on hardware). The role decision has also
changed — see Findings.

## Current State

The plumbing these two sensors ride on already exists and works:

- **Aggregator + channels (built).** `aggregator::run` owns the `TELEMETRY`
  signal, drains `SENSOR_CHANNEL` (`Channel<_, SensorReading, 8>`), folds each
  reading into a running `meteo_lib::Aggregator`, and publishes a merged frame
  at 1 Hz. New sensors just add a `SensorReading` variant + an `ingest` arm.
- **Shared I2C0 bus (built).** `bus.rs` wraps the esp-hal `I2c` in a
  `Mutex<CriticalSectionRawMutex, _>`; each task gets a cheap `I2cDevice` clone.
  Adding a sensor = one more `I2cDevice::new(bus)` in `main.rs`. No new GPIO.
- **Wire frame already carries both fields.** `meteo-lib::ble::frame` v2 has
  `humidity_pct` (bytes 5–6, centi-%RH, sentinel `u16::MAX`) and
  `luminosity_lux` (bytes 9–11, mantissa×10^exp, sentinel mantissa `u16::MAX`).
  Encode/decode + round-trip/proptest coverage already pass. **No frame change.**
- **TUI already renders both.** `ui.rs:77,79` show "Humidity" and "Luminosity"
  rows; `model.rs` has `fmt_humidity`/`fmt_lux`. The dashboard shows `N/A` today
  and will show live values once the firmware populates the fields. **No TUI
  change required for the values** (a divergence/fault flag in the diagnostics
  row is the only TUI touch — see below).
- **Existing sensor tasks as templates.** `bmp.rs` (retry-init loop, per-cycle
  heartbeat, fault on no-handle) and `mlx.rs` (silent graceful degradation) are
  the two patterns to copy from.
- **Diagnostics bitfield.** `Diagnostics` byte: bit 0 = sky-IR occlusion,
  bit 1 = BMP388 fault; bits 2–7 reserved (the type comment already earmarks
  them for BME280 / VEML7700 / MLX health).

## Findings

### BME280 — role: cross-check both (decision)

The BME280 overlaps the BMP388 (both do temp + pressure; BME280 adds humidity).
Per-metric, the BMP388 is the better baro/thermometer (pressure ±0.5 vs ±1.0 hPa;
temp ±0.3 vs ±0.5 °C @25 °C); humidity (±3 %RH) is the BME280's unique value.

**Decision: cross-check both.**

- **BMP388 stays authoritative** for the emitted `temperature_c` / `pressure_hpa`
  (the frame helpers `from_bmp388` and the `BARO_FAULT` diagnostic are already
  built around it; flipping authority to the BME280 is a one-line change in
  planning if ever wanted).
- **BME280 contributes `humidity_pct`** and its own temp/pressure are used as a
  **cross-check** against the BMP388. Divergence beyond a threshold raises a new
  **baro-divergence diagnostic bit** (sensor-health signal: detects a drifting or
  mis-reading baro without a hard fault).
- **Compensation constraint (not a choice):** BME280 humidity compensation needs
  `t_fine` from the BME280's _own_ temperature reading, so the driver must sample
  temperature internally even though that value feeds humidity + the cross-check,
  not a separate frame field. Bosch "weather/humidity" forced-mode config
  (`osrs_t`=x1, `osrs_p`=x1, `osrs_h`=x1, IIR off) covers humidity + cross-check.

### VEML7700 — auto-ranging (decision)

Outdoor light spans roughly 0 (night) to ~120k lux (full sun); the part reaches
~140k. A single fixed gain/integration-time saturates at one end or loses the
low end. **Decision: auto-ranging** — the driver adjusts ALS gain + integration
time to keep the raw count in a healthy mid-range (Vishay's documented
step-up/step-down approach), then converts via the resolution table and applies
the >1000 lux nonlinearity-correction polynomial.

Driver facts from the datasheet:

- Fixed address `0x10`; 16-bit LSB-first registers; no interrupt pin (poll).
- Powers up in **shutdown** — driver must clear `ALS_SD` (write `ALS_CONF_0`).
- After a gain/IT change, **discard the first reading** (one integration period).
- ID check: register `0x07` low byte = `0x81`.
- Lux = `ALS_raw × resolution(gain, IT)`; correct above ~1000 lux.

### Fault handling — add diagnostic bits (decision)

Failed reads from the new sensors **degrade gracefully** (field → `None`, task
stays alive, **no** watchdog beat, **no** chip reset — a flaky new sensor must
not reboot-loop the device). On top of MLX-style silence, **add diagnostic
bits** so faults are visible in the TUI:

- bit 2 = **BME280 fault** (init failing / read failing → no humidity, no
  cross-check)
- bit 3 = **VEML7700 fault**
- bit 4 = **baro divergence** (BMP388 vs BME280 temp/pressure disagree beyond
  threshold)
- bit 5 = **MLX90614 fault** (read failed — I2C error / PEC mismatch / error
  flag / sensor absent → no `sky_temp_c`)

**MLX note — divergence vs fault are two different signals.** Bit 0
(`SKY_IR_OCCLUSION`) _already_ exists and is the MLX-ambient-vs-BMP388
**divergence** flag (TA diverges from air temp beyond `OCCLUSION_THRESHOLD_C`,
i.e. possible occlusion/icing). What the MLX is missing is a **sensor-fault**
bit for the read-failed case — today a failed MLX read blanks `sky_temp_c`
fully silently, with no health signal (unlike the BMP388's `BARO_FAULT`). Adding
bit 5 gives the MLX the same fault visibility the BME280/VEML now get, so all
four sensors report faults symmetrically. This requires `mlx.rs` to send a
fault signal when a read fails (the task already computes `object_c == None` on
failure — the aggregator just needs to latch it into the new bit) and a small
`SkyIrFault`/flag addition to `SensorReading`.

(Exact bit numbers are a planning detail; bits 2–7 are free.) `fmt_diagnostics`
in the TUI names each flag, so it gains four new arms; `diagnostics_alert`
already covers any nonzero byte.

## Scope

**In scope:**

- `meteo-lib/src/sensors/bme280.rs` — hand-rolled `embedded-hal-async`, `no_std`
  driver: chip-id `0x60`, calibration read (incl. the split H4/H5 12-bit
  packing), forced-mode weather config, integer temp+humidity compensation
  (`t_fine` step). Pure logic + host tests, mirroring `bmp388.rs`.
- `meteo-lib/src/sensors/veml7700.rs` — hand-rolled driver with **auto-ranging**
  (gain/IT selection), resolution-table lux conversion, and the nonlinearity
  correction. Pure conversion/ranging logic + host tests.
- `meteo-lib` aggregate: new `SensorReading` variant(s) for humidity + the
  BME280 cross-check temp/pressure, for luminosity, and an MLX read-fault signal
  (extend the existing `SkyIr` reading or add a fault variant). `Aggregator::ingest`
  arms that populate `humidity_pct` / `luminosity_lux`, compute baro divergence,
  and set/clear the new diagnostic bits (BME280 fault, VEML fault, baro
  divergence, MLX fault). New `Diagnostics` accessors/builders for the four new
  bits (mirroring `with_baro_fault`).
- `meteo-firmware/src/mlx.rs` — send an MLX-fault signal when a read fails (the
  task already detects `object_c == None`; surface it to the aggregator so bit 5
  latches instead of degrading silently).
- `meteo-firmware/src/bme.rs` + `veml.rs` — Embassy tasks (retry-init loop,
  per-cycle resilience, fault → `None`), spawned in `main.rs` with new
  `I2cDevice` handles and `BME280_ADDR = 0x76` / `VEML7700_ADDR = 0x10`.
- Module re-exports (`sensors/mod.rs`, `lib.rs`).
- TUI: four new `fmt_diagnostics` arms for the new flags (BME280 fault, VEML
  fault, baro divergence, MLX fault). (Humidity/Luminosity value rows already
  exist.)
- Host tests for both drivers, the cross-check/divergence logic, and the new
  diagnostics arms.
- Docs: refresh CLAUDE.md (module table, sensor list, diagnostics bit map) and
  the pin/datasheet notes.

**Out of scope:**

- Wire-format / frame version change (humidity + lux fields already exist).
- The remaining weather-station inputs (wind, rain, battery, vane ADC).
- Making the BME280 authoritative for temp/pressure (kept as a cheap future flip).
- Replacing or removing the BMP388.
- On-device BLE soak/acceptance methodology (unchanged; the gaia scripts already
  cover link + notify).

**Resolved behaviour decisions:**

- BME280 = **cross-check both** (humidity + temp/pressure cross-check; BMP388
  authoritative for emitted temp/pressure; divergence → diagnostic bit).
- VEML7700 = **auto-ranging** across 0–~140k lux.
- New-sensor faults = **graceful degradation + diagnostic bits**, no watchdog
  beat / no reset.
- Publish path unchanged: aggregator merges and publishes at 1 Hz.

## Open Questions

Implementation-specific (for the planner):

- **Baro-divergence thresholds** — °C and hPa deltas (and whether divergence is
  evaluated only when both sensors have a fresh reading). Pick defaults,
  field-tunable like `OCCLUSION_THRESHOLD_C`.
- **VEML auto-ranging algorithm** — target raw-count band, step-up/step-down
  hysteresis, how many integration periods to discard after a range change, and
  read cadence (the aggregator publishes at 1 Hz regardless).
- **BME280 calibration parsing** — exact byte layout for the split H4/H5 packed
  coefficients; integer vs float compensation in `no_std` (bmp388.rs uses float).
- **SensorReading shape** — one combined BME280 variant carrying
  `{humidity, temp, pressure}` vs separate variants; how the aggregator stores
  the BME280 temp/pressure for divergence without overwriting the BMP388 values.
- **Diagnostic bit assignment** — confirm bits 2/3/4 (or other free bits) and the
  `Diagnostics` accessor naming.
- **Read cadences** for the two new tasks (BMP is 1 Hz, MLX is 2 s); VEML's
  depends on the selected integration time.
- Whether the BME280 task gets a watchdog beat (current lean: **no**, like MLX).

## Next Steps

- Run `/tyrex:code:plan-light 6` to turn this into an implementation plan.
- (Done) BME280 + VEML7700 breakouts in hand — on-device acceptance is no longer
  hardware-blocked once the firmware lands.
