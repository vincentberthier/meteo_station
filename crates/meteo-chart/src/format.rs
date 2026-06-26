//! Pure display helpers for sensor data formatting, shared by TUI and web.

/// Threshold (lux) below which luminosity reads in raw lux rather than kilolux.
///
/// Below 1000 lux, kilolux rounds to `0.0 klx` and loses all resolution (a
/// moonlit night, an overcast dusk, indoor light all collapse to zero); raw lux
/// keeps the reading meaningful. At/above the threshold kilolux is the readable
/// unit (daylight runs to ~100 klx).
pub const LUX_KLX_THRESHOLD: f64 = 1000.0;

/// Nominal 1S-LiPo energy budget for the crude autonomy estimate (best-effort).
pub const BATTERY_WH: f64 = 9.6; // 3.7 V × 2.6 Ah

/// Format the station location row.
///
/// Returns `"not set"` until both latitude and longitude are present; otherwise
/// `"{lat:.2}, {lon:.2}"` or `"{lat:.2}, {lon:.2}, {alt:.0} m"` when altitude
/// is also set. Coarse values render at 2 decimals (lat/lon, ~1 km precision).
#[must_use]
pub fn fmt_location(lat: Option<f32>, lon: Option<f32>, alt: Option<f32>) -> String {
    match (lat, lon) {
        (Some(la), Some(lo)) => alt.map_or_else(
            || format!("{la:.2}, {lo:.2}"),
            |a| format!("{la:.2}, {lo:.2}, {a:.0} m"),
        ),
        _ => "not set".to_owned(),
    }
}

/// Format luminosity with an adaptive unit.
///
/// Returns `"N/A"` for `None`. Below [`LUX_KLX_THRESHOLD`] the value reads as
/// `"{lux:.0} lx"`; at or above it as `"{klx:.1} klx"`.
#[must_use]
pub fn fmt_lux(lux: Option<f32>) -> String {
    lux.map_or_else(
        || "N/A".to_owned(),
        |x| {
            let v = f64::from(x);
            if v < LUX_KLX_THRESHOLD {
                format!("{v:.0} lx")
            } else {
                format!("{:.1} klx", v / 1000.0)
            }
        },
    )
}

/// Pick the chart unit, label scale, and precision for a luminosity series given
/// its peak value (lux).
///
/// Below [`LUX_KLX_THRESHOLD`] → `("lx", 1.0, 0)`; at or above → `("klx", 0.001,
/// 1)`. The peak (not the latest sample) drives the choice so the unit stays
/// stable as the trace scrolls — a window that captured daylight keeps klx even
/// after dark, matching the axis range, which also spans that peak.
#[must_use]
pub fn lux_chart_unit(peak_lux: f64) -> (&'static str, f64, usize) {
    if peak_lux < LUX_KLX_THRESHOLD {
        ("lx", 1.0, 0)
    } else {
        ("klx", 0.001, 1)
    }
}

/// French 16-point compass label for a heading in degrees.
///
/// Convention: 0°=N, 90°=E, 180°=S, 270°=O (Ouest). 22.5° sector bucketing;
/// returns the French rose: `N NNE NE ENE E ESE SE SSE S SSO SO OSO O ONO NO NNO`.
#[must_use]
pub fn compass_label_fr(deg: f32) -> &'static str {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSO", "SO", "OSO", "O", "ONO",
        "NO", "NNO",
    ];
    let norm = deg.rem_euclid(360.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "round() of a value in [0,16] is a small non-negative whole number"
    )]
    let sector = (norm / 22.5).round() as usize;
    // 360° rounds up to sector 16, which wraps back to N (sector 0).
    let idx = if sector >= POINTS.len() { 0 } else { sector };
    POINTS[idx]
}

/// Power in watts from bus millivolts × current milliamperes.
///
/// Returns `(mv / 1000) × (ma / 1000)` as `Some(f64)`, or `None` if either
/// input is `None`.
#[must_use]
pub fn power_w(mv: Option<u16>, ma: Option<u16>) -> Option<f64> {
    Some((f64::from(mv?) / 1000.0) * (f64::from(ma?) / 1000.0))
}

/// Format a (non-`None`) power reading in watts with an adaptive unit and ~3
/// significant digits.
///
/// Below 1 W the value reads as `"{mw:.0} mW"` — a 0.349 W load shows
/// `"349 mW"` instead of a coarse `"0.3 W"` that hides all variation; at or
/// above 1 W as `"{w:.2} W"`. The caller keeps the `None` → `"N/A"` rendering so
/// it can style the absent case distinctly.
#[must_use]
pub fn fmt_power(w: f64) -> String {
    if w.abs() < 1.0 {
        format!("{:.0} mW", w * 1000.0)
    } else {
        format!("{w:.2} W")
    }
}

/// Battery flow status line for the ÉNERGIE card.
///
/// `net = solar_w − load_w`. Returns the rendered line:
/// - `net > 0` → `"▲ en charge · +{net:.1} W"`
/// - `net < 0` → `"▼ décharge · {net:.1} W · ~{h:.1} h"` (autonomy from `pct`
///   and [`BATTERY_WH`])
/// - `net ≈ 0` → `"— stable"`
///
/// Returns `"N/A"` when either power reading is `None`.
#[must_use]
pub fn fmt_battery_flow(solar_w: Option<f64>, load_w: Option<f64>, pct: Option<u8>) -> String {
    let (Some(s), Some(l)) = (solar_w, load_w) else {
        return "N/A".to_owned();
    };
    let net = s - l;
    if net > 0.05 {
        format!("▲ en charge · +{net:.1} W")
    } else if net < -0.05 {
        let autonomy = pct.map(|p| BATTERY_WH * f64::from(p) / 100.0 / l);
        autonomy.map_or_else(
            || format!("▼ décharge · {net:.1} W"),
            |h| format!("▼ décharge · {net:.1} W · ~{h:.1} h"),
        )
    } else {
        "— stable".to_owned()
    }
}

/// Dew point in °C computed from the Magnus/WMO formula (a=17.62, b=243.12 °C).
///
/// `Td = b·γ / (a−γ)` with `γ = ln(rh/100) + a·t/(b+t)`.
/// `rh` is clamped to `(0.01, 100]` to avoid `ln(0)`.
#[must_use]
pub fn dew_point_c(temp_c: f32, rh_pct: f32) -> f32 {
    const A: f32 = 17.62;
    const B: f32 = 243.12;
    let rh = rh_pct.clamp(0.01, 100.0) / 100.0;
    let gamma = rh.ln() + (A * temp_c) / (B + temp_c);
    B * gamma / (A - gamma)
}

/// 10-min air-temperature trend classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// Temperature is increasing.
    Rising,
    /// Temperature is decreasing.
    Falling,
    /// Temperature change is within the stability epsilon.
    Stable,
}

/// Classify a trend delta.
///
/// Returns [`Trend::Stable`] if `|delta| < eps`, [`Trend::Rising`] for a positive
/// delta, and [`Trend::Falling`] for a negative delta.
#[must_use]
pub fn classify_trend(delta: f64, eps: f64) -> Trend {
    if delta.abs() < eps {
        Trend::Stable
    } else if delta > 0.0 {
        Trend::Rising
    } else {
        Trend::Falling
    }
}

/// Format an uptime duration as a compact human-readable label.
///
/// - ≥ 3600 s → `"{h}h{mm}m"` (e.g. 3725 → `"1h02m"`)
/// - ≥ 60 s   → `"{m}m{ss}s"` (e.g. 90 → `"1m30s"`)
/// - < 60 s   → `"0m{ss}s"` (e.g. 45 → `"0m45s"`)
///
/// Minutes and seconds are zero-padded to two digits; hours are unpadded.
#[must_use]
pub fn fmt_uptime(secs: u32) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let mm = (secs % 3600) / 60;
        format!("{h}h{mm:02}m")
    } else if secs >= 60 {
        let m = secs / 60;
        let ss = secs % 60;
        format!("{m}m{ss:02}s")
    } else {
        format!("0m{secs:02}s")
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "TestResult is the standard test pattern"
)]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // --- fmt_location tests ---

    #[test]
    fn fmt_location_set() -> TestResult {
        // Given / When / Then — both lat and lon present, with altitude
        assert_eq!(
            fmt_location(Some(48.85), Some(2.35), Some(35.0)),
            "48.85, 2.35, 35 m"
        );
        // Without altitude
        assert_eq!(fmt_location(Some(48.85), Some(2.35), None), "48.85, 2.35");
        Ok(())
    }

    #[test]
    fn fmt_location_unset() -> TestResult {
        // Given / When / Then — missing lat or lon → "not set"
        assert_eq!(fmt_location(None, None, None), "not set");
        assert_eq!(fmt_location(Some(48.85), None, Some(35.0)), "not set");
        assert_eq!(fmt_location(None, Some(2.35), None), "not set");
        Ok(())
    }

    // --- dew_point_c tests ---

    #[test]
    fn dew_point_known_value() -> TestResult {
        // Given
        let temp_c = 20.0_f32;
        let rh_pct = 50.0_f32;

        // When
        let result = dew_point_c(temp_c, rh_pct);

        // Then — Magnus formula for 20 °C / 50 % RH ≈ 9.3 °C
        assert!(
            (result - 9.3_f32).abs() < 0.3,
            "dew point should be ≈ 9.3 °C, got {result}"
        );
        Ok(())
    }

    #[test]
    fn dew_point_saturated_equals_temp() -> TestResult {
        // Given — saturated air (100 % RH) → dew point equals air temperature
        let temp_c = 15.0_f32;
        let rh_pct = 100.0_f32;

        // When
        let result = dew_point_c(temp_c, rh_pct);

        // Then
        assert!(
            (result - temp_c).abs() < 0.05,
            "at 100 % RH dew point should equal temp (15 °C), got {result}"
        );
        Ok(())
    }

    // --- compass_label_fr tests ---

    #[test]
    fn compass_label_fr_cardinals_and_west_is_o() -> TestResult {
        // Given / When / Then — four cardinals; West is "O" in French
        assert_eq!(compass_label_fr(0.0), "N");
        assert_eq!(compass_label_fr(90.0), "E");
        assert_eq!(compass_label_fr(180.0), "S");
        assert_eq!(compass_label_fr(270.0), "O");
        // Inter-cardinal points from the spec
        assert_eq!(compass_label_fr(202.5), "SSO");
        assert_eq!(compass_label_fr(337.5), "NNO");
        Ok(())
    }

    // --- classify_trend tests ---

    #[test]
    fn classify_trend_bands() -> TestResult {
        // Given / When / Then
        assert_eq!(classify_trend(0.05, 0.1), Trend::Stable);
        assert_eq!(classify_trend(0.3, 0.1), Trend::Rising);
        assert_eq!(classify_trend(-0.3, 0.1), Trend::Falling);
        Ok(())
    }

    // --- fmt_lux / lux_chart_unit tests ---

    #[test]
    fn fmt_lux_switches_unit_at_threshold() -> TestResult {
        // Given / When / Then — at/above 1000 lux reads in klx
        assert_eq!(fmt_lux(Some(3426.0)), "3.4 klx");
        assert_eq!(fmt_lux(Some(1000.0)), "1.0 klx");
        // Below 1000 lux reads in raw lux (klx would round to 0.0)
        assert_eq!(fmt_lux(Some(250.0)), "250 lx");
        assert_eq!(fmt_lux(Some(0.0)), "0 lx");
        assert_eq!(fmt_lux(None), "N/A");
        Ok(())
    }

    #[test]
    fn lux_chart_unit_picks_lx_below_threshold() -> TestResult {
        // Given / When / Then — peak below 1000 lux → raw lux, no scaling
        assert_eq!(lux_chart_unit(800.0), ("lx", 1.0, 0));
        // Peak at/above → kilolux with a 0.001 label scale
        assert_eq!(lux_chart_unit(1000.0), ("klx", 0.001, 1));
        assert_eq!(lux_chart_unit(45_000.0), ("klx", 0.001, 1));
        Ok(())
    }

    // --- fmt_power tests ---

    #[test]
    fn fmt_power_switches_unit_at_one_watt() -> TestResult {
        // Given / When / Then — below 1 W reads in mW with full resolution
        assert_eq!(fmt_power(0.349), "349 mW");
        assert_eq!(fmt_power(0.0), "0 mW");
        // At or above 1 W reads in W with two decimals
        assert_eq!(fmt_power(1.0), "1.00 W");
        assert_eq!(fmt_power(2.345), "2.35 W");
        Ok(())
    }

    // --- power_w tests ---

    #[test]
    fn power_w_multiplies() -> TestResult {
        // Given — 15.0 V, 600 mA → 9.0 W
        let result = power_w(Some(15_000), Some(600));

        // Then
        assert!(
            (result.ok_or("expected Some")? - 9.0).abs() < 1e-9,
            "power should be 9.0 W"
        );
        // None propagates when either input is None
        assert_eq!(power_w(None, Some(600)), None);
        assert_eq!(power_w(Some(15_000), None), None);
        Ok(())
    }

    // --- fmt_battery_flow tests ---

    #[test]
    fn fmt_battery_flow_charging_and_discharging() -> TestResult {
        // Given — solar > load: charging
        let charging = fmt_battery_flow(Some(5.0), Some(2.0), Some(80));

        // Then
        assert!(
            charging.starts_with("▲ en charge"),
            "charging line should start with '▲ en charge', got: {charging}"
        );

        // Given — load > solar: discharging with autonomy
        let discharging = fmt_battery_flow(Some(1.0), Some(3.0), Some(50));

        // Then
        assert!(
            discharging.starts_with("▼ décharge"),
            "discharge line should start with '▼ décharge', got: {discharging}"
        );
        assert!(
            discharging.contains('h'),
            "discharge line should contain autonomy hours, got: {discharging}"
        );
        Ok(())
    }

    // --- fmt_uptime tests ---

    #[test]
    fn fmt_uptime_hours() -> TestResult {
        // Given — 3725 s = 1 h 2 m 5 s → "1h02m"
        // When
        let result = fmt_uptime(3725);

        // Then
        assert_eq!(result, "1h02m");
        Ok(())
    }

    #[test]
    fn fmt_uptime_minutes() -> TestResult {
        // Given — 90 s = 1 m 30 s → "1m30s"
        // When
        let result = fmt_uptime(90);

        // Then
        assert_eq!(result, "1m30s");
        Ok(())
    }

    #[test]
    fn fmt_uptime_seconds_only() -> TestResult {
        // Given — 45 s → "0m45s"
        // When
        let result = fmt_uptime(45);

        // Then
        assert_eq!(result, "0m45s");
        Ok(())
    }
}
// grcov exclude stop
