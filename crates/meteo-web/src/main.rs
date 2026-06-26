// The SSR server binary. The wasm/hydrate side has no `main`; this file is
// compiled only when the `ssr` feature is active (cargo-leptos bin target).

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes as _, generate_route_list};
    use meteo_web::{App, shell};

    let conf = get_configuration(None)?;
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;

    let routes = generate_route_list(App);

    let app = Router::new()
        .leptos_routes(&leptos_options, routes, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    log!("MeteoStation web dashboard listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Stub for the wasm/hydrate target — the real entry-point is `hydrate()` in
/// `lib.rs`; `fn main` must exist for the binary crate, but is never called.
#[cfg(not(feature = "ssr"))]
pub fn main() {}
