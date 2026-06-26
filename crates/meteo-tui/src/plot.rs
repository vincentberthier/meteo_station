//! Plot-panel types and the styled block builder for the dashboard history charts.
//!
//! [`PlotSpec`], [`MarkerStyle`], [`Overlay`], and [`Bars`] are shared between the
//! image rasterizer (`image_render`) and the history layout (`ui::history`).
//! [`make_block`] builds the reusable bordered, titled block for each panel.

use ratatui::style::{Color, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::Block;

use crate::theme;

/// Marker style for drawing traces on the history plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerStyle {
    /// Individual Braille dots — one glyph per sample (sparse).
    Dots,
    /// Braille line segments connecting consecutive samples.
    Line,
}

/// One overlay trace rendered behind the main series line (e.g. gust @ 32 %).
pub struct Overlay<'data> {
    /// The `(t_secs, value)` samples for the overlay.
    pub points: &'data [(f64, f64)],
    /// Base colour before alpha blending toward [`BASE`].
    ///
    /// [`BASE`]: crate::theme::BASE
    pub color: Color,
    /// Alpha `∈ [0, 1]`: how much `color` survives after blending with `BASE`
    /// (`0.0` = fully transparent, `1.0` = fully opaque).
    pub alpha: f64,
}

/// Bar dataset rendered on an independent lower-half y-scale (e.g. rain rate).
///
/// Zero-valued bars receive a faint baseline tick so gaps stay visible.
pub struct Bars<'data> {
    /// The `(t_secs, value)` samples for the bars.
    pub points: &'data [(f64, f64)],
    /// Bar colour.
    pub color: Color,
}

/// Configuration for one history plot panel.
pub struct PlotSpec<'data> {
    /// Panel title shown in the block border (e.g. `"Température air"`).
    pub title: &'data str,
    /// Unit chip text shown top-right (e.g. `"°C"`, `"klx"`, `"W"`).
    pub unit: &'data str,
    /// Colour of the main trace line / dots.
    pub color: Color,
    /// Decimal precision for y-axis tick labels.
    pub prec: usize,
    /// Optional floor for `padded_value_bounds`; use `Some(0.0)` for physically
    /// non-negative metrics (luminosity, humidity) so the padded lower bound never
    /// goes negative.  `None` for metrics that can go negative (temperature).
    pub floor: Option<f64>,
    /// Display multiplier applied to y-axis **labels only** (`1.0` normally,
    /// `0.001` to display raw lux as klx). The image coordinate space and trace
    /// geometry use raw units unchanged.
    pub scale: f64,
    /// Drawing style hint for the trace.  Passed through for struct completeness;
    /// the image rasterizer always draws a connected LINE regardless of this field
    /// (braille dots are unreadable at sub-cell image resolution).
    #[expect(
        dead_code,
        reason = "marker is intentionally ignored by the image rasterizer; retained in the struct for forward compatibility"
    )]
    pub marker: MarkerStyle,
    /// Draw dotted gridlines at 25 %, 50 %, and 75 % of the y range.
    pub show_grid: bool,
    /// Draw a gradient fill under the main trace fading from ~40/255 opacity at
    /// the trace to 0 % at the baseline.
    pub fill: bool,
    /// Optional overlay trace (rendered behind the main trace).
    pub overlay: Option<Overlay<'data>>,
    /// Optional bar dataset mapped to the lower 30 % of the canvas y-range.
    pub bars: Option<Bars<'data>>,
}

/// Build the styled bordered block with title (left) and unit chip (right).
///
/// Used by [`crate::ui::history`] to frame each plot panel, and by the image
/// rasterizer's empty-series placeholder.
pub fn make_block(spec: &PlotSpec<'_>) -> Block<'static> {
    let title_line = TextLine::from(Span::styled(
        spec.title.to_owned(),
        Style::new().fg(theme::SUBTEXT0),
    ));
    let unit_chip = TextLine::from(Span::styled(
        format!(" {} ", spec.unit),
        Style::new().fg(theme::OVERLAY2).bg(theme::SURFACE0),
    ))
    .right_aligned();

    Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .title(title_line)
        .title_top(unit_chip)
        .style(Style::new().bg(theme::MANTLE))
}
