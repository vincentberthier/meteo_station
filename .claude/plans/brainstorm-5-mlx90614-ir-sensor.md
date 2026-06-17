# Plan: MLX90614 IR (Sky) Temperature Sensor + Aggregator Refactor

- **Source:** '5 (brainstorm `.claude/brainstorm/5-mlx90614-ir-sensor.md`)
- **Date:** 2026-06-17
- **Status:** Done

## Summary

Add the MLX90614 IR non-contact thermometer as the weather station's first second
sensor, and build the multi-sensor infrastructure brainstorm 4 specified (and this
work absorbs): a shared I2C0 bus, a tagged-`SensorReading` channel, and an aggregator
task that owns `TELEMETRY` and publishes a merged frame at 1 Hz. The MLX object
temperature populates the frame's existing `sky_temp_c` slot; its ambient (TA) reading
drives a new on-device occlusion diagnostic (`|TA_mlx − T_bmp| > 5 °C`) surfaced as
**frame v2**'s new diagnostics byte. The BMP388 task is reworked to send on the channel.
Decode hard-bumps to v2 (18 bytes); the TUI and acceptance scripts move with it.

## Resolved design decisions (from brainstorm open questions)

- **Decode policy:** hard v2 bump — `decode()` accepts **only** version 2 / 18 bytes.
- **MLX read cadence:** 2 s (gentler than the refresh rate; datasheet warns continuous
  reads add noise). Aggregator publishes at 1 Hz, decoupled.
- **Bus sharing:** `embassy-embedded-hal 0.6` `shared_bus::asynch::i2c::I2cDevice` over a
  `&'static embassy_sync::mutex::Mutex<CriticalSectionRawMutex, I2c>` (via `static-cell`).
  Verified: `embassy-embedded-hal 0.6.0` already binds `embassy-sync 0.8.0` — no version
  bridging (unlike the trouble-host case).
- **Channel topology:** one MPMC `Channel<CriticalSectionRawMutex, SensorReading, 8>`
  carrying a tagged enum; aggregator `select`s channel-receive vs a 1 Hz `Ticker`.
- **Diagnostics representation:** a `Diagnostics(u8)` newtype, added as a (non-`Option`)
  field on `Telemetry`. Bit 0 = sky-IR occlusion; **bit 1 = BMP388 fault** (sensor failed
  to init / a read forced re-init — surfaced from the resilient BMP task); bits 2–7
  reserved (0) for future per-sensor health flags. Per-sensor (not one generic "init
  failed") bit, since the platform is multi-sensor. The TUI renders the whole byte as a
  "Diagnostics" row (active flags joined, red when alerting), not just occlusion.
- **Watchdog:** add `AGG_BEAT`, bumped each aggregator publish. Feed RWDT iff
  `sampler_alive (BMP) && agg_alive && ble_alive`. All three beats are **task-liveness**
  signals (the loop is cycling), **not** sensor-data-presence signals — see the resilience
  decision below. The MLX task gets **no** dedicated beat (a failed MLX read is graceful →
  `sky_temp_c = None`).
- **Occlusion threshold:** `OCCLUSION_THRESHOLD_C = 5.0` (field-tunable constant).
- **MLX PWM→SMBus exit (now in-scope; was a deferred contingency).** On-device, plugging
  the MLX in jams I2C0 (`BMP388: I2c(Timeout)` + RWDT reboot loop) — the documented
  PWM-mode power-up. `main.rs` now holds SCL (GPIO11) low ≥ 1.44 ms (datasheet t_REQ)
  before bringing up I2C0, on every boot, to force the part into SMBus mode. Confirmed
  by the user: the BMP works with the MLX unplugged, so the MLX is conclusively the cause.
- **Sensor-task resilience (fragility fix).** A single sensor failing must not brick the
  device. The BMP task now **retries init in its loop** (instead of `return`ing on the
  first error) and bumps `BMP_BEAT` **every iteration** (task-liveness, decoupled from read
  success), so an absent/failing sensor degrades to `None` data without an RWDT reset loop.

## Files Modified

| File                                       | Action | Description                                                                                                                                                                 |
| ------------------------------------------ | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/meteo-lib/src/ble/frame.rs`        | modify | `Diagnostics` newtype (bit0 occlusion, bit1 BMP388 fault); `Telemetry.diagnostics` field; v2 encode/decode (`FRAME_VERSION=2`, `FRAME_LEN=18`, byte 17); layout doc + tests |
| `crates/meteo-lib/src/sensors/mlx90614.rs` | create | Hand-rolled `no_std` SMBus driver: CRC-8 PEC, error-flag, `raw·0.02−273.15`; pure helpers + tests                                                                           |
| `crates/meteo-lib/src/sensors/mod.rs`      | modify | `pub mod mlx90614;`                                                                                                                                                         |
| `crates/meteo-lib/src/aggregate.rs`        | create | `SensorReading` enum + `Aggregator` (merge + occlusion); pure, host-tested                                                                                                  |
| `crates/meteo-lib/src/lib.rs`              | modify | `pub mod aggregate;`; re-export `Diagnostics`, `SensorReading`, `Aggregator`, `mlx90614`                                                                                    |
| `Cargo.toml` (workspace)                   | modify | Add `embassy-embedded-hal = "0.6"` and `static-cell = "2.1"` to `[workspace.dependencies]`                                                                                  |
| `crates/meteo-firmware/Cargo.toml`         | modify | Add `embassy-embedded-hal` + `static-cell` to target deps (not `embedded-hal-async`; resolves transitively)                                                                 |
| `crates/meteo-firmware/src/bus.rs`         | create | `SharedI2c` type alias + the `&'static` bus `StaticCell`                                                                                                                    |
| `crates/meteo-firmware/src/aggregator.rs`  | create | `SENSOR_CHANNEL` static + `run` task (owns `TELEMETRY`, 1 Hz publish)                                                                                                       |
| `crates/meteo-firmware/src/watchdog.rs`    | modify | Add `AGG_BEAT`; gate RWDT on `bmp && agg && ble`; beats are task-liveness, not data-presence                                                                                |
| `crates/meteo-firmware/src/bmp.rs`         | modify | Take `SharedI2c`; send `SensorReading::Barometer` on the channel; **retry init + per-cycle heartbeat** (no reboot loop on sensor failure)                                   |
| `crates/meteo-firmware/src/mlx.rs`         | create | MLX task: read object+ambient, send `SensorReading::SkyIr`                                                                                                                  |
| `crates/meteo-firmware/src/main.rs`        | modify | **MLX PWM-exit (SCL-low ≥1.44 ms) before I2C init**; build shared bus; spawn aggregator + bmp + mlx; `MLX90614_ADDR=0x5A`                                                   |
| `crates/meteo-firmware/src/ble.rs`         | modify | Replace hardcoded `17`/`[u8; 17]` with `meteo_lib::FRAME_LEN` (= 18)                                                                                                        |
| `crates/meteo-tui/src/model.rs`            | modify | `fmt_diagnostics` + `diagnostics_alert`; import `Diagnostics`                                                                                                               |
| `crates/meteo-tui/src/ui.rs`               | modify | Add "Diagnostics" row (red when alerting); bump table area `Length(10)→Length(11)`                                                                                          |
| `scripts/ble_notify_check.sh`              | modify | `FRAME_LEN` default `17→18`; byte[0] `0x01→0x02`; docstring                                                                                                                 |
| `scripts/ble_soak.sh`                      | verify | Confirm it has no frame-length/version assertion (link-only); no change expected                                                                                            |
| `CLAUDE.md`                                | modify | Update GATT/wire-frame notes to v2 (18 bytes, byte[0]==0x02, diagnostics byte)                                                                                              |

## Plan

### 1. Frame v2: `Diagnostics` newtype + `Telemetry` field + encode/decode

**File:** `crates/meteo-lib/src/ble/frame.rs`

Bump the wire format and add the diagnostics byte. This is the foundation every other
substep builds on, so it lands first.

**Constants:**

```rust
pub const FRAME_VERSION: u8 = 2;
pub const FRAME_LEN: usize = 18;
```

**New `Diagnostics` type (place above `Telemetry`):**

```rust
/// Per-frame health/diagnostics bitfield (frame v2, byte 17).
///
/// Bit 0 = sky-IR sensor ambient diverges from the barometer air temperature
/// beyond the configured threshold (possible occlusion / icing).
/// Bit 1 = BMP388 fault (not initialized / read failing). Bits 2–7 are reserved
/// and always 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Diagnostics(pub u8);

impl Diagnostics {
    /// Bit 0: sky-IR ambient diverges from barometer air temp (occlusion/icing).
    pub const SKY_IR_OCCLUSION: u8 = 1 << 0;
    /// Bit 1: BMP388 not providing data — failed to initialize, or a read error
    /// forced a re-init. While set, `temperature_c`/`pressure_hpa` are `None`.
    pub const BARO_FAULT: u8 = 1 << 1;
    // Bits 2–7 reserved (0) for future per-sensor health flags (BME280, VEML7700, MLX).

    /// All-clear diagnostics (no flags set).
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns `true` if the sky-IR occlusion bit is set.
    #[must_use]
    pub const fn occlusion(self) -> bool {
        self.0 & Self::SKY_IR_OCCLUSION != 0
    }

    /// Returns a copy with the occlusion bit set to `set`.
    #[must_use]
    pub const fn with_occlusion(self, set: bool) -> Self {
        self.with_flag(Self::SKY_IR_OCCLUSION, set)
    }

    /// Returns `true` if the BMP388 fault bit is set.
    #[must_use]
    pub const fn baro_fault(self) -> bool {
        self.0 & Self::BARO_FAULT != 0
    }

    /// Returns a copy with the BMP388 fault bit set to `set`.
    #[must_use]
    pub const fn with_baro_fault(self, set: bool) -> Self {
        self.with_flag(Self::BARO_FAULT, set)
    }

    /// Returns a copy with `mask`'s bit(s) set to `set` (shared helper).
    #[must_use]
    const fn with_flag(self, mask: u8, set: bool) -> Self {
        if set {
            Self(self.0 | mask)
        } else {
            Self(self.0 & !mask)
        }
    }
}
```

**`Telemetry` change:** add a non-`Option` field (diagnostics is always present):

```rust
pub struct Telemetry {
    // ... existing fields ...
    /// Battery charge level in percent (0–100).
    pub battery_pct: Option<u8>,
    /// Per-frame diagnostics bitfield (frame v2).
    pub diagnostics: Diagnostics,
}
```

Update `Telemetry::empty()` to set `diagnostics: Diagnostics::empty()`. `from_bmp388`
already spreads `..Self::empty()`, so it is unchanged (it keeps `diagnostics` cleared).

**`encode`** — append the diagnostics byte:

```rust
frame[16] = self.battery_pct.unwrap_or(0xFF);
frame[17] = self.diagnostics.0;
frame
```

(`frame` is `[0_u8; FRAME_LEN]`, now 18 bytes.)

**`decode`** — read byte 17 (version check unchanged; it rejects anything != 2):

```rust
let diagnostics = Diagnostics(bytes[17]);
Ok(Self {
    // ... existing fields ...
    battery_pct,
    diagnostics,
})
```

**Layout doc comment** — add a row to the module table:

```
//! | 17    | diagnostics         | u8        | bitfield: bit0 = sky-IR occlusion, bit1 = BMP388 fault, bits 2–7 reserved | — (always present) |
```

and update the header line `… fixed-length, little-endian, 17 bytes.` → `18 bytes.`

**Tests to add / change (in the existing `mod tests`):**

- _Change_ `encode_emits_seventeen_bytes_with_version_one` → rename
  `encode_emits_eighteen_bytes_with_version_two`: assert `frame.len() == 18` and
  `frame[0] == 2`.
- _Change_ `decode_rejects_unknown_version`: build `[0_u8; 18]` with `frame[0] = 3`,
  assert `Err(FrameError::UnknownVersion(3))` (2 is now valid; length is correct so the
  version branch is reached).
- _Change_ `decode_rejects_wrong_length`: `[0_u8; 17]` now yields
  `Err(FrameError::WrongLength(17))` (17 is no longer the valid length). Keep a
  `[0_u8; 16]` → `WrongLength(16)` assertion too.
- _Change_ `decode_maps_sentinels_back_to_none`: also assert
  `decoded.diagnostics == Diagnostics::empty()`.
- _Add_ `encode_writes_diagnostics_byte`: given
  `Telemetry { diagnostics: Diagnostics(0b0000_0001), ..empty() }`, assert
  `frame[17] == 0x01`.
- _Add_ `decode_reads_diagnostics_byte_roundtrip`: encode a `Telemetry` with
  `diagnostics: Diagnostics::empty().with_occlusion(true)`, decode, assert
  `decoded.diagnostics.occlusion()`.
- _Add_ `diagnostics_with_occlusion_sets_and_clears_bit0`: pure test of
  `Diagnostics::empty().with_occlusion(true).occlusion() == true` and
  `…with_occlusion(false).occlusion() == false`.
- _Add_ `diagnostics_baro_fault_is_independent_of_occlusion`: assert the two flags don't
  collide — `Diagnostics::empty().with_baro_fault(true)` has `baro_fault() == true` and
  `occlusion() == false` (and its raw value is `0b10`); setting both gives `0b11` with both
  predicates true.
- _Change_ the `roundtrip_decode_encode_is_identity_at_wire_level` proptest: array size
  `[0_u8; FRAME_LEN]` (18), `bytes[0] = FRAME_VERSION` (2), and add a generated
  `b17 in any::<u8>()` written to `bytes[17]` (diagnostics has no sentinel, so it
  round-trips bit-exact).

**Done when:** `cargo nextest run -p meteo-lib` passes with the new + changed frame
tests, and `cargo test --doc -p meteo-lib` passes.

### 2. MLX90614 driver

**File:** `crates/meteo-lib/src/sensors/mlx90614.rs` (new); register in
`crates/meteo-lib/src/sensors/mod.rs` with `pub mod mlx90614;`.

Hand-rolled `embedded-hal-async` `no_std` SMBus driver, mirroring `bmp388.rs` (pure
logic host-tested; the async read path is not unit-tested, same as BMP388).

**Constants & types:**

```rust
use embedded_hal_async::i2c::I2c;

/// RAM command opcodes (SMBus Read-Word). Datasheet `datasheets/mlx90614.md`.
const RAM_TA: u8 = 0x06; // linearized ambient temperature
const RAM_TOBJ1: u8 = 0x07; // linearized object temperature, sensor 1

/// SMBus PEC CRC-8 polynomial X⁸+X²+X¹+1 (0x07), init 0x00.
const PEC_POLY: u8 = 0x07;

/// MLX90614 error-flag mask: RAM bit 15 set → reading invalid.
const ERROR_FLAG: u16 = 0x8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// Underlying I2C/SMBus transport error.
    I2c(E),
    /// PEC (CRC-8) mismatch: the reading is corrupt.
    Pec { expected: u8, got: u8 },
    /// RAM bit 15 was set: the sensor flagged the reading invalid.
    ErrorFlag,
}

pub struct Mlx90614<I> {
    i2c: I,
    address: u8,
}
```

**Pure helpers (these are the host-tested core):**

```rust
/// SMBus PEC: CRC-8 with polynomial 0x07, init 0x00, no reflection, no final XOR.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for &byte in data {
        crc ^= byte;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ PEC_POLY
            } else {
                crc << 1
            };
            bit += 1;
        }
    }
    crc
}

/// PEC over a Read-Word transaction: [SA_W, command, SA_R, LSByte, MSByte].
fn pec_for_read(address: u8, command: u8, lsb: u8, msb: u8) -> u8 {
    let sa_w = address << 1;
    let sa_r = (address << 1) | 1;
    crc8(&[sa_w, command, sa_r, lsb, msb])
}

/// Convert a raw 16-bit RAM temperature word to °C, or `None` if the error
/// flag (bit 15) is set. Formula: `T = raw · 0.02 − 273.15`.
fn temperature_from_raw(raw: u16) -> Option<f32> {
    if raw & ERROR_FLAG != 0 {
        return None;
    }
    Some(f32::from(raw) * 0.02 - 273.15)
}
```

**Async methods:**

```rust
impl<I, E> Mlx90614<I>
where
    I: I2c<Error = E>,
{
    /// Creates a driver bound to `address` (no bus traffic; MLX90614 has no
    /// chip-ID register — presence is established by the first successful read).
    #[must_use]
    pub const fn new(i2c: I, address: u8) -> Self {
        Self { i2c, address }
    }

    /// Object (IR) temperature in °C from RAM `0x07` (TOBJ1).
    ///
    /// # Errors
    /// `Error::I2c` on transport failure, `Error::Pec` on CRC mismatch,
    /// `Error::ErrorFlag` if the sensor flags the reading invalid.
    pub async fn object_temperature(&mut self) -> Result<f32, Error<E>> {
        let raw = self.read_ram(RAM_TOBJ1).await?;
        temperature_from_raw(raw).ok_or(Error::ErrorFlag)
    }

    /// Ambient (TA) temperature in °C from RAM `0x06`. Used as the occlusion
    /// health proxy; never reported as a telemetry temperature field.
    pub async fn ambient_temperature(&mut self) -> Result<f32, Error<E>> {
        let raw = self.read_ram(RAM_TA).await?;
        temperature_from_raw(raw).ok_or(Error::ErrorFlag)
    }

    /// SMBus Read-Word of a RAM cell with PEC verification.
    async fn read_ram(&mut self, command: u8) -> Result<u16, Error<E>> {
        let mut buf = [0_u8; 3]; // LSB, MSB, PEC
        self.i2c
            .write_read(self.address, &[command], &mut buf)
            .await
            .map_err(Error::I2c)?;
        let (lsb, msb, pec) = (buf[0], buf[1], buf[2]);
        let expected = pec_for_read(self.address, command, lsb, msb);
        if expected != pec {
            return Err(Error::Pec { expected, got: pec });
        }
        Ok(u16::from_le_bytes([lsb, msb]))
    }
}
```

`write_read(addr, &[command], &mut buf)` realizes the SMBus Read-Word framing
`S [SA_W] [Cmd] Sr [SA_R] [LSB][MSB][PEC]` over `embedded-hal-async`.

**Tests (host, `mod tests` with the standard layout):**

- `crc8_matches_smbus_check_vector`: `crc8(b"123456789") == 0xF4` (the canonical
  CRC-8/SMBus check value — authoritative, not self-referential).
- `crc8_empty_is_zero`: `crc8(&[]) == 0x00`.
- `pec_for_read_uses_shifted_addresses`: for `address = 0x5A`, assert the helper feeds
  `0xB4` (SA_W) and `0xB5` (SA_R) — e.g. assert
  `pec_for_read(0x5A, 0x07, 0x00, 0x00) == crc8(&[0xB4, 0x07, 0xB5, 0x00, 0x00])`.
- `temperature_from_raw_converts_object_temp`: `raw = 0x39CE` (14798) →
  `14798·0.02 − 273.15 = 22.81 °C`; assert `(v - 22.81).abs() < 0.01`.
- `temperature_from_raw_rejects_error_flag`: `temperature_from_raw(0x8000) == None`.
- `temperature_from_raw_handles_zero_celsius`: `raw = 13657` (0x3559) →
  `≈ −0.01..0.0 °C` finite; assert it is `Some` and within `[−1.0, 1.0]`.

**Done when:** `cargo nextest run -p meteo-lib` passes the MLX tests; the driver compiles
on the firmware target (verified in substep 8's build).

### 3. Aggregator + `SensorReading` (hardware-agnostic, host-tested)

**File:** `crates/meteo-lib/src/aggregate.rs` (new); register in `lib.rs` with
`pub mod aggregate;`.

The merge + occlusion logic lives here as pure code so it is host-tested; the firmware
task (substep 5) is a thin async shell around it.

```rust
//! Multi-sensor telemetry aggregation: merge per-sensor readings into one
//! running `Telemetry` and derive on-device diagnostics (sky-IR occlusion,
//! BMP388 fault).

use libm::fabsf;

use crate::ble::frame::{Diagnostics, Telemetry};

/// A reading (or health signal) from one sensor, sent over the inter-task
/// channel to the aggregator.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SensorReading {
    /// BMP388: authoritative air temperature (°C) + pressure (hPa).
    Barometer { temperature_c: f32, pressure_hpa: f32 },
    /// BMP388 is not providing data (failed to initialize, or a read error
    /// forced a re-init). Clears temperature/pressure and raises `BARO_FAULT`.
    BarometerFault,
    /// MLX90614: object/IR temp → `sky_temp_c`; ambient (TA) → occlusion proxy.
    /// Either may be `None` on a failed/invalid read (graceful degradation).
    SkyIr { object_c: Option<f32>, ambient_c: Option<f32> },
}

/// Merges per-sensor readings into one running `Telemetry` and derives the
/// diagnostics bitfield (sky-IR occlusion, BMP388 fault). Holds the latest
/// barometer air temperature and MLX ambient so occlusion can be (re)derived on
/// every publish, plus the latched barometer-fault state.
pub struct Aggregator {
    telemetry: Telemetry,
    air_temp_c: Option<f32>,
    sky_ambient_c: Option<f32>,
    baro_fault: bool,
    occlusion_threshold_c: f32,
}

impl Aggregator {
    /// New aggregator with all fields empty and the given occlusion threshold (°C).
    #[must_use]
    pub const fn new(occlusion_threshold_c: f32) -> Self {
        Self {
            telemetry: Telemetry::empty(),
            air_temp_c: None,
            sky_ambient_c: None,
            baro_fault: false,
            occlusion_threshold_c,
        }
    }

    /// Fold one reading into the running state.
    pub fn ingest(&mut self, reading: SensorReading) {
        match reading {
            SensorReading::Barometer { temperature_c, pressure_hpa } => {
                self.telemetry.temperature_c = Some(temperature_c);
                self.telemetry.pressure_hpa = Some(pressure_hpa);
                self.air_temp_c = Some(temperature_c);
                self.baro_fault = false;
            }
            SensorReading::BarometerFault => {
                // Sensor down: blank its data and latch the fault for the diagnostics
                // byte. occlusion can no longer be computed (air_temp gone → false).
                self.telemetry.temperature_c = None;
                self.telemetry.pressure_hpa = None;
                self.air_temp_c = None;
                self.baro_fault = true;
            }
            SensorReading::SkyIr { object_c, ambient_c } => {
                // A failed/invalid MLX read (object_c == None) blanks sky_temp_c
                // for subsequent frames until the next good read — matches the
                // brainstorm's graceful-degradation rule.
                self.telemetry.sky_temp_c = object_c;
                self.sky_ambient_c = ambient_c;
            }
        }
    }

    /// Current merged frame, with the diagnostics bits (re)computed.
    #[must_use]
    pub fn snapshot(&self) -> Telemetry {
        let mut t = self.telemetry;
        t.diagnostics = Diagnostics::empty()
            .with_occlusion(self.occluded())
            .with_baro_fault(self.baro_fault);
        t
    }

    /// Occluded iff both air and MLX-ambient are known and diverge beyond the
    /// threshold. Unknown inputs → not occluded (cannot determine).
    fn occluded(&self) -> bool {
        match (self.air_temp_c, self.sky_ambient_c) {
            (Some(air), Some(amb)) => fabsf(amb - air) > self.occlusion_threshold_c,
            _ => false,
        }
    }
}
```

**Tests (host):**

- `aggregator_merges_barometer_and_sky_into_one_frame`: ingest
  `Barometer { 20.0, 1013.0 }` then `SkyIr { object_c: Some(-15.0), ambient_c: Some(19.0) }`;
  assert `snapshot()` has `temperature_c == Some(20.0)`, `pressure_hpa == Some(1013.0)`,
  `sky_temp_c == Some(-15.0)`, and `diagnostics.baro_fault() == false`.
- `aggregator_sets_occlusion_bit_when_ambient_diverges`: threshold 5.0, ingest
  `Barometer { 20.0, .. }` + `SkyIr { object_c: Some(-10.0), ambient_c: Some(30.0) }`;
  assert `snapshot().diagnostics.occlusion()` is `true` (|30−20| = 10 > 5).
- `aggregator_clears_occlusion_within_threshold`: `Barometer { 20.0, .. }` +
  `SkyIr { ambient_c: Some(22.0), .. }`, threshold 5.0 → occlusion `false`.
- `aggregator_occlusion_false_at_exact_threshold`: `Barometer { 20.0, .. }` +
  `SkyIr { ambient_c: Some(25.0), .. }`, threshold 5.0 → occlusion `false` (the test
  pins the strict `>` comparison: a difference _equal_ to the threshold is not occluded,
  guarding against an accidental `>=`).
- `aggregator_no_occlusion_when_ambient_missing`: only `Barometer`, no `SkyIr` →
  occlusion `false`.
- `aggregator_sky_temp_none_on_failed_read`: ingest a good `SkyIr` then
  `SkyIr { object_c: None, ambient_c: None }`; assert `snapshot().sky_temp_c == None`.
- `aggregator_barometer_fault_sets_bit_and_blanks_data`: ingest `Barometer { 20.0, 1013.0 }`
  then `BarometerFault`; assert `snapshot()` has `temperature_c == None`,
  `pressure_hpa == None`, and `diagnostics.baro_fault() == true`.
- `aggregator_barometer_reading_clears_fault`: ingest `BarometerFault` then
  `Barometer { 21.0, 1012.0 }`; assert `diagnostics.baro_fault() == false` and
  `temperature_c == Some(21.0)`.
- `aggregator_baro_fault_forces_occlusion_false`: ingest `SkyIr { ambient_c: Some(99.0), .. }`
  then `BarometerFault`; assert `diagnostics.occlusion() == false` (air temp gone → cannot
  compute) while `diagnostics.baro_fault() == true`.

**Done when:** `cargo nextest run -p meteo-lib` passes the aggregator tests.

### 4. `lib.rs` re-exports

**File:** `crates/meteo-lib/src/lib.rs`

```rust
pub mod aggregate;
pub mod ble;
pub mod sensors;
pub mod utils;

pub use aggregate::{Aggregator, SensorReading};
pub use ble::frame::{Diagnostics, FRAME_LEN, FRAME_VERSION, FrameError, Telemetry};
pub use sensors::{bmp388, mlx90614};
pub use utils::trunc2;
```

**Done when:** `cargo build -p meteo-lib` and `cargo build -p meteo-tui --target
x86_64-unknown-linux-gnu` both compile against the new re-exports.

### 5. Dependencies (workspace + firmware)

**File:** `Cargo.toml` (workspace) — add to `[workspace.dependencies]`:

```toml
embassy-embedded-hal = { version = "0.6", default-features = false }
static-cell = "2.1"
```

(Versions: `embassy-embedded-hal 0.6.0` is **already in `Cargo.lock`** (transitive) and
resolves against `embassy-sync 0.8.0`, so adding it as a direct dep reuses that resolution
— no version skew. `static-cell` is **not** currently in the tree; it is a new dependency
that will be freshly resolved — latest is `2.1.1`, so the `"2.1"` requirement is correct.
Confirm both with `cargo update --dry-run` / `cargo tree` after editing, and run
`cargo deny check` + `cargo audit` on the new `static-cell` dep.)

**File:** `crates/meteo-firmware/Cargo.toml` — add to the
`[target.'cfg(target_arch = "riscv32")'.dependencies]` block:

```toml
embassy-embedded-hal = { workspace = true }
static-cell = { workspace = true }
```

(Only these two. `embedded-hal-async` is **deliberately not** added as a firmware direct
dep: with the `I2cDevice` approach the firmware never names the
`embedded_hal_async::i2c::I2c` trait — `I2cDevice` implements it, and the `Bmp388<I>` /
`Mlx90614<I>` `I: I2c` bounds resolve in `meteo-lib` (which owns the `embedded-hal-async`
dep). Trait-impl resolution works across the whole dependency graph without a direct edge
or a `use`, so adding it here would be an unused dep that `cargo-machete` flags. Verified
at plan time: the firmware target-deps block lists `defmt`, `embassy-*`, `esp-*`, and
`trouble-host` only — both lines above are genuinely new.)

**Done when:** `cargo build --release` (firmware target) resolves the new deps; if the
shared-bus `I2cDevice` unexpectedly fails the `I: I2c` bound for `Bmp388`/`Mlx90614`, the
fallback is to add `embedded-hal-async = { workspace = true }` here too — but that should
not be necessary.

### 6. Shared I2C bus (`SharedI2c`)

**File:** `crates/meteo-firmware/src/bus.rs` (new); add `mod bus;` to `main.rs`.

```rust
//! Shared I2C0 bus: a single `&'static` async mutex over the esp-hal I2c, with
//! per-sensor `I2cDevice` handles. Each `embedded-hal-async` transaction locks
//! the bus for its duration and releases — whole transactions interleave on the
//! wire (standard multi-device I2C), so the BMP388 and MLX90614 tasks share GPIO10/11.

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_hal::Async;
use esp_hal::i2c::master::I2c;
use static_cell::StaticCell;

/// Concrete shared-bus I2C handle handed to each sensor task.
pub type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>;

/// Backing storage for the one shared I2C0 bus mutex.
pub static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c<'static, Async>>> =
    StaticCell::new();
```

`I2cDevice::new(bus)` is called in `main.rs` (substep 9) once per sensor task. Note:
`embassy_sync::mutex::Mutex` is an **async** mutex designed to be held across `.await`;
the shared-bus pattern locks it for one transaction. This is _not_ the
"mutex-across-await deadlock" anti-pattern (that concerns blocking mutexes / re-entrant
locks) — each task locks once per transaction and releases.

**Done when:** compiles as part of substep 9's firmware build.

### 7. Aggregator task + channel + watchdog beat

**File:** `crates/meteo-firmware/src/aggregator.rs` (new); add `mod aggregator;` to
`main.rs`.

```rust
//! Aggregator task: owns `TELEMETRY`, drains the sensor channel into a running
//! `meteo_lib::Aggregator`, and publishes a merged frame at 1 Hz.

use core::sync::atomic::Ordering;

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker};
use meteo_lib::{Aggregator, SensorReading};

// TELEMETRY stays declared in ble.rs (unchanged: it is the latest-wins
// `Signal<CriticalSectionRawMutex, Telemetry>` the notify loop waits on). The
// aggregator is now its sole producer; the dependency direction is
// aggregator → ble (the BLE task no longer imports anything from the aggregator).
use crate::ble::TELEMETRY;
use crate::watchdog::AGG_BEAT;

/// Sky-IR occlusion threshold (°C). Field-tunable; revisit during real-sky testing.
const OCCLUSION_THRESHOLD_C: f32 = 5.0;

/// Inter-task sensor channel: every sensor task sends `SensorReading`s here; the
/// aggregator is the sole receiver. Capacity 8 ≫ the 2 producers at ≤1 Hz.
pub static SENSOR_CHANNEL: Channel<CriticalSectionRawMutex, SensorReading, 8> = Channel::new();

#[embassy_executor::task]
pub async fn run() {
    let mut agg = Aggregator::new(OCCLUSION_THRESHOLD_C);
    // Publish cadence: 1 Hz, decoupled from sensor read rates. A periodic Ticker is
    // the intended publish clock (a 1 Hz wall-clock publish is only observable via a
    // timer) — NOT a readiness sleep; cf. the BMP sampler and watchdog poll.
    let mut publish = Ticker::every(Duration::from_secs(1));
    loop {
        match select(SENSOR_CHANNEL.receive(), publish.next()).await {
            Either::First(reading) => agg.ingest(reading),
            Either::Second(()) => {
                TELEMETRY.signal(agg.snapshot());
                AGG_BEAT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
```

**File:** `crates/meteo-firmware/src/watchdog.rs` — add the aggregator beat and gate on it:

```rust
/// Bumped by the aggregator after each 1 Hz publish to `TELEMETRY`.
pub static AGG_BEAT: AtomicU32 = AtomicU32::new(0);
```

In `supervise`, extend the tracking tuple and the feed condition:

```rust
let (mut last_bmp, mut last_ble, mut last_adv, mut last_agg) = (0_u32, 0_u32, 0_u32, 0_u32);
// ... inside loop, after loading bmp/ble/adv:
let agg = AGG_BEAT.load(Ordering::Relaxed);
let sampler_alive = bmp != last_bmp;
let agg_alive = agg != last_agg;
let ble_alive = adv != last_adv || ble != last_ble;

if sampler_alive && agg_alive && ble_alive {
    rtc.rwdt.feed();
    trace!("rwdt fed (bmp={=u32} agg={=u32} adv={=u32} ble={=u32})", bmp, agg, adv, ble);
} else {
    warn!(
        "rwdt withheld — sampler_alive={=bool} agg_alive={=bool} ble_alive={=bool}",
        sampler_alive, agg_alive, ble_alive
    );
}
(last_bmp, last_ble, last_adv, last_agg) = (bmp, ble, adv, agg);
```

Update the doc comment on `supervise` to mention the three gates (BMP sampler,
aggregator publish, BLE liveness) and why the MLX task has no dedicated beat.

**Semantics note (changed in substep 8):** `BMP_BEAT` now means "the BMP task loop is
cycling" (bumped every iteration), **not** "a read succeeded". So `sampler_alive` is a
_task-liveness_ gate, not a _data-availability_ gate — a hung/deadlocked task still trips
the RWDT, but an absent/failing BMP sensor degrades gracefully (no Barometer frames)
without a reset loop. Update the `BMP_BEAT` doc comment in `watchdog.rs` accordingly (it
currently reads "after each successful sensor read"). The same applies to `AGG_BEAT`
(aggregator loop cycling) and the BLE beats — all three gates watch executor/task
liveness, never sensor data presence.

**Done when:** firmware builds; `defmt` `rwdt fed (… agg=…)` appears in `just run` logs
(manual on-device check, part of the acceptance gate).

### 8. BMP388 task rework (send on channel) + init/read resilience

**File:** `crates/meteo-firmware/src/bmp.rs`

Two changes: (a) the task takes a `SharedI2c` and sends `SensorReading::Barometer` instead
of signaling `TELEMETRY`; (b) it is **made resilient** so a sensor that fails to init or
read no longer bricks the device into an RWDT reboot loop — and, while faulting, it emits
`SensorReading::BarometerFault` so the fault surfaces as the `BARO_FAULT` diagnostic bit
(rendered in the TUI, substep 12).

**The fragility being fixed (observed on-device).** With the MLX jamming I2C0, the current
task's `Bmp388::new` returns `Err(I2c(Timeout))` and the task does `return` — so it exits
permanently, `BMP_BEAT` freezes at 0, the watchdog withholds the RWDT feed, and the chip
reboots every ~8 s (`rst:0x10 LP_WDT_SYS`) forever. Root causes: (1) init failure is fatal
(no retry); (2) `BMP_BEAT` conflates "a read succeeded" with "the task is alive", so a dead
_sensor_ trips a _liveness_ watchdog. Fix both: retry init in the loop, and bump `BMP_BEAT`
once per loop iteration (proving the task is cycling) regardless of read outcome.

```rust
use core::sync::atomic::Ordering;

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::bmp388::Bmp388;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

#[embassy_executor::task]
pub async fn read_barometer(i2c: SharedI2c, address: u8) {
    debug!("Setting up barometer");
    // `None` until initialized. `SharedI2c` (an `I2cDevice`) is `Clone` and cheap to
    // copy (it just holds the `&'static Mutex` bus ref), so each (re)init attempt gets
    // a fresh handle while the task keeps the original for the next retry.
    let mut sensor: Option<Bmp388<SharedI2c>> = None;

    loop {
        // (Re)initialize on demand: covers a slow/absent sensor at boot and a bus
        // glitch that forced a re-init below — instead of returning (which froze the
        // task and reboot-looped the chip).
        if sensor.is_none() {
            match Bmp388::new(i2c.clone(), address).await {
                Ok(s) => {
                    info!("BMP388 initialized successfully!");
                    sensor = Some(s);
                }
                Err(e) => warn!("BMP388 init failed, retrying: {:?}", Debug2Format(&e)),
            }
        }

        if let Some(s) = sensor.as_mut() {
            match s.read().await {
                Ok(reading) => {
                    info!(
                        "Temperature: {}°C, Pressure: {} Pa ({} hPa)",
                        trunc2(reading.temperature),
                        trunc2(reading.pressure),
                        trunc2(reading.pressure_hpa())
                    );
                    SENSOR_CHANNEL
                        .send(SensorReading::Barometer {
                            temperature_c: reading.temperature,
                            pressure_hpa: reading.pressure_hpa(),
                        })
                        .await;
                }
                Err(e) => {
                    // Drop the driver and re-init next cycle so a transient bus fault
                    // self-heals rather than wedging on a stale handle.
                    warn!("BMP read failed, re-initializing: {:?}", Debug2Format(&e));
                    sensor = None;
                }
            }
        }

        // Report a fault whenever there is no live handle this cycle (init failed, or a
        // read error just dropped it). The aggregator blanks temp/pressure and raises
        // the BARO_FAULT diagnostic bit; a later successful read clears it.
        if sensor.is_none() {
            SENSOR_CHANNEL.send(SensorReading::BarometerFault).await;
        }

        // Liveness heartbeat: bumped EVERY iteration to prove the task is cycling
        // (executor alive), NOT only on a successful read. A dead/absent sensor →
        // no Barometer frames (graceful: temperature/pressure go None downstream),
        // but the task stays alive and the RWDT is not falsely tripped. A genuinely
        // hung task (e.g. deadlocked on the bus mutex) still stalls BMP_BEAT → reset.
        crate::watchdog::BMP_BEAT.fetch_add(1, Ordering::Relaxed);

        // Sampling cadence (1 Hz). Periodic sample clock, not a readiness sleep.
        // `Timer::after` (not `Ticker::every`) is deliberate for samplers: it spaces
        // reads by a guaranteed gap *after* each read completes, so a slow bus read
        // can't make ticks pile up and back-to-back hammer the sensor. The aggregator
        // uses `Ticker::every` instead because its publish must hold a fixed 1 Hz
        // wall-clock cadence independent of how long ingest takes. A few ms of
        // per-cycle drift here is irrelevant for a weather sampler.
        Timer::after(Duration::from_secs(1)).await;
    }
}
```

Import cleanup (required — a stale unused import fails `just clippy`): the current
`bmp.rs` line 13 is `use meteo_lib::{Telemetry, trunc2};` → drop `Telemetry`, becoming
`use meteo_lib::{SensorReading, trunc2};`. Also note the sketch's defmt import drops
`error` (init failure now `warn`s + retries rather than `error`s + returns); keep
`Ordering` (used by the `BMP_BEAT` bump).

`Telemetry::from_bmp388` is now unused by the firmware but retained (public, tested) as
the documented BMP→Telemetry mapping; no change.

**Tests:** the firmware task itself is not unit-tested (project convention), but the
resilience is verifiable on-device (see the acceptance gate): with the MLX plugged in, the
device must **no longer reboot-loop** — `BMP388 init failed, retrying` may appear, the
chip stays up and advertising, and once the bus is clean (substep 10's PWM-exit) the BMP
inits and `Temperature:` lines resume.

**Done when:** firmware builds and `just clippy` is clean (no unused-import warning); the
BMP task no longer references `crate::ble::TELEMETRY` or constructs a `Telemetry`; a forced
init failure (e.g. wrong address) no longer triggers an RWDT reset loop.

### 9. MLX90614 task

**File:** `crates/meteo-firmware/src/mlx.rs` (new); add `mod mlx;` to `main.rs`.

```rust
//! MLX90614 sky-IR sampler: reads object + ambient temperature every 2 s and
//! sends a `SensorReading::SkyIr` to the aggregator. Failed/invalid reads send
//! `None` (graceful degradation → sky_temp_c blanks for that frame).

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::mlx90614::Mlx90614;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

#[embassy_executor::task]
pub async fn read_sky(i2c: SharedI2c, address: u8) {
    debug!("Setting up MLX90614 sky-IR sensor");
    let mut sensor = Mlx90614::new(i2c, address);

    loop {
        let object_c = match sensor.object_temperature().await {
            Ok(v) => { info!("Sky/object temp: {}°C", trunc2(v)); Some(v) }
            Err(e) => { warn!("MLX object read failed: {:?}", Debug2Format(&e)); None }
        };
        let ambient_c = match sensor.ambient_temperature().await {
            Ok(v) => Some(v),
            Err(e) => { warn!("MLX ambient read failed: {:?}", Debug2Format(&e)); None }
        };
        SENSOR_CHANNEL
            .send(SensorReading::SkyIr { object_c, ambient_c })
            .await;
        // Read cadence: 2 s (gentler than the refresh rate; datasheet warns
        // continuous reads add noise). Periodic sample clock, not a readiness sleep.
        // `Timer::after` (gap-after-read), not `Ticker::every` — same rationale as the
        // BMP sampler: guarantee spacing between reads rather than a fixed cadence.
        Timer::after(Duration::from_secs(2)).await;
    }
}
```

**Done when:** firmware builds; on-device `just run` logs show `Sky/object temp: …°C`.

### 10. `main.rs` wiring

**File:** `crates/meteo-firmware/src/main.rs`

- Add modules: `mod aggregator; mod bus; mod mlx;` (alongside `mod ble; mod bmp; mod
watchdog;`).
- Add `const MLX90614_ADDR: u8 = 0x5A;` next to `BMP388_ADDR`.
- Add the MLX PWM→SMBus exit pulse constant near the other consts:
  `const MLX_PWM_EXIT_SCL_LOW: Duration = Duration::from_micros(2000);`
- Replace the single-owner I2C/BMP spawn block with the PWM-exit nudge, shared-bus
  construction, and three spawns:

```rust
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::mutex::Mutex;
// Output, OutputConfig, Level, Timer, Duration are already imported in main.rs.
use crate::bus::{I2C_BUS, SharedI2c};

// in main(), replacing the current `let i2c = … spawner.spawn(bmp::read_barometer(…))`:

// MLX90614 PWM→SMBus exit. Some GY-906 breakouts power up driving a PWM waveform on
// the shared SDA/PWM pin, which jams I2C0 for EVERY device on the bus — observed
// on-device as `BMP388: I2c(Timeout)` + an RWDT reboot loop the moment the MLX is
// plugged in. Per the datasheet (t_REQ ≥ 1.44 ms), holding SCL low for that long
// forces the part into SMBus mode for the session. This must run on EVERY boot
// (the MLX re-enters PWM at each power-on if its EEPROM has PWM enabled) and BEFORE
// I2C0 takes the pins. It is a protocol-mandated minimum pulse width with NO
// observable ready signal (the mode switch is internal, only confirmable by a
// subsequent successful SMBus read) — this is the admissible hardware-timing case,
// like a reset-pulse width, NOT a readiness guess. Harmless if the part was already
// in SMBus mode. `reborrow()` lets GPIO11 be driven here and then handed to I2C0.
{
    let _scl_low = Output::new(
        peripherals.GPIO11.reborrow(),
        Level::Low,
        OutputConfig::default(),
    );
    Timer::after(MLX_PWM_EXIT_SCL_LOW).await;
} // `_scl_low` dropped → GPIO11 released (external 4.7 kΩ pull-up returns it high)

let i2c = I2c::new(
    peripherals.I2C0,
    I2cConfig::default().with_frequency(Rate::from_khz(100)),
)
.expect("I2C0 init")
.with_sda(peripherals.GPIO10)
.with_scl(peripherals.GPIO11)
.into_async();
let bus: &'static Mutex<_, _> = I2C_BUS.init(Mutex::new(i2c));

// Aggregator owns TELEMETRY; spawn it before the sensor tasks so the channel drains.
spawner.spawn(aggregator::run().expect("aggregator already spawned"));
let bmp_i2c: SharedI2c = I2cDevice::new(bus);
spawner.spawn(bmp::read_barometer(bmp_i2c, BMP388_ADDR).expect("read_barometer already spawned"));
let mlx_i2c: SharedI2c = I2cDevice::new(bus);
spawner.spawn(mlx::read_sky(mlx_i2c, MLX90614_ADDR).expect("read_sky already spawned"));
```

(`spawner.spawn(task().expect(...))` matches the existing pattern: the `Result` is from
the task token, `.expect` guards double-spawn.)

API note: `peripherals.GPIO11.reborrow()` returns `GPIO11<'_>` (generated by the
peripheral reborrow macro, esp-hal 1.1.1 `peripherals/mod.rs`), **not** `AnyPin` — no
`.degrade()` is needed. `GPIO11<'_>` implements `OutputPin` (so `Output::new(impl
OutputPin, Level, OutputConfig)` accepts it) and `PeripheralInput + PeripheralOutput` (so
`with_scl` later takes the pin by value). The reborrow's mutable borrow ends when
`_scl_low` drops (end of the block), so moving `peripherals.GPIO11` into `with_scl`
afterward is allowed (NLL). (`AnyPin::reborrow` at `gpio/mod.rs:2177` is a different method
on an already-degraded pin — not what is used here.)

**Done when:** `cargo build --release` (firmware target) succeeds; `just clippy` clean;
on-device with the MLX plugged, the BMP388 inits (no `I2c(Timeout)`) and there is no RWDT
reboot loop — i.e. the PWM-exit nudge cleared the bus.

### 11. BLE characteristic frame length 17 → `FRAME_LEN`

**File:** `crates/meteo-firmware/src/ble.rs`

Replace every hardcoded telemetry-value `17` / `[u8; 17]` with `meteo_lib::FRAME_LEN`
(now 18) so the GATT characteristic value matches the v2 frame:

- line 159: `let mut telemetry_storage = [0_u8; meteo_lib::FRAME_LEN];`
- line 173: `let telemetry_char: Characteristic<[u8; meteo_lib::FRAME_LEN]> = {`
- line 179: `[0_u8; meteo_lib::FRAME_LEN],`
- lines 217, 358: `telemetry_char: &Characteristic<[u8; meteo_lib::FRAME_LEN]>,`

(`FRAME_LEN` is a `const usize`, valid as a const-generic array length. `notify_loop`
already does `let frame = telem.encode();` which now returns `[u8; FRAME_LEN]` — types
line up.)

Update the comment on line 158 (`17-byte telemetry value` → `18-byte telemetry value`)
and the ATT sizing comment block if it mentions 17.

**Done when:** firmware builds; the characteristic value is 18 bytes (confirmed by
`ble_notify_check.sh` in substep 13).

### 12. TUI: surface the diagnostics bitfield

Render the **whole** diagnostics byte (not just occlusion) as one "Diagnostics" row that
lists every active flag and is highlighted when anything is wrong — this scales as bits 2–7
fill in.

**File:** `crates/meteo-tui/src/model.rs` — add a formatter + an "any flag set" predicate:

```rust
use meteo_lib::Diagnostics;

/// Format the diagnostics bitfield as a human-readable status line.
///
/// `"OK"` when no flags are set; otherwise a comma-joined list of active faults,
/// e.g. `"sky occluded, BMP388 fault"`. Scales as new flags are added.
#[must_use]
pub fn fmt_diagnostics(diag: Diagnostics) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if diag.occlusion() {
        flags.push("sky occluded");
    }
    if diag.baro_fault() {
        flags.push("BMP388 fault");
    }
    if flags.is_empty() {
        "OK".to_owned()
    } else {
        flags.join(", ")
    }
}

/// `true` if any diagnostics flag is set (drives red highlighting in the UI).
///
/// Tests the raw byte (`Diagnostics.0` is `pub`) so it covers every current and
/// future flag with no per-bit update — unlike `fmt_diagnostics`, which must name
/// each flag to label it.
#[must_use]
pub fn diagnostics_alert(diag: Diagnostics) -> bool {
    diag.0 != 0
}
```

Tests:

- `fmt_diagnostics_none_renders_ok`: `fmt_diagnostics(Diagnostics::empty()) == "OK"`.
- `fmt_diagnostics_occlusion_only`:
  `fmt_diagnostics(Diagnostics::empty().with_occlusion(true)) == "sky occluded"`.
- `fmt_diagnostics_baro_fault_only`:
  `fmt_diagnostics(Diagnostics::empty().with_baro_fault(true)) == "BMP388 fault"`.
- `fmt_diagnostics_multiple_flags_joined`: both flags set →
  `"sky occluded, BMP388 fault"` (order matches the push order).
- `diagnostics_alert_true_when_any_flag`: `false` for `empty()`; `true` for occlusion-only,
  for baro-fault-only, **and** for both flags set (the both-set case guards against an
  accidental `&&`/per-flag regression).

**File:** `crates/meteo-tui/src/ui.rs` — add a "Diagnostics" row (the 9th) and grow the
table area:

- In `render_table`, build the diagnostics row separately so it can carry its own style
  (the existing rows share `base`; the diagnostics row turns red when alerting):

  ```rust
  let diag_style = if model::diagnostics_alert(t.diagnostics) {
      base.fg(Color::Red)
  } else {
      base
  };
  // ...build the 8 value rows from rows_data with `base` as today, then chain:
  let diag_row = Row::new([
      "Diagnostics".to_owned(),
      model::fmt_diagnostics(t.diagnostics),
  ])
  .style(diag_style);
  ```

  Append `diag_row` after the eight mapped value rows (e.g. via
  `.chain(std::iter::once(diag_row))` on the row iterator, or collect into a `Vec<Row>`).
  Import `Color` (already imported in `ui.rs`).

- Update the doc comment "eight sensor rows" → "nine rows (eight values + diagnostics)".
- In `render`, change the table area constraint from `Constraint::Length(10)` to
  `Constraint::Length(11)` (9 rows + 2 border lines).

The `render_smoke_fills_buffer_without_panic` test builds `Telemetry { ..empty() }`
(diagnostics 0 → "OK"); add assertions `buffer_text.contains("Diagnostics")` and
`buffer_text.contains("OK")`. Add a second case asserting a faulting frame renders the
text: build `Telemetry { diagnostics: Diagnostics::empty().with_baro_fault(true),
..empty() }`, render, and assert `buffer_text.contains("BMP388 fault")`.

**Done when:** `cargo nextest run -p meteo-tui --target x86_64-unknown-linux-gnu` passes
and `just tui-clippy` is clean.

### 13. Acceptance scripts + CLAUDE.md

**File:** `scripts/ble_notify_check.sh`

- Line 57: `FRAME_LEN="${FRAME_LEN:-18}"` (was 17).
- Line 210 (python): `if len(data) == frame_len and data[0] == 0x02:` (was `0x01`).
- Update the header comments referencing "byte[0] == 0x01" and "17-byte" to `0x02` / 18.

**File:** `scripts/ble_soak.sh` — verify (no frame assertion expected; `rg -n '17|0x01'
scripts/ble_soak.sh` returned nothing). If a length/version check surfaces, bump it to
18 / `0x02`; otherwise leave unchanged and note "link-only, no frame assertion".

**File:** `CLAUDE.md` — update the BLE section's wire-frame description: the
characteristic value is now an **18-byte** frame, `byte[0]` is `0x02` (FRAME_VERSION 2),
byte 17 is the diagnostics bitfield (bit 0 = sky-IR occlusion, bit 1 = BMP388 fault). Update the
"Acceptance gate" paragraph's "byte[0] == 0x01 and a 17-byte length" to `0x02` / 18.
Add the MLX90614 to the "Currently supports" list and the pin-allocation table
(I2C0 `0x5A`, no new GPIO). Note the aggregator + shared-bus architecture briefly. Also
document the **MLX PWM→SMBus exit** (SCL held low ≥1.44 ms at boot before I2C init — why:
the part can power up driving PWM on the shared SDA line, jamming the bus; observed as
`BMP388 I2c(Timeout)` + RWDT reboot loop) and the **sensor-task resilience** (BMP retries
init + per-cycle heartbeat; watchdog beats are task-liveness, not data-presence). Update
the "PWM-vs-SMBus power-up risk" note from a contingency to an implemented mitigation.

**Done when:** `bash -n scripts/ble_notify_check.sh` parses; CLAUDE.md reflects v2.
(Running the script itself is the manual on-device acceptance gate, not a CI check.)

## Testing

**Host (CI-able), run via `just test` (`cargo nextest`):**

- `meteo-lib` frame: all existing tests pass after the v1→v2 edits, plus the new
  diagnostics encode/decode/roundtrip tests and the updated proptest (18-byte, version 2,
  arbitrary byte 17).
- `meteo-lib` mlx90614: `crc8` SMBus check vector (`0xF4`), `pec_for_read` address
  shifting, `temperature_from_raw` conversion + error-flag + zero-°C.
- `meteo-lib` aggregate: merge, occlusion set/clear/threshold/missing-ambient, sky-temp
  blanking on failed read.
- `meteo-tui` model: `fmt_diagnostics` (OK / occlusion / BMP388 fault / both joined) +
  `diagnostics_alert`; ui smoke test asserts "Diagnostics" + "OK", and a faulting-frame
  case asserts "BMP388 fault".

**Build/lint (must pass before any push, per project rules):**

```bash
just build        # firmware release (riscv32imac)
just clippy       # firmware + meteo-lib + meteo-tui, -D warnings
just test         # host unit tests
just format       # cargo fmt --check
just tui-build    # host TUI build
```

Also `cargo test --doc -p meteo-lib` for the frame/driver doc examples, and
`cargo deny check` / `cargo audit` for the two new deps.

**On-device acceptance (manual gate, not CI).** The aggregator → `TELEMETRY` → BLE-notify
integration path has **no automated test** (firmware tasks are not unit-tested in this
repo; only the pure lib logic is). It is covered solely by this gate, so verify the full
chain explicitly via the defmt log sequence:

- `just run`, and confirm the log sequence proving each link of the pipeline:
  1. `BMP388 initialized successfully!` and `Setting up MLX90614 sky-IR sensor`
     (both sensor tasks came up on the shared bus).
  2. `Temperature: …°C, Pressure: …` (BMP read) **and** `Sky/object temp: …°C` (MLX read)
     — both producers are sending on the channel.
  3. `rwdt fed (bmp=… agg=… adv=… ble=…)` with **`agg` advancing** each line (the
     aggregator is publishing 1 Hz → `AGG_BEAT` bumping → RWDT gate satisfied). A
     `rwdt withheld — … agg_alive=false …` line means the aggregator stalled.
  4. On central connect: `ConnectionParamsUpdated … supervision_ms=8000` still fires
     (the bus/aggregator refactor did not disturb the L2CAP param negotiation), and the
     BLE notify beat advances (notifications flowing from the merged frame).
- **PWM-exit + resilience gate (the regression this turn fixes):** with the **MLX plugged
  in**, the device must boot cleanly — BMP388 inits (no repeating `I2c(Timeout)`), and there
  is **no `rst:0x10 (LP_WDT_SYS)` reboot loop**. As a fault-injection check, force a BMP
  failure (e.g. wrong address) and confirm the device stays up and advertising (BMP data
  absent) instead of reboot-looping.
- `scripts/ble_notify_check.sh` (locally — the dev host _is_ gaia): ≥5 well-formed
  **18-byte** frames with `byte[0] == 0x02` in the window.
- `scripts/ble_soak.sh`: link still holds across the connect→hold→reconnect cycle (the
  bus/aggregator refactor must not regress link stability).
- `meteo-tui` shows "Sky temp" populated and a "Diagnostics" row = `OK` normally, or
  `BMP388 fault` / `sky occluded` (red) when those conditions hold — e.g. unplug the BMP
  and confirm the row shows `BMP388 fault` rather than the app just showing `N/A`.

## Risks

- **Shared-bus contention / wedge.** A hung MLX transaction could hold the bus mutex and
  starve the BMP task. _Mitigation:_ per-transaction locking (not per-driver-read), so
  whole transactions interleave and the async mutex is held only for one transaction; and
  esp-hal's I2C returns `Err(Timeout)` on a stuck bus (observed) rather than blocking
  forever, so a transaction always completes, releases the lock, the task cycles, and data
  degrades to `None` — no permanent starvation. Only a transaction that _never returns_
  (no timeout) would stall a heartbeat and trip the RWDT; that is the intended backstop.
  Note: with the substep-8 per-cycle heartbeat, a merely _erroring_ bus no longer trips the
  RWDT (that reboot loop was the bug being fixed) — the device stays up and degraded.
- **`embassy-embedded-hal` version skew.** Disproven for now (`0.6.0` → `embassy-sync
0.8.0` in the lock), but the workspace already carries multiple `embassy-sync` versions
  (0.6/0.7/0.8). _Mitigation:_ pin `"0.6"`; if `cargo update` ever pulls a build that
  re-binds `embassy-sync`, fall back to the hand-rolled `SharedI2c` wrapper (newtype over
  `embassy_sync::Mutex` implementing `I2c::transaction`) — kept as the documented plan B.
- **Frame v2 is a breaking wire change.** Any consumer left on 17-byte/`0x01` breaks.
  _Mitigation:_ all in-repo consumers (firmware GATT buffer, TUI via the lib, both
  scripts, CLAUDE.md) are updated in this plan; hard-reject keeps a stale v1 emitter from
  going unnoticed.
- **MSB error-flag vs. signed conversion.** Object temps use the full 16-bit word ×0.02
  −273.15; valid readings have bit 15 = 0 (so raw ≤ 0x7FFF). _Mitigation:_
  `temperature_from_raw` checks bit 15 first and returns `None`; tested.
- **MLX PWM-mode power-up — CONFIRMED on-device, now mitigated.** Plugging the MLX in
  jammed I2C0 (`BMP388 I2c(Timeout)` + RWDT reboot loop; BMP works fine with the MLX
  unplugged). _Mitigation (substep 10):_ hold SCL low ≥1.44 ms at boot to force SMBus mode.
  _Residual risk:_ if the 2 ms pulse proves insufficient on this breakout, lengthen
  `MLX_PWM_EXIT_SCL_LOW` (some clones want more), or do the permanent EEPROM config-`0x05`
  PWM-disable write + power cycle. The substep-8 resilience means that even if PWM-exit
  fails, the device no longer reboot-loops — it stays up and advertises, with BMP/sky data
  simply absent, which makes the failure diagnosable from logs instead of a boot cycle.
- **Watchdog false-reset during the refactor.** The new `agg_alive` gate could trip if
  the aggregator stalls. _Mitigation:_ the aggregator's only blocking points are
  `channel.receive()` / `ticker.next()` in a `select`; the 1 Hz tick always fires, so
  `AGG_BEAT` advances even with zero sensor traffic.
- **Table layout overflow in the TUI** on a short terminal after adding the 9th row.
  _Mitigation:_ `Constraint::Length(11)` with `Constraint::Min(0)` for charts; the
  existing small-terminal smoke test (`render_smoke_small_terminal_no_panic`) guards
  against panics.

## Notes

Progress tracking (checked off during `/tyrex:code:implement-light`):

- [x] 1. Frame v2 + `Diagnostics` (meteo-lib/ble/frame.rs)
- [x] 2. MLX90614 driver (meteo-lib/sensors/mlx90614.rs)
- [x] 3. Aggregator + `SensorReading` (meteo-lib/aggregate.rs)
- [x] 4. lib.rs re-exports
- [x] 5. Dependencies (workspace + firmware Cargo.toml) — embassy-embedded-hal 0.6.0, static-cell 2.1.1
- [x] 6. `SharedI2c` bus module (firmware/bus.rs)
- [x] 7. Aggregator task + channel + `AGG_BEAT`; task-liveness beat semantics (firmware/aggregator.rs, watchdog.rs)
- [x] 8. BMP388 task rework + resilience: retry init, per-cycle heartbeat (firmware/bmp.rs)
- [x] 9. MLX90614 task (firmware/mlx.rs)
- [x] 10. main.rs wiring **+ MLX PWM-exit SCL-low nudge before I2C init**
- [x] 11. BLE characteristic length → `FRAME_LEN` (firmware/ble.rs) — landed early (after substep 5) so the firmware-code substeps could compile
- [x] 12. TUI diagnostics row — occlusion + BMP388 fault, red when alerting (meteo-tui model.rs, ui.rs)
- [x] 13. Acceptance scripts + CLAUDE.md

Implementation order: 1→5, then 11 (early, to unblock firmware compilation), 6, 7, 9, 8, 10, 12, 13.
On-device acceptance (PWM-exit, RWDT gates, soak/notify scripts, TUI sky temp + diagnostics row)
remains a manual gate — all host-side checks (build, clippy, fmt, 43+38 tests, doc test, audit) pass.
