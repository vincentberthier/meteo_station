//! BLE TUI viewer for the `MeteoStation` weather station.
mod app;
mod client;
mod ui;

use std::io;

use futures::StreamExt as _;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use tokio::sync::mpsc;

use crate::app::{App, ClientEvent};

#[tokio::main]
async fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::new();
    let (tx, mut rx) = mpsc::channel::<ClientEvent>(64);
    // Auto-reconnect client runs in its own task; if it can't start (no
    // adapter) the UI still runs and shows `Scanning`.
    tokio::spawn(async move {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "client exits only when the UI is gone; nothing to report"
        )]
        let _ = client::run(tx).await;
    });

    let mut input = EventStream::new();
    terminal.draw(|f| ui::render(f, &app))?; // initial frame

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                if let Some(event) = maybe_event {
                    app.apply(event);
                }
                // `None` = client task ended; keep the UI up.
            }
            maybe_input = input.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_input
                    && key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    app.should_quit = true;
                }
            }
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::render(f, &app))?;
    }
    Ok(())
}
