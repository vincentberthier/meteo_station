//! Sensor sample → frame folding (pure, host-testable).

// Suppress false positives from defmt macro expansion (only active when defmt feature is on).
#![cfg_attr(
    feature = "defmt",
    expect(
        clippy::missing_asserts_for_indexing,
        reason = "false positives from defmt macro expansion"
    )
)]

use super::frame::Frame;

/// A single sensor reading that can be folded into a [`Frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SensorSample {
    /// Temperature and pressure reading from a barometric sensor.
    Barometer {
        /// Ambient temperature in degrees Celsius.
        temperature_c: f32,
        /// Atmospheric pressure in Pascals.
        pressure_pa: f32,
    },
}

/// Fold one sample's values into the running `Frame` (latest-wins per field,
/// others untouched).
pub const fn apply_sample(frame: &mut Frame, sample: SensorSample) {
    match sample {
        SensorSample::Barometer {
            temperature_c,
            pressure_pa,
        } => {
            frame.temperature_c = Some(temperature_c);
            frame.pressure_pa = Some(pressure_pa);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn apply_barometer_sets_temp_and_pressure() {
        // Given
        let mut frame = Frame::default();
        let sample = SensorSample::Barometer {
            temperature_c: 21.5_f32,
            pressure_pa: 101_325.0_f32,
        };

        // When
        apply_sample(&mut frame, sample);

        // Then
        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let temp = frame
            .temperature_c
            .expect("temperature_c should be Some after apply_sample");
        #[expect(clippy::float_cmp, reason = "exact value set then read, no arithmetic")]
        let temp_ok = temp == 21.5_f32;
        assert!(temp_ok, "temperature_c should be 21.5, got {temp}");

        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let pressure = frame
            .pressure_pa
            .expect("pressure_pa should be Some after apply_sample");
        #[expect(clippy::float_cmp, reason = "exact value set then read, no arithmetic")]
        let pressure_ok = pressure == 101_325.0_f32;
        assert!(
            pressure_ok,
            "pressure_pa should be 101325.0, got {pressure}"
        );

        assert!(
            frame.humidity_pct.is_none(),
            "humidity_pct should remain None"
        );
        assert!(frame.sky_temp_c.is_none(), "sky_temp_c should remain None");
        assert!(
            frame.luminosity_lux.is_none(),
            "luminosity_lux should remain None"
        );
        assert!(
            frame.wind_speed_ms.is_none(),
            "wind_speed_ms should remain None"
        );
        assert!(
            frame.wind_dir_deg.is_none(),
            "wind_dir_deg should remain None"
        );
        assert!(
            frame.battery_pct.is_none(),
            "battery_pct should remain None"
        );
    }

    #[test]
    fn apply_barometer_overwrites_previous_sample() {
        // Given
        let mut frame = Frame::default();
        let first = SensorSample::Barometer {
            temperature_c: 15.0_f32,
            pressure_pa: 99_000.0_f32,
        };
        let second = SensorSample::Barometer {
            temperature_c: 22.3_f32,
            pressure_pa: 102_000.0_f32,
        };

        // When
        apply_sample(&mut frame, first);
        apply_sample(&mut frame, second);

        // Then
        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let temp = frame
            .temperature_c
            .expect("temperature_c should be Some after second apply_sample");
        #[expect(clippy::float_cmp, reason = "exact value set then read, no arithmetic")]
        let temp_ok = temp == 22.3_f32;
        assert!(
            temp_ok,
            "temperature_c should be 22.3 (second sample), got {temp}"
        );

        #[expect(
            clippy::expect_used,
            reason = "test: .expect() surfaces failures directly"
        )]
        let pressure = frame
            .pressure_pa
            .expect("pressure_pa should be Some after second apply_sample");
        #[expect(clippy::float_cmp, reason = "exact value set then read, no arithmetic")]
        let pressure_ok = pressure == 102_000.0_f32;
        assert!(
            pressure_ok,
            "pressure_pa should be 102000.0 (second sample), got {pressure}"
        );

        assert!(
            frame.humidity_pct.is_none(),
            "humidity_pct should remain None"
        );
        assert!(frame.sky_temp_c.is_none(), "sky_temp_c should remain None");
        assert!(
            frame.luminosity_lux.is_none(),
            "luminosity_lux should remain None"
        );
        assert!(
            frame.wind_speed_ms.is_none(),
            "wind_speed_ms should remain None"
        );
        assert!(
            frame.wind_dir_deg.is_none(),
            "wind_dir_deg should remain None"
        );
        assert!(
            frame.battery_pct.is_none(),
            "battery_pct should remain None"
        );
    }
}
// grcov exclude end
