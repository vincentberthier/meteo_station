# Plan: BLE Link Soak Test (no GATT)

- **Source:** '2 (`.claude/brainstorm/2-ble-link-soak-test.md`)
- **Date:** 2026-06-15
- **Status:** Done

## Summary

Build a minimal BLE link on the RN4871 with **no GATT, no services, no
telemetry** — written **from scratch against `datasheets/rn4871.md`**. Two
deliverables: (A) firmware that brings the RN4871 up over USART2, provisions it
for continuous connectable advertising, and re-advertises on disconnect plus a
periodic keepalive and an RST_N wedge recovery; (B) a self-validating gaia bash
soak harness that drives connect → hold 6 min → disconnect → wait 90 s →
reconnect indefinitely, polling the link every second and failing loud
(non-zero exit) on any mid-window drop or failed reconnect.

## Honesty preamble — read before implementing

**The previous BLE work never produced a working link.** Across five `fix(ble)`
commits the device never held a connection for the 6-minute target and never
advertised consistently. The root cause was never found. Therefore:

- The history files (revision `snlwmrollztk`:
  `crates/meteo-lib/src/ble/rn4871.rs`, `crates/meteo-firmware/src/ble.rs`) are
  **reference notes only** — a record of what was tried, not a source of trusted
  code. Do **not** copy them verbatim. Re-derive the driver, the protocol
  handling, and the tests from the datasheet.
- Every numeric tuning value below (advertising/connection intervals,
  supervision timeout, TX power) is **design intent, not a known-good setting**.
  The only acceptance evidence is the live soak test. Treat each value as a
  hypothesis to validate, and expect to iterate.
- Host unit tests prove only that the **parser** does what its assertions say in
  isolation. They say nothing about RF behaviour. Do not let a green test suite
  be mistaken for a working link.

History observations worth heeding (caution, not fact): on V1.30, a bare
`A,<int>` reportedly went silent after ~30 s; an `STA` fast-timeout of `0`
reportedly left the module reporting "advertising" while nothing radiated; and
the module reportedly answered errors as lowercase `Err`. The datasheet
documents none of these. The plan defends against all three, but they are
unverified — confirm or refute them live.

## Datasheet-derived facts (the trusted basis)

From `datasheets/rn4871.md`:

- UART default 115200 8N1, no flow control. Commands end with `\r` (no LF).
  Success `AOK\r\n`, failure `ERR\r\n`. Prompt `CMD> ` unless suppressed with
  `SR` bit `0x4000` (No-Prompt).
- Command mode: `$$$` (100 ms silence before the first `$`) → `CMD>`. Exit:
  `---\r` → `END`.
- Status events are delimiter-framed in `%…%` (e.g. `%REBOOT%`,
  `%CONNECT,<0-1>,<addr>%`, `%DISCONNECT%`, `%STREAM_OPEN%`) and are **not**
  newline-terminated.
- Set commands (NVM, need reboot): `SN` name, `SS` default-services bitmap
  (`SS,00` = none), `SA,2` = No-Input-No-Output, `SGA`/`SGC` TX power (`0` = 0 dBm
  = max; table §"TX Power Levels"), `ST,<min>,<max>,<lat>,<to>` connection
  params, `STA,<fast_int>,<fast_to>,<slow_int>` advertisement timing,
  `SR,4000` No-Prompt. `SF,2` = full factory reset (immediate reboot).
- Action commands: `A[,<int>,<to>]` start advertising, `R,1` reboot, `V`
  firmware version.
- Reset pulse minimum 63 ns (datasheet recommends > 1 ms); power-on-to-UART 46 ms,
  full init 68 ms — but the design waits on the real `%REBOOT%` event, not a fixed
  delay.

The datasheet does **not** give units for `STA`/`ST` numeric fields. The plan
uses BLE-standard units as the working hypothesis (advertising interval ×0.625 ms;
connection interval ×1.25 ms; supervision timeout ×10 ms) and validates live.

## Tuning values (design intent — UNVALIDATED, confirmed only by the soak)

| Topic                | Value                                                                                                | Rationale (hypothesis)                                                                                                                                                                                                                                                               |
| -------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Advertising          | `STA,0020,FFFF,0020`; start `A,0020`                                                                 | 20 ms fast interval for best discovery on the marginal link; a large non-zero fast-timeout (`FFFF`) so the module never drops to slow advertising, and a slow interval equal to the fast one. Non-zero timeout chosen because history reported `0` = nothing radiates. **Unproven.** |
| Firmware conn params | `ST,0006,000C,0000,0258` (7.5–15 ms, latency 0, 6 s supervision)                                     | Peripheral-preferred params set equal to gaia's so the two ends agree and avoid an L2CAP param-update renegotiation. **Unproven.**                                                                                                                                                   |
| Gaia conn params     | debugfs `conn_min_interval=6`, `conn_max_interval=12`, `supervision_timeout=600`, reapplied each run | Fast interval to help the one-time GAP auto-resolution complete; 6 s supervision to ride out loss bursts across the 6-min hold. BLE min-supervision constraint (`to > (1+lat)·max_int·2` = 30 ms) is met. **Unproven.**                                                              |
| TX power             | `SGA,0` / `SGC,0` (0 dBm, max)                                                                       | Datasheet table confirms `0` = highest power.                                                                                                                                                                                                                                        |
| Provisioning         | full provision every boot (`SF,2` → set-commands → `R,1`)                                            | User's explicit choice. Deterministic, no NVM drift. `WR` not used.                                                                                                                                                                                                                  |
| Link-state poll      | `busctl` read of `org.bluez.Device1.Connected`                                                       | Authoritative D-Bus property; least racy under `blueman-manager`'s standing discovery. Avoids parsing `bluetoothctl info` text and avoids starting a second scan.                                                                                                                    |
| Wedge recovery       | any UART error from `next_event` → pulse RST_N → re-bring-up                                         | Deadlock circuit-breaker with an explicit failure path; no guessed response-timeout.                                                                                                                                                                                                 |

If the live soak drops, the first knobs to turn are the conn-interval/supervision
values (see Risks), not another code patch.

## Files Modified

| File                                 | Action             | Description                                                                                               |
| ------------------------------------ | ------------------ | --------------------------------------------------------------------------------------------------------- |
| `Cargo.toml` (workspace)             | modify             | Add `embedded-io-async = "0.7"`, `heapless = "0.9"` to `[workspace.dependencies]`.                        |
| `crates/meteo-lib/Cargo.toml`        | modify             | Add `embedded-io-async`, `heapless` deps.                                                                 |
| `crates/meteo-lib/src/lib.rs`        | modify             | Add `pub mod ble;`.                                                                                       |
| `crates/meteo-lib/src/ble/mod.rs`    | create             | Module root: `//!` doc + `pub mod rn4871;`. No UUIDs/constants.                                           |
| `crates/meteo-lib/src/ble/rn4871.rs` | create             | RN4871 driver written from the datasheet.                                                                 |
| `crates/meteo-firmware/Cargo.toml`   | modify             | Add `embedded-io-async`; `embassy-futures`/`embassy-sync`/`embassy-time`/`embassy-stm32` already present. |
| `crates/meteo-firmware/src/ble.rs`   | create             | BLE supervisor task (no channel, no frames).                                                              |
| `crates/meteo-firmware/src/main.rs`  | modify             | Bind USART2 IRQ, wire `BufferedUart` (PD6/PD5) + RST_N (PA4), spawn `ble::ble_task`.                      |
| `scripts/ble_soak.sh`                | create             | gaia self-validating soak harness (`scripts/` does not yet exist — create it).                            |
| `CLAUDE.md`                          | modify             | Restore USART2/RST_N/RN4871 pin rows + gaia soak-test procedure.                                          |
| `README.md`                          | modify (or create) | Document the soak script usage/deployment.                                                                |

## Plan

### 1. Workspace + crate dependencies

**Files:** `Cargo.toml`, `crates/meteo-lib/Cargo.toml`, `crates/meteo-firmware/Cargo.toml`.

`embedded-io-async 0.7.0` and `heapless 0.9.1` are already present transitively
(verified in `Cargo.lock`); pin those majors.

`Cargo.toml` `[workspace.dependencies]` — add:

```toml
embedded-io-async = "0.7"
heapless = "0.9"
```

`crates/meteo-lib/Cargo.toml` `[dependencies]` — add `embedded-io-async` and
`heapless` (both `{ workspace = true }`) alongside the existing `embedded-hal`,
`embedded-hal-async`, `libm`, optional `defmt`. The driver's host tests use
`embedded-hal`/`embedded-hal-async`/`embedded-io-async` directly, so these are
normal deps, not dev-only. Keep existing dev-deps (`test-log`, `env_logger`,
`tokio`).

`crates/meteo-firmware/Cargo.toml`
`[target.'cfg(target_arch = "arm")'.dependencies]` — add
`embedded-io-async = { workspace = true }`. USART is built into `embassy-stm32`
0.5 (no extra feature; `BufferedUart`/`BufferedInterruptHandler` are
unconditional). No other new deps.

**Test:** `cargo build -p meteo-lib --target x86_64-unknown-linux-gnu` resolves
dependencies (full build green after step 2).

**Depends on:** nothing. Do first.

### 2. RN4871 driver, written from the datasheet (`meteo-lib`)

**Files:** `crates/meteo-lib/src/ble/mod.rs` (create),
`crates/meteo-lib/src/ble/rn4871.rs` (create), `crates/meteo-lib/src/lib.rs` (modify).

Write fresh. The history file may be read as a reference for what the protocol
edge-cases were, but the implementation and tests are re-derived from
`datasheets/rn4871.md` and the protocol facts above.

`mod.rs`:

```rust
//! BLE module: minimal RN4871 link driver (no GATT, no services).
pub mod rn4871;
```

`lib.rs`: add `pub mod ble;` next to the existing sensor re-exports.

**Public types:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event { Reboot, Connect, Disconnect, StreamOpen, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    Io(E),        // UART read/write failed
    Command,      // module returned ERR (case-insensitive)
    Timeout,      // reserved for a caller-added wedge timeout
    BadResponse,  // malformed/unexpected response (e.g. version parse)
}
// + Display and core::error::Error impls.

pub struct Rn4871<U, R, D> {
    uart: U,
    reset: R,
    delay: D,
    events: heapless::Deque<Event, 4>,
}

impl<U, R, D, E> Rn4871<U, R, D>
where
    U: embedded_io_async::Read<Error = E> + embedded_io_async::Write<Error = E>,
    R: embedded_hal::digital::OutputPin,
    D: embedded_hal_async::delay::DelayNs,
{
    pub const fn new(uart: U, reset: R, delay: D) -> Self { /* events: Deque::new() */ }
}
```

**Private line model + classifier** (derived directly from the protocol: lines
end in `\n`; events are `%…%` with no newline; the prompt `CMD> ` has no newline):

```rust
#[derive(PartialEq, Eq)]
enum Line { Aok, Err, Event(Event), Prompt, Data }

fn classify(line: &[u8]) -> Line {
    // Case-insensitive AOK/ERR: defends against the reported lowercase `Err`.
    if line.eq_ignore_ascii_case(b"AOK") { Line::Aok }
    else if line.eq_ignore_ascii_case(b"ERR") { Line::Err }
    else if line.len() >= 2 && line.first() == Some(&b'%') && line.last() == Some(&b'%') {
        let inner = &line[1..line.len() - 1];
        Line::Event(
            if inner == b"REBOOT" { Event::Reboot }
            else if inner.starts_with(b"CONNECT") { Event::Connect }
            else if inner.starts_with(b"DISCONNECT") { Event::Disconnect }
            else if inner == b"STREAM_OPEN" { Event::StreamOpen }
            else { Event::Other })
    }
    else if line.starts_with(b"CMD>") { Line::Prompt }
    else { Line::Data }
}
```

**Methods** (all `async`, byte-at-a-time reads for cancel-safety):

- `write_all(&[u8]) -> Result<(), Error<E>>`, `read_byte() -> Result<u8, Error<E>>`
  — thin UART wrappers, map errors to `Error::Io`. (All driver methods that touch
  the UART are `async` and return `Result<_, Error<E>>`; the bullets above name
  only the success type where the `Result` wrapper is implied — write it out in
  the actual signatures.)
- `read_line(&mut HVec<u8,64>) -> Result<(), Error<E>>` — fills the buffer with
  one message and returns on the first of: `\n`
  (strip trailing `\r`); a complete `%…%` frame (buffer starts `%` and the byte
  just pushed is `%`, len ≥ 2); the buffer ends with `CMD> `; or the buffer is
  full. This is the crux of correct RN4871 framing and must have its own tests.
- `reset()` — `reset.set_low(); delay 2 ms; reset.set_high();` then read lines
  until `%REBOOT%`. (Wait on the event, never a fixed settle delay.)
- `enter_command_mode()` — 100 ms guard, write `$$$`, 100 ms guard, then confirm
  command mode by issuing `V` via the two-arg `query` and discarding the version
  line: `let mut buf = [0u8; 64]; let _n = self.query(b"V", &mut buf).await?;`.
  With prompts on (factory default,
  before `SR,4000`) the module emits a leading `CMD> ` and a trailing `CMD> `
  around the version line; `query` skips both (see its semantics below), and any
  trailing `CMD> ` left unread is harmlessly skipped by the next `command`. In
  No-Prompt mode (after `SR,4000`) there is no `CMD>` at all.
- `command(&[u8])` — write `cmd\r`; read lines, routing `Event` lines into
  `self.events`, **skipping `Prompt` and `Data` lines**, returning `Ok` on `Aok` /
  `Err(Command)` on `Err`. Because it skips `Prompt`, a stray `CMD> ` left in the
  stream by a prior `query` cannot poison it.
- `query(&[u8], &mut [u8]) -> Result<usize, Error<E>>` — write `cmd\r`, then read
  lines: route `Event` lines into `self.events`, **skip `Prompt` lines**, and
  return the byte count of the **first `Data`/`Aok`/`Err` line** (copied into the
  caller buffer); propagates `Error::Io` on UART failure. It does
  **NOT** wait for a subsequent `AOK` — `V` never produces one, so waiting would
  stall. A trailing `CMD> ` after the returned line is left in the stream and
  skipped by the next `command`/`query` call (both ignore `Prompt`). State this
  exact semantic in a doc-comment; the FakeUart rx streams in the tests depend on
  it.
- `firmware_version() -> Result<(u8,u8), Error<E>>` — uses the two-arg `query` with a local stack
  buffer: `let mut buf = [0u8; 64]; let n = self.query(b"V", &mut buf).await?;`
  then parse `&buf[..n]` for `…V<major>.<minor>…`; `Error::BadResponse` on parse
  failure.
- `reboot()` — write `R,1\r`, await `%REBOOT%` (it answers with the event, not
  `AOK`, so it cannot go through `command`).
- `factory_reset()` — write `SF,2\r`, await `%REBOOT%`.
- `provision()` — the no-GATT bring-up sequence (below).
- `start_advertising()` — `command(b"A,0020")` (await `AOK`).
- `restart_advertising()` — `write_all(b"A,0020\r")` fire-and-forget (awaiting
  `AOK` here can wedge if a central reconnects first; a stray `AOK` is skipped by
  `next_event`).
- `next_event() -> Result<Event, Error<E>>` — pop a buffered event, else read
  lines until one classifies as `Event`, skipping prompts/acks/data; propagates
  `Error::Io` on UART failure (this is the `Err(_)` arm the supervisor's
  `select` matches in step 3).
- `take_buffered_event() -> Option<Event>` — non-blocking drain of `self.events`.
  (Kept for the supervisor to drain an event that arrived mid-command.)

**`provision()` body** (datasheet init sequence, adapted to no-GATT/no-services):

```rust
pub async fn provision(&mut self) -> Result<(), Error<E>> {
    self.factory_reset().await?;           // SF,2 — known-clean NVM
    self.enter_command_mode().await?;
    self.command(b"SN,MeteoStation").await?;
    self.command(b"SS,00").await?;         // no default services (pure GAP)
    self.command(b"SA,2").await?;          // No-Input-No-Output (no pairing UI)
    self.command(b"SGA,0").await?;         // max TX power, advertising
    self.command(b"SGC,0").await?;         // max TX power, connection
    self.command(b"ST,0006,000C,0000,0258").await?;
    self.command(b"STA,0020,FFFF,0020").await?;
    self.command(b"SR,4000").await?;       // No-Prompt for clean MCU parsing
    self.reboot().await?;                  // R,1 — activates NVM config
    self.enter_command_mode().await?;
    Ok(())
}
```

**Tests** (host, `cargo nextest run -p meteo-lib --target x86_64-unknown-linux-gnu`).
Write a `FakeUart` (rx byte queue + tx capture), `FakePin` (Infallible),
`FakeDelay` (implements `DelayNs::delay_ns` as an empty async no-op; the
`delay_ms` guards in `enter_command_mode` resolve instantly). Re-derive, do not
copy. Each test names the behaviour and asserts it:

- `read_line_returns_event_without_trailing_newline` — feed `%DISCONNECT%`
  (no `\n`); assert one line returned containing the frame.
- `read_line_returns_prompt_without_newline` — feed `CMD> `; assert returned.
- `classify_recognises_aok_err_and_lowercase_err` — `AOK`/`ERR`/`Err`.
- `command_returns_ok_on_aok` / `command_returns_err_on_err`.
- `command_routes_events_while_awaiting_aok` — `%CONNECT…%` then `AOK`; assert
  `Ok` and the event is later returned by `next_event`.
- `next_event_parses_disconnect` — feed `%DISCONNECT%`; assert the call returns
  `Ok(Event::Disconnect)`.
- `reset_completes_on_reboot`.
- `firmware_version_parses_major_minor` — `RN4871 V1.30 …` → `(1, 30)`.
- `provision_emits_no_gatt_sequence` — drive `provision` with the **exact**
  FakeUart rx byte stream below (concatenated in order; given the `query`
  semantics above, no `CMD> ` bytes are needed — `query` returns on the first
  `Data` line and any prompt would only be skipped):

  ```text
  %REBOOT%            (factory_reset SF,2 — event, no newline)
  RN4871 V1.30\r\n    (enter_command_mode #1: query V → version Data line)
  AOK\r\n             ×8  (SN, SS,00, SA,2, SGA,0, SGC,0, ST, STA, SR — in order)
  %REBOOT%            (reboot R,1 — event, no newline)
  RN4871 V1.30\r\n    (enter_command_mode #2: query V, No-Prompt active)
  ```

  As a Rust literal:
  `b"%REBOOT%RN4871 V1.30\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\n%REBOOT%RN4871 V1.30\r\n"`.
  Assert `tx` **contains** `SN,MeteoStation\r`, `SS,00\r`, `SA,2\r`, `SGA,0\r`,
  `SGC,0\r`, `ST,0006,000C,0000,0258\r`, `STA,0020,FFFF,0020\r`, `SR,4000\r`,
  `SF,2\r`, `R,1\r` and **does not contain** `PS,`, `PC,`, `SHW`. Locks the
  no-GATT contract and the chosen STA/ST strings.

- `provision_propagates_command_error` — same stream but replace the **third**
  `AOK\r\n` (the `SA,2` reply) with `Err\r\n`; assert `provision` returns
  `Err(Error::Command)` and that `tx` does **not** contain `STA,` (it bailed
  before reaching it). Covers the provisioning error path.
- `start_advertising_sends_a_with_interval` — feed rx `AOK\r\n`; assert `tx` ends
  with `A,0020\r` and the call returns `Ok` (`command` appends the `\r`).
- `restart_advertising_is_fire_and_forget` — empty rx queue; assert `tx ==
A,0020\r` and the call returns `Ok` (it only `write_all`s, never reads).

**Depends on:** step 1.

### 3. Firmware BLE supervisor task (`meteo-firmware`)

**File:** `crates/meteo-firmware/src/ble.rs` (create). No sensor channel, no
frames — the link carries no data.

```rust
async fn bring_up_once<U, R, D, E>(dev: &mut Rn4871<U, R, D>)
    -> Result<(), meteo_lib::ble::rn4871::Error<E>>
where U: Read<Error = E> + Write<Error = E>, R: OutputPin, D: DelayNs, E: core::fmt::Debug {
    // reset() + enter_command_mode() here verify the UART link and read the
    // version BEFORE programming NVM. provision() then does its OWN factory_reset
    // + enter_command_mode — the apparent double reset is intentional: this first
    // pair is a comms/health probe, the second is the clean-slate for NVM writes.
    dev.reset().await?;
    dev.enter_command_mode().await?;
    match dev.firmware_version().await {
        Ok((maj, min)) => info!("RN4871 firmware: {}.{}", maj, min),
        Err(e) => warn!("RN4871 version unreadable: {:?}", Debug2Format(&e)),
    }
    dev.provision().await?;
    dev.start_advertising().await?;
    Ok(())
}

async fn bring_up(dev) { loop { match bring_up_once(dev).await {
    Ok(()) => { info!("BLE: advertising started"); return; }
    Err(e) => error!("BLE bring-up failed: {:?}, retrying", Debug2Format(&e)),
}}}

async fn recover(dev) { warn!("BLE: RST_N wedge recovery"); dev.reset().await.ok(); bring_up(dev).await; }

#[embassy_executor::task]
pub async fn ble_task(uart: BufferedUart<'static>, reset: Output<'static>) {
    let mut dev = Rn4871::new(uart, reset, Delay);
    bring_up(&mut dev).await;
    let mut connected = false;
    let mut keepalive = Ticker::every(Duration::from_secs(30));
    loop {
        match select(keepalive.next(), dev.next_event()).await {
            Either::First(()) => if !connected {
                info!("BLE: keepalive re-arm advertising");
                dev.restart_advertising().await.ok();
            },
            Either::Second(Ok(Event::Connect)) => { info!("BLE: connected"); connected = true; }
            Either::Second(Ok(Event::Disconnect)) => {
                info!("BLE: disconnected, re-advertising");
                connected = false; dev.restart_advertising().await.ok();
            }
            Either::Second(Ok(_)) => info!("BLE: event"),
            Either::Second(Err(_)) => { error!("BLE: UART error, recovering"); recover(&mut dev).await; connected = false; }
        }
    }
}
```

`Ticker`/`Duration` from `embassy_time`; `select`/`Either` from
`embassy_futures::select`; `BufferedUart` from `embassy_stm32::usart`; `Output`
from `embassy_stm32::gpio`; `Delay` from `embassy_time`. The keepalive `Ticker`
is a periodic maintenance re-arm (defence-in-depth behind the continuous `STA`),
**not** a synchronisation sleep — the real disconnect signal is `next_event`.

**Test:** hardware-interfacing — no host test. Validation is the firmware build
(step 6) and the live soak (Testing).

**Depends on:** step 2.

### 4. Firmware hardware wiring (`main.rs`)

**File:** `crates/meteo-firmware/src/main.rs` (modify).

- `mod ble;` beside `mod bmp;`/`mod leds;`.
- Imports: `use embassy_stm32::usart::{BufferedUart, Config as UartConfig};`,
  `use static_cell::StaticCell;` (`Output`/`Level`/`Speed` already imported).
- Extend `bind_interrupts!`:
  `USART2 => embassy_stm32::usart::BufferedInterruptHandler<peripherals::USART2>;`
- `static TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();`
  `static RX_BUF: StaticCell<[u8; 256]> = StaticCell::new();`
- After the BMP388 spawn:

```rust
// USART2 for RN4871 BLE module (CN9): D52 = PD6 (RX), D53 = PD5 (TX).
// RST_N = PA4 (CN7 pin 17), active-low, deasserted high at init.
let uart = BufferedUart::new(
    p.USART2, p.PD6, p.PD5,
    TX_BUF.init([0_u8; 256]), RX_BUF.init([0_u8; 256]),
    Irqs, UartConfig::default(),
)
.expect("USART2 init");
let rst_n = Output::new(p.PA4, Level::High, Speed::Low);
spawner.spawn(ble::ble_task(uart, rst_n)).expect("ble_task already spawned");
```

`BufferedUart::new` argument order `(peri, rx, tx, tx_buffer, rx_buffer, irq,
config)` is the embassy-stm32 0.5 signature (confirmed against the crate source);
default `UartConfig` is 115200 8N1, the RN4871 default. If the API has drifted,
defer to the compiler rather than guessing.

**Test:** `cargo build -p meteo-firmware` succeeds; `just clippy` clean.

**Depends on:** step 3.

### 5. gaia soak script (`scripts/ble_soak.sh`)

**File:** `scripts/ble_soak.sh` (create dir + file, `chmod +x`). Runs **on gaia**
(BlueZ 5.86; `busctl`, `bluetoothctl`, `doas` present). Bash skill rules:
`#!/usr/bin/env bash`, `set -euo pipefail`, ShellCheck-clean, every expansion
quoted.

**Config block (env-overridable):**

```bash
DEVICE="${DEVICE:-80:1F:12:B6:60:BF}"
ADAPTER="${ADAPTER:-hci0}"
HOLD_SECS="${HOLD_SECS:-360}"
GAP_SECS="${GAP_SECS:-90}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-30}"
CONN_MIN="${CONN_MIN:-6}"; CONN_MAX="${CONN_MAX:-12}"; SUPERVISION="${SUPERVISION:-600}"
DBUS_PATH="/org/bluez/${ADAPTER}/dev_${DEVICE//:/_}"
DEBUGFS="/sys/kernel/debug/bluetooth/${ADAPTER}"
```

**Functions:**

- `log()` — `printf '%s %s\n' "$(date -Is)" "$*"`.
- `cleanup()` — `bluetoothctl disconnect "$DEVICE" >/dev/null 2>&1 || true`.
- `fail()` — `log "FAIL(cycle=$1): $2"; cleanup; exit 1`.
- `apply_conn_params()` — write the three debugfs files via `doas tee`
  (`printf '%s' "$CONN_MIN" | doas tee "$DEBUGFS/conn_min_interval" >/dev/null`,
  likewise `conn_max_interval`, `supervision_timeout`). They reset on every
  `systemctl restart bluetooth`, so reapply each run.
- `is_connected()` — note the leading `if` so a non-zero `grep` does not trip
  `set -e`:

  ```bash
  is_connected() {
      busctl get-property org.bluez "$DBUS_PATH" org.bluez.Device1 Connected \
          2>/dev/null | grep -q 'b true'
  }
  ```

  `busctl` prints `b true` / `b false`; a D-Bus error (device absent from the
  object tree) goes to the suppressed stderr and `grep` returns 1 ⇒ "not
  connected", the correct semantics.

- `device_known()` — preflight cache check (the script never scans, so it relies
  on blueman's standing discovery to have populated the device):

  ```bash
  device_known() { bluetoothctl info "$DEVICE" 2>/dev/null | grep -q 'Device '; }
  ```

- `wait_known()` — bounded wait for the device to appear in the cache, run once
  before the first cycle. If it never appears, the device is not advertising (or
  blueman's discovery is off) ⇒ actionable fail, not a confusing `connect` error:

  ```bash
  wait_known() {
      local n=0
      until device_known; do
          sleep 1; n=$((n + 1))
          [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 1
      done
  }
  ```

- `wait_connected()` — issue the connect, then bounded poll (the `|| true` keeps
  a failed `connect` from tripping `set -e`; the loop is the real arbiter):

  ```bash
  wait_connected() {
      bluetoothctl connect "$DEVICE" >/dev/null 2>&1 || true
      local n=0
      until is_connected; do
          sleep 1; n=$((n + 1))
          [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 1
      done
  }
  ```

- `hold()` — loop `HOLD_SECS` times: `sleep 1`; `is_connected || return 1` (a
  drop before the window closes is a FAIL):

  ```bash
  hold() {
      local n=0
      while [ "$n" -lt "$HOLD_SECS" ]; do
          sleep 1; n=$((n + 1))
          is_connected || return 1
      done
  }
  ```

- `disconnect()` — request disconnect, bounded poll until down:

  ```bash
  disconnect() {
      bluetoothctl disconnect "$DEVICE" >/dev/null 2>&1 || true
      local n=0
      while is_connected; do
          sleep 1; n=$((n + 1))
          [ "$n" -ge "$CONNECT_TIMEOUT" ] && return 0   # best-effort; next cycle re-checks
      done
  }
  ```

**Main loop:**

```bash
trap 'log "interrupted"; cleanup; exit 0' INT
apply_conn_params
if ! wait_known; then
    log "FATAL: $DEVICE not in BlueZ cache within ${CONNECT_TIMEOUT}s."
    log "  -> ensure the device is powered/advertising and blueman discovery is running."
    log "  -> after a 'systemctl restart bluetooth', let blueman re-discover before re-running."
    exit 1
fi
cycle=0
while true; do
    cycle=$((cycle + 1))
    log "cycle=$cycle: connecting"
    wait_connected || fail "$cycle" "connect/re-advertise within ${CONNECT_TIMEOUT}s"
    log "cycle=$cycle: connected, holding ${HOLD_SECS}s"
    hold || fail "$cycle" "link dropped before ${HOLD_SECS}s hold completed"
    log "cycle=$cycle: PASS (held ${HOLD_SECS}s)"
    disconnect
    log "cycle=$cycle: disconnected, gap ${GAP_SECS}s"
    sleep "$GAP_SECS"
done
```

The per-second polls in `hold`/`wait_connected` are bounded poll-with-check
(each iteration reads the real signal). `HOLD_SECS`/`GAP_SECS` are the test
definition, not readiness guesses. The script **never starts a scan**
(`btmgmt find`/`scan on`) — it connects by address off blueman's standing
discovery cache, dodging the `Discovering: yes` wedge trap from the brainstorm.

**Test:** `shellcheck scripts/ble_soak.sh` clean; `bash -n scripts/ble_soak.sh`
parses. Behaviour is validated live (Testing).

**Depends on:** independent of firmware to author; needs running firmware to
exercise.

### 6. Build, lint, format, test gate

```bash
just format
just clippy            # firmware (embedded) + meteo-lib (host), -D warnings
just test              # cargo nextest, meteo-lib host tests
cargo build -p meteo-firmware
shellcheck scripts/ble_soak.sh
```

All must pass; fix every warning (zero-warning policy).

**Depends on:** steps 1–5.

### 7. Documentation (`CLAUDE.md`, `README.md`)

**`CLAUDE.md`** — add to the Pin Allocation table:

| Function              | STM32 Pin | Connector        | Label      | Peripheral |
| --------------------- | --------- | ---------------- | ---------- | ---------- |
| USART2_RX (RN4871 TX) | PD6       | CN9 pin 4 (D52)  | USART_B_RX | USART2     |
| USART2_TX (RN4871 RX) | PD5       | CN9 pin 6 (D53)  | USART_B_TX | USART2     |
| RN4871 RST_N          | PA4       | CN7 pin 17 (D24) | I/O        | GPIO       |

Add a "BLE soak test" subsection: device `80:1F:12:B6:60:BF`, module firmware
v1.30; deploy with `scp scripts/ble_soak.sh gaia:` and run on gaia; one PASS
line per 6-min cycle, non-zero exit on any drop/failed reconnect, Ctrl-C to stop.
Record the two methodology traps (no `timeout … btmgmt find`; query state via
`busctl`/the cache, never a second scan). State plainly that the link is
**unproven** and the soak is the acceptance gate.

**`README.md`** — document `scripts/ble_soak.sh`: purpose, the env knobs
(`HOLD_SECS`, `GAP_SECS`, `CONNECT_TIMEOUT`, conn params), and that it runs on
gaia with `doas` for the debugfs writes.

Verify the pin rows against `datasheets/nucleo_pins.csv`.

**Depends on:** steps 4–5.

## Testing

**Host unit tests (`just test`)** — automatable coverage of the **parser only**
(line framing, classify, command/event routing, version parse, the no-GATT
provisioning command stream). Green here means the parser is self-consistent; it
is **not** evidence the link works.

**Static checks:** `just clippy` (both targets, `-D warnings`), `cargo fmt
--check`, `shellcheck`, `bash -n`.

**Firmware build:** `cargo build -p meteo-firmware` links the embedded target.

**Live integration (manual, hardware) — the real acceptance gate:**

1. Flash with the safe `probe-rs` procedure (background, `kill -INT` for clean
   detach, never SIGKILL/timeout per CLAUDE.md). Confirm RTT shows the firmware
   version line then `BLE: advertising started`.
2. On gaia, `bluetoothctl info 80:1F:12:B6:60:BF` shows a live `RSSI:`.
3. Run `scripts/ble_soak.sh` on gaia. **Acceptance:** repeated cycles each print
   `PASS (held 360s)` with a clean disconnect / 90 s gap / reconnect between
   them, no FAIL, over a sustained run. A single passing cycle is not enough —
   the prior work sometimes connected; it never _held and repeated_.

**Edge cases:** mid-hold `%DISCONNECT%` → firmware re-advertises, script FAILs
(correct); module UART wedge → `recover` pulses RST_N and re-brings-up;
debugfs params lost after a bluetooth restart → reapplied each run.

## Risks

- **The link may still not hold — root cause is unknown.** This plan is a clean,
  honest re-attempt, not a fix for a diagnosed bug. If the soak still drops,
  the next move is **diagnosis** (sniff with `btmon` on gaia during a hold to see
  _who_ drops the link and why — supervision timeout, conn-param-update reject,
  or advertising gap) before any further code change. Do not stack patches.
- **Tuning values are guesses.** First knobs if it drops: widen conn interval to
  20–40 ms (`ST,0010,0020,…`, `CONN_MIN=16 CONN_MAX=32`) or lengthen supervision
  (`SUPERVISION=1000`). One-line changes, isolated to step 1/5 / the provision
  string.
- **Unverified history cautions** (advertising-goes-silent, `0`-timeout-no-radio,
  lowercase `Err`): the design defends against each, but confirm them with
  `btmon`/RTT rather than trusting the prior notes.
- **Silent module wedge** (no UART error, no event) would stall the supervisor;
  `recover` only triggers on UART errors. Continuous `STA` + the central's
  bounded reconnect surfaces it as a script FAIL within one cycle. If seen, add an
  explicit `next_event` response-timeout watchdog as a circuit-breaker — not as
  primary sync.
- **`BufferedUart::new` API drift** vs embassy-stm32 0.5 — defer to the compiler.
- **debugfs node names on BlueZ 5.86** — if `conn_*_interval`/`supervision_timeout`
  differ, `apply_conn_params` is the single place to adjust.

## Notes

Progress tracking (checked during `/tyrex:code:implement-light`):

- [x] 1. Workspace + crate dependencies — added `embedded-io-async = "0.7"` + `heapless = "0.9"` to workspace deps; wired into meteo-lib; `embedded-io-async` into meteo-firmware arm deps. Build/clippy/test green.
- [x] 2. RN4871 driver from datasheet + host tests — `ble/mod.rs` + `ble/rn4871.rs` (classifier, `read_line` framing, `command`/`query`, provision no-GATT sequence, advertising, `next_event`, pure `parse_version`). 18 new host tests; 29 total pass; clippy clean (verified independently).
- [x] 3. Firmware BLE supervisor task — `ble.rs`: generic `bring_up_once` (comms probe → provision → advertise), retry `bring_up`, `recover` (RST_N pulse), `ble_task` select-loop on keepalive Ticker vs `next_event`. (Implemented with substep 4 as one validatable changeset.)
- [x] 4. Firmware hardware wiring (main.rs) — `mod ble;`, USART2 IRQ bind (`BufferedInterruptHandler`), `BufferedUart` PD6/PD5 + 256B StaticCell buffers, RST_N PA4, spawn `ble_task`. `cargo build -p meteo-firmware` + firmware clippy green (`BufferedUart::new` sig + alias verified vs embassy-stm32 0.5 source).
- [x] 5. gaia soak script — `scripts/ble_soak.sh`: `set -euo pipefail`, busctl D-Bus link poll, no-scan connect-by-address, bounded poll-with-check loops, conn-param debugfs apply. shellcheck-clean, parses, executable.
- [x] 6. Build/lint/format/test gate — `cargo fmt --check` clean; `just clippy` (firmware ARM + lib host, -D warnings) clean; `just test` 29 pass; `cargo build -p meteo-firmware` ok; `shellcheck` clean.
- [x] 7. Documentation — CLAUDE.md: 3 RN4871 pin rows (PD6/PD5/PA4, verified vs nucleo_pins.csv; PA4 label corrected to `SPI_B_NSS`) + "BLE soak test" subsection (traps, btmon-first diagnosis). README.md created: build recipes + full `scripts/ble_soak.sh` env-knob table + gaia/doas deployment.

Reference only (do not copy): revision `snlwmrollztk` —
`crates/meteo-lib/src/ble/rn4871.rs`, `crates/meteo-firmware/src/ble.rs`.

## Live hardware test results (2026-06-15) — acceptance gate run

Flashed to the Nucleo (ST-LINK V3) and exercised against gaia (BlueZ 5.86).

**Firmware bring-up: PASS (confirmed on hardware).** RTT shows the full
sequence: `ble_task started` → reset (`%REBOOT%` received) → command mode →
`RN4871 firmware: 1.30` (UART RX path + `parse_version` proven) → provision →
`advertising started`. gaia sees `MeteoStation` advertising.

- First flash attempt stalled silently: the module was in an active connection
  (prior firmware's NVM config, `Connected: yes`), so `$$$` was swallowed as
  stream data and the bring-up read blocked forever. Fixed by
  `fix(ble): observable self-recovering bring-up` — per-step logs + a 20 s
  timeout circuit-breaker that converts a silent wedge into a logged retry.

**Link hold: FAIL — root cause now characterised.** The soak connects then drops
~1–3 s in. `scripts/ble_soak.sh` correctly caught it and failed loud (the
harness itself works). `btmon` on gaia during the drop shows, repeatedly:

```
LE Enhanced Connection Complete   (Supervision timeout: 5000 msec)
HCI Event: Disconnect Complete    Reason: Connection Failed to be Established (0x3e)
```

It is **not** supervision timeout (0x08) and **not** a conn-param-update reject —
it is **0x3e**: the central completes the connection request but the RN4871
peripheral never services the first connection events, so the link layer gives
up. Live RSSI was weak (−87 to −97 dBm). Most consistent hypothesis: the
peripheral fails the first post-connect PDU exchanges (marginal RF and/or the
RN4871 not engaging connection events) — an RF/link-layer problem, **not** the
driver/parser logic (which is proven correct end-to-end).

**Next investigation (not a code patch):** improve RF first (proximity/antenna;
confirm whether RSSI better than ~−80 lets the link establish), then probe the
RN4871's post-`%CONNECT%` behaviour. 0x3e at establishment is unlikely to be
fixed by the supervision/interval knobs.

### Update — root causes found (later same day)

1. **`SS,00` was wrong: the module needs services to be connectable.** With
   `SS,00` (no GATT) the RN4871 advertised but never accepted a connection —
   firmware never saw `%CONNECT`, central got 0x3e. Changed provision to
   **`SS,C0`** (Device Info + Transparent UART, as the datasheet's own example
   uses). After this the module connects, GATT discovery runs, and `ATT Handle
Value Notification` data flows. **The plan's "no GATT / no services" premise
   was the bug.**
2. **Remaining blocker is RF, not code: RSSI ≈ −90 to −101 dBm.** TX power is
   already maxed (`SGA,0`/`SGC,0`). At that level (BLE sensitivity floor) the
   link connects then drops within ~1–3 s, looping. This is an antenna/proximity
   problem — confirm by moving the device next to the adapter and re-reading
   RSSI; a healthy nearby link is −40 to −70 dBm. No conn-param/firmware change
   rescues a sub-sensitivity link.
3. Minor harness bug fixed: `apply_conn_params` wrote `conn_min` before
   `conn_max`, failing the kernel's `min<=max` check when widening. Now writes
   floor→max→min.

**Status: bring-up + connectability work and are proven on hardware; the link
still does not hold, and the cause is now identified as weak RF (~−100 dBm).**

Quality aside: `cargo deny check` fails on **pre-existing** transitive issues
(RUSTSEC-2026-0110 bare-metal, RUSTSEC-2026-0173 proc-macro-error2 — both
"unmaintained"; plus license-allowlist gaps in `deny.toml`). Unrelated to this
work; `cargo audit` reports no vulnerabilities.
