//! History grids — two bordered blocks (CAPTEURS and ÉNERGIE), each containing
//! a grid of [`crate::plot::render_plot`] panels.
//!
//! [`render_history`] splits the given area into a 2/3-tall CAPTEURS block (6
//! plots in a 2 × 3 grid) and a 1/3-tall ÉNERGIE block (3 plots in a 1 × 3
//! row).  Every cell delegates to [`crate::plot::render_plot`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Block;

use crate::app::AppState;
use crate::plot;
use crate::theme;

use super::Options;

/// Render the CAPTEURS (6 plots) and ÉNERGIE (3 plots) history grids into `area`.
///
/// `app` is `&mut` because [`crate::model::Series::points`] calls
/// `make_contiguous` on the internal deque.
#[allow(dead_code, reason = "wired into ui::render in substep 11")]
#[expect(
    clippy::too_many_lines,
    reason = "the function is a flat grid of plot calls; splitting would scatter the cohesive layout"
)]
pub fn render_history(frame: &mut Frame, area: Rect, app: &mut AppState, options: Options) {
    let [capteurs, energie] =
        Layout::vertical([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)]).areas(area);

    // ── CAPTEURS block ─────────────────────────────────────────────────────────
    let cap_block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled("CAPTEURS", Style::new().fg(theme::SUBTEXT0)));
    let cap_inner = cap_block.inner(capteurs);
    frame.render_widget(cap_block, capteurs);

    let [row1, row2] = Layout::vertical([Constraint::Ratio(1, 2); 2]).areas(cap_inner);
    let [temp_a, sky_a, lux_a] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(row1);
    let [press_a, wind_a, hum_a] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(row2);

    // ── Row 1: Température air / Température ciel / Luminosité ────────────────

    plot::render_plot(
        frame,
        temp_a,
        &plot::PlotSpec {
            title: "Temp\u{e9}rature air",
            unit: "\u{b0}C",
            color: theme::PEACH,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.temp,
    );

    plot::render_plot(
        frame,
        sky_a,
        &plot::PlotSpec {
            title: "Temp\u{e9}rature ciel",
            unit: "\u{b0}C",
            color: theme::LAVENDER,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.sky,
    );

    // scale = 0.001: raw lux stored in app.lux; y-axis labels read klx.
    plot::render_plot(
        frame,
        lux_a,
        &plot::PlotSpec {
            title: "Luminosit\u{e9}",
            unit: "klx",
            color: theme::YELLOW,
            prec: 1,
            floor: Some(0.0),
            scale: 0.001,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.lux,
    );

    // ── Row 2: Pression / Vitesse du vent (+ gust overlay) / Humidité (+ rain bars)

    plot::render_plot(
        frame,
        press_a,
        &plot::PlotSpec {
            title: "Pression",
            unit: "hPa",
            color: theme::TEAL,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.pressure,
    );

    // Extract gust points into a local Vec before the render_plot call so the
    // mutable borrow of app.gust (via points()) is released before the
    // immutable-borrow of the slice is used alongside &mut app.wind.
    let gust_pts: Vec<(f64, f64)> = app.gust.points().to_vec();
    plot::render_plot(
        frame,
        wind_a,
        &plot::PlotSpec {
            title: "Vitesse du vent",
            unit: "m/s",
            color: theme::SKY,
            prec: 1,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: Some(plot::Overlay {
                points: &gust_pts,
                color: theme::TEAL,
                alpha: 0.32,
            }),
            bars: None,
        },
        &mut app.wind,
    );

    // Extract rain points for the same reason (bars borrow vs &mut app.humidity).
    let rain_pts: Vec<(f64, f64)> = app.rain.points().to_vec();
    plot::render_plot(
        frame,
        hum_a,
        &plot::PlotSpec {
            title: "Humidit\u{e9} / Pluie",
            unit: "%HR",
            color: theme::SAPPHIRE,
            prec: 0,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: Some(plot::Bars {
                points: &rain_pts,
                color: theme::BLUE,
            }),
        },
        &mut app.humidity,
    );

    // ── ÉNERGIE block ──────────────────────────────────────────────────────────
    let ener_block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled(
            "\u{c9}NERGIE",
            Style::new().fg(theme::SUBTEXT0),
        ));
    let ener_inner = ener_block.inner(energie);
    frame.render_widget(ener_block, energie);

    let [batt_a, solar_a, load_a] =
        Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(ener_inner);

    plot::render_plot(
        frame,
        batt_a,
        &plot::PlotSpec {
            title: "Batterie",
            unit: "V",
            color: theme::GREEN,
            prec: 2,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.batt_v,
    );

    plot::render_plot(
        frame,
        solar_a,
        &plot::PlotSpec {
            title: "Puissance solaire",
            unit: "W",
            color: theme::YELLOW,
            prec: 1,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.solar_w,
    );

    plot::render_plot(
        frame,
        load_a,
        &plot::PlotSpec {
            title: "Puissance charge",
            unit: "W",
            color: theme::MAUVE,
            prec: 1,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: true,
            overlay: None,
            bars: None,
        },
        &mut app.load_w,
    );
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
    use std::time::Instant;

    use test_log::test;

    use super::*;
    use crate::app::AppState;
    use crate::ble::{BleEvent, FrameEvent};
    use crate::ui::Options;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_history_smoke() -> TestResult {
        // Given — a 150×30 terminal with five distinct frames so series have points
        let backend = ratatui::backend::TestBackend::new(150, 30);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);

        for uptime_s in [1_u32, 2, 3, 4, 5] {
            let t = meteo_lib::Telemetry {
                temperature_c: Some(22.5),
                sky_temp_c: Some(-10.0),
                pressure_hpa: Some(1013.0),
                humidity_pct: Some(60.0),
                luminosity_lux: Some(1000.0),
                wind_speed_ms: Some(3.0),
                wind_dir_deg: Some(270.0),
                rain_rate_mm_h: Some(0.5),
                solar_mv: Some(15_000),
                solar_ma: Some(500),
                batt_mv: Some(3_900),
                load_ma: Some(100),
                uptime_s,
                ..meteo_lib::Telemetry::empty()
            };
            app.apply(BleEvent::Frame(FrameEvent::new(t)), now);
        }

        // When
        terminal.draw(|f| render_history(f, f.area(), &mut app, Options::default_for_test()))?;

        // Then — buffer must contain key labels from both grids
        let buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf.contains("CAPTEURS"),
            "buffer should contain 'CAPTEURS'; got: {buf:?}"
        );
        assert!(
            buf.contains("Vitesse du vent"),
            "buffer should contain 'Vitesse du vent'; got: {buf:?}"
        );
        assert!(
            buf.contains("Puissance solaire"),
            "buffer should contain 'Puissance solaire'; got: {buf:?}"
        );
        assert!(
            buf.contains("klx"),
            "buffer should contain 'klx'; got: {buf:?}"
        );

        Ok(())
    }
}
// grcov exclude stop
