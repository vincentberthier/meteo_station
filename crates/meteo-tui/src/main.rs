//! meteo-tui: terminal dashboard for the `MeteoStation` BLE peripheral.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "meteo-tui is a host std binary; core/alloc-first lints do not apply"
)]

mod app;
mod ble;
mod model;
mod ui;

use clap::Parser;

#[derive(Parser)]
#[command(version, about = "MeteoStation live BLE dashboard")]
struct Cli {
    /// Station BLE address (`BlueZ` display order). Defaults to the firmware address.
    #[arg(long, default_value = "F0:CA:FE:00:00:01")]
    address: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let addr: bluer::Address = cli.address.parse()?;

    // ratatui::init() enables raw mode + alternate screen AND installs a panic
    // hook that restores the terminal on panic. ratatui::restore() undoes it.
    let mut terminal: ratatui::DefaultTerminal = ratatui::init();
    let res = run_app(&mut terminal, addr).await;
    ratatui::restore();
    res
}

async fn run_app(
    terminal: &mut ratatui::DefaultTerminal,
    addr: bluer::Address,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(crate::ble::run(tx, addr));

    let mut input = crossterm::event::EventStream::new();
    // Clock refresh cadence: 1 Hz, SOLELY to advance the displayed wall clock and
    // re-render. This is a display cadence, NOT a readiness sleep — you cannot
    // observe the wall clock advancing except via a timer. All DATA-driven redraws
    // happen on BLE/input events below; this tick only keeps the clock live.
    let mut clock = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut app = crate::app::AppState::new(std::time::Instant::now());
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => app.apply(ev, std::time::Instant::now()),
            Some(Ok(term_ev)) = input.next() => {
                if should_quit(&term_ev) { break; }
            }
            _ = clock.tick() => {}
        }
        terminal.draw(|f| crate::ui::render(f, &mut app, std::time::Instant::now()))?;
    }
    Ok(())
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
}
// grcov exclude stop
