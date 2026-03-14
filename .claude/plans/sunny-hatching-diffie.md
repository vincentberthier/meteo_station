# Fix: `send_multiline_query` treats `CMD>` line as `UnexpectedResponse`

## Context

The CLI (`meteo-cli`) fails with "MeteoStation not found" because the BLE device was running stale firmware. After reflashing, RTT output shows the firmware reaches advertising successfully, but the `LS` (ListServices) command produces a spurious `UnexpectedResponse` warning.

**Root cause:** `send_multiline_query` in the RN4871 driver has two termination paths:

1. Drain framed lines via `process_line` + `parser::parse` (inner loop)
2. Check raw buffer for `CMD>` marker (after inner loop)

If the RN4871 sends `CMD>\r\n` (with line ending), path 1 processes it first as `Response::Cmd`, which falls into the `_ => UnexpectedResponse` catch-all. Path 2 never runs because the data was already consumed.

The firmware tolerates this because the callback has already populated the GATT handles before the error, and the code continues past the warning. But it's still a bug — it leaves the driver's line buffer in an inconsistent state after the error.

## Plan

### Step 1: Fix `send_multiline_query` to recognize `Response::Cmd` as success

**File:** `crates/meteo-lib/src/ble/driver.rs`, lines 346-387

In the inner loop's match on parsed responses, add `Response::Cmd` as a successful termination condition, equivalent to finding the `CMD>` marker:

```rust
// In the inner loop match:
Response::Data(line_data) => { ... }  // existing
Response::Err => { ... }              // existing
Response::Cmd => {
    // CMD> arrived as a framed line — treat as success
    result = Some(Ok(()));
}
_ => {
    result = Some(Result::Err(Error::UnexpectedResponse));
}
```

### Step 2: Add a test for `CMD>` arriving as a framed line

**File:** `crates/meteo-lib/src/ble/driver.rs` (test module)

Add a test where the LS response includes `CMD>\r\n` as a line-terminated response, verifying that `query_multiline` succeeds and the callback receives all data lines.

## Verification

1. `just test` — all existing + new tests pass
2. `just clippy` — no warnings
3. Flash firmware, check RTT: the `LS` warning should be gone
4. Run `just cli` — should find and connect to MeteoStation

## Files to modify

- `crates/meteo-lib/src/ble/driver.rs` — fix match arm + add test
