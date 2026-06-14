# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the STM32H753ZI (Nucleo-144 board). Uses the Embassy async runtime for bare-metal embedded development (`no_std`).

Currently supports:

- BMP388 barometric pressure/temperature sensor (via I2C1)
- LED indicators (onboard and external)

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
│       └── main.rs        # Hardware init, interrupt bindings, task spawning
└── meteo-lib/             # Library crate: hardware-agnostic
    └── src/
        ├── lib.rs         # Re-exports sensor drivers and utilities
        ├── utils.rs       # Utility functions (trunc2, etc.)
        └── sensors/
            ├── mod.rs     # Sensor module root
            └── bmp388.rs  # BMP388 pressure/temperature driver
```

**Library vs Binary separation:**

- `meteo-lib`: Hardware-agnostic drivers using `embedded-hal-async` traits
- `meteo-firmware`: STM32H753ZI-specific hardware initialization and Embassy tasks

### Static Resource Pattern

Hardware resources use `StaticCell<Mutex<...>>` for safe sharing between tasks:

```rust
static SPI1: StaticCell<Mutex<ThreadModeRawMutex, ...>> = StaticCell::new();
```

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
