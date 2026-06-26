//! `MeteoStation` web dashboard — Leptos 0.8 SSR application.
//!
//! This crate is built twice by cargo-leptos:
//!  - server binary (`ssr` feature): axum + tokio, runs on `x86_64`/`aarch64`.
//!  - WASM bundle (`hydrate` feature): runs in the browser for hydration.

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

/// Root application component.
///
/// Provides the Catppuccin shell, the router, and the two top-level routes:
/// - `/` — live dashboard (placeholder for substep 7)
/// - `/comparaison` — historic comparison view (placeholder for substep 8)
#[must_use]
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/meteo-web.css"/>
        <Title text="MeteoStation"/>

        <Router>
            <div class="page-main">
                <SiteHeader/>
                <Routes fallback=|| view! { <p>"Page introuvable."</p> }>
                    <Route path=StaticSegment("") view=DashboardPage/>
                    <Route path=StaticSegment("comparaison") view=ComparaisonPage/>
                </Routes>
            </div>
        </Router>
    }
}

/// Shared site header with navigation (placeholder — substep 7 replaces this).
#[component]
fn SiteHeader() -> impl IntoView {
    view! {
        <header class="app-header">
            <a class="app-title" href="/">"MeteoStation"</a>
            <nav>
                <a href="/">"En direct"</a>
                <a href="/comparaison">"Comparaison"</a>
            </nav>
        </header>
    }
}

/// Live dashboard placeholder (route `/`).
#[component]
fn DashboardPage() -> impl IntoView {
    view! {
        <div class="content-area">
            <div class="live-band">
                <span class="live-label">"En direct"</span>
            </div>
            <div class="history-grid">
                <div class="chart-panel">
                    <div class="chart-panel-header">"Température de l'air"</div>
                    <div class="chart-panel-body"></div>
                </div>
                <div class="chart-panel">
                    <div class="chart-panel-header">"Vitesse du vent"</div>
                    <div class="chart-panel-body">
                        <span class="wind-summary">
                            <span class="color-sky">"— m/s"</span>
                            <span class="gust-label">"rafale"</span>
                        </span>
                    </div>
                </div>
                <div class="chart-panel">
                    <div class="chart-panel-header">"Pression"</div>
                    <div class="chart-panel-body"></div>
                </div>
            </div>
        </div>
    }
}

/// Historic comparison placeholder (route `/comparaison`).
#[component]
fn ComparaisonPage() -> impl IntoView {
    view! {
        <div class="content-area">
            <h1 class="font-mono color-peach">"Comparaison historique"</h1>
            <p class="color-subtext">"Les graphiques comparatifs s'afficheront ici."</p>
        </div>
    }
}

/// WASM hydration entry-point — called by the browser-side bundle.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
