//! Transport-agnostic data feed seam.
//!
//! This is the single place to wire a data source (serial, USB, a network
//! socket, …): parse incoming readings and emit [`ClientEvent`]s on `tx`. A
//! real implementation will look like:
//!
//! ```ignore
//! tx.send(ClientEvent::Connected).await?;
//! tx.send(ClientEvent::Reading { index, raw }).await?; // index into `sensors::SENSORS`
//! tx.send(ClientEvent::Disconnected).await?;
//! ```
//!
//! Until a transport is wired the feed has no data source, so it simply idles
//! until the UI requests shutdown and then exits cleanly. The UI keeps running
//! and shows `Scanning`.
use std::error::Error;

use tokio::sync::mpsc::Sender;
use tokio::sync::watch;

use crate::app::ClientEvent;

/// Run until shutdown. No data source is wired yet, so this produces no events.
pub async fn run(
    tx: Sender<ClientEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn Error>> {
    // Hold `tx` so the seam's type is fixed; a real transport sends on it.
    let _tx = &tx;
    // Block until the UI signals shutdown (or drops the sender on quit).
    #[expect(
        clippy::let_underscore_must_use,
        reason = "changed() errors only when the UI is gone — also a shutdown"
    )]
    let _ = shutdown.changed().await;
    Ok(())
}
