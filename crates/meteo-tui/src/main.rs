//! meteo-tui: terminal dashboard for the `MeteoStation` BLE peripheral.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "meteo-tui is a host std binary; core/alloc-first lints do not apply"
)]

mod app;
mod ble;
mod compass;
mod model;
mod plot;
mod theme;
mod ui;

use std::time::Duration;

use clap::Parser;

/// Marker style for history charts.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum MarkerArg {
    /// Individual Braille dots — one glyph per sample (sparse).
    #[default]
    Dots,
    /// Braille line segments connecting consecutive samples.
    Line,
}

impl From<MarkerArg> for plot::MarkerStyle {
    fn from(m: MarkerArg) -> Self {
        match m {
            MarkerArg::Dots => Self::Dots,
            MarkerArg::Line => Self::Line,
        }
    }
}

#[derive(Parser)]
#[command(version, about = "MeteoStation live BLE dashboard")]
struct Cli {
    /// Station BLE address (`BlueZ` display order). Defaults to the firmware address.
    #[arg(long, default_value = "F0:CA:FE:00:00:01")]
    address: String,

    /// Trace marker style for history charts (dots or line).
    #[arg(long, value_enum, default_value_t = MarkerArg::Dots)]
    marker_style: MarkerArg,

    /// Draw faint gridlines at 25 / 50 / 75 % in history charts.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    show_grid: bool,

    /// Show the 60-second heading trail in the wind compass.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    gust_trail: bool,

    /// Draw the gradient area fill under history traces (dossier look). Off by
    /// default — in a braille terminal the fill makes spiky signals unreadable.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    fill: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let addr: bluer::Address = cli.address.parse()?;
    let options = ui::Options {
        marker_style: cli.marker_style.into(),
        show_grid: cli.show_grid,
        gust_trail: cli.gust_trail,
        fill: cli.fill,
    };

    // ratatui::init() enables raw mode + alternate screen AND installs a panic
    // hook that restores the terminal on panic. ratatui::restore() undoes it.
    let mut terminal: ratatui::DefaultTerminal = ratatui::init();
    let res = run_app(&mut terminal, addr, options).await;
    ratatui::restore();
    res
}

async fn run_app(
    terminal: &mut ratatui::DefaultTerminal,
    addr: bluer::Address,
    options: ui::Options,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(crate::ble::run(tx, addr));

    let mut input = crossterm::event::EventStream::new();
    // Display cadence: 10 Hz, SOLELY to advance the displayed wall clock and
    // animate the pulsing dot. This is a display cadence, NOT a readiness
    // sleep — you cannot observe the wall clock advancing except via a timer.
    // All DATA-driven redraws happen on BLE/input events below; this tick
    // only keeps the clock and pulse animation live.
    let mut clock = tokio::time::interval(Duration::from_millis(100));

    let mut app = crate::app::AppState::new(std::time::Instant::now());
    let started = std::time::Instant::now();
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => app.apply(ev, std::time::Instant::now()),
            Some(Ok(term_ev)) = input.next() => {
                if should_quit(&term_ev) { break; }
            }
            _ = clock.tick() => {}
        }
        let pulse = pulse_intensity(started.elapsed());
        terminal.draw(|f| {
            crate::ui::render(f, &mut app, std::time::Instant::now(), options, pulse);
        })?;
    }
    Ok(())
}

/// 1.6 s triangle wave in [0.35,1.0] for the « En direct » dot. Pure.
fn pulse_intensity(elapsed: Duration) -> f64 {
    const PERIOD: f64 = 1.6;
    let phase = (elapsed.as_secs_f64() % PERIOD) / PERIOD; // 0..1
    let tri = 1.0 - phase.mul_add(2.0, -1.0).abs(); // 0..1..0
    0.35 + tri * 0.65 // 0.35..1.0
}

/// Pure: quit on 'q', Esc, or Ctrl-C. Testable (no I/O).
const fn should_quit(ev: &crossterm::event::Event) -> bool {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    matches!(
        ev,
        Event::Key(
            KeyEvent {
                code: KeyCode::Char('q') | KeyCode::Esc,
                ..
            } | KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
        )
    )
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

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn should_quit_on_q_key() -> TestResult {
        // Given
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        // When
        let result = should_quit(&ev);

        // Then
        assert!(result);
        Ok(())
    }

    #[test]
    fn should_quit_on_esc() -> TestResult {
        // Given
        let ev = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // When
        let result = should_quit(&ev);

        // Then
        assert!(result);
        Ok(())
    }

    #[test]
    fn should_quit_on_ctrl_c() -> TestResult {
        // Given
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

        // When
        let result = should_quit(&ev);

        // Then
        assert!(result);
        Ok(())
    }

    #[test]
    fn should_not_quit_on_other_key() -> TestResult {
        // Given
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        // When
        let result = should_quit(&ev);

        // Then
        assert!(!result);
        Ok(())
    }

    #[test]
    fn pulse_intensity_bounds() -> TestResult {
        // Given / When
        let at_zero = pulse_intensity(Duration::ZERO);
        let at_800ms = pulse_intensity(Duration::from_millis(800));

        // Then — both are within [0.35, 1.0] and they differ across the cycle
        assert!(
            (0.35..=1.0).contains(&at_zero),
            "pulse at 0ms should be in [0.35, 1.0]; got {at_zero}"
        );
        assert!(
            (0.35..=1.0).contains(&at_800ms),
            "pulse at 800ms should be in [0.35, 1.0]; got {at_800ms}"
        );
        assert!(
            (at_zero - at_800ms).abs() > f64::EPSILON,
            "pulse at 0ms ({at_zero}) and 800ms ({at_800ms}) should differ"
        );

        Ok(())
    }
}
// grcov exclude stop
