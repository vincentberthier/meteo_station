
# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the STM32H753ZI (Nucleo-144 board). Uses the Embassy async runtime for bare-metal embedded development (`no_std`).

Currently supports:
- BMP388 barometric pressure/temperature sensor (via I2C)
- LED indicators (onboard and external)

## Build Commands

```bash
# Build firmware (release)
cargo make build

# Flash to device
cargo make flash

# Flash and attach with RTT logging
cargo make run

# Reset device under connect-under-reset
cargo make reset

# Check code with clippy
cargo make clippy

# Format code
cargo make format

# Show binary size
cargo make size
```

## Architecture

### Async Runtime Pattern
Uses Embassy's `#[embassy_executor::task]` for concurrent tasks. All peripherals are accessed asynchronously. Tasks communicate via `embassy_sync::channel::Channel` instead of shared memory.

### Module Structure
```
src/
├── main.rs      # Binary: hardware init, interrupt bindings, task spawning
├── lib.rs       # Library root: re-exports sensor drivers
└── bmp388.rs    # BMP388 pressure/temperature sensor driver (hardware-agnostic)
```

**Library vs Binary separation:**
- `lib.rs` + sensor modules: Hardware-agnostic drivers using `embedded-hal-async` traits
- `main.rs`: STM32H753ZI-specific hardware initialization and Embassy tasks

### Static Resource Pattern
Hardware resources use `StaticCell<Mutex<...>>` for safe sharing between tasks:
```rust
static SPI1: StaticCell<Mutex<ThreadModeRawMutex, ...>> = StaticCell::new();
```

### Target Configuration
Primary: STM32H753ZI (`thumbv7em-none-eabihf`)

## Hardware

All hardware datasheets are in `datasheets/`. For pin configuration, use `datasheets/um2407-stm32h7-nucleo144-boards-mb1364-stmicroelectronics.pdf` (pinout: pages 40-42).

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
