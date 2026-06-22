// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive"
)]

use embedded_hal_async::i2c::I2c;

// VEML7700 Register addresses
const ALS_CONF_0: u8 = 0x00;
const ALS_DATA: u8 = 0x04;
const ID_REG: u8 = 0x07;
const EXPECTED_ID_LOW: u8 = 0x81;

/// Auto-ranging raw-count lower bound (Vishay app-note defaults).
/// When raw count drops below this, step to a more-sensitive setting.
pub const COUNT_LO: u16 = 100;

/// Auto-ranging raw-count upper bound (Vishay app-note defaults).
/// When raw count exceeds this, step to a less-sensitive setting.
pub const COUNT_HI: u16 = 10_000;

/// Maximum valid ladder index (= `LADDER.len() - 1`).
const LADDER_MAX_IDX: usize = 7;

/// Gain setting for the ALS channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Gain {
    /// Gain = 1/8
    X1_8,
    /// Gain = 1/4
    X1_4,
    /// Gain = 1
    X1,
    /// Gain = 2
    X2,
}

impl Gain {
    /// Returns the linear multiplier for this gain setting.
    #[must_use]
    const fn multiplier(self) -> f32 {
        match self {
            Self::X1_8 => 0.125,
            Self::X1_4 => 0.25,
            Self::X1 => 1.0,
            Self::X2 => 2.0,
        }
    }

    /// Returns the 2-bit field value for bits 12:11 of `ALS_CONF_0`.
    ///
    /// Per datasheet: X1=`00`, X2=`01`, `X1_8`=`10`, `X1_4`=`11`.
    /// These raw bits must be shifted into position by the caller.
    #[must_use]
    const fn bits(self) -> u16 {
        match self {
            Self::X1 => 0b00,
            Self::X2 => 0b01,
            Self::X1_8 => 0b10,
            Self::X1_4 => 0b11,
        }
    }
}

/// Integration time setting for the ALS channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum IntegrationTime {
    /// 25 ms integration time.
    Ms25,
    /// 50 ms integration time.
    Ms50,
    /// 100 ms integration time.
    Ms100,
    /// 200 ms integration time.
    Ms200,
    /// 400 ms integration time.
    Ms400,
    /// 800 ms integration time.
    Ms800,
}

impl IntegrationTime {
    /// Returns the integration time duration in milliseconds.
    #[must_use]
    pub const fn millis(self) -> u32 {
        match self {
            Self::Ms25 => 25,
            Self::Ms50 => 50,
            Self::Ms100 => 100,
            Self::Ms200 => 200,
            Self::Ms400 => 400,
            Self::Ms800 => 800,
        }
    }

    /// Returns the 4-bit field value for bits 9:6 of `ALS_CONF_0`.
    ///
    /// Per datasheet: 25ms=`1100`, 50ms=`1000`, 100ms=`0000`,
    /// 200ms=`0001`, 400ms=`0010`, 800ms=`0011`.
    /// These raw bits must be shifted into position by the caller.
    #[must_use]
    const fn bits(self) -> u16 {
        match self {
            Self::Ms25 => 0b1100,
            Self::Ms50 => 0b1000,
            Self::Ms100 => 0b0000,
            Self::Ms200 => 0b0001,
            Self::Ms400 => 0b0010,
            Self::Ms800 => 0b0011,
        }
    }
}

/// A combined gain + integration-time setting for the VEML7700.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// Gain selection.
    pub gain: Gain,
    /// Integration time selection.
    pub it: IntegrationTime,
}

/// Sensitivity ladder, least → most sensitive (index 0 = best for bright light).
///
/// Auto-ranging steps one rung at a time: up (towards index 7) to become more
/// sensitive when the scene is dark, down (towards index 0) when saturated.
pub const LADDER: [Setting; 8] = [
    Setting {
        gain: Gain::X1_8,
        it: IntegrationTime::Ms25,
    }, // 0  res ≈ 2.1504
    Setting {
        gain: Gain::X1_8,
        it: IntegrationTime::Ms100,
    }, // 1  res ≈ 0.5376
    Setting {
        gain: Gain::X1_4,
        it: IntegrationTime::Ms100,
    }, // 2  res ≈ 0.2688
    Setting {
        gain: Gain::X1,
        it: IntegrationTime::Ms100,
    }, // 3  res ≈ 0.0672
    Setting {
        gain: Gain::X2,
        it: IntegrationTime::Ms100,
    }, // 4  res ≈ 0.0336
    Setting {
        gain: Gain::X2,
        it: IntegrationTime::Ms200,
    }, // 5  res ≈ 0.0168
    Setting {
        gain: Gain::X2,
        it: IntegrationTime::Ms400,
    }, // 6  res ≈ 0.0084
    Setting {
        gain: Gain::X2,
        it: IntegrationTime::Ms800,
    }, // 7  res ≈ 0.0042
];

/// Starting index in [`LADDER`]: `(X1, Ms100)`, mid-range first guess.
pub const LADDER_START: usize = 3;

/// Driver error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// I2C bus error.
    I2c(E),
    /// The ID register low byte did not match the expected value (`0x81`).
    WrongId(u8),
}

/// Returns the lux-per-count resolution for a given [`Setting`].
///
/// Formula: `0.0042 * (800 / it_ms) * (2 / gain_mult)`.
#[expect(
    clippy::cast_precision_loss,
    reason = "u32 integration-time millis intentionally cast to f32 for resolution formula"
)]
#[must_use]
pub fn resolution(s: Setting) -> f32 {
    let it_ms = s.it.millis() as f32;
    let gain_mult = s.gain.multiplier();
    0.0042 * (800.0 / it_ms) * (2.0 / gain_mult)
}

/// Nonlinearity correction polynomial coefficients (Vishay datasheet).
/// Applied only when linear lux **strictly exceeds** 1000.0.
const C4: f32 = 6.0135e-13;
const C3: f32 = -9.3924e-9;
const C2: f32 = 8.1488e-5;
const C1: f32 = 1.0023;

/// Converts a raw ALS count to lux for a given [`Setting`].
///
/// When `lux > 1000.0` (strictly above), the Vishay nonlinearity polynomial is
/// applied: `C4*l^4 + C3*l^3 + C2*l^2 + C1*l`. At exactly 1000.0 the linear
/// value is returned unchanged.
#[must_use]
pub fn raw_to_lux(raw: u16, s: Setting) -> f32 {
    let lux = f32::from(raw) * resolution(s);
    if lux > 1000.0 {
        let l = lux;
        C4 * l * l * l * l + C3 * l * l * l + C2 * l * l + C1 * l
    } else {
        lux
    }
}

/// Returns the 16-bit `ALS_CONF_0` register value for a given [`Setting`].
///
/// Layout: bits 12:11 = gain field, bits 9:6 = IT field, all other bits 0
/// (including bit 0 = `ALS_SD`, which = 0 means powered on).
///
/// `gain.bits()` and `it.bits()` return pre-shifted raw field values (not
/// yet positioned); this function places them: `(gain_bits << 11) | (it_bits << 6)`.
#[must_use]
pub const fn als_conf0(s: Setting) -> u16 {
    (s.gain.bits() << 11) | (s.it.bits() << 6)
}

/// Returns the next ladder index based on the current raw count.
///
/// - If `raw > COUNT_HI` and `idx > 0`: step down (less sensitive, index - 1).
/// - If `raw < COUNT_LO` and `idx < LADDER_MAX_IDX`: step up (more sensitive, index + 1).
/// - Otherwise: stay at `idx`.
///
/// Clamps at both ends of the ladder.
#[must_use]
pub const fn next_index(idx: usize, raw: u16) -> usize {
    if raw > COUNT_HI && idx > 0 {
        idx.saturating_sub(1)
    } else if raw < COUNT_LO && idx < LADDER_MAX_IDX {
        idx.saturating_add(1)
    } else {
        idx
    }
}

/// VEML7700 ambient light sensor driver.
///
/// Manages the I2C interface to the sensor. The firmware task is responsible
/// for waiting the integration-time period between [`set_setting`](Self::set_setting)
/// and [`read_raw`](Self::read_raw), and for calling [`next_index`] to adjust the
/// sensitivity ladder index as needed.
pub struct Veml7700<I> {
    i2c: I,
    address: u8,
}

impl<I, E> Veml7700<I>
where
    I: I2c<Error = E>,
{
    /// Creates a new VEML7700 driver instance.
    ///
    /// Does not communicate with the device; call [`verify_id`](Self::verify_id)
    /// before the first measurement.
    #[must_use]
    pub const fn new(i2c: I, address: u8) -> Self {
        Self { i2c, address }
    }

    /// Reads the ID register (0x07) and verifies the low byte is `0x81`.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if communication fails, or `Error::WrongId` if the
    /// low byte of the ID register does not match `0x81`.
    pub async fn verify_id(&mut self) -> Result<(), Error<E>> {
        let mut buf = [0_u8; 2];
        self.i2c
            .write_read(self.address, &[ID_REG], &mut buf)
            .await
            .map_err(Error::I2c)?;

        let id_low = buf[0];
        if id_low != EXPECTED_ID_LOW {
            return Err(Error::WrongId(id_low));
        }
        Ok(())
    }

    /// Writes `ALS_CONF_0` with the register value for `s`, powering the sensor on
    /// (`ALS_SD` = 0).
    ///
    /// The 16-bit value is written LSB-first: `[ALS_CONF_0, lo, hi]`.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if the write fails.
    pub async fn set_setting(&mut self, s: Setting) -> Result<(), Error<E>> {
        let conf = als_conf0(s);
        let lo = u8::try_from(conf & 0xFF).unwrap_or(0);
        let hi = u8::try_from(conf >> 8).unwrap_or(0);
        self.i2c
            .write(self.address, &[ALS_CONF_0, lo, hi])
            .await
            .map_err(Error::I2c)
    }

    /// Reads the `ALS_DATA` register (0x04) and returns the raw 16-bit count.
    ///
    /// The register returns two bytes LSB-first; this function recombines them
    /// as `u16::from_le_bytes([lo, hi])`.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if the read fails.
    pub async fn read_raw(&mut self) -> Result<u16, Error<E>> {
        let mut buf = [0_u8; 2];
        self.i2c
            .write_read(self.address, &[ALS_DATA], &mut buf)
            .await
            .map_err(Error::I2c)?;
        Ok(u16::from_le_bytes([buf[0], buf[1]]))
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    /// Expected resolution values for each ladder entry (lux/count).
    struct LadderCase {
        setting: Setting,
        expected: f32,
    }

    /// All 8 ladder cases used by `resolution_matches_datasheet_table`.
    const LADDER_CASES: [LadderCase; 8] = [
        LadderCase {
            setting: LADDER[0],
            expected: 2.1504,
        }, // X1_8, Ms25
        LadderCase {
            setting: LADDER[1],
            expected: 0.5376,
        }, // X1_8, Ms100
        LadderCase {
            setting: LADDER[2],
            expected: 0.2688,
        }, // X1_4, Ms100
        LadderCase {
            setting: LADDER[3],
            expected: 0.0672,
        }, // X1,   Ms100
        LadderCase {
            setting: LADDER[4],
            expected: 0.0336,
        }, // X2,   Ms100
        LadderCase {
            setting: LADDER[5],
            expected: 0.0168,
        }, // X2,   Ms200
        LadderCase {
            setting: LADDER[6],
            expected: 0.0084,
        }, // X2,   Ms400
        LadderCase {
            setting: LADDER[7],
            expected: 0.0042,
        }, // X2,   Ms800
    ];

    /// Epsilon comparison for f32 values.
    fn approx_eq(a: f32, b: f32, epsilon: f32) -> bool {
        (a - b).abs() < epsilon
    }

    #[test]
    fn resolution_matches_datasheet_table() {
        // Given / When / Then: verify all 8 ladder entries against expected values.
        // Resolution formula: 0.0042 * (800 / it_ms) * (2 / gain_mult)
        for (i, case) in LADDER_CASES.iter().enumerate() {
            let res = resolution(case.setting);
            assert!(
                approx_eq(res, case.expected, case.expected * 0.001),
                "LADDER[{i}]: expected {}, got {res}",
                case.expected
            );
        }
    }

    #[test]
    fn raw_to_lux_linear_below_1000() {
        // Given: X1, Ms100 → resolution = 0.0672; raw=100 → lux = 6.72 (well below 1000)
        let s = Setting {
            gain: Gain::X1,
            it: IntegrationTime::Ms100,
        };
        let raw: u16 = 100;

        // When
        let lux = raw_to_lux(raw, s);

        // Then: no correction applied; lux = raw * resolution exactly
        let expected = f32::from(raw) * resolution(s);
        assert!(
            approx_eq(lux, expected, 0.001),
            "expected {expected}, got {lux}"
        );
    }

    #[test]
    fn raw_to_lux_applies_correction_above_1000() {
        // Given: X1_8, Ms25 → resolution = 2.1504; raw = 1_000 → linear ≈ 2150.4 lux
        // (clearly above 1000, so polynomial correction is applied)
        let s = Setting {
            gain: Gain::X1_8,
            it: IntegrationTime::Ms25,
        };
        let raw: u16 = 1_000;

        // When
        let lux_corrected = raw_to_lux(raw, s);

        // Then: corrected value should be greater than uncorrected (polynomial is
        // additive in this range per Vishay app note)
        let lux_uncorrected = f32::from(raw) * resolution(s);
        assert!(
            lux_uncorrected > 1000.0,
            "sanity: linear lux {lux_uncorrected} must exceed 1000 for this test"
        );
        assert!(
            lux_corrected > lux_uncorrected,
            "corrected {lux_corrected} should exceed uncorrected {lux_uncorrected}"
        );
    }

    #[test]
    fn raw_to_lux_at_1000_lux_boundary() {
        // Given: X1, Ms100 → resolution = 0.0672.
        //   raw = 14_880 → lux = 14_880 * 0.0672 = 999.936  (≤ 1000, linear returned)
        //   raw = 14_881 → lux = 14_881 * 0.0672 ≈ 1000.003 (> 1000, correction applied)
        //
        // This pins the `> 1000.0` (strictly-above) condition: at the boundary the
        // linear (uncorrected) value is returned.
        let s = Setting {
            gain: Gain::X1,
            it: IntegrationTime::Ms100,
        };

        // Below boundary: raw=14_880 → linear lux ≤ 1000.0 → no correction
        let raw_below: u16 = 14_880;
        let linear_below = f32::from(raw_below) * resolution(s);
        assert!(
            linear_below <= 1000.0,
            "sanity: linear lux {linear_below} should be <= 1000.0"
        );
        let lux_below = raw_to_lux(raw_below, s);
        assert!(
            approx_eq(lux_below, linear_below, 0.001),
            "below boundary: expected linear {linear_below}, got {lux_below}"
        );

        // Above boundary: raw=14_881 → linear lux > 1000.0 → correction applied
        let raw_above: u16 = 14_881;
        let linear_above = f32::from(raw_above) * resolution(s);
        assert!(
            linear_above > 1000.0,
            "sanity: linear lux {linear_above} should be > 1000.0"
        );
        let lux_above = raw_to_lux(raw_above, s);
        assert!(
            lux_above > linear_above,
            "above boundary: corrected {lux_above} should exceed linear {linear_above}"
        );
    }

    #[test]
    fn als_conf0_encodes_gain_and_it_bits() {
        // Given: (X2, Ms100) — gain bits = 0b01, IT bits = 0b0000
        // Expected: (0b01 << 11) | (0b0000 << 6) = 0x0800
        let s_x2_ms100 = Setting {
            gain: Gain::X2,
            it: IntegrationTime::Ms100,
        };
        let conf_x2 = als_conf0(s_x2_ms100);
        assert_eq!(
            conf_x2, 0x0800,
            "X2/Ms100: expected 0x0800, got {conf_x2:#06X}"
        );

        // Given: (X1_8, Ms25) — gain bits = 0b10, IT bits = 0b1100
        // Expected: (0b10 << 11) | (0b1100 << 6) = 0x1000 | 0x0300 = 0x1300
        let s_x18_ms25 = Setting {
            gain: Gain::X1_8,
            it: IntegrationTime::Ms25,
        };
        let conf_x18 = als_conf0(s_x18_ms25);
        assert_eq!(
            conf_x18, 0x1300,
            "X1_8/Ms25: expected 0x1300, got {conf_x18:#06X}"
        );
    }

    #[test]
    fn next_index_steps_down_when_saturated() {
        // Given: idx=3, raw=11_000 (> COUNT_HI=10_000)

        // When
        let next = next_index(3, 11_000);

        // Then: step down to idx=2 (less sensitive)
        assert_eq!(next, 2);
    }

    #[test]
    fn next_index_steps_up_when_dark() {
        // Given: idx=3, raw=50 (< COUNT_LO=100)

        // When
        let next = next_index(3, 50);

        // Then: step up to idx=4 (more sensitive)
        assert_eq!(next, 4);
    }

    #[test]
    fn next_index_stays_in_band() {
        // Given: idx=3, raw=5_000 (between COUNT_LO=100 and COUNT_HI=10_000)

        // When
        let next = next_index(3, 5_000);

        // Then: no change
        assert_eq!(next, 3);
    }

    #[test]
    fn next_index_clamps_at_ends() {
        // Given: already at the least-sensitive end (idx=0), saturated
        let next_bottom = next_index(0, 60_000);
        // Then: stays at 0 (cannot step down further)
        assert_eq!(next_bottom, 0, "should clamp at bottom");

        // Given: already at the most-sensitive end (idx=7), dark
        let next_top = next_index(7, 10);
        // Then: stays at 7 (cannot step up further)
        assert_eq!(next_top, 7, "should clamp at top");
    }
}
// grcov exclude stop
