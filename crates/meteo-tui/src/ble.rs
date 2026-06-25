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

/// Events pushed to the app loop by the BLE task.
#[derive(Debug, Clone, Copy)]
pub enum BleEvent {
    /// A well-formed telemetry frame arrived via advertisement.
    Frame(Telemetry),
}

/// Spawned task: runs the passive-scan loop forever, emitting [`BleEvent::Frame`]s.
///
/// Powers the adapter, sets an LE-only discovery filter with `duplicate_data`
/// enabled (so every advertisement triggers a `PropertiesChanged` signal), then
/// watches manufacturer-data property changes from the station device at `addr`.
pub async fn run(tx: mpsc::Sender<BleEvent>, addr: bluer::Address) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    // LE-only passive scan; `duplicate_data` makes BlueZ emit PropertiesChanged
    // for ManufacturerData on every received advertisement PDU.
    adapter
        .set_discovery_filter(bluer::DiscoveryFilter {
            transport: bluer::DiscoveryTransport::Le,
            duplicate_data: true,
            ..bluer::DiscoveryFilter::default()
        })
        .await?;

    // `discover_devices_with_changes` re-emits DeviceAdded whenever any
    // property of a discovered device changes — including ManufacturerData.
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
                emit_frame(&tx, &mfg).await;
            }
        }
    }

    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Try to decode a telemetry frame from `mfg` and send it on `tx`.
async fn emit_frame(tx: &mpsc::Sender<BleEvent>, mfg: &HashMap<u16, Vec<u8>>) {
    if let Some(t) = decode_frame(mfg) {
        // Intentionally discard send error: the app may have shut down.
        // Intentionally discard send error: the app may have shut down.
        tx.send(BleEvent::Frame(t)).await.ok();
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
}
// grcov exclude stop
