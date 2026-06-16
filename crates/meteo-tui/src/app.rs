//! Application state and the pure `apply` reducer.
//!
//! [`AppState`] holds all render-time state. The [`AppState::apply`] method is a
//! pure reducer over [`BleEvent`]s, injecting `now: Instant` for testability.
//!
//! Link-state is authoritative (driven by [`crate::ble::BleEvent::State`]); frame
//! age is cosmetic only (drives value greying, never reconnection).

// All public items in this module are consumed by main.rs wiring and the UI added
// in substeps 6–7; suppress the dead_code lint that fires until then.
#![allow(dead_code, reason = "consumed by main.rs wiring + ui in substeps 6–7")]

use std::time::{Duration, Instant};

use meteo_lib::Telemetry;

use crate::ble::BleEvent;
use crate::model::{ConnState, Series};

/// How long without a frame before values are considered cosmetically stale.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// All render-time state for the TUI dashboard.
pub struct AppState {
    /// Current BLE connection state (authoritative).
    pub conn: ConnState,
    /// Most-recently decoded telemetry frame.
    pub latest: Telemetry,
    /// Wall-clock instant of the last successfully decoded frame, if any.
    pub last_frame_at: Option<Instant>,
    /// DIS Firmware Revision String read from the peripheral, if available.
    pub fw_version: Option<String>,
    /// Version string of this application binary.
    pub app_version: &'static str,
    /// Rolling temperature time series (seconds since session start, °C).
    pub temp: Series,
    /// Rolling pressure time series (seconds since session start, hPa).
    pub pressure: Series,
    /// Session start instant, used to compute relative timestamps.
    started: Instant,
}

impl AppState {
    /// Create a new [`AppState`] anchored at `now` (the session start time).
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            conn: ConnState::Scanning,
            latest: Telemetry::empty(),
            last_frame_at: None,
            fw_version: None,
            app_version: env!("CARGO_PKG_VERSION"),
            temp: Series::new(Series::DEFAULT_CAP),
            pressure: Series::new(Series::DEFAULT_CAP),
            started: now,
        }
    }

    /// Reduce one BLE event into state.
    ///
    /// `now` is injected so tests can control the clock without real sleeps.
    pub fn apply(&mut self, ev: BleEvent, now: Instant) {
        match ev {
            BleEvent::State(s) => self.conn = s,
            BleEvent::Firmware(v) => self.fw_version = v,
            BleEvent::Frame(t) => {
                // Extract optional fields before moving `t` into `self.latest`.
                // Telemetry is Copy, so this is a no-op reorder, but kept explicit
                // for clarity and to be safe if Copy is ever removed.
                let temp_c = t.temperature_c;
                let press_hpa = t.pressure_hpa;
                self.latest = t;
                self.last_frame_at = Some(now);
                let secs = now.duration_since(self.started).as_secs_f64();
                if let Some(v) = temp_c {
                    self.temp.push(secs, f64::from(v));
                }
                if let Some(v) = press_hpa {
                    self.pressure.push(secs, f64::from(v));
                }
            }
        }
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
    use crate::model::ConnState;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn apply_frame_updates_latest_and_series() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            temperature_c: Some(22.5),
            pressure_hpa: Some(1013.0),
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then
        assert_eq!(app.latest.temperature_c, Some(22.5));
        assert!(app.last_frame_at.is_some());
        assert_eq!(app.temp.points().len(), 1);
        assert_eq!(app.pressure.points().len(), 1);

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
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(t), base);

        // Then
        assert!(app.pressure.is_empty());
        assert_eq!(app.temp.points().len(), 1);

        Ok(())
    }

    #[test]
    fn apply_state_updates_conn() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);

        // When
        app.apply(BleEvent::State(ConnState::Reconnecting), base);

        // Then
        assert_eq!(app.conn, ConnState::Reconnecting);

        Ok(())
    }

    #[test]
    fn apply_firmware_sets_version() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);

        // When
        app.apply(BleEvent::Firmware(Some("0.1.0".to_owned())), base);

        // Then
        assert_eq!(app.fw_version, Some("0.1.0".to_owned()));

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
}
// grcov exclude stop
