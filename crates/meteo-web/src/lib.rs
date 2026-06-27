//! `MeteoStation` web dashboard — Leptos 0.8 SSR application.
//!
//! This crate is built twice by cargo-leptos:
//!  - server binary (`ssr` feature): axum + tokio, runs on `x86_64`/`aarch64`.
//!  - WASM bundle (`hydrate` feature): runs in the browser for hydration.

// The AllPanelsPage + LiveBand + HistoryGrid composition produces a view type
// that is deeply monomorphised by the tachys renderer.  The default limit of
// 128 is insufficient for the combined type tree.
#![recursion_limit = "512"]
// Leptos's #[component] macro expands to a typed-builder struct. The generated
// `builder()` method has the same name as the blanket trait impl, and the
// generated `pub fn App()` wrapper does not preserve `#[must_use]` placed
// outside the macro — neither lint is actionable from user code.
#![allow(
    clippy::same_name_method,
    reason = "leptos #[component] macro generates a builder() that triggers this"
)]
#![allow(
    clippy::must_use_candidate,
    reason = "leptos #[component] macro rewrites pub fn signatures, dropping the attribute"
)]
// meteo-web is a host std binary/library; the no_std-oriented workspace lints
// that prefer core:: / alloc:: over std:: do not apply here.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "meteo-web is a host std crate; core/alloc-first lints do not apply"
)]

pub mod api;
pub mod components;
pub mod pages;
pub mod types;

#[cfg(feature = "ssr")]
pub mod db;

#[cfg(feature = "ssr")]
pub mod collector;

#[cfg(feature = "ssr")]
pub mod state;

use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    StaticSegment,
    components::{Route, Router, Routes},
};

use components::header::{Header, SignalLevel};
use pages::all_panels::AllPanelsPage;
use pages::comparison::ComparisonPage;
use types::LiveFrame;

/// HTML shell returned for every page request (SSR).
///
/// Wraps the full `<html>` document. `cargo-leptos` injects the compiled
/// WASM + JS bundle paths via [`HydrationScripts`] and [`AutoReload`].
#[must_use]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// Opens the `/live` SSE stream and wires `live_frame`, `signal_state`,
/// and a 1 Hz staleness check (browser-only).
///
/// - Each parsed [`LiveFrame`] is pushed into `live_frame` and records
///   `last_rx_ms` (milliseconds since epoch via [`js_sys::Date::now`]).
/// - `signal_state` transitions to [`SignalLevel::Live`] on every received
///   frame and to [`SignalLevel::Stale`] once no frame has arrived for > 5 s.
/// - The `EventSource` and the interval are both cancelled via [`on_cleanup`]
///   when the owning reactive scope is dropped.
#[cfg(feature = "hydrate")]
fn setup_live_state(live_frame: RwSignal<Option<LiveFrame>>, signal_state: RwSignal<SignalLevel>) {
    use std::time::Duration;
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::closure::Closure;

    let last_rx_ms: RwSignal<Option<f64>> = RwSignal::new(None);

    Effect::new(move |_| {
        // ── SSE connection ────────────────────────────────────────────────
        let Ok(es) = web_sys::EventSource::new("/live") else {
            return;
        };

        let es_clone = es.clone();
        let cb: Closure<dyn FnMut(web_sys::MessageEvent)> =
            Closure::wrap(Box::new(move |ev: web_sys::MessageEvent| {
                let data = ev.data();
                if let Some(text) = data.as_string()
                    && let Ok(lf) = serde_json::from_str::<LiveFrame>(&text)
                {
                    live_frame.set(Some(lf));
                    last_rx_ms.set(Some(js_sys::Date::now()));
                    signal_state.set(SignalLevel::Live);
                }
            }));

        es.set_onmessage(Some(cb.as_ref().unchecked_ref()));
        // Keep the EventSource and closure alive until the reactive owner
        // is dropped; the cleanup below closes the connection explicitly.
        cb.forget();

        on_cleanup(move || {
            es_clone.close();
        });

        // ── 1 Hz staleness check ──────────────────────────────────────────
        // Transitions Live → Stale when no frame has arrived in the last 5 s.
        // A bounded UI-cadence poll is the correct mechanism here: the SSE
        // stream stays open (keep-alives) even when the station stops
        // transmitting, so only frame age reveals a lost station.
        let interval = set_interval_with_handle(
            move || {
                if let Some(t) = last_rx_ms.get_untracked()
                    && js_sys::Date::now() - t > 5_000.0
                {
                    signal_state.set(SignalLevel::Stale);
                }
            },
            Duration::from_secs(1),
        );

        if let Ok(handle) = interval {
            on_cleanup(move || handle.clear());
        }
    });
}

/// Root application component.
///
/// Provides the Catppuccin shell, the router, and the two top-level routes:
/// - `/` — live dashboard
/// - `/comparaison` — historic comparison view
///
/// The shared `live_frame` context signal is provided here so any descendant
/// (including `LiveBand`) can read the latest telemetry without passing props.
/// `signal_state` is derived from frame freshness on the browser side (transitions
/// `NoSignal → Live → Stale`) and stays `NoSignal` on the server until hydration.
#[must_use]
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // ── Shared live-frame context ─────────────────────────────────────────
    // Any descendant can call `expect_context::<RwSignal<Option<LiveFrame>>>()`
    // to read the latest frame without prop-drilling.
    let live_frame: RwSignal<Option<LiveFrame>> = RwSignal::new(None);
    provide_context(live_frame);

    // ── Signal state (NoSignal → Live → Stale) ────────────────────────────
    let signal_state: RwSignal<SignalLevel> = RwSignal::new(SignalLevel::NoSignal);

    // Browser only: open the SSE stream, drive live_frame, and run the
    // freshness check that transitions signal_state between Live / Stale.
    #[cfg(feature = "hydrate")]
    setup_live_state(live_frame, signal_state);

    view! {
        <Stylesheet id="leptos" href="/pkg/meteo-web.css"/>
        <Title text="MeteoStation"/>

        <Router>
            <div class="page-main">
                <Header signal_state=signal_state.into()/>
                <Routes fallback=|| view! { <p>"Page introuvable."</p> }>
                    <Route path=StaticSegment("") view=AllPanelsPage/>
                    <Route path=StaticSegment("comparaison") view=ComparisonPage/>
                </Routes>
            </div>
        </Router>
    }
}

/// WASM hydration entry-point — called by the browser-side bundle.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
