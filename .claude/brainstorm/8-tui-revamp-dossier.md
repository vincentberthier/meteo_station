# Brainstorm: Complete TUI revamp to the design dossier

- **ID:** 8
- **Category:** Feature
- **Date:** 2026-06-26
- **Status:** Active

## Context

The firmware and BLE telemetry pipeline are stable. The user wants to rebuild the
`meteo-tui` dashboard from scratch to match `docs/Meteo Station - Dossier.dc.html`.
That dossier is the **actual target**, not an example — the rebuilt TUI must
reproduce its layout, palette, French strings, and dynamic behaviour.

The dossier specifies (Rust + Ratatui):

- Four information tiers, top to bottom: **Header → Summary band (3 cards) →
  Diagnostics bar → History (two 3-column chart grids)**.
- **Catppuccin Mocha** palette, every colour a direct `Color::Rgb`. One hue per
  quantity, kept consistent across value, plot, and axis.
- Native-terminal aesthetic: one monospace face, bordered `Block`s with inset
  titles, braille-marker traces with a faint gradient fill, restrained chrome.
- French on-screen strings, quoted verbatim (« En direct », « ATMOSPHÈRE »,
  « VENT », « ÉNERGIE », « DIAGNOSTIC », « CAPTEURS », « Pt rosée », « rafale »…).

## Current State

`crates/meteo-tui` (host-only, `x86_64`, `std`) does a passive BLE scan via
`bluer` and renders a much simpler dashboard:

- `ble.rs` — passive scan; on each `MeteoStation` advert it reads
  `manufacturer_data()`, decodes the 38-byte v5 frame (`COMPANY_ID 0xFFFF`), and
  pushes a `BleEvent::Frame(Telemetry)` over an mpsc channel. **RSSI and device
  name are not read.**
- `app.rs` — `AppState` holds `latest: Telemetry`, `last_frame_at`, and six
  `Series` ring buffers (temp, sky, pressure, lux, wind, humidity). `apply` is a
  pure reducer. `STALE_AFTER = 5 s`; `SignalState` ∈ {NoSignal, Live, Stale}.
- `model.rs` — pure formatters (`fmt_temp`, `fmt_wind`, `compass_label` 16-pt,
  `fmt_diagnostics`, …), `Series` (cap 600 = 10 min @ 1 Hz, right-anchored
  `x_window`), `padded_value_bounds`, `value_axis_labels`. Well unit-tested.
- `ui.rs` — `render` = header strip (clock | app version | signal label) + an
  11-row telemetry **table** + **6** stacked line charts. Named `Color::Light*`
  colours, no Canvas, no compass, no power charts, English labels.
- `main.rs` — tokio loop, 1 Hz clock tick for redraw, `--address` CLI flag
  (default `F0:CA:FE:00:00:01`), quit on q/Esc/Ctrl-C.

`Telemetry` (meteo-lib `ble/frame.rs`) fields available to the TUI:
`temperature_c, pressure_hpa, humidity_pct, sky_temp_c, luminosity_lux,
wind_speed_ms, wind_dir_deg, battery_pct, rain_rate_mm_h, solar_mv, solar_ma,
batt_mv, load_ma, diagnostics, uptime_s, latitude_deg, longitude_deg,
altitude_m`. **No gust, no dew point, no RSSI, no firmware-version field.**

## Findings

Feasible as a TUI-only rebuild. Data-gap resolution (all host-side):

- **RSSI** — read `Device::rssi()` in the scan loop alongside `manufacturer_data()`;
  carry it on the BLE event. Colour chip ≥−70 green / −70…−90 yellow / <−90 red.
- **Dew point** (« Pt rosée ») — derive from `temperature_c` + `humidity_pct`
  (Magnus formula). New pure function in `model.rs`, host-tested.
- **Gust** (« rafale ») — **rolling max of wind speed over the last 60 s**
  (matches the compass trail window). Needs a time-windowed max helper.
- **Station name** (« rooftop-01 ») — use the **BLE device alias/name** from the
  `Device` object (advertised name is `MeteoStation`; a BlueZ alias overrides it).
  Carry it through to the header. No new CLI flag required.
- **« actif »** = `uptime_s`; **« échantillons »** = TUI-counted frames this
  session; **« dernier paquet »** = age of `last_frame_at` (colour <2 s normal /
  2–10 s yellow / >10 s red, per dossier).
- **fw version** in the header is unavailable over passive broadcast (no DIS, no
  connection — per CLAUDE.md); show **app version only**.

New rendering work (all in `meteo-tui`):

- **Summary band** — 3 cards: « ATMOSPHÈRE » (hero air temp + humidity, pressure,
  sky temp, luminosity, rain, dew point rows; 10-min trend arrow), « VENT »
  (Canvas compass), « ÉNERGIE » (solar W, battery % + SoC gauge, load W, flow
  status line).
- **Compass** — custom `Canvas`: outer/inner rings, 45°/15° ticks, cardinals
  **N E S O** (O = Ouest/West, N in red), Sky-coloured needle triangle + opposite
  tail, fading heading trail (~22 points ≈ 60 s), centre readout (speed · m/s ·
  heading° + FR 16-pt rose · gust).
- **Diagnostics bar** — sensor chips (status dot + name), BLE RSSI chip, right
  block (« actif » · « échantillons » · « dernier paquet » + ok/alerte/panne
  legend). Deliberately separated from the measurements.
- **Sensors grid (6 plots)** — air temp · sky temp · luminosité (klx) // pression ·
  vitesse du vent (with gust overlay @32 %) · humidité+pluie (humidity line, left
  axis + rain bars, lower half, own scale).
- **Power grid (3 plots)** — batterie (V, green) · puissance solaire (W, yellow) ·
  puissance charge (W, mauve). Needs **3 new `Series`** (battery V, solar W,
  load W) computed from the frame each tick.
- **Palette** — replace named colours with Catppuccin Mocha `Color::Rgb`
  constants; one hue per quantity. App background `#1e1e2e`.
- **French strings + units** — all labels French verbatim; units per dossier
  (%HR, hPa, °C, **klx**, mm/h, V, W, mA). Luminosity shown in kilolux.
- **Dynamic rules** — battery gauge width + threshold colour (≥50 green / 20–49
  yellow / <20 red); battery flow line (▲ en charge green / ▼ décharge red, with
  ~autonomy); air-temp trend arrow (Δ/10 min, stable if |Δ|<0.1 °C); « En direct »
  pulsing dot (1.6 s) → « Hors ligne » red on link loss; calm wind <0.3 m/s hides
  needle / shows « calme »; rain 0 → baseline tick instead of bars.
- **Layout** — fixed full-screen design targeting a **maximized terminal**;
  degrade gracefully on small sizes (render what fits, never panic — keep the
  existing tiny-terminal no-panic test).

The pure-formatter + `Series` + reducer architecture is sound and should be
extended, not discarded. The `meteo-lib` frame/decode layer is untouched.

## Scope

**In scope:** complete rewrite of `crates/meteo-tui` (`ui.rs`, plus additions to
`app.rs`, `model.rs`, `ble.rs`, `main.rs`) to reproduce the dossier. Host-side
derived metrics (RSSI, dew point, 60 s gust). New Canvas compass, power charts,
combined humidity/rain chart, Catppuccin palette, French strings, dynamic rules.

**Out of scope:** firmware, the BLE wire frame (`meteo-lib::ble::frame`), the
aggregator, and any on-device change. No v6 frame. Real animation fidelity beyond
what a terminal redraw loop allows is best-effort.

## Open Questions

Implementation-specific only (for the planner):

- **Gradient fill & rain bars:** ratatui `Chart` has no under-line fill or bar
  overlay. Decide per chart: custom `Canvas` (full control, more code) vs
  `Chart` + a separate `BarChart`/scatter approximation. The « Humidité/Pluie »
  dual-axis (line + bars, independent scales) likely needs Canvas.
- **Compass rendering:** `Canvas` geometry for rings/ticks/needle/trail; how to
  fade trail opacity in a 256/truecolor terminal (alpha-blend toward bg).
- **Animation cadence:** the 1.6 s « En direct » pulse and trail fade need
  sub-second redraws; raise the redraw tick (e.g. ~5–10 Hz) while keeping it a
  display cadence, not a readiness sleep. Confirm CPU cost is acceptable.
- **Exposed options** (dossier §5: `markerStyle` dots/line, `showGrid`,
  `gustTrail`): expose as CLI flags with defaults (dots / grid on / trail on),
  or a small config — planner's call.
- **Palette organization:** a `theme`/`palette` module of `Color::Rgb` consts vs
  inline literals; per-quantity colour map.
- **Power `Series` derivation:** solar W = `solar_mv·solar_ma`, load W =
  `batt_mv·load_ma`, battery V = `batt_mv/1000` — where to compute (reducer vs
  render) and sentinel handling.

## Next Steps

Run `/tyrex:code:plan-light 8` to turn this into an implementation plan.
