# MeteoStation build system
# Usage: just <recipe> [args...]

# --- Variables ---

target := "riscv32imac-unknown-none-elf"
host_target := "x86_64-unknown-linux-gnu"
chip := "esp32h2"
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
    size {{ binary }}

# --- Flash & run recipes ---

[doc('Flash firmware to device')]
flash: build
    espflash flash --chip {{ chip }} {{ binary }}

[doc('Flash and attach with defmt logging over USB-Serial-JTAG')]
run: build
    espflash flash --monitor --chip {{ chip }} --log-format defmt {{ binary }}

[doc('Reset the device')]
reset:
    espflash reset

# --- Code quality recipes ---

[doc('Format code')]
format:
    cargo fmt -- --emit=files

# meteo-firmware is no_std/thumbv7em; meteo-lib is hardware-agnostic and lints on
# the host target, where its tests run. Linting on separate targets avoids trying
# to build host code for the embedded target (or vice versa).
[doc('Check code with clippy')]
clippy:
    cargo clippy -p meteo-firmware -- -D warnings
    cargo clippy -p meteo-lib --all-features --all-targets --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run -p meteo-lib --target {{ host_target }}

# Ignored advisories (unmaintained transitive deps) are documented in
# .cargo/audit.toml; cargo-audit reads it automatically.
[doc('Audit dependencies for security advisories')]
audit:
    cargo audit
