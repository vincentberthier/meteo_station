# MeteoStation build system
# Usage: just <recipe> [args...]

# --- Variables ---

target := "thumbv7em-none-eabihf"
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

[doc('Run the BLE client CLI')]
cli:
    cargo run -p meteo-cli

# --- Code quality recipes ---

[doc('Format code')]
format:
    cargo fmt -- --emit=files

[doc('Check code with clippy')]
clippy:
    cargo clippy -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run --lib --target x86_64-unknown-linux-gnu
