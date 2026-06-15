# meteo_station

Weather-station firmware in embedded Rust for the STM32H753ZI (Nucleo-144),
built on the [Embassy](https://embassy.dev) async runtime (`no_std`).

## Workspace layout

- `crates/meteo-firmware` — STM32H753ZI binary: hardware init, interrupt
  bindings, Embassy tasks.
- `crates/meteo-lib` — hardware-agnostic drivers (host-testable) using
  `embedded-hal-async` traits: BMP388 barometer, RN4871 BLE link.

## Build & flash

```bash
just build     # release firmware
just flash      # flash to device
just run        # flash + attach RTT logging
just clippy     # firmware (ARM) + meteo-lib (host), -D warnings
just test       # host unit tests (cargo nextest, meteo-lib)
just format     # cargo fmt
```

See `CLAUDE.md` for the pin allocation, datasheets, and the safe `probe-rs`
testing procedure.

## BLE link soak test — `scripts/ble_soak.sh`

A self-validating acceptance harness for the RN4871 BLE link. The firmware
brings the module up as device `80:1F:12:B6:60:BF`, advertising continuously
with no GATT services. The host unit tests only prove the protocol parser; the
**soak test is the real acceptance gate** for the radio link.

The script drives, indefinitely:

```
connect → hold HOLD_SECS → disconnect → wait GAP_SECS → reconnect → …
```

polling the link every second via the BlueZ D-Bus `Connected` property. It
prints one `PASS (held 360s)` line per cycle and exits **non-zero** on any
mid-window drop or failed reconnect. A single passing cycle is not acceptance —
the link must hold and repeat over a sustained run.

### Running it

It runs **on gaia** (BlueZ 5.86), which has `bluetoothctl`, `busctl`, and `doas`.
`doas` is needed because the script writes BlueZ connection parameters to
`debugfs` (these reset on every `systemctl restart bluetooth`, so they are
reapplied each run). Deploy and run:

```bash
scp scripts/ble_soak.sh gaia:
ssh gaia ./ble_soak.sh        # Ctrl-C to stop cleanly
```

The script **never starts a scan** — it connects by address off blueman's
standing discovery cache, avoiding a `Discovering: yes` adapter wedge. Ensure
the device is powered and advertising and that blueman discovery is running
before starting.

### Configuration (environment variables)

| Variable          | Default             | Meaning                                         |
| ----------------- | ------------------- | ----------------------------------------------- |
| `DEVICE`          | `80:1F:12:B6:60:BF` | BLE address of the peripheral                   |
| `ADAPTER`         | `hci0`              | Local HCI adapter                               |
| `HOLD_SECS`       | `360`               | Seconds the link must stay up per cycle (6 min) |
| `GAP_SECS`        | `90`                | Seconds between disconnect and reconnect        |
| `CONNECT_TIMEOUT` | `30`                | Per-step deadline (connect / cache appearance)  |
| `CONN_MIN`        | `6`                 | debugfs `conn_min_interval` (×1.25 ms)          |
| `CONN_MAX`        | `12`                | debugfs `conn_max_interval` (×1.25 ms)          |
| `SUPERVISION`     | `600`               | debugfs `supervision_timeout` (×10 ms = 6 s)    |

Example — a shorter hold while iterating:

```bash
HOLD_SECS=60 GAP_SECS=15 ssh gaia ./ble_soak.sh
```

If the link drops, diagnose with `btmon` on gaia during a hold before changing
firmware; the first tuning knobs are the connection-interval / supervision
values, not another code patch.
