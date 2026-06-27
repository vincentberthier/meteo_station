//! Live telemetry band — reads the shared SSE-driven frame context.
//!
//! `LiveBand` is a pure read-side component. It calls
//! [`expect_context::<RwSignal<Option<LiveFrame>>>`] to obtain the frame
//! signal that `App` provides and drives with the `/live` SSE stream.
//!
//! The band mirrors the TUI summary (`crates/meteo-tui/src/ui/summary.rs`): a
//! three-card row — **ATMOSPHÈRE** (air/humidity/pressure/sky/lux/rain/dew),
//! **VENT** (wind compass), **ÉNERGIE** (solar/battery+gauge/flow/load). Each
//! value is a reactive closure; absent fields render `"N/A"`.

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
use meteo_chart::{dew_point_c, fmt_battery_flow, fmt_lux, fmt_power, palette};

use crate::{components::WindCompass, types::LiveFrame};

/// One label/value row inside the ATMOSPHÈRE or ÉNERGIE card.
///
/// `value` is a reactive closure so the row updates on each SSE frame; `color`
/// is the value's CSS colour (a `#rrggbb` string from `meteo_chart::palette`).
fn data_row(
    label: &'static str,
    value: impl Fn() -> String + Send + Sync + 'static,
    color: String,
) -> impl IntoView {
    view! {
        <div class="data-row">
            <span class="data-label">{label}</span>
            <span class="data-value font-mono" style:color=color>{value}</span>
        </div>
    }
}

/// Live instantaneous telemetry band — three cards (ATMOSPHÈRE · VENT · ÉNERGIE).
///
/// Data arrives via the shared `RwSignal<Option<LiveFrame>>` context that `App`
/// provides. Under `hydrate` the signal is driven by the `/live` SSE endpoint
/// (one JSON `LiveFrame` per second). Under `ssr` the static shell is emitted
/// with all fields showing `"N/A"`.
#[component]
pub fn LiveBand() -> impl IntoView {
    // Read the shared live-frame context provided by App.
    let frame = expect_context::<RwSignal<Option<LiveFrame>>>();

    // ── Wind (VENT) ──────────────────────────────────────────────────────────
    let dir_deg: Signal<Option<f32>> = Signal::derive(move || frame.get()?.wind_dir_deg);
    let speed_ms: Signal<Option<f32>> = Signal::derive(move || frame.get()?.wind_speed_ms);

    // ── ATMOSPHÈRE values ─────────────────────────────────────────────────────
    let air_label = move || {
        frame
            .get()
            .and_then(|f| f.temperature_c)
            .map_or_else(|| "N/A".to_owned(), |t| format!("{t:.1} °C"))
    };
    let hum_label = move || {
        frame
            .get()
            .and_then(|f| f.humidity_pct)
            .map_or_else(|| "N/A".to_owned(), |h| format!("{h:.0} %HR"))
    };
    let press_label = move || {
        frame
            .get()
            .and_then(|f| f.pressure_hpa)
            .map_or_else(|| "N/A".to_owned(), |p| format!("{p:.1} hPa"))
    };
    let sky_label = move || {
        frame
            .get()
            .and_then(|f| f.sky_temp_c)
            .map_or_else(|| "N/A".to_owned(), |s| format!("{s:.1} °C"))
    };
    let lux_label = move || fmt_lux(frame.get().and_then(|f| f.luminosity_lux));
    let rain_label = move || {
        frame
            .get()
            .and_then(|f| f.rain_rate_mm_h)
            .map_or_else(|| "N/A".to_owned(), |r| format!("{r:.1} mm/h"))
    };
    let dew_label = move || {
        let f = frame.get();
        match (
            f.as_ref().and_then(|x| x.temperature_c),
            f.as_ref().and_then(|x| x.humidity_pct),
        ) {
            (Some(t), Some(h)) => format!("{:.1} °C", dew_point_c(t, h)),
            _ => "N/A".to_owned(),
        }
    };

    // ── ÉNERGIE values ─────────────────────────────────────────────────────────
    let solar_label = move || {
        frame
            .get()
            .and_then(|f| f.solar_w)
            .map_or_else(|| "N/A".to_owned(), fmt_power)
    };
    let solar_sub = move || {
        let f = frame.get();
        match (
            f.as_ref().and_then(|x| x.solar_mv),
            f.as_ref().and_then(|x| x.solar_ma),
        ) {
            (Some(mv), Some(ma)) => format!("{:.2} V · {ma} mA", f64::from(mv) / 1000.0),
            _ => "N/A".to_owned(),
        }
    };
    let load_label = move || {
        frame
            .get()
            .and_then(|f| f.load_w)
            .map_or_else(|| "N/A".to_owned(), fmt_power)
    };
    let load_sub = move || {
        frame
            .get()
            .and_then(|f| f.load_ma)
            .map_or_else(|| "N/A".to_owned(), |ma| format!("{ma} mA"))
    };
    let battery_label = move || {
        frame
            .get()
            .and_then(|f| f.battery_pct)
            .map_or_else(|| "N/A".to_owned(), |p| format!("{p} %"))
    };
    let flow_label = move || {
        let f = frame.get();
        fmt_battery_flow(
            f.as_ref().and_then(|fr| fr.solar_w),
            f.as_ref().and_then(|fr| fr.load_w),
            f.as_ref().and_then(|fr| fr.battery_pct),
        )
    };
    // Colour the flow line like the TUI: ▲ charge → green, ▼ discharge → red,
    // anything else (stable / N/A) → neutral overlay.
    let flow_color = move || {
        let s = flow_label();
        palette::css(if s.starts_with('\u{25b2}') {
            palette::GREEN
        } else if s.starts_with('\u{25bc}') {
            palette::RED
        } else {
            palette::OVERLAY1
        })
    };

    // Battery gauge: fill width tracks percent; colour tiers like the TUI.
    let batt_pct = move || frame.get()?.battery_pct;
    let batt_fill = move || format!("{}%", batt_pct().unwrap_or(0));
    let batt_color = move || {
        palette::css(match batt_pct() {
            Some(p) if p >= 50 => palette::GREEN,
            Some(p) if p >= 20 => palette::YELLOW,
            Some(_) => palette::RED,
            None => palette::OVERLAY2,
        })
    };

    // Static colours (one per metric; matches the TUI summary palette).
    let peach = palette::css(palette::PEACH);
    let sapphire = palette::css(palette::SAPPHIRE);
    let teal = palette::css(palette::TEAL);
    let lavender = palette::css(palette::LAVENDER);
    let yellow = palette::css(palette::YELLOW);
    let blue = palette::css(palette::BLUE);
    let overlay = palette::css(palette::OVERLAY2);
    let mauve = palette::css(palette::MAUVE);

    view! {
        <div class="live-band">
            // ── ATMOSPHÈRE ────────────────────────────────────────────────────
            <section class="live-card live-card-atmo">
                <h3 class="live-card-title font-mono">"ATMOSPHÈRE"</h3>
                {data_row("Air", air_label, peach)}
                {data_row("Humidité", hum_label, sapphire)}
                {data_row("Pression", press_label, teal)}
                {data_row("Temp. ciel", sky_label, lavender)}
                {data_row("Luminosité", lux_label, yellow.clone())}
                {data_row("Pluie", rain_label, blue)}
                {data_row("Pt rosée", dew_label, overlay)}
            </section>

            // ── VENT ──────────────────────────────────────────────────────────
            <section class="live-card live-card-vent">
                <h3 class="live-card-title font-mono">"VENT"</h3>
                <WindCompass dir_deg=dir_deg speed_ms=speed_ms />
            </section>

            // ── ÉNERGIE ───────────────────────────────────────────────────────
            <section class="live-card live-card-ener">
                <h3 class="live-card-title font-mono">"ÉNERGIE"</h3>
                {data_row("Solaire", solar_label, yellow)}
                <div class="data-sub font-mono">{solar_sub}</div>

                <div class="data-row">
                    <span class="data-label">"Batterie"</span>
                    <span class="data-value font-mono" style:color=batt_color>{battery_label}</span>
                </div>
                <div class="batt-gauge">
                    <div class="batt-gauge-fill" style:width=batt_fill style:background-color=batt_color></div>
                </div>
                <div class="data-flow font-mono" style:color=flow_color>{flow_label}</div>

                {data_row("Charge", load_label, mauve)}
                <div class="data-sub font-mono">{load_sub}</div>
            </section>
        </div>
    }
}
