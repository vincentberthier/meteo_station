//! Pixel-based chart rasterizer and compass composer for the dashboard.
//!
//! Wired into the render loop in the next step.

// Public API is consumed by the render loop wired in the next step.
#![allow(dead_code, reason = "wired into the render loop in the next step")]

use core::num::NonZeroU16;
use std::collections::HashMap;
use std::io::Cursor;

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage, imageops};
use imageproc::{
    drawing::draw_antialiased_line_segment_mut,
    geometric_transformations::{Interpolation, rotate_about_center},
    pixelops::interpolate,
};
use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    style::Color,
};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};

use crate::{plot::PlotSpec, theme};

// ─── A. Pixel-color helper ───────────────────────────────────────────────────

/// Map a ratatui `Color::Rgb(r, g, b)` to `image::Rgba<u8>` (full opacity).
/// Any non-`Rgb` variant (named, Reset, Indexed) falls back to opaque black.
const fn rgba(c: Color) -> Rgba<u8> {
    match c {
        Color::Rgb(r, g, b) => Rgba([r, g, b, 255]),
        Color::Reset
        | Color::Black
        | Color::Red
        | Color::Green
        | Color::Yellow
        | Color::Blue
        | Color::Magenta
        | Color::Cyan
        | Color::Gray
        | Color::DarkGray
        | Color::LightRed
        | Color::LightGreen
        | Color::LightYellow
        | Color::LightBlue
        | Color::LightMagenta
        | Color::LightCyan
        | Color::White
        | Color::Indexed(_) => Rgba([0, 0, 0, 255]),
    }
}

// ─── Internal drawing helpers ────────────────────────────────────────────────

/// Convert a `(time, value)` data-space point to pixel `(x, y)`.
///
/// The y-axis is flipped: larger data values map to smaller pixel y (nearer
/// the top of the image).  Results are raw `i32` and may be outside
/// `[0, w-1] × [0, h-1]`; the antialiased line drawer clips automatically.
#[expect(
    clippy::too_many_arguments,
    reason = "coordinate mapper needs all 8 parameters: t/v data coords, x0/x1/y0/y1 window, w/h image size"
)]
fn to_px(t: f64, v: f64, x0: f64, x1: f64, y0: f64, y1: f64, w: u32, h: u32) -> (i32, i32) {
    let x_range = x1 - x0;
    let y_range = y1 - y0;

    let px = if x_range.abs() > f64::EPSILON {
        ((t - x0) / x_range) * f64::from(w.saturating_sub(1))
    } else {
        0.0
    };

    let py_frac = if y_range.abs() > f64::EPSILON {
        (v - y0) / y_range
    } else {
        0.5
    };
    // Flip: data-max → smallest pixel row (top of image).
    let py = (1.0 - py_frac) * f64::from(h.saturating_sub(1));

    #[expect(
        clippy::cast_possible_truncation,
        reason = "pixel x: .round() removes fractional part; image dims are always within i32 range"
    )]
    let xi = px.round() as i32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "pixel y: .round() removes fractional part; image dims are always within i32 range"
    )]
    let yi = py.round() as i32;
    (xi, yi)
}

/// Alpha-composite `fg` at the given `alpha` over `bg` (both opaque RGBA).
/// Returns an opaque pixel.
fn alpha_over(fg: Rgba<u8>, alpha: u8, bg: Rgba<u8>) -> Rgba<u8> {
    let a = f64::from(alpha) / 255.0;
    let blend_ch = |f: u8, b: u8| -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "result of linear blend is always in [0.0, 255.0]"
        )]
        {
            f64::from(b).mul_add(1.0 - a, f64::from(f) * a).round() as u8
        }
    };
    Rgba([
        blend_ch(fg.0[0], bg.0[0]),
        blend_ch(fg.0[1], bg.0[1]),
        blend_ch(fg.0[2], bg.0[2]),
        255,
    ])
}

/// Draw a connected anti-aliased polyline through `pts` onto `img`.
///
/// **NOTE: `spec.marker` is intentionally ignored.** The image renderer
/// ALWAYS draws a connected LINE — braille dots are unreadable at sub-cell
/// image resolution, which is the entire reason this pixel renderer exists.
/// A +1 y-offset duplicate stroke gives ~2 px effective line weight.
#[expect(
    clippy::too_many_arguments,
    reason = "polyline needs image ref, points, color, and all 4 window/size params"
)]
#[expect(
    clippy::missing_asserts_for_indexing,
    reason = "pair comes from .windows(2) which always yields slices of exactly 2 elements"
)]
fn draw_polyline(
    img: &mut RgbaImage,
    pts: &[(f64, f64)],
    color: Rgba<u8>,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    w: u32,
    h: u32,
) {
    for pair in pts.windows(2) {
        let (ta, va) = pair[0];
        let (tb, vb) = pair[1];
        let start = to_px(ta, va, x0, x1, y0, y1, w, h);
        let end = to_px(tb, vb, x0, x1, y0, y1, w, h);
        draw_antialiased_line_segment_mut(img, start, end, color, interpolate);
        // Duplicate one pixel lower for ~2 px effective weight.
        draw_antialiased_line_segment_mut(
            img,
            (start.0, start.1.saturating_add(1)),
            (end.0, end.1.saturating_add(1)),
            color,
            interpolate,
        );
    }
}

// ─── B. Pure chart rasterizer ────────────────────────────────────────────────

/// Rasterize one history chart into a `w × h` pixel image.
///
/// Drawing order: background → gridlines → gradient fill → overlay trace →
/// bar dataset → main trace.  No text is rendered here; axis labels remain
/// in the TUI layout layer.
///
/// - **`spec.marker`** is ignored: the rasterizer always draws a connected
///   LINE (see [`draw_polyline`]).
/// - **`spec.scale`** is ignored: it only affects axis-label formatting in
///   the TUI.
#[expect(
    clippy::too_many_lines,
    reason = "the rasterizer is inherently long; all drawing passes are sequential and splitting would obscure the layering order"
)]
pub fn render_chart_image(
    spec: &PlotSpec<'_>,
    pts: &[(f64, f64)],
    x_win: [f64; 2],
    y_win: [f64; 2],
    w: u32,
    h: u32,
) -> RgbaImage {
    #[expect(
        clippy::shadow_reuse,
        reason = "clamping to 1 prevents zero-size image panic"
    )]
    let w = w.max(1);
    #[expect(
        clippy::shadow_reuse,
        reason = "clamping to 1 prevents zero-size image panic"
    )]
    let h = h.max(1);
    let mut img = RgbaImage::new(w, h);

    // 1. Fill the entire image with the panel background colour.
    let bg = rgba(theme::MANTLE);
    for px in img.pixels_mut() {
        *px = bg;
    }

    let [x0, x1] = x_win;
    let [y0, y1] = y_win;
    let x_range = x1 - x0;
    let y_range = y1 - y0;

    // Thin closure that captures window + size.
    let map = |t: f64, v: f64| to_px(t, v, x0, x1, y0, y1, w, h);

    // Signed image bounds — always within i32 range for real terminal images.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "image dims come from u16 cell counts × u16 font pixels; never reach i32::MAX"
    )]
    let w_i32 = w as i32;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "image dims come from u16 cell counts × u16 font pixels; never reach i32::MAX"
    )]
    let h_i32 = h as i32;

    // 2. Horizontal gridlines at 25 / 50 / 75 % of the y range.
    if spec.show_grid && y_range.abs() > f64::EPSILON {
        let grid_color = rgba(theme::blend_rgb(theme::SURFACE2, theme::BASE, 0.18));
        for frac in [0.25_f64, 0.50, 0.75] {
            let y_grid = frac.mul_add(y_range, y0);
            let (_, py) = map(x0, y_grid);
            if py < 0 || py >= h_i32 {
                continue;
            }
            #[expect(
                clippy::cast_sign_loss,
                reason = "py is verified >= 0 in the guard above"
            )]
            let row = py as u32;
            for col in 0..w {
                img.put_pixel(col, row, grid_color);
            }
        }
    }

    // 3. True-alpha gradient fill under the trace.
    //    For each consecutive point-pair whose temporal gap is ≤ 8 % of the
    //    x-window (so signal-loss gaps stay empty), iterate each x-column and
    //    fill vertically from the interpolated trace y down to the baseline.
    //    Alpha peaks at ~40/255 immediately under the trace and fades to 0 at
    //    the baseline.
    if spec.fill && pts.len() >= 2 {
        let max_gap = x_range * 0.08;
        let fill_fg = rgba(spec.color);
        let (_, y_bottom_raw) = map(x0, y0);
        let y_bottom = y_bottom_raw.clamp(0, h_i32.saturating_sub(1));

        for pair in pts.windows(2) {
            let &[(ta, va), (tb, vb)] = pair else {
                continue;
            };
            let seg_span = tb - ta;
            if seg_span <= 0.0 || seg_span > max_gap {
                continue;
            }
            let (xa, _) = map(ta, va);
            let (xb, _) = map(tb, vb);
            let xi_lo = xa.min(xb).max(0);
            let xi_hi = xa.max(xb).min(w_i32.saturating_sub(1));

            for xi in xi_lo..=xi_hi {
                // Linearly interpolate the data value at this x column.
                let x_frac = if xa == xb {
                    0.5_f64
                } else {
                    f64::from(xi.saturating_sub(xa)) / f64::from(xb.saturating_sub(xa))
                };
                let v_interp = x_frac.mul_add(vb - va, va);
                let (_, y_trace_raw) = map(x_frac.mul_add(seg_span, ta), v_interp);
                let y_trace = y_trace_raw.clamp(0, h_i32.saturating_sub(1));

                // Fill the column from trace (top, smaller y) to baseline.
                let fill_top = y_trace.min(y_bottom);
                let fill_bot = y_trace.max(y_bottom);
                let fill_span = fill_bot.saturating_sub(fill_top);

                for yi in fill_top..=fill_bot {
                    // Alpha: 40 at the trace row, 0 at the baseline row.
                    let alpha: u8 = if fill_span > 0 {
                        #[expect(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "value in [0, 40] after multiplication; non-negative"
                        )]
                        {
                            (f64::from(fill_bot.saturating_sub(yi)) / f64::from(fill_span) * 40.0)
                                .round() as u8
                        }
                    } else {
                        20
                    };
                    #[expect(
                        clippy::cast_sign_loss,
                        reason = "yi ∈ [fill_top, fill_bot] ⊆ [0, h-1]; non-negative"
                    )]
                    let row = yi as u32;
                    #[expect(
                        clippy::cast_sign_loss,
                        reason = "xi ∈ [xi_lo, xi_hi] ⊆ [0, w-1]; non-negative"
                    )]
                    let col = xi as u32;
                    let existing = *img.get_pixel(col, row);
                    img.put_pixel(col, row, alpha_over(fill_fg, alpha, existing));
                }
            }
        }
    }

    // 4. Overlay trace (rendered before the main trace so the main sits on top).
    if let Some(ov) = &spec.overlay {
        let ov_color = rgba(theme::blend_rgb(ov.color, theme::BASE, ov.alpha));
        draw_polyline(&mut img, ov.points, ov_color, x0, x1, y0, y1, w, h);
    }

    // 5. Bar dataset — vertical bars in the lower 30 % of the image on their
    //    own independent [0, max] scale.
    if let Some(bars) = &spec.bars
        && !bars.points.is_empty()
    {
        let max_bar = bars.points.iter().map(|&(_, v)| v).fold(0.0_f64, f64::max);
        let bar_area_h = f64::from(h) * 0.3;
        let bar_color = rgba(bars.color);
        for &(t, v) in bars.points {
            let (xi, _) = map(t, v);
            if xi < 0 || xi >= w_i32 {
                continue;
            }
            let bar_h_f = if max_bar > f64::EPSILON {
                (v / max_bar * bar_area_h).max(0.0)
            } else {
                1.0
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bar_h_f is in [0, bar_area_h] ≤ h; non-negative"
            )]
            let bar_h = bar_h_f.round() as u32;
            let y_bot = h.saturating_sub(1);
            let y_top = y_bot.saturating_sub(bar_h);
            #[expect(clippy::cast_sign_loss, reason = "xi is verified >= 0 above")]
            let col = xi as u32;
            for row in y_top..=y_bot {
                img.put_pixel(col, row, bar_color);
            }
        }
    }

    // 6. Main trace — always a connected LINE (see draw_polyline for rationale).
    draw_polyline(&mut img, pts, rgba(spec.color), x0, x1, y0, y1, w, h);

    img
}

// ─── C. Pure compass rasterizer ──────────────────────────────────────────────

/// Compose a compass image: the dial with an optionally rotated needle on top.
///
/// `rotate_about_center` rotates **clockwise** for positive `theta`, matching
/// the standard compass convention (0 ° = needle up / North, 90 ° = right /
/// East, 180 ° = down / South).  No sign negation is needed.
///
/// When `calm` is `true` or `heading_deg` is `None` the bare dial is returned
/// without a needle.  The transparent rotation default keeps the dial visible
/// through the needle's empty areas.
pub fn render_compass_image(
    dial: &RgbaImage,
    needle: &RgbaImage,
    heading_deg: Option<f64>,
    calm: bool,
    size: u32,
) -> RgbaImage {
    #[expect(
        clippy::shadow_reuse,
        reason = "clamping to 1 prevents zero-size image panic"
    )]
    let size = size.max(1);

    // Resize the dial to the target square.
    let mut out = imageops::resize(dial, size, size, imageops::FilterType::Lanczos3);

    // Overlay the rotated needle unless calm or heading unknown.
    if !calm && let Some(heading) = heading_deg {
        let needle_sized = imageops::resize(needle, size, size, imageops::FilterType::Lanczos3);
        // Clockwise rotation: positive theta → needle points more East for
        // positive heading. No sign flip needed for compass convention.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64→f32 precision loss is acceptable for a compass rotation angle"
        )]
        let theta = (heading as f32).to_radians();
        let rotated = rotate_about_center(
            &needle_sized,
            theta,
            Interpolation::Bilinear,
            Rgba([0, 0, 0, 0]), // transparent: dial shows through
        );
        // Needle and dial share the same centre; overlay at origin (0, 0).
        imageops::overlay(&mut out, &rotated, 0, 0);
    }

    out
}

// ─── D. Protocol cache entries ───────────────────────────────────────────────

/// Cached iTerm2 image escape for one chart widget.
///
/// Charts bypass `ratatui-image` and use a hand-rolled iTerm2 inline-image
/// escape sized in **cells** (`width=<cols>;height=<rows>`, no `px`), so the
/// terminal scales a small transmitted PNG up to fill the panel. `ratatui-image`
/// hard-codes `width=<n>px`, which forces the transmitted image to the panel's
/// full pixel size — ~1656 px on `HiDPI`, nine of which the terminal drops.
struct ChartCache {
    /// `(data_version, area_width_cells, area_height_cells)`.
    /// Invalidated when data changes or the widget area changes size.
    key: (u64, u16, u16),
    /// The full iTerm2 escape sequence (already base64-encoded) for the first cell.
    escape: String,
}

/// Cached terminal-graphics protocol for the compass widget.
struct CompassCache {
    /// `(heading_bucket, calm, area_width_cells, area_height_cells)`.
    /// Heading is bucketed to ~2 ° (× 50, rounded) so we only re-rasterize
    /// on meaningful directional changes, not on ADC noise.  `i64::MIN` is
    /// the sentinel for calm / no-heading (no needle is drawn in either case).
    key: (i64, bool, u16, u16),
    proto: StatefulProtocol,
}

// ─── D. Images state ─────────────────────────────────────────────────────────

/// Image-rendering state: picker, embedded compass assets, and protocol caches.
///
/// PNG assets are decoded once at construction.  Calls to [`Images::draw_chart`]
/// and [`Images::draw_compass`] rebuild the cached [`StatefulProtocol`] only
/// when the data version or widget area changes.
pub struct Images {
    picker: Picker,
    /// Dial decoded from the embedded `assets/compass/compass-dial.png`.
    pub dial: RgbaImage,
    /// Needle decoded from the embedded `assets/compass/compass-needle.png`.
    /// Points North (up) at 0 °; the rasterizer rotates it clockwise.
    pub needle: RgbaImage,
    charts: HashMap<&'static str, ChartCache>,
    compass: Option<CompassCache>,
}

/// Longest-side pixel cap for the *transmitted* chart PNG. The terminal scales
/// this up to fill the panel's cell box, so it need not match the panel's full
/// `HiDPI` pixel size (~1656 px) — nine of those overwhelm the terminal's image
/// budget. This keeps each transmitted texture small while the display fills.
const CHART_MAX_DIM: u32 = 800;

/// PNG-encode `img` and wrap it in an iTerm2 inline-image escape sized in CELLS.
///
/// The key difference from `ratatui-image`: `width`/`height` are given in **cells**
/// (`width=<cols>;height=<rows>`, no `px`), so the terminal scales the transmitted
/// PNG up to fill `cols × rows` cells. We can therefore transmit a small PNG and
/// still fill a `HiDPI` panel. `preserveAspectRatio=0` stretches to the cell box
/// exactly (the PNG already carries the panel's pixel aspect, so no distortion);
/// `doNotMoveCursor=1` keeps the cursor put so ratatui's layout is undisturbed.
fn encode_iterm2_cells(img: &RgbaImage, cols: u16, rows: u16) -> String {
    let mut png: Vec<u8> = Vec::new();
    if DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .is_err()
    {
        return String::new();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    format!(
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=0;doNotMoveCursor=1:{b64}\x07",
        png.len(),
    )
}

/// Write a pre-built terminal-image `escape` into `area` of the buffer.
///
/// Mirrors `ratatui-image`'s integration: the escape goes in the first cell (forced
/// to one column wide so ratatui doesn't pad it), and every other cell in `area` is
/// marked `Skip` so ratatui's diff renderer leaves the image region untouched.
fn write_image_cells(buf: &mut Buffer, area: Rect, escape: &str) {
    if escape.is_empty() {
        return;
    }
    if let Some(cell) = buf.cell_mut((area.x, area.y)) {
        cell.set_symbol(escape)
            .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::MIN));
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x == area.left() && y == area.top() {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_diff_option(CellDiffOption::Skip);
            }
        }
    }
}

/// Append a diagnostic line to the debug log when `METEO_TUI_DEBUG` is set in
/// the environment. No-op otherwise (the TUI owns the terminal, so stderr is
/// unusable). Used to investigate image-protocol rendering on the real host.
fn debug_log(msg: &str) {
    use std::io::Write as _;
    if let Some(path) = std::env::var_os("METEO_TUI_DEBUG")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::path::Path::new(&path))
    {
        writeln!(f, "{msg}").ok();
    }
}

impl Images {
    /// Normal constructor.
    ///
    /// `picker` should come from `Picker::from_query_stdio()` in the real TUI.
    /// `set_background_color` is applied here so that transparent image edges
    /// blend against the MANTLE palette colour.
    pub fn new(mut picker: Picker) -> Self {
        picker.set_background_color(Some(rgba(theme::MANTLE)));

        #[expect(
            clippy::expect_used,
            reason = "embedded PNG bytes are validated at compile time via include_bytes!"
        )]
        let dial = image::load_from_memory(include_bytes!("../assets/compass/compass-dial.png"))
            .expect("embedded compass-dial.png must be a valid PNG")
            .to_rgba8();

        #[expect(
            clippy::expect_used,
            reason = "embedded PNG bytes are validated at compile time via include_bytes!"
        )]
        let needle =
            image::load_from_memory(include_bytes!("../assets/compass/compass-needle.png"))
                .expect("embedded compass-needle.png must be a valid PNG")
                .to_rgba8();

        debug_log(&format!(
            "Images::new protocol={:?} font={:?}",
            picker.protocol_type(),
            picker.font_size(),
        ));

        Self {
            picker,
            dial,
            needle,
            charts: HashMap::new(),
            compass: None,
        }
    }

    /// Test constructor — uses the Halfblocks protocol; no terminal query
    /// needed so tests run headlessly.
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self::new(Picker::halfblocks())
    }

    /// Render a chart image into `area` using the cached protocol.
    ///
    /// `id` is a stable `&'static str` key (e.g. `"temperature"`).
    /// `version` is a monotonically increasing counter bumped by the caller
    /// whenever the underlying data changes; the cache is invalidated on
    /// version change or area resize.
    /// `build(w_px, h_px)` constructs the [`RgbaImage`] when the cache is
    /// cold or stale — it receives the pixel dimensions derived from the font
    /// size so the image fills the area exactly.
    pub fn draw_chart<F>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        id: &'static str,
        version: u64,
        build: F,
    ) where
        F: FnOnce(u32, u32) -> RgbaImage,
    {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let font = self.picker.font_size();
        let raw_w = u32::from(area.width).saturating_mul(u32::from(font.width));
        let raw_h = u32::from(area.height).saturating_mul(u32::from(font.height));
        // Transmit resolution: keep the panel's pixel ASPECT (so the terminal's
        // stretch-to-cells doesn't distort) but cap the longest side. The terminal
        // scales this small PNG up to fill the panel, so it need not be panel-sized.
        let longest = raw_w.max(raw_h).max(1);
        let (w_px, h_px) = if longest > CHART_MAX_DIM {
            (
                raw_w
                    .saturating_mul(CHART_MAX_DIM)
                    .checked_div(longest)
                    .unwrap_or(raw_w)
                    .max(1),
                raw_h
                    .saturating_mul(CHART_MAX_DIM)
                    .checked_div(longest)
                    .unwrap_or(raw_h)
                    .max(1),
            )
        } else {
            (raw_w, raw_h)
        };
        let key = (version, area.width, area.height);

        if self.charts.get(id).is_none_or(|c| c.key != key) {
            let img = build(w_px, h_px);
            let escape = encode_iterm2_cells(&img, area.width, area.height);
            debug_log(&format!(
                "chart[{id}] img={}x{} cells={}x{} esc_bytes={}",
                img.width(),
                img.height(),
                area.width,
                area.height,
                escape.len(),
            ));
            self.charts.insert(id, ChartCache { key, escape });
        }

        if let Some(cache) = self.charts.get(id) {
            write_image_cells(frame.buffer_mut(), area, &cache.escape);
        }
    }

    /// Render the compass widget into `area`.
    ///
    /// The needle is omitted when `calm` is `true` or `heading_deg` is `None`.
    /// Heading is bucketed to ~2 ° so only meaningful directional changes
    /// cause a re-rasterize.
    pub fn draw_compass(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        heading_deg: Option<f64>,
        calm: bool,
    ) {
        let font = self.picker.font_size();
        let w_px = u32::from(area.width).saturating_mul(u32::from(font.width));
        let h_px = u32::from(area.height).saturating_mul(u32::from(font.height));
        let size = w_px.min(h_px);

        // Centre the square dial within the (wider) panel: ratatui-image's Fit
        // aligns top-left, so without this the compass hugs the left edge. Compute
        // the square's cell footprint and offset it to the middle of `area`.
        let fw = u32::from(font.width.max(1));
        let fh = u32::from(font.height.max(1));
        let cells_w = u16::try_from(size.checked_div(fw).unwrap_or(0))
            .unwrap_or(area.width)
            .clamp(1, area.width);
        let cells_h = u16::try_from(size.checked_div(fh).unwrap_or(0))
            .unwrap_or(area.height)
            .clamp(1, area.height);
        let target = ratatui::layout::Rect {
            x: area
                .x
                .saturating_add(area.width.saturating_sub(cells_w) / 2),
            y: area
                .y
                .saturating_add(area.height.saturating_sub(cells_h) / 2),
            width: cells_w,
            height: cells_h,
        };

        // `i64::MIN` sentinel: calm or no heading → no needle → same raster.
        let heading_bucket: i64 = if calm || heading_deg.is_none() {
            i64::MIN
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "bucket = heading×50 is a small integer; i64 can hold all compass values"
            )]
            {
                (heading_deg.unwrap_or(0.0) * 50.0).round() as i64
            }
        };
        let key = (heading_bucket, calm, target.width, target.height);

        if self.compass.as_ref().is_none_or(|c| c.key != key) {
            let heading = if calm { None } else { heading_deg };
            let img = render_compass_image(&self.dial, &self.needle, heading, calm, size);
            let proto = self
                .picker
                .new_resize_protocol(DynamicImage::ImageRgba8(img));
            self.compass = Some(CompassCache { key, proto });
        }

        if let Some(cache) = &mut self.compass {
            frame.render_stateful_widget(
                StatefulImage::<StatefulProtocol>::default(),
                target,
                &mut cache.proto,
            );
            if let Some(r) = cache.proto.last_encoding_result() {
                debug_log(&format!("compass encode={r:?} target={target:?}"));
            }
        }
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "TestResult is the standard test pattern"
)]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use ratatui::style::Color;
    use test_log::test;

    use super::*;
    use crate::{
        plot::{MarkerStyle, PlotSpec},
        theme,
    };

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// Minimal `PlotSpec` with all optional features disabled.
    fn base_spec() -> PlotSpec<'static> {
        PlotSpec {
            title: "test",
            unit: "u",
            color: theme::PEACH,
            prec: 1,
            floor: None,
            scale: 1.0,
            marker: MarkerStyle::Line,
            show_grid: false,
            fill: false,
            overlay: None,
            bars: None,
        }
    }

    // ── A. rgba helper ──────────────────────────────────────────────────────

    #[test]
    fn rgba_maps_rgb() -> TestResult {
        // Given
        let color = Color::Rgb(1, 2, 3);

        // When
        let result = rgba(color);

        // Then
        assert_eq!(result, Rgba([1, 2, 3, 255]));
        Ok(())
    }

    // ── B. Chart rasterizer ─────────────────────────────────────────────────

    #[test]
    fn render_chart_image_dimensions() -> TestResult {
        // Given — five ascending points; grid + fill enabled to exercise more paths
        let spec = PlotSpec {
            show_grid: true,
            fill: true,
            ..base_spec()
        };
        let pts: &[(f64, f64)] = &[
            (0.0, 1.0),
            (100.0, 2.0),
            (200.0, 3.0),
            (300.0, 2.5),
            (400.0, 1.5),
        ];
        let x_win = [0.0_f64, 400.0];
        let y_win = [0.5_f64, 3.5];

        // When
        let img = render_chart_image(&spec, pts, x_win, y_win, 200, 100);

        // Then — correct dimensions
        assert_eq!(img.width(), 200, "image width should be 200");
        assert_eq!(img.height(), 100, "image height should be 100");

        // And — at least some pixels differ from the background (trace was drawn)
        let bg = rgba(theme::MANTLE);
        let has_drawing = img.pixels().any(|&p| p != bg);
        assert!(
            has_drawing,
            "200×100 chart with 5 points should contain non-background pixels"
        );
        Ok(())
    }

    #[test]
    fn render_chart_image_empty_points() -> TestResult {
        // Given — no data; all optional features off so nothing is drawn
        let spec = base_spec(); // show_grid=false, fill=false
        let pts: &[(f64, f64)] = &[];
        let x_win = [0.0_f64, 100.0];
        let y_win = [0.0_f64, 10.0];

        // When
        let img = render_chart_image(&spec, pts, x_win, y_win, 100, 60);

        // Then — correct dimensions, no panic
        assert_eq!(img.width(), 100, "image width should be 100");
        assert_eq!(img.height(), 60, "image height should be 60");

        // And — image is entirely the background colour (nothing was drawn)
        let bg = rgba(theme::MANTLE);
        let all_bg = img.pixels().all(|&p| p == bg);
        assert!(
            all_bg,
            "empty-points chart (show_grid=false, fill=false) should be entirely MANTLE"
        );
        Ok(())
    }

    // ── C. Compass rasterizer ───────────────────────────────────────────────

    #[test]
    fn render_compass_image_dimensions() -> TestResult {
        // Given — use the embedded assets from Images::for_test
        let images = Images::for_test();

        // When — heading East (90°), not calm → needle is overlaid
        let img_with_needle =
            render_compass_image(&images.dial, &images.needle, Some(90.0), false, 128);

        // Then — correct square size
        assert_eq!(
            img_with_needle.width(),
            128,
            "compass should be 128 px wide"
        );
        assert_eq!(
            img_with_needle.height(),
            128,
            "compass should be 128 px tall"
        );

        // And — differs from the calm (needle-less) render
        let calm = render_compass_image(&images.dial, &images.needle, None, true, 128);
        let differs = img_with_needle
            .pixels()
            .zip(calm.pixels())
            .any(|(a, b)| a != b);
        assert!(
            differs,
            "compass with needle (heading=90) should differ from calm (no needle)"
        );
        Ok(())
    }

    #[test]
    fn render_compass_image_calm_is_dial_only() -> TestResult {
        // Given — two calm renders, one with heading and one without
        let images = Images::for_test();

        // When
        let calm_no_heading = render_compass_image(&images.dial, &images.needle, None, true, 64);
        let calm_with_heading =
            render_compass_image(&images.dial, &images.needle, Some(45.0), true, 64);

        // Then — calm=true suppresses the needle regardless of heading_deg
        let identical = calm_no_heading
            .pixels()
            .zip(calm_with_heading.pixels())
            .all(|(a, b)| a == b);
        assert!(
            identical,
            "calm=true should produce identical images regardless of heading_deg"
        );
        Ok(())
    }

    // ── D. Images constructor ───────────────────────────────────────────────

    #[test]
    fn images_new_decodes_assets() -> TestResult {
        // Given / When
        let images = Images::for_test();

        // Then — both PNG assets decoded to non-empty images
        assert!(
            images.dial.width() > 0 && images.dial.height() > 0,
            "dial asset should decode to a non-empty image"
        );
        assert!(
            images.needle.width() > 0 && images.needle.height() > 0,
            "needle asset should decode to a non-empty image"
        );
        Ok(())
    }

    // ── Smoke test for draw_chart ────────────────────────────────────────────

    #[test]
    fn draw_chart_smoke_no_panic() -> TestResult {
        // Given — 40×8 headless terminal, three data points
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let mut images = Images::for_test();
        let spec = base_spec();
        let pts: &[(f64, f64)] = &[(0.0, 1.0), (100.0, 2.0), (200.0, 1.5)];
        let x_win = [0.0_f64, 200.0];
        let y_win = [0.5_f64, 2.5];

        // When / Then — must not panic
        terminal.draw(|f| {
            let area = f.area();
            images.draw_chart(f, area, "smoke", 1, |w, h| {
                render_chart_image(&spec, pts, x_win, y_win, w, h)
            });
        })?;
        Ok(())
    }
}
// grcov exclude stop
