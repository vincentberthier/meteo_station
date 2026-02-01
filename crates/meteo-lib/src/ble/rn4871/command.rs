//! Typed RN4871 command definitions.
//!
//! Each [`Command`] variant encodes a specific RN4871 operation, its wire
//! format, and the expected response type. This replaces raw byte slices with
//! a type-safe API that makes the protocol self-documenting.

/// The expected response type for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    /// Command returns `AOK` on success.
    Aok,
    /// Command returns a single data line.
    SingleLine,
    /// Command returns multiple data lines terminated by `CMD>`.
    MultiLine,
    /// Command triggers a module reboot (`%REBOOT%`).
    Reboot,
}

/// A typed command for the RN4871 BLE module.
///
/// Each variant knows its wire format (via [`write_to`](Command::write_to))
/// and expected response type (via [`response_type`](Command::response_type)).
///
/// # Examples
///
/// ```ignore
/// // Set the device name
/// driver.execute(Command::SetName("MeteoStation")).await?;
///
/// // Query firmware version
/// let n = driver.query(Command::GetFirmwareVersion, &mut buf).await?;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Set the device name (`SN,<name>`). Expects `AOK`.
    SetName(&'a str),
    /// Set feature bitmap (`SR,<hex>`). Expects `AOK`.
    SetFeatures(u16),
    /// Query firmware version (`V`). Returns a single data line.
    GetFirmwareVersion,
    /// Query device name (`GN`). Returns a single data line.
    GetDeviceName,
    /// Dump full device configuration (`D`). Returns multiple data lines.
    DumpConfig,
    /// Factory reset (`SF,1`). Module reboots after this command.
    ///
    /// Do NOT use with `execute()`/`query()` — use
    /// [`Rn4871::factory_reset`](super::super::Rn4871::factory_reset) instead,
    /// which handles the reboot sequence.
    #[cfg(feature = "factory-reset")]
    FactoryReset,
}

impl Command<'_> {
    /// Returns the expected response type for this command.
    #[must_use]
    pub const fn response_type(&self) -> ResponseType {
        match self {
            Self::SetName(_) | Self::SetFeatures(_) => ResponseType::Aok,
            Self::GetFirmwareVersion | Self::GetDeviceName => ResponseType::SingleLine,
            Self::DumpConfig => ResponseType::MultiLine,
            #[cfg(feature = "factory-reset")]
            Self::FactoryReset => ResponseType::Reboot,
        }
    }

    /// Writes the command bytes into `buf`, returning the number of bytes
    /// written.
    ///
    /// The trailing `\r` is NOT included — the driver appends it.
    ///
    /// Returns `None` if the buffer is too small.
    #[must_use]
    pub fn write_to(&self, buf: &mut [u8]) -> Option<usize> {
        let bytes: &[u8] = match self {
            Self::GetFirmwareVersion => b"V",
            Self::GetDeviceName => b"GN",
            Self::DumpConfig => b"D",
            #[cfg(feature = "factory-reset")]
            Self::FactoryReset => b"SF,1",
            Self::SetName(name) => {
                return write_prefixed(b"SN,", name.as_bytes(), buf);
            }
            Self::SetFeatures(bits) => {
                return write_features(*bits, buf);
            }
        };

        if buf.len() < bytes.len() {
            return None;
        }
        buf[..bytes.len()].copy_from_slice(bytes);
        Some(bytes.len())
    }
}

/// Writes `prefix` followed by `suffix` into `buf`.
/// Returns the total length, or `None` if the buffer is too small.
fn write_prefixed(prefix: &[u8], suffix: &[u8], buf: &mut [u8]) -> Option<usize> {
    let total = prefix.len().checked_add(suffix.len())?;
    if buf.len() < total {
        return None;
    }
    buf[..prefix.len()].copy_from_slice(prefix);
    buf[prefix.len()..total].copy_from_slice(suffix);
    Some(total)
}

/// Writes `SR,` followed by the hex representation of `bits` into `buf`.
/// Uses uppercase hex with no leading zeros (matches RN4871 expectations).
/// Returns the total length, or `None` if the buffer is too small.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "hex formatting: shift/mask on u16 are safe, pos tracks index within bounds"
)]
fn write_features(bits: u16, buf: &mut [u8]) -> Option<usize> {
    const PREFIX: &[u8] = b"SR,";

    // Format the u16 as uppercase hex without leading zeros.
    // Max is "FFFF" (4 chars), min is "0" (1 char).
    let mut hex_buf = [0_u8; 4];
    let mut hex_len = 0_usize;
    let mut started = false;

    for shift in [12_u8, 8, 4, 0] {
        let nibble = ((bits >> shift) & 0xF) as u8;
        if nibble != 0 || started || shift == 0 {
            hex_buf[hex_len] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'A' + nibble - 10
            };
            hex_len += 1;
            started = true;
        }
    }

    let total = PREFIX.len().checked_add(hex_len)?;
    if buf.len() < total {
        return None;
    }
    buf[..PREFIX.len()].copy_from_slice(PREFIX);
    buf[PREFIX.len()..total].copy_from_slice(&hex_buf[..hex_len]);
    Some(total)
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::{error, result};

    use std::boxed::Box;
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // --- response_type tests ---

    #[test]
    fn set_name_expects_aok() -> TestResult {
        // Given
        let cmd = Command::SetName("Test");

        // When
        let rt = cmd.response_type();

        // Then
        assert_eq!(rt, ResponseType::Aok, "SetName should expect AOK");
        Ok(())
    }

    #[test]
    fn set_features_expects_aok() -> TestResult {
        // Given
        let cmd = Command::SetFeatures(0x2000);

        // When
        let rt = cmd.response_type();

        // Then
        assert_eq!(rt, ResponseType::Aok, "SetFeatures should expect AOK");
        Ok(())
    }

    #[test]
    fn get_firmware_version_expects_single_line() -> TestResult {
        // Given
        let cmd = Command::GetFirmwareVersion;

        // When
        let rt = cmd.response_type();

        // Then
        assert_eq!(
            rt,
            ResponseType::SingleLine,
            "GetFirmwareVersion should expect SingleLine"
        );
        Ok(())
    }

    #[test]
    fn get_device_name_expects_single_line() -> TestResult {
        // Given
        let cmd = Command::GetDeviceName;

        // When
        let rt = cmd.response_type();

        // Then
        assert_eq!(
            rt,
            ResponseType::SingleLine,
            "GetDeviceName should expect SingleLine"
        );
        Ok(())
    }

    #[test]
    fn dump_config_expects_multi_line() -> TestResult {
        // Given
        let cmd = Command::DumpConfig;

        // When
        let rt = cmd.response_type();

        // Then
        assert_eq!(
            rt,
            ResponseType::MultiLine,
            "DumpConfig should expect MultiLine"
        );
        Ok(())
    }

    // --- write_to tests ---

    #[test]
    fn write_get_firmware_version() -> TestResult {
        // Given
        let cmd = Command::GetFirmwareVersion;
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"V", "should write V");
        Ok(())
    }

    #[test]
    fn write_get_device_name() -> TestResult {
        // Given
        let cmd = Command::GetDeviceName;
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"GN", "should write GN");
        Ok(())
    }

    #[test]
    fn write_dump_config() -> TestResult {
        // Given
        let cmd = Command::DumpConfig;
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"D", "should write D");
        Ok(())
    }

    #[test]
    fn write_set_name() -> TestResult {
        // Given
        let cmd = Command::SetName("MeteoStation");
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(
            &buf[..n],
            b"SN,MeteoStation",
            "should write SN,MeteoStation"
        );
        Ok(())
    }

    #[test]
    fn write_set_features_typical() -> TestResult {
        // Given
        let cmd = Command::SetFeatures(0x2000);
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"SR,2000", "should write SR,2000");
        Ok(())
    }

    #[test]
    fn write_set_features_zero() -> TestResult {
        // Given
        let cmd = Command::SetFeatures(0);
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"SR,0", "should write SR,0 for zero");
        Ok(())
    }

    #[test]
    fn write_set_features_max() -> TestResult {
        // Given
        let cmd = Command::SetFeatures(0xFFFF);
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"SR,FFFF", "should write SR,FFFF for max");
        Ok(())
    }

    #[test]
    fn write_set_features_lowercase_check() -> TestResult {
        // Given — 0xABCD should produce uppercase
        let cmd = Command::SetFeatures(0xABCD);
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"SR,ABCD", "should use uppercase hex");
        Ok(())
    }

    #[test]
    fn write_to_returns_none_on_small_buffer() -> TestResult {
        // Given
        let cmd = Command::SetName("MeteoStation");
        let mut buf = [0_u8; 3]; // too small for "SN,MeteoStation"

        // When
        let result = cmd.write_to(&mut buf);

        // Then
        assert!(result.is_none(), "should return None for small buffer");
        Ok(())
    }

    #[test]
    fn write_set_features_single_digit() -> TestResult {
        // Given
        let cmd = Command::SetFeatures(0x5);
        let mut buf = [0_u8; 32];

        // When
        let n = cmd
            .write_to(&mut buf)
            .expect("buffer should be large enough");

        // Then
        assert_eq!(&buf[..n], b"SR,5", "should not pad with leading zeros");
        Ok(())
    }
}
// grcov exclude stop
