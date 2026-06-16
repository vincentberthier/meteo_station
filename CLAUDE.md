# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the ESP32-H2 (ESP32-H2-DevKitM-1 board). Uses the Embassy async runtime on esp-hal for bare-metal embedded development (`no_std`). The target triple is `riscv32imac-unknown-none-elf`.

Currently supports:

- BMP388 barometric pressure/temperature sensor (via I2C0)
- Status LED on GPIO8

> Ported from the STM32H753ZI (Nucleo-144). The on-chip BLE 5.3 radio replaces the
> external RN4871 module, so the RN4871/USART path was dropped (bringing up native
> BLE via esp-radio/trouble is a separate, later task). The `meteo-lib` RN4871
> parser is kept for its host tests.

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

# Check code with clippy
just clippy

# Format code
just format

# Run tests on host
just test

# Show binary size
just size
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
│       └── bmp.rs         # BMP388 task; reads and logs barometer samples
└── meteo-lib/             # Library crate: hardware-agnostic
    └── src/
        ├── lib.rs         # Re-exports sensor drivers and utilities
        ├── utils.rs       # Utility functions (trunc2, etc.)
        ├── ble/           # RN4871 parser (kept for host tests; not flashed)
        └── sensors/
            ├── mod.rs     # Sensor module root
            └── bmp388.rs  # BMP388 pressure/temperature driver
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

### BLE (dropped on the ESP32-H2 port — historical)

> The external RN4871 module and its USART/firmware path were **removed** in the
> ESP32-H2 port; the H2 has on-chip BLE 5.3 (to be brought up later via
> esp-radio/trouble). The notes below describe the old STM32+RN4871 soak setup and
> the `meteo-lib` RN4871 parser (still present, host-tested). They are kept for the
> hard-won methodology lessons and the eventual on-chip BLE work.

The RN4871 BLE link is brought up by `ble::ble_task` (firmware) as device
`80:1F:12:B6:60:BF`, module firmware v1.30, advertising continuously with no
GATT services. The link is **unproven** — prior attempts never held a connection
for the 6-minute target, and the root cause was never found. The live soak is the
**acceptance gate**, not the host unit tests (which only prove the parser).

Acceptance harness: `scripts/ble_soak.sh`, run **on gaia** (BlueZ 5.86). Deploy
and run:

```bash
scp scripts/ble_soak.sh gaia:
ssh gaia ./ble_soak.sh        # Ctrl-C to stop
```

It drives connect → hold 6 min → disconnect → 90 s gap → reconnect, indefinitely,
printing one `PASS (held 360s)` line per cycle and exiting non-zero on any
mid-window drop or failed reconnect. A single passing cycle is **not** acceptance —
the link must hold and repeat over a sustained run.

Two methodology traps to avoid (learned the hard way):

- **Never** run a blocking scan (`timeout … btmgmt find`, `bluetoothctl scan on`)
  to find the device — it wedges the adapter in `Discovering: yes`. The script
  connects by address off blueman's standing discovery cache instead.
- Query link state via `busctl` (`org.bluez.Device1.Connected`) or the BlueZ
  cache, **never** a second scan.

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
