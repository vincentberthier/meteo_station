//! Auto-reconnecting btleplug central feeding readings to the UI.
use std::error::Error;
use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt as _;
use meteo_lib::ble::encoding::decode_f32;
use meteo_lib::ble::registry::{SENSORS, index_for_uuid};
use tokio::sync::mpsc::Sender;
use tokio::time;
use uuid::Uuid;

use crate::app::ClientEvent;

/// Poll cadence while waiting for the device to (re)appear during a scan.
/// This is a bounded poll-with-check (each tick inspects `peripherals()`), the
/// allowed form of waiting — not a fixed guess at how long a step takes. The
/// project's no-`sleep` rule bans *bare fixed delays used as synchronisation*;
/// this loop is exempt because every iteration checks a real condition (device
/// present in scan results) before sleeping again.
const SCAN_POLL: Duration = Duration::from_millis(200);

/// Run forever: scan → connect → stream → on drop, rescan.
/// Returns only on an unrecoverable setup error (no adapter); the UI keeps
/// running and shows `Scanning`.
pub async fn run(tx: Sender<ClientEvent>) -> Result<(), Box<dyn Error>> {
    let manager = Manager::new().await?;
    let adapter = first_adapter(&manager).await?;
    loop {
        // A session error (or normal disconnect) drops us back to rescan; the
        // outcome is intentionally ignored. `#[expect]` documents the discard
        // and satisfies the workspace `let_underscore_must_use` lint (which
        // fires on `let _ = <Result>` under `-D warnings`).
        #[expect(
            clippy::let_underscore_must_use,
            reason = "session end/error both mean: rescan"
        )]
        let _ = session(&adapter, &tx).await;
        if tx.send(ClientEvent::Disconnected).await.is_err() {
            return Ok(()); // UI gone (user quit) — stop.
        }
    }
}

async fn first_adapter(manager: &Manager) -> Result<Adapter, Box<dyn Error>> {
    manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "no BLE adapters found".into())
}

/// One connection lifecycle: wait for the device, connect, subscribe to every
/// registered NOTIFY characteristic, then forward readings until the stream
/// ends (disconnect).
async fn session(adapter: &Adapter, tx: &Sender<ClientEvent>) -> Result<(), Box<dyn Error>> {
    let device = wait_for_station(adapter).await?;
    device.connect().await?;
    device.discover_services().await?;

    let chars = device.characteristics();
    let mut subscribed = 0_usize;
    for desc in SENSORS {
        let uuid = Uuid::from_bytes(desc.uuid);
        let Some(ch) = chars.iter().find(|c| c.uuid == uuid) else {
            continue;
        };

        // Initial read for an immediate value (best-effort).
        if let Ok(data) = device.read(ch).await
            && let Some(index) = index_for_uuid(&desc.uuid)
            && let Some(raw) = decode_reading(&data)
        {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "best-effort seed value; a closed channel is handled later"
            )]
            let _ = tx.send(ClientEvent::Reading { index, raw }).await;
        }
        if ch.properties.contains(CharPropFlags::NOTIFY) {
            device.subscribe(ch).await?;
            subscribed = subscribed.saturating_add(1);
        }
    }
    if subscribed == 0 {
        return Err("no registered characteristics found on device".into());
    }
    tx.send(ClientEvent::Connected).await?;

    let mut events = device.notifications().await?;
    while let Some(n) = events.next().await {
        if let Some(index) = index_for_uuid(n.uuid.as_bytes())
            && let Some(raw) = decode_reading(&n.value)
            && tx.send(ClientEvent::Reading { index, raw }).await.is_err()
        {
            break; // UI gone — let `run` observe the closed channel.
        }
    }
    // Stream ended → device disconnected. Best-effort tidy disconnect.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "tidy disconnect; already disconnected if this errors"
    )]
    let _ = device.disconnect().await;
    Ok(())
}

/// Scan, polling `peripherals()` until a `MeteoStation` appears. Waits as long
/// as needed (the device may be powered off); the UI shows `Scanning`.
async fn wait_for_station(adapter: &Adapter) -> Result<Peripheral, Box<dyn Error>> {
    adapter.start_scan(ScanFilter::default()).await?;
    loop {
        for p in adapter.peripherals().await? {
            if let Some(props) = p.properties().await?
                && props
                    .local_name
                    .as_deref()
                    .is_some_and(|name| name.contains("MeteoStation"))
            {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "stop_scan failure is harmless; we have the peripheral"
                )]
                let _ = adapter.stop_scan().await;
                return Ok(p);
            }
        }
        time::sleep(SCAN_POLL).await;
    }
}

/// Decode an f32 from a 4-byte LE characteristic value (reuses `meteo-lib`).
fn decode_reading(data: &[u8]) -> Option<f32> {
    let bytes: &[u8; 4] = data.first_chunk()?;
    Some(decode_f32(bytes))
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use meteo_lib::ble::encoding::encode_f32;

    use super::*;

    #[test]
    fn decode_reading_round_trip() {
        // Given
        let bytes = encode_f32(23.45_f32);

        // When
        let result = decode_reading(&bytes);

        // Then
        let v = result.expect("decode_reading should return Some for a valid 4-byte value");
        assert!(
            (v - 23.45_f32).abs() < 1e-3,
            "decoded value should be approximately 23.45, got {v}"
        );
    }

    #[test]
    fn decode_reading_too_short_returns_none() {
        // Given
        let data = [0x01_u8, 0x02_u8];

        // When
        let result = decode_reading(&data);

        // Then
        assert!(
            result.is_none(),
            "decode_reading should return None for a too-short slice"
        );
    }
}
// grcov exclude stop
