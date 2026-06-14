//! `BlueZ` central transport: scan → connect → subscribe → decode → reconnect.
//!
//! The public entry point is [`run`], which owns the full BLE connection
//! lifecycle.  Only truly fatal errors (no session, no adapter) propagate out
//! as `Err`; transient per-connection failures log and rescan.
//!
//! Emits on `tx`:
//! ```ignore
//! tx.send(ClientEvent::Connected).await?;
//! tx.send(ClientEvent::Reading { index, raw }).await?;  // index into sensors::SENSORS
//! tx.send(ClientEvent::Disconnected).await?;
//! ```
use std::error::Error;

use bluer::{
    AdapterEvent, Device, DiscoveryFilter, DiscoveryTransport, gatt::remote::Characteristic,
};
use futures::StreamExt as _;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use uuid::Uuid;

use meteo_lib::ble::frame::Frame;
use meteo_lib::ble::{CHAR_UUID, DEVICE_NAME, SERVICE_UUID};

use crate::app::ClientEvent;
use crate::sensors::field_to_index;

/// Run until shutdown.  Fatal setup errors (no session, no adapter) propagate;
/// transient BLE errors cause a rescan via the inner helper.
pub async fn run(
    tx: Sender<ClientEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn Error>> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let service_uuid = Uuid::from_u128(SERVICE_UUID);
    let char_uuid = Uuid::from_u128(CHAR_UUID);

    // Scope every discovery session: LE-only, and match by device-name prefix.
    // The firmware advertises only its name (`DEVICE_NAME`) — the 128-bit
    // service UUID is *not* in the advertising payload — so a UUID filter would
    // hide it. `bluer` applies the adapter's current filter when
    // `discover_devices` opens the session, so setting it once here keeps each
    // scan from caching every advertiser in radio range (the cause of the
    // runaway `org.bluez` device cache). `DuplicateData` defaults to false.
    adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            pattern: Some(DEVICE_NAME.to_owned()),
            ..DiscoveryFilter::default()
        })
        .await?;

    loop {
        if *shutdown.borrow() {
            break;
        }
        // Transient errors (connect failure, GATT resolution, notification
        // stream drop) go through the inner helper; `Err` just rescans.
        let result = run_connection(&adapter, &tx, &mut shutdown, service_uuid, char_uuid).await;
        if let Err(_err) = result {
            // Swallow transient error; outer loop rescans.
        }
    }
    Ok(())
}

/// Drive one full connection cycle: scan → connect → subscribe → pump.
///
/// Returns `Ok(())` when a clean disconnect or shutdown occurs.
/// Returns `Err` on any transient `BlueZ` error so the caller rescans.
async fn run_connection(
    adapter: &bluer::Adapter,
    tx: &Sender<ClientEvent>,
    shutdown: &mut watch::Receiver<bool>,
    service_uuid: Uuid,
    char_uuid: Uuid,
) -> Result<(), bluer::Error> {
    // 1. SCAN — wait for the first device advertising our device name.
    let Some(device) = scan_for_device(adapter, shutdown).await? else {
        // Shutdown was signalled during scan.
        return Ok(());
    };

    // 2. CONNECT.
    if !device.is_connected().await? {
        device.connect().await?;
    }

    // 3. Find the target characteristic.
    let Some(ch) = find_characteristic(&device, service_uuid, char_uuid).await? else {
        // Not our characteristic (wrong/incompatible peripheral); disconnect
        // and rescan.  No `RemoveDevice` here — that would churn the cache (and
        // the viewer UI) on every cycle.
        // Read the flag into a local so the non-`Send` watch guard is not held
        // across the await below.
        let removing = *shutdown.borrow();
        teardown(adapter, &device, removing).await;
        return Ok(());
    };

    // 4. SUBSCRIBE and pump notifications.
    // If the channel is closed the UI has exited; treat as clean shutdown.
    if tx.send(ClientEvent::Connected).await.is_err() {
        return Ok(());
    }

    let pump_result = pump_notifications(&ch, tx, shutdown).await;

    // 5. Tear down, then notify the UI we are reconnecting.  Read the flag into
    //    a local so the non-`Send` watch guard is not held across the await.
    let removing = *shutdown.borrow();
    teardown(adapter, &device, removing).await;

    // Ignore channel-closed here; the UI may have exited.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "UI may have exited; channel close is also a shutdown"
    )]
    let _ = tx.send(ClientEvent::Disconnected).await;

    pump_result
}

/// Best-effort teardown after a connection cycle: always disconnect; only drop
/// the cached `Device1` object (`RemoveDevice`) when `remove` is set — i.e. on a
/// clean shutdown.  Removing on every transient reconnect would churn the
/// adapter's device cache and make it flicker in `BlueZ` viewers; the discovery
/// filter already keeps the cache scoped to our device.  Any error here is
/// itself effectively a disconnect / already-removed, so errors are ignored.
#[expect(
    clippy::let_underscore_must_use,
    reason = "best-effort teardown; any error is itself a disconnect/removal"
)]
async fn teardown(adapter: &bluer::Adapter, device: &Device, remove: bool) {
    let _ = device.disconnect().await;
    if remove {
        let _ = adapter.remove_device(device.address()).await;
    }
}

/// Scan adapter events until a device whose name is [`DEVICE_NAME`] is found or
/// shutdown is signalled.  Returns `None` when shutdown fires.
///
/// Matching is by name because the firmware does not advertise its service UUID;
/// the service/characteristic are verified over GATT after connecting (see
/// [`find_characteristic`]).
async fn scan_for_device(
    adapter: &bluer::Adapter,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<Device>, bluer::Error> {
    let mut events = adapter.discover_devices().await?;
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                return Ok(None);
            }
            ev = events.next() => {
                match ev {
                    Some(AdapterEvent::DeviceAdded(addr)) => {
                        let dev = adapter.device(addr)?;
                        if dev.name().await?.as_deref() == Some(DEVICE_NAME) {
                            return Ok(Some(dev));
                        }
                    }
                    Some(_) => {}
                    None => {
                        // Discovery stream ended unexpectedly; rescan.
                        return Ok(None);
                    }
                }
            }
        }
    }
}

/// Walk services/characteristics to find the target characteristic.
async fn find_characteristic(
    device: &Device,
    service_uuid: Uuid,
    char_uuid: Uuid,
) -> Result<Option<Characteristic>, bluer::Error> {
    for svc in device.services().await? {
        if svc.uuid().await? != service_uuid {
            continue;
        }
        for ch in svc.characteristics().await? {
            if ch.uuid().await? == char_uuid {
                return Ok(Some(ch));
            }
        }
    }
    Ok(None)
}

/// Subscribe and forward decoded frames until disconnect or shutdown.
async fn pump_notifications(
    ch: &Characteristic,
    tx: &Sender<ClientEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), bluer::Error> {
    let notify = ch.notify().await?;
    // The notify stream returned by bluer is not Unpin, so pin it on the stack.
    tokio::pin!(notify);
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                return Ok(());
            }
            item = notify.next() => {
                match item {
                    Some(bytes) => {
                        if let Ok(frame) = Frame::decode(&bytes) {
                            for (field, value) in frame.present_fields() {
                                if let Some(idx) = field_to_index(field) {
                                    // Channel closed = UI exited; treat as shutdown.
                                    if tx
                                        .send(ClientEvent::Reading {
                                            index: idx,
                                            raw: value,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        // Err(_): truncated or unknown schema version — skip frame.
                    }
                    None => {
                        // Notification stream ended — device disconnected.
                        return Ok(());
                    }
                }
            }
        }
    }
}
