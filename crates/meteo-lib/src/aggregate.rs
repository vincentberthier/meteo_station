// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive"
)]

//! Multi-sensor telemetry aggregation: merge per-sensor readings into one
//! running `Telemetry` and derive on-device diagnostics (sky-IR occlusion,
//! BMP388 fault).

use libm::fabsf;

use crate::ble::frame::{Diagnostics, Telemetry};

/// A reading (or health signal) from one sensor, sent over the inter-task
/// channel to the aggregator.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SensorReading {
    /// BMP388: authoritative air temperature (°C) + pressure (hPa).
    Barometer {
        temperature_c: f32,
        pressure_hpa: f32,
    },
    /// BMP388 is not providing data (failed to initialize, or a read error
    /// forced a re-init). Clears temperature/pressure and raises `BARO_FAULT`.
    BarometerFault,
    /// MLX90614: object/IR temp → `sky_temp_c`; ambient (TA) → occlusion proxy.
    /// Either may be `None` on a failed/invalid read (graceful degradation).
    SkyIr {
        object_c: Option<f32>,
        ambient_c: Option<f32>,
    },
}

/// Merges per-sensor readings into one running `Telemetry` and derives the
/// diagnostics bitfield (sky-IR occlusion, BMP388 fault).
///
/// Holds the latest barometer air temperature and MLX ambient so occlusion can
/// be (re)derived on every publish, plus the latched barometer-fault state.
pub struct Aggregator {
    telemetry: Telemetry,
    air_temp_c: Option<f32>,
    sky_ambient_c: Option<f32>,
    baro_fault: bool,
    occlusion_threshold_c: f32,
}

impl Aggregator {
    /// New aggregator with all fields empty and the given occlusion threshold (°C).
    #[must_use]
    pub const fn new(occlusion_threshold_c: f32) -> Self {
        Self {
            telemetry: Telemetry::empty(),
            air_temp_c: None,
            sky_ambient_c: None,
            baro_fault: false,
            occlusion_threshold_c,
        }
    }

    /// Fold one reading into the running state.
    pub const fn ingest(&mut self, reading: SensorReading) {
        match reading {
            SensorReading::Barometer {
                temperature_c,
                pressure_hpa,
            } => {
                self.telemetry.temperature_c = Some(temperature_c);
                self.telemetry.pressure_hpa = Some(pressure_hpa);
                self.air_temp_c = Some(temperature_c);
                self.baro_fault = false;
            }
            SensorReading::BarometerFault => {
                // Sensor down: blank its data and latch the fault for the diagnostics
                // byte. occlusion can no longer be computed (air_temp gone → false).
                self.telemetry.temperature_c = None;
                self.telemetry.pressure_hpa = None;
                self.air_temp_c = None;
                self.baro_fault = true;
            }
            SensorReading::SkyIr {
                object_c,
                ambient_c,
            } => {
                // A failed/invalid MLX read (object_c == None) blanks sky_temp_c
                // for subsequent frames until the next good read — matches the
                // brainstorm's graceful-degradation rule.
                self.telemetry.sky_temp_c = object_c;
                self.sky_ambient_c = ambient_c;
            }
        }
    }

    /// Current merged frame, with the diagnostics bits (re)computed.
    #[must_use]
    pub fn snapshot(&self) -> Telemetry {
        let mut t = self.telemetry;
        t.diagnostics = Diagnostics::empty()
            .with_occlusion(self.occluded())
            .with_baro_fault(self.baro_fault);
        t
    }

    /// Occluded iff both air and MLX-ambient are known and diverge beyond the
    /// threshold. Unknown inputs → not occluded (cannot determine).
    fn occluded(&self) -> bool {
        match (self.air_temp_c, self.sky_ambient_c) {
            (Some(air), Some(amb)) => fabsf(amb - air) > self.occlusion_threshold_c,
            _ => false,
        }
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn aggregator_merges_barometer_and_sky_into_one_frame() {
        // Given
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-15.0),
            ambient_c: Some(19.0),
        });
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.temperature_c, Some(20.0));
        assert_eq!(snap.pressure_hpa, Some(1013.0));
        assert_eq!(snap.sky_temp_c, Some(-15.0));
        assert!(!snap.diagnostics.baro_fault());
    }

    #[test]
    fn aggregator_sets_occlusion_bit_when_ambient_diverges() {
        // Given
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-10.0),
            ambient_c: Some(30.0),
        });
        let snap = agg.snapshot();

        // Then — |30 - 20| = 10 > 5 → occluded
        assert!(snap.diagnostics.occlusion());
    }

    #[test]
    fn aggregator_clears_occlusion_within_threshold() {
        // Given
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-10.0),
            ambient_c: Some(22.0),
        });
        let snap = agg.snapshot();

        // Then — |22 - 20| = 2 < 5 → not occluded
        assert!(!snap.diagnostics.occlusion());
    }

    #[test]
    fn aggregator_occlusion_false_at_exact_threshold() {
        // Given — exactly at threshold (strict > comparison: should NOT be occluded)
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-10.0),
            ambient_c: Some(25.0),
        });
        let snap = agg.snapshot();

        // Then — |25 - 20| = 5, which is NOT > 5 → not occluded
        assert!(!snap.diagnostics.occlusion());
    }

    #[test]
    fn aggregator_no_occlusion_when_ambient_missing() {
        // Given — only barometer, no SkyIr reading
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        let snap = agg.snapshot();

        // Then
        assert!(!snap.diagnostics.occlusion());
    }

    #[test]
    fn aggregator_sky_temp_none_on_failed_read() {
        // Given — first a good SkyIr, then a failed one
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-15.0),
            ambient_c: Some(19.0),
        });
        agg.ingest(SensorReading::SkyIr {
            object_c: None,
            ambient_c: None,
        });
        let snap = agg.snapshot();

        // Then — failed read blanks sky_temp_c
        assert_eq!(snap.sky_temp_c, None);
    }

    #[test]
    fn aggregator_barometer_fault_sets_bit_and_blanks_data() {
        // Given
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::BarometerFault);
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.temperature_c, None);
        assert_eq!(snap.pressure_hpa, None);
        assert!(snap.diagnostics.baro_fault());
    }

    #[test]
    fn aggregator_barometer_reading_clears_fault() {
        // Given
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::BarometerFault);
        agg.ingest(SensorReading::Barometer {
            temperature_c: 21.0,
            pressure_hpa: 1012.0,
        });
        let snap = agg.snapshot();

        // Then
        assert!(!snap.diagnostics.baro_fault());
        assert_eq!(snap.temperature_c, Some(21.0));
    }

    #[test]
    fn aggregator_baro_fault_forces_occlusion_false() {
        // Given — SkyIr with extreme ambient, then BarometerFault (air temp gone)
        let mut agg = Aggregator::new(5.0);

        // When
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-10.0),
            ambient_c: Some(99.0),
        });
        agg.ingest(SensorReading::BarometerFault);
        let snap = agg.snapshot();

        // Then — air_temp_c is None → cannot compute occlusion → false
        assert!(!snap.diagnostics.occlusion());
        assert!(snap.diagnostics.baro_fault());
    }
}
// grcov exclude stop
