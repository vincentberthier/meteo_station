# Brainstorm: Ratatui Station Dashboard (Linux BLE central)

- **ID:** 3
- **Category:** Feature
- **Date:** 2026-06-17
- **Status:** Active

## Context

The user wants a nice-looking ratatui TUI that displays all the weather-station
data — including app/firmware version, current date/time, and graphs of the
sensor readings over time. Data comes from the live BLE connection to the
on-chip ESP32-H2 peripheral (`MeteoStation`). This is the host-side **Linux BLE
central** that `meteo-lib::ble::frame::decode()` was explicitly written for (the
"future Linux central" referenced in CLAUDE.md and the frame docs).

## Current State

- **Firmware** advertises as `MeteoStation` (static random addr
  `F0:CA:FE:00:00:01`), exposes a GATT service `7e700001-…` with a Read+Notify
  characteristic `7e700002-…`, and pushes a 17-byte v1 telemetry frame at 1 Hz.
- **Wire frame** (`crates/meteo-lib/src/ble/frame.rs`): `Telemetry` struct with 8
  `Option` fields — `temperature_c`, `pressure_hpa`, `humidity_pct`,
  `sky_temp_c`, `luminosity_lux`, `wind_speed_ms`, `wind_dir_deg`, `battery_pct`.
  `decode(&[u8]) -> Result<Telemetry, FrameError>` maps sentinels back to `None`.
  Host-tested (24/24), no_std, builds on the host — directly reusable.
- **Today the device only populates `temperature_c` + `pressure_hpa`** (BMP388).
  The other 6 fields are sentinels → `None`.
- **The frame carries no timestamp and no firmware version** — only a
  protocol-version byte (`0x01` at byte[0], validated by `decode`). The central
  must stamp arrival time itself.
- **No host/std crate exists yet** — the workspace has only `meteo-lib` (no_std
  lib) and `meteo-firmware` (riscv32 binary). esp deps are gated behind
  `cfg(target_arch = "riscv32")`.
- **Proven central-side mechanics** (`scripts/ble_notify_check.sh`): connect by
  address (no blocking scan — adapter-wedging trap), wait for `ServicesResolved`,
  subscribe via BlueZ `AcquireNotify` to get every raw PDU (the `Value` property
  dedupes near-constant telemetry to silence). The TUI must subscribe the same
  raw-notify way, not value-change events.

## Findings

- **Runs locally on gaia.** The working machine _is_ gaia (the BlueZ 5.86 host
  with the adapter) — not a separate workstation and not `hephaistos`. The TUI,
  the adapter, and the station are all local; no `scp`/`ssh` hop. (Recorded in
  memory `project-dev-host-is-gaia`; note that existing CLAUDE.md/README/scripts
  still describe gaia from an off-box perspective with `scp … gaia:`.)
- **New workspace crate** is the natural home: a std binary crate (e.g.
  `crates/meteo-tui`) that depends on `meteo-lib` (`default-features = false`) to
  reuse `Telemetry`/`decode`. It builds for the host target only; the existing
  riscv32 firmware build is unaffected because the esp deps stay cfg-gated and
  the new crate is host-std. Need to confirm the workspace lints (the heavy
  clippy restriction set tuned for no_std embedded) make sense for a std TUI crate
  or get per-crate relaxations — an implementation detail for the planner.
- **BLE central library:** `btleplug` is the standard Rust cross-platform central
  (BlueZ/D-Bus backend on Linux, tokio-based). It exposes peripheral discovery,
  connect, characteristic subscribe → a notification stream of raw bytes — exactly
  the `AcquireNotify` semantics the scripts rely on. Pairs with `ratatui` +
  `crossterm` and a tokio event loop. (Library choice is a planner decision, but
  btleplug is the obvious candidate and shapes feasibility — it's feasible.)
- **Graphs:** ratatui ships `Chart` (line/scatter with axes) and `Sparkline`.
  Temperature and pressure (the only live series today) plot as time-series line
  charts; the in-memory ring buffer feeds them.
- **Auto-connect** mirrors the scripts' scan-then-connect-by-address pattern,
  with automatic reconnect on drop (the firmware re-advertises immediately and the
  8 s supervision negotiation re-runs each connection). The reconnect-needs-a-fresh-
  scan caveat (BlueZ evicts the non-bonded LE object) applies to the central here
  too — the TUI's reconnect path must re-scan briefly before reconnecting.

## Scope

**In scope:**

- A new host-side std crate rendering a ratatui dashboard.
- Live BLE central: auto-connect to `F0:CA:FE:00:00:01` (address
  config/CLI-overridable), subscribe to the notify characteristic, decode frames
  via `meteo-lib`, handle disconnect + auto-reconnect. See **Connection
  lifecycle** below — this is a known sharp edge and a hard requirement.
- Display **all 8 telemetry fields**, showing `N/A` for fields that are `None`
  (most are, today). Graceful, not hidden.
- **Graphs** (ratatui `Chart`/`Sparkline`) of the time series held in an
  **in-memory ring buffer** for the running session (no disk persistence). The
  live series today are temperature and pressure; the layout should accommodate
  the other fields becoming live later.
- Header showing **current date/time** (local clock, live-updating) and a
  **version** panel: the **TUI app version** (from its Cargo.toml) and the
  **device firmware version**.
- Connection-status indicator (connected / reconnecting / link metrics if cheap).

### Connection lifecycle (disconnect detection — handle with care)

Previous iterations got disconnect detection wrong; this is a first-class
requirement, not an afterthought. The state machine is **Scanning → Connecting →
Subscribed/Live → (link lost) → Scanning → …**, and it must obey these rules:

- **Detect disconnect from the BLE stack's link-state signal, NEVER from
  data-flow silence.** Telemetry is near-constant and BlueZ value-deduped, so a
  gap in notifications does **not** mean the link dropped — and a healthy 1 Hz
  feed can stall briefly without a drop. The authoritative trigger is the central
  stack's link event (btleplug `CentralEvent::DeviceDisconnected` / the
  peripheral's `is_connected()` going false / the notification stream _ending_),
  equivalent to the scripts' BlueZ `Connected` D-Bus property. Drive the reconnect
  state machine off that signal alone.
- **Keep data-freshness separate from link-state.** A "last frame age" watchdog is
  fine as a **UI staleness indicator** (e.g. grey the values after N s of no
  frames), but it must **not** be the disconnect trigger and must not by itself
  tear down / reconnect the link. Two independent signals: link-state (authoritative,
  drives reconnect) and frame-age (cosmetic/staleness only).
- **Reconnect requires a fresh bounded scan first.** BlueZ evicts the non-bonded LE
  device object after disconnect, so a cold reconnect-by-address fails ("not
  available") until a bounded, self-terminating discovery repopulates the cache.
  Mirror the scripts: bounded scan → connect-by-address → `discover_services` →
  re-subscribe. Never a blocking/unbounded scan (wedges the adapter in
  "Discovering: yes").
- **Reconnect loop must be observe-driven, not sleep-driven.** Retry on the real
  signals (scan result present, connect result, services-resolved), with a bounded
  per-attempt deadline as a circuit-breaker only — no fixed "wait then assume
  ready" delays. The firmware re-advertises immediately on disconnect and re-runs
  the 8 s supervision negotiation each connection, so a clean reconnect is expected.
- **Surface every state transition in the UI** (Live / Reconnecting / Scanning /
  Connecting) so a botched detection is visible, not silent.

**Firmware change required (in scope, small):** the device does **not** transmit
a firmware version today. To show it, the firmware must expose it — e.g. a GATT
**Device Information Service** "Firmware Revision String", or a small custom
read characteristic — which the central reads once on connect. This is extra
embedded work beyond the pure host app.

**Out of scope:**

- Disk persistence / historical logging (explicitly in-memory only).
- Interactive scan + device picker UI (auto-connect to the known address only).
- Showing the frame _protocol_ version in the UI (byte[0] is still validated by
  `decode`; it's just not surfaced as a display field).
- Any new sensors / firmware telemetry fields beyond the firmware-version read.
- Rewriting the existing soak/notify shell scripts or their off-box `scp`/`ssh`
  framing.

## Open Questions

Implementation-specific (_how_), for the planner:

1. **Crate layout & lints:** new `crates/meteo-tui` std binary in the workspace
   vs. how to scope the no_std-tuned workspace clippy config for a std crate
   (per-crate `[lints]` override vs. carve-outs).
2. **Async/UI architecture:** tokio task for the btleplug notification stream +
   crossterm input + a render tick, communicating over a channel; exact widget
   layout and ring-buffer sizing/window.
3. **BLE crate confirmation:** verify `btleplug` on gaia's BlueZ 5.86 delivers
   raw notifications equivalent to `AcquireNotify` (no value-dedup surprise);
   confirm how btleplug reports link loss (`CentralEvent::DeviceDisconnected`
   reliability vs. polling `is_connected()` vs. notification-stream end) so the
   disconnect-detection rule above has a concrete API hook; confirm the
   reconnect-needs-rescan flow within btleplug's API (`start_scan`/`stop_scan` →
   reconnect-by-address).
4. **Firmware version transport:** Device Information Service (standard
   `0x180A` / Firmware Revision String `0x2A26`) vs. a custom read characteristic;
   where the version string comes from in firmware (Cargo `CARGO_PKG_VERSION` /
   git describe). Affects both firmware and central.

## Next Steps

- Run `/tyrex:code:plan-light 3` to turn this into an implementation plan.
- The plan should cover two coordinated parts: (a) the host `meteo-tui` crate and
  (b) the small firmware change to expose a firmware-version GATT read.
