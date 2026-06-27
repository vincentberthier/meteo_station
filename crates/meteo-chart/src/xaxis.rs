//! Time x-axis decorations for the history charts: local day boundaries and
//! solar-event marks (sunrise/sunset + civil/nautical/astronomical twilight).
//!
//! All functions are pure and host-tested. The renderer positions each mark by
//! its unix-second `x` against the chart's x-domain. Sun times are computed
//! locally from the station's coarse coordinates via the `sunrise` crate — no
//! network call is involved.

use chrono::{Datelike as _, NaiveDate, TimeZone as _, Utc};
use sunrise::{Coordinates, DawnType, SolarDay, SolarEvent};

/// Seconds in one day.
const DAY_SECS: f64 = 86_400.0;

/// A solar twilight / sun event kind, ordered dawn → dusk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SunKind {
    /// Astronomical dawn (sun 18° below horizon, rising).
    AstroDawn,
    /// Nautical dawn (sun 12° below horizon, rising).
    NauticalDawn,
    /// Civil dawn (sun 6° below horizon, rising).
    CivilDawn,
    /// Sunrise (upper limb at the horizon).
    Sunrise,
    /// Sunset (upper limb at the horizon).
    Sunset,
    /// Civil dusk (sun 6° below horizon, setting).
    CivilDusk,
    /// Nautical dusk (sun 12° below horizon, setting).
    NauticalDusk,
    /// Astronomical dusk (sun 18° below horizon, setting).
    AstroDusk,
}

impl SunKind {
    /// Relative glyph opacity for the x-axis sun marker: brightest at
    /// sunrise/sunset, fading with twilight depth.
    #[must_use]
    pub const fn glyph_opacity(self) -> f64 {
        match self {
            Self::Sunrise | Self::Sunset => 1.0,
            Self::CivilDawn | Self::CivilDusk => 0.68,
            Self::NauticalDawn | Self::NauticalDusk => 0.48,
            Self::AstroDawn | Self::AstroDusk => 0.32,
        }
    }

    /// Stable short identifier (for CSS classes / colour lookup).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::AstroDawn => "astro-dawn",
            Self::NauticalDawn => "nautical-dawn",
            Self::CivilDawn => "civil-dawn",
            Self::Sunrise => "sunrise",
            Self::Sunset => "sunset",
            Self::CivilDusk => "civil-dusk",
            Self::NauticalDusk => "nautical-dusk",
            Self::AstroDusk => "astro-dusk",
        }
    }
}

/// A solar-event mark at unix-second `x`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SunMark {
    /// Event instant as unix seconds (UTC).
    pub x: f64,
    /// Which solar event this mark denotes.
    pub kind: SunKind,
}

/// The eight tracked events with their `sunrise`-crate `SolarEvent` mapping.
const EVENTS: [(SunKind, SolarEvent); 8] = [
    (SunKind::AstroDawn, SolarEvent::Dawn(DawnType::Astronomical)),
    (SunKind::NauticalDawn, SolarEvent::Dawn(DawnType::Nautical)),
    (SunKind::CivilDawn, SolarEvent::Dawn(DawnType::Civil)),
    (SunKind::Sunrise, SolarEvent::Sunrise),
    (SunKind::Sunset, SolarEvent::Sunset),
    (SunKind::CivilDusk, SolarEvent::Dusk(DawnType::Civil)),
    (SunKind::NauticalDusk, SolarEvent::Dusk(DawnType::Nautical)),
    (SunKind::AstroDusk, SolarEvent::Dusk(DawnType::Astronomical)),
];

/// UTC calendar date of a unix-second instant.
#[expect(
    clippy::cast_possible_truncation,
    reason = "unix seconds for any realistic chart window fit i64 exactly"
)]
fn date_of(ts: f64) -> Option<NaiveDate> {
    Utc.timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.date_naive())
}

/// All solar-event marks within `[from_ts, to_ts]` (unix seconds) for the given
/// coordinates.
///
/// Returns an empty vector for invalid coordinates or a non-positive range.
/// Days are enumerated in UTC with a ±1-day pad so events straddling the window
/// edges are not missed; only marks landing inside the range are kept. The
/// result is sorted ascending by `x`.
#[must_use]
pub fn sun_marks(lat: f64, lon: f64, from_ts: f64, to_ts: f64) -> Vec<SunMark> {
    let Some(coord) = Coordinates::new(lat, lon) else {
        return Vec::new();
    };
    let (Some(start), Some(end)) = (date_of(from_ts), date_of(to_ts)) else {
        return Vec::new();
    };
    if to_ts <= from_ts {
        return Vec::new();
    }

    // Enumerate UTC dates from one day before the window to one day after, using
    // pred_opt/succ_opt (NaiveDate has no panic-free `+ Duration`, which the
    // arithmetic-side-effects lint forbids).
    let mut marks = Vec::new();
    let mut date = start.pred_opt().unwrap_or(start);
    let last_date = end.succ_opt().unwrap_or(end);
    loop {
        let day = SolarDay::new(coord, date);
        for (kind, event) in EVENTS {
            // `event_time` is `None` when the event does not occur that day
            // (e.g. white nights at high latitude) — skip those.
            let Some(dt) = day.event_time(event) else {
                continue;
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "unix seconds as f64 for x-axis placement — sub-second precision irrelevant"
            )]
            let x = dt.timestamp() as f64;
            if (from_ts..=to_ts).contains(&x) {
                marks.push(SunMark { x, kind });
            }
        }
        if date >= last_date {
            break;
        }
        let Some(next) = date.succ_opt() else { break };
        date = next;
    }
    marks.sort_by(|a, b| a.x.total_cmp(&b.x));
    marks
}

/// Local-midnight (00:00) instants within `[from_ts, to_ts]`, as unix seconds.
///
/// `tz_offset_secs` is the local offset from UTC (local − UTC; e.g. `7200` for
/// UTC+2). The result is ascending.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "midnight count is a small non-negative integer bounded by the window span"
)]
pub fn day_marks(from_ts: f64, to_ts: f64, tz_offset_secs: i32) -> Vec<f64> {
    if to_ts <= from_ts {
        return Vec::new();
    }
    let off = f64::from(tz_offset_secs);
    // First local midnight at or after from_ts, computed in local seconds then
    // mapped back to UTC. Iterate by count (no float `while` condition).
    let first_local = ((from_ts + off) / DAY_SECS).ceil() * DAY_SECS;
    let span = to_ts - (first_local - off);
    if span < 0.0 {
        return Vec::new();
    }
    let count = (span / DAY_SECS).floor() as usize;
    let mut marks = Vec::with_capacity(count.saturating_add(1));
    for k in 0..=count {
        marks.push(DAY_SECS.mul_add(k as f64, first_local - off));
    }
    marks
}

/// Format a unix-second instant as a local `HH:MM` label.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "seconds-of-day is a small non-negative value; u32 holds it exactly"
)]
pub fn fmt_hm(ts: f64, tz_offset_secs: i32) -> String {
    let local = ts + f64::from(tz_offset_secs);
    let sod = local.rem_euclid(DAY_SECS);
    // Decompose with float ops (integer / and % are forbidden by the lint).
    let h = (sod / 3600.0).floor();
    let m = (h.mul_add(-3600.0, sod) / 60.0).floor();
    format!("{:02}:{:02}", h as u32, m as u32)
}

/// Format a unix-second instant as a local `DD/MM` date label.
#[must_use]
pub fn fmt_dm(ts: f64, tz_offset_secs: i32) -> String {
    // date_of converts via UTC, so shifting by the offset first yields the local
    // calendar date.
    date_of(ts + f64::from(tz_offset_secs))
        .map_or_else(String::new, |d| format!("{:02}/{:02}", d.day(), d.month()))
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

    // Paris-ish coordinates and a clear summer day (2024-06-21).
    const LAT: f64 = 48.85;
    const LON: f64 = 2.35;
    // 2024-06-21 00:00:00 UTC .. +1 day.
    const JUN21: f64 = 1_718_928_000.0;

    #[test]
    fn sun_marks_orders_dawn_before_sunrise_before_sunset() -> TestResult {
        // Given a full UTC day window
        // When
        let marks = sun_marks(LAT, LON, JUN21, JUN21 + DAY_SECS);

        // Then — events present and globally ascending by x
        assert!(!marks.is_empty(), "expected solar marks for a summer day");
        for w in marks.windows(2) {
            assert!(w[0].x <= w[1].x, "marks must be sorted ascending by x");
        }

        // Sunrise must precede sunset, and civil dawn precede sunrise.
        let find = |k: SunKind| marks.iter().find(|m| m.kind == k).map(|m| m.x);
        let sunrise = find(SunKind::Sunrise).expect("sunrise present");
        let sunset = find(SunKind::Sunset).expect("sunset present");
        assert!(sunrise < sunset, "sunrise must be before sunset");
        if let Some(civil) = find(SunKind::CivilDawn) {
            assert!(civil <= sunrise, "civil dawn must be at/before sunrise");
        }
        Ok(())
    }

    #[test]
    fn sun_marks_invalid_inputs_yield_empty() -> TestResult {
        // Given an out-of-range latitude / a non-positive window
        // When / Then
        assert!(
            sun_marks(200.0, 0.0, JUN21, JUN21 + DAY_SECS).is_empty(),
            "invalid latitude must yield no marks"
        );
        assert!(
            sun_marks(LAT, LON, JUN21, JUN21).is_empty(),
            "zero-width window must yield no marks"
        );
        Ok(())
    }

    #[test]
    fn day_marks_lands_on_local_midnights() -> TestResult {
        // Given a 3-day window starting exactly at a UTC midnight, offset +2 h
        let off = 7_200; // UTC+2
        let from = JUN21; // 2024-06-21 00:00 UTC == 02:00 local
        let to = 3.0_f64.mul_add(DAY_SECS, JUN21);

        // When
        let marks = day_marks(from, to, off);

        // Then — local midnights are at UTC 22:00 the previous day; first one
        // after `from` is 2024-06-21 22:00 UTC = from + 22 h.
        assert_eq!(marks.len(), 3, "three local midnights in a 3-day window");
        let expected_first = 22.0_f64.mul_add(3600.0, from);
        assert!(
            (marks[0] - expected_first).abs() < 1.0,
            "first local midnight mismatch: {} vs {}",
            marks[0],
            expected_first
        );
        for w in marks.windows(2) {
            assert!(
                (w[1] - w[0] - DAY_SECS).abs() < 1.0,
                "midnights must be exactly one day apart"
            );
        }
        Ok(())
    }

    #[test]
    fn fmt_helpers_render_local_time_and_date() -> TestResult {
        // Given 2024-06-21 00:00 UTC with a +2 h offset
        // When / Then
        assert_eq!(fmt_hm(JUN21, 7_200), "02:00", "local HH:MM");
        assert_eq!(fmt_hm(JUN21, 0), "00:00", "UTC HH:MM");
        assert_eq!(fmt_dm(JUN21, 7_200), "21/06", "local DD/MM");
        Ok(())
    }

    #[test]
    fn glyph_opacity_decreases_with_depth() -> TestResult {
        assert!(SunKind::Sunrise.glyph_opacity() > SunKind::CivilDawn.glyph_opacity());
        assert!(SunKind::CivilDawn.glyph_opacity() > SunKind::NauticalDawn.glyph_opacity());
        assert!(SunKind::NauticalDawn.glyph_opacity() > SunKind::AstroDawn.glyph_opacity());
        Ok(())
    }
}
// grcov exclude stop
