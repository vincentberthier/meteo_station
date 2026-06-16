# Plan: Ratatui Station Dashboard (Linux BLE central)

- **Source:** '3 (`.claude/brainstorm/3-ratatui-station-dashboard.md`)
- **Date:** 2026-06-17
- **Status:** Done

## Summary

Build a host-side terminal dashboard that connects to the on-chip ESP32-H2 BLE
peripheral (`MeteoStation`, `F0:CA:FE:00:00:01`), subscribes to the telemetry
notify characteristic, decodes the v1 wire frame via `meteo-lib`, and renders a
live `ratatui` dashboard: all 8 telemetry fields (`N/A` for absent ones), live
time-series charts for the populated fields (temperature + pressure today), a
header with the local clock, the TUI app version, the device firmware version,
and a connection-status indicator. A small coordinated firmware change exposes
the firmware version over a standard GATT **Device Information Service** so the
central can read it once on connect.

The central uses **`bluer` 0.17** (the official BlueZ binding), not `btleplug`.
Verification during planning showed `btleplug` 0.12's BlueZ backend subscribes
with `StartNotify` and delivers values via the `PropertiesChanged` `Value`
property — the deduplicated path `scripts/ble_notify_check.sh` deliberately
avoids, because BlueZ suppresses repeat `Value` signals and near-constant
telemetry collapses to silence. `bluer`'s `Characteristic::notify_io()` returns
a `CharacteristicReader` backed by BlueZ **`AcquireNotify`** (raw fd, no dedup) —
the same mechanism the proven scripts rely on. `bluer` is Linux/BlueZ-only, which
matches the deployment target exactly: the dev host **is** gaia (memory
`project-dev-host-is-gaia`).

## Files Modified

| File                               | Action | Description                                                                                                                                          |
| ---------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/meteo-firmware/src/ble.rs` | modify | Add DIS (`0x180A`) + Firmware Revision String (`0x2A26`) read characteristic; bump `ATT_MAX` 10→13.                                                  |
| `Cargo.toml` (workspace)           | modify | Add `crates/meteo-tui` to `members`.                                                                                                                 |
| `crates/meteo-tui/Cargo.toml`      | create | New host std binary crate; deps pinned locally (host-only).                                                                                          |
| `crates/meteo-tui/src/main.rs`     | create | Entry point: tokio runtime, terminal setup/teardown, CLI, event loop.                                                                                |
| `crates/meteo-tui/src/model.rs`    | create | Pure domain logic: `ConnState`, `LinkEvent`, `next_state`, field formatting, `Series` ring buffer, FW-version parse. Fully unit-tested.              |
| `crates/meteo-tui/src/ble.rs`      | create | `bluer` central: scan → connect → resolve → read version → `notify_io` subscribe → disconnect detection → reconnect state machine. Emits `BleEvent`. |
| `crates/meteo-tui/src/app.rs`      | create | `AppState` + `apply(BleEvent)`; staleness check. Pure parts unit-tested.                                                                             |
| `crates/meteo-tui/src/ui.rs`       | create | `render(frame, &AppState)`: header, telemetry table, charts.                                                                                         |
| `Justfile`                         | modify | Add `tui-build`, `tui-run`, `tui-clippy`; fold `meteo-tui` into `clippy`/`test`.                                                                     |
| `CLAUDE.md`                        | modify | Document the `meteo-tui` crate, host-only build, bluer/AcquireNotify rationale, DIS version transport.                                               |
| `README.md`                        | modify | Add a "Dashboard" usage section (if a README section pattern exists).                                                                                |

## Plan

Substeps are ordered by dependency. **Substep 1 (firmware) is independent** and
can land first or in parallel; it fixes the DIS UUIDs (`0x180A` / `0x2A26`) that
**Substep 4** reads. Substeps 2→7 are the host crate, built bottom-up
(scaffold → pure logic → I/O → UI → wiring → docs).

---

### 1. Firmware: expose firmware version via a Device Information Service

**File:** `crates/meteo-firmware/src/ble.rs` (modify)

Add a standard read-only DIS so the central can read the firmware version once on
connect. The GATT table is built by hand (no derive macros), so this is three new
attribute slots and one `add_service`/`add_characteristic` block mirroring the
existing telemetry service.

**Changes:**

1. New UUID constants and the version source near the existing UUID block
   (`SERVICE_UUID` ~line 90):

   ```rust
   /// Standard Device Information Service (0x180A) and Firmware Revision String
   /// (0x2A26), expanded against the Bluetooth base UUID by `Uuid::new_short`.
   const DIS_UUID: Uuid = Uuid::new_short(0x180A);
   const FW_REV_UUID: Uuid = Uuid::new_short(0x2A26);

   /// Firmware version string surfaced over DIS. Sourced from the crate version
   /// (workspace 0.1.0). The DIS Firmware Revision String is a UTF-8 string with
   /// no NUL terminator; `add_characteristic_ro` stores the `&'static str` bytes
   /// directly (see step 3), so no length const or backing buffer is needed.
   const FW_VERSION: &str = env!("CARGO_PKG_VERSION");
   ```

   > `Uuid::new_short` exists in trouble-host 0.6 (16-bit → base-UUID expansion).
   > Verify the exact constructor name against the vendored
   > `third_party/trouble-host` source during implementation; if it differs, use
   > the long form with the base-UUID bytes.

2. Bump the attribute-table sizing comment + constant (~line 68–84). DIS adds
   1 (primary service) + 2 (characteristic declaration + value) = 3 attributes;
   no CCCD (read-only), so `CCCD_MAX` is unchanged:

   ```rust
   // ... existing GAP (6) + MeteoService (4) = 10, plus DIS:
   //   1  primary-service attribute
   //   2  firmware-revision characteristic (declaration + value)
   // ───────────────────────────────────
   // 13  total
   const ATT_MAX: usize = 13;
   ```

3. In `run()` (after the telemetry service block, ~line 171, before
   `AttributeServer::new`), add the DIS. Use the vendored trouble-host's
   **`add_characteristic_ro`**, which takes a `&'d str` directly (via
   `impl AsGatt for str`) and forces `Read` — no storage buffer, no fixed-size
   array, no runtime `expect`. (Verified in `third_party/trouble-host`:
   `attribute.rs:849` `add_characteristic_ro<T: AsGatt + ?Sized>(uuid, value: &'d T)`
   and `types/gatt_traits.rs:173` `impl AsGatt for str`.) `FW_VERSION` is
   `'static`, satisfying the `'d` value lifetime:

   ```rust
   // Device Information Service: a single read-only Firmware Revision String the
   // central reads once on connect to show the device firmware version. The value
   // is the 'static FW_VERSION str, stored as ReadOnlyData — no backing buffer.
   {
       let mut dis = table.add_service(Service::new(DIS_UUID));
       // The CharacteristicBuilder is dropped at the `;`, releasing its mutable
       // borrow on `dis`, so the following `dis.build()` compiles.
       dis.add_characteristic_ro(FW_REV_UUID, FW_VERSION).build();
       dis.build();
   }
   ```

   > This is simpler than the telemetry-char pattern (which needs a `[u8; 17]`
   > storage buffer because it is writable/notifiable); the read-only DIS string
   > needs none. **Drop the `FW_VERSION_LEN` const from step 1** — it is no longer
   > used; keep only `FW_VERSION = env!("CARGO_PKG_VERSION")`.

**Tests:** none — firmware/hardware code is not host-testable (per `CLAUDE.md`
Code Standards).

**Verification:**

- `just clippy` (firmware leg) and `just build` pass with zero warnings.
- On-device manual read once flashed: `gatttool`/`bluetoothctl` or the new TUI
  reads `0x2A26` and returns `"0.1.0"`. (Acceptance is the existing manual gate;
  no new script required.)

**Risk:** `ATT_MAX` undersized → `add_service`/`build` panics at init. Mitigation:
the recount above is explicit; if init panics, the count is wrong — re-derive from
the GAP+GATT+service attribute breakdown.

---

### 2. Scaffold the `meteo-tui` host crate

**Files:** `crates/meteo-tui/Cargo.toml` (create), `crates/meteo-tui/src/main.rs`
(create, skeleton), `Cargo.toml` workspace `members` (modify).

The default build target is `riscv32imac-unknown-none-elf` (`.cargo/config.toml`
`[build] target`). `meteo-tui` is host-std and **must be built with
`--target x86_64-unknown-linux-gnu`** — the same scoping the `Justfile` already
uses for `meteo-lib` (`host_target`). All cargo invocations for this crate are
`-p meteo-tui --target {{host_target}}`; a bare workspace-wide `cargo build`
remains firmware-only by virtue of `-p` scoping (do not add `--workspace` builds).

**`crates/meteo-tui/Cargo.toml`** — deps declared locally (host-only; keep the
shared `[workspace.dependencies]` table, which is embedded-tuned, untouched).
Versions are current stable (looked up 2026-06-17):

```toml
[package]
name = "meteo-tui"
version.workspace = true
authors.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
meteo-lib = { workspace = true }          # default-features = false (no defmt) via workspace dep
bluer = "0.17"                            # official BlueZ binding; needs libdbus-1-dev + bluetoothd
tokio = { version = "1.52", features = ["rt-multi-thread", "macros", "sync", "time", "io-util"] }
futures = "0.3"
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }  # version matches ratatui 0.30's crossterm
chrono = { version = "0.4", default-features = false, features = ["clock"] }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
uuid = { version = "1", features = ["macro"] }  # "macro" enables uuid::uuid! used in ble.rs; unifies with bluer's uuid 1.x

[dev-dependencies]
test-log = { workspace = true }
env_logger = { workspace = true }

[lints]
workspace = true
```

**Lints.** Inherit the workspace bar (`workspace = true`), then carve out the
std-app noise at the crate root in `main.rs` — the embedded workspace set keeps
`std_instead_of_core` / `alloc_instead_of_*` **active on purpose** (the firmware
is no_std), and they fire pervasively on a std crate. Mirror the existing
crate-root `#![expect(...)]` style:

```rust
#![expect(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core,
    reason = "meteo-tui is a host std binary; core/alloc-first lints do not apply"
)]
#![expect(
    clippy::print_stderr,
    reason = "fatal startup errors are reported to stderr before the TUI takes the terminal"
)]
```

Run `just tui-clippy` and add further documented `#![expect(...)]` only for
restriction lints that genuinely conflict with idiomatic std/tokio code; do not
blanket-allow. `expect_used`/`unwrap_used` stay active — the app uses `anyhow` +
`?`.

**`crates/meteo-tui/src/main.rs`** (skeleton for this substep — compiles, exits):

```rust
// crate-root lint expects (above)
mod app;
mod ble;
mod model;
mod ui;

fn main() -> anyhow::Result<()> {
    Ok(())
}
```

> Create empty `app.rs`, `ble.rs`, `model.rs`, `ui.rs` stubs so the module tree
> compiles; they are filled in substeps 3–7.

> **`meteo-lib` (no_std) into a std host crate:** legal in Rust — a `#![no_std]`
> library compiles and links into a std binary (std is a superset of core/alloc).
> `meteo-lib` uses `libm` for float math, which builds on the host target. The
> workspace dep is `meteo-lib = { path = "…", default-features = false }`, so the
> optional `defmt` feature stays off for the TUI — exactly what we want on host.
> `Telemetry`, `decode`, `FRAME_LEN`, `FRAME_VERSION`, `FrameError` are all
> re-exported from `meteo_lib` (lib.rs:8) and used directly.

**Workspace `Cargo.toml`:**

```toml
[workspace]
members = [
    "crates/meteo-lib",
    "crates/meteo-firmware",
    "crates/meteo-tui",
]
```

**Tests:** none yet (scaffold). **Verification:**
`cargo build -p meteo-tui --target x86_64-unknown-linux-gnu` succeeds;
`cargo build -p meteo-firmware` (default riscv target) still succeeds (no
regression from the new member).

**Risk:** adding the member breaks the default-target firmware build if cargo
tries to build `meteo-tui` for riscv. Mitigation: never invoke `--workspace`
under the default target; all recipes scope with `-p` + `--target` (substep 7).
Document this in `CLAUDE.md`.

---

### 3. Pure domain logic — `model.rs`

**File:** `crates/meteo-tui/src/model.rs` (create). This is the TDD core: no I/O,
fully host-testable. Follow the project test module structure.

**Connection state machine** (single source of truth for both UI labels and the
disconnect-detection rule):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Scanning,
    Connecting,
    Resolving,
    Live,
    Reconnecting,
}

/// Authoritative link-state events. NOTE: there is deliberately **no** frame-age
/// variant here — data-flow silence must never drive reconnection (brainstorm
/// "Connection lifecycle" rule + memory `feedback-ble-disconnect-detection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    ScanStarted,
    DeviceFound,
    Connected,
    Subscribed,     // services resolved + notify_io acquired
    LinkLost,       // BlueZ Connected → false, or notify reader EOF
    AttemptFailed,  // bounded per-step deadline elapsed / connect error
}

impl ConnState {
    /// Pure transition. `LinkLost`/`AttemptFailed` from any state → `Reconnecting`;
    /// `ScanStarted` → `Scanning`; happy path Scanning→Connecting→Resolving→Live.
    #[must_use]
    pub fn next(self, ev: LinkEvent) -> Self {
        match (self, ev) {
            (_, LinkEvent::LinkLost | LinkEvent::AttemptFailed) => Self::Reconnecting,
            (_, LinkEvent::ScanStarted) => Self::Scanning,
            (Self::Scanning, LinkEvent::DeviceFound) => Self::Connecting,
            (Self::Connecting, LinkEvent::Connected) => Self::Resolving,
            (Self::Resolving, LinkEvent::Subscribed) => Self::Live,
            (s, _) => s, // ignore events that don't apply to the current state
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Scanning => "Scanning",
            Self::Connecting => "Connecting",
            Self::Resolving => "Resolving",
            Self::Live => "Live",
            Self::Reconnecting => "Reconnecting",
        }
    }
}
```

**Telemetry field formatting** (`N/A` for `None`, fixed precision):

```rust
use meteo_lib::Telemetry;

#[must_use] pub fn fmt_temp(v: Option<f32>) -> String { fmt_unit(v, "°C", 1) }
#[must_use] pub fn fmt_pressure(v: Option<f32>) -> String { fmt_unit(v, "hPa", 1) }
#[must_use] pub fn fmt_humidity(v: Option<f32>) -> String { fmt_unit(v, "%RH", 0) }
#[must_use] pub fn fmt_lux(v: Option<f32>) -> String { fmt_unit(v, "lux", 0) }
#[must_use] pub fn fmt_wind_speed(v: Option<f32>) -> String { fmt_unit(v, "m/s", 1) }
#[must_use] pub fn fmt_wind_dir(v: Option<f32>) -> String { fmt_unit(v, "°", 0) }
#[must_use] pub fn fmt_battery(v: Option<u8>) -> String {
    v.map_or_else(|| "N/A".to_owned(), |b| format!("{b} %"))
}

fn fmt_unit(v: Option<f32>, unit: &str, prec: usize) -> String {
    v.map_or_else(|| "N/A".to_owned(), |x| format!("{x:.prec$} {unit}"))
}
```

**Ring buffer for one time series:**

```rust
use std::collections::VecDeque;

/// Capped time-series of (seconds-since-session-start, value) points for charting.
pub struct Series {
    points: VecDeque<(f64, f64)>,
    cap: usize,
}

impl Series {
    /// 600 points = 10 min at the 1 Hz feed.
    pub const DEFAULT_CAP: usize = 600;

    #[must_use] pub fn new(cap: usize) -> Self {
        Self { points: VecDeque::with_capacity(cap), cap }
    }

    /// Append a sample, dropping the oldest once `cap` is exceeded.
    pub fn push(&mut self, t_secs: f64, value: f64) {
        if self.points.len() == self.cap { self.points.pop_front(); }
        self.points.push_back((t_secs, value));
    }

    #[must_use] pub fn points(&mut self) -> &[(f64, f64)] { self.points.make_contiguous() }
    #[must_use] pub fn is_empty(&self) -> bool { self.points.is_empty() }
    /// (min, max) of the value axis, for ratatui Axis bounds; None if empty.
    #[must_use] pub fn y_bounds(&self) -> Option<(f64, f64)> {
        let mut it = self.points.iter().map(|p| p.1);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }
    /// (first_t, last_t) of the time axis; None if empty.
    #[must_use] pub fn x_bounds(&self) -> Option<(f64, f64)> {
        Some((self.points.front()?.0, self.points.back()?.0))
    }
}
```

**Firmware-version parse** (DIS bytes → display string):

```rust
/// Decode the DIS Firmware Revision String. Returns `None` on invalid UTF-8 so
/// the UI shows "unknown" rather than garbage.
#[must_use]
pub fn parse_fw_revision(bytes: &[u8]) -> Option<String> {
    core::str::from_utf8(bytes).ok().map(|s| s.trim_end_matches('\0').trim().to_owned())
}
```

**Tests** (`#[cfg(test)] mod tests`, run on host):

- `next_state_link_lost_from_live_returns_reconnecting` — `ConnState::Live.next(LinkLost) == Reconnecting`.
- `next_state_attempt_failed_from_connecting_returns_reconnecting`.
- `next_state_happy_path_scanning_to_live` — drive `DeviceFound, Connected, Subscribed`, assert `Live`.
- `next_state_scan_started_resets_to_scanning` — from `Reconnecting`.
- `next_state_full_reconnect_sequence` — drive `Live →(LinkLost)→ Reconnecting
→(ScanStarted)→ Scanning →(DeviceFound)→ Connecting →(Connected)→ Resolving
→(Subscribed)→ Live`, asserting each intermediate state (the reconnect loop path).
- `next_state_ignores_inapplicable_event` — `Live.next(DeviceFound) == Live`
  (proves no spurious transitions). The stronger "data silence never reconnects"
  guarantee is structural, not a runtime test: `LinkEvent` simply has no frame-age
  variant, so no event reachable from the BLE task can drive a reconnect off frame
  age. (Do not frame this as a runtime assertion.)
- `fmt_temp_some_renders_one_decimal_with_unit` — `fmt_temp(Some(23.5)) == "23.5 °C"`.
  Use the exactly-representable `23.5_f32` (not `23.45`) so the `{:.1}` rounding is
  platform-independent.
- `fmt_temp_none_renders_na`.
- `fmt_battery_none_renders_na` / `fmt_battery_some_renders_percent`.
- `series_caps_at_capacity_dropping_oldest` — push `cap+5`, assert `len == cap` and front is the 6th sample.
- `series_push_preserves_order_and_bounds` — push 3 samples, then assert
  `points()` returns a contiguous slice of len 3 in push order **and**
  `x_bounds()`/`y_bounds()` equal the expected first/last and min/max.
- `parse_fw_revision_trims_and_decodes` — `parse_fw_revision(b"0.1.0") == Some("0.1.0")`.
- `parse_fw_revision_rejects_invalid_utf8` — `parse_fw_revision(&[0xFF, 0xFE]) == None`.

**Risk:** `format!("{x:.prec$}")` precision-from-variable syntax — confirm it
compiles (it does in stable Rust). Edge: `f32` NaN/inf from a corrupt decode —
`meteo-lib::decode` already clamps/sentinels, so values are finite; no extra
guard needed.

---

### 4. BLE central — `ble.rs` (bluer)

**File:** `crates/meteo-tui/src/ble.rs` (create). Owns the `bluer` session and
the connection state machine; runs as a spawned tokio task and emits `BleEvent`
to the app over an `mpsc` channel. This module is **I/O — not unit-tested**
(consistent with project norms); its testable logic already lives in `model.rs`
(`ConnState::next`, `parse_fw_revision`, frame decode in `meteo-lib`).

**Public surface:**

```rust
use meteo_lib::{Telemetry, FRAME_LEN};
use tokio::sync::mpsc;
use crate::model::{ConnState, LinkEvent, parse_fw_revision};

/// Events pushed to the app loop.
#[derive(Debug, Clone)]
pub enum BleEvent {
    State(ConnState),
    Frame(Telemetry),
    Firmware(Option<String>),
}

// The station address is NOT a const here: it arrives as the `addr` parameter,
// parsed from the clap CLI (`--address`, default "F0:CA:FE:00:00:01" — BlueZ
// display order, MSB first; the REVERSE of the firmware's on-air little-endian
// STATION_ADDR, and matching the `DEVICE` default in scripts/ble_*.sh). This
// sidesteps any question of whether `bluer::Address` has a const constructor —
// `bluer::Address: FromStr` does the parse in main.rs (substep 7).

/// Telemetry notify characteristic (128-bit) and DIS firmware-revision (16-bit).
const TELEMETRY_UUID: uuid::Uuid = uuid::uuid!("7e700002-b1df-42a1-bb5f-6a1028c793b0");
fn dis_service_uuid() -> uuid::Uuid { uuid16(0x180A) }
fn fw_rev_uuid() -> uuid::Uuid { uuid16(0x2A26) }

/// Expand a 16-bit Bluetooth UUID against the base UUID.
fn uuid16(x: u16) -> uuid::Uuid {
    uuid::Uuid::from_fields(u32::from(x), 0x0000, 0x1000,
        &[0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb])
}

/// Spawned task: runs the connect/reconnect loop forever, emitting `BleEvent`s.
/// `addr` is the station address parsed from the CLI. Never returns under normal
/// operation.
pub async fn run(tx: mpsc::Sender<BleEvent>, addr: bluer::Address) -> anyhow::Result<()>;
```

**Connection lifecycle** — the brainstorm's hard requirement. Implement the loop
**observe-driven**, with `tokio::time::timeout` as a per-step circuit-breaker
only (the one admissible use of a deadline — paired with an explicit failure path
that emits `AttemptFailed` and re-scans). **No fixed inter-retry sleeps.**

```rust
pub async fn run(tx: mpsc::Sender<BleEvent>, addr: bluer::Address) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let mut state = ConnState::Reconnecting; // forces an initial ScanStarted
    loop {
        // ---- Scanning: bounded, self-terminating discovery to repopulate the
        // BlueZ cache (it evicts the non-bonded LE object after disconnect; cold
        // connect-by-address fails until a fresh scan). Mirror the scripts. ----
        state = emit(&tx, state, LinkEvent::ScanStarted).await;
        let device = match scan_for(&adapter, addr, SCAN_DEADLINE).await {
            Some(d) => d,
            None => { state = emit(&tx, state, LinkEvent::AttemptFailed).await; continue; }
        };
        state = emit(&tx, state, LinkEvent::DeviceFound).await;

        // ---- Connecting (bounded) ----
        // timeout(..) -> Result<Result<(), bluer::Error>, Elapsed>: both the
        // deadline (outer Err) and a connect error (inner Err) mean "failed".
        match timeout(CONNECT_DEADLINE, device.connect()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => {
                state = emit(&tx, state, LinkEvent::AttemptFailed).await;
                continue;
            }
        }
        state = emit(&tx, state, LinkEvent::Connected).await;

        // ---- Resolving services (observe is_services_resolved via events) ----
        if wait_services_resolved(&device, RESOLVE_DEADLINE).await.is_err() {
            state = emit(&tx, state, LinkEvent::AttemptFailed).await; continue;
        }

        // ---- Read firmware version once (DIS), best-effort ----
        let fw = read_fw_version(&device).await; // Option<String>
        let _ = tx.send(BleEvent::Firmware(fw)).await;

        // ---- Subscribe via notify_io (AcquireNotify, raw fd, NO dedup) ----
        let telem_char = match find_char(&device, TELEMETRY_UUID).await {
            Some(c) => c,
            None => { state = emit(&tx, state, LinkEvent::AttemptFailed).await; continue; }
        };
        let reader = match telem_char.notify_io().await {
            Ok(r) => r,
            Err(_) => { state = emit(&tx, state, LinkEvent::AttemptFailed).await; continue; }
        };
        state = emit(&tx, state, LinkEvent::Subscribed).await; // → Live

        // ---- Live: pump frames until LINK-STATE says down ----
        pump_until_disconnect(&device, reader, &tx).await;
        state = emit(&tx, state, LinkEvent::LinkLost).await; // → Reconnecting
        // loop back to Scanning (fresh scan before reconnect)
    }
}
```

**`pump_until_disconnect`** — two authoritative signals, joined; **frame silence
is NOT one of them**:

```rust
async fn pump_until_disconnect(
    device: &bluer::Device,
    mut reader: bluer::gatt::remote::CharacteristicReader,
    tx: &mpsc::Sender<BleEvent>,
) {
    use tokio::io::AsyncReadExt;  // reader.read(..)
    use futures::StreamExt;       // dev_events.next()
    let mut dev_events = match device.events().await { Ok(e) => e, Err(_) => return };
    let mut buf = [0u8; FRAME_LEN];
    loop {
        tokio::select! {
            // (1) raw notification PDU; EOF (Ok(0)) = link closed.
            r = reader.read(&mut buf) => match r {
                Ok(0) | Err(_) => break,                 // reader EOF / error → disconnect
                Ok(n) => if let Ok(t) = Telemetry::decode(&buf[..n]) {
                    let _ = tx.send(BleEvent::Frame(t)).await;
                } // malformed frame: ignore, keep pumping (do NOT treat as disconnect)
            },
            // (2) authoritative link-state: BlueZ Connected → false.
            ev = dev_events.next() => match ev {
                Some(bluer::DeviceEvent::PropertyChanged(
                        bluer::DeviceProperty::Connected(false))) => break,
                None => break,                            // event stream ended
                _ => {}                                   // RSSI/other props: ignore
            },
        }
    }
}
```

Bounded deadlines are named constants (circuit-breakers, documented as such):
`SCAN_DEADLINE = 30s`, `CONNECT_DEADLINE = 30s`, `RESOLVE_DEADLINE = 15s`
(matching the scripts' `CONNECT_TIMEOUT`/15 s resolve). Helper bodies — all
observe-driven, each wrapped in `tokio::time::timeout` by its caller or inline:

```rust
use futures::StreamExt; // for the `.next()` on bluer event streams below

/// `tx.send` the next state and return it. Send failure (UI gone) is ignored;
/// the loop is torn down by the runtime when `main` exits.
async fn emit(tx: &mpsc::Sender<BleEvent>, state: ConnState, ev: LinkEvent) -> ConnState {
    let next = state.next(ev);
    let _ = tx.send(BleEvent::State(next)).await;
    next
}

/// Bounded, observe-driven discovery. Returns the device once BlueZ reports it,
/// else None on deadline. Mirrors the scripts' bounded scan (no unbounded scan).
async fn scan_for(adapter: &bluer::Adapter, addr: bluer::Address, deadline: Duration)
    -> Option<bluer::Device>
{
    // Already cached? (blueman's standing discovery may have it.)
    if adapter.device_addresses().await.ok()?.contains(&addr) {
        return adapter.device(addr).ok();
    }
    let scan = async {
        let mut events = adapter.discover_devices().await.ok()?;
        while let Some(ev) = events.next().await {
            if let bluer::AdapterEvent::DeviceAdded(a) = ev {
                if a == addr { return adapter.device(addr).ok(); }
            }
        }
        None
    };
    timeout(deadline, scan).await.ok().flatten()
}

/// Await ServicesResolved == true (observe the property), bounded by `deadline`.
async fn wait_services_resolved(device: &bluer::Device, deadline: Duration)
    -> anyhow::Result<()>
{
    if device.is_services_resolved().await? { return Ok(()); }
    let wait = async {
        let mut events = device.events().await?;
        while let Some(ev) = events.next().await {
            if let bluer::DeviceEvent::PropertyChanged(
                    bluer::DeviceProperty::ServicesResolved(true)) = ev {
                return anyhow::Ok(());
            }
        }
        anyhow::bail!("device event stream ended before services resolved")
    };
    timeout(deadline, wait).await?
}

/// Walk services for the telemetry characteristic by UUID.
async fn find_char(device: &bluer::Device, uuid: uuid::Uuid)
    -> Option<bluer::gatt::remote::Characteristic>
{
    for svc in device.services().await.ok()? {
        for ch in svc.characteristics().await.ok()? {
            if ch.uuid().await.ok()? == uuid { return Some(ch); }
        }
    }
    None
}

/// Read the DIS Firmware Revision String once (best-effort → Option).
async fn read_fw_version(device: &bluer::Device) -> Option<String> {
    for svc in device.services().await.ok()? {
        if svc.uuid().await.ok()? != dis_service_uuid() { continue; }
        for ch in svc.characteristics().await.ok()? {
            if ch.uuid().await.ok()? == fw_rev_uuid() {
                return parse_fw_revision(&ch.read().await.ok()?);
            }
        }
    }
    None
}
```

> **Verify during implementation against `docs.rs/bluer/0.17`** (and the
> `bluer-tools` `gattcat.rs` reference): exact names `AdapterEvent::DeviceAdded`,
> `DeviceEvent::PropertyChanged`, `DeviceProperty::Connected`,
> `DeviceProperty::ServicesResolved`, `Device::is_services_resolved`,
> `Device::device_addresses`/`Adapter::device_addresses`, `CharacteristicReader`
> impl of `AsyncRead`, and `Service::uuid()`/`characteristics()`. Adjust the
> helper bodies above if a name differs — the control flow is the contract, the
> exact identifiers are to be confirmed.

**Tests:** none (D-Bus/hardware I/O). Logic is covered in substep 3.

**Verification:** with the firmware flashed (substep 1) and the station
advertising, `just tui-run` connects, shows `Live`, and the telemetry table
updates at ~1 Hz; pulling power → `Reconnecting`/`Scanning`; restoring →
auto-reconnect (matches `ble_soak.sh` behaviour). Cross-check raw delivery
against `scripts/ble_notify_check.sh` (both use AcquireNotify; frame counts
should agree).

**Risks:**

- `bluer` API names drift between versions — mitigated by the explicit
  verify-against-docs note and the `gattcat.rs` reference.
- A `notify_io` reader could coalesce two PDUs in one `read()` if MTU > frame —
  unlikely at 17 bytes, but `decode` rejects wrong-length slices; if observed,
  frame on `FRAME_LEN` boundaries. Documented as a known edge.
- Missing `libdbus-1-dev`/`bluetoothd` → build/link or runtime failure. Document
  the host prerequisite (present on gaia).

---

### 5. App state — `app.rs`

**File:** `crates/meteo-tui/src/app.rs` (create). Holds render state and the pure
`apply` reducer (testable); separates **link-state** (authoritative) from
**frame-age** (cosmetic staleness only).

```rust
use std::time::{Duration, Instant};
use meteo_lib::Telemetry;
use crate::ble::BleEvent;
use crate::model::{ConnState, Series};

pub struct AppState {
    pub conn: ConnState,
    pub latest: Telemetry,
    pub last_frame_at: Option<Instant>,
    pub fw_version: Option<String>,
    pub app_version: &'static str,     // env!("CARGO_PKG_VERSION")
    pub temp: Series,
    pub pressure: Series,
    started: Instant,
}

impl AppState {
    #[must_use] pub fn new(now: Instant) -> Self {
        Self {
            conn: ConnState::Scanning,
            latest: Telemetry::empty(),
            last_frame_at: None,
            fw_version: None,
            app_version: env!("CARGO_PKG_VERSION"),
            temp: Series::new(Series::DEFAULT_CAP),
            pressure: Series::new(Series::DEFAULT_CAP),
            started: now,
        }
    }

    /// Reduce one BLE event into state. `now` is injected for testability.
    pub fn apply(&mut self, ev: BleEvent, now: Instant) {
        match ev {
            BleEvent::State(s) => self.conn = s,
            BleEvent::Firmware(v) => self.fw_version = v,
            BleEvent::Frame(t) => {
                self.latest = t;
                self.last_frame_at = Some(now);
                let secs = now.duration_since(self.started).as_secs_f64();
                if let Some(v) = t.temperature_c { self.temp.push(secs, f64::from(v)); }
                if let Some(v) = t.pressure_hpa { self.pressure.push(secs, f64::from(v)); }
            }
        }
    }

    /// Cosmetic staleness for greying values — NEVER drives reconnect.
    #[must_use] pub fn is_stale(&self, now: Instant, max_age: Duration) -> bool {
        self.last_frame_at.map_or(true, |t| now.duration_since(t) > max_age)
    }
}

pub const STALE_AFTER: Duration = Duration::from_secs(5);
```

**Tests** (host):

- `apply_frame_updates_latest_and_series` — apply `Frame(temp+pressure)`, assert
  `latest` set, both series len 1, `last_frame_at.is_some()`.
- `apply_frame_skips_none_fields_in_series` — frame with `pressure_hpa=None`
  leaves `pressure` empty, `temp` len 1.
- `apply_state_updates_conn` — `apply(State(Reconnecting))` sets `conn`.
- `apply_firmware_sets_version`.
- `is_stale_true_before_first_frame` — fresh state is stale.
- `is_stale_false_within_window` / `is_stale_true_after_window` — using
  injected `Instant`s (`started + N`).

**Risk:** `Instant` is monotonic and can't be constructed at arbitrary values in
tests — use `Instant::now()` as a base and add `Duration`s (`base + STALE_AFTER +
Duration::from_secs(1)`), which is what the injected-`now` signature enables.

---

### 6. UI rendering — `ui.rs`

**File:** `crates/meteo-tui/src/ui.rs` (create). One pure-ish `render` function
(reads `&mut AppState` only because `Series::points()` needs `make_contiguous`);
no I/O, no state mutation beyond the deque contiguity.

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Chart, Dataset, Paragraph, Row, Table};
use crate::app::AppState;
use crate::model;

/// Draw the full dashboard for one frame.
pub fn render(frame: &mut Frame, app: &mut AppState, now: std::time::Instant) {
    let [header, table_area, charts] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(10),
        Constraint::Min(0),
    ]).areas(frame.area());

    render_header(frame, header, app);          // clock | versions | conn label
    render_table(frame, table_area, app, now);  // 8 rows; dimmed if is_stale
    render_charts(frame, charts, app);          // temp + pressure, or placeholders
}

fn render_header(frame: &mut Frame, area: Rect, app: &AppState) {
    let [clock, versions, status] =
        Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area);
    frame.render_widget(
        Paragraph::new(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
        clock);
    frame.render_widget(
        Paragraph::new(format!("app v{}  fw {}",
            app.app_version, app.fw_version.as_deref().unwrap_or("unknown"))),
        versions);
    let color = match app.conn {
        ConnState::Live => Color::Green,
        ConnState::Reconnecting => Color::Red,
        _ => Color::Yellow,
    };
    frame.render_widget(
        Paragraph::new(app.conn.label()).style(Style::default().fg(color)), status);
}

fn render_table(frame: &mut Frame, area: Rect, app: &AppState, now: std::time::Instant) {
    let t = &app.latest;
    let rows = [
        ("Temperature", model::fmt_temp(t.temperature_c)),
        ("Pressure",    model::fmt_pressure(t.pressure_hpa)),
        ("Humidity",    model::fmt_humidity(t.humidity_pct)),
        ("Sky temp",    model::fmt_temp(t.sky_temp_c)),
        ("Luminosity",  model::fmt_lux(t.luminosity_lux)),
        ("Wind speed",  model::fmt_wind_speed(t.wind_speed_ms)),
        ("Wind dir",    model::fmt_wind_dir(t.wind_dir_deg)),
        ("Battery",     model::fmt_battery(t.battery_pct)),
    ];
    let base = if app.is_stale(now, crate::app::STALE_AFTER) {
        Style::default().add_modifier(Modifier::DIM)   // cosmetic only
    } else {
        Style::default()
    };
    let table = Table::new(
        rows.iter().map(|(k, v)| Row::new([(*k).to_owned(), v.clone()]).style(base)),
        [Constraint::Length(14), Constraint::Min(0)],
    ).block(Block::bordered().title("Telemetry"));
    frame.render_widget(table, area);
}

fn render_charts(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let [top, bottom] =
        Layout::vertical([Constraint::Ratio(1, 2); 2]).areas(area);
    render_series_chart(frame, top, "Temperature (°C)", &mut app.temp);
    render_series_chart(frame, bottom, "Pressure (hPa)", &mut app.pressure);
}

/// One line chart, or an "awaiting data" placeholder when the series is empty
/// (true for every field except temperature/pressure today).
fn render_series_chart(frame: &mut Frame, area: Rect, title: &str, series: &mut Series) {
    let (Some((x0, x1)), Some((y0, y1))) = (series.x_bounds(), series.y_bounds()) else {
        frame.render_widget(
            Paragraph::new("awaiting data").block(Block::bordered().title(title)),
            area);
        return;
    };
    let data = series.points(); // &[(f64, f64)] — make_contiguous
    let datasets = vec![Dataset::default().graph_type(GraphType::Line).data(data)];
    let chart = Chart::new(datasets)
        .block(Block::bordered().title(title))
        .x_axis(Axis::default().bounds([x0, x1]))
        .y_axis(Axis::default().bounds([y0, y1]));
    frame.render_widget(chart, area);
}
```

> Imports elided above for brevity: `ratatui::layout::Rect`,
> `ratatui::style::{Color, Modifier, Style}`,
> `ratatui::widgets::{Axis, GraphType}`, `crate::model::{ConnState, Series}`.
> The table lists all 8 fields today; charts are wired for the two live series
> (temperature, pressure) and the layout already accommodates the others becoming
> live later (add a `render_series_chart` call per newly-populated series).

**Colour mapping:** `Live` green, `Reconnecting` red, all in-progress states
(`Scanning`/`Connecting`/`Resolving`) yellow — encoded in `render_header` above.
Staleness dimming is cosmetic and independent of `conn` (driven by `is_stale`,
never the reverse).

**Tests (required):** a `TestBackend` smoke test — this guards the known
layout-constraint panic failure mode (see Risk), so it is not optional:

- `render_smoke_fills_buffer_without_panic` — build a `Terminal::new(TestBackend::new(120, 40))`,
  seed an `AppState` with one applied frame and `conn = Live`, call `render`,
  assert the rendered buffer contains `"Live"` and `"app v"`. Repeat once at a
  small size (e.g. `TestBackend::new(40, 12)`) to prove no constraint overflow.

**Risk:** ratatui 0.30 API names (`GraphType::Line`, `Dataset::data`,
`Layout::vertical`) — verify against `docs.rs/ratatui/0.30`; 0.30 is current and
these are stable. Layout constraint over-allocation panics on tiny terminals —
the `TestBackend` smoke test + `Constraint::Min(0)` mitigate.

---

### 7. Wiring — `main.rs`, event loop, CLI

**File:** `crates/meteo-tui/src/main.rs` (fill in). Tokio entry; terminal RAII
setup/teardown; `clap` CLI; the `tokio::select!` loop merging three sources.

```rust
use clap::Parser;

#[derive(Parser)]
#[command(version, about = "MeteoStation live BLE dashboard")]
struct Cli {
    /// Station BLE address (BlueZ display order). Defaults to the firmware address.
    #[arg(long, default_value = "F0:CA:FE:00:00:01")]
    address: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let addr: bluer::Address = cli.address.parse()?; // bluer::Address: FromStr

    // ratatui::init() enables raw mode + alternate screen AND installs a panic
    // hook that restores the terminal on panic, so a crash never leaves it wedged.
    // It returns `ratatui::DefaultTerminal`
    // (= `Terminal<CrosstermBackend<Stdout>>`). ratatui::restore() undoes it.
    let mut terminal: ratatui::DefaultTerminal = ratatui::init();
    let res = run_app(&mut terminal, addr).await;
    ratatui::restore();
    res
}

async fn run_app(
    terminal: &mut ratatui::DefaultTerminal,
    addr: bluer::Address,
) -> anyhow::Result<()> {
    use futures::StreamExt;
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(crate::ble::run(tx, addr));   // BLE manager task

    let mut input = crossterm::event::EventStream::new();
    // Clock refresh cadence: 1 Hz, SOLELY to advance the displayed wall clock and
    // re-render. This is a display cadence, NOT a readiness sleep — you cannot
    // observe the wall clock advancing except via a timer. Same rationale as the
    // firmware's documented periodic RWDT poll in watchdog.rs. All DATA-driven
    // redraws happen on BLE/input events below; this tick only keeps the clock live.
    let mut clock = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut app = crate::app::AppState::new(std::time::Instant::now());
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => app.apply(ev, std::time::Instant::now()),
            Some(Ok(term_ev)) = input.next() => {
                if should_quit(&term_ev) { break; }   // 'q' or Ctrl-C
            }
            _ = clock.tick() => {}
        }
        terminal.draw(|f| crate::ui::render(f, &mut app, std::time::Instant::now()))?;
    }
    Ok(())
}

/// Pure: quit on 'q', Esc, or Ctrl-C. Testable (no I/O).
fn should_quit(ev: &crossterm::event::Event) -> bool {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    matches!(ev,
        Event::Key(KeyEvent { code: KeyCode::Char('q') | KeyCode::Esc, .. })
        | Event::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }))
}
```

> Note: `EventStream` emits `KeyEventKind::Press` _and_ `Release` on some
> terminals; the `matches!` above fires on both. If double-trigger is observed,
> add `kind: KeyEventKind::Press` to the press arms — a one-line refinement, not a
> design change.

**Tests** (host): `should_quit` is pure →

- `should_quit_on_q_key` / `should_quit_on_esc` / `should_quit_on_ctrl_c` /
  `should_not_quit_on_other_key`.

**Verification:** `just tui-run` launches the dashboard; `q` exits cleanly and the
terminal is restored (no wedged raw mode); a forced panic also restores
(`ratatui::restore` / RAII guard).

**Risk:** terminal not restored on panic. Mitigation: use `ratatui::init()` /
`ratatui::restore()` (0.30 installs a panic hook) or a `Drop` guard; verify the
panic path leaves the terminal usable.

---

### 8. Justfile recipes, docs, and final checks

**Files:** `Justfile` (modify), `CLAUDE.md` (modify), `README.md` (modify if a
usage section exists).

**Justfile** — add host-target recipes and fold `meteo-tui` into the shared
checks (mirroring the existing `host_target` pattern):

```just
[doc('Build the TUI dashboard (host target)')]
tui-build:
    cargo build -p meteo-tui --target {{ host_target }}

[doc('Run the TUI dashboard (host target)')]
tui-run *ARGS:
    cargo run -p meteo-tui --target {{ host_target }} -- {{ ARGS }}

[doc('Clippy the TUI crate only (fast host-side loop)')]
tui-clippy:
    cargo clippy -p meteo-tui --all-targets --target {{ host_target }} -- -D warnings

[doc('Check code with clippy')]
clippy:
    cargo clippy -p meteo-firmware -- -D warnings
    cargo clippy -p meteo-lib --all-features --all-targets --target {{ host_target }} -- -D warnings
    cargo clippy -p meteo-tui --all-targets --target {{ host_target }} -- -D warnings

[doc('Run tests on host')]
test:
    cargo nextest run -p meteo-lib --target {{ host_target }}
    cargo nextest run -p meteo-tui --target {{ host_target }}
```

**CLAUDE.md** — add a "Dashboard (`meteo-tui`)" subsection under Architecture /
Module Structure: the new crate, host-only build (`--target host`, never
`--workspace` under the default riscv target), `bluer`/`AcquireNotify` rationale
(why not btleplug — the `StartNotify` dedup trap; link to
`scripts/ble_notify_check.sh`), the DIS firmware-version transport, the
disconnect-detection rule (link-state authoritative, frame-age cosmetic), and the
host prerequisite (`libdbus-1-dev`, `bluetoothd`; present on gaia).

**README.md** — a short "Live dashboard" usage block: `just tui-run`, `--address`
override, `q` to quit. (Only if README has a usage section; otherwise skip and
keep it in CLAUDE.md.)

**Final checks (all must pass before finalizing):**

```bash
cargo fmt --all -- --check
just clippy          # firmware (riscv) + lib + tui (host), -D warnings
just test            # lib + tui host tests via nextest
just build           # firmware still builds (no regression)
cargo build -p meteo-tui --target x86_64-unknown-linux-gnu
```

On-device acceptance (manual gate, after flashing substep 1): `just tui-run`
shows `Live`, telemetry updates at ~1 Hz, the firmware version reads `0.1.0`, and
a power-cycle of the station drives `Reconnecting → Scanning → Live` automatically.

## Testing

**Host unit tests** (`cargo nextest -p meteo-tui --target x86_64-unknown-linux-gnu`)
cover every pure decision:

- State machine: `ConnState::next` transitions, including the rule that **no
  frame-age event exists** and inapplicable events are no-ops (substep 3).
- Field formatting: `Some`/`None` → value-with-unit / `N/A` (substep 3).
- Ring buffer: capacity cap, ordering, axis bounds (substep 3).
- FW-version parse: valid/invalid UTF-8, trimming (substep 3).
- App reducer: frame updates latest+series, skips `None` fields, state/firmware
  updates; `is_stale` window logic with injected `Instant` (substep 5).
- UI smoke test via `TestBackend` — no layout panic, header content present
  (substep 6).
- `should_quit` key mapping (substep 7).

`meteo-lib`'s existing 24/24 frame tests already cover `decode`; `meteo-tui`
reuses `decode` rather than re-testing it.

**Edge cases:** all-`None` telemetry (sentinels) → every field `N/A`, charts show
"awaiting data"; malformed/short notification → ignored, link stays up;
invalid-UTF-8 firmware string → "unknown"; tiny terminal → no panic (smoke test +
`Min(0)`); first-frame-not-yet-arrived → stale/grey but `conn` may already be
`Live`.

**I/O not unit-tested** (project norm): `bluer` D-Bus calls and the terminal
backend. Validated by the on-device manual gate and cross-checked against
`scripts/ble_notify_check.sh` (shared `AcquireNotify` semantics).

## Risks

1. **`bluer` API name drift (0.17).** Method/enum names
   (`AdapterEvent::DeviceAdded`, `DeviceProperty::Connected`,
   `CharacteristicReader` `AsyncRead`, `notify_io`, `Service::uuid`) are taken
   from current docs but must be verified against `docs.rs/bluer/0.17` and the
   `bluer-tools` `gattcat.rs` example during implementation. _Mitigation:_ the
   verify-against-docs notes in substep 4; logic isolated in `model.rs` is
   unaffected.
2. **Disconnect detection done wrong (historical failure mode).** _Mitigation:_
   the design takes link-state from `DeviceProperty::Connected(false)` / reader
   EOF only; frame-age is a separate cosmetic signal that can never tear down the
   link. Enforced structurally — `LinkEvent` has no frame-age variant — and
   asserted in tests. Aligns with memory `feedback-ble-disconnect-detection`.
3. **`btleplug` dedup trap (avoided).** Resolved by choosing `bluer` +
   `notify_io` (AcquireNotify). If a future cross-platform need forces btleplug,
   it would require a separate raw-fd path — out of scope here.
4. **Dual-target workspace.** `meteo-tui` must never be built for riscv.
   _Mitigation:_ all recipes scope `-p meteo-tui --target host`; no `--workspace`
   builds; documented in `CLAUDE.md`. Risk: a contributor runs bare
   `cargo build --workspace` and hits a riscv build error for the host crate —
   acceptable, documented.
5. **Render-tick vs. the project "no sleeps" rule.** The 1 Hz `interval` is a
   display cadence for the live clock, not a readiness wait; all data redraws are
   event-driven. _Mitigation:_ documented in code with the same rationale as
   `watchdog.rs`'s periodic poll; reconnect uses observed signals with bounded
   `timeout` circuit-breakers, never fixed sleeps.
6. **Host BLE prerequisites.** `bluer` needs `libdbus-1-dev` at build and a
   running `bluetoothd` at runtime. _Mitigation:_ present on gaia (the dev host);
   documented as a prerequisite.
7. **Firmware `ATT_MAX` miscount.** A wrong count panics at GATT init.
   _Mitigation:_ explicit attribute breakdown in substep 1; caught immediately
   on first boot.

## Notes

Progress tracking (checked during implementation):

- [x] 1. Firmware DIS firmware-revision characteristic (`ble.rs`, `ATT_MAX` 10→13) — DIS `0x180A` + FW Revision String `0x2A26` via `add_characteristic_ro`.
- [x] 2. Scaffold `meteo-tui` crate + workspace member + skeleton — host-std binary, deps pinned (bluer 0.17, ratatui 0.30, crossterm 0.29, tokio, clap, uuid `macro-diagnostics`).
- [x] 3. `model.rs` pure logic + tests — `ConnState`/`LinkEvent` FSM, `fmt_*`, `Series`, `parse_fw_revision`; 14 tests.
- [x] 4. `ble.rs` bluer central + reconnect state machine — `notify_io`/AcquireNotify; link-state-only disconnect detection; bluer needs `bluetoothd` feature.
- [x] 5. `app.rs` AppState + apply reducer + staleness + tests — 7 tests; frame-age cosmetic only.
- [x] 6. `ui.rs` ratatui layout + render + smoke test — header/table/charts; 2 `TestBackend` smoke tests.
- [x] 7. `main.rs` tokio loop + crossterm input + clock tick + CLI + should_quit tests — `ratatui::init/restore`; 4 tests. (Wiring made model/ble live → folded in `const fn` lint fixes.)
- [x] 8. Justfile recipes + CLAUDE.md/README docs + full checks green — `tui-build`/`tui-run`/`tui-clippy`; folded into `clippy`/`test`. All 5 final checks pass (51 host tests).

**Coordination:** substep 1 (firmware) and substeps 2–3 (host scaffold + pure
logic) are independent and may proceed in either order. Substep 4 depends on
substep 1's UUIDs (`0x180A`/`0x2A26`) and on substeps 2–3. Substeps 5–7 are
strictly sequential after 4. Substep 8 last.

**Next step:** `/tyrex:code:implement-light` once the plan passes review.
