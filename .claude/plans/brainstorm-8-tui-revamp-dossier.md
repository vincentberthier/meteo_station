# Plan: Complete TUI revamp to the design dossier

- **Source:** '8 (`.claude/brainstorm/8-tui-revamp-dossier.md`)
- **Date:** 2026-06-26
- **Status:** Planned

## Summary

Rewrite `crates/meteo-tui` so the live dashboard reproduces
`docs/Meteo Station - Dossier.dc.html` — its four-tier layout (Header → Summary
band of three cards → Diagnostics bar → History: 6-plot sensors grid + 3-plot
power grid), its Catppuccin Mocha palette (every colour a `Color::Rgb`), its
French verbatim strings, and its dynamic rules. The pure-formatter + `Series` +
reducer architecture is kept and extended, never discarded; `meteo-lib`'s
frame/decode layer is untouched. New work: a Catppuccin `theme` module, a
Canvas-based plot primitive (braille/line markers, gradient fill, dotted
gridlines, gust overlay, dual-axis rain bars), a Canvas wind compass, host-side
derived metrics (RSSI, station name, dew point, 60 s gust, 10-min trend, power
series), and a 10 Hz display redraw for the « En direct » pulse and the fading
heading trail. Three options (`markerStyle`, `showGrid`, `gustTrail`) are exposed
as CLI flags with defaults (dots / on / on).

## Confirmed decisions (from the planning Q&A)

1. **Plot fidelity:** custom **Canvas** plot primitive (full dossier fidelity:
   gradient fill 13 %→0, dotted gridlines at 25/50/75 %, gust overlay @32 %, rain
   bars on their own scale). Not the simpler `Chart` path.
2. **Animation:** **10 Hz** display redraw (≈100 ms tick) — a display cadence,
   not a readiness sleep (same exemption the existing 1 Hz clock documents).
   Drives the 1.6 s « En direct » pulse and the ~60 s trail fade.
3. **Options:** `--marker-style` (default `dots`), `--show-grid` (default on),
   `--gust-trail` (default on) clap flags alongside the existing `--address`.

## Known bug to fix in the rewrite — chart truncation (~minute 6)

The current dashboard's charts only ever fill from ~−6 min to `now`; the left
~40 % stays empty even after the session runs past 10 minutes. **Root cause
(diagnosed, not guessed):**

- The firmware advertises with `AdvertisementParameters::default()` →
  `interval_min = interval_max = 160 ms` (trouble-host default,
  `third_party/trouble-host/src/advertise.rs:110`). The radio emits an
  advertising PDU ~6×/second; the _payload_ changes only once per second (when
  `uptime_s` increments).
- The TUI scans with `duplicate_data: true` and pushes a `Series` point on
  **every** BlueZ event (`app.rs:apply`). BlueZ re-emits
  `DeviceAdded`/`PropertiesChanged` faster than 1 Hz (RSSI churns every PDU), so
  each chart is sampled at **>1 point/second** (~1.6 Hz observed).
- `Series` evicts by **count** (`DEFAULT_CAP = 600`) while the x-axis window is
  by **time** (`WINDOW_SECS = 600 s`). Once the push rate exceeds 1 Hz, 600
  points span **less than 600 seconds** of wall-clock, so the trace never reaches
  the left edge. At ~1.6 Hz, 600 points ≈ 375 s ≈ **6.25 min** — the observed
  truncation.

**Fix (folded into §4):** dedupe series pushes by `uptime_s` in the reducer — one
sample per device-second. `latest` / `last_frame_at` / `rssi` / `station` still
update on every event (responsive instantaneous values + link liveness), but the
charts, the derived series, and « échantillons » advance only on a _distinct_
frame. That restores 600 points = 600 s = a full window and matches the dossier's
1 s sampling. The count/time invariant is documented on `Series` so the trap
cannot silently return.

## Module structure (target)

```
crates/meteo-tui/src/
├── main.rs        # CLI (4 flags), 10 Hz redraw loop, Options, quit keys
├── ble.rs         # passive scan; enrich event with rssi + station alias
├── app.rs         # AppState: +rssi/station/frame_count, +5 Series, +heading trail
├── model.rs       # pure: existing fmt_* kept; +dew point, +FR rose, +trend,
│                  #        +Series window helpers, +power/flow/klx formatters
├── theme.rs       # NEW Catppuccin Mocha Color::Rgb consts + threshold→colour
│                  #        helpers + blend_rgb (gradient/trail alpha)
├── plot.rs        # NEW Canvas plot primitive (markers, fill, grid, overlay, bars)
├── compass.rs     # NEW Canvas wind compass (rings, ticks, needle, trail, readout)
└── ui/
    ├── mod.rs     # render() orchestrator, Options, 4-tier layout, degrade
    ├── header.rs  # identity / clock / link-state band
    ├── summary.rs # 3 cards: ATMOSPHÈRE · VENT (compass) · ÉNERGIE
    ├── diagnostics.rs # sensor chips · RSSI chip · actif/échantillons/dernier paquet
    └── history.rs # CAPTEURS 6-plot grid + ÉNERGIE 3-plot grid
```

`ui.rs` becomes `ui/mod.rs`. `theme`, `plot`, `compass` are top-level modules
declared in `main.rs`; `header/summary/diagnostics/history` are submodules of
`ui` declared in `ui/mod.rs`.

## Files Modified

| File                                     | Action | Description                                                                                                            |
| ---------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------- |
| `crates/meteo-tui/src/theme.rs`          | create | Catppuccin Mocha palette, threshold→colour helpers, `blend_rgb`                                                        |
| `crates/meteo-tui/src/model.rs`          | modify | +dew point, +FR 16-pt rose, +`Trend`/`classify_trend`, +`Series::window_max`/`trend_delta`, +klx/power/flow formatters |
| `crates/meteo-tui/src/ble.rs`            | modify | Read `rssi()` + `alias()`; carry on a new `FrameEvent`                                                                 |
| `crates/meteo-tui/src/app.rs`            | modify | +`rssi`/`station`/`frame_count`, +`gust`/`heading`/`batt_v`/`solar_w`/`load_w` Series, reducer derivations             |
| `crates/meteo-tui/src/plot.rs`           | create | Canvas plot primitive + pure coord-mapping helpers                                                                     |
| `crates/meteo-tui/src/compass.rs`        | create | Canvas compass + pure geometry helpers                                                                                 |
| `crates/meteo-tui/src/ui.rs`             | delete | replaced by `ui/` module dir                                                                                           |
| `crates/meteo-tui/src/ui/mod.rs`         | create | render orchestrator, `Options`, layout, degrade, smoke tests                                                           |
| `crates/meteo-tui/src/ui/header.rs`      | create | Header band                                                                                                            |
| `crates/meteo-tui/src/ui/summary.rs`     | create | Atmosphere / Wind / Power cards                                                                                        |
| `crates/meteo-tui/src/ui/diagnostics.rs` | create | Diagnostics bar                                                                                                        |
| `crates/meteo-tui/src/ui/history.rs`     | create | Sensors + Power grids                                                                                                  |
| `crates/meteo-tui/src/main.rs`           | modify | CLI flags, `Options`, 10 Hz loop, module decls                                                                         |

## Reference constants (pin these verbatim)

**Catppuccin Mocha (`theme.rs`, all `Color::Rgb`):**

| Const         | Hex       | Role                                        |
| ------------- | --------- | ------------------------------------------- |
| `BASE`        | `#1e1e2e` | app background                              |
| `MANTLE`      | `#181825` | panel fill                                  |
| `CRUST`       | `#11111b` | tracks/wells (gauge, compass hub)           |
| `BORDER`      | `#2a2a3c` | panel outline                               |
| `SURFACE0`    | `#313244` | chip frame · separators                     |
| `HAIRLINE`    | `#26263a` | internal dotted rules                       |
| `SURFACE1`    | `#45475a` | minor compass ticks                         |
| `SURFACE2`    | `#585b70` | X axis · faint strokes                      |
| `TEXT`        | `#cdd6f4` | values · clock                              |
| `SUBTEXT1`    | `#bac2de` | sensor names                                |
| `SUBTEXT0`    | `#a6adc8` | panel titles · cardinals                    |
| `OVERLAY2`    | `#9399b2` | section labels · units · dew point          |
| `OVERLAY1`    | `#7f849c` | min/max axes · « maint. »                   |
| `OVERLAY0`    | `#6c7086` | dimmed labels                               |
| `PEACH`       | `#fab387` | air temperature                             |
| `LAVENDER`    | `#b4befe` | sky temperature                             |
| `TEAL`        | `#94e2d5` | pressure · battery gauge · gust             |
| `SAPPHIRE`    | `#74c7ec` | humidity                                    |
| `YELLOW`      | `#f9e2af` | luminosity · solar · warn                   |
| `BLUE`        | `#89b4fa` | rain                                        |
| `SKY`         | `#89dceb` | wind · compass                              |
| `GREEN`       | `#a6e3a1` | battery · charging · OK · north-marker base |
| `MAUVE`       | `#cba6f7` | load (draw)                                 |
| `RED`         | `#f38ba8` | fault · North marker                        |
| `NEEDLE_TAIL` | `#3d4d5e` | compass needle tail                         |

**Thresholds (tunable; dossier §3 starting points):**

- Battery SoC: `≥50` GREEN · `20..=49` YELLOW · `<20` RED.
- RSSI dBm: `≥-70` GREEN · `-90..=-71` YELLOW · `<-90` RED.
- Last-packet age: `<2 s` GREEN · `2..=10 s` YELLOW · `>10 s` RED.
- Trend stable band: `|Δ| < 0.1 °C` over 10 min.
- Calm wind: `< 0.3 m/s` → hide needle, show « calme ».
- Gradient fill peak alpha `0.13`; gust overlay alpha `0.32`; gridline alpha `0.18`.

**French strings (verbatim):** « En direct » · « Hors ligne » · « ATMOSPHÈRE »
· « VENT » · « ÉNERGIE » · « DIAGNOSTIC » · « CAPTEURS » · « Air » · « Humidité »
· « Pression » · « Temp. ciel » · « Lumin. » · « Pluie » · « Pt rosée »
· « Solaire » · « Batterie » · « Charge » · « actif » · « échantillons »
· « dernier paquet » · « ok » · « alerte » · « panne » · « rafale » · « calme »
· « en charge » · « décharge » · « stable » · « maint. » · « Température air »
· « Température ciel » · « Luminosité » · « Vitesse du vent » · « Humidité / Pluie »
· « Puissance solaire » · « Puissance charge ». Units: `%HR` `hPa` `°C` `klx`
`mm/h` `V` `W` `mA`. FR 16-pt rose: `N NNE NE ENE E ESE SE SSE S SSO SO OSO O ONO NO NNO`.

## Plan

### 1. Theme module — Catppuccin Mocha palette + colour helpers

**File:** `crates/meteo-tui/src/theme.rs` (create); declared `mod theme;` in `main.rs`.

Pure colour module. All palette entries as `pub const NAME: Color = Color::Rgb(r,g,b);`
using the hex table above. Plus threshold→colour helpers and an alpha blend used
by the gradient fill and the trail fade.

**Signatures:**

```rust
use ratatui::style::Color;

// Palette consts (one per row of the table) ...
pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
// ... etc.

/// Battery state-of-charge → gauge/percent colour (≥50 green / 20–49 yellow / <20 red).
#[must_use] pub fn battery_color(pct: u8) -> Color;

/// BLE RSSI dBm → chip colour (≥−70 green / −90..=−71 yellow / <−90 red).
#[must_use] pub fn rssi_color(dbm: i16) -> Color;

/// Last-packet age → colour (<2 s green / 2..=10 s yellow / >10 s red).
#[must_use] pub fn packet_age_color(age_secs: f64) -> Color;

/// Linear per-channel blend `fg*a + bg*(1-a)`, `a` clamped to [0,1].
/// Used for the gradient fill (alpha toward BASE) and the heading-trail fade.
/// Returns `Color::Rgb`. Non-`Rgb` inputs fall back to `fg`.
#[must_use] pub fn blend_rgb(fg: Color, bg: Color, a: f64) -> Color;
```

**Code sketch (`blend_rgb`):**

```rust
pub fn blend_rgb(fg: Color, bg: Color, a: f64) -> Color {
    let a = a.clamp(0.0, 1.0);
    let (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) = (fg, bg) else { return fg; };
    let mix = |f: u8, b: u8| -> u8 {
        // f*a + b*(1-a), rounded; inputs are u8 so no overflow after clamp.
        (f64::from(f) * a + f64::from(b) * (1.0 - a)).round() as u8
    };
    Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
}
```

(`as u8` is sound here — value is in `[0,255]` by construction; add
`#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "...")]`.)

**Tests** (`theme::tests`):

- `battery_color_thresholds` — `battery_color(50)==GREEN`, `(49)==YELLOW`,
  `(20)==YELLOW`, `(19)==RED`.
- `rssi_color_thresholds` — `-70→GREEN`, `-81→YELLOW`, `-91→RED`.
- `packet_age_color_thresholds` — `1.9→GREEN`, `5.0→YELLOW`, `10.5→RED`.
- `blend_rgb_endpoints_and_midpoint` — `a=1.0`→`fg`; `a=0.0`→`bg`; `a=0.5`
  of `#000000`/`#ffffff` → `Color::Rgb(128,128,128)` (128 from rounding).
- `blend_rgb_clamps_out_of_range` — `a=2.0` behaves as `1.0`.

**Depends on:** nothing. **Blocks:** 5, 6, 7, 8, 9, 10.

---

### 2. Model extensions — derived metrics + French formatters

**File:** `crates/meteo-tui/src/model.rs` (modify). Keep every existing `fmt_*`,
`Series`, `SignalState`, axis helper and its tests intact. Add:

**Signatures:**

```rust
/// Dew point in °C (Magnus/WMO: a=17.62, b=243.12 °C).
/// `Td = b·γ / (a−γ)` with `γ = ln(rh/100) + a·t/(b+t)`.
/// `rh` clamped to (0,100]; returns `f32`.
#[must_use] pub fn dew_point_c(temp_c: f32, rh_pct: f32) -> f32;

/// French 16-point compass label (0°=N, 90°=E … same bucketing as `compass_label`).
/// Returns one of `N NNE NE ENE E ESE SE SSE S SSO SO OSO O ONO NO NNO`.
#[must_use] pub fn compass_label_fr(deg: f32) -> &'static str;

/// 10-min air-temp trend classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Trend { Rising, Falling, Stable }
/// `Stable` if `|delta| < eps`, else `Rising`/`Falling` by sign.
#[must_use] pub fn classify_trend(delta: f64, eps: f64) -> Trend;

/// Luminosity rendered in kilolux: `"{klx:.1} klx"`, `"N/A"` for None.
#[must_use] pub fn fmt_lux_klx(lux: Option<f32>) -> String;

/// Power in watts from bus mV × current mA: `(mv/1000)*(ma/1000)`.
/// `None` if either input is `None`.
#[must_use] pub fn power_w(mv: Option<u16>, ma: Option<u16>) -> Option<f64>;

/// Battery flow status line for the ÉNERGIE card.
/// `net = solar_w − load_w`. Returns the rendered line:
///  net>0 → "▲ en charge · +{net:.1} W"; net<0 → "▼ décharge · {net:.1} W · ~{h:.1} h";
///  net≈0 → "— stable". `pct` + `load_w` size the discharge autonomy via `BATTERY_WH`.
#[must_use] pub fn fmt_battery_flow(solar_w: Option<f64>, load_w: Option<f64>, pct: Option<u8>) -> String;

/// Nominal 1S-LiPo energy budget for the crude autonomy estimate (best-effort).
pub const BATTERY_WH: f64 = 9.6; // 3.7 V × 2.6 Ah
```

Add two `Series` window helpers (used for gust and trend), next to `x_window`,
and extend the existing `DEFAULT_CAP` doc-comment with the count/time invariant:

```rust
impl Series {
    /// Max value among points whose timestamp is within `window_secs` of the
    /// latest point. `None` if empty. Drives the 60 s gust.
    #[must_use] pub fn window_max(&self, window_secs: f64) -> Option<f64>;

    /// `latest_value − value_of_oldest_point_within(window_secs)`.
    /// `None` if empty. Drives the 10-min trend arrow.
    #[must_use] pub fn trend_delta(&self, window_secs: f64) -> Option<f64>;
}
```

> **Invariant to document on `DEFAULT_CAP`/`WINDOW_SECS`:** the count cap and the
> time window must agree at the feed rate — `DEFAULT_CAP` (600) points must cover
> `WINDOW_SECS` (600 s) of wall-clock. This holds **only** if the producer pushes
> at ≤1 Hz; the §4 `uptime_s` dedup is what guarantees that. Add a one-line note
> on both consts pointing at the dedup so the truncation trap cannot return
> silently.

**Code sketch (`dew_point_c`, `window_max`):**

```rust
pub fn dew_point_c(temp_c: f32, rh_pct: f32) -> f32 {
    const A: f32 = 17.62; const B: f32 = 243.12;
    let rh = rh_pct.clamp(0.01, 100.0) / 100.0;
    let gamma = rh.ln() + (A * temp_c) / (B + temp_c);
    B * gamma / (A - gamma)
}

pub fn window_max(&self, window_secs: f64) -> Option<f64> {
    let last_t = self.points.back()?.0;
    self.points.iter()
        .filter(|(t, _)| *t >= last_t - window_secs)
        .map(|(_, v)| *v)
        .fold(None, |acc, v| Some(acc.map_or(v, |m: f64| m.max(v))))
}
```

(`window_max`/`trend_delta` iterate `self.points` directly — no `make_contiguous`,
so they stay `&self`.)

**Tests** (add to `model::tests`):

- `dew_point_known_value` — `dew_point_c(20.0, 50.0)` ≈ `9.3 °C` (assert within `0.3`).
- `dew_point_saturated_equals_temp` — `dew_point_c(15.0, 100.0)` ≈ `15.0` (within `0.05`).
- `compass_label_fr_cardinals_and_west_is_o` — `0→"N"`, `90→"E"`, `180→"S"`,
  `270→"O"`, `202.5→"SSO"`, `337.5→"NNO"`.
- `classify_trend_bands` — `(0.05,0.1)→Stable`, `(0.3,0.1)→Rising`, `(-0.3,0.1)→Falling`.
- `fmt_lux_klx_divides_by_1000` — `Some(3426.0)→"3.4 klx"`, `None→"N/A"`.
- `power_w_multiplies` — `power_w(Some(15_000),Some(600))≈Some(9.0)`;
  `power_w(None,Some(600))==None`. (Sentinel handling is upstream: `u16::MAX`
  decodes to `None` in `Telemetry::decode`, so `power_w`'s `Option` inputs never
  see the sentinel — `power_w(None, _) == None` is the only sentinel path and is
  covered here.)
- `fmt_battery_flow_charging_and_discharging` — solar>load → starts `"▲ en charge"`;
  load>solar → starts `"▼ décharge"` and contains `"h"`.
- `series_window_max_only_within_window` — push `(0,5),(10,9),(70,3)` with last_t=70,
  window 60 → max over `t≥10` = `9.0`.
- `series_window_max_empty_is_none` — empty `Series` → `window_max(60.0) == None`.
- `series_trend_delta_uses_oldest_in_window` — push `(0,10),(600,12)`, window 600 →
  `2.0`.
- `series_trend_delta_empty_is_none` — empty `Series` → `trend_delta(600.0) == None`.

**Depends on:** nothing. **Blocks:** 6, 7, 8, 9, 10.

---

### 3. BLE event enrichment — RSSI + station name

**File:** `crates/meteo-tui/src/ble.rs` (modify).

`bluer::Device` exposes `async fn rssi(&self) -> Result<Option<i16>>`,
`async fn alias(&self) -> Result<String>` (falls back to the advertised name),
and `async fn name(&self) -> Result<Option<String>>` (verified in
`bluer-0.17.4/src/device.rs`). Read RSSI + alias alongside `manufacturer_data()`
and carry them on the event.

`BleEvent::Frame(Telemetry)` is `Copy`; carrying a `String` breaks that. Replace
the payload with a non-`Copy` `FrameEvent`:

**Signatures:**

```rust
#[derive(Debug, Clone)]
pub struct FrameEvent {
    pub telemetry: Telemetry,
    pub rssi: Option<i16>,
    pub station: Option<String>,
}
impl FrameEvent {
    /// Frame-only constructor (rssi/station = None) — used by tests and the
    /// decode helper.
    #[must_use] pub fn new(telemetry: Telemetry) -> Self;
}

#[derive(Debug, Clone)]
pub enum BleEvent { Frame(FrameEvent) }
```

`decode_frame` is unchanged (returns `Option<Telemetry>`). `emit_frame` gains the
device so it can read rssi/alias:

**Code sketch (`scan_session` inner + `emit_frame`):**

```rust
if let Ok(Some(mfg)) = device.manufacturer_data().await {
    if let Some(telemetry) = decode_frame(&mfg) {
        let rssi = device.rssi().await.ok().flatten();
        let station = device.alias().await.ok().filter(|s| !s.is_empty());
        tx.send(BleEvent::Frame(FrameEvent { telemetry, rssi, station })).await.ok();
    }
}
```

(Inline the decode/send into `scan_session`, dropping the separate `emit_frame`,
or keep `emit_frame(tx, device, &mfg)` taking `&bluer::Device`.)

**Tests** (existing `ble::tests` keep working — `decode_frame` is unchanged).
Add: `frame_event_new_defaults_none` — `FrameEvent::new(t).rssi.is_none() &&
.station.is_none() && .telemetry.uptime_s == t.uptime_s`.

**Depends on:** nothing. **Blocks:** 4.

---

### 4. AppState extensions — new state, series, trail, reducer

**File:** `crates/meteo-tui/src/app.rs` (modify).

Add render-time state and derived series. Keep `SignalState`, `STALE_AFTER`,
`is_stale`, `signal_state` intact.

**New fields on `AppState`:**

```rust
pub rssi: Option<i16>,            // latest advertised RSSI (updates every event)
pub station: Option<String>,     // BLE alias; header falls back to "MeteoStation"
pub frame_count: u64,            // « échantillons » — DISTINCT frames this session
last_uptime_s: Option<u32>,      // dedup key: last DISTINCT frame's uptime_s
// new derived Series (all Series::DEFAULT_CAP):
pub gust: Series,                // wind.window_max(60) each distinct frame, for overlay + « rafale »
pub heading: Series,             // (t, wind_dir_deg) for the compass trail
pub batt_v: Series,              // batt_mv/1000 (V)
pub solar_w: Series,             // power_w(solar_mv, solar_ma)
pub load_w: Series,              // power_w(batt_mv, load_ma)
pub rain: Series,                // rain_rate_mm_h, for the « Humidité / Pluie » bars
```

`STATION_DEFAULT: &str = "MeteoStation"` const for the header fallback. All new
`Series` are constructed with `Series::new(Series::DEFAULT_CAP)` in `new()`, and
`last_uptime_s` starts `None`.

**Reducer changes (`apply`):** instantaneous state (`latest`, `last_frame_at`,
`rssi`, `station`) updates on **every** event so the cards and link liveness stay
responsive. The series pushes, `frame_count`, and the derived series are gated on
a **distinct `uptime_s`** (the truncation fix above):

```rust
pub fn apply(&mut self, ev: BleEvent, now: Instant) {
    let BleEvent::Frame(fe) = ev;
    let t = fe.telemetry;
    // Always update instantaneous state + liveness (fires ~6×/s on duplicates).
    if let Some(r) = fe.rssi { self.rssi = Some(r); }
    if let Some(s) = fe.station { self.station = Some(s); }
    self.latest = t;
    self.last_frame_at = Some(now);

    // Gate the historical series on a NEW device-second; duplicate adverts
    // carry the same uptime_s and must not over-sample the charts.
    if self.last_uptime_s == Some(t.uptime_s) {
        return;
    }
    self.last_uptime_s = Some(t.uptime_s);
    self.frame_count = self.frame_count.saturating_add(1);

    let secs = now.duration_since(self.started).as_secs_f64();
    // existing six pushes (temp/sky/pressure/lux/wind/humidity), each guarded on Some ...
    if let Some(d) = t.wind_dir_deg { self.heading.push(secs, f64::from(d)); }
    // push wind BEFORE window_max so the current sample is included:
    if let Some(g) = self.wind.window_max(60.0) { self.gust.push(secs, g); }
    if let Some(mv) = t.batt_mv { self.batt_v.push(secs, f64::from(mv) / 1000.0); }
    if let Some(w) = model::power_w(t.solar_mv, t.solar_ma) { self.solar_w.push(secs, w); }
    if let Some(w) = model::power_w(t.batt_mv, t.load_ma) { self.load_w.push(secs, w); }
    if let Some(r) = t.rain_rate_mm_h { self.rain.push(secs, f64::from(r)); }
}
```

(`uptime_s` is `u32` and always present in the frame, so it is a reliable dedup
key. `window_max(60.0)` — the 60 s gust window — is a literal, not
`WINDOW_SECS.min(60.0)`.)

**Tests** (extend `app::tests`; update existing constructions from
`BleEvent::Frame(t)` to `BleEvent::Frame(FrameEvent::new(t))`. Because
`FrameEvent::new` defaults work, give each test frame a distinct `uptime_s` when
it must register as a new sample):

- `apply_dedupes_duplicate_uptime` — apply two frames with the **same**
  `uptime_s` → `frame_count == 1`, `temp.points().len() == 1`; a third with a
  **new** `uptime_s` → `frame_count == 2`, `temp.points().len() == 2`. This is
  the truncation-fix regression test.
- `apply_updates_latest_on_duplicate` — duplicate `uptime_s` but a changed
  `rssi`/`temperature_c` → `latest` and `rssi` still reflect the newest event
  even though no series point was added.
- `apply_increments_frame_count` — two applies with distinct `uptime_s` →
  `frame_count == 2`.
- `apply_carries_rssi_and_station` — `FrameEvent { rssi: Some(-65), station:
Some("rooftop-01"), .. }` → `app.rssi == Some(-65)`, `app.station.as_deref()
== Some("rooftop-01")`.
- `apply_derives_power_series` — frame with `solar_mv=15000, solar_ma=600,
batt_mv=3900, load_ma=120` → `solar_w` last ≈ `9.0`, `load_w` last ≈ `0.468`,
  `batt_v` last ≈ `3.9`.
- `apply_pushes_heading_gust_rain` — frame with `wind_speed_ms=4.0,
wind_dir_deg=270.0, rain_rate_mm_h=1.5` → `heading.points().len()==1`,
  `gust.points().len()==1`, `rain.points().len()==1`.
- Update `apply_frame_updates_latest_and_series`,
  `apply_frame_skips_none_fields_in_series`, `is_stale_*`,
  `signal_state_transitions` to the new event shape (distinct `uptime_s` where a
  series point is asserted).

**Depends on:** 2, 3. **Blocks:** 7, 8, 9, 10, 11.

---

### 5. Plot primitive — Canvas-based history plot

**File:** `crates/meteo-tui/src/plot.rs` (create); `mod plot;` in `main.rs`.

One reusable Canvas widget for all 9 history plots. Renders into a bordered
`Block` (title inset, unit chip top-right), then a `Canvas` for the trace. Pure
coordinate-mapping helpers are unit-tested; the Canvas paint is covered by a
no-panic smoke test.

**Public types/signatures:**

```rust
use ratatui::{Frame, layout::Rect, style::Color};

/// Marker style for traces (CLI `--marker-style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MarkerStyle { Dots, Line }

/// One overlay trace drawn under the main line (e.g. gust @32 %).
pub struct Overlay<'a> { pub points: &'a [(f64, f64)], pub color: Color, pub alpha: f64 }

/// Bars on an independent lower-half scale (rain). `None` = no bars.
pub struct Bars<'a> { pub points: &'a [(f64, f64)], pub color: Color }

pub struct PlotSpec<'a> {
    pub title: &'a str,          // FR panel title, e.g. "Température air"
    pub unit: &'a str,           // chip text, e.g. "°C" / "klx" / "W"
    pub color: Color,            // quantity colour
    pub prec: usize,             // y-tick decimals
    pub floor: Option<f64>,      // padded_value_bounds floor (0.0 for non-negative)
    pub scale: f64,              // display multiplier for the y bounds + labels;
                                 // 1.0 normally, 0.001 for lux→klx (keeps app.lux raw)
    pub marker: MarkerStyle,
    pub show_grid: bool,         // dotted gridlines at 25/50/75 %
    pub fill: bool,              // gradient under-fill 13 %→0
    pub overlay: Option<Overlay<'a>>,
    pub bars: Option<Bars<'a>>,
}

/// Render one plot. `series` is `&mut` because `Series::points()` calls
/// `make_contiguous`. Draws an "en attente…" placeholder when empty.
pub fn render_plot(frame: &mut Frame, area: Rect, spec: &PlotSpec, series: &mut Series);

/// Pure: map a data point to Canvas coords given x/y bounds — testable.
/// Canvas uses `x_bounds`/`y_bounds` = the data bounds, so this is identity-ish,
/// but the helper centralises the right-anchored window + floor clamp.
#[must_use] pub fn fill_columns(points: &[(f64,f64)], y_lo: f64) -> Vec<(f64,f64,f64)>;
```

**Rendering approach:**

- Block: `Block::bordered().border_style(fg=BORDER).title(Line(title, SUBTEXT0))`;
  unit chip drawn as a right-aligned title span `Span(" {unit} ", fg=OVERLAY2 on
SURFACE0)`. Panel fill `MANTLE`.
- Inner `Canvas::default().x_bounds(x_win).y_bounds(y_win).marker(Marker::Braille)`
  with `paint = |ctx| { … }`.
- Gridlines (`show_grid`): for f in [0.25,0.5,0.75], draw a dotted horizontal
  `Points` row at `y = lo + f*(hi-lo)` in `blend_rgb(SURFACE2, BASE, 0.18)`.
- Fill (`fill`): for each consecutive pair, draw a vertical `Line` from baseline
  `y_lo` up to the point in `blend_rgb(color, BASE, 0.13 * height_fraction)` —
  fading from 13 % at the line to 0 at the baseline (approximate with a few
  stacked segments per column; `fill_columns` returns `(x, y_top, y_bottom)`).
- Trace: `MarkerStyle::Dots` → `ctx.draw(&Points{coords, color})`;
  `MarkerStyle::Line` → `ctx.draw(&Line{...})` between consecutive points.
- Overlay: same as trace but `color = blend_rgb(overlay.color, BASE, overlay.alpha)`.
- Bars: map to the lower half of the frame on their own `[0, max]` scale; draw
  short vertical `Line`s. Rain `== 0` → a faint baseline tick (per dossier).
- Axes: reuse `model::padded_value_bounds` + `model::value_axis_labels`; Y max
  top-left, Y min bottom-left (OVERLAY1); X labels `["-10m","-5m","maint."]`
  (SURFACE2). Window via `series.x_window()`. Apply `spec.scale` to the y-axis
  **labels only** (multiply the padded bounds before `value_axis_labels`); the
  Canvas data/bounds stay in raw units, so the trace geometry is unchanged and
  only the printed tick text reads in klx.

**Tests** (`plot::tests`):

- `fill_columns_spans_baseline_to_point` — input `[(0,1),(1,3)]`, `y_lo=0` →
  each tuple has `y_bottom==0.0` and `y_top==value`.
- `render_plot_empty_shows_placeholder` — empty series on a `TestBackend(40,8)`
  → buffer contains `"en attente"`; no panic.
- `render_plot_smoke_with_grid_fill_overlay` — series with 5 points + an overlay,
  `TestBackend(60,10)`, `show_grid+fill` on → no panic, buffer contains the title.

**Depends on:** 1, 2. **Blocks:** 10.

---

### 6. Compass — Canvas wind dial

**File:** `crates/meteo-tui/src/compass.rs` (create); `mod compass;` in `main.rs`.

A `Canvas` painter for the VENT card centre. Pure geometry helpers unit-tested;
paint covered by a smoke test.

**Signatures:**

```rust
/// Heading (deg, 0°=N=up, 90°=E=right) → unit-circle coords on radius `r`.
/// `x = r·sin(θ)`, `y = r·cos(θ)`. Testable.
#[must_use] pub fn heading_to_xy(deg: f64, r: f64) -> (f64, f64);

/// Inputs the compass needs (kept render-agnostic for the smoke test).
pub struct CompassData<'a> {
    pub speed_ms: Option<f64>,
    pub heading_deg: Option<f64>,
    pub gust_ms: Option<f64>,
    pub trail: &'a [(f64, f64)],   // (t, heading) newest-last; faded by age
    pub now_secs: f64,             // latest series time, for trail age
    pub show_trail: bool,          // CLI --gust-trail
}

/// Render the dial into `area` (rings, 45°/15° ticks, N E S O cardinals with N
/// red, needle triangle + tail, fading trail, centre readout). Draws « calme »
/// and hides the needle when `speed < 0.3`.
pub fn render_compass(frame: &mut Frame, area: Rect, data: &CompassData);
```

**Rendering approach (Canvas `x_bounds([-1,1]) y_bounds([-1,1])`):**

- Outer ring r=0.9 (SURFACE0), inner ring r=0.55 (HAIRLINE), hub r=0.12 (CRUST),
  drawn as dotted `Points` circles or `canvas::Circle` shapes.
- Ticks: every 15° short (SURFACE1) from r=0.82→0.9; every 45° long (OVERLAY0)
  from r=0.74→0.9.
- Cardinals via `ctx.print`: N at (0,0.97) RED, E at (0.97,0), S at (0,-0.97),
  O at (-0.97,0) in OVERLAY1.
- Needle: filled triangle from hub to `heading_to_xy(h, 0.78)` (SKY), base two
  points at `heading±90°` × small r; tail opposite to `heading_to_xy(h+180, 0.3)`
  in NEEDLE_TAIL. Hidden when calm.
- Trail (`show_trail`): for each `(t, hdg)` in `trail`, plot a dot at
  `heading_to_xy(hdg, 0.65)` with `blend_rgb(SKY, BASE, age_alpha)` where
  `age_alpha = (1 - (now_secs - t)/60).clamp(0,1)`.
- Centre readout (below the dial, separate `Line`s): `"{speed:.1}"` SKY + `" m/s · {h:.0}° "`
  - `compass_label_fr(h)` (SUBTEXT0) + « rafale {gust:.1} » (TEAL). Calm →
    « calme ».

**Tests** (`compass::tests`):

- `heading_to_xy_cardinals` — `(0,1)→(0,1)` (N up), `(90,1)→(1,0)` (E right),
  `(180,1)→(0,-1)`, `(270,1)→(-1,0)` (assert within `1e-9`).
- `render_compass_smoke_no_panic` — `TestBackend(40,16)`, heading 270, speed 4,
  gust 6, 3 trail points → no panic; buffer contains `"O"` and `"rafale"`.
- `render_compass_calm_shows_calme` — speed 0.1 → buffer contains `"calme"`.

**Depends on:** 1, 2. **Blocks:** 8.

---

### 7. Header band

**File:** `crates/meteo-tui/src/ui/header.rs` (create); `mod header;` in `ui/mod.rs`.

**Signature (single, final form):**

```rust
pub fn render_header(frame: &mut Frame, area: Rect, app: &AppState, now: Instant, pulse: f64);
```

**Layout:** one row, bottom rule `BORDER`. Left `Line`: `◆` (SKY) + station name
(`app.station.as_deref().unwrap_or(STATION_DEFAULT)`, TEXT bold) + `" · app v{ver}"`
(OVERLAY1) + `" · "` + GPS from `model::fmt_location(...)` (OVERLAY1). Right `Line`,
right-aligned: clock `chrono::Local::now().format("%Y-%m-%d %H:%M:%S")` (SUBTEXT1)

- link state.

**Link state (dynamic):** `match app.signal_state(now)` — `Live` →
`●` GREEN (pulsing) + « En direct »; `Stale`/`NoSignal` → `●` RED (static) +
« Hors ligne ». The broadcast-rate suffix « · 1 Hz » (SUBTEXT0) is shown in
**both** states (it is the firmware's advertising cadence, per the dossier header
— only the dot + label change with link state). The `pulse` alpha is passed down
from `mod.rs` (computed from wall-clock elapsed); the header blends the green dot
colour `blend_rgb(GREEN, BASE, pulse)`, while the red offline dot is static.

**Code sketch:**

```rust
pub fn render_header(frame: &mut Frame, area: Rect, app: &AppState, now: Instant, pulse: f64) {
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
    let station = app.station.as_deref().unwrap_or(crate::app::STATION_DEFAULT);
    let gps = model::fmt_location(app.latest.latitude_deg, app.latest.longitude_deg, app.latest.altitude_m);
    let left_line = Line::from(vec![
        Span::styled("◆ ", Style::new().fg(theme::SKY)),
        Span::styled(station, Style::new().fg(theme::TEXT).bold()),
        Span::styled(format!("  ·  app v{}  ·  {gps}", app.app_version), Style::new().fg(theme::OVERLAY1)),
    ]);
    let clock = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let (dot, label, dot_col) = match app.signal_state(now) {
        SignalState::Live => ("●", "En direct", theme::blend_rgb(theme::GREEN, theme::BASE, pulse)),
        _                  => ("●", "Hors ligne", theme::RED),
    };
    let right_line = Line::from(vec![
        Span::styled(clock, Style::new().fg(theme::SUBTEXT1)),
        Span::styled(format!("   {dot} "), Style::new().fg(dot_col)),
        Span::styled(format!("{label}  ·  1 Hz"), Style::new().fg(theme::SUBTEXT0)),
    ]).right_aligned();
    frame.render_widget(Paragraph::new(left_line), left);
    frame.render_widget(Paragraph::new(right_line), right);
    // bottom rule: render a BORDER-styled bottom border block over `area`.
}
```

**Tests** (`header::tests`): `render_header_live_shows_en_direct` —
`TestBackend(120,1)`, fresh frame → buffer contains `"En direct"`, the station
name, and `"1 Hz"`; `render_header_offline_when_stale` — last frame > STALE_AFTER
→ buffer contains `"Hors ligne"` and still `"1 Hz"` (the rate suffix is
state-independent).

**Depends on:** 1, 2, 4. **Blocks:** 11.

---

### 8. Summary band — three cards

**File:** `crates/meteo-tui/src/ui/summary.rs` (create); `mod summary;` in `ui/mod.rs`.

**Signature:** `pub fn render_summary(frame: &mut Frame, area: Rect, app: &mut AppState, options: &Options);`
(`&mut` for the compass trail / series points.) Splits `area` into three columns
(`Constraint::Ratio(1,3)×3`), each a bordered `Block` (fill MANTLE, title SUBTEXT0).

**ATMOSPHÈRE card:** hero `Line` « Air » + big `temperature_c` (PEACH, no threshold
colour) + dimmed `°C` (OVERLAY2); trend arrow top-right from
`app.temp.trend_delta(600.0)` → `classify_trend` → `"▲ +0.3 °C / 10m"` (GREEN up /
PEACH down / « stable » OVERLAY1). Rows (name OVERLAY0, value quantity-colour,
unit SURFACE2): « Humidité » `%HR` (SAPPHIRE) · « Pression » `hPa` (TEAL) ·
« Temp. ciel » `°C` (LAVENDER) · « Lumin. » `klx` via `fmt_lux_klx` (YELLOW) ·
« Pluie » `mm/h` (BLUE) · « Pt rosée » `°C` via `dew_point_c(temp, rh)` when both
present (OVERLAY2). Sensor-fault rows show dimmed « N/A » (driven by
`app.latest.diagnostics`).

**VENT card:** delegates to `compass::render_compass` with a `CompassData` built
from `app.latest.wind_speed_ms`, `wind_dir_deg`, `app.gust` latest, and the
`app.heading.points()` trail; `show_trail = options.gust_trail`.

**ÉNERGIE card:** « Solaire » `●`YELLOW power W (`power_w(solar_mv,solar_ma)`) +
dimmed `V · mA` sub-line. « Batterie » `●`GREEN `pct %` + a `ratatui::widgets::Gauge`
(`ratio = pct/100`, `gauge_style fg=battery_color(pct) bg=CRUST`, frame SURFACE0) +
flow line via `fmt_battery_flow(...)` (GREEN charge / RED discharge). « Charge »
`●`MAUVE power W (`power_w(batt_mv,load_ma)`) + dimmed `mA` sub-line.

**Code sketch (layout splits + the ATMOSPHÈRE hero & one row):**

```rust
pub fn render_summary(frame: &mut Frame, area: Rect, app: &mut AppState, options: &Options) {
    let [atmo, vent, ener] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area);
    render_atmosphere(frame, atmo, app);
    render_vent(frame, vent, app, options);   // builds CompassData, calls compass::render_compass
    render_energie(frame, ener, app);
}

fn render_atmosphere(frame: &mut Frame, area: Rect, app: &AppState) {
    let t = &app.latest;
    let block = Block::bordered().border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().bg(theme::MANTLE))
        .title(Span::styled("ATMOSPHÈRE", Style::new().fg(theme::SUBTEXT0)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // hero + 6 rows stacked vertically:
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(inner);
    // hero: "Air" + big temp (PEACH, no threshold colour) + dimmed °C + trend arrow.
    let trend = app.temp.trend_delta(600.0).map(|d| (model::classify_trend(d, 0.1), d));
    let hero = Line::from(vec![
        Span::styled("Air  ", Style::new().fg(theme::OVERLAY0)),
        Span::styled(format!("{:.1}", t.temperature_c.unwrap_or(f32::NAN)), Style::new().fg(theme::PEACH).bold()),
        Span::styled(" °C", Style::new().fg(theme::OVERLAY2)),
        // trend arrow right-aligned, GREEN ▲ / PEACH ▼ / OVERLAY1 « stable » ...
    ]);
    frame.render_widget(Paragraph::new(hero), rows[0]);
    // one representative row helper (name OVERLAY0 · value quantity-colour · unit SURFACE2),
    // dimmed « N/A » when the driving sensor's diagnostics bit is set:
    let row = |name: &str, val: String, col: Color| Line::from(vec![
        Span::styled(format!("{name:<11}"), Style::new().fg(theme::OVERLAY0)),
        Span::styled(val, Style::new().fg(col)),
    ]);
    // rows: Humidité %HR SAPPHIRE · Pression hPa TEAL · Temp. ciel °C LAVENDER ·
    //       Lumin. (fmt_lux_klx) YELLOW · Pluie mm/h BLUE ·
    //       Pt rosée (dew_point_c(temp, rh) when both Some) OVERLAY2 ...
}
```

**Tests** (`summary::tests`):

- `render_summary_smoke` — `TestBackend(120,16)`, a full frame
  (temp/humidity/pressure/solar/batt set) → no panic; buffer contains
  `"ATMOSPHÈRE"`, `"VENT"`, `"ÉNERGIE"`, `"Pt rosée"`.
- `render_summary_none_fields` — `AppState::new` with **no** frame applied (all
  sensor fields `None`) → no panic; buffer still contains `"ATMOSPHÈRE"` and the
  dimmed `"N/A"` placeholder, exercising `fmt_lux_klx(None)` and the absent
  dew-point / `fmt_battery_flow(None, None, None)` paths.

**Depends on:** 1, 2, 4, 6. **Blocks:** 11.

---

### 9. Diagnostics bar

**File:** `crates/meteo-tui/src/ui/diagnostics.rs` (create); `mod diagnostics;` in `ui/mod.rs`.

**Signature:** `pub fn render_diagnostics(frame: &mut Frame, area: Rect, app: &AppState, now: Instant);`
Bordered `Block` titled « DIAGNOSTIC ». Two regions (left chips, right block).

**Sensor chips:** one chip per sensor with a status dot + name (SUBTEXT1).
Map each `Diagnostics` bit to a sensor; dot colour GREEN ok / RED panne (YELLOW
« alerte » reserved for soft warnings like occlusion/divergence). Sensors:
BMP388 (`baro_fault`), BME280 (`bme280_fault`), VEML7700 (`veml7700_fault`),
MLX90614 (`mlx90614_fault` / `occlusion`→alerte), INA PV (`ina_pv_fault`), INA
batt (`ina_batt_fault`), baro-divergence → alerte on BMP/BME.

**BLE chip:** `RSSI {dbm} dBm` coloured by `theme::rssi_color`; « Hors ligne »/RED
when `rssi` is `None` or signal is not `Live`.

**Right block:** « actif » `{uptime_s}` formatted `HhMm` · « échantillons »
`{frame_count}` · « dernier paquet » `{age:.1} s` coloured by
`packet_age_color(age)` · legend `● ok  ● alerte  ● panne` (GREEN/YELLOW/RED dots).

**Helper (pure, in model.rs or diagnostics.rs):**

```rust
/// Uptime seconds → compact label, three branches by magnitude:
///   ≥ 3600 s → "{h}h{mm}m"  (e.g. 3725 → "1h02m")
///   ≥ 60 s   → "{m}m{ss}s"  (e.g. 90   → "1m30s")
///   < 60 s   → "0m{ss}s"    (e.g. 45   → "0m45s")
/// Minutes/seconds zero-padded to two digits; hours unpadded.
#[must_use] pub fn fmt_uptime(secs: u32) -> String;
```

Add `fmt_uptime` to model.rs with tests:
`fmt_uptime_hours` (`3725 → "1h02m"`), `fmt_uptime_minutes` (`90 → "1m30s"`),
`fmt_uptime_seconds_only` (`45 → "0m45s"`).

**Code sketch (region split + one sensor chip + the right block):**

```rust
pub fn render_diagnostics(frame: &mut Frame, area: Rect, app: &AppState, now: Instant) {
    let block = Block::bordered().border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled("DIAGNOSTIC", Style::new().fg(theme::SUBTEXT0)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [chips_a, right_a] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(56)]).areas(inner);

    // One chip = colored dot + name. `chip(name, color)` builds the spans;
    // status from the matching diagnostics bit (GREEN ok / YELLOW alerte / RED panne):
    let d = app.latest.diagnostics;
    let chip = |name: &str, col: Color| vec![
        Span::styled("● ", Style::new().fg(col)),
        Span::styled(format!("{name}  "), Style::new().fg(theme::SUBTEXT1)),
    ];
    let mut spans = Vec::new();
    spans.extend(chip("BMP388", if d.baro_fault() { theme::RED } else { theme::GREEN }));
    // ... BME280, VEML7700, MLX90614 (occlusion→YELLOW), INA PV, INA batt ...
    // BLE chip: RSSI {dbm} dBm coloured by theme::rssi_color, else « Hors ligne »/RED.
    frame.render_widget(Paragraph::new(Line::from(spans)), chips_a);

    // Right block: actif · échantillons · dernier paquet (packet_age_color) + legend.
    let age = app.last_frame_at.map_or(f64::INFINITY, |t| now.duration_since(t).as_secs_f64());
    let right = Line::from(vec![
        Span::styled(format!("actif {}   ", model::fmt_uptime(app.latest.uptime_s)), Style::new().fg(theme::OVERLAY1)),
        Span::styled(format!("échantillons {}   ", app.frame_count), Style::new().fg(theme::OVERLAY1)),
        Span::styled(format!("dernier paquet {age:.1} s"), Style::new().fg(theme::packet_age_color(age))),
        // + legend "● ok  ● alerte  ● panne" (GREEN/YELLOW/RED dots) ...
    ]).right_aligned();
    frame.render_widget(Paragraph::new(right), right_a);
}
```

**Tests** (`diagnostics::tests`): `render_diagnostics_smoke` — `TestBackend(120,3)`,
app with `rssi=Some(-65)`, `frame_count=10`, `uptime_s=3725` → buffer contains
`"actif"`, `"échantillons"`, `"dernier paquet"`, `"ok"`; `render_diagnostics_fault`
— frame with `baro_fault` → buffer contains `"BMP388"` and the BLE RSSI chip text
`"RSSI"`.

**Depends on:** 1, 2, 4. **Blocks:** 11.

---

### 10. History grids — sensors (6) + power (3)

**File:** `crates/meteo-tui/src/ui/history.rs` (create); `mod history;` in `ui/mod.rs`.

**Signature:** `pub fn render_history(frame: &mut Frame, area: Rect, app: &mut AppState, options: &Options);`

Vertical split into a « CAPTEURS » block (6 plots, 2 rows × 3 cols) and an
« ÉNERGIE » block (3 plots, 1 row × 3 cols), each a titled outer `Block`; inner
plots laid out with `Layout`. Every plot delegates to `plot::render_plot`.

**Code sketch (nesting + one representative `render_plot` call):**

```rust
pub fn render_history(frame: &mut Frame, area: Rect, app: &mut AppState, options: &Options) {
    let [capteurs, energie] = Layout::vertical([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)]).areas(area);
    let cap_block = Block::bordered().border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled("CAPTEURS", Style::new().fg(theme::SUBTEXT0)));
    let cap_inner = cap_block.inner(capteurs);
    frame.render_widget(cap_block, capteurs);
    let [row1, row2] = Layout::vertical([Constraint::Ratio(1, 2); 2]).areas(cap_inner);
    let [a, b, c] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(row1);
    // one representative cell (row1, col1 — air temperature):
    plot::render_plot(frame, a, &plot::PlotSpec {
        title: "Température air", unit: "°C", color: theme::PEACH, prec: 1,
        floor: None, scale: 1.0, marker: options.marker_style,
        show_grid: options.show_grid, fill: true, overlay: None, bars: None,
    }, &mut app.temp);
    // ... b=sky (LAVENDER), c=lux (scale 0.001, unit "klx") ; row2 = pression /
    //     vent (overlay = gust @0.32) / humidité (bars = rain) ; then ÉNERGIE block
    //     with batt_v (V, prec 2) / solar_w / load_w on its own row.
}
```

**Sensors grid (row1 / row2):**
| Plot | Series | title | unit | colour | floor | extras |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `temp` | Température air | °C | PEACH | None | — |
| 2 | `sky` | Température ciel | °C | LAVENDER | None | — |
| 3 | `lux` | Luminosité | klx | YELLOW | 0.0 | `scale = 0.001` (raw lux in `app.lux`; labels read klx) |
| 4 | `pressure` | Pression | hPa | TEAL | None | — |
| 5 | `wind` | Vitesse du vent | m/s | SKY | 0.0 | `overlay = gust @0.32` from `app.gust` |
| 6 | `humidity` | Humidité / Pluie | %HR | SAPPHIRE | 0.0 | `bars = rain` (own scale, lower half) |

Plot 3 luminosity uses `PlotSpec.scale = 0.001` (lux→klx) — `app.lux` stays raw,
only the y-tick labels read in klx (per §5). Plot 6 humidity overlays rain bars
from `app.rain` (the `rain: Series` and its push are already defined in §4) on
their own lower-half `[0, max]` scale. All other plots use `scale = 1.0`.

**Power grid:**
| Plot | Series | title | unit | colour |
| --- | --- | --- | --- | --- |
| 1 | `batt_v` | Batterie | V | GREEN (prec 2) |
| 2 | `solar_w` | Puissance solaire | W | YELLOW |
| 3 | `load_w` | Puissance charge | W | MAUVE |

All carry `marker = options.marker_style`, `show_grid = options.show_grid`,
`fill = true`.

**Tests** (`history::tests`): `render_history_smoke` — `TestBackend(150,30)`, app
fed several frames → no panic; buffer contains `"CAPTEURS"`, `"Vitesse du vent"`,
`"Puissance solaire"`, `"klx"`.

**Depends on:** 1, 2, 4, 5. **Blocks:** 11.

---

### 11. UI orchestrator + main loop + CLI options

**Files:** `crates/meteo-tui/src/ui.rs` → delete; `crates/meteo-tui/src/ui/mod.rs`
(create); `crates/meteo-tui/src/main.rs` (modify).

**`Options` (in `ui/mod.rs`, built from CLI):**

```rust
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub marker_style: plot::MarkerStyle,
    pub show_grid: bool,
    pub gust_trail: bool,
}

impl Options {
    /// Dossier defaults (dots / grid on / trail on) — also the render smoke-test
    /// fixture, so the tests don't depend on clap parsing.
    #[must_use]
    pub fn default_for_test() -> Self {
        Self { marker_style: plot::MarkerStyle::Dots, show_grid: true, gust_trail: true }
    }
}
```

**`ui/mod.rs` render orchestrator:**

```rust
mod header; mod summary; mod diagnostics; mod history;

/// Draw the full dashboard. `pulse` ∈ [0,1] is the « En direct » dot intensity
/// (computed from wall-clock elapsed in main.rs).
pub fn render(frame: &mut Frame, app: &mut AppState, now: Instant, options: &Options, pulse: f64) {
    frame.render_widget(Block::default().style(Style::new().bg(theme::BASE)), frame.area());
    let [header_a, summary_a, diag_a, history_a] = Layout::vertical([
        Constraint::Length(2),   // header
        Constraint::Length(13),  // summary band (cards + compass)
        Constraint::Length(3),   // diagnostics bar
        Constraint::Min(0),      // history grids
    ]).areas(frame.area());
    header::render_header(frame, header_a, app, now, pulse);
    summary::render_summary(frame, summary_a, app, options);
    diagnostics::render_diagnostics(frame, diag_a, app, now);
    history::render_history(frame, history_a, app, options);
}
```

**Graceful degradation:** the layout uses `Min(0)` so small terminals shrink the
history first; each panel already no-ops/placeholders on tiny `Rect`s. Never
panic. Keep the tiny-terminal test.

**`main.rs` changes:**

- Module decls: `mod app; mod ble; mod model; mod theme; mod plot; mod compass; mod ui;`.
- `Cli` adds (with a fully-defined `MarkerArg` so the implementer makes no
  decision):

  ```rust
  #[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
  enum MarkerArg { #[default] Dots, Line }

  impl From<MarkerArg> for plot::MarkerStyle {
      fn from(m: MarkerArg) -> Self {
          match m { MarkerArg::Dots => Self::Dots, MarkerArg::Line => Self::Line }
      }
  }

  // in Cli:
  #[arg(long, value_enum, default_value_t = MarkerArg::Dots)] marker_style: MarkerArg,
  #[arg(long, default_value_t = true, action = clap::ArgAction::Set)] show_grid: bool,
  #[arg(long, default_value_t = true, action = clap::ArgAction::Set)] gust_trail: bool,
  ```

  `clap::ValueEnum` derives the value parsing/help; `default_value_t` needs the
  `Default` derive (no manual `Display` required for a `ValueEnum`). Build
  `Options { marker_style: cli.marker_style.into(), show_grid: cli.show_grid,
gust_trail: cli.gust_trail }`.

- Redraw loop: replace the 1 Hz interval with **10 Hz** (`Duration::from_millis(100)`),
  keeping the explicit "display cadence, not a readiness sleep" comment. Compute
  `pulse` from elapsed: `let pulse = pulse_intensity(started.elapsed());` where
  ```rust
  /// 1.6 s triangle wave in [0.35,1.0] for the « En direct » dot. Pure.
  fn pulse_intensity(elapsed: Duration) -> f64;
  ```
  (testable; add `mod tests` in main.rs). Pass `&options` + `pulse` into
  `ui::render`.
- `should_quit` unchanged.

**Tests:**

- Adapt the existing `ui::tests` smoke tests (currently in `ui.rs`:
  `render_smoke_fills_buffer_without_panic`, `render_shows_baro_fault_diagnostic`,
  `render_smoke_small_terminal_no_panic`) to the new
  `render(frame, app, now, &Options::default_for_test(), 1.0)` signature. **Drop**
  the old English-label assertions that no longer exist — specifically the
  `"Live"`, `"app v"`, `"time"`, `"Diagnostics"`, `"Sky temperature"`, `"OK"`,
  `"Location"` substring checks in `render_smoke_fills_buffer_without_panic` — and
  **replace** them with the French verbatim strings « En direct », « ATMOSPHÈRE »,
  « CAPTEURS ». `render_shows_baro_fault_diagnostic` keeps asserting a fault is
  surfaced but via the new diagnostics chip (`"BMP388"`), not the table row.
- Keep `render_smoke_small_terminal_no_panic` (40×12) verbatim in intent — the
  no-panic guarantee on a tiny terminal.
- `main::tests` add `pulse_intensity_bounds` — `pulse_intensity(0)` and
  `pulse_intensity(800ms)` stay within `[0.35, 1.0]`; differs across the cycle.
- Keep all `should_quit_*` tests.

**Depends on:** 1, 5, 6, 7, 8, 9, 10. **Blocks:** nothing (final).

---

## Testing

- **Pure logic (host, `cargo nextest`):** every new `model`/`theme`/`plot`/
  `compass` helper has Given/When/Then tests per the table in each substep
  (dew point, FR rose, trend, window_max/trend_delta, power_w, flow, blend_rgb,
  threshold colours, heading_to_xy, fill_columns, fmt_uptime, pulse_intensity).
- **Reducer:** `app::apply` tests cover frame_count, rssi/station carry, the five
  new derived series, heading/gust/rain pushes, and — crucially — the
  `uptime_s` **dedup regression** (`apply_dedupes_duplicate_uptime`,
  `apply_updates_latest_on_duplicate`) that guards the chart-truncation fix;
  existing series/staleness tests are migrated to the `FrameEvent` shape.
- **Render smoke (TestBackend):** one no-panic + key-string assertion per panel
  (header, summary, diagnostics, history, plot, compass) plus the full-screen
  `render` smoke and the **tiny-terminal (40×12) no-panic** test (kept verbatim
  in intent). Assertions check French verbatim strings actually reach the buffer.
- **Edge cases:** empty series → "en attente…" placeholder; calm wind → « calme »
  - no needle; rain 0 → baseline tick; missing sensor → dimmed « N/A »; stale
    link → « Hors ligne » + RED BLE chip; `None` RSSI → no crash.
- **Full workspace gate (before squashing each substep and before any push):**
  ```bash
  just tui-clippy            # fast loop while iterating
  cargo fmt --all -- --check
  cargo clippy --all-features --all-targets -- -D warnings   # via just clippy
  cargo nextest run -p meteo-tui --target x86_64-unknown-linux-gnu  # via just test
  ```
  `just clippy`/`just test`/`just tui-build` already scope `meteo-tui` to the host
  target — **never** build the workspace on the default riscv target.
- **Manual visual check (host = gaia, local):** `just tui-run` against the live
  station (or `--address`) to eyeball palette, compass, pulse, and the grids.

## Risks

- **Canvas fidelity vs effort.** The gradient fill and dual-axis rain bars are the
  hardest pieces; `Canvas` gives full control but is verbose. Mitigation: the
  `plot` primitive centralises it once; pure `fill_columns`/`heading_to_xy`
  helpers are unit-tested so geometry bugs surface without a terminal.
- **Truecolor terminals.** `Color::Rgb` needs a 24-bit terminal; on 256-colour
  terminals ratatui downsamples (acceptable degradation, not a panic). No code
  change needed; note in the crate docs.
- **Small terminals.** Fixed full-screen design targets a maximized terminal.
  `Min(0)` + per-panel placeholders keep it from panicking; the 40×12 test guards
  this. Layout `Length` budgets (2/13/3/Min) must be re-checked against the
  compass/card heights during impl — tune if cards clip.
- **Test churn from the `FrameEvent` change.** Every `BleEvent::Frame(t)` in
  `app::tests`/`ui::tests` must move to `FrameEvent::new(t)`. Mechanical but
  must be exhaustive or the crate won't compile — grep `BleEvent::Frame(` before
  finishing §4/§11.
- **10 Hz redraw cost.** Negligible on the dev host (user confirmed). The tick is
  a display cadence; all data redraws remain event-driven. Keep the explicit
  comment so the no-sleep rule isn't misread.
- **bluer property latency.** `rssi()`/`alias()` are D-Bus property reads per
  advert; they're cached by BlueZ and cheap, but wrap in `.ok()` so a transient
  read failure degrades to `None`, never drops the frame.
- **`as` casts / clippy.** `blend_rgb`, canvas coord rounding, and percent maths
  need `#[expect(clippy::cast_*)]` with reasons (ANSSI: no bare `as`). Prefer
  `f64::from`/`try_into` where a fallible path exists.

## Notes

Progress tracking (checked during `/tyrex:code:implement-light`):

- [ ] 1 — theme.rs (palette + threshold colours + blend_rgb)
- [ ] 2 — model.rs extensions (dew point, FR rose, trend, window helpers, formatters)
- [ ] 3 — ble.rs (FrameEvent + rssi/alias)
- [ ] 4 — app.rs (state + series + reducer; rain Series; **uptime_s dedup / truncation fix**)
- [ ] 5 — plot.rs (Canvas plot primitive)
- [ ] 6 — compass.rs (Canvas compass)
- [ ] 7 — ui/header.rs
- [ ] 8 — ui/summary.rs (3 cards)
- [ ] 9 — ui/diagnostics.rs
- [ ] 10 — ui/history.rs (6+3 grids)
- [ ] 11 — ui/mod.rs + main.rs (orchestrator, Options, 10 Hz loop, CLI flags)
- [ ] Full `just clippy` + `just test` + `just tui-build` green
- [ ] Manual visual check via `just tui-run`
