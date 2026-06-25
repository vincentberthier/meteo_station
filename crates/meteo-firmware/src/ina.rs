//! INA219 power-monitor task. One task instance per rail (spawned twice):
//! the PV-side monitor (U6 @ 0x40) and the battery-side monitor (U7 @ 0x41),
//! both on the shared I2C0 bus.
//!
//! Like the BME280 and VEML7700 tasks it bumps **no** watchdog beat: a failing
//! or absent INA219 degrades to `None` power data (and raises the matching INA
//! diagnostics bit) without resetting the chip.

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Ticker};
use meteo_lib::SensorReading;
use meteo_lib::ina219::Ina219;

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

/// Which power rail an INA219 instance monitors. Selects the `SensorReading`
/// variant (and fault variant) the task publishes.
#[derive(Clone, Copy)]
pub enum Rail {
    /// PV-side monitor (U6 @ 0x40): panel voltage + harvest current.
    Solar,
    /// Battery-side monitor (U7 @ 0x41): battery voltage + load current.
    Battery,
}

impl Rail {
    /// Builds the data reading for this rail.
    const fn reading(self, mv: u16, ma: u16) -> SensorReading {
        match self {
            Self::Solar => SensorReading::SolarPower { mv, ma },
            Self::Battery => SensorReading::BatteryPower { mv, ma },
        }
    }

    /// Builds the fault reading for this rail.
    const fn fault(self) -> SensorReading {
        match self {
            Self::Solar => SensorReading::SolarPowerFault,
            Self::Battery => SensorReading::BatteryPowerFault,
        }
    }

    /// Human-readable label for logs.
    const fn label(self) -> &'static str {
        match self {
            Self::Solar => "INA219 PV",
            Self::Battery => "INA219 batt",
        }
    }
}

/// INA219 power sampler. Spawned once per rail (`pool_size = 2`). The INA219 runs
/// in continuous shunt+bus mode, so a 1 Hz read is always fresh; a `Ticker` paces
/// both the publish cadence and the re-init retry.
#[embassy_executor::task(pool_size = 2)]
pub async fn read_power(i2c: SharedI2c, address: u8, rail: Rail) {
    debug!("Setting up {}", rail.label());
    let mut sensor = Ina219::new(i2c, address);
    let mut initialized = false;
    let mut ticker = Ticker::every(Duration::from_secs(1));

    loop {
        ticker.next().await;

        if !initialized {
            match sensor.init().await {
                Ok(()) => {
                    info!("{} initialized successfully!", rail.label());
                    initialized = true;
                }
                Err(e) => {
                    warn!(
                        "{} init failed, retrying: {:?}",
                        rail.label(),
                        Debug2Format(&e)
                    );
                }
            }
        }

        if initialized {
            match sensor.read().await {
                Ok(reading) => {
                    // Current is clamped to ≥ 0 for the unsigned wire field; this
                    // wiring only ever sources (harvest) or sinks (load), never
                    // reverses, so a small negative noise reading floors at 0.
                    let ma = u16::try_from(reading.current_ma.max(0)).unwrap_or(u16::MAX);
                    info!(
                        "{}: {} mV, {} mA",
                        rail.label(),
                        reading.bus_mv,
                        reading.current_ma
                    );
                    SENSOR_CHANNEL.send(rail.reading(reading.bus_mv, ma)).await;
                }
                Err(e) => {
                    warn!(
                        "{} read failed, re-initializing: {:?}",
                        rail.label(),
                        Debug2Format(&e)
                    );
                    initialized = false;
                }
            }
        }

        // No live reading this cycle: report a fault so the aggregator blanks the
        // rail's V/I and raises the matching INA diagnostics bit.
        if !initialized {
            SENSOR_CHANNEL.send(rail.fault()).await;
        }
    }
}
