//! Pure domain model for the TUI: signal state, telemetry formatting,
//! and ring-buffer time series.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

// Re-export pure helpers from meteo-chart so existing call sites
// (`crate::model::gaussian_smooth`, `crate::model::fmt_lux`, etc.) resolve
// without modification.
pub use meteo_chart::{
    Trend, classify_trend, compass_label_fr, dew_point_c, fmt_battery_flow, fmt_location, fmt_lux,
    fmt_power, fmt_uptime, gaussian_smooth, lux_chart_unit, padded_value_bounds, power_w,
    value_axis_labels,
};

/// Dashboard signal state derived purely from frame age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalState {
    /// No frame has been received yet.
    NoSignal,
    /// The last frame arrived within `stale_after`.
    Live,
    /// Frames have been seen, but the latest is older than `stale_after`.
    Stale,
}

impl SignalState {
    /// Derive the state from the last-frame timestamp.
    ///
    /// `stale_after` is passed as a parameter so `model` does not depend on
    /// `app::STALE_AFTER`.
    #[must_use]
    pub fn from_age(last_frame_at: Option<Instant>, now: Instant, stale_after: Duration) -> Self {
        match last_frame_at {
            None => Self::NoSignal,
            Some(t) if now.duration_since(t) > stale_after => Self::Stale,
            Some(_) => Self::Live,
        }
    }
}

/// Capped time-series of `(seconds-since-session-start, value)` points for charting.
pub struct Series {
    points: VecDeque<(f64, f64)>,
    cap: usize,
}

impl Series {
    /// Default capacity: 600 points = 10 min at the 1 Hz feed.
    ///
    /// **Invariant:** the count cap (600 points) must cover [`Series::WINDOW_SECS`]
    /// (600 s) of wall-clock; this holds **only** if the producer pushes at ≤ 1 Hz.
    /// The `uptime_s` dedup in the scan loop enforces that rate — see the §4 dedup
    /// guard in `app.rs` that prevents the chart-truncation trap from returning
    /// silently.
    pub const DEFAULT_CAP: usize = 600;

    /// Visible chart window, in seconds. The x-axis is right-anchored at the
    /// latest sample and spans this many seconds backwards, so new points enter
    /// at the right edge and scroll left as the window fills. Matched to
    /// [`Series::DEFAULT_CAP`] at the 1 Hz feed (600 points ≈ 600 s).
    ///
    /// See [`Series::DEFAULT_CAP`] for the count/time invariant and the `uptime_s`
    /// dedup guard that upholds it.
    pub const WINDOW_SECS: f64 = 600.0;

    /// Create a new `Series` with the given capacity.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            points: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append a sample, dropping the oldest once `cap` is exceeded.
    pub fn push(&mut self, t_secs: f64, value: f64) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points.push_back((t_secs, value));
    }

    /// Return a contiguous slice of all stored `(t, value)` points.
    #[must_use]
    pub fn points(&mut self) -> &[(f64, f64)] {
        self.points.make_contiguous()
    }

    /// Returns `true` if no points are stored.
    #[must_use]
    #[allow(dead_code, reason = "test-only assertion helper")]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// `(min, max)` of the value axis, for ratatui `Axis` bounds; `None` if empty.
    #[must_use]
    pub fn y_bounds(&self) -> Option<(f64, f64)> {
        let mut it = self.points.iter().map(|p| p.1);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }

    /// `(first_t, last_t)` of the time axis; `None` if empty.
    #[must_use]
    #[allow(dead_code, reason = "test-only assertion helper")]
    pub fn x_bounds(&self) -> Option<(f64, f64)> {
        Some((self.points.front()?.0, self.points.back()?.0))
    }

    /// Right-anchored x-axis window `[lo, hi]` for charting: `hi` is the latest
    /// sample's timestamp (so the newest point sits at the right edge) and `lo`
    /// is `hi - WINDOW_SECS`. Points older than the window scroll off the left.
    /// Shaped as `[f64; 2]` to feed ratatui's `Axis::bounds` directly. `None` if
    /// empty.
    #[must_use]
    pub fn x_window(&self) -> Option<[f64; 2]> {
        let hi = self.points.back()?.0;
        Some([hi - Self::WINDOW_SECS, hi])
    }

    /// Maximum value among points whose timestamp is within `window_secs` of the
    /// latest point.
    ///
    /// Returns `None` if the series is empty. Drives the 60 s gust calculation.
    #[must_use]
    pub fn window_max(&self, window_secs: f64) -> Option<f64> {
        let last_t = self.points.back()?.0;
        self.points
            .iter()
            .filter(|(t, _)| *t >= last_t - window_secs)
            .map(|(_, v)| *v)
            .fold(None, |acc, v| Some(acc.map_or(v, |m: f64| m.max(v))))
    }

    /// Difference between the latest value and the oldest point within `window_secs`.
    ///
    /// Returns `None` if the series is empty. Drives the 10-min trend arrow.
    #[must_use]
    pub fn trend_delta(&self, window_secs: f64) -> Option<f64> {
        let (last_t, latest_v) = *self.points.back()?;
        let oldest_v = self
            .points
            .iter()
            .filter(|(t, _)| *t >= last_t - window_secs)
            .map(|(_, v)| *v)
            .next()?;
        Some(latest_v - oldest_v)
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

    // --- Series tests ---

    #[test]
    fn series_caps_at_capacity_dropping_oldest() -> TestResult {
        // Given
        let cap = 5_usize;
        let mut s = Series::new(cap);

        // When — push cap+5 = 10 samples; first 5 should be evicted
        for i in 0..10_i32 {
            s.push(f64::from(i), f64::from(i));
        }

        // Then
        let pts = s.points();
        assert_eq!(pts.len(), cap);
        // The oldest retained point should be the 6th pushed (index 5)
        assert_eq!(pts[0], (5.0, 5.0));
        Ok(())
    }

    #[test]
    fn series_push_preserves_order_and_bounds() -> TestResult {
        // Given
        let mut s = Series::new(Series::DEFAULT_CAP);

        // When
        s.push(0.0, 10.0);
        s.push(1.0, 5.0);
        s.push(2.0, 15.0);

        // Then — contiguous slice in push order
        let pts = s.points();
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0], (0.0, 10.0));
        assert_eq!(pts[1], (1.0, 5.0));
        assert_eq!(pts[2], (2.0, 15.0));

        // x_bounds: first=0.0, last=2.0
        assert_eq!(s.x_bounds(), Some((0.0, 2.0)));
        // y_bounds: min=5.0, max=15.0
        assert_eq!(s.y_bounds(), Some((5.0, 15.0)));
        Ok(())
    }

    #[test]
    fn x_window_right_anchors_on_latest() -> TestResult {
        // Given
        let mut s = Series::new(Series::DEFAULT_CAP);
        s.push(10.0, 1.0);
        s.push(42.0, 2.0);

        // When
        let [lo, hi] = s.x_window().ok_or("non-empty series has a window")?;

        // Then — hi is the latest timestamp; the window spans WINDOW_SECS back.
        assert!(
            (hi - 42.0).abs() < f64::EPSILON,
            "window hi should be the latest sample time"
        );
        assert!(
            (hi - lo - Series::WINDOW_SECS).abs() < f64::EPSILON,
            "window width should equal WINDOW_SECS"
        );
        Ok(())
    }

    #[test]
    fn x_window_empty_is_none() -> TestResult {
        // Given
        let s = Series::new(Series::DEFAULT_CAP);

        // When / Then
        assert_eq!(s.x_window(), None);
        Ok(())
    }

    // --- SignalState tests ---

    #[test]
    fn signal_state_no_signal() -> TestResult {
        // Given
        let now = Instant::now();

        // When
        let state = SignalState::from_age(None, now, Duration::from_secs(5));

        // Then
        assert_eq!(state, SignalState::NoSignal);
        Ok(())
    }

    #[test]
    fn signal_state_live() -> TestResult {
        // Given
        let now = Instant::now();

        // When — frame received exactly at `now`; age is zero, within stale_after
        let state = SignalState::from_age(Some(now), now, Duration::from_secs(5));

        // Then
        assert_eq!(state, SignalState::Live);
        Ok(())
    }

    #[test]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "test: Instant + Duration cannot overflow in practice"
    )]
    fn signal_state_stale() -> TestResult {
        // Given — simulate a frame received 10 s in the past by advancing `now`
        let base = Instant::now();
        let later = base + Duration::from_secs(10);

        // When
        let state = SignalState::from_age(Some(base), later, Duration::from_secs(5));

        // Then
        assert_eq!(state, SignalState::Stale);
        Ok(())
    }

    // --- Series::window_max tests ---

    #[test]
    fn series_window_max_only_within_window() -> TestResult {
        // Given — three points; only two fall inside the 60 s window
        let mut s = Series::new(Series::DEFAULT_CAP);
        s.push(0.0, 5.0);
        s.push(10.0, 9.0);
        s.push(70.0, 3.0);

        // When — last_t=70, window=60 → filter t≥10 → points (10,9) and (70,3)
        let result = s.window_max(60.0);

        // Then
        assert_eq!(result, Some(9.0));
        Ok(())
    }

    #[test]
    fn series_window_max_empty_is_none() -> TestResult {
        // Given
        let s = Series::new(Series::DEFAULT_CAP);

        // When / Then
        assert_eq!(s.window_max(60.0), None);
        Ok(())
    }

    // --- Series::trend_delta tests ---

    #[test]
    fn series_trend_delta_uses_oldest_in_window() -> TestResult {
        // Given — two points spanning exactly the window
        let mut s = Series::new(Series::DEFAULT_CAP);
        s.push(0.0, 10.0);
        s.push(600.0, 12.0);

        // When — window 600 s → oldest_in_window=(0,10), latest=(600,12)
        let result = s.trend_delta(600.0);

        // Then
        assert_eq!(result, Some(2.0));
        Ok(())
    }

    #[test]
    fn series_trend_delta_empty_is_none() -> TestResult {
        // Given
        let s = Series::new(Series::DEFAULT_CAP);

        // When / Then
        assert_eq!(s.trend_delta(600.0), None);
        Ok(())
    }
}
// grcov exclude stop
