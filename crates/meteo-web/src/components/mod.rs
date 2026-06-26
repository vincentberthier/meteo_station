//! Reusable UI components for the `MeteoStation` web dashboard.
//!
//! Components compile under **both** `ssr` and `hydrate` features. Pure helpers
//! (like `chart::project`) are plain Rust and can be unit-tested under `ssr`.

pub mod chart;
pub mod compass;
pub mod header;
pub mod live_band;
pub mod time_select;

pub use chart::{ChartSeries, PlotPanel};
pub use compass::WindCompass;
pub use header::{Header, SignalLevel};
pub use live_band::LiveBand;
pub use time_select::{
    TimeSelect, TimeWindow, pan_by, preset_day, preset_month, preset_week, zoom_about,
};
