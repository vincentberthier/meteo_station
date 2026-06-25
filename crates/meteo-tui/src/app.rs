//! Application state and the pure `apply` reducer.
//!
//! [`AppState`] holds all render-time state. The [`AppState::apply`] method is a
//! pure reducer over [`BleEvent`]s, injecting `now: Instant` for testability.
//!
//! Signal state is derived purely from frame age; data silence is cosmetic only
//! (drives value greying) and is never used to trigger a reconnect.

use std::time::{Duration, Instant};

use meteo_lib::Telemetry;

use crate::ble::BleEvent;
use crate::model::{Series, SignalState};

/// How long without a frame before values are considered cosmetically stale.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// All render-time state for the TUI dashboard.
pub struct AppState {
    /// Most-recently decoded telemetry frame.
    pub latest: Telemetry,
    /// Wall-clock instant of the last successfully decoded frame, if any.
    pub last_frame_at: Option<Instant>,
    /// Version string of this application binary.
    pub app_version: &'static str,
    /// Rolling temperature time series (seconds since session start, °C).
    pub temp: Series,
    /// Rolling sky/IR temperature time series (seconds since session start, °C).
    pub sky: Series,
    /// Rolling pressure time series (seconds since session start, hPa).
    pub pressure: Series,
    /// Rolling luminosity time series (seconds since session start, lux).
    pub lux: Series,
    /// Rolling wind-speed time series (seconds since session start, m/s).
    pub wind: Series,
    /// Rolling relative-humidity time series (seconds since session start, %RH).
    pub humidity: Series,
    /// Session start instant, used to compute relative timestamps.
    started: Instant,
}

impl AppState {
    /// Create a new [`AppState`] anchored at `now` (the session start time).
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            latest: Telemetry::empty(),
            last_frame_at: None,
            app_version: env!("CARGO_PKG_VERSION"),
            temp: Series::new(Series::DEFAULT_CAP),
            sky: Series::new(Series::DEFAULT_CAP),
            pressure: Series::new(Series::DEFAULT_CAP),
            lux: Series::new(Series::DEFAULT_CAP),
            wind: Series::new(Series::DEFAULT_CAP),
            humidity: Series::new(Series::DEFAULT_CAP),
            started: now,
        }
    }

    /// Reduce one BLE event into state.
    ///
    /// `now` is injected so tests can control the clock without real sleeps.
    pub fn apply(&mut self, ev: BleEvent, now: Instant) {
        let BleEvent::Frame(t) = ev;
        // Extract optional fields before moving `t` into `self.latest`.
        let temp_c = t.temperature_c;
        let sky_c = t.sky_temp_c;
        let press_hpa = t.pressure_hpa;
        let lux = t.luminosity_lux;
        let wind_ms = t.wind_speed_ms;
        let humidity = t.humidity_pct;
        self.latest = t;
        self.last_frame_at = Some(now);
        let secs = now.duration_since(self.started).as_secs_f64();
        if let Some(v) = temp_c {
            self.temp.push(secs, f64::from(v));
        }
        if let Some(v) = sky_c {
            self.sky.push(secs, f64::from(v));
        }
        if let Some(v) = press_hpa {
            self.pressure.push(secs, f64::from(v));
        }
        if let Some(v) = lux {
            self.lux.push(secs, f64::from(v));
        }
        if let Some(v) = wind_ms {
            self.wind.push(secs, f64::from(v));
        }
        if let Some(v) = humidity {
            self.humidity.push(secs, f64::from(v));
        }
    }

    /// Derive the current [`SignalState`] from the age of the last received frame.
    ///
    /// **Cosmetic only** — drives header colour and value greying. Must never
    /// be used to trigger a reconnect.
    #[must_use]
    pub fn signal_state(&self, now: Instant) -> SignalState {
        SignalState::from_age(self.last_frame_at, now, STALE_AFTER)
    }

    /// Returns `true` when no frame has arrived, or the last frame arrived more
    /// than `max_age` ago.
    ///
    /// This is **cosmetic only** — it drives value greying in the UI and must
    /// never be used to trigger a reconnect.
    #[must_use]
    pub fn is_stale(&self, now: Instant, max_age: Duration) -> bool {
        self.last_frame_at
            .is_none_or(|t| now.duration_since(t) > max_age)
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
    use core::{error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn apply_frame_updates_latest_and_series() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            temperature_c: Some(22.5),
            sky_temp_c: Some(-8.0),
            pressure_hpa: Some(1013.0),
            luminosity_lux: Some(1200.0),
            wind_speed_ms: Some(3.5),
            humidity_pct: Some(55.0),
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then
        assert_eq!(app.latest.temperature_c, Some(22.5));
        assert!(app.last_frame_at.is_some());
        assert_eq!(app.temp.points().len(), 1);
        assert_eq!(app.sky.points().len(), 1);
        assert_eq!(app.pressure.points().len(), 1);
        assert_eq!(app.lux.points().len(), 1);
        assert_eq!(app.wind.points().len(), 1);
        assert_eq!(app.humidity.points().len(), 1);

        Ok(())
    }

    #[test]
    fn apply_frame_skips_none_fields_in_series() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            temperature_c: Some(22.5),
            pressure_hpa: None,
            luminosity_lux: None,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then
        assert!(app.pressure.is_empty());
        assert!(app.lux.is_empty());
        assert_eq!(app.temp.points().len(), 1);

        Ok(())
    }

    #[test]
    fn is_stale_true_before_first_frame() -> TestResult {
        // Given
        let base = Instant::now();
        let app = AppState::new(base);

        // When / Then
        assert!(app.is_stale(base, STALE_AFTER));

        Ok(())
    }

    #[test]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "test: Instant + Duration cannot overflow in practice"
    )]
    fn is_stale_false_within_window() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            temperature_c: Some(20.0),
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then — 1 s after the frame, still within STALE_AFTER (5 s)
        assert!(!app.is_stale(base + Duration::from_secs(1), STALE_AFTER));

        Ok(())
    }

    #[test]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "test: Instant + Duration cannot overflow in practice"
    )]
    fn is_stale_true_after_window() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            temperature_c: Some(20.0),
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then — STALE_AFTER + 1 s after the frame → stale
        assert!(app.is_stale(base + STALE_AFTER + Duration::from_secs(1), STALE_AFTER));

        Ok(())
    }

    #[test]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "test: Instant + Duration cannot overflow in practice"
    )]
    fn signal_state_transitions() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);

        // No frame yet → NoSignal
        assert_eq!(app.signal_state(base), SignalState::NoSignal);

        // When — apply a frame at `base`
        let t = Telemetry {
            temperature_c: Some(20.0),
            ..Telemetry::empty()
        };
        app.apply(BleEvent::Frame(t), base);

        // Then — immediately → Live
        assert_eq!(app.signal_state(base), SignalState::Live);

        // Then — 1 s later, still within STALE_AFTER (5 s) → Live
        assert_eq!(
            app.signal_state(base + Duration::from_secs(1)),
            SignalState::Live
        );

        // Then — STALE_AFTER + 1 s later → Stale
        assert_eq!(
            app.signal_state(base + STALE_AFTER + Duration::from_secs(1)),
            SignalState::Stale
        );

        Ok(())
    }
}
// grcov exclude stop
