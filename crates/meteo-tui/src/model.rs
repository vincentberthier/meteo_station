//! Pure domain model for the TUI: connection state machine, telemetry formatting,
//! ring-buffer time series, and firmware-version parsing.

// All public items in this module are consumed by later substeps; suppress the
// dead_code lint that fires because main.rs does not yet call them.
#![allow(dead_code, reason = "consumed by BLE, UI, and app substeps")]

use std::collections::VecDeque;

use meteo_lib::Diagnostics;

/// BLE connection state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Adapter is scanning for the peripheral.
    Scanning,
    /// Device found; initiating GATT connection.
    Connecting,
    /// Connected; resolving services and acquiring notify handle.
    Resolving,
    /// Subscribed and receiving telemetry.
    Live,
    /// Link was lost; preparing to rescan and reconnect.
    Reconnecting,
}

/// Authoritative link-state events.
///
/// NOTE: there is deliberately **no** frame-age variant here — data-flow silence
/// must never drive reconnection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    /// BLE adapter started scanning.
    ScanStarted,
    /// Target device appeared in scan results.
    DeviceFound,
    /// GATT connection established.
    Connected,
    /// Services resolved and notify I/O handle acquired.
    Subscribed,
    /// `BlueZ` `Connected` property went false, or notify reader reached EOF.
    LinkLost,
    /// Bounded per-step deadline elapsed or connect error.
    AttemptFailed,
}

impl ConnState {
    /// Pure state transition.
    ///
    /// `LinkLost`/`AttemptFailed` from any state → `Reconnecting`;
    /// `ScanStarted` → `Scanning`; happy path: `Scanning→Connecting→Resolving→Live`.
    #[must_use]
    pub const fn next(self, ev: LinkEvent) -> Self {
        match (self, ev) {
            (_, LinkEvent::LinkLost | LinkEvent::AttemptFailed) => Self::Reconnecting,
            (_, LinkEvent::ScanStarted) => Self::Scanning,
            (Self::Scanning, LinkEvent::DeviceFound) => Self::Connecting,
            (Self::Connecting, LinkEvent::Connected) => Self::Resolving,
            (Self::Resolving, LinkEvent::Subscribed) => Self::Live,
            (cur, _) => cur,
        }
    }

    /// Human-readable label for the connection state, suitable for the TUI status bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Scanning => "Scanning",
            Self::Connecting => "Connecting",
            Self::Resolving => "Resolving",
            Self::Live => "Live",
            Self::Reconnecting => "Reconnecting",
        }
    }
}

/// Format a temperature value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.1} °C"`.
#[must_use]
pub fn fmt_temp(v: Option<f32>) -> String {
    fmt_unit(v, "°C", 1)
}

/// Format a pressure value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.1} hPa"`.
#[must_use]
pub fn fmt_pressure(v: Option<f32>) -> String {
    fmt_unit(v, "hPa", 1)
}

/// Format a relative-humidity value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.0} %RH"`.
#[must_use]
pub fn fmt_humidity(v: Option<f32>) -> String {
    fmt_unit(v, "%RH", 0)
}

/// Format an illuminance value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.0} lux"`.
#[must_use]
pub fn fmt_lux(v: Option<f32>) -> String {
    fmt_unit(v, "lux", 0)
}

/// Format a wind-speed value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.1} m/s"`.
#[must_use]
pub fn fmt_wind_speed(v: Option<f32>) -> String {
    fmt_unit(v, "m/s", 1)
}

/// Format a wind-direction value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value:.0} °"`.
#[must_use]
pub fn fmt_wind_dir(v: Option<f32>) -> String {
    fmt_unit(v, "°", 0)
}

/// Format a battery-percentage value for display.
///
/// Returns `"N/A"` for `None`, otherwise `"{value} %"`.
#[must_use]
pub fn fmt_battery(v: Option<u8>) -> String {
    v.map_or_else(|| "N/A".to_owned(), |b| format!("{b} %"))
}

/// Format the diagnostics bitfield as a human-readable status line.
///
/// `"OK"` when no flags are set; otherwise a comma-joined list of active faults,
/// e.g. `"sky occluded, BMP388 fault"`. Scales as new flags are added.
#[must_use]
pub fn fmt_diagnostics(diag: Diagnostics) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if diag.occlusion() {
        flags.push("sky occluded");
    }
    if diag.baro_fault() {
        flags.push("BMP388 fault");
    }
    if diag.bme280_fault() {
        flags.push("BME280 fault");
    }
    if diag.veml7700_fault() {
        flags.push("VEML7700 fault");
    }
    if diag.baro_divergence() {
        flags.push("baro divergence");
    }
    if diag.mlx90614_fault() {
        flags.push("MLX90614 fault");
    }
    if flags.is_empty() {
        "OK".to_owned()
    } else {
        flags.join(", ")
    }
}

/// `true` if any diagnostics flag is set (drives red highlighting in the UI).
///
/// Tests the raw byte (`Diagnostics.0` is `pub`) so it covers every current and
/// future flag with no per-bit update — unlike `fmt_diagnostics`, which must name
/// each flag to label it.
#[must_use]
pub const fn diagnostics_alert(diag: Diagnostics) -> bool {
    diag.0 != 0
}

/// Format a floating-point sensor value with a physical unit.
///
/// Returns `"N/A"` when `v` is `None`; otherwise renders `"{v:.prec$} {unit}"`.
fn fmt_unit(v: Option<f32>, unit: &str, prec: usize) -> String {
    v.map_or_else(|| "N/A".to_owned(), |x| format!("{x:.prec$} {unit}"))
}

/// Capped time-series of `(seconds-since-session-start, value)` points for charting.
pub struct Series {
    points: VecDeque<(f64, f64)>,
    cap: usize,
}

impl Series {
    /// Default capacity: 600 points = 10 min at the 1 Hz feed.
    pub const DEFAULT_CAP: usize = 600;

    /// Visible chart window, in seconds. The x-axis is right-anchored at the
    /// latest sample and spans this many seconds backwards, so new points enter
    /// at the right edge and scroll left as the window fills. Matched to
    /// [`Series::DEFAULT_CAP`] at the 1 Hz feed (600 points ≈ 600 s).
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

/// Decode the DIS Firmware Revision String.
///
/// Returns `None` on invalid UTF-8 so the UI can show "unknown" rather than garbage.
#[must_use]
pub fn parse_fw_revision(bytes: &[u8]) -> Option<String> {
    core::str::from_utf8(bytes)
        .ok()
        .map(|s| s.trim_end_matches('\0').trim().to_owned())
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

    // --- ConnState::next tests ---

    #[test]
    fn next_state_link_lost_from_live_returns_reconnecting() -> TestResult {
        // Given
        let state = ConnState::Live;

        // When
        let next = state.next(LinkEvent::LinkLost);

        // Then
        assert_eq!(next, ConnState::Reconnecting);
        Ok(())
    }

    #[test]
    fn next_state_attempt_failed_from_connecting_returns_reconnecting() -> TestResult {
        // Given
        let state = ConnState::Connecting;

        // When
        let next = state.next(LinkEvent::AttemptFailed);

        // Then
        assert_eq!(next, ConnState::Reconnecting);
        Ok(())
    }

    #[test]
    fn next_state_happy_path_scanning_to_live() -> TestResult {
        // Given
        let mut state = ConnState::Scanning;

        // When
        state = state.next(LinkEvent::DeviceFound);
        state = state.next(LinkEvent::Connected);
        state = state.next(LinkEvent::Subscribed);

        // Then
        assert_eq!(state, ConnState::Live);
        Ok(())
    }

    #[test]
    fn next_state_scan_started_resets_to_scanning() -> TestResult {
        // Given
        let state = ConnState::Reconnecting;

        // When
        let next = state.next(LinkEvent::ScanStarted);

        // Then
        assert_eq!(next, ConnState::Scanning);
        Ok(())
    }

    #[test]
    fn next_state_full_reconnect_sequence() -> TestResult {
        // Given
        let mut state = ConnState::Live;

        // When / Then — assert each intermediate state
        state = state.next(LinkEvent::LinkLost);
        assert_eq!(state, ConnState::Reconnecting);

        state = state.next(LinkEvent::ScanStarted);
        assert_eq!(state, ConnState::Scanning);

        state = state.next(LinkEvent::DeviceFound);
        assert_eq!(state, ConnState::Connecting);

        state = state.next(LinkEvent::Connected);
        assert_eq!(state, ConnState::Resolving);

        state = state.next(LinkEvent::Subscribed);
        assert_eq!(state, ConnState::Live);

        Ok(())
    }

    #[test]
    fn next_state_ignores_inapplicable_event() -> TestResult {
        // Given
        let state = ConnState::Live;

        // When
        let next = state.next(LinkEvent::DeviceFound);

        // Then
        assert_eq!(next, ConnState::Live);
        Ok(())
    }

    // --- fmt_* tests ---

    #[test]
    fn fmt_temp_some_renders_one_decimal_with_unit() -> TestResult {
        // Given
        let value = Some(23.5_f32);

        // When
        let result = fmt_temp(value);

        // Then
        assert_eq!(result, "23.5 °C");
        Ok(())
    }

    #[test]
    fn fmt_temp_none_renders_na() -> TestResult {
        // Given / When
        let result = fmt_temp(None);

        // Then
        assert_eq!(result, "N/A");
        Ok(())
    }

    #[test]
    fn fmt_battery_none_renders_na() -> TestResult {
        // Given / When
        let result = fmt_battery(None);

        // Then
        assert_eq!(result, "N/A");
        Ok(())
    }

    #[test]
    fn fmt_battery_some_renders_percent() -> TestResult {
        // Given
        let value = Some(80_u8);

        // When
        let result = fmt_battery(value);

        // Then
        assert_eq!(result, "80 %");
        Ok(())
    }

    // --- fmt_diagnostics / diagnostics_alert tests ---

    #[test]
    fn fmt_diagnostics_none_renders_ok() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty());

        // Then
        assert_eq!(result, "OK");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_occlusion_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_occlusion(true));

        // Then
        assert_eq!(result, "sky occluded");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_baro_fault_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_baro_fault(true));

        // Then
        assert_eq!(result, "BMP388 fault");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_multiple_flags_joined() -> TestResult {
        // Given — both occlusion and baro fault set
        let diag = Diagnostics::empty()
            .with_occlusion(true)
            .with_baro_fault(true);

        // When
        let result = fmt_diagnostics(diag);

        // Then — occlusion first, then baro fault, comma-separated
        assert_eq!(result, "sky occluded, BMP388 fault");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_bme280_fault_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_bme280_fault(true));

        // Then
        assert_eq!(result, "BME280 fault");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_veml7700_fault_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_veml7700_fault(true));

        // Then
        assert_eq!(result, "VEML7700 fault");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_baro_divergence_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_baro_divergence(true));

        // Then
        assert_eq!(result, "baro divergence");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_mlx_fault_only() -> TestResult {
        // Given / When
        let result = fmt_diagnostics(Diagnostics::empty().with_mlx90614_fault(true));

        // Then
        assert_eq!(result, "MLX90614 fault");
        Ok(())
    }

    #[test]
    fn fmt_diagnostics_all_flags_joined_in_bit_order() -> TestResult {
        // Given — all six flags set
        let diag = Diagnostics::empty()
            .with_occlusion(true)
            .with_baro_fault(true)
            .with_bme280_fault(true)
            .with_veml7700_fault(true)
            .with_baro_divergence(true)
            .with_mlx90614_fault(true);

        // When
        let result = fmt_diagnostics(diag);

        // Then — labels appear in bit order (0→5)
        assert_eq!(
            result,
            "sky occluded, BMP388 fault, BME280 fault, VEML7700 fault, baro divergence, MLX90614 fault"
        );
        Ok(())
    }

    #[test]
    fn diagnostics_alert_true_when_any_flag() -> TestResult {
        // Given / When / Then — no flags: false
        assert!(!diagnostics_alert(Diagnostics::empty()));

        // occlusion only: true
        assert!(diagnostics_alert(Diagnostics::empty().with_occlusion(true)));

        // baro fault only: true
        assert!(diagnostics_alert(
            Diagnostics::empty().with_baro_fault(true)
        ));

        // both flags: true
        assert!(diagnostics_alert(
            Diagnostics::empty()
                .with_occlusion(true)
                .with_baro_fault(true)
        ));

        Ok(())
    }

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

    // --- parse_fw_revision tests ---

    #[test]
    fn parse_fw_revision_trims_and_decodes() -> TestResult {
        // Given
        let bytes = b"0.1.0";

        // When
        let result = parse_fw_revision(bytes);

        // Then
        assert_eq!(result, Some("0.1.0".to_owned()));
        Ok(())
    }

    #[test]
    fn parse_fw_revision_rejects_invalid_utf8() -> TestResult {
        // Given
        let bytes: &[u8] = &[0xFF, 0xFE];

        // When
        let result = parse_fw_revision(bytes);

        // Then
        assert_eq!(result, None);
        Ok(())
    }
}
// grcov exclude stop
