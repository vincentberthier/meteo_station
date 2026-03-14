# BLE GATT Service Implementation Plan

## Context

The RN4871 BLE driver is functional: hardware reset, command mode, configuration, status event monitoring all work. What's missing is the application layer: defining GATT services so a BLE client can read weather data, and piping sensor readings from the BMP388 task to the BLE task.

**User choices:**

- Private 128-bit UUIDs (custom MeteoStation service, not BLE SIG standard)
- Always define services on boot (simple, reliable, ~2s extra for reboot)
- Multiple sensors planned — design channel/GATT for extensibility

**Data format:** Since we use private UUIDs, send f32 values as 4 bytes little-endian. Simple and avoids BLE SIG encoding complexity.

---

## Step 1: Hex encoding helpers

**New file:** `crates/meteo-lib/src/ble/encoding.rs`

Single source of truth for hex encoding/decoding used by both command serialization and data encoding:

```rust
/// Encode f32 as 4 bytes little-endian.
#[expect(clippy::little_endian_bytes, reason = "BLE wire format is LE")]
pub fn encode_f32(value: f32) -> [u8; 4]

/// Decode 4 bytes little-endian into f32.
#[expect(clippy::little_endian_bytes, reason = "BLE wire format is LE")]
pub fn decode_f32(bytes: &[u8; 4]) -> f32

/// Write a byte slice as uppercase hex into buf.
/// Returns number of hex chars written (2 * data.len()), or None if buf too small.
pub fn bytes_to_hex(data: &[u8], buf: &mut [u8]) -> Option<usize>

/// Write a u8 as exactly 2 uppercase hex digits.
pub fn u8_to_hex(val: u8, buf: &mut [u8]) -> Option<usize>

/// Write a u16 as exactly 4 uppercase hex digits.
pub fn u16_to_hex(val: u16, buf: &mut [u8]) -> Option<usize>

/// Parse up to 4 hex chars as u16. Returns None on invalid hex or empty input.
pub fn parse_hex_u16(bytes: &[u8]) -> Option<u16>

/// Parse 32 hex chars as a 128-bit UUID byte array.
pub fn parse_uuid128(hex: &[u8]) -> Option<[u8; 16]>
```

Tests: encode/decode f32 round-trips, hex encoding of known values, u8/u16 padding (e.g. `0x05` → `"05"`, `0x001A` → `"001A"`), parse_hex_u16 valid/invalid, parse_uuid128 valid/invalid/wrong-length, negative temperatures, high pressures (~110000 Pa).

---

## Step 2: Add GATT commands to Command enum

**File:** `crates/meteo-lib/src/ble/rn4871/command.rs`

Add variants to `Command<'a>`:

| Variant                                                                 | Wire format                | ResponseType |
| ----------------------------------------------------------------------- | -------------------------- | ------------ |
| `ClearPrivateServices`                                                  | `PZ`                       | Aok          |
| `DefineService(&'a [u8; 16])`                                           | `PS,<32hex>`               | Aok          |
| `DefineCharacteristic { uuid: &'a [u8; 16], properties: u8, size: u8 }` | `PC,<32hex>,<2hex>,<2hex>` | Aok          |
| `ListServices`                                                          | `LS`                       | MultiLine    |
| `ServerWrite { handle: u16, data: &'a [u8] }`                           | `SHW,<4hex>,<2hex*N>`      | Aok          |

The `write_to()` implementations import hex helpers from `encoding.rs`:

- `ClearPrivateServices`: write `b"PZ"`
- `DefineService(uuid)`: write `PS,` then `bytes_to_hex(uuid, ...)`
- `DefineCharacteristic { uuid, properties, size }`: write `PC,` then `bytes_to_hex(uuid)`, `,`, `u8_to_hex(properties)`, `,`, `u8_to_hex(size)`
- `ListServices`: write `b"LS"`
- `ServerWrite { handle, data }`: write `SHW,` then `u16_to_hex(handle)`, `,`, `bytes_to_hex(data)`

Buffer budget: largest command is `PC,<32>,<2>,<2>` = `PC,` (3) + 32 + `,` (1) + 2 + `,` (1) + 2 = 41 bytes. Well within CMD_BUF_SIZE (64).

**Note on existing `write_features`:** The `SetFeatures` variant uses a hand-rolled hex encoder that strips leading zeros (e.g. `SR,2000` not `SR,00002000`) — the RN4871 `SR` command expects that format. Do not refactor it to use `u16_to_hex` (which always zero-pads to 4 digits). The two coexist for different wire format requirements.

Tests: `response_type()` and `write_to()` for each new variant, buffer-too-small edge cases.

---

## Step 3: Handle NFail response from SHW

**File:** `crates/meteo-lib/src/ble/rn4871/response.rs`

Add `Response::NFail` variant. The RN4871 returns `NFail` when SHW succeeds locally but notification delivery failed.

**File:** `crates/meteo-lib/src/ble/rn4871/parser.rs`

Add match arm: `b"NFail" => Response::NFail`.

**File:** `crates/meteo-lib/src/ble/driver.rs`

Three changes in driver.rs:

1. Add `NFail` to the `ResponseKind` enum (line 78-84).
2. Add arm to `ResponseKind::from_response()` (line 88-96): `Response::NFail => Self::NFail`
3. Update `wait_for()` (line 393-405) to treat NFail as success when expecting Aok:

```rust
async fn wait_for(&mut self, expected: ResponseKind) -> Result<(), Error<U::Error>> {
    loop {
        let kind = self.read_response_kind().await?;
        if kind == expected {
            return Ok(());
        }
        match kind {
            ResponseKind::Data => {}
            // NFail means the local write succeeded but notification delivery failed.
            // Treat as success — the characteristic value was still updated.
            ResponseKind::NFail if expected == ResponseKind::Aok => return Ok(()),
            ResponseKind::Err => return Result::Err(Error::CommandFailed),
            _ => return Result::Err(Error::UnexpectedResponse),
        }
    }
}
```

Tests (in `crates/meteo-lib/src/ble/rn4871/parser.rs` test module and `crates/meteo-lib/src/ble/driver.rs` test module — the existing `MockUart` already supports multi-response sequences via its read queue):

- `parser.rs`: parse `b"NFail"` → `Response::NFail`
- `driver.rs`: `execute()` with MockUart returning `b"NFail\r\nCMD> "` succeeds (NFail treated as Aok)
- `driver.rs`: `execute()` with MockUart returning `b"NFail\r\nCMD> "` for a ServerWrite command succeeds

---

## Step 4: LS output parser

**New file:** `crates/meteo-lib/src/ble/rn4871/ls_parser.rs`

Parse individual lines from the `LS` multi-line output to extract characteristic handles.

LS output format:

```
A4E64B8B8DB34E08A7D57D3C3F2E1A00       ← service UUID (no leading spaces)
  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,12   ← char UUID, handle, props (indented)
  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,12
END
```

```rust
pub struct CharacteristicInfo {
    pub uuid_bytes: [u8; 16],
    pub handle: u16,
}

/// Parse a characteristic line (starts with whitespace).
/// Returns None for service UUID lines or unrecognizable input.
pub fn parse_characteristic_line(line: &[u8]) -> Option<CharacteristicInfo>
```

Uses `parse_hex_u16` and `parse_uuid128` from `encoding.rs`.

Logic:

1. If line doesn't start with whitespace → None (service UUID line)
2. Strip leading whitespace
3. Split on `,`, require at least 2 fields (extra fields like config value are ignored)
4. First field: 32-char hex → `parse_uuid128` → uuid_bytes
5. Second field: up to 4-char hex → `parse_hex_u16` → handle
6. Return `Some(CharacteristicInfo { uuid_bytes, handle })`

Lines with 3 fields (`uuid,handle,props`) and 4 fields (`uuid,handle,props,config`) both parse successfully — extra fields beyond the second are ignored.

Tests: parse 3-field characteristic line, parse 4-field characteristic line (config value ignored, returns Some), service UUID lines (return None), malformed input (bad hex → None), END line (return None).

---

## Step 5: GATT service definitions and handle tracking

**New file:** `crates/meteo-lib/src/ble/gatt.rs`

```rust
/// MeteoStation custom service UUID: a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a00
pub const METEO_SERVICE_UUID: [u8; 16] = [
    0xa4, 0xe6, 0x4b, 0x8b, 0x8d, 0xb3, 0x4e, 0x08,
    0xa7, 0xd5, 0x7d, 0x3c, 0x3f, 0x2e, 0x1a, 0x00,
];

/// Temperature characteristic UUID: a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a01
pub const TEMPERATURE_CHAR_UUID: [u8; 16] = [
    0xa4, 0xe6, 0x4b, 0x8b, 0x8d, 0xb3, 0x4e, 0x08,
    0xa7, 0xd5, 0x7d, 0x3c, 0x3f, 0x2e, 0x1a, 0x01,
];

/// Pressure characteristic UUID: a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a02
pub const PRESSURE_CHAR_UUID: [u8; 16] = [
    0xa4, 0xe6, 0x4b, 0x8b, 0x8d, 0xb3, 0x4e, 0x08,
    0xa7, 0xd5, 0x7d, 0x3c, 0x3f, 0x2e, 0x1a, 0x02,
];

pub const PROP_READ: u8 = 0x02;
pub const PROP_NOTIFY: u8 = 0x10;
pub const PROP_READ_NOTIFY: u8 = 0x12;
pub const F32_SIZE: u8 = 4;

#[derive(Debug, Clone, Copy, Default)]
pub struct GattHandles {
    pub temperature: Option<u16>,
    pub pressure: Option<u16>,
}

/// Callback for query_multiline(ListServices, ...).
/// Matches characteristic UUIDs and stores their handles.
pub fn collect_handles(line: &[u8], handles: &mut GattHandles) {
    if let Some(info) = ls_parser::parse_characteristic_line(line) {
        if info.uuid_bytes == TEMPERATURE_CHAR_UUID {
            handles.temperature = Some(info.handle);
        } else if info.uuid_bytes == PRESSURE_CHAR_UUID {
            handles.pressure = Some(info.handle);
        }
    }
}
```

Extensibility: adding a sensor = add UUID constant + `GattHandles` field + match arm in `collect_handles`.

Tests: `collect_handles` with realistic LS output lines matching/not-matching UUIDs, verify both handles populated.

---

## Step 6: WC status event parsing

**File:** `crates/meteo-lib/src/ble/rn4871/status_event.rs`

Add variant:

```rust
/// Client changed notification/indication subscription (CCCD write).
/// handle: characteristic config handle. data: raw hex (e.g., b"0100" = notify on).
WriteConfig { handle: u16, data: &'a [u8] },
```

**File:** `crates/meteo-lib/src/ble/rn4871/status_parser.rs`

Add: `_ if inner.starts_with(b"WC,") => parse_wc_event(inner)`

`parse_wc_event`: split on commas, parse first field as hex u16 handle via `encoding::parse_hex_u16`, remaining bytes are data.

**File:** `crates/meteo-lib/src/ble/rn4871/format.rs`

Add `defmt::Format` arm for `WriteConfig`.

Tests: `WC,0072,0100` → `WriteConfig { handle: 0x0072, data: b"0100" }`, `WC,0072,0000` → unsubscribe, malformed `WC,` (no comma) → `Unknown`.

---

## Step 7: Module wiring in meteo-lib

**File:** `crates/meteo-lib/src/ble/mod.rs`

```rust
pub mod driver;
pub mod encoding;
pub mod gatt;
pub mod line_buffer;
pub mod rn4871;

pub use driver::{Error, Rn4871, Uart};
pub use encoding::{bytes_to_hex, decode_f32, encode_f32};
pub use gatt::{GattHandles, METEO_SERVICE_UUID, PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};
pub use line_buffer::LineBuffer;
pub use rn4871::{Command, StatusEvent, parse_status_event};
```

**File:** `crates/meteo-lib/src/ble/rn4871/mod.rs`

Add `pub mod ls_parser;`.

---

## Step 8: Inter-task communication

**File:** `crates/meteo-firmware/src/main.rs`

Add a static Channel:

```rust
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use meteo_lib::bmp388::Reading;

static SENSOR_CHANNEL: Channel<ThreadModeRawMutex, Reading, 1> = Channel::new();
```

Pass `&SENSOR_CHANNEL` to both `read_barometer` and `ble_task`:

```rust
spawner.spawn(bmp::read_barometer(i2c, &SENSOR_CHANNEL)).expect("...");
spawner.spawn(ble::ble_task(ble_uart, ble_rst_n, &SENSOR_CHANNEL)).expect("...");
```

Note: `embassy-sync` is a workspace dependency already used by `meteo-firmware` (target-gated under `cfg(target_arch = "arm")`). The `Channel` type only appears in `meteo-firmware` code, which is only compiled for the ARM target — never for host tests. No new dependencies needed.

---

## Step 9: Barometer task — publish readings

**File:** `crates/meteo-firmware/src/bmp.rs`

Add channel parameter to task signature:

```rust
pub async fn read_barometer(
    i2c: I2c<'static, Async, Master>,
    channel: &'static Channel<ThreadModeRawMutex, Reading, 1>,
)
```

After each successful read, publish: `let _ = channel.try_send(reading);`

Using `try_send` (non-blocking): if the channel is full (BLE hasn't consumed yet), the reading is dropped silently. Next one comes in 1s.

---

## Step 10: BLE task — GATT setup and data streaming

**File:** `crates/meteo-firmware/src/ble.rs`

### Task signature change

```rust
pub async fn ble_task(
    uart: BufferedUart<'static>,
    mut rst_n: Output<'static>,
    sensor_channel: &'static Channel<ThreadModeRawMutex, Reading, 1>,
)
```

### GATT setup sequence (after existing config, before exit command mode)

Insert between "set name" and "exit command mode":

1. `execute(ClearPrivateServices)` — PZ
2. `execute(DefineService(&METEO_SERVICE_UUID))` — PS
3. `execute(DefineCharacteristic { uuid: &TEMPERATURE_CHAR_UUID, properties: PROP_READ_NOTIFY, size: F32_SIZE })` — PC
4. `execute(DefineCharacteristic { uuid: &PRESSURE_CHAR_UUID, properties: PROP_READ_NOTIFY, size: F32_SIZE })` — PC
5. Exit command mode
6. Hardware reboot (rst_n low/high) to activate NVM-stored services
7. Wait for reboot
8. Re-enter command mode
9. `query_multiline(ListServices, |line| collect_handles(line, &mut handles))` — discover handles
10. Log discovered handles, warn if any are None
11. Exit command mode

### Monitoring loop — borrow-safe structure

The current code holds `let raw_uart = ble.uart_mut()` across the loop, which prevents calling `ble.enter_command_mode()` later. Fix: call `ble.uart_mut().read(...)` inline so the mutable borrow is temporary.

```rust
let mut connected = false;
let mut line_buf = LineBuffer::<256>::new();
let mut rx_buf = [0_u8; 64];

loop {
    // Phase 1: drain UART data with adaptive timeout.
    // The with_timeout(...).await expression creates a temporary borrow of `ble`
    // via uart_mut(). This borrow is dropped when the expression fully evaluates
    // (before the match arms execute), so `ble` is free to use in Phase 2.
    let timeout = if connected {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(5)
    };
    let uart_result = embassy_time::with_timeout(timeout, ble.uart_mut().read(&mut rx_buf)).await;
    // ble borrow released here — uart_result owns the Result, not the reference.
    match uart_result {
        Ok(Ok(n)) => {
            line_buf.push_bytes(&rx_buf[..n]);
            while line_buf.process_status_event(|event| {
                match parse_status_event(event) {
                    StatusEvent::Connect { .. } => connected = true,
                    StatusEvent::Disconnect => connected = false,
                    StatusEvent::WriteConfig { handle, data } => {
                        debug!("BLE: CCCD write handle={:04X} data={}", handle, data);
                    }
                    other => debug!("BLE: {:?}", other),
                }
            }) {}
            line_buf.for_each_line(|_| {});
        }
        Ok(Err(e)) => warn!("BLE UART read error: {:?}", Debug2Format(&e)),
        Err(_) => {} // timeout — fall through to check sensor data
    }

    // Phase 2: push sensor data when connected.
    // ble is no longer borrowed here — safe to call driver methods.
    if connected {
        if let Ok(reading) = sensor_channel.try_receive() {
            if let (Some(t_handle), Some(p_handle)) = (handles.temperature, handles.pressure) {
                let t_bytes = encode_f32(reading.temperature);
                let p_bytes = encode_f32(reading.pressure);
                if let Err(e) = ble.enter_command_mode().await {
                    warn!("BLE: cmd mode failed: {:?}", Debug2Format(&e));
                    continue;
                }
                let _ = ble.execute(Command::ServerWrite { handle: t_handle, data: &t_bytes }).await;
                let _ = ble.execute(Command::ServerWrite { handle: p_handle, data: &p_bytes }).await;
                if let Err(e) = ble.exit_command_mode().await {
                    warn!("BLE: exit cmd mode failed: {:?}", Debug2Format(&e));
                }
            }
        }
    }
}
```

The timeout adapts: 5s disconnected (save CPU), 100ms connected (responsive). The `ble.uart_mut().read()` borrow is released before Phase 2, so `ble.enter_command_mode()` etc. compile without conflict.

---

## Step 11: BLE client CLI (`meteo-cli`)

**New crate:** `crates/meteo-cli/`

A desktop Rust binary that connects to the weather station over BLE and prints readings. Uses `btleplug` for BLE and `tokio` runtime. First step toward a full TUI client.

### Shared definitions

The CLI imports UUIDs from `meteo_lib::ble::gatt` and `decode_f32` from `meteo_lib::ble::encoding`. These modules use only core types (`[u8; N]`, `f32`) — they work in both `no_std` and `std`. `meteo-lib` already compiles on the host for its test suite (`cargo nextest run` runs on x86_64), so pulling it in from the CLI is safe. Its `embedded-hal-async` dependency is a traits-only crate that compiles on any target.

### Crate setup

**File:** `Cargo.toml` (workspace root) — add `"crates/meteo-cli"` to members.

**File:** `crates/meteo-cli/Cargo.toml`

```toml
[package]
name = "meteo-cli"
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
meteo-lib = { workspace = true }
btleplug = "0.11"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
uuid = "1"
```

### CLI behavior (`crates/meteo-cli/src/main.rs`)

1. Scan for BLE peripherals, find one named "MeteoStation"
2. Connect
3. Discover services, match `METEO_SERVICE_UUID` (convert with `uuid::Uuid::from_bytes()`)
4. Find temperature and pressure characteristics by UUID
5. Read initial values, decode with `decode_f32`, print
6. Subscribe to notifications on both characteristics
7. Print each notification: `Temperature: 23.45°C  Pressure: 101325.0 Pa (1013.25 hPa)`
8. On Ctrl+C: disconnect gracefully

Tests (in `crates/meteo-cli/src/main.rs` test module):

- `uuid_from_bytes_matches_expected_string`: `uuid::Uuid::from_bytes(METEO_SERVICE_UUID).to_string() == "a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a00"` — validates byte-order assumption between meteo-lib and btleplug
- `decode_sensor_reading_round_trip`: encode then decode a realistic temperature (23.45) and pressure (101325.0), assert equality

BLE connection behavior is integration-tested manually via Step 6 of Verification.

---

## Implementation Order

1. **Step 1** — hex/encoding helpers (foundation, no dependencies)
2. **Step 2** — new commands (depends on Step 1 for hex helpers)
3. **Step 3** — NFail handling (independent)
4. **Step 4** — LS parser (depends on Step 1 for parse_hex_u16/parse_uuid128)
5. **Step 5** — GATT definitions (depends on Steps 1, 4)
6. **Step 6** — WC events (depends on Step 1 for parse_hex_u16)
7. **Step 7** — module wiring (depends on 1-6)
8. **Steps 8-10** — firmware integration (depends on all above)
9. **Step 11** — CLI client (depends on Steps 1, 5, 7)

Steps 1-4 and 6 are independent once Step 1 is done.

---

## Verification

1. **Unit tests**: `cargo nextest run` — all new parsers, encoders, command serialization
2. **Clippy**: `cargo make clippy` — no warnings
3. **Build firmware**: `cargo make build` — thumbv7em target compiles
4. **Build CLI**: `cargo build -p meteo-cli` — host target compiles
5. **On-device test**: `cargo make run` — verify via RTT logs:
   - GATT setup sequence completes (PZ, PS, PC, reboot, LS)
   - Handles discovered and logged (e.g. `temperature=0x0072, pressure=0x0075`)
6. **End-to-end test**: `cargo run -p meteo-cli` while device is powered:
   - CLI discovers and connects to MeteoStation
   - Reads temperature + pressure
   - Notifications stream at ~1 Hz

---

## Key Files

| File                                               | Change                                       |
| -------------------------------------------------- | -------------------------------------------- |
| `crates/meteo-lib/src/ble/encoding.rs`             | **New** — hex helpers, f32 encode/decode     |
| `crates/meteo-lib/src/ble/rn4871/command.rs`       | New command variants + write_to()            |
| `crates/meteo-lib/src/ble/rn4871/response.rs`      | Add NFail variant                            |
| `crates/meteo-lib/src/ble/rn4871/parser.rs`        | Parse NFail                                  |
| `crates/meteo-lib/src/ble/driver.rs`               | ResponseKind::NFail + wait_for() update      |
| `crates/meteo-lib/src/ble/rn4871/ls_parser.rs`     | **New** — LS output parser                   |
| `crates/meteo-lib/src/ble/gatt.rs`                 | **New** — UUIDs, handles, constants          |
| `crates/meteo-lib/src/ble/rn4871/status_event.rs`  | Add WriteConfig variant                      |
| `crates/meteo-lib/src/ble/rn4871/status_parser.rs` | Parse WC events                              |
| `crates/meteo-lib/src/ble/rn4871/format.rs`        | defmt Format for new types                   |
| `crates/meteo-lib/src/ble/rn4871/mod.rs`           | Add ls_parser module                         |
| `crates/meteo-lib/src/ble/mod.rs`                  | Wire new modules + re-exports                |
| `crates/meteo-firmware/src/main.rs`                | Static Channel, pass to tasks                |
| `crates/meteo-firmware/src/bmp.rs`                 | Publish readings to channel                  |
| `crates/meteo-firmware/src/ble.rs`                 | GATT setup + borrow-safe data streaming loop |
| `crates/meteo-cli/Cargo.toml`                      | **New** — CLI crate with btleplug            |
| `crates/meteo-cli/src/main.rs`                     | **New** — BLE client CLI                     |
