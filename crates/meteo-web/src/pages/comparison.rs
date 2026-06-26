//! Comparison page — overlay independent `(date, metric)` traces on a shared
//! time-of-day X axis, with automatic dual-Y or normalisation when metrics differ.
//!
//! # Axis layout
//!
//! [`axis_layout`] inspects the distinct metrics of the selected traces:
//! - **0–1 distinct metric** → [`AxisLayout::Shared`]: one Y axis, combined bounds.
//! - **Exactly 2 distinct** → [`AxisLayout::DualY`]: left/right axes, one per metric.
//! - **≥3 distinct** → [`AxisLayout::Normalized`]: each trace normalised to 0–1.

// Leptos #[component] macro generates a typed-builder struct whose field names
// shadow the function parameters. Neither shadow is actionable from user code.
#![allow(
    clippy::shadow_reuse,
    reason = "leptos #[component] macro generates param shadows in the builder"
)]
// Component props are owned values consumed at call-site. Leptos does not support
// borrowed props, so the pass-by-value is intentional even when the body only borrows.
#![allow(
    clippy::needless_pass_by_value,
    reason = "leptos component props must be owned"
)]
// SVG domain variables share the `y_` prefix (y_lo, y_hi, y_span, …).
#![allow(
    clippy::similar_names,
    reason = "SVG Y-domain variables share the y_ prefix by design"
)]

use std::fmt::Write as _;

use crate::{
    api::get_comparison_trace,
    components::chart::project,
    types::{Metric, TracePoint},
};
use chrono::NaiveDate;
use leptos::prelude::*;
use meteo_chart::{
    padded_value_bounds,
    palette::{
        BLUE, CRUST, GREEN, LAVENDER, MAUVE, OVERLAY1, PEACH, SAPPHIRE, SKY, SURFACE2, TEAL,
        YELLOW, css,
    },
};

/// Logical SVG canvas width for the comparison chart.
const COMP_W: f64 = 600.0;
/// Logical SVG canvas height for the comparison chart.
const COMP_H: f64 = 240.0;
/// X domain: seconds-of-day 0..86 400.
const X_DOM: [f64; 2] = [0.0, 86_400.0];

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// One overlaid trace selection: a UTC calendar date and a sensor metric.
///
/// `Copy` so it can be moved freely into multiple closures inside `<For>` items.
#[derive(Clone, Copy, PartialEq)]
struct TraceSel {
    date: NaiveDate,
    metric: Metric,
}

/// Y-axis layout for the overlaid comparison traces.
///
/// Derived by [`axis_layout`] from the set of distinct metrics present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisLayout {
    /// All traces share one Y axis (0 or 1 distinct metric).
    Shared,
    /// Exactly 2 distinct metrics — left Y axis for `a`, right for `b`.
    DualY(Metric, Metric),
    /// 3 or more distinct metrics — each trace normalised to its own 0–1 range.
    Normalized,
}

/// Choose the Y-axis layout from the metrics of the overlaid traces.
///
/// Counts distinct metrics in `metrics` in first-seen order:
/// - 0 or 1 distinct → [`AxisLayout::Shared`]
/// - exactly 2 → [`AxisLayout::DualY`] with the two metrics in order
/// - 3 or more → [`AxisLayout::Normalized`]
///
/// The empty slice is treated as 0 distinct metrics and returns `Shared`.
#[must_use]
pub fn axis_layout(metrics: &[Metric]) -> AxisLayout {
    let mut seen: Vec<Metric> = Vec::new();
    for &m in metrics {
        if !seen.contains(&m) {
            seen.push(m);
        }
    }
    match seen.as_slice() {
        [] | [_] => AxisLayout::Shared,
        [a, b] => AxisLayout::DualY(*a, *b),
        _ => AxisLayout::Normalized,
    }
}

/// One reactive trace slot stored in the parent's `traces` signal.
///
/// Fields: `(stable_id, selection, per_trace_data_signal)`.
/// - `stable_id` is the `<For>` key — never reused so scope identity is stable.
/// - `pts_sig` is written by the item's `Effect` when its `Resource` resolves and
///   read by the combined SVG renderer in the parent scope.
type TraceSlot = (usize, TraceSel, RwSignal<Option<Vec<TracePoint>>>);

// ---------------------------------------------------------------------------
// Metric helpers
// ---------------------------------------------------------------------------

/// Catppuccin Mocha CSS hex colour for a sensor metric.
fn metric_color(m: Metric) -> String {
    match m {
        Metric::AirTemp => css(PEACH),
        Metric::Pressure => css(TEAL),
        Metric::Humidity => css(SAPPHIRE),
        Metric::SkyTemp => css(LAVENDER),
        Metric::Lux | Metric::Solar => css(YELLOW),
        Metric::Wind => css(SKY),
        Metric::Rain => css(BLUE),
        Metric::Battery => css(GREEN),
        Metric::Load => css(MAUVE),
    }
}

/// Short French display label for a metric.
const fn metric_label(m: Metric) -> &'static str {
    match m {
        Metric::AirTemp => "Temp. air",
        Metric::Pressure => "Pression",
        Metric::Humidity => "Humidité",
        Metric::SkyTemp => "Temp. ciel",
        Metric::Lux => "Luminosité",
        Metric::Wind => "Vent",
        Metric::Rain => "Pluie",
        Metric::Battery => "Batterie",
        Metric::Solar => "Solaire",
        Metric::Load => "Charge",
    }
}

/// Physical unit label for a metric.
const fn metric_unit(m: Metric) -> &'static str {
    match m {
        Metric::AirTemp | Metric::SkyTemp => "°C",
        Metric::Pressure => "hPa",
        Metric::Humidity | Metric::Battery => "%",
        Metric::Lux => "lx",
        Metric::Wind => "m/s",
        Metric::Rain => "mm/h",
        Metric::Solar | Metric::Load => "W",
    }
}

/// Parse a [`Metric`] from the ASCII key used as `<option value="…">`.
fn metric_from_str(s: &str) -> Metric {
    match s {
        "Pressure" => Metric::Pressure,
        "Humidity" => Metric::Humidity,
        "SkyTemp" => Metric::SkyTemp,
        "Lux" => Metric::Lux,
        "Wind" => Metric::Wind,
        "Rain" => Metric::Rain,
        "Battery" => Metric::Battery,
        "Solar" => Metric::Solar,
        "Load" => Metric::Load,
        _ => Metric::AirTemp,
    }
}

// ---------------------------------------------------------------------------
// SVG rendering
// ---------------------------------------------------------------------------

/// Per-trace data prepared for the comparison SVG renderer.
struct TraceRenderInfo {
    points: Vec<TracePoint>,
    color_hex: String,
    metric: Metric,
    /// Human-readable label: `"YYYY-MM-DD — label (unit)"`.
    label: String,
    /// Y minimum across all points (for [`AxisLayout::Normalized`] legend).
    y_min: f64,
    /// Y maximum across all points (for [`AxisLayout::Normalized`] legend).
    y_max: f64,
}

/// Append a `<polyline>` (≥2 projected points) or `<circle>` (1 point) to `svg`.
fn append_polyline(svg: &mut String, pts: &[(f64, f64)], color: &str) {
    match pts {
        [] => {}
        [(cx, cy)] => {
            write!(
                svg,
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="2" fill="{color}"/>"#
            )
            .ok();
        }
        _ => {
            let s: String = pts
                .iter()
                .map(|&(px, py)| format!("{px:.1},{py:.1}"))
                .collect::<Vec<_>>()
                .join(" ");
            write!(
                svg,
                r#"<polyline points="{s}" fill="none" stroke="{color}" stroke-width="1.5"/>"#
            )
            .ok();
        }
    }
}

/// Compute the padded Y domain for all points whose metric matches `target`.
fn metric_y_dom(infos: &[TraceRenderInfo], target: Metric) -> [f64; 2] {
    let (mn, mx) = infos
        .iter()
        .filter(|i| i.metric == target)
        .flat_map(|i| i.points.iter().map(|p| p.y))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if mn.is_finite() && mx.is_finite() {
        padded_value_bounds(mn, mx, None)
    } else {
        [0.0, 1.0]
    }
}

/// Build the full comparison `<svg>…</svg>` string.
///
/// Renders all traces onto a shared time-of-day X axis (0–86 400 s). Y axes
/// are chosen by `layout`. Returns a bare background when `infos` is empty.
#[allow(
    clippy::too_many_lines,
    reason = "SVG string builder — splitting would obscure the visual structure, analogous to render_svg_chart"
)]
fn render_comparison_svg(infos: &[TraceRenderInfo], layout: AxisLayout) -> String {
    const W: f64 = COMP_W;
    const H: f64 = COMP_H;

    let crust_hex = css(CRUST);
    let grid_hex = css(SURFACE2);
    let label_hex = css(OVERLAY1);

    let mut svg = String::with_capacity(8192);

    write!(
        svg,
        r#"<svg viewBox="0 0 {W} {H}" xmlns="http://www.w3.org/2000/svg" class="plot-svg" preserveAspectRatio="none">"#
    )
    .ok();
    write!(
        svg,
        r#"<rect x="0" y="0" width="{W}" height="{H}" fill="{crust_hex}"/>"#
    )
    .ok();

    // Horizontal gridlines at 25 %, 50 %, 75 %.
    for frac in [0.25_f64, 0.50, 0.75] {
        let yg = H * frac;
        write!(
            svg,
            r#"<line x1="0" y1="{yg:.1}" x2="{W}" y2="{yg:.1}" stroke="{grid_hex}" stroke-dasharray="2,4" stroke-width="0.5"/>"#
        )
        .ok();
    }

    // Vertical gridlines at 6 h, 12 h, 18 h.
    for h_f in [6.0_f64, 12.0, 18.0] {
        let xg = h_f * 3_600.0 / 86_400.0 * W;
        write!(
            svg,
            r#"<line x1="{xg:.1}" y1="0" x2="{xg:.1}" y2="{H}" stroke="{grid_hex}" stroke-dasharray="2,4" stroke-width="0.5"/>"#
        )
        .ok();
    }

    if !infos.is_empty() {
        match layout {
            AxisLayout::Shared => {
                let (y_raw_lo, y_raw_hi) = infos
                    .iter()
                    .flat_map(|i| i.points.iter().map(|p| p.y))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    });
                let ydom = if y_raw_lo.is_finite() && y_raw_hi.is_finite() {
                    padded_value_bounds(y_raw_lo, y_raw_hi, None)
                } else {
                    [0.0, 1.0]
                };
                let [y_lo, y_hi] = ydom;
                let y_mid = f64::midpoint(y_lo, y_hi);

                write!(svg, r#"<text x="2" y="10" fill="{label_hex}" font-size="9" class="font-mono">{y_hi:.1}</text>"#).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">{y_mid:.1}</text>"#, H / 2.0 + 4.0).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">{y_lo:.1}</text>"#, H - 12.0).ok();

                for info in infos {
                    let proj: Vec<(f64, f64)> = info
                        .points
                        .iter()
                        .map(|p| project(p.x, p.y, X_DOM, ydom, W, H))
                        .collect();
                    append_polyline(&mut svg, &proj, &info.color_hex);
                }
            }

            AxisLayout::DualY(metric_a, metric_b) => {
                let ydom_a = metric_y_dom(infos, metric_a);
                let ydom_b = metric_y_dom(infos, metric_b);
                let [a_lo, a_hi] = ydom_a;
                let [b_lo, b_hi] = ydom_b;
                let a_mid = f64::midpoint(a_lo, a_hi);
                let b_mid = f64::midpoint(b_lo, b_hi);

                // Left axis labels (metric A).
                write!(svg, r#"<text x="2" y="10" fill="{label_hex}" font-size="9" class="font-mono">{a_hi:.1}</text>"#).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">{a_mid:.1}</text>"#, H / 2.0 + 4.0).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">{a_lo:.1}</text>"#, H - 12.0).ok();
                // Right axis labels (metric B, text-anchor end).
                write!(svg, r#"<text x="{:.1}" y="10" fill="{label_hex}" font-size="9" class="font-mono" text-anchor="end">{b_hi:.1}</text>"#, W - 2.0).ok();
                write!(svg, r#"<text x="{:.1}" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono" text-anchor="end">{b_mid:.1}</text>"#, W - 2.0, H / 2.0 + 4.0).ok();
                write!(svg, r#"<text x="{:.1}" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono" text-anchor="end">{b_lo:.1}</text>"#, W - 2.0, H - 12.0).ok();

                for info in infos {
                    let ydom = if info.metric == metric_a {
                        ydom_a
                    } else {
                        ydom_b
                    };
                    let proj: Vec<(f64, f64)> = info
                        .points
                        .iter()
                        .map(|p| project(p.x, p.y, X_DOM, ydom, W, H))
                        .collect();
                    append_polyline(&mut svg, &proj, &info.color_hex);
                }
            }

            AxisLayout::Normalized => {
                let ydom = [0.0_f64, 1.0_f64];
                write!(svg, r#"<text x="2" y="10" fill="{label_hex}" font-size="9" class="font-mono">1.0</text>"#).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">0.5</text>"#, H / 2.0 + 4.0).ok();
                write!(svg, r#"<text x="2" y="{:.1}" fill="{label_hex}" font-size="9" class="font-mono">0.0</text>"#, H - 12.0).ok();

                for info in infos {
                    let y_span = info.y_max - info.y_min;
                    let y_lo = info.y_min;
                    let proj: Vec<(f64, f64)> = info
                        .points
                        .iter()
                        .map(|p| {
                            let ny = if y_span.abs() < f64::EPSILON {
                                0.5
                            } else {
                                (p.y - y_lo) / y_span
                            };
                            project(p.x, ny, X_DOM, ydom, W, H)
                        })
                        .collect();
                    append_polyline(&mut svg, &proj, &info.color_hex);
                }
            }
        }
    }

    // X-axis time labels (bottom of canvas).
    for (h_i, label) in [(0_i32, "0h"), (6, "6h"), (12, "12h"), (18, "18h")] {
        let xg = f64::from(h_i) * 3_600.0 / 86_400.0 * W;
        write!(
            svg,
            r#"<text x="{xg:.1}" y="{:.1}" fill="{label_hex}" font-size="8" class="font-mono">{label}</text>"#,
            H - 1.0
        )
        .ok();
    }
    write!(
        svg,
        r#"<text x="{W:.1}" y="{:.1}" fill="{label_hex}" font-size="8" class="font-mono" text-anchor="end">24h</text>"#,
        H - 1.0
    )
    .ok();

    svg.push_str("</svg>");
    svg
}

/// Build a [`TraceRenderInfo`] from a fetched `(TraceSel, Vec<TracePoint>)` pair.
fn make_render_info(sel: TraceSel, points: Vec<TracePoint>) -> TraceRenderInfo {
    let y_min = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let y_max = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    let label = format!(
        "{} — {} ({})",
        sel.date.format("%Y-%m-%d"),
        metric_label(sel.metric),
        metric_unit(sel.metric),
    );
    TraceRenderInfo {
        points,
        color_hex: metric_color(sel.metric),
        metric: sel.metric,
        label,
        y_min,
        y_max,
    }
}

// ---------------------------------------------------------------------------
// Page component
// ---------------------------------------------------------------------------

/// Comparison page — overlay independent `(date, metric)` traces on a shared
/// time-of-day X axis.
///
/// Route: `/comparaison`.
///
/// Each trace in the list has its **own** [`Resource`] calling
/// [`get_comparison_trace`] — changing or adding one trace only refetches that
/// trace, not the others. The loaded data is written into a per-trace
/// [`RwSignal`] (see [`TraceSlot`]), and the combined SVG reads all per-trace
/// signals reactively so any individual load or update propagates to the chart.
///
/// - Lists selected `(date, metric)` pairs, each with a remove button and a
///   loading indicator (⟳) while its Resource is still pending.
/// - Form to add a new trace: date picker + metric `<select>` + "Ajouter".
/// - Single SVG chart using all resolved traces with automatic Y-axis layout.
#[allow(
    clippy::too_many_lines,
    reason = "Leptos view! macro expands inline HTML; splitting would fragment the reactive signal graph"
)]
#[component]
pub fn ComparisonPage() -> impl IntoView {
    let today = chrono::Utc::now().date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();

    // Monotonically increasing ID counter — provides stable <For> keys.
    let next_id = StoredValue::new(1_usize);

    // All trace slots. The first is seeded with today's air temperature.
    // Each slot carries its own per-trace data signal (updated by the
    // slot's Effect when its Resource resolves).
    let traces: RwSignal<Vec<TraceSlot>> = RwSignal::new(vec![(
        0,
        TraceSel {
            date: today,
            metric: Metric::AirTemp,
        },
        RwSignal::new(None),
    )]);

    // Form state for the next trace to add.
    let new_date_str: RwSignal<String> = RwSignal::new(today_str.clone());
    let new_metric: RwSignal<Metric> = RwSignal::new(Metric::AirTemp);

    // Add a new trace slot with its own data signal and a fresh stable ID.
    let add_trace = move |_| {
        let date_str = new_date_str.get();
        let metric = new_metric.get();
        if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            let id = next_id.get_value();
            next_id.update_value(|n| *n = n.saturating_add(1));
            let pts_sig: RwSignal<Option<Vec<TracePoint>>> = RwSignal::new(None);
            traces.update(|v| v.push((id, TraceSel { date, metric }, pts_sig)));
        }
    };

    view! {
        <div class="content-area">
            <h1 class="font-mono color-peach">"Comparaison"</h1>

            // Trace list — each item owns its own Resource + Effect.
            <div class="trace-sel-list">
                <For
                    each=move || traces.get()
                    key=|slot: &TraceSlot| slot.0
                    children=move |slot: TraceSlot| {
                        let (id, sel, pts_sig) = slot;

                        // Per-trace Resource: keyed only on this trace's (date, metric).
                        // Changing another trace does NOT cause this Resource to refetch.
                        let resource: Resource<Vec<TracePoint>> = Resource::new(
                            move || (sel.date, sel.metric),
                            |(date, metric)| async move {
                                get_comparison_trace(
                                    date.format("%Y-%m-%d").to_string(),
                                    metric,
                                )
                                .await
                                .unwrap_or_default()
                            },
                        );

                        // When the Resource resolves, write into the per-trace signal
                        // so the combined SVG (which reads all `pts_sig`s) updates.
                        Effect::new(move |_| {
                            if let Some(pts) = resource.get() {
                                pts_sig.set(Some(pts));
                            }
                        });

                        // Row UI — colour dot, label, loading indicator, remove button.
                        let color = metric_color(sel.metric);
                        let label = format!(
                            "{} — {} ({})",
                            sel.date.format("%Y-%m-%d"),
                            metric_label(sel.metric),
                            metric_unit(sel.metric),
                        );
                        view! {
                            <div class="trace-sel-row">
                                <span style=format!("color:{color};")>"■ "</span>
                                <span class="color-subtext">{label}</span>
                                // Loading indicator: visible while Resource hasn't resolved yet.
                                {move || {
                                    resource.get().is_none().then_some(
                                        view! { <span class="color-subtext">" ⟳"</span> }
                                    )
                                }}
                                <button
                                    class="pz-btn"
                                    on:click=move |_| {
                                        traces.update(|v| v.retain(|s| s.0 != id));
                                    }
                                >"×"</button>
                            </div>
                        }
                    }
                />
            </div>

            // Add-trace form.
            <div class="trace-add-form">
                <input
                    type="date"
                    value=today_str
                    on:change=move |ev| new_date_str.set(event_target_value(&ev))
                />
                <select on:change=move |ev| {
                    new_metric.set(metric_from_str(&event_target_value(&ev)));
                }>
                    <option value="AirTemp">"Temp. air (°C)"</option>
                    <option value="Pressure">"Pression (hPa)"</option>
                    <option value="Humidity">"Humidité (%)"</option>
                    <option value="SkyTemp">"Temp. ciel (°C)"</option>
                    <option value="Lux">"Luminosité (lx)"</option>
                    <option value="Wind">"Vent (m/s)"</option>
                    <option value="Rain">"Pluie (mm/h)"</option>
                    <option value="Battery">"Batterie (%)"</option>
                    <option value="Solar">"Solaire (W)"</option>
                    <option value="Load">"Charge (W)"</option>
                </select>
                <button class="preset-btn" on:click=add_trace>"Ajouter"</button>
            </div>

            // Combined SVG — reactive to ALL per-trace signals.
            // Re-renders only when any individual trace's data changes or the
            // trace list itself changes; each per-trace change triggers only a
            // local signal update, not a full re-fetch of all traces.
            {move || {
                let slot_list = traces.get();
                let infos: Vec<TraceRenderInfo> = slot_list
                    .iter()
                    .map(|(_, sel, pts_sig)| {
                        // Reads pts_sig — subscribes to this per-trace signal.
                        let pts = pts_sig.get().unwrap_or_default();
                        make_render_info(*sel, pts)
                    })
                    .collect();
                let metrics: Vec<Metric> = infos.iter().map(|i| i.metric).collect();
                let layout = axis_layout(&metrics);

                // Colour-tagged legend: one entry per trace.
                let legend_html: String = infos
                    .iter()
                    .map(|i| {
                        format!(
                            r#"<span style="color:{};">■</span> {}"#,
                            i.color_hex, i.label,
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  |  ");

                // For Normalized mode: per-trace actual value ranges.
                let norm_html: Option<String> =
                    (layout == AxisLayout::Normalized).then(|| {
                        infos
                            .iter()
                            .map(|i| {
                                format!(
                                    r#"<span style="color:{};">■</span> {}: [{:.1}, {:.1}] {}"#,
                                    i.color_hex,
                                    i.label,
                                    i.y_min,
                                    i.y_max,
                                    metric_unit(i.metric),
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("<br/>")
                    });

                let svg = render_comparison_svg(&infos, layout);

                view! {
                    <div>
                        <div class="plot-svg-outer" inner_html=svg />
                        <div class="color-subtext font-mono" inner_html=legend_html />
                        {norm_html.map(|html| view! {
                            <div class="color-subtext" inner_html=html />
                        })}
                    </div>
                }
            }}
        </div>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(all(test, feature = "ssr"))]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::*;
    use crate::types::Metric;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // ── axis_layout ────────────────────────────────────────────────────────

    /// Empty slice → Shared (no panic, no out-of-bounds).
    #[test]
    fn axis_layout_empty_is_shared() -> TestResult {
        // Given / When
        let layout = axis_layout(&[]);

        // Then
        assert_eq!(layout, AxisLayout::Shared);
        Ok(())
    }

    /// 1 distinct → Shared; exactly 2 distinct → DualY; ≥3 distinct → Normalized.
    #[test]
    fn axis_layout_picks_shared_dual_normalized() -> TestResult {
        // Given / When / Then — 1 distinct metric (repeated) → Shared
        assert_eq!(
            axis_layout(&[Metric::AirTemp, Metric::AirTemp]),
            AxisLayout::Shared,
            "repeated single metric should be Shared"
        );

        // Single entry → Shared
        assert_eq!(axis_layout(&[Metric::Pressure]), AxisLayout::Shared);

        // Exactly 2 distinct → DualY (first-seen order preserved)
        assert_eq!(
            axis_layout(&[Metric::AirTemp, Metric::Pressure, Metric::AirTemp]),
            AxisLayout::DualY(Metric::AirTemp, Metric::Pressure),
            "2 distinct metrics should yield DualY in first-seen order"
        );

        // 3 or more distinct → Normalized
        assert_eq!(
            axis_layout(&[Metric::AirTemp, Metric::Pressure, Metric::Humidity]),
            AxisLayout::Normalized,
            "3 distinct metrics should yield Normalized"
        );
        assert_eq!(
            axis_layout(&[
                Metric::AirTemp,
                Metric::Pressure,
                Metric::Humidity,
                Metric::Lux
            ]),
            AxisLayout::Normalized,
            "4 distinct metrics should yield Normalized"
        );

        Ok(())
    }

    // ── comparison_trace_x_is_seconds_of_day ──────────────────────────────

    fn empty_bucket(bucket_ts: i64) -> crate::db::BucketRow {
        crate::db::BucketRow {
            bucket_ts,
            temp_min: None,
            temp_max: None,
            temp_avg: None,
            pressure_min: None,
            pressure_max: None,
            pressure_avg: None,
            humidity_min: None,
            humidity_max: None,
            humidity_avg: None,
            sky_min: None,
            sky_max: None,
            sky_avg: None,
            lux_min: None,
            lux_max: None,
            lux_avg: None,
            wind_min: None,
            wind_max: None,
            wind_avg: None,
            wind_dir_avg: None,
            rain_avg: None,
            rain_max: None,
            battery_avg: None,
            solar_mv_avg: None,
            solar_ma_avg: None,
            batt_mv_avg: None,
            load_ma_avg: None,
            sample_count: 1,
        }
    }

    /// A bucket at 13:00:00 UTC must map to `x = 46 800` seconds-of-day.
    #[test]
    fn comparison_trace_x_is_seconds_of_day() -> TestResult {
        use rusqlite::Connection;

        // Given — in-memory DB with schema; one bucket at 2024-01-15 13:00:00 UTC
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(include_str!("../db/schema.sql"))?;

        // 2024-01-15 00:00:00 UTC = unix 1 705 276 800
        let day_start: i64 = 1_705_276_800;
        // 13:00:00 = 13 × 3600 = 46 800 s into the day
        let bucket_ts = day_start + 46_800;

        let row = crate::db::BucketRow {
            bucket_ts,
            temp_avg: Some(20.0),
            ..empty_bucket(bucket_ts)
        };
        crate::db::store_bucket_impl(&conn, &row)?;

        // When
        let date = NaiveDate::parse_from_str("2024-01-15", "%Y-%m-%d")?;
        let pts = crate::db::comparison_impl(&conn, date, Metric::AirTemp)?;

        // Then
        assert_eq!(pts.len(), 1, "expected exactly one TracePoint");
        assert!(
            (pts[0].x - 46_800.0).abs() < 1e-9,
            "x must be 46 800 s (13:00 UTC), got {}",
            pts[0].x
        );
        assert!(
            (pts[0].y - 20.0).abs() < 1e-9,
            "y must be 20.0 °C, got {}",
            pts[0].y
        );

        Ok(())
    }
}
// grcov exclude stop
