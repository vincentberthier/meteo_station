//! Anemometer task: counts reed-switch closures on GPIO22 and publishes wind
//! speed at 1 Hz.
//!
//! The cup anemometer is a passive reed switch (`SparkFun` SEN-15901): one closure
//! per revolution, 1 Hz = 2.4 km/h. The pin is held high by the internal pull-up
//! and pulsed low on each closure. Like the other graceful-degradation sensors it
//! bumps **no** watchdog beat — a calm anemometer legitimately reports 0 m/s and
//! must not trip a reset.

use defmt::{debug, info};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::gpio::Input;
use meteo_lib::weather_meter::wind_speed_ms;
use meteo_lib::{SensorReading, trunc2};

use crate::aggregator::SENSOR_CHANNEL;

/// Reed-switch debounce: closures closer together than this are contact bounce,
/// not a new revolution (datasheet suggests ~10–15 ms). Enforced by comparing
/// edge timestamps — not a fixed sleep.
const DEBOUNCE: Duration = Duration::from_millis(10);

/// Counts falling edges on GPIO22 and publishes [`SensorReading::WindSpeed`] every
/// second (pulses in the 1 s window == pulses/second).
#[embassy_executor::task]
pub async fn read_wind_speed(mut input: Input<'static>) {
    debug!("Setting up anemometer (GPIO22)");
    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut pulses: u16 = 0;
    let mut last_edge = Instant::now();

    loop {
        match select(input.wait_for_falling_edge(), ticker.next()).await {
            Either::First(()) => {
                let now = Instant::now();
                if now.duration_since(last_edge) >= DEBOUNCE {
                    pulses = pulses.saturating_add(1);
                    last_edge = now;
                }
            }
            Either::Second(()) => {
                let speed = wind_speed_ms(f32::from(pulses));
                info!(
                    "Wind speed: {} m/s ({=u16} pulses/s)",
                    trunc2(speed),
                    pulses
                );
                SENSOR_CHANNEL
                    .send(SensorReading::WindSpeed { speed_ms: speed })
                    .await;
                pulses = 0;
            }
        }
    }
}
