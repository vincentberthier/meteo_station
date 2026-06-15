//! `BlueZ` central transport: scan → connect → subscribe → decode → reconnect.
//!
//! The public entry point is [`run`], which owns the full BLE connection
//! lifecycle.  Only truly fatal errors (no session, no adapter) propagate out
//! as `Err`; transient per-connection failures are logged and trigger a rescan.
//!
//! Every state transition and failure is reported through `tracing`, so the
//! headless `--no-tui` mode can show exactly what the link is doing over SSH.
//!
//! Emits on `tx`:
//! ```ignore
//! tx.send(ClientEvent::Connected).await?;
//! tx.send(ClientEvent::Reading { index, raw }).await?;  // index into sensors::SENSORS
//! tx.send(ClientEvent::Disconnected).await?;
//! ```
use std::error::Error;

use bluer::{
    AdapterEvent, Address, Device, DiscoveryFilter, DiscoveryTransport,
    gatt::remote::Characteristic,
};
use futures::StreamExt as _;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tracing::{debug, info, warn};
use uuid::Uuid;

use meteo_lib::ble::frame::Frame;
use meteo_lib::ble::{CHAR_UUID, DEVICE_NAME, SERVICE_UUID};

use crate::app::ClientEvent;
use crate::sensors::field_to_index;

/// Run until shutdown.  Fatal setup errors (no session, no adapter) propagate;
/// transient BLE errors are logged and cause a rescan via the inner helper.
pub async fn run(
    tx: Sender<ClientEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn Error>> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    info!(adapter = adapter.name(), "BLE adapter ready");

    let service_uuid = Uuid::from_u128(SERVICE_UUID);
    let char_uuid = Uuid::from_u128(CHAR_UUID);

    // Scope every discovery session: LE-only, and match by device-name prefix.
    // The firmware advertises only its name (`DEVICE_NAME`) — the 128-bit
    // service UUID is *not* in the advertising payload — so a UUID filter would
    // hide it. `bluer` applies the adapter's current filter when discovery
    // opens, so setting it once here keeps each scan from caching every
    // advertiser in radio range. `DuplicateData` defaults to false.
    adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            pattern: Some(DEVICE_NAME.to_owned()),
            ..DiscoveryFilter::default()
        })
        .await?;

    // Clear devices left in `org.bluez`'s cache by previous runs.  Discovery
    // re-reports every already-known device (including ones no longer in range:
    // see `bluer::Adapter::discover_devices`), so stale `MeteoStation` entries
    // would otherwise be handed back on every scan and read as "duplicates".
    let purged = purge_stale_devices(&adapter).await;
    if purged > 0 {
        info!(count = purged, "purged stale cached MeteoStation devices");
    }

    loop {
        if *shutdown.borrow() {
            break;
        }
        // Transient errors (connect failure, GATT resolution, notification
        // stream drop) go through the inner helper; an `Err` is logged and the
        // outer loop rescans.
        if let Err(err) =
            run_connection(&adapter, &tx, &mut shutdown, service_uuid, char_uuid).await
        {
            warn!(%err, "connection cycle failed; rescanning");
        }
    }
    info!("shutdown requested; feed exiting");
    Ok(())
}

/// Remove every cached device that advertises [`DEVICE_NAME`].  Returns the
/// number removed.  Errors on individual devices are logged and skipped — a
/// device that cannot be inspected or removed is not worth aborting startup.
async fn purge_stale_devices(adapter: &bluer::Adapter) -> usize {
    let addresses = match adapter.device_addresses().await {
        Ok(addrs) => addrs,
        Err(err) => {
            warn!(%err, "could not list cached devices for purge");
            return 0_usize;
        }
    };

    let mut removed = 0_usize;
    for addr in addresses {
        let Ok(dev) = adapter.device(addr) else {
            continue;
        };
        if dev.name().await.ok().flatten().as_deref() != Some(DEVICE_NAME) {
            continue;
        }
        match adapter.remove_device(addr).await {
            Ok(()) => {
                debug!(%addr, "removed stale cached device");
                removed = removed.saturating_add(1_usize);
            }
            Err(err) => warn!(%addr, %err, "failed to remove stale cached device"),
        }
    }
    removed
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
    // 1. SCAN — wait for a present device advertising our device name.
    let Some(device) = scan_for_device(adapter, shutdown).await? else {
        // Shutdown was signalled during scan.
        return Ok(());
    };
    let addr = device.address();

    // 2. CONNECT.
    if device.is_connected().await? {
        debug!(%addr, "already connected");
    } else {
        info!(%addr, "connecting");
        device.connect().await?;
        info!(%addr, "connected");
    }

    // 3. Find the target characteristic.  `Device::services` waits for GATT
    //    resolution internally, so a missing characteristic here means the
    //    peripheral genuinely does not expose it — most often a stale cache
    //    entry whose GATT predates provisioning.  Remove it so the next scan
    //    rediscovers a fresh copy instead of looping on the same dead device.
    let Some(ch) = find_characteristic(&device, service_uuid, char_uuid).await? else {
        warn!(%addr, "target characteristic not found; removing and rescanning");
        teardown(adapter, &device, true).await;
        return Ok(());
    };
    info!(%addr, "subscribed to measurement characteristic");

    // 4. SUBSCRIBE and pump notifications.
    // If the channel is closed the UI has exited; treat as clean shutdown.
    if tx.send(ClientEvent::Connected).await.is_err() {
        return Ok(());
    }

    let pump_result = pump_notifications(&ch, tx, shutdown).await;

    // 5. Tear down, then notify the UI we are reconnecting.  Read the flag into
    //    a local so the non-`Send` watch guard is not held across the await.
    let removing = *shutdown.borrow();
    info!(%addr, "link closed; disconnecting");
    teardown(adapter, &device, removing).await;

    // Ignore channel-closed here; the UI may have exited.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "UI may have exited; channel close is also a shutdown"
    )]
    let _ = tx.send(ClientEvent::Disconnected).await;

    pump_result
}

/// Best-effort teardown after a connection cycle: always disconnect; drop the
/// cached `Device1` object (`RemoveDevice`) only when `remove` is set — on a
/// clean shutdown, or when the device proved unusable (no characteristic).
/// During a normal mid-session reconnect the entry is kept: the firmware
/// re-advertises within ~1 s and the RSSI-gated scan reconnects without cache
/// churn.  Any error here is itself effectively a disconnect / already-removed,
/// so errors are ignored.
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

/// Scan adapter events until a device that is both named [`DEVICE_NAME`] and
/// currently present is found, or shutdown is signalled.  Returns `None` when
/// shutdown fires or the discovery stream ends.
///
/// Uses [`discover_devices_with_changes`] (not `discover_devices`) so a fresh
/// `DeviceAdded` is delivered every time a device property updates.  This
/// matters because `Name` and `RSSI` are resolved asynchronously and are often
/// absent at the first `DeviceAdded`; re-checking on each change closes the
/// race where the live station is seen once, before its name resolves, and then
/// never revisited.
///
/// A present device is one `BlueZ` reports with an `RSSI`; `RSSI` is `None` for a
/// device that is known but not currently in range, which excludes stale cache
/// entries.  The service/characteristic are verified over GATT after connecting
/// (see [`find_characteristic`]).
///
/// [`discover_devices_with_changes`]: bluer::Adapter::discover_devices_with_changes
async fn scan_for_device(
    adapter: &bluer::Adapter,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<Device>, bluer::Error> {
    info!("scanning for MeteoStation");
    let mut events = adapter.discover_devices_with_changes().await?;
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                return Ok(None);
            }
            ev = events.next() => {
                match ev {
                    Some(AdapterEvent::DeviceAdded(addr)) => {
                        if let Some(dev) = inspect_candidate(adapter, addr).await? {
                            return Ok(Some(dev));
                        }
                    }
                    Some(AdapterEvent::DeviceRemoved(addr)) => {
                        debug!(%addr, "device removed during scan");
                    }
                    Some(AdapterEvent::PropertyChanged(_)) => {}
                    None => {
                        // Discovery stream ended unexpectedly; rescan.
                        warn!("discovery stream ended; rescanning");
                        return Ok(None);
                    }
                }
            }
        }
    }
}

/// Evaluate one discovered address: accept it only when it both advertises
/// [`DEVICE_NAME`] and is currently present (has an `RSSI`).  Returns the
/// matching [`Device`], or `None` to keep scanning.
async fn inspect_candidate(
    adapter: &bluer::Adapter,
    addr: Address,
) -> Result<Option<Device>, bluer::Error> {
    let dev = adapter.device(addr)?;
    let name = dev.name().await.ok().flatten();
    let rssi = dev.rssi().await.ok().flatten();
    debug!(%addr, ?name, ?rssi, "discovery candidate");

    if name.as_deref() != Some(DEVICE_NAME) {
        return Ok(None);
    }
    if rssi.is_none() {
        debug!(%addr, "MeteoStation name matches but device is not present (no RSSI); waiting");
        return Ok(None);
    }
    info!(%addr, ?rssi, "found MeteoStation");
    Ok(Some(dev))
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
                let Some(bytes) = item else {
                    // Notification stream ended — device disconnected.
                    debug!("notification stream ended");
                    return Ok(());
                };
                // `false` = the UI channel closed; unwind cleanly.
                if !forward_frame(&bytes, tx).await {
                    return Ok(());
                }
            }
        }
    }
}

/// Decode one notification payload and forward its present fields to the UI.
/// Returns `false` when the UI channel has closed (caller should stop pumping),
/// `true` otherwise.
async fn forward_frame(bytes: &[u8], tx: &Sender<ClientEvent>) -> bool {
    let Ok(frame) = Frame::decode(bytes) else {
        // Truncated or unknown schema version — skip frame.
        debug!(len = bytes.len(), "dropping undecodable frame");
        return true;
    };
    for (field, value) in frame.present_fields() {
        let Some(idx) = field_to_index(field) else {
            continue;
        };
        if tx
            .send(ClientEvent::Reading {
                index: idx,
                raw: value,
            })
            .await
            .is_err()
        {
            // Channel closed = UI exited.
            return false;
        }
    }
    true
}
