//! SVG chart component (`PlotPanel`) and supporting helpers.
//!
//! The rendering pipeline:
//! 1. `gaussian_smooth` the avg trace (default σ = 3.5, matching the TUI Medium preset).
//! 2. `padded_value_bounds` + `value_axis_labels` for the y-axis domain and tick labels.
//! 3. `project` maps `(x, y)` data coordinates to SVG pixel coordinates (y inverted).
//! 4. `render_svg_chart` emits a complete `<svg>` string: CRUST background, dotted
//!    gridlines at 25/50/75 %, an optional min–max band, a gradient fill under the avg
//!    trace, the smoothed avg `<polyline>`, and corner axis labels.
//! 5. `PlotPanel` injects the SVG string as `inner_html` on a wrapper `<div>`.

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

use std::fmt::Write as _;

use leptos::prelude::*;
use meteo_chart::{
    gaussian_smooth, padded_value_bounds,
    palette::{CRUST, OVERLAY1, SURFACE2, css},
    value_axis_labels,
};

/// Logical SVG canvas width (px in the viewBox coordinate system).
const SVG_W: f64 = 400.0;
/// Logical SVG canvas height (px in the viewBox coordinate system).
const SVG_H: f64 = 200.0;

/// Default Gaussian smoothing σ in samples — matches the TUI "Medium" preset.
const DEFAULT_SIGMA: f64 = 3.5;

/// Data series for one chart metric.
#[derive(Clone)]
pub struct ChartSeries {
    /// `(x, avg)` pairs; `x` is typically unix seconds or elapsed seconds.
    pub points: Vec<(f64, f64)>,
    /// Optional `(x, min, max)` envelope for the min–max band.
    pub band: Option<Vec<(f64, f64, f64)>>,
    /// Metric colour as a `#rrggbb` string (from `meteo_chart::palette::css`).
    pub color_hex: String,
    /// Lower bound floor for physically non-negative metrics; `None` for temperature.
    pub floor: Option<f64>,
    /// Minimum decimal precision passed to `value_axis_labels`.
    pub prec: usize,
}

/// Map a data point to SVG pixel coordinates.
///
/// The y-axis is **inverted** (SVG y grows downward): a higher data value maps to a
/// smaller `py`. Both axes are scaled proportionally within the provided domains.
///
/// # Arguments
/// * `x`, `y` — data-space input.
/// * `xdom` — `[x_min, x_max]` domain.
/// * `ydom` — `[y_lo, y_hi]` domain (from `padded_value_bounds`).
/// * `w`, `h` — SVG canvas size.
///
/// Returns `(px, py)` in SVG pixel space.
#[must_use]
pub fn project(x: f64, y: f64, xdom: [f64; 2], ydom: [f64; 2], w: f64, h: f64) -> (f64, f64) {
    let xspan = xdom[1] - xdom[0];
    let yspan = ydom[1] - ydom[0];
    let px = if xspan.abs() < f64::EPSILON {
        w * 0.5
    } else {
        (x - xdom[0]) / xspan * w
    };
    let py = if yspan.abs() < f64::EPSILON {
        h * 0.5
    } else {
        // y is inverted: higher data value → smaller SVG y coordinate.
        ((y - ydom[0]) / yspan).mul_add(-h, h)
    };
    (px, py)
}

/// Build the complete `<svg>…</svg>` string for the given `series`.
///
/// All computation is pure (no I/O). The resulting string is injected as
/// `innerHTML` by `PlotPanel`.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "SVG template string builder — splitting would obscure the visual structure"
)]
fn render_svg_chart(series: &ChartSeries, smooth_sigma: f64) -> String {
    const W: f64 = SVG_W;
    const H: f64 = SVG_H;

    let crust_hex = css(CRUST);
    let grid_hex = css(SURFACE2);
    let label_hex = css(OVERLAY1);
    let color = &series.color_hex;

    // ── x domain ─────────────────────────────────────────────────────────────
    let xdom: [f64; 2] = if series.points.is_empty() {
        [0.0, 1.0]
    } else {
        let xmin = series
            .points
            .iter()
            .map(|p| p.0)
            .fold(f64::INFINITY, f64::min);
        let xmax = series
            .points
            .iter()
            .map(|p| p.0)
            .fold(f64::NEG_INFINITY, f64::max);
        if (xmax - xmin).abs() < f64::EPSILON {
            [xmin - 1.0, xmin + 1.0]
        } else {
            [xmin, xmax]
        }
    };

    // ── Smooth the avg trace ──────────────────────────────────────────────────
    let smoothed = gaussian_smooth(&series.points, smooth_sigma);

    // ── y domain ──────────────────────────────────────────────────────────────
    let ydom: [f64; 2] = if smoothed.is_empty() {
        [0.0, 1.0]
    } else {
        let mut ymin = smoothed.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let mut ymax = smoothed
            .iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max);
        if let Some(band) = &series.band {
            for &(_, lo, hi) in band {
                ymin = ymin.min(lo);
                ymax = ymax.max(hi);
            }
        }
        padded_value_bounds(ymin, ymax, series.floor)
    };

    // ── Axis labels ───────────────────────────────────────────────────────────
    let [lo_label, mid_label, hi_label] = value_axis_labels(ydom, series.prec);

    // ── projection closure ────────────────────────────────────────────────────
    let proj = |x: f64, y: f64| -> (f64, f64) { project(x, y, xdom, ydom, W, H) };

    // ── Build SVG string ─────────────────────────────────────────────────────
    let mut svg = String::with_capacity(2048);

    // SVG root element with fixed viewBox; CSS scales it responsively.
    // write! on String is infallible; .ok() discards the always-Ok Result.
    write!(
        svg,
        r#"<svg viewBox="0 0 {W} {H}" xmlns="http://www.w3.org/2000/svg" class="plot-svg" preserveAspectRatio="none">"#
    )
    .ok();

    // Background well (CRUST colour).
    write!(
        svg,
        r#"<rect x="0" y="0" width="{W}" height="{H}" fill="{crust_hex}"/>"#
    )
    .ok();

    // Gradient definition: metric colour at ≈13 % → 0 % alpha (top → bottom).
    let grad_id = format!("pg-{}", color.trim_start_matches('#'));
    write!(
        svg,
        r#"<defs><linearGradient id="{grad_id}" x1="0" y1="0" x2="0" y2="1">"#
    )
    .ok();
    write!(
        svg,
        r#"<stop offset="0%" stop-color="{color}" stop-opacity="0.13"/>"#
    )
    .ok();
    write!(
        svg,
        r#"<stop offset="100%" stop-color="{color}" stop-opacity="0"/>"#
    )
    .ok();
    svg.push_str("</linearGradient></defs>");

    // Dotted gridlines at 25 %, 50 %, 75 %.
    for frac in [0.25_f64, 0.50, 0.75] {
        let yg = H * frac;
        write!(
            svg,
            r#"<line x1="0" y1="{yg:.1}" x2="{W}" y2="{yg:.1}" stroke="{grid_hex}" stroke-dasharray="2,4" stroke-width="0.5"/>"#
        )
        .ok();
    }

    if !smoothed.is_empty() {
        // Min–max band: palette colour at low alpha (same idiom as the TUI gust band).
        if let Some(band) = &series.band
            && band.len() >= 2
        {
            let top: Vec<(f64, f64)> = band.iter().map(|&(x, _lo, hi)| proj(x, hi)).collect();
            let bot_rev: Vec<(f64, f64)> =
                band.iter().rev().map(|&(x, lo, _hi)| proj(x, lo)).collect();
            let mut d = format!("M {:.1},{:.1}", top[0].0, top[0].1);
            for &(px, py) in &top[1..] {
                write!(d, " L {px:.1},{py:.1}").ok();
            }
            for &(px, py) in &bot_rev {
                write!(d, " L {px:.1},{py:.1}").ok();
            }
            d.push('Z');
            write!(svg, r#"<path d="{d}" fill="{color}" fill-opacity="0.12"/>"#).ok();
        }

        // Project the smoothed avg trace into SVG space.
        let proj_pts: Vec<(f64, f64)> = smoothed.iter().map(|&(x, y)| proj(x, y)).collect();

        // Use a slice pattern to bind first and last without indexing arithmetic.
        match proj_pts.as_slice() {
            [first, .., last] => {
                let (fx, fy) = *first;
                let (lx, _) = *last;

                // Gradient fill under the avg trace (≈13 % → 0 alpha fill).
                let mut fill_d = format!("M {fx:.1},{H:.1} L {fx:.1},{fy:.1}");
                for &(px, py) in &proj_pts[1..] {
                    write!(fill_d, " L {px:.1},{py:.1}").ok();
                }
                write!(fill_d, " L {lx:.1},{H:.1} Z").ok();
                write!(svg, r#"<path d="{fill_d}" fill="url(#{grad_id})"/>"#).ok();

                // Avg trace polyline in the metric colour.
                let pts_str: String = proj_pts
                    .iter()
                    .map(|&(px, py)| format!("{px:.1},{py:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                write!(
                    svg,
                    r#"<polyline points="{pts_str}" fill="none" stroke="{color}" stroke-width="1.5"/>"#
                )
                .ok();
            }
            [only] => {
                // Single-point series — render a dot.
                let (px, py) = *only;
                write!(
                    svg,
                    r#"<circle cx="{px:.1}" cy="{py:.1}" r="2" fill="{color}"/>"#
                )
                .ok();
            }
            [] => {}
        }
    }

    // Axis labels: top (hi), mid, bottom (lo). CSS class `font-mono` → JetBrains Mono.
    let text_y_top = 10.0_f64;
    let text_y_mid = H / 2.0 + 4.0;
    let text_y_bot = H - 2.0;
    write!(
        svg,
        r#"<text x="2" y="{text_y_top:.1}" fill="{label_hex}" font-size="10" class="font-mono">{hi_label}</text>"#
    )
    .ok();
    write!(
        svg,
        r#"<text x="2" y="{text_y_mid:.1}" fill="{label_hex}" font-size="10" class="font-mono">{mid_label}</text>"#
    )
    .ok();
    write!(
        svg,
        r#"<text x="2" y="{text_y_bot:.1}" fill="{label_hex}" font-size="10" class="font-mono">{lo_label}</text>"#
    )
    .ok();

    svg.push_str("</svg>");
    svg
}

/// A reusable historic chart panel rendering a time-series in SVG.
///
/// The component:
/// - Applies Gaussian smoothing to the avg trace (`smooth_sigma`; 0 ⇒ default 3.5 σ).
/// - Derives the y-axis domain via `padded_value_bounds`.
/// - Emits an inline SVG (CRUST well, dotted gridlines, optional min–max band,
///   gradient fill, avg polyline, axis tick labels).
/// - Uses CSS class `plot-svg` on the `<svg>` for responsive scaling.
///
/// The `series` prop is a **reactive signal** so the SVG is patched in-place
/// whenever the data changes (no panel remount).  Pass `Signal::stored(s)` for
/// a static value or `Signal::derive(move || …)` for a derived one.
///
/// The caller is responsible for choosing the correct `color_hex` from `meteo_chart::palette`.
#[component]
pub fn PlotPanel(
    /// Panel title (e.g. "Température de l'air").
    title: String,
    /// Unit label (e.g. "°C").
    unit: String,
    /// Reactive time-series data including optional min–max band.
    series: Signal<ChartSeries>,
    /// Gaussian smoothing σ in samples. 0.0 (default) uses the TUI Medium preset (3.5).
    #[prop(optional)]
    smooth_sigma: f64,
) -> impl IntoView {
    let sigma = if smooth_sigma > 0.0 {
        smooth_sigma
    } else {
        DEFAULT_SIGMA
    };

    view! {
        <div class="plot-panel">
            <div class="plot-panel-header">
                <span class="plot-title">{title}</span>
                <span class="plot-unit">{unit}</span>
            </div>
            <div class="plot-svg-outer" inner_html=move || render_svg_chart(&series.get(), sigma)/>
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

    // ── project ──────────────────────────────────────────────────────────────

    /// `project` must map the domain corners to the SVG canvas corners (y inverted).
    #[test]
    fn project_maps_domain_corners() -> TestResult {
        // Given — a known 100×50 canvas with [0,10] × [0,20] domain
        let xdom = [0.0_f64, 10.0];
        let ydom = [0.0_f64, 20.0];
        let (w, h) = (100.0_f64, 50.0_f64);

        // When — project the four corners
        let (px_lo_lo, py_lo_lo) = project(0.0, 0.0, xdom, ydom, w, h);
        let (px_hi_hi, py_hi_hi) = project(10.0, 20.0, xdom, ydom, w, h);
        let (px_lo_hi, py_lo_hi) = project(0.0, 20.0, xdom, ydom, w, h);
        let (px_hi_lo, py_hi_lo) = project(10.0, 0.0, xdom, ydom, w, h);

        // Then — (xmin, ymin) → (0, H) bottom-left; (xmax, ymax) → (W, 0) top-right
        assert!(
            (px_lo_lo - 0.0).abs() < 1e-9,
            "bottom-left x should be 0, got {px_lo_lo}"
        );
        assert!(
            (py_lo_lo - 50.0).abs() < 1e-9,
            "bottom-left y should be H=50, got {py_lo_lo}"
        );
        assert!(
            (px_hi_hi - 100.0).abs() < 1e-9,
            "top-right x should be W=100, got {px_hi_hi}"
        );
        assert!(
            (py_hi_hi - 0.0).abs() < 1e-9,
            "top-right y should be 0, got {py_hi_hi}"
        );
        assert!(
            (px_lo_hi - 0.0).abs() < 1e-9,
            "top-left x should be 0, got {px_lo_hi}"
        );
        assert!(
            (py_lo_hi - 0.0).abs() < 1e-9,
            "top-left y should be 0, got {py_lo_hi}"
        );
        assert!(
            (px_hi_lo - 100.0).abs() < 1e-9,
            "bottom-right x should be W=100, got {px_hi_lo}"
        );
        assert!(
            (py_hi_lo - 50.0).abs() < 1e-9,
            "bottom-right y should be H=50, got {py_hi_lo}"
        );
        Ok(())
    }

    // ── PlotPanel ────────────────────────────────────────────────────────────

    /// `PlotPanel` must render a `<polyline` (or `<path`) and include the metric colour.
    #[test]
    fn plotpanel_renders_polyline_for_series() -> TestResult {
        // Given — run inside a reactive Owner so Signal::stored can allocate
        let html = Owner::new().with(|| {
            let series = Signal::stored(ChartSeries {
                points: vec![
                    (0.0, 20.0),
                    (1.0, 21.5),
                    (2.0, 19.8),
                    (3.0, 22.1),
                    (4.0, 20.5),
                ],
                band: None,
                color_hex: "#fab387".to_owned(),
                floor: None,
                prec: 1,
            });

            // When — render to HTML under SSR
            view! {
                <PlotPanel
                    title="Température".to_string()
                    unit="°C".to_string()
                    series=series
                />
            }
            .to_html()
        });

        // Then — must contain a polyline or path with metric data AND the colour
        let has_trace = html.contains("<polyline") || html.contains("<path");
        assert!(
            has_trace,
            "expected <polyline or <path in rendered HTML, got:\n{html}"
        );
        assert!(
            html.contains("#fab387"),
            "expected metric colour #fab387 in rendered HTML"
        );
        assert!(html.contains("<rect"), "expected background <rect in HTML");
        Ok(())
    }

    /// `PlotPanel` with an empty series must not panic and must still render the
    /// panel well (`<rect`), even though no trace is drawn.
    #[test]
    fn plotpanel_empty_series_renders_placeholder() -> TestResult {
        // Given — run inside a reactive Owner so Signal::stored can allocate
        let html = Owner::new().with(|| {
            let series = Signal::stored(ChartSeries {
                points: vec![],
                band: None,
                color_hex: "#89dceb".to_owned(),
                floor: Some(0.0),
                prec: 0,
            });

            // When
            view! {
                <PlotPanel
                    title="Vent".to_string()
                    unit="m/s".to_string()
                    series=series
                />
            }
            .to_html()
        });

        // Then — the panel well must be rendered; no panic
        assert!(
            html.contains("<rect"),
            "expected <rect (CRUST well) even for an empty series"
        );
        assert!(
            html.contains("plot-panel"),
            "expected plot-panel class in HTML"
        );
        Ok(())
    }
}
// grcov exclude stop
