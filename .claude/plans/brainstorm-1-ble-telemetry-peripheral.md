# Plan: BLE Telemetry Peripheral (ESP32-H2 on-chip)

- **Source:** '1 (`.claude/brainstorm/1-ble-module-design.md`)
- **Date:** 2026-06-16
- **Status:** Planned

## Summary

Bring up the on-chip BLE 5.3 peripheral on the ESP32-H2 using **esp-radio +
trouble-host**, replacing the retired RN4871 external-module path. The firmware
advertises connectably, accepts one central, and pushes a self-describing 17-byte
telemetry frame at 1 Hz over a single custom GATT service / Notify characteristic.
The wire frame lives in `meteo-lib` (hardware-agnostic, host-tested with encode +
decode + property roundtrip). An RWDT-backed heartbeat supervisor resets the chip
if the BLE or sampling task wedges. The vestigial RN4871 parser is removed and
`CLAUDE.md` is re-grounded on the on-chip model. Acceptance is the existing gaia
`scripts/ble_soak.sh` link-stability loop plus a new minimal notify-check.

The Linux central (`meteo-tui`) is **deferred to a follow-up plan** (decision
2026-06-16). The frame `decode()` and `FrameError` are still built and host-tested
here so the future central reuses them unchanged.

## Scope

**In scope (this plan):**

1. Remove the vestigial RN4871 driver from `meteo-lib`.
2. Telemetry wire frame v1 (`meteo-lib/src/ble/frame.rs`): `Telemetry`, `encode`,
   `decode`, `FrameError`, `Telemetry::from_bmp388`, full host tests.
3. Firmware BLE stack bring-up: deps, heap, controller, connectable advertising,
   re-advertise-on-disconnect, fixed static random address.
4. GATT telemetry service + 1 Hz Notify, BMP388 → `Telemetry` fan-in via a Signal.
5. RWDT firmware-hang backstop with a heartbeat supervisor.
6. Acceptance tooling + docs: notify-check script, `ble_soak.sh` address update,
   `CLAUDE.md` on-chip rewrite.

**Out of scope (deferred / per brainstorm):** the `meteo-tui` central and its UI;
pairing/bonding/encryption; per-sensor hardware bring-up for the six not-yet-sampled
values (the frame _carries_ them via sentinels, acquisition is separate); updating
brainstorm 2; 802.15.4/Thread.

## Files Modified

| File                                    | Action | Description                                                                                                               |
| --------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------- |
| `crates/meteo-lib/src/ble/rn4871.rs`    | delete | Remove RN4871 ASCII parser (1053 lines) + its host tests                                                                  |
| `crates/meteo-lib/src/ble/mod.rs`       | modify | Drop `rn4871`; declare `pub mod frame;`                                                                                   |
| `crates/meteo-lib/src/ble/frame.rs`     | create | Telemetry wire frame v1: encode/decode/sentinels/tests                                                                    |
| `crates/meteo-lib/Cargo.toml`           | modify | Add `proptest` dev-dependency                                                                                             |
| `crates/meteo-lib/src/lib.rs`           | modify | Re-export `ble::frame` types                                                                                              |
| `Cargo.toml` (workspace)                | modify | Add esp-radio, esp-alloc, trouble-host, embassy-sync, embassy-futures; extend esp-rtos features; optional `[patch]` block |
| `crates/meteo-firmware/Cargo.toml`      | modify | Add the new target-gated BLE deps                                                                                         |
| `crates/meteo-firmware/src/main.rs`     | modify | Heap, RWDT, spawn BLE task; pass telemetry Signal to bmp task                                                             |
| `crates/meteo-firmware/src/ble.rs`      | create | BLE stack: controller, GATT server, advertise loop, notify                                                                |
| `crates/meteo-firmware/src/bmp.rs`      | modify | Publish `Telemetry` to the Signal; bump a heartbeat                                                                       |
| `crates/meteo-firmware/src/watchdog.rs` | create | RWDT heartbeat supervisor task                                                                                            |
| `scripts/ble_notify_check.sh`           | create | gaia-side notify-flow check                                                                                               |
| `scripts/ble_soak.sh`                   | modify | Update default `DEVICE` to the H2 static random address                                                                   |
| `CLAUDE.md`                             | modify | Re-ground the BLE section on the on-chip model                                                                            |

## Verified dependency matrix (research 2026-06-16)

Canonical known-good combo from `embassy-rs/trouble` `examples/esp32`:

| Crate           | Version                        | Features                                        |
| --------------- | ------------------------------ | ----------------------------------------------- |
| esp-hal         | 1.1.0                          | `esp32h2, unstable, defmt` (unchanged)          |
| esp-rtos        | 0.3.0                          | `esp32h2, embassy` **+ `esp-alloc, esp-radio`** |
| esp-radio       | 0.18.0                         | `esp32h2, ble, unstable` (+ `defmt`)            |
| esp-alloc       | 0.10.0                         | —                                               |
| trouble-host    | 0.6.0                          | `default-packet-pool-mtu-255`                   |
| embassy-sync    | 0.8.0 (matches esp-rtos 0.3.0) | `defmt`                                         |
| embassy-futures | 0.1                            | —                                               |
| bt-hci          | transitive (do not pin)        | —                                               |

> **CRITICAL RISK — `[patch]` requirement.** The upstream example pins
> `embassy-rs/esp-hal` rev `b7eec0f` and `embassy-rs/embassy` rev `1d3c3de` rather
> than building against crates.io verbatim. Substep 3 must first confirm a
> crates.io-only resolve; if `cargo build` fails to resolve, replicate the upstream
> `[patch.crates-io]` block (documented inline). Do not assume clean resolution.

## Plan

### 1. Remove the vestigial RN4871 driver

**Goal:** delete RN4871-specific dead code so the `ble` module is a clean home for
the telemetry frame. Per brainstorm "BLE mess to clean up." Each step compiles.

**Files:**

- Delete `crates/meteo-lib/src/ble/rn4871.rs` (use `trash put`).
- Rewrite `crates/meteo-lib/src/ble/mod.rs` to:

  ```rust
  //! BLE telemetry support: the self-describing wire frame pushed over the
  //! on-chip BLE Notify characteristic. (The RN4871 external-module parser was
  //! removed in the ESP32-H2 port — on-chip BLE replaces it.)
  ```

  (Leave it with only the doc comment in this substep — `pub mod frame;` is added
  in substep 2 so each changeset compiles. An empty declared module is valid.)

- `crates/meteo-lib/src/lib.rs`: `pub mod ble;` stays. No re-export to remove (the
  current `lib.rs` only does `pub mod ble;` plus sensor/util re-exports).

**Verify:**

```bash
cargo build -p meteo-lib --target x86_64-unknown-linux-gnu
cargo nextest run -p meteo-lib --target x86_64-unknown-linux-gnu   # rn4871 tests gone, rest pass
```

**Tests:** removal only — no new tests. The remaining `utils`/`bmp388` tests must
still pass (proves nothing else depended on `rn4871`).

**Depends on:** none. **Blocks:** substep 2 (shares `ble/mod.rs`).

---

### 2. Telemetry wire frame v1 (`meteo-lib`)

**Goal:** the host-testable, hardware-agnostic 17-byte v1 frame. Full 8-field
schema; `encode` fills available fields and writes a defined sentinel for the rest;
`decode` (reused later by the central) maps sentinels back to `None` and rejects
bad version/length.

**File:** create `crates/meteo-lib/src/ble/frame.rs`; add `pub mod frame;` to
`ble/mod.rs`; re-export from `lib.rs`:

```rust
// lib.rs
pub use ble::frame::{FRAME_LEN, FRAME_VERSION, FrameError, Telemetry};
```

**Constants & layout** (little-endian for multi-byte fields — documented; the
central must match):

```rust
pub const FRAME_VERSION: u8 = 1;
pub const FRAME_LEN: usize = 17;
```

| Off   | Field               | Wire type | Encoding                     | Sentinel (`None`)                |
| ----- | ------------------- | --------- | ---------------------------- | -------------------------------- |
| 0     | version             | `u8`      | `FRAME_VERSION` (=1)         | —                                |
| 1–2   | temperature         | `i16` LE  | `round(°C × 100)` centi-°C   | `i16::MIN`                       |
| 3–4   | pressure            | `u16` LE  | `round(hPa × 10)` deci-hPa   | `u16::MAX`                       |
| 5–6   | humidity            | `u16` LE  | `round(%RH × 100)` centi-%RH | `u16::MAX`                       |
| 7–8   | sky/IR temp         | `i16` LE  | centi-°C                     | `i16::MIN`                       |
| 9–10  | luminosity mantissa | `u16` LE  | see lux note                 | `u16::MAX`                       |
| 11    | luminosity exponent | `u8`      | see lux note                 | (paired: mantissa==MAX ⇒ `None`) |
| 12–13 | wind speed          | `u16` LE  | `round(m/s × 100)` cm/s      | `u16::MAX`                       |
| 14–15 | wind direction      | `u16` LE  | `round(deg × 10)` deci-deg   | `u16::MAX`                       |
| 16    | battery             | `u8`      | percent 0..=100              | `0xFF`                           |

**Lux note:** encode as `mantissa × 10^exponent`. Pick the smallest `exponent ≥ 0`
such that `round(lux / 10^exponent) ≤ 65534`; `mantissa = round(lux / 10^exponent)`.
Decode = `mantissa as f32 × 10f32.powi(exponent as i32)`.

**Public API (signatures):**

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Telemetry {
    pub temperature_c: Option<f32>,
    pub pressure_hpa: Option<f32>,
    pub humidity_pct: Option<f32>,
    pub sky_temp_c: Option<f32>,
    pub luminosity_lux: Option<f32>,
    pub wind_speed_ms: Option<f32>,
    pub wind_dir_deg: Option<f32>,
    pub battery_pct: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    WrongLength(usize),
    UnknownVersion(u8),
}

impl Telemetry {
    #[must_use]
    pub const fn empty() -> Self { /* all None */ }

    /// Map a BMP388 reading onto the frame (temperature + pressure only).
    #[must_use]
    pub fn from_bmp388(reading: &crate::sensors::bmp388::Reading) -> Self;

    /// Serialize to the fixed 17-byte v1 wire frame.
    #[must_use]
    pub fn encode(&self) -> [u8; FRAME_LEN];

    /// Parse a v1 wire frame; sentinels decode back to `None`.
    ///
    /// # Errors
    /// `FrameError::WrongLength` if not exactly 17 bytes;
    /// `FrameError::UnknownVersion` if byte 0 != `FRAME_VERSION`.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError>;
}
```

`#[cfg_attr(feature = "defmt", derive(defmt::Format))]` on `Telemetry` and
`FrameError` (matches the optional-`defmt` convention).

**Code sketch (encode, per-field helper):**

```rust
pub fn encode(&self) -> [u8; FRAME_LEN] {
    let mut b = [0u8; FRAME_LEN];
    b[0] = FRAME_VERSION;
    let t = self.temperature_c.map_or(i16::MIN, |v| scale_i16(v, 100.0));
    b[1..3].copy_from_slice(&t.to_le_bytes());
    let p = self.pressure_hpa.map_or(u16::MAX, |v| scale_u16(v, 10.0));
    b[3..5].copy_from_slice(&p.to_le_bytes());
    let h = self.humidity_pct.map_or(u16::MAX, |v| scale_u16(v, 100.0));
    b[5..7].copy_from_slice(&h.to_le_bytes());
    let s = self.sky_temp_c.map_or(i16::MIN, |v| scale_i16(v, 100.0));
    b[7..9].copy_from_slice(&s.to_le_bytes());
    let (mant, exp) = self.luminosity_lux.map_or((u16::MAX, 0u8), encode_lux);
    b[9..11].copy_from_slice(&mant.to_le_bytes());
    b[11] = exp;
    let w = self.wind_speed_ms.map_or(u16::MAX, |v| scale_u16(v, 100.0));
    b[12..14].copy_from_slice(&w.to_le_bytes());
    let d = self.wind_dir_deg.map_or(u16::MAX, |v| scale_u16(v, 10.0));
    b[14..16].copy_from_slice(&d.to_le_bytes());
    b[16] = self.battery_pct.unwrap_or(0xFF);
    b
}

/// lux → (mantissa, exponent) with mantissa·10^exp ≈ lux, mantissa ≤ 65534.
fn encode_lux(lux: f32) -> (u16, u8) {
    let mut exp = 0u8;
    let mut m = libm::roundf(lux.max(0.0));
    while m > 65_534.0 {
        exp += 1;
        m = libm::roundf(lux / libm::powf(10.0, f32::from(exp)));
    }
    (m as u16, exp)   // as-cast guarded: m ≤ 65534 here
}
```

`scale_i16(v, k)` / `scale_u16(v, k)` round `v*k` (`libm::roundf`) and clamp into
range; the clamp must keep one value reserved so a real reading can never produce
the sentinel (`i16::MIN` / `u16::MAX`). All scaled fixed-point integers are
< 2^16 < 2^24, so they are exactly representable as `f32` (see the proptest note).

**Cargo:** add to `crates/meteo-lib/Cargo.toml` `[dev-dependencies]`:

```toml
proptest = "1"
```

(Confirm current 1.x at implementation time.)

**Tests** (`mod tests` in `frame.rs`, house Given/When/Then + `TestResult`):

- `from_bmp388_sets_temperature_and_pressure_only` — other six fields `None`.
- `encode_emits_seventeen_bytes_with_version_one` — `len == 17`, `b[0] == 1`.
- `encode_writes_sentinels_for_none_fields` — temp bytes == `i16::MIN` LE, battery
  == `0xFF`, etc.
- `encode_scales_temperature_and_pressure` — `Telemetry{temp:Some(23.45),
pressure:Some(1013.2),..}` ⇒ exact expected bytes (`2345` LE, `10132` LE).
- `decode_rejects_wrong_length` — `decode(&[0u8;16])` ⇒ `Err(WrongLength(16))`.
- `decode_rejects_unknown_version` — byte0=2 ⇒ `Err(UnknownVersion(2))`.
- `decode_maps_sentinels_back_to_none` — sentinel frame ⇒ all `None`.
- `decode_recovers_scaled_values` — known bytes ⇒ values within scale tolerance.
- `encode_lux_large_value_uses_nonzero_exponent` — `Some(120_000.0)` ⇒ exponent ≥ 1
  and `mantissa·10^exp` within tolerance of 120000 (the only field whose encode
  branches).
- `encode_lux_zero_emits_zero_mantissa_zero_exponent` — `Some(0.0)` ⇒ mantissa 0,
  exponent 0, and `decode` recovers `Some(0.0)` (explicit zero-boundary case).
- proptest `roundtrip_decode_encode_is_identity_at_wire_level` — generate arbitrary
  `[u8;17]` with `bytes[0]=1`, `decode` then `encode`, assert bytes **equal**.
  This bit-exact assertion holds because every fixed-point field is an integer
  < 2^16 (exactly `f32`-representable), so `int → f32 → roundf(·×scale)` is the
  identity — **except** the lux mantissa/exponent pair, where re-encoding can pick a
  different exponent for the same value. Exclude lux from the bit-exact arm: in the
  generator, force `bytes[9..12]` to the lux sentinel (`mantissa = u16::MAX`) so the
  lux field is `None` on both sides. The lux roundtrip is covered separately:
- proptest `lux_roundtrip_preserves_value_within_tolerance` — generate `lux` in
  `0.0..=120_000.0`, `decode(encode(Telemetry{luminosity_lux: Some(lux), ..empty}))`
  recovers a value within `0.5%` (the mantissa/exponent quantization bound).

**Verify:**

```bash
cargo nextest run -p meteo-lib --target x86_64-unknown-linux-gnu
cargo clippy -p meteo-lib --all-features --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

**Depends on:** substep 1. **Blocks:** substep 4.

---

### 3. Firmware BLE stack bring-up (advertising, no GATT yet)

**Goal:** prove the riskiest unknowns first — dependency resolution, esp-radio
controller init, and connectable advertising that re-advertises on disconnect —
_before_ layering GATT. No telemetry yet.

**Step 3a — dependencies (verify resolution first):**

Workspace `Cargo.toml` `[workspace.dependencies]`:

```toml
esp-radio = { version = "0.18", features = ["esp32h2", "ble", "unstable", "defmt"] }
esp-alloc = "0.10"
trouble-host = { version = "0.6", default-features = false, features = ["default-packet-pool-mtu-255"] }
embassy-sync = { version = "0.8", features = ["defmt"] }   # matches esp-rtos 0.3.0 (Cargo.lock)
embassy-futures = "0.1"
```

Extend the existing `esp-rtos` entry features to add `"esp-alloc", "esp-radio"`.

`crates/meteo-firmware/Cargo.toml` `[target.'cfg(target_arch = "riscv32")'.dependencies]`:
add `esp-radio`, `esp-alloc`, `trouble-host`, `embassy-sync`, `embassy-futures`
(all `{ workspace = true }`).

Then **immediately**:

```bash
cargo build --release -p meteo-firmware    # confirms the crates.io combo resolves
```

If resolution fails, add to the workspace `Cargo.toml`:

```toml
# Upstream trouble esp32 example pins these git revs; crates.io 1.1.0/0.x did not
# resolve the esp-radio 0.18 / trouble 0.6 / esp-rtos 0.3 combo on its own.
[patch.crates-io]
esp-hal = { git = "https://github.com/esp-rs/esp-hal", rev = "b7eec0f" }
# embassy crates as needed, rev = "1d3c3de"
```

Re-run the build until it resolves. **Record in the plan Notes which path worked.**

**Step 3b — `main.rs` init reordering + heap.** Per the verified example, the
order is: `esp_hal::init` → `esp_alloc::heap_allocator!` → `esp_rtos::start` →
create `BleConnector`. Add near the top of `main`:

```rust
esp_alloc::heap_allocator!(size: 72 * 1024);   // BLE stack heap; H2 has 320 KiB SRAM
```

(Placed after `esp_hal::init`, before `esp_rtos::start`.)

**Step 3c — `crates/meteo-firmware/src/ble.rs` (new), advertising only:**

```rust
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_futures::join::join;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

/// Fixed BLE static random address for the weather station (top two bits of the
/// MSB are 1 → random-static per BLE spec). Keep in sync with scripts/ble_soak.sh.
const STATION_ADDR: [u8; 6] = [0xF0, 0xCA, 0xFE, 0x00, 0x00, 0x01];
const STATION_NAME: &str = "MeteoStation";

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

/// Concrete controller type, fixed once so the BLE task is `'static`-spawnable.
/// (`run` cannot itself be an `#[embassy_executor::task]` — those need concrete
/// `'static` args, and `trouble_host::new` is generic; we pin the type here.)
pub type Controller = ExternalController<BleConnector<'static>, 20>;

/// Bumped every advertise-loop iteration; proves the GAP loop is cycling even with
/// no central connected (read by the RWDT supervisor, substep 5).
pub static ADV_BEAT: AtomicU32 = AtomicU32::new(0);

pub async fn run(controller: Controller) {
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>
        = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(STATION_ADDR));
    let Host { mut peripheral, runner, .. } = stack.build();   // confirm destructure shape

    join(ble_runner(runner), advertise_loop(&mut peripheral)).await;
}

async fn ble_runner(mut runner: Runner<'_, Controller, DefaultPacketPool>) {
    runner.run().await.expect("BLE runner exited");   // confirm error type
}

async fn advertise_loop(peripheral: &mut Peripheral<'_, Controller, DefaultPacketPool>) {
    let mut adv = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(STATION_NAME.as_bytes()),
            // 128-bit service UUID AD added in substep 4
        ],
        &mut adv,
    ).expect("adv data");

    loop {
        ADV_BEAT.fetch_add(1, Ordering::Relaxed);
        let advertiser = peripheral.advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected { adv_data: &adv[..len], scan_data: &[] },
        ).await.expect("advertise");
        let conn = advertiser.accept().await.expect("accept");
        // substep 3: just hold until disconnect, then re-advertise
        let _ = conn.next().await;   // first event; substep 4 replaces with full GATT loop
    }
}
```

`main.rs` builds the controller (concrete types are in scope there) and spawns a
thin `'static` wrapper task — this is how the generic `run` reaches the executor:

```rust
// main.rs
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::ExternalController;

#[embassy_executor::task]
async fn ble_task(controller: ble::Controller) {
    ble::run(controller).await;
}

// in main(), after esp_rtos::start:
let connector = BleConnector::new(peripherals.BT, Default::default()).expect("BLE controller");
//                                                 ^ upstream example uses `Default::default()`
//                                                   verbatim — BleConnectorConfig: Default holds.
let controller: ble::Controller = ExternalController::new(connector);
spawner.spawn(ble_task(controller)).expect("ble_task already spawned");
```

Confirm the exact `Host`/`Runner`/`Peripheral` type names and the `.build()` /
`set_random_address` builder shape against trouble-host 0.6 `prelude` at
implementation; the type _names_ may differ, but pinning `ble::Controller` to a
concrete type is the load-bearing decision and is fixed here.

The `ble` module will likely need module-level `#![expect(...)]` for restriction
lints tripped by trouble macro expansion (mirror `main.rs`/`bmp.rs` precedent);
add only the ones clippy actually flags, each with a `reason`.

**Verify:**

```bash
cargo build --release -p meteo-firmware
cargo clippy -p meteo-firmware -- -D warnings
just run            # flash; defmt should log controller init + "advertising"
```

On-device (gaia): the device shows up in blueman's discovery cache at
`F0:CA:FE:00:00:01` / name `MeteoStation`; `bluetoothctl connect` then
`disconnect` succeeds and the device re-advertises (reconnect works). **Do not run a
blocking scan** (CLAUDE.md trap).

**Tests:** firmware/hardware — no host unit tests (per CLAUDE.md, hardware code is
not auto-tested). Gate = build + clippy + on-device connect/reconnect.

**Depends on:** none (parallel to 1–2). **Blocks:** substep 4, 5.

---

### 4. GATT telemetry service + 1 Hz Notify

**Goal:** one custom 128-bit service with one Notify characteristic; the BMP388
sample becomes a `Telemetry`, encodes to 17 bytes, and is pushed via `notify()` at
1 Hz; re-advertise on disconnect carries over from substep 3.

**UUIDs (chosen):**

- Service: `7e700001-b1df-42a1-bb5f-6a1028c793b0`
- Telemetry characteristic: `7e700002-b1df-42a1-bb5f-6a1028c793b0`

**GATT server (`ble.rs`):**

```rust
#[gatt_server]
struct Server {
    meteo: MeteoService,
}

#[gatt_service(uuid = "7e700001-b1df-42a1-bb5f-6a1028c793b0")]
struct MeteoService {
    // value type is the encoded frame; verify trouble-host 0.6 AsGatt/FromGatt
    // impl for [u8; 17] — if absent, wrap in a newtype implementing the trait.
    #[characteristic(uuid = "7e700002-b1df-42a1-bb5f-6a1028c793b0", read, notify, value = [0u8; 17])]
    telemetry: [u8; 17],
}
```

**Sensor → BLE fan-in (latest-wins):** a static Signal carrying the newest
`Telemetry`:

```rust
// ble.rs (or a small shared module)
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use meteo_lib::Telemetry;

pub static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();
```

`bmp.rs`: after each successful `sensor.read()`, publish and beat the heartbeat:

```rust
let telem = Telemetry::from_bmp388(&reading);
crate::ble::TELEMETRY.signal(telem);
crate::watchdog::BMP_BEAT.fetch_add(1, Ordering::Relaxed);   // substep 5
```

(Keep the existing `Timer::after(Duration::from_secs(1))` — that is the _specified_
1 Hz sample cadence, not a synchronization sleep.)

**Per-connection GATT + notify loop** (replaces the substep-3 placeholder):

```rust
let conn = advertiser.accept().await.expect("accept").with_attribute_server(&server).expect("attach");
select(
    gatt_events(&conn),                  // poll conn.next(); break on Disconnected
    notify_loop(&server, &conn),         // await TELEMETRY, encode, notify
).await;
// connection ended → outer loop re-advertises

async fn notify_loop(server: &Server<'_>, conn: &GattConnection<'_, '_, DefaultPacketPool>) {
    loop {
        let telem = TELEMETRY.wait().await;          // latest-wins; no backlog
        let frame = telem.encode();
        if server.meteo.telemetry.notify(conn, &frame).await.is_err() { break; }
        crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);   // substep 5
    }
}
```

> **Version note:** trouble-host 0.6 `notify(&conn, &value)` (2-arg). If the
> resolved lockfile is 0.5.x, use `notify(conn, &value, true)` (3-arg). Match the
> lockfile.

Add the 128-bit service UUID to the advertising `AdStructure` (use the 128-bit
service-UUID AD variant from `trouble_host::prelude`).

Build the server **once** at the top of `run`, before the advertise loop, and pass
`&server` into `advertise_loop` (which gains a `server: &Server<'_>` parameter vs
substep 3). Use the `#[gatt_server]`-generated `new_with_config` with a peripheral
GAP config that names the station:

```rust
let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
    name: STATION_NAME,
    appearance: &appearance::sensor::GENERIC_SENSOR,   // confirm exact appearance const path
})).expect("gatt server");
```

The decision is fixed: one `Server`, built once, configured as a named peripheral.
If the macro emitted a different constructor symbol in 0.6, it is a mechanical
rename — do not re-design (e.g. `Server::new_default(STATION_NAME)` is the
no-appearance fallback).

`main.rs`: spawn the BMP388 task (it now publishes to `TELEMETRY`) and the BLE task;
the BLE task owns the server + advertise loop.

**Verify:**

```bash
cargo build --release -p meteo-firmware
cargo clippy -p meteo-firmware -- -D warnings
just run     # defmt logs a notify each second once a central subscribes
```

On-device (gaia): the notify-check (substep 6) subscribes to
`7e700002-…` and observes ≥5 consecutive 17-byte frames with `byte[0]==1` within a
~10 s window; temperature/pressure bytes track the BMP388 readout, the other six
fields are sentinels.

**Tests:** the pure mapping `Telemetry::from_bmp388` is host-tested in substep 2.
The notify path is hardware — gated by build/clippy + the gaia notify-check.

**Depends on:** substeps 2 and 3. **Blocks:** substep 6 (notify-check).

---

### 5. RWDT firmware-hang backstop (heartbeat supervisor)

**Goal:** the "chip is its own supervisor" guarantee from the brainstorm. The RWDT
resets the whole chip if either the sampling or the BLE task stops making progress —
**without** false-resetting during the normal always-advertising-no-central state.

**Design:** each supervised task bumps a per-task heartbeat counter every iteration.
A watchdog supervisor task wakes on a fixed cadence (the watchdog poll — allowed as
a circuit-breaker mechanism), checks that _every_ heartbeat advanced since its last
look, and **only then feeds the RWDT**. If any task stalled, it stops feeding and
the RWDT fires. Disconnected-but-advertising still beats `BLE_BEAT` because the
advertise loop keeps cycling; an actually-wedged task does not.

**File:** create `crates/meteo-firmware/src/watchdog.rs`:

Liveness is computed from three heartbeats: `BMP_BEAT` (substep 4, sampler loop),
`BLE_BEAT` (substep 4, successful notify — only ticks while connected), and
`ADV_BEAT` (substep 3c, `ble::ADV_BEAT`, every advertise-loop iteration). BLE is
alive when **either** the advertise loop is cycling **or** notifies are flowing, so
an idle-but-advertising device (no central) is not falsely reset.

```rust
use core::sync::atomic::{AtomicU32, Ordering};
use defmt::{trace, warn};
use embassy_time::{Duration, Timer};
use esp_hal::rtc_cntl::{Rtc, RwdtStage, RwdtStageAction};
use esp_hal::time::Duration as HalDuration;

use crate::ble::ADV_BEAT;

pub static BMP_BEAT: AtomicU32 = AtomicU32::new(0);
pub static BLE_BEAT: AtomicU32 = AtomicU32::new(0);

#[embassy_executor::task]
pub async fn supervise(mut rtc: Rtc<'static>) {
    // RWDT timeout must exceed the longest legitimate gap between checks (2 s poll).
    rtc.rwdt.set_timeout(RwdtStage::Stage0, HalDuration::from_secs(8));
    rtc.rwdt
        .set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem); // see "confirm variant" below
    rtc.rwdt.enable();

    let (mut last_bmp, mut last_ble, mut last_adv) = (0u32, 0u32, 0u32);
    loop {
        Timer::after(Duration::from_secs(2)).await; // watchdog poll cadence
        let bmp = BMP_BEAT.load(Ordering::Relaxed);
        let ble = BLE_BEAT.load(Ordering::Relaxed);
        let adv = ADV_BEAT.load(Ordering::Relaxed);

        let sampler_alive = bmp != last_bmp;
        let ble_alive = adv != last_adv || ble != last_ble; // advertising OR notifying

        if sampler_alive && ble_alive {
            rtc.rwdt.feed();
            trace!("rwdt fed (bmp={=u32} adv={=u32} ble={=u32})", bmp, adv, ble);
        } else {
            // Withhold the feed: a stalled task lets the RWDT reset the chip.
            warn!(
                "rwdt withheld — sampler_alive={=bool} ble_alive={=bool}",
                sampler_alive, ble_alive
            );
        }
        (last_bmp, last_ble, last_adv) = (bmp, ble, adv);
    }
}
```

The `trace!`/`warn!` lines make the supervisor's decision observable over defmt
during the gaia soak, so a withheld feed is visible before the reset.

**`main.rs`:** `mod watchdog;`, `let rtc = Rtc::new(peripherals.LPWR);` (H2 uses
`LPWR`), then `spawner.spawn(watchdog::supervise(rtc))`.

**Verify:**

```bash
cargo build --release -p meteo-firmware
cargo clippy -p meteo-firmware -- -D warnings
just run     # normal operation: no spurious resets across a multi-minute idle (no central)
```

On-device sanity: leave it advertising with no central for several minutes → no
reset (proves the no-central case doesn't trip the dog). The wedge-recovery path
(a deliberately stalled task → reset within ~8 s) is a manual confirmation, noted
in the plan Notes, not an automated test.

**Tests:** hardware — no host tests. The `RwdtStageAction::ResetSystem` reference is
self-checking: a wrong variant name fails to compile. Before writing the loop,
confirm the variant set with
`rg -n 'enum RwdtStageAction' -A6 $(rustc --print sysroot 2>/dev/null; echo) ~/.cargo` or
read [docs.rs/esp-hal/1.1.0 `RwdtStageAction`](https://docs.rs/esp-hal/1.1.0/esp_hal/rtc_cntl/enum.RwdtStageAction.html);
if the chip-reset variant is named differently (`ResetRtc`/`ResetSystem`/…), use
that name — the chosen _action_ (reset the whole system on stage-0 timeout) is fixed.

**Depends on:** substep 3 (main structure); pairs with substep 4 heartbeats.
**Blocks:** none.

---

### 6. Acceptance tooling + docs

**Goal:** make the gaia gate exercise the on-chip radio and the notify flow, and
re-ground `CLAUDE.md` on the on-chip model.

**6a — `scripts/ble_soak.sh`:** change the default device address to the H2's
static random address:

```bash
DEVICE="${DEVICE:-F0:CA:FE:00:00:01}"   # was 80:1F:12:B6:60:BF (RN4871)
```

Everything else in the soak harness (connect/hold/reconnect, no-scan discipline,
debugfs conn params, fail-loud) carries over unchanged — it is address-agnostic.

**6b — `scripts/ble_notify_check.sh` (new):** a gaia-side check that subscribes to
the telemetry characteristic and asserts frames flow. Primary path: non-interactive
`bluetoothctl` (`gatt.select-attribute <char-uuid>` → `gatt.notify on`) capturing
notifications for a bounded window, asserting ≥5 frames of 17 bytes with the first
byte == `0x01`. Fallback if the scripted `bluetoothctl` notify capture proves
fiddly on 5.86: a short `bleak` (Python) subscriber. **Flag/verify:** confirm
`bleak` availability on gaia before relying on it; otherwise stay on `bluetoothctl`
/ `btgatt-client`. Reuse the no-scan, connect-by-address discipline from
`ble_soak.sh`. Mirror its env-knob + fail-loud style (ShellCheck-clean,
`set -euo pipefail`).

Deploy/run identically to the soak (`scp … gaia:`; `ssh gaia ./ble_notify_check.sh`).

**6c — `CLAUDE.md`:** rewrite the BLE section to the on-chip model:

- Replace the "BLE (dropped on the ESP32-H2 port — historical)" framing with the
  live on-chip design: esp-radio + trouble-host peripheral, custom service
  `7e700001-…` + Notify characteristic `7e700002-…`, static random address
  `F0:CA:FE:00:00:01`, name `MeteoStation`, 1 Hz telemetry, RWDT supervisor.
- Remove the "RN4871 parser kept for host tests" line (parser deleted in substep 1);
  document `meteo-lib::ble::frame` (the v1 wire frame) instead.
- Keep the gaia soak methodology + traps (still valid); add `ble_notify_check.sh` as
  the data-flow half of the acceptance gate alongside `ble_soak.sh`.
- Add the new `crates/meteo-firmware/src/ble.rs` + `watchdog.rs` to the module map.

**Verify:** `scp scripts/ble_*.sh gaia:` then `ssh gaia ./ble_soak.sh` (link holds
≥6 min, reconnects) and `ssh gaia ./ble_notify_check.sh` (frames flow). Markdown
docs: no build impact.

**Tests:** scripts validated by running on gaia; `CLAUDE.md` is documentation.

**Depends on:** substep 4 (notify must work for 6b). **Blocks:** none.

## Testing

- **Host (automated, `cargo nextest -p meteo-lib`):** the entire wire frame —
  `from_bmp388` mapping, `encode` scaling + sentinels, `decode` length/version
  rejection + sentinel→`None`, and the proptest wire-level roundtrip. These are the
  pure-logic core and must pass before the firmware is flashed.
- **Build/lint gate (every substep, before push):** `just build`, `just clippy`
  (both crates, both targets), `just format -- --check`, `just test`. Zero warnings.
- **Firmware/hardware (not auto-tested, per CLAUDE.md):** gated on-device via the
  gaia acceptance harness — `ble_soak.sh` (link holds 6 min, reconnects across
  cycles) **and** `ble_notify_check.sh` (≥5 valid 17-byte v1 frames flow). A single
  passing soak cycle is **not** acceptance; the link must hold and repeat.
- **Edge cases covered by host tests:** unknown version byte, short/long buffers,
  every-field-`None` (current firmware reality: only temp+pressure populated),
  fully-populated frame, scale boundaries near the sentinel values.

## Risks

1. **`[patch]` dependency resolution (highest).** esp-radio 0.18 / trouble 0.6 /
   esp-rtos 0.3 may not resolve against crates.io alone; upstream pins git revs
   (esp-hal `b7eec0f`, embassy `1d3c3de`). _Mitigation:_ substep 3a verifies resolve
   first and replicates the `[patch]` block if needed, before any BLE code is
   written. Record which path worked in Notes.
2. **trouble-host / esp-radio API shape drift (provisional import paths).** Because
   esp-radio and trouble-host are not yet in the lockfile, every esp-radio/trouble
   symbol in the sketches is provisional: the import paths
   (`esp_radio::ble::controller::BleConnector`, `trouble_host::prelude::*`), the
   `notify` arity (2-arg on 0.6 vs 3-arg on 0.5), the `Host`/`Runner`/`Peripheral`
   destructure, the `Server::new_with_config` constructor + `GapConfig`/`appearance`
   const paths, and the 128-bit service-UUID AD variant must all be confirmed against
   the resolved versions. _Mitigation:_ each is flagged inline at its call site; the
   load-bearing _decisions_ (concrete `ble::Controller` type, one server built once,
   `notify` per sample) are fixed — only the literal symbol names may shift, which the
   compiler catches. Resolve names against `docs.rs` for the exact lockfile versions.
3. **`AsGatt`/`FromGatt` for `[u8; 17]`.** trouble-host may not impl its GATT value
   traits for fixed byte arrays. _Mitigation:_ substep 4 notes a newtype wrapper
   implementing the trait as the fallback.
4. **RWDT false reset when idle (no central).** Feeding only on notify would reset a
   healthy idle device. _Mitigation:_ heartbeat supervisor feeds on _advertise-loop_
   progress (`ADV_BEAT`), not just notify; BLE liveness = adv OR notify progressed.
5. **`RwdtStageAction` variant name.** Docs didn't enumerate variants. _Mitigation:_
   confirm `ResetSystem` (vs `ResetRtc`/etc.) against the installed esp-hal before
   relying on it.
6. **Heap sizing.** 72 KiB is the upstream BLE-only figure; the H2 has 320 KiB SRAM.
   _Mitigation:_ start at 72 KiB, instrument free heap, raise only if allocation
   fails — do not pre-inflate.
7. **BLE link instability (the historical RN4871 6-min failure).** The link was
   never proven to hold. The cause moves on-chip now, but if the soak drops:
   diagnose with `btmon` on gaia during a hold _before_ changing code (CLAUDE.md);
   first knobs are conn-interval / supervision-timeout, not another patch.
8. **`embassy-sync` version (resolved).** `Cargo.lock` already shows esp-rtos 0.3.0
   resolving `embassy-sync 0.8.0`, so the plan pins `0.8` to match — pinning `0.7`
   would add a conflicting duplicate. No open question remains.
9. **gaia `bleak` availability.** The notify-check's fallback assumes `bleak`.
   _Mitigation:_ confirm before relying on it; primary path is `bluetoothctl`.
10. **`Rtc::set_timeout` duration type.** `set_timeout` takes `esp_hal::time::Duration`
    (a `fugit` re-export), **not** `embassy_time::Duration` (which the rest of the
    firmware uses). The watchdog sketch aliases it as `HalDuration`; keep the two
    distinct. Low risk — a wrong path fails to compile — but noted so the alias isn't
    "fixed" to the embassy type by reflex.

## Notes

Progress tracking (checked during implementation):

- [ ] 1. Remove vestigial RN4871 driver
- [ ] 2. Telemetry wire frame v1 (`meteo-lib`) + host tests
- [ ] 3. BLE stack bring-up — advertising only
  - [ ] 3a. Deps resolve (record: crates.io-only ☐ / `[patch]` required ☐)
- [ ] 4. GATT telemetry service + 1 Hz Notify
- [ ] 5. RWDT heartbeat supervisor
- [ ] 6. Acceptance tooling + docs
  - [ ] 6a. `ble_soak.sh` address
  - [ ] 6b. `ble_notify_check.sh`
  - [ ] 6c. `CLAUDE.md` on-chip rewrite
- [ ] Wedge-recovery manual confirmation (stalled task → reset ≤8 s)
- [ ] gaia acceptance: `ble_soak.sh` 6-min hold + reconnect, repeated
- [ ] gaia acceptance: `ble_notify_check.sh` frames flow
