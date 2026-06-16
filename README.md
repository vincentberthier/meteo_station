# meteo_station

Weather-station firmware in embedded Rust for the ESP32-H2 (ESP32-H2-DevKitM-1),
built on the [Embassy](https://embassy.dev) async runtime over esp-hal + esp-rtos
(`no_std`, `riscv32imac-unknown-none-elf`).

## Workspace layout

- `crates/meteo-firmware` — ESP32-H2 binary: esp-hal/esp-rtos init, GPIO8 status
  LED, Embassy tasks. esp deps are gated to `cfg(target_arch = "riscv32")`.
- `crates/meteo-lib` — hardware-agnostic drivers (host-testable) using
  `embedded-hal-async` traits: BMP388 barometer, plus an RN4871 BLE parser kept
  for its host tests (not flashed; the H2 uses on-chip BLE, brought up later).

## Build & flash

```bash
just build     # release firmware (riscv32imac)
just flash      # flash to device (espflash, over native USB-Serial-JTAG)
just run        # flash + attach defmt monitor
just clippy     # firmware (riscv) + meteo-lib (host), -D warnings
just test       # host unit tests (cargo nextest, meteo-lib)
just format     # cargo fmt
```

See `CLAUDE.md` for the pin allocation, datasheets, and the espflash logging
procedure.

## BLE link soak test — `scripts/ble_soak.sh`

> **Historical (STM32 + RN4871).** The ESP32-H2 port dropped the external RN4871
> module; native on-chip BLE is a later task. This harness and the `meteo-lib`
> RN4871 parser are retained for that work and for the methodology below.

A self-validating acceptance harness for the RN4871 BLE link. The firmware
brought the module up as device `80:1F:12:B6:60:BF`, advertising continuously
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
