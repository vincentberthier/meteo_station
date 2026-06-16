#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::{Debug2Format, debug, error, info, warn};
use embassy_time::{Duration, Timer};
use esp_hal::Async;
use esp_hal::i2c::master::I2c;
use meteo_lib::bmp388::Bmp388;
use meteo_lib::{Telemetry, trunc2};

#[embassy_executor::task]
pub async fn read_barometer(i2c: I2c<'static, Async>, address: u8) {
    debug!("Setting up barometer");

    let mut sensor = match Bmp388::new(i2c, address).await {
        Ok(s) => {
            info!("BMP388 initialized successfully!");
            s
        }
        Err(e) => {
            error!("Failed to initialize BMP388: {:?}", Debug2Format(&e));
            return;
        }
    };

    loop {
        match sensor.read().await {
            Ok(reading) => {
                info!(
                    "Temperature: {}°C, Pressure: {} Pa ({} hPa)",
                    trunc2(reading.temperature),
                    trunc2(reading.pressure),
                    trunc2(reading.pressure_hpa())
                );
                let telem = Telemetry::from_bmp388(&reading);
                crate::ble::TELEMETRY.signal(telem);
            }
            Err(e) => {
                warn!("Failed to read sensor: {:?}", Debug2Format(&e));
            }
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}
