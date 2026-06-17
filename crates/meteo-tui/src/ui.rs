//! Ratatui UI rendering — one `render` function that draws the full dashboard
//! for a single frame.

// render is not yet called from main.rs; wired in substep 7.
#![allow(dead_code, reason = "consumed by main.rs wiring in substep 7")]

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Chart, Dataset, GraphType, LegendPosition, Paragraph, Row, Table,
};

use crate::app::{AppState, STALE_AFTER};
use crate::model::{self, ConnState, Series};

/// Draw the full dashboard for one frame.
///
/// Takes `app` as `&mut AppState` because [`Series::points`] calls
/// `make_contiguous` on the internal deque.
pub fn render(frame: &mut Frame, app: &mut AppState, now: Instant) {
    let [header, table_area, charts] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(11),
        Constraint::Min(0),
    ])
    .areas(frame.area());

    // Immutable borrows must be fully consumed before the mutable borrow for
    // render_charts below.  The render_* calls are sequenced accordingly.
    render_header(frame, header, app);
    render_table(frame, table_area, app, now);
    render_charts(frame, charts, app);
}

/// Render the top header strip: clock | version info | connection status.
fn render_header(frame: &mut Frame, area: Rect, app: &AppState) {
    let [clock, versions, status] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area);

    frame.render_widget(
        Paragraph::new(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
        clock,
    );
    frame.render_widget(
        Paragraph::new(format!(
            "app v{}  fw {}",
            app.app_version,
            app.fw_version.as_deref().unwrap_or("unknown")
        )),
        versions,
    );

    let color = match app.conn {
        ConnState::Live => Color::Green,
        ConnState::Reconnecting => Color::Red,
        ConnState::Scanning | ConnState::Connecting | ConnState::Resolving => Color::Yellow,
    };
    frame.render_widget(
        Paragraph::new(app.conn.label()).style(Style::new().fg(color)),
        status,
    );
}

/// Render the telemetry table with nine rows (eight values + diagnostics).
///
/// Values are dimmed cosmetically when the last frame is older than
/// [`STALE_AFTER`]. The diagnostics row is highlighted in red when any
/// diagnostic flag is set.
fn render_table(frame: &mut Frame, area: Rect, app: &AppState, now: Instant) {
    let t = &app.latest;
    let rows_data = [
        ("Temperature", model::fmt_temp(t.temperature_c)),
        ("Pressure", model::fmt_pressure(t.pressure_hpa)),
        ("Humidity", model::fmt_humidity(t.humidity_pct)),
        ("Sky temp", model::fmt_temp(t.sky_temp_c)),
        ("Luminosity", model::fmt_lux(t.luminosity_lux)),
        ("Wind speed", model::fmt_wind_speed(t.wind_speed_ms)),
        ("Wind dir", model::fmt_wind_dir(t.wind_dir_deg)),
        ("Battery", model::fmt_battery(t.battery_pct)),
    ];
    let base = if app.is_stale(now, STALE_AFTER) {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    };
    let diag_style = if model::diagnostics_alert(t.diagnostics) {
        base.fg(Color::Red)
    } else {
        base
    };
    let diag_row = Row::new([
        "Diagnostics".to_owned(),
        model::fmt_diagnostics(t.diagnostics),
    ])
    .style(diag_style);
    let table = Table::new(
        rows_data
            .iter()
            .map(|(k, v)| Row::new([(*k).to_owned(), v.clone()]).style(base))
            .chain(std::iter::once(diag_row)),
        [Constraint::Length(14), Constraint::Min(0)],
    )
    .block(Block::bordered().title("Telemetry"));
    frame.render_widget(table, area);
}

/// Static descriptors for one telemetry chart.
struct ChartSpec {
    /// Block border title (the metric name).
    title: &'static str,
    /// Unit, used as the y-axis title and the legend key (e.g. `"°C"`).
    unit: &'static str,
    /// Decimal precision for the y-axis tick labels.
    prec: usize,
    /// Plot-line colour.
    color: Color,
}

/// Render the two time-series charts (temperature and pressure).
fn render_charts(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let [top, bottom] = Layout::vertical([Constraint::Ratio(1, 2); 2]).areas(area);
    render_series_chart(
        frame,
        top,
        &ChartSpec {
            title: "Temperature",
            unit: "°C",
            prec: 1,
            color: Color::LightRed,
        },
        &mut app.temp,
    );
    render_series_chart(
        frame,
        bottom,
        &ChartSpec {
            title: "Pressure",
            unit: "hPa",
            prec: 1,
            color: Color::LightCyan,
        },
        &mut app.pressure,
    );
}

/// Render a single time-series line chart, or an "awaiting data" placeholder
/// when the series has no points yet.
///
/// The x-axis is a right-anchored [`Series::WINDOW_SECS`] window: the newest
/// sample sits at the right edge and older samples scroll left, so the chart
/// fills in from the right rather than stretching a few points across the full
/// width. Both axes carry titles, tick labels, and a legend.
fn render_series_chart(frame: &mut Frame, area: Rect, spec: &ChartSpec, series: &mut Series) {
    let (Some(x_win), Some(y_raw)) = (series.x_window(), series.y_bounds()) else {
        frame.render_widget(
            Paragraph::new("awaiting data").block(Block::bordered().title(spec.title)),
            area,
        );
        return;
    };
    // `x_win` and `y_win` are `[f64; 2]`, feeding `Axis::bounds` directly.
    let y_win = model::padded_value_bounds(y_raw.0, y_raw.1);
    let y_labels = model::value_axis_labels(y_win, spec.prec);

    let data = series.points();
    let datasets = vec![
        Dataset::default()
            .name(spec.unit)
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(spec.color))
            .data(data),
    ];

    let axis_style = Style::new().fg(Color::DarkGray);
    let chart = Chart::new(datasets)
        .block(Block::bordered().title(spec.title))
        .legend_position(Some(LegendPosition::TopRight))
        .x_axis(
            Axis::default()
                .title("time")
                .style(axis_style)
                .bounds(x_win)
                // Fixed right-anchored window (Series::WINDOW_SECS = 600 s),
                // oldest → newest.
                .labels(["-10m", "-5m", "now"]),
        )
        .y_axis(
            Axis::default()
                .title(spec.unit)
                .style(axis_style)
                .bounds(y_win)
                .labels(y_labels),
        );
    frame.render_widget(chart, area);
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
    use crate::ble::BleEvent;
    use crate::model::ConnState;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_smoke_fills_buffer_without_panic() -> TestResult {
        // Given
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = meteo_lib::Telemetry {
            temperature_c: Some(22.5),
            pressure_hpa: Some(1013.25),
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(t), now);
        app.apply(BleEvent::State(ConnState::Live), now);

        // When
        terminal.draw(|f| render(f, &mut app, now))?;

        // Then — buffer must contain the connection label and app version prefix
        let buffer_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buffer_text.contains("Live"),
            "buffer should contain 'Live'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("app v"),
            "buffer should contain 'app v'; got: {buffer_text:?}"
        );
        // Chart-exclusive content: the x-axis title proves the labelled chart
        // rendered (the telemetry table has no "time" cell).
        assert!(
            buffer_text.contains("time"),
            "buffer should contain the chart x-axis title 'time'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("Diagnostics"),
            "buffer should contain 'Diagnostics'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("OK"),
            "buffer should contain 'OK' for a clear diagnostics field; got: {buffer_text:?}"
        );

        Ok(())
    }

    #[test]
    fn render_shows_baro_fault_diagnostic() -> TestResult {
        // Given
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = meteo_lib::Telemetry {
            diagnostics: meteo_lib::Diagnostics::empty().with_baro_fault(true),
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(t), now);
        app.apply(BleEvent::State(ConnState::Live), now);

        // When
        terminal.draw(|f| render(f, &mut app, now))?;

        // Then — buffer must contain the BMP388 fault label
        let buffer_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buffer_text.contains("BMP388 fault"),
            "buffer should contain 'BMP388 fault'; got: {buffer_text:?}"
        );

        Ok(())
    }

    #[test]
    fn render_smoke_small_terminal_no_panic() -> TestResult {
        // Given — tiny terminal that might trigger layout edge cases
        let backend = ratatui::backend::TestBackend::new(40, 12);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);

        // When / Then — must not panic, must return Ok
        terminal.draw(|f| render(f, &mut app, now))?;

        Ok(())
    }
}
// grcov exclude stop
