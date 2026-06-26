//! Shared application state — SSR-only.
//!
//! [`AppState`] is the axum router state (via `.with_state(app_state)`) and is
//! injected into leptos context by [`leptos_axum::LeptosRoutes::leptos_routes_with_context`],
//! so server functions can retrieve it with `expect_context::<AppState>()`.
//!
//! [`LeptosOptions`] must be extractable from `AppState` (the
//! `LeptosOptions: FromRef<AppState>` bound imposed by `LeptosRoutes<S>` and
//! `file_and_error_handler`), so we include it as a field and provide a manual
//! `FromRef` impl.

use axum::extract::FromRef;
use leptos::config::LeptosOptions;
use meteo_lib::Telemetry;
use tokio::sync::watch;

use crate::db::DbHandle;

/// Shared application state.
///
/// Passed as axum state (`Router::with_state`) and provided as leptos context
/// by `leptos_routes_with_context`, making it available to all server functions
/// via `expect_context::<AppState>()`.
#[derive(Clone)]
pub struct AppState {
    /// Leptos framework configuration (site address, output paths, …).
    ///
    /// Stored here so that `LeptosOptions: FromRef<AppState>` can be satisfied
    /// without storing a second copy outside the state.
    pub leptos_options: LeptosOptions,
    /// Handle to the SQLite samples database.
    pub db: DbHandle,
    /// Watch channel receiver for the latest decoded BLE telemetry frame.
    ///
    /// Starts as `None` until the collector decodes its first frame.
    /// The SSE handler borrows this each tick to push the current frame to
    /// connected browser clients.
    pub live_rx: watch::Receiver<Option<Telemetry>>,
}

/// Allow axum (and `leptos_axum`) to extract `LeptosOptions` from `AppState`.
///
/// This satisfies the `LeptosOptions: FromRef<S>` bound required by:
/// - [`leptos_axum::LeptosRoutes::leptos_routes_with_context`]
/// - [`leptos_axum::file_and_error_handler`]
impl FromRef<AppState> for LeptosOptions {
    fn from_ref(input: &AppState) -> Self {
        input.leptos_options.clone()
    }
}
