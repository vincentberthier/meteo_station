// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive"
)]

//! Telemetry wire frame v2 — fixed-length, little-endian, 18 bytes.
//!
//! All multi-byte fields are encoded **little-endian**; the BLE central must
//! decode them accordingly.
//!
//! # Frame layout
//!
//! | Off   | Field               | Wire type | Encoding                                                                    | Sentinel (None)    |
//! |-------|---------------------|-----------|-----------------------------------------------------------------------------|--------------------|
//! | 0     | version             | u8        | [`FRAME_VERSION`] (= 2)                                                     | —                  |
//! | 1–2   | temperature         | i16 LE    | round(°C × 100) centi-°C                                                    | `i16::MIN`         |
//! | 3–4   | pressure            | u16 LE    | round(hPa × 10) deci-hPa                                                   | `u16::MAX`         |
//! | 5–6   | humidity            | u16 LE    | round(%RH × 100) centi-%RH                                                  | `u16::MAX`         |
//! | 7–8   | sky/IR temp         | i16 LE    | centi-°C                                                                    | `i16::MIN`         |
//! | 9–10  | luminosity mantissa | u16 LE    | mantissa × 10^exp ≈ lux                                                     | `u16::MAX`         |
//! | 11    | luminosity exponent | u8        | see lux encoding                                                            | (mantissa=MAX)     |
//! | 12–13 | wind speed          | u16 LE    | round(m/s × 100) cm/s                                                       | `u16::MAX`         |
//! | 14–15 | wind direction      | u16 LE    | round(deg × 10) deci-deg                                                    | `u16::MAX`         |
//! | 16    | battery             | u8        | percent 0..=100                                                             | `0xFF`             |
//! | 17    | diagnostics         | u8        | bitfield: bit0 = sky-IR occlusion, bit1 = BMP388 fault, bit2 = BME280 fault, bit3 = VEML7700 fault, bit4 = baro divergence, bit5 = MLX90614 fault, bits 6–7 reserved | — (always present) |

/// Wire-format version tag written to byte 0 of every frame.
pub const FRAME_VERSION: u8 = 2;

/// Total length (in bytes) of a v2 telemetry frame.
pub const FRAME_LEN: usize = 18;

/// Per-frame health/diagnostics bitfield (frame v2, byte 17).
///
/// Bit 0 = sky-IR sensor ambient diverges from the barometer air temperature
/// beyond the configured threshold (possible occlusion / icing).
/// Bit 1 = BMP388 fault (not initialized / read failing).
/// Bit 2 = BME280 fault (not initialized / read failing → no humidity or cross-check).
/// Bit 3 = VEML7700 fault (not initialized / read failing → no luminosity).
/// Bit 4 = BMP388 vs BME280 temperature/pressure disagree beyond threshold.
/// Bit 5 = MLX90614 object read failed → no `sky_temp_c`.
/// Bits 6–7 are reserved and always 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Diagnostics(pub u8);

impl Diagnostics {
    /// Bit 0: sky-IR ambient diverges from barometer air temp (occlusion/icing).
    pub const SKY_IR_OCCLUSION: u8 = 1 << 0;
    /// Bit 1: BMP388 not providing data — failed to initialize, or a read error
    /// forced a re-init. While set, `temperature_c`/`pressure_hpa` are `None`.
    pub const BARO_FAULT: u8 = 1 << 1;
    /// Bit 2: BME280 init/read failing → no humidity or cross-check data.
    pub const BME280_FAULT: u8 = 1 << 2;
    /// Bit 3: VEML7700 init/read failing → no luminosity data.
    pub const VEML7700_FAULT: u8 = 1 << 3;
    /// Bit 4: BMP388 vs BME280 temperature/pressure disagree beyond threshold.
    pub const BARO_DIVERGENCE: u8 = 1 << 4;
    /// Bit 5: MLX90614 object read failed → no `sky_temp_c`.
    pub const MLX90614_FAULT: u8 = 1 << 5;
    // Bits 6–7 reserved (0).

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

    /// Returns `true` if the BME280 fault bit is set.
    #[must_use]
    pub const fn bme280_fault(self) -> bool {
        self.0 & Self::BME280_FAULT != 0
    }

    /// Returns a copy with the BME280 fault bit set to `set`.
    #[must_use]
    pub const fn with_bme280_fault(self, set: bool) -> Self {
        self.with_flag(Self::BME280_FAULT, set)
    }

    /// Returns `true` if the VEML7700 fault bit is set.
    #[must_use]
    pub const fn veml7700_fault(self) -> bool {
        self.0 & Self::VEML7700_FAULT != 0
    }

    /// Returns a copy with the VEML7700 fault bit set to `set`.
    #[must_use]
    pub const fn with_veml7700_fault(self, set: bool) -> Self {
        self.with_flag(Self::VEML7700_FAULT, set)
    }

    /// Returns `true` if the baro divergence bit is set.
    #[must_use]
    pub const fn baro_divergence(self) -> bool {
        self.0 & Self::BARO_DIVERGENCE != 0
    }

    /// Returns a copy with the baro divergence bit set to `set`.
    #[must_use]
    pub const fn with_baro_divergence(self, set: bool) -> Self {
        self.with_flag(Self::BARO_DIVERGENCE, set)
    }

    /// Returns `true` if the MLX90614 fault bit is set.
    #[must_use]
    pub const fn mlx90614_fault(self) -> bool {
        self.0 & Self::MLX90614_FAULT != 0
    }

    /// Returns a copy with the MLX90614 fault bit set to `set`.
    #[must_use]
    pub const fn with_mlx90614_fault(self, set: bool) -> Self {
        self.with_flag(Self::MLX90614_FAULT, set)
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

/// All sensor readings bundled for one telemetry update.
///
/// Every sensor field is `Option<_>`; `None` encodes to the field's sentinel value and
/// decodes back to `None`. The `diagnostics` field is always present.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Telemetry {
    /// Ambient temperature in degrees Celsius.
    pub temperature_c: Option<f32>,
    /// Barometric pressure in hPa.
    pub pressure_hpa: Option<f32>,
    /// Relative humidity in percent (0–100).
    pub humidity_pct: Option<f32>,
    /// Sky (IR) temperature in degrees Celsius.
    pub sky_temp_c: Option<f32>,
    /// Illuminance in lux.
    pub luminosity_lux: Option<f32>,
    /// Wind speed in metres per second.
    pub wind_speed_ms: Option<f32>,
    /// Wind direction in degrees (0–360).
    pub wind_dir_deg: Option<f32>,
    /// Battery charge level in percent (0–100).
    pub battery_pct: Option<u8>,
    /// Per-frame diagnostics bitfield (frame v2).
    pub diagnostics: Diagnostics,
}

/// Errors returned by [`Telemetry::decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameError {
    /// The byte slice was not exactly [`FRAME_LEN`] bytes long.
    WrongLength(usize),
    /// Byte 0 was not [`FRAME_VERSION`].
    UnknownVersion(u8),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongLength(n) => write!(f, "wrong frame length: expected {FRAME_LEN}, got {n}"),
            Self::UnknownVersion(v) => write!(f, "unknown frame version: {v}"),
        }
    }
}

impl core::error::Error for FrameError {}

impl Telemetry {
    /// Returns a [`Telemetry`] with all sensor fields set to `None` and diagnostics cleared.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            temperature_c: None,
            pressure_hpa: None,
            humidity_pct: None,
            sky_temp_c: None,
            luminosity_lux: None,
            wind_speed_ms: None,
            wind_dir_deg: None,
            battery_pct: None,
            diagnostics: Diagnostics::empty(),
        }
    }

    /// Constructs a [`Telemetry`] from a BMP388 reading.
    ///
    /// Only `temperature_c` and `pressure_hpa` are populated; all other fields
    /// are `None`. The BMP388 `Reading::pressure` field is in Pascals and is
    /// divided by 100 to convert to hPa.
    #[must_use]
    pub fn from_bmp388(reading: &crate::sensors::bmp388::Reading) -> Self {
        Self {
            temperature_c: Some(reading.temperature),
            pressure_hpa: Some(reading.pressure_hpa()),
            ..Self::empty()
        }
    }

    /// Serialises this reading to the fixed 18-byte v2 wire frame.
    ///
    /// All multi-byte fields are little-endian. `None` fields encode to their
    /// respective sentinels (see module-level table).
    #[must_use]
    pub fn encode(&self) -> [u8; FRAME_LEN] {
        let mut frame = [0_u8; FRAME_LEN];
        frame[0] = FRAME_VERSION;

        let temp = self.temperature_c.map_or(i16::MIN, |v| scale_i16(v, 100.0));
        frame[1..3].copy_from_slice(&temp.to_le_bytes());

        let press = self.pressure_hpa.map_or(u16::MAX, |v| scale_u16(v, 10.0));
        frame[3..5].copy_from_slice(&press.to_le_bytes());

        let hum = self.humidity_pct.map_or(u16::MAX, |v| scale_u16(v, 100.0));
        frame[5..7].copy_from_slice(&hum.to_le_bytes());

        let sky = self.sky_temp_c.map_or(i16::MIN, |v| scale_i16(v, 100.0));
        frame[7..9].copy_from_slice(&sky.to_le_bytes());

        let (lux_mant, lux_exp) = self.luminosity_lux.map_or((u16::MAX, 0_u8), encode_lux);
        frame[9..11].copy_from_slice(&lux_mant.to_le_bytes());
        frame[11] = lux_exp;

        let wind = self.wind_speed_ms.map_or(u16::MAX, |v| scale_u16(v, 100.0));
        frame[12..14].copy_from_slice(&wind.to_le_bytes());

        let dir = self.wind_dir_deg.map_or(u16::MAX, |v| scale_u16(v, 10.0));
        frame[14..16].copy_from_slice(&dir.to_le_bytes());

        frame[16] = self.battery_pct.unwrap_or(0xFF);
        frame[17] = self.diagnostics.0;

        frame
    }

    /// Parses a v2 wire frame, mapping sentinels back to `None`.
    ///
    /// # Errors
    ///
    /// - [`FrameError::WrongLength`] if `bytes` is not exactly [`FRAME_LEN`] bytes.
    /// - [`FrameError::UnknownVersion`] if byte 0 is not [`FRAME_VERSION`].
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() != FRAME_LEN {
            return Err(FrameError::WrongLength(bytes.len()));
        }

        if bytes[0] != FRAME_VERSION {
            return Err(FrameError::UnknownVersion(bytes[0]));
        }

        let temperature_c = {
            let raw = i16::from_le_bytes([bytes[1], bytes[2]]);
            if raw == i16::MIN {
                None
            } else {
                Some(f32::from(raw) / 100.0)
            }
        };

        let pressure_hpa = {
            let raw = u16::from_le_bytes([bytes[3], bytes[4]]);
            if raw == u16::MAX {
                None
            } else {
                Some(f32::from(raw) / 10.0)
            }
        };

        let humidity_pct = {
            let raw = u16::from_le_bytes([bytes[5], bytes[6]]);
            if raw == u16::MAX {
                None
            } else {
                Some(f32::from(raw) / 100.0)
            }
        };

        let sky_temp_c = {
            let raw = i16::from_le_bytes([bytes[7], bytes[8]]);
            if raw == i16::MIN {
                None
            } else {
                Some(f32::from(raw) / 100.0)
            }
        };

        let luminosity_lux = {
            let mant = u16::from_le_bytes([bytes[9], bytes[10]]);
            if mant == u16::MAX {
                None
            } else {
                let exp = bytes[11];
                Some(f32::from(mant) * libm::powf(10.0, f32::from(exp)))
            }
        };

        let wind_speed_ms = {
            let raw = u16::from_le_bytes([bytes[12], bytes[13]]);
            if raw == u16::MAX {
                None
            } else {
                Some(f32::from(raw) / 100.0)
            }
        };

        let wind_dir_deg = {
            let raw = u16::from_le_bytes([bytes[14], bytes[15]]);
            if raw == u16::MAX {
                None
            } else {
                Some(f32::from(raw) / 10.0)
            }
        };

        let battery_pct = if bytes[16] == 0xFF {
            None
        } else {
            Some(bytes[16])
        };

        let diagnostics = Diagnostics(bytes[17]);

        Ok(Self {
            temperature_c,
            pressure_hpa,
            humidity_pct,
            sky_temp_c,
            luminosity_lux,
            wind_speed_ms,
            wind_dir_deg,
            battery_pct,
            diagnostics,
        })
    }
}

/// Encodes `lux` as `(mantissa, exponent)` such that `mantissa × 10^exponent ≈ lux`
/// and `mantissa ≤ 65534`.
///
/// Picks the smallest non-negative exponent that keeps `round(lux / 10^exp) ≤ 65534`.
fn encode_lux(lux: f32) -> (u16, u8) {
    // Use an integer sentinel (65_535 == u16::MAX) to avoid float comparison.
    const MANT_LIMIT: u32 = 0xFFFE; // = 65534; u16::MAX - 1, the highest non-sentinel mantissa

    let mut exp = 0_u8;
    let mut mantissa_f = libm::roundf(lux.max(0.0));
    // Cast is safe: roundf returns a whole number; values > u32::MAX are handled
    // by the loop (lux ≤ 3.4e38 but the loop exits well before overflow).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mantissa_f ≥ 0 after max(0.0); starts at lux ≤ f32::MAX but loop terminates quickly"
    )]
    let mut mant_u32 = mantissa_f as u32;

    while mant_u32 > MANT_LIMIT {
        exp = exp.saturating_add(1);
        mantissa_f = libm::roundf(lux / libm::powf(10.0, f32::from(exp)));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mantissa_f ≥ 0 (lux ≥ 0 and powf > 0); loop invariant keeps value ≤ 65534"
        )]
        {
            mant_u32 = mantissa_f as u32;
        }
    }

    // Safety: mant_u32 ≤ 65534 ≤ u16::MAX by loop invariant.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mant_u32 ≤ 65534 — guaranteed by loop invariant"
    )]
    (mant_u32 as u16, exp)
}

/// Scales a float by `factor`, rounds to nearest, and clamps to `[i16::MIN+1, i16::MAX]`.
///
/// The lower bound is kept one above `i16::MIN` so that `i16::MIN` remains exclusively
/// the sentinel for `None`.
fn scale_i16(v: f32, factor: f32) -> i16 {
    let rounded = libm::roundf(v * factor);
    // Clamp away from the sentinel i16::MIN.
    let clamped = rounded
        .max(f32::from(i16::MIN) + 1.0)
        .min(f32::from(i16::MAX));
    // Safety: value is clamped to [i16::MIN+1, i16::MAX] before the cast.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "value is clamped to i16 range before cast"
    )]
    (clamped as i16)
}

/// Scales a float by `factor`, rounds to nearest, and clamps to `[0, u16::MAX-1]`.
///
/// The upper bound is kept one below `u16::MAX` so that `u16::MAX` remains exclusively
/// the sentinel for `None`.
fn scale_u16(v: f32, factor: f32) -> u16 {
    let rounded = libm::roundf(v * factor);
    // Clamp away from the sentinel u16::MAX.
    let clamped = rounded.max(0.0).min(f32::from(u16::MAX) - 1.0);
    // Safety: value is clamped to [0, u16::MAX-1] before the cast.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is clamped to u16 range before cast"
    )]
    (clamped as u16)
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::boxed::Box;
    use core::{error::Error, result};

    use proptest::prelude::*;
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn Error>>;

    // -------------------------------------------------------------------------
    // from_bmp388
    // -------------------------------------------------------------------------

    #[test]
    fn from_bmp388_sets_temperature_and_pressure_only() {
        // Given
        let reading = crate::sensors::bmp388::Reading {
            temperature: 23.5,
            pressure: 101_325.0,
        };

        // When
        let telem = Telemetry::from_bmp388(&reading);

        // Then
        assert!(telem.temperature_c.is_some());
        assert!(telem.pressure_hpa.is_some());
        assert!(telem.humidity_pct.is_none());
        assert!(telem.sky_temp_c.is_none());
        assert!(telem.luminosity_lux.is_none());
        assert!(telem.wind_speed_ms.is_none());
        assert!(telem.wind_dir_deg.is_none());
        assert!(telem.battery_pct.is_none());
    }

    // -------------------------------------------------------------------------
    // encode
    // -------------------------------------------------------------------------

    #[test]
    fn encode_emits_eighteen_bytes_with_version_two() {
        // Given
        let telem = Telemetry::empty();

        // When
        let frame = telem.encode();

        // Then
        assert_eq!(frame.len(), 18);
        assert_eq!(frame[0], 2);
    }

    #[test]
    fn encode_writes_sentinels_for_none_fields() {
        // Given
        let telem = Telemetry::empty();

        // When
        let frame = telem.encode();

        // Then
        // temperature sentinel: i16::MIN as LE
        assert_eq!(&frame[1..3], &i16::MIN.to_le_bytes());
        // pressure sentinel: u16::MAX as LE
        assert_eq!(&frame[3..5], &u16::MAX.to_le_bytes());
        // humidity sentinel: u16::MAX as LE
        assert_eq!(&frame[5..7], &u16::MAX.to_le_bytes());
        // sky temp sentinel: i16::MIN as LE
        assert_eq!(&frame[7..9], &i16::MIN.to_le_bytes());
        // luminosity mantissa sentinel: u16::MAX as LE
        assert_eq!(&frame[9..11], &u16::MAX.to_le_bytes());
        // wind speed sentinel: u16::MAX as LE
        assert_eq!(&frame[12..14], &u16::MAX.to_le_bytes());
        // wind dir sentinel: u16::MAX as LE
        assert_eq!(&frame[14..16], &u16::MAX.to_le_bytes());
        // battery sentinel: 0xFF
        assert_eq!(frame[16], 0xFF);
    }

    #[test]
    fn encode_scales_temperature_and_pressure() {
        // Given
        // temperature = 23.45 °C → 2345 as i16 LE
        // pressure = 1013.2 hPa → 10132 as u16 LE
        let telem = Telemetry {
            temperature_c: Some(23.45),
            pressure_hpa: Some(1013.2),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();

        // Then
        let expected_temp: i16 = 2345;
        let expected_press: u16 = 10132;
        assert_eq!(&frame[1..3], &expected_temp.to_le_bytes());
        assert_eq!(&frame[3..5], &expected_press.to_le_bytes());
    }

    #[test]
    fn encode_writes_diagnostics_byte() {
        // Given
        let telem = Telemetry {
            diagnostics: Diagnostics(0b0000_0001),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();

        // Then
        assert_eq!(frame[17], 0x01);
    }

    // -------------------------------------------------------------------------
    // decode
    // -------------------------------------------------------------------------

    #[test]
    fn decode_rejects_wrong_length() {
        // Given — 17 bytes is no longer valid (v2 requires 18)
        let short17 = [0_u8; 17];

        // When
        let result = Telemetry::decode(&short17);

        // Then
        assert_eq!(result, Err(FrameError::WrongLength(17)));

        // Also check 16 bytes
        let short16 = [0_u8; 16];
        assert_eq!(
            Telemetry::decode(&short16),
            Err(FrameError::WrongLength(16))
        );
    }

    #[test]
    fn decode_rejects_unknown_version() {
        // Given — 18 bytes with version byte = 3 (2 is now valid; 3 is unknown)
        let mut frame = [0_u8; 18];
        frame[0] = 3;

        // When
        let result = Telemetry::decode(&frame);

        // Then
        assert_eq!(result, Err(FrameError::UnknownVersion(3)));
    }

    #[test]
    fn decode_maps_sentinels_back_to_none() -> TestResult {
        // Given — a frame with all sentinel values
        let telem = Telemetry::empty();
        let frame = telem.encode();

        // When
        let decoded = Telemetry::decode(&frame)?;

        // Then
        assert_eq!(decoded.temperature_c, None);
        assert_eq!(decoded.pressure_hpa, None);
        assert_eq!(decoded.humidity_pct, None);
        assert_eq!(decoded.sky_temp_c, None);
        assert_eq!(decoded.luminosity_lux, None);
        assert_eq!(decoded.wind_speed_ms, None);
        assert_eq!(decoded.wind_dir_deg, None);
        assert_eq!(decoded.battery_pct, None);
        assert_eq!(decoded.diagnostics, Diagnostics::empty());

        Ok(())
    }

    #[test]
    #[expect(
        clippy::unwrap_used,
        reason = "test: values known to be Some after encode/decode"
    )]
    fn decode_recovers_scaled_values() -> TestResult {
        // Given
        let telem = Telemetry {
            temperature_c: Some(23.45),
            pressure_hpa: Some(1013.2),
            humidity_pct: Some(55.0),
            wind_speed_ms: Some(3.5),
            wind_dir_deg: Some(270.0),
            battery_pct: Some(80),
            ..Telemetry::empty()
        };
        let frame = telem.encode();

        // When
        let decoded = Telemetry::decode(&frame)?;

        // Then — values must be within 1 unit of the LSB
        assert!((decoded.temperature_c.unwrap() - 23.45).abs() < 0.01);
        assert!((decoded.pressure_hpa.unwrap() - 1013.2).abs() < 0.1);
        assert!((decoded.humidity_pct.unwrap() - 55.0).abs() < 0.01);
        assert!((decoded.wind_speed_ms.unwrap() - 3.5).abs() < 0.01);
        assert!((decoded.wind_dir_deg.unwrap() - 270.0).abs() < 0.1);
        assert_eq!(decoded.battery_pct, Some(80));

        Ok(())
    }

    #[test]
    fn decode_reads_diagnostics_byte_roundtrip() -> TestResult {
        // Given
        let telem = Telemetry {
            diagnostics: Diagnostics::empty().with_occlusion(true),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();
        let decoded = Telemetry::decode(&frame)?;

        // Then
        assert!(decoded.diagnostics.occlusion());

        Ok(())
    }

    // -------------------------------------------------------------------------
    // lux encoding
    // -------------------------------------------------------------------------

    #[test]
    fn encode_lux_large_value_uses_nonzero_exponent() {
        // Given
        let telem = Telemetry {
            luminosity_lux: Some(120_000.0),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();
        let mant = u16::from_le_bytes([frame[9], frame[10]]);
        let exp = frame[11];

        // Then
        assert!(
            exp >= 1,
            "exponent should be >= 1 for 120000 lux, got {exp}"
        );
        let recovered = f32::from(mant) * libm::powf(10.0, f32::from(exp));
        let tolerance = 120_000.0 * 0.005; // 0.5%
        assert!(
            (recovered - 120_000.0).abs() <= tolerance,
            "recovered {recovered} not within tolerance of 120000"
        );
    }

    #[test]
    fn encode_lux_zero_emits_zero_mantissa_zero_exponent() -> TestResult {
        // Given
        let telem = Telemetry {
            luminosity_lux: Some(0.0),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();
        let mant = u16::from_le_bytes([frame[9], frame[10]]);
        let exp = frame[11];

        // Then
        assert_eq!(mant, 0, "mantissa should be 0 for lux=0.0");
        assert_eq!(exp, 0, "exponent should be 0 for lux=0.0");

        // Decode recovers Some(0.0)
        let decoded = Telemetry::decode(&frame)?;
        assert_eq!(
            decoded.luminosity_lux,
            Some(0.0),
            "decoded lux should be Some(0.0)"
        );

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Diagnostics type tests
    // -------------------------------------------------------------------------

    #[test]
    fn diagnostics_with_occlusion_sets_and_clears_bit0() {
        // Given / When / Then — setting occlusion
        assert!(Diagnostics::empty().with_occlusion(true).occlusion());

        // Clearing occlusion
        assert!(!Diagnostics::empty().with_occlusion(false).occlusion());

        // Round-trip: set then clear
        let diag = Diagnostics::empty()
            .with_occlusion(true)
            .with_occlusion(false);
        assert!(!diag.occlusion());
        assert_eq!(diag.0, 0);
    }

    #[test]
    fn diagnostics_baro_fault_is_independent_of_occlusion() {
        // Given — baro fault set, occlusion clear
        let diag = Diagnostics::empty().with_baro_fault(true);

        // Then
        assert!(diag.baro_fault());
        assert!(!diag.occlusion());
        assert_eq!(diag.0, 0b0000_0010);

        // Given — both set
        let both = diag.with_occlusion(true);
        assert!(both.baro_fault());
        assert!(both.occlusion());
        assert_eq!(both.0, 0b0000_0011);
    }

    #[test]
    fn diagnostics_new_bits_set_and_clear_independently() {
        // Given / When / Then — BME280_FAULT (bit 2)
        let bme280 = Diagnostics::empty().with_bme280_fault(true);
        assert!(bme280.bme280_fault());
        assert_eq!(bme280.0, 0b0000_0100);
        assert!(!bme280.with_bme280_fault(false).bme280_fault());
        assert_eq!(bme280.with_bme280_fault(false).0, 0b0000_0000);

        // VEML7700_FAULT (bit 3)
        let veml = Diagnostics::empty().with_veml7700_fault(true);
        assert!(veml.veml7700_fault());
        assert_eq!(veml.0, 0b0000_1000);
        assert!(!veml.with_veml7700_fault(false).veml7700_fault());
        assert_eq!(veml.with_veml7700_fault(false).0, 0b0000_0000);

        // BARO_DIVERGENCE (bit 4)
        let baro_div = Diagnostics::empty().with_baro_divergence(true);
        assert!(baro_div.baro_divergence());
        assert_eq!(baro_div.0, 0b0001_0000);
        assert!(!baro_div.with_baro_divergence(false).baro_divergence());
        assert_eq!(baro_div.with_baro_divergence(false).0, 0b0000_0000);

        // MLX90614_FAULT (bit 5)
        let mlx = Diagnostics::empty().with_mlx90614_fault(true);
        assert!(mlx.mlx90614_fault());
        assert_eq!(mlx.0, 0b0010_0000);
        assert!(!mlx.with_mlx90614_fault(false).mlx90614_fault());
        assert_eq!(mlx.with_mlx90614_fault(false).0, 0b0000_0000);
    }

    #[test]
    fn diagnostics_all_six_flags_compose() {
        // Given — set all six flags
        let diag = Diagnostics::empty()
            .with_occlusion(true)
            .with_baro_fault(true)
            .with_bme280_fault(true)
            .with_veml7700_fault(true)
            .with_baro_divergence(true)
            .with_mlx90614_fault(true);

        // Then — all six accessors return true
        assert!(diag.occlusion());
        assert!(diag.baro_fault());
        assert!(diag.bme280_fault());
        assert!(diag.veml7700_fault());
        assert!(diag.baro_divergence());
        assert!(diag.mlx90614_fault());
        assert_eq!(diag.0, 0b0011_1111);
    }

    #[test]
    fn decode_preserves_new_diagnostic_bits() -> TestResult {
        // Given — Telemetry with bits 2–5 all set (0b0011_1100)
        let telem = Telemetry {
            diagnostics: Diagnostics(0b0011_1100),
            ..Telemetry::empty()
        };

        // When
        let frame = telem.encode();
        let decoded = Telemetry::decode(&frame)?;

        // Then — each new accessor round-trips correctly
        assert!(
            decoded.diagnostics.bme280_fault(),
            "bit 2 (BME280_FAULT) must survive encode/decode"
        );
        assert!(
            decoded.diagnostics.veml7700_fault(),
            "bit 3 (VEML7700_FAULT) must survive encode/decode"
        );
        assert!(
            decoded.diagnostics.baro_divergence(),
            "bit 4 (BARO_DIVERGENCE) must survive encode/decode"
        );
        assert!(
            decoded.diagnostics.mlx90614_fault(),
            "bit 5 (MLX90614_FAULT) must survive encode/decode"
        );
        // Bits 0 and 1 must be clear
        assert!(!decoded.diagnostics.occlusion(), "bit 0 must be clear");
        assert!(!decoded.diagnostics.baro_fault(), "bit 1 must be clear");

        Ok(())
    }

    // -------------------------------------------------------------------------
    // proptest round-trips
    // -------------------------------------------------------------------------

    proptest! {
        #[test]
        #[expect(clippy::expect_used, reason = "proptest: inputs are constructed to always succeed")]
        fn roundtrip_decode_encode_is_identity_at_wire_level(
            // Generate random bytes for a valid v2 frame; force lux to sentinel
            // (mantissa = u16::MAX, exponent = 0) so lux is None on both sides.
            // When lux is None, encode always writes exponent=0, so we must use
            // exponent=0 here to get a bit-exact roundtrip.
            b1 in any::<u8>(),
            b2 in any::<u8>(),
            b3 in any::<u8>(),
            b4 in any::<u8>(),
            b5 in any::<u8>(),
            b6 in any::<u8>(),
            b7 in any::<u8>(),
            b8 in any::<u8>(),
            b12 in any::<u8>(),
            b13 in any::<u8>(),
            b14 in any::<u8>(),
            b15 in any::<u8>(),
            b16 in any::<u8>(),
            b17 in any::<u8>(),
        ) {
            let mut bytes = [0_u8; FRAME_LEN];
            bytes[0] = FRAME_VERSION;
            bytes[1] = b1;
            bytes[2] = b2;
            bytes[3] = b3;
            bytes[4] = b4;
            bytes[5] = b5;
            bytes[6] = b6;
            bytes[7] = b7;
            bytes[8] = b8;
            // Force lux mantissa = u16::MAX (sentinel → None); exponent must be 0
            // because encode writes 0 for the exponent when lux is None.
            bytes[9] = 0xFF;
            bytes[10] = 0xFF;
            bytes[11] = 0x00;
            bytes[12] = b12;
            bytes[13] = b13;
            bytes[14] = b14;
            bytes[15] = b15;
            bytes[16] = b16;
            bytes[17] = b17;

            let decoded = Telemetry::decode(&bytes).expect("valid v2 frame must decode");
            let re_encoded = decoded.encode();
            prop_assert_eq!(bytes, re_encoded);
        }

        #[test]
        #[expect(clippy::expect_used, reason = "proptest: encode always produces a valid v2 frame")]
        fn lux_roundtrip_preserves_value_within_tolerance(
            lux in 0.0_f32..=120_000.0_f32,
        ) {
            let telem = Telemetry {
                luminosity_lux: Some(lux),
                ..Telemetry::empty()
            };
            let frame = telem.encode();
            let decoded = Telemetry::decode(&frame).expect("encode always produces a valid frame");
            let recovered = decoded.luminosity_lux.expect("lux should be Some after roundtrip");

            // Allow 0.5% tolerance OR an absolute tolerance of 1.0 for very small values.
            let tolerance = (lux * 0.005_f32).max(1.0);
            prop_assert!(
                (recovered - lux).abs() <= tolerance,
                "lux={lux}, recovered={recovered}, tolerance={tolerance}"
            );
        }
    }
}
// grcov exclude stop
