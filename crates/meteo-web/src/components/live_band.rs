//! Live telemetry band — reads the shared SSE-driven frame context.
//!
//! `LiveBand` is a pure read-side component. It calls
//! [`expect_context::<RwSignal<Option<LiveFrame>>>`] to obtain the frame
//! signal that `App` provides and drives with the `/live` SSE stream.
//!
//! Under `ssr`, the component renders a static placeholder shell with all
//! fields showing `"N/A"`; hydration wires the reactive updates in the browser.

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
// The LiveBand component function renders all the live fields in one pass.
// Splitting it would obscure the semantic grouping without any clarity gain.
#![allow(
    clippy::too_many_lines,
    reason = "LiveBand renders all live fields in one view! block — splitting would obscure structure"
)]

use leptos::prelude::*;
use meteo_chart::{fmt_battery_flow, fmt_power, palette};

use crate::{components::WindCompass, types::LiveFrame};

/// Live instantaneous telemetry band.
///
/// Displays:
/// - Air temperature (colour: Peach).
/// - Wind compass (`WindCompass`) wired to the live frame's direction + speed.
/// - Power row: Solaire / Charge / Batterie via `fmt_power` / `fmt_battery_flow`.
///
/// Data arrives via the shared `RwSignal<Option<LiveFrame>>` context that
/// `App` provides. Under `hydrate` the signal is driven by the `/live` SSE
/// endpoint (one JSON `LiveFrame` per second). Under `ssr` the static shell
/// is emitted with all fields showing `"N/A"`.
#[component]
pub fn LiveBand() -> impl IntoView {
    // Read the shared live-frame context provided by App.
    let frame = expect_context::<RwSignal<Option<LiveFrame>>>();

    // ── Derived signals ─────────────────────────────────────────────────────
    let dir_deg: Signal<Option<f32>> = Signal::derive(move || {
        let f = frame.get()?;
        f.wind_dir_deg
    });
    let speed_ms: Signal<Option<f32>> = Signal::derive(move || {
        let f = frame.get()?;
        f.wind_speed_ms
    });

    let temp_label = move || {
        frame
            .get()
            .and_then(|f| f.temperature_c)
            .map_or_else(|| "N/A".to_owned(), |t| format!("{t:.1} °C"))
    };

    let solar_label = move || {
        frame
            .get()
            .and_then(|f| f.solar_w)
            .map_or_else(|| "N/A".to_owned(), fmt_power)
    };

    let load_label = move || {
        frame
            .get()
            .and_then(|f| f.load_w)
            .map_or_else(|| "N/A".to_owned(), fmt_power)
    };

    let battery_label = move || {
        frame
            .get()
            .and_then(|f| f.battery_pct)
            .map_or_else(|| "N/A".to_owned(), |p| format!("{p} %"))
    };

    let flow_label = move || {
        let f = frame.get();
        let solar = f.as_ref().and_then(|fr| fr.solar_w);
        let load = f.as_ref().and_then(|fr| fr.load_w);
        let pct = f.as_ref().and_then(|fr| fr.battery_pct);
        fmt_battery_flow(solar, load, pct)
    };

    let peach = palette::css(palette::PEACH);
    let yellow = palette::css(palette::YELLOW);
    let mauve = palette::css(palette::MAUVE);
    let green = palette::css(palette::GREEN);

    view! {
        <div class="live-band">
            <span class="live-label">"En direct"</span>

            // ── Air temperature ────────────────────────────────────────────
            <div class="live-cell">
                <span class="live-cell-label">"Température"</span>
                <span class="live-cell-value font-mono" style:color=peach>
                    {temp_label}
                </span>
            </div>

            // ── Wind compass ───────────────────────────────────────────────
            <div class="live-cell live-cell-compass">
                <span class="live-cell-label">"Vent"</span>
                <WindCompass dir_deg=dir_deg speed_ms=speed_ms />
            </div>

            // ── Power ──────────────────────────────────────────────────────
            <div class="live-cell">
                <span class="live-cell-label">"Solaire"</span>
                <span class="live-cell-value font-mono" style:color=yellow.clone()>
                    {solar_label}
                </span>
            </div>
            <div class="live-cell">
                <span class="live-cell-label">"Charge"</span>
                <span class="live-cell-value font-mono" style:color=mauve>
                    {load_label}
                </span>
            </div>
            <div class="live-cell">
                <span class="live-cell-label">"Batterie"</span>
                <span class="live-cell-value font-mono" style:color=green>
                    {battery_label}
                </span>
            </div>
            <div class="live-cell live-cell-wide">
                <span class="live-cell-value font-mono" style:color=yellow>
                    {flow_label}
                </span>
            </div>
        </div>
    }
}
