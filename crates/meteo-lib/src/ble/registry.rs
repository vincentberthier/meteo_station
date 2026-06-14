//! Single source of truth describing each BLE sensor characteristic.
//!
//! Provides the UUID and how to present its readings. Host viewers iterate
//! this table to build panels; adding a sensor is one entry here (plus its
//! UUID in `gatt`).

use super::gatt::{PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};

/// Identity + presentation metadata for one sensor characteristic.
#[derive(Debug, Clone, Copy)]
pub struct SensorDescriptor {
    /// 128-bit characteristic UUID (same big-endian byte order as `gatt`).
    pub uuid: [u8; 16],
    /// Human-readable name, e.g. "Temperature".
    pub name: &'static str,
    /// Display unit, e.g. "°C" or "hPa".
    pub unit: &'static str,
    /// Fractional digits to display.
    pub precision: u8,
    /// Optional transform raw-wire-f32 → display value (e.g. Pa → hPa).
    pub transform: Option<fn(f32) -> f32>,
}

impl SensorDescriptor {
    /// Apply the transform (identity when `None`).
    #[must_use]
    pub fn display_value(&self, raw: f32) -> f32 {
        self.transform.map_or(raw, |f| f(raw))
    }
}

/// Pascals → hectopascals (float division does not trip
/// `arithmetic_side_effects`; see `meteo-cli` line `p / 100.0`).
#[must_use]
pub fn pa_to_hpa(pa: f32) -> f32 {
    pa / 100.0
}

/// All sensors the station can expose, in display order.
pub static SENSORS: &[SensorDescriptor] = &[
    SensorDescriptor {
        uuid: TEMPERATURE_CHAR_UUID,
        name: "Temperature",
        unit: "°C",
        precision: 2,
        transform: None,
    },
    SensorDescriptor {
        uuid: PRESSURE_CHAR_UUID,
        name: "Pressure",
        unit: "hPa",
        precision: 2,
        transform: Some(pa_to_hpa),
    },
];

/// Registry index of the sensor whose characteristic UUID matches, if any.
#[must_use]
pub fn index_for_uuid(uuid: &[u8; 16]) -> Option<usize> {
    SENSORS.iter().position(|s| &s.uuid == uuid)
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::{error, result};

    use std::boxed::Box;
    use test_log::test;

    use super::super::gatt::{PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};
    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn index_for_uuid_temperature_returns_zero() -> TestResult {
        // Given
        let uuid = TEMPERATURE_CHAR_UUID;

        // When
        let result = index_for_uuid(&uuid);

        // Then
        assert_eq!(result, Some(0), "temperature should be at index 0");
        Ok(())
    }

    #[test]
    fn index_for_uuid_pressure_returns_one() -> TestResult {
        // Given
        let uuid = PRESSURE_CHAR_UUID;

        // When
        let result = index_for_uuid(&uuid);

        // Then
        assert_eq!(result, Some(1), "pressure should be at index 1");
        Ok(())
    }

    #[test]
    fn index_for_uuid_unknown_returns_none() -> TestResult {
        // Given
        let uuid = [0xFF_u8; 16];

        // When
        let result = index_for_uuid(&uuid);

        // Then
        assert!(result.is_none(), "unknown UUID should return None");
        Ok(())
    }

    #[test]
    fn pa_to_hpa_converts() -> TestResult {
        // Given
        let pa = 101_325.0_f32;

        // When
        let hpa = pa_to_hpa(pa);

        // Then
        assert!(
            (hpa - 1013.25_f32).abs() < 1e-3,
            "101325 Pa should convert to ~1013.25 hPa, got {hpa}"
        );
        Ok(())
    }

    #[test]
    fn display_value_applies_pressure_transform() -> TestResult {
        // Given
        let raw = 101_325.0_f32;

        // When
        let displayed = SENSORS[1].display_value(raw);

        // Then
        assert!(
            (displayed - 1013.25_f32).abs() < 1e-3,
            "pressure display_value should convert Pa to hPa, got {displayed}"
        );
        Ok(())
    }

    #[test]
    fn display_value_identity_without_transform() -> TestResult {
        // Given
        let raw = 21.5_f32;

        // When
        let displayed = SENSORS[0].display_value(raw);

        // Then
        assert_eq!(
            displayed, raw,
            "temperature display_value should be identity"
        );
        Ok(())
    }
}
// grcov exclude stop
