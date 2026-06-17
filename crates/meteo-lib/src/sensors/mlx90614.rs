// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive"
)]

//! MLX90614 IR thermometer driver — `SMBus` Read-Word with PEC (CRC-8) verification.
//!
//! The MLX90614 is a contactless infrared temperature sensor that communicates
//! over `SMBus` (I²C-compatible). Each read returns a 16-bit word followed by a
//! Packet Error Code (PEC) byte computed with CRC-8 polynomial 0x07.
//!
//! # Usage
//!
//! ```no_run
//! # use meteo_lib::sensors::mlx90614::{Mlx90614, Error};
//! # async fn example<I, E>(i2c: I) -> Result<(), Error<E>>
//! # where
//! #     I: embedded_hal_async::i2c::I2c<Error = E>,
//! # {
//! use meteo_lib::sensors::mlx90614::Mlx90614;
//!
//! let mut sensor = Mlx90614::new(i2c, 0x5A);
//! let t_obj = sensor.object_temperature().await?;
//! let t_amb = sensor.ambient_temperature().await?;
//! #     Ok(())
//! # }
//! ```

use embedded_hal_async::i2c::I2c;

/// RAM opcode: linearized ambient temperature (TA).
const RAM_TA: u8 = 0x06;
/// RAM opcode: linearized object temperature from sensor 1 (TOBJ1).
const RAM_TOBJ1: u8 = 0x07;

/// `SMBus` PEC CRC-8 polynomial X⁸+X²+X¹+1 (0x07), init 0x00.
const PEC_POLY: u8 = 0x07;

/// MLX90614 error-flag mask: RAM bit 15 set → reading invalid.
const ERROR_FLAG: u16 = 0x8000;

/// Errors that can occur when communicating with the MLX90614.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// Underlying I2C/`SMBus` transport error.
    I2c(E),
    /// PEC (CRC-8) mismatch: the reading is corrupt.
    Pec {
        /// The PEC byte we computed from the received data.
        expected: u8,
        /// The PEC byte returned by the sensor.
        got: u8,
    },
    /// RAM bit 15 was set: the sensor flagged the reading invalid.
    ErrorFlag,
}

/// MLX90614 IR thermometer driver.
///
/// Communicates over `SMBus` using `embedded-hal-async` I²C. The driver has no
/// init sequence — presence is established by the first successful read.
pub struct Mlx90614<I> {
    i2c: I,
    address: u8,
}

impl<I, E> Mlx90614<I>
where
    I: I2c<Error = E>,
{
    /// Creates a driver bound to `address` (no bus traffic; the MLX90614 has no
    /// chip-ID register — presence is established by the first successful read).
    #[must_use]
    pub const fn new(i2c: I, address: u8) -> Self {
        Self { i2c, address }
    }

    /// Object (IR) temperature in °C from RAM `0x07` (TOBJ1).
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` on transport failure, `Error::Pec` on CRC mismatch,
    /// `Error::ErrorFlag` if the sensor flags the reading invalid.
    pub async fn object_temperature(&mut self) -> Result<f32, Error<E>> {
        let raw = self.read_ram(RAM_TOBJ1).await?;
        temperature_from_raw(raw).ok_or(Error::ErrorFlag)
    }

    /// Ambient (TA) temperature in °C from RAM `0x06`.
    ///
    /// Used as the occlusion health proxy; never reported as a telemetry
    /// temperature field.
    ///
    /// # Errors
    ///
    /// Returns `Error::I2c` on transport failure, `Error::Pec` on CRC mismatch,
    /// `Error::ErrorFlag` if the sensor flags the reading invalid.
    pub async fn ambient_temperature(&mut self) -> Result<f32, Error<E>> {
        let raw = self.read_ram(RAM_TA).await?;
        temperature_from_raw(raw).ok_or(Error::ErrorFlag)
    }

    /// `SMBus` Read-Word of a RAM cell with PEC verification.
    ///
    /// The `SMBus` Read-Word transaction is:
    /// `S [SA_W] [Cmd] Sr [SA_R] [LSByte] [MSByte] [PEC] P`
    ///
    /// `write_read` realises the combined write-then-read framing over
    /// `embedded-hal-async`, delivering 3 bytes: LSB, MSB, PEC.
    async fn read_ram(&mut self, command: u8) -> Result<u16, Error<E>> {
        let mut buf = [0_u8; 3]; // [LSB, MSB, PEC]
        self.i2c
            .write_read(self.address, &[command], &mut buf)
            .await
            .map_err(Error::I2c)?;
        let lsb = buf[0];
        let msb = buf[1];
        let pec = buf[2];
        let expected = pec_for_read(self.address, command, lsb, msb);
        if expected != pec {
            return Err(Error::Pec { expected, got: pec });
        }
        Ok(u16::from_le_bytes([lsb, msb]))
    }
}

/// `SMBus` PEC: CRC-8 with polynomial 0x07, init 0x00, no reflection, no final XOR.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8_u8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ PEC_POLY
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Compute the PEC over a Read-Word transaction: `[SA_W, command, SA_R, LSByte, MSByte]`.
///
/// The `SMBus` address is shifted left by one for the wire format:
/// `SA_W = address << 1`, `SA_R = (address << 1) | 1`.
fn pec_for_read(address: u8, command: u8, lsb: u8, msb: u8) -> u8 {
    let sa_w = address << 1;
    let sa_r = (address << 1) | 1;
    crc8(&[sa_w, command, sa_r, lsb, msb])
}

/// Convert a raw 16-bit RAM temperature word to °C, or `None` if the error
/// flag (bit 15) is set.
///
/// Formula from MLX90614 datasheet: `T_K = raw × 0.02`, then `T_C = T_K − 273.15`.
fn temperature_from_raw(raw: u16) -> Option<f32> {
    if raw & ERROR_FLAG != 0 {
        return None;
    }
    Some(f32::from(raw) * 0.02 - 273.15)
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn crc8_matches_smbus_check_vector() {
        // Given
        // The canonical CRC-8/SMBus check value for the ASCII string "123456789"
        // is 0xF4 (defined in the SMBus specification).

        // When
        let result = crc8(b"123456789");

        // Then
        assert_eq!(result, 0xF4);
    }

    #[test]
    fn crc8_empty_is_zero() {
        // Given / When
        let result = crc8(&[]);

        // Then
        assert_eq!(result, 0x00);
    }

    #[test]
    fn pec_for_read_uses_shifted_addresses() {
        // Given
        // For address 0x5A: SA_W = 0xB4, SA_R = 0xB5
        let address = 0x5A_u8;
        let command = RAM_TOBJ1; // 0x07

        // When
        let result = pec_for_read(address, command, 0x00, 0x00);
        let expected = crc8(&[0xB4, command, 0xB5, 0x00, 0x00]);

        // Then
        assert_eq!(result, expected);
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test: value known to be Some")]
    fn temperature_from_raw_converts_object_temp() {
        // Given
        // raw = 0x39CE = 14798 decimal
        // T = 14798 × 0.02 − 273.15 = 295.96 − 273.15 = 22.81 °C
        let raw: u16 = 0x39CE;

        // When
        let result = temperature_from_raw(raw);

        // Then
        let v = result.expect("should be Some");
        assert!((v - 22.81).abs() < 0.01, "expected ≈22.81, got {v}");
    }

    #[test]
    fn temperature_from_raw_rejects_error_flag() {
        // Given
        // Bit 15 set → sensor error flag

        // When
        let result = temperature_from_raw(0x8000);

        // Then
        assert_eq!(result, None);
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test: value known to be Some")]
    fn temperature_from_raw_handles_zero_celsius() {
        // Given
        // 0 °C → T_K = 273.15 → raw = 273.15 / 0.02 = 13657.5, truncate to 13657
        // T = 13657 × 0.02 − 273.15 = 273.14 − 273.15 = −0.01 °C
        let raw: u16 = 13657; // 0x3559

        // When
        let result = temperature_from_raw(raw);

        // Then
        let v = result.expect("should be Some");
        assert!(v.is_finite(), "temperature should be finite, got {v}");
        assert!(
            (-1.0..=1.0).contains(&v),
            "temperature should be near 0°C, got {v}"
        );
    }
}
// grcov exclude stop
