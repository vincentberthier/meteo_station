//! Ratatui UI rendering — one `render` function that draws the full dashboard
//! for a single frame.

// render is not yet called from main.rs; wired in substep 7.
#![allow(dead_code, reason = "consumed by main.rs wiring in substep 7")]

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph, Row, Table};

use crate::app::{AppState, STALE_AFTER};
use crate::model::{self, ConnState, Series};

/// Draw the full dashboard for one frame.
///
/// Takes `app` as `&mut AppState` because [`Series::points`] calls
/// `make_contiguous` on the internal deque.
pub fn render(frame: &mut Frame, app: &mut AppState, now: Instant) {
    let [header, table_area, charts] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(10),
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

/// Render the telemetry table with eight sensor rows.
///
/// Values are dimmed cosmetically when the last frame is older than
/// [`STALE_AFTER`].
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
    let table = Table::new(
        rows_data
            .iter()
            .map(|(k, v)| Row::new([(*k).to_owned(), v.clone()]).style(base)),
        [Constraint::Length(14), Constraint::Min(0)],
    )
    .block(Block::bordered().title("Telemetry"));
    frame.render_widget(table, area);
}

/// Render the two time-series charts (temperature and pressure).
fn render_charts(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let [top, bottom] = Layout::vertical([Constraint::Ratio(1, 2); 2]).areas(area);
    render_series_chart(frame, top, "Temperature (°C)", &mut app.temp);
    render_series_chart(frame, bottom, "Pressure (hPa)", &mut app.pressure);
}

/// Render a single line chart, or an "awaiting data" placeholder when the
/// series has no points yet.
fn render_series_chart(frame: &mut Frame, area: Rect, title: &str, series: &mut Series) {
    let (Some(x_range), Some(y_range)) = (series.x_bounds(), series.y_bounds()) else {
        frame.render_widget(
            Paragraph::new("awaiting data").block(Block::bordered().title(title)),
            area,
        );
        return;
    };
    let data = series.points();
    let datasets = vec![Dataset::default().graph_type(GraphType::Line).data(data)];
    let chart = Chart::new(datasets)
        .block(Block::bordered().title(title))
        .x_axis(Axis::default().bounds([x_range.0, x_range.1]))
        .y_axis(Axis::default().bounds([y_range.0, y_range.1]));
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
