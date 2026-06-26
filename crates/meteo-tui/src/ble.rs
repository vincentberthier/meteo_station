//! BLE central: passive-scan advertisement receiver.
//!
//! Runs as a spawned tokio task; emits [`BleEvent`] to the app over an `mpsc`
//! channel. The firmware broadcasts telemetry frames as manufacturer-specific
//! advertisement data (company ID [`COMPANY_ID`]). No GATT connection is made.

use std::collections::HashMap;

use futures::StreamExt as _;
use meteo_lib::{FRAME_LEN, Telemetry};
use tokio::sync::mpsc;

// ── Passive-scan helpers ──────────────────────────────────────────────────────

/// Bluetooth Company Identifier used by the firmware in manufacturer-specific
/// advertising data (`0xFFFF` = reserved for testing / internal use).
pub const COMPANY_ID: u16 = 0xFFFF;

/// Decode a telemetry frame from a BLE advertisement's manufacturer-data map.
///
/// Returns `Some(Telemetry)` when `mfg` contains an entry for [`COMPANY_ID`]
/// whose payload is exactly [`FRAME_LEN`] bytes and passes `Telemetry::decode`.
/// Returns `None` on any mismatch (wrong company, wrong length, decode error).
pub fn decode_frame(mfg: &HashMap<u16, Vec<u8>>) -> Option<Telemetry> {
    let payload = mfg.get(&COMPANY_ID)?;
    if payload.len() != FRAME_LEN {
        return None;
    }
    Telemetry::decode(payload).ok()
}

// ── Public surface ────────────────────────────────────────────────────────────

/// Data carried by a [`BleEvent::Frame`] event.
#[derive(Debug, Clone)]
pub struct FrameEvent {
    /// Decoded telemetry frame.
    pub telemetry: Telemetry,
    /// RSSI of the advertisement, if the adapter reported it.
    #[allow(dead_code, reason = "consumed by the app reducer in the next substep")]
    pub rssi: Option<i16>,
    /// Alias (advertised name) of the station, if available.
    #[allow(dead_code, reason = "consumed by the app reducer in the next substep")]
    pub station: Option<String>,
}

impl FrameEvent {
    /// Construct a frame event with `rssi` and `station` defaulting to `None`.
    ///
    /// Useful in tests and wherever only the decoded telemetry is available.
    #[must_use]
    #[allow(
        dead_code,
        reason = "called from test helpers; unused in the binary until next substep"
    )]
    pub const fn new(telemetry: Telemetry) -> Self {
        Self {
            telemetry,
            rssi: None,
            station: None,
        }
    }
}

/// Events pushed to the app loop by the BLE task.
#[derive(Debug, Clone)]
pub enum BleEvent {
    /// A well-formed telemetry frame arrived via advertisement.
    Frame(FrameEvent),
}

/// Backoff between discovery (re)establishment attempts. Short enough that the
/// dashboard recovers within ~1 s of the adapter coming back, long enough to
/// avoid a busy spin while it is mid-reset.
const RESCAN_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// Spawned task: runs the passive-scan loop forever, emitting [`BleEvent::Frame`]s.
///
/// Resilient to `BlueZ` adapter resets: a single discovery session ends (the
/// `discover_devices_with_changes` stream returns `None`) or fails to start
/// whenever the adapter is removed, powered off, or restarted (e.g. a
/// `bluetoothctl power off/on`). This outer loop re-acquires the adapter and
/// restarts discovery each time, so the dashboard reconnects on its own instead
/// of going permanently `Stale`. Only an unrecoverable session failure (no
/// `D-Bus` / `bluetoothd`) propagates as `Err`.
pub async fn run(tx: mpsc::Sender<BleEvent>, addr: bluer::Address) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;
    loop {
        // Both a clean stream end (adapter went away) and a setup error mean the
        // current discovery session is gone; either way, retry. The dashboard's
        // own `SignalState` surfaces the gap (Live -> Stale) and the recovery
        // (Stale -> Live) once frames resume, so no separate logging is needed
        // (the TUI installs no runtime logger and owns the terminal).
        drop(scan_session(&session, &tx, addr).await);
        // Re-check by retrying the real operation each iteration; the backoff
        // only prevents a tight spin while the adapter is unavailable.
        tokio::time::sleep(RESCAN_BACKOFF).await;
    }
}

/// Runs one discovery session: powers the adapter, sets the LE-only filter, and
/// pumps advertisement events until the stream ends. Returns `Ok(())` on a clean
/// stream end (adapter went away) and `Err` if any setup step fails — the caller
/// retries either way.
///
/// `duplicate_data` makes `BlueZ` emit `PropertiesChanged` for `ManufacturerData`
/// on every received advertisement PDU; `discover_devices_with_changes` re-emits
/// `DeviceAdded` whenever a discovered device's properties change.
async fn scan_session(
    session: &bluer::Session,
    tx: &mpsc::Sender<BleEvent>,
    addr: bluer::Address,
) -> anyhow::Result<()> {
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    adapter
        .set_discovery_filter(bluer::DiscoveryFilter {
            transport: bluer::DiscoveryTransport::Le,
            duplicate_data: true,
            ..bluer::DiscoveryFilter::default()
        })
        .await?;

    let mut events = adapter.discover_devices_with_changes().await?;
    while let Some(ev) = events.next().await {
        if let bluer::AdapterEvent::DeviceAdded(a) = ev {
            if a != addr {
                continue;
            }
            let Ok(device) = adapter.device(a) else {
                continue;
            };
            if let Ok(Some(mfg)) = device.manufacturer_data().await {
                emit_frame(tx, &device, &mfg).await;
            }
        }
    }

    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Try to decode a telemetry frame from `mfg` and send it on `tx`.
///
/// Reads RSSI and alias from `device` alongside the frame; transient D-Bus
/// read failures degrade to `None` and never drop the frame.
async fn emit_frame(
    tx: &mpsc::Sender<BleEvent>,
    device: &bluer::Device,
    mfg: &HashMap<u16, Vec<u8>>,
) {
    if let Some(telemetry) = decode_frame(mfg) {
        let rssi = device.rssi().await.ok().flatten();
        let station = device.alias().await.ok().filter(|s| !s.is_empty());
        // Intentionally discard send error: the app may have shut down.
        tx.send(BleEvent::Frame(FrameEvent {
            telemetry,
            rssi,
            station,
        }))
        .await
        .ok();
    }
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

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn decode_frame_accepts_valid_company_payload() -> TestResult {
        // Given — a valid v5 frame encoded from a Telemetry with uptime_s = 7
        let telem = Telemetry {
            uptime_s: 7,
            ..Telemetry::empty()
        };
        let encoded = telem.encode();
        let mut mfg = HashMap::new();
        mfg.insert(COMPANY_ID, encoded.to_vec());

        // When
        let result = decode_frame(&mfg);

        // Then
        let decoded = result.ok_or("expected Some(Telemetry), got None")?;
        assert_eq!(decoded.uptime_s, 7);
        Ok(())
    }

    #[test]
    fn decode_frame_rejects_wrong_company() -> TestResult {
        // Given — payload under a different company ID
        let telem = Telemetry {
            uptime_s: 7,
            ..Telemetry::empty()
        };
        let encoded = telem.encode();
        let mut mfg = HashMap::new();
        mfg.insert(0x0059_u16, encoded.to_vec()); // Nordic Semiconductor, not 0xFFFF

        // When
        let result = decode_frame(&mfg);

        // Then
        assert!(result.is_none(), "expected None for wrong company ID");
        Ok(())
    }

    #[test]
    fn decode_frame_rejects_wrong_length() -> TestResult {
        // Given — correct company ID but payload is too short
        let mut mfg = HashMap::new();
        mfg.insert(COMPANY_ID, vec![0_u8; 10]);

        // When
        let result = decode_frame(&mfg);

        // Then
        assert!(result.is_none(), "expected None for wrong-length payload");
        Ok(())
    }

    #[test]
    fn frame_event_new_defaults_none() -> TestResult {
        // Given — a minimal Telemetry fixture with a distinct uptime_s
        let t = Telemetry {
            uptime_s: 42,
            ..Telemetry::empty()
        };

        // When
        let fe = FrameEvent::new(t);

        // Then
        assert!(fe.rssi.is_none(), "rssi should default to None");
        assert!(fe.station.is_none(), "station should default to None");
        assert_eq!(fe.telemetry.uptime_s, 42, "telemetry must be preserved");
        Ok(())
    }
}
// grcov exclude stop
