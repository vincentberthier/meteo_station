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
use crate::model::{self, Series, SignalState};

/// How long without a frame before values are considered cosmetically stale.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// Fallback station name displayed in the header when the BLE advertisement
/// carries no alias.
#[allow(dead_code, reason = "wired in a later rendering substep")]
pub const STATION_DEFAULT: &str = "MeteoStation";

/// All render-time state for the TUI dashboard.
pub struct AppState {
    /// Most-recently decoded telemetry frame.
    pub latest: Telemetry,
    /// Wall-clock instant of the last successfully decoded frame, if any.
    pub last_frame_at: Option<Instant>,
    /// Version string of this application binary.
    pub app_version: &'static str,
    /// Latest advertised RSSI (dBm), updated on every event (including duplicates).
    pub rssi: Option<i16>,
    /// BLE alias of the station; header falls back to [`STATION_DEFAULT`].
    pub station: Option<String>,
    /// Number of distinct frames (by `uptime_s`) received this session.
    pub frame_count: u64,
    /// Dedup key: `uptime_s` of the last distinct frame counted.
    last_uptime_s: Option<u32>,
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
    /// Rolling 60-second gust (peak wind in the trailing 60 s) time series.
    pub gust: Series,
    /// Rolling wind-direction trail (seconds since session start, degrees).
    pub heading: Series,
    /// Rolling battery voltage time series (seconds since session start, V).
    pub batt_v: Series,
    /// Rolling solar power time series (seconds since session start, W).
    pub solar_w: Series,
    /// Rolling load power time series (seconds since session start, W).
    pub load_w: Series,
    /// Rolling rain-rate time series (seconds since session start, mm/h).
    pub rain: Series,
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
            rssi: None,
            station: None,
            frame_count: 0,
            last_uptime_s: None,
            temp: Series::new(Series::DEFAULT_CAP),
            sky: Series::new(Series::DEFAULT_CAP),
            pressure: Series::new(Series::DEFAULT_CAP),
            lux: Series::new(Series::DEFAULT_CAP),
            wind: Series::new(Series::DEFAULT_CAP),
            humidity: Series::new(Series::DEFAULT_CAP),
            gust: Series::new(Series::DEFAULT_CAP),
            heading: Series::new(Series::DEFAULT_CAP),
            batt_v: Series::new(Series::DEFAULT_CAP),
            solar_w: Series::new(Series::DEFAULT_CAP),
            load_w: Series::new(Series::DEFAULT_CAP),
            rain: Series::new(Series::DEFAULT_CAP),
            started: now,
        }
    }

    /// Reduce one BLE event into state.
    ///
    /// `now` is injected so tests can control the clock without real sleeps.
    ///
    /// Instantaneous fields (`latest`, `last_frame_at`, `rssi`, `station`) are
    /// updated on **every** event so the header and link-liveness indicators stay
    /// responsive even during the ~6 Hz duplicate-advert bursts from `BlueZ`.
    ///
    /// Historical series and `frame_count` are gated on a **distinct** `uptime_s`
    /// (the chart-truncation fix): duplicate advertisements share the same
    /// firmware-side `uptime_s` and must not over-sample the time series.
    pub fn apply(&mut self, ev: BleEvent, now: Instant) {
        let BleEvent::Frame(fe) = ev;
        let t = fe.telemetry;
        // Always update instantaneous state + liveness (fires ~6×/s on duplicates).
        if let Some(r) = fe.rssi {
            self.rssi = Some(r);
        }
        if let Some(s) = fe.station {
            self.station = Some(s);
        }
        self.latest = t;
        self.last_frame_at = Some(now);

        // Gate historical series on a NEW device-second; duplicate adverts carry the
        // same uptime_s and must not over-sample the charts.
        if self.last_uptime_s == Some(t.uptime_s) {
            return;
        }
        self.last_uptime_s = Some(t.uptime_s);
        self.frame_count = self.frame_count.saturating_add(1);

        let secs = now.duration_since(self.started).as_secs_f64();
        if let Some(v) = t.temperature_c {
            self.temp.push(secs, f64::from(v));
        }
        if let Some(v) = t.sky_temp_c {
            self.sky.push(secs, f64::from(v));
        }
        if let Some(v) = t.pressure_hpa {
            self.pressure.push(secs, f64::from(v));
        }
        if let Some(v) = t.luminosity_lux {
            self.lux.push(secs, f64::from(v));
        }
        // Push wind speed BEFORE computing window_max so the current sample is
        // included in the 60-second gust calculation.
        if let Some(v) = t.wind_speed_ms {
            self.wind.push(secs, f64::from(v));
        }
        if let Some(g) = self.wind.window_max(60.0) {
            self.gust.push(secs, g);
        }
        if let Some(v) = t.humidity_pct {
            self.humidity.push(secs, f64::from(v));
        }
        if let Some(d) = t.wind_dir_deg {
            self.heading.push(secs, f64::from(d));
        }
        if let Some(mv) = t.batt_mv {
            self.batt_v.push(secs, f64::from(mv) / 1000.0);
        }
        if let Some(w) = model::power_w(t.solar_mv, t.solar_ma) {
            self.solar_w.push(secs, w);
        }
        if let Some(w) = model::power_w(t.batt_mv, t.load_ma) {
            self.load_w.push(secs, w);
        }
        if let Some(r) = t.rain_rate_mm_h {
            self.rain.push(secs, f64::from(r));
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
    #[allow(dead_code, reason = "old table-renderer helper pending cleanup")]
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

    use crate::ble::FrameEvent;

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
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

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
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

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
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

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
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

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
            uptime_s: 1,
            ..Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

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

    // ── Dedup / frame_count tests ─────────────────────────────────────────────

    #[test]
    fn apply_dedupes_duplicate_uptime() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t1 = Telemetry {
            temperature_c: Some(22.5),
            uptime_s: 10,
            ..Telemetry::empty()
        };
        let t2 = Telemetry {
            temperature_c: Some(23.0),
            uptime_s: 10, // same uptime_s → duplicate
            ..Telemetry::empty()
        };
        let t3 = Telemetry {
            temperature_c: Some(23.5),
            uptime_s: 11, // new uptime_s → distinct
            ..Telemetry::empty()
        };

        // When — first frame then duplicate
        app.apply(BleEvent::Frame(FrameEvent::new(t1)), base);
        app.apply(BleEvent::Frame(FrameEvent::new(t2)), base);

        // Then — duplicate not counted in series or frame_count
        assert_eq!(
            app.frame_count, 1,
            "duplicate must not increment frame_count"
        );
        assert_eq!(
            app.temp.points().len(),
            1,
            "duplicate must not add a second series point"
        );

        // When — new distinct uptime_s
        app.apply(BleEvent::Frame(FrameEvent::new(t3)), base);

        // Then — new frame counted
        assert_eq!(app.frame_count, 2);
        assert_eq!(app.temp.points().len(), 2);

        Ok(())
    }

    #[test]
    fn apply_updates_latest_on_duplicate() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t1 = Telemetry {
            temperature_c: Some(22.5),
            uptime_s: 5,
            ..Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t1)), base);

        // When — duplicate uptime_s but changed temperature and rssi
        let t2 = Telemetry {
            temperature_c: Some(99.0),
            uptime_s: 5, // same uptime_s → duplicate
            ..Telemetry::empty()
        };
        let fe2 = FrameEvent {
            telemetry: t2,
            rssi: Some(-70),
            station: None,
        };
        app.apply(BleEvent::Frame(fe2), base);

        // Then — instantaneous state updated, but series and frame_count unchanged
        assert_eq!(
            app.latest.temperature_c,
            Some(99.0),
            "latest must reflect the most recent event even on a duplicate"
        );
        assert_eq!(
            app.rssi,
            Some(-70),
            "rssi must update on every event, including duplicates"
        );
        assert_eq!(
            app.frame_count, 1,
            "frame_count must not increment on duplicate"
        );
        assert_eq!(
            app.temp.points().len(),
            1,
            "series must not gain a point on duplicate"
        );

        Ok(())
    }

    #[test]
    fn apply_increments_frame_count() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t1 = Telemetry {
            uptime_s: 10,
            ..Telemetry::empty()
        };
        let t2 = Telemetry {
            uptime_s: 11,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t1)), base);
        app.apply(BleEvent::Frame(FrameEvent::new(t2)), base);

        // Then
        assert_eq!(app.frame_count, 2);

        Ok(())
    }

    #[test]
    fn apply_carries_rssi_and_station() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            uptime_s: 1,
            ..Telemetry::empty()
        };
        let fe = FrameEvent {
            telemetry: t,
            rssi: Some(-65),
            station: Some("rooftop-01".to_owned()),
        };

        // When
        app.apply(BleEvent::Frame(fe), base);

        // Then
        assert_eq!(app.rssi, Some(-65));
        assert_eq!(app.station.as_deref(), Some("rooftop-01"));

        Ok(())
    }

    #[test]
    fn apply_derives_power_series() -> TestResult {
        // Given — solar: 15 V × 0.6 A = 9 W; load: 3.9 V × 0.12 A = 0.468 W
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            solar_mv: Some(15_000),
            solar_ma: Some(600),
            batt_mv: Some(3_900),
            load_ma: Some(120),
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

        // Then — solar_w ≈ 9.0 W
        let solar_last = app
            .solar_w
            .points()
            .last()
            .copied()
            .ok_or("solar_w empty")?;
        assert!(
            (solar_last.1 - 9.0).abs() < 1e-9,
            "solar_w should be ≈ 9.0 W, got {}",
            solar_last.1
        );

        // Then — load_w ≈ 0.468 W
        let load_last = app.load_w.points().last().copied().ok_or("load_w empty")?;
        assert!(
            (load_last.1 - 0.468).abs() < 1e-9,
            "load_w should be ≈ 0.468 W, got {}",
            load_last.1
        );

        // Then — batt_v ≈ 3.9 V
        let batt_last = app.batt_v.points().last().copied().ok_or("batt_v empty")?;
        assert!(
            (batt_last.1 - 3.9).abs() < 1e-9,
            "batt_v should be ≈ 3.9 V, got {}",
            batt_last.1
        );

        Ok(())
    }

    #[test]
    fn apply_pushes_heading_gust_rain() -> TestResult {
        // Given
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            wind_speed_ms: Some(4.0),
            wind_dir_deg: Some(270.0),
            rain_rate_mm_h: Some(1.5),
            uptime_s: 1,
            ..Telemetry::empty()
        };

        // When
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);

        // Then
        assert_eq!(
            app.heading.points().len(),
            1,
            "heading should have one point"
        );
        assert_eq!(app.gust.points().len(), 1, "gust should have one point");
        assert_eq!(app.rain.points().len(), 1, "rain should have one point");

        Ok(())
    }
}
// grcov exclude stop
