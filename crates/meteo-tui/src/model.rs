//! Pure domain model for the TUI: signal state, telemetry formatting,
//! and ring-buffer time series.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

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

/// Format the station location row.
///
/// Returns `"not set"` until both latitude and longitude are present; otherwise
/// `"{lat:.2}, {lon:.2}"` or `"{lat:.2}, {lon:.2}, {alt:.0} m"` when altitude
/// is also set. Coarse values render at 2 decimals (lat/lon, ~1 km precision).
#[must_use]
pub fn fmt_location(lat: Option<f32>, lon: Option<f32>, alt: Option<f32>) -> String {
    match (lat, lon) {
        (Some(la), Some(lo)) => alt.map_or_else(
            || format!("{la:.2}, {lo:.2}"),
            |a| format!("{la:.2}, {lo:.2}, {a:.0} m"),
        ),
        _ => "not set".to_owned(),
    }
}

/// Format luminosity in kilolux.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.1} klx"`.
#[must_use]
pub fn fmt_lux_klx(lux: Option<f32>) -> String {
    lux.map_or_else(
        || "N/A".to_owned(),
        |x| format!("{:.1} klx", f64::from(x) / 1000.0),
    )
}

/// French 16-point compass label for a heading in degrees.
///
/// Convention: 0°=N, 90°=E, 180°=S, 270°=O (Ouest). 22.5° sector bucketing;
/// returns the French rose: `N NNE NE ENE E ESE SE SSE S SSO SO OSO O ONO NO NNO`.
#[must_use]
pub fn compass_label_fr(deg: f32) -> &'static str {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSO", "SO", "OSO", "O", "ONO",
        "NO", "NNO",
    ];
    let norm = deg.rem_euclid(360.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "round() of a value in [0,16] is a small non-negative whole number"
    )]
    let sector = (norm / 22.5).round() as usize;
    // 360° rounds up to sector 16, which wraps back to N (sector 0).
    let idx = if sector >= POINTS.len() { 0 } else { sector };
    POINTS[idx]
}

/// Power in watts from bus millivolts × current milliamperes.
///
/// Returns `(mv / 1000) × (ma / 1000)` as `Some(f64)`, or `None` if either
/// input is `None`.
#[must_use]
pub fn power_w(mv: Option<u16>, ma: Option<u16>) -> Option<f64> {
    Some((f64::from(mv?) / 1000.0) * (f64::from(ma?) / 1000.0))
}

/// Nominal 1S-LiPo energy budget for the crude autonomy estimate (best-effort).
pub const BATTERY_WH: f64 = 9.6; // 3.7 V × 2.6 Ah

/// Battery flow status line for the ÉNERGIE card.
///
/// `net = solar_w − load_w`. Returns the rendered line:
/// - `net > 0` → `"▲ en charge · +{net:.1} W"`
/// - `net < 0` → `"▼ décharge · {net:.1} W · ~{h:.1} h"` (autonomy from `pct`
///   and [`BATTERY_WH`])
/// - `net ≈ 0` → `"— stable"`
///
/// Returns `"N/A"` when either power reading is `None`.
#[must_use]
pub fn fmt_battery_flow(solar_w: Option<f64>, load_w: Option<f64>, pct: Option<u8>) -> String {
    let (Some(s), Some(l)) = (solar_w, load_w) else {
        return "N/A".to_owned();
    };
    let net = s - l;
    if net > 0.05 {
        format!("▲ en charge · +{net:.1} W")
    } else if net < -0.05 {
        let autonomy = pct.map(|p| BATTERY_WH * f64::from(p) / 100.0 / l);
        autonomy.map_or_else(
            || format!("▼ décharge · {net:.1} W"),
            |h| format!("▼ décharge · {net:.1} W · ~{h:.1} h"),
        )
    } else {
        "— stable".to_owned()
    }
}

/// Dew point in °C computed from the Magnus/WMO formula (a=17.62, b=243.12 °C).
///
/// `Td = b·γ / (a−γ)` with `γ = ln(rh/100) + a·t/(b+t)`.
/// `rh` is clamped to `(0.01, 100]` to avoid `ln(0)`.
#[must_use]
pub fn dew_point_c(temp_c: f32, rh_pct: f32) -> f32 {
    const A: f32 = 17.62;
    const B: f32 = 243.12;
    let rh = rh_pct.clamp(0.01, 100.0) / 100.0;
    let gamma = rh.ln() + (A * temp_c) / (B + temp_c);
    B * gamma / (A - gamma)
}

/// 10-min air-temperature trend classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// Temperature is increasing.
    Rising,
    /// Temperature is decreasing.
    Falling,
    /// Temperature change is within the stability epsilon.
    Stable,
}

/// Classify a trend delta.
///
/// Returns [`Trend::Stable`] if `|delta| < eps`, [`Trend::Rising`] for a positive
/// delta, and [`Trend::Falling`] for a negative delta.
#[must_use]
pub fn classify_trend(delta: f64, eps: f64) -> Trend {
    if delta.abs() < eps {
        Trend::Stable
    } else if delta > 0.0 {
        Trend::Rising
    } else {
        Trend::Falling
    }
}

/// Format an uptime duration as a compact human-readable label.
///
/// - ≥ 3600 s → `"{h}h{mm}m"` (e.g. 3725 → `"1h02m"`)
/// - ≥ 60 s   → `"{m}m{ss}s"` (e.g. 90 → `"1m30s"`)
/// - < 60 s   → `"0m{ss}s"` (e.g. 45 → `"0m45s"`)
///
/// Minutes and seconds are zero-padded to two digits; hours are unpadded.
#[must_use]
pub fn fmt_uptime(secs: u32) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let mm = (secs % 3600) / 60;
        format!("{h}h{mm:02}m")
    } else if secs >= 60 {
        let m = secs / 60;
        let ss = secs % 60;
        format!("{m}m{ss:02}s")
    } else {
        format!("0m{secs:02}s")
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

/// Pad a value range so the chart line never sits flush against the axis, and a
/// degenerate (single-point or flat) series stays visible.
///
/// For a zero-width range (`min == max`) the bounds open to `±1.0`; otherwise a
/// 5 % margin is added on each side. Returns `[lo, hi]` (with `lo < hi`), shaped
/// to feed ratatui's `Axis::bounds` directly.
///
/// `floor` clamps the lower bound for physically non-negative metrics
/// (e.g. luminosity, humidity): pass `Some(0.0)` so the padding can never render
/// an unphysical negative axis label. Metrics that legitimately go negative
/// (temperature) pass `None`. The clamp only ever raises `lo`, so `lo < hi`
/// holds as long as `hi` exceeds the floor (always true for real data).
#[must_use]
pub fn padded_value_bounds(min: f64, max: f64, floor: Option<f64>) -> [f64; 2] {
    let span = max - min;
    let [lo, hi] = if span.abs() < f64::EPSILON {
        [min - 1.0, max + 1.0]
    } else {
        let margin = span * 0.05;
        [min - margin, max + margin]
    };
    match floor {
        Some(f) if lo < f => [f, hi],
        _ => [lo, hi],
    }
}

/// Three evenly spaced tick labels for a value axis spanning `bounds` (`[lo, hi]`),
/// each formatted to `prec` decimals (bottom, middle, top).
#[must_use]
pub fn value_axis_labels(bounds: [f64; 2], prec: usize) -> [String; 3] {
    let [lo, hi] = bounds;
    let mid = f64::midpoint(lo, hi);
    [
        format!("{lo:.prec$}"),
        format!("{mid:.prec$}"),
        format!("{hi:.prec$}"),
    ]
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

    // --- axis-helper tests ---

    #[test]
    fn padded_value_bounds_equal_expands() -> TestResult {
        // Given a degenerate (single-value) range
        // When
        let [lo, hi] = padded_value_bounds(5.0, 5.0, None);

        // Then — opens to ±1 so the flat line stays visible
        assert!(lo < 5.0, "lo should drop below the value");
        assert!(hi > 5.0, "hi should rise above the value");
        assert!((lo - 4.0).abs() < f64::EPSILON);
        assert!((hi - 6.0).abs() < f64::EPSILON);
        Ok(())
    }

    #[test]
    fn padded_value_bounds_range_adds_margin() -> TestResult {
        // Given a non-degenerate range
        // When
        let [lo, hi] = padded_value_bounds(0.0, 10.0, None);

        // Then — 5 % margin each side
        assert!((lo - -0.5).abs() < f64::EPSILON, "lo should be -0.5");
        assert!((hi - 10.5).abs() < f64::EPSILON, "hi should be 10.5");
        Ok(())
    }

    #[test]
    fn padded_value_bounds_floor_clamps_negative_lower_bound() -> TestResult {
        // Given a spike-over-low-baseline range whose 5 % margin would push the
        // padded lower bound below zero (the negative-lux case)
        // When a zero floor is applied
        let [lo, hi] = padded_value_bounds(2.0, 3426.0, Some(0.0));

        // Then — lower bound is clamped to 0, upper bound keeps its margin
        assert!((lo - 0.0).abs() < f64::EPSILON, "lo should clamp to 0.0");
        assert!(hi > 3426.0, "hi should keep its upper margin");
        Ok(())
    }

    #[test]
    fn padded_value_bounds_floor_leaves_positive_lower_bound() -> TestResult {
        // Given a range already well above the floor
        // When a zero floor is applied
        let [lo, hi] = padded_value_bounds(100.0, 200.0, Some(0.0));

        // Then — the floor does not raise an already-positive lower bound
        assert!(
            (lo - 95.0).abs() < f64::EPSILON,
            "lo should keep its margin (95.0)"
        );
        assert!((hi - 205.0).abs() < f64::EPSILON, "hi should be 205.0");
        Ok(())
    }

    #[test]
    fn value_axis_labels_formats_min_mid_max() -> TestResult {
        // Given / When
        let labels = value_axis_labels([0.0, 10.0], 1);

        // Then
        assert_eq!(
            labels,
            ["0.0".to_owned(), "5.0".to_owned(), "10.0".to_owned()]
        );
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

    // --- fmt_location tests ---

    #[test]
    fn fmt_location_set() -> TestResult {
        // Given / When / Then — both lat and lon present, with altitude
        assert_eq!(
            fmt_location(Some(48.85), Some(2.35), Some(35.0)),
            "48.85, 2.35, 35 m"
        );
        // Without altitude
        assert_eq!(fmt_location(Some(48.85), Some(2.35), None), "48.85, 2.35");
        Ok(())
    }

    #[test]
    fn fmt_location_unset() -> TestResult {
        // Given / When / Then — missing lat or lon → "not set"
        assert_eq!(fmt_location(None, None, None), "not set");
        assert_eq!(fmt_location(Some(48.85), None, Some(35.0)), "not set");
        assert_eq!(fmt_location(None, Some(2.35), None), "not set");
        Ok(())
    }

    // --- dew_point_c tests ---

    #[test]
    fn dew_point_known_value() -> TestResult {
        // Given
        let temp_c = 20.0_f32;
        let rh_pct = 50.0_f32;

        // When
        let result = dew_point_c(temp_c, rh_pct);

        // Then — Magnus formula for 20 °C / 50 % RH ≈ 9.3 °C
        assert!(
            (result - 9.3_f32).abs() < 0.3,
            "dew point should be ≈ 9.3 °C, got {result}"
        );
        Ok(())
    }

    #[test]
    fn dew_point_saturated_equals_temp() -> TestResult {
        // Given — saturated air (100 % RH) → dew point equals air temperature
        let temp_c = 15.0_f32;
        let rh_pct = 100.0_f32;

        // When
        let result = dew_point_c(temp_c, rh_pct);

        // Then
        assert!(
            (result - temp_c).abs() < 0.05,
            "at 100 % RH dew point should equal temp (15 °C), got {result}"
        );
        Ok(())
    }

    // --- compass_label_fr tests ---

    #[test]
    fn compass_label_fr_cardinals_and_west_is_o() -> TestResult {
        // Given / When / Then — four cardinals; West is "O" in French
        assert_eq!(compass_label_fr(0.0), "N");
        assert_eq!(compass_label_fr(90.0), "E");
        assert_eq!(compass_label_fr(180.0), "S");
        assert_eq!(compass_label_fr(270.0), "O");
        // Inter-cardinal points from the spec
        assert_eq!(compass_label_fr(202.5), "SSO");
        assert_eq!(compass_label_fr(337.5), "NNO");
        Ok(())
    }

    // --- classify_trend tests ---

    #[test]
    fn classify_trend_bands() -> TestResult {
        // Given / When / Then
        assert_eq!(classify_trend(0.05, 0.1), Trend::Stable);
        assert_eq!(classify_trend(0.3, 0.1), Trend::Rising);
        assert_eq!(classify_trend(-0.3, 0.1), Trend::Falling);
        Ok(())
    }

    // --- fmt_lux_klx tests ---

    #[test]
    fn fmt_lux_klx_divides_by_1000() -> TestResult {
        // Given / When / Then
        assert_eq!(fmt_lux_klx(Some(3426.0)), "3.4 klx");
        assert_eq!(fmt_lux_klx(None), "N/A");
        Ok(())
    }

    // --- power_w tests ---

    #[test]
    fn power_w_multiplies() -> TestResult {
        // Given — 15.0 V, 600 mA → 9.0 W
        let result = power_w(Some(15_000), Some(600));

        // Then
        assert!(
            (result.ok_or("expected Some")? - 9.0).abs() < 1e-9,
            "power should be 9.0 W"
        );
        // None propagates when either input is None
        assert_eq!(power_w(None, Some(600)), None);
        assert_eq!(power_w(Some(15_000), None), None);
        Ok(())
    }

    // --- fmt_battery_flow tests ---

    #[test]
    fn fmt_battery_flow_charging_and_discharging() -> TestResult {
        // Given — solar > load: charging
        let charging = fmt_battery_flow(Some(5.0), Some(2.0), Some(80));

        // Then
        assert!(
            charging.starts_with("▲ en charge"),
            "charging line should start with '▲ en charge', got: {charging}"
        );

        // Given — load > solar: discharging with autonomy
        let discharging = fmt_battery_flow(Some(1.0), Some(3.0), Some(50));

        // Then
        assert!(
            discharging.starts_with("▼ décharge"),
            "discharge line should start with '▼ décharge', got: {discharging}"
        );
        assert!(
            discharging.contains('h'),
            "discharge line should contain autonomy hours, got: {discharging}"
        );
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

    // --- fmt_uptime tests ---

    #[test]
    fn fmt_uptime_hours() -> TestResult {
        // Given — 3725 s = 1 h 2 m 5 s → "1h02m"
        // When
        let result = fmt_uptime(3725);

        // Then
        assert_eq!(result, "1h02m");
        Ok(())
    }

    #[test]
    fn fmt_uptime_minutes() -> TestResult {
        // Given — 90 s = 1 m 30 s → "1m30s"
        // When
        let result = fmt_uptime(90);

        // Then
        assert_eq!(result, "1m30s");
        Ok(())
    }

    #[test]
    fn fmt_uptime_seconds_only() -> TestResult {
        // Given — 45 s → "0m45s"
        // When
        let result = fmt_uptime(45);

        // Then
        assert_eq!(result, "0m45s");
        Ok(())
    }
}
// grcov exclude stop
