//! Pure display and chart-math helpers shared by the TUI dashboard and the web
//! dashboard. No ratatui, no bluer — compiles for host and `wasm32-unknown-unknown`.

pub mod chart;
pub mod format;
pub mod palette;
pub mod xaxis;

pub use chart::{gaussian_smooth, padded_value_bounds, value_axis_labels};
pub use format::{
    Trend, classify_trend, compass_label_fr, dew_point_c, fmt_battery_flow, fmt_location, fmt_lux,
    fmt_power, fmt_uptime, lux_chart_unit, power_w,
};
pub use xaxis::{SunKind, SunMark, day_marks, fmt_dm, fmt_hm, sun_marks};
