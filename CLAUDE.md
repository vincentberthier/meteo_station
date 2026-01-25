
# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Weather station firmware - an embedded Rust firmware for a meteorological monitoring device targeting the STM32H753ZI (Nucleo-144 board). Uses the Embassy async runtime for bare-metal embedded development. It's obviously a `no_std` project.

Currently supports:
- BMP388 barometric pressure/temperature sensor (via I2C)
- LED indicators (onboard and external)

## Repo management

I use jujutsu (jj) to manage the repository.

The standard flow is:
- Create an empty changeset with `jj new -m '$MESSAGE'`, replacing `$MESSAGE` by whatever the current task is (so for example `jj new -m 'add PMP388 sensor'`).
- Straight away, do another `jj new`: this will be the working changeset. Ideally, you can give it a temporary name
- When the current subtask is finished, do a `jj squash -u`. This will "merge" the current changes into the parent (so the one with the `firmware: $MESSAGE` description), and leave the current change empty.
- When the current task is finished, so everything has been squashed into the `firmware: $MESSAGE` changeset, do a `jj abandon` (it’s an empty changeset, so no problem)

A subtask is considered finished when:
- The code compiles
- Clippy & format don’t raise any issue
- The current subgoal has been achieved

A task is finished when all subtasks are finished AND I validate it. So we could have a task to implement something specific, with subtasks to implement, debug, document, test, debug again, *etc.* 

## Tools

Prefer using rust-written tools:
- exa instead of ls
- fd instead of find
- rg instead of grep
- sd instead of sed

## Build Commands

All commands need to be compatible with the fish shell or wrapped through bash.

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

Requires `probe-rs` for flashing and `cargo-make` for task automation. For auto-diagnostics, run `cargo make debug` with a timeout of 30s, you’ll get all the relevant logs.

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
- Primary: STM32H753ZI (`thumbv7em-none-eabihf`)

## Code Standards

Strict clippy configuration enforced - see `Cargo.toml` `[workspace.lints.clippy]`:
- No `unwrap()` or `expect()` - use proper error handling
- No undocumented unsafe blocks - document all unsafe code
- Arithmetic overflow checks enabled
- `panic`, `todo`, `unimplemented` trigger warnings

Use `defmt` macros for logging (`defmt::info!`, `defmt::error!`, etc.) - outputs via RTT over JTAG.

### Tests

Functions need to be pure as much as possible to make testing easy. Obviously considering the target, anything actually dealing with the hardware cannot be automatically tested, but that doesn’t mean nothing should be. New functionalities (or updated functionalities) should be tested as much as possible. The tests only run on the development machine (not the targetted hardware), so can use `std` stuff as needed.

Tests modules should have the following structure:
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

Obviously, if nothing is faillible in the tests, then don’t use `-> TestResult`.

The tests will be run with `cargo nextest`.

### Documentation

Modules, functions, traits, structs & enums (basically everything) should be documented properly (including non-public stuff), with examples where pertinent. Function documentation should follow this template (obviously in parameters, ignore `self` if present):

~~~rust
/// Description.
///
/// # Parameters
/// * ``argument_name`` - type and description,
///
/// # Example
/// ```rust
/// # use crate_name::Error;
/// // write me later
/// # Ok::<(), Error>(())
/// ```
~~~

Obviously only add examples if it’s something a bit complex, not for absolutely every little thing.

## Misc

When retrieving documents on the net, put a timeout of a minute or so.
