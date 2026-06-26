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

// ---------------------------------------------------------------------------
// Tests (ssr only — LiveFrame::from_telemetry is ssr-gated)
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(all(test, feature = "ssr"))]
mod tests {
    use core::{error, result};

    use meteo_lib::{Telemetry, ble::frame::Diagnostics};
    use test_log::test;

    use super::LiveFrame;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// Construct a fully-populated `Telemetry` frame for tests.
    fn full_telemetry() -> Telemetry {
        Telemetry {
            temperature_c: Some(20.0),
            pressure_hpa: Some(1013.25),
            humidity_pct: Some(60.0),
            sky_temp_c: Some(-5.0),
            luminosity_lux: Some(500.0),
            wind_speed_ms: Some(2.0),
            wind_dir_deg: Some(180.0),
            battery_pct: Some(90),
            rain_rate_mm_h: Some(0.5),
            solar_mv: Some(5_000),
            solar_ma: Some(200),
            batt_mv: Some(4_100),
            load_ma: Some(100),
            diagnostics: Diagnostics(0),
            uptime_s: 1_234,
            latitude_deg: None,
            longitude_deg: None,
            altitude_m: None,
        }
    }

    /// `from_telemetry` must preserve all present sensor fields and compute
    /// power in watts using `meteo_chart::power_w`.
    #[test]
    #[expect(clippy::unwrap_used, reason = "test: values asserted to be Some")]
    fn live_frame_from_telemetry_maps_fields() -> TestResult {
        // Given
        let t = full_telemetry();

        // When
        let lf = LiveFrame::from_telemetry(&t);

        // Then — scalar fields are copied verbatim
        assert!(
            (lf.temperature_c.unwrap() - 20.0_f32).abs() < 1e-4,
            "temperature_c mismatch"
        );
        assert!(
            (lf.humidity_pct.unwrap() - 60.0_f32).abs() < 1e-4,
            "humidity_pct mismatch"
        );
        assert!(
            (lf.pressure_hpa.unwrap() - 1013.25_f32).abs() < 1e-2,
            "pressure_hpa mismatch"
        );
        assert!(
            (lf.sky_temp_c.unwrap() - (-5.0_f32)).abs() < 1e-4,
            "sky_temp_c mismatch"
        );
        assert!(
            (lf.wind_speed_ms.unwrap() - 2.0_f32).abs() < 1e-4,
            "wind_speed_ms mismatch"
        );
        assert!(
            (lf.wind_dir_deg.unwrap() - 180.0_f32).abs() < 1e-4,
            "wind_dir_deg mismatch"
        );
        assert_eq!(lf.battery_pct, Some(90), "battery_pct mismatch");
        assert_eq!(lf.uptime_s, 1_234, "uptime_s mismatch");

        // Power must match meteo_chart::power_w
        let expected_solar = meteo_chart::power_w(t.solar_mv, t.solar_ma);
        assert_eq!(lf.solar_w, expected_solar, "solar_w mismatch");
        // 5 V × 0.2 A = 1.0 W
        assert!(
            (lf.solar_w.unwrap() - 1.0).abs() < 1e-9,
            "solar_w should be 1.0 W"
        );

        let expected_load = meteo_chart::power_w(t.batt_mv, t.load_ma);
        assert_eq!(lf.load_w, expected_load, "load_w mismatch");
        // 4.1 V × 0.1 A = 0.41 W
        assert!(
            (lf.load_w.unwrap() - 0.41).abs() < 1e-6,
            "load_w should be 0.41 W"
        );

        Ok(())
    }

    /// A `Telemetry` with every optional field `None` must produce a
    /// `LiveFrame` with `solar_w` and `load_w` both `None` (no panic).
    #[test]
    fn live_frame_from_telemetry_all_none() -> TestResult {
        // Given — all Optional fields are None
        let t = Telemetry {
            temperature_c: None,
            pressure_hpa: None,
            humidity_pct: None,
            sky_temp_c: None,
            luminosity_lux: None,
            wind_speed_ms: None,
            wind_dir_deg: None,
            battery_pct: None,
            rain_rate_mm_h: None,
            solar_mv: None,
            solar_ma: None,
            batt_mv: None,
            load_ma: None,
            diagnostics: Diagnostics(0),
            uptime_s: 42,
            latitude_deg: None,
            longitude_deg: None,
            altitude_m: None,
        };

        // When
        let lf = LiveFrame::from_telemetry(&t);

        // Then — no panic, power fields are None, uptime_s is preserved
        assert!(
            lf.solar_w.is_none(),
            "solar_w must be None when inputs are None"
        );
        assert!(
            lf.load_w.is_none(),
            "load_w must be None when inputs are None"
        );
        assert_eq!(lf.uptime_s, 42, "uptime_s must be preserved");
        assert!(lf.temperature_c.is_none());
        assert!(lf.battery_pct.is_none());

        Ok(())
    }
}
// grcov exclude stop
