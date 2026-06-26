//! Catppuccin Mocha palette — canonical RGB + hex, shared by TUI and web.
//!
//! These are the single source of truth for all 22 named colours used in the
//! dashboard. The TUI derives its `ratatui::style::Color::Rgb` values from these
//! constants; the web CSS derives its `#rrggbb` values via [`css`].

/// An sRGB colour as three byte channels (R, G, B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// App background.
pub const BASE: Rgb = Rgb(0x1e, 0x1e, 0x2e);
/// Panel fill.
pub const MANTLE: Rgb = Rgb(0x18, 0x18, 0x25);
/// Tracks / wells (chart backgrounds).
pub const CRUST: Rgb = Rgb(0x11, 0x11, 0x1b);
/// Panel outline.
pub const BORDER: Rgb = Rgb(0x2a, 0x2a, 0x3c);
/// Chip frame / separators.
pub const SURFACE0: Rgb = Rgb(0x31, 0x32, 0x44);
/// X-axis / faint strokes.
pub const SURFACE2: Rgb = Rgb(0x58, 0x5b, 0x70);
/// Values / clock.
pub const TEXT: Rgb = Rgb(0xcd, 0xd6, 0xf4);
/// Sensor names.
pub const SUBTEXT1: Rgb = Rgb(0xba, 0xc2, 0xde);
/// Panel titles / cardinals.
pub const SUBTEXT0: Rgb = Rgb(0xa6, 0xad, 0xc8);
/// Section labels / units / dew point.
pub const OVERLAY2: Rgb = Rgb(0x93, 0x99, 0xb2);
/// Min / max axes / maintenance.
pub const OVERLAY1: Rgb = Rgb(0x7f, 0x84, 0x9c);
/// Dimmed labels.
pub const OVERLAY0: Rgb = Rgb(0x6c, 0x70, 0x86);
/// Air temperature.
pub const PEACH: Rgb = Rgb(0xfa, 0xb3, 0x87);
/// Sky temperature.
pub const LAVENDER: Rgb = Rgb(0xb4, 0xbe, 0xfe);
/// Pressure / battery gauge / gust.
pub const TEAL: Rgb = Rgb(0x94, 0xe2, 0xd5);
/// Humidity.
pub const SAPPHIRE: Rgb = Rgb(0x74, 0xc7, 0xec);
/// Luminosity / solar / warn.
pub const YELLOW: Rgb = Rgb(0xf9, 0xe2, 0xaf);
/// Rain.
pub const BLUE: Rgb = Rgb(0x89, 0xb4, 0xfa);
/// Wind / compass.
pub const SKY: Rgb = Rgb(0x89, 0xdc, 0xeb);
/// Battery / charging / OK.
pub const GREEN: Rgb = Rgb(0xa6, 0xe3, 0xa1);
/// Load draw.
pub const MAUVE: Rgb = Rgb(0xcb, 0xa6, 0xf7);
/// Fault / North marker.
pub const RED: Rgb = Rgb(0xf3, 0x8b, 0xa8);

/// Lowercase `#rrggbb` string (for CSS / SVG attributes).
#[must_use]
pub fn css(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
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
    fn css_formats_hex() -> TestResult {
        // Given / When / Then
        assert_eq!(css(RED), "#f38ba8");
        assert_eq!(css(BASE), "#1e1e2e");
        assert_eq!(css(GREEN), "#a6e3a1");
        Ok(())
    }

    #[test]
    fn rgb_equality() -> TestResult {
        // Given / When / Then — same value is equal, different is not
        assert_eq!(PEACH, Rgb(0xfa, 0xb3, 0x87));
        assert_ne!(PEACH, RED);
        Ok(())
    }
}
// grcov exclude stop
