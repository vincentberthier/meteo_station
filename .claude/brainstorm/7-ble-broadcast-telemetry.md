# Brainstorm: BLE broadcast telemetry (with reserved connectable channel)

- **ID:** 7
- **Category:** Feature (transport-architecture change)
- **Date:** 2026-06-25
- **Status:** Active

## Context

The station currently pushes telemetry over a single BLE GATT connection (one
central at a time) via a Notify characteristic at 1 Hz. The user wants any number
of observers to read the weather data at once, and asked whether to switch to
**broadcast** for all meteo data — provided it does not cost more power — while
**keeping a connectable in/out channel open** for future bidirectional use (not
used now). Cadence stays **1 Hz** ("more than enough").

## Current State

- **Firmware (`crates/meteo-firmware/src/ble.rs`)** advertises as `MeteoStation`
  (connectable, static-random `F0:CA:FE:00:00:01`), accepts one central, attaches
  a manual `AttributeServer` (custom 128-bit telemetry service with a Read+Notify
  characteristic + DIS firmware-revision string), negotiates an 8 s supervision
  timeout via the vendored trouble-host L2CAP patch, and notifies the 28-byte v4
  frame at 1 Hz until disconnect, then re-advertises.
- **Wire frame (`meteo-lib::ble::frame`)** is a fixed 28-byte v4 layout
  (`Telemetry`, `encode`/`decode`, version sentinel `0x04`). `encode` is used by
  firmware; `decode` by the TUI.
- **Watchdog (`watchdog.rs`)** feeds the RWDT only when BMP + aggregator + BLE
  are all live; BLE liveness = `ADV_BEAT || BLE_BEAT` advancing.
- **TUI (`crates/meteo-tui`)** is a `bluer` central: scan → connect → resolve →
  `notify_io()` → decode frames; a `ConnState` state machine drives a status bar;
  reads firmware version once over DIS on connect.

## Findings

Feasibility: **confirmed in the vendored trouble-host 0.6 source.**

- `Advertisement::ExtConnectableNonscannableUndirected { adv_data }` is
  **connectable AND carries `adv_data`**. A passive scanner reads its
  manufacturer-data payload without connecting, while a central can still connect.
  So a **single extended advertisement does both jobs** — no second advertising
  set, `ADV_SETS` stays 1, no vendored-crate change.
- `AdStructure::ManufacturerSpecificData { company_identifier, payload }` exists;
  `Peripheral::advertise_ext(&[set], &mut handles)` starts it and
  `update_adv_data_ext` refreshes the payload **in place at 1 Hz without
  restarting advertising** (proper beacon-update path). `try_accept()`
  (non-consuming) polls for an incoming connection from the same 1 Hz loop while
  the broadcast keeps running.
- Power: broadcast is **lower** power than today's persistent connection (no
  ~80 ms connection-event upkeep). A connectable advert adds a tiny per-event RX
  window vs pure broadcast — the cost of literally keeping the channel open, which
  the user wants. Net vs current design: a power win.
- Extended advertising removes the legacy 24-byte payload ceiling (~hundreds of
  bytes available), so the frame can grow freely.
- Watchdog needs **no change**: `ADV_BEAT` advancing every second keeps
  `ble_alive` true; `BLE_BEAT` will also be bumped on each broadcast refresh.

One fact needs **on-device confirmation** (only if we ever go dual-set):
`LeReadNumberOfSupportedAdvSets` on the ESP32-H2. Not needed for the single-set
design chosen here.

### Resolved design decisions ("what" / "why")

1. **One advertisement, not two.** `ExtConnectableNonscannableUndirected` carries
   the broadcast payload and stays connectable. Behavioral consequence, accepted:
   while a central is _actively connected_ (rare — channel unused now), the
   broadcast pauses and resumes on disconnect. Continuing broadcast _during_ a
   live connection is exactly when a second non-connectable set would be added —
   deferred until there's a real connected use.
2. **Carrier:** Manufacturer-Specific Data, company id `0xFFFF` (reserved/test).
   Sole consumer ⇒ company-id collisions and identity are non-issues; simplest.
3. **Frame growth:** compatibility is **not a consideration** — the user is the
   only consumer, firmware and TUI ship together. No append-only/TLV/versioning
   ceremony; the frame layout may change freely whenever a field is added. (The
   `0x04` sentinel byte can stay as a cheap sanity marker, not a compat contract.)
4. **Add an uptime-seconds field** to the broadcast frame. Purpose: forces the
   payload to change every second (defeats BlueZ `manufacturer_data` dedup, the
   same trap the notify path documents), lets the TUI detect dropped frames, and
   signals device reboots/age. Monotonic, resets on boot.
5. **Connectable channel:** reserve **one stable 128-bit service UUID now**, as a
   bare/empty ("reserved") GATT service. No committed purpose yet. The connection
   carries **no meteo data** (that moved to broadcast). The firmware still accepts
   a connection, serves the bare service, and re-advertises on disconnect; the 8 s
   supervision-timeout negotiation is retained for whenever the channel is used.
6. **TUI rework:** replace connect+notify with **passive scanning + decode of the
   manufacturer-data frame**. The `ConnState` lifecycle machine is vestigial and
   collapses to a **frame-age status** model (e.g. No signal → Live → Stale),
   which is now the honest signal (no link state to be authoritative). The
   DIS-read **firmware-version header is dropped** for v1 (the dashboard no longer
   connects); a build/version byte could ride in the broadcast frame later if
   wanted.

## Scope

**In scope**

- `meteo-lib::ble::frame`: add the uptime-seconds field; adjust `Telemetry`,
  `encode`, `decode`, `FRAME_LEN`, and host round-trip tests.
- Firmware `ble.rs`: single `ExtConnectableNonscannableUndirected` advert carrying
  the manufacturer-data frame, refreshed at 1 Hz via `update_adv_data_ext`; bare
  reserved GATT service behind a new fixed UUID; accept/serve/re-advertise loop
  using `try_accept`; keep `ADV_BEAT`/`BLE_BEAT` beats. Track on-device uptime.
- TUI: passive-scan acquisition path; frame-age status model; drop DIS firmware
  display; keep all rendering, formatting, charts, and the diagnostics row.
- Docs: update `CLAUDE.md` (BLE section, frame layout, TUI rationale) and the
  `scripts/ble_*.sh` acceptance notes for the broadcast model.

**Out of scope (noted for later)**

- Second (non-connectable) advertising set for broadcast-during-connection.
- Defining/implementing the connectable channel's actual function (config writes,
  set-time, command/control such as parking the MLX servo, OTA, history backfill).
- Adaptive advertising (drop the connectable property on low battery to shed RX
  cost).
- Multi-station identity (a station-id field) and PIN/pairing on the connection
  (pairing attaches to the _connection_, not the public broadcast).

## Open Questions

Implementation-specific only (for the planner):

- Uptime field width/type (`u16` wraps ~18 h vs `u32` ~136 y) and its exact byte
  offset in the frame; whether to keep the `0x04` sentinel and where.
- Exact `AdStructure` assembly and length/MTU bookkeeping for the extended
  advert; whether to include `CompleteLocalName` alongside the manufacturer data.
- Advertise-loop structure: holding the `Advertiser` while polling `try_accept`
  and `update_adv_data_ext`, and re-arming the connectable set after a disconnect
  (single advertise-command lock; re-`advertise_ext` resets the set).
- Reserved service UUID value and minimal attribute-table sizing (`ATT_MAX`,
  `CCCD_MAX`) for a bare service.
- TUI scan mechanics in `bluer`: passive discovery stream vs `AdvertisementMonitor`,
  and exactly how manufacturer-data updates are surfaced without dedup loss;
  concrete new status enum.
- How the firmware sources monotonic uptime (Embassy `Instant`).

## Next Steps

- Run `/tyrex:code:plan-light 7` to turn this into an implementation plan.
- The H2 `LeReadNumberOfSupportedAdvSets` check is only needed if the deferred
  dual-set option is ever taken — not for this plan.
