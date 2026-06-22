#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::veml7700::{self, Veml7700};
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

/// (Re)initialise the VEML7700: verify the ID, then write the current ladder
/// setting (which also powers the part on). Returns `false` on any bus error.
async fn init(sensor: &mut Veml7700<SharedI2c>, idx: usize) -> bool {
    if let Err(e) = sensor.verify_id().await {
        warn!("VEML verify_id failed, retrying: {:?}", Debug2Format(&e));
        return false;
    }
    if let Err(e) = sensor.set_setting(veml7700::LADDER[idx]).await {
        warn!("VEML set_setting failed, retrying: {:?}", Debug2Format(&e));
        return false;
    }
    true
}

/// One sample cycle: wait the integration period, read the raw count, auto-range,
/// and publish lux once settled. Returns `false` on a bus error (caller re-inits).
///
/// The VEML7700 exposes **no** data-ready pin or status bit (datasheet): the
/// integration time *is* the conversion period, so the wait below is the
/// hardware's specified settling time derived from the active setting — not a
/// tuned readiness guess. It plays the same role bmp388's DRDY poll does, which
/// the VEML lacks. On any gain/IT change the next reading is discarded (datasheet
/// rule), tracked by `discard_next`.
async fn sample(
    sensor: &mut Veml7700<SharedI2c>,
    idx: &mut usize,
    discard_next: &mut bool,
) -> bool {
    let setting = veml7700::LADDER[*idx];
    Timer::after(Duration::from_millis(u64::from(setting.it.millis()))).await;

    let raw = match sensor.read_raw().await {
        Ok(raw) => raw,
        Err(e) => {
            warn!("VEML read failed, re-initializing: {:?}", Debug2Format(&e));
            return false;
        }
    };

    let next = veml7700::next_index(*idx, raw);
    if next != *idx {
        *idx = next;
        if let Err(e) = sensor.set_setting(veml7700::LADDER[*idx]).await {
            warn!(
                "VEML set_setting failed, re-initializing: {:?}",
                Debug2Format(&e)
            );
            return false;
        }
        *discard_next = true; // first read after a range change is stale
    } else if *discard_next {
        *discard_next = false; // settle done; the next cycle reports
    } else {
        let lux = veml7700::raw_to_lux(raw, setting);
        info!("Luminosity: {} lux (raw {})", trunc2(lux), raw);
        SENSOR_CHANNEL.send(SensorReading::Luminosity { lux }).await;
    }
    true
}

/// VEML7700 ambient-light sampler with auto-ranging. Like the BME280 task it
/// bumps **no** watchdog beat: a failing or absent VEML7700 degrades to `None`
/// luminosity without resetting the chip.
///
/// `Veml7700::new` is infallible (`const fn`, no bus traffic) and the owned
/// `I2cDevice` handle stays valid across a transient bus error, so recovery only
/// repeats the `verify_id + set_setting` handshake — there is nothing to
/// re-create. Hence the handle is taken by value (no `clone()`).
#[embassy_executor::task]
pub async fn read_luminosity(i2c: SharedI2c, address: u8) {
    debug!("Setting up VEML7700");
    let mut sensor = Veml7700::new(i2c, address);
    let mut idx = veml7700::LADDER_START;
    let mut initialized = false;
    let mut discard_next = true; // first sample after any (re)config is stale

    loop {
        if !initialized && init(&mut sensor, idx).await {
            info!("VEML7700 initialized successfully!");
            initialized = true;
            discard_next = true;
        }

        if initialized {
            initialized = sample(&mut sensor, &mut idx, &mut discard_next).await;
        }

        // No live handshake this cycle: report a fault (aggregator blanks luminosity
        // and raises VEML7700_FAULT) and pace the re-init attempts at 1 Hz. A settled
        // sample path does its own integration-time wait, so no extra delay there.
        if !initialized {
            SENSOR_CHANNEL.send(SensorReading::LuminosityFault).await;
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}
