// The SSR server binary. The wasm/hydrate side has no `main`; this file is
// compiled only when the `ssr` feature is active (cargo-leptos bin target).

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::path::Path;

    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes as _, generate_route_list};
    use meteo_web::{App, api::sse::live_sse, collector, db::DbHandle, shell, state::AppState};
    use tokio::sync::watch;

    let conf = get_configuration(None)?;
    let leptos_options = conf.leptos_options.clone();
    let addr = leptos_options.site_addr;

    // --- Database -------------------------------------------------------
    // Path is overridable via env; default to `meteo.db` in the working dir.
    let db_path = std::env::var("METEO_DB_PATH").unwrap_or_else(|_| "meteo.db".into());
    let db = DbHandle::open(Path::new(&db_path))
        .map_err(|e| anyhow::anyhow!("failed to open database at {db_path}: {e}"))?;

    // --- BLE live channel ----------------------------------------------
    let (live_tx, live_rx) = watch::channel(None);

    // --- Station address -----------------------------------------------
    // Use the compile-time constant; can be extended to parse METEO_STATION_ADDR
    // env in the future (bluer::Address parsing requires MAC bytes).
    let station_addr = collector::STATION_ADDR;

    // --- Application state --------------------------------------------
    let app_state = AppState {
        leptos_options: leptos_options.clone(),
        db: db.clone(),
        live_rx,
    };

    // --- Spawn the BLE collector --------------------------------------
    // The collector runs forever (until bluetoothd disappears unrecoverably).
    // We spawn it as a background task; if it exits, the SSE stream will just
    // stop receiving new frames but the HTTP server stays up.
    tokio::spawn(collector::run(db, live_tx, station_addr));

    // --- Routes -------------------------------------------------------
    let routes = generate_route_list(App);

    let app = Router::new()
        // Live SSE endpoint — mounted before leptos routes so it is not
        // intercepted by the leptos fallback handler.
        .route("/live", axum::routing::get(live_sse))
        // Leptos page routes + server-function routes.
        // leptos_routes_with_context provides AppState as leptos context
        // (via provide_context::<AppState>) so server fns can call
        // expect_context::<AppState>().
        .leptos_routes_with_context(&app_state, routes, || {}, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .with_state(app_state);

    log!("MeteoStation web dashboard listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Stub for the wasm/hydrate target — the real entry-point is `hydrate()` in
/// `lib.rs`; `fn main` must exist for the binary crate, but is never called.
#[cfg(not(feature = "ssr"))]
pub fn main() {}
