# Power subsystem

Solar charging + power path for the weather station, captured as a KiCad 10 schematic.

## Chain

```
PV-12W ─▶ [U6 INA219] ─▶ CN3791 12V MPPT ─▶ 1S LiPo 10Ah ─▶ [U7 INA219] ─▶ MT3608 (5.0V) ─▶ D1 ─▶ DevKit 5V pin
            0x40           (CV term 4.2V)     (buffer + PCM)     0x41
            (harvest)                                            (load + Vbatt)
```

- **Panel:** Seeed PV-12W — Vmp 12 V, Voc 14 V, Isc ~1 A (matches the **12 V** CN3791 variant).
- **Charger:** Hailege CN3791 single-cell MPPT, terminates at 4.2 V. Panel's ~1 A is a
  gentle 0.1C into the 10 Ah cell.
- **Boost:** MT3608 module trimmed to **5.0 V**, feeding the DevKit `5V` pin (J1/14).
- **Isolation diode (D1, 1N5817):** the MT3608 5 V feeds the DevKit 5V pin through D1
  (~0.45 V drop, LDO still has headroom). When you flash over USB, VBUS appears on the 5V
  pin internally (~5 V) and wins; D1 blocks any back-feed into the boost. USB is the
  DevKit's own port, so no second diode is needed.
- **Current/power telemetry (2× INA219, I²C):** two INA219 breakouts (DEWOTHV / INA219B,
  on-board 0.1 Ω 1 % shunt, ±3.2 A) on the shared I2C0 bus, VCC on 3V3.
  - **U6 @ 0x40** — high-side on the PV feed (`SOLAR+ → SOLAR_CHG`): solar **harvest
    current** + panel voltage (bus input rated to 26 V, fine for the ~12–14 V panel).
  - **U7 @ 0x41** — high-side on the battery→boost feed (`VBAT → VLOAD`): **load current**
    drawn by the system + **battery voltage** (the INA219 bus register, more accurate than
    the SAR ADC). Addresses set by the A0 solder jumper.
- **Battery sense:** the GPIO2 / ADC1_CH1 divider is **gone** — battery voltage now comes
  from U7's bus reading. GPIO2 is freed as a spare ADC.
- **I²C pull-ups:** R3/R4 = 4.75 kΩ on SDA/SCL (now six devices on the bus).
- **Decoupling:** C1 (22 µF) at the MT3608 input on `VLOAD` (right at boost VIN, after the
  U7 shunt), C2/C3 (100 µF + 10 µF) on the 5 V rail, C4–C6 on 3V3 (10 µF bulk + 100 nF per
  sensor) — sized to ride out the BLE radio's TX current bursts.

All parts are on hand (see the project Mouser order); the two INA219 modules arrived
2026-06-21 (lot of 2). Current/power telemetry is now populated on the sheet.

## Files

| File                       | What                                      |
| -------------------------- | ----------------------------------------- |
| `meteo_power.kicad_sch`    | The schematic (KiCad 10, ERC-clean).      |
| `meteo_power.kicad_pro`    | Project file.                             |
| `meteo_power.pdf` / `.svg` | Rendered exports.                         |
| `gen_power_sch.py`         | Generator that produces the `.kicad_sch`. |

The schematic is **generated**, not hand-drawn: `gen_power_sch.py` reuses the system KiCad
symbol libraries and wires nets with on-sheet labels (no point-to-point routing). Edit the
`COMPS` / net map in the script and regenerate rather than editing the `.kicad_sch` by hand
— but the `.kicad_sch` is committed so it opens without regenerating.

## Regenerate

```bash
just power-sch        # regenerate sch + ERC + PDF/SVG
```

Needs `kicad-cli` and the KiCad symbol libraries under `/usr/share/kicad/symbols`. The
generator also takes an explicit output directory: `python3 gen_power_sch.py <outdir>`.

## Layout note

Modules (panel, CN3791, MT3608, DevKit, sensors) appear as connector symbols; only the
discretes (R/C/D) are real placed parts. This is a system-interconnect schematic, not a
board — there is no PCB. A soldered carrier board is a possible later step.
