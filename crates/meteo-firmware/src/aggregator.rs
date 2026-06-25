//! Aggregator task: owns `TELEMETRY`, drains the sensor channel into a running
//! `meteo_lib::Aggregator`, and publishes a merged frame at 1 Hz.

use core::sync::atomic::Ordering;

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker};
use meteo_lib::aggregate::AggregatorConfig;
use meteo_lib::{Aggregator, SensorReading};

// TELEMETRY stays declared in ble.rs (unchanged: it is the latest-wins
// `Signal<CriticalSectionRawMutex, Telemetry>` the notify loop waits on). The
// aggregator is now its sole producer; the dependency direction is
// aggregator → ble (the BLE task no longer imports anything from the aggregator).
use crate::ble::TELEMETRY;
use crate::watchdog::AGG_BEAT;

/// Sky-IR occlusion threshold (°C). Field-tunable; revisit during real-sky testing.
const OCCLUSION_THRESHOLD_C: f32 = 5.0;
/// BMP388-vs-BME280 temperature cross-check divergence threshold (°C).
const TEMP_DIVERGENCE_C: f32 = 2.0;
/// BMP388-vs-BME280 pressure cross-check divergence threshold (hPa).
const PRESS_DIVERGENCE_HPA: f32 = 3.0;

/// Aggregator thresholds, grouped for a single readable construction site.
const AGG_CONFIG: AggregatorConfig = AggregatorConfig {
    occlusion_threshold_c: OCCLUSION_THRESHOLD_C,
    temp_divergence_c: TEMP_DIVERGENCE_C,
    press_divergence_hpa: PRESS_DIVERGENCE_HPA,
};

/// Inter-task sensor channel: every sensor task sends `SensorReading`s here; the
/// aggregator is the sole receiver. Capacity 16 ≫ the 9 producers at ≤1 Hz
/// (BMP388, MLX90614, BME280, VEML7700, anemometer, vane, rain, INA219 PV,
/// INA219 batt).
pub static SENSOR_CHANNEL: Channel<CriticalSectionRawMutex, SensorReading, 16> = Channel::new();

#[embassy_executor::task]
pub async fn run() {
    let mut agg = Aggregator::new(AGG_CONFIG);
    // Publish cadence: 1 Hz, decoupled from sensor read rates. A periodic Ticker is
    // the intended publish clock (a 1 Hz wall-clock publish is only observable via a
    // timer) — NOT a readiness sleep; cf. the BMP sampler and watchdog poll.
    let mut publish = Ticker::every(Duration::from_secs(1));
    loop {
        match select(SENSOR_CHANNEL.receive(), publish.next()).await {
            Either::First(reading) => agg.ingest(reading),
            Either::Second(()) => {
                TELEMETRY.signal(agg.snapshot());
                AGG_BEAT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
