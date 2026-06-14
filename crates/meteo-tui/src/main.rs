//! TUI viewer for the `MeteoStation` weather station.
mod app;
mod feed;
mod sensors;
mod ui;

use std::io;
use std::time::Duration;

use futures::StreamExt as _;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::timeout;

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
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // The data feed runs in its own task. With no transport wired yet it
    // produces nothing and the UI shows `Scanning`; see `feed.rs`.
    let feed_task = tokio::spawn(async move {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "feed exits only when the UI is gone; nothing to report"
        )]
        let _ = feed::run(tx, shutdown_rx).await;
    });

    let mut input = EventStream::new();
    terminal.draw(|f| ui::render(f, &app))?; // initial frame

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                if let Some(event) = maybe_event {
                    app.apply(event);
                }
                // `None` = feed task ended; keep the UI up.
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

    // Signal the feed task to stop and wait for it to tear down.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "feed may have already exited"
    )]
    let _ = shutdown_tx.send(true);
    // Bounded deadlock circuit-breaker: if teardown hangs, proceed after 2 s
    // rather than blocking quit indefinitely.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "best-effort clean teardown before exit"
    )]
    let _ = timeout(Duration::from_secs(2), feed_task).await;

    Ok(())
}
