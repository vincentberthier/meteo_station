//! Pure domain model for the TUI: connection state machine, telemetry formatting,
//! ring-buffer time series, and firmware-version parsing.

// All public items in this module are consumed by later substeps; suppress the
// dead_code lint that fires because main.rs does not yet call them.
#![allow(dead_code, reason = "consumed by BLE, UI, and app substeps")]

use std::collections::VecDeque;

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
