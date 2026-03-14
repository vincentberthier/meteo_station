# NUCLEO-H753ZI Technical Reference

**Sources:** DB3171 Rev 18, UM2407 Rev 5
**Board:** MB1364
**MCU:** STM32H753ZIT6 (LQFP144)

## MCU Specifications

| Feature     | Value                       |
| ----------- | --------------------------- |
| Core        | Arm Cortex-M7 with FPU      |
| Flash       | 2 MB                        |
| Special     | Hardware crypto accelerator |
| I/O voltage | 3.3V (NOT 5V tolerant)      |

## Clock Configuration

### HSE (default)

**8 MHz MCO from ST-LINK** (not a crystal). Configurable via solder bridges SB3/SB4/SB44/SB45/SB46.

### LSE

**32.768 kHz on-board crystal (X2)**.

## Power Supply

### Source Selection (JP2)

| JP2               | Source                | Voltage    | Max Current |
| ----------------- | --------------------- | ---------- | ----------- |
| [1-2] **default** | ST-LINK USB (CN1)     | 5V         | 500 mA      |
| [3-4]             | VIN (CN8-15, CN11-24) | 7-12V      | 250-800 mA  |
| [5-6]             | 5V_EXT (CN11-6)       | 4.75-5.25V | 500 mA      |
| [7-8]             | USB charger           | 5V         | --          |

If board + shields > 300 mA, use external supply.

### VDD_MCU (JP5)

| JP5               | Level |
| ----------------- | ----- |
| [1-2] **default** | 3.3V  |
| [2-3]             | 1.8V  |

## Debug (STLINK-V3E)

- USB 2.0 high-speed via CN1
- SWD + JTAG + SWO trace
- VCP via **USART3 (PD8 TX / PD9 RX)** by default
- MIPI-10 connector CN5

## LEDs

| LED | Color    | Pin  | Function       |
| --- | -------- | ---- | -------------- |
| LD1 | Green    | PB0  | User (HIGH=on) |
| LD2 | Yellow   | PE1  | User           |
| LD3 | Red      | PB14 | User           |
| LD4 | Tricolor | --   | ST-LINK status |
| LD5 | Green    | --   | Power (+5V)    |

## Push-Buttons

| Button     | Pin  | Function       |
| ---------- | ---- | -------------- |
| B1 (USER)  | PC13 | User (blue)    |
| B2 (RESET) | NRST | Hardware reset |

## Jumper Summary

| Jumper | Function        | Default      |
| ------ | --------------- | ------------ |
| JP1    | STLK_RST        | OFF          |
| JP2    | Power source    | [1-2] STLINK |
| JP3    | NRST to ST-LINK | ON           |
| JP4    | IDD measurement | ON           |
| JP5    | VDD_MCU         | [1-2] 3V3    |
| JP6    | Ethernet TXD1   | ON           |

## Connector Pinout (Key Pins)

### CN7

| Pin | Name | STM32   | Function                  |
| --- | ---- | ------- | ------------------------- |
| 2   | D15  | **PB8** | **I2C1_SCL**              |
| 4   | D14  | **PB9** | **I2C1_SDA**              |
| 10  | D13  | PA5     | SPI1_SCK                  |
| 12  | D12  | PA6     | SPI1_MISO                 |
| 14  | D11  | PB5     | SPI1_MOSI                 |
| 17  | D24  | **PA4** | **SPI_B_NSS (BLE RST_N)** |

### CN8

| Pin | Name | STM32   | Function         |
| --- | ---- | ------- | ---------------- |
| 7   | 3V3  | --      | 3.3V output      |
| 9   | 5V   | --      | 5V output        |
| 14  | D49  | **PG2** | **External LED** |
| 15  | VIN  | --      | 7-12V input      |

### CN9

| Pin | Name | STM32   | Function       |
| --- | ---- | ------- | -------------- |
| 1   | A0   | PA3     | ADC            |
| 4   | D52  | **PD6** | **USART2_RX**  |
| 6   | D53  | **PD5** | **USART2_TX**  |
| 8   | D54  | **PD4** | **USART2_RTS** |
| 10  | D55  | **PD3** | **USART2_CTS** |

### CN10

| Pin | Name | STM32   | Function          |
| --- | ---- | ------- | ----------------- |
| 14  | D1   | PB6     | LPUART1 TX        |
| 16  | D0   | PB7     | LPUART1 RX        |
| 31  | D33  | **PB0** | **LD1 green LED** |

## Project Pin Cross-Reference

| Function            | STM32 Pin | Connector |
| ------------------- | --------- | --------- |
| LED green (LD1)     | PB0       | CN10-31   |
| LED yellow (LD2)    | PE1       | onboard   |
| LED red (LD3)       | PB14      | onboard   |
| External LED        | PG2       | CN8-14    |
| I2C1_SCL (BMP388)   | PB8       | CN7-2     |
| I2C1_SDA (BMP388)   | PB9       | CN7-4     |
| USART2_TX (RN4871)  | PD5       | CN9-6     |
| USART2_RX (RN4871)  | PD6       | CN9-4     |
| USART2_RTS (RN4871) | PD4       | CN9-8     |
| USART2_CTS (RN4871) | PD3       | CN9-10    |
| BLE RST_N (RN4871)  | PA4       | CN7-17    |

## Gotchas

1. **3.3V I/O only** -- no 5V tolerance
2. HSE is 8 MHz from ST-LINK MCO, not a crystal
3. USB CN13 cannot power the board -- power first, then connect
4. Ethernet ties up PA1, PA2, PA7, PC1, PC4, PC5, PG11, PG13, PB13
5. VCP default is USART3 (PD8/PD9), not LPUART1
6. PA13/PA14 are SWD -- don't use as GPIO while debugging
7. Power sequencing with external supply: power board first, then USB
