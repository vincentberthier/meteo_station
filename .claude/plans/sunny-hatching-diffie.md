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

## FINDINGS (2026-06-14 capture) — H1 CONFIRMED

Capture: `logs/{rtt,btmon,cli}-20260614-145304.log`. Central = Gaia (`D8:F3:BC:63:2E:56`).

**Root cause: the firmware enters RN4871 command mode the instant the client
connects, which prevents the module from answering GATT service discovery.**

Evidence, correlated:

- **RTT** — immediately after `BLE: connected (type=0, addr=D8F3BC632E56)` the firmware
  logs `BLE: pushing sensor data (...) via SHW; entering command mode`, then goes
  **silent**: no `temp SHW ok`, no `exited command mode`, no `disconnected`. A sensor
  reading was already queued (`sensor_channel`), so Phase 2 fired on the first loop
  iteration after connect.
- **HCI** — MTU exchange and one `Read By Type Request` are answered, then the central
  sends `Read By Group Type Request` (Primary Service 0x2800, `0x0001-0xffff`) at
  `t=12.344` and gets **no response**. At `t=12.881` the link drops with
  `Disconnect Complete, Reason: Connection Timeout (0x08)` (LL supervision timeout).
- **CLI** — `Error: Other(ServiceDiscoveryTimedOut)` — never reaches reads/subscribe.

Two distinct problems, in order:

1. **Premature command mode.** Entering command mode (`$$$`) on connect makes the
   RN4871 stop bridging GATT, so primary service discovery is never answered. This is
   H1; it also produces the H4 symptom (unanswered ATT request → timeout), but the
   trigger is H1, not an inherently unhandled request.
2. **Deadlock in the SHW path.** The BLE task never logs `temp SHW ok` nor recovers —
   it is blocked in `enter_command_mode()`/`execute()` awaiting a `CMD>`/`AOK` marker
   that the module does not emit the expected way while in an active connection. There
   is no timeout on that await, so the task is stuck even after the link drops (the
   `%DISCONNECT%` is never processed).

H2/H3 are moot: the client never gets far enough to subscribe, so SHW timing vs CCCD
is not the issue here.

## FIX (implemented — `fix(ble): defer GATT push until client subscribes`)

- Phase 2 now gates on a CCCD notify-enable (`%WC%` with value `01..`) tracked in a
  `subscribed` flag, not merely `connected`. Discovery + initial reads complete before
  the firmware ever enters command mode. `subscribed` resets on `%DISCONNECT%`.
- Each command-mode op (`enter`/`execute`/`exit`) is wrapped in a 500 ms
  `CMD_OP_TIMEOUT` circuit-breaker so a missing handshake marker can't wedge the task.

## VALIDATION (2026-06-14 capture `…-161014`) — fix works; H3 now isolated

Re-ran `just ble-debug` after the fix (took a few attempts — Gaia's controller threw
transient `le-connection-abort-by-local` / HCI `0x3e` establishment failures unrelated
to the firmware; the firmware's pre-connect path is unchanged by the fix).

What the fix achieved (all previously broken):

- **Service discovery completes** — CLI: `Connected. Discovering services...` then
  `Subscribed to temperature notifications` / `Subscribed to pressure notifications`.
- **Connection is stable** — RTT shows **0 disconnect events** over the whole session
  (the deterministic ~30 s drop is gone). The client listens indefinitely.
- **No deadlock** — the `CMD_OP_TIMEOUT` circuit-breaker fires and the task recovers
  every cycle (`enter`→SHW→`exit` repeats cleanly, ~390 cycles).
- RTT confirms the gate: `CCCD write handle=0073 data=0100 (notify=true)` flips
  `subscribed`, then Phase 2 starts pushing.

What is still broken: notifications never reach the client. The HCI trace shows **0
`Handle Value Notification` PDUs** the entire session, despite the central writing both
CCCDs to enable notify. Per RTT cycle: `enter_command_mode` (`$$$`) and
`exit_command_mode` (`---`) succeed, but each `SHW,<handle>,<value>` gets **no `AOK`**
within 500 ms → `temp/pressure SHW timed out` → value never updated → no notification.

**Likely root cause — off-by-one handle (revised by HCI evidence).** The central's GATT
discovery (ground truth, btmon `…-161014`) shows:

| Characteristic        | Value handle | CCCD handle |
| --------------------- | ------------ | ----------- |
| temperature (`…1a01`) | **0x0072**   | 0x0073      |
| pressure (`…1a02`)    | **0x0075**   | 0x0076      |

The firmware discovers and uses **0x0073 / 0x0076** as its `SHW` target handles — those
are the **CCCD descriptor** handles, not the characteristic **value** handles. `SHW` to a
CCCD descriptor isn't a valid server write, so the module never `AOK`s. This supersedes
the earlier "RN4871 can't `SHW` during a connection" guess: the more likely problem is
simply the wrong handle.

## RESOLUTION (2026-06-14 capture `…-170851`) — notifications confirmed flowing

**Root cause confirmed by observing the raw LS output.** Temporary instrumentation
(`info!("BLE: LS line: …")` in the discovery callback) showed the RN4871 lists a
read+notify characteristic as **two** LS lines sharing one UUID:

```
  A4E64B8B…1A01,0072,02       <- value handle 0072, property 0x02 (READ)
  A4E64B8B…1A01,0073,10,0     <- CCCD  handle 0073, property 0x10 (NOTIFY)
  A4E64B8B…1A02,0075,02       <- value handle 0075, property 0x02 (READ)
  A4E64B8B…1A02,0076,10,0     <- CCCD  handle 0076, property 0x10 (NOTIFY)
```

`gatt::collect_handles` matched by UUID and let the second (CCCD) line overwrite the
value handle, so `SHW` targeted the CCCD descriptor (`0073`/`0076`) — never a valid
write — and no notification was triggered.

**Fix** (`fix(ble): discover characteristic value handle, not CCCD, for SHW`): the LS
parser now extracts the property field, and `collect_handles` skips the CCCD line
(notify/indicate-only, no read/write bits), keeping the value handle. New host tests
cover the two-line decomposition and the off-by-one regression.

**End-to-end validation** (`logs/{rtt,btmon,cli}-20260614-170851.log`), all three streams
agree:

- RTT: `handles discovered: temperature=0072, pressure=0075`, then **52 `… SHW ok`** and
  **0 timeouts** (was ~390 timeouts / 0 ok).
- HCI: **136 `Handle Value Notification` PDUs** on handle `0x0072`/`0x0075` (was 0).
- CLI: live `Temperature: 24.5x°C` / `Pressure: 1009.9 hPa` streaming.

Notes:

- The earlier H1/connect fix is unchanged and still holds — the firmware no longer enters
  command mode before the client subscribes.
- Connection establishment was flaky during validation (HCI `0x3e` /
  `le-connection-abort-by-local`, supervision-timeout `0x08`) — a Gaia-controller RF/timing
  issue independent of the firmware. `scripts/ble-debug.sh` now retries these transients.
- Minor (unchanged): the firmware logs only one `%WC%` (temp); the pressure `%WC%` arrives
  while it is mid-SHW in command mode and is dropped. Harmless — subscribe is already
  latched and notifications flow for both characteristics.

## Files involved

- `crates/meteo-firmware/src/ble.rs` — monitoring loop (diagnostic logging only)
- `crates/meteo-lib/src/ble/driver.rs` — command mode methods (read-only analysis)
- `crates/meteo-cli/src/main.rs` — CLI connection flow (read-only analysis)

## What NOT to do yet

- Do NOT change the monitoring loop logic
- Do NOT change the command mode / SHW flow
- Do NOT change connection parameters or timeouts
- Only add diagnostic logging, then collect data
