//! Summary band — three side-by-side cards: ATMOSPHÈRE, VENT, ÉNERGIE.
//!
//! [`render_summary`] splits the given area into three equal columns and
//! renders each card in its column. The ATMOSPHÈRE card shows atmospheric
//! sensor readings; the VENT card delegates to the compass widget; the
//! ÉNERGIE card shows solar, battery, and load data.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::app::AppState;
use crate::compass::{CompassData, render_compass};
use crate::model;
use crate::theme;

use super::Options;

/// Render the three-card summary band (ATMOSPHÈRE · VENT · ÉNERGIE).
///
/// `app` is `&mut` because the VENT card calls [`crate::model::Series::points`]
/// (which calls `make_contiguous` internally) on the gust and heading series.
#[allow(dead_code, reason = "wired into ui::render in substep 11")]
pub fn render_summary(frame: &mut Frame, area: Rect, app: &mut AppState, options: Options) {
    let [atmo, vent, ener] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area);
    render_atmosphere(frame, atmo, app);
    render_vent(frame, vent, app, options);
    render_energie(frame, ener, app);
}

// ── ATMOSPHÈRE ────────────────────────────────────────────────────────────────

/// Render the ATMOSPHÈRE card: hero temperature, 10-min trend, and six sensor rows.
#[expect(
    clippy::too_many_lines,
    reason = "the card body is a vertical stack of styled rows; splitting would scatter cohesive layout"
)]
fn render_atmosphere(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().bg(theme::MANTLE))
        .title(Line::from(Span::styled(
            "ATMOSPH\u{00c8}RE",
            Style::new().fg(theme::SUBTEXT0),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let t = &app.latest;
    let diag = t.diagnostics;

    let [
        hero_area,
        hum_a,
        press_a,
        sky_a,
        lux_a,
        rain_a,
        dew_a,
        _rest,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    // ── Hero row: temperature (left) + trend arrow (right) ───────────────────
    let [hero_left, hero_right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(hero_area);

    let (temp_str, temp_fg) = t.temperature_c.map_or_else(
        || ("N/A".to_owned(), theme::OVERLAY2),
        |v| (format!("{v:.1}"), theme::PEACH),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Air  ", Style::new().fg(theme::OVERLAY2)),
            Span::styled(temp_str, Style::new().fg(temp_fg)),
            Span::styled(" \u{00b0}C", Style::new().fg(theme::OVERLAY2)),
        ])),
        hero_left,
    );

    if let Some(d) = app.temp.trend_delta(600.0) {
        let (trend_str, trend_col) = match model::classify_trend(d, 0.1) {
            model::Trend::Rising => (format!("\u{25b2} +{d:.1} \u{00b0}C / 10m"), theme::GREEN),
            model::Trend::Falling => (format!("\u{25bc} {d:.1} \u{00b0}C / 10m"), theme::PEACH),
            model::Trend::Stable => ("stable".to_owned(), theme::OVERLAY1),
        };
        frame.render_widget(
            Paragraph::new(
                Line::from(Span::styled(trend_str, Style::new().fg(trend_col))).right_aligned(),
            ),
            hero_right,
        );
    }

    // ── Sensor data rows ─────────────────────────────────────────────────────

    // Humidité — BME280
    let hum_line = t.humidity_pct.filter(|_| !diag.bme280_fault()).map_or_else(
        || row_na("Humidit\u{00e9}"),
        |v| row_line("Humidit\u{00e9}", format!("{v:.0} %RH"), theme::SAPPHIRE),
    );
    frame.render_widget(Paragraph::new(hum_line), hum_a);

    // Pression — BMP388
    let press_line = t.pressure_hpa.filter(|_| !diag.baro_fault()).map_or_else(
        || row_na("Pression"),
        |v| row_line("Pression", format!("{v:.1} hPa"), theme::TEAL),
    );
    frame.render_widget(Paragraph::new(press_line), press_a);

    // Temp. ciel — MLX90614
    let sky_line = t.sky_temp_c.filter(|_| !diag.mlx90614_fault()).map_or_else(
        || row_na("Temp. ciel"),
        |v| row_line("Temp. ciel", format!("{v:.1} \u{00b0}C"), theme::LAVENDER),
    );
    frame.render_widget(Paragraph::new(sky_line), sky_a);

    // Lumin. — VEML7700
    let lux_line = t
        .luminosity_lux
        .filter(|_| !diag.veml7700_fault())
        .map_or_else(
            || row_na("Lumin."),
            |v| row_line("Lumin.", model::fmt_lux_klx(Some(v)), theme::YELLOW),
        );
    frame.render_widget(Paragraph::new(lux_line), lux_a);

    // Pluie — rain gauge (no dedicated fault bit)
    let rain_line = t.rain_rate_mm_h.map_or_else(
        || row_na("Pluie"),
        |v| row_line("Pluie", format!("{v:.1} mm/h"), theme::BLUE),
    );
    frame.render_widget(Paragraph::new(rain_line), rain_a);

    // Pt rosée — derived from temperature + humidity (both must be valid)
    let dew_line = match (
        t.temperature_c.filter(|_| !diag.baro_fault()),
        t.humidity_pct.filter(|_| !diag.bme280_fault()),
    ) {
        (Some(temp), Some(rh)) => {
            let dp = model::dew_point_c(temp, rh);
            row_line(
                "Pt ros\u{00e9}e",
                format!("{dp:.1} \u{00b0}C"),
                theme::OVERLAY2,
            )
        }
        _ => row_na("Pt ros\u{00e9}e"),
    };
    frame.render_widget(Paragraph::new(dew_line), dew_a);
}

// ── VENT ──────────────────────────────────────────────────────────────────────

/// Render the VENT card by delegating to the compass widget.
fn render_vent(frame: &mut Frame, area: Rect, app: &mut AppState, options: Options) {
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().bg(theme::MANTLE))
        .title(Line::from(Span::styled(
            "VENT",
            Style::new().fg(theme::SUBTEXT0),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let speed_ms = app.latest.wind_speed_ms.map(f64::from);
    let heading_deg = app.latest.wind_dir_deg.map(f64::from);

    // Consume the gust series point; borrow is released after .copied().map().
    let gust_ms = app.gust.points().last().copied().map(|(_, v)| v);

    // Borrow the heading trail; lasts until CompassData is dropped.
    let heading_pts = app.heading.points();
    let now_secs = heading_pts.last().map_or(0.0, |&(t, _)| t);

    let data = CompassData {
        speed_ms,
        heading_deg,
        gust_ms,
        trail: heading_pts,
        now_secs,
        show_trail: options.gust_trail,
    };

    render_compass(frame, inner, &data);
}

// ── ÉNERGIE ───────────────────────────────────────────────────────────────────

/// Render the ÉNERGIE card: solar, battery (gauge + flow), and load rows.
#[expect(
    clippy::too_many_lines,
    reason = "the card body is a vertical stack of styled rows; splitting would scatter cohesive layout"
)]
fn render_energie(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().bg(theme::MANTLE))
        .title(Line::from(Span::styled(
            "\u{00c9}NERGIE",
            Style::new().fg(theme::SUBTEXT0),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let t = &app.latest;
    let solar_w = model::power_w(t.solar_mv, t.solar_ma);
    let load_w = model::power_w(t.batt_mv, t.load_ma);

    let [
        sol_head_a,
        sol_sub_a,
        bat_head_a,
        bat_gauge_a,
        flow_a,
        load_head_a,
        load_sub_a,
        _rest,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    // ── Solaire ───────────────────────────────────────────────────────────────
    let solar_val_span = solar_w.map_or_else(
        || {
            Span::styled(
                "N/A",
                Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
            )
        },
        |w| Span::styled(format!("{w:.1} W"), Style::new().fg(theme::YELLOW)),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(theme::YELLOW)),
            Span::styled("Solaire   ", Style::new().fg(theme::OVERLAY0)),
            solar_val_span,
        ])),
        sol_head_a,
    );

    let solar_sub = match (t.solar_mv, t.solar_ma) {
        (Some(mv), Some(ma)) => {
            let v = f64::from(mv) / 1000.0;
            Line::from(Span::styled(
                format!("  {v:.2} V \u{00b7} {ma} mA"),
                Style::new().fg(theme::SURFACE2).add_modifier(Modifier::DIM),
            ))
        }
        _ => Line::from(Span::styled(
            "  N/A",
            Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
        )),
    };
    frame.render_widget(Paragraph::new(solar_sub), sol_sub_a);

    // ── Batterie ──────────────────────────────────────────────────────────────
    let bat_val_span = t.battery_pct.map_or_else(
        || {
            Span::styled(
                "N/A",
                Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
            )
        },
        |pct| {
            Span::styled(
                format!("{pct} %"),
                Style::new().fg(theme::battery_color(pct)),
            )
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(theme::GREEN)),
            Span::styled("Batterie  ", Style::new().fg(theme::OVERLAY0)),
            bat_val_span,
        ])),
        bat_head_a,
    );

    // Battery gauge (1-row progress bar)
    let pct_ratio = t
        .battery_pct
        .map_or(0.0, |p| (f64::from(p) / 100.0).clamp(0.0, 1.0));
    let bat_col = t.battery_pct.map_or(theme::OVERLAY2, theme::battery_color);
    frame.render_widget(
        Gauge::default()
            .ratio(pct_ratio)
            .gauge_style(Style::new().fg(bat_col).bg(theme::CRUST))
            .block(Block::default().style(Style::new().bg(theme::SURFACE0))),
        bat_gauge_a,
    );

    // Flow line (charge / discharge / stable / N/A)
    let flow_str = model::fmt_battery_flow(solar_w, load_w, t.battery_pct);
    let flow_col = if flow_str.starts_with('\u{25b2}') {
        theme::GREEN
    } else if flow_str.starts_with('\u{25bc}') {
        theme::RED
    } else {
        theme::OVERLAY1
    };
    frame.render_widget(
        Paragraph::new(Span::styled(flow_str, Style::new().fg(flow_col))),
        flow_a,
    );

    // ── Charge (load) ─────────────────────────────────────────────────────────
    let load_val_span = load_w.map_or_else(
        || {
            Span::styled(
                "N/A",
                Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
            )
        },
        |w| Span::styled(format!("{w:.1} W"), Style::new().fg(theme::MAUVE)),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(theme::MAUVE)),
            Span::styled("Charge    ", Style::new().fg(theme::OVERLAY0)),
            load_val_span,
        ])),
        load_head_a,
    );

    let load_sub = t.load_ma.map_or_else(
        || {
            Line::from(Span::styled(
                "  N/A",
                Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
            ))
        },
        |ma| {
            Line::from(Span::styled(
                format!("  {ma} mA"),
                Style::new().fg(theme::SURFACE2).add_modifier(Modifier::DIM),
            ))
        },
    );
    frame.render_widget(Paragraph::new(load_sub), load_sub_a);
}

// ── Row helpers ───────────────────────────────────────────────────────────────

/// Build a name-value row line with the given colour for the value span.
///
/// Name is left-padded to 11 chars in [`theme::OVERLAY0`]; value in `col`.
fn row_line(name: &str, val: String, col: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name:<11}"), Style::new().fg(theme::OVERLAY0)),
        Span::styled(val, Style::new().fg(col)),
    ])
}

/// Build a dimmed « N/A » row for a faulted or absent sensor.
fn row_na(name: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name:<11}"), Style::new().fg(theme::OVERLAY0)),
        Span::styled(
            "N/A",
            Style::new().fg(theme::OVERLAY2).add_modifier(Modifier::DIM),
        ),
    ])
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
    fn render_summary_smoke() -> TestResult {
        // Given — a full frame applied so all major sensor fields are populated
        let backend = ratatui::backend::TestBackend::new(120, 16);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = meteo_lib::Telemetry {
            temperature_c: Some(22.5),
            humidity_pct: Some(65.0),
            pressure_hpa: Some(1013.2),
            solar_mv: Some(15_000),
            solar_ma: Some(600),
            batt_mv: Some(3_900),
            battery_pct: Some(80),
            load_ma: Some(120),
            uptime_s: 1,
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);

        // When
        terminal.draw(|f| {
            render_summary(f, f.area(), &mut app, Options::default_for_test());
        })?;

        // Then — all three card titles and dew-point label must appear in the buffer
        let buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf.contains("ATMOSPH"),
            "buffer should contain 'ATMOSPH\u{c8}RE'; got: {buf:?}"
        );
        assert!(
            buf.contains("VENT"),
            "buffer should contain 'VENT'; got: {buf:?}"
        );
        assert!(
            buf.contains("NERGIE"),
            "buffer should contain '\u{c9}NERGIE'; got: {buf:?}"
        );
        assert!(
            buf.contains("Pt ros"),
            "buffer should contain 'Pt ros\u{e9}e'; got: {buf:?}"
        );

        Ok(())
    }

    #[test]
    fn render_summary_none_fields() -> TestResult {
        // Given — no frame applied; all sensor fields are None
        let backend = ratatui::backend::TestBackend::new(120, 16);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);

        // When
        terminal.draw(|f| {
            render_summary(f, f.area(), &mut app, Options::default_for_test());
        })?;

        // Then — card title present, N/A placeholders rendered (exercises None paths)
        let buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf.contains("ATMOSPH"),
            "buffer should contain 'ATMOSPH\u{c8}RE' even with no data; got: {buf:?}"
        );
        assert!(
            buf.contains("N/A"),
            "buffer should contain 'N/A' placeholder for absent sensors; got: {buf:?}"
        );

        Ok(())
    }
}
// grcov exclude end
