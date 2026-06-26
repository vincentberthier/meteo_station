# Roadmap — future ideas

The firmware and the bench prototype are at a good **v1**. This file collects ideas
for a future **v2** (mostly mechanical / hardware enclosure work). Nothing here is
committed or scheduled — these are notes to revisit, not a backlog.

**Target deployment: outdoors at ~1500 m in the Alps.** Hard freezing, snow, rime ice,
and large day/night temperature swings are design drivers throughout — see
[Cold-climate operation](#cold-climate-operation-alps-1500-m).

## Enclosure & mechanical

### Proper box

Move off the bench into a real weatherproof enclosure. This is the umbrella task that
several items below feed into (sensor placement, waterproofing, PCB mounting).

### MLX90614 sky-IR protection

The MLX90614 must point straight up at the sky to detect clouds, so its FOV cannot be
shaded or covered in normal operation. But an upward-facing can collects rain and snow,
which then sits in front of the optics and ruins readings.

Options under consideration:

- **Flap** — simple, but a flat flap doesn't drain; water/snow pools on it.
- **Servo** — park the sensor (tilt/rotate away, or under a small shelter) when
  precipitation is detected, point back at zenith otherwise. Better drainage behaviour,
  but more mechanical design work to integrate cleanly into the box. Could trigger off
  the rain-gauge tip events that the firmware already counts.

No decision yet; the servo path is the leading idea but the box geometry has to support it.

### VEML7700 luminosity placement

The ambient-light sensor **cannot** sit inside the box or in shade — it needs an open
view of the sky to read luminosity. Mounting it on top exposes it to weather, so it
needs its own small waterproof (étanche) window/dome. Placement and sealing are an open
design question and interact with the box and the MLX mount.

### Air-temperature radiation shield

The air-temp sensors (BMP388 / BME280) must read true ambient air, not a sun-baked box.
Direct sun on the enclosure will bias the temperature high by several °C and also throws
off the MLX90614 ambient reference. A louvered radiation shield (Stevenson-screen style)
with airflow around the sensor is the standard fix, and it interacts with where the box,
the VEML window, and the MLX mount all sit. Not optional for credible temperature data.

## Electronics

### Stronger BLE radio / antenna

The on-chip ESP32-H2 radio with the DevKitM PCB antenna is weak in practice: **~-80 dBm
at ~5 m** to a phone. That's marginal for a node that may sit at the bottom of the garden
behind walls. Look at a module/board with a proper external antenna (u.FL + whip, or a
PCB-trace antenna with better gain), or a board variant that breaks out the RF for an
external antenna. Goal: solid link at the real deployment distance, not bench distance.

**Check config first — we're at 0 dBm.** Before buying anything: the firmware currently
advertises at **TX power 0 dBm** (it uses `AdvertisementParameters::default()` everywhere,
whose default `tx_power` is `TxPower::ZerodBm`; see `crates/meteo-firmware/src/ble.rs` and
`third_party/trouble-host/src/advertise.rs`). The `TxPower` enum goes up to `Plus20dBm`,
and the ESP32-H2 can transmit well above 0 dBm — so there's ~10–20 dB of link budget
available for free. Raise `tx_power` (mind ETSI EN 300 328: ~+10 dBm is safe without
adaptivity) and re-measure RSSI before concluding the radio/antenna is the bottleneck.
Coded-PHY (long-range) is another no-hardware lever, but central-side support varies.

**Hard constraint: stay in the same family.** If, after maxing TX power, range is still
short, any replacement must be an Espressif / esp-rs part (e.g. an ESP32-C6/C3/S3/H2
variant with a better antenna), so the existing firmware stack (esp-hal / esp-rtos /
esp-radio / trouble-host) ports over with minimal changes. No going back to a separate
external BLE module (the old RN4871 over UART was far too poor and would mean
reprogramming everything) and no other-vendor radio that forces a rewrite.

### Federated multi-board architecture (power + main + tracker + heaters)

Split into separate boards, each with its own power domain, in a hub-and-spoke layout —
the power board is the trunk feeding the other three; the logic boards talk over a
separate comms link:

1. **Power / PMU (trunk).** PV input, MPPT charger (CN3791), battery, cold-charge inhibit
   - over-discharge protection, and all regulated rails (3.3 V logic, 5–6 V servo,
     switched heater rail). Distributes power to the other three. The INA219 current
     monitors live here — with three consumers, prefer **per-domain shunts** (main /
     tracker / heaters) over one lumped reading, so the winter energy balance is legible.
2. **Main.** The current weather station (sensors + BLE), unchanged.
3. **Solar tracker.** 2 servos + 4 LDRs; runs the sun-seek loop locally.
4. **Heaters.** Heating elements; closes a thermostat loop on a local NTC.

**Why split:**

- **Power-domain & EMC isolation** (the real win). Heater amps and inductive servo
  switching stay off the sensitive sensor/analog/RF board — they'd otherwise inject noise
  into the INA219 sensing, the vane/LDR ADC, and the BLE radio.
- **Fault isolation.** A hung tracker or a shorted heater MOSFET doesn't stop core
  telemetry from broadcasting.
- **Independent build/test** per board. Resolves the earlier "separate MCU for the
  tracker?" question — yes, and the same logic gives the heaters their own controller.

**Battery safety stays in hardware.** The power board's cold-charge inhibit and
over-discharge cutoff must be hardware-latched / autonomous, **not** dependent on a
firmware MCU that could hang — a single software fault must not be able to cook or
over-discharge the pack. (The cold-charge and over-discharge items above live on this
board.)

**Keep the satellites autonomous — the main board sends only high-level _mode_, never
real-time control:**

- **→ Heaters:** a single "cold AND precip → enable" latch — the main board already has
  both inputs (BME280 air temp + rain-gauge tips), so it computes the gate itself and
  raises one SR-latch flag; the heater board just acts on it (see below).
- **→ Tracker:** a small command set — `TRACK / LOCK / RESET` (park morning-ready) /
  `STOW` (safety). The tracker runs its own LDR seek loop between commands.

**Signalling = latched hardware flags, not a live bus poll.** Because the satellites
mostly sleep, carry each command as a **hardware SR latch** (set-reset flip-flop) on the
always-on µA rail: the main board **pulses SET**, the latch holds the state with both MCUs
asleep, and the satellite **reads it on wake then pulses RESET** (clear) once it has acted
— a hardware semaphore that "remembers" until cleared. A single `74LVC1G74`-class part per
flag, or an octal latch (`74xx573`) to hold several command bits in one chip. Firmware
side, let the satellite **deep-sleep and wake on the latch line's edge** (ESP32
`ext0/ext1` GPIO wake) rather than polling on a timer — near-zero idle draw. If a flag
must survive a full power-down (not just sleep), use a **latching/bistable relay** or an
FRAM/EEPROM bit instead.

**Heater board needs no MCU — `enable-latch → MOSFET → PTC element`.** Preferred design:
**self-regulating PTC heating elements**. A PTC pulls lots of power when cold, then its
resistance climbs steeply near its Curie temp and it tapers off as it (and its
surroundings) warm — so it regulates itself with no control loop, and a **better-protected
sensor sits warmer → the same PTC self-throttles and draws less**, matching power to local
demand automatically. It's also **intrinsically fail-safe** (can't thermally run away — no
fire path in an unattended box). Caveats: it regulates a _surface_ temperature, not a
precise sensor setpoint (coarse, but fine for frost/snow), and a cold PTC has an **inrush
spike** at switch-on (low resistance until it self-heats) — size the MOSFET + power stage
for it (the per-domain INA219 will show it).

The thermal regulation is therefore the PTC's job; _when to power it at all_ is the main
board's "cold AND precip" enable latch (above). So a separate cold-switch isn't needed for
control — but a **bimetallic snap thermostat** (e.g. KSD9700, normally-closed sub-zero
variant, snap hysteresis) in series is worth keeping as an **independent hardware
backstop**: a physical guarantee the element can't heat above freezing even if the enable
flag sticks on, so a firmware bug can't trickle-drain the battery in summer.

**Fail-safe defaults** (each satellite, on link loss or its own watchdog firing):

- **Tracker → STOW** (never hunt blind); **Heaters → OFF** (no fire, no battery drain).
- Each board carries its own watchdog so a hang can't leave servos energised or a heater
  stuck on.

**Panel safety positions:**

- **Night:** the tracker self-detects darkness (all four LDRs low) and parks east / locked
  — don't hunt on moonlight or a streetlight; the main board's `LOCK` is a backstop.
- **Hail / storm:** stow to a protective angle (edge-on, minimal exposed area). The open
  question is the _trigger_ — hail is hard to sense directly; tie `STOW` to high wind (the
  anemometer) and/or storm conditions, or add a piezo impact sensor for true detection.
  High wind should stow regardless.

**Inter-board link.** Low-rate, but in a heater/servo-noisy outdoor system pick a robust
bus: **CAN** (the H2 has a TWAI controller — differential, multi-drop, noise-immune) is
the industrial-correct choice; isolated GPIO/UART is the simpler path. Consider digital
isolators / optocouplers so satellite noise doesn't flow back into the sensor/RF board.

**Rails & conversion — distribute one bus, regulate at each board.** Don't ship multiple
regulated rails across cables: distribute a **single bus** (the battery, or one
intermediate rail) + the comms/latch signals, and let each board do **point-of-load
regulation** for its own 3.3 V (3.3 V is too drop/noise-sensitive to run between boards),
with the servo 5–6 V generated on the tracker board. Connectors then carry only the bus +
signals, keeping their current rating simple.

**Decision: go to a ~12 V pack and buck everything down** (was an A/B choice; settled
because there is no existing heater to preserve and the battery is being replaced anyway,
so nothing ties the design to 1S). The current 1S (~3.7 V) pack makes 12 V a big _step-up_
boost — a 12 V × 2 A heater ≈ 24 W would pull **7–13 A from a single cell**, beyond many
cells' continuous rating and into a sag→more-current spiral. Raising the battery flips
every rail to a **step-DOWN buck** (efficient, simple, cool), cuts distribution current to
~⅓ (thin wires, small connectors), and sits closer to the panel voltage so the charge path
converts less.

- **Pack:** **4S LiFePO4 (~12.8 V nominal)** with an integrated **BMS** — the right
  chemistry for an unattended cold box (safe, no thermal-runaway fire path, long cycle
  life, better cold _discharge_), and the BMS gives over-discharge protection + cell
  balancing for free (see [over-discharge protection](#battery-over-discharge-protection)).
- **Charger:** a 12 V/4S MPPT solar charge controller replaces the CN3791; the panel Voc
  must clear the ~14.6 V charge voltage with margin (a ~12 V-class panel is fine).
- **Rails:** point-of-load bucks per board — 12→3.3 V (logic), 12→5–6 V (servos), heater
  rail direct off the pack or its own buck.
- **Still required:** the cold-charge inhibit (no lithium charges sub-zero, LiFePO4
  included) stays.
- **Firmware:** drop the 1S-LiPo SoC curve. LiFePO4's discharge curve is very flat, so
  voltage-based SoC is poor — **coulomb-count via the battery-domain INA219** instead
  (the per-domain sensing pays off twice).

**Connectors / board-to-board ports.** Segment by role; don't use one type for everything,
and split power from signal on each run (EMC + independent current rating). High-current
trunk (PV / battery / charge): **XT30/XT60** or **Anderson Powerpole**. Per-board power
feed: a keyed, positive-latch 2-pin (**Molex Micro-Fit 3.0** if a crimp tool is on hand).
Per-board signal (bus + latch lines): a small keyed, latching multi-pin (Micro-Fit / JST-GH
/ small terminal block). **Recommended default given a minimal bench:** pluggable
screw/spring **terminal blocks** (Phoenix COMBICON-style) — keyed, locking, **no crimp
tool**, field-rewireable with a screwdriver; use a **different pin count per port** so a
cable can't be cross-plugged. Non-negotiables: **keying/polarization** (a reversed power
feed fries a board; don't trust wire colour), **positive latch** (wind + thermal cycling
back out friction connectors), **current-rate per port** (heater port worst case),
strain-relief + label every cable. Avoid bare unlatched JST for a winter deployment, fixed
screw terminals where you want clean disconnects, and Dupont/breadboard jumpers entirely.

### Proper PCB

Replace the breadboard/jumper wiring with a designed PCB. Cleaner, more robust, and a
prerequisite for a real deployable unit.

**Use the certified module, not the bare chip.** Lay down an `ESP32-H2-MINI-1` (or
`-C6-MINI-1`) castellated module — chip + flash + crystal + RF frontend + antenna + CE/FCC
cert in one SMD part. The bare QFN means designing the RF match, crystal, antenna tuning,
and re-certifying — not worth it. Drop the devkit extras (USB-JTAG bridge, AMS1117,
buttons, headers); add back only a USB connector + boot/reset strapping (flashing is over
native USB-Serial-JTAG) and an on-board 3.3 V regulator — ideally the buck-boost from the
power phase-2 note, killing the double conversion.

**What forces component changes (design the board around these):**

- **BLE/MCU variant — decide first; it is the board's centre.** A different ESP variant
  also means re-validating the BLE stack (the vendored supervision-timeout patch is
  H2-controller-specific; a C6 may behave differently, possibly not needing it).
- **Servos** (MLX rain-park + solar tracker) — PWM pins + power-switching MOSFETs + a
  **5–6 V servo rail** (not the 3.3 V logic rail).
- **Tracker's 4 photoresistors** — 4 ADC inputs; watch the pin/ADC budget (the wind vane
  already uses one ADC channel).
- **Heaters** — MOSFET drivers + a heater rail straight off battery/12 V + NTCs for
  thermostatic control. Size the power stage for this worst-case current.
- **Battery cold-charge / over-discharge protection** — NTC + charge-inhibit, a protection
  IC / protected cell, or a charger with a temp pin.
- **RTC + SD** (on-device logging) — RTC chip + backup coin cell + SD socket; optional if
  the Pi collector owns the history.

**No component change:** all current sensors (BMP/BME/MLX/VEML/INA/weather-meter), the web
server and Android app (off-board), deep-sleep (firmware), and the box / radiation shield /
VEML mounting (mechanical). Battery and solar don't change _type_ — but heaters + the
winter worst case could force a bigger panel/battery _size_ (a measurement call, see
[Power & energy](#power--energy)).

**Future-proof the layout.** A respin costs weeks; break out **unpopulated (DNP) footprints
and headers** for the deferred loads — servo header + MOSFET pads, a spare-ADC breakout for
the LDRs, heater-driver footprints, an NTC pad, I²C pads for an RTC, SD pads — and spec the
power stage for the heater worst case even if unpopulated. Then each v2 feature is a
soldering job, not a new board.

### 2-axis solar tracker

Add a 2-axis tracking mount for the PV panel, using **4 photoresistors** that orient the
panel by balancing the shade/light across the four quadrants (classic LDR-bridge tracker).

- Runs on its own board with its own MCU — see
  [Federated multi-board architecture](#federated-multi-board-architecture-power--main--tracker--heaters).
  The standalone controller keeps the servo PWM and the sun-seek loop (and night/stow
  safety) off the weather firmware, and the main board commands it only by mode
  (`TRACK / LOCK / RESET / STOW`).
- **Hybrid duty cycle.** Active by day — timer-paced ~1 min wakes to re-aim — but
  **deep-sleep at night / when stowed**, waking only on a latch-line edge (`RESET`/`STOW`
  from main) for near-zero idle draw. Entering night sleep is gated by the LDRs going dark
  (or the `LOCK`/`STOW` flag). The setter (main) never sleeps — it runs the BLE radio
  continuously — so the latch line is always actively driven; no RTC-GPIO hold needed on
  the main side.
- Single-axis reference design:
  <https://www.sciencebuddies.org/science-fair-projects/project-ideas/Energy_p045/energy-power/solar-tracker>

## Power & energy

### Energy-budget analysis before resizing

Concern: stronger BLE TX + a 2-axis tracker may need a bigger battery/panel. Settle it by
**measurement, not guesswork** — the INA219s already log `solar_mv/ma`, `batt_mv`,
`load_ma`, so capture the real balance over several days (ideally across a low-sun spell)
before buying anything. Earlier analysis had the 12 W panel massively over-supplying the
node, so there is real headroom. Notes on the two additions:

- **BLE TX bump is negligible.** TX is brief bursts at 1 Hz advertising; 0 → +10 dBm
  raises peak TX current but barely moves the average. Not a battery-sizing driver.
- **The tracker is the real consumer — but it's net-positive if designed right.** A
  2-axis tracker harvests roughly +30–40% over a fixed panel. Keep servo energy well
  under that gain: **power-gate the servos** (MOSFET, off between moves), adjust only
  every few minutes (the sun moves ~15°/h), and avoid holding position under power
  (worm gear / detent, or tolerate small drift). Then it funds its own draw and then
  some, especially mornings/evenings/winter.
- A bigger battery is cheap insurance for low-sun spells regardless — but make it a
  measured call, not a pre-emptive guess.

### Deep-sleep / duty-cycling

The firmware currently runs continuously and advertises at 1 Hz. Duty-cycling the sensors
and radio (sample + broadcast a burst, then sleep) is the biggest lever for average draw —
potentially removing the need for a bigger battery at all. Tension: continuous 1 Hz
broadcast is what makes the passive-scan dashboards work, so a battery profile would trade
freshness for endurance (e.g. broadcast a burst every N seconds, sleep between). Pairs
naturally with the buck-boost efficiency upgrade already noted in the power phase-2 work.

### Battery over-discharge protection

Any lithium pack run flat is damaged or dangerous. The chosen **4S LiFePO4 with integrated
BMS** (see [Rails & conversion](#rails--conversion-distribute-one-bus-regulate-at-each-board))
covers the low-voltage cutoff and balancing in hardware; still add a firmware low-battery
state that sheds load (drop servo moves, slow the broadcast) before the BMS cutoff trips.
The `batt_mv` telemetry gives the early-warning signal. See also cold-charge protection
under Cold-climate operation below.

## Cold-climate operation (Alps, ~1500 m)

The station lives outdoors at altitude: hard freezing, snow, rime ice, condensation, and
big temperature swings. This is a cross-cutting constraint, not a single feature.

### Battery cold-charge protection (critical)

No lithium chemistry — the chosen **4S LiFePO4** included — **may be charged below 0 °C**:
charging a frozen cell plates lithium, permanently damages it, and is a fire risk. At
1500 m in winter the pack will be sub-zero exactly when the sun appears to charge it, so
this must be solved regardless of sensors:

- An **NTC thermistor on the pack** driving a **charge-inhibit below ~0–5 °C** (gate the
  charge path, or use a 12 V/4S MPPT charger that exposes a temp/NTC pin).
- And/or insulate + gently warm the battery compartment up to a safe charge temperature
  before enabling charge.
- LiFePO4 tolerates cold _discharge_ better than LiPo — but, as above, it still can't be
  charged sub-zero, so the inhibit is needed either way.
- Discharge is fine down to roughly -20 °C, just at reduced capacity; insulation helps
  hold the pack warmer than ambient.
- **The pack is getting replaced for the cold anyway, so size it once for the winter
  worst case _with_ heaters** — the marginal cost of a bigger cell now beats a second swap
  later. But note the cold-charge inhibit is **required regardless of chemistry**: LiFePO4
  tolerates cold _discharge_ better, yet still can't be charged sub-zero, so swapping
  chemistry does not remove this requirement.

### Sensor heaters

Heating to keep sensors reading truthfully through frost and snow. Ranked by whether the
watts are worth it:

- **Rain gauge** — a tipping bucket can't measure snow; it only registers later when the
  snow melts, so timing and rate are wrong. A heated funnel melts it on contact. This is
  the **biggest power draw** (several watts) and the main budget problem.
- **Anemometer + wind vane** — rime ice seizes the moving parts and reports false calm.
  Heating moving parts/bearings is hard; at minimum, **detect a stuck rotor and flag it**
  (diagnostics bit) rather than broadcasting 0 km/h as truth.
- **MLX90614 / VEML7700 windows** — frost and condensation block the optics. Small
  anti-frost window heaters, or for the MLX the servo-park already planned under
  "MLX90614 sky-IR protection".
- **Enclosure condensation** — breather vent + desiccant, possibly a trickle heater, to
  stop temp swings from fogging/icing the inside.

### The power-budget collision

Heaters are by far the largest loads, and they peak exactly when solar is weakest: short
winter days, low sun angle, snow on the panel. Continuous rain-gauge heating may not be
sustainable on the current solar budget. Honest options, to decide per sensor:

- **Thermostatic / demand-driven** heating — energise only when near-freezing **and**
  precipitation is detected (the temp + rain-gauge telemetry already exist to gate it);
  PTC self-regulating elements simplify the control.
- A materially **bigger panel + battery** sized for the winter worst case.
- Accept **degraded snow/ice measurement** in deep winter as a deliberate tradeoff.

This interacts directly with [Power & energy](#power--energy) — size the winter budget
from measured data before committing to heaters.

## Software

### Android app

A phone app would be a nice front-end alongside the `meteo-tui` dashboard: passively scan
for the `MeteoStation` advert, decode the same manufacturer-data telemetry frame
(company `0xFFFF`, the v5 wire format in `meteo-lib::ble::frame`), and show live readings +
history. No GATT connection needed for telemetry; the PIN-gated location write could be a
nice-to-have config screen. A stronger BLE radio (above) would help the app's link too.

### Over-the-air (OTA) firmware update

Update the firmware without physically reaching the station. Feasible, no blockers, but a
meaningful project.

- **BLE-only path.** The ESP32-H2 has no Wi-Fi (802.15.4 + BLE), so OTA must go over the
  existing BLE link — not the usual Wi-Fi/HTTP OTA.
- **Reuses what's already here.** Partition-table reads via `esp-bootloader-esp-idf`,
  flash writes from the config task (`sequential-storage`), and the PIN-gated GATT write
  channel are all patterns the firmware already uses.
- **New work:** an OTA partition layout (`ota_0` / `ota_1` / `otadata` instead of the
  single app partition; check the image fits twice in flash), a DFU GATT service that
  receives the image in chunks → writes the inactive slot → flips `otadata` → resets,
  rollback-on-failure safety, and a host-side uploader tool.
- **Throughput caveat.** BLE moves an image slowly (low tens of KB/s even with a larger
  MTU / 2M PHY / data-length extension) — minutes per update for a few-hundred-KB image.
  Fine for occasional field updates; OTA complements the USB-JTAG flash path, it doesn't
  replace it for day-to-day iteration. A stronger BLE link (see above) would also speed
  this up.

### Programming & updating all the boards

The OTA item above covers the Main board; with the
[multi-board architecture](#federated-multi-board-architecture-power--main--tracker--heaters)
the whole fleet needs a flashing/update story. How it stays manageable:

- **Fewer programmable boards is the first win.** The heaters and power/PMU boards are
  designed MCU-less (PTC + latches; hardware-latched battery safety), so they have **no
  firmware** and drop out of the update problem. Only **Main + Tracker** are programmable,
  and only Main has a radio.
- **A wired programming path on every programmable board, always.** Each carries a
  programming/debug header or USB-Serial-JTAG pads — for bring-up _and_ as the recovery
  path when an OTA bricks. Never seal a board you can't physically re-flash.
- **Main is the field update gateway.** An image arrives over BLE to Main (the OTA item
  above), and Main **forwards firmware to the Tracker over the inter-board bus** (CAN/UART)
  into a bootloader on the Tracker that writes its own flash — same A/B + rollback pattern
  as Main's OTA, with the bus as transport instead of BLE. One BLE push (or the Pi) updates
  the sealed box. The bus must carry bulk payloads, not just command flags (CAN needs
  ISO-TP and is slow-ish; UART is faster — minutes per image either way).
- **Same MCU family across satellites** (extends the BLE "stay in the family" rule): one
  toolchain, one bootloader protocol, one update mechanism, shared `meteo-lib`.
  Heterogeneous MCUs would mean parallel toolchains and update paths.
- **Version the fleet.** Each board reports its firmware version (Main in telemetry,
  Tracker over the bus), and the **inter-board protocol is versioned** so a freshly-updated
  Main doesn't issue a command a stale Tracker can't parse. A/B + rollback per board; a
  Tracker that bricks mid-update must fall to its `STOW` fail-safe and stay re-flashable.
  Main is the most critical — if Main bricks, the gateway to everyone is lost, so its
  rollback + wired recovery matter most.
- **The Pi collector, if built, is the natural local update host** — it already talks to
  the station, so it can serve firmware over BLE (or a wired link when co-located).

### Web server for historic data (Raspberry Pi collector) — IMPLEMENTED (`meteo-web`)

**Status: implemented as `crates/meteo-web`.** The `meteo-web` crate is a Leptos 0.8 SSR
application (cargo-leptos) that runs the BLE collector and the web server in one binary.
See `CLAUDE.md §"Web dashboard (meteo-web / meteo-chart)"` for the full architecture.

What was built matches the original design notes:

- Passive BLE scan (bluer) reusing `meteo-lib::ble::frame::decode` — same wire format as
  `meteo-tui`, no re-implementation.
- 1-minute min/max/avg SQLite buckets (`samples` table, WAL, `~170 KiB/day`).
- Query-time re-aggregation (`GROUP BY bucket_ts / bucket_secs`) with
  sample-count-weighted averages; no rollup tables needed.
- Two web pages: `/` (all-panels live + history grid) and `/comparaison` (multi-day
  overlay chart). Custom Leptos SVG charts matching the TUI palette and layout.
- Live SSE stream at `GET /live` — 1 Hz `LiveFrame` JSON events.
- Pi deployment path: `just web-build-pi` (aarch64 cross-build; needs toolchain installed
  — see `CLAUDE.md`).
- Timestamps from NTP wall-clock (the Pi has it), making on-device RTC optional for
  history.

Remaining open item from the original note: the Pi cross-build toolchain (`aarch64-
unknown-linux-gnu` rustup target + cross linker) is not yet installed; `just web-build-pi`
will fail until it is. `just web-build` and `just web-serve` work on the dev host today.

### On-device logging & RTC

Today telemetry is broadcast live only — if no observer is scanning, that data is gone.
On-device storage (a ring buffer in flash, or an SD card for real capacity) would keep a
continuous record the dashboard/app can backfill from on next contact. This needs real
timestamps, so pair it with a battery-backed RTC (e.g. DS3231 on the existing I2C0 bus) —
the H2 has no network time and loses its clock on power loss. The frame already carries
`uptime_s`, but that resets every reboot; wall-clock time makes logs comparable.

## References

- Example weather station (Printables) — **for ideas only, not to copy**:
  <https://www.printables.com/model/61709-weather-station-one-part-1-the-central-station>

## Related notes

- Power-subsystem phase-2 upgrades (buck-boost to 3.3V for efficiency; the INA219
  current monitoring from that note is now built) are tracked separately.
- See `CLAUDE.md` for the firmware-side follow-ups (WS2812 colour LED, upstreaming the
  vendored trouble-host patch).
