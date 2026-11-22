use super::layout::CustomCode;

pub(super) struct Table {
    entries: Vec<Entry>,
}

struct Entry {
    token: String,
    legend: String,
    code: u16,
    /// Set from the table row. Callers ask the table. They do not compare a code number.
    hold_first: bool,
}

pub(in crate::session) struct Found {
    pub(in crate::session) code: u16,
    pub(in crate::session) legend: String,
}

pub(super) fn merge(custom: &[CustomCode]) -> Table {
    let mut entries = basic();
    for item in custom {
        entries.push(Entry {
            token: item.name.clone(),
            legend: item.name.clone(),
            code: item.code,
            hold_first: false,
        });
    }
    Table { entries }
}

/// Prefix before substring. Shorter legend before longer. Then vec order.
/// Case-insensitive over token and legend.
pub(super) fn search(table: &Table, query: &str) -> Vec<Found> {
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    let mut ranked = Vec::new();
    for (index, entry) in table.entries.iter().enumerate() {
        let legend = entry.legend.to_lowercase();
        let token = entry.token.to_lowercase();
        let prefix = legend.starts_with(&query) || token.starts_with(&query);
        let inside = legend.contains(&query) || token.contains(&query);
        if !prefix && !inside {
            continue;
        }
        ranked.push((
            if prefix { 0u8 } else { 1 },
            entry.legend.len(),
            index,
            Found {
                code: entry.code,
                legend: entry.legend.clone(),
            },
        ));
    }
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(_, _, _, found)| found).collect()
}

pub(super) fn legend(table: &Table, code: u16) -> String {
    table
        .entries
        .iter()
        .find(|entry| entry.code == code)
        .map(|entry| entry.legend.clone())
        .unwrap_or_else(|| format!("{code:04X}"))
}

pub(super) fn hold_first(table: &Table, code: u16) -> bool {
    table
        .entries
        .iter()
        .any(|entry| entry.code == code && entry.hold_first)
}

fn basic() -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut push = |token: &str, legend: &str, code: u16, hold_first: bool| {
        entries.push(Entry {
            token: token.to_string(),
            legend: legend.to_string(),
            code,
            hold_first,
        });
    };

    push("KC_NO", "NO", 0x0000, false);
    push("KC_TRNS", "TRNS", 0x0001, false);
    for index in 0..26u16 {
        let letter = (b'A' + index as u8) as char;
        push(
            &format!("KC_{letter}"),
            &letter.to_string(),
            0x0004 + index,
            false,
        );
    }
    for digit in 1..=9u16 {
        push(
            &format!("KC_{digit}"),
            &digit.to_string(),
            0x001E + digit - 1,
            false,
        );
    }
    push("KC_0", "0", 0x0027, false);
    push("KC_ENT", "Enter", 0x0028, false);
    push("KC_ESC", "Esc", 0x0029, false);
    push("KC_BSPC", "Bspc", 0x002A, false);
    push("KC_TAB", "Tab", 0x002B, false);
    push("KC_SPC", "Space", 0x002C, false);
    push("KC_MINS", "-", 0x002D, false);
    push("KC_EQL", "=", 0x002E, false);
    push("KC_LBRC", "[", 0x002F, false);
    push("KC_RBRC", "]", 0x0030, false);
    push("KC_BSLS", "\\", 0x0031, false);
    push("KC_SCLN", ";", 0x0033, false);
    push("KC_QUOT", "'", 0x0034, false);
    push("KC_GRV", "`", 0x0035, false);
    push("KC_COMM", ",", 0x0036, false);
    push("KC_DOT", ".", 0x0037, false);
    push("KC_SLSH", "/", 0x0038, false);
    push("KC_CAPS", "Caps", 0x0039, false);
    for index in 0..12u16 {
        let n = index + 1;
        push(&format!("KC_F{n}"), &format!("F{n}"), 0x003A + index, false);
    }
    push("KC_PSCR", "PScr", 0x0046, false);
    push("KC_SCRL", "Scrl", 0x0047, false);
    push("KC_PAUS", "Pause", 0x0048, false);
    push("KC_INS", "Ins", 0x0049, false);
    push("KC_HOME", "Home", 0x004A, false);
    push("KC_PGUP", "PgUp", 0x004B, false);
    push("KC_DEL", "Del", 0x004C, false);
    push("KC_END", "End", 0x004D, false);
    push("KC_PGDN", "PgDn", 0x004E, false);
    push("KC_RGHT", "Right", 0x004F, false);
    push("KC_LEFT", "Left", 0x0050, false);
    push("KC_DOWN", "Down", 0x0051, false);
    push("KC_UP", "Up", 0x0052, false);
    push("KC_LCTL", "LCtl", 0x00E0, false);
    push("KC_LSFT", "LSft", 0x00E1, false);
    push("KC_LALT", "LAlt", 0x00E2, false);
    push("KC_LGUI", "LGui", 0x00E3, false);
    push("KC_RCTL", "RCtl", 0x00E4, false);
    push("KC_RSFT", "RSft", 0x00E5, false);
    push("KC_RALT", "RAlt", 0x00E6, false);
    push("KC_RGUI", "RGui", 0x00E7, false);

    // QMK ranges. The low 5 bits are the layer, or the mod mask for OSM.
    const QK_TO: u16 = 0x5200;
    const QK_MOMENTARY: u16 = 0x5220;
    const QK_TOGGLE_LAYER: u16 = 0x5260;
    const QK_ONE_SHOT_LAYER: u16 = 0x5280;
    const QK_ONE_SHOT_MOD: u16 = 0x52A0;
    const QK_LAYER_TAP_TOGGLE: u16 = 0x52C0;
    for layer in 0..16u16 {
        push(
            &format!("MO({layer})"),
            &format!("MO({layer})"),
            QK_MOMENTARY | layer,
            false,
        );
        push(
            &format!("TG({layer})"),
            &format!("TG({layer})"),
            QK_TOGGLE_LAYER | layer,
            false,
        );
        push(
            &format!("TO({layer})"),
            &format!("TO({layer})"),
            QK_TO | layer,
            false,
        );
        push(
            &format!("TT({layer})"),
            &format!("TT({layer})"),
            QK_LAYER_TAP_TOGGLE | layer,
            false,
        );
        push(
            &format!("OSL({layer})"),
            &format!("OSL({layer})"),
            QK_ONE_SHOT_LAYER | layer,
            false,
        );
    }
    let osm = [
        ("LCtl", 0x01u16),
        ("LSft", 0x02),
        ("LAlt", 0x04),
        ("LGui", 0x08),
        ("RCtl", 0x11),
        ("RSft", 0x12),
        ("RAlt", 0x14),
        ("RGui", 0x18),
    ];
    for (name, mask) in osm {
        push(
            &format!("OSM({name})"),
            &format!("OSM({name})"),
            QK_ONE_SHOT_MOD | mask,
            false,
        );
    }

    // QMK's QK_BOOT. The flag, not this number, is what begin_hold checks.
    push("QK_BOOT", "Reset", 0x7C00, true);
    entries
}
