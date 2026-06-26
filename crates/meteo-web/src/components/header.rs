//! Site header component — wall clock, app version, signal state.
//!
//! The `Header` component is compiled under **both** `ssr` and `hydrate` features.
//!
//! **Wall clock:**
//! - Under `ssr`: rendered as the current server time via `chrono::Local::now()`.
//! - Under `hydrate`: rendered as a `"--:--"` placeholder (no `js_sys`/`web_sys`
//!   dependency required); can be wired to a reactive timer in a future substep.
//!
//! **Signal state** (`SignalLevel`) mirrors the TUI's `SignalState` concept but is
//! defined locally so `meteo-web` has no dependency on `meteo-tui`.

// The leptos #[component] macro generates a typed-builder struct whose field names
// shadow the function parameters.  Neither shadow is actionable from user code.
#![allow(
    clippy::shadow_reuse,
    reason = "leptos #[component] macro generates param shadows in the builder"
)]

use leptos::prelude::*;

/// Reception quality of the live BLE broadcast.
///
/// Derived from the age of the last received telemetry frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalLevel {
    /// No frame has ever been received.
    NoSignal,
    /// Last frame received within `STALE_AFTER` seconds.
    Live,
    /// Last frame older than `STALE_AFTER` seconds.
    Stale,
}

/// Returns the current local time as `"HH:MM"` under SSR.
///
/// Under `hydrate`, returns a static `"--:--"` placeholder.
fn current_time_str() -> String {
    #[cfg(feature = "ssr")]
    {
        chrono::Local::now().format("%H:%M").to_string()
    }
    #[cfg(not(feature = "ssr"))]
    {
        "--:--".to_owned()
    }
}

/// Site header: navigation, signal state badge, current time, and app version.
///
/// `signal_state` is a reactive `Signal<SignalLevel>` so the badge updates when
/// the live-data stream transitions (No signal → Live → Stale).
///
/// Under SSR the wall clock is rendered as the current server time; the browser
/// side shows `"--:--"` until hydration (future substep may add a live timer).
#[component]
pub fn Header(
    /// Current signal quality — drives the coloured badge.
    signal_state: Signal<SignalLevel>,
) -> impl IntoView {
    let version = env!("CARGO_PKG_VERSION");

    let badge_label = move || match signal_state.get() {
        SignalLevel::Live => "En direct",
        SignalLevel::Stale => "Obsolète",
        SignalLevel::NoSignal => "Hors-ligne",
    };
    let badge_class = move || match signal_state.get() {
        SignalLevel::Live => "signal-badge signal-live",
        SignalLevel::Stale => "signal-badge signal-stale",
        SignalLevel::NoSignal => "signal-badge signal-offline",
    };

    let clock = current_time_str();

    view! {
        <header class="app-header">
            <a class="app-title" href="/">"MeteoStation"</a>
            <nav class="app-nav">
                <a href="/">"En direct"</a>
                <a href="/comparaison">"Comparaison"</a>
            </nav>
            <div class="header-meta">
                <span class={badge_class}>{badge_label}</span>
                <span class="app-clock font-mono">{clock}</span>
                <span class="app-version font-mono">"v"{version}</span>
            </div>
        </header>
    }
}
