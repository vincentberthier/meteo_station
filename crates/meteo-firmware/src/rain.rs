//! Rain-gauge task: counts tipping-bucket closures on GPIO12 and publishes the
//! rainfall rate at 1 Hz.
//!
//! The tipping bucket is a passive reed switch (`SparkFun` SEN-15901): one closure
//! per 0.2794 mm of rain. The pin is held high by the internal pull-up and pulsed
//! low on each tip. The per-second tip count feeds a sliding-window
//! [`RainRate`](meteo_lib::weather_meter::RainRate) that reports mm/h. No watchdog
//! beat — a dry gauge legitimately reports 0 mm/h.

use defmt::{debug, info};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::gpio::Input;
use meteo_lib::SensorReading;
use meteo_lib::weather_meter::RainRate;

use crate::aggregator::SENSOR_CHANNEL;

/// Tipping-bucket debounce: the mechanical tip is slow, so closures within this
/// window are bounce, not a new tip (datasheet suggests ~100–200 ms). Enforced by
/// comparing edge timestamps — not a fixed sleep.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Counts falling edges on GPIO12, folds each second's tip count into a
/// [`RainRate`] window, and publishes [`SensorReading::Rain`] every second.
#[embassy_executor::task]
pub async fn read_rain(mut input: Input<'static>) {
    debug!("Setting up rain gauge (GPIO12)");
    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut rain = RainRate::new();
    let mut tips: u16 = 0;
    let mut last_edge = Instant::now();

    loop {
        match select(input.wait_for_falling_edge(), ticker.next()).await {
            Either::First(()) => {
                let now = Instant::now();
                if now.duration_since(last_edge) >= DEBOUNCE {
                    tips = tips.saturating_add(1);
                    last_edge = now;
                }
            }
            Either::Second(()) => {
                let rate = rain.push(tips);
                if let Some(mm_h) = rate {
                    info!("Rain: {=u16} tips/s, rate {} mm/h", tips, mm_h);
                } else {
                    info!("Rain: {=u16} tips/s (warming up, no rate yet)", tips);
                }
                SENSOR_CHANNEL
                    .send(SensorReading::Rain { rate_mm_h: rate })
                    .await;
                tips = 0;
            }
        }
    }
}
