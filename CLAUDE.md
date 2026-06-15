# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the STM32H753ZI (Nucleo-144 board). Uses the Embassy async runtime for bare-metal embedded development (`no_std`).

Currently supports:

- BMP388 barometric pressure/temperature sensor (via I2C1)
- LED indicators (onboard and external)
- BLE telemetry link via an RN4871 module (USART2): the firmware advertises a
  custom GATT service and pushes one measurement frame per second; the
  `meteo-tui` viewer is a BlueZ central that subscribes and decodes the frames.

## Build Commands

```bash
# Build firmware (release)
just build

# Flash to device
just flash

# Flash and attach with RTT logging
just run

# Reset device under connect-under-reset
just reset

# Check code with clippy
just clippy

# Format code
just format

# Run tests on host
just test

# Show binary size
just size
```

### Testing with probe-rs

**CRITICAL:** When running `probe-rs run` for testing, NEVER let it be killed by timeout or SIGTERM. This leaves the debug probe locked and the chip halted, requiring a physical unplug/replug to recover.

**Correct way to test firmware:**

```bash
# Run in background, let it run for a few seconds, then cleanly terminate
probe-rs run --chip STM32H753ZITx target/thumbv7em-none-eabihf/release/meteo-firmware &
PROBE_PID=$!
sleep 5  # Let it run for 5 seconds (or more, adjust as needed)
kill -INT $PROBE_PID  # Send SIGINT (Ctrl+C) for clean exit
wait $PROBE_PID
```

This ensures probe-rs can cleanly detach from the debug probe before exiting.

## Architecture

### Async Runtime Pattern

Uses Embassy's `#[embassy_executor::task]` for concurrent tasks. All peripherals are accessed asynchronously. Tasks communicate via `embassy_sync::channel::Channel` instead of shared memory.

### Module Structure

```
crates/
├── meteo-firmware/        # Binary crate: STM32H753ZI-specific
│   ├── build.rs
│   └── src/
│       ├── main.rs        # Hardware init, interrupt bindings, task spawning
│       ├── bmp.rs         # BMP388 task; publishes samples to SENSOR_CHANNEL
│       └── ble.rs         # SENSOR_CHANNEL + ble_task (provision + supervisor)
├── meteo-lib/             # Library crate: hardware-agnostic
│   └── src/
│       ├── lib.rs         # Re-exports sensor drivers and utilities
│       ├── utils.rs       # Utility functions (trunc2, etc.)
│       ├── sensors/
│       │   ├── mod.rs     # Sensor module root
│       │   └── bmp388.rs  # BMP388 pressure/temperature driver
│       └── ble/
│           ├── mod.rs     # Shared BLE constants (UUIDs, device name)
│           ├── frame.rs   # 17-byte wire-frame codec (encode/decode) + Frame
│           ├── sample.rs  # SensorSample + apply_sample (pure, host-tested)
│           └── rn4871.rs  # RN4871 async ASCII driver (embedded-io-async)
└── meteo-tui/             # Host TUI viewer: BlueZ central (bluer)
    └── src/
        ├── feed.rs        # scan → connect → subscribe → decode → reconnect
        └── sensors.rs     # SENSORS registry + field_to_index mapping
```

**Library vs Binary separation:**

- `meteo-lib`: Hardware-agnostic drivers using `embedded-hal-async` /
  `embedded-io-async` traits. The `ble` module is the single source of truth for
  the wire contract — the frame codec is shared by firmware (encode) and the
  `meteo-tui` central (decode) so the two sides cannot drift.
- `meteo-firmware`: STM32H753ZI-specific hardware initialization and Embassy tasks
- `meteo-tui`: host-only viewer (tokio + ratatui + `bluer`); builds for the host
  target, not the embedded one

### Static Resource Pattern

Hardware resources use `StaticCell<Mutex<...>>` for safe sharing between tasks:

```rust
static SPI1: StaticCell<Mutex<ThreadModeRawMutex, ...>> = StaticCell::new();
```

### BLE Link (RN4871)

The wire contract lives in `meteo-lib::ble` and is shared by both sides:

- **Service/characteristic.** A custom 128-bit GATT service
  (`SERVICE_UUID`/`CHAR_UUID` in `ble/mod.rs`) with one Notify characteristic.
  The UUIDs are defined once as `u128`; the firmware formats them to 32-char hex
  for the RN4871 `PS`/`PC` commands, the central builds `uuid::Uuid::from_u128`.
- **Frame.** A fixed 17-byte, little-endian, schema-v1 packet of scaled integers
  (byte 0 is the schema version). Absent sensors use per-field sentinels
  (`i16::MIN` / `u16::MAX` / `u8::MAX`) that decode to `None`. `Frame::encode`
  and `Frame::decode` are the only place units/scaling/sentinels are defined.
  Pressure is carried as Pa so the central's `pa_to_hpa` registry transform stays
  correct. The 20-byte ATT default is the ceiling — the v1 frame uses 17 of 20.
- **Provisioning is verify-and-repair.** At boot the driver reads the module's
  current config and only writes the set-commands + `WR` + reboot when it differs
  from desired. The RN4871 runs in No-Prompt mode (`SR,4000`): there is no `CMD>`
  terminator, so command responses key off `AOK`/`ERR`.
- **Supervisor.** `ble_task` provisions once, then pushes one frame per
  `SENSOR_CHANNEL` sample and re-advertises on disconnect. A `RST_N` (PA4) pulse
  is the wedge-recovery circuit-breaker — used only on the explicit error path.

The firmware ↔ central seam is `ClientEvent` (`meteo-tui`); the central maps only
the frame fields the registry presents (Temperature, Pressure today) via
`sensors::field_to_index`, dropping the rest until that hardware exists.

#### Testing the BLE link

The frame codec and driver are host-tested (`just test`). The full radio path is
not: the dev box has no Bluetooth radio, so run the `meteo-tui` central on
**gaia** (`D8:F3:BC:63:2E:56`, BlueZ 5.86) over SSH — `just tui` there. Confirm
`MeteoStation` advertising with `bluetoothctl` first. **Never reboot gaia.**
`bluer` pulls a large D-Bus dependency tree and only needs to compile/run where a
radio exists; host CI covers the pure-logic tests only.

**Required gaia connection parameters (or GATT resolution fails).** The link to
the station is marginal (~-89/-91 dBm at gaia), so the central must connect with
a FAST interval — otherwise too few connection events occur per second and GATT
service resolution cannot complete before the link drops
(`le-connection-abort-by-local`). Set these on gaia BEFORE running the viewer
(debugfs, resets on every `systemctl restart bluetooth` and on reboot — reapply
each time; never reboot gaia):

```sh
doas sh -c 'echo 6   > /sys/kernel/debug/bluetooth/hci0/conn_min_interval   # 7.5 ms
            echo 12  > /sys/kernel/debug/bluetooth/hci0/conn_max_interval   # 15 ms
            echo 500 > /sys/kernel/debug/bluetooth/hci0/supervision_timeout' # 5 s
```

With these, GATT resolves and live Temperature/Pressure stream (verified ~2 min
continuous). Without them the device is discovered and connects but service
resolution aborts. (Firmware already requests good params via `ST`, but BlueZ as
central uses its own debugfs defaults at connection time and only honours the
peripheral's request after an update — too late for the resolution window.)

**Test-methodology trap:** do NOT probe discovery with `timeout N btmgmt find` —
SIGKILL mid-scan leaves `org.bluez` stuck `Discovering: yes` and wedges every
later scan (returns 0 devices), which looks like "the station stopped radiating."
Also, gaia runs `blueman-manager` which holds a continuous discovery, so a
second `btmgmt find` can't start. To check presence, just query the cache:
`bluetoothctl info 80:1F:12:B6:60:BF` (look for a live `RSSI:`).

For debugging over SSH, run the viewer headless: `just tui-headless` (or
`meteo-tui --no-tui`). Instead of the full-screen TUI it logs the BLE feed
lifecycle — adapter ready, scan, discovery candidates (name + RSSI), connect,
subscribe, readings, disconnect, and every error the feed would otherwise
swallow — to the console via `tracing`. Tune verbosity with `RUST_LOG`
(`RUST_LOG=meteo_tui=debug` is the default; use `=trace` for more). The feed
purges stale cached `MeteoStation` devices at startup and accepts a peer only
when it both advertises the name and is currently present (has an RSSI), so
`org.bluez`'s cache no longer hands back out-of-range ghosts on every scan.

### Target Configuration

Primary: STM32H753ZI (`thumbv7em-none-eabihf`)

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

Pin reference: `datasheets/nucleo_pins.csv`.

### Pin Allocation

| Function            | STM32 Pin | Connector         | Label       | Peripheral |
| ------------------- | --------- | ----------------- | ----------- | ---------- |
| LED green (LD1)     | PB0       | CN10 pin 31 (D33) | TIM_D_PWM1  | GPIO       |
| LED yellow (LD2)    | PE1       | onboard           | -           | GPIO       |
| LED red (LD3)       | PB14      | onboard           | -           | GPIO       |
| External LED        | PG2       | CN8 pin 14 (D49)  | I/O         | GPIO       |
| I2C1_SCL (BMP388)   | PB8       | CN7 pin 2 (D15)   | I2C_A_SCL   | I2C1       |
| I2C1_SDA (BMP388)   | PB9       | CN7 pin 4 (D14)   | I2C_A_SDA   | I2C1       |
| USART2_TX (RN4871)  | PD5       | CN9 pin 6 (D53)   | USART_B_TX  | USART2     |
| USART2_RX (RN4871)  | PD6       | CN9 pin 4 (D52)   | USART_B_RX  | USART2     |
| USART2_RTS (RN4871) | PD4       | CN9 pin 8 (D54)   | USART_B_RTS | USART2     |
| USART2_CTS (RN4871) | PD3       | CN9 pin 10 (D55)  | USART_B_CTS | USART2     |
| BLE RST_N (RN4871)  | PA4       | CN7 pin 17 (D24)  | SPI_B_NSS   | GPIO       |

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
