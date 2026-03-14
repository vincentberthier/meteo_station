# Fix BLE status event parsing — strip `%` delimiters

## Context

All BLE status events (`%CONNECT%`, `%WC%`, `%DISCONNECT%`, etc.) are logged as `Unknown`
because the `%` delimiters aren't stripped before parsing. This single bug causes:

1. `CONNECT` events not recognized → `connected` flag never set to `true`
2. `WC` (CCCD write) events not recognized → notification subscriptions not logged
3. Since `connected` is always `false`, sensor data is never pushed over BLE

## Root cause

- `LineBuffer::process_status_event()` passes the full event **including** `%` delimiters
  to its callback (e.g., `%CONNECT,0,D8F3BC632E56%`) — by design, documented in line_buffer.rs:209
- `parse_status_event()` (status_parser.rs:16) expects the inner content **without** delimiters
  (e.g., `CONNECT,0,D8F3BC632E56`)
- `ble.rs:262` calls `parse_status_event(event)` without stripping — integration bug

## Fix: Strip in `process_status_event`

Change `LineBuffer::process_status_event` to pass only the inner content (without `%`
delimiters) to the callback. Every caller needs the inner content, never the delimiters.

### Changes in `crates/meteo-lib/src/ble/line_buffer.rs`

1. **Line 227** — change callback invocation:
   - From: `f(&self.buf[event_start..event_end])`
   - To: `f(&self.buf[event_start + 1..event_end - 1])`

2. **Lines 205-213** — update doc comment to say content is passed **without** `%` delimiters

3. **Tests** — update all `process_status_event` test assertions:
   - `b"%DISCONNECT%"` → `b"DISCONNECT"`
   - `b"%CONNECT,1,AABBCCDDEEFF%"` → `b"CONNECT,1,AABBCCDDEEFF"`
   - `b"%DISCONNECT%"` and `b"%CONNECT,0,112233445566%"` in the multiple-events test

No changes needed in `ble.rs` — it already calls `parse_status_event(event)` which
will now receive correctly stripped content.

## Verification

1. `just test` — all existing tests pass with updated assertions
2. `just clippy` — no warnings
3. Flash to device, connect with BLE client, verify:
   - `CONNECT` event recognized (not `Unknown`)
   - `WC` events recognized (CCCD writes logged)
   - Sensor data (temperature/pressure) appears on BLE client
