// consumed by the VENT card (substep 8)
#![allow(dead_code, reason = "consumed by the VENT card (substep 8)")]
//! Canvas compass widget for the VENT card centre.
//!
//! [`heading_to_xy`] is a pure geometry helper (unit-tested on the host).
//! [`render_compass`] draws a full wind dial: rings, ticks, N/E/S/O cardinal
//! labels, needle triangle with tail, optional fading heading trail, and a
//! readout line showing speed, direction, compass point, and gust speed.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line, Points};

use crate::model::compass_label_fr;
use crate::theme;

/// Heading (deg, 0°=N=up, 90°=E=right) → unit-circle coords on radius `r`.
///
/// `x = r·sin(θ)`, `y = r·cos(θ)`. Testable without ratatui.
#[must_use]
pub fn heading_to_xy(deg: f64, r: f64) -> (f64, f64) {
    let rad = deg.to_radians();
    (r * rad.sin(), r * rad.cos())
}

/// Inputs the compass widget needs (kept render-agnostic for smoke tests).
pub struct CompassData<'data> {
    /// Wind speed in m/s; `None` if unavailable.
    pub speed_ms: Option<f64>,
    /// Wind direction in degrees (0°=N, 90°=E); `None` if unavailable.
    pub heading_deg: Option<f64>,
    /// Gust speed in m/s; `None` if unavailable.
    pub gust_ms: Option<f64>,
    /// `(t_secs, heading_deg)` trail points, newest-last; faded by age.
    pub trail: &'data [(f64, f64)],
    /// Latest timestamp used for trail age calculation (seconds).
    pub now_secs: f64,
    /// Whether to draw the heading trail (driven by the `--gust-trail` CLI flag).
    pub show_trail: bool,
}

/// Render the wind compass dial into `area`.
///
/// Draws outer ring (r=0.9), inner ring (r=0.55), hub (r=0.12), short ticks
/// every 15° (r 0.82→0.9) and long ticks every 45° (r 0.74→0.9), N/E/S/O
/// cardinal labels (N in red), needle triangle + tail, optional fading trail,
/// and a centre readout. Shows « calme » and hides the needle when
/// `speed_ms < 0.3` or `speed_ms` is `None`.
#[expect(
    clippy::too_many_lines,
    reason = "painting closure is inherently long; splitting it would fragment the draw order"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f64 heading truncated to f32 for compass label lookup; display precision loss is acceptable"
)]
pub fn render_compass(frame: &mut Frame, area: Rect, data: &CompassData) {
    let speed = data.speed_ms;
    let heading = data.heading_deg;
    let gust = data.gust_ms;
    let trail = data.trail;
    let now_secs = data.now_secs;
    let show_trail = data.show_trail;

    let calm = speed.is_none_or(|s| s < 0.3);

    let canvas = Canvas::default()
        .x_bounds([-1.1, 1.1])
        .y_bounds([-1.4, 1.2])
        .marker(Marker::Braille)
        .background_color(theme::MANTLE)
        .paint(move |ctx| {
            // --- Rings ---
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.9,
                color: theme::SURFACE0,
            });
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.55,
                color: theme::HAIRLINE,
            });
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.12,
                color: theme::CRUST,
            });

            // --- Short ticks every 15°: r 0.82 → 0.9 ---
            for i in 0..24_u32 {
                let deg = f64::from(i) * 15.0;
                let (x1, y1) = heading_to_xy(deg, 0.82);
                let (x2, y2) = heading_to_xy(deg, 0.9);
                ctx.draw(&Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    color: theme::SURFACE1,
                });
            }

            // --- Long ticks every 45°: r 0.74 → 0.9 ---
            for i in 0..8_u32 {
                let deg = f64::from(i) * 45.0;
                let (x1, y1) = heading_to_xy(deg, 0.74);
                let (x2, y2) = heading_to_xy(deg, 0.9);
                ctx.draw(&Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    color: theme::OVERLAY0,
                });
            }

            // --- Cardinal labels ---
            ctx.print(
                0.0,
                0.97,
                TextLine::from(Span::styled("N", Style::new().fg(theme::RED))),
            );
            ctx.print(
                0.97,
                0.0,
                TextLine::from(Span::styled("E", Style::new().fg(theme::OVERLAY1))),
            );
            ctx.print(
                0.0,
                -0.97,
                TextLine::from(Span::styled("S", Style::new().fg(theme::OVERLAY1))),
            );
            ctx.print(
                -0.97,
                0.0,
                TextLine::from(Span::styled("O", Style::new().fg(theme::OVERLAY1))),
            );

            // --- Needle (hidden when calm or heading unknown) ---
            if !calm && let Some(h) = heading {
                let (tip_x, tip_y) = heading_to_xy(h, 0.78);
                // Base of the needle triangle: perpendicular to the heading at hub radius.
                let base_port = heading_to_xy(h + 90.0, 0.08);
                let base_stbd = heading_to_xy(h - 90.0, 0.08);
                let (tail_x, tail_y) = heading_to_xy(h + 180.0, 0.3);

                // Needle triangle (SKY colour)
                ctx.draw(&Line {
                    x1: tip_x,
                    y1: tip_y,
                    x2: base_port.0,
                    y2: base_port.1,
                    color: theme::SKY,
                });
                ctx.draw(&Line {
                    x1: tip_x,
                    y1: tip_y,
                    x2: base_stbd.0,
                    y2: base_stbd.1,
                    color: theme::SKY,
                });
                ctx.draw(&Line {
                    x1: base_port.0,
                    y1: base_port.1,
                    x2: base_stbd.0,
                    y2: base_stbd.1,
                    color: theme::SKY,
                });

                // Needle tail (NEEDLE_TAIL colour)
                ctx.draw(&Line {
                    x1: tail_x,
                    y1: tail_y,
                    x2: base_port.0,
                    y2: base_port.1,
                    color: theme::NEEDLE_TAIL,
                });
                ctx.draw(&Line {
                    x1: tail_x,
                    y1: tail_y,
                    x2: base_stbd.0,
                    y2: base_stbd.1,
                    color: theme::NEEDLE_TAIL,
                });
            }

            // --- Optional heading trail ---
            if show_trail {
                for &(t, hdg) in trail {
                    let age = now_secs - t;
                    let age_alpha = (1.0 - age / 60.0).clamp(0.0, 1.0);
                    let color = theme::blend_rgb(theme::SKY, theme::BASE, age_alpha);
                    let (tx, ty) = heading_to_xy(hdg, 0.65);
                    ctx.draw(&Points {
                        coords: &[(tx, ty)],
                        color,
                    });
                }
            }

            // --- Centre readout (below the dial, y ≈ −1.15) ---
            let readout = if calm {
                TextLine::from(Span::styled("calme", Style::new().fg(theme::SKY)))
            } else {
                let speed_str = speed.map_or_else(|| "N/A".to_owned(), |s| format!("{s:.1}"));
                let (heading_str, label_str) = heading.map_or_else(
                    || ("N/A".to_owned(), ""),
                    |h| {
                        let label = compass_label_fr(h as f32);
                        (format!("{h:.0}"), label)
                    },
                );
                let gust_str = gust.map_or(String::new(), |g| format!(" rafale {g:.1}"));
                TextLine::from(vec![
                    Span::styled(speed_str, Style::new().fg(theme::SKY)),
                    Span::styled(
                        format!(" m/s · {heading_str}° {label_str}"),
                        Style::new().fg(theme::SUBTEXT0),
                    ),
                    Span::styled(gust_str, Style::new().fg(theme::TEAL)),
                ])
            };
            ctx.print(-1.0, -1.15, readout);
        });

    frame.render_widget(canvas, area);
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

    #[test]
    fn heading_to_xy_cardinals() -> TestResult {
        // Given
        let eps = 1e-9_f64;

        // When / Then — N: 0° → (0, 1) (up)
        let north_pt = heading_to_xy(0.0, 1.0);
        assert!(
            north_pt.0.abs() < eps,
            "N x should be 0, got {}",
            north_pt.0
        );
        assert!(
            (north_pt.1 - 1.0).abs() < eps,
            "N y should be 1, got {}",
            north_pt.1
        );

        // E: 90° → (1, 0) (right)
        let east_pt = heading_to_xy(90.0, 1.0);
        assert!(
            (east_pt.0 - 1.0).abs() < eps,
            "E x should be 1, got {}",
            east_pt.0
        );
        assert!(east_pt.1.abs() < eps, "E y should be 0, got {}", east_pt.1);

        // S: 180° → (0, −1) (down)
        let south_pt = heading_to_xy(180.0, 1.0);
        assert!(
            south_pt.0.abs() < eps,
            "S x should be 0, got {}",
            south_pt.0
        );
        assert!(
            (south_pt.1 - (-1.0)).abs() < eps,
            "S y should be -1, got {}",
            south_pt.1
        );

        // W: 270° → (−1, 0) (left)
        let west_pt = heading_to_xy(270.0, 1.0);
        assert!(
            (west_pt.0 - (-1.0)).abs() < eps,
            "W x should be -1, got {}",
            west_pt.0
        );
        assert!(west_pt.1.abs() < eps, "W y should be 0, got {}", west_pt.1);

        Ok(())
    }

    #[test]
    fn render_compass_smoke_no_panic() -> TestResult {
        // Given — heading 270° (West/O), speed 4 m/s, gust 6 m/s, 3 trail points
        let trail = [(100.0_f64, 260.0_f64), (110.0, 265.0), (120.0, 270.0)];
        let data = CompassData {
            speed_ms: Some(4.0),
            heading_deg: Some(270.0),
            gust_ms: Some(6.0),
            trail: &trail,
            now_secs: 120.0,
            show_trail: true,
        };
        let backend = ratatui::backend::TestBackend::new(40, 16);
        let mut terminal = ratatui::Terminal::new(backend)?;

        // When
        terminal.draw(|f| render_compass(f, f.area(), &data))?;

        // Then — no panic; buffer contains "O" (West cardinal / readout) and "rafale"
        let buf_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf_text.contains('O'),
            "buffer should contain 'O' (West cardinal or readout); got: {buf_text:?}"
        );
        assert!(
            buf_text.contains("rafale"),
            "buffer should contain 'rafale' in the readout; got: {buf_text:?}"
        );
        Ok(())
    }

    #[test]
    fn render_compass_calm_shows_calme() -> TestResult {
        // Given — speed below the calm threshold (0.3 m/s)
        let data = CompassData {
            speed_ms: Some(0.1),
            heading_deg: None,
            gust_ms: None,
            trail: &[],
            now_secs: 0.0,
            show_trail: false,
        };
        let backend = ratatui::backend::TestBackend::new(40, 16);
        let mut terminal = ratatui::Terminal::new(backend)?;

        // When
        terminal.draw(|f| render_compass(f, f.area(), &data))?;

        // Then — buffer contains "calme"
        let buf_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf_text.contains("calme"),
            "buffer should contain 'calme' when calm; got: {buf_text:?}"
        );
        Ok(())
    }
}
// grcov exclude stop
