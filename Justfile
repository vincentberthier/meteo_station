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

# meteo-tui is host-only (ratatui/tokio/bluer); the default target is the
# embedded one, so it needs an explicit host target or it would try to build for
# thumbv7em and fail.
[doc('Build firmware (release) and the TUI viewer')]
build:
    cargo build --release -p meteo-firmware
    cargo build --release -p meteo-tui --target {{ host_target }}

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

# Headless mode logs the BLE feed lifecycle to the console — much easier to
# follow over SSH than the full-screen TUI. Tune verbosity with RUST_LOG
# (e.g. RUST_LOG=meteo_tui=debug just tui-headless).
[doc('Run the viewer headless, logging BLE events to the console')]
tui-headless:
    cargo run -p meteo-tui --target {{ host_target }} -- --no-tui

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
    cargo clippy -p meteo-lib -p meteo-tui --all-features --all-targets --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run -p meteo-lib -p meteo-tui --target {{ host_target }}

# Ignored advisories (unmaintained transitive deps) are documented in
# .cargo/audit.toml; cargo-audit reads it automatically.
[doc('Audit dependencies for security advisories')]
audit:
    cargo audit
