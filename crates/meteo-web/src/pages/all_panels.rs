//! All-panels page — live band + time-select + historic chart grid.
//!
//! Route `/`. Renders:
//! - `LiveBand`: instantaneous SSE-driven telemetry.
//! - `TimeSelect`: preset and custom range buttons that mutate `window`.
//! - CAPTEURS chart group: air temp, pressure, humidity, sky temp, lux, wind+gust.
//! - ÉNERGIE chart group: solar, battery, load.
//!
//! Pan/zoom are exposed as button controls (`◀ ◀◀ ▶▶ ▶` and `− +`) that call
//! the pure `pan_by`/`zoom_about` helpers and `set` the `window` signal, which
//! re-fires the `Resource` (no debounce sleep — driven by signal change).

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
// i64 → f64 precision: unix timestamps fit well within f64 mantissa resolution
// for chart x-axis purposes (sub-second precision is irrelevant here).
#![allow(
    clippy::cast_precision_loss,
    reason = "unix timestamps as f64 for chart math — sub-second precision irrelevant"
)]

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meteo_chart::palette::{GREEN, LAVENDER, MAUVE, PEACH, SAPPHIRE, SKY, TEAL, YELLOW, css};

use crate::{
    api::get_history,
    components::{
        ChartSeries, LiveBand, PlotPanel, TimeSelect,
        time_select::{TimeWindow, pan_by, preset_day, zoom_about},
    },
    types::HistoryRow,
};

/// All-panels dashboard page (route `/`).
///
/// A `RwSignal<TimeWindow>` (default = last 24 h) drives a `Resource` that
/// fetches `get_history`. The resource re-fires whenever the signal changes
/// (preset click, custom range apply, pan/zoom button).
#[component]
pub fn AllPanelsPage() -> impl IntoView {
    // Default: last 24 h — chrono clock feature enabled for both ssr and wasm32
    let now_ts = chrono::Utc::now().timestamp();
    let window: RwSignal<TimeWindow> = RwSignal::new(preset_day(now_ts));

    // History resource — re-fetched on every window change.
    let history = Resource::new(
        move || window.get(),
        move |w| async move {
            get_history(w.from_ts, w.to_ts, w.bucket_secs())
                .await
                .unwrap_or_default()
        },
    );

    // ── Pan / zoom button handlers ─────────────────────────────────────────
    let zoom_in = move |_| window.update(|w| *w = zoom_about(*w, 0.5, 0.5));
    let zoom_out = move |_| window.update(|w| *w = zoom_about(*w, 0.5, 2.0));
    let pan_back = move |_| window.update(|w| *w = pan_by(*w, -0.25));
    let pan_back_big = move |_| window.update(|w| *w = pan_by(*w, -1.0));
    let pan_fwd = move |_| window.update(|w| *w = pan_by(*w, 0.25));
    let pan_fwd_big = move |_| window.update(|w| *w = pan_by(*w, 1.0));

    // Return AnyView to erase the concrete view-tuple type before the leptos
    // router's `.into_any()` call sees it — prevents the compiler's query-depth
    // overflow that occurs when tachys computes the layout of the full type.
    view! {
        <div class="content-area">
            <LiveBand />
            <div class="time-controls">
                <TimeSelect window=window />
                <div class="pan-zoom-controls">
                    <button class="pz-btn" on:click=pan_back_big title="Reculer d'une période">"◀◀"</button>
                    <button class="pz-btn" on:click=pan_back title="Reculer d'¼ période">"◀"</button>
                    <button class="pz-btn" on:click=zoom_in title="Zoom avant">"-"</button>
                    <button class="pz-btn" on:click=zoom_out title="Zoom arrière">"+"</button>
                    <button class="pz-btn" on:click=pan_fwd title="Avancer d'¼ période">"▶"</button>
                    <button class="pz-btn" on:click=pan_fwd_big title="Avancer d'une période">"▶▶"</button>
                </div>
            </div>

            <Suspense fallback=move || view! { <p class="color-subtext">"Chargement…"</p> }>
                {move || {
                    history.get().map(|rows| {
                        view! { <HistoryGrid rows=rows /> }.into_any()
                    })
                }}
            </Suspense>
        </div>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Chart grid — extracted so Suspense can wrap it cleanly
// ---------------------------------------------------------------------------

/// `(avg points, optional min–max band)` returned by [`series_from_rows`].
type SeriesAndBand = (Vec<(f64, f64)>, Option<Vec<(f64, f64, f64)>>);

/// Build `(points, band)` from a `HistoryRow` slice using a metric extractor.
fn series_from_rows(
    rows: &[HistoryRow],
    avg_fn: impl Fn(&HistoryRow) -> Option<f64>,
    min_fn: impl Fn(&HistoryRow) -> Option<f64>,
    max_fn: impl Fn(&HistoryRow) -> Option<f64>,
) -> SeriesAndBand {
    let points: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|r| avg_fn(r).map(|v| (r.ts as f64, v)))
        .collect();

    let band: Vec<(f64, f64, f64)> = rows
        .iter()
        .filter_map(|r| {
            let lo = min_fn(r)?;
            let hi = max_fn(r)?;
            Some((r.ts as f64, lo, hi))
        })
        .collect();

    let band = (band.len() >= 2).then_some(band);
    (points, band)
}

/// Build one `AnyView`-erased `PlotPanel` — avoids monomorphizing the same
/// 9-tuple type tree inline, which overflows the compiler's query-depth limit.
fn panel(title: &str, unit: &str, series: ChartSeries) -> AnyView {
    view! {
        <PlotPanel
            title=title.to_owned()
            unit=unit.to_owned()
            series=series
        />
    }
    .into_any()
}

/// Grid of history charts for all sensor metrics.
///
/// Panels are built as `Vec<AnyView>` so the per-panel type is erased before
/// being placed into the containing `<div>`, preventing the monomorphized view
/// type from overflowing the compiler's query-depth limit.
#[component]
fn HistoryGrid(rows: Vec<HistoryRow>) -> impl IntoView {
    // ── CAPTEURS ──────────────────────────────────────────────────────────

    let (temp_pts, temp_band) =
        series_from_rows(&rows, |r| r.temp.avg, |r| r.temp.min, |r| r.temp.max);
    let (pres_pts, pres_band) = series_from_rows(
        &rows,
        |r| r.pressure.avg,
        |r| r.pressure.min,
        |r| r.pressure.max,
    );
    let (hum_pts, hum_band) = series_from_rows(
        &rows,
        |r| r.humidity.avg,
        |r| r.humidity.min,
        |r| r.humidity.max,
    );
    let (sky_pts, sky_band) = series_from_rows(&rows, |r| r.sky.avg, |r| r.sky.min, |r| r.sky.max);
    let (lux_pts, _) = series_from_rows(&rows, |r| r.lux.avg, |r| r.lux.min, |r| r.lux.max);
    let (wind_pts, wind_band) =
        series_from_rows(&rows, |r| r.wind.avg, |r| r.wind.min, |r| r.wind.max);

    let capteurs: Vec<AnyView> = vec![
        panel(
            "Température de l'air",
            "°C",
            ChartSeries {
                points: temp_pts,
                band: temp_band,
                color_hex: css(PEACH),
                floor: None,
                prec: 1,
            },
        ),
        panel(
            "Pression",
            "hPa",
            ChartSeries {
                points: pres_pts,
                band: pres_band,
                color_hex: css(TEAL),
                floor: None,
                prec: 1,
            },
        ),
        panel(
            "Humidité",
            "%",
            ChartSeries {
                points: hum_pts,
                band: hum_band,
                color_hex: css(SAPPHIRE),
                floor: Some(0.0),
                prec: 0,
            },
        ),
        panel(
            "Température du ciel",
            "°C",
            ChartSeries {
                points: sky_pts,
                band: sky_band,
                color_hex: css(LAVENDER),
                floor: None,
                prec: 1,
            },
        ),
        panel(
            "Luminosité",
            "lx",
            ChartSeries {
                points: lux_pts,
                band: None,
                color_hex: css(YELLOW),
                floor: Some(0.0),
                prec: 0,
            },
        ),
        panel(
            "Vitesse du vent",
            "m/s",
            ChartSeries {
                points: wind_pts,
                band: wind_band,
                color_hex: css(SKY),
                floor: Some(0.0),
                prec: 1,
            },
        ),
    ];

    // ── ÉNERGIE ───────────────────────────────────────────────────────────

    let solar_pts: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|r| r.solar_w_avg.map(|v| (r.ts as f64, v)))
        .collect();
    let batt_pts: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|r| r.battery_avg.map(|v| (r.ts as f64, v)))
        .collect();
    let load_pts: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|r| r.load_w_avg.map(|v| (r.ts as f64, v)))
        .collect();

    let energie: Vec<AnyView> = vec![
        panel(
            "Solaire",
            "W",
            ChartSeries {
                points: solar_pts,
                band: None,
                color_hex: css(YELLOW),
                floor: Some(0.0),
                prec: 2,
            },
        ),
        panel(
            "Batterie",
            "%",
            ChartSeries {
                points: batt_pts,
                band: None,
                color_hex: css(GREEN),
                floor: Some(0.0),
                prec: 0,
            },
        ),
        panel(
            "Charge",
            "W",
            ChartSeries {
                points: load_pts,
                band: None,
                color_hex: css(MAUVE),
                floor: Some(0.0),
                prec: 2,
            },
        ),
    ];

    view! {
        <div class="history-grid">
            <h2 class="chart-group-label font-mono">"CAPTEURS"</h2>
            <div class="chart-group">{capteurs}</div>
            <h2 class="chart-group-label font-mono">"ÉNERGIE"</h2>
            <div class="chart-group">{energie}</div>
        </div>
    }
}
