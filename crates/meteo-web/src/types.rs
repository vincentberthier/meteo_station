//! Shared serde DTOs — compiled for both SSR and wasm32 (hydrate) targets.
//!
//! Plain struct fields carry no `meteo-lib` dependencies so they compile for
//! wasm32 without issue. The [`LiveFrame::from_telemetry`] constructor is
//! `#[cfg(feature = "ssr")]`-gated because the [`meteo_lib::ble::frame::Telemetry`]
//! type originates from BLE data that only the server receives.

use serde::{Deserialize, Serialize};

/// One aggregated history bucket sent to the client.
///
/// `ts` is the bucket's representative unix second (typically the earliest
/// stored minute in the re-aggregated window). Each sensor metric carries its
/// `(min, max, avg)` triple; any component is `None` when all raw rows in the
/// window had `NULL` for that field.
///
/// Wind-direction note: `wind_dir_avg` is a simple arithmetic mean of the
/// per-minute stored values. Direction is display-only and minute resolution
/// already smooths the signal sufficiently; a vector-mean at re-aggregation
/// would add complexity for negligible accuracy gain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRow {
    /// Representative unix timestamp (seconds) for this bucket.
    pub ts: i64,
    /// Air temperature `(min, max, avg)` in °C.
    pub temp: MetricStat,
    /// Barometric pressure `(min, max, avg)` in hPa.
    pub pressure: MetricStat,
    /// Relative humidity `(min, max, avg)` in %.
    pub humidity: MetricStat,
    /// Sky (IR) temperature `(min, max, avg)` in °C.
    pub sky: MetricStat,
    /// Illuminance `(min, max, avg)` in lux.
    pub lux: MetricStat,
    /// Wind speed `(min, max, avg)` in m/s; `max` = gust.
    pub wind: MetricStat,
    /// Average wind direction in degrees (0–360).
    pub wind_dir_avg: Option<f64>,
    /// Rain rate stats in mm/h; `min` field is unused (always `None`).
    pub rain: MetricStat,
    /// Average battery state-of-charge in percent.
    pub battery_avg: Option<f64>,
    /// Average solar power in watts (`power_w(solar_mv_avg, solar_ma_avg)`).
    /// Computed in Rust from the raw millivolt/milliamp averages.
    pub solar_w_avg: Option<f64>,
    /// Average load power in watts (`power_w(batt_mv_avg, load_ma_avg)`).
    /// Load draws from the battery rail, so `batt_mv` is the bus voltage.
    pub load_w_avg: Option<f64>,
}

/// `(min, max, avg)` triple for one sensor metric; any component may be `None`
/// when no valid samples existed in the window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MetricStat {
    /// Minimum value in the bucket window, or `None`.
    pub min: Option<f64>,
    /// Maximum value in the bucket window, or `None`.
    pub max: Option<f64>,
    /// Sample-count-weighted average in the bucket window, or `None`.
    pub avg: Option<f64>,
}

/// Selects a sensor metric for historical and comparison queries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Metric {
    /// Ambient air temperature.
    AirTemp,
    /// Barometric pressure.
    Pressure,
    /// Relative humidity.
    Humidity,
    /// Sky (IR) temperature from the MLX90614.
    SkyTemp,
    /// Illuminance in lux.
    Lux,
    /// Wind speed.
    Wind,
    /// Rainfall rate.
    Rain,
    /// Battery state-of-charge.
    Battery,
    /// Solar harvest power (derived from PV-side INA219 voltage × current).
    Solar,
    /// Load power (derived from battery-side INA219 voltage × current).
    Load,
}

/// One point in a historical trace used by the comparison view.
/// `x` = seconds elapsed since the start of the queried range (e.g. midnight
/// UTC for a daily trace); `y` = the metric's average value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracePoint {
    /// Seconds since the range start.
    pub x: f64,
    /// Metric value (units depend on the [`Metric`] variant).
    pub y: f64,
}

/// Instantaneous sensor frame for the live dashboard band.
///
/// Decoded from a BLE manufacturer-data advertisement; power values are already
/// converted to watts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveFrame {
    /// Ambient temperature in °C.
    pub temperature_c: Option<f32>,
    /// Relative humidity in %.
    pub humidity_pct: Option<f32>,
    /// Barometric pressure in hPa.
    pub pressure_hpa: Option<f32>,
    /// Sky (IR) temperature in °C.
    pub sky_temp_c: Option<f32>,
    /// Wind speed in m/s.
    pub wind_speed_ms: Option<f32>,
    /// Wind direction in degrees (0–360).
    pub wind_dir_deg: Option<f32>,
    /// Solar harvest power in watts (`power_w(solar_mv, solar_ma)`).
    pub solar_w: Option<f64>,
    /// Load power in watts (`power_w(batt_mv, load_ma)`). The load draws from
    /// the battery rail, so `batt_mv` is the bus voltage for this calculation.
    pub load_w: Option<f64>,
    /// Battery state-of-charge in percent (0–100), derived on-device from
    /// `batt_mv` via the 1S-LiPo voltage curve.
    pub battery_pct: Option<u8>,
    /// Seconds since the station's last reboot (monotonic).
    pub uptime_s: u32,
}

#[cfg(feature = "ssr")]
impl LiveFrame {
    /// Convert a [`meteo_lib::ble::frame::Telemetry`] frame to a [`LiveFrame`].
    ///
    /// `solar_w` = `power_w(solar_mv, solar_ma)`.
    /// `load_w` = `power_w(batt_mv, load_ma)` — load draws from the battery
    /// rail, so `batt_mv` (not a separate load-side voltage) is the bus voltage.
    #[must_use]
    pub fn from_telemetry(t: &meteo_lib::ble::frame::Telemetry) -> Self {
        Self {
            temperature_c: t.temperature_c,
            humidity_pct: t.humidity_pct,
            pressure_hpa: t.pressure_hpa,
            sky_temp_c: t.sky_temp_c,
            wind_speed_ms: t.wind_speed_ms,
            wind_dir_deg: t.wind_dir_deg,
            solar_w: meteo_chart::power_w(t.solar_mv, t.solar_ma),
            load_w: meteo_chart::power_w(t.batt_mv, t.load_ma),
            battery_pct: t.battery_pct,
            uptime_s: t.uptime_s,
        }
    }
}
