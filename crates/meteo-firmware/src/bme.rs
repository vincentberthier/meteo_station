#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::bme280::Bme280;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

/// BME280 humidity sampler. Unlike the BMP388 task this bumps **no** watchdog
/// beat: a failing or absent BME280 degrades to `None` humidity (and drops the
/// baro cross-check) without resetting the chip — it is a non-critical sensor.
#[embassy_executor::task]
pub async fn read_humidity(i2c: SharedI2c, address: u8) {
    debug!("Setting up BME280");
    // `None` until initialized. `SharedI2c` (an `I2cDevice`) is `Clone` and cheap to
    // copy (it just holds the `&'static Mutex` bus ref), so each (re)init attempt gets
    // a fresh handle while the task keeps the original for the next retry.
    let mut sensor: Option<Bme280<SharedI2c>> = None;
    // Whether we've already logged the current fault. Logging is edge-gated: one
    // WARN when the sensor goes from working to absent/failing, one INFO when it
    // comes back. The retry itself keeps running silently every cycle — a single
    // NAKing I2C probe per second is cheap and lets a real BME280 self-heal if one
    // is ever plugged in — but it never spams the log (retrying 1000× changes
    // nothing). The bench part is usually a mis-ordered BMP280, which has no
    // humidity and never ACKs at 0x76 here.
    let mut faulted = false;

    loop {
        // (Re)initialize on demand: covers a slow/absent sensor at boot and a bus
        // glitch that forced a re-init below.
        if sensor.is_none() {
            match Bme280::new(i2c.clone(), address).await {
                Ok(s) => {
                    info!("BME280 initialized successfully!");
                    sensor = Some(s);
                    faulted = false;
                }
                Err(e) => {
                    if !faulted {
                        warn!(
                            "BME280 not responding (absent or mis-ordered BMP280?); retrying silently: {:?}",
                            Debug2Format(&e)
                        );
                        faulted = true;
                    }
                }
            }
        }

        if let Some(s) = sensor.as_mut() {
            match s.read().await {
                Ok(reading) => {
                    info!(
                        "BME280 H:{}%RH T:{}°C P:{} hPa",
                        trunc2(reading.humidity),
                        trunc2(reading.temperature),
                        trunc2(reading.pressure_hpa())
                    );
                    SENSOR_CHANNEL
                        .send(SensorReading::Bme280 {
                            humidity_pct: reading.humidity,
                            temperature_c: reading.temperature,
                            pressure_hpa: reading.pressure_hpa(),
                        })
                        .await;
                }
                Err(e) => {
                    // Drop the driver and re-init next cycle so a transient bus fault
                    // self-heals rather than wedging on a stale handle. Edge-gated WARN.
                    if !faulted {
                        warn!(
                            "BME280 read failed, re-initializing: {:?}",
                            Debug2Format(&e)
                        );
                        faulted = true;
                    }
                    sensor = None;
                }
            }
        }

        // Report a fault whenever there is no live handle this cycle. The aggregator
        // blanks humidity + cross-check and raises the BME280_FAULT diagnostic bit;
        // a later successful read clears it.
        if sensor.is_none() {
            SENSOR_CHANNEL.send(SensorReading::Bme280Fault).await;
        }

        // Sampling cadence (1 Hz). Periodic sample clock, not a readiness sleep:
        // `Timer::after` spaces reads by a guaranteed gap *after* each read, same
        // rationale as the BMP388 sampler. No watchdog beat by design.
        Timer::after(Duration::from_secs(1)).await;
    }
}
