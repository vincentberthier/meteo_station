# Plan: BME280 + VEML7700 firmware integration

- **Source:** '6 (`.claude/brainstorm/6-bme280-veml7700-firmware.md`)
- **Date:** 2026-06-23
- **Status:** Done

## Summary

Drive two new I2C0 sensors — the BME280 (humidity + cross-check temp/pressure at
`0x76`) and the VEML7700 (auto-ranging ambient light at `0x10`) — through the
existing aggregator → telemetry → BLE → TUI path. Add hand-rolled `no_std`
`embedded-hal-async` drivers (float compensation for the BME280 like `bmp388.rs`;
resolution-table + auto-ranging + nonlinearity correction for the VEML7700), wire
two new Embassy sensor tasks (graceful degradation, no watchdog beat), extend the
aggregator to populate `humidity_pct`/`luminosity_lux` and compute a BMP388-vs-BME280
baro-divergence cross-check, and surface four new diagnostics bits (BME280 fault,
VEML7700 fault, baro divergence, MLX read fault) in the frame and the dashboard.
The wire frame is **unchanged** — `humidity_pct` and `luminosity_lux` already exist
with encode/decode coverage, and the diagnostics byte already reserves bits 2–7.

**Design decisions (resolved with the user):**

- **BME280 compensation: float**, mirroring `bmp388.rs` (no exact datasheet golden
  vectors; tests assert physical ranges/finiteness like the BMP388 tests do).
- **MLX read-fault (bit 5): derived in the aggregator** from the existing
  `SkyIr { object_c: None }` signal. **No new `SensorReading` variant and no change
  to `mlx.rs`** — it already sends `object_c: None` on a failed/invalid read.

## Files Modified

| File                                                       | Action | Description                                                                               |
| ---------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------- |
| `crates/meteo-lib/src/ble/frame.rs`                        | modify | Add 4 `Diagnostics` bits (BME280 fault, VEML fault, baro divergence, MLX fault) + tests   |
| `crates/meteo-lib/src/sensors/bme280.rs`                   | create | Hand-rolled BME280 driver: chip-id, calib parse (H4/H5 packing), forced-mode, float comp  |
| `crates/meteo-lib/src/sensors/veml7700.rs`                 | create | VEML7700 driver: config encoding, resolution table, lux + nonlinearity, auto-ranging      |
| `crates/meteo-lib/src/sensors/mod.rs`                      | modify | `pub mod bme280; pub mod veml7700;`                                                       |
| `crates/meteo-lib/src/lib.rs`                              | modify | Re-export `bme280`, `veml7700`                                                            |
| `crates/meteo-lib/src/aggregate.rs`                        | modify | New `SensorReading` variants, `AggregatorConfig`, aggregator fields, ingest, divergence   |
| `crates/meteo-firmware/src/aggregator.rs`                  | modify | Threshold consts → `AggregatorConfig`; `Aggregator::new(cfg)`                             |
| `crates/meteo-firmware/src/bme.rs`                         | create | BME280 Embassy task (1 Hz, retry-init, fault → `Bme280Fault`, no watchdog beat)           |
| `crates/meteo-firmware/src/veml.rs`                        | create | VEML7700 Embassy task (auto-ranging, discard-first-after-change, fault, no beat)          |
| `crates/meteo-firmware/src/main.rs`                        | modify | `mod bme; mod veml;`, `BME280_ADDR`/`VEML7700_ADDR`, new `I2cDevice` handles, spawn tasks |
| `crates/meteo-tui/src/model.rs`                            | modify | 4 new `fmt_diagnostics` arms + tests                                                      |
| `CLAUDE.md`, `README.md`, `datasheets/esp32_h2_devkitm.md` | modify | Module table, sensor list, diagnostics bit map, pin/address notes                         |

**Not touched:** `crates/meteo-firmware/src/mlx.rs` (MLX fault derived in the
aggregator), `crates/meteo-lib/src/ble/frame.rs` wire layout (fields already exist),
`crates/meteo-tui/src/ui.rs` value rows (Humidity/Luminosity rows already render;
the diagnostics row auto-extends via `fmt_diagnostics`).

## Plan

### 1. Diagnostics bits (meteo-lib frame.rs)

**File:** `crates/meteo-lib/src/ble/frame.rs`

Foundational substep — the aggregator (substep 6) and TUI (substep 10) depend on
these accessors. Add four bit constants, accessors, and `with_*` builders mirroring
the existing `with_baro_fault` pattern. Bit assignment (confirmed from brainstorm):

| Bit | Const              | Meaning                                            |
| --- | ------------------ | -------------------------------------------------- |
| 0   | `SKY_IR_OCCLUSION` | (existing) MLX ambient vs BMP388 air-temp diverge  |
| 1   | `BARO_FAULT`       | (existing) BMP388 not providing data               |
| 2   | `BME280_FAULT`     | BME280 init/read failing → no humidity/cross-check |
| 3   | `VEML7700_FAULT`   | VEML7700 init/read failing → no luminosity         |
| 4   | `BARO_DIVERGENCE`  | BMP388 vs BME280 temp/pressure disagree            |
| 5   | `MLX90614_FAULT`   | MLX object read failed → no sky_temp_c             |

**Signatures to add** (inside `impl Diagnostics`):

```rust
pub const BME280_FAULT: u8 = 1 << 2;
pub const VEML7700_FAULT: u8 = 1 << 3;
pub const BARO_DIVERGENCE: u8 = 1 << 4;
pub const MLX90614_FAULT: u8 = 1 << 5;

#[must_use] pub const fn bme280_fault(self) -> bool { self.0 & Self::BME280_FAULT != 0 }
#[must_use] pub const fn with_bme280_fault(self, set: bool) -> Self { self.with_flag(Self::BME280_FAULT, set) }
#[must_use] pub const fn veml7700_fault(self) -> bool { self.0 & Self::VEML7700_FAULT != 0 }
#[must_use] pub const fn with_veml7700_fault(self, set: bool) -> Self { self.with_flag(Self::VEML7700_FAULT, set) }
#[must_use] pub const fn baro_divergence(self) -> bool { self.0 & Self::BARO_DIVERGENCE != 0 }
#[must_use] pub const fn with_baro_divergence(self, set: bool) -> Self { self.with_flag(Self::BARO_DIVERGENCE, set) }
#[must_use] pub const fn mlx90614_fault(self) -> bool { self.0 & Self::MLX90614_FAULT != 0 }
#[must_use] pub const fn with_mlx90614_fault(self, set: bool) -> Self { self.with_flag(Self::MLX90614_FAULT, set) }
```

Update the `Diagnostics` doc comment and the byte-17 row of the module-level frame
table to enumerate bits 2–5 (no longer "reserved").

**Tests** (add to `frame.rs` `mod tests`):

- `diagnostics_new_bits_set_and_clear_independently` — each of the four new
  `with_*` builders sets its bit and `with_*(false)` clears it; assert the raw
  `.0` mask value for each (e.g. `with_bme280_fault(true).0 == 0b0000_0100`).
- `diagnostics_all_six_flags_compose` — set all six flags, assert each accessor
  returns `true` and `.0 == 0b0011_1111`.
- `decode_preserves_new_diagnostic_bits` — build a `Telemetry` with
  `Diagnostics(0b0011_1100)`, `encode()` then `decode()`, assert all four new
  accessors round-trip.

### 2. BME280 driver (meteo-lib sensors/bme280.rs)

**File:** `crates/meteo-lib/src/sensors/bme280.rs` (new). Models `bmp388.rs`.

No dependency on other substeps. Pure logic + host tests.

**Registers / constants:**

```rust
const CHIP_ID_REG: u8 = 0xD0;
const EXPECTED_CHIP_ID: u8 = 0x60;
const CTRL_HUM: u8 = 0xF2;
const STATUS: u8 = 0xF3;
const CTRL_MEAS: u8 = 0xF4;
const CONFIG: u8 = 0xF5;
const CALIB_00: u8 = 0x88;     // block 1: 0x88..=0xA1, 26 bytes (T1..P9, gap, H1)
const CALIB_26: u8 = 0xE1;     // block 2: 0xE1..=0xE7, 7 bytes (H2..H6)
const STATUS_MEASURING: u8 = 1 << 3;

// Weather/humidity forced config: osrs_h=x1, osrs_t=x1, osrs_p=x1, IIR off, forced.
const CTRL_HUM_X1: u8 = 0b0000_0001;                 // osrs_h = x1
const CTRL_MEAS_FORCED: u8 = 0b0010_0101;            // osrs_t=001, osrs_p=001, mode=01 (forced)
const CONFIG_IIR_OFF: u8 = 0b0000_0000;
```

**Public API:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> { I2c(E), WrongChipId(u8) }

pub struct Bme280<I> { i2c: I, address: u8, calib: CalibData }

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub temperature: f32,  // °C
    pub pressure: f32,     // Pa
    pub humidity: f32,     // %RH (0..=100)
}
impl Reading { #[must_use] pub fn pressure_hpa(&self) -> f32 { self.pressure / 100.0 } }

impl<I, E> Bme280<I> where I: I2c<Error = E> {
    /// Verify chip id (0xD0 == 0x60), read both calib blocks, write ctrl_hum +
    /// config once (forced mode triggers per-read). Sensor left in sleep.
    pub async fn new(mut i2c: I, address: u8) -> Result<Self, Error<E>>;
    /// Trigger one forced measurement (write CTRL_MEAS), poll STATUS bit 3 until
    /// the measuring flag clears (no fixed delay — same pattern as bmp388 DRDY),
    /// burst-read 0xF7..=0xFE (8 bytes), compensate T (sets t_fine) → P → H.
    pub async fn read(&mut self) -> Result<Reading, Error<E>>;
}
```

`new()` order (datasheet ordering rule): write `CTRL_HUM = CTRL_HUM_X1` **before**
the first `CTRL_MEAS`. Write `CONFIG = CONFIG_IIR_OFF` in `new()`. `read()` writes
`CTRL_MEAS_FORCED` to trigger each cycle.

**Calibration parse** (`CalibData::from_raw_bytes(b1: &[u8; 26], b2: &[u8; 7])`):

```text
b1 (from 0x88):                          b2 (from 0xE1):
 dig_t1 = u16  [0,1]                       dig_h2 = i16 [0,1]
 dig_t2 = i16  [2,3]                       dig_h3 = u8  [2]
 dig_t3 = i16  [4,5]                       dig_h4 = ((i8(b2[3]) as i16) * 16) | (b2[4] & 0x0F) as i16
 dig_p1 = u16  [6,7]                       dig_h5 = ((i8(b2[5]) as i16) * 16) | (b2[4] >> 4) as i16
 dig_p2..p9 = i16 [8..24]                  dig_h6 = i8  [6]
 dig_h1 = u8   [25]   (0xA1)
```

The H4/H5 12-bit packing is the only subtlety: H4 high byte (0xE4) is signed,
`*16`, OR'd with the low nibble of 0xE5; H5 high byte (0xE6) signed `*16` OR'd
with the high nibble of 0xE5. Store coefficients as their typed integers; carry a
`t_fine: f32` field (like `bmp388`'s `t_lin`).

**Compensation (f32 port of the Bosch double-precision reference):**

```rust
fn compensate_temperature(&mut self, adc_t: i32) -> f32 {
    let v1 = (adc_t as f32 / 16384.0 - self.dig_t1 as f32 / 1024.0) * self.dig_t2 as f32;
    let d  = adc_t as f32 / 131072.0 - self.dig_t1 as f32 / 8192.0;
    let v2 = d * d * self.dig_t3 as f32;
    self.t_fine = v1 + v2;
    self.t_fine / 5120.0
}
fn compensate_pressure(&self, adc_p: i32) -> f32 { /* Bosch double formula, f32; returns Pa, 0.0 if var1==0 */ }
fn compensate_humidity(&self, adc_h: i32) -> f32 { /* Bosch double formula, f32; clamp to [0,100] */ }
```

(Reuse the existing `#[expect(clippy::cast_precision_loss, ...)]` / `similar_names`
annotation style from `bmp388.rs`.)

**ADC assembly in `read()`** from the 8-byte burst (`d[0..=7]` = 0xF7..0xFE):

```rust
let adc_p = (i32::from(d[0]) << 12) | (i32::from(d[1]) << 4) | (i32::from(d[2]) >> 4);
let adc_t = (i32::from(d[3]) << 12) | (i32::from(d[4]) << 4) | (i32::from(d[5]) >> 4);
let adc_h = (i32::from(d[6]) << 8)  |  i32::from(d[7]);
```

**Tests** (`bme280.rs` `mod tests`, no I2C — test the pure parse/compensation):

- `calib_parses_temperature_pressure_coefficients` — golden 26-byte block →
  expected `dig_t1`/`dig_t2`/`dig_p1` integer values.
- `calib_parses_packed_h4_h5_with_sign` — golden 7-byte block exercising the
  12-bit packing including a negative H4/H5; assert the exact reconstructed i16.
- `compensate_temperature_sets_t_fine_and_returns_celsius` — realistic `adc_t` →
  temperature in `-40..=85`, and `t_fine` set consistently.
- `compensate_pressure_returns_plausible_sea_level` — after a temperature call, a
  realistic `adc_p` → finite and within a sea-level-plausible band
  (`30_000.0..=110_000.0` Pa), tighter than just `> 0` so a near-zero result fails.
- `compensate_humidity_clamps_to_0_100` — drive an out-of-range result and assert
  the clamp holds the output within `0.0..=100.0`.
- `reading_pressure_hpa_converts` — `Reading { pressure: 101_325.0, .. }` →
  `≈ 1013.25`.

Use a `sample_calib_b1()`/`sample_calib_b2()` helper pair like
`bmp388.rs::sample_calib_bytes`.

### 3. VEML7700 driver (meteo-lib sensors/veml7700.rs)

**File:** `crates/meteo-lib/src/sensors/veml7700.rs` (new). No dependency on other
substeps. The auto-ranging _decision_ and lux conversion are pure and fully tested;
the firmware task (substep 7) owns the integration-time wait.

**Types + constants:**

```rust
const ALS_CONF_0: u8 = 0x00;
const ALS_DATA: u8 = 0x04;
const ID_REG: u8 = 0x07;
const EXPECTED_ID_LOW: u8 = 0x81;

/// Auto-ranging raw-count band (Vishay app-note defaults). Wide gap = hysteresis.
pub const COUNT_LO: u16 = 100;
pub const COUNT_HI: u16 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[cfg_attr(feature="defmt", derive(defmt::Format))]
pub enum Gain { X1_8, X1_4, X1, X2 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[cfg_attr(feature="defmt", derive(defmt::Format))]
pub enum IntegrationTime { Ms25, Ms50, Ms100, Ms200, Ms400, Ms800 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting { pub gain: Gain, pub it: IntegrationTime }

/// Sensitivity ladder, least → most sensitive (index 0 = darkest-tolerant).
/// Auto-ranging steps one rung at a time.
pub const LADDER: [Setting; 8] = [
    Setting { gain: Gain::X1_8, it: IntegrationTime::Ms25 },   // 0  res ≈ 2.1504
    Setting { gain: Gain::X1_8, it: IntegrationTime::Ms100 },  // 1  res ≈ 0.5376
    Setting { gain: Gain::X1_4, it: IntegrationTime::Ms100 },  // 2  res ≈ 0.2688
    Setting { gain: Gain::X1,   it: IntegrationTime::Ms100 },  // 3  res ≈ 0.0672
    Setting { gain: Gain::X2,   it: IntegrationTime::Ms100 },  // 4  res ≈ 0.0336
    Setting { gain: Gain::X2,   it: IntegrationTime::Ms200 },  // 5  res ≈ 0.0168
    Setting { gain: Gain::X2,   it: IntegrationTime::Ms400 },  // 6  res ≈ 0.0084
    Setting { gain: Gain::X2,   it: IntegrationTime::Ms800 },  // 7  res ≈ 0.0042
];
pub const LADDER_START: usize = 3; // (X1, Ms100): mid-range first guess

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Error<E> { I2c(E), WrongId(u8) }
```

**Pure helpers (the testable core):**

```rust
impl Gain { fn multiplier(self) -> f32; fn bits(self) -> u16; }            // x1->1.0, x2->2.0, x1/4->0.25, x1/8->0.125; bits per datasheet 12:11
impl IntegrationTime { fn millis(self) -> u32; fn bits(self) -> u16; }     // 25..800; bits per datasheet 9:6

/// resolution = 0.0042 * (800 / it_ms) * (2 / gain_mult)  (lx/count)
#[must_use] pub fn resolution(s: Setting) -> f32;
/// lux = raw * resolution(s); if lux > 1000.0 (strictly above) apply the Vishay
/// nonlinearity polynomial. At exactly 1000.0 lux the linear value is returned.
#[must_use] pub fn raw_to_lux(raw: u16, s: Setting) -> f32;
/// ALS_CONF_0 value for a setting (gain<<11 | it_bits<<6); ALS_SD=0 (powered on).
#[must_use] pub fn als_conf0(s: Setting) -> u16;
/// Auto-ranging step: raw > COUNT_HI and not least-sensitive → idx-1; raw < COUNT_LO
/// and not most-sensitive → idx+1; else idx. Clamps at both ends.
#[must_use] pub fn next_index(idx: usize, raw: u16) -> usize;
```

Nonlinearity polynomial (datasheet, applied when `lux > 1000.0`):

```rust
const C4: f32 = 6.0135e-13; const C3: f32 = -9.3924e-9;
const C2: f32 = 8.1488e-5;  const C1: f32 = 1.0023;
// lux_corrected = C4*l^4 + C3*l^3 + C2*l^2 + C1*l
```

**Driver methods (register accessors; the task orchestrates ranging + waits):**

```rust
pub struct Veml7700<I> { i2c: I, address: u8 }
impl<I, E> Veml7700<I> where I: I2c<Error = E> {
    #[must_use] pub const fn new(i2c: I, address: u8) -> Self;
    /// Read ID (0x07); low byte must be 0x81. Call before first measurement.
    pub async fn verify_id(&mut self) -> Result<(), Error<E>>;
    /// Write ALS_CONF_0 (16-bit LSB-first) = als_conf0(setting). Powers on (ALS_SD=0).
    pub async fn set_setting(&mut self, s: Setting) -> Result<(), Error<E>>;
    /// Read ALS_DATA (0x04), 16-bit LSB-first raw count.
    pub async fn read_raw(&mut self) -> Result<u16, Error<E>>;
}
```

16-bit register writes/reads are **LSB-first** (datasheet): write `[reg, lo, hi]`;
read returns `[lo, hi]` → `u16::from_le_bytes`.

**Tests** (`veml7700.rs` `mod tests`):

- `resolution_matches_datasheet_table` — assert `resolution` for the 8 ladder
  entries within tolerance of the datasheet table (e.g. `(X2,Ms800)≈0.0042`,
  `(X1,Ms100)≈0.0672`, `(X1_8,Ms25)≈2.1504`).
- `raw_to_lux_linear_below_1000` — small raw count, `(X1,Ms100)` → `raw*0.0672`
  exactly (no correction).
- `raw_to_lux_applies_correction_above_1000` — a raw count giving > 1000 lux →
  result equals the polynomial evaluation and is `>` the uncorrected product.
- `raw_to_lux_at_1000_lux_boundary` — choose a `(raw, setting)` giving exactly
  1000.0 lux linear; assert the **linear** value is returned (correction not yet
  applied), pinning the `>` vs `>=` condition at the boundary.
- `als_conf0_encodes_gain_and_it_bits` — `(X2,Ms100)` and `(X1_8,Ms25)` → exact
  16-bit register values (gain in 12:11, IT in 9:6, ALS_SD=0).
- `next_index_steps_down_when_saturated` — `next_index(3, 11_000) == 2`.
- `next_index_steps_up_when_dark` — `next_index(3, 50) == 4`.
- `next_index_stays_in_band` — `next_index(3, 5_000) == 3`.
- `next_index_clamps_at_ends` — `next_index(0, 60_000) == 0` and
  `next_index(7, 10) == 7`.

### 4. Re-exports (meteo-lib)

**Files:** `crates/meteo-lib/src/sensors/mod.rs`, `crates/meteo-lib/src/lib.rs`.

```rust
// sensors/mod.rs
pub mod bme280;
pub mod veml7700;

// lib.rs — extend the existing sensors re-export line
pub use sensors::{bmp388, mlx90614, bme280, veml7700};
```

Depends on substeps 2 and 3. No new test; `cargo build -p meteo-lib` must compile.

### 5. Aggregate wiring (meteo-lib aggregate.rs)

**File:** `crates/meteo-lib/src/aggregate.rs`. Depends on substep 1 (diagnostics
builders).

**New `SensorReading` variants** (extend the existing enum):

```rust
/// BME280: humidity (emitted) plus its own temp/pressure used only for the
/// BMP388 cross-check (not emitted as telemetry temp/pressure fields).
Bme280 { humidity_pct: f32, temperature_c: f32, pressure_hpa: f32 },
/// BME280 down (init/read failing): blanks humidity + cross-check, raises BME280_FAULT.
Bme280Fault,
/// VEML7700: ambient light in lux.
Luminosity { lux: f32 },
/// VEML7700 down: blanks luminosity, raises VEML7700_FAULT.
LuminosityFault,
```

(No MLX variant — the MLX fault is derived from the existing `SkyIr`.)

**New `AggregatorConfig` struct** (groups thresholds; keeps call sites readable):

```rust
#[derive(Debug, Clone, Copy)]
pub struct AggregatorConfig {
    pub occlusion_threshold_c: f32,
    pub temp_divergence_c: f32,    // BMP vs BME temperature divergence (°C)
    pub press_divergence_hpa: f32, // BMP vs BME pressure divergence (hPa)
}
```

**New `Aggregator` fields** (added to the struct + `new`):

```rust
pub struct Aggregator {
    telemetry: Telemetry,
    air_temp_c: Option<f32>,
    sky_ambient_c: Option<f32>,
    baro_fault: bool,
    // new:
    bme_temp_c: Option<f32>,
    bme_pressure_hpa: Option<f32>,
    bme_fault: bool,
    veml_fault: bool,
    mlx_fault: bool,
    cfg: AggregatorConfig,
}

#[must_use] pub const fn new(cfg: AggregatorConfig) -> Self { /* all None/false, store cfg */ }
```

`Aggregator::new` signature changes from `new(occlusion_threshold_c: f32)` to
`new(cfg: AggregatorConfig)`. Update the firmware call site (substep 6) and all
existing aggregate tests (add a `const TEST_CFG: AggregatorConfig` helper, e.g.
`{ occlusion_threshold_c: 5.0, temp_divergence_c: 2.0, press_divergence_hpa: 3.0 }`,
and replace `Aggregator::new(5.0)` with `Aggregator::new(TEST_CFG)`).

**`ingest` arms** (extend the `match`):

```rust
SensorReading::SkyIr { object_c, ambient_c } => {
    self.telemetry.sky_temp_c = object_c;
    self.sky_ambient_c = ambient_c;
    self.mlx_fault = object_c.is_none();      // derive MLX fault (bit 5)
}
SensorReading::Bme280 { humidity_pct, temperature_c, pressure_hpa } => {
    self.telemetry.humidity_pct = Some(humidity_pct);
    self.bme_temp_c = Some(temperature_c);
    self.bme_pressure_hpa = Some(pressure_hpa);
    self.bme_fault = false;
}
SensorReading::Bme280Fault => {
    self.telemetry.humidity_pct = None;
    self.bme_temp_c = None;
    self.bme_pressure_hpa = None;
    self.bme_fault = true;
}
SensorReading::Luminosity { lux } => {
    self.telemetry.luminosity_lux = Some(lux);
    self.veml_fault = false;
}
SensorReading::LuminosityFault => {
    self.telemetry.luminosity_lux = None;
    self.veml_fault = true;
}
```

(`ingest` is currently `const fn`. All four new arms are plain `Option`/`bool` field
assignments — const-compatible — and the updated `SkyIr` arm's
`mlx_fault = object_c.is_none()` is const too (`Option::is_none` is const). Keep
`const`. The non-const `fabsf` lives only in `diverged()`, which is called from
`snapshot()`, never from `ingest`.)

**`snapshot`** — extend the diagnostics build and add a `diverged()` helper:

```rust
pub fn snapshot(&self) -> Telemetry {
    let mut t = self.telemetry;
    t.diagnostics = Diagnostics::empty()
        .with_occlusion(self.occluded())
        .with_baro_fault(self.baro_fault)
        .with_bme280_fault(self.bme_fault)
        .with_veml7700_fault(self.veml_fault)
        .with_baro_divergence(self.diverged())
        .with_mlx90614_fault(self.mlx_fault);
    t
}

/// Diverged iff BOTH baros are fresh and either metric disagrees beyond threshold.
/// Compares BMP authoritative values (air_temp_c, telemetry.pressure_hpa) against
/// the BME cross-check values. Any missing input → not diverged (cannot determine).
fn diverged(&self) -> bool {
    let temp_div = match (self.air_temp_c, self.bme_temp_c) {
        (Some(a), Some(b)) => fabsf(a - b) > self.cfg.temp_divergence_c,
        _ => false,
    };
    let press_div = match (self.telemetry.pressure_hpa, self.bme_pressure_hpa) {
        (Some(a), Some(b)) => fabsf(a - b) > self.cfg.press_divergence_hpa,
        _ => false,
    };
    temp_div || press_div
}
```

`occluded()` keeps reading `self.cfg.occlusion_threshold_c` (rename the field
access from `self.occlusion_threshold_c`).

**Tests** (extend `aggregate.rs` `mod tests`; add `TEST_CFG` and update existing
`Aggregator::new(5.0)` call sites):

- `aggregator_bme280_populates_humidity_and_clears_fault` — ingest `Bme280{..}`,
  assert `snap.humidity_pct == Some(..)` and `!bme280_fault()`.
- `aggregator_bme280_fault_blanks_humidity_and_sets_bit` — good `Bme280` then
  `Bme280Fault`: `humidity_pct == None`, `bme280_fault()`.
- `aggregator_luminosity_populates_lux` / `aggregator_luminosity_fault_blanks_and_sets_bit`.
- `aggregator_mlx_fault_derived_from_skyir_none` — `SkyIr{object_c:None,..}` sets
  `mlx90614_fault()`; a following `SkyIr{object_c:Some,..}` clears it.
- `aggregator_baro_divergence_set_when_temp_disagrees` — `Barometer{20.0,1013.0}` +
  `Bme280{_,25.0,1013.0}` (Δ5°C > 2) → `baro_divergence()`.
- `aggregator_baro_divergence_set_when_pressure_disagrees` — Δpressure > 3 hPa.
- `aggregator_no_divergence_within_threshold` — both within thresholds → clear.
- `aggregator_no_divergence_when_bme_missing` — only `Barometer`, no `Bme280` →
  clear (cannot determine).
- `aggregator_bme_fault_drops_divergence_input` — diverging pair then `Bme280Fault`:
  `baro_divergence()` false (the cross-check input is now `None` → "cannot determine"
  path in `diverged()`), `bme280_fault()` true.

### 6. Firmware aggregator config (meteo-firmware aggregator.rs)

**File:** `crates/meteo-firmware/src/aggregator.rs`. Depends on substep 5.

Replace the single `OCCLUSION_THRESHOLD_C` const-arg call with an
`AggregatorConfig` built from three field-tunable consts:

```rust
use meteo_lib::aggregate::AggregatorConfig;

/// Sky-IR occlusion threshold (°C). Field-tunable; revisit during real-sky testing.
const OCCLUSION_THRESHOLD_C: f32 = 5.0;
/// BMP388-vs-BME280 temperature cross-check divergence threshold (°C).
const TEMP_DIVERGENCE_C: f32 = 2.0;
/// BMP388-vs-BME280 pressure cross-check divergence threshold (hPa).
const PRESS_DIVERGENCE_HPA: f32 = 3.0;

const AGG_CONFIG: AggregatorConfig = AggregatorConfig {
    occlusion_threshold_c: OCCLUSION_THRESHOLD_C,
    temp_divergence_c: TEMP_DIVERGENCE_C,
    press_divergence_hpa: PRESS_DIVERGENCE_HPA,
};

// in run():
let mut agg = Aggregator::new(AGG_CONFIG);
```

Also update the now-stale `SENSOR_CHANNEL` docstring in `aggregator.rs` (currently
`"Capacity 8 ≫ the 2 producers at ≤1 Hz"`) to read **`4 producers`** — after this
work the BMP388, MLX90614, BME280, and VEML7700 tasks all send on it.

No new test (firmware glue); covered by substep 5's host tests and the build.

### 7. BME280 Embassy task (meteo-firmware bme.rs)

**File:** `crates/meteo-firmware/src/bme.rs` (new). Models `bmp.rs` but **no
watchdog beat** (graceful degradation, per brainstorm). Depends on substeps 2 and 5.

```rust
#[embassy_executor::task]
pub async fn read_humidity(i2c: SharedI2c, address: u8) {
    // `SharedI2c` (an `I2cDevice`) is `Clone` and cheap to copy (it just holds the
    // `&'static Mutex` bus ref), so each (re)init attempt gets a fresh handle while
    // the task keeps the original for the next retry — identical to `bmp.rs`.
    let mut sensor: Option<Bme280<SharedI2c>> = None;
    loop {
        if sensor.is_none() {
            match Bme280::new(i2c.clone(), address).await {
                Ok(s) => { info!("BME280 initialized"); sensor = Some(s); }
                Err(e) => warn!("BME280 init failed, retrying: {:?}", Debug2Format(&e)),
            }
        }
        if let Some(s) = sensor.as_mut() {
            match s.read().await {
                Ok(r) => {
                    info!("BME280 H:{}%RH T:{}°C P:{}hPa",
                        trunc2(r.humidity), trunc2(r.temperature), trunc2(r.pressure_hpa()));
                    SENSOR_CHANNEL.send(SensorReading::Bme280 {
                        humidity_pct: r.humidity,
                        temperature_c: r.temperature,
                        pressure_hpa: r.pressure_hpa(),
                    }).await;
                }
                Err(e) => { warn!("BME280 read failed, re-init: {:?}", Debug2Format(&e)); sensor = None; }
            }
        }
        if sensor.is_none() {
            SENSOR_CHANNEL.send(SensorReading::Bme280Fault).await;
        }
        // 1 Hz sample clock (Timer::after gap-after-read, same rationale as bmp.rs).
        Timer::after(Duration::from_secs(1)).await;
    }
}
```

No `BME_BEAT` — a failing BME280 must not reset the chip. Copy the `#![expect(...)]`
header and imports from `bmp.rs`.

**Tests:** none (hardware task); the driver's pure logic is tested in substep 2.

### 8. VEML7700 Embassy task (meteo-firmware veml.rs)

**File:** `crates/meteo-firmware/src/veml.rs` (new). Depends on substeps 3 and 5.
No watchdog beat. Owns the auto-ranging loop and the one justified hardware timer.

Unlike `bmp.rs`/`bme.rs` (which drop and re-create the driver on a read error),
the VEML task keeps one `Veml7700` for the task's lifetime. `Veml7700::new` is
infallible (`const fn`, no bus traffic) and the owned `I2cDevice` handle stays
valid across a transient bus error, so recovery only needs to repeat the
`verify_id + set_setting` handshake — there is nothing to re-create. The task takes
the handle by value (no `clone()`) for the same reason: there is no second init path.

```rust
#[embassy_executor::task]
pub async fn read_luminosity(i2c: SharedI2c, address: u8) {
    let mut sensor = Veml7700::new(i2c, address);
    let mut idx = veml7700::LADDER_START;
    let mut initialized = false;
    let mut discard_next = true; // first sample after any (re)config is stale

    loop {
        // (Re)initialise: verify ID + write the current setting.
        if !initialized {
            match init(&mut sensor, idx).await {        // verify_id + set_setting
                Ok(()) => { initialized = true; discard_next = true; }
                Err(e) => { warn!("VEML init failed, retrying: {:?}", Debug2Format(&e)); }
            }
        }

        let setting = veml7700::LADDER[idx];
        if initialized {
            // Wait the device's documented integration period before the sample is
            // valid. The VEML7700 has NO data-ready pin/status bit (datasheet), so
            // the integration time IS the conversion period — this is the hardware's
            // specified settling time derived from `setting.it.millis()`, not a tuned
            // readiness guess. Same role as bmp388's DRDY poll, which the VEML lacks.
            Timer::after(Duration::from_millis(setting.it.millis().into())).await;

            match sensor.read_raw().await {
                Ok(raw) => {
                    let next = veml7700::next_index(idx, raw);
                    if next != idx {
                        idx = next;
                        match sensor.set_setting(veml7700::LADDER[idx]).await {
                            Ok(()) => discard_next = true,        // first post-change read is stale
                            Err(e) => { warn!(..); initialized = false; }
                        }
                    } else if discard_next {
                        discard_next = false;                    // settle done; next loop reports
                    } else {
                        let lux = veml7700::raw_to_lux(raw, setting);
                        info!("Luminosity: {} lux (raw {})", trunc2(lux), raw);
                        SENSOR_CHANNEL.send(SensorReading::Luminosity { lux }).await;
                    }
                }
                Err(e) => { warn!("VEML read failed, re-init: {:?}", Debug2Format(&e)); initialized = false; }
            }
        }

        if !initialized {
            SENSOR_CHANNEL.send(SensorReading::LuminosityFault).await;
            Timer::after(Duration::from_secs(1)).await;  // retry cadence when down
        }
    }
}
```

The integration-period `Timer::after` is the **only** timer here and is justified
in a comment: the VEML7700 exposes no conversion-ready signal, so the
datasheet-specified integration time is the conversion period (value derived from
the selected setting, not hand-tuned). On any gain/IT change, the next reading is
discarded (datasheet rule). A small private `init()` helper keeps the task function
under the 100-line / complexity-8 limits — split the (re)init and the
read-and-range bodies into helpers if the lint trips.

**Tests:** none (hardware task); ranging/lux logic tested in substep 3.

### 9. Firmware wiring (meteo-firmware main.rs)

**File:** `crates/meteo-firmware/src/main.rs`. Depends on substeps 7 and 8.

```rust
mod bme;   // add to the module list
mod veml;

/// BME280 I2C address (SDO → GND). 0x77 is the BMP388.
const BME280_ADDR: u8 = 0x76;
/// VEML7700 fixed I2C address (not configurable).
const VEML7700_ADDR: u8 = 0x10;

// after the existing mlx spawn, with fresh per-sensor I2cDevice handles:
let bme_i2c: SharedI2c = I2cDevice::new(bus);
spawner.spawn(bme::read_humidity(bme_i2c, BME280_ADDR).expect("read_humidity already spawned"));
let veml_i2c: SharedI2c = I2cDevice::new(bus);
spawner.spawn(veml::read_luminosity(veml_i2c, VEML7700_ADDR).expect("read_luminosity already spawned"));
```

Update the BMP388 `BMP388_ADDR` doc comment (it currently says "leaves 0x76 free
for a future BME280" — now occupied). No new GPIO; both ride the shared bus.

**Tests:** none (firmware init); covered by `just build`.

### 10. TUI diagnostics labels (meteo-tui model.rs)

**File:** `crates/meteo-tui/src/model.rs`. Depends on substep 1. The Humidity and
Luminosity **value** rows already render (`ui.rs:77,79`); only `fmt_diagnostics`
needs the four new flag arms.

```rust
pub fn fmt_diagnostics(diag: Diagnostics) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if diag.occlusion()       { flags.push("sky occluded"); }
    if diag.baro_fault()      { flags.push("BMP388 fault"); }
    if diag.bme280_fault()    { flags.push("BME280 fault"); }
    if diag.veml7700_fault()  { flags.push("VEML7700 fault"); }
    if diag.baro_divergence() { flags.push("baro divergence"); }
    if diag.mlx90614_fault()  { flags.push("MLX90614 fault"); }
    if flags.is_empty() { "OK".to_owned() } else { flags.join(", ") }
}
```

Ordering follows bit order (0→5). `diagnostics_alert` is unchanged (tests the raw
byte, already covers all bits).

**Tests** (extend `model.rs` `mod tests`):

- `fmt_diagnostics_bme280_fault_only` → `"BME280 fault"`.
- `fmt_diagnostics_veml7700_fault_only` → `"VEML7700 fault"`.
- `fmt_diagnostics_baro_divergence_only` → `"baro divergence"`.
- `fmt_diagnostics_mlx_fault_only` → `"MLX90614 fault"`.
- `fmt_diagnostics_all_flags_joined_in_bit_order` — all six set →
  `"sky occluded, BMP388 fault, BME280 fault, VEML7700 fault, baro divergence, MLX90614 fault"`.

(Optional: add a `render_*` smoke assertion in `ui.rs` tests for one new flag,
mirroring `render_shows_baro_fault_diagnostic`.)

### 11. Documentation

**Files:** `CLAUDE.md`, `README.md`, `datasheets/esp32_h2_devkitm.md`. Depends on all
prior substeps.

- `CLAUDE.md`: add `bme.rs`/`veml.rs` to the module-structure tree and
  `bme280.rs`/`veml7700.rs` under `meteo-lib/src/sensors/`; add BME280 + VEML7700 to
  the "Currently supports" list; update the diagnostics bitfield description (byte 17
  now uses bits 0–5); add BME280 (`0x76`) and VEML7700 (`0x10`) to the I2C device /
  pin-allocation notes (shared bus, no new GPIO); note the BMP388 comment no longer
  "leaves 0x76 free".
- `README.md`: mirror the supported-sensor and diagnostics updates.
- `datasheets/esp32_h2_devkitm.md`: record the two new I2C addresses on the shared
  GPIO10/11 bus.

**Tests:** none (docs).

## Testing

All host tests run via `cargo nextest run --all-features --all-targets` on the dev
machine; the firmware tasks are hardware glue (no host tests) and are exercised by
`just build` + on-device acceptance.

Per-substep host coverage (function names listed above):

- **frame.rs** — 3 new diagnostics tests (set/clear, compose, decode round-trip).
- **bme280.rs** — 6 tests: calib parse (incl. signed 12-bit H4/H5 packing), T/P/H
  compensation (range/finite/clamp), `pressure_hpa`.
- **veml7700.rs** — 8 tests: resolution table, lux linear + nonlinearity, conf-reg
  encoding, `next_index` step-up/down/stay/clamp.
- **aggregate.rs** — 9 new tests + all existing tests updated to `TEST_CFG`:
  humidity/lux populate + fault, MLX-fault derivation, divergence (temp, pressure,
  within-threshold, missing-input, fault-clears).
- **model.rs** — 5 new `fmt_diagnostics` tests.

**Full gate before finalize (`tyrex-lang-rust:rust` validation set):**

```bash
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings   # via `just clippy` (+ tui-clippy)
cargo nextest run --all-features --all-targets             # via `just test`
just build                                                  # firmware compiles for riscv32imac
just size                                                   # sanity on binary growth
```

**On-device acceptance (manual gate, unchanged methodology):** flash via `just run`,
confirm defmt logs show BME280 humidity + VEML7700 lux, then run the gaia scripts
(`ble_notify_check.sh` for ≥5 well-formed v2 frames; `ble_soak.sh` for link
stability). Verify the dashboard shows live Humidity/Luminosity values and that
unplugging a new sensor flips the matching diagnostics flag without an RWDT reboot.

**Edge cases under test:** signed 12-bit calibration packing (negative H4/H5);
humidity compensation clamp at 0/100; lux nonlinearity boundary at 1000 lux;
auto-ranging saturation (raw > COUNT_HI) and darkness (raw < COUNT_LO) including
ladder-end clamping; divergence only when both baros are fresh; MLX fault asserted
only after a failed read (not at cold start).

## Risks

- **BME280 float compensation precision.** The f32 port of Bosch's f64 reference may
  drift slightly from integer results, but weather accuracy (±3 %RH, ±1 hPa) swamps
  f32 error — `bmp388.rs` already proves f32 is adequate. _Mitigation:_ tests assert
  physical ranges, not bit-exact values; cross-check against the BMP388 on-device.
- **H4/H5 12-bit packing.** The most error-prone parse. _Mitigation:_ a dedicated
  test with a negative-coefficient golden vector pins the sign-extension.
- **VEML7700 integration-period timer.** The one fixed `Timer::after` could read as a
  "guessed delay." _Mitigation:_ the part exposes no data-ready signal (datasheet),
  so the integration time _is_ the conversion period; the value is computed from the
  selected setting and documented inline — admissible like a fixed ADC conversion time.
- **Auto-ranging oscillation.** A light level near a step boundary could ping-pong
  rungs. _Mitigation:_ the COUNT_LO=100 / COUNT_HI=10000 band is ~100× wide, far
  exceeding the ~2–4× per-rung count change, so a single step lands inside the band.
- **Shared-bus contention.** Four devices now share I2C0 (BMP388, MLX90614, BME280,
  VEML7700). _Mitigation:_ each task holds the async mutex for one transaction only
  (existing `bus.rs` contract); capacity-8 channel ≫ 4 producers at ≤1 Hz; bus stays
  at 100 kHz.
- **`Aggregator::new` signature change.** Breaks every existing aggregate test call
  site. _Mitigation:_ substep 5 explicitly updates them to a shared `TEST_CFG`; the
  compiler flags any missed site.
- **`ingest` const-fn constraint.** The new `mlx_fault = object_c.is_none()` line and
  the new arms must stay const-compatible. _Mitigation:_ `Option::is_none` and simple
  field assignment are const; if a future arm needs non-const ops, drop `const` from
  `ingest` (no caller depends on it being const).
- **Watchdog interaction.** New tasks deliberately have no beat; a hung (not just
  failed) BME/VEML task would not trip the RWDT. _Accepted_ per brainstorm — these
  are degradable sensors, and the BMP/AGG/BLE beats still cover executor liveness.

## Notes

Progress checkboxes (filled during `/tyrex:code:implement-light`):

- [x] 1. Diagnostics bits (frame.rs) + tests — added bits 2–5 (BME280/VEML7700/baro-divergence/MLX) with accessors + builders; 3 tests; spec review pass
- [x] 2. BME280 driver (bme280.rs) + tests — hand-rolled driver, calib parse incl. signed 12-bit H4/H5, f32 Bosch T/P/H compensation, 6 tests; mod.rs gets `pub mod bme280;`; spec review pass
- [x] 3. VEML7700 driver (veml7700.rs) + tests — resolution table, lux + Vishay nonlinearity, als_conf0 encoding, next_index auto-ranging; 9 tests; mod.rs gets `pub mod veml7700;`; spec review pass
- [x] 4. Re-exports (sensors/mod.rs, lib.rs) — lib.rs re-exports bme280+veml7700 (mod.rs lines added in substeps 2/3); meteo-lib builds, 61 tests pass
- [x] 5. Aggregate wiring (aggregate.rs) + tests — 4 new SensorReading variants, AggregatorConfig, new() sig change + TEST_CFG, ingest arms (incl. MLX-fault from SkyIr), diverged() cross-check, all 6 diagnostics bits; 10 new tests; spec review pass
- [x] 6. Firmware aggregator config (aggregator.rs) — OCCLUSION/TEMP/PRESS thresholds → AGG_CONFIG (AggregatorConfig); Aggregator::new(AGG_CONFIG); channel docstring → 4 producers; firmware builds
- [x] 7. BME280 task (bme.rs) — 1 Hz sampler mirroring bmp.rs, retry-init, sends Bme280/Bme280Fault, no watchdog beat (validated at substep 9 build)
- [x] 8. VEML7700 task (veml.rs) — auto-ranging loop split into init()/sample() helpers, integration-time wait (only timer, justified), discard-first-after-change, no watchdog beat (validated at substep 9 build)
- [x] 9. Firmware wiring (main.rs) — mod bme/veml, BME280_ADDR=0x76/VEML7700_ADDR=0x10, fresh I2cDevice handles, spawn both tasks; firmware builds + clippy clean (validates substeps 7-9)
- [x] 10. TUI diagnostics labels (model.rs) + tests — 4 new fmt_diagnostics arms (bit order), 5 tests incl. all-six-joined; 43 TUI tests pass; spec review pass
- [x] 11. Documentation (CLAUDE.md, README.md, datasheets) — CLAUDE.md: supported sensors, module tree (bme/veml + bme280/veml7700), diagnostics bits 0–5, four-device I2C bus; README.md: sensor list + workspace layout; datasheet already listed both addresses
