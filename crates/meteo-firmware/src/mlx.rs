#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

//! MLX90614 sky-IR sampler: reads object + ambient temperature every 2 s and
//! sends a `SensorReading::SkyIr` to the aggregator. Failed/invalid reads send
//! `None` (graceful degradation → `sky_temp_c` blanks for that frame).

use defmt::{Debug2Format, debug, info, warn};
use embassy_time::{Duration, Timer};
use meteo_lib::mlx90614::Mlx90614;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;
use crate::bus::SharedI2c;

#[allow(dead_code, reason = "spawned in main.rs in substep 10")]
#[embassy_executor::task]
pub async fn read_sky(i2c: SharedI2c, address: u8) {
    debug!("Setting up MLX90614 sky-IR sensor");
    let mut sensor = Mlx90614::new(i2c, address);

    loop {
        let object_c = match sensor.object_temperature().await {
            Ok(v) => {
                info!("Sky/object temp: {}°C", trunc2(v));
                Some(v)
            }
            Err(e) => {
                warn!("MLX object read failed: {:?}", Debug2Format(&e));
                None
            }
        };
        let ambient_c = match sensor.ambient_temperature().await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("MLX ambient read failed: {:?}", Debug2Format(&e));
                None
            }
        };
        SENSOR_CHANNEL
            .send(SensorReading::SkyIr {
                object_c,
                ambient_c,
            })
            .await;
        // Read cadence: 2 s (gentler than the refresh rate; datasheet warns
        // continuous reads add noise). Periodic sample clock, not a readiness sleep.
        // `Timer::after` (gap-after-read), not `Ticker::every` — same rationale as the
        // BMP sampler: guarantee spacing between reads rather than a fixed cadence.
        Timer::after(Duration::from_secs(2)).await;
    }
}
