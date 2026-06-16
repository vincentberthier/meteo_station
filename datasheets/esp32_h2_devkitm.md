# ESP32-H2-DevKitM-1

**SoC:** Espressif ESP32-H2 (`ESP32-H2-MINI-1` module, QFN32) — candidate
replacement for the STM32H753ZI + RN4871 pair. BLE is **on-chip**, so the whole
external-module path (RN4871 + USART2 + reset GPIO) disappears.

Module variants: **`-N4`** (PCB antenna) and **`-1U-N4`** (external antenna
connector). Both have 4 MB in-package flash, 0 MB PSRAM.

## Chip summary (datasheet v1.2)

| Item             | Value                                                                     |
| ---------------- | ------------------------------------------------------------------------- |
| Core             | RISC-V 32-bit single-core @ 96 MHz, 4-stage pipeline                      |
| Memory           | 320 KB SRAM, 128 KB ROM, 16 KB cache, 4 KB LP, 4 MB flash                 |
| Radio            | **Bluetooth 5.3 LE** (Coded PHY long range, 2 Mbps, adv. ext.) + 802.15.4 |
| I2C / UART / SPI | 2× I2C, 2× UART, general-purpose SPI                                      |
| ADC              | One 12-bit SAR ADC, **5 channels on GPIO1–GPIO5** (ADC1 only)             |
| USB              | Native USB-Serial-JTAG (flash + `defmt` log + debug over one USB-C)       |
| GPIOs            | 19 programmable; **strapping: GPIO8, GPIO9, GPIO25**                      |
| Deep-sleep       | 7 µA (LP memory retained)                                                 |
| Toolchain        | `riscv32imac-unknown-none-elf`, stock Rust + `espflash` (no Xtensa fork)  |

Reference docs:

- [ESP32-H2 chip datasheet (v1.2)](https://documentation.espressif.com/esp32-h2_datasheet_en.pdf)
- [ESP32-H2-MINI-1/1U module datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-h2-mini-1_mini-1u_datasheet_en.pdf)
- [DevKitM-1 user guide](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32h2/esp32-h2-devkitm-1/user_guide.html)
- [DevKitM-1 schematics v1.3](https://dl.espressif.com/dl/schematics/esp32-h2-devkitm-1_v1.3_schematics.pdf)

## Board pinout + weather-station wiring

Two 15-pin headers, `J1` and `J3`. **The big label is the SILKSCREEN printed on
TOP of the board — that's what you read at a glance.** `(n)` is the J#/pin number,
which is printed on the **underside only**. Every pin is listed; nothing is left
blank.

```
   ── J1 header ──────────────────────────────────────────
   SILK   GPIO     connect / role                      (#)
   ────   ──────   ─────────────────────────────────   ───
   3V3      —      → sensor VCC (3.3 V rail)            (1)
   RST      —      ✗ reset button — leave              (2)
   0      GPIO0    · free  (spare digital)             (3)
   1      GPIO1    → WIND VANE         [ADC1_CH0]       (4)
   2      GPIO2    → BATTERY sense     [ADC1_CH1]       (5)
   3      GPIO3    · free  (spare ADC)                  (6)
   13/N   GPIO13   · free  (or 32k XTAL if fitted)      (7)
   14/N   GPIO14   · free  (or 32k XTAL if fitted)      (8)
   4      GPIO4    · free  (spare ADC)                  (9)
   5      GPIO5    · free  (spare ADC)                 (10)
   NC       —      ✗ no connect                        (11)
   VBAT     —      ✗ LiPo supply IN — optional         (12)
   G        —      → ground                            (13)
   5V       —      ✗ 5 V rail                          (14)
   G        —      → ground                            (15)

   ── J3 header ──────────────────────────────────────────
   SILK   GPIO     connect / role                      (#)
   ────   ──────   ─────────────────────────────────   ───
   G        —      → ground                            (1)
   TX     GPIO24   · debug UART TX — optional          (2)
   RX     GPIO23   · debug UART RX — optional          (3)
   10     GPIO10   → I2C SDA                            (4)
   11     GPIO11   → I2C SCL                            (5)
   25     GPIO25   ✗ reserved — strapping, leave open  (6)
   12     GPIO12   → RAIN GAUGE                         (7)
   8      GPIO8    ◆ onboard RGB LED — no wire needed   (8)
   22     GPIO22   → ANEMOMETER                         (9)
   G        —      → ground                            (10)
   9      GPIO9    ✗ reserved — BOOT button            (11)
   G        —      → ground                            (12)
   27     GPIO27   ✗ reserved — native USB D+          (13)
   26     GPIO26   ✗ reserved — native USB D-          (14)
   G        —      → ground                            (15)

   Legend:  → wire to sensor      · free / spare (usable later)
            ✗ reserved / leave open      ◆ onboard (no external wire)
```

### Pin assignment table

| Function               | Silk / GPIO   | Header / No.             | Notes                                        |
| ---------------------- | ------------- | ------------------------ | -------------------------------------------- |
| I2C SDA                | `10` / GPIO10 | J3 / 4                   | non-ADC; shared bus for all 4 I2C sensors    |
| I2C SCL                | `11` / GPIO11 | J3 / 5                   | 4.7 kΩ pull-ups to 3V3                       |
| Wind vane (analog)     | `1` / GPIO1   | J1 / 4                   | ADC1_CH0                                     |
| Battery sense (analog) | `2` / GPIO2   | J1 / 5                   | ADC1_CH1                                     |
| Anemometer (pulse)     | `22` / GPIO22 | J3 / 9                   | input IRQ, pull-up, ~10–15 ms debounce       |
| Rain gauge (pulse)     | `12` / GPIO12 | J3 / 7                   | input IRQ, pull-up, ~100–200 ms debounce     |
| Status LED             | `8` / GPIO8   | J3 / 8 (onboard RGB)     | addressable RGB; strapping — drive post-boot |
| Sensor power           | `3V3`         | J1 / 1                   | all sensors run at 3.3 V (no level shifting) |
| Ground                 | `G`           | J1/13,15 · J3/1,10,12,15 |                                              |

**Pin budget: 6 signal pins + onboard LED used; GPIO0, GPIO3, GPIO4, GPIO5 (3
spare ADC channels) remain free** — not pin-constrained.

## I2C bus — four sensors, one bus

All addresses distinct, so they coexist on the single GPIO10/GPIO11 bus:

| Sensor   | Address       | Measures                 |
| -------- | ------------- | ------------------------ |
| VEML7700 | `0x10`        | ambient light            |
| MLX90614 | `0x5A`        | IR (object) temperature  |
| BMP388   | `0x76`/`0x77` | pressure, temperature    |
| BME280   | `0x76`/`0x77` | humidity, pressure, temp |

- BMP388 and BME280 share the `0x76`/`0x77` pair — put one at each. You cannot
  place two of the _same_ part on the bus. (BME280 is nearly a BMP388 superset;
  you may not want both.)
- **MLX90614 is SMBus**: it can power up in PWM mode — the driver must issue an
  SMBus request to switch to I2C. Drive any GY-906 breakout at 3.3 V for clean
  levels.

## Weather-meter wiring (SparkFun SEN-15901)

```
Anemometer:   GPIO22 ──┬── reed switch ── GND      (internal pull-up + SW debounce)
Rain gauge:   GPIO12 ──┬── reed switch ── GND      (internal pull-up + SW debounce)

Wind vane:    3V3 ── 10kΩ ──┬── GPIO1 (ADC1_CH0)
                            │
                       vane resistive ── GND        (recompute table for 3.3 V)
```

- Wind vane resistance table and the `V_out = VCC·R_vane/(R_vane+R_pullup)`
  lookup are in `weather_meter.md` — recompute the voltage thresholds for the
  H2's attenuated ~0–3.1 V ADC range.
- The ESP32 SAR ADC is noisier/less linear than the STM32H7's, but has eFuse
  calibration and the vane only needs 16 coarse buckets (±2 %), so it's adequate.

## Battery monitoring (LiPo)

```
LiPo+ ── R1 ──┬── GPIO2 (ADC1_CH1)
              │
              R2 ── GND
```

Size the divider so the max cell voltage (4.2 V) lands under the ADC full-scale
(~3.1 V with 12 dB attenuation). E.g. `R1 = R2 = 100 kΩ` → 2.1 V at 4.2 V.
(`VBAT` on J1/12 is a _power input_ to the chip, not a measurement pin — wire
your own divider.)

## Notes / open items

- **32 kHz crystal:** GPIO13/GPIO14 (J1/7,8) are consumed only if a 32.768 kHz
  crystal is populated, which matters for an accurate RTC (log timestamps).
  Confirm against the v1.3 schematic; if absent, they're two more free GPIOs.
- **Debug logging:** flash and stream `defmt` over the native USB-Serial-JTAG
  port (`espflash`); the J3 TX/RX (U0TXD/U0RXD) UART console is a fallback.
- **GPIO2–GPIO5** carry the JTAG (MTxx) functions but are free here because the
  H2's JTAG runs over USB — usable as ADC/GPIO.
