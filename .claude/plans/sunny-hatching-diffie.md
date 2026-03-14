# Debug: BLE connection drops after ~30s, no GATT notifications

## Context

After fixing the scan/advertising issue, the CLI connects to MeteoStation but:

1. **No notifications** — CLI subscribes but never receives sensor data
2. **Auto-disconnect after ~30s** — connection drops consistently
3. **No connect/disconnect events visible in RTT** — because the firmware doesn't log them (lines 263-264 of `ble.rs` silently set `connected` state without any `info!`/`debug!` output)

## What the research tells us

**30-second disconnect** is almost certainly the **BLE GATT ATT procedure timeout** (BLE spec, not RN4871-specific). It fires when a GATT operation from the central goes unanswered for 30 seconds. The RN4871's supervision timeout is only ~5s — a different mechanism entirely.

**Notifications via SHW** should work: `SHW,<handle>,<data>` writes the value and triggers notification delivery to subscribed clients. SHW returns `NFail` when notification delivery fails (firmware already treats NFail as success — silently).

**btleplug `subscribe()`** automatically writes the CCCD (0x0001) to enable notifications. The RN4871 sends a `%WC,<cccd_handle>,<value>%` event to the MCU when this happens.

## Hypotheses to test (ordered by likelihood)

### H1: Firmware enters command mode while CLI is still doing GATT operations

The monitoring loop enters command mode (`$$$` → SHW → `---`) as soon as `connected == true` AND sensor data is available. If a buffered sensor reading is already in the channel when the client connects, the firmware enters command mode immediately — potentially before the CLI finishes service discovery, initial reads, or CCCD subscription.

While the firmware is in command mode, `wait_for_marker` and `send_command` consume ALL UART data and discard anything that isn't the expected response. Status events (`%CONNECT%`, `%WC%`, `%DISCONNECT%`) arriving during these windows are silently lost.

**Evidence needed:** btmon trace showing timing of GATT operations vs firmware entering command mode.

### H2: SHW returns NFail because the client hasn't subscribed yet

The firmware does SHW as soon as it has data, without checking whether the client has written the CCCD. The first SHW call might happen before the CLI has subscribed. Since NFail is treated as success, the firmware thinks it worked but no notification was delivered.

**Evidence needed:** Debug logging of SHW responses (AOK vs NFail).

### H3: RN4871 can't deliver notifications while in command mode

The datasheet says SHW "triggers notify," but it's unclear whether the BLE stack can deliver the notification while the module is still in command mode. If the notification is only queued and delivered after exiting command mode, timing matters.

**Evidence needed:** btmon trace showing whether notification PDUs are sent during or after command mode.

### H4: The 30-second timeout is caused by an unhandled GATT request

The CLI sends ATT requests (MTU exchange, service discovery, reads, CCCD writes). If any of these stalls because the RN4871 is in command mode and can't respond to the BLE client, the 30-second ATT timeout fires and the central disconnects.

**Evidence needed:** btmon trace showing an ATT request with no response.

## Debugging plan (NO code changes to core logic)

### Step 1: Add diagnostic logging to the firmware

**File:** `crates/meteo-firmware/src/ble.rs`

Add `info!` logging for the events that are currently silent:

```rust
StatusEvent::Connect { address_type, address } => {
    info!("BLE: connected (type={=u8}, addr={})",
        address_type,
        str::from_utf8(address).unwrap_or("?"));
    connected = true;
}
StatusEvent::Disconnect => {
    info!("BLE: disconnected");
    connected = false;
}
```

Also add logging around the SHW phase:

```rust
// Before entering command mode for SHW:
debug!("BLE: pushing sensor data (t={}, p={})", reading.temperature, reading.pressure);

// After each SHW execute(), log the result explicitly (AOK vs NFail is invisible today)
```

### Step 2: Capture btmon HCI trace

Run `btmon` in a separate terminal BEFORE connecting the CLI. This captures:

- Exact timing of ATT requests and responses
- Whether notification PDUs are sent by the RN4871
- What causes the disconnect (which side, reason code)
- Whether there's an unanswered ATT request hanging for 30 seconds

```bash
# Terminal 1:
doas btmon | tee /tmp/btmon-meteo.log

# Terminal 2:
just run   # firmware with debug logs

# Terminal 3:
just cli   # connect and wait for disconnect
```

### Step 3: Correlate RTT logs with btmon trace

Cross-reference timestamps:

- When does `%CONNECT%` arrive vs when does the firmware enter command mode?
- Does btmon show notification PDUs from the RN4871?
- What ATT request (if any) goes unanswered before the disconnect?
- What's the HCI disconnect reason code? (0x08 = connection timeout, 0x13 = remote user terminated, 0x22 = instant passed)

### Step 4: Determine root cause and plan the fix

Based on the evidence, the fix will likely involve one or more of:

- **Delaying SHW** until after the client has subscribed (wait for `%WC%` event)
- **Not entering command mode** while GATT operations might be in progress (e.g., add a post-connect settling delay)
- **Checking for status events** during command mode transitions instead of discarding them

## Files involved

- `crates/meteo-firmware/src/ble.rs` — monitoring loop (diagnostic logging only)
- `crates/meteo-lib/src/ble/driver.rs` — command mode methods (read-only analysis)
- `crates/meteo-cli/src/main.rs` — CLI connection flow (read-only analysis)

## What NOT to do yet

- Do NOT change the monitoring loop logic
- Do NOT change the command mode / SHW flow
- Do NOT change connection parameters or timeouts
- Only add diagnostic logging, then collect data
