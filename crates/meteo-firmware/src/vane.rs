//! Wind-vane task: reads the resistive vane divider on GPIO1 (ADC1) and publishes
//! the heading at 1 Hz.
//!
//! The wind vane (`SparkFun` SEN-15901) is a resistor network wired as
//! `3V3 ─ 10kΩ ─ GPIO1 ─ vane ─ GND`; each of the 16 headings presents a distinct
//! divider voltage. The ADC is read with line calibration so `read_oneshot`
//! returns millivolts, which
//! [`wind_direction_deg`](meteo_lib::weather_meter::wind_direction_deg) maps to the
//! nearest compass heading. No watchdog beat — a degraded read just yields a
//! plausible heading.

use defmt::{debug, info};
use embassy_time::{Duration, Ticker};
use esp_hal::analog::adc::{Adc, AdcCalLine, AdcConfig, Attenuation};
use esp_hal::peripherals::{ADC1, GPIO1};
use meteo_lib::SensorReading;
use meteo_lib::weather_meter::wind_direction_deg;

use crate::aggregator::SENSOR_CHANNEL;

/// Builds ADC1 on GPIO1 (line-calibrated, 11 dB for the full ~0–3.3 V range) and
/// publishes [`SensorReading::WindDir`] every second.
#[embassy_executor::task]
pub async fn read_wind_dir(adc1: ADC1<'static>, gpio1: GPIO1<'static>) {
    debug!("Setting up wind vane (GPIO1 / ADC1)");
    let mut config = AdcConfig::new();
    let mut pin = config.enable_pin_with_cal::<_, AdcCalLine<ADC1>>(gpio1, Attenuation::_11dB);
    let mut adc = Adc::new(adc1, config).into_async();

    // 1 Hz sample cadence (the publish clock, not a readiness sleep).
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        let mv = adc.read_oneshot(&mut pin).await;
        let dir = wind_direction_deg(mv);
        info!("Wind dir: {=u16} mV -> {} deg", mv, dir);
        SENSOR_CHANNEL
            .send(SensorReading::WindDir { dir_deg: dir })
            .await;
    }
}
