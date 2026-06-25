//! BLE central: `bluer`-backed connection state machine.
//!
//! Runs as a spawned tokio task; emits [`BleEvent`] to the app over an `mpsc`
//! channel.  The authoritative disconnect signal is the `BlueZ` `Connected`
//! property going false or the notify-IO reader reaching EOF — frame silence
//! is **never** used to infer a disconnect.

// All public items are consumed by the app wiring added in substep 7.
#![allow(dead_code, reason = "consumed by main.rs wiring in substep 7")]

use std::collections::HashMap;

use futures::StreamExt as _;
use meteo_lib::{FRAME_LEN, Telemetry};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

use crate::model::{ConnState, LinkEvent, parse_fw_revision};

// ── Deadline constants (circuit-breakers only — each is paired with an
//    explicit failure path; they are never the primary sync mechanism) ────────
const SCAN_DEADLINE: Duration = Duration::from_secs(30);
const CONNECT_DEADLINE: Duration = Duration::from_secs(30);
const RESOLVE_DEADLINE: Duration = Duration::from_secs(15);

// ── UUIDs ────────────────────────────────────────────────────────────────────

/// Telemetry notify characteristic (128-bit).
const TELEMETRY_UUID: uuid::Uuid = uuid::uuid!("7e700002-b1df-42a1-bb5f-6a1028c793b0");

/// Expand a 16-bit Bluetooth UUID against the Bluetooth base UUID.
const fn uuid16(x: u16) -> uuid::Uuid {
    uuid::Uuid::from_fields(
        x as u32,
        0x0000_u16,
        0x1000_u16,
        &[0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
    )
}

/// Device Information Service UUID (0x180A).
const fn dis_service_uuid() -> uuid::Uuid {
    uuid16(0x180A)
}

/// Firmware Revision String characteristic UUID (0x2A26).
const fn fw_rev_uuid() -> uuid::Uuid {
    uuid16(0x2A26)
}

// ── Passive-scan helpers (substep 6) ─────────────────────────────────────────

/// Bluetooth Company Identifier used by the firmware in manufacturer-specific
/// advertising data (`0xFFFF` = reserved for testing / internal use).
///
/// TODO(substep 7): remove once app/ui/main migrate to SignalState/passive-scan
const COMPANY_ID: u16 = 0xFFFF;

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
#[derive(Debug, Clone)]
pub enum BleEvent {
    /// Connection state changed.
    State(ConnState),
    /// A well-formed telemetry frame arrived.
    Frame(Telemetry),
    /// DIS Firmware Revision String, read once on connection.
    Firmware(Option<String>),
}

/// Spawned task: runs the connect/reconnect loop forever, emitting [`BleEvent`]s.
///
/// The `addr` is the `BlueZ` display-order address of the station peripheral
/// (e.g. `F0:CA:FE:00:00:01`).
pub async fn run(tx: mpsc::Sender<BleEvent>, addr: bluer::Address) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    // Start in Reconnecting so the first iteration emits ScanStarted → Scanning.
    let mut state = ConnState::Reconnecting;
    loop {
        state = emit(&tx, state, LinkEvent::ScanStarted).await;

        let Some(device) = scan_for(&adapter, addr, SCAN_DEADLINE).await else {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await;
            continue;
        };
        state = emit(&tx, state, LinkEvent::DeviceFound).await;

        if !matches!(
            timeout(CONNECT_DEADLINE, device.connect()).await,
            Ok(Ok(()))
        ) {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await;
            continue;
        }
        state = emit(&tx, state, LinkEvent::Connected).await;

        if wait_services_resolved(&device, RESOLVE_DEADLINE)
            .await
            .is_err()
        {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await;
            continue;
        }

        let fw = read_fw_version(&device).await;
        // Intentionally discard send error: the app may have shut down.
        drop(tx.send(BleEvent::Firmware(fw)).await);

        let Some(telem_char) = find_char(&device, TELEMETRY_UUID).await else {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await;
            continue;
        };

        let Ok(reader) = telem_char.notify_io().await else {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await;
            continue;
        };
        state = emit(&tx, state, LinkEvent::Subscribed).await; // → Live

        pump_until_disconnect(&device, reader, &tx).await;
        state = emit(&tx, state, LinkEvent::LinkLost).await; // → Reconnecting
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Advance `state` by `ev`, send the new state over `tx`, and return it.
async fn emit(tx: &mpsc::Sender<BleEvent>, state: ConnState, ev: LinkEvent) -> ConnState {
    let next = state.next(ev);
    // Intentionally discard send error: the app may have shut down.
    drop(tx.send(BleEvent::State(next)).await);
    next
}

/// Pump telemetry frames until the link drops.
///
/// Two authoritative disconnect signals are monitored in parallel:
/// - The notify-IO reader returning `Ok(0)` or an I/O error (remote EOF / link
///   dropped at the socket level).
/// - The `BlueZ` `Connected` property going `false`.
///
/// Frame silence is **not** a disconnect signal.
///
/// A note on PDU coalescing: `notify_io` delivers each notification as a
/// datagram; at 17-byte frames that fits comfortably in a single read.
/// `Telemetry::decode` rejects wrong-length slices, so a coalesced or
/// truncated read is silently discarded and pumping continues.
async fn pump_until_disconnect(
    device: &bluer::Device,
    mut reader: bluer::gatt::CharacteristicReader,
    tx: &mpsc::Sender<BleEvent>,
) {
    use tokio::io::AsyncReadExt as _;

    let Ok(mut dev_events) = device.events().await else {
        return;
    };
    let mut buf = [0_u8; FRAME_LEN];
    loop {
        tokio::select! {
            r = reader.read(&mut buf) => match r {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(t) = Telemetry::decode(&buf[..n]) {
                        // Intentionally discard send error: app may have shut down.
                        drop(tx.send(BleEvent::Frame(t)).await);
                    }
                    // Malformed / short frame → ignore, keep pumping.
                }
            },
            ev = dev_events.next() => match ev {
                Some(bluer::DeviceEvent::PropertyChanged(
                    bluer::DeviceProperty::Connected(false),
                )) | None => break,
                _ => {}
            },
        }
    }
}

/// Scan for `addr`; returns the [`bluer::Device`] if found within `deadline`.
///
/// First checks `BlueZ`'s existing device cache; only starts active discovery if
/// the address is not already known — avoiding unnecessary scanning when the
/// peripheral is already in the cache.
async fn scan_for(
    adapter: &bluer::Adapter,
    addr: bluer::Address,
    deadline: Duration,
) -> Option<bluer::Device> {
    // Fast path: device already in cache.
    if adapter.device_addresses().await.ok()?.contains(&addr) {
        return adapter.device(addr).ok();
    }

    // Slow path: bounded active discovery.
    let scan = async {
        let mut events = adapter.discover_devices().await.ok()?;
        while let Some(ev) = events.next().await {
            if let bluer::AdapterEvent::DeviceAdded(a) = ev
                && a == addr
            {
                return adapter.device(addr).ok();
            }
        }
        None
    };
    timeout(deadline, scan).await.ok().flatten()
}

/// Wait until `BlueZ` reports `ServicesResolved = true` for `device`.
///
/// Returns `Ok(())` immediately if already resolved; otherwise subscribes to
/// device events and waits.  The `deadline` is a circuit-breaker in case the
/// event stream stalls.
async fn wait_services_resolved(device: &bluer::Device, deadline: Duration) -> anyhow::Result<()> {
    if device.is_services_resolved().await? {
        return Ok(());
    }
    let wait = async {
        let mut events = device.events().await?;
        while let Some(ev) = events.next().await {
            if matches!(
                ev,
                bluer::DeviceEvent::PropertyChanged(bluer::DeviceProperty::ServicesResolved(true))
            ) {
                return anyhow::Ok(());
            }
        }
        anyhow::bail!("device event stream ended before services resolved")
    };
    timeout(deadline, wait).await?
}

/// Walk all services and characteristics, returning the first characteristic
/// whose UUID matches `uuid`.
async fn find_char(
    device: &bluer::Device,
    uuid: uuid::Uuid,
) -> Option<bluer::gatt::remote::Characteristic> {
    for svc in device.services().await.ok()? {
        for ch in svc.characteristics().await.ok()? {
            if ch.uuid().await.ok()? == uuid {
                return Some(ch);
            }
        }
    }
    None
}

/// Read the DIS Firmware Revision String from the connected device.
///
/// Returns `None` if the DIS service or the characteristic is absent, or if
/// the read fails.
async fn read_fw_version(device: &bluer::Device) -> Option<String> {
    for svc in device.services().await.ok()? {
        if svc.uuid().await.ok()? != dis_service_uuid() {
            continue;
        }
        for ch in svc.characteristics().await.ok()? {
            if ch.uuid().await.ok()? == fw_rev_uuid() {
                return parse_fw_revision(&ch.read().await.ok()?);
            }
        }
    }
    None
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
