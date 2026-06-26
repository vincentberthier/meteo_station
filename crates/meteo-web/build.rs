//! Build script for `meteo-web`.
//!
//! Generates `style/_palette.scss` from the canonical `meteo_chart::palette`
//! constants so CSS variables always stay in sync with the Rust palette.
//! There is exactly one colour source: `meteo_chart::palette`.
#![allow(
    clippy::expect_used,
    reason = "build scripts are allowed to abort on configuration errors"
)]
#![allow(
    clippy::print_stdout,
    reason = "cargo directives are written to stdout"
)]
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "build scripts run on the host with std available"
)]

use std::fmt::Write as _;

use meteo_chart::palette::{
    BASE, BLUE, BORDER, CRUST, GREEN, LAVENDER, MANTLE, MAUVE, OVERLAY0, OVERLAY1, OVERLAY2, PEACH,
    RED, SAPPHIRE, SKY, SUBTEXT0, SUBTEXT1, SURFACE0, SURFACE2, TEAL, TEXT, YELLOW, css,
};

/// Canonical 22-colour Catppuccin Mocha palette in the order required by the
/// substep spec. Each entry is `(css_var_name, const_value)`.
const PALETTE: &[(&str, meteo_chart::palette::Rgb)] = &[
    ("base", BASE),
    ("mantle", MANTLE),
    ("crust", CRUST),
    ("border", BORDER),
    ("surface0", SURFACE0),
    ("surface2", SURFACE2),
    ("text", TEXT),
    ("subtext1", SUBTEXT1),
    ("subtext0", SUBTEXT0),
    ("overlay2", OVERLAY2),
    ("overlay1", OVERLAY1),
    ("overlay0", OVERLAY0),
    ("peach", PEACH),
    ("lavender", LAVENDER),
    ("teal", TEAL),
    ("sapphire", SAPPHIRE),
    ("yellow", YELLOW),
    ("blue", BLUE),
    ("sky", SKY),
    ("green", GREEN),
    ("mauve", MAUVE),
    ("red", RED),
];

fn main() {
    // Locate the crate root (directory containing meteo-web/Cargo.toml).
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let palette_path = std::path::Path::new(manifest_dir)
        .join("style")
        .join("_palette.scss");

    let mut scss = String::from(":root {\n");
    for (name, colour) in PALETTE {
        writeln!(scss, "    --{name}: {};", css(*colour)).expect("write to String is infallible");
    }
    scss.push_str("}\n");

    std::fs::write(&palette_path, scss).expect("Failed to write style/_palette.scss");

    // Trigger regeneration when either the build script or the palette source changes.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../meteo-chart/src/palette.rs");
}
