//! Minute-bucket accumulator — pure, no I/O.
//!
//! [`BucketAccumulator`] folds 1 Hz [`Telemetry`] frames into running
//! per-field statistics (min / max / avg). [`BucketAccumulator::finish`]
//! produces a [`BucketRow`] ready for insertion into SQLite.
//!
//! Wind direction uses a **circular / vector mean**: each heading is
//! decomposed into (sin, cos) components which are averaged independently,
//! then `atan2(mean_sin, mean_cos)` recovers the mean angle. This correctly
//! handles the 350 °–0 °–10 ° seam (result ≈ 0 °, not 180 °).

use meteo_lib::Telemetry;

use crate::db::BucketRow;

// ---------------------------------------------------------------------------
// Internal running-statistics type (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MinMaxAvg {
    min: f64,
    max: f64,
    sum: f64,
    n: u32,
}

impl MinMaxAvg {
    /// Create an empty accumulator (no values yet).
    ///
    /// `min` is initialised to `+∞` and `max` to `−∞` so that the first
    /// call to [`add`](Self::add) unconditionally updates both.
    const fn new_empty() -> Self {
        Self {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sum: 0.0,
            n: 0,
        }
    }

    /// Incorporate one value into the running statistics.
    fn add(&mut self, v: f64) {
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        self.sum += v;
        self.n = self.n.saturating_add(1);
    }

    /// Average of all accumulated values (`sum / n`).
    ///
    /// Callers must ensure `n >= 1` before calling this; it is only called
    /// from `finish` after at least one `add` call populated the `Option`.
    fn avg(&self) -> f64 {
        self.sum / f64::from(self.n)
    }
}

// ---------------------------------------------------------------------------
// Module-level fold helper
// ---------------------------------------------------------------------------

/// Fold `v` into `acc`, initialising the accumulator on first use.
fn fold(acc: &mut Option<MinMaxAvg>, v: f64) {
    acc.get_or_insert_with(MinMaxAvg::new_empty).add(v);
}

// ---------------------------------------------------------------------------
// Public accumulator
// ---------------------------------------------------------------------------

/// Folds a minute's worth of 1 Hz [`Telemetry`] frames into one min/max/avg row.
///
/// Call [`add`](Self::add) for each incoming frame, then
/// [`finish`](Self::finish) to collapse the statistics into a [`BucketRow`].
/// [`is_empty`](Self::is_empty) returns `true` before the first [`add`](Self::add).
#[derive(Default)]
pub struct BucketAccumulator {
    count: u32,
    temp: Option<MinMaxAvg>,
    pressure: Option<MinMaxAvg>,
    humidity: Option<MinMaxAvg>,
    sky: Option<MinMaxAvg>,
    lux: Option<MinMaxAvg>,
    wind: Option<MinMaxAvg>,
    wind_dir_sin: f64,
    wind_dir_cos: f64,
    wind_dir_n: u32,
    rain: Option<MinMaxAvg>,
    battery: Option<MinMaxAvg>,
    solar_mv: Option<MinMaxAvg>,
    solar_ma: Option<MinMaxAvg>,
    batt_mv: Option<MinMaxAvg>,
    load_ma: Option<MinMaxAvg>,
}

impl BucketAccumulator {
    /// Incorporate one telemetry frame into the running statistics.
    ///
    /// `None` fields in `t` are silently skipped; only `Some` values advance
    /// the corresponding accumulator. The frame counter is always incremented.
    pub fn add(&mut self, t: &Telemetry) {
        self.count = self.count.saturating_add(1);

        if let Some(v) = t.temperature_c {
            fold(&mut self.temp, f64::from(v));
        }
        if let Some(v) = t.pressure_hpa {
            fold(&mut self.pressure, f64::from(v));
        }
        if let Some(v) = t.humidity_pct {
            fold(&mut self.humidity, f64::from(v));
        }
        if let Some(v) = t.sky_temp_c {
            fold(&mut self.sky, f64::from(v));
        }
        if let Some(v) = t.luminosity_lux {
            fold(&mut self.lux, f64::from(v));
        }
        if let Some(v) = t.wind_speed_ms {
            fold(&mut self.wind, f64::from(v));
        }
        if let Some(dir) = t.wind_dir_deg {
            let rad = f64::from(dir).to_radians();
            self.wind_dir_sin += rad.sin();
            self.wind_dir_cos += rad.cos();
            self.wind_dir_n = self.wind_dir_n.saturating_add(1);
        }
        if let Some(v) = t.rain_rate_mm_h {
            fold(&mut self.rain, f64::from(v));
        }
        if let Some(v) = t.battery_pct {
            fold(&mut self.battery, f64::from(v));
        }
        if let Some(v) = t.solar_mv {
            fold(&mut self.solar_mv, f64::from(v));
        }
        if let Some(v) = t.solar_ma {
            fold(&mut self.solar_ma, f64::from(v));
        }
        if let Some(v) = t.batt_mv {
            fold(&mut self.batt_mv, f64::from(v));
        }
        if let Some(v) = t.load_ma {
            fold(&mut self.load_ma, f64::from(v));
        }
    }

    /// Returns `true` if no frames have been added yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Collapse the accumulated statistics into a [`BucketRow`].
    ///
    /// Each field's average is `sum / count`, or `None` when no frame ever
    /// provided a value for that field. Wind direction is the circular mean
    /// wrapped to `[0, 360)` degrees, or `None` when no heading was reported.
    /// `sample_count` reflects the total number of [`add`](Self::add) calls.
    #[must_use]
    pub fn finish(self, bucket_ts: i64) -> BucketRow {
        let wind_dir_avg = (self.wind_dir_n > 0).then(|| {
            let mean_sin = self.wind_dir_sin / f64::from(self.wind_dir_n);
            let mean_cos = self.wind_dir_cos / f64::from(self.wind_dir_n);
            mean_sin.atan2(mean_cos).to_degrees().rem_euclid(360.0)
        });

        BucketRow {
            bucket_ts,
            temp_min: self.temp.as_ref().map(|a| a.min),
            temp_max: self.temp.as_ref().map(|a| a.max),
            temp_avg: self.temp.as_ref().map(MinMaxAvg::avg),
            pressure_min: self.pressure.as_ref().map(|a| a.min),
            pressure_max: self.pressure.as_ref().map(|a| a.max),
            pressure_avg: self.pressure.as_ref().map(MinMaxAvg::avg),
            humidity_min: self.humidity.as_ref().map(|a| a.min),
            humidity_max: self.humidity.as_ref().map(|a| a.max),
            humidity_avg: self.humidity.as_ref().map(MinMaxAvg::avg),
            sky_min: self.sky.as_ref().map(|a| a.min),
            sky_max: self.sky.as_ref().map(|a| a.max),
            sky_avg: self.sky.as_ref().map(MinMaxAvg::avg),
            lux_min: self.lux.as_ref().map(|a| a.min),
            lux_max: self.lux.as_ref().map(|a| a.max),
            lux_avg: self.lux.as_ref().map(MinMaxAvg::avg),
            wind_min: self.wind.as_ref().map(|a| a.min),
            wind_max: self.wind.as_ref().map(|a| a.max),
            wind_avg: self.wind.as_ref().map(MinMaxAvg::avg),
            wind_dir_avg,
            rain_avg: self.rain.as_ref().map(MinMaxAvg::avg),
            rain_max: self.rain.as_ref().map(|a| a.max),
            battery_avg: self.battery.as_ref().map(MinMaxAvg::avg),
            solar_mv_avg: self.solar_mv.as_ref().map(MinMaxAvg::avg),
            solar_ma_avg: self.solar_ma.as_ref().map(MinMaxAvg::avg),
            batt_mv_avg: self.batt_mv.as_ref().map(MinMaxAvg::avg),
            load_ma_avg: self.load_ma.as_ref().map(MinMaxAvg::avg),
            sample_count: i64::from(self.count),
        }
    }
}

// ---------------------------------------------------------------------------
// Floor-to-minute helper
// ---------------------------------------------------------------------------

/// Floor a unix-second timestamp to the start of its minute (the bucket key).
///
/// The result is always a multiple of 60 and satisfies
/// `result <= unix_secs < result + 60`.
///
/// Idempotent: `floor_to_minute(floor_to_minute(t)) == floor_to_minute(t)`.
#[must_use]
pub const fn floor_to_minute(unix_secs: i64) -> i64 {
    // rem_euclid always returns a value in [0, 60); saturating_sub is used to
    // satisfy the arithmetic_side_effects lint (overflow is not possible for
    // any realistic timestamp, but the lint applies unconditionally).
    unix_secs.saturating_sub(unix_secs.rem_euclid(60))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "TestResult is the standard test pattern"
)]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a [`Telemetry`] with a single temperature value, all else `None`.
    fn telem_with_temp(temp_c: f32) -> Telemetry {
        Telemetry {
            temperature_c: Some(temp_c),
            ..Telemetry::empty()
        }
    }

    /// Build a [`Telemetry`] with a single wind-direction value.
    fn telem_with_dir(deg: f32) -> Telemetry {
        Telemetry {
            wind_dir_deg: Some(deg),
            ..Telemetry::empty()
        }
    }

    // -----------------------------------------------------------------------
    // Accumulator tests
    // -----------------------------------------------------------------------

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: values asserted to be Some")]
    fn accumulator_min_max_avg_over_three_frames() -> TestResult {
        // Given — three frames with temperatures 10, 20, 30
        let mut acc = BucketAccumulator::default();

        // When
        acc.add(&telem_with_temp(10.0));
        acc.add(&telem_with_temp(20.0));
        acc.add(&telem_with_temp(30.0));
        let row = acc.finish(0);

        // Then
        let min = row.temp_min.unwrap();
        let max = row.temp_max.unwrap();
        let avg = row.temp_avg.unwrap();

        assert!(
            (min - 10.0).abs() < 1e-9,
            "temp_min should be 10.0, got {min}"
        );
        assert!(
            (max - 30.0).abs() < 1e-9,
            "temp_max should be 30.0, got {max}"
        );
        assert!(
            (avg - 20.0).abs() < 1e-9,
            "temp_avg should be 20.0, got {avg}"
        );
        assert_eq!(row.sample_count, 3);

        Ok(())
    }

    #[test]
    fn accumulator_skips_none_fields() -> TestResult {
        // Given — frames with temperature = None but with humidity present
        let mut acc = BucketAccumulator::default();
        let frame1 = Telemetry {
            temperature_c: None,
            humidity_pct: Some(60.0),
            ..Telemetry::empty()
        };
        let frame2 = Telemetry {
            temperature_c: None,
            humidity_pct: Some(80.0),
            ..Telemetry::empty()
        };

        // When
        acc.add(&frame1);
        acc.add(&frame2);
        let row = acc.finish(0);

        // Then — temp columns remain None; humidity columns are populated
        assert!(row.temp_min.is_none(), "temp_min should be None");
        assert!(row.temp_max.is_none(), "temp_max should be None");
        assert!(row.temp_avg.is_none(), "temp_avg should be None");
        assert!(
            row.humidity_avg.is_some(),
            "humidity_avg should be Some after two frames"
        );
        assert_eq!(row.sample_count, 2);

        Ok(())
    }

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: wind_dir_avg asserted to be Some")]
    fn accumulator_wind_dir_vector_mean_wraps_seam() -> TestResult {
        // Given — headings 350° and 10°; arithmetic mean = 180°, vector mean ≈ 0°
        let mut acc = BucketAccumulator::default();

        // When
        acc.add(&telem_with_dir(350.0));
        acc.add(&telem_with_dir(10.0));
        let row = acc.finish(0);

        // Then — vector mean wraps the 0° seam; result must be ≈ 0° (not 180°)
        let dir = row.wind_dir_avg.unwrap();
        // rem_euclid maps near-zero angles correctly; allow ±1° tolerance
        assert!(
            dir < 1.0 || dir > 359.0,
            "wind_dir_avg for [350°, 10°] should be ≈ 0° (mod 360), got {dir}"
        );

        Ok(())
    }

    #[test]
    fn accumulator_empty_is_empty() -> TestResult {
        // Given — no frames added
        let acc = BucketAccumulator::default();

        // Then
        assert!(
            acc.is_empty(),
            "fresh accumulator must report is_empty() = true"
        );

        // When — finish on empty accumulator
        let row = BucketAccumulator::default().finish(0);

        // Then — all optional columns are None; sample_count = 0
        assert!(row.temp_min.is_none());
        assert!(row.pressure_min.is_none());
        assert!(row.humidity_min.is_none());
        assert!(row.wind_dir_avg.is_none());
        assert_eq!(row.sample_count, 0);

        Ok(())
    }

    // -----------------------------------------------------------------------
    // floor_to_minute tests
    // -----------------------------------------------------------------------

    #[test]
    fn floor_to_minute_floors_and_is_idempotent() -> TestResult {
        // Given / When / Then — non-multiple of 60 floors down
        assert_eq!(floor_to_minute(125), 120, "125 should floor to 120");
        // Exact multiple is unchanged
        assert_eq!(floor_to_minute(120), 120, "120 should stay 120");
        // Idempotent: applying twice gives the same result
        assert_eq!(
            floor_to_minute(floor_to_minute(125)),
            floor_to_minute(125),
            "floor_to_minute must be idempotent"
        );
        // Zero
        assert_eq!(floor_to_minute(0), 0, "0 should floor to 0");
        // 59 seconds → 0
        assert_eq!(floor_to_minute(59), 0, "59 should floor to 0");

        Ok(())
    }
}
// grcov exclude end
