// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive"
)]

//! Multi-sensor telemetry aggregation: merge per-sensor readings into one
//! running `Telemetry` and derive on-device diagnostics.
//!
//! Derived diagnostics: sky-IR occlusion, BMP388 fault, BME280 fault,
//! VEML7700 luminosity fault, BMP388/BME280 barometer divergence, MLX90614 fault.

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
    /// BME280: humidity (emitted) plus its own temp/pressure used only for the
    /// BMP388 cross-check (not emitted as telemetry temp/pressure fields).
    Bme280 {
        humidity_pct: f32,
        temperature_c: f32,
        pressure_hpa: f32,
    },
    /// BME280 down (init/read failing): blanks humidity + cross-check, raises `BME280_FAULT`.
    Bme280Fault,
    /// VEML7700: ambient light in lux.
    Luminosity { lux: f32 },
    /// VEML7700 down: blanks luminosity, raises `VEML7700_FAULT`.
    LuminosityFault,
}

/// Configuration for the aggregator.
#[derive(Debug, Clone, Copy)]
pub struct AggregatorConfig {
    /// Maximum allowed divergence between MLX90614 ambient and BMP388 air
    /// temperature before setting the sky-IR occlusion flag (°C).
    pub occlusion_threshold_c: f32,
    /// Maximum allowed temperature divergence between BMP388 and BME280 (°C).
    pub temp_divergence_c: f32,
    /// Maximum allowed pressure divergence between BMP388 and BME280 (hPa).
    pub press_divergence_hpa: f32,
}

/// Merges per-sensor readings into one running `Telemetry` and derives the
/// diagnostics bitfield.
///
/// Derived diagnostics: sky-IR occlusion, BMP388 fault, BME280 fault,
/// VEML7700 luminosity fault, BMP388/BME280 barometer divergence, MLX90614 fault.
///
/// Holds the latest barometer air temperature and MLX ambient so occlusion can
/// be (re)derived on every publish, plus latched fault states for each sensor.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a distinct sensor-fault latch; a state-machine would add noise"
)]
pub struct Aggregator {
    telemetry: Telemetry,
    air_temp_c: Option<f32>,
    sky_ambient_c: Option<f32>,
    baro_fault: bool,
    bme_temp_c: Option<f32>,
    bme_pressure_hpa: Option<f32>,
    bme_fault: bool,
    veml_fault: bool,
    mlx_fault: bool,
    cfg: AggregatorConfig,
}

impl Aggregator {
    /// New aggregator with all fields empty and the given configuration.
    #[must_use]
    pub const fn new(cfg: AggregatorConfig) -> Self {
        Self {
            telemetry: Telemetry::empty(),
            air_temp_c: None,
            sky_ambient_c: None,
            baro_fault: false,
            bme_temp_c: None,
            bme_pressure_hpa: None,
            bme_fault: false,
            veml_fault: false,
            mlx_fault: false,
            cfg,
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
                self.mlx_fault = object_c.is_none();
            }
            SensorReading::Bme280 {
                humidity_pct,
                temperature_c,
                pressure_hpa,
            } => {
                self.telemetry.humidity_pct = Some(humidity_pct);
                self.bme_temp_c = Some(temperature_c);
                self.bme_pressure_hpa = Some(pressure_hpa);
                self.bme_fault = false;
            }
            SensorReading::Bme280Fault => {
                self.telemetry.humidity_pct = None;
                self.bme_temp_c = None;
                self.bme_pressure_hpa = None;
                self.bme_fault = true;
            }
            SensorReading::Luminosity { lux } => {
                self.telemetry.luminosity_lux = Some(lux);
                self.veml_fault = false;
            }
            SensorReading::LuminosityFault => {
                self.telemetry.luminosity_lux = None;
                self.veml_fault = true;
            }
        }
    }

    /// Current merged frame, with the diagnostics bits (re)computed.
    #[must_use]
    pub fn snapshot(&self) -> Telemetry {
        let mut t = self.telemetry;
        t.diagnostics = Diagnostics::empty()
            .with_occlusion(self.occluded())
            .with_baro_fault(self.baro_fault)
            .with_bme280_fault(self.bme_fault)
            .with_veml7700_fault(self.veml_fault)
            .with_baro_divergence(self.diverged())
            .with_mlx90614_fault(self.mlx_fault);
        t
    }

    /// Occluded iff both air and MLX-ambient are known and diverge beyond the
    /// threshold. Unknown inputs → not occluded (cannot determine).
    fn occluded(&self) -> bool {
        match (self.air_temp_c, self.sky_ambient_c) {
            (Some(air), Some(amb)) => fabsf(amb - air) > self.cfg.occlusion_threshold_c,
            _ => false,
        }
    }

    /// Diverged iff BOTH baros are fresh and either metric disagrees beyond threshold.
    /// Compares BMP authoritative values (`air_temp_c`, `telemetry.pressure_hpa`) against
    /// the BME cross-check values. Any missing input → not diverged (cannot determine).
    fn diverged(&self) -> bool {
        let temp_div = match (self.air_temp_c, self.bme_temp_c) {
            (Some(a), Some(b)) => fabsf(a - b) > self.cfg.temp_divergence_c,
            _ => false,
        };
        let press_div = match (self.telemetry.pressure_hpa, self.bme_pressure_hpa) {
            (Some(a), Some(b)) => fabsf(a - b) > self.cfg.press_divergence_hpa,
            _ => false,
        };
        temp_div || press_div
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    const TEST_CFG: AggregatorConfig = AggregatorConfig {
        occlusion_threshold_c: 5.0,
        temp_divergence_c: 2.0,
        press_divergence_hpa: 3.0,
    };

    #[test]
    fn aggregator_merges_barometer_and_sky_into_one_frame() {
        // Given
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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
        let mut agg = Aggregator::new(TEST_CFG);

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

    #[test]
    fn aggregator_bme280_populates_humidity_and_clears_fault() {
        // Given
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 55.0,
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.humidity_pct, Some(55.0));
        assert!(!snap.diagnostics.bme280_fault());
    }

    #[test]
    fn aggregator_bme280_fault_blanks_humidity_and_sets_bit() {
        // Given — good reading followed by fault
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 55.0,
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::Bme280Fault);
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.humidity_pct, None);
        assert!(snap.diagnostics.bme280_fault());
    }

    #[test]
    fn aggregator_luminosity_populates_lux() {
        // Given
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Luminosity { lux: 1234.0 });
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.luminosity_lux, Some(1234.0));
        assert!(!snap.diagnostics.veml7700_fault());
    }

    #[test]
    fn aggregator_luminosity_fault_blanks_and_sets_bit() {
        // Given — good reading followed by fault
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Luminosity { lux: 1234.0 });
        agg.ingest(SensorReading::LuminosityFault);
        let snap = agg.snapshot();

        // Then
        assert_eq!(snap.luminosity_lux, None);
        assert!(snap.diagnostics.veml7700_fault());
    }

    #[test]
    fn aggregator_mlx_fault_derived_from_skyir_none() {
        // Given
        let mut agg = Aggregator::new(TEST_CFG);

        // When — none object_c sets mlx_fault
        agg.ingest(SensorReading::SkyIr {
            object_c: None,
            ambient_c: None,
        });
        let snap_fault = agg.snapshot();

        // Then — mlx_fault is set
        assert!(snap_fault.diagnostics.mlx90614_fault());

        // When — good SkyIr clears mlx_fault
        agg.ingest(SensorReading::SkyIr {
            object_c: Some(-10.0),
            ambient_c: Some(15.0),
        });
        let snap_clear = agg.snapshot();

        // Then — mlx_fault is cleared
        assert!(!snap_clear.diagnostics.mlx90614_fault());
    }

    #[test]
    fn aggregator_baro_divergence_set_when_temp_disagrees() {
        // Given — BMP temp=20, BME temp=25, ΔT=5 > threshold of 2
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 50.0,
            temperature_c: 25.0,
            pressure_hpa: 1013.0,
        });
        let snap = agg.snapshot();

        // Then — ΔT = 5 > 2 → diverged
        assert!(snap.diagnostics.baro_divergence());
    }

    #[test]
    fn aggregator_baro_divergence_set_when_pressure_disagrees() {
        // Given — BMP press=1013, BME press=1020, ΔP=7 > threshold of 3; ΔT=0
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 50.0,
            temperature_c: 20.0,
            pressure_hpa: 1020.0,
        });
        let snap = agg.snapshot();

        // Then — ΔP = 7 > 3 → diverged
        assert!(snap.diagnostics.baro_divergence());
    }

    #[test]
    fn aggregator_no_divergence_within_threshold() {
        // Given — ΔT=1≤2, ΔP=1≤3 → not diverged
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 50.0,
            temperature_c: 21.0,
            pressure_hpa: 1014.0,
        });
        let snap = agg.snapshot();

        // Then — within thresholds → not diverged
        assert!(!snap.diagnostics.baro_divergence());
    }

    #[test]
    fn aggregator_no_divergence_when_bme_missing() {
        // Given — only BMP388, no BME280 reading yet
        let mut agg = Aggregator::new(TEST_CFG);

        // When
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        let snap = agg.snapshot();

        // Then — cannot determine → not diverged
        assert!(!snap.diagnostics.baro_divergence());
    }

    #[test]
    fn aggregator_bme_fault_drops_divergence_input() {
        // Given — diverging pair, then BME280Fault
        let mut agg = Aggregator::new(TEST_CFG);

        // When — first establish divergence
        agg.ingest(SensorReading::Barometer {
            temperature_c: 20.0,
            pressure_hpa: 1013.0,
        });
        agg.ingest(SensorReading::Bme280 {
            humidity_pct: 50.0,
            temperature_c: 25.0,
            pressure_hpa: 1013.0,
        });
        // Then confirm divergence is set
        assert!(agg.snapshot().diagnostics.baro_divergence());

        // When — BME280Fault clears cross-check inputs
        agg.ingest(SensorReading::Bme280Fault);
        let snap = agg.snapshot();

        // Then — cross-check inputs are None → cannot determine → not diverged
        assert!(!snap.diagnostics.baro_divergence());
        // And bme280_fault is set
        assert!(snap.diagnostics.bme280_fault());
    }
}
// grcov exclude stop
