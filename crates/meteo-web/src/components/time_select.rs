//! Time-window selector component — preset buttons + custom date range.
//!
//! Pure helpers (`preset_day`, `preset_week`, `preset_month`, `zoom_about`,
//! `pan_by`) operate on `i64` unix timestamps and have no Leptos dependency, so
//! they are fully testable under `ssr`.

// The leptos #[component] macro generates a typed-builder struct whose field names
// shadow the function parameters.  Neither shadow is actionable from user code.
#![allow(
    clippy::shadow_reuse,
    reason = "leptos #[component] macro generates param shadows in the builder"
)]
// Component props are owned values consumed at call-site.  Leptos does not support
// borrowed props, so the pass-by-value is intentional even when the body only borrows.
#![allow(
    clippy::needless_pass_by_value,
    reason = "leptos component props must be owned"
)]
// Timestamp arithmetic: all values are well within i64 range for any realistic
// unix timestamp; overflow is not a concern for calendar-scale windows.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "unix-timestamp arithmetic cannot overflow within any realistic calendar range"
)]
// i64 → f64 precision: unix timestamps fit well within f64 mantissa resolution
// for chart x-axis purposes (sub-second precision is irrelevant here).
#![allow(
    clippy::cast_precision_loss,
    reason = "unix timestamps as f64 for chart math — sub-second precision irrelevant"
)]
// f64.round() → i64: the values are bounded unix-timestamp deltas, never exceeding
// i64::MAX; truncation is intentional (round-to-nearest-second).
#![allow(
    clippy::cast_possible_truncation,
    reason = "f64 round() result is a bounded unix-timestamp delta, fits in i64"
)]

use leptos::prelude::*;

/// A half-open time window `[from_ts, to_ts)` in Unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    /// Start of the window (inclusive), Unix seconds.
    pub from_ts: i64,
    /// End of the window (exclusive), Unix seconds.
    pub to_ts: i64,
}

impl TimeWindow {
    /// Length of the window in seconds.
    #[must_use]
    pub const fn span_secs(&self) -> i64 {
        self.to_ts - self.from_ts
    }

    /// Bucket size for query-time aggregation — span→bucket ladder.
    ///
    /// Targets ≲ 4 000 data points so browsers render fluently; 1-minute
    /// resolution is used for spans up to 2 days.
    ///
    /// | Span        | Bucket   | Max rows |
    /// |-------------|----------|----------|
    /// | ≤ 2 d       | 1 min    | 2 880    |
    /// | ≤ 2 wk      | 5 min    | 4 032    |
    /// | ≤ 3 mo      | 30 min   | 4 416    |
    /// | ≤ 1 yr      | 1 h      | 8 784    |
    /// | > 1 yr      | 1 day    | unbounded|
    #[must_use]
    pub const fn bucket_secs(&self) -> i64 {
        match self.span_secs() {
            s if s <= 2 * 86_400 => 60,      // ≤ 2 days   → 1 min  (≤ 2 880 pts)
            s if s <= 14 * 86_400 => 300,    // ≤ 2 weeks  → 5 min  (≤ 4 032 pts)
            s if s <= 92 * 86_400 => 1_800,  // ≤ 3 months → 30 min (≤ 4 416 pts)
            s if s <= 366 * 86_400 => 3_600, // ≤ 1 year   → 1 h    (≤ 8 784 pts)
            _ => 86_400,                     // > 1 year   → 1 day
        }
    }
}

// ---------------------------------------------------------------------------
// Pure preset helpers — testable without a Leptos runtime
// ---------------------------------------------------------------------------

const SECS_PER_DAY: i64 = 86_400;

/// Last 24 h ending at `now`.
#[must_use]
pub const fn preset_day(now: i64) -> TimeWindow {
    TimeWindow {
        from_ts: now - SECS_PER_DAY,
        to_ts: now,
    }
}

/// Last 7 days ending at `now`.
#[must_use]
pub const fn preset_week(now: i64) -> TimeWindow {
    TimeWindow {
        from_ts: now - 7 * SECS_PER_DAY,
        to_ts: now,
    }
}

/// Last 30 days ending at `now`.
#[must_use]
pub const fn preset_month(now: i64) -> TimeWindow {
    TimeWindow {
        from_ts: now - 30 * SECS_PER_DAY,
        to_ts: now,
    }
}

/// Zoom the window about a cursor fraction `f ∈ [0, 1]` of the current span
/// by `factor` (< 1 = zoom-in, > 1 = zoom-out).
///
/// The timestamp under cursor fraction `f` stays fixed:
/// `t_cursor = from_ts + f * span`.  The new span is `span * factor`; both
/// bounds are recomputed to keep `t_cursor` stationary.
#[must_use]
pub fn zoom_about(w: TimeWindow, f: f64, factor: f64) -> TimeWindow {
    let span = w.span_secs() as f64;
    let t_cursor = f.mul_add(span, w.from_ts as f64);
    let new_span = span * factor;
    let new_from = f.mul_add(-new_span, t_cursor);
    let new_to = new_from + new_span;
    TimeWindow {
        from_ts: new_from.round() as i64,
        to_ts: new_to.round() as i64,
    }
}

/// Pan the window by `frac` of the current span (positive = forward in time).
///
/// Both bounds shift by `frac * span`; the span itself is unchanged.
#[must_use]
pub fn pan_by(w: TimeWindow, frac: f64) -> TimeWindow {
    let shift = (w.span_secs() as f64 * frac).round() as i64;
    TimeWindow {
        from_ts: w.from_ts + shift,
        to_ts: w.to_ts + shift,
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Returns current unix timestamp in seconds.
fn now_ts() -> i64 {
    #[cfg(feature = "ssr")]
    {
        chrono::Utc::now().timestamp()
    }
    #[cfg(not(feature = "ssr"))]
    {
        // Under hydrate, preset clicks happen in the browser.  Chrono's
        // `clock` feature is enabled so `Utc::now()` compiles on wasm32 too.
        chrono::Utc::now().timestamp()
    }
}

/// Time-window preset selector + custom date range inputs.
///
/// Renders three FR preset buttons (Jour / Semaine / Mois) that `set` the
/// `window` signal relative to current time, plus two `<input type="datetime-local">`
/// fields for a custom range.
///
/// Preset buttons re-enable "follow" (`following.set(true)`) so the dashboard
/// tracks live data. The custom-range "Appliquer" disables it
/// (`following.set(false)`) since the user is exploring a fixed historical range.
#[component]
pub fn TimeSelect(
    /// The current time window; updated when a preset or custom range is applied.
    window: RwSignal<TimeWindow>,
    /// Whether the dashboard should auto-advance the window to follow "now".
    /// Preset buttons set this to `true`; custom range apply sets it to `false`.
    following: RwSignal<bool>,
) -> impl IntoView {
    let on_day = move |_| {
        following.set(true);
        window.set(preset_day(now_ts()));
    };
    let on_week = move |_| {
        following.set(true);
        window.set(preset_week(now_ts()));
    };
    let on_month = move |_| {
        following.set(true);
        window.set(preset_month(now_ts()));
    };

    // Custom range: from / to stored as datetime-local strings (YYYY-MM-DDTHH:MM).
    let from_input = RwSignal::new(String::new());
    let to_input = RwSignal::new(String::new());

    let on_apply = move |_| {
        // User chose a fixed historical range — stop following "now".
        following.set(false);
        let from_str = from_input.get();
        let to_str = to_input.get();
        // Parse as naive UTC datetimes → convert to unix timestamps.
        if let (Ok(from_dt), Ok(to_dt)) = (
            chrono::NaiveDateTime::parse_from_str(&from_str, "%Y-%m-%dT%H:%M"),
            chrono::NaiveDateTime::parse_from_str(&to_str, "%Y-%m-%dT%H:%M"),
        ) {
            let from_ts = from_dt.and_utc().timestamp();
            let to_ts = to_dt.and_utc().timestamp();
            if from_ts < to_ts {
                window.set(TimeWindow { from_ts, to_ts });
            }
        }
    };

    view! {
        <div class="time-select">
            <div class="time-presets">
                <button class="preset-btn" on:click=on_day>"Jour"</button>
                <button class="preset-btn" on:click=on_week>"Semaine"</button>
                <button class="preset-btn" on:click=on_month>"Mois"</button>
            </div>
            <div class="time-custom">
                <input
                    class="time-input font-mono"
                    type="datetime-local"
                    on:input=move |ev| from_input.set(event_target_value(&ev))
                />
                <span class="time-sep">"→"</span>
                <input
                    class="time-input font-mono"
                    type="datetime-local"
                    on:input=move |ev| to_input.set(event_target_value(&ev))
                />
                <button class="preset-btn" on:click=on_apply>"Appliquer"</button>
            </div>
        </div>
    }
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
#[cfg(all(test, feature = "ssr"))]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// `bucket_secs` returns the correct bucket size for representative spans.
    #[test]
    fn time_window_bucket_secs_scales_with_span() -> TestResult {
        // Given — 1-hour span → 60 s buckets (≤ 2 days branch)
        let w1h = TimeWindow {
            from_ts: 0,
            to_ts: 3_600,
        };
        assert_eq!(w1h.bucket_secs(), 60, "1-hour span should use 60 s buckets");

        // Given — 1-day span (86400 s) → 1-min buckets (≤ 2 days branch)
        let w1d = TimeWindow {
            from_ts: 0,
            to_ts: 86_400,
        };
        assert_eq!(
            w1d.bucket_secs(),
            60,
            "1-day span should use 60 s (1 min) buckets"
        );

        // Given — 2-week span (14 days) → 5-min buckets (≤ 2 weeks branch)
        let w2w = TimeWindow {
            from_ts: 0,
            to_ts: 14 * 86_400,
        };
        assert_eq!(
            w2w.bucket_secs(),
            300,
            "2-week span should use 300 s (5 min) buckets"
        );

        // Given — exactly 1 year (366 days) → 1-hour buckets (≤ 1 year branch)
        let w1y = TimeWindow {
            from_ts: 0,
            to_ts: 366 * 86_400,
        };
        assert_eq!(
            w1y.bucket_secs(),
            3_600,
            "366-day span should use 3600 s (1 h) buckets"
        );

        // Verify row-count invariant: rows should stay within reason (≤ 9 000)
        for (span, bucket) in [
            (3_600_i64, 60_i64),
            (86_400, 60),
            (14 * 86_400, 300),
            (366 * 86_400, 3_600),
        ] {
            let w = TimeWindow {
                from_ts: 0,
                to_ts: span,
            };
            let rows = span / bucket;
            assert!(
                rows <= 9_000,
                "span {span} / bucket {bucket} = {rows} rows, expected ≤ 9 000"
            );
            assert_eq!(
                w.bucket_secs(),
                bucket,
                "span {span} should yield bucket {bucket}"
            );
        }

        Ok(())
    }

    /// Preset helpers produce the correct `from_ts`/`to_ts` relative to a fixed `now`.
    #[test]
    fn time_window_presets_compute_expected_ranges() -> TestResult {
        // Given — a fixed reference timestamp
        let now: i64 = 1_700_000_000;

        // When
        let day = preset_day(now);
        let week = preset_week(now);
        let month = preset_month(now);

        // Then
        assert_eq!(day.to_ts, now, "preset_day: to_ts must equal now");
        assert_eq!(
            day.from_ts,
            now - 86_400,
            "preset_day: from_ts must be now - 86400"
        );

        assert_eq!(week.to_ts, now, "preset_week: to_ts must equal now");
        assert_eq!(
            week.from_ts,
            now - 7 * 86_400,
            "preset_week: from_ts must be now - 7 days"
        );

        assert_eq!(month.to_ts, now, "preset_month: to_ts must equal now");
        assert_eq!(
            month.from_ts,
            now - 30 * 86_400,
            "preset_month: from_ts must be now - 30 days"
        );

        Ok(())
    }

    /// `zoom_about(w, 0.5, 0.5)` must keep the midpoint timestamp fixed and
    /// halve the span.
    #[test]
    fn zoom_about_keeps_cursor_timestamp_fixed() -> TestResult {
        // Given — 2-hour window
        let w = TimeWindow {
            from_ts: 1_000_000,
            to_ts: 1_000_000 + 7_200,
        };
        let mid_before = w.from_ts + w.span_secs() / 2;

        // When — zoom in by 0.5× about the centre
        let zoomed = zoom_about(w, 0.5, 0.5);

        // Then — span is halved (within 1 s rounding)
        let expected_span = w.span_secs() / 2;
        assert!(
            (zoomed.span_secs() - expected_span).abs() <= 1,
            "span should be halved: expected ≈{expected_span}, got {}",
            zoomed.span_secs()
        );

        // Then — midpoint timestamp is unchanged (within 1 s rounding)
        let mid_after = zoomed.from_ts + zoomed.span_secs() / 2;
        assert!(
            (mid_after - mid_before).abs() <= 1,
            "midpoint should be unchanged: before={mid_before}, after={mid_after}"
        );

        Ok(())
    }

    /// `pan_by(w, 0.25)` shifts both bounds by +¼ span; the span is unchanged.
    #[test]
    fn pan_by_shifts_both_bounds_equally() -> TestResult {
        // Given
        let w = TimeWindow {
            from_ts: 1_000_000,
            to_ts: 1_000_000 + 3_600,
        };
        let shift = w.span_secs() / 4;

        // When
        let panned = pan_by(w, 0.25);

        // Then — span unchanged
        assert_eq!(
            panned.span_secs(),
            w.span_secs(),
            "span must be unchanged after pan"
        );

        // Then — both bounds shifted by ¼ span (within 1 s rounding)
        assert!(
            (panned.from_ts - (w.from_ts + shift)).abs() <= 1,
            "from_ts should shift by ¼ span: expected ≈{}, got {}",
            w.from_ts + shift,
            panned.from_ts
        );
        assert!(
            (panned.to_ts - (w.to_ts + shift)).abs() <= 1,
            "to_ts should shift by ¼ span: expected ≈{}, got {}",
            w.to_ts + shift,
            panned.to_ts
        );

        Ok(())
    }
}
// grcov exclude end
