//! Header band for the TUI dashboard.
//!
//! Renders a one-row strip: station name and GPS location (left),
//! wall clock and link-state indicator (right), with a [`theme::BORDER`]-coloured
//! bottom rule below the content row.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{AppState, STATION_DEFAULT};
use crate::model::{self, SignalState};
use crate::theme;

/// Render the top header strip.
///
/// Left column: a `◆` glyph in [`theme::SKY`], station name in bold
/// [`theme::TEXT`], then `·  app v{version}  ·  {gps}` in [`theme::OVERLAY1`].
///
/// Right column (right-aligned): wall clock in [`theme::SUBTEXT1`], a coloured
/// link-state dot and label, and `·  1 Hz` broadcast-rate suffix in
/// [`theme::SUBTEXT0`].
///
/// `pulse` drives the pulsing animation of the `Live` dot: `0.0` fades the dot
/// to [`theme::BASE`] (invisible), `1.0` renders at full [`theme::GREEN`].
///
/// A [`theme::BORDER`]-coloured bottom rule is drawn over `area`.
#[allow(dead_code, reason = "wired into ui::render in substep 11")]
pub fn render_header(frame: &mut Frame, area: Rect, app: &AppState, now: Instant, pulse: f64) {
    // Bottom rule drawn first so content paragraphs are painted on top of it.
    frame.render_widget(
        Block::new()
            .borders(Borders::BOTTOM)
            .border_style(Style::new().fg(theme::BORDER)),
        area,
    );

    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);

    let station = app.station.as_deref().unwrap_or(STATION_DEFAULT);
    let gps = model::fmt_location(
        app.latest.latitude_deg,
        app.latest.longitude_deg,
        app.latest.altitude_m,
    );

    let left_line = Line::from(vec![
        Span::styled("◆ ", Style::new().fg(theme::SKY)),
        Span::styled(station.to_owned(), Style::new().fg(theme::TEXT).bold()),
        Span::styled(
            format!("  ·  app v{}  ·  {gps}", app.app_version),
            Style::new().fg(theme::OVERLAY1),
        ),
    ]);

    let clock = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let (label, dot_col) = match app.signal_state(now) {
        SignalState::Live => (
            "En direct",
            theme::blend_rgb(theme::GREEN, theme::BASE, pulse),
        ),
        SignalState::Stale | SignalState::NoSignal => ("Hors ligne", theme::RED),
    };

    let right_line = Line::from(vec![
        Span::styled(clock, Style::new().fg(theme::SUBTEXT1)),
        Span::styled("   ● ", Style::new().fg(dot_col)),
        Span::styled(
            format!("{label}  ·  1 Hz"),
            Style::new().fg(theme::SUBTEXT0),
        ),
    ])
    .right_aligned();

    frame.render_widget(Paragraph::new(left_line), left);
    frame.render_widget(Paragraph::new(right_line), right);
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
    use std::time::{Duration, Instant};

    use meteo_lib::Telemetry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use test_log::test;

    use super::*;
    use crate::app::{AppState, STALE_AFTER};
    use crate::ble::{BleEvent, FrameEvent};

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_header_live_shows_en_direct() -> TestResult {
        // Given — a fresh frame applied so signal_state returns Live
        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = Telemetry {
            uptime_s: 1,
            ..Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);

        // When
        terminal.draw(|f| render_header(f, f.area(), &app, now, 1.0))?;

        // Then — buffer must contain "En direct", the station name, and "1 Hz"
        let buffer_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buffer_text.contains("En direct"),
            "buffer should contain 'En direct'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("MeteoStation"),
            "buffer should contain 'MeteoStation' (default station name); got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("1 Hz"),
            "buffer should contain '1 Hz'; got: {buffer_text:?}"
        );

        Ok(())
    }

    #[test]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "test: Instant + Duration cannot overflow in practice"
    )]
    fn render_header_offline_when_stale() -> TestResult {
        // Given — a frame applied at `base`, then `now` advanced well past STALE_AFTER
        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend)?;
        let base = Instant::now();
        let mut app = AppState::new(base);
        let t = Telemetry {
            uptime_s: 1,
            ..Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), base);
        // Advance `now` well past STALE_AFTER (5 s) so signal_state returns Stale
        let future_now = base + STALE_AFTER + Duration::from_secs(10);

        // When
        terminal.draw(|f| render_header(f, f.area(), &app, future_now, 1.0))?;

        // Then — buffer must contain "Hors ligne" and "1 Hz"
        let buffer_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buffer_text.contains("Hors ligne"),
            "buffer should contain 'Hors ligne'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("1 Hz"),
            "buffer should contain '1 Hz'; got: {buffer_text:?}"
        );

        Ok(())
    }
}
// grcov exclude stop
