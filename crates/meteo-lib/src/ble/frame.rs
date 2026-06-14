//! Wire-frame codec for `MeteoStation` BLE measurements.
//!
//! A [`Frame`] is 17 bytes in little-endian layout (schema v1).  Absent
//! measurements are encoded as type-specific sentinel values so both the
//! firmware (encode path) and the central application (decode path) share
//! identical conversion factors.

// Suppress false positives from defmt macro expansion (only active when defmt feature is on).
#![cfg_attr(
    feature = "defmt",
    expect(
        clippy::missing_asserts_for_indexing,
        reason = "false positives from defmt macro expansion"
    )
)]

use core::fmt;

use super::SCHEMA_VERSION;

// ── Frame length ─────────────────────────────────────────────────────────────

/// Number of bytes in one encoded [`Frame`].
pub const FRAME_LEN: usize = 17;

// ── Sentinels ─────────────────────────────────────────────────────────────────

const SENTINEL_I16: i16 = i16::MIN;
const SENTINEL_U16: u16 = u16::MAX;
const SENTINEL_U8: u8 = u8::MAX;

// ── Frame ─────────────────────────────────────────────────────────────────────

/// All sensor measurements that may be transmitted in a single BLE notification.
///
/// Every field is `Option<_>`; `None` encodes to the field's sentinel value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Frame {
    /// Ambient temperature in degrees Celsius.
    pub temperature_c: Option<f32>,
    /// Atmospheric pressure in Pascals.
    pub pressure_pa: Option<f32>,
    /// Relative humidity in percent.
    pub humidity_pct: Option<f32>,
    /// Sky / IR temperature in degrees Celsius.
    pub sky_temp_c: Option<f32>,
    /// Illuminance in lux.
    pub luminosity_lux: Option<f32>,
    /// Wind speed in metres per second.
    pub wind_speed_ms: Option<f32>,
    /// Wind direction in degrees (0–360).
    pub wind_dir_deg: Option<f32>,
    /// Battery charge in percent (0–100).
    pub battery_pct: Option<u8>,
}

// ── Field tag ─────────────────────────────────────────────────────────────────

/// Tags identifying each field in wire order, used by [`Frame::present_fields`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameField {
    /// Ambient temperature (°C).
    Temperature,
    /// Atmospheric pressure (Pa).
    Pressure,
    /// Relative humidity (%RH).
    Humidity,
    /// Sky / IR temperature (°C).
    SkyTemp,
    /// Illuminance (lux).
    Luminosity,
    /// Wind speed (m/s).
    WindSpeed,
    /// Wind direction (°).
    WindDir,
    /// Battery charge (%).
    Battery,
}

// ── Decode error ──────────────────────────────────────────────────────────────

/// Errors that can occur when decoding a byte slice into a [`Frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer was shorter than [`FRAME_LEN`].
    TooShort {
        /// Actual length of the provided slice.
        got: usize,
    },
    /// The header byte does not match any known schema version.
    UnknownVersion(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { got } => {
                write!(f, "frame too short: need {FRAME_LEN} bytes, got {got}")
            }
            Self::UnknownVersion(v) => write!(f, "unknown frame schema version: {v}"),
        }
    }
}

#[expect(
    clippy::absolute_paths,
    reason = "core::error::Error path avoids a use import that could shadow other Error types"
)]
impl core::error::Error for DecodeError {}

// ── LE byte helpers ───────────────────────────────────────────────────────────

#[expect(
    clippy::little_endian_bytes,
    reason = "BLE wire format is little-endian"
)]
const fn put_i16(buf: &mut [u8; FRAME_LEN], offset: usize, value: i16) {
    let bytes = value.to_le_bytes();
    buf[offset] = bytes[0];
    buf[offset.saturating_add(1_usize)] = bytes[1];
}

#[expect(
    clippy::little_endian_bytes,
    reason = "BLE wire format is little-endian"
)]
const fn put_u16(buf: &mut [u8; FRAME_LEN], offset: usize, value: u16) {
    let bytes = value.to_le_bytes();
    buf[offset] = bytes[0];
    buf[offset.saturating_add(1_usize)] = bytes[1];
}

#[expect(
    clippy::little_endian_bytes,
    reason = "BLE wire format is little-endian"
)]
fn get_i16(bytes: &[u8], offset: usize) -> Option<i16> {
    let lo = *bytes.get(offset)?;
    let hi = *bytes.get(offset.saturating_add(1_usize))?;
    Some(i16::from_le_bytes([lo, hi]))
}

#[expect(
    clippy::little_endian_bytes,
    reason = "BLE wire format is little-endian"
)]
fn get_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let lo = *bytes.get(offset)?;
    let hi = *bytes.get(offset.saturating_add(1_usize))?;
    Some(u16::from_le_bytes([lo, hi]))
}

// ── Scaling helpers ───────────────────────────────────────────────────────────

/// Scale `value` by `factor`, round, and clamp to the safe `i16` range
/// `[i16::MIN + 1, i16::MAX]` (avoiding the sentinel `i16::MIN`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "value is clamped to i16 range before cast"
)]
fn scale_i16(value: f32, factor: f32) -> i16 {
    let scaled = libm::roundf(value * factor);
    // Safe min: i16::MIN + 1 (one above sentinel). i16::MIN as f32 = -32768.0
    let min = -32_767.0_f32;
    let max = 32_767.0_f32; // i16::MAX
    let clamped = if scaled < min {
        min
    } else if scaled > max {
        max
    } else {
        scaled
    };
    clamped as i16
}

/// Scale `value` by `factor`, round, and clamp to the safe `u16` range
/// `[0, u16::MAX - 1]` (avoiding the sentinel `u16::MAX`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "value is clamped to u16 range before cast"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "value is clamped to non-negative range before cast"
)]
fn scale_u16(value: f32, factor: f32) -> u16 {
    let scaled = libm::roundf(value * factor);
    // Safe max: u16::MAX - 1 = 65534 (one below sentinel)
    let max = 65_534.0_f32;
    let clamped = if scaled < 0.0_f32 {
        0.0_f32
    } else if scaled > max {
        max
    } else {
        scaled
    };
    clamped as u16
}

// ── Luminosity codec ──────────────────────────────────────────────────────────

/// Encode `lux` as `(mantissa: u16, exponent: u8)`.
///
/// Picks the smallest base-10 exponent that keeps the mantissa within
/// `[0, u16::MAX - 1]`.  Sentinel: mantissa `u16::MAX`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "value is clamped to u16 range before cast"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "lux is non-negative; value clamped before cast"
)]
fn enc_lux(lux: f32) -> (u16, u8) {
    if lux < 0.0_f32 {
        return (0_u16, 0_u8);
    }
    // Safe max mantissa: u16::MAX - 1 = 65534
    let max_mantissa = 65_534.0_f32;
    let mut exp = 0_u8;
    let mut mantissa = lux;
    while mantissa > max_mantissa && exp < u8::MAX {
        mantissa = libm::roundf(mantissa / 10.0_f32);
        exp = exp.saturating_add(1_u8);
    }
    let m = if mantissa > max_mantissa {
        max_mantissa as u16
    } else {
        mantissa as u16
    };
    (m, exp)
}

/// Decode `(mantissa, exponent)` back to lux.
///
/// Returns `None` when mantissa is the sentinel `u16::MAX`.
fn dec_lux(mantissa: u16, exp: u8) -> Option<f32> {
    if mantissa == SENTINEL_U16 {
        return None;
    }
    // Compute 10^exp via repeated multiplication to stay no_std (no powi).
    let mut power = 1.0_f32;
    let mut remaining = exp;
    while remaining > 0_u8 {
        power *= 10.0_f32;
        remaining = remaining.saturating_sub(1_u8);
    }
    Some(f32::from(mantissa) * power)
}

// ── Frame impl ────────────────────────────────────────────────────────────────

impl Frame {
    /// Encode this frame into exactly [`FRAME_LEN`] bytes (little-endian, schema v1).
    #[must_use]
    pub fn encode(&self) -> [u8; FRAME_LEN] {
        let mut buf = [0_u8; FRAME_LEN];

        // Byte 0: schema version header
        buf[0_usize] = SCHEMA_VERSION;

        // Bytes 1–2: temperature (centi-°C, i16; sentinel = i16::MIN)
        match self.temperature_c {
            Some(c) => put_i16(&mut buf, 1_usize, scale_i16(c, 100.0_f32)),
            None => put_i16(&mut buf, 1_usize, SENTINEL_I16),
        }

        // Bytes 3–4: pressure (deci-hPa, u16; sentinel = u16::MAX)
        // Pa → deci-hPa: divide by 10 (multiply by 0.1)
        match self.pressure_pa {
            Some(pa) => put_u16(&mut buf, 3_usize, scale_u16(pa, 0.1_f32)),
            None => put_u16(&mut buf, 3_usize, SENTINEL_U16),
        }

        // Bytes 5–6: humidity (centi-%RH, u16; sentinel = u16::MAX)
        match self.humidity_pct {
            Some(pct) => put_u16(&mut buf, 5_usize, scale_u16(pct, 100.0_f32)),
            None => put_u16(&mut buf, 5_usize, SENTINEL_U16),
        }

        // Bytes 7–8: sky/IR temperature (centi-°C, i16; sentinel = i16::MIN)
        match self.sky_temp_c {
            Some(c) => put_i16(&mut buf, 7_usize, scale_i16(c, 100.0_f32)),
            None => put_i16(&mut buf, 7_usize, SENTINEL_I16),
        }

        // Bytes 9–10, 11: luminosity (mantissa u16 + exponent u8; sentinel = mantissa u16::MAX)
        if let Some(lux) = self.luminosity_lux {
            let (m, e) = enc_lux(lux);
            put_u16(&mut buf, 9_usize, m);
            buf[11_usize] = e;
        } else {
            put_u16(&mut buf, 9_usize, SENTINEL_U16);
            buf[11_usize] = 0_u8;
        }

        // Bytes 12–13: wind speed (cm/s, u16; sentinel = u16::MAX)
        // m/s → cm/s: multiply by 100
        match self.wind_speed_ms {
            Some(ms) => put_u16(&mut buf, 12_usize, scale_u16(ms, 100.0_f32)),
            None => put_u16(&mut buf, 12_usize, SENTINEL_U16),
        }

        // Bytes 14–15: wind direction (deci-degree, u16; sentinel = u16::MAX)
        // ° → deci-°: multiply by 10
        match self.wind_dir_deg {
            Some(deg) => put_u16(&mut buf, 14_usize, scale_u16(deg, 10.0_f32)),
            None => put_u16(&mut buf, 14_usize, SENTINEL_U16),
        }

        // Byte 16: battery (%, u8; sentinel = u8::MAX)
        match self.battery_pct {
            Some(pct) => {
                // Clamp away from sentinel (u8::MAX)
                buf[16_usize] = if pct == SENTINEL_U8 {
                    SENTINEL_U8.saturating_sub(1_u8)
                } else {
                    pct
                };
            }
            None => buf[16_usize] = SENTINEL_U8,
        }

        buf
    }

    /// Decode a byte slice produced by [`Frame::encode`].
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::TooShort`] if `bytes.len() < FRAME_LEN`, or
    /// [`DecodeError::UnknownVersion`] if the header byte is not the current
    /// schema version.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < FRAME_LEN {
            return Err(DecodeError::TooShort { got: bytes.len() });
        }

        // Safe: we just verified bytes.len() >= FRAME_LEN >= 1
        let version = bytes[0_usize];
        if version != SCHEMA_VERSION {
            return Err(DecodeError::UnknownVersion(version));
        }

        // Temperature (bytes 1–2): centi-°C → °C
        let temperature_c = get_i16(bytes, 1_usize).and_then(|raw| {
            if raw == SENTINEL_I16 {
                None
            } else {
                Some(f32::from(raw) / 100.0_f32)
            }
        });

        // Pressure (bytes 3–4): deci-hPa → Pa (× 10)
        let pressure_pa = get_u16(bytes, 3_usize).and_then(|raw| {
            if raw == SENTINEL_U16 {
                None
            } else {
                Some(f32::from(raw) * 10.0_f32)
            }
        });

        // Humidity (bytes 5–6): centi-%RH → %RH
        let humidity_pct = get_u16(bytes, 5_usize).and_then(|raw| {
            if raw == SENTINEL_U16 {
                None
            } else {
                Some(f32::from(raw) / 100.0_f32)
            }
        });

        // Sky temperature (bytes 7–8): centi-°C → °C
        let sky_temp_c = get_i16(bytes, 7_usize).and_then(|raw| {
            if raw == SENTINEL_I16 {
                None
            } else {
                Some(f32::from(raw) / 100.0_f32)
            }
        });

        // Luminosity (bytes 9–10 mantissa, byte 11 exponent)
        let luminosity_lux = get_u16(bytes, 9_usize).and_then(|mantissa| {
            let exp = *bytes.get(11_usize).unwrap_or(&0_u8);
            dec_lux(mantissa, exp)
        });

        // Wind speed (bytes 12–13): cm/s → m/s
        let wind_speed_ms = get_u16(bytes, 12_usize).and_then(|raw| {
            if raw == SENTINEL_U16 {
                None
            } else {
                Some(f32::from(raw) / 100.0_f32)
            }
        });

        // Wind direction (bytes 14–15): deci-° → °
        let wind_dir_deg = get_u16(bytes, 14_usize).and_then(|raw| {
            if raw == SENTINEL_U16 {
                None
            } else {
                Some(f32::from(raw) / 10.0_f32)
            }
        });

        // Battery (byte 16): direct percent
        let battery_pct = bytes
            .get(16_usize)
            .and_then(|&raw| if raw == SENTINEL_U8 { None } else { Some(raw) });

        Ok(Self {
            temperature_c,
            pressure_pa,
            humidity_pct,
            sky_temp_c,
            luminosity_lux,
            wind_speed_ms,
            wind_dir_deg,
            battery_pct,
        })
    }

    /// Yield `(tag, value)` pairs for every `Some` field, in wire order.
    ///
    /// Pressure is yielded in Pascals; temperature and sky temperature in °C;
    /// humidity in %RH; luminosity in lux; wind speed in m/s; wind direction
    /// in degrees; battery as f32 percent.
    pub fn present_fields(&self) -> impl Iterator<Item = (FrameField, f32)> + '_ {
        // Build a fixed-size array of options in wire order and flatten.
        [
            self.temperature_c.map(|v| (FrameField::Temperature, v)),
            self.pressure_pa.map(|v| (FrameField::Pressure, v)),
            self.humidity_pct.map(|v| (FrameField::Humidity, v)),
            self.sky_temp_c.map(|v| (FrameField::SkyTemp, v)),
            self.luminosity_lux.map(|v| (FrameField::Luminosity, v)),
            self.wind_speed_ms.map(|v| (FrameField::WindSpeed, v)),
            self.wind_dir_deg.map(|v| (FrameField::WindDir, v)),
            self.battery_pct
                .map(|v| (FrameField::Battery, f32::from(v))),
        ]
        .into_iter()
        .flatten()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;
    use std::boxed::Box;
    use std::error;
    use std::result;
    use std::vec::Vec;

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // ── round-trip all fields ─────────────────────────────────────────────────

    #[test]
    fn encode_then_decode_roundtrips_present_values() -> TestResult {
        // Given
        let frame = Frame {
            temperature_c: Some(23.45_f32),
            pressure_pa: Some(101_300.0_f32),
            humidity_pct: Some(55.5_f32),
            sky_temp_c: Some(-10.25_f32),
            luminosity_lux: Some(1_000.0_f32),
            wind_speed_ms: Some(5.5_f32),
            wind_dir_deg: Some(270.0_f32),
            battery_pct: Some(80_u8),
        };

        // When
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded)?;

        // Then
        let temp_ok = decoded
            .temperature_c
            .is_some_and(|t| (t - 23.45_f32).abs() < 0.02_f32);
        assert!(temp_ok, "temperature round-trip failed: {decoded:?}");

        let pressure_ok = decoded
            .pressure_pa
            .is_some_and(|p| (p - 101_300.0_f32).abs() < 15.0_f32);
        assert!(pressure_ok, "pressure round-trip failed: {decoded:?}");

        let humidity_ok = decoded
            .humidity_pct
            .is_some_and(|h| (h - 55.5_f32).abs() < 0.02_f32);
        assert!(humidity_ok, "humidity round-trip failed: {decoded:?}");

        let sky_ok = decoded
            .sky_temp_c
            .is_some_and(|s| (s - (-10.25_f32)).abs() < 0.02_f32);
        assert!(sky_ok, "sky temperature round-trip failed: {decoded:?}");

        let lux_ok = decoded
            .luminosity_lux
            .is_some_and(|l| (l - 1_000.0_f32).abs() < 1.0_f32);
        assert!(lux_ok, "luminosity round-trip failed: {decoded:?}");

        let wind_ok = decoded
            .wind_speed_ms
            .is_some_and(|w| (w - 5.5_f32).abs() < 0.02_f32);
        assert!(wind_ok, "wind speed round-trip failed: {decoded:?}");

        let dir_ok = decoded
            .wind_dir_deg
            .is_some_and(|d| (d - 270.0_f32).abs() < 0.2_f32);
        assert!(dir_ok, "wind direction round-trip failed: {decoded:?}");

        assert_eq!(
            decoded.battery_pct,
            Some(80_u8),
            "battery round-trip failed"
        );

        Ok(())
    }

    // ── absent fields → sentinels and None ───────────────────────────────────

    #[expect(
        clippy::little_endian_bytes,
        reason = "test fixtures verify little-endian sentinel bytes in the wire format"
    )]
    #[test]
    fn absent_fields_encode_to_sentinels_and_decode_to_none() -> TestResult {
        // Given
        let frame = Frame::default();

        // When
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded)?;

        // Then – all None after decode
        assert!(
            decoded.temperature_c.is_none(),
            "temperature should be None"
        );
        assert!(decoded.pressure_pa.is_none(), "pressure should be None");
        assert!(decoded.humidity_pct.is_none(), "humidity should be None");
        assert!(decoded.sky_temp_c.is_none(), "sky_temp should be None");
        assert!(
            decoded.luminosity_lux.is_none(),
            "luminosity should be None"
        );
        assert!(decoded.wind_speed_ms.is_none(), "wind speed should be None");
        assert!(
            decoded.wind_dir_deg.is_none(),
            "wind direction should be None"
        );
        assert!(decoded.battery_pct.is_none(), "battery should be None");

        // Check sentinel bytes at expected offsets
        // Temperature i16::MIN at bytes 1-2
        let temp_raw = i16::from_le_bytes([encoded[1_usize], encoded[2_usize]]);
        assert_eq!(temp_raw, i16::MIN, "temperature sentinel byte check");

        // Pressure u16::MAX at bytes 3-4
        let press_raw = u16::from_le_bytes([encoded[3_usize], encoded[4_usize]]);
        assert_eq!(press_raw, u16::MAX, "pressure sentinel byte check");

        // Humidity u16::MAX at bytes 5-6
        let hum_raw = u16::from_le_bytes([encoded[5_usize], encoded[6_usize]]);
        assert_eq!(hum_raw, u16::MAX, "humidity sentinel byte check");

        // Sky temp i16::MIN at bytes 7-8
        let sky_raw = i16::from_le_bytes([encoded[7_usize], encoded[8_usize]]);
        assert_eq!(sky_raw, i16::MIN, "sky temp sentinel byte check");

        // Luminosity sentinel: mantissa u16::MAX at bytes 9-10
        let lux_raw = u16::from_le_bytes([encoded[9_usize], encoded[10_usize]]);
        assert_eq!(lux_raw, u16::MAX, "luminosity sentinel byte check");

        // Wind speed u16::MAX at bytes 12-13
        let wind_raw = u16::from_le_bytes([encoded[12_usize], encoded[13_usize]]);
        assert_eq!(wind_raw, u16::MAX, "wind speed sentinel byte check");

        // Wind direction u16::MAX at bytes 14-15
        let dir_raw = u16::from_le_bytes([encoded[14_usize], encoded[15_usize]]);
        assert_eq!(dir_raw, u16::MAX, "wind direction sentinel byte check");

        // Battery u8::MAX at byte 16
        assert_eq!(encoded[16_usize], u8::MAX, "battery sentinel byte check");

        Ok(())
    }

    // ── frame length ──────────────────────────────────────────────────────────

    #[test]
    fn encoded_frame_is_exactly_frame_len_bytes() {
        // Given
        let frame = Frame::default();

        // When
        let encoded = frame.encode();

        // Then
        assert_eq!(
            encoded.len(),
            FRAME_LEN,
            "encoded frame length must be FRAME_LEN"
        );
    }

    // ── error cases ───────────────────────────────────────────────────────────

    #[test]
    fn decode_rejects_short_buffer() {
        // Given
        let short = [0_u8; 10];

        // When
        let result = Frame::decode(&short);

        // Then
        assert!(
            matches!(result, Err(DecodeError::TooShort { got: 10 })),
            "expected TooShort{{got: 10}}, got {result:?}"
        );
    }

    #[test]
    fn decode_rejects_unknown_version() {
        // Given
        let mut buf = [0_u8; FRAME_LEN];
        buf[0_usize] = 0xFF_u8;

        // When
        let result = Frame::decode(&buf);

        // Then
        assert!(
            matches!(result, Err(DecodeError::UnknownVersion(0xFF))),
            "expected UnknownVersion(0xFF), got {result:?}"
        );
    }

    // ── pressure in Pascals ───────────────────────────────────────────────────

    #[test]
    fn pressure_roundtrips_in_pascals() -> TestResult {
        // Given
        let frame = Frame {
            pressure_pa: Some(101_325.0_f32),
            ..Frame::default()
        };

        // When
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded)?;

        // Then
        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let pa = decoded
            .pressure_pa
            .expect("pressure should be Some after round-trip");
        assert!(
            (pa - 101_325.0_f32).abs() < 10.0_f32,
            "pressure round-trip error too large: got {pa}"
        );

        Ok(())
    }

    // ── saturation without hitting sentinel ───────────────────────────────────

    #[expect(
        clippy::little_endian_bytes,
        reason = "test fixture verifies little-endian encoding of saturated i16 value"
    )]
    #[test]
    fn values_saturate_without_hitting_sentinel() {
        // Given — extreme temperature (400 °C exceeds i16 centi-°C range)
        let frame = Frame {
            temperature_c: Some(400.0_f32),
            ..Frame::default()
        };

        // When
        let encoded = frame.encode();
        let temp_raw = i16::from_le_bytes([encoded[1_usize], encoded[2_usize]]);

        // Then — clamped to i16::MAX, not i16::MIN (sentinel)
        assert_ne!(
            temp_raw,
            i16::MIN,
            "saturated temperature must not equal sentinel"
        );
        assert_eq!(
            temp_raw,
            i16::MAX,
            "saturated temperature should equal i16::MAX"
        );
    }

    // ── luminosity mantissa/exponent roundtrip ────────────────────────────────

    #[test]
    fn luminosity_mantissa_exponent_roundtrips() -> TestResult {
        // Given
        let lux = 98_765.0_f32;
        let frame = Frame {
            luminosity_lux: Some(lux),
            ..Frame::default()
        };

        // When
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded)?;

        // Then — within 5% relative error
        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let got = decoded
            .luminosity_lux
            .expect("luminosity should be Some after round-trip");
        let rel_err = (got - lux).abs() / lux;
        assert!(
            rel_err <= 0.05_f32,
            "luminosity relative error {rel_err} exceeds 5%: expected ~{lux}, got {got}"
        );

        Ok(())
    }

    // ── present_fields ordering ───────────────────────────────────────────────

    #[test]
    fn present_fields_yields_only_some_in_wire_order() {
        // Given
        let frame = Frame {
            temperature_c: Some(20.0_f32),
            pressure_pa: Some(100_000.0_f32),
            ..Frame::default()
        };

        // When
        let fields: Vec<(FrameField, f32)> = frame.present_fields().collect();

        // Then
        assert_eq!(fields.len(), 2_usize, "expected exactly 2 present fields");
        assert_eq!(
            fields[0_usize].0,
            FrameField::Temperature,
            "first field should be Temperature"
        );
        assert_eq!(
            fields[1_usize].0,
            FrameField::Pressure,
            "second field should be Pressure"
        );

        // Pressure should be in Pascals
        assert!(
            (fields[1_usize].1 - 100_000.0_f32).abs() < 1.0_f32,
            "pressure field should be in Pa, got {}",
            fields[1_usize].1
        );
    }
}
// grcov exclude end
