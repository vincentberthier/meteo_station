# CLAUDE.md

Weather station firmware - embedded Rust for a meteorological monitoring device targeting the ESP32-H2 (ESP32-H2-DevKitM-1 board). Uses the Embassy async runtime on esp-hal for bare-metal embedded development (`no_std`). The target triple is `riscv32imac-unknown-none-elf`.

Currently supports:

- BMP388 barometric pressure/temperature sensor (via I2C0, address `0x77`)
- MLX90614 IR thermometer — sky temperature (via I2C0, address `0x5A`, shares GPIO10/11)
- BME280 humidity/pressure/temperature sensor (via I2C0, address `0x76`, shares GPIO10/11)
- VEML7700 ambient light sensor — luminosity in lux (via I2C0, address `0x10`, shares GPIO10/11)
- Dual INA219 power monitors (via I2C0, shares GPIO10/11): U6 `0x40` on the PV feed
  (panel voltage + harvest current), U7 `0x41` on the battery feed (battery voltage +
  load current); battery percent derived on-device from a 1S-LiPo voltage curve
- Weather meter (SparkFun SEN-15901): anemometer (GPIO22), rain gauge (GPIO12),
  wind vane (GPIO1/ADC1)
- Status LED on GPIO8

> Ported from the STM32H753ZI (Nucleo-144). The on-chip BLE 5.3 radio replaces the
> external RN4871 module (RN4871/USART path dropped). On-chip BLE telemetry is now
> implemented: the firmware advertises as `MeteoStation` using **extended connectable
> non-scannable** advertising, embedding the 38-byte v5 telemetry frame as
> Manufacturer-Specific Data (company `0xFFFF`) at 1 Hz — any observer reads it
> without connecting. A coarse GPS location (~1 km) can be set over the connectable
> channel by writing a PIN-gated GATT characteristic; the location is persisted to
> NVS flash and broadcast in the frame. An RWDT heartbeat supervisor guards all
> critical tasks. The supervision-timeout fix (vendored trouble-host patch) is retained
> for the location-config connection. On-device acceptance via the gaia soak harness
> is a pending manual gate.

## Build Commands

```bash
# Build firmware (release)
just build

# Flash to device (espflash, over native USB-Serial-JTAG)
just flash

# Flash and attach the defmt monitor
just run

# Reset the device
just reset

# Check code with clippy (firmware + meteo-lib + meteo-tui)
just clippy

# Format code
just format

# Run tests on host (meteo-lib + meteo-tui)
just test

# Show binary size
just size

# Dashboard (host target only)
just tui-build           # build the TUI dashboard
just tui-run             # run the TUI dashboard
just tui-clippy          # clippy the TUI crate only (fast loop)
```

### Flashing & logging with espflash

The ESP32-H2 flashes and logs over its **native USB-Serial-JTAG** (`/dev/ttyACM0`),
not an external debug probe. `just run` flashes and then streams defmt over the same
port; espflash decodes the defmt framing from the freshly built ELF
(`--log-format defmt`). Stop the monitor with **Ctrl-C** — unlike probe-rs, espflash
holds no JTAG lock, so an interrupted monitor leaves the chip running the flashed
image and the port free.

```bash
just run     # build + espflash flash --monitor --log-format defmt
just flash    # build + flash without attaching the monitor
just reset    # espflash reset
```

`DEFMT_LOG` (set to `trace` in `.cargo/config.toml`) controls the compile-time log
level filter.

## Architecture

### Async Runtime Pattern

Uses Embassy's `#[embassy_executor::task]` for concurrent tasks, scheduled by the
**esp-rtos** thread-mode executor: `main` is marked `#[esp_rtos::main]`, and
`esp_rtos::start(timg0.timer0, sw_int.software_interrupt0)` brings up the scheduler
plus the embassy time driver. All peripherals are accessed asynchronously. Tasks
communicate via `embassy_sync::channel::Channel` instead of shared memory.

Per esp-rtos, `embassy-executor` must **not** enable any `arch-*` feature (esp-rtos
supplies the executor). `embassy-time`/`embassy-executor` versions follow esp-rtos's
bounds.

### Module Structure

```
crates/
├── meteo-firmware/        # Binary crate: ESP32-H2-specific (riscv32imac)
│   └── src/
│       ├── main.rs        # esp-hal/esp-rtos init, GPIO8 LED blink, task spawning
│       ├── bmp.rs         # BMP388 task; retries init in loop; bumps BMP_BEAT
│       ├── mlx.rs         # MLX90614 task; reads sky/ambient IR temps, sends on the
│       │                  #   sensor channel (not watchdog-gated; see resilience note)
│       ├── bme.rs         # BME280 task; 1 Hz humidity reads; graceful degradation,
│       │                  #   no watchdog beat
│       ├── veml.rs        # VEML7700 task; auto-ranging lux; no watchdog beat
│       ├── ina.rs         # INA219 power task (spawned per rail: PV 0x40, batt 0x41);
│       │                  #   bus V + current at 1 Hz; no watchdog beat
│       ├── anemometer.rs  # Anemometer task; GPIO22 pulse count → wind speed; no beat
│       ├── rain.rs        # Rain-gauge task; GPIO12 tip count → mm/h rate; no beat
│       ├── vane.rs        # Wind-vane task; GPIO1/ADC1 divider → heading; no beat
│       ├── bus.rs         # Shared I2C0 async-mutex bus; per-sensor I2cDevice handles
│       ├── aggregator.rs  # Aggregator task: merges BMP + MLX readings into TELEMETRY,
│       │                  #   publishes a merged frame at 1 Hz; bumps AGG_BEAT
│       ├── ble.rs         # On-chip BLE stack: extended connectable broadcast,
│       │                  #   manufacturer-data frame at 1 Hz, PIN-gated location-
│       │                  #   write GATT service (esp-radio + trouble-host)
│       ├── config.rs      # Flash-backed config task: restores persisted coarse
│       │                  #   location at boot, persists validated BLE writes
│       │                  #   via sequential-storage MapStorage on the NVS partition
│       └── watchdog.rs    # RWDT heartbeat supervisor (BMP_BEAT, AGG_BEAT, ADV_BEAT,
│                          #   BLE_BEAT); all four must stay live to feed the watchdog
├── meteo-lib/             # Library crate: hardware-agnostic
│   └── src/
│       ├── lib.rs         # Re-exports sensor drivers and utilities
│       ├── utils.rs       # Utility functions (trunc2, etc.)
│       ├── battery.rs     # 1S-LiPo voltage→SoC curve (battery_pct_from_mv); host-tested
│       ├── aggregate.rs   # SensorReading enum + Aggregator; sensor-data channel type
│       ├── ble/
│       │   ├── frame.rs   # v5 wire frame (38 B): Telemetry, encode/decode, FrameError,
│       │   │              #   sentinels, uptime_s + coarse location fields; host-tested
│       │   └── location.rs # Location (coarse ~1 km), parse_authorized_write,
│       │                  #   AUTH_WRITE_LEN / LOCATION_WIRE_LEN; host-tested
│       └── sensors/
│           ├── mod.rs     # Sensor module root
│           ├── bmp388.rs  # BMP388 pressure/temperature driver
│           ├── mlx90614.rs  # MLX90614 IR thermometer driver (SMBus over I2C)
│           ├── bme280.rs  # BME280 humidity/pressure/temp driver (float compensation)
│           ├── veml7700.rs  # VEML7700 ambient light driver (auto-ranging lux)
│           ├── ina219.rs  # INA219 current/bus-voltage driver (host-tested conversions)
│           └── weather_meter.rs # SEN-15901 conversions: wind speed/dir, rain rate
└── meteo-tui/             # Binary crate: terminal dashboard (host, x86_64-linux)
    └── src/
        └── main.rs        # ratatui TUI: passive BLE scan, decode manufacturer-data
                           #   frames (company 0xFFFF), SignalState header, location
                           #   row, diagnostics row; no GATT connection required
```

**Library vs Binary separation:**

- `meteo-lib`: Hardware-agnostic drivers using `embedded-hal-async` traits. The
  esp-hal async I2C implements those traits, so the BMP388 driver is unchanged.
- `meteo-firmware`: ESP32-H2-specific init and Embassy tasks. The esp deps are gated
  behind `[target.'cfg(target_arch = "riscv32")'.dependencies]`.

### Target Configuration

Primary: ESP32-H2 (`riscv32imac-unknown-none-elf`), stable Rust (no `build-std`).
esp-hal's build script emits the linker scripts, so `.cargo/config.toml` only sets
the target, `force-frame-pointers` (for esp-backtrace), and the espflash runner.

### Dashboard (`meteo-tui`)

`crates/meteo-tui` is a host-only (`x86_64-unknown-linux-gnu`) `std` binary crate. It
performs a **passive BLE scan** via **bluer 0.17** (the official Rust BlueZ binding),
watches for manufacturer-data property changes on the `MeteoStation` device, decodes
each 38-byte v5 frame from company `0xFFFF` via `meteo-lib::ble::frame::decode`, and
renders a live terminal dashboard with ratatui. No GATT connection is made.

- All frame fields (air temperature, pressure, humidity, sky/IR temperature,
  luminosity, wind speed + direction shown together with a compass label, rain rate,
  battery, location) plus the diagnostics row.
- Live scrolling air-temperature, sky-temperature, and pressure mini-charts.
- Header bar: wall clock, app version, signal state (No signal / Live / Stale).

**Host-only build — never build with `--workspace` on the default target.** The
`.cargo/config.toml` default target is `riscv32imac-unknown-none-elf`; `meteo-tui`
uses `std` and cannot compile for that target. All recipes scope the crate explicitly:

```bash
just tui-build           # cargo build -p meteo-tui --target x86_64-unknown-linux-gnu
just tui-run [-- ARGS]   # cargo run   -p meteo-tui --target x86_64-unknown-linux-gnu
just tui-clippy          # clippy for the dashboard only (fast iteration loop)
```

The dashboard crate is also included in `just clippy` and `just test`.

**`bluer` / passive-scan dedup rationale.** The dashboard uses
`adapter.discover_devices_with_changes()` with `duplicate_data: true` in the discovery
filter. BlueZ re-emits a `DeviceAdded` / `PropertiesChanged` event whenever a device's
`ManufacturerData` property changes. Because `uptime_s` increments every second, the
manufacturer-data payload is distinct on every advertisement frame, defeating BlueZ's
property dedup (which suppresses re-emitting unchanged values). No GATT connection,
`AcquireNotify`, or characteristic subscription is used.

**Signal state.** The `SignalState` model replaces the old connection-state machine:
**No signal** (no frame ever received, rendered red) → **Live** (last frame within
`STALE_AFTER` = 5 s, rendered green) → **Stale** (last frame older than 5 s, rendered
yellow). State is derived purely from last-frame age; it is cosmetic only and never
triggers a reconnect. The firmware-version display is removed (no DIS; no connection).

**Host prerequisites.** At build time: `libdbus-1-dev`. At runtime: a running
`bluetoothd` (present on the dev machine / gaia).

## Hardware

Component datasheets are in `datasheets/`. Each PDF has a corresponding `.md` summary -- **read the `.md` files instead of the PDFs**:

| Component                     | Summary                          | PDF                                         |
| ----------------------------- | -------------------------------- | ------------------------------------------- |
| BMP388 pressure/temp sensor   | `datasheets/bmp388.md`           | `bst-bmp388-ds001.pdf`                      |
| BME280 humidity/pressure/temp | `datasheets/bme280.md`           | `bst-bme280-ds002.pdf`                      |
| MLX90614 IR thermometer       | `datasheets/mlx90614.md`         | `MLX90614-Datasheet-Melexis.pdf`            |
| VEML7700 ambient light sensor | `datasheets/veml7700.md`         | `veml7700.pdf`                              |
| RN4870/71 BLE module          | `datasheets/rn4871.md`           | `rn4870.pdf`, `RN4870-71-...User-Guide.pdf` |
| MT3608 boost converter        | `datasheets/mt3608.md`           | `MT3608-3223743.pdf`                        |
| Weather meter kit             | `datasheets/weather_meter.md`    | `DS-15901-Weather_Meter.pdf`                |
| Nucleo-H753ZI board           | `datasheets/nucleo_h753zi.md`    | `nucleo-h753zi.pdf`, `um2407-...pdf`        |
| ESP32-S3-DevKitM              | `datasheets/esp32_s3_devkitm.md` | (online only)                               |
| LiPo battery                  | `datasheets/lipo_battery.md`     | (text file)                                 |

Pin reference: `datasheets/esp32_h2_devkitm.md` (full ESP32-H2 board pinout +
weather-station wiring). The Nucleo map lives in `datasheets/nucleo_pins.csv`.

### Pin Allocation (ESP32-H2-DevKitM-1)

Wired and used today:

| Function                                        | GPIO   | Header / silk | Peripheral | Notes                                      |
| ----------------------------------------------- | ------ | ------------- | ---------- | ------------------------------------------ |
| I2C SDA (BMP388 + MLX90614 + BME280 + VEML7700) | GPIO10 | J3/4 `10`     | I2C0       | external 4.7 kΩ pull-up to 3V3; shared bus |
| I2C SCL (BMP388 + MLX90614 + BME280 + VEML7700) | GPIO11 | J3/5 `11`     | I2C0       | external 4.7 kΩ pull-up to 3V3; shared bus |
| Status LED                                      | GPIO8  | J3/8 `8`      | GPIO       | external LED + onboard WS2812 (shared)     |

GPIO8 carries both the onboard addressable WS2812 RGB _and_ the external LED. It is
driven as a plain push-pull GPIO, so the external LED blinks; the WS2812 needs a
precise data stream and stays dark. Driving colour would require either esp-hal
pinned to 1.0 + `esp-hal-smartled` (incompatible with current esp-rtos/embassy), or
a hand-rolled RMT WS2812 path on esp-hal 1.1 — tracked as a follow-up.

The remaining weather-station wiring (anemometer/rain-gauge pulse inputs, wind-vane
and battery ADC) is documented in `datasheets/esp32_h2_devkitm.md` but not yet in
firmware.

### BLE — on-chip ESP32-H2 peripheral

The firmware brings up the on-chip BLE 5.3 radio via **esp-radio** and
**trouble-host**, advertises as `MeteoStation` (**extended connectable, non-scannable,
undirected**, static random address `F0:CA:FE:00:00:01`), and broadcasts the 38-byte
v5 telemetry frame as **Manufacturer-Specific Data** (company `0xFFFF`) refreshed at
1 Hz. Any observer reads the frame passively without connecting. The advert stays
connectable so a central can connect to set the station location via a PIN-gated GATT
write.

**GATT layout (location config service):**

| Role           | UUID                                   |
| -------------- | -------------------------------------- |
| Service        | `7e700010-b1df-42a1-bb5f-6a1028c793b0` |
| Characteristic | `7e700011-b1df-42a1-bb5f-6a1028c793b0` |
| Properties     | Read + Write                           |

Telemetry is carried in the advertising Manufacturer-Specific Data, not in a GATT
characteristic. The GATT service exposes only the location config write. The location
characteristic write payload is 10 bytes (`AUTH_WRITE_LEN = 10`): bytes 0–3 are the
PIN (u32 LE, compile-time `CONFIG_PIN = 911`), bytes 4–9 are the coarse location wire
form (lat i16 LE, lon i16 LE, alt i16 LE — each × the appropriate scale factor; see
`meteo-lib::ble::location`). Wrong PIN → ATT `INSUFFICIENT_AUTHORISATION`; bad
length or out-of-range location → ATT `OUT_OF_RANGE`.

**Security caveat:** the PIN and coordinates travel in cleartext over BLE during the
one-time config connection (no SMP pairing or encryption). The PIN is `911` in
firmware, scripts, and docs. This is an application-level gate that blocks accidental
writes, not a cryptographic one. Real SMP passkey pairing is unverified on the
esp-radio H2 controller; for a low-value device this tradeoff is deliberate.

**Coarse by construction (~1 km privacy):** coordinates are stored and broadcast as
`i16` deg×100 — 0.01° resolution (~1.1 km). The station never holds or transmits a
finer fix.

The v5 telemetry frame is 38 bytes. Byte[0] is the version sentinel (`0x05`,
`FRAME_VERSION 5`). Bytes 0–27 are byte-for-byte identical to v4: byte[0] version,
bytes 1–2 temperature i16 LE (centi-°C, sentinel `i16::MIN`), bytes 3–4 pressure u16
LE (deci-hPa, sentinel `u16::MAX`), bytes 5–6 humidity u16 LE (centi-%), bytes 7–8
sky temp i16 LE (centi-°C), bytes 9–11 luminosity (mantissa u16 LE + exponent u8),
bytes 12–13 wind speed u16 LE (cm/s), bytes 14–15 wind direction u16 LE (deci-deg),
byte[16] battery percent u8 (sentinel `0xFF`), bytes 17–18 rain rate u16 LE
(deci-mm/h), byte[19] diagnostics bitfield, bytes 20–27 four power fields
(`solar_mv`, `solar_ma`, `batt_mv`, `load_ma` — each u16 LE, sentinel `u16::MAX`).
v5 appends: bytes 28–31 `uptime_s` u32 LE (monotonic seconds since boot, always
present; defeats BlueZ ManufacturerData dedup so each second's broadcast is counted
as distinct), bytes 32–33 `latitude` i16 LE (deg×100, sentinel `i16::MIN` = unset),
bytes 34–35 `longitude` i16 LE (deg×100, `i16::MIN` = unset), bytes 36–37 `altitude`
i16 LE (metres, `i16::MIN` = unset). Byte[16] `battery_pct` is derived on-device
from `batt_mv` via the 1S-LiPo curve. The diagnostics bitfield:

- bit 0 = sky-IR occlusion (MLX90614 sky temp too close to ambient)
- bit 1 = BMP388 fault
- bit 2 = BME280 fault
- bit 3 = VEML7700 fault
- bit 4 = baro divergence (BMP388 vs BME280 temp/pressure disagree beyond threshold)
- bit 5 = MLX90614 fault
- bit 6 = INA219 PV fault (panel-side monitor)
- bit 7 = INA219 battery fault (battery-side monitor)

Frame encoding and decoding live in `meteo-lib::ble::frame` (`Telemetry`,
`encode`, `decode`, `FrameError`). The firmware calls only `encode`; `decode` is
used by `meteo-tui` to interpret the broadcaster's manufacturer-data payload.

**Implementation notes:**

- The GATT server is built manually via trouble-host's `AttributeTable` /
  `AttributeServer` / `Characteristic` primitives. The `derive` macros
  (`trouble-host-macros 0.4`) are absent from the crates.io registry used by this
  workspace, so the table is constructed by hand. The table hosts only the
  location-config service (one Read + Write characteristic); the telemetry notify
  characteristic is gone — telemetry is broadcast via advertising.
- `esp_sync::RawMutex` is used for the attribute table mutex. trouble-host 0.6
  depends on `embassy-sync 0.7`; the workspace targets `embassy-sync 0.8`.
  `esp_sync::RawMutex` implements `RawMutex` for both versions, bridging the two.
- An RWDT heartbeat supervisor (`crates/meteo-firmware/src/watchdog.rs`) watches
  four beats: `BMP_BEAT` (BMP388 sampler alive), `AGG_BEAT` (aggregator alive),
  `ADV_BEAT` (advertise loop alive), and `BLE_BEAT` (broadcast frame sent or
  connection heartbeat). All four must stay live to keep feeding the watchdog; a
  stalled task fires the RWDT and resets
  the chip. These are **task-liveness** signals — a sensor that fails to read
  degrades to `None` data without stalling the task, so an absent/failed sensor
  does not cause an RWDT reboot loop.
- **Shared I2C0 bus.** BMP388 (0x77), MLX90614 (0x5A), BME280 (0x76), and VEML7700
  (0x10) all share GPIO10/11 via a `embassy_sync::Mutex`-wrapped `I2cBus`; each
  sensor task holds a per-sensor `I2cDevice` handle from `bus.rs`. The bus is never
  accessed from two tasks simultaneously.
- **Aggregator task (`aggregator.rs`).** Each sensor task sends a `SensorReading`
  variant (defined in `meteo-lib::sensors::aggregate`) over a channel. The
  aggregator task drains the channel, accumulates readings in an `Aggregator`
  struct, and signals the `TELEMETRY` `Signal` at 1 Hz. `ble.rs` waits on
  `TELEMETRY` to encode the `Telemetry` frame and update the manufacturer-data
  advertisement payload via `update_adv_data_ext`.
- **Sensor-task resilience.** Each sensor task retries initialisation in its loop
  on failure (e.g. I2C timeout) and bumps its heartbeat on every iteration
  regardless of read success. A failing sensor produces `None` fields in the frame
  rather than stalling the task, so the watchdog stays fed and BLE telemetry
  continues to flow.
- **Flash-backed location (`config.rs`).** A dedicated task owns all flash I/O; the
  BLE task never touches flash directly. At boot the task reads any persisted coarse
  location from the NVS partition and seeds the aggregator via `SENSOR_CHANNEL`. On a
  validated BLE write the BLE task signals `LOCATION_WRITE`; the config task persists
  the 6-byte blob (lat/lon/alt i16 LE) via **`sequential-storage` 7.x `MapStorage`**
  (`MapStorage::<u8,_,_>::new(BlockingAsync::new(flash), MapConfig::new(range),
NoCache::new())` with `storage.fetch_item` / `storage.store_item`) and republishes
  via `SENSOR_CHANNEL`. The NVS partition range is found at boot via
  **`esp-bootloader-esp-idf`**'s `read_partition_table` →
  `find_partition(PartitionType::Data(DataPartitionSubType::Nvs))`. The flash write
  runs in a critical section (cache off < 10 ms), absorbed by the negotiated 8 s
  supervision timeout. These dependencies raised the workspace `rust-version` to
  `1.96`.
- **MLX90614 wiring (bare TO-39 chip).** The MLX needs no special firmware
  sequencing — it powers up in SMBus and shares I2C0 like any other device. The one
  trap is the bare can's pinout: the datasheet pin table is a **bottom view**
  (pin 1 SCL → GPIO11, pin 2 SDA → GPIO10, pin 3 VDD → 3V3, pin 4 VSS → GND).
  Reading it top-down mirrors the can, which shorts SCL to ground and drags the whole
  bus — observed as `BMP388 I2c(Timeout)` + an RWDT reboot loop the moment the MLX is
  connected, because _no_ device can clock a grounded SCL. Confirmed by reading the
  lines as inputs under internal pulls (MCU-as-voltmeter): correct wiring → both lines
  float high-Z, MLX ACKs at `0x5A`, object temp reads with valid PEC. There is no PWM
  problem; an earlier "PWM-jam / SCL-low-at-POR" theory was wrong.
- **Supervision-timeout fix (vendored trouble-host patch).** BlueZ connects with a
  ~420 ms supervision timeout, so the link dropped (`Connection Timeout`, HCI 0x08)
  on any brief central-radio stall. On connect the firmware requests a robust 8 s
  supervision timeout + 80 ms interval via `update_connection_params`, but the H2
  controller accepts the HCI `LE Connection Update` (Command Status = success)
  without ever running the LL connection-parameters-request procedure on air — so
  the parameters never changed. The workspace vendors trouble-host 0.6.0 at
  `third_party/trouble-host` (copied from the crates.io source) with a one-line change in
  `update_connection_params` that restricts the HCI path to the central role; a
  peripheral now uses the **L2CAP Connection Parameter Update** signaling, which the
  controller forwards correctly. Wired via `[patch.crates-io]` in the workspace
  `Cargo.toml`. Verified on-device: the `ConnectionParamsUpdated` log fires with
  `supervision_ms=8000` and a 6-min hold held with zero drops (was 2–203 s before).
- The esp-rs stack (esp-hal 1.1, esp-rtos 0.3, esp-radio `1.0.0-beta.0`) is pulled
  from crates.io. An earlier local esp-hal fork that carried an LP-clock change was
  dropped: the clock was disproven as the drop cause (the C6 ships the identical
  no-op `ble_rtc_clk_init` and `sleep_en: 0` means the LP clock does not gate
  connection-event timing).
- **TODO — drop the vendored patch and upstream the fix (blocked on bt-hci).**
  We are pinned to **trouble-host 0.6** because esp-radio `1.0.0-beta.0` (the latest,
  including esp-hal `main`) pins **bt-hci 0.8**, while **trouble-host 0.7 requires
  bt-hci 0.9** — the two `Controller` trait versions are incompatible and nothing
  bridges them (esp-radio implements only the 0.8 trait). When esp-radio adopts
  bt-hci 0.9, bump to trouble-host 0.7 and **delete the vendored copy +
  `[patch.crates-io]`**. At that point also upstream the work: (a) an esp-rs/esp-hal
  issue for the controller bug (HCI `LE Connection Update` accepted with Command
  Status = success but no `LE Connection Update Complete` ever emitted as a
  peripheral), and (b) an embassy-rs/trouble PR — _not_ our blanket force-L2CAP
  (it would regress controllers where the LL procedure works), but an additive
  opt-in (e.g. a `ForceL2cap` method/param), since the completion is delivered to
  the connection's public event stream and can't be cleanly awaited inside
  `update_connection_params`. Forks for both are at `../esp-hal` and `../trouble`.

**Wire frame (`meteo-lib::ble::frame`):**

Host-tested. The `encode`/`decode` round-trip and all field sentinels are verified
on the development machine via `cargo nextest`. The firmware calls only `encode`;
`decode` is used by `meteo-tui` to interpret the broadcaster's manufacturer-data
payload.

**Acceptance gate:**

On-device acceptance requires `gaia` (BlueZ 5.86). Three scripts cover the three
halves:

```bash
# Link-stability soak (connect → hold 6 min → disconnect → gap → reconnect …)
scp scripts/ble_soak.sh gaia:
ssh gaia ./ble_soak.sh        # Ctrl-C to stop

# Broadcast data-flow check (passive scan, ≥5 well-formed 38-byte v5 frames in 15 s)
scp scripts/ble_broadcast_check.sh gaia:
ssh gaia ./ble_broadcast_check.sh

# Set station location (one-time config write, PIN-authenticated)
scp scripts/ble_set_location.sh gaia:
ssh gaia ./ble_set_location.sh 48.8566 2.3522 35
```

`ble_soak.sh` prints one `PASS (held 360s)` line per cycle and exits non-zero on
any mid-window drop or failed reconnect. A single passing cycle is **not**
acceptance — the link must hold and repeat over a sustained run. The soak exercises
the **location-config GATT channel** (service `7e700010-…`, characteristic
`7e700011-…`) and the retained 8 s supervision-timeout negotiation; broadcast
telemetry continuity is checked separately by `ble_broadcast_check.sh`.

`ble_broadcast_check.sh` runs a bounded, self-terminating `bluetoothctl --timeout`
passive scan, then polls the `ManufacturerData` D-Bus property via python-dbus for
`WINDOW_SECS` (default 15 s). It counts **distinct** frames (detected by payload
change — driven by the per-second `uptime_s` increment) and asserts at least
`MIN_FRAMES` (default 5) 38-byte frames with byte[0] == `0x05`. Because `uptime_s`
changes every second, near-constant sensor readings no longer defeat BlueZ's
ManufacturerData dedup. Needs `python3` + `python-dbus` on gaia. Both env knobs
(`WINDOW_SECS`, `MIN_FRAMES`) are overridable.

`ble_set_location.sh LAT LON [ALT_M]` computes the 10-byte PIN+coords payload,
connects to the config GATT channel, and writes it to the location characteristic
(`7e700011-…`). `PIN` env defaults to `911`; a wrong PIN → ATT error → non-zero
exit.

**Methodology traps (learned the hard way — do not bypass):**

- **Never** run a blocking scan (`timeout … btmgmt find`, or `bluetoothctl scan on`
  **without** `--timeout`) — it wedges the adapter in `Discovering: yes`. Use
  `bluetoothctl --timeout N scan on` (bounded, self-terminating) when a discovery is
  needed. `ble_soak.sh` connects by address off blueman's standing cache without
  scanning at all.
- Query link state via `busctl` (`org.bluez.Device1.Connected`) or the BlueZ
  cache, **never** a second scan.

**Reconnecting requires a fresh scan first (central-side, not a device fault).**
The device advertises continuously and re-advertises immediately after a
disconnect (verified: connect → disconnect → link-down gap → reconnect all hold).
But BlueZ evicts the non-bonded LE device object once discovery stops, so a _cold_
`bluetoothctl connect F0:CA:FE:00:00:01` reports `Device … not available` until a
bounded discovery repopulates the cache. Always do one bounded, self-terminating
`bluetoothctl --timeout N scan on` immediately before each (re)connect — the same
scan-then-connect-by-address pattern `ble_set_location.sh` uses. The 8 s supervision
negotiation runs in the firmware accept loop on every connection, so reconnections
get the same robust link as the first.

If the soak drops, diagnose with `btmon` on gaia during a hold (who drops the link
and why) before changing code; the first tuning knobs are the conn-interval /
supervision-timeout values, not another patch.

## Debugging & Accountability Principles

These are permanent rules for any debugging or troubleshooting on this project:

- **Do not blame the hosts.** They work perfectly fine. If — and only if — you wedge
  something, you may _request_ a sub-system reboot. Never assert a host is at fault.
- **Do not blame the hardware** except as a last resort, after you have exhausted _all_
  other explanations.
- **Fix the system, not the symptom.** A stack of patches is not wanted. When something
  doesn't work, fix the system in its entirety — not whichever little thing happened to
  trip the diagnostics.
- **Reading code is a short part of problem-solving.** If the cause isn't quickly obvious
  from the code, stop reading and reach for web searches, better/more diagnostics, and
  throwaway test scripts instead.

## Code Standards

Use `defmt` macros for logging (`defmt::info!`, `defmt::error!`, etc.) - outputs via RTT over JTAG.

### Tests

Functions should be pure as much as possible to make testing easy. Hardware-interfacing code cannot be automatically tested, but logic should be. Tests run on the development machine (not the target hardware), so can use `std`.

Tests modules should have this structure:

```rust
// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::super::Error;
    use super::*;
    type TestResult = result::Result<(), Box<dyn error::Error>>;

    #[test]
    fn test_name() -> TestResult {
        // Given


        // When


        // Then


        Ok(())
    }
}
// grcov exclude stop
```

Tests are run with `cargo nextest`.
