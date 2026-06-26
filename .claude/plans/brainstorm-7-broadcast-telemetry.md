# Plan: BLE broadcast telemetry + GPS-config connectable channel

- **Source:** '7 (`.claude/brainstorm/7-ble-broadcast-telemetry.md`)
- **Date:** 2026-06-25
- **Status:** Done

## Summary

Switch the station from a single-central GATT-notify connection to an **extended
connectable broadcast**: one `ExtConnectableNonscannableUndirected` advertising
set carries the telemetry frame as Manufacturer-Specific Data (company id
`0xFFFF`), refreshed in place at 1 Hz via `update_adv_data_ext`, so any number of
passive observers read the weather at once. The advert stays _connectable_, and
the reserved 128-bit GATT service now has a concrete purpose: a **writable
location characteristic** lets a user set the station's GPS coordinates, which are
**persisted to flash** (survive reboot) and **broadcast** in the frame. The write
is gated by a **compile-time PIN** (`000911`): the 10-byte write payload is
`PIN (u32 LE) + 6-byte coarse location`, and the firmware rejects it unless the
PIN matches — an application-level gate (the PIN travels in cleartext on the
one-time config connection; acceptable for this device, no BLE pairing/SMP). A new
monotonic `uptime_s` (u32) field makes the payload change every second (defeating
BlueZ manufacturer-data dedup) and lets the dashboard spot reboots and dropped
frames. Coordinates are stored and broadcast at **~1 km resolution by
construction** (`i16`, deg×100) — the station never holds a precise fix. The TUI
is reworked from connect+notify to passive-scan + manufacturer-data decode, with a
frame-age signal-state model. The connection-only Device Information Service
(firmware version) is removed. A `scripts/ble_set_location.sh` writes the
PIN + coordinates over GATT; `scripts/ble_broadcast_check.sh` (renamed) verifies
the broadcast.

## Files Modified

| File                                                             | Action         | Description                                                                                                                                                                                                                                                            |
| ---------------------------------------------------------------- | -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/meteo-lib/src/ble/frame.rs`                              | modify         | `FRAME_VERSION` 4→5; `FRAME_LEN` 28→38; add `uptime_s: u32` (28–31) + `latitude_deg`/`longitude_deg`/`altitude_m` (`Option<f32>`, coarse `i16` at 32–37); encode/decode; doc; tests                                                                                    |
| `crates/meteo-lib/src/ble/location.rs`                           | create         | `Location` coarse-wire type (`from_wire`/`to_wire`, validation, `LocationError`) + `parse_authorized_write` (PIN + location, `LocationWriteError`); host-tested                                                                                                        |
| `crates/meteo-lib/src/ble/mod.rs`                                | modify         | `pub mod location;`                                                                                                                                                                                                                                                    |
| `crates/meteo-lib/src/lib.rs`                                    | modify         | Re-export `Location`, `LocationError`                                                                                                                                                                                                                                  |
| `crates/meteo-lib/src/aggregate.rs`                              | modify         | `SensorReading::Location(Location)` + ingest → set the three location fields; tests                                                                                                                                                                                    |
| `crates/meteo-firmware/src/aggregator.rs`                        | modify         | Stamp `uptime_s` from `embassy_time::Instant` before signalling `TELEMETRY`                                                                                                                                                                                            |
| `crates/meteo-firmware/src/config.rs`                            | create         | Flash-backed config: `config_task` (boot-read seeds `SENSOR_CHANNEL`, awaits `LOCATION_WRITE` → persist + republish); `esp-storage`+`sequential-storage`                                                                                                               |
| `crates/meteo-firmware/src/ble.rs`                               | modify         | Extended connectable broadcast; manufacturer-data adv; reserved service **with PIN-gated location write characteristic**; GATT write handling → check PIN + validate + signal `LOCATION_WRITE`; drop telemetry Notify char + DIS + `notify_loop`; connection heartbeat |
| `crates/meteo-firmware/src/main.rs`                              | modify         | Create `FlashStorage` + flash range; spawn `config_task`; module decl                                                                                                                                                                                                  |
| `crates/meteo-firmware/Cargo.toml`                               | modify         | Add `esp-storage`, `sequential-storage` (target deps)                                                                                                                                                                                                                  |
| `Cargo.toml` (workspace)                                         | modify         | Add `esp-storage`, `sequential-storage` to `[workspace.dependencies]`                                                                                                                                                                                                  |
| `crates/meteo-tui/src/model.rs`                                  | modify         | Replace `ConnState`/`LinkEvent` with `SignalState`; add `fmt_location`; remove `parse_fw_revision`                                                                                                                                                                     |
| `crates/meteo-tui/src/ble.rs`                                    | modify         | Passive-scan + manufacturer-data decode; `BleEvent` collapses to `Frame(Telemetry)`                                                                                                                                                                                    |
| `crates/meteo-tui/src/app.rs`                                    | modify         | Drop `conn`/`fw_version`; add `signal_state(now)`; simplify `apply`                                                                                                                                                                                                    |
| `crates/meteo-tui/src/ui.rs`                                     | modify         | Header shows signal state; drop firmware version; add Location row                                                                                                                                                                                                     |
| `crates/meteo-tui/src/main.rs`                                   | modify         | Wire simplified `BleEvent`; pass `now` for signal state                                                                                                                                                                                                                |
| `scripts/ble_set_location.sh`                                    | create         | gatttool/bluetoothctl GATT write of the 10-byte PIN + coarse location blob                                                                                                                                                                                             |
| `scripts/ble_notify_check.sh` → `scripts/ble_broadcast_check.sh` | rename+rewrite | Passive-scan capture of company-`0xFFFF` 38-byte v5 frames                                                                                                                                                                                                             |
| `scripts/ble_soak.sh`                                            | modify         | Notes: connection now exercises the location-config channel; meteo data is broadcast                                                                                                                                                                                   |
| `CLAUDE.md`                                                      | modify         | BLE section, frame layout, GATT table, location config, TUI rationale, scripts                                                                                                                                                                                         |

## Design constants (pin once, used everywhere)

- **Frame v5:** `FRAME_VERSION = 5`, `FRAME_LEN = 38`.
- **Location encoding (coarse, ~1 km):** lat/lon as `i16` = `round(deg × 100)`
  (0.01° ≈ 1.1 km at the equator); altitude as `i16` metres. Sentinel `i16::MIN`
  for every field = unset → `None`. Valid ranges: lat ∈ `[-9000, 9000]`,
  lon ∈ `[-18000, 18000]`, alt ∈ `[-32767, 32767]`. The 6-byte wire form (lat,
  lon, alt — all `i16` LE) is identical across the flash blob, `Location::to_wire`,
  and frame bytes 32–37 (the GATT **write** payload prefixes this with the 4-byte
  PIN — see below).
- **Manufacturer company id:** `0xFFFF` (reserved/test).
- **Reserved service UUID:** `7e700010-b1df-42a1-bb5f-6a1028c793b0`.
- **Location characteristic UUID:** `7e700011-b1df-42a1-bb5f-6a1028c793b0`.
- **Config PIN (compile-time):** `CONFIG_PIN: u32 = 911` (entered as `000911`),
  defined in `crates/meteo-firmware/src/ble.rs`.
- **Authorized-write payload:** `AUTH_WRITE_LEN = 10` = PIN (`u32` LE, bytes 0–3) +
  coarse location (`LOCATION_WIRE_LEN = 6`, bytes 4–9). The PIN is never stored or
  broadcast — it only gates the write.

## Plan

### 1. Frame v5: append `uptime_s` + coarse location (`meteo-lib`)

**File:** `crates/meteo-lib/src/ble/frame.rs`

**What:** Grow the frame to 38 bytes (version 5). Append `uptime_s` (u32, 28–31,
always present) and three coarse location fields (i16 each, 32–37, sentinel
`i16::MIN`).

```rust
pub const FRAME_VERSION: u8 = 5;   // was 4
pub const FRAME_LEN: usize = 38;   // was 28

pub struct Telemetry {
    // ... existing fields ...
    pub diagnostics: Diagnostics,
    /// Seconds since boot (monotonic, resets on reboot). Always present.
    pub uptime_s: u32,                    // NEW (28–31)
    /// Station latitude in degrees, coarse ~1 km (0.01° steps). `None` until set.
    pub latitude_deg: Option<f32>,        // NEW (32–33)
    /// Station longitude in degrees, coarse ~1 km. `None` until set.
    pub longitude_deg: Option<f32>,       // NEW (34–35)
    /// Station altitude in metres. `None` until set.
    pub altitude_m: Option<f32>,          // NEW (36–37)
}
```

**Encode (after the power write at 26..28):**

```rust
frame[28..32].copy_from_slice(&self.uptime_s.to_le_bytes());
frame[32..34].copy_from_slice(&scale_loc_i16(self.latitude_deg, 100.0).to_le_bytes());
frame[34..36].copy_from_slice(&scale_loc_i16(self.longitude_deg, 100.0).to_le_bytes());
frame[36..38].copy_from_slice(&scale_loc_i16(self.altitude_m, 1.0).to_le_bytes());
frame
```

**Decode (after `load_ma`):**

```rust
let uptime_s = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
let latitude_deg  = decode_loc(i16::from_le_bytes([bytes[32], bytes[33]]), 100.0);
let longitude_deg = decode_loc(i16::from_le_bytes([bytes[34], bytes[35]]), 100.0);
let altitude_m    = decode_loc(i16::from_le_bytes([bytes[36], bytes[37]]), 1.0);
```

**New helpers (module-private, mirror `scale_u16`/`scale_i16` style):**

```rust
/// Scale an optional degrees/metres value to the coarse i16 wire form, clamping
/// away from the i16::MIN sentinel; `None` → i16::MIN.
fn scale_loc_i16(v: Option<f32>, factor: f32) -> i16 {
    match v {
        None => i16::MIN,
        Some(x) => {
            let r = libm::roundf(x * factor);
            #[expect(clippy::cast_possible_truncation, reason = "clamped to i16 range below")]
            { r.max(f32::from(i16::MIN) + 1.0).min(f32::from(i16::MAX)) as i16 }
        }
    }
}
/// Inverse of `scale_loc_i16`: i16::MIN → None, else raw / factor.
fn decode_loc(raw: i16, factor: f32) -> Option<f32> {
    if raw == i16::MIN { None } else { Some(f32::from(raw) / factor) }
}
```

**Also:** `Telemetry::empty()` adds `uptime_s: 0, latitude_deg: None,
longitude_deg: None, altitude_m: None`. Update the module doc table (bump version
row to 5; add rows: `28–31 uptime u32 LE`; `32–33 latitude i16 LE deg×100,
i16::MIN`; `34–35 longitude i16 LE deg×100, i16::MIN`; `36–37 altitude i16 LE m,
i16::MIN`).

**Tests:**

- `encode_emits_thirty_eight_bytes_with_version_five` — replaces the v4 length/version
  test; asserts `frame.len() == 38`, `frame[0] == 5`.
- `decode_rejects_wrong_length` — assert `WrongLength(28)` and `WrongLength(37)`.
- `decode_rejects_unknown_version` — `frame[0] = 6`, expect `UnknownVersion(6)`,
  array sized 38.
- `uptime_roundtrips_value` — `uptime_s: 123_456` round-trips; bytes 28–31 match LE.
- `location_roundtrips_within_coarse_resolution` — `latitude_deg: Some(48.853),
longitude_deg: Some(2.349), altitude_m: Some(35.0)`; after encode→decode each is
  within the coarse LSB (`|Δlat| ≤ 0.01`, `|Δlon| ≤ 0.01`, `|Δalt| ≤ 1.0`); assert
  bytes 32–33 == `4885i16.to_le_bytes()`.
- `location_sentinels_decode_to_none` — `Telemetry::empty()` encodes lat/lon/alt to
  `i16::MIN` and decodes back to `None`.
- `decode_maps_sentinels_back_to_none` — add `uptime_s == 0`, `latitude_deg == None`, etc.
- proptest `roundtrip_decode_encode_is_identity_at_wire_level` — extend to
  `[0u8; 38]`, add `uptime in any::<u32>()` into 28–31, and to keep bit-exactness
  force location bytes 32–37 to the `i16::MIN` sentinel (the coarse scaling is not
  bit-exact for arbitrary inputs, same reason lux/rain are forced to sentinels).
  Add a separate `location_roundtrip_within_tolerance` proptest:
  `lat in -90.0..=90.0`, `lon in -180.0..=180.0`, `alt in -1000.0..=9000.0`;
  assert recovered within the coarse LSB.

**Dependencies:** none — do first.

### 2. `Location` wire type + `SensorReading::Location` (`meteo-lib`)

**Files:** `crates/meteo-lib/src/ble/location.rs` (new),
`crates/meteo-lib/src/ble/mod.rs`, `crates/meteo-lib/src/lib.rs`,
`crates/meteo-lib/src/aggregate.rs`

**What:** A pure, host-tested coarse-location type shared by the GATT write handler,
the flash store, and the aggregator. The GATT write payload, the flash blob, and
frame bytes 32–37 are all this same 6-byte form.

**`location.rs`:**

```rust
//! Coarse (~1 km) station-location wire type, shared by the BLE config write,
//! flash persistence, and the broadcast frame.

/// Length of the coarse location wire blob: lat i16 + lon i16 + alt i16, all LE.
pub const LOCATION_WIRE_LEN: usize = 6;

/// Coarse station location. Resolution is ~1.1 km (lat/lon 0.01°), 1 m altitude —
/// the station never holds a finer fix.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Location {
    pub latitude_deg: f32,
    pub longitude_deg: f32,
    pub altitude_m: f32,
}

/// Errors from [`Location::from_wire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocationError {
    WrongLength(usize),
    OutOfRange,
}

impl Location {
    /// Parse the 6-byte coarse wire form (lat,lon,alt as i16 LE, deg×100 / m).
    /// Rejects wrong length, the i16::MIN sentinel in any field, and lat/lon out
    /// of geographic range.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, LocationError> {
        if bytes.len() != LOCATION_WIRE_LEN {
            return Err(LocationError::WrongLength(bytes.len()));
        }
        let lat = i16::from_le_bytes([bytes[0], bytes[1]]);
        let lon = i16::from_le_bytes([bytes[2], bytes[3]]);
        let alt = i16::from_le_bytes([bytes[4], bytes[5]]);
        if lat == i16::MIN || lon == i16::MIN || alt == i16::MIN {
            return Err(LocationError::OutOfRange);
        }
        if !(-9000..=9000).contains(&lat) || !(-18000..=18000).contains(&lon) {
            return Err(LocationError::OutOfRange);
        }
        Ok(Self {
            latitude_deg: f32::from(lat) / 100.0,
            longitude_deg: f32::from(lon) / 100.0,
            altitude_m: f32::from(alt),
        })
    }

    /// Serialize to the 6-byte coarse wire form (for flash storage).
    #[must_use]
    pub fn to_wire(&self) -> [u8; LOCATION_WIRE_LEN] {
        let lat = clamp_i16(self.latitude_deg * 100.0);
        let lon = clamp_i16(self.longitude_deg * 100.0);
        let alt = clamp_i16(self.altitude_m);
        let mut b = [0_u8; LOCATION_WIRE_LEN];
        b[0..2].copy_from_slice(&lat.to_le_bytes());
        b[2..4].copy_from_slice(&lon.to_le_bytes());
        b[4..6].copy_from_slice(&alt.to_le_bytes());
        b
    }
}
// clamp_i16: round + clamp to [i16::MIN+1, i16::MAX] (away from sentinel).

/// Length of the PIN-gated GATT write payload: PIN (u32 LE) + LOCATION_WIRE_LEN.
pub const AUTH_WRITE_LEN: usize = 4 + LOCATION_WIRE_LEN; // 10

/// Errors from [`parse_authorized_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocationWriteError {
    /// Payload was not exactly `AUTH_WRITE_LEN` bytes.
    WrongLength(usize),
    /// The leading PIN did not match the expected value.
    BadPin,
    /// The location portion was invalid.
    Location(LocationError),
}

/// Parse a PIN-gated location write: bytes 0..4 = PIN (u32 LE), bytes 4..10 =
/// coarse location wire form. The PIN is checked **before** the location is
/// parsed; rejects wrong length, PIN mismatch, and any `Location::from_wire` error.
pub fn parse_authorized_write(
    bytes: &[u8],
    expected_pin: u32,
) -> Result<Location, LocationWriteError> {
    if bytes.len() != AUTH_WRITE_LEN {
        return Err(LocationWriteError::WrongLength(bytes.len()));
    }
    let pin = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if pin != expected_pin {
        return Err(LocationWriteError::BadPin);
    }
    Location::from_wire(&bytes[4..]).map_err(LocationWriteError::Location)
}
```

**`ble/mod.rs`:** add `pub mod location;`. **`lib.rs`:** add
`pub use ble::location::{AUTH_WRITE_LEN, Location, LocationError, LocationWriteError, parse_authorized_write};`.

**`aggregate.rs`:** add the channel variant and ingest:

```rust
pub enum SensorReading {
    // ... existing ...
    /// Station location set over BLE / restored from flash. Sets the three frame
    /// location fields. (No fault variant: location is config, not a live sensor.)
    Location(crate::ble::location::Location),
}
// in ingest():
SensorReading::Location(loc) => {
    self.telemetry.latitude_deg = Some(loc.latitude_deg);
    self.telemetry.longitude_deg = Some(loc.longitude_deg);
    self.telemetry.altitude_m = Some(loc.altitude_m);
}
```

(`SensorReading` derives `Copy`; `Location` is `Copy`, so this holds.)

**Tests (`location.rs` + `aggregate.rs`):**

- `from_wire_roundtrips_to_wire` — build `Location { 48.85, 2.35, 35.0 }`, `to_wire`,
  `from_wire`, assert each field within the coarse LSB (`|Δlat|, |Δlon| ≤ 0.01`,
  `|Δalt| ≤ 1.0`).
- `from_wire_rejects_wrong_length` — 5 bytes → `WrongLength(5)`.
- `from_wire_rejects_sentinel` — lat bytes = `i16::MIN.to_le_bytes()` → `OutOfRange`.
- `from_wire_rejects_out_of_range` — lat = `9001` (>90°) → `OutOfRange`.
- `parse_authorized_write_accepts_correct_pin` — `[911u32 LE][Location{48.85,2.35,35}.to_wire()]`
  with `expected_pin = 911` → `Ok`, fields within the same coarse LSB tolerance
  (`|Δlat|, |Δlon| ≤ 0.01`, `|Δalt| ≤ 1.0`).
- `parse_authorized_write_rejects_bad_pin` — same payload but PIN bytes `0u32`,
  `expected_pin = 911` → `Err(BadPin)`.
- `parse_authorized_write_rejects_wrong_length` — 9 bytes → `Err(WrongLength(9))`.
- `parse_authorized_write_propagates_location_error` — correct PIN, lat = `9001`
  → `Err(Location(LocationError::OutOfRange))`.
- `aggregator_location_sets_three_fields` (in `aggregate.rs` tests) — ingest
  `SensorReading::Location(Location{..})`, assert snapshot lat/lon/alt are `Some`
  within tolerance.

**Dependencies:** substep 1 (the `Telemetry` location fields must exist).

### 3. Firmware aggregator: stamp `uptime_s` (firmware)

**File:** `crates/meteo-firmware/src/aggregator.rs`

```rust
Either::Second(()) => {
    let mut snap = agg.snapshot();
    #[expect(clippy::cast_possible_truncation, reason = "u32 holds ~136 y of seconds")]
    { snap.uptime_s = embassy_time::Instant::now().as_secs() as u32; }
    TELEMETRY.signal(snap);
    AGG_BEAT.fetch_add(1, Ordering::Relaxed);
}
```

Add `use embassy_time::Instant;` (or fully-qualify). No tests (hardware clock).

**Dependencies:** substep 1.

### 4. Firmware flash config module + main wiring (firmware)

**Files:** `crates/meteo-firmware/src/config.rs` (new),
`crates/meteo-firmware/src/main.rs`, `crates/meteo-firmware/Cargo.toml`,
workspace `Cargo.toml`

**What:** A flash-backed config owner. At boot it reads the stored coarse location
and seeds the aggregator; thereafter it waits on a `LOCATION_WRITE` signal (set by
the BLE write handler), persists the new blob, and republishes it. **All flash
I/O lives here, off the BLE task** — `esp-storage` wraps the ROM flash op in a
critical section (cache off < 10 ms); the already-negotiated 8 s supervision
timeout absorbs the pause without dropping the link (researched).

**Cargo deps** (workspace `[workspace.dependencies]` + firmware target deps):

```toml
esp-storage = { version = "0.9", features = ["esp32h2"] }
sequential-storage = "7"
```

(`embedded-storage`/`embedded-storage-async` are already in the tree;
`embassy-embedded-hal` is already a dep and provides `BlockingAsync`.)

**`config.rs` sketch:**

```rust
use core::ops::Range;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_storage::FlashStorage;
use meteo_lib::{Location, SensorReading};
use sequential_storage::cache::NoCache;
use sequential_storage::map::{fetch_item, store_item};

use crate::aggregator::SENSOR_CHANNEL;

/// Set by the BLE write handler with a validated new location; awaited here.
pub static LOCATION_WRITE: Signal<CriticalSectionRawMutex, Location> = Signal::new();

/// sequential-storage map key for the single location record.
const LOCATION_KEY: u8 = 0;

#[embassy_executor::task]
pub async fn run(flash: FlashStorage, range: Range<u32>) {
    let mut flash = BlockingAsync::new(flash);
    let mut cache = NoCache::new();
    let mut buf = [0_u8; 128]; // scratch ≥ largest item + overhead

    // Boot read: restore + seed the aggregator so the broadcast carries the saved
    // location immediately.
    match fetch_item::<u8, [u8; meteo_lib::ble::location::LOCATION_WIRE_LEN], _>(
        &mut flash, range.clone(), &mut cache, &mut buf, &LOCATION_KEY,
    ).await {
        Ok(Some(bytes)) => match Location::from_wire(&bytes) {
            Ok(loc) => { SENSOR_CHANNEL.send(SensorReading::Location(loc)).await; }
            Err(e) => warn!("config: stored location invalid: {:?}", e),
        },
        Ok(None) => info!("config: no stored location"),
        Err(e) => warn!("config: flash read failed: {:?}", defmt::Debug2Format(&e)),
    }

    loop {
        let loc = LOCATION_WRITE.wait().await;
        let bytes = loc.to_wire();
        if let Err(e) = store_item(
            &mut flash, range.clone(), &mut cache, &mut buf, &LOCATION_KEY, &bytes,
        ).await {
            warn!("config: flash write failed: {:?}", defmt::Debug2Format(&e));
        }
        // Republish regardless of persistence outcome so the broadcast reflects the
        // user's intent even if a write transiently failed.
        SENSOR_CHANNEL.send(SensorReading::Location(loc)).await;
        info!("config: location updated");
    }
}
```

> **Verify at build time:**
> (a) `sequential-storage = "7"` is correct — the crate is at **7.2.0** (confirmed
> via the crates.io sparse index; it uses 7.x, not 0.x versioning), so `"7"`
> resolves. Still confirm the exact `fetch_item`/`store_item` signatures and
> `Value`/`Key` bounds against docs.rs for 7.2.0 (the call shape above matches the
> 7.x map free functions). (b) `esp-storage` 0.9 `FlashStorage::new()` and the
> `esp32h2` feature. (c) `BlockingAsync` implements
> `embedded-storage-async::NorFlash` for `FlashStorage`.

**Flash range (`main.rs`):** reserve a region from the esp-idf partition table.
Preferred: locate the default `nvs` partition (unused by this firmware) via
`esp-bootloader-esp-idf`'s partitions API and pass its `offset..offset+size`.

```rust
// esp-bootloader-esp-idf is ALREADY a dependency (main.rs uses `esp_app_desc!`),
// so no Cargo change for it. read_partition_table takes the flash + a scratch
// buffer (confirmed against the 0.5.0 registry source — two args, not one).
// Locate the default, firmware-unused `nvs` partition and pass its range on.
use esp_bootloader_esp_idf::partitions::{
    DataPartitionSubType, PartitionType, read_partition_table,
};

let mut flash = esp_storage::FlashStorage::new();
let mut pt_buf = [0_u8; 512]; // partition-table scratch
let pt = read_partition_table(&mut flash, &mut pt_buf).expect("partition table");
let nvs = pt
    .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
    .expect("nvs lookup")
    .expect("nvs partition present");
// PartitionEntry exposes `offset()` (:49) and `len()` (:54) — NOT `size()`.
// saturating_add keeps the active `arithmetic_side_effects` lint happy.
let flash_range = nvs.offset()..nvs.offset().saturating_add(nvs.len());
```

> **Verify at build:** the `find_partition` return shape (the `Result`/`Option`
> nesting that drives the double `.expect()` above) against esp-bootloader-esp-idf
> 0.5.0 (`partitions.rs:294`); `offset()`/`len()` are confirmed (`:49`/`:54`). The
> `FlashStorage` value is then moved into `config::run`.

> **Fallback if the 0.5 partitions API differs or is absent:** add a custom
> `partitions.csv` with a dedicated `config, data, nvs, <offset>, 0x4000` entry,
> flash it via espflash, and pass that fixed `offset..offset+0x4000` range. Document
> whichever path is taken in `CLAUDE.md`. Either way the range must not overlap the
> bootloader/app/OTA regions — verify against the flashed partition table.

**`main.rs` wiring:**

```rust
mod config; // add to the module list

// ... after esp_rtos::start: build `flash` + `flash_range` exactly as in the
// partition-lookup block above (the FlashStorage is moved into config::run), then:
spawner.spawn(config::run(flash, flash_range).expect("config task already spawned"));
```

**Tests:** none (hardware flash). Validated by `just build` and on-device:
`ble_set_location.sh` then a reboot, confirming the location persists in the
broadcast.

**Dependencies:** substep 2 (`Location`, `SensorReading::Location`).

### 5. Firmware BLE: broadcast + writable reserved location service (firmware)

**File:** `crates/meteo-firmware/src/ble.rs`

**What:** Replace connect-accept-notify with the continuous extended broadcast
(see `advertise_loop` below), and give the reserved service a **PIN-gated writable
location characteristic**. On a write the handler validates via
`parse_authorized_write` (checks the compile-time `CONFIG_PIN`, then the
location), signals `config::LOCATION_WRITE` (flash + republish happen in
`config_task`), and ACKs; writes with a wrong PIN or an invalid location are
rejected. No flash I/O on the BLE task.

**Remove:** the telemetry Notify service/characteristic + constants, the DIS
service + `FW_*` constants, `notify_loop`, `telemetry_storage`/`telemetry_char`,
the legacy 31-byte `adv_buf`/`scan_buf` + `ConnectableScannableUndirected`.

**New constants:**

```rust
const RESERVED_SERVICE_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x10, 0x00, 0x70, 0x7e,
]); // 7e700010-…
const LOCATION_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x11, 0x00, 0x70, 0x7e,
]); // 7e700011-…
const COMPANY_ID: u16 = 0xFFFF;

/// Compile-time PIN gating the location write (entered as `000911`). An app-level
/// gate, not a cryptographic secret: it stops the config channel being wide open,
/// but travels in cleartext (no BLE pairing). See the security note in CLAUDE.md.
const CONFIG_PIN: u32 = 911;

// GAP/GATT mandatory (6) + reserved primary-service (1) + location char
// (declaration + value = 2) = 9.
const ATT_MAX: usize = 9;     // was 13
const CCCD_MAX: usize = 1;    // mandatory GATT service-changed slot; location char has no CCCD
```

**Imports:** add `trouble_host::advertise::{AdvertisementSet, AdvSet}` and
`Advertisement::ExtConnectableNonscannableUndirected`; add
`trouble_host::attribute::CharacteristicProp`; add
`trouble_host::gatt::GattEvent`; add `AttErrorCode` (confirmed at
`third_party/trouble-host/src/att.rs:64` — import from `trouble_host::att` or the
prelude); add `embassy_time::Ticker` (for `connection_heartbeat`). Keep `Service`,
`AttributeTable`, `AttributeServer`, `Characteristic`, `AdStructure`,
`AdvertisementParameters`, `Peripheral`.

**HostResources:** `HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX, 1>`
(explicit `ADV_SETS = 1`).

**Attribute table (replace telemetry+DIS blocks):**

```rust
// The characteristic value holds the full 10-byte PIN-gated write payload.
let mut location_storage = [0_u8; meteo_lib::ble::location::AUTH_WRITE_LEN]; // 10
let location_char: Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]> = {
    let mut svc = table.add_service(Service::new(RESERVED_SERVICE_UUID));
    let ch = svc
        .add_characteristic(
            LOCATION_UUID,
            &[CharacteristicProp::Read, CharacteristicProp::Write],
            [0_u8; meteo_lib::ble::location::AUTH_WRITE_LEN],
            &mut location_storage,
        )
        .build();
    svc.build();
    ch
};
let server: MeteoServer<'_> = AttributeServer::new(table);
```

**`advertise_loop`** — the continuous extended broadcast. Re-arms with the latest
frame, refreshes the manufacturer data in place at 1 Hz (paced by `TELEMETRY`), and
polls `try_accept()` each refresh:

```rust
async fn advertise_loop(
    stack: &Stack<'_, Controller, DefaultPacketPool>,
    peripheral: &mut Peripheral<'_, Controller, DefaultPacketPool>,
    server: &MeteoServer<'_>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
) {
    // adv_data: Flags(3) + CompleteLocalName "MeteoStation" (14) +
    // ManufacturerSpecificData(4 header + FRAME_LEN=38 = 42) = 59 ≤ 64. Extended
    // advertising removes the 31-byte legacy ceiling.
    let mut adv_buf = [0_u8; 64];

    loop {
        // (Re)arm with the latest (or empty) frame. AdvertisementParameters has no
        // Copy impl in trouble-host 0.6, so build it per set with `::default()`
        // (Le1M PHY, 160 ms interval).
        let telem = TELEMETRY.try_take().unwrap_or_else(Telemetry::empty);
        let frame = telem.encode();
        let adv_len = encode_adv(&mut adv_buf, &frame);
        let sets = [AdvertisementSet {
            params: AdvertisementParameters::default(),
            data: Advertisement::ExtConnectableNonscannableUndirected {
                adv_data: &adv_buf[..adv_len],
            },
        }];
        let mut handles = AdvertisementSet::handles(&sets);
        let Ok(advertiser) = peripheral.advertise_ext(&sets, &mut handles).await else {
            warn!("BLE: advertise_ext() failed, retrying");
            continue;
        };
        ADV_BEAT.fetch_add(1, Ordering::Relaxed);
        crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);
        info!("BLE: broadcasting (beat={})", ADV_BEAT.load(Ordering::Relaxed));

        // Refresh-and-poll. The aggregator signals TELEMETRY at 1 Hz, pacing this
        // loop and supplying a fresh uptime each frame; `try_accept()` is polled
        // each refresh (a connecting central waits ≤ ~1 s to be served).
        let raw_conn = loop {
            let telem = TELEMETRY.wait().await; // ~1 Hz
            let frame = telem.encode();
            let adv_len = encode_adv(&mut adv_buf, &frame); // mutate buffer in place
            let sets = [AdvertisementSet {
                params: AdvertisementParameters::default(),
                data: Advertisement::ExtConnectableNonscannableUndirected {
                    adv_data: &adv_buf[..adv_len],
                },
            }];
            if let Err(e) = peripheral.update_adv_data_ext(&sets, &mut handles).await {
                warn!("BLE: update_adv_data_ext failed: {:?}", defmt::Debug2Format(&e));
            }
            ADV_BEAT.fetch_add(1, Ordering::Relaxed);
            crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);
            if let Some(conn) = peripheral.try_accept() {
                break conn;
            }
        };

        // A central connected: stop broadcasting (drop the advertiser; the
        // connection already stopped the connectable set), serve it, then re-arm.
        drop(advertiser);
        serve_connection(stack, server, location_char, raw_conn).await;
    }
}
```

> **`TELEMETRY.try_take()`**: if `embassy_sync::signal::Signal` exposes no
> `try_take`, use `Telemetry::empty()` for the initial arm — the first `wait()` in
> the refresh loop supplies real data within ~1 s.

> **Borrow note (key):** `advertise_ext` returns `Advertiser<'d,…>` bound to the
> _stack_, not the `&mut self` receiver, so `peripheral` stays usable for
> `update_adv_data_ext`/`try_accept` while the advertiser is held. Drop it to stop
> broadcasting when a connection forms.

**`encode_adv`** — builds the advertisement payload:

```rust
/// Encode Flags + name + the manufacturer-data frame into `buf`, returning the
/// length. The 64 B buffer fits Flags(3) + CompleteLocalName(14) +
/// ManufacturerSpecificData(4 + FRAME_LEN).
fn encode_adv(buf: &mut [u8], frame: &[u8; meteo_lib::FRAME_LEN]) -> usize {
    AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(STATION_NAME.as_bytes()),
            AdStructure::ManufacturerSpecificData {
                company_identifier: COMPANY_ID,
                payload: frame,
            },
        ],
        buf,
    )
    .expect("adv encode (64 B fits Flags+name+frame)")
}
```

**`serve_connection`** — keep the 8 s supervision negotiation; serve until
disconnect with the write-handling event loop + heartbeat:

```rust
async fn serve_connection(
    stack: &Stack<'_, Controller, DefaultPacketPool>,
    server: &MeteoServer<'_>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
    raw_conn: Connection<'_, DefaultPacketPool>,
) {
    if let Err(e) = raw_conn.update_connection_params(stack, &RequestedConnParams::default()).await {
        warn!("BLE: conn-params update failed: {:?}", defmt::Debug2Format(&e));
    }
    let Ok(conn) = raw_conn.with_attribute_server(server) else {
        warn!("BLE: with_attribute_server() failed"); return;
    };
    info!("BLE: central connected (config channel)");
    select(gatt_events(&conn, location_char), connection_heartbeat()).await;
    info!("BLE: disconnected, resuming broadcast");
}

async fn connection_heartbeat() -> ! {
    // Fixed 1 Hz liveness cadence → Ticker::every (the aggregator's pattern), NOT
    // Timer::after: a periodic clock, not a readiness wait (per CLAUDE.md).
    let mut tick = Ticker::every(Duration::from_secs(1));
    loop {
        tick.next().await;
        crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);
    }
}
```

**`gatt_events`** — extend to handle the location write (the one functional GATT op):

```rust
async fn gatt_events(
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
) {
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                info!("BLE: disconnected (reason={:?})", defmt::Debug2Format(&reason));
                break;
            }
            GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, supervision_timeout } => {
                info!("BLE: conn params: interval_us={=u64} latency={=u16} supervision_ms={=u64}",
                    conn_interval.as_micros(), peripheral_latency, supervision_timeout.as_millis());
            }
            GattConnectionEvent::Gatt { event } => match event {
                GattEvent::Write(write) if write.handle() == location_char.handle => {
                    // PIN-gated: parse_authorized_write checks CONFIG_PIN before the location.
                    match meteo_lib::parse_authorized_write(write.data(), CONFIG_PIN) {
                        Ok(loc) => {
                            crate::config::LOCATION_WRITE.signal(loc); // flash + republish in config_task
                            info!("BLE: location write accepted");
                            if let Ok(reply) = write.accept() { reply.send().await; }
                        }
                        Err(e) => {
                            // Wrong PIN, wrong length, or invalid location → reject.
                            warn!("BLE: rejected location write: {:?}", e);
                            let code = match e {
                                meteo_lib::LocationWriteError::BadPin => {
                                    AttErrorCode::INSUFFICIENT_AUTHORISATION
                                }
                                _ => AttErrorCode::OUT_OF_RANGE,
                            };
                            if let Ok(reply) = write.reject(code) { reply.send().await; }
                        }
                    }
                }
                // All other GATT requests (reads, writes to other handles): let the
                // attribute server process them normally.
                other => { if let Ok(reply) = other.accept() { reply.send().await; } }
            }
            GattConnectionEvent::PhyUpdated { .. }
            | GattConnectionEvent::RequestConnectionParams(_)
            | GattConnectionEvent::DataLengthUpdated { .. } => {}
        }
    }
}
```

> **Verify against the vendored source** (mapped, but pin exact names): `GattEvent`
> variants (`Read`/`Write`/`Other`/`NotAllowed`), `WriteEvent::handle()` /
> `.data()` / `.accept()` / `.reject(AttErrorCode)`, `Reply::send().await`, the
> `AttErrorCode` variants are confirmed in `att.rs`:
> `INSUFFICIENT_AUTHORISATION` (`:64`, bad PIN) and `OUT_OF_RANGE` (`:96`, bad
> payload). `GattEvent::Other`/`NotAllowed` may need their own `accept()`/handling
> arms — match all four variants exhaustively.

**`run()`:** drop telemetry/DIS storage; build the location characteristic (above);
call `advertise_loop(&stack, &mut peripheral, &server, &location_char)`.

**Tests:** none (hardware BLE). Validated by `just build` and the on-device gate
(`ble_set_location.sh` write accepted; coarse coords appear in the broadcast and
survive reboot).

**Dependencies:** substeps 1, 2, 4 (`parse_authorized_write`, `Location`,
`config::LOCATION_WRITE`). No new Cargo feature (`gatt` + `peripheral` already
enabled; the PIN gate is app-level, so no `security`/SMP feature).

### 6. TUI model + passive-scan BLE (host)

**Files:** `crates/meteo-tui/src/model.rs`, `crates/meteo-tui/src/ble.rs`

**`model.rs`:** remove `ConnState`, `LinkEvent`, their methods+tests, and
`parse_fw_revision` (+tests). Add the frame-age `SignalState` enum. It uses
`std::time::{Instant, Duration}` (the TUI's existing time types — `app.rs` and
`main.rs` already use std `Instant`) and the **already-defined** `STALE_AFTER`
constant (`crates/meteo-tui/src/app.rs:21` = `Duration::from_secs(5)`; no new value
to choose):

```rust
use std::time::{Duration, Instant};

/// Dashboard signal state, derived purely from frame age (no link layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalState {
    NoSignal, // no frame received yet
    Live,     // last frame within `stale_after`
    Stale,    // frames seen, but the latest is older than `stale_after`
}

impl SignalState {
    /// Derive the state from the last-frame timestamp.
    #[must_use]
    pub fn from_age(last_frame_at: Option<Instant>, now: Instant, stale_after: Duration) -> Self {
        match last_frame_at {
            None => Self::NoSignal,
            Some(t) if now.duration_since(t) > stale_after => Self::Stale,
            Some(_) => Self::Live,
        }
    }

    /// Status-bar label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoSignal => "No signal",
            Self::Live => "Live",
            Self::Stale => "Stale",
        }
    }
}
```

Add a location formatter:

```rust
/// Format the station location row. `"not set"` until coordinates exist; else
/// e.g. `"48.85, 2.35, 35 m"`. Coarse values render at 2 decimals (lat/lon).
#[must_use]
pub fn fmt_location(lat: Option<f32>, lon: Option<f32>, alt: Option<f32>) -> String {
    match (lat, lon) {
        (Some(la), Some(lo)) => match alt {
            Some(a) => format!("{la:.2}, {lo:.2}, {a:.0} m"),
            None => format!("{la:.2}, {lo:.2}"),
        },
        _ => "not set".to_owned(),
    }
}
```

Tests: `signal_state_*` (no-signal/live/stale/labels) and `fmt_location_set`,
`fmt_location_unset`.

**`ble.rs`:** replace connect→resolve→notify with passive discovery +
manufacturer-data decode: `BleEvent` collapses to `Frame(Telemetry)`; remove
`State`/`Firmware`, `emit`, `find_char`, `read_fw_version`,
`wait_services_resolved`, `pump_until_disconnect`; add `COMPANY_ID = 0xFFFF`, the
discovery loop, `emit_frame`, and the pure
`decode_frame(&HashMap<u16, Vec<u8>>) -> Option<Telemetry>` (company-id + length +
`Telemetry::decode`). Tests:

- `decode_frame_accepts_valid_company_payload` — build the valid case from
  `Telemetry { uptime_s: 7, ..Telemetry::empty() }.encode()` (38-byte v5) under
  company `0xFFFF`; assert `Some` **and** the decoded `uptime_s == 7` (explicit
  coverage for the dedup-defeating field).
- `decode_frame_rejects_wrong_company` — payload under a different company id → `None`.
- `decode_frame_rejects_wrong_length` — `0xFFFF -> vec![0u8; 10]` → `None`.

**Dependencies:** substep 1.

### 7. TUI app + UI + main wiring (host)

**Files:** `crates/meteo-tui/src/app.rs`, `ui.rs`, `main.rs`

**`app.rs`:** drop `conn`/`fw_version` fields + their init; `apply` handles only
`BleEvent::Frame`; add `signal_state(&self, now) -> SignalState` (delegates to
`SignalState::from_age(self.last_frame_at, now, STALE_AFTER)`). Keep series/staleness.

**`ui.rs`:** `render_header` gains `now: Instant`, renders `app.app_version` only
(drop fw), and colours/labels the status from `app.signal_state(now)`
(Live=green, Stale=yellow, NoSignal=red). Add a **Location** row to the telemetry
table: `("Location", model::fmt_location(t.latitude_deg, t.longitude_deg, t.altitude_m))`
(table area is `Length(13)`; one extra row fits, or bump to 14 — confirm with the
render smoke test).

**`main.rs`:** unchanged except that the `BleEvent` it forwards now only carries
`Frame`.

**Tests:** `app.rs` — keep frame/series reducers; drop `apply_state_*`/
`apply_firmware_*`; add `signal_state_transitions`. `ui.rs` — update
`render_smoke_*` (drop `State(Live)`; assert `"Live"` from the signal state after a
fresh frame, `"app v"`, `"time"`, `"Diagnostics"`, `"Sky temperature"`, `"OK"`,
and `"Location"`); keep the baro-fault render test (drop its `State` line).

**Dependencies:** substeps 1, 6.

### 8. Scripts: location write + broadcast check (host/gaia)

**Files:** `scripts/ble_set_location.sh` (new), `scripts/ble_notify_check.sh` →
`scripts/ble_broadcast_check.sh` (rename+rewrite), `scripts/ble_soak.sh` (notes)

**`ble_set_location.sh` (new):** usage `ble_set_location.sh LAT LON [ALT_M]`
(decimal degrees, metres; ALT defaults to 0). It:

- Computes the coarse i16s with `awk`: `lat_c = round(LAT*100)`, `lon_c =
round(LON*100)`, `alt_m = round(ALT)`; range-checks (`|lat_c| ≤ 9000`,
  `|lon_c| ≤ 18000`).
- Packs the **10-byte authorized-write payload** as a 20-hex-digit string: the PIN
  (`PIN` env, default `911`) as `u32` LE (bytes 0–3), then the three `i16` LE coords
  (bytes 4–9). Signed→two's-complement and little-endian packing via `printf`/`awk`.
- Connects (reuse the soak's bounded-scan-then-connect-by-address pattern; **never**
  a blocking scan), finds the location characteristic handle for `7e700011-…`,
  and writes the 10 bytes via `gatttool --char-write-req` (or `bluetoothctl
gatt.write`). Confirms the write response, disconnects.
- Env knobs mirror the other scripts (`DEVICE`, `ADAPTER`, `CHAR_UUID`,
  `CONNECT_TIMEOUT`) plus `PIN` (default `911`). Exit 0 on a confirmed write,
  non-zero otherwise. A wrong PIN → the firmware rejects the write (ATT error) →
  non-zero exit.

**`ble_broadcast_check.sh` (rename + rewrite):** rename the file (jj tracks it),
rewrite the data-flow check for broadcast: one bounded self-terminating
`bluetoothctl --timeout WINDOW scan on &`; in `python3`/`dbus`, count
`ManufacturerData` updates whose `0xFFFF` entry is `FRAME_LEN` bytes with
`byte[0] == FRAME_VERSION`; seed from the initial `ManufacturerData` read. New env
defaults `FRAME_LEN=38`, `FRAME_VERSION=5`, `COMPANY_ID=0xFFFF`; assert
`≥ MIN_FRAMES` distinct frames (distinctness from per-second `uptime_s`); same
`case "$rc"` reporting. Update the header: the dedup rationale now points at
`manufacturer_data`, defeated by `uptime_s` (not worked around with
`AcquireNotify`).

**`ble_soak.sh` (notes only):** header update — meteo data is broadcast; the
connect/hold/disconnect/reconnect cycle now validates the **location-config
channel** and the retained 8 s supervision negotiation; broadcast continuity is
checked by `ble_broadcast_check.sh`. No logic change.

**Checks:** `bash -n` + ShellCheck on both new/renamed scripts. On-device PASS is
the real gate.

**Dependencies:** substeps 1, 5 (frame v5 + the write characteristic exist).

### 9. Docs: `CLAUDE.md` (docs)

**File:** `CLAUDE.md`

- Intro/feature bullet: BLE telemetry is now **broadcast** (extended connectable
  advertising, manufacturer-data frame at 1 Hz). Add a **GPS location** line:
  set over the connectable channel, persisted to flash, broadcast at ~1 km.
- **GATT layout** table: replace the telemetry service/characteristic with the
  reserved service (`7e700010-…`) + its PIN-gated location write characteristic
  (`7e700011-…`, Read+Write, 10-byte PIN + coarse-location write payload). Remove
  the DIS firmware-version paragraph and the "DIS firmware-version transport" TUI
  subsection.
- **Frame**: 38-byte v5 (was 28-byte v4); add `uptime_s` (u32, 28–31) and the three
  coarse location fields (i16, 32–37, deg×100 / m, sentinel i16::MIN); version
  sentinel `0x05`.
- **New "Location config" subsection**: the write path (`ble_set_location.sh` →
  GATT write of `PIN + coords` → `parse_authorized_write` (PIN check, then
  `Location::from_wire`) → `LOCATION_WRITE` signal → `config_task` flash persist via
  esp-storage/sequential-storage → republish to the aggregator → broadcast); the
  **PIN gate** (`CONFIG_PIN = 000911`, compile-time) and its **security caveat** —
  an app-level gate, not BLE pairing: the PIN travels in cleartext on the one-time
  config connection, chosen deliberately because real SMP pairing is unverified on
  the esp-radio H2 controller and this is a low-value device (link the two research
  findings); the radio-safe flash-write note (critical-section cache pause < 10 ms
  tolerated by the 8 s supervision timeout); the coarse ~1 km privacy property (the
  station never holds a precise fix); the flash-region source (nvs partition or
  custom `partitions.csv`).
- **Module structure** comments: add `config.rs`; update `ble.rs` (broadcast +
  location-write reserved service) and `frame.rs` (v5 38-byte).
- **Dashboard** section: passive-scan + manufacturer-data decode; `SignalState`
  (No signal → Live → Stale) replacing the connection state machine; firmware
  version display dropped; Location row added.
- **Acceptance gate**: rename `ble_notify_check.sh` → `ble_broadcast_check.sh`
  everywhere; add `ble_set_location.sh`; update `ble_soak.sh` description. Keep the
  methodology traps (no blocking scan; query link state via `busctl`).

**Dependencies:** substeps 1–8.

## Testing

- **Host unit tests (`just test` / `cargo nextest`)** for `meteo-lib` + `meteo-tui`:
  - Frame: 38-byte length, v5 sentinel, `uptime_s` round-trip, coarse-location
    round-trip within LSB + sentinel→None, extended proptests (u32 uptime;
    location within tolerance).
  - `Location`: `from_wire`/`to_wire` round-trip, wrong-length/sentinel/out-of-range
    rejection.
  - `parse_authorized_write`: correct-PIN accept, bad-PIN reject, wrong-length
    reject, and `Location` error propagation.
  - Aggregator: `SensorReading::Location` sets the three fields.
  - TUI model: `SignalState::from_age`/`label`, `fmt_location` set/unset.
  - TUI ble: `decode_frame` accept/reject.
  - TUI app: `signal_state` transitions; frame/series reducers.
  - TUI ui: render smoke (buffer contains `Live`, `app v`, `time`, `Diagnostics`,
    `Sky temperature`, `Location`, `OK`) + baro-fault render.
- **Firmware build (`just build`)**: compiles for `riscv32imac` with the broadcast
  loop, the writable reserved service (`ATT_MAX = 9`), the `config_task`, and the
  new `esp-storage`/`sequential-storage` deps.
- **Lint/format (`just clippy`, `just format`)**: clean across all crates,
  including the new `#[expect]` casts.
- **Edge cases:** `uptime_s` u64→u32 cast (no realistic truncation); coarse-location
  clamp away from `i16::MIN`; flash write failure → warn + still republish; invalid
  GATT write → reject with an ATT error; `adv_buf` 64 B holds Flags+name+38 B frame.
- **On-device acceptance gate (manual, gaia):**
  - `scripts/ble_broadcast_check.sh` — ≥5 well-formed 38-byte v5 frames in 15 s from
    passive scanning.
  - `scripts/ble_set_location.sh 48.85 2.35 35` — write accepted; the coarse coords
    appear in the broadcast within ~1 s; **reboot the device and confirm they
    persist** (flash round-trip).
  - `PIN=1 scripts/ble_set_location.sh 48.85 2.35 35` — **wrong PIN** → firmware
    rejects the write (ATT error) → script exits non-zero; broadcast coords unchanged.
  - `scripts/ble_soak.sh` — connect to the config channel, hold 6 min, disconnect,
    reconnect; `supervision_ms=8000` still negotiated; broadcast resumes after
    disconnect.

## Risks

- **Advertiser/peripheral borrow model.** The broadcast loop relies on
  `advertise_ext` returning an `Advertiser` bound to the stack (`'d`), leaving
  `peripheral` free for `update_adv_data_ext`/`try_accept`. Confirmed by the API
  map; fallback is to re-check lifetimes in `third_party/trouble-host/src/peripheral.rs`.
- **Flash write while BLE is live.** Researched as safe: `esp-storage` wraps the ROM
  op in a critical section (cache off < 10 ms), and the 8 s supervision timeout
  absorbs the pause — a disconnect is _not_ expected. Mitigation: flash I/O is
  isolated in `config_task` (never on the BLE task), writes are rare (config only),
  and a failed write degrades to a warn while the broadcast still updates. If a
  drop is ever observed during a write, that is a finding to diagnose with `btmon`,
  not a reason to amputate persistence.
- **sequential-storage / partition API drift.** The exact 7.x map signatures and the
  `esp-bootloader-esp-idf` 0.5 partition API are pinned-version-sensitive (researcher
  medium-confidence). Substep 4 lists explicit build-time verifications and a
  `partitions.csv` fallback for the flash range. Caught at `just build`.
- **GATT write event surface.** The write-handling arms (`GattEvent::Write` plus
  exhaustive `Read`/`Other`/`NotAllowed`, `AttErrorCode` variant, `Reply::send`)
  must match the vendored trouble-host exactly; substep 5 flags verifying every name.
- **PIN gate is app-level, not cryptographic.** `CONFIG_PIN` and the coordinates
  travel in cleartext over the one-time config connection — sniffable by anyone
  listening during that write, and the PIN lives in the firmware/script/docs. This
  is a deliberate, documented trade-off: real SMP passkey pairing exists in the
  vendored trouble-host but is **unverified on the esp-radio H2 controller** (two
  research findings), and this is a low-value device. The gate blocks
  casual/accidental writes, nothing more. Escalation path if real auth is ever
  needed: the `security`-feature SMP route with a fixed-passkey vendored patch —
  scoped out here.
- **Empty/odd GATT table sizing.** The reserved service now _has_ a characteristic,
  so `ATT_MAX = 9` (no empty-service concern). Verified at firmware init (the table
  asserts its size).
- **Watchdog starvation while connected.** Broadcast pauses during a connection, so
  `connection_heartbeat` bumps `BLE_BEAT` each second; the `select(gatt_events,
connection_heartbeat)` covers it.
- **BlueZ manufacturer-data dedup.** Defeated by the per-second `uptime_s` change;
  the broadcast check asserts ≥5 frames/15 s to catch a regression.
- **Extended-advertising controller support.** The ESP32-H2 controller must accept
  `LeSetExtAdv*`; first on-device smoke (`ble_broadcast_check.sh`) is the gate. If
  unsupported, that is a hardware/controller finding to surface, not a silent
  fallback to legacy advertising.
- **Coordinate precision.** Coarse `i16` deg×100 is enforced in `Location::from_wire`,
  `to_wire`, and the frame encode — the station cannot represent or broadcast finer
  than ~1 km, satisfying the privacy requirement by construction.
- **No host test for firmware BLE/flash.** Correctness rests on the host-tested
  frame + `Location` round-trips plus the on-device acceptance scripts.

## Notes

Progress tracking (checked off during `/tyrex:code:implement-light`):

- [x] 1. Frame v5: `uptime_s` + coarse location (meteo-lib) — host tests green. v5/38-byte; `scale_loc_i16`/`decode_loc`; 5 new tests. Spec review pass.
- [x] 2. `Location` wire type + `parse_authorized_write` (PIN) + `SensorReading::Location` (meteo-lib) — host tests green. `location.rs` (357 lines), re-exported; 9 tests. Spec review pass.
- [x] 3. Aggregator uptime stamp (firmware) — `just build` green; `Instant::now().as_secs() as u32` stamped before `TELEMETRY.signal`. Spec verified inline.
- [x] 4. Flash config module + main wiring (firmware) — `just build` green. `config.rs` MapStorage (sequential-storage **7.2 API differs from plan sketch — uses `MapStorage` struct, not free fns**); NVS partition lookup; deps added; **workspace rust-version → 1.96** (deps need ≥1.88); fixed a `config.rs` `shadow_reuse` clippy. Spec review pass.
- [x] 5. Firmware broadcast loop + PIN-gated writable reserved service — `just build` green. `advertise_ext`/`update_adv_data_ext`, mfg-data company 0xFFFF; PIN-gated location write; DIS/notify removed; heartbeat. trouble-host APIs confirmed vs vendored source (`try_accept` → Option; GattEvent 4 variants). Spec review pass.
- [x] 6. TUI model (`SignalState`, `fmt_location`) + passive-scan ble.rs — host tests green. Additive (old items kept for substep 7); `decode_frame`; 10 tests. Spec review pass.
- [x] 7. TUI app/ui/main wiring + Location row — render smoke green. Passive-scan migration; old ConnState/LinkEvent/DIS path removed; Location row (Length 14); main.rs already pre-wired `now`. Spec review pass.
- [x] 8. Scripts: `ble_set_location.sh` + `ble_broadcast_check.sh` rename + `ble_soak.sh` notes — `bash -n` + ShellCheck clean; python3+dbus GATT write + manufacturer-data polling. Spec review pass.
- [x] 9. Docs: CLAUDE.md (broadcast, location config, frame v5, GATT, TUI) — broadcast/location/v5/passive-scan/scripts reconciled. Spec verified inline.
- [x] `just clippy`, `just format`, `just test`, `just build` all clean before finalize — 164 tests pass, clippy clean, fmt clean, firmware build OK. Post-implementation review: pass (9/9 substantive).
