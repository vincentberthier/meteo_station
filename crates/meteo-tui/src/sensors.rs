//! Single source of truth describing each sensor the viewer can display.
//!
//! Transport-agnostic: it carries only presentation metadata (name, unit,
//! precision, optional raw→display transform). A data feed maps its incoming
//! readings onto registry indices; adding a sensor is one entry here.

use meteo_lib::ble::frame::FrameField;

/// Presentation metadata for one sensor.
#[derive(Debug, Clone, Copy)]
pub struct SensorDescriptor {
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

/// Pascals → hectopascals.
#[must_use]
pub fn pa_to_hpa(pa: f32) -> f32 {
    pa / 100.0
}

/// All sensors the viewer can present, in display order.
pub static SENSORS: &[SensorDescriptor] = &[
    SensorDescriptor {
        name: "Temperature",
        unit: "°C",
        precision: 2,
        transform: None,
    },
    SensorDescriptor {
        name: "Pressure",
        unit: "hPa",
        precision: 2,
        transform: Some(pa_to_hpa),
    },
];

/// Map a decoded frame field to its registry index, or `None` if this build's
/// registry does not present it. Keeps wire order and display order decoupled.
#[must_use]
pub const fn field_to_index(field: FrameField) -> Option<usize> {
    match field {
        FrameField::Temperature => Some(0_usize),
        FrameField::Pressure => Some(1_usize),
        // humidity, sky, lux, wind, battery: not in registry yet
        FrameField::Humidity
        | FrameField::SkyTemp
        | FrameField::Luminosity
        | FrameField::WindSpeed
        | FrameField::WindDir
        | FrameField::Battery => None,
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use meteo_lib::ble::frame::Frame;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn pa_to_hpa_converts() {
        // Given
        let pa = 101_325.0_f32;

        // When
        let hpa = pa_to_hpa(pa);

        // Then
        assert!(
            (hpa - 1013.25_f32).abs() < 1e-3,
            "101325 Pa should convert to ~1013.25 hPa, got {hpa}"
        );
    }

    #[test]
    fn display_value_applies_pressure_transform() {
        // Given
        let raw = 101_325.0_f32;

        // When
        let displayed = SENSORS[1].display_value(raw);

        // Then
        assert!(
            (displayed - 1013.25_f32).abs() < 1e-3,
            "pressure display_value should convert Pa to hPa, got {displayed}"
        );
    }

    #[test]
    fn display_value_identity_without_transform() {
        // Given
        let raw = 21.5_f32;

        // When
        let displayed = SENSORS[0].display_value(raw);

        // Then
        #[expect(
            clippy::float_cmp,
            reason = "exact value set then read with identity transform, no arithmetic"
        )]
        let ok = displayed == raw;
        assert!(ok, "temperature display_value should be identity");
    }

    #[test]
    fn field_to_index_maps_temperature_and_pressure() {
        // Given / When / Then
        assert_eq!(
            field_to_index(FrameField::Temperature),
            Some(0_usize),
            "Temperature should map to registry index 0"
        );
        assert_eq!(
            field_to_index(FrameField::Pressure),
            Some(1_usize),
            "Pressure should map to registry index 1"
        );
        assert_eq!(
            field_to_index(FrameField::Humidity),
            None,
            "Humidity should map to None (not in registry yet)"
        );
    }

    #[test]
    fn decoded_pressure_feeds_registry_in_pascals() -> TestResult {
        // Given
        let frame = Frame {
            pressure_pa: Some(101_325.0_f32),
            ..Frame::default()
        };

        // When — exercise the wire path
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded)?;
        let pair = decoded
            .present_fields()
            .find(|(f, _)| *f == FrameField::Pressure);

        // Then
        let (field, value) = pair.ok_or("pressure field missing from present_fields")?;
        assert_eq!(
            field_to_index(field),
            Some(1_usize),
            "Pressure field should map to registry index 1"
        );
        assert!(
            (value - 101_325.0_f32).abs() < 10.0_f32,
            "decoded pressure should be ~101325 Pa (±10), got {value}"
        );
        let displayed = SENSORS[1_usize].display_value(value);
        assert!(
            (displayed - 1013.25_f32).abs() < 0.1_f32,
            "SENSORS[1].display_value should give ~1013.25 hPa, got {displayed}"
        );
        Ok(())
    }
}
// grcov exclude stop
