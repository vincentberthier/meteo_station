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
//!
//! ## Live-in-place updates
//!
//! `HistoryGrid` is mounted **once**.  All panel SVGs derive exclusively from
//! the minute-bucketed `rows` signal (avg line + min–max band across the full
//! width), which the `Resource` fills on each fetch.  When `following` is
//! `true`, a browser-side interval (~30 s) advances the `window` to track
//! "now", triggering a history refetch that patches `rows` in place — no grid
//! remount and no scroll-reset.  Raw 1 Hz live frames are NOT appended to the
//! charts; they feed only the `LiveBand` and signal-state indicator.

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
// Timestamp arithmetic: all values are well within i64 range for any realistic
// unix timestamp; overflow is not a concern for calendar-scale windows.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "unix-timestamp arithmetic cannot overflow within any realistic calendar range"
)]

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meteo_chart::palette::{BLUE, GREEN, LAVENDER, MAUVE, PEACH, SAPPHIRE, SKY, YELLOW, css};

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
/// (preset click, custom range apply, pan/zoom button, or the 30 s follow-now
/// interval).
///
/// `following` tracks whether the window should track "now":
/// - preset buttons set `following = true` (resume live tracking)
/// - pan / zoom / custom-range "Appliquer" set `following = false` (history exploration)
///
/// When `following` is `true`, a browser-side interval (~30 s) advances the
/// `window` to track "now" and refetches history into `rows`.  The mounted
/// `HistoryGrid` patches its SVGs in place — no remount.
#[component]
pub fn AllPanelsPage() -> impl IntoView {
    // Default: last 24 h — chrono clock feature enabled for both ssr and wasm32
    let now_ts = chrono::Utc::now().timestamp();
    let window: RwSignal<TimeWindow> = RwSignal::new(preset_day(now_ts));

    // Whether the window is tracking "now".
    let following: RwSignal<bool> = RwSignal::new(true);

    // History resource — re-fetched only when `window` changes (explicit user
    // action or the 30 s follow-now interval).
    let history = Resource::new(
        move || window.get(),
        move |w| async move {
            get_history(w.from_ts, w.to_ts, w.bucket_secs())
                .await
                .unwrap_or_default()
        },
    );

    // Stable signal for the grid — updated whenever `history` resolves.
    // The grid reads `rows` directly so it never remounts on resource re-fetch.
    let rows: RwSignal<Vec<HistoryRow>> = RwSignal::new(Vec::new());
    Effect::new(move |_| {
        if let Some(r) = history.get() {
            rows.set(r);
        }
    });

    // Render the chart grid CLIENT-SIDE ONLY (after hydration). The panels use a
    // reactive `inner_html` for the SVG, which is fragile to hydrate (intermittent
    // `tachys` hydration "unreachable" panic that takes the whole page offline).
    // `mounted` is `false` on the server and on the client's first (hydrating)
    // render — so both trees match (no grid) — then this client-only Effect flips
    // it to `true`, mounting the grid as a fresh client render (no hydration).
    let (mounted, set_mounted) = signal(false);
    Effect::new(move |_| set_mounted.set(true));

    // Browser-only: advance `window` every 30 s when following "now".
    // This triggers a history refetch → `rows` updates → panels patch SVGs in
    // place.  The 30 s cadence is a UI refresh interval, NOT a synchronization
    // sleep; each tick checks `following` before acting.  Safe because the
    // grid is mounted exactly once and only `rows` changes.
    #[cfg(feature = "hydrate")]
    {
        use std::time::Duration;
        let interval = set_interval_with_handle(
            move || {
                if following.get_untracked() {
                    let now = chrono::Utc::now().timestamp();
                    let span = window.with_untracked(TimeWindow::span_secs);
                    window.set(TimeWindow {
                        from_ts: now - span,
                        to_ts: now,
                    });
                }
            },
            Duration::from_secs(30),
        );
        if let Ok(handle) = interval {
            on_cleanup(move || handle.clear());
        }
    }

    // ── Pan / zoom button handlers ─────────────────────────────────────────
    // These disable "follow" — the user is exploring a fixed history slice.
    let zoom_in = move |_| {
        following.set(false);
        window.update(|w| *w = zoom_about(*w, 0.5, 0.5));
    };
    let zoom_out = move |_| {
        following.set(false);
        window.update(|w| *w = zoom_about(*w, 0.5, 2.0));
    };
    let pan_back = move |_| {
        following.set(false);
        window.update(|w| *w = pan_by(*w, -0.25));
    };
    let pan_back_big = move |_| {
        following.set(false);
        window.update(|w| *w = pan_by(*w, -1.0));
    };
    let pan_fwd = move |_| {
        following.set(false);
        window.update(|w| *w = pan_by(*w, 0.25));
    };
    let pan_fwd_big = move |_| {
        following.set(false);
        window.update(|w| *w = pan_by(*w, 1.0));
    };

    // Return AnyView to erase the concrete view-tuple type before the leptos
    // router's `.into_any()` call sees it — prevents the compiler's query-depth
    // overflow that occurs when tachys computes the layout of the full type.
    view! {
        <div class="content-area">
            <LiveBand />
            <div class="time-controls">
                <TimeSelect window=window following=following />
                <div class="pan-zoom-controls">
                    <button class="pz-btn" on:click=pan_back_big title="Reculer d'une période">"◀◀"</button>
                    <button class="pz-btn" on:click=pan_back title="Reculer d'¼ période">"◀"</button>
                    <button class="pz-btn" on:click=zoom_in title="Zoom avant">"-"</button>
                    <button class="pz-btn" on:click=zoom_out title="Zoom arrière">"+"</button>
                    <button class="pz-btn" on:click=pan_fwd title="Avancer d'¼ période">"▶"</button>
                    <button class="pz-btn" on:click=pan_fwd_big title="Avancer d'une période">"▶▶"</button>
                </div>
            </div>

            // Client-only mount (see `mounted` above): identical empty tree on
            // server + first client render, then the grid renders fresh on the
            // client. Mounted once; panel SVGs then update in place via reactive
            // Signal<ChartSeries> props — no remount on history refetch.
            {move || mounted.get().then(|| view! { <HistoryGrid rows=rows /> })}
        </div>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Chart grid — mounted once, panels reactive
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
fn panel(title: &str, unit: &str, series: Signal<ChartSeries>) -> AnyView {
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
/// Mounted **once** in `AllPanelsPage`.  Each panel receives a
/// `Signal<ChartSeries>` built with `Signal::derive` that derives exclusively
/// from the stable history backfill (`rows`): avg line + min–max band across
/// the full width.  Raw live frames are NOT appended — the chart is uniform in
/// character (minute-bucketed) across its entire width.  The SVG is patched in
/// place via `inner_html` on the panel's wrapper `<div>` — no subtree remount
/// occurs when `rows` is updated by a history refetch.
///
/// Two full-width sections stacked vertically — CAPTEURS (6 panels, 3-column
/// grid matching the TUI row ordering) then ÉNERGIE (3 panels).  Panels are
/// built as `Vec<AnyView>` so the per-panel type is erased before being placed
/// into the containing `<div>`, preventing the monomorphized view type from
/// overflowing the compiler's query-depth limit.
///
/// Panel order mirrors `crates/meteo-tui/src/ui/history.rs`:
/// - CAPTEURS row 1: Température air · Température ciel · Luminosité
/// - CAPTEURS row 2: Pression · Vitesse du vent · Humidité
/// - ÉNERGIE row 1: Batterie · Solaire · Charge
///
/// Energy series (Batterie / Solaire / Charge) have only avg values in
/// `HistoryRow` (no min/max columns), so their `band` is always `None`.
#[component]
fn HistoryGrid(rows: RwSignal<Vec<HistoryRow>>) -> impl IntoView {
    // ── CAPTEURS — reactive series in TUI display order ───────────────────

    let temp_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) = series_from_rows(
            &current_rows,
            |r| r.temp.avg,
            |r| r.temp.min,
            |r| r.temp.max,
        );
        ChartSeries {
            points: pts,
            band,
            color_hex: css(PEACH),
            floor: None,
            prec: 1,
        }
    });

    let sky_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) =
            series_from_rows(&current_rows, |r| r.sky.avg, |r| r.sky.min, |r| r.sky.max);
        ChartSeries {
            points: pts,
            band,
            color_hex: css(LAVENDER),
            floor: None,
            prec: 1,
        }
    });

    let lux_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) =
            series_from_rows(&current_rows, |r| r.lux.avg, |r| r.lux.min, |r| r.lux.max);
        ChartSeries {
            points: pts,
            band,
            color_hex: css(YELLOW),
            floor: Some(0.0),
            prec: 0,
        }
    });

    let pres_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) = series_from_rows(
            &current_rows,
            |r| r.pressure.avg,
            |r| r.pressure.min,
            |r| r.pressure.max,
        );
        ChartSeries {
            points: pts,
            band,
            // BLUE (not TEAL) — matches the TUI's pressure colour so
            // pressure reads distinct from the cyan SKY wind line.
            color_hex: css(BLUE),
            floor: None,
            prec: 1,
        }
    });

    let wind_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) = series_from_rows(
            &current_rows,
            |r| r.wind.avg,
            |r| r.wind.min,
            |r| r.wind.max,
        );
        ChartSeries {
            points: pts,
            band,
            color_hex: css(SKY),
            floor: Some(0.0),
            prec: 1,
        }
    });

    let hum_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let (pts, band) = series_from_rows(
            &current_rows,
            |r| r.humidity.avg,
            |r| r.humidity.min,
            |r| r.humidity.max,
        );
        ChartSeries {
            points: pts,
            band,
            color_hex: css(SAPPHIRE),
            floor: Some(0.0),
            prec: 0,
        }
    });

    // Row 1: Temp air (PEACH) · Temp ciel (LAVENDER) · Luminosité (YELLOW)
    // Row 2: Pression (BLUE)  · Vent (SKY)           · Humidité (SAPPHIRE)
    let capteurs: Vec<AnyView> = vec![
        panel("Température de l'air", "°C", temp_series),
        panel("Température du ciel", "°C", sky_series),
        panel("Luminosité", "lx", lux_series),
        panel("Pression", "hPa", pres_series),
        panel("Vitesse du vent", "m/s", wind_series),
        panel("Humidité", "%", hum_series),
    ];

    // ── ÉNERGIE — Batterie · Solaire · Charge ─────────────────────────────
    // `HistoryRow` only carries avg for energy metrics (no min/max columns),
    // so `band` is always `None` for these panels.

    let batt_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let pts: Vec<(f64, f64)> = current_rows
            .iter()
            .filter_map(|r| r.battery_avg.map(|v| (r.ts as f64, v)))
            .collect();
        ChartSeries {
            points: pts,
            band: None,
            color_hex: css(GREEN),
            floor: Some(0.0),
            prec: 0,
        }
    });

    let solar_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let pts: Vec<(f64, f64)> = current_rows
            .iter()
            .filter_map(|r| r.solar_w_avg.map(|v| (r.ts as f64, v)))
            .collect();
        ChartSeries {
            points: pts,
            band: None,
            color_hex: css(YELLOW),
            floor: Some(0.0),
            prec: 2,
        }
    });

    let load_series: Signal<ChartSeries> = Signal::derive(move || {
        let current_rows = rows.get();
        let pts: Vec<(f64, f64)> = current_rows
            .iter()
            .filter_map(|r| r.load_w_avg.map(|v| (r.ts as f64, v)))
            .collect();
        ChartSeries {
            points: pts,
            band: None,
            color_hex: css(MAUVE),
            floor: Some(0.0),
            prec: 2,
        }
    });

    let energie: Vec<AnyView> = vec![
        panel("Batterie", "%", batt_series),
        panel("Solaire", "W", solar_series),
        panel("Charge", "W", load_series),
    ];

    view! {
        <div class="history-grid">
            <section class="chart-group">
                <h2 class="chart-group-label font-mono">"CAPTEURS"</h2>
                <div class="panel-grid">{capteurs}</div>
            </section>
            <section class="chart-group">
                <h2 class="chart-group-label font-mono">"ÉNERGIE"</h2>
                <div class="panel-grid">{energie}</div>
            </section>
        </div>
    }
}
