#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use core::sync::atomic::Ordering;

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::bmp388::Bmp388;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

#[embassy_executor::task]
pub async fn read_barometer(i2c: SharedI2c, address: u8) {
    debug!("Setting up barometer");
    // `None` until initialized. `SharedI2c` (an `I2cDevice`) is `Clone` and cheap to
    // copy (it just holds the `&'static Mutex` bus ref), so each (re)init attempt gets
    // a fresh handle while the task keeps the original for the next retry.
    let mut sensor: Option<Bmp388<SharedI2c>> = None;

    loop {
        // (Re)initialize on demand: covers a slow/absent sensor at boot and a bus
        // glitch that forced a re-init below — instead of returning (which froze the
        // task and reboot-looped the chip).
        if sensor.is_none() {
            match Bmp388::new(i2c.clone(), address).await {
                Ok(s) => {
                    info!("BMP388 initialized successfully!");
                    sensor = Some(s);
                }
                Err(e) => warn!("BMP388 init failed, retrying: {:?}", Debug2Format(&e)),
            }
        }

        if let Some(s) = sensor.as_mut() {
            match s.read().await {
                Ok(reading) => {
                    info!(
                        "Temperature: {}°C, Pressure: {} Pa ({} hPa)",
                        trunc2(reading.temperature),
                        trunc2(reading.pressure),
                        trunc2(reading.pressure_hpa())
                    );
                    SENSOR_CHANNEL
                        .send(SensorReading::Barometer {
                            temperature_c: reading.temperature,
                            pressure_hpa: reading.pressure_hpa(),
                        })
                        .await;
                }
                Err(e) => {
                    // Drop the driver and re-init next cycle so a transient bus fault
                    // self-heals rather than wedging on a stale handle.
                    warn!("BMP read failed, re-initializing: {:?}", Debug2Format(&e));
                    sensor = None;
                }
            }
        }

        // Report a fault whenever there is no live handle this cycle (init failed, or a
        // read error just dropped it). The aggregator blanks temp/pressure and raises
        // the BARO_FAULT diagnostic bit; a later successful read clears it.
        if sensor.is_none() {
            SENSOR_CHANNEL.send(SensorReading::BarometerFault).await;
        }

        // Liveness heartbeat: bumped EVERY iteration to prove the task is cycling
        // (executor alive), NOT only on a successful read. A dead/absent sensor →
        // no Barometer frames (graceful: temperature/pressure go None downstream),
        // but the task stays alive and the RWDT is not falsely tripped. A genuinely
        // hung task (e.g. deadlocked on the bus mutex) still stalls BMP_BEAT → reset.
        crate::watchdog::BMP_BEAT.fetch_add(1, Ordering::Relaxed);

        // Sampling cadence (1 Hz). Periodic sample clock, not a readiness sleep.
        // `Timer::after` (not `Ticker::every`) is deliberate for samplers: it spaces
        // reads by a guaranteed gap *after* each read completes, so a slow bus read
        // can't make ticks pile up and back-to-back hammer the sensor. The aggregator
        // uses `Ticker::every` instead because its publish must hold a fixed 1 Hz
        // wall-clock cadence independent of how long ingest takes. A few ms of
        // per-cycle drift here is irrelevant for a weather sampler.
        Timer::after(Duration::from_secs(1)).await;
    }
}
