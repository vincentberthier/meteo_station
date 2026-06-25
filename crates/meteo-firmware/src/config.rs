#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

//! Flash-backed configuration task.
//!
//! At boot this task reads any persisted coarse location from the NVS flash
//! partition and seeds the aggregator via [`SENSOR_CHANNEL`]. It then waits
//! on [`LOCATION_WRITE`]; whenever the BLE write handler (substep 5) deposits
//! a new validated [`Location`] there, this task persists it via
//! `sequential-storage` and republishes it to the aggregator.
//!
//! All flash I/O is isolated here; the BLE task never touches flash directly.

use core::ops::Range;

use defmt::{Debug2Format, info, warn};
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_storage::FlashStorage;
use meteo_lib::ble::location::LOCATION_WIRE_LEN;
use meteo_lib::{Location, SensorReading};
use sequential_storage::cache::NoCache;
use sequential_storage::map::{MapConfig, MapStorage};

use crate::aggregator::SENSOR_CHANNEL;

/// Set by the BLE write handler with a validated new location; awaited here.
///
/// A `Signal` is appropriate: only the latest write matters; no queue is needed.
pub static LOCATION_WRITE: Signal<CriticalSectionRawMutex, Location> = Signal::new();

/// `sequential-storage` map key for the single stored location record.
const LOCATION_KEY: u8 = 0;

/// Flash-backed config task.
///
/// `flash` must be the sole owner of the NVS flash peripheral (moves in, never
/// returns). `range` must be the NVS partition byte range, 4-KiB-aligned,
/// ≥ 8 KiB — satisfied by the default ESP32-H2 partition table (0x9000…0xF000).
///
/// # Panics
///
/// Panics at boot if `range` does not meet `sequential-storage`'s alignment /
/// minimum-size requirements (two 4-KiB pages). This mirrors the `expect()`
/// pattern used for all other firmware init failures.
#[embassy_executor::task]
pub async fn run(raw_flash: FlashStorage<'static>, range: Range<u32>) {
    let flash = BlockingAsync::new(raw_flash);
    // `MapConfig::new()` panics for an invalid range; acceptable at init time.
    let config = MapConfig::new(range);
    let mut storage = MapStorage::<u8, _, _>::new(flash, config, NoCache::new());
    // Scratch buffer: must hold serialized key (1 B) + value (6 B) + item-header
    // overhead; 64 B is ample. Word-aligned by allocator on this RISC-V target.
    let mut buf = [0_u8; 64];

    // ── Boot read: restore persisted location and seed the aggregator ──────────
    match storage
        .fetch_item::<[u8; LOCATION_WIRE_LEN]>(&mut buf, &LOCATION_KEY)
        .await
    {
        Ok(Some(bytes)) => match Location::from_wire(&bytes) {
            Ok(loc) => {
                info!("config: restored location {:?}", loc);
                SENSOR_CHANNEL.send(SensorReading::Location(loc)).await;
            }
            Err(e) => warn!("config: stored location invalid: {:?}", e),
        },
        Ok(None) => info!("config: no stored location"),
        Err(e) => warn!("config: flash read failed: {:?}", Debug2Format(&e)),
    }

    // ── Update loop: persist and republish whenever a new location arrives ─────
    loop {
        let loc = LOCATION_WRITE.wait().await;
        let bytes = loc.to_wire();
        match storage.store_item(&mut buf, &LOCATION_KEY, &bytes).await {
            Ok(()) => info!("config: location updated"),
            Err(e) => warn!("config: flash write failed: {:?}", Debug2Format(&e)),
        }
        SENSOR_CHANNEL.send(SensorReading::Location(loc)).await;
    }
}
