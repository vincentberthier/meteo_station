#!/usr/bin/env python3
"""Generate the Meteo-Station power-subsystem KiCad schematic.

Strategy: reuse real symbol definitions from the system KiCad libraries (so symbols
look right and pins are correct), place each module/part at rotation 0, and wire nets
with on-sheet labels on short stubs (robust: no point-to-point routing geometry).

Output: meteo_power.kicad_sch (+ a minimal meteo_power.kicad_pro), written next to
this script by default, or into the directory given as argv[1].
Validate:  kicad-cli sch erc meteo_power.kicad_sch
           kicad-cli sch export svg meteo_power.kicad_sch
"""
import re, sys, uuid, math, json, os

SYMDIR = "/usr/share/kicad/symbols"
ROOT_UUID = "0b1d0c2a-1111-4aaa-8bbb-000000000001"  # fixed sheet uuid
PROJECT = "meteo_power"

# ---------- tiny s-expression parser/serialiser (preserves quoted vs bare) ----------
class Sym(str):
    """A bare s-expr atom (keyword/number) — serialised without quotes."""

def tokenize(s):
    return re.findall(r'"(?:[^"\\]|\\.)*"|\(|\)|[^\s()]+', s)

def parse(toks):
    x = toks.pop(0)
    if x == '(':
        lst = []
        while toks[0] != ')':
            lst.append(parse(toks))
        toks.pop(0)
        return lst
    if x.startswith('"'):
        return x[1:-1].replace('\\"', '"').replace('\\\\', '\\')
    return Sym(x)

def ser(n, indent=0):
    if isinstance(n, list):
        parts = []
        for i, c in enumerate(n):
            # The first atom of any s-expr list is the tag/keyword and is ALWAYS
            # bare. Constructed nodes use plain str heads ('symbol', 'property', …);
            # emit them unquoted so KiCad accepts the file.
            if i == 0 and not isinstance(c, list):
                parts.append(str(c))
            else:
                parts.append(ser(c))
        return "(" + " ".join(parts) + ")"
    if isinstance(n, Sym):
        return str(n)
    s = str(n).replace('\\', '\\\\').replace('"', '\\"')
    return '"' + s + '"'

_LIBTEXT = {}
def load_symbol(lib, name):
    """Return the symbol node from <lib>.kicad_sym, renamed to lib_id form.

    Extracts just the one symbol's balanced-paren span before parsing — the full
    libraries are multi-MB and parsing all of them is needlessly slow."""
    text = _LIBTEXT.setdefault(lib, open(f"{SYMDIR}/{lib}.kicad_sym").read())
    i = text.find(f'(symbol "{name}"')
    if i < 0:
        raise SystemExit(f"symbol {lib}:{name} not found")
    depth, j = 0, i
    while j < len(text):
        c = text[j]
        if c == '"':                       # skip over quoted strings
            j += 1
            while j < len(text) and text[j] != '"':
                j += 2 if text[j] == '\\' else 1
        elif c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                j += 1
                break
        j += 1
    node = parse(tokenize(text[i:j]))
    node[1] = f"{lib}:{name}"               # "R" -> "Device:R"
    return clean_symbol(node)

# v10-only tokens that the v9 (20250114) schematic parser does not accept inside
# embedded lib_symbols — strip them so the embedded defs match the schematic version.
_DROP = {"in_pos_files", "duplicate_pin_numbers_are_jumpers", "embedded_fonts",
         "show_name", "do_not_autoplace"}
def clean_symbol(node):
    if not isinstance(node, list):
        return node
    out = []
    for c in node:
        if isinstance(c, list) and c and isinstance(c[0], str) and c[0] in _DROP:
            continue
        out.append(clean_symbol(c))
    return out

def U():  # fresh uuid string
    return str(uuid.uuid4())

# ---------- pin geometry in SHEET space (rotation 0): pin -> (dx, dy, outx, outy) ----------
PINS = {
    "Device:R":            {'1': (0, -3.81, 0, -1), '2': (0, 3.81, 0, 1)},
    "Device:C":            {'1': (0, -3.81, 0, -1), '2': (0, 3.81, 0, 1)},
    "Device:C_Polarized":  {'1': (0, -3.81, 0, -1), '2': (0, 3.81, 0, 1)},
    "Device:D_Schottky":   {'1': (-3.81, 0, -1, 0), '2': (3.81, 0, 1, 0)},   # 1=K, 2=A
    "Device:Battery_Cell": {'1': (0, -5.08, 0, -1), '2': (0, 2.54, 0, 1)},   # 1=+, 2=-
    "Connector_Generic:Conn_01x02": {'1': (-5.08, 0, -1, 0), '2': (-5.08, 2.54, -1, 0)},
    "Connector_Generic:Conn_01x04": {'1': (-5.08, -2.54, -1, 0), '2': (-5.08, 0, -1, 0),
                                      '3': (-5.08, 2.54, -1, 0), '4': (-5.08, 5.08, -1, 0)},
    "Connector_Generic:Conn_01x06": {'1': (-5.08, -5.08, -1, 0), '2': (-5.08, -2.54, -1, 0),
                                      '3': (-5.08, 0, -1, 0), '4': (-5.08, 2.54, -1, 0),
                                      '5': (-5.08, 5.08, -1, 0), '6': (-5.08, 7.62, -1, 0)},
}

# ---------- the design: (ref, lib_id, value, x, y, {pin: net}) ----------
COMPS = [
    ("J1",  "Connector_Generic:Conn_01x02", "PV-12W  12V 1A",        45, 60, {'1':'SOLAR+', '2':'GND'}),
    ("U1",  "Connector_Generic:Conn_01x04", "CN3791 12V MPPT",      100, 60, {'1':'SOLAR+', '2':'GND', '3':'VBAT', '4':'GND'}),
    ("BT1", "Device:Battery_Cell",          "1S LiPo 3.7V 10Ah",    100, 120,{'1':'VBAT', '2':'GND'}),
    ("U2",  "Connector_Generic:Conn_01x04", "MT3608 -> 5.0V",       165, 60, {'1':'VBAT', '2':'GND', '3':'BOOST5V', '4':'GND'}),
    ("D1",  "Device:D_Schottky",            "1N5817",               210, 47, {'2':'BOOST5V', '1':'V5'}),
    ("U3",  "Connector_Generic:Conn_01x06", "ESP32-H2-DevKitM-1",   305, 70, {'1':'V5', '2':'GND', '3':'V3V3', '4':'VSENSE', '5':'SDA', '6':'SCL'}),
    # battery-sense divider
    ("R1",  "Device:R",                     "100k",                 250, 150,{'1':'VBAT',  '2':'VSENSE'}),
    ("R2",  "Device:R",                     "100k",                 250, 180,{'1':'VSENSE','2':'GND'}),
    # I2C pull-ups
    ("R3",  "Device:R",                     "4.75k",                360, 105,{'1':'V3V3', '2':'SDA'}),
    ("R4",  "Device:R",                     "4.75k",                378, 105,{'1':'V3V3', '2':'SCL'}),
    # I2C sensors
    ("U4",  "Connector_Generic:Conn_01x04", "BMP388 0x76",          305, 150,{'1':'V3V3','2':'GND','3':'SDA','4':'SCL'}),
    ("U5",  "Connector_Generic:Conn_01x04", "MLX90614 0x5A",        375, 150,{'1':'V3V3','2':'GND','3':'SDA','4':'SCL'}),
    # decoupling / bulk
    ("C1",  "Device:C_Polarized",           "22uF",                 140, 115,{'1':'VBAT','2':'GND'}),
    ("C2",  "Device:C_Polarized",           "100uF",                280, 105,{'1':'V5','2':'GND'}),
    ("C3",  "Device:C",                     "10uF",                 295, 105,{'1':'V5','2':'GND'}),
    ("C4",  "Device:C",                     "10uF",                 330, 200,{'1':'V3V3','2':'GND'}),
    ("C5",  "Device:C",                     "100nF",                305, 200,{'1':'V3V3','2':'GND'}),
    ("C6",  "Device:C",                     "100nF",                375, 200,{'1':'V3V3','2':'GND'}),
]

NOTES = [
    (40, 25, 3.0, "Meteo Station — Power Subsystem  (generated)"),
    (40, 33, 1.6, "PV-12W -> CN3791 12V MPPT -> 1S LiPo -> MT3608 (5.0V) -> DevKit 5V pin."),
    (40, 38, 1.6, "D1 (1N5817): MT3608 5V -> DevKit 5V pin; isolates the boost so USB VBUS wins when flashing (no back-feed)."),
    (40, 43, 1.6, "R1/R2 divide VBAT to ~2.1V into GPIO2 (ADC1_CH1). I2C pull-ups 4.75k on SDA/SCL."),
    (210, 145, 1.6, "FUTURE: INA219 + owned 0.1ohm shunt in series with VBAT for current/power telemetry."),
    # Where each decoupling/bulk cap physically goes (placed in the empty lower-left).
    (40, 235, 2.2, "Capacitor placement (physical):"),
    (42, 243, 1.6, "C1 22uF   - bulk at the MT3608 input (by the battery / boost VIN)"),
    (42, 249, 1.6, "C2 100uF  - bulk on the 5V rail, at the DevKit 5V pin"),
    (42, 255, 1.6, "C3 10uF   - beside C2 at the DevKit 5V pin"),
    (42, 261, 1.6, "C4 10uF   - bulk on the 3V3 rail, at the DevKit 3V3 pin"),
    (42, 267, 1.6, "C5 100nF  - across BMP388 VCC-GND, hard against the chip's pins"),
    (42, 273, 1.6, "C6 100nF  - across MLX90614 VCC-GND, hard against the chip's pins"),
    (42, 281, 1.6, "Rule: a 100nF sits right at each chip; bulk caps go where current steps (boost out, radio, regulator)."),
]

G = 1.27  # KiCad connection grid (50 mil)
def gr(v):
    """Snap a coordinate to the 1.27 mm connection grid (exact 2-decimal multiple)."""
    return round(round(v / G) * G, 2)
def co(v):
    """Format a coordinate as a bare atom, rounded to kill float epsilon."""
    return Sym(f"{round(v, 2):g}")

def mk_props(ref, value, x, y):
    eff = ['effects', ['font', ['size', Sym('1.27'), Sym('1.27')]]]
    effh = ['effects', ['font', ['size', Sym('1.27'), Sym('1.27')]], ['hide', Sym('yes')]]
    return [
        ['property', "Reference", ref, ['at', co(x), co(y-9), Sym('0')], eff],
        ['property', "Value", value, ['at', co(x), co(y+11), Sym('0')], eff],
        ['property', "Footprint", "", ['at', co(x), co(y), Sym('0')], effh],
        ['property', "Datasheet", "~", ['at', co(x), co(y), Sym('0')], effh],
    ]

def mk_instance(ref, lib_id, value, x, y, pinmap):
    pins = [['pin', pn, ['uuid', U()]] for pn in pinmap]
    inst = ['instances', ['project', PROJECT,
              ['path', f"/{ROOT_UUID}", ['reference', ref], ['unit', Sym('1')]]]]
    node = ['symbol',
            ['lib_id', lib_id],
            ['at', co(x), co(y), Sym('0')],
            ['unit', Sym('1')],
            ['exclude_from_sim', Sym('no')], ['in_bom', Sym('yes')],
            ['on_board', Sym('yes')], ['dnp', Sym('no')],
            ['uuid', U()],
            *mk_props(ref, value, x, y),
            *pins, inst]
    return node

def mk_wire(x1, y1, x2, y2):
    return ['wire', ['pts', ['xy', co(x1), co(y1)], ['xy', co(x2), co(y2)]],
            ['stroke', ['width', Sym('0')], ['type', Sym('default')]], ['uuid', U()]]

def mk_label(net, x, y, ox):
    just = Sym('right') if ox < 0 else Sym('left')
    return ['label', net, ['at', co(x), co(y), Sym('0')],
            ['effects', ['font', ['size', Sym('1.27'), Sym('1.27')]], ['justify', just]],
            ['uuid', U()]]

def mk_text(x, y, size, txt):
    return ['text', txt, ['exclude_from_sim', Sym('no')],
            ['at', Sym(f"{x}"), Sym(f"{y}"), Sym('0')],
            ['effects', ['font', ['size', Sym(f"{size}"), Sym(f"{size}")]], ['justify', Sym('left')]],
            ['uuid', U()]]

def main():
    used_libids = sorted({c[1] for c in COMPS})
    lib_symbols = ['lib_symbols']
    for lib_id in used_libids:
        lib, name = lib_id.split(':', 1)
        lib_symbols.append(load_symbol(lib, name))

    body = []
    for ref, lib_id, value, x, y, pinmap in COMPS:
        x, y = gr(x), gr(y)               # snap origin to the connection grid
        body.append(mk_instance(ref, lib_id, value, x, y, pinmap))
        geo = PINS[lib_id]
        for pn, net in pinmap.items():
            dx, dy, ox, oy = geo[pn]
            px, py = x + dx, y + dy
            qx, qy = px + ox * 2.54, py + oy * 2.54
            body.append(mk_wire(px, py, qx, qy))
            body.append(mk_label(net, qx, qy, ox))

    for x, y, size, txt in NOTES:
        body.append(mk_text(x, y, size, txt))

    tree = ['kicad_sch',
            ['version', Sym('20250114')],
            ['generator', "meteo-power-gen"],
            ['generator_version', "9.0"],
            ['uuid', ROOT_UUID],
            ['paper', "A3"],
            ['title_block', ['title', "Meteo Station Power"], ['rev', "1.0"]],
            lib_symbols,
            *body,
            ['sheet_instances', ['path', "/", ['page', "1"]]],
            ['embedded_fonts', Sym('no')]]

    outdir = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
    sch = os.path.join(outdir, f"{PROJECT}.kicad_sch")
    with open(sch, "w") as f:
        f.write(ser(tree) + "\n")

    # minimal project file so it opens as a project
    pro = {"board": {}, "boards": [], "meta": {"filename": f"{PROJECT}.kicad_pro", "version": 1},
           "schematic": {}, "sheets": [[ROOT_UUID, "Root"]]}
    with open(os.path.join(outdir, f"{PROJECT}.kicad_pro"), "w") as f:
        json.dump(pro, f, indent=2)

    print(f"wrote {sch} ({os.path.getsize(sch)} bytes) and {PROJECT}.kicad_pro")
    print(f"components: {len(COMPS)}  nets: {len(sorted({n for c in COMPS for n in c[5].values()}))}")

if __name__ == '__main__':
    main()
