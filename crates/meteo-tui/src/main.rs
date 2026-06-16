//! meteo-tui: terminal dashboard for the `MeteoStation` BLE peripheral.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "meteo-tui is a host std binary; core/alloc-first lints do not apply"
)]
#![allow(
    clippy::print_stderr,
    reason = "fatal startup errors are reported to stderr before the TUI takes the terminal"
)]
// The skeleton main() is trivially const and wraps nothing; both lints are
// suppressed here because later substeps will add async setup, I/O, and
// early-exit error handling that naturally resolve them.
#![allow(
    clippy::missing_const_for_fn,
    clippy::unnecessary_wraps,
    reason = "skeleton; main will be non-trivial once BLE/TUI setup is added"
)]

mod app;
mod ble;
mod model;
mod ui;

fn main() -> anyhow::Result<()> {
    Ok(())
}
