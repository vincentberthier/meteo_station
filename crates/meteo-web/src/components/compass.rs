//! Wind compass component — layered dial + rotated needle SVG with live readout.
//!
//! `WindCompass` stacks two absolute-positioned SVG images (a static dial and a
//! rotating needle) inside a relative-positioned wrapper, then overlays a live
//! readout: wind speed, numeric heading, the 16-point French rose label, and an
//! optional gust line.

// The leptos #[component] macro generates a typed-builder struct whose field names
// shadow the function parameters.  Neither shadow is actionable from user code.
#![allow(
    clippy::shadow_reuse,
    reason = "leptos #[component] macro generates param shadows in the builder"
)]
// Component props are owned values consumed at call-site.  Leptos does not support
// borrowed props, so the pass-by-value is intentional even when the body only borrows.
#![allow(
    clippy::needless_pass_by_value,
    reason = "leptos component props must be owned"
)]

use leptos::prelude::*;
use meteo_chart::compass_label_fr;

/// Wind compass widget.
///
/// Renders two stacked, absolute-positioned images sharing the same centre:
/// - A static dial (`/compass/compass-dial.svg`).
/// - A needle (`/compass/compass-needle.svg`) rotated clockwise by `dir_deg`
///   degrees (the SVG is authored North-up; 0° = no rotation).
///
/// An overlay shows:
/// - Wind speed (e.g. `"3.2 m/s"`) or `"--"` when no signal.
/// - Numeric heading (`"270°"`) and the 16-point French rose label (`"O"`).
/// - An optional « Rafale » gust line when `gust_ms` is `Some`.
#[component]
pub fn WindCompass(
    /// Wind direction in degrees (0° = N, clockwise). `None` = no signal.
    dir_deg: Signal<Option<f32>>,
    /// Wind speed in m/s. `None` = no signal.
    speed_ms: Signal<Option<f32>>,
    /// Optional peak gust speed in m/s — shown as a « Rafale » line when present.
    #[prop(optional)]
    gust_ms: Option<f32>,
) -> impl IntoView {
    let speed_label = move || {
        speed_ms
            .get()
            .map_or_else(|| "--".to_owned(), |s| format!("{s:.1} m/s"))
    };

    let heading_label = move || {
        dir_deg.get().map_or_else(
            || "--°".to_owned(),
            |d| format!("{:.0}°", d.rem_euclid(360.0)),
        )
    };

    let rose_label = move || dir_deg.get().map_or("--", |d| compass_label_fr(d));

    let needle_transform = move || format!("rotate({}deg)", dir_deg.get().unwrap_or(0.0));

    view! {
        <div class="compass-wrapper">
            <div class="compass-layers">
                <img
                    class="compass-dial"
                    src="/compass/compass-dial.svg"
                    alt="Cadran de boussole"
                />
                <img
                    class="compass-needle"
                    src="/compass/compass-needle.svg"
                    alt="Aiguille"
                    style:transform=needle_transform
                />
            </div>
            <div class="compass-overlay">
                <span class="compass-speed">{speed_label}</span>
                <span class="compass-heading">
                    {heading_label}
                    " "
                    {rose_label}
                </span>
                {gust_ms.map(|g| view! {
                    <span class="compass-gust">
                        "Rafale : "
                        {format!("{g:.1} m/s")}
                    </span>
                })}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "TestResult is the standard test pattern"
)]
#[cfg(all(test, feature = "ssr"))]
mod tests {
    use core::{error, result};

    use leptos::prelude::*;
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// `WindCompass` rotates the needle by `dir_deg`.
    ///
    /// With `dir = Some(90.0)` the rendered style must contain `rotate(90`.
    #[test]
    fn compass_renders_rotation_transform() -> TestResult {
        // Given — run inside a reactive Owner so Signal::stored can allocate
        let html = Owner::new().with(|| {
            let dir = Signal::stored(Some(90.0_f32));
            let speed = Signal::stored(Some(5.2_f32));

            // When
            view! {
                <WindCompass dir_deg=dir speed_ms=speed />
            }
            .to_html()
        });

        // Then — needle image must carry rotate(90...) in its inline style
        assert!(
            html.contains("rotate(90"),
            "expected `rotate(90...` in the rendered transform, got:\n{html}"
        );
        Ok(())
    }

    /// `WindCompass` with `dir = None` defaults to 0° rotation without panicking.
    #[test]
    fn compass_handles_none_dir() -> TestResult {
        // Given — run inside a reactive Owner so Signal::stored can allocate
        let html = Owner::new().with(|| {
            let dir = Signal::stored(None::<f32>);
            let speed = Signal::stored(None::<f32>);

            // When
            view! {
                <WindCompass dir_deg=dir speed_ms=speed />
            }
            .to_html()
        });

        // Then — defaults to 0° rotation and shows placeholder text
        assert!(
            html.contains("rotate(0"),
            "expected `rotate(0...` for None direction, got:\n{html}"
        );
        assert!(
            html.contains("--"),
            "expected placeholder '--' for None values"
        );
        Ok(())
    }
}
// grcov exclude stop
