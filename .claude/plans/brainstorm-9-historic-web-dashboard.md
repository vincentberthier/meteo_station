# Plan: Historic-data web dashboard (Pi collector + Leptos SSR)

- **Source:** '9 (`.claude/brainstorm/9-historic-web-dashboard.md`)
- **Date:** 2026-06-26
- **Status:** Planned (plan-reviewer: PASS, round 3)

## Summary

Build an always-on Raspberry-Pi collector + web dashboard that stores weather
history and serves a Leptos dashboard matching the `meteo-tui` look (Catppuccin
Mocha, French UI). A single binary on the Pi (aarch64 Linux/std) passively scans
BLE, aggregates the firmware's 1 Hz telemetry stream into 1-minute min/max/avg
SQLite buckets, holds the freshest 1 Hz frame in shared state, and serves a
**Leptos SSR** app (cargo-leptos) with two pages: an **all-panels** page (live
instantaneous band + historic charts for a selected period, with presets +
custom range + pan/zoom) and a **comparison** page (flexible `(date, metric)`
overlay traces on a shared time-of-day X axis). Charts are **custom Leptos SVG**
reproducing the TUI's smoothing, gradient fill, and min–max envelope band.
Pure display/chart math currently living in `meteo-tui` is extracted into a new
shared `meteo-chart` crate so nothing is duplicated. Read-only, no auth.

### Architecture decisions (resolved with the user)

| Decision          | Choice                                             |
| ----------------- | -------------------------------------------------- |
| Render mode       | **SSR + cargo-leptos** (server fns + hydration)    |
| Charting          | **Custom Leptos-drawn SVG** (matches TUI exactly)  |
| SQLite driver     | **rusqlite** (bundled libsqlite3, sync + WAL)      |
| Process topology  | **Single binary** (collector task + web server)    |
| Live push         | **axum SSE** route on the leptos router (1 Hz)     |
| Shared logic      | New `meteo-chart` crate; migrate `meteo-tui` to it |
| Telemetry on-wire | web-side **serde DTOs** (`meteo-lib` stays clean)  |

### Pinned versions (verified on crates.io, 2026-06-26)

`leptos = "0.8.20"`, `leptos_axum = "0.8.10"`, `cargo-leptos = "0.3.6"` (dev
tool), `axum = "0.8.9"`, `rusqlite = "0.40.1"` (feature `bundled`),
`tokio-stream = "0.1.18"`, `serde = "1.0.228"`, `bluer = "0.17"` (already used),
`chrono = "0.4"` (already used). `meteo-lib`/`meteo-chart` shared.
**`cargo-leptos` is already installed on the dev host** — no toolchain install
needed for substep 2. **Rust nightly is installed and current**, so the web
crate enables leptos's `nightly` feature (ergonomic signal-call syntax) and the
web recipes run under `cargo +nightly leptos …`; firmware / `meteo-lib` /
`meteo-tui` / `meteo-chart` stay on **stable** (nightly is scoped to the web
recipes only — no repo-wide `rust-toolchain.toml`).

## Files Modified

| File                                             | Action             | Description                                                                                                                                                                                          |
| ------------------------------------------------ | ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml` (workspace)                         | modify             | Add `crates/meteo-chart`, `crates/meteo-web` members; add workspace deps (leptos stack, rusqlite, serde, tokio-stream); add `[profile.wasm-release]` (cargo-leptos lib profile)                      |
| `crates/meteo-chart/Cargo.toml`                  | create             | New host-std shared crate (palette + chart math + FR formatters)                                                                                                                                     |
| `crates/meteo-chart/src/lib.rs`                  | create             | Re-exports                                                                                                                                                                                           |
| `crates/meteo-chart/src/palette.rs`              | create             | Catppuccin Mocha as canonical `Rgb(u8,u8,u8)` consts + `css(Rgb) -> String` helper                                                                                                                   |
| `crates/meteo-chart/src/chart.rs`                | create             | `gaussian_smooth`, `padded_value_bounds`, `value_axis_labels` (moved from `meteo-tui`) + tests                                                                                                       |
| `crates/meteo-chart/src/format.rs`               | create             | `compass_label_fr`, `fmt_lux`, `lux_chart_unit`, `power_w`, `fmt_power`, `fmt_battery_flow`, `dew_point_c`, `fmt_uptime`, `fmt_location`, `classify_trend`, `Trend` (moved from `meteo-tui`) + tests |
| `crates/meteo-tui/Cargo.toml`                    | modify             | Add `meteo-chart` dep                                                                                                                                                                                |
| `crates/meteo-tui/src/model.rs`                  | modify             | Delete moved fns + their tests; `pub use meteo_chart::{…}` so call sites are unchanged                                                                                                               |
| `crates/meteo-tui/src/theme.rs`                  | modify             | Derive `Color::Rgb` from `meteo_chart::palette` consts (no duplicated hex)                                                                                                                           |
| `crates/meteo-web/Cargo.toml`                    | create             | cargo-leptos package (`cdylib`+`rlib`), `ssr`/`hydrate` features, `[package.metadata.leptos]`                                                                                                        |
| `crates/meteo-web/src/main.rs`                   | create             | (ssr) axum + leptos bootstrap; spawn collector; mount `/live` SSE                                                                                                                                    |
| `crates/meteo-web/src/lib.rs`                    | create             | `App` root component, hydrate entrypoint, module decls                                                                                                                                               |
| `crates/meteo-web/src/state.rs`                  | create             | `AppState { db: DbHandle, live_rx: watch::Receiver<Option<Telemetry>> }`                                                                                                                             |
| `crates/meteo-web/src/types.rs`                  | create             | Shared serde DTOs: `HistoryRow`, `MetricStat`, `TracePoint`, `Metric`, `LiveFrame` (breaks the db↔api type cycle)                                                                                    |
| `crates/meteo-web/src/db/mod.rs`                 | create             | `DbHandle`, `BucketRow`, schema/migration, `store_bucket`, `query_history`, `query_comparison` (imports DTOs from `types`)                                                                           |
| `crates/meteo-web/src/db/schema.sql`             | create             | `samples` table DDL                                                                                                                                                                                  |
| `crates/meteo-web/src/collector/mod.rs`          | create             | BLE scan loop (lifted from `meteo-tui::ble`) → bucket flush → DB + live watch                                                                                                                        |
| `crates/meteo-web/src/collector/bucket.rs`       | create             | `BucketAccumulator` (pure, tested): add/finish min/max/avg + vector-mean wind dir                                                                                                                    |
| `crates/meteo-web/src/api/mod.rs`                | create             | server fns `get_history`, `get_comparison_trace` + `history_impl`/`comparison_impl`; `pub use crate::types::*`                                                                                       |
| `crates/meteo-web/src/api/sse.rs`                | create             | `live_sse` axum handler (1 Hz `text/event-stream`)                                                                                                                                                   |
| `crates/meteo-web/src/components/mod.rs`         | create             | component module root                                                                                                                                                                                |
| `crates/meteo-web/src/components/chart.rs`       | create             | `PlotPanel` SVG component (trace, gradient fill, min–max band, grid, axes)                                                                                                                           |
| `crates/meteo-web/src/components/compass.rs`     | create             | `WindCompass` (dial + rotated needle SVG)                                                                                                                                                            |
| `crates/meteo-web/src/components/header.rs`      | create             | header bar (clock, signal state, version)                                                                                                                                                            |
| `crates/meteo-web/src/components/live_band.rs`   | create             | live instantaneous band (air temp · compass · power) via SSE                                                                                                                                         |
| `crates/meteo-web/src/components/time_select.rs` | create             | period presets + custom range + pan/zoom controls                                                                                                                                                    |
| `crates/meteo-web/src/pages/all_panels.rs`       | create             | Page 1: live band + historic chart grid                                                                                                                                                              |
| `crates/meteo-web/src/pages/comparison.rs`       | create             | Page 2: flexible `(date, metric)` overlay traces                                                                                                                                                     |
| `crates/meteo-web/build.rs`                      | create             | Codegen `style/_palette.scss` from `meteo_chart::palette` (single colour source, no drift); `meteo-chart` is a `[build-dependencies]` + `[dev-dependencies]` entry                                   |
| `crates/meteo-web/tests/palette_css.rs`          | create             | Integration test: `meteo_chart::css(...)` hex values + generated `_palette.scss` has all 22 vars                                                                                                     |
| `crates/meteo-web/style/main.scss`               | create             | `@use "palette";` + `@font-face` + layout                                                                                                                                                            |
| `crates/meteo-web/style/_palette.scss`           | create (generated) | `:root { --base:#1e1e2e; … }` written by build.rs                                                                                                                                                    |
| `crates/meteo-web/assets/compass/*.svg`          | create             | Vendored from `crates/meteo-tui/assets/compass`                                                                                                                                                      |
| `crates/meteo-web/assets/fonts/*.woff2`          | create             | `JetBrainsMono-{Regular,Bold}.woff2` (JetBrains/JetBrainsMono), `IBMPlexSans-{Regular,Bold}.woff2` (IBM/plex)                                                                                        |
| `Justfile`                                       | modify             | `web-build`, `web-build-pi`, `web-serve`, `web-watch`, `web-clippy`; extend `clippy`/`test`                                                                                                          |
| `CLAUDE.md`                                      | modify             | Document `meteo-web`/`meteo-chart`, build wiring, schema, endpoints                                                                                                                                  |
| `ROADMAP.md`                                     | modify             | Mark "Web server for historic data" in progress/done                                                                                                                                                 |

## Plan

### 1. Extract shared display/chart math into `meteo-chart`

**Goal:** one home for the pure functions both the TUI and the web dashboard
need, so nothing is duplicated (global rule: no duplication — extract shared
logic).

**Create `crates/meteo-chart/Cargo.toml`:**

```toml
[package]
name = "meteo-chart"
version.workspace = true
authors.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
# pure std crate — no ratatui, no bluer; compiles for host AND wasm32
[dev-dependencies]
test-log = { workspace = true }
env_logger = { workspace = true }

[lints]
workspace = true
```

(No runtime deps: `f64` math is in `std` on host and wasm; `Vec`/`String` are
`std`. This crate must compile for `wasm32-unknown-unknown` — keep it `std`,
no `libm`, no `ratatui`.)

**`src/palette.rs`** — canonical Catppuccin Mocha as RGB tuples + hex strings.
These become the single source of truth; `meteo-tui::theme` and the web CSS both
derive from here.

```rust
//! Catppuccin Mocha palette — canonical RGB + hex, shared by TUI and web.
/// `(r, g, b)` for each named colour. `Copy` so palette consts pass to `css()`
/// by value with no borrow ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const BASE: Rgb = Rgb(0x1e, 0x1e, 0x2e);
pub const MANTLE: Rgb = Rgb(0x18, 0x18, 0x25);
// … ALL 22 colours from meteo-tui::theme, same hex values, in this order:
// BASE MANTLE CRUST BORDER SURFACE0 SURFACE2 TEXT SUBTEXT1 SUBTEXT0 OVERLAY2
// OVERLAY1 OVERLAY0 PEACH LAVENDER TEAL SAPPHIRE YELLOW BLUE SKY GREEN MAUVE RED
pub const RED: Rgb = Rgb(0xf3, 0x8b, 0xa8);

/// Lowercase `#rrggbb` string (for CSS / SVG attributes).
#[must_use]
pub fn css(c: Rgb) -> String { format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2) }
```

**`src/chart.rs`** — MOVE verbatim from `meteo-tui::model`: `gaussian_smooth`,
`padded_value_bounds`, `value_axis_labels` (signatures unchanged):

```rust
pub fn gaussian_smooth(pts: &[(f64, f64)], sigma: f64) -> Vec<(f64, f64)>;
pub fn padded_value_bounds(min: f64, max: f64, floor: Option<f64>) -> [f64; 2];
pub fn value_axis_labels(bounds: [f64; 2], min_prec: usize) -> [String; 3];
```

Move their `#[test]`s too (`gaussian_smooth_*`, `padded_value_bounds_*`,
`value_axis_labels_*`) into `meteo-chart`'s test module (standard layout from
CLAUDE.md / rust skill).

**`src/format.rs`** — MOVE the pure FR display helpers from `meteo-tui::model`:
`compass_label_fr`, `fmt_lux`, `lux_chart_unit`, `power_w`, `fmt_power`,
`fmt_battery_flow`, `dew_point_c`, `fmt_uptime`, `fmt_location`,
`classify_trend` + the `Trend` enum. Move their tests. (`SignalState` and the
`Series` ring buffer STAY in `meteo-tui` — they are TUI-runtime state, not pure
shared math.)

**`src/lib.rs`:**

```rust
pub mod chart;
pub mod format;
pub mod palette;
pub use chart::{gaussian_smooth, padded_value_bounds, value_axis_labels};
pub use format::{compass_label_fr, dew_point_c, fmt_battery_flow, fmt_location,
    fmt_lux, fmt_power, fmt_uptime, lux_chart_unit, power_w, classify_trend, Trend};
```

**Migrate `meteo-tui`:** add `meteo-chart = { path = "../meteo-chart" }` to its
`Cargo.toml`; in `model.rs` delete the moved definitions and add
`pub use meteo_chart::{gaussian_smooth, padded_value_bounds, value_axis_labels,
compass_label_fr, …};` so every existing call site (`crate::model::…`) resolves
unchanged. In `theme.rs`, define each `Color::Rgb` from the palette const:
`pub const PEACH: Color = Color::Rgb(palette::PEACH.0, palette::PEACH.1,
palette::PEACH.2);` (still `const`). Keep `battery_color`/`rssi_color`/
`packet_age_color`/`blend_rgb` in `meteo-tui::theme` (ratatui-typed).

**Files:** `crates/meteo-chart/{Cargo.toml,src/lib.rs,src/palette.rs,
src/chart.rs,src/format.rs}`; modify `crates/meteo-tui/{Cargo.toml,src/model.rs,
src/theme.rs}`; add member to workspace `Cargo.toml`.

**Tests (must pass):**

- Moved tests run under `meteo-chart`: `gaussian_smooth_attenuates_spike`,
  `gaussian_smooth_preserves_len_and_timestamps`, `padded_value_bounds_*`,
  `value_axis_labels_*`, `compass_label_fr_cardinals`, `fmt_lux_*`, etc.
- New `palette_css_formats_lowercase_hex` asserts `css(RED) == "#f38ba8"`.
- `just test` (meteo-lib + meteo-tui) still green → migration preserved behaviour.
- `just tui-clippy` clean.

**Risk:** call-site breakage in `meteo-tui` ui/_ files. Mitigation: the
`pub use` re-export in `model.rs` keeps the `crate::model::_` paths valid, so no
ui/\* edits needed; compiler + existing tests catch any miss.

**Depends on:** nothing. Do this first.

---

### 2. `meteo-web` crate skeleton + cargo-leptos wiring (SSR "hello")

**Goal:** a buildable SSR app that serves an empty Catppuccin shell, with the
workspace/build wiring proven before any feature code.

**`crates/meteo-web/Cargo.toml`** (cargo-leptos canonical shape):

```toml
[package]
name = "meteo-web"
version.workspace = true
# … workspace inheritance …

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
leptos = { version = "0.8.20", features = ["nightly"] }   # see risk: pin features
leptos_axum = { version = "0.8.10", optional = true }
leptos_meta = "0.8"
leptos_router = "0.8"
axum = { version = "0.8.9", optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"], optional = true }
tokio-stream = { version = "0.1.18", optional = true }
rusqlite = { version = "0.40.1", features = ["bundled"], optional = true }
bluer = { version = "0.17", features = ["bluetoothd"], optional = true }
futures = { version = "0.3", optional = true }
serde = { version = "1.0.228", features = ["derive"] }
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
wasm-bindgen = { version = "0.2", optional = true }
console_error_panic_hook = { version = "0.1", optional = true }
meteo-lib = { workspace = true }
meteo-chart = { path = "../meteo-chart" }
anyhow = { version = "1", optional = true }

[features]
default = []
hydrate = ["leptos/hydrate", "dep:wasm-bindgen", "dep:console_error_panic_hook"]
ssr = ["dep:leptos_axum", "dep:axum", "dep:tokio", "dep:tokio-stream",
       "dep:rusqlite", "dep:bluer", "dep:futures", "dep:anyhow",
       "leptos/ssr", "leptos_meta/ssr", "leptos_router/ssr"]

[package.metadata.leptos]
output-name = "meteo-web"
site-root = "target/site"
site-pkg-dir = "pkg"
style-file = "style/main.scss"
assets-dir = "assets"
site-addr = "0.0.0.0:3000"
bin-features = ["ssr"]
lib-features = ["hydrate"]
bin-target-triple = "x86_64-unknown-linux-gnu"   # dev host; Pi build overrides → aarch64
lib-profile-release = "wasm-release"

[lints]
workspace = true
```

**Workspace `Cargo.toml` — add the cargo-leptos wasm profile.** `lib-profile-
release = "wasm-release"` above references a profile that does **not** exist yet
(the workspace has only `release`/`dev`/`gdb`/`profiling`). In a workspace,
profiles must live in the **root** manifest. Add:

```toml
# crates/meteo-web's wasm (hydrate) lib build, selected by cargo-leptos via
# `lib-profile-release`. Small + single codegen unit for a lean wasm bundle.
[profile.wasm-release]
inherits = "release"
opt-level = "s"
codegen-units = 1
```

(The firmware's own `[profile.release]` is unchanged; this is additive.)

**`src/main.rs`** (gated `#[cfg(feature = "ssr")]`): build the axum router via
`leptos_axum::generate_route_list(App)` + `.leptos_routes(...)`, fallback to
`leptos_axum::file_and_error_handler`, serve with `axum::serve`. A
`#[cfg(not(feature = "ssr"))] fn main() {}` stub keeps the wasm bin target happy.

**`src/lib.rs`:** `#[component] pub fn App()` returning the shell with
`<Stylesheet/>`, `<Title text="MeteoStation"/>`, `<Router>` and the two routes
(placeholders for now). `#[cfg(feature = "hydrate")] #[wasm_bindgen] pub fn
hydrate()` calls `leptos::mount::hydrate_body(App)`.

**Build wiring — critical:** the workspace `.cargo/config.toml` sets
`build.target = "riscv32imac…"`. cargo-leptos passes explicit `--target`
(host for the bin, `wasm32-unknown-unknown` for the lib) which overrides
`build.target`; `bin-target-triple` is pinned to the host so the server half
does not inherit riscv. **`just build` is untouched** (it only builds
`-p meteo-firmware`). Verify both still work.

**`Justfile` recipes:**

```makefile
[doc('Build the web dashboard (SSR + wasm via cargo-leptos)')]
web-build:
    cargo +nightly leptos build --release -p meteo-web

# Pi (aarch64) server build: override the host bin triple. The wasm front is
# host-agnostic; only the server binary cross-compiles. Needs the rustup target
# `aarch64-unknown-linux-gnu` + a cross linker (e.g. the gcc-aarch64-linux-gnu
# toolchain) configured in .cargo/config.toml's [target.aarch64-…] section.
[doc('Cross-build the web dashboard server for the Raspberry Pi (aarch64)')]
web-build-pi:
    cargo +nightly leptos build --release -p meteo-web --bin-target-triple aarch64-unknown-linux-gnu

[doc('Serve the web dashboard locally (hot-reload)')]
web-serve:
    cargo +nightly leptos serve -p meteo-web

[doc('Watch + rebuild the web dashboard')]
web-watch:
    cargo +nightly leptos watch -p meteo-web

[doc('Clippy the web crate (ssr + hydrate)')]
web-clippy:
    cargo +nightly clippy -p meteo-web --no-default-features --features ssr --target {{ host_target }} -- -D warnings
    cargo +nightly clippy -p meteo-web --no-default-features --features hydrate --target wasm32-unknown-unknown -- -D warnings
```

Extend `clippy` and `test` recipes to add `meteo-chart` (host target) and the
`meteo-web --features ssr` host clippy. (`test` adds `meteo-chart`; `meteo-web`
logic is unit-tested under `--features ssr` host target.)

**Tests (must pass):**

- `cargo leptos build -p meteo-web` succeeds; `GET /` returns 200 with the
  Catppuccin `BASE` background (manual curl / browser).
- `just build` (firmware) still succeeds unchanged.
- `web-clippy` clean on both feature sets.

**Risk:** (a) `leptos`'s `nightly` feature needs the nightly toolchain — it is
installed and current, so keep `features = ["nightly"]` and run the `web-*`
recipes under `cargo +nightly leptos …` (nightly scoped to the web crate only;
the rest of the workspace stays stable).
(b) cargo-leptos picking up the riscv `build.target` for the server — mitigated
by `bin-target-triple`; verify with `cargo leptos build -v`. (c) workspace
restriction-lint set tripping on leptos macro expansion — add targeted
`#![allow(...)]` with reasons at the crate root as needed.

**Depends on:** 1 (uses `meteo-chart`).

---

### 3. Shared DTO types + SQLite storage layer (`src/types.rs`, `src/db`)

**Goal:** the serde DTOs and the typed read/write layer over rusqlite, all
blocking calls off the async runtime via `spawn_blocking`. Defining the DTOs
**here** (not in substep 5) breaks the would-be cycle: `db/mod.rs` returns
`HistoryRow`/`TracePoint` keyed by `Metric`, and substep 5's `api` module simply
`pub use`s them — so the dependency flows strictly 3 → 5, never back.

**`src/types.rs`** — shared serde DTOs (one home, imported by `db`, `api`, and
the Leptos components):

```rust
/// One aggregated history bucket sent to the client. `ts` is the bucket's unix
/// second; each metric carries its `(min, max, avg)` triple (NULL → None).
#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryRow {
    pub ts: i64,
    pub temp: MetricStat, pub pressure: MetricStat, pub humidity: MetricStat,
    pub sky: MetricStat, pub lux: MetricStat, pub wind: MetricStat,
    pub wind_dir_avg: Option<f64>,
    pub rain: MetricStat,            // min unused for rain; kept uniform
    pub battery_avg: Option<f64>,
    pub solar_w_avg: Option<f64>,    // power_w(solar_mv_avg, solar_ma_avg)
    pub load_w_avg: Option<f64>,     // power_w(batt_mv_avg, load_ma_avg)
}
/// `(min, max, avg)` triple for one metric; any component may be NULL.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct MetricStat { pub min: Option<f64>, pub max: Option<f64>, pub avg: Option<f64> }

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Metric { AirTemp, Pressure, Humidity, SkyTemp, Lux, Wind, Rain,
                  Battery, Solar, Load }

#[derive(Clone, Serialize, Deserialize)]
pub struct TracePoint { pub x: f64, pub y: f64 }   // x = secs-of-day / since range start

/// Instantaneous frame for the live band — one field per value the band shows;
/// `None` when the firmware reported the field absent this second.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveFrame {
    pub temperature_c: Option<f32>,    // Telemetry::temperature_c
    pub humidity_pct: Option<f32>,     // Telemetry::humidity_pct
    pub pressure_hpa: Option<f32>,     // Telemetry::pressure_hpa
    pub sky_temp_c: Option<f32>,       // Telemetry::sky_temp_c
    pub wind_speed_ms: Option<f32>,    // Telemetry::wind_speed_ms
    pub wind_dir_deg: Option<f32>,     // Telemetry::wind_dir_deg
    pub solar_w: Option<f64>,          // power_w(solar_mv, solar_ma)
    pub load_w: Option<f64>,           // power_w(batt_mv, load_ma) — load is on the battery rail
    pub battery_pct: Option<u8>,       // Telemetry::battery_pct
    pub uptime_s: u32,                 // Telemetry::uptime_s (signal-age / header)
}
#[cfg(feature = "ssr")]
impl LiveFrame {
    /// `solar_w`/`load_w` via `meteo_chart::power_w`; the load circuit draws
    /// from the battery rail, so its bus voltage is `batt_mv` (Telemetry has no
    /// `load_mv`).
    pub fn from_telemetry(t: &Telemetry) -> Self;   // power_w(batt_mv, load_ma) for load
}
```

(The collector's internal `bucket::MinMaxAvg` is a _different_, non-serde type;
`MetricStat` is the wire/DTO triple.)

**`src/db/schema.sql`** — single flat table, `bucket_ts` PRIMARY KEY (implicit
index for range scans), min/max/avg per field + `sample_count` for weighted
re-aggregation:

```sql
CREATE TABLE IF NOT EXISTS samples (
  bucket_ts     INTEGER PRIMARY KEY,   -- unix epoch seconds, floored to the minute
  temp_min REAL, temp_max REAL, temp_avg REAL,
  pressure_min REAL, pressure_max REAL, pressure_avg REAL,
  humidity_min REAL, humidity_max REAL, humidity_avg REAL,
  sky_min REAL, sky_max REAL, sky_avg REAL,
  lux_min REAL, lux_max REAL, lux_avg REAL,
  wind_min REAL, wind_max REAL, wind_avg REAL,   -- wind_max = gust
  wind_dir_avg REAL,                              -- vector-mean heading, degrees
  rain_avg REAL, rain_max REAL,
  battery_avg REAL,
  solar_mv_avg REAL, solar_ma_avg REAL, batt_mv_avg REAL, load_ma_avg REAL,
  sample_count INTEGER NOT NULL
);
```

**`src/db/mod.rs`** (`use crate::types::{HistoryRow, MetricStat, TracePoint, Metric};`):

```rust
/// One persisted minute bucket — exactly one `Option<f64>` per `samples` column
/// (NULL when no sample carried that field this minute), plus the count. Field
/// order and names mirror `schema.sql` 1:1 so the INSERT is positional.
#[derive(Debug, Clone, PartialEq)]
pub struct BucketRow {
    pub bucket_ts: i64,                                   // unix secs, floored to minute
    pub temp_min: Option<f64>, pub temp_max: Option<f64>, pub temp_avg: Option<f64>,
    pub pressure_min: Option<f64>, pub pressure_max: Option<f64>, pub pressure_avg: Option<f64>,
    pub humidity_min: Option<f64>, pub humidity_max: Option<f64>, pub humidity_avg: Option<f64>,
    pub sky_min: Option<f64>, pub sky_max: Option<f64>, pub sky_avg: Option<f64>,
    pub lux_min: Option<f64>, pub lux_max: Option<f64>, pub lux_avg: Option<f64>,
    pub wind_min: Option<f64>, pub wind_max: Option<f64>, pub wind_avg: Option<f64>, // wind_max = gust
    pub wind_dir_avg: Option<f64>,                        // vector-mean heading, degrees
    pub rain_avg: Option<f64>, pub rain_max: Option<f64>,
    pub battery_avg: Option<f64>,
    pub solar_mv_avg: Option<f64>, pub solar_ma_avg: Option<f64>,
    pub batt_mv_avg: Option<f64>, pub load_ma_avg: Option<f64>,
    pub sample_count: i64,
}

#[derive(Clone)]
pub struct DbHandle { conn: Arc<Mutex<rusqlite::Connection>> }   // Mutex: writer is 1/min

impl DbHandle {
    pub fn open(path: &Path) -> anyhow::Result<Self>;            // PRAGMA journal_mode=WAL; run schema.sql
    pub async fn store_bucket(&self, row: BucketRow) -> anyhow::Result<()>;     // spawn_blocking
    pub async fn query_history(&self, q: HistoryQuery) -> anyhow::Result<Vec<HistoryRow>>; // spawn_blocking
    pub async fn query_comparison(&self, date: NaiveDate, metric: Metric)
        -> anyhow::Result<Vec<TracePoint>>;                      // spawn_blocking
}

pub struct HistoryQuery { pub from_ts: i64, pub to_ts: i64, pub bucket_secs: i64 }
```

`BucketRow` is produced by the collector's `BucketAccumulator::finish`
(substep 4) and is the single DTO crossing the collector↔db boundary.

`query_history` re-buckets at query time:
`GROUP BY (bucket_ts / :bucket_secs)`, selecting `MIN(field_min)`,
`MAX(field_max)`, and **sample-count-weighted** avg
`SUM(field_avg * sample_count) / SUM(sample_count)`. `bucket_secs` is chosen by
the caller from the span so the row count stays bounded (≈ ≤1000 points). **Power
in watts is computed in Rust, not SQL:** `query_history` reads the raw
`solar_mv_avg`/`solar_ma_avg`/`batt_mv_avg`/`load_ma_avg` columns and maps them
through `meteo_chart::power_w` while building each `HistoryRow`. `power_w` takes
`Option<u16>`, so the stored `Option<f64>` averages are rounded back to `u16`
(`.map(|v| v.round() as u16)`):

```rust
let to_u16 = |v: Option<f64>| v.map(|x| x.round() as u16);
solar_w_avg: power_w(to_u16(row.solar_mv_avg), to_u16(row.solar_ma_avg)),
load_w_avg:  power_w(to_u16(row.batt_mv_avg),  to_u16(row.load_ma_avg)),
```

Wrap blocking rusqlite calls in `tokio::task::spawn_blocking` with a cloned
`Arc<Mutex<Connection>>`. The real query bodies live in plain helpers
`history_impl(&conn, q)` / `comparison_impl(&conn, …)` so substep 5's `#[server]`
fns are thin wrappers and these are unit-testable directly.

**Tests (must pass, `--features ssr` host):**

- `store_then_query_roundtrips_one_bucket` — open in-memory (`:memory:`),
  store a `BucketRow`, `query_history` over a span returns it.
- `query_history_reaggregates_to_coarser_buckets` — store 10 one-minute rows with
  `temp_avg = 0,1,2,…,9` and `sample_count = 1` each (plus row 0 carrying
  `temp_max = 100`), query with `bucket_secs = 600`; assert exactly one row with
  `temp.max == Some(100.0)` and the weighted `temp.avg == Some(4.5)`
  (`(0+1+…+9)/10`).
- `query_history_computes_power_watts` — seed `solar_mv_avg = 5000`,
  `solar_ma_avg = 200`; assert `solar_w_avg == Some(1.0)` (5 V × 0.2 A).
- `query_history_empty_range_returns_empty`.

**Risk:** avg-of-avg inaccuracy → mitigated by `sample_count`-weighted avg.
Wind direction averaging across 0/360 handled at write time (substep 4), stored
pre-averaged, so query-time `wind_dir_avg` re-aggregation uses the same
vector-mean helper on stored components — store `wind_dir_sin_sum`/`cos_sum`?
**Decision:** store only `wind_dir_avg` (degrees) per minute; coarser
re-aggregation of direction uses a simple mean (acceptable: direction is
display-only and minute resolution already smooths it). Documented.

**Depends on:** 2.

---

### 4. Collector task + pure bucket accumulator (`src/collector`)

**Goal:** lift the proven bluer scan, fold the 1 Hz stream into 1-minute
buckets, write to SQLite, and publish the freshest frame for the live band.

**`src/collector/bucket.rs` — pure + tested:**

```rust
/// Folds a minute's worth of 1 Hz frames into one min/max/avg row.
#[derive(Default)]
pub struct BucketAccumulator {
    count: u32,
    temp: Option<MinMaxAvg>, pressure: Option<MinMaxAvg>, /* … per field … */
    wind: Option<MinMaxAvg>, wind_dir_sin: f64, wind_dir_cos: f64, wind_dir_n: u32,
    /* solar/batt/load running sums … */
}
struct MinMaxAvg { min: f64, max: f64, sum: f64, n: u32 }

impl BucketAccumulator {
    pub fn add(&mut self, t: &Telemetry);          // each present field folds in; None skipped
    pub fn is_empty(&self) -> bool;                // count == 0
    pub fn finish(self, bucket_ts: i64) -> BucketRow;  // avg = sum/n; wind_dir = atan2(sin,cos)→[0,360)
}

/// Floor a unix-second timestamp to its minute (the bucket key).
#[must_use]
pub fn floor_to_minute(unix_secs: i64) -> i64 { unix_secs - unix_secs.rem_euclid(60) }
```

Wind direction uses a **vector mean** (`sin`/`cos` accumulation, `atan2`,
wrapped to `[0,360)`) so it is correct across the 0/360 seam. A field with no
samples this minute serializes as `NULL` (its column `Option<f64>` → `None`).

**`src/collector/mod.rs`:**

```rust
pub async fn run(db: DbHandle, live_tx: watch::Sender<Option<Telemetry>>,
                 addr: bluer::Address) -> anyhow::Result<()>;
```

Re-implement the scan structure from `crates/meteo-tui/src/ble.rs` (bluer 0.17,
`DiscoveryFilter { transport: Le, duplicate_data: true }`,
`discover_devices_with_changes`, adapter-reset-resilient outer loop). For each
decoded `Telemetry`: (a) `live_tx.send(Some(t))` for the live band; (b) fold into
the current `BucketAccumulator`, keyed on `floor_to_minute(now)`. The flush
decision is a **pure, observed condition**, not a timed wait:

```rust
// open_minute = the minute key of the currently-accumulating bucket.
// frame_minute = floor_to_minute(chrono::Utc::now().timestamp()) at each event.
// Flush iff the observed minute changed (the data crossed the boundary).
fn should_flush(open_minute: i64, frame_minute: i64) -> bool { frame_minute != open_minute }
```

The event source is `tokio::select!` over (i) the next advertisement from the
discovery stream and (ii) a 1 Hz `tokio::time::interval` **tick used only as a
clock sample** so an idle minute still flushes — the tick never gates work, it
just re-evaluates `should_flush` against the wall clock. On `true`: `finish` →
`db.store_bucket` (skip if `is_empty()`), open a fresh accumulator at
`frame_minute`.

**No fixed-delay reconnect.** The TUI's `ble.rs` outer loop uses
`tokio::time::sleep(RESCAN_BACKOFF)` (a bare fixed wait) between discovery
re-establishment attempts — this is **not lifted**; it would violate the
project's no-sleep rule (CLAUDE.md: "observe, don't guess"). Instead the
collector's reconnect awaits the real signal — a `bluer` **session event**
(`AdapterAdded` / power-on) — then retries; the retry of the real adapter
operation **is** the observed readiness check, so the loop proceeds the moment
the adapter is back, not after a guessed interval:

```rust
// After scan_session(...) returns (adapter went away / reset), wait for the
// adapter to come back — on the event, not on a clock — then loop to rescan.
loop {
    match session.default_adapter().await {
        Ok(a) if a.is_powered().await.unwrap_or(false) => break, // back → rescan
        _ => {
            let mut evts = session.events().await?;       // bluer session-event stream
            while let Some(ev) = evts.next().await {
                if matches!(ev, bluer::SessionEvent::AdapterAdded(_)) { break; }
            }
        }
    }
}
```

(Document this divergence from the TUI in CLAUDE.md; the pre-existing TUI sleep
is noted as a separate cleanup, out of scope here. Verify the exact
`bluer::SessionEvent` variant name against the bluer 0.17 docs when implementing
— the shape, not the spelling, is the contract: await the event, never sleep.)

The default station address is `F0:CA:FE:00:00:01` (matches firmware /
`meteo-tui` default), overridable by CLI/env.

**Tests (must pass, host):**

- `accumulator_min_max_avg_over_three_frames` — add 3 frames (temp 10/20/30),
  `finish` → min 10, max 30, avg 20.
- `accumulator_skips_none_fields` — frames with `temperature_c: None` leave
  `temp` column `None` while other fields still accumulate.
- `accumulator_wind_dir_vector_mean_wraps_seam` — headings 350° and 10°
  vector-mean to ≈0°, not 180°.
- `accumulator_empty_is_empty` — no `add` → `is_empty()`.
- `floor_to_minute_floors_and_is_idempotent` — `floor_to_minute(125) == 120`,
  `floor_to_minute(120) == 120`.
- `should_flush_only_on_minute_change` — `should_flush(120, 120) == false`,
  `should_flush(120, 180) == true`.

**Risk:** lifting vs duplicating the bluer scan. The scan core is small and
genuinely platform-shared; to avoid duplication, the per-frame handling differs
(write+publish vs `mpsc` to a TUI), so the ~30-line scan loop is **re-implemented**
in `meteo-web` rather than shared. `decode_frame` reuses `meteo-lib`'s
`Telemetry::decode` + `FRAME_LEN` (a 4-line wrapper — not meaningful
duplication). The reconnect path is deliberately observer-based (above), not a
copy of the TUI's `sleep` backoff. Document this in CLAUDE.md.

**Depends on:** 3.

---

### 5. Server functions + SSE live endpoint (`src/api`, `src/state.rs`)

**Goal:** the read paths the UI binds to.

**`src/state.rs`:**

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: DbHandle,
    pub live_rx: watch::Receiver<Option<Telemetry>>,
}
```

Provided to leptos via `leptos_axum` context so server fns can pull `DbHandle`;
`live_rx` used by the SSE handler.

**`src/api/mod.rs` — server fns (DTOs come from `crate::types`):**

```rust
pub use crate::types::{HistoryRow, MetricStat, TracePoint, Metric, LiveFrame};

#[server]
pub async fn get_history(from_ts: i64, to_ts: i64, bucket_secs: i64)
    -> Result<Vec<HistoryRow>, ServerFnError>;   // thin wrapper over db::history_impl

#[server]
pub async fn get_comparison_trace(date: String /* YYYY-MM-DD */, metric: Metric)
    -> Result<Vec<TracePoint>, ServerFnError>;   // thin wrapper over db::comparison_impl
```

The `#[server]` fns pull `AppState` from leptos context (`expect_context`) and
delegate to the `db::history_impl` / `db::comparison_impl` helpers (substep 3) —
all DTO types are defined once in `src/types.rs`, so there is no db↔api type
cycle.

**`src/api/sse.rs` — 1 Hz live push:**

```rust
#[cfg(feature = "ssr")]
pub async fn live_sse(State(st): State<AppState>)
    -> Sse<impl Stream<Item = Result<Event, Infallible>>>;

/// Pure: build one SSE event from the latest frame (testable without HTTP).
/// `None` → a `: keep-alive`-style empty-data comment event; `Some` → JSON of
/// `LiveFrame::from_telemetry`.
fn live_event(frame: &Option<Telemetry>) -> axum::response::sse::Event;
```

Builds a stream from `tokio_stream::wrappers::WatchStream::new(st.live_rx)` (or a
1 Hz interval reading `*live_rx.borrow()`), mapping each value through
`live_event`. Mounted in `main.rs`: `.route("/live",
axum::routing::get(live_sse))` **before** the leptos routes, sharing `AppState`.

Client (`live_band.rs`, substep 8) consumes via `web_sys::EventSource` (under
`hydrate`).

**Tests (must pass, `--features ssr` host):**

- `live_frame_from_telemetry_maps_fields` — `from_telemetry` preserves present
  fields and `None`s; `solar_w`/`load_w` match `meteo_chart::power_w`.
- `live_frame_from_telemetry_all_none` — a `Telemetry` with every optional field
  `None` yields a `LiveFrame` whose `solar_w`/`load_w` are `None` (no panic in the
  `power_w` calls), `uptime_s` preserved.
- `get_history_smoke` — open a `:memory:` `DbHandle`, seed one `BucketRow`, call
  `db::history_impl(&conn, q)` (substep 3), assert one `HistoryRow` with the
  seeded values (including the `power_w`-derived `solar_w_avg`/`load_w_avg`).
- `live_sse_emits_json_event` — build a `watch::channel(Some(frame))`, construct
  the SSE stream from the receiver, pull the first item via
  `futures::StreamExt::next`, and assert its `Event` data deserializes back to
  the seeded `LiveFrame` (factor the per-tick body into
  `fn live_event(frame: &Option<Telemetry>) -> Event` so it is unit-testable
  without a live HTTP server).

**Risk:** `#[server]` fns and axum handlers are awkward to unit-test. Mitigation:
push the real logic into plain helpers — `async fn history_impl(state, q)`,
`async fn comparison_impl(state, …)`, and `fn live_event(frame)` — each
directly testable; the `#[server]`/handler shells stay trivial.

**Depends on:** 3, 4.

---

### 6. Catppuccin CSS, fonts, app shell + routing

**Goal:** the visual foundation — palette as CSS variables (from
`meteo-chart::palette`, same hex), fonts, and the two-page router/layout.

**Palette CSS — generated, never hand-mirrored (kills drift).** Add
`crates/meteo-web/build.rs` that depends on `meteo-chart` as a
`[build-dependencies]` entry and calls `meteo_chart::palette::css(...)` on each
of the **22** palette consts directly (build scripts are a separate crate — they
cannot import the library under build, so the generator lives **in** `build.rs`
and reads the build-dep `meteo-chart`, not a `meteo_web::build_support` helper).
It writes `style/_palette.scss` = `:root { --base: #1e1e2e; … --red: #f38ba8; }`
for all 22 colours (`BASE MANTLE CRUST BORDER SURFACE0 SURFACE2 TEXT SUBTEXT1
SUBTEXT0 OVERLAY2 OVERLAY1 OVERLAY0 PEACH LAVENDER TEAL SAPPHIRE YELLOW BLUE SKY
GREEN MAUVE RED`). `main.scss` does `@use "palette";`. There is then exactly
**one** source of colour truth (`meteo_chart::palette`); the SVG charts read the
same consts at runtime, so Rust and CSS cannot drift.

**`style/main.scss`:** `@use "palette";` then `@font-face` for JetBrains Mono
(titles/axes/labels) and IBM Plex Sans (body); base layout (header band, summary
band, diagnostics bar deliberately separated, history grids). All UI strings
**French verbatim** (« En direct », « Vitesse du vent », « rafale », N E S O…).

**`src/lib.rs` `App`:** `<Router>` with `<Routes>`:
`/` → `AllPanelsPage`, `/comparaison` → `ComparisonPage`; a shared `<Header/>`
(substep 7) on both. `<Stylesheet id="leptos" href="/pkg/meteo-web.css"/>`.

**Vendor assets:** copy `crates/meteo-tui/assets/compass/compass-dial.svg` and
`compass-needle.svg` into `crates/meteo-web/assets/compass/`. Fonts (vendored to
`assets/fonts/`, no CDN — the Pi may be offline), exact files + sources:

- `JetBrainsMono-Regular.woff2`, `JetBrainsMono-Bold.woff2` — from the JetBrains
  Mono GitHub release (`JetBrains/JetBrainsMono`, `fonts/webfonts/`).
- `IBMPlexSans-Regular.woff2`, `IBMPlexSans-Bold.woff2` — from the IBM Plex
  GitHub release (`IBM/plex`, `IBM-Plex-Sans/fonts/complete/woff2/`).

Each referenced by a `@font-face` in `main.scss` with `font-display: swap`.

**Tests (must pass):**

- `tests/palette_css.rs` (integration test in `meteo-web`, with `meteo-chart`
  added under `[dev-dependencies]`) — asserts the canonical generator output is
  correct **at the source**: `meteo_chart::palette::css(meteo_chart::palette::BASE)
== "#1e1e2e"`, `… RED) == "#f38ba8"`, and that the build's generated
  `style/_palette.scss` (read from disk via `env!("CARGO_MANIFEST_DIR")`) contains
  a `--base:`…`--red:` line for all 22 names. This tests the real generated file
  without trying to import `build.rs` (which Rust forbids).
- Manual: `web-serve`, confirm Catppuccin background, both fonts load, both
  routes render their placeholder.

**Depends on:** 1 (palette), 2 (crate exists).

---

### 7. SVG chart component `PlotPanel` + header

**Goal:** the reusable historic chart, reproducing the TUI aesthetic in SVG.

**`src/components/chart.rs`:**

```rust
pub struct ChartSeries {
    pub points: Vec<(f64, f64)>,         // (x, avg)
    pub band: Option<Vec<(f64, f64, f64)>>, // (x, min, max) min–max envelope
    pub color_hex: String,               // from meteo_chart::palette
    pub floor: Option<f64>,
    pub prec: usize,
}

#[component]
pub fn PlotPanel(title: String, unit: String, series: ChartSeries,
                 #[prop(optional)] smooth_sigma: f64) -> impl IntoView;
```

Rendering pipeline (pure → SVG):

1. `meteo_chart::gaussian_smooth(&points, smooth_sigma)` (default sigma matching
   TUI) for the avg trace.
2. `meteo_chart::padded_value_bounds(min, max, floor)` → y-domain;
   `meteo_chart::value_axis_labels(bounds, prec)` → 3 corner/mid labels.
3. Map (x,y) → viewBox coords via a pure helper
   `project(x: f64, y: f64, xdom: [f64;2], ydom: [f64;2], w: f64, h: f64) ->
(f64, f64)` (returns SVG pixel `(px, py)`, y inverted), unit-tested.
4. Emit: `<rect>` CRUST well; dotted gridlines at 25/50/75 %; min–max **band**
   as a filled `<path>` (`palette` colour at low alpha — same idiom as the TUI
   gust band); gradient fill under the avg trace via `<linearGradient>` (≈13 %→0);
   the avg trace as a `<polyline>`/dotted `<path>` in the metric colour; axis
   min/max corner labels.

One hue per quantity from `meteo_chart::palette` (Air = Peach, Sky = Lavender,
Pressure = Teal, Humidity = Sapphire, Lux/Solar = Yellow, Rain = Blue,
Wind = Sky, Battery = Green, Load = Mauve).

**`src/components/header.rs`:** `<Header/>` — wall clock (client-side
`chrono`/`js_sys::Date` under hydrate), app version (`env!("CARGO_PKG_VERSION")`),
signal state (Live/Stale from live-frame age). Mirrors the TUI header.

**Tests (must pass, host):**

- `project_maps_domain_corners` — `project` sends `(xmin,ymin)`→bottom-left,
  `(xmax,ymax)`→top-right of the viewBox (y inverted).
- `plotpanel_renders_polyline_for_series` — render `PlotPanel` to a string
  (leptos `render_to_string` under ssr), assert the output contains a
  `<polyline`/`<path` and the metric colour hex.
- `plotpanel_empty_series_renders_placeholder` — empty points → no panic, well
  rendered.

**Depends on:** 1, 6.

---

### 8. Wind compass component

**Goal:** the image-based compass reproduced on the web — inline dial SVG +
needle SVG rotated by CSS, with the live readout overlay. (Built before the
all-panels page, which consumes it.)

**`src/components/compass.rs`:**

```rust
#[component]
pub fn WindCompass(dir_deg: Signal<Option<f32>>, speed_ms: Signal<Option<f32>>,
                   #[prop(optional)] gust_ms: Option<f32>) -> impl IntoView;
```

Two stacked layers (`position: absolute`, shared centre): dial
`<img src="/compass/compass-dial.svg">` (static) and needle
`<img src="/compass/compass-needle.svg" style:transform=move || format!(
"rotate({}deg)", dir_deg.get().unwrap_or(0.0))>` (North-up at 0°, clockwise
positive — no extra offset needed; the SVG is authored North-up). Overlay text:
speed · `cap°` · 16-pt FR rose via `meteo_chart::compass_label_fr` · « rafale »
gust line. No `ratatui-image` raster path (web inlines the SVG directly).

**Tests (must pass, host):**

- `compass_renders_rotation_transform` — `render_to_string` with `dir = 90°`
  contains `rotate(90deg)`.
- `compass_label_fr` already covered in `meteo-chart` (substep 1) — reused.

**Depends on:** 1, 6.

---

### 9. All-panels page (live band + historic grid + time select)

**Goal:** Page 1 — live instantaneous header fed by SSE, historic charts below
for the selected period, with presets + custom range + pan/zoom.

**`src/components/time_select.rs`:**

```rust
#[derive(Clone, Copy)]
pub struct TimeWindow { pub from_ts: i64, pub to_ts: i64 }

impl TimeWindow {
    pub fn span_secs(&self) -> i64 { self.to_ts - self.from_ts }
    /// Bucket size for query-time aggregation, chosen so the rendered point
    /// count stays bounded (~target). A fixed span→bucket ladder (deterministic,
    /// no float target maths — the implementer makes zero choices):
    pub fn bucket_secs(&self) -> i64 {
        match self.span_secs() {
            s if s <=        2 * 3600 =>     60, // ≤2 h     → 1 min  (≤120 pts)
            s if s <=       12 * 3600 =>    300, // ≤12 h    → 5 min  (≤144 pts)
            s if s <=   3 * 86400     =>    900, // ≤3 d     → 15 min (≤288 pts)
            s if s <=  14 * 86400     =>   3600, // ≤2 wk    → 1 h    (≤336 pts)
            s if s <=  92 * 86400     =>  21600, // ≤3 mo    → 6 h    (≤368 pts)
            s if s <= 366 * 86400     =>  86400, // ≤1 yr    → 1 d    (≤366 pts)
            _                          => 604800, // >1 yr    → 1 wk
        }
    }
}

#[component]
pub fn TimeSelect(window: RwSignal<TimeWindow>) -> impl IntoView;  // Jour/Semaine/Mois + custom range
```

The ladder caps the rendered point count (~≤370) at every zoom level, so
multi-year zoom-outs stay cheap (query-time aggregation, substep 3).

**Zoom/pan are pure, testable transforms on `TimeWindow`:**

```rust
/// Zoom about a cursor fraction `f ∈ [0,1]` of the current span by `factor`
/// (<1 zoom-in, >1 zoom-out); the timestamp under the cursor stays fixed.
pub fn zoom_about(w: TimeWindow, f: f64, factor: f64) -> TimeWindow;
/// Pan by a fraction of the current span (positive = forward in time).
pub fn pan_by(w: TimeWindow, frac: f64) -> TimeWindow;
```

**`src/components/live_band.rs`:** subscribes to `/live` via
`web_sys::EventSource` (hydrate), deserializes `LiveFrame`, shows the
instantaneous band — air temp (Peach), wind compass (`<WindCompass/>`, substep
8), power (Solar/Load/Battery via
`meteo_chart::power_w`/`fmt_power`/`fmt_battery_flow`). French label « En direct ».

**`src/pages/all_panels.rs`:** a `Resource` keyed on `window` calling
`get_history`; renders the `<LiveBand/>` then the chart grid — CAPTEURS (6:
air temp, pressure, humidity, sky temp, lux, wind+gust band) + ÉNERGIE (3:
solar, battery, load) `PlotPanel`s, each fed the matching metric from the
`HistoryRow`s, with the min–max band from each `MetricStat`'s `min`/`max`.
Pan/zoom: wheel calls `zoom_about`, drag calls `pan_by` — both `set` the
`window` signal, which re-fires the `Resource`. No fixed debounce sleep; refetch
is driven by the signal change (leptos batches).

**Tests (must pass, host):**

- `time_window_bucket_secs_scales_with_span` — a 1-day span → 900 s buckets, a
  1-year span → 86400 s buckets, a 1-hour span → 60 s; every case yields ≤ ~370
  rows over its span.
- `time_window_presets_compute_expected_ranges` — `Jour`/`Semaine`/`Mois`
  produce the right `from_ts`/`to_ts` relative to a fixed `now`.
- `zoom_about_keeps_cursor_timestamp_fixed` — zoom-in by 0.5 about `f=0.5`
  leaves the midpoint timestamp unchanged and halves the span.
- `pan_by_shifts_both_bounds_equally` — `pan_by(w, 0.25)` moves `from`/`to` by
  +¼ span, span unchanged.
- Manual: `web-serve`, confirm live band updates at 1 Hz, charts redraw on
  preset/zoom/pan.

**Depends on:** 5, 7, 8.

---

### 10. Comparison page (flexible date/metric overlays)

**Goal:** Page 2 — overlay independent `(date, metric)` traces on a shared
time-of-day X axis, auto dual-Y when overlaid metrics differ.

**`src/pages/comparison.rs`:**

```rust
#[derive(Clone)]
struct TraceSel { date: NaiveDate, metric: Metric }   // one row per overlaid trace

/// How the overlaid traces share the Y axis, decided from the distinct metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisLayout {
    Shared,                       // 1 distinct metric → one Y axis
    DualY(Metric, Metric),        // exactly 2 distinct → left/right axes
    Normalized,                   // ≥3 distinct → each trace mapped to 0–1
}
/// Pure: choose the layout from the selected traces' metrics.
pub fn axis_layout(metrics: &[Metric]) -> AxisLayout;

#[component]
pub fn ComparisonPage() -> impl IntoView;   // add/remove TraceSel rows; each → get_comparison_trace
```

State: `RwSignal<Vec<TraceSel>>`. Each selection drives a `get_comparison_trace`
`Resource` (X = seconds-of-day so different days align). Render all traces in one
SVG via the substep-7 projection helpers: a single time-of-day X domain (0…86400)
and the `axis_layout(...)` choice above — `DualY` uses each metric's own
`padded_value_bounds` on left/right axes; `Normalized` maps each trace to 0–1
with a per-trace legend. Each trace coloured by its metric's `palette` hue;
legend lists date + metric + colour. Covers same-metric/different-days and
different-metrics/same-day from one mechanism.

**Tests (must pass, host):**

- `axis_layout_picks_shared_dual_normalized` — `axis_layout` returns `Shared`
  for 1 distinct metric, `DualY(a, b)` for exactly 2, `Normalized` for ≥3.
- `axis_layout_empty_is_shared` — `axis_layout(&[])` returns `Shared` (no traces
  selected → degenerate single axis, no panic).
- `comparison_trace_x_is_seconds_of_day` — the `get_comparison_trace` inner
  helper (`comparison_impl`) maps a stored bucket at 13:00 to `x = 46800`.

**Depends on:** 5, 7.

---

### 11. Docs, build-recipe finalisation, full checks

**Goal:** document the new surface and prove the whole workspace is green.

- **`CLAUDE.md`:** add `meteo-chart` + `meteo-web` to the module map; a "Web
  dashboard" section covering the SSR/cargo-leptos topology, the single-binary
  collector+server, the SQLite schema + query-time aggregation, the server
  fns/SSE endpoints, the build recipes, and the build-wiring caveat (cargo-leptos
  overrides the riscv `build.target`; `bin-target-triple` pins the host/Pi
  triple; `just build` stays firmware-only). Document the Pi deployment:
  `just web-build-pi` (aarch64 server cross-build — needs the rustup target +
  cross linker), the systemd unit, the DB path, and the BlueZ runtime
  prerequisites (`bluetoothd` + powered LE adapter).
- **`ROADMAP.md`:** mark "Web server for historic data (Raspberry Pi collector)"
  as implemented.
- **`Justfile`:** confirm `web-build`/`web-build-pi`/`web-serve`/`web-watch`/
  `web-clippy` and the extended `clippy`/`test` recipes (meteo-chart + meteo-web
  ssr).

**Final gate (all must pass — global "before pushing" rule):**

```bash
just format
just clippy          # firmware + meteo-lib + meteo-tui (+ meteo-chart)
just web-clippy      # meteo-web ssr (host) + hydrate (wasm32)
just test            # meteo-lib + meteo-tui + meteo-chart + meteo-web (ssr host)
just build           # firmware still builds
just web-build       # SSR + wasm build succeeds
```

**Depends on:** all.

## Testing

- **Unit (host, `cargo nextest`):** all moved `meteo-chart` tests; DB roundtrip +
  re-aggregation; `BucketAccumulator` min/max/avg + vector-mean wind dir + None
  handling; DTO mapping; `query_history` impl; chart `project`; `TimeWindow`
  bucket scaling + presets; comparison axis layout + time-of-day mapping;
  `PlotPanel`/`WindCompass` `render_to_string` assertions.
- **Build/integration:** `just build` (firmware unaffected), `cargo leptos build`
  (SSR+wasm), `web-clippy` on both feature sets.
- **Manual (acceptance):** `just web-serve` on the dev host (which _is_ gaia, the
  BLE host) → live band updates at 1 Hz off real adverts; historic charts render
  after the collector has written ≥1 bucket; presets/zoom/pan refetch; comparison
  overlays render with correct axes; visual parity with the TUI / design dossier.
- **Edge cases:** empty DB (no buckets) → charts render empty wells, no panic;
  all-`None` frame fields → `NULL` columns, charts skip gaps; adapter reset
  mid-scan → collector recovers (lifted resilience); 0/360 wind-dir seam;
  multi-year zoom-out stays bounded in point count.

## Risks

- **cargo-leptos vs the embedded `build.target`.** The workspace defaults to
  `riscv32imac…`. cargo-leptos passes explicit `--target` and `bin-target-triple`
  pins the host/Pi triple, overriding it; `just build` is firmware-scoped and
  untouched. Verify early in substep 2 with `cargo leptos build -v`. _If_ a plain
  `cargo build -p meteo-web` (no leptos) is ever needed it must carry an explicit
  `--target`; the clippy/test recipes already do.
- **Stable vs nightly leptos — RESOLVED.** Nightly is installed and current, so
  the web crate keeps `leptos`'s `nightly` feature (call-syntax ergonomics) and
  the `web-*` recipes run under `cargo +nightly leptos …`. Firmware / lib / tui /
  chart stay on stable; nightly is scoped to the web recipes only (no repo-wide
  `rust-toolchain.toml`). The signal-call syntax choice is therefore fixed, not
  open.
- **Workspace restriction-lint set on leptos macro code.** The heavy
  `clippy::restriction`/`pedantic`/`nursery` groups may fire inside `view!`/
  `#[server]` expansions. Mitigate with targeted crate-root `#![allow(…, reason
= …)]` (matching the firmware's existing carve-outs), not by weakening the
  workspace config.
- **No-duplication discipline.** Shared pure logic is centralised in
  `meteo-chart`; the ~30-line bluer scan is re-implemented in `meteo-web` only
  because the TUI's copy is entangled with ratatui and the per-frame handling
  differs (write+publish vs `mpsc`). `decode`/`FRAME_LEN`/`Telemetry` are reused
  from `meteo-lib`. Documented so it is a deliberate, minimal seam.
- **Wind-direction aggregation.** Vector mean at write time fixes the 0/360 seam;
  query-time coarser re-aggregation of _direction_ uses a plain mean (display-only,
  already minute-smoothed) — documented as an accepted approximation.
- **rusqlite blocking on the async runtime.** All DB calls go through
  `spawn_blocking`; the writer is 1/min and reads are bounded, so a single
  `Arc<Mutex<Connection>>` + WAL suffices (no pool needed).
- **Offline Pi.** Fonts and compass assets are vendored locally (no CDN), served
  from `assets/`.
- **BlueZ host prerequisites.** `meteo-web`'s `ssr` build pulls `bluer`, which
  needs `libdbus-1-dev` at **build** time and a running `bluetoothd` + a powered
  LE adapter at **runtime** (same as `meteo-tui`). The SSR binary cannot acquire
  the adapter without them. The dev host **is gaia** (the BLE host), so local
  `web-serve` works; the Pi target needs BlueZ installed and running. Call this
  out in CLAUDE.md's deployment notes.

## Notes

Progress tracking (checked during implementation):

- [ ] 1. `meteo-chart` extraction + `meteo-tui` migration
- [ ] 2. `meteo-web` skeleton + cargo-leptos wiring (SSR hello)
- [ ] 3. SQLite storage layer
- [ ] 4. Collector task + pure bucket accumulator
- [ ] 5. Server functions + SSE live endpoint
- [ ] 6. Catppuccin CSS + fonts + app shell/routing
- [ ] 7. `PlotPanel` SVG chart + header
- [ ] 8. Wind compass component
- [ ] 9. All-panels page (live band + grid + time select)
- [ ] 10. Comparison page
- [ ] 11. Docs + final checks

Next step after approval: `/tyrex:code:implement-light` (substeps are ordered by
dependency; 1→2→3→4 are a chain, 6/7/8 can interleave, 9/10 land last).
