//! TUI viewer for the `MeteoStation` weather station.
//!
//! Two front-ends share one BLE data feed (`feed.rs`):
//! - the default ratatui dashboard, and
//! - a headless `--no-tui` mode that logs feed events to the console (handy
//!   over SSH, where the feed's `tracing` output is the easiest way to see what
//!   the BLE link is doing). Set `RUST_LOG` to tune verbosity, e.g.
//!   `RUST_LOG=meteo_tui=debug`.
mod app;
mod feed;
mod sensors;
mod ui;

use std::env;
use std::io;
use std::time::Duration;

use futures::StreamExt as _;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::app::{App, ClientEvent};
use crate::sensors::SENSORS;

/// How long to wait for the feed task to tear down before exiting anyway.
const TEARDOWN_GRACE: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    if args.help {
        print_help();
        return Ok(());
    }
    if args.no_tui {
        return run_headless().await;
    }

    let mut terminal = ratatui::init();
    let result = run_tui(&mut terminal).await;
    ratatui::restore();
    result
}

/// Parsed command-line options.
#[derive(Debug, Default, Clone, Copy)]
struct Args {
    /// Run without the TUI, logging feed events to the console.
    no_tui: bool,
    /// Print usage and exit.
    help: bool,
}

impl Args {
    /// Parse `std::env::args`, ignoring the program name.  Unknown flags are
    /// ignored deliberately — this is a single-binary debug viewer, not a CLI
    /// with a stable contract.
    fn parse() -> Self {
        let mut args = Self::default();
        for arg in env::args().skip(1_usize) {
            match arg.as_str() {
                "--no-tui" => args.no_tui = true,
                "-h" | "--help" => args.help = true,
                _ => {}
            }
        }
        args
    }
}

#[expect(
    clippy::print_stdout,
    reason = "usage text is the one thing that legitimately goes to stdout"
)]
fn print_help() {
    println!("meteo-tui — MeteoStation BLE viewer\n");
    println!("USAGE:\n    meteo-tui [OPTIONS]\n");
    println!("OPTIONS:");
    println!("    --no-tui    Run headless; log BLE feed events to the console");
    println!("    -h, --help  Print this help\n");
    println!("Set RUST_LOG to tune log verbosity (e.g. RUST_LOG=meteo_tui=debug).");
}

/// Initialise console logging for headless mode.  Defaults to `info` for the
/// world and `debug` for this crate so the BLE lifecycle is visible without
/// extra configuration; `RUST_LOG` overrides it.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,meteo_tui=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Spawn the BLE feed and wire a shutdown channel.  Returns the receiver of
/// [`ClientEvent`]s, the join handle, and the shutdown sender.
fn spawn_feed() -> (
    mpsc::Receiver<ClientEvent>,
    JoinHandle<()>,
    watch::Sender<bool>,
) {
    let (tx, rx) = mpsc::channel::<ClientEvent>(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        if let Err(err) = feed::run(tx, shutdown_rx).await {
            error!(%err, "BLE feed terminated with a fatal error");
        }
    });
    (rx, handle, shutdown_tx)
}

/// Signal the feed to stop and wait (bounded) for it to tear down.
async fn stop_feed(shutdown_tx: &watch::Sender<bool>, feed_task: JoinHandle<()>) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "feed may have already exited"
    )]
    let _ = shutdown_tx.send(true);
    // Bounded deadlock circuit-breaker: if teardown hangs, proceed anyway.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "best-effort clean teardown before exit"
    )]
    let _ = timeout(TEARDOWN_GRACE, feed_task).await;
}

/// Headless front-end: log every feed event to the console until a signal or a
/// closed feed channel.
async fn run_headless() -> io::Result<()> {
    init_tracing();
    info!("meteo-tui headless mode — Ctrl-C to quit");

    let (mut rx, feed_task, shutdown_tx) = spawn_feed();
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                let Some(event) = maybe_event else {
                    warn!("feed channel closed; exiting");
                    break;
                };
                log_event(event);
            }
            _ = sigterm.recv() => {
                info!("SIGTERM received; shutting down");
                break;
            }
            _ = sigint.recv() => {
                info!("SIGINT received; shutting down");
                break;
            }
        }
    }

    stop_feed(&shutdown_tx, feed_task).await;
    Ok(())
}

/// Log one feed event in headless mode, resolving sensor names from the
/// registry for readable output.
fn log_event(event: ClientEvent) {
    match event {
        ClientEvent::Connected => info!("link connected"),
        ClientEvent::Disconnected => warn!("link disconnected; rescanning"),
        ClientEvent::Reading { index, raw } => {
            if let Some(desc) = SENSORS.get(index) {
                let value = desc.display_value(raw);
                info!(sensor = desc.name, value, unit = desc.unit, "reading");
            } else {
                warn!(index, raw, "reading for unknown sensor index");
            }
        }
    }
}

/// TUI front-end: render the dashboard and drive it until quit.
async fn run_tui(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::new();
    let (mut rx, feed_task, shutdown_tx) = spawn_feed();

    let mut input = EventStream::new();
    // External terminations (`kill`, systemd stop, SSH hang-up) must reach the
    // same graceful shutdown path as `q`/`Esc`; otherwise the feed task never
    // drops its discovery session and BlueZ keeps scanning until the D-Bus
    // connection dies. In crossterm raw mode Ctrl-C is delivered as a key event,
    // but SIGINT/SIGTERM raised by another process still need explicit handling.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
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
            _ = sigterm.recv() => {
                app.should_quit = true;
            }
            _ = sigint.recv() => {
                app.should_quit = true;
            }
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::render(f, &app))?;
    }

    stop_feed(&shutdown_tx, feed_task).await;
    Ok(())
}
