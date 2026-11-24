#!/usr/bin/env python3
"""Generate src/session/vial/keycodes.rs from a QMK firmware checkout.

    python3 scripts/keycodes.py ~/qmk_firmware > src/session/vial/keycodes.rs

Only interface facts come from QMK: keycode identifiers and their numeric values
(quantum/keycodes.h, quantum/quantum_keycodes.h, quantum/modifiers.h,
quantum/keymap_extras/keymap_us.h). Which codes land on which shelf, and every
legend, are decided here.
"""

import re
import subprocess
import sys
from pathlib import Path

qmk = Path(sys.argv[1])


def text(rel):
    return (qmk / rel).read_text()


# Primary QMK name for each code: the first `NAME = 0x...,` in qk_keycode_defines.
keycodes_h = text("quantum/keycodes.h")
defines = keycodes_h.split("enum qk_keycode_defines {", 1)[1].split("// Alias", 1)[0]
NAME = {}
for name, value in re.findall(r"^\s*(\w+) = (0x[0-9A-F]+),", defines, re.M):
    NAME.setdefault(int(value, 16), name)
CODE = {name: code for code, name in NAME.items()}
RANGE = {k: int(v, 16) for k, v in re.findall(r"^\s*(QK_\w+)\s+= (0x[0-9A-F]+),", keycodes_h, re.M)}

QK_MOD = {k: int(v, 16) for k, v in re.findall(r"#define (QK_[LR](?:CTL|SFT|ALT|GUI))\s+(0x[0-9A-F]+)", text("quantum/quantum_keycodes.h"))}
MOD = {k: int(v, 16) for k, v in re.findall(r"(MOD_[LR](?:CTL|SFT|ALT|GUI)) = (0x[0-9A-F]+),", text("quantum/modifiers.h"))}

# Own legends. Anything not here is named from its QMK identifier by `humanize`.
SYMBOL = {
    "MINUS": "-", "EQUAL": "=", "LEFT_BRACKET": "[", "RIGHT_BRACKET": "]",
    "BACKSLASH": "\\", "SEMICOLON": ";", "QUOTE": "'", "GRAVE": "`",
    "COMMA": ",", "DOT": ".", "SLASH": "/",
}
SHIFTED = dict(zip("`1234567890-=[]\\;',./", "~!@#$%^&*()_+{}|:\"<>?"))
WORDS = {
    "KC_NO": "NO", "KC_TRANSPARENT": "TRNS", "KC_ESCAPE": "Esc", "KC_BACKSPACE": "Bksp",
    "KC_DELETE": "Del", "KC_INSERT": "Ins", "KC_PAGE_UP": "PgUp", "KC_PAGE_DOWN": "PgDn",
    "KC_PRINT_SCREEN": "PrtSc", "KC_SCROLL_LOCK": "ScrLk", "KC_NUM_LOCK": "NumLk",
    "KC_CAPS_LOCK": "Caps", "KC_APPLICATION": "Menu", "KC_KP_ENTER": "Num Enter",
    "KC_NONUS_HASH": "ISO #", "KC_NONUS_BACKSLASH": "ISO \\",
    "KC_AUDIO_MUTE": "Mute", "KC_AUDIO_VOL_UP": "Vol +", "KC_AUDIO_VOL_DOWN": "Vol -",
    "KC_KB_MUTE": "KB Mute", "KC_KB_VOLUME_UP": "KB Vol +", "KC_KB_VOLUME_DOWN": "KB Vol -",
    "KC_MEDIA_NEXT_TRACK": "Next", "KC_MEDIA_PREV_TRACK": "Prev", "KC_MEDIA_STOP": "Stop Media",
    "KC_MEDIA_PLAY_PAUSE": "Play", "KC_MEDIA_FAST_FORWARD": "Fwd", "KC_MEDIA_REWIND": "Rewind",
    "KC_BRIGHTNESS_UP": "Bri +", "KC_BRIGHTNESS_DOWN": "Bri -",
    "KC_SYSTEM_POWER": "Power", "KC_SYSTEM_SLEEP": "Sleep", "KC_SYSTEM_WAKE": "Wake",
    "QK_BOOTLOADER": "Bootloader", "QK_CLEAR_EEPROM": "Clear EEPROM",
    "QK_GRAVE_ESCAPE": "Esc / ~",
    "QK_SPACE_CADET_LEFT_CTRL_PARENTHESIS_OPEN": "LCtrl / (",
    "QK_SPACE_CADET_RIGHT_CTRL_PARENTHESIS_CLOSE": "RCtrl / )",
    "QK_SPACE_CADET_LEFT_SHIFT_PARENTHESIS_OPEN": "LShift / (",
    "QK_SPACE_CADET_RIGHT_SHIFT_PARENTHESIS_CLOSE": "RShift / )",
    "QK_SPACE_CADET_LEFT_ALT_PARENTHESIS_OPEN": "LAlt / (",
    "QK_SPACE_CADET_RIGHT_ALT_PARENTHESIS_CLOSE": "RAlt / )",
    "QK_SPACE_CADET_RIGHT_SHIFT_ENTER": "RShift / Enter",
    "QK_REPEAT_KEY": "Repeat", "QK_ALT_REPEAT_KEY": "Alt Repeat",
    "QK_CAPS_WORD_TOGGLE": "Caps Word",
    "QK_DYNAMIC_MACRO_RECORD_START_1": "DM Rec 1", "QK_DYNAMIC_MACRO_RECORD_START_2": "DM Rec 2",
    "QK_DYNAMIC_MACRO_RECORD_STOP": "DM Stop", "QK_DYNAMIC_MACRO_PLAY_1": "DM Play 1",
    "QK_DYNAMIC_MACRO_PLAY_2": "DM Play 2",
}
UPPER = {"RGB", "LED", "NKRO", "GUI", "EEPROM", "BSD", "WWW", "KB", "LCTL", "LGUI",
         "RCTL", "RGUI", "LALT", "RALT", "CTL", "ALT", "MIDI", "CC", "BL"}
SIDE = {"LEFT": "L", "RIGHT": "R"}
MODS = {"CTRL": "Ctrl", "SHIFT": "Shift", "ALT": "Alt", "GUI": "Gui"}


def humanize(name):
    words = re.sub(r"^(KC|QK)_", "", name).split("_")
    return " ".join(w if w in UPPER else w.capitalize() for w in words)


def legend(name):
    if name in WORDS:
        return WORDS[name]
    m = re.fullmatch(r"KC_(\w|F\d+)", name)
    if m:
        return m.group(1)
    m = re.fullmatch(r"KC_(LEFT|RIGHT)_(CTRL|SHIFT|ALT|GUI)", name)
    if m:
        return SIDE[m.group(1)] + MODS[m.group(2)]
    m = re.fullmatch(r"KC_KP_([A-Z0-9]+)", name)
    if m:
        sym = {"SLASH": "/", "ASTERISK": "*", "MINUS": "-", "PLUS": "+", "DOT": ".", "EQUAL": "=", "COMMA": ","}
        return "Num " + sym.get(m.group(1), m.group(1).capitalize())
    if name[3:] in SYMBOL:
        return SYMBOL[name[3:]]
    m = re.fullmatch(r"KC_(INTERNATIONAL|LANGUAGE)_(\d)", name)
    if m:
        return ("Intl " if m.group(1) == "INTERNATIONAL" else "Lang ") + m.group(2)
    m = re.fullmatch(r"QK_MOUSE_(CURSOR|BUTTON|WHEEL|ACCELERATION)_(\w+)", name)
    if m:
        kind = {"CURSOR": "Mouse", "BUTTON": "Mouse", "WHEEL": "Wheel", "ACCELERATION": "Accel"}[m.group(1)]
        return f"{kind} {m.group(2).capitalize()}"
    m = re.fullmatch(r"QK_MIDI_NOTE_([A-G])(_SHARP)?_(\d)", name)
    if m:
        return f"{m.group(1)}{'#' if m.group(2) else ''}{m.group(3)}"
    m = re.fullmatch(r"QK_KB_(\d+)", name)
    if m:
        return f"User {m.group(1)}"
    return humanize(name)


def keys(codes):
    return [(NAME[c], legend(NAME[c]), c) for c in codes if c in NAME]


def span(lo, hi, skip=()):
    return [c for c in range(lo, hi + 1) if c not in skip]


# Shift-ed US symbols: `#define KC_TILD S(KC_GRAVE)` -> 0x0200 | KC_GRAVE.
shifted = []
for name, base in re.findall(r"#define (KC_\w+)\s+S\((KC_\w+)\)", text("quantum/keymap_extras/keymap_us.h")):
    base_legend = legend(base)
    shifted.append((name, SHIFTED[base_legend], QK_MOD["QK_LSFT"] | CODE[base]))

# Mod wrappers and mod-taps, from their QMK macro definitions.
qk_h = text("quantum/quantum_keycodes.h")
mod_letter = lambda bits: "".join(l for b, l in [(1, "C"), (2, "S"), (4, "A"), (8, "G")] if bits & b)
wrappers = []
for name, body in re.findall(r"#define (\w+)\(kc\) \(((?:QK_\w+ \| )+)\(kc\)\)", qk_h):
    wrappers.append((f"{name}(kc)", f"{name}(kc)", sum(QK_MOD[m] for m in body.split(" | ") if m)))
taps = []
for name, body in re.findall(r"#define (\w+_T)\(kc\) MT\(([\w |]+), kc\)", qk_h):
    bits = 0
    for m in body.split("|"):
        bits |= MOD[m.strip()]
    taps.append((f"{name}(kc)", f"{name}(kc)", 0x2000 | bits << 8))

# One-shot mods: every combination on each hand.
osm = []
for right in (0, 0x10):
    for bits in range(1, 16):
        names = [f"MOD_{'R' if right else 'L'}{m}" for b, m in [(1, "CTL"), (2, "SFT"), (4, "ALT"), (8, "GUI")] if bits & b]
        osm.append((f"OSM({'|'.join(names)})", f"OSM {'R' if right else 'L'}{mod_letter(bits)}", RANGE["QK_ONE_SHOT_MOD"] | right | bits))

LAYER_KEYS = [CODE[n] for n in ("QK_LAYER_LOCK", "QK_TRI_LAYER_LOWER", "QK_TRI_LAYER_UPPER")]
DYNAMIC = span(CODE["QK_DYNAMIC_MACRO_RECORD_START_1"], CODE["QK_DYNAMIC_MACRO_PLAY_2"])
ISO = [CODE["KC_NONUS_HASH"], CODE["KC_NONUS_BACKSLASH"]] + span(CODE["KC_INTERNATIONAL_1"], CODE["KC_LANGUAGE_9"])
BASIC_EXTRA = [CODE["KC_APPLICATION"], CODE["KC_KP_EQUAL"], CODE["KC_KP_COMMA"]]
midi = [c for c in NAME if 0x7100 <= c <= 0x71FF]
midi_basic = [c for c in midi if c <= CODE["QK_MIDI_TOGGLE"] or re.fullmatch(r"QK_MIDI_NOTE_\w+_0", NAME[c])]

TABLES = {
    "BASIC": keys([0, 1] + span(0x04, 0x63, ISO) + BASIC_EXTRA + span(0xE0, 0xE7)) + shifted,
    "ISO_JIS": keys(ISO),
    "QUANTUM": keys(span(0x7C00, 0x7C0F))
    + osm
    + wrappers
    + taps
    + keys(span(0x7C10, 0x7CFF, set(LAYER_KEYS) | set(DYNAMIC)) + span(0x7000, 0x70FF) + span(0x7480, 0x74BF) + span(0x56F0, 0x56FF)),
    "BACKLIGHT": keys(span(0x7800, 0x78FF)),
    "MEDIA": keys(span(0x66, 0xDF, set(ISO) | set(BASIC_EXTRA))),
    "MIDI_BASIC": keys(midi_basic),
    "MIDI_ADVANCED": keys(sorted(set(midi) - set(midi_basic))),
    "USER_DEFAULT": keys(span(0x7E00, 0x7E0F)),
    "MACRO_DYNAMIC": keys(DYNAMIC),
    "LAYER": keys(LAYER_KEYS),
}

rev = subprocess.run(["git", "-C", str(qmk), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
version = re.search(r'QMK_KEYCODES_VERSION "([\d.]+)"', keycodes_h).group(1)
out = [
    "//! Keycode shelves, generated by `scripts/keycodes.py` from QMK",
    f"//! (qmk/qmk_firmware@{rev}, keycodes {version}). Do not edit by hand.",
    "//! `(qmk_id, legend, code)`: identifiers and codes are QMK's interface; shelves and legends are ours.",
    "//! A `(kc)` id takes a basic key inside it. Layers depend on the board's layer count, so `catalog` builds them.",
]
for const, rows in TABLES.items():
    out.append("")
    out.append(f"pub(super) const {const}: &[(&str, &str, u16)] = &[")
    for name, label, code in rows:
        esc = lambda s: s.replace("\\", "\\\\").replace('"', '\\"')
        out.append(f'    ("{esc(name)}", "{esc(label)}", 0x{code:04X}),')
    out.append("];")
print("\n".join(out))
