//! Single source of truth describing each sensor the viewer can display.
//!
//! Transport-agnostic: it carries only presentation metadata (name, unit,
//! precision, optional raw→display transform). A data feed maps its incoming
//! readings onto registry indices; adding a sensor is one entry here.

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

// grcov exclude start
#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            displayed, raw,
            "temperature display_value should be identity"
        );
    }
}
// grcov exclude stop
