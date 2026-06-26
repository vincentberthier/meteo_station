//! Catppuccin Mocha colour palette and threshold→colour helpers for the dashboard.
//!
//! The canonical RGB values live in [`meteo_chart::palette`]; this module
//! re-exposes them as `ratatui::style::Color::Rgb` constants so the rest of the
//! TUI can keep using `theme::PEACH` etc. unchanged.

use meteo_chart::palette;
use ratatui::style::Color;

/// App background.
pub const BASE: Color = Color::Rgb(palette::BASE.0, palette::BASE.1, palette::BASE.2);
/// Panel fill.
pub const MANTLE: Color = Color::Rgb(palette::MANTLE.0, palette::MANTLE.1, palette::MANTLE.2);
/// Tracks / wells (chart backgrounds).
pub const CRUST: Color = Color::Rgb(palette::CRUST.0, palette::CRUST.1, palette::CRUST.2);
/// Panel outline.
pub const BORDER: Color = Color::Rgb(palette::BORDER.0, palette::BORDER.1, palette::BORDER.2);
/// Chip frame / separators.
pub const SURFACE0: Color = Color::Rgb(
    palette::SURFACE0.0,
    palette::SURFACE0.1,
    palette::SURFACE0.2,
);
/// X-axis / faint strokes.
pub const SURFACE2: Color = Color::Rgb(
    palette::SURFACE2.0,
    palette::SURFACE2.1,
    palette::SURFACE2.2,
);
/// Values / clock.
pub const TEXT: Color = Color::Rgb(palette::TEXT.0, palette::TEXT.1, palette::TEXT.2);
/// Sensor names.
pub const SUBTEXT1: Color = Color::Rgb(
    palette::SUBTEXT1.0,
    palette::SUBTEXT1.1,
    palette::SUBTEXT1.2,
);
/// Panel titles / cardinals.
pub const SUBTEXT0: Color = Color::Rgb(
    palette::SUBTEXT0.0,
    palette::SUBTEXT0.1,
    palette::SUBTEXT0.2,
);
/// Section labels / units / dew point.
pub const OVERLAY2: Color = Color::Rgb(
    palette::OVERLAY2.0,
    palette::OVERLAY2.1,
    palette::OVERLAY2.2,
);
/// Min / max axes / maintenance.
pub const OVERLAY1: Color = Color::Rgb(
    palette::OVERLAY1.0,
    palette::OVERLAY1.1,
    palette::OVERLAY1.2,
);
/// Dimmed labels.
pub const OVERLAY0: Color = Color::Rgb(
    palette::OVERLAY0.0,
    palette::OVERLAY0.1,
    palette::OVERLAY0.2,
);
/// Air temperature.
pub const PEACH: Color = Color::Rgb(palette::PEACH.0, palette::PEACH.1, palette::PEACH.2);
/// Sky temperature.
pub const LAVENDER: Color = Color::Rgb(
    palette::LAVENDER.0,
    palette::LAVENDER.1,
    palette::LAVENDER.2,
);
/// Pressure / battery gauge / gust.
pub const TEAL: Color = Color::Rgb(palette::TEAL.0, palette::TEAL.1, palette::TEAL.2);
/// Humidity.
pub const SAPPHIRE: Color = Color::Rgb(
    palette::SAPPHIRE.0,
    palette::SAPPHIRE.1,
    palette::SAPPHIRE.2,
);
/// Luminosity / solar / warn.
pub const YELLOW: Color = Color::Rgb(palette::YELLOW.0, palette::YELLOW.1, palette::YELLOW.2);
/// Rain.
pub const BLUE: Color = Color::Rgb(palette::BLUE.0, palette::BLUE.1, palette::BLUE.2);
/// Wind / compass.
pub const SKY: Color = Color::Rgb(palette::SKY.0, palette::SKY.1, palette::SKY.2);
/// Battery / charging / OK.
pub const GREEN: Color = Color::Rgb(palette::GREEN.0, palette::GREEN.1, palette::GREEN.2);
/// Load draw.
pub const MAUVE: Color = Color::Rgb(palette::MAUVE.0, palette::MAUVE.1, palette::MAUVE.2);
/// Fault / North marker.
pub const RED: Color = Color::Rgb(palette::RED.0, palette::RED.1, palette::RED.2);

/// Battery state-of-charge → gauge/percent colour.
///
/// - `>=50` → [`GREEN`]
/// - `20..=49` → [`YELLOW`]
/// - `<20` → [`RED`]
#[must_use]
pub const fn battery_color(pct: u8) -> Color {
    if pct >= 50 {
        GREEN
    } else if pct >= 20 {
        YELLOW
    } else {
        RED
    }
}

/// BLE RSSI dBm → chip colour.
///
/// - `>=-70` → [`GREEN`]
/// - `-90..=-71` → [`YELLOW`]
/// - `<-90` → [`RED`]
#[must_use]
pub const fn rssi_color(dbm: i16) -> Color {
    if dbm >= -70 {
        GREEN
    } else if dbm >= -90 {
        YELLOW
    } else {
        RED
    }
}

/// Last-packet age → colour.
///
/// - `<2 s` → [`GREEN`]
/// - `2..=10 s` → [`YELLOW`]
/// - `>10 s` → [`RED`]
#[must_use]
pub fn packet_age_color(age_secs: f64) -> Color {
    if age_secs < 2.0 {
        GREEN
    } else if age_secs <= 10.0 {
        YELLOW
    } else {
        RED
    }
}

/// Linear per-channel blend `fg*a + bg*(1-a)`, `a` clamped to `[0, 1]`.
///
/// Used for the gradient fill (alpha toward [`BASE`]) and the heading-trail fade.
/// Returns [`Color::Rgb`]. Non-`Rgb` inputs fall back to `fg`.
#[must_use]
pub fn blend_rgb(fg: Color, bg: Color, a: f64) -> Color {
    let alpha = a.clamp(0.0, 1.0);
    let (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) = (fg, bg) else {
        return fg;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "blend result is in [0, 255] after clamp"
    )]
    let mix = |f: u8, b: u8| -> u8 {
        f64::from(b)
            .mul_add(1.0 - alpha, f64::from(f) * alpha)
            .round() as u8
    };
    Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
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

    #[test]
    fn battery_color_thresholds() -> TestResult {
        // Given / When / Then
        assert_eq!(battery_color(50), GREEN);
        assert_eq!(battery_color(49), YELLOW);
        assert_eq!(battery_color(20), YELLOW);
        assert_eq!(battery_color(19), RED);
        Ok(())
    }

    #[test]
    fn rssi_color_thresholds() -> TestResult {
        // Given / When / Then
        assert_eq!(rssi_color(-70), GREEN);
        assert_eq!(rssi_color(-81), YELLOW);
        assert_eq!(rssi_color(-91), RED);
        Ok(())
    }

    #[test]
    fn packet_age_color_thresholds() -> TestResult {
        // Given / When / Then
        assert_eq!(packet_age_color(1.9), GREEN);
        assert_eq!(packet_age_color(5.0), YELLOW);
        assert_eq!(packet_age_color(10.5), RED);
        Ok(())
    }

    #[test]
    fn blend_rgb_endpoints_and_midpoint() -> TestResult {
        // Given
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);

        // When / Then — a=1.0 returns fg
        assert_eq!(blend_rgb(black, white, 1.0), black);

        // a=0.0 returns bg
        assert_eq!(blend_rgb(black, white, 0.0), white);

        // a=0.5 midpoint
        assert_eq!(blend_rgb(black, white, 0.5), Color::Rgb(128, 128, 128));
        Ok(())
    }

    #[test]
    fn blend_rgb_clamps_out_of_range() -> TestResult {
        // Given
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);

        // When — a=2.0 should clamp to 1.0 and return fg (black)
        let result = blend_rgb(black, white, 2.0);

        // Then
        assert_eq!(result, black);
        Ok(())
    }
}
// grcov exclude stop
