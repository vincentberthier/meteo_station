//! GATT service and characteristic definitions for the `MeteoStation`.
//!
//! Defines custom 128-bit UUIDs, property constants, and handle tracking for
//! the weather data GATT service.

use super::rn4871::ls_parser;

/// `MeteoStation` custom service UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a00`.
pub const METEO_SERVICE_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A,
    0x00,
];

/// Temperature characteristic UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a01`.
pub const TEMPERATURE_CHAR_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A,
    0x01,
];

/// Pressure characteristic UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a02`.
pub const PRESSURE_CHAR_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A,
    0x02,
];

/// BLE GATT property: read (0x02).
pub const PROP_READ: u8 = 0x02;

/// BLE GATT property: notify (0x10).
pub const PROP_NOTIFY: u8 = 0x10;

/// BLE GATT property: read + notify (0x12).
pub const PROP_READ_NOTIFY: u8 = 0x12;

/// Data size for an f32 value (4 bytes).
pub const F32_SIZE: u8 = 4;

/// Discovered GATT characteristic handles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GattHandles {
    /// Handle for the temperature characteristic, if discovered.
    pub temperature: Option<u16>,
    /// Handle for the pressure characteristic, if discovered.
    pub pressure: Option<u16>,
}

/// Callback for `query_multiline(ListServices, ...)`.
///
/// Matches characteristic UUIDs from LS output lines and stores their handles.
pub fn collect_handles(line: &[u8], handles: &mut GattHandles) {
    if let Some(info) = ls_parser::parse_characteristic_line(line) {
        if info.uuid_bytes == TEMPERATURE_CHAR_UUID {
            handles.temperature = Some(info.handle);
        } else if info.uuid_bytes == PRESSURE_CHAR_UUID {
            handles.pressure = Some(info.handle);
        } else {
            // Unknown characteristic — ignore
        }
    }
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

    #[test]
    fn collect_handles_temperature() -> TestResult {
        // Given
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,12";
        let mut handles = GattHandles::default();

        // When
        collect_handles(line, &mut handles);

        // Then
        assert_eq!(
            handles.temperature,
            Some(0x0072),
            "temperature handle should be set"
        );
        assert_eq!(handles.pressure, None, "pressure should still be None");
        Ok(())
    }

    #[test]
    fn collect_handles_pressure() -> TestResult {
        // Given
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,12";
        let mut handles = GattHandles::default();

        // When
        collect_handles(line, &mut handles);

        // Then
        assert_eq!(
            handles.pressure,
            Some(0x0075),
            "pressure handle should be set"
        );
        assert_eq!(
            handles.temperature, None,
            "temperature should still be None"
        );
        Ok(())
    }

    #[test]
    fn collect_handles_both() -> TestResult {
        // Given — simulate two LS output lines
        let temp_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,12";
        let pres_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,12";
        let mut handles = GattHandles::default();

        // When
        collect_handles(temp_line, &mut handles);
        collect_handles(pres_line, &mut handles);

        // Then
        assert_eq!(handles.temperature, Some(0x0072), "temperature handle");
        assert_eq!(handles.pressure, Some(0x0075), "pressure handle");
        Ok(())
    }

    #[test]
    fn collect_handles_ignores_service_uuid_line() -> TestResult {
        // Given — service UUID line (no leading whitespace)
        let line = b"A4E64B8B8DB34E08A7D57D3C3F2E1A00";
        let mut handles = GattHandles::default();

        // When
        collect_handles(line, &mut handles);

        // Then
        assert_eq!(handles.temperature, None, "should not match service UUID");
        assert_eq!(handles.pressure, None, "should not match service UUID");
        Ok(())
    }

    #[test]
    fn collect_handles_ignores_unknown_uuid() -> TestResult {
        // Given — characteristic with unknown UUID
        let line = b"  00112233445566778899AABBCCDDEEFF,0080,02";
        let mut handles = GattHandles::default();

        // When
        collect_handles(line, &mut handles);

        // Then
        assert_eq!(handles.temperature, None, "unknown UUID should be ignored");
        assert_eq!(handles.pressure, None, "unknown UUID should be ignored");
        Ok(())
    }
}
// grcov exclude stop
