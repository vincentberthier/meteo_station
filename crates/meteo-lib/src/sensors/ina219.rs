//! INA219 high-side current / bus-voltage monitor driver.
//!
//! Two of these sit on the shared I2C0 bus: U6 @ `0x40` on the PV feed
//! (panel voltage + harvest current) and U7 @ `0x41` on the battery→boost feed
//! (battery voltage + load current). Both use the on-board 0.1 Ω shunt.
//!
//! All INA219 registers are 16-bit and transmitted **MSB-first** (big-endian),
//! unlike the little-endian VEML7700.
//!
//! # Conversion constants
//!
//! With the 0.1 Ω shunt and a chosen `Current_LSB` of 100 µA (0.1 mA):
//!
//! ```text
//! cal = trunc(0.04096 / (Current_LSB × Rshunt))
//!     = trunc(0.04096 / (0.0001 × 0.1)) = 4096
//! ```
//!
//! - Bus-voltage register (0x02): value in bits 15:3, LSB = 4 mV.
//! - Current register (0x04): signed, LSB = `Current_LSB` = 0.1 mA.

use embedded_hal_async::i2c::I2c;

/// Configuration register (R/W).
const REG_CONFIG: u8 = 0x00;
/// Bus-voltage register (R).
const REG_BUS_VOLTAGE: u8 = 0x02;
/// Current register (R) — valid only after the calibration register is set.
const REG_CURRENT: u8 = 0x04;
/// Calibration register (R/W).
const REG_CALIBRATION: u8 = 0x05;

/// Config: 32 V bus range, ÷8 PGA (±320 mV shunt), **128-sample averaging** on both
/// the bus and shunt ADCs (BADC = SADC = 0b1111, ~68 ms/conversion), shunt-and-bus
/// continuous mode.
///
/// Averaging is the key bit: the battery node is fed by the MT3608 boost converter,
/// which draws current in switching pulses, so a single 12-bit conversion (the
/// power-on default 0x399F) aliases that ripple and the reading jumps ±100+ mV /
/// hundreds of mA between 1 Hz samples. 128-sample hardware averaging reports the
/// mean over ~68 ms — a stable value — without us guessing at a settle delay.
const CONFIG: u16 = 0x3FFF;

/// Calibration for a 0.1 Ω shunt with `Current_LSB` = 0.1 mA (see module docs).
const CALIBRATION: u16 = 4096;

/// Driver error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// I2C bus error.
    I2c(E),
}

/// One INA219 sample: bus voltage and current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Reading {
    /// Bus voltage in millivolts (always ≥ 0).
    pub bus_mv: u16,
    /// Current in milliamps; positive = flowing into Vin+ (harvest / load draw).
    pub current_ma: i16,
}

/// Converts the raw bus-voltage register value to millivolts.
///
/// The measurement lives in bits 15:3; the low 3 bits are status/flags. Each
/// step is 4 mV. Max value (8191 × 4 = 32 764 mV) fits in `u16`.
#[must_use]
pub const fn bus_mv_from_raw(raw: u16) -> u16 {
    (raw >> 3).saturating_mul(4)
}

/// Converts the raw (signed) current register value to milliamps.
///
/// With `Current_LSB` = 0.1 mA, milliamps = raw ÷ 10.
#[must_use]
pub const fn current_ma_from_raw(raw: i16) -> i16 {
    raw / 10
}

/// INA219 current / bus-voltage monitor driver.
pub struct Ina219<I> {
    i2c: I,
    address: u8,
}

impl<I, E> Ina219<I>
where
    I: I2c<Error = E>,
{
    /// Creates a new INA219 driver instance.
    ///
    /// Does not communicate with the device; call [`init`](Self::init) before
    /// the first [`read`](Self::read).
    #[must_use]
    pub const fn new(i2c: I, address: u8) -> Self {
        Self { i2c, address }
    }

    /// Writes the configuration and calibration registers.
    ///
    /// The calibration register must be set for the current register to report
    /// meaningful values.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if either register write fails.
    pub async fn init(&mut self) -> Result<(), Error<E>> {
        self.write_reg(REG_CONFIG, CONFIG).await?;
        self.write_reg(REG_CALIBRATION, CALIBRATION).await
    }

    /// Reads the bus voltage and current.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` if either register read fails.
    pub async fn read(&mut self) -> Result<Reading, Error<E>> {
        let bus_raw = self.read_reg(REG_BUS_VOLTAGE).await?;
        let current_raw = self.read_reg(REG_CURRENT).await?;
        Ok(Reading {
            bus_mv: bus_mv_from_raw(bus_raw),
            #[expect(
                clippy::cast_possible_wrap,
                reason = "current register is a two's-complement i16 transmitted as u16"
            )]
            current_ma: current_ma_from_raw(current_raw as i16),
        })
    }

    /// Writes a 16-bit register, MSB-first.
    async fn write_reg(&mut self, reg: u8, value: u16) -> Result<(), Error<E>> {
        let [hi, lo] = value.to_be_bytes();
        self.i2c
            .write(self.address, &[reg, hi, lo])
            .await
            .map_err(Error::I2c)
    }

    /// Reads a 16-bit register, MSB-first.
    async fn read_reg(&mut self, reg: u8) -> Result<u16, Error<E>> {
        let mut buf = [0_u8; 2];
        self.i2c
            .write_read(self.address, &[reg], &mut buf)
            .await
            .map_err(Error::I2c)?;
        Ok(u16::from_be_bytes(buf))
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn bus_voltage_drops_low_three_status_bits() {
        // Given — raw with value 0x0FA0 in bits 15:3 plus status bits set low.
        // 0x6FA8 = 0b0110_1111_1010_1000; >>3 = 0b0110_1111_1010_1 = 0x0DF5 = 3573 steps
        // (the low 3 bits 0b000 are dropped). 3573 * 4 = 14292 mV.
        let raw = 0x6FA8;

        // When
        let mv = bus_mv_from_raw(raw);

        // Then
        assert_eq!(mv, 14_292);
    }

    #[test]
    fn bus_voltage_4mv_per_step() {
        // Given — value 1 in bits 15:3 is raw 0b1000 = 8
        // When / Then
        assert_eq!(bus_mv_from_raw(0b1000), 4);
        assert_eq!(bus_mv_from_raw(0), 0);
    }

    #[test]
    fn bus_voltage_max_fits_u16() {
        // Given — all value bits set (u16::MAX): 8191 steps × 4 = 32764 mV
        // When / Then — 32_764 == 0x7FFC
        assert_eq!(bus_mv_from_raw(u16::MAX), 0x7FFC);
    }

    #[test]
    fn current_positive_scaled_by_tenth() {
        // Given — raw 4096 with Current_LSB 0.1 mA → 409.6 mA, truncated to 409
        // When / Then
        assert_eq!(current_ma_from_raw(4096), 409);
        assert_eq!(current_ma_from_raw(10), 1);
        assert_eq!(current_ma_from_raw(0), 0);
    }

    #[test]
    fn current_handles_negative_two_complement() {
        // Given — a reverse current reads negative
        // When / Then
        assert_eq!(current_ma_from_raw(-4096), -409);
        assert_eq!(current_ma_from_raw(-10), -1);
    }
}
// grcov exclude stop
