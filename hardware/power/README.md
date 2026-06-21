# Power subsystem

Solar charging + power path for the weather station, captured as a KiCad 10 schematic.

## Chain

```
PV-12W panel ─▶ CN3791 12V MPPT ─▶ 1S LiPo 10Ah ─▶ MT3608 (5.0V) ─▶ DevKit 5V pin
              (CV term 4.2V)        (buffer + PCM)   │
                                                     └▶ R1/R2 divider ─▶ GPIO2 (ADC)
```

- **Panel:** Seeed PV-12W — Vmp 12 V, Voc 14 V, Isc ~1 A (matches the **12 V** CN3791 variant).
- **Charger:** Hailege CN3791 single-cell MPPT, terminates at 4.2 V. Panel's ~1 A is a
  gentle 0.1C into the 10 Ah cell.
- **Boost:** MT3608 module trimmed to **5.0 V**, feeding the DevKit `5V` pin (J1/14).
- **Isolation diode (D1, 1N5817):** the MT3608 5 V feeds the DevKit 5V pin through D1
  (~0.45 V drop, LDO still has headroom). When you flash over USB, VBUS appears on the 5V
  pin internally (~5 V) and wins; D1 blocks any back-feed into the boost. USB is the
  DevKit's own port, so no second diode is needed.
- **Battery sense:** R1 = R2 = 100 kΩ divide VBAT to ~2.1 V (at 4.2 V) into GPIO2 / ADC1_CH1.
- **I²C pull-ups:** R3/R4 = 4.75 kΩ on SDA/SCL.
- **Decoupling:** C1 (22 µF) at VBAT, C2/C3 (100 µF + 10 µF) on the 5 V rail, C4–C6 on 3V3
  (10 µF bulk + 100 nF per sensor) — sized to ride out the BLE radio's TX current bursts.

All parts are on hand (see the project Mouser order); nothing extra is required for v1.
Future current/power telemetry (INA219 + the owned 0.1 Ω shunt) is noted on the sheet but
not populated.

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
