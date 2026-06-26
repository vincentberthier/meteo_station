//! History grids — two bordered blocks (CAPTEURS and ÉNERGIE), each containing
//! a grid of image-rendered chart panels.
//!
//! [`render_history`] splits the given area into a 2/3-tall CAPTEURS block (6
//! plots in a 2 × 3 grid) and a 1/3-tall ÉNERGIE block (3 plots in a 1 × 3
//! row).  Every cell delegates to [`draw_chart_panel`] which drives
//! [`crate::image_render::Images::draw_chart`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Paragraph};

use crate::app::AppState;
use crate::image_render;
use crate::model::{self, Series};
use crate::plot;
use crate::theme;

use super::Options;

/// Draw one chart panel: block frame + image chart + axis-label overlays.
///
/// `spec` carries the panel configuration (title, unit, color, etc.).
/// `series` is `&mut` because [`Series::points`] calls `make_contiguous`.
/// `version` is `app.frame_count` so the cached image is rebuilt only when
/// new data arrives; `id` is a unique `&'static str` key per panel.
#[expect(
    clippy::too_many_arguments,
    reason = "panel renderer needs frame, images, area, spec, series, id, version, and the smoothing width"
)]
fn draw_chart_panel(
    frame: &mut Frame,
    images: &mut image_render::Images,
    area: Rect,
    spec: &plot::PlotSpec<'_>,
    series: &mut Series,
    id: &'static str,
    version: u64,
    smooth_sigma: f64,
) {
    let block = plot::make_block(spec);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let x_win = series.x_window().unwrap_or([-Series::WINDOW_SECS, 0.0]);
    let yb = series.y_bounds().unwrap_or((-1.0, 1.0));
    let y_win = model::padded_value_bounds(yb.0, yb.1, spec.floor);

    // Layout: a left gutter for the Y-axis labels and a bottom row for the X-axis
    // labels, with the image filling the remaining plot area. The gutters are
    // DISJOINT from the image: the image's anchor cell (which carries the iTerm2
    // escape) must never be overwritten by a label, or the image vanishes.
    let [body_a, xaxis_a] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let [ygut_a, plot_a] =
        Layout::horizontal([Constraint::Length(7), Constraint::Min(0)]).areas(body_a);

    // The chart image fills `plot_a`. Rebuilds are gated on `version`
    // (= frame_count) inside Images, so the smoothing pass below runs only on a
    // cache miss (new data / resize), not on every 10 Hz redraw. Only the main
    // trace is smoothed; the gust overlay and rain bars in `spec` stay raw.
    images.draw_chart(frame, plot_a, id, version, |w, h| {
        let smoothed = model::gaussian_smooth(series.points(), smooth_sigma);
        image_render::render_chart_image(spec, &smoothed, x_win, y_win, w, h)
    });

    // ── Y-axis labels in the left gutter (top = max, bottom = min) ───────────
    let y_win_scaled = [y_win[0] * spec.scale, y_win[1] * spec.scale];
    let [ymin_str, _ymid, ymax_str] = model::value_axis_labels(y_win_scaled, spec.prec);
    let ov1 = Style::new().fg(theme::OVERLAY1);
    let ymax_area = Rect {
        height: ygut_a.height.min(1),
        ..ygut_a
    };
    frame.render_widget(Paragraph::new(Span::styled(ymax_str, ov1)), ymax_area);
    let ymin_area = Rect {
        y: ygut_a.bottom().saturating_sub(1),
        height: ygut_a.height.min(1),
        ..ygut_a
    };
    frame.render_widget(Paragraph::new(Span::styled(ymin_str, ov1)), ymin_area);

    // ── X-axis labels in the bottom row, aligned under the plot area ─────────
    let surf2 = Style::new().fg(theme::SURFACE2);
    let [_xgut, xlab_a] =
        Layout::horizontal([Constraint::Length(7), Constraint::Min(0)]).areas(xaxis_a);
    let [xl, xm, xr] = Layout::horizontal([Constraint::Fill(1); 3]).areas(xlab_a);
    frame.render_widget(Paragraph::new(Span::styled("-10m", surf2)), xl);
    frame.render_widget(Paragraph::new(Span::styled("-5m", surf2)).centered(), xm);
    frame.render_widget(
        Paragraph::new(Span::styled("maint.", surf2)).right_aligned(),
        xr,
    );
}

/// Render the CAPTEURS (6 plots) and ÉNERGIE (3 plots) history grids into `area`.
///
/// `app` is `&mut` because [`Series::points`] calls `make_contiguous`.
/// `images` carries the cached image protocols; each plot is re-rasterized only
/// when `app.frame_count` changes or the panel area resizes.
#[expect(
    clippy::too_many_lines,
    reason = "the function is a flat grid of panel calls; splitting would scatter the cohesive layout"
)]
pub fn render_history(
    frame: &mut Frame,
    area: Rect,
    app: &mut AppState,
    options: Options,
    images: &mut image_render::Images,
) {
    let version = app.frame_count;

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

    draw_chart_panel(
        frame,
        images,
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
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.temp,
        "temp",
        version,
        options.smooth_sigma,
    );

    draw_chart_panel(
        frame,
        images,
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
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.sky,
        "sky",
        version,
        options.smooth_sigma,
    );

    // Adaptive unit: the series stores raw lux. A peak below 1000 lux reads in
    // raw lux (klx would round the whole axis to 0.0); above it reads in klx.
    let lux_peak = app.lux.y_bounds().map_or(0.0, |(_, hi)| hi);
    let (lux_unit, lux_scale, lux_prec) = model::lux_chart_unit(lux_peak);
    draw_chart_panel(
        frame,
        images,
        lux_a,
        &plot::PlotSpec {
            title: "Luminosit\u{e9}",
            unit: lux_unit,
            color: theme::YELLOW,
            prec: lux_prec,
            floor: Some(0.0),
            scale: lux_scale,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.lux,
        "lux",
        version,
        options.smooth_sigma,
    );

    // ── Row 2: Pression / Vitesse du vent (+ gust overlay) / Humidité (+ rain bars)

    draw_chart_panel(
        frame,
        images,
        press_a,
        &plot::PlotSpec {
            title: "Pression",
            unit: "hPa",
            // BLUE (not TEAL) so pressure is clearly distinct from the cyan wind line.
            color: theme::BLUE,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.pressure,
        "press",
        version,
        options.smooth_sigma,
    );

    // Extract gust points before borrowing app.wind so the mutable borrow of
    // app.gust (via points()) is released before &mut app.wind is taken.
    let gust_pts: Vec<(f64, f64)> = app.gust.points().to_vec();
    let wind_spec = plot::PlotSpec {
        title: "Vitesse du vent",
        unit: "m/s",
        color: theme::SKY,
        prec: 1,
        floor: Some(0.0),
        scale: 1.0,
        marker: options.marker_style,
        show_grid: options.show_grid,
        fill: options.fill,
        overlay: Some(plot::Overlay {
            points: &gust_pts,
            // TEAL at a higher alpha so the gust envelope reads clearly above the
            // SKY wind line instead of washing out into it.
            color: theme::TEAL,
            alpha: 0.55,
        }),
        bars: None,
    };
    draw_chart_panel(
        frame,
        images,
        wind_a,
        &wind_spec,
        &mut app.wind,
        "wind",
        version,
        options.smooth_sigma,
    );

    // Extract rain points for the same reason (bars borrow vs &mut app.humidity).
    let rain_pts: Vec<(f64, f64)> = app.rain.points().to_vec();
    let hum_spec = plot::PlotSpec {
        title: "Humidit\u{e9} / Pluie",
        unit: "%HR",
        color: theme::SAPPHIRE,
        prec: 0,
        floor: Some(0.0),
        scale: 1.0,
        marker: options.marker_style,
        show_grid: options.show_grid,
        fill: options.fill,
        overlay: None,
        bars: Some(plot::Bars {
            points: &rain_pts,
            color: theme::BLUE,
        }),
    };
    draw_chart_panel(
        frame,
        images,
        hum_a,
        &hum_spec,
        &mut app.humidity,
        "hum",
        version,
        options.smooth_sigma,
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

    draw_chart_panel(
        frame,
        images,
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
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.batt_v,
        "batt",
        version,
        options.smooth_sigma,
    );

    draw_chart_panel(
        frame,
        images,
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
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.solar_w,
        "solar",
        version,
        options.smooth_sigma,
    );

    draw_chart_panel(
        frame,
        images,
        load_a,
        &plot::PlotSpec {
            title: "Puissance utilis\u{e9}e",
            unit: "W",
            color: theme::MAUVE,
            prec: 1,
            floor: Some(0.0),
            scale: 1.0,
            marker: options.marker_style,
            show_grid: options.show_grid,
            fill: options.fill,
            overlay: None,
            bars: None,
        },
        &mut app.load_w,
        "load",
        version,
        options.smooth_sigma,
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
    use crate::image_render::Images;
    use crate::ui::Options;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_history_smoke() -> TestResult {
        // Given — a 150×30 terminal with five distinct frames so series have points
        let backend = ratatui::backend::TestBackend::new(150, 30);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let mut images = Images::for_test();

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
        terminal.draw(|f| {
            render_history(
                f,
                f.area(),
                &mut app,
                Options::default_for_test(),
                &mut images,
            );
        })?;

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
        // With data present, no chart shows the empty placeholder.
        assert!(
            !buf.contains("attente"),
            "no chart should show the empty placeholder with data present; got: {buf:?}"
        );

        Ok(())
    }
}
// grcov exclude stop
