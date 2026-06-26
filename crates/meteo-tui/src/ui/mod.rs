//! Ratatui UI rendering — one `render` function that draws the full dashboard
//! for a single frame.

pub mod diagnostics;
pub mod header;
pub mod history;
pub mod summary;

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::widgets::Block;

use crate::app::AppState;
use crate::image_render::Images;
use crate::plot;
use crate::theme;

/// Rendering options shared across summary, history, and main render paths.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Trace marker style for history charts.
    pub marker_style: plot::MarkerStyle,
    /// Draw faint gridlines at 25 / 50 / 75 % in history charts.
    pub show_grid: bool,
    /// Show the 60-second heading trail in the wind compass.
    ///
    /// Parsed from the `--gust-trail` CLI flag and retained for API stability,
    /// but no longer affects compass rendering: the image compass replaced the
    /// canvas trail and does not currently render a heading history.
    #[expect(
        dead_code,
        reason = "parsed from --gust-trail CLI flag; retained for API stability — the image compass does not currently render a heading trail"
    )]
    pub gust_trail: bool,
    /// Draw the gradient area fill under history traces. Off by default: in a
    /// monochrome braille terminal the fill turns spiky signals into an
    /// unreadable column skyline, so it is opt-in (`--fill`) for the dossier look.
    pub fill: bool,
}

impl Options {
    /// Test fixture defaults (dots / grid on / trail on / fill on so the fill
    /// path stays exercised by the render smoke tests).
    #[must_use]
    #[cfg(test)]
    pub const fn default_for_test() -> Self {
        Self {
            marker_style: plot::MarkerStyle::Dots,
            show_grid: true,
            gust_trail: true,
            fill: true,
        }
    }
}

/// Draw the full dashboard. `pulse` ∈ [0,1] is the « En direct » dot intensity
/// (computed from wall-clock elapsed in main.rs).
/// `images` carries the image-protocol caches for charts and the compass widget;
/// rebuilds are gated on data version inside [`Images`] so unchanged redraws are cheap.
pub fn render(
    frame: &mut Frame,
    app: &mut AppState,
    now: Instant,
    options: Options,
    pulse: f64,
    images: &mut Images,
) {
    frame.render_widget(
        Block::default().style(Style::new().bg(theme::BASE)),
        frame.area(),
    );
    let [header_a, summary_a, diag_a, history_a] = Layout::vertical([
        Constraint::Length(2),  // header
        Constraint::Length(13), // summary band (cards + compass)
        Constraint::Length(3),  // diagnostics bar
        Constraint::Min(0),     // history grids
    ])
    .areas(frame.area());
    header::render_header(frame, header_a, app, now, pulse);
    summary::render_summary(frame, summary_a, app, images);
    diagnostics::render_diagnostics(frame, diag_a, app, now);
    history::render_history(frame, history_a, app, options, images);
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
    use crate::ble::{BleEvent, FrameEvent};
    use crate::image_render::Images;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_smoke_fills_buffer_without_panic() -> TestResult {
        // Given — large enough that all tiers get room (header 2 + summary 13
        // + diag 3 + history fills remaining rows at 150×40).
        let backend = ratatui::backend::TestBackend::new(150, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let mut images = Images::for_test();
        let t = meteo_lib::Telemetry {
            temperature_c: Some(22.5),
            sky_temp_c: Some(-12.0),
            pressure_hpa: Some(1013.25),
            ..meteo_lib::Telemetry::empty()
        };
        // Feed a fresh frame so signal_state returns Live.
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);

        // When
        terminal.draw(|f| {
            render(
                f,
                &mut app,
                now,
                Options::default_for_test(),
                1.0,
                &mut images,
            );
        })?;

        // Then — buffer must contain the French UI labels from each tier.
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
            buffer_text.contains("ATMOSPH"),
            "buffer should contain 'ATMOSPH\u{c8}RE'; got: {buffer_text:?}"
        );
        assert!(
            buffer_text.contains("CAPTEURS"),
            "buffer should contain 'CAPTEURS'; got: {buffer_text:?}"
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
        let mut images = Images::for_test();
        let t = meteo_lib::Telemetry {
            diagnostics: meteo_lib::Diagnostics::empty().with_baro_fault(true),
            uptime_s: 1,
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);

        // When
        terminal.draw(|f| {
            render(
                f,
                &mut app,
                now,
                Options::default_for_test(),
                1.0,
                &mut images,
            );
        })?;

        // Then — the diagnostics chip for BMP388 must appear in the buffer.
        let buffer_text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buffer_text.contains("BMP388"),
            "buffer should contain 'BMP388'; got: {buffer_text:?}"
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
        let mut images = Images::for_test();

        // When / Then — must not panic, must return Ok
        terminal.draw(|f| {
            render(
                f,
                &mut app,
                now,
                Options::default_for_test(),
                1.0,
                &mut images,
            );
        })?;

        Ok(())
    }
}
// grcov exclude stop
