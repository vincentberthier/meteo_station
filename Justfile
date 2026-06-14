# MeteoStation build system
# Usage: just <recipe> [args...]

# --- Variables ---

target := "thumbv7em-none-eabihf"
host_target := "x86_64-unknown-linux-gnu"
chip := "STM32H753ZITx"
binary := "target" / target / "release/meteo-firmware"

# --- Default recipe ---

[doc('List all available recipes')]
default:
    @just --list --unsorted

# --- Build recipes ---

[doc('Build firmware (release)')]
build:
    cargo build --release -p meteo-firmware

[doc('Clean build artifacts')]
clean:
    cargo clean

[doc('Show binary size information')]
size: build
    arm-none-eabi-size {{ binary }}

# --- Flash & run recipes ---

[doc('Flash firmware to device')]
flash: build
    probe-rs run --chip {{ chip }} {{ binary }}

[doc('Flash and attach with RTT logging')]
run: build
    probe-rs run --chip {{ chip }} {{ binary }}

[doc('Reset device under connect-under-reset')]
reset:
    probe-rs reset --chip {{ chip }} --connect-under-reset

[doc('Run the TUI viewer')]
tui:
    cargo run -p meteo-tui --target {{ host_target }}

# --- Code quality recipes ---

[doc('Format code')]
format:
    cargo fmt -- --emit=files

# meteo-firmware is no_std/thumbv7em; meteo-tui is host-only (ratatui/tokio).
# Lint each on its own target — a single workspace clippy would try to build the
# host crate (and its deps) for the embedded target and fail.
[doc('Check code with clippy')]
clippy:
    cargo clippy -p meteo-firmware -- -D warnings
    cargo clippy -p meteo-lib -p meteo-tui --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run -p meteo-lib -p meteo-tui --target {{ host_target }}
