//! Catppuccin Mocha colour palette and threshold→colour helpers for the dashboard.

use ratatui::style::Color;

/// App background.
pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
/// Panel fill.
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
/// Tracks / wells (chart backgrounds).
pub const CRUST: Color = Color::Rgb(0x11, 0x11, 0x1b);
/// Panel outline.
pub const BORDER: Color = Color::Rgb(0x2a, 0x2a, 0x3c);
/// Chip frame / separators.
pub const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
/// X-axis / faint strokes.
pub const SURFACE2: Color = Color::Rgb(0x58, 0x5b, 0x70);
/// Values / clock.
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
/// Sensor names.
pub const SUBTEXT1: Color = Color::Rgb(0xba, 0xc2, 0xde);
/// Panel titles / cardinals.
pub const SUBTEXT0: Color = Color::Rgb(0xa6, 0xad, 0xc8);
/// Section labels / units / dew point.
pub const OVERLAY2: Color = Color::Rgb(0x93, 0x99, 0xb2);
/// Min / max axes / maintenance.
pub const OVERLAY1: Color = Color::Rgb(0x7f, 0x84, 0x9c);
/// Dimmed labels.
pub const OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
/// Air temperature.
pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
/// Sky temperature.
pub const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
/// Pressure / battery gauge / gust.
pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
/// Humidity.
pub const SAPPHIRE: Color = Color::Rgb(0x74, 0xc7, 0xec);
/// Luminosity / solar / warn.
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
/// Rain.
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
/// Wind / compass.
pub const SKY: Color = Color::Rgb(0x89, 0xdc, 0xeb);
/// Battery / charging / OK.
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
/// Load draw.
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
/// Fault / North marker.
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

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
