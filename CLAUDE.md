# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the ESP32-H2 (ESP32-H2-DevKitM-1 board). Uses the Embassy async runtime on esp-hal for bare-metal embedded development (`no_std`). The target triple is `riscv32imac-unknown-none-elf`.

Currently supports:

- BMP388 barometric pressure/temperature sensor (via I2C0)
- Status LED on GPIO8

> Ported from the STM32H753ZI (Nucleo-144). The on-chip BLE 5.3 radio replaces the
> external RN4871 module (RN4871/USART path dropped). On-chip BLE telemetry is now
> implemented: the firmware advertises as `MeteoStation`, exposes a GATT notify
> characteristic, and pushes sensor data at 1 Hz, with an RWDT heartbeat supervisor.
> The earlier supervision-timeout disconnects are fixed by negotiating an 8 s
> supervision timeout over L2CAP (a vendored trouble-host patch — see the BLE
> implementation notes). On-device acceptance via the gaia soak harness is a
> pending manual gate.

## Build Commands

```bash
# Build firmware (release)
just build

# Flash to device (espflash, over native USB-Serial-JTAG)
just flash

# Flash and attach the defmt monitor
just run

# Reset the device
just reset

# Check code with clippy (firmware + meteo-lib + meteo-tui)
just clippy

# Format code
just format

# Run tests on host (meteo-lib + meteo-tui)
just test

# Show binary size
just size

# Dashboard (host target only)
just tui-build           # build the TUI dashboard
just tui-run             # run the TUI dashboard
just tui-clippy          # clippy the TUI crate only (fast loop)
```

### Flashing & logging with espflash

The ESP32-H2 flashes and logs over its **native USB-Serial-JTAG** (`/dev/ttyACM0`),
not an external debug probe. `just run` flashes and then streams defmt over the same
port; espflash decodes the defmt framing from the freshly built ELF
(`--log-format defmt`). Stop the monitor with **Ctrl-C** — unlike probe-rs, espflash
holds no JTAG lock, so an interrupted monitor leaves the chip running the flashed
image and the port free.

```bash
just run     # build + espflash flash --monitor --log-format defmt
just flash    # build + flash without attaching the monitor
just reset    # espflash reset
```

`DEFMT_LOG` (set to `trace` in `.cargo/config.toml`) controls the compile-time log
level filter.

## Architecture

### Async Runtime Pattern

Uses Embassy's `#[embassy_executor::task]` for concurrent tasks, scheduled by the
**esp-rtos** thread-mode executor: `main` is marked `#[esp_rtos::main]`, and
`esp_rtos::start(timg0.timer0, sw_int.software_interrupt0)` brings up the scheduler
plus the embassy time driver. All peripherals are accessed asynchronously. Tasks
communicate via `embassy_sync::channel::Channel` instead of shared memory.

Per esp-rtos, `embassy-executor` must **not** enable any `arch-*` feature (esp-rtos
supplies the executor). `embassy-time`/`embassy-executor` versions follow esp-rtos's
bounds.

### Module Structure

```
crates/
├── meteo-firmware/        # Binary crate: ESP32-H2-specific (riscv32imac)
│   └── src/
│       ├── main.rs        # esp-hal/esp-rtos init, GPIO8 LED blink, task spawning
│       ├── bmp.rs         # BMP388 task; reads and logs barometer samples
│       ├── ble.rs         # On-chip BLE stack: controller, manual GATT server,
│       │                  #   advertise loop, 1 Hz notify (esp-radio + trouble-host)
│       └── watchdog.rs    # RWDT heartbeat supervisor (watches ADV_BEAT + BLE_BEAT)
├── meteo-lib/             # Library crate: hardware-agnostic
│   └── src/
│       ├── lib.rs         # Re-exports sensor drivers and utilities
│       ├── utils.rs       # Utility functions (trunc2, etc.)
│       ├── ble/
│       │   └── frame.rs   # v1 wire frame: Telemetry, encode/decode, FrameError,
│       │                  #   sentinels; host-tested; decode() targets Linux central
│       └── sensors/
│           ├── mod.rs     # Sensor module root
│           └── bmp388.rs  # BMP388 pressure/temperature driver
└── meteo-tui/             # Binary crate: terminal dashboard (host, x86_64-linux)
    └── src/
        └── main.rs        # ratatui TUI: BLE connect, telemetry subscribe, render
```

**Library vs Binary separation:**

- `meteo-lib`: Hardware-agnostic drivers using `embedded-hal-async` traits. The
  esp-hal async I2C implements those traits, so the BMP388 driver is unchanged.
- `meteo-firmware`: ESP32-H2-specific init and Embassy tasks. The esp deps are gated
  behind `[target.'cfg(target_arch = "riscv32")'.dependencies]`.

### Target Configuration

Primary: ESP32-H2 (`riscv32imac-unknown-none-elf`), stable Rust (no `build-std`).
esp-hal's build script emits the linker scripts, so `.cargo/config.toml` only sets
the target, `force-frame-pointers` (for esp-backtrace), and the espflash runner.

### Dashboard (`meteo-tui`)

`crates/meteo-tui` is a host-only (`x86_64-unknown-linux-gnu`) `std` binary crate. It
connects to the `MeteoStation` BLE peripheral via **bluer 0.17** (the official Rust
BlueZ binding), subscribes to the telemetry notify characteristic, decodes each
17-byte v1 frame via `meteo-lib::ble::frame::decode`, and renders a live terminal
dashboard with ratatui:

- All 8 frame fields (temperature, pressure, humidity, light, wind speed/direction,
  rainfall, battery).
- Live scrolling temperature and pressure mini-charts.
- Header bar: wall clock, app version, firmware version (read from DIS), connection
  status.

**Host-only build — never build with `--workspace` on the default target.** The
`.cargo/config.toml` default target is `riscv32imac-unknown-none-elf`; `meteo-tui`
uses `std` and cannot compile for that target. All recipes scope the crate explicitly:

```bash
just tui-build           # cargo build -p meteo-tui --target x86_64-unknown-linux-gnu
just tui-run [-- ARGS]   # cargo run   -p meteo-tui --target x86_64-unknown-linux-gnu
just tui-clippy          # clippy for the dashboard only (fast iteration loop)
```

The dashboard crate is also included in `just clippy` and `just test`.

**`bluer` / `AcquireNotify` rationale.** The dashboard uses `bluer`'s
`Characteristic::notify_io()`, which is backed by BlueZ `AcquireNotify`. This
delivers every notification PDU over a raw file descriptor without deduplication.
`btleplug`'s BlueZ backend uses `StartNotify` and surfaces values through the
`PropertiesChanged`/`Value` D-Bus property, which BlueZ only re-emits when the value
_changes_. The telemetry payload is near-constant (sensor readings barely move
second-to-second), so that path collapses notifications to silence — the same trap
`scripts/ble_notify_check.sh` documents and avoids.

**DIS firmware-version transport.** The firmware exposes a standard Device
Information Service (`0x180A`) with a Firmware Revision String (`0x2A26`). The
dashboard reads that characteristic once on connect and displays it in the header
alongside the app version.

**Disconnect detection.** Link state is authoritative: the dashboard treats a BlueZ
`Connected` → false transition or an EOF on the notify fd as the disconnect signal.
Frame age is cosmetic only (it drives value greying when frames stop arriving) and
never triggers reconnection. On reconnect, the dashboard performs a fresh bounded
scan first — BlueZ evicts the non-bonded LE device object after disconnect, so a
cold `connect` by address would fail until the cache is repopulated.

**Host prerequisites.** At build time: `libdbus-1-dev`. At runtime: a running
`bluetoothd` (present on the dev machine / gaia).

## Hardware

Component datasheets are in `datasheets/`. Each PDF has a corresponding `.md` summary -- **read the `.md` files instead of the PDFs**:

| Component                     | Summary                          | PDF                                         |
| ----------------------------- | -------------------------------- | ------------------------------------------- |
| BMP388 pressure/temp sensor   | `datasheets/bmp388.md`           | `bst-bmp388-ds001.pdf`                      |
| BME280 humidity/pressure/temp | `datasheets/bme280.md`           | `bst-bme280-ds002.pdf`                      |
| MLX90614 IR thermometer       | `datasheets/mlx90614.md`         | `MLX90614-Datasheet-Melexis.pdf`            |
| VEML7700 ambient light sensor | `datasheets/veml7700.md`         | `veml7700.pdf`                              |
| RN4870/71 BLE module          | `datasheets/rn4871.md`           | `rn4870.pdf`, `RN4870-71-...User-Guide.pdf` |
| MT3608 boost converter        | `datasheets/mt3608.md`           | `MT3608-3223743.pdf`                        |
| Weather meter kit             | `datasheets/weather_meter.md`    | `DS-15901-Weather_Meter.pdf`                |
| Nucleo-H753ZI board           | `datasheets/nucleo_h753zi.md`    | `nucleo-h753zi.pdf`, `um2407-...pdf`        |
| ESP32-S3-DevKitM              | `datasheets/esp32_s3_devkitm.md` | (online only)                               |
| LiPo battery                  | `datasheets/lipo_battery.md`     | (text file)                                 |

Pin reference: `datasheets/esp32_h2_devkitm.md` (full ESP32-H2 board pinout +
weather-station wiring). The Nucleo map lives in `datasheets/nucleo_pins.csv`.

### Pin Allocation (ESP32-H2-DevKitM-1)

Wired and used today:

| Function         | GPIO   | Header / silk | Peripheral | Notes                                  |
| ---------------- | ------ | ------------- | ---------- | -------------------------------------- |
| I2C SDA (BMP388) | GPIO10 | J3/4 `10`     | I2C0       | external 4.7 kΩ pull-up to 3V3         |
| I2C SCL (BMP388) | GPIO11 | J3/5 `11`     | I2C0       | external 4.7 kΩ pull-up to 3V3         |
| Status LED       | GPIO8  | J3/8 `8`      | GPIO       | external LED + onboard WS2812 (shared) |

GPIO8 carries both the onboard addressable WS2812 RGB _and_ the external LED. It is
driven as a plain push-pull GPIO, so the external LED blinks; the WS2812 needs a
precise data stream and stays dark. Driving colour would require either esp-hal
pinned to 1.0 + `esp-hal-smartled` (incompatible with current esp-rtos/embassy), or
a hand-rolled RMT WS2812 path on esp-hal 1.1 — tracked as a follow-up.

The remaining weather-station wiring (anemometer/rain-gauge pulse inputs, wind-vane
and battery ADC) is documented in `datasheets/esp32_h2_devkitm.md` but not yet in
firmware.

### BLE — on-chip ESP32-H2 peripheral

The firmware brings up the on-chip BLE 5.3 radio via **esp-radio** and
**trouble-host**, advertises as `MeteoStation` (connectable undirected, static
random address `F0:CA:FE:00:00:01`), and pushes a 17-byte telemetry frame at 1 Hz
over a GATT Notify characteristic.

**GATT layout:**

| Role           | UUID                                   |
| -------------- | -------------------------------------- |
| Service        | `7e700001-b1df-42a1-bb5f-6a1028c793b0` |
| Characteristic | `7e700002-b1df-42a1-bb5f-6a1028c793b0` |
| Properties     | Read + Notify                          |

The characteristic value is a 17-byte frame: byte[0] is the version sentinel
(`0x01`), followed by encoded sensor data. Frame encoding and decoding live in
`meteo-lib::ble::frame` (`Telemetry`, `encode`, `decode`, `FrameError`). The
`decode()` path is built for a future Linux central; it is not used by the firmware
itself.

**Implementation notes:**

- The GATT server is built manually via trouble-host's `AttributeTable` /
  `AttributeServer` / `Characteristic` primitives. The `derive` macros
  (`trouble-host-macros 0.4`) are absent from the crates.io registry used by this
  workspace, so the table is constructed by hand.
- `esp_sync::RawMutex` is used for the attribute table mutex. trouble-host 0.6
  depends on `embassy-sync 0.7`; the workspace targets `embassy-sync 0.8`.
  `esp_sync::RawMutex` implements `RawMutex` for both versions, bridging the two.
- An RWDT heartbeat supervisor (`crates/meteo-firmware/src/watchdog.rs`) watches
  `ADV_BEAT` (bumped each advertise-loop iteration) and `BLE_BEAT` (bumped each
  successful notify). If either counter stalls, the RWDT fires and resets the chip.
- **Supervision-timeout fix (vendored trouble-host patch).** BlueZ connects with a
  ~420 ms supervision timeout, so the link dropped (`Connection Timeout`, HCI 0x08)
  on any brief central-radio stall. On connect the firmware requests a robust 8 s
  supervision timeout + 80 ms interval via `update_connection_params`, but the H2
  controller accepts the HCI `LE Connection Update` (Command Status = success)
  without ever running the LL connection-parameters-request procedure on air — so
  the parameters never changed. The workspace vendors trouble-host 0.6.0 at
  `third_party/trouble-host` (copied from the crates.io source) with a one-line change in
  `update_connection_params` that restricts the HCI path to the central role; a
  peripheral now uses the **L2CAP Connection Parameter Update** signaling, which the
  controller forwards correctly. Wired via `[patch.crates-io]` in the workspace
  `Cargo.toml`. Verified on-device: the `ConnectionParamsUpdated` log fires with
  `supervision_ms=8000` and a 6-min hold held with zero drops (was 2–203 s before).
- The esp-rs stack (esp-hal 1.1, esp-rtos 0.3, esp-radio `1.0.0-beta.0`) is pulled
  from crates.io. An earlier local esp-hal fork that carried an LP-clock change was
  dropped: the clock was disproven as the drop cause (the C6 ships the identical
  no-op `ble_rtc_clk_init` and `sleep_en: 0` means the LP clock does not gate
  connection-event timing).
- **TODO — drop the vendored patch and upstream the fix (blocked on bt-hci).**
  We are pinned to **trouble-host 0.6** because esp-radio `1.0.0-beta.0` (the latest,
  including esp-hal `main`) pins **bt-hci 0.8**, while **trouble-host 0.7 requires
  bt-hci 0.9** — the two `Controller` trait versions are incompatible and nothing
  bridges them (esp-radio implements only the 0.8 trait). When esp-radio adopts
  bt-hci 0.9, bump to trouble-host 0.7 and **delete the vendored copy +
  `[patch.crates-io]`**. At that point also upstream the work: (a) an esp-rs/esp-hal
  issue for the controller bug (HCI `LE Connection Update` accepted with Command
  Status = success but no `LE Connection Update Complete` ever emitted as a
  peripheral), and (b) an embassy-rs/trouble PR — _not_ our blanket force-L2CAP
  (it would regress controllers where the LL procedure works), but an additive
  opt-in (e.g. a `ForceL2cap` method/param), since the completion is delivered to
  the connection's public event stream and can't be cleanly awaited inside
  `update_connection_params`. Forks for both are at `../esp-hal` and `../trouble`.

**Wire frame (`meteo-lib::ble::frame`):**

Host-tested (24/24 tests). The `encode`/`decode` round-trip is verified on the
development machine via `cargo nextest`. The firmware calls only `encode`; `decode`
is present for the deferred Linux central.

**Acceptance gate:**

On-device acceptance requires `gaia` (BlueZ 5.86). Two scripts cover the two halves:

```bash
# Link-stability soak (connect → hold 6 min → disconnect → gap → reconnect …)
scp scripts/ble_soak.sh gaia:
ssh gaia ./ble_soak.sh        # Ctrl-C to stop

# Data-flow check (subscribe to notify, assert ≥5 well-formed frames in 15 s)
scp scripts/ble_notify_check.sh gaia:
ssh gaia ./ble_notify_check.sh
```

`ble_soak.sh` prints one `PASS (held 360s)` line per cycle and exits non-zero on
any mid-window drop or failed reconnect. A single passing cycle is **not**
acceptance — the link must hold and repeat over a sustained run.

`ble_notify_check.sh` connects, then subscribes via BlueZ **`AcquireNotify`** (a
python-dbus reader), captures frames for `WINDOW_SECS` (default 15 s), and asserts
at least `MIN_FRAMES` (default 5) 17-byte frames with byte[0] == 0x01. It uses
`AcquireNotify` — **not** bluetoothctl's `notify on` output — because BlueZ only
re-emits the `Value` property when it _changes_, and the near-constant telemetry
gets deduped to silence even while notifications flow on-air; `AcquireNotify`
delivers every PDU raw. Needs `python3` + `python-dbus` on gaia (`bleak` is not
installed; `btmon` would also work but needs root). If the device is not already in
blueman's cache the script does one bounded, self-terminating `--timeout` discovery
(verified to leave the adapter idle, not wedged). Both env knobs are overridable.

**Methodology traps (learned the hard way — do not bypass):**

- **Never** run a blocking scan (`timeout … btmgmt find`, `bluetoothctl scan on`)
  to find the device — it wedges the adapter in `Discovering: yes`. Both scripts
  connect by address off blueman's standing discovery cache instead.
- Query link state via `busctl` (`org.bluez.Device1.Connected`) or the BlueZ
  cache, **never** a second scan.

**Reconnecting requires a fresh scan first (central-side, not a device fault).**
The device advertises continuously and re-advertises immediately after a
disconnect (verified: connect → disconnect → link-down gap → reconnect all hold).
But BlueZ evicts the non-bonded LE device object once discovery stops, so a _cold_
`bluetoothctl connect F0:CA:FE:00:00:01` reports `Device … not available` until a
bounded discovery repopulates the cache. Always do one bounded, self-terminating
`bluetoothctl --timeout N scan on` immediately before each (re)connect — the same
scan-then-connect-by-address pattern the soak scripts use. The 8 s supervision
negotiation runs in the firmware accept loop on every connection, so reconnections
get the same robust link as the first.

If the soak drops, diagnose with `btmon` on gaia during a hold (who drops the link
and why) before changing code; the first tuning knobs are the conn-interval /
supervision-timeout values, not another patch.

## Debugging & Accountability Principles

These are permanent rules for any debugging or troubleshooting on this project:

- **Do not blame the hosts.** They work perfectly fine. If — and only if — you wedge
  something, you may _request_ a sub-system reboot. Never assert a host is at fault.
- **Do not blame the hardware** except as a last resort, after you have exhausted _all_
  other explanations.
- **Fix the system, not the symptom.** A stack of patches is not wanted. When something
  doesn't work, fix the system in its entirety — not whichever little thing happened to
  trip the diagnostics.
- **Reading code is a short part of problem-solving.** If the cause isn't quickly obvious
  from the code, stop reading and reach for web searches, better/more diagnostics, and
  throwaway test scripts instead.

## Code Standards

Use `defmt` macros for logging (`defmt::info!`, `defmt::error!`, etc.) - outputs via RTT over JTAG.

### Tests

Functions should be pure as much as possible to make testing easy. Hardware-interfacing code cannot be automatically tested, but logic should be. Tests run on the development machine (not the target hardware), so can use `std`.

Tests modules should have this structure:

```rust
// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::super::Error;
    use super::*;
    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn test_name() -> TestResult {
        // Given


        // When


        // Then


        Ok(())
    }
}
// grcov exclude stop
```

Tests are run with `cargo nextest`.
