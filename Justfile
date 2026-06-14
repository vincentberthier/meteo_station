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

[doc('Run the BLE client CLI')]
cli:
    cargo run -p meteo-cli --target {{ host_target }}

# --- BLE debug recipes ---

# Full cross-machine BLE capture: RTT here + HCI trace & meteo-cli on Gaia.
# See CLAUDE.md "BLE debugging on Gaia" for prerequisites.
[doc('Run a full cross-machine BLE debug capture (probe here, BT adapter on Gaia)')]
ble-debug:
    ./scripts/ble-debug.sh

[doc('Run the BLE client CLI on the Gaia host (the machine with the BT adapter)')]
cli-gaia:
    ssh gaia "bash -c 'cd ~/code/meteo_station && cargo run -q -p meteo-cli --target {{ host_target }}'"

# --- Code quality recipes ---

[doc('Format code')]
format:
    cargo fmt -- --emit=files

# meteo-firmware is no_std/thumbv7em; meteo-cli is host-only (btleplug/tokio).
# Lint each on its own target — a single workspace clippy would try to build the
# host crate (and its deps) for the embedded target and fail.
[doc('Check code with clippy')]
clippy:
    cargo clippy -p meteo-firmware -- -D warnings
    cargo clippy -p meteo-lib -p meteo-cli --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run --lib --target x86_64-unknown-linux-gnu
