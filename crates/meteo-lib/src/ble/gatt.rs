//! GATT service and characteristic definitions for the `MeteoStation`.
//!
//! Defines custom 128-bit UUIDs, property constants, and handle tracking for
//! the weather data GATT service.

use super::rn4871::ls_parser;

/// `MeteoStation` custom service UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a00`.
pub const METEO_SERVICE_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A, 0x00,
];

/// Temperature characteristic UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a01`.
pub const TEMPERATURE_CHAR_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A, 0x01,
];

/// Pressure characteristic UUID: `a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a02`.
pub const PRESSURE_CHAR_UUID: [u8; 16] = [
    0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E, 0x1A, 0x02,
];

/// BLE GATT property: read (0x02).
pub const PROP_READ: u8 = 0x02;

/// BLE GATT property: write without response (0x04).
pub const PROP_WRITE_NO_RESPONSE: u8 = 0x04;

/// BLE GATT property: write (0x08).
pub const PROP_WRITE: u8 = 0x08;

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

/// Property bits that mark an LS line as carrying the characteristic *value*
/// handle (as opposed to a descriptor). A line with none of these is a CCCD
/// descriptor line and must not be used as an SHW target.
const VALUE_HANDLE_PROPERTIES: u8 = PROP_READ | PROP_WRITE | PROP_WRITE_NO_RESPONSE;

/// Callback for `query_multiline(ListServices, ...)`.
///
/// Matches characteristic UUIDs from LS output lines and stores their value
/// handles.
///
/// The RN4871 lists a read+notify characteristic as **two** LS lines sharing
/// the same UUID: the value line (read/write property, e.g. `0x02`) carrying
/// the handle that `SHW` must target, and the CCCD descriptor line (notify
/// property `0x10`) whose handle is `value + 1`. Skipping the descriptor line
/// stops it from overwriting the value handle — writing to the CCCD handle
/// never triggers a notification.
pub fn collect_handles(line: &[u8], handles: &mut GattHandles) {
    let Some(info) = ls_parser::parse_characteristic_line(line) else {
        return;
    };

    // Ignore the CCCD descriptor line; only the value line is an SHW target.
    if info.properties & VALUE_HANDLE_PROPERTIES == 0 {
        return;
    }

    if info.uuid_bytes == TEMPERATURE_CHAR_UUID {
        handles.temperature = Some(info.handle);
    } else if info.uuid_bytes == PRESSURE_CHAR_UUID {
        handles.pressure = Some(info.handle);
    } else {
        // Unknown characteristic — ignore
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
        // Given — the RN4871 value line (read property) for temperature
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,02";
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
        // Given — the RN4871 value line (read property) for pressure
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,02";
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
        // Given — value lines for both characteristics
        let temp_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,02";
        let pres_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,02";
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
    fn collect_handles_keeps_value_handle_over_cccd() -> TestResult {
        // Given — the real RN4871 LS decomposition: a value line followed by a
        // CCCD descriptor line for the same UUID (handle = value + 1).
        let value_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,02";
        let cccd_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0073,10,0";
        let mut handles = GattHandles::default();

        // When — both lines arrive in LS order
        collect_handles(value_line, &mut handles);
        collect_handles(cccd_line, &mut handles);

        // Then — the value handle wins; the CCCD line must not overwrite it
        // (writing the CCCD handle via SHW never triggers a notification).
        assert_eq!(
            handles.temperature,
            Some(0x0072),
            "value handle 0x0072, not CCCD handle 0x0073"
        );
        Ok(())
    }

    #[test]
    fn collect_handles_ignores_lone_cccd_line() -> TestResult {
        // Given — only a CCCD descriptor line (notify property, no value bits)
        let cccd_line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0073,10,0";
        let mut handles = GattHandles::default();

        // When
        collect_handles(cccd_line, &mut handles);

        // Then — a descriptor line alone yields no handle
        assert_eq!(
            handles.temperature, None,
            "CCCD descriptor line is not a value handle"
        );
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
