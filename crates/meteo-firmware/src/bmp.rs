#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::*;
use embassy_stm32::i2c::{I2c, Master};
use embassy_stm32::mode::Async;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use meteo_lib::bmp388::{Bmp388, Reading};
use meteo_lib::trunc2;

const BMP388_ADDR: u8 = 0x77;

#[embassy_executor::task]
pub async fn read_barometer(
    i2c: I2c<'static, Async, Master>,
    channel: &'static Channel<ThreadModeRawMutex, Reading, 1>,
) {
    debug!("Setting up barometer");
    Timer::after(Duration::from_millis(100)).await;

    let mut sensor = match Bmp388::new(i2c, BMP388_ADDR).await {
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
                // Publish to BLE task. Non-blocking: if channel is full, drop silently.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "intentional: next reading comes in 1s"
                )]
                let _ = channel.try_send(reading);
            }
            Err(e) => {
                warn!("Failed to read sensor: {:?}", Debug2Format(&e));
            }
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}
