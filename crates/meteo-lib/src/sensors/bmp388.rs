use embedded_hal_async::i2c::I2c;

// BMP388 Register addresses
const CHIP_ID_REG: u8 = 0x00;
const STATUS: u8 = 0x03;
const PWR_CTRL: u8 = 0x1B;
const PRESS_MSB: u8 = 0x04;
const TEMP_MSB: u8 = 0x07;
const EXPECTED_CHIP_ID: u8 = 0x50;
const CALIB_DATA: u8 = 0x31;

// PWR_CTRL value: press_en (bit 0) | temp_en (bit 1) | forced mode (bits 5:4 = 01).
// Forced mode takes exactly one measurement per trigger, then returns to sleep,
// so the sampling rate is driven solely by how often we re-trigger (1 Hz here)
// instead of the sensor free-running at its 200 Hz normal-mode ODR.
const PWR_CTRL_FORCED: u8 = 0b0001_0011;

// STATUS register data-ready flags: drdy_press (bit 5), drdy_temp (bit 6).
const DRDY_PRESS: u8 = 1 << 5;
const DRDY_TEMP: u8 = 1 << 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    I2c(E),
    WrongChipId(u8),
}

pub struct Bmp388<I> {
    i2c: I,
    address: u8,
    calib: CalibData,
}

impl<I, E> Bmp388<I>
where
    I: I2c<Error = E>,
{
    /// Creates a new BMP388 driver instance.
    ///
    /// Verifies the chip ID and reads the factory calibration data. The sensor
    /// is left in its power-on sleep state; each [`read`](Self::read) triggers a
    /// single forced-mode measurement, so the sampling rate is set entirely by
    /// how often the caller reads (no free-running normal-mode sampling).
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if communication fails, or `Error::WrongChipId`
    /// if the chip doesn't identify as a BMP388.
    pub async fn new(mut i2c: I, address: u8) -> Result<Self, Error<E>> {
        // Verify chip ID
        let mut chip_id = [0_u8; 1];
        i2c.write_read(address, &[CHIP_ID_REG], &mut chip_id)
            .await
            .map_err(Error::I2c)?;

        if chip_id[0] != EXPECTED_CHIP_ID {
            return Err(Error::WrongChipId(chip_id[0]));
        }

        // Read calibration data
        let mut calib_raw = [0_u8; 21];
        i2c.write_read(address, &[CALIB_DATA], &mut calib_raw)
            .await
            .map_err(Error::I2c)?;

        let calib = CalibData::from_raw_bytes(&calib_raw);

        Ok(Self {
            i2c,
            address,
            calib,
        })
    }

    /// Triggers a single forced-mode measurement and returns the compensated
    /// temperature and pressure.
    ///
    /// Writing `PWR_CTRL` starts one measurement; the sensor then returns to
    /// sleep on its own. We wait for completion by polling the `STATUS`
    /// data-ready flags (no fixed delay), so the wait ends the moment the
    /// conversion is done regardless of the configured oversampling.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if communication with the sensor fails.
    pub async fn read(&mut self) -> Result<Reading, Error<E>> {
        // Trigger one measurement.
        self.i2c
            .write(self.address, &[PWR_CTRL, PWR_CTRL_FORCED])
            .await
            .map_err(Error::I2c)?;

        // Wait for the conversion to finish by polling the data-ready flags.
        // Each I2C transaction awaits, yielding to the executor between polls.
        loop {
            let mut status = [0_u8; 1];
            self.i2c
                .write_read(self.address, &[STATUS], &mut status)
                .await
                .map_err(Error::I2c)?;
            if status[0] & (DRDY_PRESS | DRDY_TEMP) == (DRDY_PRESS | DRDY_TEMP) {
                break;
            }
        }

        let mut press_data = [0_u8; 3];
        self.i2c
            .write_read(self.address, &[PRESS_MSB], &mut press_data)
            .await
            .map_err(Error::I2c)?;

        let press_raw = (u32::from(press_data[2]) << 16_i32)
            | (u32::from(press_data[1]) << 8_i32)
            | u32::from(press_data[0]);

        let mut temp_data = [0_u8; 3];
        self.i2c
            .write_read(self.address, &[TEMP_MSB], &mut temp_data)
            .await
            .map_err(Error::I2c)?;

        let temp_raw = (u32::from(temp_data[2]) << 16_i32)
            | (u32::from(temp_data[1]) << 8_i32)
            | u32::from(temp_data[0]);

        let temperature = self.calib.compensate_temperature(temp_raw);
        let pressure = self.calib.compensate_pressure(press_raw);

        Ok(Reading {
            temperature,
            pressure,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    /// Temperature in degrees Celsius
    pub temperature: f32,
    /// Pressure in Pascals
    pub pressure: f32,
}

impl Reading {
    #[must_use]
    pub fn pressure_hpa(&self) -> f32 {
        self.pressure / 100.0
    }
}

struct CalibData {
    par_t1: f32,
    par_t2: f32,
    par_t3: f32,
    par_p1: f32,
    par_p2: f32,
    par_p3: f32,
    par_p4: f32,
    par_p5: f32,
    par_p6: f32,
    par_p7: f32,
    par_p8: f32,
    par_p9: f32,
    par_p10: f32,
    par_p11: f32,
    t_lin: f32,
}

impl CalibData {
    #[expect(
        clippy::similar_names,
        reason = "names match Bosch BMP388 datasheet nomenclature"
    )]
    #[expect(
        clippy::little_endian_bytes,
        reason = "BMP388 stores calibration data in little-endian"
    )]
    #[expect(
        clippy::lossy_float_literal,
        reason = "constants from Bosch reference compensation code"
    )]
    fn from_raw_bytes(data: &[u8; 21]) -> Self {
        let nvm_par_t1 = u16::from_le_bytes([data[0], data[1]]);
        let nvm_par_t2 = u16::from_le_bytes([data[2], data[3]]);
        let nvm_par_t3 = i8::from_le_bytes([data[4]]);

        let nvm_par_p1 = i16::from_le_bytes([data[5], data[6]]);
        let nvm_par_p2 = i16::from_le_bytes([data[7], data[8]]);
        let nvm_par_p3 = i8::from_le_bytes([data[9]]);
        let nvm_par_p4 = i8::from_le_bytes([data[10]]);
        let nvm_par_p5 = u16::from_le_bytes([data[11], data[12]]);
        let nvm_par_p6 = u16::from_le_bytes([data[13], data[14]]);
        let nvm_par_p7 = i8::from_le_bytes([data[15]]);
        let nvm_par_p8 = i8::from_le_bytes([data[16]]);
        let nvm_par_p9 = i16::from_le_bytes([data[17], data[18]]);
        let nvm_par_p10 = i8::from_le_bytes([data[19]]);
        let nvm_par_p11 = i8::from_le_bytes([data[20]]);

        Self {
            par_t1: f32::from(nvm_par_t1) * 256.0,
            par_t2: f32::from(nvm_par_t2) / 1_073_741_824.0,
            par_t3: f32::from(nvm_par_t3) / 281_474_976_710_656.0,
            par_p1: (f32::from(nvm_par_p1) - 16384.0) / 1_048_576.0,
            par_p2: (f32::from(nvm_par_p2) - 16384.0) / 536_870_912.0,
            par_p3: f32::from(nvm_par_p3) / 4_294_967_296.0,
            par_p4: f32::from(nvm_par_p4) / 137_438_953_472.0,
            par_p5: f32::from(nvm_par_p5) / 0.125,
            par_p6: f32::from(nvm_par_p6) / 64.0,
            par_p7: f32::from(nvm_par_p7) / 256.0,
            par_p8: f32::from(nvm_par_p8) / 32768.0,
            par_p9: f32::from(nvm_par_p9) / 281_474_976_710_656.0,
            par_p10: f32::from(nvm_par_p10) / 281_474_976_710_656.0,
            par_p11: f32::from(nvm_par_p11) / 36_893_488_147_419_103_232.0,
            t_lin: 0.0,
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "u32 ADC value intentionally cast to f32 for Bosch compensation formula"
    )]
    fn compensate_temperature(&mut self, uncomp_temp: u32) -> f32 {
        let partial_data1 = uncomp_temp as f32 - self.par_t1;
        let partial_data2 = partial_data1 * self.par_t2;
        self.t_lin = partial_data2 + (partial_data1 * partial_data1) * self.par_t3;
        self.t_lin
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "u32 ADC value intentionally cast to f32 for Bosch compensation formula"
    )]
    #[expect(
        clippy::shadow_unrelated,
        reason = "partial_data re-bindings follow Bosch reference algorithm stages"
    )]
    fn compensate_pressure(&self, uncomp_press: u32) -> f32 {
        let partial_data1 = self.par_p6 * self.t_lin;
        let partial_data2 = self.par_p7 * (self.t_lin * self.t_lin);
        let partial_data3 = self.par_p8 * (self.t_lin * self.t_lin * self.t_lin);
        let partial_out1 = self.par_p5 + partial_data1 + partial_data2 + partial_data3;

        let partial_data1 = self.par_p2 * self.t_lin;
        let partial_data2 = self.par_p3 * (self.t_lin * self.t_lin);
        let partial_data3 = self.par_p4 * (self.t_lin * self.t_lin * self.t_lin);
        let partial_out2 =
            (uncomp_press as f32) * (self.par_p1 + partial_data1 + partial_data2 + partial_data3);

        let partial_data1 = (uncomp_press as f32) * (uncomp_press as f32);
        let partial_data2 = self.par_p9 + self.par_p10 * self.t_lin;
        let partial_data3 = partial_data1 * partial_data2;
        let partial_data4 = partial_data3
            + ((uncomp_press as f32) * (uncomp_press as f32) * (uncomp_press as f32))
                * self.par_p11;

        partial_out1 + partial_out2 + partial_data4
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    /// Sample calibration data (realistic values based on BMP388 datasheet ranges).
    fn sample_calib_bytes() -> [u8; 21] {
        [
            0x00, 0x80, // nvm_par_t1 = 32768
            0x00, 0x40, // nvm_par_t2 = 16384
            0xF0, // nvm_par_t3 = -16
            0x00, 0x10, // nvm_par_p1 = 4096
            0x00, 0x08, // nvm_par_p2 = 2048
            0x00, // nvm_par_p3 = 0
            0x00, // nvm_par_p4 = 0
            0x00, 0x80, // nvm_par_p5 = 32768
            0x00, 0x40, // nvm_par_p6 = 16384
            0x00, // nvm_par_p7 = 0
            0x00, // nvm_par_p8 = 0
            0x00, 0x00, // nvm_par_p9 = 0
            0x00, // nvm_par_p10 = 0
            0x00, // nvm_par_p11 = 0
        ]
    }

    #[test]
    fn calib_data_from_raw_bytes_parses_temperature_coefficients() {
        // Given
        let raw = sample_calib_bytes();

        // When
        let calib = CalibData::from_raw_bytes(&raw);

        // Then
        // par_t1 = 32768 * 256 = 8388608
        assert!((calib.par_t1 - 8_388_608.0).abs() < 0.1);
        // par_t2 = 16384 / 2^30
        assert!((calib.par_t2 - 1.525_878_9e-5).abs() < 1e-10);
        // par_t3 = -16 / 2^48
        assert!(calib.par_t3 < 0.0);
    }

    #[test]
    fn calib_data_from_raw_bytes_parses_pressure_coefficients() {
        // Given
        let raw = sample_calib_bytes();

        // When
        let calib = CalibData::from_raw_bytes(&raw);

        // Then
        // par_p5 = 32768 / 0.125 = 262144
        assert!((calib.par_p5 - 262_144.0).abs() < 0.1);
        // par_p6 = 16384 / 64 = 256
        assert!((calib.par_p6 - 256.0).abs() < 0.1);
    }

    #[test]
    fn compensate_temperature_returns_reasonable_value() {
        // Given
        let raw = sample_calib_bytes();
        let mut calib = CalibData::from_raw_bytes(&raw);
        // Raw temperature value (typical ADC output around 25°C)
        let uncomp_temp: u32 = 8_000_000;

        // When
        let temperature = calib.compensate_temperature(uncomp_temp);

        // Then
        // Temperature should be in a reasonable range (-40 to 85°C for BMP388)
        assert!(temperature > -50.0 && temperature < 100.0);
        // t_lin should be set
        assert!((calib.t_lin - temperature).abs() < 0.001);
    }

    #[test]
    fn compensate_pressure_returns_finite_value() {
        // Given
        let raw = sample_calib_bytes();
        let mut calib = CalibData::from_raw_bytes(&raw);
        // First compute temperature to set t_lin
        let uncomp_temp: u32 = 8_000_000;
        let _ = calib.compensate_temperature(uncomp_temp);
        // Raw pressure value
        let uncomp_press: u32 = 6_000_000;

        // When
        let pressure = calib.compensate_pressure(uncomp_press);

        // Then
        // Pressure should be finite (not NaN or infinite)
        assert!(pressure.is_finite(), "Pressure should be finite");
        // Pressure should be positive (physical constraint)
        assert!(pressure > 0.0, "Pressure should be positive");
    }

    #[test]
    fn reading_pressure_hpa_converts_correctly() {
        // Given
        let reading = Reading {
            temperature: 25.0,
            pressure: 101_325.0, // 1 atm in Pa
        };

        // When
        let hpa = reading.pressure_hpa();

        // Then
        assert!((hpa - 1013.25).abs() < 0.01);
    }

    #[test]
    fn reading_pressure_hpa_handles_low_pressure() {
        // Given
        let reading = Reading {
            temperature: 20.0,
            pressure: 30_000.0, // Low pressure (high altitude)
        };

        // When
        let hpa = reading.pressure_hpa();

        // Then
        assert!((hpa - 300.0).abs() < 0.01);
    }
}
// grcov exclude stop
