# Brainstorm: Historic-data web dashboard (Pi collector + Leptos)

- **ID:** 9
- **Category:** Feature
- **Date:** 2026-06-26
- **Status:** Active

## Context

From `ROADMAP.md` → "Web server for historic data (Raspberry Pi collector)". The
firmware broadcasts telemetry live at 1 Hz; if nobody is scanning, the data is gone.
We want an always-on collector that stores history and serves a web dashboard with
roughly the same look as `meteo-tui`, but for **past-data browsing** instead of just
the live scrolling window.

User direction for this exploration:

- Backend in **Rust**; frontend in **Leptos**.
- Look & feel must match the existing TUI (the Catppuccin Mocha design system).
- **Read-only, no auth.** No data-writing path is foreseen.
- Selectable time period + scrolling.
- **Page 1 "all panels":** every panel at once for a chosen period.
- **Page 2 "comparison":** overlay one plot over another, selecting date + plot type.
- Wind compass artwork to come from the imported Claude Design project
  ("Custom meteo station TUI", file `Wind Compass - Assets.dc.html`).

## Current State

The repo today is a BLE peripheral (firmware) + a BLE central (TUI). There is **no
web, database, HTTP, or wasm code anywhere** (confirmed by grep: no axum/actix/rocket,
no sqlx/diesel/rusqlite, no leptos/yew/web-sys).

Reusable building blocks already present:

- **`meteo-lib`** (`crates/meteo-lib`) — hardware-agnostic, `no_std`-friendly,
  `default-features = false` for host use. Exposes the wire frame:
  - `meteo_lib::ble::frame::Telemetry` — 18 fields, all the telemetry below.
  - `Telemetry::decode(bytes: &[u8]) -> Result<Self, FrameError>` (38-byte v5 frame).
  - `Diagnostics(u8)` bitfield (8 sensor/fault bits).
- **`meteo-tui/src/ble.rs`** — the bluer 0.17 passive-scan loop:
  `Session` → adapter `DiscoveryFilter { transport: Le, duplicate_data: true }` →
  `discover_devices_with_changes()` → read `ManufacturerData[0xFFFF]` →
  `decode_frame()`. Adapter-reset resilient. **This is the collector's ingest core,
  liftable almost verbatim** (the Pi is aarch64 Linux/std, same as the dev host).
- **`meteo-tui/src/theme.rs`** — Catppuccin Mocha as `Color::Rgb` constants. The same
  hex values are mirrored in `docs/Meteo Station - Dossier.dc.html` (the design dossier)
  and must be ported to CSS variables for the web.
- **`meteo-tui/src/plot.rs` + `model.rs`** — chart model: per-quantity colours,
  gradient fill, gust **overlay band**, rain bars, and `gaussian_smooth(pts, sigma)`
  (centred kernel, no phase lag) — the trace-smoothing the web charts should reproduce.
- The TUI already renders the **image-based wind compass** (dial + rotated needle), so
  the compass artwork exists in-repo and as SVG/PNG in the design project.

`Telemetry` fields (type · unit · wire): `temperature_c` f32 °C · `pressure_hpa` f32
hPa · `humidity_pct` f32 %RH · `sky_temp_c` f32 °C · `luminosity_lux` f32 lux ·
`wind_speed_ms` f32 m/s · `wind_dir_deg` f32 deg · `battery_pct` u8 % · `rain_rate_mm_h`
f32 mm/h · `solar_mv`/`solar_ma`/`batt_mv`/`load_ma` u16 · `diagnostics` u8 bitfield ·
`uptime_s` u32 · `latitude_deg`/`longitude_deg`/`altitude_m` f32 (coarse).

## Findings

**Feasible, low-risk, mostly new code.** The hard part (wire format + BLE scan) is
already solved and shared via `meteo-lib`. New surface = storage + HTTP + Leptos UI.

### Design system (matches the TUI exactly)

From `docs/Meteo Station - Dossier.dc.html` (the authoritative spec):

- **Palette: Catppuccin Mocha.** Backgrounds: Base `#1e1e2e`, Mantle `#181825` (panel
  fill), Crust `#11111b` (wells/tracks), Border `#2a2a3c`, Surface0 `#313244`. Text
  ramp: Text `#cdd6f4` → Subtext1 `#bac2de` → Subtext0 `#a6adc8` → Overlay2 `#9399b2`
  → Overlay1 `#7f849c` → Overlay0 `#6c7086` → Surface2 `#585b70`.
- **One hue per quantity:** Air temp = Peach `#fab387`; Sky temp = Lavender `#b4befe`;
  Pressure / batt gauge / gust = Teal `#94e2d5`; Humidity = Sapphire `#74c7ec`;
  Luminosity / Solar = Yellow `#f9e2af`; Rain = Blue `#89b4fa`; Wind / compass =
  Sky `#89dceb`; Battery / charging / OK = Green `#a6e3a1`; Load = Mauve `#cba6f7`.
  States: OK Green, Warn Yellow `#f9e2af`, Fault/North Red `#f38ba8`.
- **Fonts:** JetBrains Mono (titles, axes, labels) + IBM Plex Sans (body).
- **Layout tiers:** Header → Summary band (ATMOSPHÈRE / VENT compass / ÉNERGIE) →
  Diagnostics bar (deliberately separated) → History grids (CAPTEURS 6 + ÉNERGIE 3).
- **Trace style:** dotted "braille" points in the quantity colour + faint gradient
  fill (13 %→0); dotted gridlines at 25/50/75 %; axes min/max corners. **UI strings
  are French, verbatim** (« En direct », « Vitesse du vent », « rafale », N E S O…).

### Wind compass (imported design)

`Wind Compass - Assets.dc.html` ships two square, shared-pivot layers — a static
**dial** and a **needle** (North-up at 0°, clockwise-positive) — as PNG (1024²) +
editable SVG. Dial: ring `#313244`, inner `#26263a`, ticks `#585b70`/`#7f849c`, N
marker `#f38ba8`, cardinals `#a6adc8`. Needle: Sky blade gradient `#4aa6c9→#b6f0fb`,
slate tail `#46586a→#2c3a47`, hub `#11111b`/`#45475a`.

The TUI's `ratatui-image` raster-compositing path is **not needed on the web**: inline
the dial SVG, overlay the needle SVG, rotate it with CSS `transform: rotate(<dir>deg)`
about the shared centre. Live readout text (speed · cap° · 16-pt FR rose · « rafale »)
overlays on top, exactly as in the dossier.

### Storage model (ROADMAP-decided, adopted)

- Pi runs the bluer passive scan; aggregates the 1 Hz stream to **1-minute buckets**.
- Store **min / max / avg per field per bucket** (not just avg) — preserves wind gusts
  and pressure spikes. Gust = `max(wind_speed)` per bucket for free.
- **Single flat SQLite table, kept forever.** ~1440 rows/day × ~120 B ≈ 170 KiB/day ≈
  ~60 MiB/year. No retention tiers, no pre-stored rollups.
- **Aggregate at query time** (`min/max/avg GROUP BY` a zoom-sized bucket) for fast
  multi-year zoom-outs — the only thing that scales is rendering point count.
- The Pi's NTP wall clock timestamps each bucket, so the on-device RTC item is moot.

### Resolved scope decisions (this session)

- **Dashboard = historic + live band.** The all-panels page carries a live
  instantaneous header (air temp, wind compass, power) fed by the collector's freshest
  **1 Hz** frame (not the 1-min DB), via a server push (SSE/poll). Charts below browse
  stored history.
- **Comparison page = fully flexible (date, metric) traces.** Each overlaid trace is an
  independent pick of (date + metric) on a shared time-of-day X axis; auto dual Y when
  the overlaid metrics differ. Covers both same-metric/different-days and
  different-metrics/same-day.
- **Time selection = presets (day/week/month) + custom range + continuous pan/zoom.**
  Query-time aggregation keeps zoom-outs cheap.
- **Charts show the min–max envelope band** behind the avg line (same idiom as the TUI
  gust band), preserving spikes.
- **Read-only, no auth** throughout (confirmed; no write path planned).

## Scope

**In scope**

- New host/Pi crate(s) in this workspace (aarch64 Linux/std), sharing `meteo-lib` for
  decode and lifting the bluer scan from `meteo-tui`.
- A collector that passively scans BLE, aggregates 1 Hz → 1-min min/max/avg buckets,
  and persists to SQLite.
- An HTTP server exposing read-only history queries + a live-frame push.
- A Leptos frontend with two pages — all-panels (live band + historic charts for a
  selected period) and comparison (flexible date/metric overlays) — styled to the
  Catppuccin Mocha design system, French UI strings, with the imported SVG compass.

**Out of scope**

- Any write/config path, auth, or user accounts.
- Firmware changes, on-device logging/RTC (the Pi's NTP clock covers timestamps).
- Multi-board / OTA / hardware-v2 items from the ROADMAP.
- Grafana (won't match the TUI look) — explicitly not the chosen path.

## Open Questions

Implementation-only — for the planner:

- **Leptos rendering mode:** SSR + server functions (axum integration, idiomatic full
  stack) vs CSR SPA + a thin axum JSON/SSE API. Both viable for a read-only Pi
  dashboard; pick per build complexity and pan/zoom interactivity needs.
- **Charting implementation:** pure Leptos-drawn SVG (full control, reproduces the
  braille/gradient/gust-band aesthetic, Rust-only) vs a JS charting lib (uPlot/ECharts)
  via wasm interop (strong large-series pan/zoom, but styling to match the TUI is extra
  work). Look-and-feel fidelity favours custom SVG; performance at multi-year zoom
  favours a mature lib — decide with the query-time aggregation in mind.
- **Process topology:** single binary (collector + server sharing one SQLite handle /
  WAL) vs two processes (collector daemon writes, web server reads). Recommend a single
  always-on binary on the Pi for simplicity.
- **Workspace/build wiring:** the workspace default target is `riscv32imac…`; host
  crates compile by having host-only deps. A **wasm32 Leptos frontend crate** needs its
  own target — decide how it sits in the workspace (separate target dir / trunk /
  cargo-leptos / not a workspace member) without breaking `just build`/`just tui-*`.
- **SQLite driver:** `rusqlite` (sync, simple, bundled) vs `sqlx` (async, compile-time
  checked). Schema: exact column set for min/max/avg per field + indices on timestamp.
- **Live push mechanism:** SSE vs WebSocket vs short-poll for the 1 Hz live band.
- **Vendoring the compass assets:** pull `compass/*.svg` (+ PNG) from the design
  project into the web crate's static assets during implementation.

## Next Steps

- Run `/tyrex:code:plan-light 9` to turn this into an implementation plan (crate
  layout, schema, endpoints, Leptos page/component breakdown, build wiring, and the
  decisions left open above).
