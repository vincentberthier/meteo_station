//! Pure chart-math helpers: bounds padding, Gaussian smoothing, axis labels.

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

/// Centered Gaussian-weighted moving average over a `(t, value)` series.
///
/// Smooths only the `value` axis; each output point keeps its original timestamp.
/// The kernel is **centered** (zero phase lag — a peak stays at the sample where
/// it occurred, unlike a trailing average or EWMA which drag features later), and
/// it operates over sample **indices** (the feed is a uniform ~1 Hz). At the two
/// ends the kernel is truncated and its weights renormalized, so the first and
/// last samples are not pulled toward the interior — the live right edge keeps
/// tracking the latest reading instead of flattening.
///
/// `sigma` is the Gaussian width in samples; the kernel half-width is `⌈3·sigma⌉`
/// (weights beyond that are negligible). `sigma <= 0`, a non-finite `sigma`, or
/// fewer than three points returns the input unchanged.
///
/// Index-based weighting means a temporal gap (signal loss) is blended across as
/// if the two sides were adjacent; gaps are rare and the rasterizer already draws
/// a straight connector across them, so this is acceptable.
#[must_use]
pub fn gaussian_smooth(pts: &[(f64, f64)], sigma: f64) -> Vec<(f64, f64)> {
    if !sigma.is_finite() || sigma <= 0.0 || pts.len() < 3 {
        return pts.to_vec();
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "⌈3·sigma⌉ is a small positive finite value; usize holds it"
    )]
    let radius = (sigma * 3.0).ceil() as usize;

    // Precompute kernel weights indexed by absolute sample distance d ∈ [0, radius].
    let kernel: Vec<f64> = (0..=radius)
        .map(|d| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "d ≤ radius is a small integer, exact in f64"
            )]
            let x = d as f64 / sigma;
            (-0.5 * x * x).exp()
        })
        .collect();

    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let lo = i.saturating_sub(radius);
        let hi = i.saturating_add(radius).min(n.saturating_sub(1));
        let mut acc = 0.0_f64;
        let mut wsum = 0.0_f64;
        for j in lo..=hi {
            let w = kernel[j.abs_diff(i)];
            acc = w.mul_add(pts[j].1, acc);
            wsum += w;
        }
        let v = if wsum > 0.0 { acc / wsum } else { pts[i].1 };
        out.push((pts[i].0, v));
    }
    out
}

/// Three evenly spaced tick labels for a value axis spanning `bounds` (`[lo, hi]`),
/// formatted (bottom, middle, top).
///
/// `min_prec` is the *minimum* decimal precision. When the bottom and top labels
/// would render identically at that precision — a narrow range over a small
/// magnitude, e.g. a 0.27–0.34 W load reading both showing `"0.3"` — the
/// precision is bumped (up to `min_prec + 4`) until they differ, so the axis
/// always conveys the actual span instead of a flat pair of equal numbers.
#[must_use]
pub fn value_axis_labels(bounds: [f64; 2], min_prec: usize) -> [String; 3] {
    let [lo, hi] = bounds;
    let mid = f64::midpoint(lo, hi);
    let mut prec = min_prec;
    loop {
        let labels = [
            format!("{lo:.prec$}"),
            format!("{mid:.prec$}"),
            format!("{hi:.prec$}"),
        ];
        if labels[0] != labels[2] || prec >= min_prec.saturating_add(4) {
            return labels;
        }
        prec = prec.saturating_add(1);
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

    // --- gaussian_smooth tests ---

    #[test]
    fn gaussian_smooth_identity_when_disabled() -> TestResult {
        // Given — a varying series
        let pts = [(0.0, 1.0), (1.0, 5.0), (2.0, 2.0), (3.0, 8.0)];

        // When — sigma <= 0 disables smoothing
        let off = gaussian_smooth(&pts, 0.0);
        let neg = gaussian_smooth(&pts, -2.0);

        // Then — input returned unchanged
        assert_eq!(off, pts.to_vec(), "sigma=0 must be identity");
        assert_eq!(neg, pts.to_vec(), "negative sigma must be identity");
        Ok(())
    }

    #[test]
    fn gaussian_smooth_too_few_points_unchanged() -> TestResult {
        // Given — fewer than three points
        let pts = [(0.0, 1.0), (1.0, 9.0)];

        // When
        let out = gaussian_smooth(&pts, 3.5);

        // Then — returned unchanged (kernel needs neighbours to act)
        assert_eq!(out, pts.to_vec());
        Ok(())
    }

    #[test]
    fn gaussian_smooth_preserves_len_and_timestamps() -> TestResult {
        // Given
        let pts: Vec<(f64, f64)> = (0..20).map(|i| (f64::from(i), f64::from(i % 3))).collect();

        // When
        let out = gaussian_smooth(&pts, 3.5);

        // Then — same length, timestamps untouched (only values change)
        assert_eq!(out.len(), pts.len());
        for (o, p) in out.iter().zip(pts.iter()) {
            assert!(
                (o.0 - p.0).abs() < f64::EPSILON,
                "timestamp must be preserved: {} vs {}",
                o.0,
                p.0
            );
        }
        Ok(())
    }

    #[test]
    fn gaussian_smooth_attenuates_spike() -> TestResult {
        // Given — a flat baseline with one tall spike in the middle
        let mut pts: Vec<(f64, f64)> = (0..21).map(|i| (f64::from(i), 0.0)).collect();
        pts[10].1 = 10.0;

        // When
        let out = gaussian_smooth(&pts, 3.5);

        // Then — the spike sample is pulled down and its neighbours lifted
        assert!(
            out[10].1 < 10.0,
            "spike should be attenuated, got {}",
            out[10].1
        );
        assert!(
            out[10].1 > 0.0,
            "spike center should stay above baseline, got {}",
            out[10].1
        );
        assert!(
            out[9].1 > 0.0 && out[11].1 > 0.0,
            "neighbours should be lifted by the spike: {} / {}",
            out[9].1,
            out[11].1
        );
        Ok(())
    }

    #[test]
    fn gaussian_smooth_constant_is_unchanged() -> TestResult {
        // Given — a flat series; the average of equal values is the same value
        let pts: Vec<(f64, f64)> = (0..30).map(|i| (f64::from(i), 7.0)).collect();

        // When
        let out = gaussian_smooth(&pts, 4.0);

        // Then — every smoothed value stays at the constant (within fp tolerance)
        for o in &out {
            assert!(
                (o.1 - 7.0).abs() < 1e-9,
                "constant series must stay constant, got {}",
                o.1
            );
        }
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

    #[test]
    fn value_axis_labels_bumps_precision_for_narrow_range() -> TestResult {
        // Given a narrow sub-watt range that collapses to "0.3"/"0.3" at prec 1
        // When
        let labels = value_axis_labels([0.27, 0.34], 1);

        // Then — precision rises until bottom and top labels differ
        assert_ne!(
            labels[0], labels[2],
            "axis ends must be distinct: {labels:?}"
        );
        assert_eq!(
            labels,
            ["0.27".to_owned(), "0.31".to_owned(), "0.34".to_owned()]
        );
        Ok(())
    }
}
// grcov exclude stop
