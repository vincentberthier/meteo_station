# Plan: BLE Module Implementation (RN4871)

- **Source:** '1 (`.claude/brainstorm/1-ble-module-design.md`)
- **Date:** 2026-06-14
- **Status:** Done

## Summary

Implement the full BLE link for the weather station in one combined pass: a
shared, host-testable wire-frame codec in `meteo-lib`; a hardware-agnostic
RN4871 ASCII driver in `meteo-lib` over `embedded-io-async`; an Embassy firmware
BLE task on USART2 + `RST_N` (PA4) that provisions the module (verify-and-repair),
supervises advertising/disconnect/wedge, and pushes one Notify per second; and a
`bluer` central transport in `meteo-tui` that scans, connects, subscribes,
decodes frames, drives the existing `ClientEvent` seam, and reconnects. The frame
is the resolved 17-byte fixed-schema scaled-integer packet with **per-field
sentinels** marking absent sensors (only temperature + pressure exist today). The
codec lives in `meteo-lib` and is shared by encode (firmware) and decode
(central) so the two sides cannot drift.

## Key Decisions (locked with user)

- **Scope:** one combined plan (frame + driver + firmware task + central).
- **Absent sensors:** per-field sentinels in the 17-byte frame (`i16::MIN` for
  signed, `u16::MAX` / `u8::MAX` for unsigned), decoded to `None`.
- **Provisioning:** verify-and-repair at boot — read current config, only write
  set-commands + `WR` + reboot when it differs from desired.
- **TUI registry:** stays at Temperature + Pressure; central maps only present
  frame fields onto registry indices, others are dropped until hardware arrives.

## Shared constants (single source of truth, `meteo-lib`)

| Constant         | Value                                              | Used by             |
| ---------------- | -------------------------------------------------- | ------------------- |
| `SCHEMA_VERSION` | `1u8`                                              | encode + decode     |
| `FRAME_LEN`      | `17`                                               | encode + decode     |
| `SERVICE_UUID`   | `0x7E9A_0001_B5A3_4F6E_9C11_2D4E6F8A0B1C` (`u128`) | provision + central |
| `CHAR_UUID`      | `0x7E9A_0002_B5A3_4F6E_9C11_2D4E6F8A0B1C` (`u128`) | provision + central |
| `DEVICE_NAME`    | `"MeteoStation"`                                   | provision + central |
| `CHAR_PROPS`     | `0x10` (Notify)                                    | provision (`PC`)    |
| `CHAR_SIZE`      | `0x14` (20 bytes)                                  | provision (`PC`)    |
| `SR_FEATURES`    | `0x4000` (No-Prompt)                               | provision (`SR`)    |

UUIDs are defined **once** as `u128` in `meteo-lib`; the firmware formats them to
the 32-char hex `PS`/`PC` accept, the central builds `uuid::Uuid::from_u128`.

## Frame layout (17 bytes, little-endian, schema v1)

| Offset | Field          | Type | Encoding             | Natural unit | Sentinel (absent) |
| ------ | -------------- | ---- | -------------------- | ------------ | ----------------- |
| 0      | header         | u8   | schema version (`1`) | —            | n/a (always `1`)  |
| 1–2    | temperature    | i16  | centi-°C             | °C           | `i16::MIN`        |
| 3–4    | pressure       | u16  | deci-hPa             | Pa           | `u16::MAX`        |
| 5–6    | humidity       | u16  | centi-%RH            | %RH          | `u16::MAX`        |
| 7–8    | sky / IR temp  | i16  | centi-°C             | °C           | `i16::MIN`        |
| 9–10   | lum. mantissa  | u16  | mantissa             | lux          | `u16::MAX`        |
| 11     | lum. exponent  | u8   | base-10 exponent     | (lux)        | (mantissa marks)  |
| 12–13  | wind speed     | u16  | cm/s                 | m/s          | `u16::MAX`        |
| 14–15  | wind direction | u16  | deci-degree          | °            | `u16::MAX`        |
| 16     | battery        | u8   | percent              | %            | `u8::MAX`         |

Unit conversions (centralized in the codec, identical on both sides):
`pressure_pa = deci_hpa * 10`, `wind_ms = cm_s / 100`, `dir_deg = decideg / 10`,
`lux = mantissa * 10^exponent`. Pressure feeds the central as **Pa** so the
existing `pa_to_hpa` registry transform stays correct.

## Files Modified

| File                                 | Action | Description                                                                                                                                                                                 |
| ------------------------------------ | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml` (workspace)             | modify | Add `heapless = "0.9"`, `embedded-hal = "1.0"`, `static-cell = "2"`, `embassy-futures = "0.1"` to `[workspace.dependencies]`; add `bluer = "0.17.4"`, `uuid = "1.23.3"` for the host build. |
| `crates/meteo-lib/Cargo.toml`        | modify | Add `embedded-io-async`, `embedded-hal`, `heapless` deps.                                                                                                                                   |
| `crates/meteo-lib/src/lib.rs`        | modify | `pub mod ble;` + re-exports.                                                                                                                                                                |
| `crates/meteo-lib/src/ble/mod.rs`    | create | BLE module root: shared constants (UUIDs, names, props), re-exports.                                                                                                                        |
| `crates/meteo-lib/src/ble/frame.rs`  | create | `Frame` struct + `encode`/`decode` + scaling helpers + tests.                                                                                                                               |
| `crates/meteo-lib/src/ble/rn4871.rs` | create | `Rn4871` async driver over `embedded-io-async` + tests.                                                                                                                                     |
| `crates/meteo-firmware/Cargo.toml`   | modify | Add `embassy-sync`, `heapless`, `embedded-io-async` deps (already have via workspace where present).                                                                                        |
| `crates/meteo-firmware/src/main.rs`  | modify | Bind USART2 IRQ; init `BufferedUart` (PD5/PD6) + `RST_N` (PA4); spawn `ble_task`.                                                                                                           |
| `crates/meteo-firmware/src/ble.rs`   | create | `SENSOR_CHANNEL`, `SensorSample`, `ble_task` (provision + supervisor loop).                                                                                                                 |
| `crates/meteo-firmware/src/bmp.rs`   | modify | Send `SensorSample::Barometer` into `SENSOR_CHANNEL` each sample.                                                                                                                           |
| `crates/meteo-tui/Cargo.toml`        | modify | Add `meteo-lib`, `bluer = "0.17.4"`, `uuid = "1.23.3"` deps.                                                                                                                                |
| `crates/meteo-tui/src/feed.rs`       | modify | Replace stub with `bluer` scan→connect→subscribe→decode→reconnect state machine.                                                                                                            |
| `crates/meteo-tui/src/sensors.rs`    | modify | Add a stable frame-field → registry-index mapping helper (`field_to_index`).                                                                                                                |
| `CLAUDE.md`                          | modify | Document BLE module, `just tui` BlueZ/gaia testing note, new module paths.                                                                                                                  |

## Plan

### 1. Shared wire-frame codec in `meteo-lib`

**Goal:** a pure, `no_std`, host-testable codec that both firmware (encode) and
central (decode) call, so units/sentinels/scaling live in exactly one place.

**Files:** `crates/meteo-lib/src/ble/mod.rs` (create),
`crates/meteo-lib/src/ble/frame.rs` (create), `crates/meteo-lib/src/lib.rs`
(modify), `crates/meteo-lib/Cargo.toml` (modify).

**`Cargo.toml` change:**

```toml
[dependencies]
defmt = { workspace = true, optional = true }
embedded-hal-async = { workspace = true }
embedded-hal = { workspace = true }
embedded-io-async = { workspace = true }
heapless = { workspace = true }
```

Add to workspace `[workspace.dependencies]`: `heapless = "0.9"`,
`embedded-hal = "1.0"` (and ensure `embedded-io-async = "0.7"` already present —
it is).

**`lib.rs` change:** add `pub mod ble;`.

**`ble/mod.rs`:** the shared constants table above, e.g.

```rust
pub mod frame;
pub mod rn4871;

pub const SCHEMA_VERSION: u8 = 1;
pub const SERVICE_UUID: u128 = 0x7E9A_0001_B5A3_4F6E_9C11_2D4E_6F8A_0B1C;
pub const CHAR_UUID: u128 = 0x7E9A_0002_B5A3_4F6E_9C11_2D4E_6F8A_0B1C;
pub const DEVICE_NAME: &str = "MeteoStation";
```

**`ble/frame.rs` signatures:**

```rust
pub const FRAME_LEN: usize = 17;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Frame {
    pub temperature_c: Option<f32>,
    pub pressure_pa: Option<f32>,
    pub humidity_pct: Option<f32>,
    pub sky_temp_c: Option<f32>,
    pub luminosity_lux: Option<f32>,
    pub wind_speed_ms: Option<f32>,
    pub wind_dir_deg: Option<f32>,
    pub battery_pct: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    TooShort { got: usize },
    UnknownVersion(u8),
}

impl Frame {
    #[must_use]
    pub fn encode(&self) -> [u8; FRAME_LEN];
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError>;

    /// Present (`Some`) fields as `(FrameField, value_in_registry_unit)` pairs,
    /// in wire order. Lets a consumer iterate decoded values without matching
    /// every field by hand. Battery is yielded as `f32` percent for uniformity.
    pub fn present_fields(&self) -> impl Iterator<Item = (FrameField, f32)> + '_;
}

/// Canonical wire-field identity, in frame order. Lives here (next to `Frame`)
/// because it mirrors the wire layout; the central's registry mapping
/// (`meteo-tui::sensors::field_to_index`) is built on it. RESOLVED placement —
/// `FrameField` is defined in `meteo-lib::ble::frame`, not in `meteo-tui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameField {
    Temperature,
    Pressure,
    Humidity,
    SkyTemp,
    Luminosity,
    WindSpeed,
    WindDir,
    Battery,
}
```

**Code sketch (encode/decode core):**

```rust
// Each field: map Option<natural> -> scaled int or sentinel, then to_le_bytes.
// LE chosen because BLE ATT values are little-endian by convention.
#[expect(clippy::little_endian_bytes, reason = "BLE wire format is little-endian")]
fn put_i16(buf: &mut [u8; FRAME_LEN], off: usize, v: i16) {
    let b = v.to_le_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
}

// temperature: centi-°C, sentinel i16::MIN
fn enc_temp(c: Option<f32>) -> i16 {
    c.map_or(i16::MIN, |x| scale_i16(x, 100.0)) // round, saturate to [MIN+1, MAX]
}
fn dec_temp(raw: i16) -> Option<f32> {
    (raw != i16::MIN).then(|| f32::from(raw) / 100.0)
}
```

- Provide saturating/rounding `scale_i16` / `scale_u16` helpers that clamp to the
  representable range and never collide with the sentinel (e.g. signed values
  clamp to `i16::MIN + 1 ..= i16::MAX`).
- Luminosity: `enc_lux` picks the smallest `exponent` (0..=255) keeping the
  mantissa ≤ `u16::MAX - 1`; sentinel = mantissa `u16::MAX`. `dec_lux` returns
  `None` on the sentinel mantissa, else `mantissa as f32 * 10f32.powi(exp)`.
- `decode` checks `bytes.len() >= FRAME_LEN` (→ `TooShort`) and
  `bytes[0] == SCHEMA_VERSION` (→ `UnknownVersion`), then reads each field with
  bounds-checked `.get()`.

**Tests (`#[cfg(test)]` in `frame.rs`, host, `test-log`):**

- `encode_then_decode_roundtrips_present_values` — build a `Frame` with all eight
  fields `Some`, `encode`, `decode`, assert each field back within scale epsilon.
- `absent_fields_encode_to_sentinels_and_decode_to_none` — `Frame::default()`
  (all `None`) → decode yields all `None`; assert sentinel bytes at each offset.
- `encoded_frame_is_exactly_frame_len_bytes` — `assert_eq!(frame.encode().len(),
FRAME_LEN)`.
- `decode_rejects_short_buffer` — `decode(&[0u8; 10])` → `Err(TooShort)`.
- `decode_rejects_unknown_version` — first byte `0xFF` → `Err(UnknownVersion)`.
- `pressure_roundtrips_in_pascals` — `Some(101_325.0)` Pa → decode within 10 Pa
  (deci-hPa resolution), confirms `pa_to_hpa` compatibility.
- `values_saturate_without_hitting_sentinel` — extreme inputs (e.g. temp 400 °C)
  clamp to max, never to the sentinel.
- `luminosity_mantissa_exponent_roundtrips` — `Some(98_765.0)` lux → decode within
  the resolution of one mantissa count at the chosen exponent (assert relative
  error ≤ 5%, since the base-10 mantissa/exponent encoding is lossy for large
  values — "ULP" is not meaningful here).
- `present_fields_yields_only_some_in_wire_order` — a `Frame` with temperature +
  pressure `Some` (rest `None`) → `present_fields()` yields exactly
  `[(Temperature, _), (Pressure, _)]` in that order, with pressure in Pa.

**Dependencies:** none — foundational. Substeps 2, 3, 5 build on it.

### 2. RN4871 ASCII driver in `meteo-lib`

**Goal:** a hardware-agnostic async driver: command/response, command-mode entry,
status-event parsing, firmware-version query, verify-and-repair provisioning,
characteristic handle discovery, and frame push via `SHW`.

**Files:** `crates/meteo-lib/src/ble/rn4871.rs` (create).

**Generics & construction:**

```rust
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};

pub struct Rn4871<U, R, D> {
    uart: U,            // BufferedUart on the firmware; Read + Write
    reset: R,           // RST_N, active-low
    delay: D,
    char_handle: Option<u16>,
    events: heapless::Deque<Event, 4>, // status events seen while awaiting a response
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event { Reboot, Connect, Disconnect, StreamOpen, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> { Io(E), Command, Timeout, BadResponse, NoHandle, UnsupportedFirmware }

impl<U, R, D, E> Rn4871<U, R, D>
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
{
    pub fn new(uart: U, reset: R, delay: D) -> Self;

    /// Pulse RST_N low (≥ 1 ms hold — datasheet timing) then await `%REBOOT%`.
    pub async fn reset(&mut self) -> Result<(), Error<E>>;

    /// `$$$` with the datasheet 100 ms guard; confirm command mode by issuing
    /// `V` and accepting a version line.
    pub async fn enter_command_mode(&mut self) -> Result<(), Error<E>>;

    /// Send `cmd` + `\r`, read lines routing `%...%` to `self.events`, return
    /// Ok on `AOK`, Err(Command) on `ERR`.
    pub async fn command(&mut self, cmd: &[u8]) -> Result<(), Error<E>>;

    /// Send `cmd` + `\r`, capture the first non-event data line into `out`.
    pub async fn query(&mut self, cmd: &[u8], out: &mut [u8]) -> Result<usize, Error<E>>;

    pub async fn firmware_version(&mut self) -> Result<(u8, u8), Error<E>>; // (major, minor)

    /// Verify-and-repair: read current config (`D`/`GS`/`LS`), only write
    /// `SN/SS/PZ/PS/PC/SR` + `WR` + reboot when it differs from desired.
    pub async fn provision(&mut self) -> Result<(), Error<E>>;

    /// Parse `LS` output, store the value handle for `CHAR_UUID`.
    pub async fn discover_char_handle(&mut self) -> Result<u16, Error<E>>;

    pub async fn start_advertising(&mut self) -> Result<(), Error<E>>; // `A`

    /// `SHW,<handle>,<hex>` — hex-encode `frame` into a heapless buffer, await AOK.
    pub async fn push_frame(&mut self, frame: &[u8]) -> Result<(), Error<E>>;

    /// Drain a buffered event, else read one line and classify it.
    pub async fn next_event(&mut self) -> Result<Event, Error<E>>;
}
```

**Internals / sketches:**

- `read_line(&mut self, buf: &mut heapless::Vec<u8, 64>) -> Result<(), Error<E>>`
  — clear `buf`, then read **one byte at a time** via `Read::read(&mut [b])`
  until `\n`; strip a trailing `\r`; map a read error to `Error::Io`. Byte-wise
  reads keep the call cancel-safe under `select` when backed by a `BufferedUart`
  ring buffer.
- `classify(line: &[u8]) -> Line` → `Event(_)` when wrapped in `%…%`
  (match `%REBOOT%`, `%CONNECT`, `%DISCONNECT%`, `%STREAM_OPEN%`), `Aok` for
  `"AOK"`, `Err` for `"ERR"`, else `Data`.
- `command` response loop (handles **No-Prompt** — there is no `CMD>` terminator
  to anchor on, so the loop keys off `AOK`/`ERR` and ignores everything else):
  ```rust
  async fn command(&mut self, cmd: &[u8]) -> Result<(), Error<E>> {
      self.write_all(cmd).await?;
      self.write_all(b"\r").await?;
      let mut line = heapless::Vec::<u8, 64>::new();
      loop {
          self.read_line(&mut line).await?;
          match classify(&line) {
              Line::Aok => return Ok(()),
              Line::Err => return Err(Error::Command),
              Line::Event(e) => { let _ = self.events.push_back(e); } // buffer, keep waiting
              Line::Data => {} // echoes / blank lines under No-Prompt: ignore
          }
      }
  }
  ```
  An empty line (No-Prompt can emit a bare `\r\n`) classifies as `Data` and is
  skipped, so the loop never mistakes it for a response.
- `reset`: `self.reset.set_low()`; `self.delay.delay_ms(2).await` (datasheet
  reset-pulse hold, a hardware timing minimum — **not** a readiness guess);
  `self.reset.set_high()`; then **await the real signal** `%REBOOT%` (no fixed
  settle delay).
- `enter_command_mode`: `self.delay.delay_ms(100).await` (mandatory pre-`$`
  guard), write `$$$`, `delay_ms(100)` (post guard), then `V` and accept a
  version line as the command-mode confirmation (No-Prompt suppresses `CMD>`).
- `provision` desired state: `SN,MeteoStation`; `SS,80` (Device Info only, drops
  Transparent UART); `PZ`; `PS,<SERVICE_UUID hex32>`;
  `PC,<CHAR_UUID hex32>,10,14`; `SR,4000`; then `WR`; `R,1`; await `%REBOOT%`;
  re-enter command mode; `discover_char_handle`. Skip the write block when a prior
  `D`/`LS` read already matches desired (verify-and-repair).
- `firmware_version`: parse the `V` line (e.g. `"RN4871 V1.40 ..."`); return
  `UnsupportedFirmware` only as a logged soft-gate (warn below 1.40, hard-fail
  below 1.28 — caller decides).
- `discover_char_handle`: issue `LS` (list local/server services) and parse its
  output. The RN4871 `LS` block lists each private service UUID on its own line,
  then one indented line per characteristic as
  `<char-UUID-hex32>,<value-handle-hex>,<prop-hex>`, terminated by `END`.
  Representative block for our provisioned service:
  ```text
  7E9A0001B5A34F6E9C112D4E6F8A0B1C
    7E9A0002B5A34F6E9C112D4E6F8A0B1C,0072,10
  END
  ```
  Parse: find the line whose first comma-field equals `CHAR_UUID` (hex32,
  case-insensitive), take the second field as the `u16` value handle (here
  `0x0072`), store it in `self.char_handle`. Return `Error::NoHandle` if absent.
  (Confirm the exact `LS` layout against User Guide DS50002466 at bring-up; the
  parser keys off the comma-separated char line, not a fixed handle.)
- hex encoding: a `to_hex(frame, &mut heapless::Vec<u8, 40>)` helper (2 chars per
  byte, upper-case) — no `alloc`.

**Tests (`#[cfg(test)]`, host; hand-rolled fakes — `embedded-hal-mock` lacks
`embedded-io-async` coverage):**

- `std` in `no_std` test modules: `meteo-lib` is `#![no_std]`, but `cargo test`
  builds it for the host, so the `#[cfg(test)]` module brings `std` in with
  `extern crate std;` (the existing `utils.rs` test module already does exactly
  this — follow it). `VecDeque`/`Vec` come from `std::collections` / `std::vec`
  inside that module; the codec/driver themselves stay `no_std`.
- Define `FakeUart { rx: VecDeque<u8>, tx: Vec<u8> }` implementing
  `embedded_io_async::{Read, Write}`, `FakePin` implementing `OutputPin`, and a
  `FakeDelay` no-op `DelayNs` (its `delay_ns` returns immediately — tests assert
  on bytes, never on wall-clock).
- `command_returns_ok_on_aok` — preload rx with `"AOK\r\n"`, assert
  `command(b"SN,MeteoStation")` is `Ok` and tx contains `SN,MeteoStation\r`.
- `command_returns_err_on_err` — rx `"ERR\r\n"` → `Err(Command)`.
- `command_returns_err_after_event` — rx `"%CONNECT,0,...%\r\nERR\r\n"`; assert
  `command(...)` returns `Err(Command)` **and** a later `next_event()` still
  yields `Event::Connect` (the event seen mid-command is buffered, not dropped).
- `status_events_are_routed_while_awaiting_aok` — rx
  `"%CONNECT,0,...%\r\nAOK\r\n"`; `command` returns `Ok` **and** a later
  `next_event()` yields `Event::Connect`.
- `next_event_parses_disconnect` — rx `"%DISCONNECT%\r\n"` → `Event::Disconnect`.
- `firmware_version_parses_major_minor` — rx `"RN4871 V1.40\r\n"` → `(1, 40)`.
- `discover_char_handle_parses_ls` — preload rx with the representative `LS`
  fixture above (`7E9A0001…\r\n  7E9A0002…,0072,10\r\nEND\r\n`); assert
  `discover_char_handle()` returns `0x0072` and a subsequent `push_frame` targets
  handle `0072`.
- `discover_char_handle_missing_returns_no_handle` — `LS` block without
  `CHAR_UUID` → `Err(NoHandle)`.
- `push_frame_emits_shw_with_hex` — `push_frame(&[0x01, 0xAB])` with a known
  handle → tx contains `SHW,<handle>,01AB\r`; rx `"AOK\r\n"` → `Ok`.

**Dependencies:** substep 1 (uses `FRAME_LEN`/constants only indirectly; the hex
encode is generic over bytes).

### 3. Firmware BLE task + supervisor (`meteo-firmware`)

**Goal:** wire USART2 + `RST_N`, own the driver, provision once at boot, push a
frame per sensor sample, and supervise the link (re-advertise on disconnect,
`RST_N` wedge recovery).

**Files:** `crates/meteo-firmware/src/ble.rs` (create),
`crates/meteo-firmware/src/main.rs` (modify),
`crates/meteo-firmware/Cargo.toml` (modify).

**`Cargo.toml`:** the firmware crate currently depends only on
`cortex-m{,-rt}`, `defmt{,-rtt}`, `embassy-{executor,stm32,time}`, and
`panic-probe`. Add to the `[target.'cfg(target_arch = "arm")'.dependencies]`
block (all `{ workspace = true }`): `embassy-sync` (defined in workspace, not
yet used here), `embassy-futures` (for `select`), `heapless`,
`embedded-io-async`, and `static-cell`. Add `static-cell = "2"`,
`heapless = "0.9"`, and `embassy-futures = "0.1"` to `[workspace.dependencies]`
first; `embassy-sync` and `embedded-io-async` already exist there.

- `embassy-futures = "0.1"` is VERIFIED to unify with the `embassy-futures 0.1.2`
  already pulled transitively by `embassy-stm32` 0.5 (no new version). The
  supervisor uses `use embassy_futures::select::{select, Either};`. `embassy-time`
  / `embassy-executor` do **not** re-export `select`.

**`ble.rs` signatures:**

```rust
use embassy_futures::select::{select, Either};
use embassy_stm32::usart::BufferedUart;
use embassy_stm32::gpio::Output;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Delay;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};
use meteo_lib::ble::frame::Frame;
use meteo_lib::ble::rn4871::{Event, Rn4871};

#[derive(Debug, Clone, Copy)]
pub enum SensorSample {
    Barometer { temperature_c: f32, pressure_pa: f32 },
}

/// Sampling tasks publish samples; the BLE task consumes them. Capacity 8 covers
/// transient bursts; consumer keeps pace at 1 Hz so it never blocks producers.
pub static SENSOR_CHANNEL: Channel<ThreadModeRawMutex, SensorSample, 8> = Channel::new();

#[embassy_executor::task]
pub async fn ble_task(uart: BufferedUart<'static>, reset: Output<'static>) { /* ... */ }

// Helpers owned by ble.rs (generic over the driver's type params so they unit-
// test against the fakes from substep 2 if desired; in firmware U =
// BufferedUart, R = Output, D = Delay).

/// Full bring-up: `dev.reset()` → `enter_command_mode` → `firmware_version`
/// (soft-gate, log only) → `provision` → `discover_char_handle` →
/// `start_advertising`. Retries the whole sequence on `Err` (each retry begins
/// with another `RST_N` pulse); logs each attempt via defmt. Returns once the
/// module is advertising. The where-clause mirrors the `Rn4871` impl bounds.
async fn bring_up<U, R, D, E>(dev: &mut Rn4871<U, R, D>)
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
{ /* loop { match bring_up_once(dev).await { Ok(()) => return, Err(_) => continue } } */ }

/// Wedge recovery (circuit-breaker path): pulse `RST_N`, await `%REBOOT%`, then
/// delegate to `bring_up` to reprovision + re-advertise. Called only when a
/// command/event read fails (the explicit failure path).
async fn recover<U, R, D, E>(dev: &mut Rn4871<U, R, D>)
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
{ /* let _ = dev.reset().await; bring_up(dev).await */ }

/// Pure: fold one sample's values into the running `Frame` (latest-wins per
/// field, others untouched). Free function so it is host-testable.
fn apply_sample(frame: &mut Frame, sample: SensorSample) {
    match sample {
        SensorSample::Barometer { temperature_c, pressure_pa } => {
            frame.temperature_c = Some(temperature_c);
            frame.pressure_pa = Some(pressure_pa);
        }
    }
}
```

**Supervisor sketch:**

```rust
let mut dev = Rn4871::new(uart, reset, Delay);
// Bring-up: reset -> command mode -> version gate -> provision -> advertise.
bring_up(&mut dev).await;          // retries with RST_N on hard failure

let mut frame = Frame::default();  // accumulates latest per-sensor values
loop {
    match select(SENSOR_CHANNEL.receive(), dev.next_event()).await {
        Either::First(sample) => {
            apply_sample(&mut frame, sample);     // update temp/pressure fields
            if dev.push_frame(&frame.encode()).await.is_err() {
                recover(&mut dev).await;          // wedge path -> RST_N + reprovision
            }
        }
        Either::Second(Ok(Event::Disconnect)) => { let _ = dev.start_advertising().await; }
        Either::Second(Ok(_)) => {}               // Connect/Reboot/etc.: log only
        Either::Second(Err(_)) => recover(&mut dev).await,
    }
}
```

- `select` over a `BufferedUart`-backed byte read is cancel-safe (unread bytes
  stay in the ring buffer), so a sensor sample arriving mid-read loses nothing.
- **Wedge watchdog** = deadlock circuit-breaker only: `recover` pulses `RST_N`,
  awaits `%REBOOT%`, reprovisions, re-advertises, paired with the explicit
  `Error` failure path. `SHW` while no central is connected merely refreshes the
  local value (harmless), so absent-central needs no special handling — the loop
  always drains and pushes (latest-wins behaviour).

**`main.rs` changes:**

```rust
bind_interrupts!(struct Irqs {
    I2C1_EV => embassy_stm32::i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C1_ER => embassy_stm32::i2c::ErrorInterruptHandler<peripherals::I2C1>;
    USART2  => embassy_stm32::usart::InterruptHandler<peripherals::USART2>;
});
// ...
static TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
static RX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
// Signature VERIFIED against embassy-stm32 0.5.0
// (src/usart/buffered.rs:220): new(peri, rx, tx, tx_buffer, rx_buffer, irq,
// config) -> Result<Self, ConfigError>. Note: rx BEFORE tx, tx_buffer BEFORE
// rx_buffer, irq SECOND-TO-LAST.
let uart = BufferedUart::new(
    p.USART2,
    p.PD6,                       // rx
    p.PD5,                       // tx
    TX_BUF.init([0; 256]),       // tx_buffer
    RX_BUF.init([0; 256]),       // rx_buffer
    Irqs,                        // irq binding
    UartConfig::default(),       // 115200 8N1
).expect("USART2 init");
let rst_n = Output::new(p.PA4, Level::High, Speed::Low); // active-low, deasserted
spawner.spawn(ble::ble_task(uart, rst_n)).expect("ble_task already spawned");
```

- `BufferedUart::new` returns `Result<Self, ConfigError>` (verified), so the
  `.expect()` is valid for firmware-main (no recovery from peripheral init).
- `StaticCell` is **not** yet used anywhere in this codebase and `static-cell` is
  **not** in the workspace `Cargo.toml`. Add `static-cell = "2"` to
  `[workspace.dependencies]` and to the firmware `[target.'cfg(target_arch =
"arm")'.dependencies]` block, and `use static_cell::StaticCell;` in `main.rs`.
  (The `StaticCell<Mutex<…>>` pattern in CLAUDE.md is aspirational, not current.)

**Tests:** the task is hardware-interfacing (no host test). Keep logic pure where
possible — `apply_sample(&mut Frame, SensorSample)` is a free function tested in
`ble.rs`'s `#[cfg(test)]` module:

- `apply_barometer_sets_temp_and_pressure` — assert the two fields become `Some`
  with the sampled values, others stay `None`.
- `apply_barometer_overwrites_previous_sample` — apply two `Barometer` samples;
  assert temperature/pressure hold the **second** sample's values (latest-wins),
  and the six untouched fields remain `None`.

**Dependencies:** substeps 1 and 2.

### 4. Feed sensor samples from the sampling task

**Goal:** the BMP388 task publishes each reading to `SENSOR_CHANNEL` so the BLE
task can push it; keep the existing RTT logging.

**Files:** `crates/meteo-firmware/src/bmp.rs` (modify).

**Code sketch:**

```rust
use crate::ble::{SensorSample, SENSOR_CHANNEL};
// inside the Ok(reading) arm, after logging:
SENSOR_CHANNEL.send(SensorSample::Barometer {
    temperature_c: reading.temperature,
    pressure_pa: reading.pressure,
}).await;
```

- `Channel::send` awaits only if full; at 1 Hz with a same-rate consumer it never
  blocks. No fixed delays added; the existing 1 s cadence is the sampling rate.

**Tests:** none new (hardware task); covered by substep 3's `apply_sample` test.

**Dependencies:** substep 3 (defines the channel and `SensorSample`).

### 5. `bluer` central transport in `meteo-tui`

**Goal:** replace the idle stub in `feed.rs` with a real Linux/BlueZ central:
scan → connect → discover service/char → subscribe → decode → drive
`ClientEvent` → reconnect on drop. Validated on **gaia** (the only host with a
radio).

**Files:** `crates/meteo-tui/Cargo.toml` (modify),
`crates/meteo-tui/src/feed.rs` (modify),
`crates/meteo-tui/src/sensors.rs` (modify),
`crates/meteo-lib` dependency wired into `meteo-tui`.

**`Cargo.toml`:**

```toml
meteo-lib = { workspace = true }            # no_std crate, builds on host; for the frame codec
bluer = { version = "0.17.4", features = ["bluetoothd"] }
uuid = "1.23.3"
```

`meteo-lib` is `default-features = false` (no `defmt`) via the workspace dep.

**`sensors.rs` — frame-field → registry-index mapping:**

```rust
use meteo_lib::ble::frame::FrameField; // RESOLVED: defined in meteo-lib (substep 1)

/// Map a decoded frame field to its registry index, or `None` if this build's
/// registry does not present it. Keeps wire order and display order decoupled.
#[must_use]
pub fn field_to_index(field: FrameField) -> Option<usize> {
    match field {
        FrameField::Temperature => Some(0),
        FrameField::Pressure => Some(1),
        _ => None, // humidity, sky, lux, wind, battery: not in registry yet
    }
}
```

`FrameField` is reused from `meteo-lib::ble::frame` (defined in substep 1) — not
redefined here. Pressure is fed as **Pa** (`pa_to_hpa` transform stays valid),
temperature as **°C**; both come straight from `Frame::present_fields`.

**`feed.rs` sketch:**

API names below are VERIFIED against the `bluer` 0.17 `gatt_client` example.
Note: `bluer` has **no** `service_by_uuid`/`characteristic_by_uuid` helpers — you
iterate `services()` / `characteristics()` and match `uuid().await`.

```rust
use futures::StreamExt as _;
use bluer::{AdapterEvent, Device, gatt::remote::Characteristic};
use meteo_lib::ble::frame::Frame;
use crate::sensors::field_to_index;

pub async fn run(tx: Sender<ClientEvent>, mut shutdown: watch::Receiver<bool>)
    -> Result<(), Box<dyn Error>>
{
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    let service_uuid = Uuid::from_u128(meteo_lib::ble::SERVICE_UUID);
    let char_uuid = Uuid::from_u128(meteo_lib::ble::CHAR_UUID);

    'outer: loop {
        if *shutdown.borrow() { break; }
        // 1. SCAN: stream adapter events, take the first device advertising our service.
        let mut events = adapter.discover_devices().await?;             // Stream<AdapterEvent>
        let device: Device = loop {
            tokio::select! {
                _ = shutdown.changed() => break 'outer,
                ev = events.next() => match ev {
                    Some(AdapterEvent::DeviceAdded(addr)) => {
                        let dev = adapter.device(addr)?;
                        // device.uuids() -> Result<Option<Vec<Uuid>>>
                        if dev.uuids().await?.unwrap_or_default().contains(&service_uuid) {
                            break dev;
                        }
                    }
                    Some(_) => {}                                       // DeviceRemoved / property change
                    None => continue 'outer,                           // stream ended → restart scan
                }
            }
        };
        drop(events);                                                  // stop scanning before connecting

        // 2. CONNECT + resolve characteristic by iterating services/characteristics.
        if !device.is_connected().await? { device.connect().await?; }
        let ch: Option<Characteristic> = 'find: {
            for s in device.services().await? {                        // Vec<Service>
                if s.uuid().await? != service_uuid { continue; }
                for c in s.characteristics().await? {                  // Vec<Characteristic>
                    if c.uuid().await? == char_uuid { break 'find Some(c); }
                }
            }
            None
        };
        let Some(ch) = ch else {                                       // service/char missing → retry
            let _ = device.disconnect().await;
            continue 'outer;
        };

        // 3. SUBSCRIBE + pump notifications until disconnect or shutdown.
        tx.send(ClientEvent::Connected).await?;
        let mut notify = ch.notify().await?;                           // Stream<Vec<u8>>
        loop {
            tokio::select! {
                _ = shutdown.changed() => { let _ = device.disconnect().await; break 'outer; }
                item = notify.next() => match item {
                    Some(bytes) => match Frame::decode(&bytes) {
                        Ok(frame) => for (field, value) in frame.present_fields() {
                            if let Some(i) = field_to_index(field) {
                                tx.send(ClientEvent::Reading { index: i, raw: value }).await?;
                            }
                        },
                        Err(_) => { /* log + skip: truncated or unknown schema version */ }
                    },
                    None => break,                                     // stream ended = disconnect
                }
            }
        }
        // 4. DISCONNECTED → notify UI and reconnect via the outer loop.
        tx.send(ClientEvent::Disconnected).await?;
    }
    Ok(())
}
```

- `dev.uuids()` returns `Result<Option<Vec<Uuid>>>`; treat `None` as "no service
  data advertised yet" and keep scanning.
- Reconnect = the `'outer` loop: a disconnect (notify stream → `None`) or any
  connect/resolve error emits `Disconnected` and loops back to a fresh
  `discover_devices()` scan. No fixed backoff sleep — the next iteration blocks on
  the real adapter event stream. A bounded backoff is acceptable only as a
  circuit-breaker if a tight reconnect spin is observed.
- `shutdown` is honored in every `await` via `tokio::select!`.
- The two `.unwrap_or_default()` / pattern uses above are illustrative;
  production code replaces `?`-on-`Box<dyn Error>` with logged, non-fatal handling
  so one BlueZ hiccup restarts the scan instead of killing the feed task.

**Tests (host, `#[cfg(test)]`):**

- `field_to_index_maps_temperature_and_pressure` — assert `Temperature → 0`,
  `Pressure → 1`, and that an unmapped field (`Humidity`) → `None`.
- `decoded_pressure_feeds_registry_in_pascals` — lives in `sensors.rs`'s test
  module (has `SENSORS` + `field_to_index` in scope; imports `Frame`/`FrameField`
  from `meteo_lib::ble::frame`). Build `Frame { pressure_pa: Some(101_325.0),
..Default::default() }`, `encode()` then `decode()` (exercises the wire path),
  take the `(FrameField::Pressure, value)` pair from `present_fields()`, assert
  `field_to_index(FrameField::Pressure) == Some(1)`, that `value` ≈ 101_325 Pa
  (within deci-hPa resolution), and that `SENSORS[1].display_value(value)` ≈
  1013.25 hPa.
- The `bluer` I/O path is not unit-tested (needs a radio); it is validated
  manually on gaia (see Testing).

**Dependencies:** substep 1 (frame decode + UUID constants).

### 6. Documentation

**Goal:** keep `CLAUDE.md` accurate for the new module layout and BLE testing.

**Files:** `CLAUDE.md` (modify).

- Add `ble/` module paths to the Module Structure tree (`meteo-lib/src/ble/`,
  `meteo-firmware/src/ble.rs`).
- Add a "BLE testing" note: `meteo-tui` needs BlueZ + a radio; the dev box has
  none, so run `just tui` on **gaia** (`D8:F3:BC:63:2E:56`, BlueZ 5.86) over SSH;
  never reboot gaia.
- Note the RN4871 provisioning is verify-and-repair and the wire contract
  (service/char UUIDs, 17-byte v1 frame) lives in `meteo-lib::ble`.

**Dependencies:** all prior substeps (documents the final shape).

## Testing

**Automated (host, `just test` → `cargo nextest ... --target x86_64-...`):**

- Frame codec: roundtrip, sentinels/absence, length, short-buffer + bad-version
  rejection, saturation, pressure-in-Pa, luminosity mantissa/exponent (substep 1).
- RN4871 driver: AOK/ERR handling, event routing during command, disconnect
  parse, version parse, `LS` handle discovery, `SHW` hex emission — all via
  hand-rolled `embedded-io-async` fakes (substep 2).
- `apply_sample` purity test (substep 3).
- `field_to_index` + pressure-unit path (substep 5).

**Lint/format/build (per `just clippy`, `just format`, `just build`):**

- `cargo clippy -p meteo-firmware -- -D warnings` (embedded target).
- `cargo clippy -p meteo-lib -p meteo-tui --target x86_64-... -- -D warnings`.
- Zero-warning policy (these workspace restriction lints are active at warn and
  must be handled, not discovered at build time):
  - `little_endian_bytes` — every `to_le_bytes`/`from_le_bytes` needs an inline
    `#[expect(clippy::little_endian_bytes, reason = "BLE wire format is LE")]`.
  - `arithmetic_side_effects` — the codec's scaling math (`x * 100.0`,
    `f32::from(raw) / 100.0`, `mantissa as f32 * 10f32.powi(exp)`, deci-hPa `* 10`)
    trips it; wrap each scaling helper with
    `#[expect(clippy::arithmetic_side_effects, reason = "scaled-int codec, inputs clamped before scaling")]`.
    Prefer saturating ops where integer math could overflow.
  - `integer_division` — avoid it: decode helpers convert to `f32` _before_
    dividing (`f32::from(cm_s) / 100.0`, `f32::from(decideg) / 10.0`), which is
    float division and does not trip the lint. If any genuinely integer/integer
    division remains, annotate it
    `#[expect(clippy::integer_division, reason = "scaled-int decode, truncation intended")]`.
  - `else_if_without_else` — the `classify` event/response chain (and any
    if/else-if in the driver) needs a terminal `else` arm (e.g. `else { Line::Data }`)
    or a `match` instead.
  - `default_numeric_fallback` / `unseparated_literal_suffix` — annotate numeric
    literals with explicit suffixes (`100.0_f32`, `10_i32`) as the existing code does.
  - Any `as` cast uses `TryFrom`/saturating helpers; no `unwrap`/`expect` outside
    test modules and firmware-`main`.
- `cargo build --release -p meteo-firmware` and `cargo build -p meteo-tui` must
  both succeed.

**Manual hardware validation (after the above pass):**

- Flash via `just run`; confirm RTT shows provisioning + `%REBOOT%` + advertising.
- On **gaia**: `bluetoothctl` shows `MeteoStation` advertising; `just tui` (run on
  gaia) connects, subscribes, and the TUI leaves `Scanning` for `Connected` with
  live temperature + pressure.
- Disconnect test: kill the central; firmware re-advertises; reconnect succeeds.
- Wedge test (best-effort): confirm `RST_N` recovery path on an induced stall.
- Follow the CLAUDE.md `probe-rs` rule: background the process, `kill -INT`, never
  SIGTERM/timeout.

**Edge cases covered by tests:** absent sensors (sentinels → `None`), truncated
notification (short buffer), schema-version mismatch (central rejects), value
saturation, out-of-range registry index (already handled by `App::apply`).

## Risks

- **20-byte cap is a real ceiling.** The v1 frame uses 17 of 20. A larger roster
  or higher precision forces multiple characteristics or the Transparent UART
  path. Mitigation: the version byte lets the central reject unknown schemas; the
  ceiling is documented, not hidden.
- **RN4871 command-mode / No-Prompt interplay.** With `SR,4000` there is no
  `CMD>` to await; command-mode entry is confirmed by a successful `V`. If the
  module also has Command Mode Guard (0x0008) behavior on the installed firmware,
  `$$$` timing differs — the driver must confirm via response, not assume.
  Mitigation: `enter_command_mode` validates with `V`; bring-up retries via
  `RST_N`.
- **`LS` output format for handle discovery is firmware-version-dependent.**
  Hardcoding a handle would break across firmware. Mitigation: parse `LS` at
  bring-up; fail loudly (`NoHandle`) rather than push to a wrong handle.
- **`select`-cancellation data loss.** Only safe because USART2 uses
  `BufferedUart` (RAM ring buffer). If a plain DMA `Uart` is substituted, mid-read
  cancellation could drop bytes. Mitigation: the plan mandates `BufferedUart`.
- **`bluer` build pulls a large dependency tree** (zbus/D-Bus); the dev box has no
  radio so it only compiles there. Mitigation: all runtime validation is on gaia;
  CI/host runs cover only the pure-logic tests.
- **`meteo-lib` gaining `embedded-io-async`/`heapless`** must not break the
  embedded build. Mitigation: keep `ble::frame` free of HAL deps; only
  `ble::rn4871` uses the I/O traits; run `just build` (embedded) after substep 2.
  Heapless version: VERIFIED — `embassy-stm32` 0.5 already depends on
  `heapless 0.9.1`, so `heapless = "0.9"` unifies with it (no new duplicate);
  `embassy-sync`'s separate `heapless 0.8` is pre-existing and internal to
  embassy, unaffected.
- **Firmware-version interop (< v1.40).** Android private-addressing and
  command-mode reboots on old firmware. Mitigation: `firmware_version` soft-gates
  (warn < 1.40, the caller may hard-fail < 1.28).

## Notes

Progress tracking (checked during `/tyrex:code:implement-light`):

- [x] 1. Shared wire-frame codec (`meteo-lib/src/ble/frame.rs` + constants) — 17-byte LE codec, sentinels, `present_fields` (pressure in Pa), `FrameField`; 9 host tests; added `heapless`/`embedded-hal`/`embedded-io-async`/`libm` deps. Spec review: pass.
- [x] 2. RN4871 driver (`meteo-lib/src/ble/rn4871.rs`) — async ASCII driver over `embedded-io-async`: command/response (No-Prompt), event buffering, provisioning, `LS` handle discovery, `SHW` hex push; 9 fake-based host tests. Spec review: pass.
- [x] 3. Firmware BLE task + supervisor (`meteo-firmware/src/ble.rs`, `main.rs`) — USART2 `BufferedUart` + `RST_N` (PA4), `SENSOR_CHANNEL`, `ble_task` with `select`-based supervisor + `bring_up`/`recover`. DEVIATION: pure `SensorSample`/`apply_sample` moved to `meteo-lib::ble::sample` (host-testable; firmware crate is arm-only and never host-tested) — 2 purity tests there. Spec review: pass.
- [x] 4. BMP388 task feeds `SENSOR_CHANNEL` (`bmp.rs`) — publishes `SensorSample::Barometer` each reading (done inline; arm clippy + build pass).
- [x] 5. `bluer` central transport (`meteo-tui/src/feed.rs`, `sensors.rs`) — scan→connect→discover→subscribe→decode→reconnect state machine, shutdown-aware; `field_to_index` mapping; 2 host tests. Spec review (inline): pass.
- [x] 6. Documentation (`CLAUDE.md`) — module tree, BLE wire contract, verify-and-repair provisioning, gaia testing note.

Open implementation choices left to the implementer (non-blocking):

- `PC` property `0x10` (Notify) vs `0x12` (Read + Notify); plan uses Notify per
  the brainstorm — switch to `0x12` if generic BLE tools need a readable value.
- `SR` may add `0x0080` (reboot-after-disconnect) as a belt-and-suspenders
  fallback to host re-advertise; left off to keep the host the sole supervisor.
