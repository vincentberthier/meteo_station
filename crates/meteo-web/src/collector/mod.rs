//! BLE passive-scan collector task.
//!
//! Re-implements the proven scan structure from `meteo-tui/src/ble.rs` for the
//! web backend, with added bucketing and SQLite persistence.
//!
//! ## Architecture
//!
//! * [`run`] opens a `bluer` session and loops forever.
//! * Each scan session (inner [`scan_session`]) sets the LE filter,
//!   starts `discover_devices_with_changes`, and pumps advertisement events
//!   through a `tokio::select!` that also fires a 1 Hz clock tick.
//! * Each decoded [`Telemetry`] frame is
//!   (a) forwarded to `live_tx` for the live dashboard band, and
//!   (b) folded into the current [`BucketAccumulator`].
//! * When the minute changes ([`should_flush`] returns `true`), the finished
//!   bucket is written to SQLite via [`crate::db::DbHandle::store_bucket`].
//! * On adapter loss the outer loop waits for [`bluer::SessionEvent::AdapterAdded`]
//!   before retrying — **no `sleep` is used** (see the in-code NOTE comment).

pub mod bucket;

use std::collections::HashMap;

use futures::StreamExt as _;
use meteo_lib::{FRAME_LEN, Telemetry};
use tokio::sync::watch;

use self::bucket::{BucketAccumulator, floor_to_minute};
use crate::db::DbHandle;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Bluetooth Manufacturer-Specific Data company identifier used by the firmware.
///
/// `0xFFFF` = reserved for internal / testing use.
const COMPANY_ID: u16 = 0xFFFF;

/// Default BLE address of the weather station as configured in firmware.
pub const STATION_ADDR: bluer::Address = bluer::Address::new([0xF0, 0xCA, 0xFE, 0x00, 0x00, 0x01]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Flush decision: returns `true` when the frame's minute differs from the
/// currently open bucket minute.
///
/// This is a pure, observed condition: the bucket is only sealed when a real
/// clock boundary is detected, never on a timer alone.
pub(crate) const fn should_flush(open_minute: i64, frame_minute: i64) -> bool {
    frame_minute != open_minute
}

/// Collect BLE telemetry from `addr`, persist minute buckets to `db`, and
/// broadcast each decoded frame to `live_tx`.
///
/// The function loops forever; it is resilient to adapter resets. It returns
/// `Err` only when the D-Bus session itself is unrecoverable (i.e. `bluetoothd`
/// is gone). The caller should treat any `Err` return as a fatal startup
/// failure.
///
/// # Errors
///
/// Returns an error if:
/// * The initial `bluer` session cannot be created.
/// * The adapter-event stream (used to wait for adapter recovery) fails,
///   indicating that the D-Bus connection to `bluetoothd` is broken.
pub async fn run(
    db: DbHandle,
    live_tx: watch::Sender<Option<Telemetry>>,
    addr: bluer::Address,
) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;

    loop {
        // Run one scan session; adapter loss or any scan error is recoverable.
        scan_session(&session, &db, &live_tx, addr).await.ok();

        // NOTE: Deliberate divergence from meteo-tui's sleep-based backoff.
        //
        // Project rule (CLAUDE.md §"No timeouts or sleeps in code"): fixed
        // delays are forbidden as synchronisation primitives. Here we await the
        // REAL signal — `bluer::SessionEvent::AdapterAdded` — rather than
        // sleeping an arbitrary duration. The retry of the real adapter
        // operation (`default_adapter` + `is_powered`) is the observed-
        // readiness check; the wait ends the instant the adapter is back, not
        // when the clock says so.
        loop {
            match session.default_adapter().await {
                Ok(a) if a.is_powered().await.unwrap_or(false) => break,
                _ => {
                    // Subscribe to adapter-lifecycle events and wait for the
                    // AdapterAdded signal. A stream-end (None) without an
                    // AdapterAdded just sends us back to re-check the adapter
                    // state above. A failure from `events()` means D-Bus is
                    // gone — propagate it as unrecoverable.
                    let mut evts = Box::pin(session.events().await?);
                    while let Some(ev) = evts.next().await {
                        if matches!(ev, bluer::SessionEvent::AdapterAdded(_)) {
                            break;
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Decode a telemetry frame from a BLE advertisement's manufacturer-data map.
///
/// Returns `Some(Telemetry)` when `mfg` contains an entry for [`COMPANY_ID`]
/// whose payload is exactly [`FRAME_LEN`] bytes and passes `Telemetry::decode`.
fn decode_frame(mfg: &HashMap<u16, Vec<u8>>) -> Option<Telemetry> {
    let payload = mfg.get(&COMPANY_ID)?;
    if payload.len() != FRAME_LEN {
        return None;
    }
    Telemetry::decode(payload).ok()
}

/// Flush one closed bucket to SQLite if it contains any data.
///
/// `std::mem::take` resets `acc` to its `Default` state (empty), then we
/// call `finish` on the old accumulator and store it. Errors from the
/// database write are silently discarded: a missed bucket is not fatal.
async fn maybe_store(db: &DbHandle, acc: &mut BucketAccumulator, bucket_ts: i64) {
    if !acc.is_empty() {
        let old = std::mem::take(acc);
        db.store_bucket(old.finish(bucket_ts)).await.ok();
    }
}

/// Run one scan session: power the adapter, set the LE filter, and pump
/// advertisement events until the stream ends or the adapter disappears.
///
/// Returns `Ok(())` on a clean stream end (adapter went away) and `Err` on
/// any setup failure — the caller retries either way.
async fn scan_session(
    session: &bluer::Session,
    db: &DbHandle,
    live_tx: &watch::Sender<Option<Telemetry>>,
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

    let mut accumulator = BucketAccumulator::default();
    let mut open_minute = floor_to_minute(chrono::Utc::now().timestamp());

    // 1 Hz interval used ONLY as a clock sample to evaluate should_flush
    // when the station is silent (no advertisements arrive). It never gates
    // work: the real signal is the minute-boundary transition detected by
    // should_flush().
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_ev = events.next() => {
                let Some(ev) = maybe_ev else { break; };

                if let bluer::AdapterEvent::DeviceAdded(a) = ev
                    && a == addr
                    && let Ok(device) = adapter.device(a)
                    && let Ok(Some(mfg)) = device.manufacturer_data().await
                    && let Some(t) = decode_frame(&mfg)
                {
                    // (a) Forward to live band; ignore send error if no
                    //     receivers are subscribed.
                    live_tx.send(Some(t)).ok();
                    // (b) Fold into current minute bucket.
                    accumulator.add(&t);
                }

                // Re-evaluate flush on every event (the clock may have
                // crossed a minute boundary while we were processing).
                let now_minute = floor_to_minute(chrono::Utc::now().timestamp());
                if should_flush(open_minute, now_minute) {
                    maybe_store(db, &mut accumulator, open_minute).await;
                    open_minute = now_minute;
                }
            }
            _ = tick.tick() => {
                // Clock sample only: re-evaluate flush when no frame arrived.
                let now_minute = floor_to_minute(chrono::Utc::now().timestamp());
                if should_flush(open_minute, now_minute) {
                    maybe_store(db, &mut accumulator, open_minute).await;
                    open_minute = now_minute;
                }
            }
        }
    }

    // Flush any partial bucket collected in this session before exiting.
    maybe_store(db, &mut accumulator, open_minute).await;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    use super::should_flush;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn should_flush_only_on_minute_change() -> TestResult {
        // Given / When / Then — same minute: no flush
        assert!(
            !should_flush(120, 120),
            "should_flush(120, 120) must be false"
        );
        // Different minute: flush
        assert!(
            should_flush(120, 180),
            "should_flush(120, 180) must be true"
        );
        // Edge: minute advanced by exactly 60 s
        assert!(should_flush(0, 60), "should_flush(0, 60) must be true");

        Ok(())
    }
}
// grcov exclude end
