# meteo_station

Weather-station firmware in embedded Rust for the ESP32-H2 (ESP32-H2-DevKitM-1),
built on the [Embassy](https://embassy.dev) async runtime over esp-hal + esp-rtos
(`no_std`, `riscv32imac-unknown-none-elf`).

## Workspace layout

- `crates/meteo-firmware` — ESP32-H2 binary: esp-hal/esp-rtos init, GPIO8 status
  LED, Embassy tasks (BMP388 + MLX90614 + BME280 + VEML7700 on a shared I2C bus,
  aggregator, on-chip BLE, RWDT watchdog). esp deps are gated to
  `cfg(target_arch = "riscv32")`.
- `crates/meteo-lib` — hardware-agnostic drivers (host-testable) using
  `embedded-hal-async` traits: BMP388 barometer, MLX90614 IR thermometer, BME280
  humidity sensor, VEML7700 ambient light sensor, and the v2 BLE wire-frame
  (encode/decode + diagnostics byte).
- `crates/meteo-tui` — terminal dashboard (host, `x86_64-linux`): connects to the
  station over BLE, decodes telemetry frames, and renders a live ratatui UI
  (telemetry table + air-temp / sky-temp / pressure charts).

## Build & flash

```bash
just build     # release firmware (riscv32imac)
just flash      # flash to device (espflash, over native USB-Serial-JTAG)
just run        # flash + attach defmt monitor
just clippy     # firmware (riscv) + meteo-lib + meteo-tui (host), -D warnings
just test       # host unit tests (cargo nextest, meteo-lib + meteo-tui)
just format     # cargo fmt
```

See `CLAUDE.md` for the pin allocation, datasheets, and the espflash logging
procedure.

## Live dashboard

`just tui-run` launches the terminal dashboard. It connects to the `MeteoStation`
BLE peripheral and renders live telemetry in the terminal.

```bash
just tui-build                      # build only
just tui-run                        # connect to default address F0:CA:FE:00:00:01
just tui-run -- --address AA:BB:CC:DD:EE:FF   # override station address
```

Press `q`, Esc, or Ctrl-C to quit.

Host prerequisite: `bluetoothd` must be running at runtime; `libdbus-1-dev` is
required at build time (provides the BlueZ D-Bus binding).

## BLE acceptance — `scripts/ble_soak.sh` + `scripts/ble_notify_check.sh`

The ESP32-H2 advertises **on-chip** BLE as `MeteoStation` (static random address
`F0:CA:FE:00:00:01`) and pushes an 18-byte v2 telemetry frame at 1 Hz over a GATT
notify characteristic. The host unit tests only prove the frame codec; two scripts
(run on gaia, BlueZ 5.86) are the real acceptance gate for the radio link:

- **`ble_soak.sh`** — link-stability soak (described below).
- **`ble_notify_check.sh`** — data-flow check: subscribes via BlueZ `AcquireNotify`
  and asserts at least 5 well-formed 18-byte frames (byte[0] == `0x02`) within the
  window. It uses `AcquireNotify` rather than `bluetoothctl notify on` because BlueZ
  only re-emits the `Value` property when it _changes_, and the near-constant
  telemetry would otherwise be deduped to silence.

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
| `DEVICE`          | `F0:CA:FE:00:00:01` | BLE address of the peripheral                   |
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
