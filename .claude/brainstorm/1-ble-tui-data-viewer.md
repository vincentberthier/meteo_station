# Brainstorm: BLE TUI Data Viewer

- **ID:** 1
- **Category:** Feature
- **Date:** 2026-06-14
- **Status:** Active

## Context

The user wants a small ratatui CLI application that connects to the
`MeteoStation` device over BLE and displays its sensor data live. Today that
data is just temperature and pressure, but the viewer must be designed so new
sensors (humidity, light, wind, …) can be added with minimal effort.

## Current State

A `meteo-cli` crate already exists (`crates/meteo-cli`). It is a one-shot
`btleplug` BLE central that:

- scans for a peripheral named `MeteoStation`,
- connects and discovers the custom GATT service,
- reads initial temperature + pressure values,
- subscribes to notifications, and
- prints each reading to stdout, exiting when the device disconnects.

Relevant facts discovered during exploration:

- **Data model** — one custom GATT service (`METEO_SERVICE_UUID`,
  `…3f2e1a00`) with one characteristic per sensor, each carrying a 4-byte
  little-endian `f32`. UUIDs are sequential: temperature `…1a01`, pressure
  `…1a02`; a future sensor would be `…1a03`, etc. Defined in
  `crates/meteo-lib/src/ble/gatt.rs`.
- **Shared decoding** — `meteo-lib::ble::encoding` exposes `decode_f32` /
  `encode_f32`, and `gatt` exposes the UUID constants. `meteo-cli` already
  reuses these, so `meteo-lib` is the natural home for shared, host-agnostic
  logic. Note `meteo-lib` is `no_std` (std only under `#[cfg(test)]`).
- **Cadence** — the firmware produces a reading ~1 Hz and only pushes
  notifications once the central has subscribed (CCCD write).
- **Host constraint** — this dev machine has no Bluetooth adapter; BLE clients
  run on the `gaia` host (which has `hci0`). The Justfile has `just cli`
  (local) and `just cli-gaia` (remote over ssh). A TUI will therefore be run
  on a machine with an adapter (Gaia for now), interactively over ssh.
- **The plain CLI is load-bearing** — `scripts/ble-debug.sh` captures
  `meteo-cli` stdout into `cli-<ts>.log` as part of the correlated BLE debug
  capture. A full-screen TUI must not replace that text output.

## Findings

The feature is clearly feasible and most of the BLE plumbing already exists in
`meteo-cli`. The work is: a new TUI crate that reuses `meteo-lib` decoding +
a new shared sensor registry, wraps the existing `btleplug` connect/subscribe
flow in an auto-reconnecting loop, and renders current values plus history
charts with ratatui.

Resolved scope decisions (from clarification):

1. **Crate layout — new `meteo-tui` crate.** `meteo-cli` stays as the plain
   text/capture tool used by `ble-debug.sh`. Both share `meteo-lib`. No change
   to the existing capture flow.
2. **Display — current values + history charts.** Per-sensor: a large current
   readout, a history sparkline/line chart of recent values, and min/max/avg
   stats. (No dedicated log pane in this version; a connection-status line is
   in scope.)
3. **Disconnect handling — auto-reconnect, keep history.** On the ~30s firmware
   disconnect, show a "reconnecting" state, re-scan/connect automatically, and
   preserve accumulated per-sensor history across the gap.
4. **Persistence — in-memory only.** History lives for the session; nothing is
   written to disk. Bounded in-memory ring buffer per sensor.
5. **Extensibility — shared sensor registry in `meteo-lib`.** A table mapping
   UUID → { name, unit, display precision, optional value transform (e.g.
   Pa→hPa) }. Adding a sensor is a single registry entry that both firmware and
   TUI draw from as the single source of truth. Unknown characteristics are
   ignored (no raw fallback). The TUI iterates the registry to lay out panels,
   so new sensors appear with correct names/units automatically.

## Scope

In scope:

- A new `crates/meteo-tui` ratatui application (host-only, `btleplug` + tokio,
  like `meteo-cli`).
- A shared sensor registry in `meteo-lib` describing each sensor
  (UUID, name, unit, precision, optional transform).
- Auto-reconnecting BLE client loop that scans, connects, subscribes to all
  registered sensors' characteristics, and feeds readings to the UI.
- TUI rendering: per-sensor current value, history chart, min/max/avg, and a
  connection-status indicator. Layout driven by the registry.
- Bounded in-memory history per sensor.
- A Justfile recipe to run it (local + Gaia, mirroring `cli` / `cli-gaia`).

Out of scope:

- Firmware changes / adding new physical sensors (the registry is built to make
  that a one-line follow-up, but no new sensor is added here).
- On-disk persistence / logging of readings.
- Non-`f32` characteristic encodings (all current/planned values are 4-byte LE
  f32; revisit when a sensor needs a different wire format).
- A dedicated scrollable event/log pane.
- Replacing or modifying `meteo-cli`'s plain output.

## Open Questions

_Implementation-specific only — for the planner:_

- Where the auto-reconnect BLE client logic should live: a module inside
  `meteo-tui`, or a shared host-side (`std`/tokio, feature-gated) client in
  `meteo-lib`. `meteo-lib` being `no_std` makes a feature-gated host module the
  awkward-but-possible option; a `meteo-tui`-local module is simpler. Decide
  during planning.
- Exact registry data structure and how the TUI iterates it to build panels
  (slice of descriptors with a decoder fn vs. enum-based).
- History ring-buffer sizing/policy (fixed point count vs. time window) and the
  ratatui widget choice for charts (`Sparkline` vs. `Chart`/`Dataset`).
- Async/runtime structure: how the tokio BLE task communicates readings to the
  render loop (channel) and how input + redraw are scheduled.
- Keybindings (at minimum quit); whether to support per-sensor focus/scroll.

## Next Steps

Run `/tyrex:code:plan-light 1` to turn this into an implementation plan.
