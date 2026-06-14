# Plan: BLE TUI Data Viewer

- **Source:** '1 (`.claude/brainstorm/1-ble-tui-data-viewer.md`)
- **Date:** 2026-06-14
- **Status:** Planned

## Summary

Add a new host-only `meteo-tui` crate: a ratatui application that connects to
the `MeteoStation` device over BLE, auto-reconnects across the firmware's ~30 s
disconnect, and renders per-sensor current value, a history line chart, and
min/max/avg stats. A new shared sensor registry in `meteo-lib`
(`ble::registry`) maps each characteristic UUID to display metadata (name,
unit, precision, optional value transform) and is the single source of truth
the TUI iterates to lay out panels — so adding a sensor later is one registry
entry. `meteo-cli` and the firmware are left untouched (the registry is built
so a firmware refactor to draw from it is a clean follow-up).

## Decisions locked (from clarification)

- **BLE client lives in `meteo-tui`** (a local `client.rs` module), not a
  feature-gated `std` module in the `no_std` `meteo-lib`.
- **History is a fixed-count ring buffer** (`HISTORY_CAPACITY = 600`,
  ≈ 10 min at the ~1 Hz cadence). No timestamps. Preserved across reconnects.
- **Charts use ratatui `Chart` + `Dataset`** (real f64 axis, handles negative
  temperatures), not `Sparkline`.

## Files Modified

| File                                   | Action | Description                                                                    |
| -------------------------------------- | ------ | ------------------------------------------------------------------------------ |
| `crates/meteo-lib/src/ble/registry.rs` | create | Sensor registry: `SensorDescriptor`, `SENSORS`, `index_for_uuid`, `pa_to_hpa`. |
| `crates/meteo-lib/src/ble/mod.rs`      | modify | `pub mod registry;` + re-exports.                                              |
| `crates/meteo-tui/Cargo.toml`          | create | New host binary crate manifest.                                                |
| `crates/meteo-tui/src/main.rs`         | create | Terminal init/restore, spawn client, select loop.                              |
| `crates/meteo-tui/src/app.rs`          | create | `App`, `SensorState`, `ClientEvent`, `ConnectionStatus` (pure, tested).        |
| `crates/meteo-tui/src/client.rs`       | create | Auto-reconnecting btleplug central.                                            |
| `crates/meteo-tui/src/ui.rs`           | create | Registry-driven ratatui rendering.                                             |
| `Cargo.toml` (workspace)               | modify | Add `crates/meteo-tui` member.                                                 |
| `Justfile`                             | modify | `tui` / `tui-gaia` recipes; add `meteo-tui` to `clippy` and `test`.            |

## Plan

### 1. Sensor registry in `meteo-lib`

**Depends on:** nothing. **Foundation for substeps 3–5.**

**File:** create `crates/meteo-lib/src/ble/registry.rs`. The crate is `no_std`
(std only under `#[cfg(test)]`), so the registry uses only `&'static str`, a
`&'static [..]` slice, and `fn(f32) -> f32` pointers — no `String`/`Vec`/`alloc`.
It reuses the UUID constants already in `ble::gatt` (stays DRY).

**Signatures / data:**

```rust
//! Single source of truth describing each BLE sensor characteristic: its UUID
//! and how to present its readings. Host viewers iterate this table to build
//! panels; adding a sensor is one entry here (plus its UUID in `gatt`).

use super::gatt::{PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};

/// Identity + presentation metadata for one sensor characteristic.
#[derive(Debug, Clone, Copy)]
pub struct SensorDescriptor {
    /// 128-bit characteristic UUID (same big-endian byte order as `gatt`).
    pub uuid: [u8; 16],
    /// Human-readable name, e.g. "Temperature".
    pub name: &'static str,
    /// Display unit, e.g. "°C" or "hPa".
    pub unit: &'static str,
    /// Fractional digits to display.
    pub precision: u8,
    /// Optional transform raw-wire-f32 → display value (e.g. Pa → hPa).
    pub transform: Option<fn(f32) -> f32>,
}

impl SensorDescriptor {
    /// Apply the transform (identity when `None`).
    #[must_use]
    pub fn display_value(&self, raw: f32) -> f32 {
        match self.transform {
            Some(f) => f(raw),
            None => raw,
        }
    }
}

/// Pascals → hectopascals (float division does not trip
/// `arithmetic_side_effects`; see `meteo-cli` line `p / 100.0`).
#[must_use]
pub fn pa_to_hpa(pa: f32) -> f32 {
    pa / 100.0
}

/// All sensors the station can expose, in display order.
pub static SENSORS: &[SensorDescriptor] = &[
    SensorDescriptor {
        uuid: TEMPERATURE_CHAR_UUID,
        name: "Temperature",
        unit: "°C",
        precision: 2,
        transform: None,
    },
    SensorDescriptor {
        uuid: PRESSURE_CHAR_UUID,
        name: "Pressure",
        unit: "hPa",
        precision: 2,
        transform: Some(pa_to_hpa),
    },
];

/// Registry index of the sensor whose characteristic UUID matches, if any.
#[must_use]
pub fn index_for_uuid(uuid: &[u8; 16]) -> Option<usize> {
    SENSORS.iter().position(|s| &s.uuid == uuid)
}
```

**Re-export** — in `crates/meteo-lib/src/ble/mod.rs` add:

```rust
pub mod registry;
// ... existing pub use lines ...
pub use registry::{SENSORS, SensorDescriptor, index_for_uuid, pa_to_hpa};
```

**Tests** (append a test module to `registry.rs`, using the established
`meteo-lib` pattern — `extern crate std;`, `use test_log::test;`,
`TestResult`, grcov markers). Test names + assertions:

- `index_for_uuid_temperature_returns_zero` — `index_for_uuid(&TEMPERATURE_CHAR_UUID) == Some(0)`.
- `index_for_uuid_pressure_returns_one` — `index_for_uuid(&PRESSURE_CHAR_UUID) == Some(1)`.
- `index_for_uuid_unknown_returns_none` — an all-`0xFF` UUID → `None`.
- `pa_to_hpa_converts` — `pa_to_hpa(101_325.0)` ≈ `1013.25` (compare with epsilon).
- `display_value_applies_pressure_transform` — `SENSORS[1].display_value(101_325.0)` ≈ `1013.25`.
- `display_value_identity_without_transform` — `SENSORS[0].display_value(21.5) == 21.5`.

### 2. Scaffold the `meteo-tui` crate + build wiring

**Depends on:** 1 (manifest references `meteo-lib`). **Blocks:** 3–6.

**File:** create `crates/meteo-tui/Cargo.toml` (mirrors `meteo-cli`, adds
ratatui/crossterm). Versions verified against crates.io on 2026-06-14:
ratatui `0.30.1`, crossterm `0.29.0`, btleplug `0.11` (already used by cli).

```toml
[package]
name = "meteo-tui"
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
meteo-lib = { workspace = true }
btleplug = "0.11"
ratatui = "0.30"
# Bare crossterm dep exists only to switch on the `event-stream` feature; all
# crossterm *types* are imported via `ratatui::crossterm` so there is a single
# crossterm version. Verify with `cargo tree -p meteo-tui -i crossterm` — it
# must show exactly one 0.29.x. If ratatui resolves a different crossterm, add
# `features = ["crossterm_0_29"]` to the ratatui dep above.
crossterm = { version = "0.29", features = ["event-stream"] }
futures = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
uuid = "1"

[lints]
workspace = true
```

**Feature wiring — confirmed against the ratatui 0.30.1 manifest (2026-06-14):**

- ratatui's default features include `crossterm`, which pulls
  `ratatui-crossterm` whose default selects **crossterm 0.29.x** — so plain
  `ratatui = "0.30"` already gives a crossterm-0.29 backend. `crossterm_0_29`
  is a real explicit feature (`crossterm_0_29 = ["crossterm",
"ratatui-crossterm/crossterm_0_29"]`); add it to the ratatui dep **only** if
  `cargo tree -p meteo-tui -i crossterm` ever shows more than one crossterm.
- ratatui does **not** enable crossterm's `event-stream` feature; it must be
  enabled on the crossterm crate directly (done above). Cargo feature
  unification then turns `event-stream` on for the single crossterm 0.29 that
  `ratatui::crossterm` re-exports, so `ratatui::crossterm::event::EventStream`
  is available with no second crossterm version.

**Workspace** — in `/Cargo.toml`, add `"crates/meteo-tui"` to `members`. No
other root change is needed: `meteo-lib = { workspace = true }` above resolves
from the existing `[workspace.dependencies]` entry
(`meteo-lib = { path = "crates/meteo-lib", default-features = false }`).

**Stub `src/main.rs`** so the crate compiles before later substeps:

```rust
//! BLE TUI viewer for the MeteoStation weather station.
mod app;
mod client;
mod ui;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    Ok(())
}
```

(`app`/`client`/`ui` modules are filled in substeps 3–5; create them as empty
files first so this compiles, or land 3–5 before wiring `main` in 6.)

**Justfile** — the default cargo target is `thumbv7em-none-eabihf`
(`.cargo/config.toml`), so every host crate must pass `--target {{ host_target }}`.

Add after the `cli-gaia` recipe:

```just
[doc('Run the BLE TUI viewer')]
tui:
    cargo run -p meteo-tui --target {{ host_target }}

[doc('Run the BLE TUI viewer on the Gaia host (the machine with the BT adapter)')]
tui-gaia:
    ssh gaia "bash -c 'cd ~/code/meteo_station && cargo run -q -p meteo-tui --target {{ host_target }}'"
```

Update the host `clippy` line to include the new crate:

```just
    cargo clippy -p meteo-lib -p meteo-cli -p meteo-tui --target {{ host_target }} -- -D warnings
```

Replace the `test` recipe body so it runs all host-crate tests (the current
`--lib` form skips the binary crates' unit tests; `-p` selection avoids building
the embedded firmware for the host target):

```just
test:
    cargo nextest run -p meteo-lib -p meteo-cli -p meteo-tui --target {{ host_target }}
```

**Test for this substep:** `just clippy` and `cargo build -p meteo-tui
--target x86_64-unknown-linux-gnu` succeed with the stub.

### 3. App state & logic (`app.rs`) — pure, tested

**Depends on:** 1, 2. **Blocks:** 4 (`ClientEvent`), 5 (`App`/`SensorState`), 6.

**File:** create `crates/meteo-tui/src/app.rs`. All logic here is pure and
host-`std`; it is the unit-tested core (UI, client, and `main` are
terminal/hardware-interfacing and verified manually per project convention).

```rust
//! UI state and pure update logic for the TUI.
use std::collections::VecDeque;

use meteo_lib::ble::registry::SENSORS;

/// Max readings retained per sensor (≈ 10 min at the ~1 Hz cadence).
pub const HISTORY_CAPACITY: usize = 600;

/// Connection state shown in the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not connected: initial scan or post-disconnect rescan.
    Scanning,
    /// Connected and receiving notifications.
    Connected,
}

/// Message from the BLE client task to the UI loop.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// Link established and subscribed.
    Connected,
    /// Link lost; client is rescanning. History is kept.
    Disconnected,
    /// A new raw-wire reading for the sensor at registry `index`.
    Reading { index: usize, raw: f32 },
}

/// Rolling display-value history for one sensor (post-transform values).
#[derive(Debug, Default, Clone)]
pub struct SensorState {
    values: VecDeque<f32>,
}

impl SensorState {
    /// Append a display value, evicting the oldest beyond `HISTORY_CAPACITY`.
    pub fn push(&mut self, value: f32) {
        if self.values.len() == HISTORY_CAPACITY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    #[must_use]
    pub fn latest(&self) -> Option<f32> { self.values.back().copied() }
    #[must_use]
    pub fn len(&self) -> usize { self.values.len() }
    #[must_use]
    pub fn is_empty(&self) -> bool { self.values.is_empty() }
    #[must_use]
    pub fn min(&self) -> Option<f32> { self.values.iter().copied().reduce(f32::min) }
    #[must_use]
    pub fn max(&self) -> Option<f32> { self.values.iter().copied().reduce(f32::max) }

    /// Mean of retained values (`None` when empty). `u16::try_from` keeps the
    /// divisor cast lossless (len ≤ `HISTORY_CAPACITY` < `u16::MAX`), avoiding
    /// a `cast_precision_loss` warning.
    #[must_use]
    pub fn avg(&self) -> Option<f32> {
        let n = u16::try_from(self.values.len()).ok()?;
        if n == 0 { return None; }
        Some(self.values.iter().sum::<f32>() / f32::from(n))
    }

    /// `(x, y)` points for a ratatui `Dataset` (x = sample index).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample index ≤ HISTORY_CAPACITY, exact in f64"
    )]
    pub fn points(&self) -> Vec<(f64, f64)> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, &v)| (i as f64, f64::from(v)))
            .collect()
    }
}

/// Top-level UI state: per-sensor history parallel to `SENSORS`, plus status.
pub struct App {
    pub sensors: Vec<SensorState>,
    pub status: ConnectionStatus,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self { Self::new() }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sensors: SENSORS.iter().map(|_| SensorState::default()).collect(),
            status: ConnectionStatus::Scanning,
            should_quit: false,
        }
    }

    /// Apply one client event. Unknown / out-of-range sensor indices are
    /// ignored (mirrors the firmware's "unknown characteristic — ignore").
    pub fn apply(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Connected => self.status = ConnectionStatus::Connected,
            ClientEvent::Disconnected => self.status = ConnectionStatus::Scanning,
            ClientEvent::Reading { index, raw } => {
                if let (Some(desc), Some(state)) =
                    (SENSORS.get(index), self.sensors.get_mut(index))
                {
                    state.push(desc.display_value(raw));
                }
            }
        }
    }
}
```

**Tests** (test module at the bottom of `app.rs`). All `meteo-tui` test modules
(here, `client.rs`, `ui.rs`) follow `meteo-cli`'s convention: a grcov-excluded
`#[cfg(test)] mod tests` block (`// grcov exclude start` / `// grcov exclude
stop` markers) using plain `#[test]` — no `test-log`/`TestResult` (those are the
`meteo-lib` `no_std` pattern; the `meteo-lib` registry tests in substep 1 keep
it). Names + assertions:

- `push_appends_and_reports_latest` — push 3 values; `latest() == Some(last)`, `len() == 3`.
- `push_evicts_oldest_at_capacity` — push `HISTORY_CAPACITY + 5`; `len() == HISTORY_CAPACITY`, `latest()` is the final value.
- `min_max_avg_over_values` — push `[10.0, 20.0, 30.0]`; `min == Some(10.0)`, `max == Some(30.0)`, `avg ≈ 20.0`.
- `stats_none_when_empty` — fresh `SensorState`: `min/max/avg/latest` all `None`, `is_empty()`.
- `apply_reading_transforms_pressure` — `App::new()`, `apply(Reading { index: 1, raw: 101_325.0 })`; `sensors[1].latest()` ≈ `1013.25`.
- `apply_connected_sets_status` — `apply(Connected)` → `status == Connected`.
- `apply_disconnected_keeps_history` — `App::new()`, `apply(Reading { index: 0, raw: 20.0 })`, then `apply(Disconnected)`; assert `status == Scanning` **and** `sensors[0].len() == 1` (history retained across the drop).
- `apply_reading_out_of_range_index_ignored` — `apply(Reading { index: 99, raw: 1.0 })` does not panic and leaves all histories empty.
- `app_sensor_count_matches_registry` — `App::new().sensors.len() == SENSORS.len()` (guards against a registry entry added without a matching state slot).
- `points_maps_index_and_value` — push `[10.0, 20.0]`; assert `points() == vec![(0.0, 10.0), (1.0, 20.0)]` (sample-index → `(x, y)` mapping used by the chart).

### 4. Auto-reconnecting BLE client (`client.rs`)

**Depends on:** 1 (`SENSORS`, `index_for_uuid`), 2, 3 (`ClientEvent`).
**Blocks:** 6.

**File:** create `crates/meteo-tui/src/client.rs`. Reuses the connect/subscribe
flow from `meteo-cli/src/main.rs`, wrapped in a forever loop that rescans after
any session ends. **No `println!`/`eprintln!`** — stdout/stderr would corrupt
the alternate screen, and those are `print_stdout`/`print_stderr` lints; errors
are surfaced only as a `Disconnected` event.

```rust
//! Auto-reconnecting btleplug central feeding readings to the UI.
use std::error::Error;
use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt as _;
use meteo_lib::ble::encoding::decode_f32;
use meteo_lib::ble::registry::{SENSORS, index_for_uuid};
use tokio::sync::mpsc::Sender;
use tokio::time;
use uuid::Uuid;

use crate::app::ClientEvent;

/// Poll cadence while waiting for the device to (re)appear during a scan.
/// This is a bounded poll-with-check (each tick inspects `peripherals()`), the
/// allowed form of waiting — not a fixed guess at how long a step takes. The
/// project's no-`sleep` rule bans *bare fixed delays used as synchronisation*;
/// this loop is exempt because every iteration checks a real condition (device
/// present in scan results) before sleeping again.
const SCAN_POLL: Duration = Duration::from_millis(200);

/// Run forever: scan → connect → stream → on drop, rescan.
/// Returns only on an unrecoverable setup error (no adapter); the UI keeps
/// running and shows `Scanning`.
pub async fn run(tx: Sender<ClientEvent>) -> Result<(), Box<dyn Error>> {
    let manager = Manager::new().await?;
    let adapter = first_adapter(&manager).await?;
    loop {
        // A session error (or normal disconnect) drops us back to rescan; the
        // outcome is intentionally ignored. `#[expect]` documents the discard
        // and satisfies the workspace `let_underscore_must_use` lint (which
        // fires on `let _ = <Result>` under `-D warnings`).
        #[expect(
            clippy::let_underscore_must_use,
            reason = "session end/error both mean: rescan"
        )]
        let _ = session(&adapter, &tx).await;
        if tx.send(ClientEvent::Disconnected).await.is_err() {
            return Ok(()); // UI gone (user quit) — stop.
        }
    }
}

async fn first_adapter(manager: &Manager) -> Result<Adapter, Box<dyn Error>> {
    manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "no BLE adapters found".into())
}

/// One connection lifecycle: wait for the device, connect, subscribe to every
/// registered NOTIFY characteristic, then forward readings until the stream
/// ends (disconnect).
async fn session(adapter: &Adapter, tx: &Sender<ClientEvent>) -> Result<(), Box<dyn Error>> {
    let device = wait_for_station(adapter).await?;
    device.connect().await?;
    device.discover_services().await?;

    let chars = device.characteristics();
    let mut subscribed = 0_usize;
    for desc in SENSORS {
        let uuid = Uuid::from_bytes(desc.uuid);
        let Some(ch) = chars.iter().find(|c| c.uuid == uuid) else { continue };

        // Initial read for an immediate value (best-effort).
        if let Ok(data) = device.read(ch).await
            && let Some(index) = index_for_uuid(&desc.uuid)
            && let Some(raw) = decode_reading(&data)
        {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "best-effort seed value; a closed channel is handled later"
            )]
            let _ = tx.send(ClientEvent::Reading { index, raw }).await;
        }
        if ch.properties.contains(CharPropFlags::NOTIFY) {
            device.subscribe(ch).await?;
            subscribed = subscribed.saturating_add(1);
        }
    }
    if subscribed == 0 {
        return Err("no registered characteristics found on device".into());
    }
    tx.send(ClientEvent::Connected).await?;

    let mut events = device.notifications().await?;
    while let Some(n) = events.next().await {
        if let Some(index) = index_for_uuid(n.uuid.as_bytes())
            && let Some(raw) = decode_reading(&n.value)
            && tx.send(ClientEvent::Reading { index, raw }).await.is_err()
        {
            break; // UI gone — let `run` observe the closed channel.
        }
    }
    // Stream ended → device disconnected. Best-effort tidy disconnect.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "tidy disconnect; already disconnected if this errors"
    )]
    let _ = device.disconnect().await;
    Ok(())
}

/// Scan, polling `peripherals()` until a `MeteoStation` appears. Waits as long
/// as needed (the device may be powered off); the UI shows `Scanning`.
async fn wait_for_station(adapter: &Adapter) -> Result<Peripheral, Box<dyn Error>> {
    adapter.start_scan(ScanFilter::default()).await?;
    loop {
        for p in adapter.peripherals().await? {
            if let Some(props) = p.properties().await?
                && props
                    .local_name
                    .as_deref()
                    .is_some_and(|name| name.contains("MeteoStation"))
            {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "stop_scan failure is harmless; we have the peripheral"
                )]
                let _ = adapter.stop_scan().await;
                return Ok(p);
            }
        }
        time::sleep(SCAN_POLL).await;
    }
}

/// Decode an f32 from a 4-byte LE characteristic value (reuses `meteo-lib`).
fn decode_reading(data: &[u8]) -> Option<f32> {
    let bytes: &[u8; 4] = data.first_chunk()?;
    Some(decode_f32(bytes))
}
```

**Tests:** the btleplug session/scan code is hardware-interfacing and is not
auto-tested (project rule) — verified via `just tui-gaia` against the live
device. The pure `decode_reading` helper **is** unit-tested (the same helper is
tested in `meteo-cli`), in a `#[cfg(test)]` module at the bottom of
`client.rs`, plain `#[test]` style:

- `decode_reading_round_trip` — `encode_f32(23.45)` → `decode_reading` returns
  `Some(23.45)` (import `meteo_lib::ble::encoding::encode_f32`).
- `decode_reading_too_short_returns_none` — `decode_reading(&[0x01, 0x02])` is
  `None`.

### 5. Registry-driven UI rendering (`ui.rs`)

**Depends on:** 1 (`SENSORS`), 3 (`App`, `SensorState`, `ConnectionStatus`).
**Blocks:** 6.

**File:** create `crates/meteo-tui/src/ui.rs`. Layout is driven by iterating
`SENSORS`, so a new registry entry gets a panel automatically. Uses
`Constraint::Fill(1)` for equal sensor rows (avoids any sensor-count cast).

**Signatures:**

```rust
//! Registry-driven ratatui rendering.
use meteo_lib::ble::registry::{SENSORS, SensorDescriptor};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};

use crate::app::{App, ConnectionStatus, SensorState};

/// Render the full frame: a status line, then one row per registered sensor.
pub fn render(frame: &mut Frame, app: &App);

/// Top status line: connection indicator (`● Connected` / `○ Scanning…`).
fn render_status(frame: &mut Frame, area: Rect, app: &App);

/// One sensor row: left readout (current value + min/max/avg) and right chart.
fn render_sensor(frame: &mut Frame, area: Rect, desc: &SensorDescriptor, state: &SensorState);

/// y-axis bounds for a sensor's chart: `[min, max]` of its history, padded by
/// 5% (or `[v-1, v+1]` for a single point / flat line) so the line is visible.
fn y_bounds(state: &SensorState) -> [f64; 2];

/// x-axis upper bound from the sample count (≥ 1 so an empty chart is valid).
fn x_axis_max(len: usize) -> f64;
```

**Code sketch — `render`:**

```rust
pub fn render(frame: &mut Frame, app: &App) {
    // `1_u16` / `1_u16`: type the integer literals so `default_numeric_fallback`
    // cannot fire (both `Constraint::Length`/`Fill` take `u16`).
    let mut constraints = vec![Constraint::Length(1_u16)];
    constraints.extend(SENSORS.iter().map(|_| Constraint::Fill(1_u16)));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    render_status(frame, chunks[0], app);
    for (i, desc) in SENSORS.iter().enumerate() {
        if let (Some(state), Some(&area)) = (app.sensors.get(i), chunks.get(i + 1)) {
            render_sensor(frame, area, desc, state);
        }
    }
}
```

**Code sketch — `render_status`** (one-line `Paragraph`; green dot when
connected, yellow when scanning):

```rust
fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let (text, color) = match app.status {
        ConnectionStatus::Connected => ("● Connected", Color::Green),
        ConnectionStatus::Scanning => ("○ Scanning…", Color::Yellow),
    };
    let line = Line::from(text).style(Style::default().fg(color));
    frame.render_widget(Paragraph::new(line), area);
}
```

**Code sketch — `render_sensor`** (precision via the `.*` form with a
`usize::from(u8)` — lossless, no cast lint):

```rust
fn render_sensor(frame: &mut Frame, area: Rect, desc: &SensorDescriptor, state: &SensorState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24_u16), Constraint::Min(0_u16)])
        .split(area);

    let prec = usize::from(desc.precision);
    let cur = state.latest().map_or_else(
        || "—".to_string(),
        |v| format!("{v:.prec$} {}", desc.unit),
    );
    let stats = match (state.min(), state.max(), state.avg()) {
        (Some(lo), Some(hi), Some(avg)) => format!(
            "min {lo:.prec$}  max {hi:.prec$}  avg {avg:.prec$}"
        ),
        _ => "no data".to_string(),
    };
    let readout = Paragraph::new(vec![Line::from(cur), Line::from(stats)])
        .block(Block::default().title(desc.name).borders(Borders::ALL));
    frame.render_widget(readout, cols[0]);

    let points = state.points();
    let bounds = y_bounds(state);
    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(&points),
    ];
    let x_max = x_axis_max(points.len());
    let chart = Chart::new(datasets)
        .block(Block::default().title("history").borders(Borders::ALL))
        .x_axis(Axis::default().bounds([0.0, x_max]))
        .y_axis(
            Axis::default()
                .bounds(bounds)
                .labels(vec![
                    format!("{:.prec$}", bounds[0]),
                    format!("{:.prec$}", bounds[1]),
                ]),
        );
    frame.render_widget(chart, cols[1]);
}
```

**Code sketch — pure chart-axis helpers** (kept free of any ratatui type so
they are unit-testable):

```rust
/// y-axis bounds: padded `[min, max]` of history; a flat line / single point
/// expands to `[v - 1, v + 1]`; empty history falls back to `[0, 1]`.
fn y_bounds(state: &SensorState) -> [f64; 2] {
    match (state.min(), state.max()) {
        (Some(lo), Some(hi)) => {
            let (lo, hi) = (f64::from(lo), f64::from(hi));
            let span = hi - lo;
            if span <= f64::EPSILON {
                [lo - 1.0, hi + 1.0] // flat line / single point
            } else {
                let pad = span * 0.05;
                [lo - pad, hi + pad]
            }
        }
        _ => [0.0, 1.0], // empty
    }
}

/// x-axis upper bound; ≥ 1.0 so an empty/one-point chart still has a range.
#[expect(
    clippy::cast_precision_loss,
    reason = "len ≤ HISTORY_CAPACITY = 600, exact in f64"
)]
fn x_axis_max(len: usize) -> f64 {
    len.max(1) as f64
}
```

> ratatui 0.30 API check: `Dataset::data` takes `&[(f64, f64)]` and
> `Axis::labels` takes an `IntoIterator` of items convertible to `Line`
> (`Line::from(String)` is the safe form). Confirm the exact `Chart`/`Axis`
> builder names against the 0.30 docs at implementation; the structure
> (Layout → `Chart` + `Paragraph`) is stable. This is the only spot where a
> 0.30 signature might differ from the sketch.

**Tests** (pure helpers, in a `#[cfg(test)]` module at the bottom of `ui.rs`,
plain `#[test]` style; the rendering functions themselves are terminal-bound and
verified manually):

- `y_bounds_pads_range` — push `[10.0, 20.0]`; assert `bounds[0] < 10.0` and
  `bounds[1] > 20.0` (5% padding applied).
- `y_bounds_single_point` — push `[15.0]` (len 1, `min == max`); assert
  `y_bounds == [14.0, 16.0]`.
- `y_bounds_two_equal_values` — push `[15.0, 15.0]` (len 2, `min == max`, the
  distinct flat-multi-point path); assert `y_bounds == [14.0, 16.0]`.
- `y_bounds_empty_is_unit_range` — fresh state; assert `y_bounds == [0.0, 1.0]`.
- `x_axis_max_is_at_least_one` — `x_axis_max(0) == 1.0` and `x_axis_max(5) == 5.0`.

### 6. Wire `main.rs`: terminal lifecycle + event loop

**Depends on:** 3, 4, 5. **Final integration.**

**File:** rewrite `crates/meteo-tui/src/main.rs`. Use ratatui 0.30's
`ratatui::init()` / `ratatui::restore()` — they enable raw mode + alternate
screen **and install a panic hook that restores the terminal**, removing the
risk of a panic leaving the terminal wedged. Crossterm event types are imported
via `ratatui::crossterm` to guarantee a single crossterm version; the bare
`crossterm` dep only turns on `event-stream`.

```rust
//! BLE TUI viewer for the MeteoStation weather station.
mod app;
mod client;
mod ui;

use std::io;

use futures::StreamExt as _;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use tokio::sync::mpsc;

use crate::app::{App, ClientEvent};

#[tokio::main]
async fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::new();
    let (tx, mut rx) = mpsc::channel::<ClientEvent>(64);
    // Auto-reconnect client runs in its own task; if it can't start (no
    // adapter) the UI still runs and shows `Scanning`.
    tokio::spawn(async move {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "client exits only when the UI is gone; nothing to report"
        )]
        let _ = client::run(tx).await;
    });

    let mut input = EventStream::new();
    terminal.draw(|f| ui::render(f, &app))?; // initial frame

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                if let Some(event) = maybe_event {
                    app.apply(event);
                }
                // `None` = client task ended; keep the UI up.
            }
            maybe_input = input.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_input
                    && key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    app.should_quit = true;
                }
            }
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::render(f, &app))?;
    }
    Ok(())
}
```

Redraw is **event-driven** (after every client event or keypress) — no fixed
render tick / sleep, satisfying the observe-don't-guess rule. On quit, `run`
returns, `rx` drops, the client's next `tx.send` errors and `client::run`
returns; the tokio runtime shuts the task down on `main` exit regardless.

**Test for this substep:** `just build`-equivalent for the host
(`cargo build -p meteo-tui --target x86_64-unknown-linux-gnu`), `just clippy`,
`just test` all green; then manual `just tui-gaia` against the live device.

## Testing

**Automated (host, via `just test`):**

- `meteo-lib` registry: the 6 tests in substep 1 (UUID↔index mapping, transform
  application, identity).
- `meteo-tui` app: the 10 tests in substep 3 (ring-buffer push/evict/capacity,
  min/max/avg, empty-state, event application incl. transform, history kept on
  disconnect, out-of-range index ignored, sensor-count matches registry,
  `points()` mapping).
- `meteo-tui` client: 2 tests for the pure `decode_reading` helper (substep 4).
- `meteo-tui` ui: 5 tests for the pure chart-axis helpers `y_bounds` /
  `x_axis_max` (substep 5).

Total: 23 automated tests (6 + 10 + 2 + 5).

**Checks before finalizing (all must pass — workspace splits host vs embedded):**

```bash
cargo fmt --all -- --check
cargo clippy -p meteo-firmware -- -D warnings
cargo clippy -p meteo-lib -p meteo-cli -p meteo-tui --target x86_64-unknown-linux-gnu -- -D warnings
cargo nextest run -p meteo-lib -p meteo-cli -p meteo-tui --target x86_64-unknown-linux-gnu
cargo build --release -p meteo-firmware   # ensure firmware untouched still builds
```

**Manual / integration (needs the BT adapter — Gaia):**

- `just tui-gaia`, device powered on: both sensor panels show a live current
  value, a growing history chart, and min/max/avg; status shows `Connected`.
- Negative-range sanity: temperature chart renders correctly for sub-zero
  values (the reason for `Chart` over `Sparkline`).
- **Reconnect:** observe the firmware's ~30 s disconnect → status flips to
  `Scanning`, history is **retained**, and it auto-reconnects, appending new
  points to the existing chart.
- **Quit:** `q` / `Esc` exits cleanly and the terminal is fully restored.
- **Regression:** `just cli` / `scripts/ble-debug.sh` still produce the same
  plain `cli-<ts>.log` text output (meteo-cli untouched).

## Risks

- **ratatui 0.30 / crossterm version skew.** 0.30 introduced the
  `ratatui-crossterm` adapter and gates crossterm behind `crossterm_0_28` /
  `crossterm_0_29` features. _Confirmed (2026-06-14) from the 0.30.1 manifest:_
  the default `crossterm` feature selects crossterm 0.29 via `ratatui-crossterm`,
  and `event-stream` must be enabled on the crossterm crate directly (the plan
  does so). _Mitigation:_ import all crossterm types via `ratatui::crossterm`;
  keep the bare `crossterm` dep only for `event-stream`; verify a single version
  with `cargo tree -p meteo-tui -i crossterm`, adding `features =
["crossterm_0_29"]` to ratatui only if a second version appears.
- **ratatui 0.30 widget API drift.** Exact `Dataset::data`, `Axis::labels`,
  `Chart` builder signatures may differ from older snippets. _Mitigation:_
  substep 5 notes to confirm against 0.30 docs; the structure (Layout → Chart +
  Paragraph) is stable. The numeric-only chart math is isolated in the pure,
  tested `y_bounds` / `x_axis_max` helpers, so only the ratatui glue is
  unverified until first build.
- **`tokio::spawn` requires `Send` futures.** btleplug's `Adapter`/`Peripheral`
  are `Send`/`Sync`, so the client task should spawn fine. _Mitigation/fallback:_
  if a non-`Send` type appears, run the client via a `LocalSet` or fold its
  loop into the `select!` instead of spawning.
- **Stale peripheral cache on reconnect.** btleplug may keep a `MeteoStation`
  entry in `peripherals()` after it stops advertising, so `wait_for_station`
  could return a stale handle and `connect()` fail. _Mitigation:_ a failed
  session just loops back to rescan; it reconnects once the device truly
  re-advertises. Acceptable for this in-memory viewer.
- **Numeric-cast clippy warnings** (pedantic `cast_precision_loss` /
  `cast_possible_truncation`). _Mitigation:_ prefer lossless conversions
  (`f32::from(u16)`, `usize::from(u8)`, typed `_u16` constraint literals); annotate
  the two unavoidable spots (`SensorState::points()` sample index and
  `x_axis_max`) with `#[expect(clippy::cast_precision_loss, reason = ...)]`,
  matching the codebase style. Clippy runs without `--all-targets`, so test code
  is not linted.
- **`let _ = <Result>` trips `let_underscore_must_use`** (warn → error under the
  host clippy gate). Five fire-and-forget sites (client scan/disconnect/seed-send,
  session result, spawned client) genuinely ignore their outcome. _Mitigation:_
  each carries a statement-level `#[expect(clippy::let_underscore_must_use,
reason = ...)]` in the sketches — not a bare discard.
- **Terminal corruption on panic.** _Mitigation:_ `ratatui::init()`/`restore()`
  install a panic hook that restores the terminal. A `terminal.draw(...)` error
  propagates via `?` out of `run`; `main` still calls `ratatui::restore()` before
  returning the error, so teardown always runs.

## Notes

Progress tracking (checked off during `/tyrex:code:implement-light`):

- [x] 1. Sensor registry in `meteo-lib` (`ble::registry`) + re-exports + 6 tests — `SensorDescriptor`/`SENSORS`/`index_for_uuid`/`pa_to_hpa`, re-exported from `ble::mod`; 6 tests pass.
- [ ] 2. Scaffold `meteo-tui` crate, workspace member, Justfile recipes/clippy/test
- [ ] 3. `app.rs` state & logic + 10 unit tests
- [ ] 4. `client.rs` auto-reconnecting btleplug central + 2 `decode_reading` tests
- [ ] 5. `ui.rs` registry-driven rendering + 5 chart-axis helper tests
- [ ] 6. `main.rs` terminal lifecycle + event loop wiring
- [ ] All checks green (fmt, clippy host+firmware, nextest, firmware release build)
- [ ] Manual verification on Gaia (live values, charts, reconnect, quit)

Follow-up (out of scope, enabled by this work): refactor `meteo-firmware`'s
`ble.rs` and `gatt::collect_handles` to iterate `ble::registry::SENSORS` instead
of the two hard-coded constants, making the registry the literal single source
of truth for firmware too.
