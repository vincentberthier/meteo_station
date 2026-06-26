//! Canvas-based history plot primitive for the `MeteoStation` TUI dashboard.
//!
//! [`render_plot`] draws one bordered panel containing a Braille-resolution Canvas
//! trace, optional gridlines, gradient fill, overlay trace, and bar series.
//! [`fill_columns`] is a pure helper that computes fill-column geometry.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::canvas::{Canvas, Context, Line, Points};
use ratatui::widgets::{Block, Paragraph};

use crate::model::{self, Series};
use crate::theme;

/// Marker style for drawing traces on the history plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerStyle {
    /// Individual Braille dots — one glyph per sample (sparse).
    Dots,
    /// Braille line segments connecting consecutive samples.
    Line,
}

/// One overlay trace rendered behind the main series line (e.g. gust @ 32 %).
pub struct Overlay<'data> {
    /// The `(t_secs, value)` samples for the overlay.
    pub points: &'data [(f64, f64)],
    /// Base colour before alpha blending toward [`BASE`].
    ///
    /// [`BASE`]: crate::theme::BASE
    pub color: Color,
    /// Alpha `∈ [0, 1]`: how much `color` survives after blending with `BASE`
    /// (`0.0` = fully transparent, `1.0` = fully opaque).
    pub alpha: f64,
}

/// Bar dataset rendered on an independent lower-half y-scale (e.g. rain rate).
///
/// Zero-valued bars receive a faint baseline tick so gaps stay visible.
pub struct Bars<'data> {
    /// The `(t_secs, value)` samples for the bars.
    pub points: &'data [(f64, f64)],
    /// Bar colour.
    pub color: Color,
}

/// Configuration for one history plot panel.
pub struct PlotSpec<'data> {
    /// Panel title shown in the block border (e.g. `"Température air"`).
    pub title: &'data str,
    /// Unit chip text shown top-right (e.g. `"°C"`, `"klx"`, `"W"`).
    pub unit: &'data str,
    /// Colour of the main trace line / dots.
    pub color: Color,
    /// Decimal precision for y-axis tick labels.
    pub prec: usize,
    /// Optional floor for `padded_value_bounds`; use `Some(0.0)` for physically
    /// non-negative metrics (luminosity, humidity) so the padded lower bound never
    /// goes negative.  `None` for metrics that can go negative (temperature).
    pub floor: Option<f64>,
    /// Display multiplier applied to y-axis **labels only** (`1.0` normally,
    /// `0.001` to display raw lux as klx). The Canvas coordinate space and trace
    /// geometry use raw units unchanged.
    pub scale: f64,
    /// Drawing style for the main trace and the overlay trace.
    pub marker: MarkerStyle,
    /// Draw dotted gridlines at 25 %, 50 %, and 75 % of the y range.
    pub show_grid: bool,
    /// Draw a gradient fill under the main trace fading from ~13 % opacity at the
    /// trace to 0 % at the baseline.
    pub fill: bool,
    /// Optional overlay trace (rendered behind the main trace).
    pub overlay: Option<Overlay<'data>>,
    /// Optional bar dataset mapped to the lower 30 % of the canvas y-range.
    pub bars: Option<Bars<'data>>,
}

/// Pure: compute continuous fill-column geometry for the gradient under-fill.
///
/// Walks each consecutive `(x0,y0)→(x1,y1)` segment in steps of `x_step`,
/// linearly interpolating the trace value, and returns `(x, y_top, y_bottom)`
/// per column where `y_bottom = y_lo` (baseline) and `y_top` is the interpolated
/// value. Segments wider than `max_gap` are skipped so a signal-loss gap is left
/// empty rather than filled with a fabricated ramp. Testable without ratatui.
///
/// Returns an empty vector when there are fewer than two points or `x_step`
/// is non-positive (a single sample has no area to fill).
#[must_use]
pub fn fill_columns(
    points: &[(f64, f64)],
    y_lo: f64,
    x_step: f64,
    max_gap: f64,
) -> Vec<(f64, f64, f64)> {
    let mut cols = Vec::new();
    if x_step <= 0.0 {
        return cols;
    }
    for pair in points.windows(2) {
        let &[(x0, y0), (x1, y1)] = pair else {
            continue;
        };
        let span = x1 - x0;
        if span <= 0.0 || span > max_gap {
            continue;
        }
        // Integer step count avoids a float `while` condition.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "step count is small, positive and finite (span > 0, x_step > 0)"
        )]
        let steps = (span / x_step).ceil() as u32;
        for i in 0..=steps {
            let x = f64::from(i).mul_add(x_step, x0).min(x1);
            let frac = (x - x0) / span;
            let y_top = frac.mul_add(y1 - y0, y0);
            cols.push((x, y_top, y_lo));
        }
    }
    cols
}

/// Render one history plot panel into `area`.
///
/// Draws an `"en attente…"` placeholder paragraph when `series` has no points.
/// Takes `series` as `&mut` because [`Series::points`] calls `make_contiguous`.
#[expect(
    clippy::too_many_lines,
    reason = "the painting closure is inherently long; splitting it would fragment the draw order"
)]
pub fn render_plot(frame: &mut Frame, area: Rect, spec: &PlotSpec<'_>, series: &mut Series) {
    // Empty series: show placeholder.
    let Some(x_win) = series.x_window() else {
        frame.render_widget(
            Paragraph::new("en attente\u{2026}").block(make_block(spec)),
            area,
        );
        return;
    };

    let y_bounds_raw = series.y_bounds().unwrap_or((-1.0, 1.0));
    // Call points() last: it takes &mut self and the returned slice borrows series.
    let pts = series.points();

    let y_win = model::padded_value_bounds(y_bounds_raw.0, y_bounds_raw.1, spec.floor);
    let y_lo = y_win[0];
    let y_hi = y_win[1];

    // Y labels use scaled units; Canvas bounds stay in raw units.
    let y_win_scaled = [y_lo * spec.scale, y_hi * spec.scale];
    let y_labels = model::value_axis_labels(y_win_scaled, spec.prec);

    // Fill is interpolated across the window: ~240 columns for a near-solid area,
    // skipping segments wider than 8 % of the window so signal-loss gaps stay empty.
    let x_range = x_win[1] - x_win[0];
    let fill_cols = if spec.fill {
        fill_columns(pts, y_lo, x_range / 240.0, x_range * 0.08)
    } else {
        Vec::new()
    };

    // One canvas cell in data-y units, used to lift the Y-min label one row above
    // the X-axis label row so they do not collide in the bottom-left corner.
    let inner_h = area.height.saturating_sub(2).max(1);
    let cell_y = (y_hi - y_lo) / f64::from(inner_h);

    // Extract spec fields before the move closure so we do not borrow spec inside it.
    let show_grid = spec.show_grid;
    let do_fill = spec.fill;
    let marker = spec.marker;
    let trace_color = spec.color;
    let overlay_data = spec
        .overlay
        .as_ref()
        .map(|ov| (ov.points, ov.color, ov.alpha));
    let bars_data = spec.bars.as_ref().map(|b| (b.points, b.color));

    let block = make_block(spec);
    let canvas = Canvas::default()
        .block(block)
        .x_bounds(x_win)
        .y_bounds(y_win)
        .marker(Marker::Braille)
        .background_color(theme::MANTLE)
        .paint(move |ctx| {
            // --- Gridlines at 25 / 50 / 75 % ---
            if show_grid {
                let grid_color = theme::blend_rgb(theme::SURFACE2, theme::BASE, 0.18);
                for frac in [0.25_f64, 0.50, 0.75] {
                    let y_grid = frac.mul_add(y_hi - y_lo, y_lo);
                    let grid_pts: Vec<(f64, f64)> = (0_u32..=50)
                        .map(|i| {
                            let t = f64::from(i) / 50.0;
                            (x_win[0] + t * (x_win[1] - x_win[0]), y_grid)
                        })
                        .collect();
                    ctx.draw(&Points {
                        coords: &grid_pts,
                        color: grid_color,
                    });
                }
            }

            // --- Gradient fill (baseline → trace, alpha 0 → 13 %) ---
            if do_fill {
                let total_span = y_hi - y_lo;
                for &(x, y_top, y_bottom) in &fill_cols {
                    let height_frac = if total_span > f64::EPSILON {
                        (y_top - y_lo) / total_span
                    } else {
                        0.0
                    };
                    let alpha = height_frac.clamp(0.0, 1.0) * 0.13;
                    let fill_color = theme::blend_rgb(trace_color, theme::BASE, alpha);
                    ctx.draw(&Line {
                        x1: x,
                        y1: y_bottom,
                        x2: x,
                        y2: y_top,
                        color: fill_color,
                    });
                }
            }

            // --- Overlay trace (rendered before the main trace) ---
            if let Some((ov_pts, ov_color, ov_alpha)) = overlay_data {
                let blended = theme::blend_rgb(ov_color, theme::BASE, ov_alpha);
                draw_trace(ctx, ov_pts, blended, marker);
            }

            // --- Bar dataset (lower 30 % of canvas) ---
            if let Some((bar_pts, bar_color)) = bars_data {
                draw_bars(ctx, bar_pts, bar_color, y_lo, y_hi);
            }

            // --- Main trace ---
            draw_trace(ctx, pts, trace_color, marker);

            // --- Y-axis labels (top-left max; bottom-left min lifted one row up) ---
            let ov1 = Style::new().fg(theme::OVERLAY1);
            ctx.print(
                x_win[0],
                y_hi,
                TextLine::from(Span::styled(y_labels[2].clone(), ov1)),
            );
            // Lift the min label one cell so it clears the X-axis label row below.
            ctx.print(
                x_win[0],
                y_lo + cell_y,
                TextLine::from(Span::styled(y_labels[0].clone(), ov1)),
            );

            // --- X-axis labels: oldest edge, midpoint, newest edge (bottom row) ---
            // `x_range` is captured from the enclosing scope (computed for the fill).
            let surf2 = Style::new().fg(theme::SURFACE2);
            ctx.print(x_win[0], y_lo, TextLine::from(Span::styled("-10m", surf2)));
            ctx.print(
                x_win[0] + x_range / 2.0,
                y_lo,
                TextLine::from(Span::styled("-5m", surf2)),
            );
            // Inset slightly so the right-edge label stays on screen.
            ctx.print(
                x_range.mul_add(-0.05, x_win[1]),
                y_lo,
                TextLine::from(Span::styled("maint.", surf2)),
            );
        });

    frame.render_widget(canvas, area);
}

/// Draw a trace (dots or connected line segments) onto the canvas context.
#[expect(
    clippy::missing_asserts_for_indexing,
    reason = "pair comes from .windows(2) which always yields exactly 2 elements"
)]
fn draw_trace(ctx: &mut Context<'_>, pts: &[(f64, f64)], color: Color, marker: MarkerStyle) {
    match marker {
        MarkerStyle::Dots => {
            ctx.draw(&Points { coords: pts, color });
        }
        MarkerStyle::Line => {
            for pair in pts.windows(2) {
                let (x1, y1) = pair[0];
                let (x2, y2) = pair[1];
                ctx.draw(&Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    color,
                });
            }
        }
    }
}

/// Draw bars in the lower 30 % of the canvas y-range on their own `[0, max]` scale.
///
/// A zero-valued bar receives a faint baseline tick so the gap remains visible.
fn draw_bars(ctx: &mut Context<'_>, pts: &[(f64, f64)], color: Color, y_lo: f64, y_hi: f64) {
    if pts.is_empty() {
        return;
    }
    let max_bar = pts.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
    let bar_area_height = (y_hi - y_lo) * 0.3;
    let bar_scale = if max_bar > f64::EPSILON {
        bar_area_height / max_bar
    } else {
        1.0
    };
    for &(x, v) in pts {
        if v > 0.0 {
            let y_top = v.mul_add(bar_scale, y_lo);
            ctx.draw(&Line {
                x1: x,
                y1: y_lo,
                x2: x,
                y2: y_top,
                color,
            });
        } else {
            // Faint baseline tick for zero intervals.
            let tick_color = theme::blend_rgb(color, theme::BASE, 0.15);
            let tick_h = (y_hi - y_lo) * 0.01;
            ctx.draw(&Line {
                x1: x,
                y1: y_lo,
                x2: x,
                y2: y_lo + tick_h,
                color: tick_color,
            });
        }
    }
}

/// Build the styled bordered block with title (left) and unit chip (right).
fn make_block(spec: &PlotSpec<'_>) -> Block<'static> {
    let title_line = TextLine::from(Span::styled(
        spec.title.to_owned(),
        Style::new().fg(theme::SUBTEXT0),
    ));
    let unit_chip = TextLine::from(Span::styled(
        format!(" {} ", spec.unit),
        Style::new().fg(theme::OVERLAY2).bg(theme::SURFACE0),
    ))
    .right_aligned();

    Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .title(title_line)
        .title_top(unit_chip)
        .style(Style::new().bg(theme::MANTLE))
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
    use crate::model::Series;
    use crate::theme;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // --- fill_columns ---

    #[test]
    fn fill_columns_interpolates_baseline_to_trace() -> TestResult {
        // Given — a single rising segment, step 0.5, gap budget 10.
        let pts = [(0.0_f64, 1.0_f64), (1.0, 3.0)];
        let y_lo = 0.0_f64;

        // When
        let cols = fill_columns(&pts, y_lo, 0.5, 10.0);

        // Then — columns at x = 0.0, 0.5, 1.0 with interpolated tops 1.0, 2.0, 3.0,
        // each anchored at the baseline.
        assert_eq!(cols.len(), 3, "expected three interpolated columns");
        let expected = [(0.0, 1.0), (0.5, 2.0), (1.0, 3.0)];
        for (idx, (&(x, y_top, y_bottom), &(ex, ey))) in
            cols.iter().zip(expected.iter()).enumerate()
        {
            assert!((x - ex).abs() < 1e-9, "col {idx}: x {x} != {ex}");
            assert!(
                (y_top - ey).abs() < 1e-9,
                "col {idx}: y_top {y_top} != {ey}"
            );
            assert!(
                y_bottom.abs() < f64::EPSILON,
                "col {idx}: y_bottom should equal y_lo (0.0), got {y_bottom}"
            );
        }
        Ok(())
    }

    #[test]
    fn fill_columns_skips_wide_gaps() -> TestResult {
        // Given — two points 100 apart but a gap budget of only 10.
        let pts = [(0.0_f64, 1.0_f64), (100.0, 2.0)];

        // When
        let cols = fill_columns(&pts, 0.0, 1.0, 10.0);

        // Then — the segment exceeds max_gap and is skipped entirely.
        assert!(cols.is_empty(), "wide gap should produce no fill columns");
        Ok(())
    }

    // --- render_plot: empty series ---

    #[test]
    fn render_plot_empty_shows_placeholder() -> TestResult {
        // Given — empty series, 40×8 terminal
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let spec = PlotSpec {
            title: "Temp\u{e9}rature",
            unit: "\u{b0}C",
            color: theme::PEACH,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: MarkerStyle::Dots,
            show_grid: false,
            fill: false,
            overlay: None,
            bars: None,
        };
        let mut series = Series::new(Series::DEFAULT_CAP);

        // When
        terminal.draw(|f| render_plot(f, f.area(), &spec, &mut series))?;

        // Then — buffer must contain the placeholder text; no panic
        let buf_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf_text.contains("en attente"),
            "buffer should contain 'en attente'; got: {buf_text:?}"
        );
        Ok(())
    }

    // --- render_plot: non-empty series (smoke) ---

    #[test]
    fn render_plot_smoke_with_grid_fill_overlay() -> TestResult {
        // Given — five points, overlay, grid and fill enabled, 60×10 terminal
        let backend = ratatui::backend::TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend)?;

        let overlay_pts = [
            (0.0_f64, 0.5_f64),
            (100.0, 1.5),
            (200.0, 2.0),
            (300.0, 2.5),
            (400.0, 3.0),
        ];
        let spec = PlotSpec {
            title: "Temp",
            unit: "\u{b0}C",
            color: theme::PEACH,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: MarkerStyle::Line,
            show_grid: true,
            fill: true,
            overlay: Some(Overlay {
                points: &overlay_pts,
                color: theme::TEAL,
                alpha: 0.3,
            }),
            bars: None,
        };
        let mut series = Series::new(Series::DEFAULT_CAP);
        series.push(0.0, 1.0);
        series.push(100.0, 2.0);
        series.push(200.0, 3.0);
        series.push(300.0, 2.5);
        series.push(400.0, 2.0);

        // When
        terminal.draw(|f| render_plot(f, f.area(), &spec, &mut series))?;

        // Then — no panic; buffer contains the title
        let buf_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf_text.contains("Temp"),
            "buffer should contain the title 'Temp'; got: {buf_text:?}"
        );
        Ok(())
    }
}
// grcov exclude stop
