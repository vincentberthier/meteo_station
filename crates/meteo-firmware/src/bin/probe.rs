//! Hands-free RJ11 live-pair probe — a throwaway bench tool, not part of the
//! weather-station firmware.
//!
//! Purpose: identify which two conductors of a `SparkFun` weather-meter RJ11 cable
//! are the live pair, *without* a multimeter and without enough hands to hold
//! probes while spinning/tipping the sensor. Jumper any of the cable's conductors
//! to the candidate GPIOs below, actuate the sensor (spin the cups, tip the
//! bucket, rotate the vane), and the live pair prints over defmt:
//!
//! ```text
//! LIVE PAIR: GPIO4 <-> GPIO5
//! ```
//!
//! How it works: the firmware runs a continuous matrix scan. Each candidate pin
//! takes a turn driven LOW (acting as the common return) while the others are
//! pulled up and read. A closed reed switch (anemometer, rain gauge) or a
//! low-resistance wind-vane position pulls a reader pin LOW, which identifies the
//! pair — regardless of which conductor is which, so no manual "this one is
//! ground" guessing. Each newly-found pair is logged once.
//!
//! Wiring: jumper as many of the cable's conductors as you have leads to any of
//! GPIO0 / GPIO3 / GPIO4 / GPIO5 (all free spare pins on the `DevKitM-1`). No GND
//! jumper is needed — the scan supplies the return. Unconnected candidate pins
//! never produce a false hit (a floating pulled-up input reads HIGH).
//!
//! Flash + monitor: `just probe` (or
//! `espflash flash --monitor --chip esp32h2 --log-format defmt \
//!   target/riscv32imac-unknown-none-elf/release/probe`).

#![no_std]
#![no_main]
#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::info;
use esp_hal::gpio::{Flex, InputConfig, Level, OutputConfig, Pull};
use esp_hal::main;
use {esp_backtrace as _, esp_println as _};

// The ESP-IDF second-stage bootloader (espflash v4) needs this app descriptor in
// the image to boot it.
esp_bootloader_esp_idf::esp_app_desc!();

/// Number of candidate probe pins.
const N: usize = 4;

/// Human-readable names for the candidate pins, parallel to the `Flex` array.
const PIN_NAMES: [&str; N] = ["GPIO0", "GPIO3", "GPIO4", "GPIO5"];

/// Bounded poll budget for a reader pin to charge back HIGH through its pull-up
/// after the scan reconfigures it. An *open* line wins the pull-up and reads HIGH
/// within its RC time (tens of µs worst case for a long cable); a line held LOW by
/// a closed reed never does. The cap is a circuit-breaker, sized well above any
/// realistic cable RC, so the only thing that exhausts it is a genuine closure.
const SETTLE_POLLS: u32 = 4000;

/// Configure a probe pin as a pulled-up input (a reader).
fn make_reader(pin: &mut Flex<'_>) {
    pin.set_output_enable(false);
    pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
    pin.set_input_enable(true);
}

/// Configure a probe pin as a push-pull output driven LOW (the common return).
fn make_driver_low(pin: &mut Flex<'_>) {
    pin.set_input_enable(false);
    pin.set_level(Level::Low);
    pin.apply_output_config(&OutputConfig::default());
    pin.set_output_enable(true);
}

/// Returns `true` iff the reader pin stays LOW for the whole bounded settle
/// window — i.e. it is held down by a closure to the current driver rather than
/// merely still charging. Bails out the instant it reads HIGH (an open line).
fn held_low(pin: &Flex<'_>) -> bool {
    let mut polls = 0_u32;
    while polls < SETTLE_POLLS {
        if pin.is_high() {
            return false;
        }
        polls = polls.wrapping_add(1);
    }
    true
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Candidate pins: GPIO0/3/4/5, all free spares on the DevKitM-1. Indices line
    // up with PIN_NAMES.
    let mut pins: [Flex<'_>; N] = [
        Flex::new(peripherals.GPIO0),
        Flex::new(peripherals.GPIO3),
        Flex::new(peripherals.GPIO4),
        Flex::new(peripherals.GPIO5),
    ];

    info!("RJ11 live-pair probe ready.");
    info!("Wire cable conductors to GPIO0/GPIO3/GPIO4/GPIO5 (no GND needed),");
    info!("then actuate the sensor. The live pair prints once when detected.");

    // Each pair is reported once; `reported[a][b]` with a < b.
    let mut reported = [[false; N]; N];

    loop {
        for driver in 0..N {
            // Reconfigure: `driver` drives LOW, every other pin reads pulled-up.
            for (idx, pin) in pins.iter_mut().enumerate() {
                if idx == driver {
                    make_driver_low(pin);
                } else {
                    make_reader(pin);
                }
            }

            for (reader, pin) in pins.iter().enumerate() {
                if reader == driver {
                    continue;
                }
                if held_low(pin) {
                    let (a, b) = if driver < reader {
                        (driver, reader)
                    } else {
                        (reader, driver)
                    };
                    if !reported[a][b] {
                        reported[a][b] = true;
                        info!("LIVE PAIR: {} <-> {}", PIN_NAMES[a], PIN_NAMES[b]);
                    }
                }
            }
        }
    }
}
