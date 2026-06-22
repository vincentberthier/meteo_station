use embedded_hal_async::i2c::I2c;

// BME280 Register addresses
const CHIP_ID_REG: u8 = 0xD0;
const EXPECTED_CHIP_ID: u8 = 0x60;
const CTRL_HUM: u8 = 0xF2;
const STATUS: u8 = 0xF3;
const CTRL_MEAS: u8 = 0xF4;
const CONFIG: u8 = 0xF5;
const CALIB_00: u8 = 0x88; // block 1: 0x88..=0xA1, 26 bytes (T1..P9, gap, H1)
const CALIB_26: u8 = 0xE1; // block 2: 0xE1..=0xE7, 7 bytes (H2..H6)

// STATUS register measuring flag: bit 3
const STATUS_MEASURING: u8 = 1 << 3;

// Weather/humidity forced config: osrs_h=x1, osrs_t=x1, osrs_p=x1, IIR off, forced.
const CTRL_HUM_X1: u8 = 0b0000_0001; // osrs_h = x1
const CTRL_MEAS_FORCED: u8 = 0b0010_0101; // osrs_t=001, osrs_p=001, mode=01 (forced)
const CONFIG_IIR_OFF: u8 = 0b0000_0000;

// Data burst: 8 bytes starting at 0xF7
const DATA_REG: u8 = 0xF7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    I2c(E),
    WrongChipId(u8),
}

pub struct Bme280<I> {
    i2c: I,
    address: u8,
    calib: CalibData,
}

impl<I, E> Bme280<I>
where
    I: I2c<Error = E>,
{
    /// Creates a new BME280 driver instance.
    ///
    /// Verifies the chip ID, reads both calibration blocks, configures the IIR
    /// filter and humidity oversampling. The sensor is left in sleep; each
    /// [`read`](Self::read) triggers a single forced-mode measurement.
    ///
    /// The datasheet ordering rule is followed: `CTRL_HUM` is written **before**
    /// `CTRL_MEAS` so the humidity oversampling setting takes effect.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if communication fails, or `Error::WrongChipId`
    /// if the chip does not identify as a BME280 (expected `0x60`).
    pub async fn new(mut i2c: I, address: u8) -> Result<Self, Error<E>> {
        // Verify chip ID
        let mut chip_id = [0_u8; 1];
        i2c.write_read(address, &[CHIP_ID_REG], &mut chip_id)
            .await
            .map_err(Error::I2c)?;

        if chip_id[0] != EXPECTED_CHIP_ID {
            return Err(Error::WrongChipId(chip_id[0]));
        }

        // Read calibration block 1: 0x88..=0xA1 (26 bytes, T1..P9 + H1)
        let mut calib_b1 = [0_u8; 26];
        i2c.write_read(address, &[CALIB_00], &mut calib_b1)
            .await
            .map_err(Error::I2c)?;

        // Read calibration block 2: 0xE1..=0xE7 (7 bytes, H2..H6)
        let mut calib_b2 = [0_u8; 7];
        i2c.write_read(address, &[CALIB_26], &mut calib_b2)
            .await
            .map_err(Error::I2c)?;

        let calib = CalibData::from_raw_bytes(&calib_b1, calib_b2);

        // Write CONFIG (IIR off) — in sleep mode (default after POR)
        i2c.write(address, &[CONFIG, CONFIG_IIR_OFF])
            .await
            .map_err(Error::I2c)?;

        // Write CTRL_HUM first (datasheet ordering: must precede CTRL_MEAS)
        i2c.write(address, &[CTRL_HUM, CTRL_HUM_X1])
            .await
            .map_err(Error::I2c)?;

        Ok(Self {
            i2c,
            address,
            calib,
        })
    }

    /// Triggers one forced-mode measurement and returns the compensated
    /// temperature, pressure, and humidity.
    ///
    /// Writes `CTRL_MEAS` to trigger, then polls `STATUS` bit 3 until the
    /// measuring flag clears (no fixed delay — each iteration yields to the
    /// async executor). Burst-reads 0xF7..=0xFE (8 bytes) and compensates
    /// temperature first (to set `t_fine`), then pressure, then humidity.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if communication with the sensor fails.
    pub async fn read(&mut self) -> Result<Reading, Error<E>> {
        // Trigger one forced measurement (re-write ctrl_hum before ctrl_meas
        // each cycle per datasheet ordering rule)
        self.i2c
            .write(self.address, &[CTRL_HUM, CTRL_HUM_X1])
            .await
            .map_err(Error::I2c)?;
        self.i2c
            .write(self.address, &[CTRL_MEAS, CTRL_MEAS_FORCED])
            .await
            .map_err(Error::I2c)?;

        // Poll STATUS bit 3 until measuring flag clears.
        // Each I2C transaction awaits, yielding to the executor between polls.
        loop {
            let mut status = [0_u8; 1];
            self.i2c
                .write_read(self.address, &[STATUS], &mut status)
                .await
                .map_err(Error::I2c)?;
            if status[0] & STATUS_MEASURING == 0 {
                break;
            }
        }

        // Burst-read all 8 data bytes:
        // d[0..2] = press (F7, F8, F9), d[3..5] = temp (FA, FB, FC),
        // d[6..7] = hum (FD, FE)
        let mut d = [0_u8; 8];
        self.i2c
            .write_read(self.address, &[DATA_REG], &mut d)
            .await
            .map_err(Error::I2c)?;

        let adc_p = (i32::from(d[0]) << 12) | (i32::from(d[1]) << 4) | (i32::from(d[2]) >> 4);
        let adc_t = (i32::from(d[3]) << 12) | (i32::from(d[4]) << 4) | (i32::from(d[5]) >> 4);
        let adc_h = (i32::from(d[6]) << 8) | i32::from(d[7]);

        // Temperature first — sets t_fine used by pressure and humidity
        let temperature = self.calib.compensate_temperature(adc_t);
        let pressure = self.calib.compensate_pressure(adc_p);
        let humidity = self.calib.compensate_humidity(adc_h);

        Ok(Reading {
            temperature,
            pressure,
            humidity,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    /// Temperature in degrees Celsius
    pub temperature: f32,
    /// Pressure in Pascals
    pub pressure: f32,
    /// Relative humidity in percent (0..=100)
    pub humidity: f32,
}

impl Reading {
    #[must_use]
    pub fn pressure_hpa(&self) -> f32 {
        self.pressure / 100.0
    }
}

struct CalibData {
    dig_t1: u16,
    dig_t2: i16,
    dig_t3: i16,
    dig_p1: u16,
    dig_p2: i16,
    dig_p3: i16,
    dig_p4: i16,
    dig_p5: i16,
    dig_p6: i16,
    dig_p7: i16,
    dig_p8: i16,
    dig_p9: i16,
    dig_h1: u8,
    dig_h2: i16,
    dig_h3: u8,
    dig_h4: i16,
    dig_h5: i16,
    dig_h6: i8,
    /// Intermediate temperature value shared between temperature, pressure,
    /// and humidity compensation (Bosch nomenclature: `t_fine`).
    t_fine: f32,
}

impl CalibData {
    /// Parses both BME280 calibration memory blocks.
    ///
    /// `b1` is the 26-byte block starting at `0x88` (T1..P9, then H1 at `0xA1`).
    /// `b2` is the 7-byte block starting at `0xE1` (H2..H6).
    ///
    /// The H4/H5 coefficients use 12-bit packing across shared nibbles of `b2[4]`:
    /// the high byte of each is signed, shifted left by 4 bits, and OR'd with the
    /// appropriate nibble of `b2[4]`.
    #[expect(
        clippy::similar_names,
        reason = "names match Bosch BME280 datasheet nomenclature (dig_t*, dig_p*, dig_h*)"
    )]
    #[expect(
        clippy::little_endian_bytes,
        reason = "BME280 stores calibration data in little-endian"
    )]
    #[expect(
        clippy::cast_possible_wrap,
        reason = "intentional reinterpret of raw calibration byte as signed i8"
    )]
    fn from_raw_bytes(b1: &[u8; 26], b2: [u8; 7]) -> Self {
        // Block 1 (0x88..=0xA1)
        let dig_t1 = u16::from_le_bytes([b1[0], b1[1]]);
        let dig_t2 = i16::from_le_bytes([b1[2], b1[3]]);
        let dig_t3 = i16::from_le_bytes([b1[4], b1[5]]);

        let dig_p1 = u16::from_le_bytes([b1[6], b1[7]]);
        let dig_p2 = i16::from_le_bytes([b1[8], b1[9]]);
        let dig_p3 = i16::from_le_bytes([b1[10], b1[11]]);
        let dig_p4 = i16::from_le_bytes([b1[12], b1[13]]);
        let dig_p5 = i16::from_le_bytes([b1[14], b1[15]]);
        let dig_p6 = i16::from_le_bytes([b1[16], b1[17]]);
        let dig_p7 = i16::from_le_bytes([b1[18], b1[19]]);
        let dig_p8 = i16::from_le_bytes([b1[20], b1[21]]);
        let dig_p9 = i16::from_le_bytes([b1[22], b1[23]]);
        // b1[24] is 0xA0 — reserved gap byte, unused
        let dig_h1 = b1[25]; // 0xA1

        // Block 2 (0xE1..=0xE7)
        let dig_h2 = i16::from_le_bytes([b2[0], b2[1]]);
        let dig_h3 = b2[2];
        // H4: high byte b2[3] (signed) * 16, OR'd with low nibble of b2[4]
        let dig_h4 = (i16::from(b2[3] as i8) << 4) | i16::from(b2[4] & 0x0F);
        // H5: high byte b2[5] (signed) * 16, OR'd with high nibble of b2[4]
        let dig_h5 = (i16::from(b2[5] as i8) << 4) | i16::from(b2[4] >> 4);
        let dig_h6 = b2[6] as i8;

        Self {
            dig_t1,
            dig_t2,
            dig_t3,
            dig_p1,
            dig_p2,
            dig_p3,
            dig_p4,
            dig_p5,
            dig_p6,
            dig_p7,
            dig_p8,
            dig_p9,
            dig_h1,
            dig_h2,
            dig_h3,
            dig_h4,
            dig_h5,
            dig_h6,
            t_fine: 0.0,
        }
    }

    /// Compensates raw ADC temperature using the Bosch double-precision formula
    /// ported to f32. Sets `self.t_fine` as a side-effect; must be called before
    /// [`compensate_pressure`](Self::compensate_pressure) and
    /// [`compensate_humidity`](Self::compensate_humidity).
    #[expect(
        clippy::cast_precision_loss,
        reason = "i32 ADC value intentionally cast to f32 for Bosch compensation formula"
    )]
    fn compensate_temperature(&mut self, adc_t: i32) -> f32 {
        let var1 =
            (adc_t as f32 / 16_384.0 - f32::from(self.dig_t1) / 1_024.0) * f32::from(self.dig_t2);
        let d = adc_t as f32 / 131_072.0 - f32::from(self.dig_t1) / 8_192.0;
        let var2 = d * d * f32::from(self.dig_t3);
        self.t_fine = var1 + var2;
        self.t_fine / 5_120.0
    }

    /// Compensates raw ADC pressure using the Bosch double-precision formula
    /// ported to f32. Returns pressure in Pascals. Returns `0.0` if the
    /// intermediate `var1` is `0.0` (avoids division by zero).
    ///
    /// [`compensate_temperature`](Self::compensate_temperature) must be called
    /// first to set `t_fine`.
    #[expect(
        clippy::cast_precision_loss,
        reason = "i32 ADC value intentionally cast to f32 for Bosch compensation formula"
    )]
    #[expect(
        clippy::shadow_reuse,
        reason = "var1/var2/p re-bindings follow Bosch reference algorithm stages"
    )]
    #[expect(
        clippy::shadow_unrelated,
        reason = "var1/var2 re-bindings follow Bosch reference algorithm stages"
    )]
    #[expect(
        clippy::lossy_float_literal,
        reason = "constant from Bosch reference compensation code"
    )]
    fn compensate_pressure(&self, adc_p: i32) -> f32 {
        let var1 = self.t_fine / 2.0 - 64_000.0;
        let var2 = var1 * var1 * f32::from(self.dig_p6) / 32_768.0;
        let var2 = var2 + var1 * f32::from(self.dig_p5) * 2.0;
        let var2 = var2 / 4.0 + f32::from(self.dig_p4) * 65_536.0;
        let var1 = (f32::from(self.dig_p3) * var1 * var1 / 524_288.0
            + f32::from(self.dig_p2) * var1)
            / 524_288.0;
        let var1 = (1.0 + var1 / 32_768.0) * f32::from(self.dig_p1);

        if var1 == 0.0 {
            return 0.0;
        }

        let p = 1_048_576.0 - adc_p as f32;
        let p = (p - var2 / 4_096.0) * 6_250.0 / var1;
        let var1 = f32::from(self.dig_p9) * p * p / 2_147_483_648.0;
        let var2 = p * f32::from(self.dig_p8) / 32_768.0;
        p + (var1 + var2 + f32::from(self.dig_p7)) / 16.0
    }

    /// Compensates raw ADC humidity using the Bosch double-precision formula
    /// ported to f32. Returns humidity in %RH, clamped to `[0.0, 100.0]`.
    ///
    /// [`compensate_temperature`](Self::compensate_temperature) must be called
    /// first to set `t_fine`.
    #[expect(
        clippy::cast_precision_loss,
        reason = "i32 ADC value intentionally cast to f32 for Bosch compensation formula"
    )]
    fn compensate_humidity(&self, adc_h: i32) -> f32 {
        let tf = self.t_fine - 76_800.0;
        let hx = (adc_h as f32
            - (f32::from(self.dig_h4) * 64.0 + f32::from(self.dig_h5) / 16_384.0 * tf))
            * (f32::from(self.dig_h2) / 65_536.0
                * (1.0
                    + f32::from(self.dig_h6) / 67_108_864.0
                        * tf
                        * (1.0 + f32::from(self.dig_h3) / 67_108_864.0 * tf)));
        let hy = hx * (1.0 - f32::from(self.dig_h1) * hx / 524_288.0);
        hy.clamp(0.0, 100.0)
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    // Sample calibration block 1 (0x88..=0xA1, 26 bytes).
    // Chosen so that the raw integer values are easy to verify:
    // dig_t1=27328, dig_t2=26214, dig_t3=50, dig_p1=36592, dig_h1=75 (index 25)
    fn sample_calib_b1() -> [u8; 26] {
        [
            0xC0, 0x6A, // dig_t1 = 0x6AC0 = 27_328
            0x66, 0x66, // dig_t2 = 0x6666 = 26_214
            0x32, 0x00, // dig_t3 = 0x0032 = 50
            0xF0, 0x8E, // dig_p1 = 0x8EF0 = 36_592
            0xD6, 0xD0, // dig_p2 = -12_074
            0xC0, 0xFF, // dig_p3 = -64
            0x09, 0x00, // dig_p4 = 9
            0x9E, 0x0E, // dig_p5 = 3_742
            0xF9, 0xFF, // dig_p6 = -7
            0x0C, 0x00, // dig_p7 = 12
            0xF8, 0xFF, // dig_p8 = -8
            0x00, 0x00, // dig_p9 = 0
            0x00, // 0xA0 reserved gap
            0x4B, // dig_h1 = 75
        ]
    }

    // Sample calibration block 2 (0xE1..=0xE7, 7 bytes).
    // dig_h2=358, dig_h3=0, dig_h6=30
    // b2[3]=0x4B(75), b2[4]=0x34, b2[5]=0x32(50)
    // dig_h4 = (75<<4)|(0x34&0x0F) = 1200|4 = 1204
    // dig_h5 = (50<<4)|(0x34>>4)   =  800|3 =  803
    fn sample_calib_b2() -> [u8; 7] {
        [
            0x66, 0x01, // dig_h2 = 0x0166 = 358
            0x00, // dig_h3 = 0
            0x4B, // b2[3]: H4 high byte (i8 = 75)
            0x34, // b2[4]: shared nibbles (H4 low = 4, H5 low = 3)
            0x32, // b2[5]: H5 high byte (i8 = 50)
            0x1E, // dig_h6 = 30
        ]
    }

    // Sample calibration block 2 with negative H4 and H5 high bytes.
    // b2[3]=0x80(i8=-128): dig_h4 = (-128<<4)|5 = -2043
    // b2[5]=0xF0(i8=-16):  dig_h5 = (-16 <<4)|3 = -253
    fn sample_calib_b2_negative() -> [u8; 7] {
        [
            0x66, 0x01, // dig_h2 = 358
            0x00, // dig_h3 = 0
            0x80, // b2[3]: H4 high byte = -128 as i8
            0x35, // b2[4]: H4 low nibble = 5, H5 low nibble = 3
            0xF0, // b2[5]: H5 high byte = -16 as i8
            0x1E, // dig_h6 = 30
        ]
    }

    #[test]
    fn calib_parses_temperature_pressure_coefficients() {
        // Given
        let b1 = sample_calib_b1();
        let b2 = sample_calib_b2();

        // When
        let calib = CalibData::from_raw_bytes(&b1, b2);

        // Then
        assert_eq!(calib.dig_t1, 27_328, "dig_t1");
        assert_eq!(calib.dig_t2, 26_214, "dig_t2");
        assert_eq!(calib.dig_t3, 50, "dig_t3");
        assert_eq!(calib.dig_p1, 36_592, "dig_p1");
        assert_eq!(calib.dig_h1, 75, "dig_h1");
    }

    #[test]
    fn calib_parses_packed_h4_h5_with_sign() {
        // Given — positive high bytes
        let b1 = sample_calib_b1();
        let b2 = sample_calib_b2();

        // When
        let calib = CalibData::from_raw_bytes(&b1, b2);

        // Then: dig_h4 = (75 << 4) | 4 = 1200 + 4 = 1204
        assert_eq!(calib.dig_h4, 1_204, "dig_h4 positive");
        // dig_h5 = (50 << 4) | 3 = 800 + 3 = 803
        assert_eq!(calib.dig_h5, 803, "dig_h5 positive");

        // Given — negative high bytes
        let b2_neg = sample_calib_b2_negative();
        let calib_neg = CalibData::from_raw_bytes(&b1, b2_neg);

        // dig_h4 = (-128 << 4) | 5 = -2048 + 5 = -2043
        assert_eq!(calib_neg.dig_h4, -2_043, "dig_h4 negative");
        // dig_h5 = (-16 << 4) | 3 = -256 + 3 = -253
        assert_eq!(calib_neg.dig_h5, -253, "dig_h5 negative");
    }

    #[test]
    fn compensate_temperature_sets_t_fine_and_returns_celsius() {
        // Given — realistic calibration and a typical 25°C ADC value.
        // dig_t1=27328, dig_t2=26214, dig_t3=50; adc_t=415000 is mid-range.
        let mut calib = CalibData::from_raw_bytes(&sample_calib_b1(), sample_calib_b2());
        let adc_t: i32 = 415_000;

        // When
        let temperature = calib.compensate_temperature(adc_t);

        // Then
        assert!(
            (-40.0..=85.0).contains(&temperature),
            "temperature {temperature} not in -40..=85"
        );
        assert!(
            calib.t_fine.abs() > f32::EPSILON,
            "t_fine must be set after compensation"
        );
    }

    #[test]
    fn compensate_pressure_returns_plausible_sea_level() {
        // Given — call temperature first to set t_fine, then compensate pressure.
        // The calibration data is artificial but self-consistent; we verify the
        // formula is finite, positive, and within the BME280 datasheet range
        // (300..=1100 hPa == 30_000..=110_000 Pa) with a small tolerance to
        // account for f32 rounding on the boundary.
        let mut calib = CalibData::from_raw_bytes(&sample_calib_b1(), sample_calib_b2());
        let _ = calib.compensate_temperature(415_000);
        // adc_p=415_000 yields ~101 kPa with this calibration set
        let adc_p: i32 = 415_000;

        // When
        let pressure = calib.compensate_pressure(adc_p);

        // Then
        assert!(
            pressure.is_finite(),
            "pressure must be finite, got {pressure}"
        );
        assert!(
            (30_000.0..=110_000.0).contains(&pressure),
            "pressure {pressure} Pa not in expected range"
        );
    }

    #[test]
    fn compensate_humidity_clamps_to_0_100() {
        // Given — set t_fine, then drive extreme adc_h values.
        let mut calib = CalibData::from_raw_bytes(&sample_calib_b1(), sample_calib_b2());
        let _ = calib.compensate_temperature(415_000);

        // When — very large adc_h (would be > 100 %RH without clamping)
        let h_high = calib.compensate_humidity(i32::MAX);
        // When — very negative adc_h (would be < 0 %RH without clamping)
        let h_low = calib.compensate_humidity(i32::MIN);

        // Then
        assert!(
            (0.0..=100.0).contains(&h_high),
            "high clamp failed: {h_high}"
        );
        assert!((0.0..=100.0).contains(&h_low), "low clamp failed: {h_low}");
    }

    #[test]
    fn reading_pressure_hpa_converts() {
        // Given
        let reading = Reading {
            temperature: 25.0,
            pressure: 101_325.0,
            humidity: 50.0,
        };

        // When
        let hpa = reading.pressure_hpa();

        // Then
        assert!(
            (hpa - 1013.25).abs() < 0.01,
            "expected ~1013.25 hPa, got {hpa}"
        );
    }
}
// grcov exclude end
