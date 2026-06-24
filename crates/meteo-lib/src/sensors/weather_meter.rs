//! `SparkFun` weather-meter (SEN-15901) signal conversions: anemometer pulse rate
//! → wind speed, wind-vane divider voltage → heading, and rain-gauge tips → rate.
//!
//! Pure, host-tested logic. The firmware tasks own the GPIO edge counting and ADC
//! sampling; this module only converts the sampled quantities into physical units.

use libm::fabsf;

/// Anemometer calibration: one reed closure per second (1 Hz) equals
/// 2.4 km/h = `0.666_666_7` m/s (datasheet `weather_meter.md`).
const MS_PER_HZ: f32 = 0.666_666_7;

/// Wind speed in metres per second from the anemometer pulse rate (reed closures
/// per second).
#[must_use]
pub fn wind_speed_ms(pulses_per_sec: f32) -> f32 {
    pulses_per_sec * MS_PER_HZ
}

/// Wind-vane divider supply (mV). The vane resistance sits between the ADC node
/// and ground, pulled up to this rail through [`VANE_PULLUP_OHMS`].
const VANE_VCC_MV: f32 = 3300.0;
/// Wind-vane divider pull-up resistance (Ω): `3V3 ─ 10kΩ ─ ADC ─ vane ─ GND`.
const VANE_PULLUP_OHMS: f32 = 10_000.0;

/// `(vane resistance Ω, heading degrees)` for the 16 SEN-15901 vane positions
/// (datasheet `weather_meter.md`).
const VANE_TABLE: [(f32, f32); 16] = [
    (33_000.0, 0.0),
    (6_570.0, 22.5),
    (8_200.0, 45.0),
    (891.0, 67.5),
    (1_000.0, 90.0),
    (688.0, 112.5),
    (2_200.0, 135.0),
    (1_410.0, 157.5),
    (3_900.0, 180.0),
    (3_140.0, 202.5),
    (16_000.0, 225.0),
    (14_120.0, 247.5),
    (120_000.0, 270.0),
    (42_120.0, 292.5),
    (64_900.0, 315.0),
    (21_880.0, 337.5),
];

/// Expected divider output (mV) for a vane leg of `ohms`:
/// `Vout = VCC · R / (R + Rpull)`.
fn vane_mv(ohms: f32) -> f32 {
    VANE_VCC_MV * ohms / (ohms + VANE_PULLUP_OHMS)
}

/// Maps a measured wind-vane ADC voltage (`mv`, millivolts) to the nearest of the
/// 16 compass headings (degrees, 0–337.5 in 22.5° steps).
///
/// Nearest-neighbour against the divider voltages derived from [`VANE_TABLE`].
/// Note: a disconnected vane (open circuit) reads near the rail and resolves to a
/// spurious heading — there is no fault flag for it yet (TODO: add an out-of-band
/// rejection band once a diagnostics bit is allocated).
#[must_use]
pub fn wind_direction_deg(mv: u16) -> f32 {
    let measured = f32::from(mv);
    let mut best_deg = 0.0;
    let mut best_err = f32::INFINITY;
    for (ohms, deg) in VANE_TABLE {
        let err = fabsf(measured - vane_mv(ohms));
        if err < best_err {
            best_err = err;
            best_deg = deg;
        }
    }
    best_deg
}

/// Rain-gauge calibration: one tipping-bucket closure equals 0.2794 mm of
/// rainfall (datasheet `weather_meter.md`).
pub const MM_PER_TIP: f32 = 0.2794;

/// Sliding-window length (seconds) over which the rain rate is averaged.
///
/// At 0.2794 mm/tip a 300 s window gives ~3.4 mm/h per tip of resolution while
/// staying responsive; lengthen it for finer drizzle resolution at the cost of
/// latency.
pub const RAIN_WINDOW_SECS: usize = 300;

/// Sliding-window rain-rate estimator.
///
/// Fed one tip count per second via [`RainRate::push`]; reports rainfall rate in
/// mm/h averaged over the trailing [`RAIN_WINDOW_SECS`] window. Until the window
/// has actually filled (the first [`RAIN_WINDOW_SECS`] after boot) it reports
/// `None` rather than extrapolating from a partial window — a partial window makes
/// a few tips look like a cloudburst, and no value beats a wrong one.
pub struct RainRate {
    buckets: [u16; RAIN_WINDOW_SECS],
    head: usize,
    filled: usize,
    total: u32,
}

impl RainRate {
    /// A fresh estimator with an empty window.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buckets: [0; RAIN_WINDOW_SECS],
            head: 0,
            filled: 0,
            total: 0,
        }
    }

    /// Records this second's `tips`, advancing the window, and returns the current
    /// rate in mm/h (`None` until the window has filled).
    pub fn push(&mut self, tips: u16) -> Option<f32> {
        // Evict the bucket about to be overwritten from the running total, then add
        // the new second. saturating ops keep the running sum sound for clippy even
        // though `evicted <= total` always holds.
        let evicted = self.buckets[self.head];
        self.total = self
            .total
            .saturating_sub(u32::from(evicted))
            .saturating_add(u32::from(tips));
        self.buckets[self.head] = tips;
        self.head = self.head.wrapping_add(1) % RAIN_WINDOW_SECS;
        if self.filled < RAIN_WINDOW_SECS {
            self.filled = self.filled.saturating_add(1);
        }
        self.rate_mm_h()
    }

    /// Current rainfall rate (mm/h) averaged over the full trailing window, or
    /// `None` until the window has filled ([`RAIN_WINDOW_SECS`] after boot). The
    /// rate is only ever computed over a true full window, never extrapolated.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "total tips and window length are small; f32 precision is ample"
    )]
    pub fn rate_mm_h(&self) -> Option<f32> {
        if self.filled < RAIN_WINDOW_SECS {
            return None;
        }
        let mm = self.total as f32 * MM_PER_TIP;
        let window_h = RAIN_WINDOW_SECS as f32 / 3600.0;
        Some(mm / window_h)
    }
}

impl Default for RainRate {
    fn default() -> Self {
        Self::new()
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "TestResult is the standard test pattern"
)]
#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::boxed::Box;
    use core::{error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // -------------------------------------------------------------------------
    // wind_speed_ms
    // -------------------------------------------------------------------------

    #[test]
    fn wind_speed_zero_pulses_is_calm() -> TestResult {
        // Given / When
        let speed = wind_speed_ms(0.0);

        // Then
        assert!(speed.abs() < f32::EPSILON);
        Ok(())
    }

    #[test]
    fn wind_speed_one_hz_is_point_six_seven_ms() -> TestResult {
        // Given — 1 Hz == 2.4 km/h == 0.6667 m/s
        // When
        let speed = wind_speed_ms(1.0);

        // Then
        assert!((speed - 0.666_666_7).abs() < 0.001);
        Ok(())
    }

    #[test]
    fn wind_speed_scales_linearly() -> TestResult {
        // Given / When
        let speed = wind_speed_ms(10.0);

        // Then — 10 Hz ≈ 6.667 m/s
        assert!((speed - 6.666_667).abs() < 0.01);
        Ok(())
    }

    // -------------------------------------------------------------------------
    // wind_direction_deg
    // -------------------------------------------------------------------------

    #[test]
    fn wind_direction_resolves_exact_bucket_voltages() -> TestResult {
        // Given — feed the exact divider voltage of each known position
        for (ohms, deg) in VANE_TABLE {
            // When — round to the integer mV the ADC would report
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "divider mV is in 0..3300, well within u16"
            )]
            let mv = libm::roundf(vane_mv(ohms)) as u16;
            let heading = wind_direction_deg(mv);

            // Then
            assert!(
                (heading - deg).abs() < f32::EPSILON,
                "{ohms} Ω → {mv} mV should map to {deg}°, got {heading}°"
            );
        }
        Ok(())
    }

    #[test]
    fn wind_direction_snaps_near_value_to_nearest_bucket() -> TestResult {
        // Given — E is 1000 Ω → 3300·1000/11000 = 300 mV; 305 mV is closest to E (90°)
        // When
        let heading = wind_direction_deg(305);

        // Then
        assert!((heading - 90.0).abs() < f32::EPSILON, "got {heading}°");
        Ok(())
    }

    #[test]
    fn wind_direction_low_voltage_is_southeast_family() -> TestResult {
        // Given — ESE is the lowest divider voltage (688 Ω → ~212 mV)
        // When
        let heading = wind_direction_deg(210);

        // Then
        assert!((heading - 112.5).abs() < f32::EPSILON, "got {heading}°");
        Ok(())
    }

    // -------------------------------------------------------------------------
    // RainRate
    // -------------------------------------------------------------------------

    #[test]
    fn rain_rate_dry_full_window_is_zero() -> TestResult {
        // Given
        let mut rain = RainRate::new();

        // When — a full window of dry seconds
        let mut rate = None;
        for _ in 0..RAIN_WINDOW_SECS {
            rate = rain.push(0);
        }

        // Then — full window with no tips → Some(0.0)
        let Some(got) = rate else {
            return Err("rate must be Some once the window is full".into());
        };
        assert!(got.abs() < f32::EPSILON, "dry window should be 0 mm/h");
        Ok(())
    }

    #[test]
    fn rain_rate_is_none_until_window_full() -> TestResult {
        // Given — a fresh estimator
        let mut rain = RainRate::new();

        // When — a tip then dry seconds, but fewer than a full window
        let first = rain.push(1);

        // Then — no rate yet: a partial window would extrapolate a few tips into a
        // cloudburst, so we report nothing rather than a wrong value.
        assert_eq!(first, None, "first tip must not yield a rate");
        for _ in 0..(RAIN_WINDOW_SECS - 2) {
            assert_eq!(rain.push(0), None, "must stay None until the window fills");
        }
        Ok(())
    }

    #[test]
    fn rain_rate_steady_drizzle_over_full_window() -> TestResult {
        // Given — exactly one tip per second for a full window
        let mut rain = RainRate::new();

        // When
        let mut rate = None;
        for _ in 0..RAIN_WINDOW_SECS {
            rate = rain.push(1);
        }

        // Then — once the window is full: RAIN_WINDOW_SECS tips over the full window
        // = 1 tip/s = 0.2794 mm/s × 3600 = 1005.84 mm/h.
        let expected = MM_PER_TIP * 3600.0;
        let Some(got) = rate else {
            return Err("rate must be Some after a full window".into());
        };
        assert!(
            (got - expected).abs() < 1.0,
            "expected ~{expected} mm/h, got {got}"
        );
        Ok(())
    }

    #[test]
    fn rain_rate_evicts_old_tips_after_window() -> TestResult {
        // Given — one burst of tips, then a full window of dry seconds
        let mut rain = RainRate::new();
        rain.push(10);

        // When — push a full window of dry seconds so the burst is evicted
        let mut rate = None;
        for _ in 0..RAIN_WINDOW_SECS {
            rate = rain.push(0);
        }

        // Then — window is full and the burst has scrolled out → Some(0.0)
        let Some(got) = rate else {
            return Err("rate must be Some once the window is full".into());
        };
        assert!(
            got.abs() < f32::EPSILON,
            "evicted burst should be 0, got {got}"
        );
        Ok(())
    }
}
// grcov exclude stop
