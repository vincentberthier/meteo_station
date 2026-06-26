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

# Bench tool: identify the live conductor pair of a weather-meter RJ11 cable
# hands-free. Wire cable conductors to GPIO0/3/4/5, actuate the sensor, watch the
# log print the live pair. See crates/meteo-firmware/src/bin/probe.rs.
[doc('Flash + monitor the RJ11 live-pair probe')]
probe:
    cargo build --release -p meteo-firmware --bin probe
    espflash flash --monitor --chip {{ chip }} --log-format defmt target/{{ target }}/release/probe

# --- Code quality recipes ---

[doc('Format code')]
format:
    cargo fmt -- --emit=files

# meteo-firmware is no_std/riscv32imac; meteo-lib is hardware-agnostic and lints on
# the host target, where its tests run. Linting on separate targets avoids trying
# to build host code for the embedded target (or vice versa).
[doc('Check code with clippy')]
clippy:
    cargo clippy -p meteo-firmware -- -D warnings
    cargo clippy -p meteo-lib --all-features --all-targets --target {{ host_target }} -- -D warnings
    cargo clippy -p meteo-tui --all-targets --target {{ host_target }} -- -D warnings
    cargo clippy -p meteo-chart --all-targets --target {{ host_target }} -- -D warnings
    cargo +nightly clippy -p meteo-web --no-default-features --features ssr --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run -p meteo-lib --target {{ host_target }}
    cargo nextest run -p meteo-tui --target {{ host_target }}
    cargo nextest run -p meteo-chart --target {{ host_target }}

# --- Dashboard recipes ---

[doc('Build the TUI dashboard (host target)')]
tui-build:
    cargo build -p meteo-tui --target {{ host_target }}

[doc('Run the TUI dashboard (host target)')]
tui-run *ARGS:
    cargo run -p meteo-tui --target {{ host_target }} -- {{ ARGS }}

[doc('Clippy the TUI crate only (fast host-side loop)')]
tui-clippy:
    cargo clippy -p meteo-tui --all-targets --target {{ host_target }} -- -D warnings

[doc('Build the web dashboard (SSR + wasm via cargo-leptos)')]
web-build:
    cargo +nightly leptos build --release -p meteo-web

[doc('Cross-build the web dashboard server for the Raspberry Pi (aarch64)')]
web-build-pi:
    cargo +nightly leptos build --release -p meteo-web --bin-target-triple aarch64-unknown-linux-gnu

[doc('Serve the web dashboard locally (hot-reload)')]
web-serve:
    cargo +nightly leptos serve -p meteo-web

[doc('Watch + rebuild the web dashboard')]
web-watch:
    cargo +nightly leptos watch -p meteo-web

[doc('Clippy the web crate (ssr + hydrate)')]
web-clippy:
    cargo +nightly clippy -p meteo-web --no-default-features --features ssr --target {{ host_target }} -- -D warnings
    cargo +nightly clippy -p meteo-web --no-default-features --features hydrate --target wasm32-unknown-unknown -- -D warnings

# Ignored advisories (unmaintained transitive deps) are documented in
# .cargo/audit.toml; cargo-audit reads it automatically.
[doc('Audit dependencies for security advisories')]
audit:
    cargo audit

# --- Hardware recipes ---

# Regenerates the schematic from gen_power_sch.py, checks ERC, and re-exports the
# PDF/SVG. Needs kicad-cli + the system KiCad symbol libraries. ERC report goes to
# /tmp to keep the tree clean.
[doc('Regenerate the power-subsystem KiCad schematic + PDF/SVG')]
power-sch:
    python3 hardware/power/gen_power_sch.py hardware/power
    kicad-cli sch erc -o /tmp/meteo_power-erc.rpt --exit-code-violations hardware/power/meteo_power.kicad_sch
    kicad-cli sch export pdf -o hardware/power/meteo_power.pdf hardware/power/meteo_power.kicad_sch
    kicad-cli sch export svg -o /tmp/meteo_power_svg hardware/power/meteo_power.kicad_sch
    cp /tmp/meteo_power_svg/meteo_power.svg hardware/power/meteo_power.svg
