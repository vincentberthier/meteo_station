//! Reusable UI components for the `MeteoStation` web dashboard.
//!
//! Components compile under **both** `ssr` and `hydrate` features. Pure helpers
//! (like `chart::project`) are plain Rust and can be unit-tested under `ssr`.

pub mod chart;
pub mod compass;
pub mod header;

pub use chart::{ChartSeries, PlotPanel};
pub use compass::WindCompass;
pub use header::{Header, SignalLevel};
