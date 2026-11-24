//! Vial tap dance and combo records, and the macro buffer codec, after vial-gui
//! `protocol/tap_dance.py`, `protocol/combo.py`, and `protocol/macro.py` (protocol 2 and later).

/// An editor tab after the layer tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::session) enum Tab {
    TapDance,
    Combos,
    Macros,
}

impl Tab {
    pub(in crate::session) fn name(self) -> &'static str {
        match self {
            Tab::TapDance => "Tap Dance",
            Tab::Combos => "Combos",
            Tab::Macros => "Macros",
        }
    }
}

/// The cursor on a tap dance, combo, or macro tab. `row` is the entry, `col` the field or action.
/// A macro row has one more column than actions: the slot that appends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::session) struct Spot {
    pub(in crate::session) tab: Tab,
    pub(in crate::session) row: usize,
    pub(in crate::session) col: usize,
}

/// On tap, on hold, on double tap, on tap then hold, and the tapping term in ms.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(in crate::session) struct TapDance {
    pub(in crate::session) keys: [u16; 4],
    pub(in crate::session) term: u16,
}

impl TapDance {
    pub(super) fn words(self) -> [u16; 5] {
        let [a, b, c, d] = self.keys;
        [a, b, c, d, self.term]
    }

    pub(super) fn from_words(words: [u16; 5]) -> Self {
        let [a, b, c, d, term] = words;
        Self {
            keys: [a, b, c, d],
            term,
        }
    }
}

/// Four input keys, then the output key.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(in crate::session) struct Combo {
    pub(in crate::session) keys: [u16; 5],
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::session) enum Action {
    /// Printable ASCII. A NUL ends the macro and `0x01` starts a code, so neither may appear.
    Text(String),
    Tap(u16),
    Down(u16),
    Up(u16),
    /// Milliseconds, at most `MAX_DELAY`.
    Delay(u16),
}

/// The firmware stores a delay as two bytes of `value + 1`, so 254 * 255 + 254 is the ceiling.
pub(in crate::session) const MAX_DELAY: u16 = 65024;

const PREFIX: u8 = 0x01;
const TAP: u8 = 0x01;
const DOWN: u8 = 0x02;
const UP: u8 = 0x03;
const DELAY: u8 = 0x04;
const EXT_TAP: u8 = 0x05;
const EXT_DOWN: u8 = 0x06;
const EXT_UP: u8 = 0x07;

/// Every macro, NUL-terminated, back to back. This is the buffer the firmware reads.
pub(super) fn encode_macros(macros: &[Vec<Action>]) -> Vec<u8> {
    let mut out = Vec::new();
    for actions in macros {
        for action in actions {
            encode_action(action, &mut out);
        }
        out.push(0);
    }
    out
}

fn encode_action(action: &Action, out: &mut Vec<u8>) {
    let (short, ext, code) = match action {
        Action::Text(text) => {
            out.extend(text.bytes().filter(|byte| (0x20..0x7F).contains(byte)));
            return;
        }
        Action::Delay(ms) => {
            let ms = (*ms).min(MAX_DELAY);
            out.extend([PREFIX, DELAY, (ms % 255) as u8 + 1, (ms / 255) as u8 + 1]);
            return;
        }
        Action::Tap(code) => (TAP, EXT_TAP, *code),
        Action::Down(code) => (DOWN, EXT_DOWN, *code),
        Action::Up(code) => (UP, EXT_UP, *code),
    };
    if code < 0x100 {
        out.extend([PREFIX, short, code as u8]);
        return;
    }
    // QMK `decode_keycode`: a code with a zero low byte travels as `0xFF00 | high`, so no NUL hits the buffer.
    let wire = if code & 0xFF == 0 {
        0xFF00 | (code >> 8)
    } else {
        code
    };
    out.push(PREFIX);
    out.push(ext);
    out.extend(wire.to_le_bytes());
}

/// Split on NUL and keep `count` macros. Missing macros are empty. A cut-off code is dropped.
pub(super) fn decode_macros(buffer: &[u8], count: usize) -> Vec<Vec<Action>> {
    let mut macros: Vec<Vec<Action>> = buffer
        .split(|byte| *byte == 0)
        .take(count)
        .map(decode_macro)
        .collect();
    macros.resize(count, Vec::new());
    macros
}

fn decode_macro(mut data: &[u8]) -> Vec<Action> {
    let mut out = Vec::new();
    let mut text = Vec::new();
    while let Some(&first) = data.first() {
        if first != PREFIX {
            text.push(first);
            data = &data[1..];
            continue;
        }
        if !text.is_empty() {
            out.push(Action::Text(String::from_utf8_lossy(&text).into_owned()));
            text.clear();
        }
        let Some(&kind) = data.get(1) else {
            break;
        };
        let (action, used) = match kind {
            TAP | DOWN | UP => {
                let Some(&code) = data.get(2) else { break };
                (key_action(kind, u16::from(code)), 3)
            }
            EXT_TAP | EXT_DOWN | EXT_UP => {
                let (Some(&lo), Some(&hi)) = (data.get(2), data.get(3)) else {
                    break;
                };
                let mut code = u16::from_le_bytes([lo, hi]);
                if code > 0xFF00 {
                    code = (code & 0xFF) << 8;
                }
                (key_action(kind - (EXT_TAP - TAP), code), 4)
            }
            DELAY => {
                let (Some(&lo), Some(&hi)) = (data.get(2), data.get(3)) else {
                    break;
                };
                let ms = u16::from(lo.saturating_sub(1)) + u16::from(hi.saturating_sub(1)) * 255;
                (Some(Action::Delay(ms)), 4)
            }
            // vial-gui skips two bytes of an unknown code and keeps going.
            _ => (None, 2),
        };
        out.extend(action);
        data = &data[used.min(data.len())..];
    }
    if !text.is_empty() {
        out.push(Action::Text(String::from_utf8_lossy(&text).into_owned()));
    }
    out
}

fn key_action(kind: u8, code: u16) -> Option<Action> {
    match kind {
        TAP => Some(Action::Tap(code)),
        DOWN => Some(Action::Down(code)),
        UP => Some(Action::Up(code)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_round_trip_through_the_buffer() {
        let macros = vec![
            vec![
                Action::Text("hi!".into()),
                Action::Tap(0x0004),
                Action::Down(0x00E0),
                Action::Tap(0x5700),
                Action::Up(0x2104),
                Action::Delay(300),
            ],
            Vec::new(),
            vec![Action::Delay(MAX_DELAY)],
        ];
        let bytes = encode_macros(&macros);
        assert_eq!(bytes.iter().filter(|byte| **byte == 0).count(), 3);
        assert_eq!(decode_macros(&bytes, 3), macros);
    }

    #[test]
    fn vial_gui_bytes_decode() {
        // `a`, tap KC_B, delay 1000 (1000 = 235 + 3 * 255), then ext tap of 0x5700 as 0xFF57.
        let bytes = [b'a', 1, 1, 5, 1, 4, 236, 4, 1, 5, 0x57, 0xFF, 0, 0, 0];
        assert_eq!(
            decode_macros(&bytes, 2),
            vec![
                vec![
                    Action::Text("a".into()),
                    Action::Tap(5),
                    Action::Delay(1000),
                    Action::Tap(0x5700),
                ],
                Vec::new(),
            ]
        );
    }
}
