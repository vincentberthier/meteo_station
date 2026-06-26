//! Integration test: verifies the palette CSS generator and the generated
//! `style/_palette.scss` file produced by `build.rs`.
//!
//! Run with: `cargo nextest run -p meteo-web --no-default-features --features ssr`
// grcov exclude start
#![expect(
    clippy::panic_in_result_fn,
    reason = "test module — assert! panics are expected"
)]
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "integration tests run on the host with std available"
)]

use core::{error, result};

use meteo_chart::palette;
use test_log::test;

type TestResult = result::Result<(), Box<dyn error::Error>>;

/// The `css()` helper must produce canonical lowercase hex for BASE and RED.
#[test]
fn css_base_colour() -> TestResult {
    // Given / When / Then
    assert_eq!(palette::css(palette::BASE), "#1e1e2e");
    Ok(())
}

#[test]
fn css_red_colour() -> TestResult {
    // Given / When / Then
    assert_eq!(palette::css(palette::RED), "#f38ba8");
    Ok(())
}

/// The generated `_palette.scss` must contain a CSS custom property declaration
/// for each of the 22 canonical Catppuccin Mocha colours.
#[test]
fn generated_palette_scss_contains_all_vars() -> TestResult {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let scss_path = std::path::Path::new(manifest_dir).join("style/_palette.scss");
    let scss = std::fs::read_to_string(&scss_path)
        .map_err(|e| format!("Cannot read {}: {e}", scss_path.display()))?;

    // Canonical 22-variable set — must all be present.
    let expected_vars = [
        "--base:",
        "--mantle:",
        "--crust:",
        "--border:",
        "--surface0:",
        "--surface2:",
        "--text:",
        "--subtext1:",
        "--subtext0:",
        "--overlay2:",
        "--overlay1:",
        "--overlay0:",
        "--peach:",
        "--lavender:",
        "--teal:",
        "--sapphire:",
        "--yellow:",
        "--blue:",
        "--sky:",
        "--green:",
        "--mauve:",
        "--red:",
    ];

    for var in &expected_vars {
        assert!(
            scss.contains(var),
            "Missing CSS variable {var} in _palette.scss"
        );
    }
    Ok(())
}
// grcov exclude stop
