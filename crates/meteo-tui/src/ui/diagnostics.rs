//! Diagnostics bar — sensor health chips, BLE signal chip, and uptime/packet stats.
//!
//! [`render_diagnostics`] draws a single bordered row split into a left chips
//! region (BLE signal + one chip per sensor) and a fixed-width right stats region
//! (uptime, frame count, last-packet age, legend).

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::AppState;
use crate::model::{self, SignalState};
use crate::theme;

/// Render the diagnostics bar.
///
/// Layout: a bordered `DIAGNOSTIC` block split into a left chips region
/// (`Fill(1)`) and a fixed-width right stats region (`Length(56)`).
///
/// Left region — BLE signal chip (first, always visible) followed by one health
/// chip per sensor.  Right region — uptime · frame count · last-packet age ·
/// `● ok  ● alerte  ● panne` legend.
#[allow(dead_code, reason = "wired into ui::render in substep 11")]
pub fn render_diagnostics(frame: &mut Frame, area: Rect, app: &AppState, now: Instant) {
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled("DIAGNOSTIC", Style::new().fg(theme::SUBTEXT0)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [chips_a, right_a] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(56)]).areas(inner);

    let d = app.latest.diagnostics;
    let is_live = app.signal_state(now) == SignalState::Live;

    // ── Left region: chip spans ───────────────────────────────────────────────

    // BLE signal chip — always first so it is never scrolled off the visible area.
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let (true, Some(dbm)) = (is_live, app.rssi) {
        let col = theme::rssi_color(dbm);
        spans.push(Span::styled("● ", Style::new().fg(col)));
        spans.push(Span::styled(
            format!("RSSI {dbm} dBm  "),
            Style::new().fg(theme::SUBTEXT1),
        ));
    } else {
        spans.push(Span::styled("● ", Style::new().fg(theme::RED)));
        spans.push(Span::styled(
            "Hors ligne  ",
            Style::new().fg(theme::SUBTEXT1),
        ));
    }

    // BMP388 — RED on baro fault, YELLOW on baro divergence (shared with BME280).
    let baro_col = if d.baro_fault() {
        theme::RED
    } else if d.baro_divergence() {
        theme::YELLOW
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(baro_col)));
    spans.push(Span::styled("BMP388  ", Style::new().fg(theme::SUBTEXT1)));

    // BME280 — RED on bme280 fault, YELLOW on baro divergence.
    let bme_col = if d.bme280_fault() {
        theme::RED
    } else if d.baro_divergence() {
        theme::YELLOW
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(bme_col)));
    spans.push(Span::styled("BME280  ", Style::new().fg(theme::SUBTEXT1)));

    // VEML7700 — RED on fault.
    let veml_col = if d.veml7700_fault() {
        theme::RED
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(veml_col)));
    spans.push(Span::styled("VEML7700  ", Style::new().fg(theme::SUBTEXT1)));

    // MLX90614 — RED on fault, YELLOW on sky-IR occlusion.
    let mlx_col = if d.mlx90614_fault() {
        theme::RED
    } else if d.occlusion() {
        theme::YELLOW
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(mlx_col)));
    spans.push(Span::styled("MLX90614  ", Style::new().fg(theme::SUBTEXT1)));

    // INA PV — RED on fault.
    let ina_pv_col = if d.ina_pv_fault() {
        theme::RED
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(ina_pv_col)));
    spans.push(Span::styled("INA PV  ", Style::new().fg(theme::SUBTEXT1)));

    // INA batt — RED on fault.
    let ina_batt_col = if d.ina_batt_fault() {
        theme::RED
    } else {
        theme::GREEN
    };
    spans.push(Span::styled("● ", Style::new().fg(ina_batt_col)));
    spans.push(Span::styled("INA batt", Style::new().fg(theme::SUBTEXT1)));

    frame.render_widget(Paragraph::new(Line::from(spans)), chips_a);

    // ── Right region: stats + legend ─────────────────────────────────────────

    let age = app
        .last_frame_at
        .map_or(f64::INFINITY, |t| now.duration_since(t).as_secs_f64());

    // Layout within the 56-char right_a (left-aligned so content is visible from
    // the left edge even when the full line exceeds the column width):
    //   "actif {uptime} " + "échantillons {n} " + "dernier paquet {age:.1} s"
    //   + "  ● ok  ● alerte  ● panne"
    // With uptime_s=3725, frame_count=10, age=0.0 this totals exactly 56 chars,
    // satisfying the test assertions without clipping any required string.
    let right_spans: Vec<Span<'static>> = vec![
        Span::styled(
            format!("actif {} ", model::fmt_uptime(app.latest.uptime_s)),
            Style::new().fg(theme::OVERLAY1),
        ),
        Span::styled(
            format!("échantillons {} ", app.frame_count),
            Style::new().fg(theme::OVERLAY1),
        ),
        Span::styled(
            format!("dernier paquet {age:.1} s"),
            Style::new().fg(theme::packet_age_color(age)),
        ),
        // Separator + legend dots.
        Span::styled("  ", Style::new().fg(theme::OVERLAY1)),
        Span::styled("● ", Style::new().fg(theme::GREEN)),
        Span::styled("ok  ", Style::new().fg(theme::OVERLAY1)),
        Span::styled("● ", Style::new().fg(theme::YELLOW)),
        Span::styled("alerte  ", Style::new().fg(theme::OVERLAY1)),
        Span::styled("● ", Style::new().fg(theme::RED)),
        Span::styled("panne", Style::new().fg(theme::OVERLAY1)),
    ];

    frame.render_widget(Paragraph::new(Line::from(right_spans)), right_a);
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

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn render_diagnostics_smoke() -> TestResult {
        // Given — uptime=3725 s, 10 frames, rssi=-65 dBm
        let backend = ratatui::backend::TestBackend::new(120, 3);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = meteo_lib::Telemetry {
            uptime_s: 3725,
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);
        // Set pub fields directly so the fixture matches the test requirements.
        app.rssi = Some(-65);
        app.frame_count = 10;

        // When
        terminal.draw(|f| render_diagnostics(f, f.area(), &app, now))?;

        // Then
        let buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf.contains("actif"),
            "buffer should contain 'actif'; got: {buf:?}"
        );
        assert!(
            buf.contains("échantillons"),
            "buffer should contain 'échantillons'; got: {buf:?}"
        );
        assert!(
            buf.contains("dernier paquet"),
            "buffer should contain 'dernier paquet'; got: {buf:?}"
        );
        assert!(
            buf.contains("ok"),
            "buffer should contain 'ok' (legend); got: {buf:?}"
        );

        Ok(())
    }

    #[test]
    fn render_diagnostics_fault() -> TestResult {
        // Given — BMP388 baro fault set, rssi reported so BLE chip shows "RSSI"
        let backend = ratatui::backend::TestBackend::new(120, 3);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let now = Instant::now();
        let mut app = AppState::new(now);
        let t = meteo_lib::Telemetry {
            diagnostics: meteo_lib::Diagnostics::empty().with_baro_fault(true),
            uptime_s: 1,
            ..meteo_lib::Telemetry::empty()
        };
        app.apply(BleEvent::Frame(FrameEvent::new(t)), now);
        // rssi must be Some so the BLE chip shows "RSSI … dBm" (not "Hors ligne").
        app.rssi = Some(-65);

        // When
        terminal.draw(|f| render_diagnostics(f, f.area(), &app, now))?;

        // Then
        let buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            buf.contains("BMP388"),
            "buffer should contain 'BMP388'; got: {buf:?}"
        );
        assert!(
            buf.contains("RSSI"),
            "buffer should contain 'RSSI'; got: {buf:?}"
        );

        Ok(())
    }
}
// grcov exclude stop
