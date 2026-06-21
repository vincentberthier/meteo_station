# SparkFun Weather Meter Kit (SEN-15901)

**Manufacturer:** Shenzhen Fine Offset Electronics

## Overview

Three passive sensors using sealed magnetic reed switches -- no active electronics.

1. **Cup anemometer** (wind speed)
2. **Wind vane** (direction)
3. **Tipping bucket rain gauge**

---

## Wind Speed Sensor (Anemometer)

- **Interface:** Reed switch (contact closure), count pulses via GPIO interrupt
- **Calibration:** 1 Hz = 2.4 km/h

```
wind_speed_kmh = pulses_per_second * 2.4
wind_speed_ms  = pulses_per_second * 0.667
```

- External pull-up required (or MCU internal)
- **Debounce:** ~10-15 ms (hardware RC or software)
- Max frequency at extreme wind: ~67 Hz

---

## Wind Direction Sensor (Wind Vane)

- **Interface:** Analog (resistive), read via voltage divider + ADC
- 8 reed switches with different resistors, magnet can close 1-2 adjacent -> **16 positions** (22.5 degree steps)

### Resistance Table

| Direction | Degrees | Resistance (ohm) |
| --------- | ------- | ---------------- |
| N         | 0       | 33,000           |
| NNE       | 22.5    | 6,570            |
| NE        | 45      | 8,200            |
| ENE       | 67.5    | 891              |
| E         | 90      | 1,000            |
| ESE       | 112.5   | 688              |
| SE        | 135     | 2,200            |
| SSE       | 157.5   | 1,410            |
| S         | 180     | 3,900            |
| SSW       | 202.5   | 3,140            |
| SW        | 225     | 16,000           |
| WSW       | 247.5   | 14,120           |
| W         | 270     | 120,000          |
| WNW       | 292.5   | 42,120           |
| NW        | 315     | 64,900           |
| NNW       | 337.5   | 21,880           |

### ADC Voltage Formula

With external pull-up R_pullup to VCC:

```
V_out = VCC * R_vane / (R_vane + R_pullup)
```

Use 10k pull-up. Match ADC reading against lookup table with +/-2% tolerance.

---

## Rain Gauge

- **Interface:** Reed switch (contact closure), GPIO interrupt
- **Calibration:** 1 tip = 0.2794 mm rain

```
rainfall_mm = tip_count * 0.2794
```

- Recommended debounce: **100-200 ms** (mechanical tipping is slow)
- Reference circuit: 100k pull-up, 10 pF debounce cap

---

## Connectors (RJ-11)

**This unit ships three separate RJ-11 cables — one per sensor.** (Some SparkFun
SEN-15901 revisions combine the wind vane + anemometer onto a single shared cable;
this one does not. Verified against the physical kit, 2026-06.) Each sensor is a
2-terminal passive device on its own jack:

| Cable      | Sensor function       | Notes                           |
| ---------- | --------------------- | ------------------------------- |
| Anemometer | reed switch (2 wires) | non-polar contact closure       |
| Rain gauge | reed switch (2 wires) | non-polar contact closure       |
| Wind vane  | resistive (2 wires)   | resistance-to-common, non-polar |

**Which two of the (up to 6) conductors in each RJ-11 are the live pair varies by
cable batch — confirm with a multimeter before wiring.** Continuity mode:
spin the cups (anemometer pair beeps intermittently), tip the bucket (rain-gauge
pair beeps), rotate the vane (pair reads a changing resistance per the table
above). Ignore any open/unused conductors.

**Breakout:** use an RJ-11 6P6C breakout board (jack → labelled 0.1″ header pins
1–6) per sensor — plug in, meter the live pair, jumper to the MCU. A 6P6C jack
seats 6P4C/6P2C plugs fine; the extra pins stay dead.

---

## STM32H753ZI Integration Notes

- **Anemometer + rain gauge:** two GPIO pins as external interrupts (falling edge) with pull-ups
- **Wind vane:** one ADC channel, recompute voltage table for 3.3V supply
- All sensors passive, compatible with 3.3V or 5V -- no level shifting needed at 3.3V
