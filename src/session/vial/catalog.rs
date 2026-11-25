use super::keycodes;
use super::layout::{CustomCode, Midi, Tone};

pub(super) struct Table {
    entries: Vec<Entry>,
    shelves: Vec<Shelf>,
}

struct Shelf {
    name: &'static str,
    start: usize,
    end: usize,
}

struct Entry {
    token: String,
    legend: String,
    code: u16,
    /// Set from the table row. Callers ask the table. They do not compare a code number.
    hold_first: bool,
    /// A Vial `(kc)` code. Confirm takes a basic key to fill its low byte.
    masked: bool,
}

pub(in crate::session) struct Found {
    pub(in crate::session) code: u16,
    pub(in crate::session) legend: String,
    /// The QMK name, shown beside the legend when it adds something.
    pub(in crate::session) token: String,
    pub(in crate::session) masked: bool,
}

pub(in crate::session) struct Listed {
    pub(in crate::session) title: String,
    pub(in crate::session) hits: Vec<Found>,
}

pub(in crate::session) enum Range<'a> {
    Shelf(usize),
    Find(&'a str),
}

/// Optional QMK features Vial checks before it shows a key.
#[derive(Clone, Copy, Default)]
pub(super) struct Features {
    pub(super) caps_word: bool,
    pub(super) layer_lock: bool,
    pub(super) repeat_key: bool,
    pub(super) persistent_default_layer: bool,
}

/// Shelves are the Vial keycode tabs, in Vial's order. An empty tab is omitted.
pub(super) fn merge(
    custom: &[CustomCode],
    tap_dance: u8,
    macros: u8,
    midi: Midi,
    layers: u8,
    features: Features,
) -> Table {
    let midi = match midi {
        Midi::None => Vec::new(),
        Midi::Basic => rows(keycodes::MIDI_BASIC),
        Midi::Advanced => [keycodes::MIDI_BASIC, keycodes::MIDI_ADVANCED]
            .iter()
            .flat_map(|table| rows(table))
            .collect(),
    };
    let user = if custom.is_empty() {
        rows(keycodes::USER_DEFAULT)
    } else {
        custom
            .iter()
            .map(|item| entry(&item.name, &item.name, item.code))
            .collect()
    };
    let dances = (0..u16::from(tap_dance))
        .map(|index| {
            let name = format!("TD({index})");
            entry(&name, &name, 0x5700 | index)
        })
        .collect();
    let mut macro_keys: Vec<Entry> = (0..u16::from(macros))
        .map(|index| {
            let name = format!("M{index}");
            entry(&name, &name, 0x7700 | index)
        })
        .collect();
    macro_keys.extend(rows(keycodes::MACRO_DYNAMIC));

    let mut entries = Vec::new();
    let mut shelves = Vec::new();
    for (name, body) in [
        ("Basic", rows(keycodes::BASIC)),
        ("ISO/JIS", rows(keycodes::ISO_JIS)),
        ("Layers", layer_keys(layers, features)),
        (
            "Quantum",
            rows(keycodes::QUANTUM)
                .into_iter()
                .filter(|entry| match entry.token.as_str() {
                    "QK_CAPS_WORD_TOGGLE" => features.caps_word,
                    "QK_REPEAT_KEY" | "QK_ALT_REPEAT_KEY" => features.repeat_key,
                    _ => true,
                })
                .collect(),
        ),
        ("Backlight", rows(keycodes::BACKLIGHT)),
        ("App, Media and Mouse", rows(keycodes::MEDIA)),
        ("MIDI", midi),
        ("Tap Dance", dances),
        ("User", user),
        ("Macro", macro_keys),
    ] {
        if body.is_empty() {
            continue;
        }
        let start = entries.len();
        entries.extend(body);
        shelves.push(Shelf {
            name,
            start,
            end: entries.len(),
        });
    }
    Table { entries, shelves }
}

/// Adds a custom keycode to the User shelf, made at the end when there is none. A code the
/// table already has is left as it is.
pub(super) fn add_custom(table: &mut Table, code: u16, name: &str) {
    if table.entries.iter().any(|entry| entry.code == code) {
        return;
    }
    let Some(at) = table.shelves.iter().position(|shelf| shelf.name == "User") else {
        let start = table.entries.len();
        table.entries.push(entry(name, name, code));
        table.shelves.push(Shelf {
            name: "User",
            start,
            end: start + 1,
        });
        return;
    };
    let end = table.shelves[at].end;
    table.entries.insert(end, entry(name, name, code));
    table.shelves[at].end += 1;
    for shelf in &mut table.shelves[at + 1..] {
        shelf.start += 1;
        shelf.end += 1;
    }
}

/// Drops the entries `keep` refuses, and a shelf left empty. A `(kc)` entry is asked with `A`
/// inside it.
pub(super) fn retain(table: Table, keep: impl Fn(u16) -> bool) -> Table {
    let mut entries = Vec::new();
    let mut shelves = Vec::new();
    let mut all = table.entries.into_iter();
    for shelf in table.shelves {
        let start = entries.len();
        entries.extend(all.by_ref().take(shelf.end - shelf.start).filter(|entry| {
            keep(if entry.masked {
                entry.code | 0x04
            } else {
                entry.code
            })
        }));
        if entries.len() > start {
            shelves.push(Shelf {
                name: shelf.name,
                start,
                end: entries.len(),
            });
        }
    }
    Table { entries, shelves }
}

/// The Layers shelf for a board with `count` layers: each kind for every layer, `LT` for the first 16.
fn layer_keys(count: u8, features: Features) -> Vec<Entry> {
    // QMK packs the layer into the low 5 bits.
    let count = u16::from(count.min(32));
    let mut out = Vec::new();
    for &(token, legend, code) in keycodes::LAYER {
        let shown = match token {
            "QK_LAYER_LOCK" => features.layer_lock,
            // Tri-layer needs a lower, an upper and the third layer above them.
            _ => count >= 4,
        };
        if shown {
            out.push(entry(token, legend, code));
        }
    }
    let pdf = features.persistent_default_layer;
    for (kind, base, shown) in [
        ("MO", 0x5220, true),  // QK_MOMENTARY
        ("DF", 0x5240, true),  // QK_DEF_LAYER
        ("PDF", 0x52E0, pdf),  // QK_PERSISTENT_DEF_LAYER
        ("TG", 0x5260, true),  // QK_TOGGLE_LAYER
        ("TT", 0x52C0, true),  // QK_LAYER_TAP_TOGGLE
        ("OSL", 0x5280, true), // QK_ONE_SHOT_LAYER
        ("TO", 0x5200, true),  // QK_TO
    ] {
        if !shown {
            continue;
        }
        for layer in 0..count {
            let name = format!("{kind}({layer})");
            out.push(entry(&name, &name, base | layer));
        }
    }
    for layer in 0..count.min(16) {
        let name = format!("LT{layer}(kc)");
        out.push(entry(&name, &name, 0x4000 | layer << 8));
    }
    out
}

fn rows(table: &[(&str, &str, u16)]) -> Vec<Entry> {
    table
        .iter()
        .map(|&(token, legend, code)| entry(token, legend, code))
        .collect()
}

fn entry(token: &str, legend: &str, code: u16) -> Entry {
    Entry {
        token: token.to_string(),
        legend: legend.to_string(),
        code,
        // QMK's QK_BOOT. The flag, not the number, is what begin_hold checks.
        hold_first: token == "QK_BOOTLOADER",
        masked: token.ends_with("(kc)"),
    }
}

pub(super) fn shelf_count(table: &Table) -> usize {
    table.shelves.len()
}

/// `Shelf` returns that run in vec order. `Find` ranks a prefix before a substring,
/// then a shorter legend, then vec order. Case-insensitive over token and legend.
/// An empty token returns no hits.
pub(super) fn list(table: &Table, range: Range<'_>) -> Listed {
    match range {
        Range::Shelf(index) => Listed {
            title: shelf_title(&table.shelves, index),
            hits: table
                .shelves
                .get(index)
                .map(|shelf| {
                    table.entries[shelf.start..shelf.end]
                        .iter()
                        .map(found)
                        .collect()
                })
                .unwrap_or_default(),
        },
        Range::Find(token) => Listed {
            title: format!("search {token}"),
            hits: if token.is_empty() {
                Vec::new()
            } else {
                rank(table, token)
            },
        },
    }
}

fn shelf_title(shelves: &[Shelf], index: usize) -> String {
    shelves
        .iter()
        .enumerate()
        .map(|(place, shelf)| {
            if place == index {
                format!("[{}]", shelf.name)
            } else {
                shelf.name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn found(entry: &Entry) -> Found {
    Found {
        code: entry.code,
        legend: entry.legend.clone(),
        token: entry.token.clone(),
        masked: entry.masked,
    }
}

fn rank(table: &Table, query: &str) -> Vec<Found> {
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
            found(entry),
        ));
    }
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(_, _, _, hit)| hit).collect()
}

pub(super) fn legend(table: &Table, code: u16) -> String {
    if let Some(entry) = table.entries.iter().find(|entry| entry.code == code) {
        return entry.legend.clone();
    }
    compose(table, code).unwrap_or_else(|| format!("{code:04X}"))
}

/// The QMK name when the table lists this exact code, as `KC_ESCAPE`.
pub(super) fn token(table: &Table, code: u16) -> Option<String> {
    table
        .entries
        .iter()
        .find(|entry| entry.code == code)
        .map(|entry| entry.token.clone())
}

/// What a keycode does, in a few words. Ranges are QMK's, as in `compose` and `tone`.
/// Layers count from 0, as QMK, the legends (`MO(1)`), and the tabs (`L1`) do.
pub(super) fn kind(code: u16) -> String {
    let tap = code & 0xFF;
    let layer = |bits: u16| format!("layer {bits}");
    match code {
        0x0000 => "does nothing".to_string(),
        0x0001 => "transparent: uses the layer below".to_string(),
        0x00E0..=0x00E7 => "modifier".to_string(),
        0x00A8..=0x00AE => "media key".to_string(),
        0x0002..=0x00FF => "basic key".to_string(),
        0x0100..=0x1FFF => "key with modifiers held".to_string(),
        0x2000..=0x3FFF => "mod-tap: hold for the modifier, tap for the key".to_string(),
        0x4000..=0x4FFF => format!(
            "layer-tap: hold for {}, tap for the key",
            layer((code >> 8) & 0x0F)
        ),
        0x5200..=0x521F => format!("switches to {}", layer(code & 0x1F)),
        0x5220..=0x523F => format!("{} while held", layer(code & 0x1F)),
        0x5240..=0x525F => format!("sets the default layer to {}", layer(code & 0x1F)),
        0x5260..=0x527F => format!("toggles {}", layer(code & 0x1F)),
        0x5280..=0x529F => format!("{} for the next key", layer(code & 0x1F)),
        0x52C0..=0x52DF => format!("{} while held, toggles on taps", layer(code & 0x1F)),
        0x5700..=0x57FF => format!("tap dance {tap}"),
        0x7700..=0x77FF => format!("macro {tap}"),
        0x7E00..=0x7FFF => "keyboard's own keycode".to_string(),
        _ => "QMK function".to_string(),
    }
}

/// Short name for a composite QMK code the table does not list one by one.
fn compose(table: &Table, code: u16) -> Option<String> {
    let tap = code & 0xFF;
    if (0x0100..0x2000).contains(&code) {
        let mods = (code >> 8) & 0x1F;
        return Some(format!("{}-{}", mod_name(mods), legend(table, tap)));
    }
    if (0x2000..0x4000).contains(&code) {
        let mods = (code >> 8) & 0x1F;
        return Some(format!("{}({})", mod_name(mods), legend(table, tap)));
    }
    if (0x4000..0x5000).contains(&code) {
        let layer = (code >> 8) & 0x0F;
        return Some(format!("L{layer}({})", legend(table, tap)));
    }
    if (0x5700..0x5800).contains(&code) {
        return Some(format!("TD({})", code & 0xFF));
    }
    None
}

/// Paint family for a keycode. Ranges sit next to `compose` so a new family is one match.
pub(super) fn tone(code: u16) -> Tone {
    if code == 0x0000 {
        return Tone::None;
    }
    if code == 0x0001 {
        return Tone::Trans;
    }
    if (0x00A8..=0x00AE).contains(&code) {
        return Tone::Media;
    }
    if (0x00E0..=0x00E7).contains(&code) || (0x0100..0x4000).contains(&code) {
        return Tone::Hold;
    }
    if (0x4000..0x5000).contains(&code)
        || (0x5200..0x52A0).contains(&code)
        || (0x52C0..0x52E0).contains(&code)
    {
        return Tone::Layer;
    }
    if (0x52A0..0x52C0).contains(&code) || (0x5700..0x5800).contains(&code) {
        return Tone::Hold;
    }
    if code >= 0x7E00 {
        return Tone::Key;
    }
    if code >= 0x5000 {
        return Tone::Raw;
    }
    Tone::Key
}

fn mod_name(mask: u16) -> String {
    match mask {
        0x01 => "C".to_string(),
        0x02 => "S".to_string(),
        0x04 => "A".to_string(),
        0x08 => "G".to_string(),
        0x11 => "Rc".to_string(),
        0x12 => "Rs".to_string(),
        0x14 => "Ra".to_string(),
        0x18 => "Rg".to_string(),
        0x03 => "CS".to_string(),
        0x07 => "Meh".to_string(),
        0x0F => "Hyper".to_string(),
        other => format!("{other:02X}"),
    }
}

pub(super) fn hold_first(table: &Table, code: u16) -> bool {
    table
        .entries
        .iter()
        .any(|entry| entry.code == code && entry.hold_first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Features = Features {
        caps_word: true,
        layer_lock: true,
        repeat_key: true,
        persistent_default_layer: true,
    };

    fn tokens(table: &Table, shelf: usize) -> Vec<&str> {
        let shelf = &table.shelves[shelf];
        table.entries[shelf.start..shelf.end]
            .iter()
            .map(|entry| entry.token.as_str())
            .collect()
    }
    #[test]
    fn composites_use_short_names() {
        let table = merge(&[], 0, 0, Midi::None, 4, ALL);
        assert_eq!(legend(&table, 0x2104), "C(A)");
        assert_eq!(legend(&table, 0x3133), "Rc(;)");
        assert_eq!(legend(&table, 0x00A9), "Vol +");
        assert_eq!(legend(&table, 0x0235), "~");
        assert_eq!(legend(&table, 0x5719), "TD(25)");
        assert_eq!(legend(&table, 0x5F0D), "5F0D");
        assert!(matches!(tone(0x0000), Tone::None));
        assert!(matches!(tone(0x0001), Tone::Trans));
        assert!(matches!(tone(0x00A9), Tone::Media));
        assert!(matches!(tone(0x00E0), Tone::Hold));
        assert!(matches!(tone(0x2104), Tone::Hold));
        assert!(matches!(tone(0x5221), Tone::Layer));
        assert!(matches!(tone(0x4104), Tone::Layer));
        assert!(matches!(tone(0x5719), Tone::Hold));
        assert!(matches!(tone(0x5F0D), Tone::Raw));
        assert!(matches!(tone(0x7C00), Tone::Raw));
        assert!(matches!(tone(0x7E00), Tone::Key));
        assert!(matches!(tone(0x0004), Tone::Key));
    }

    #[test]
    fn shelf_zero_is_basic() {
        let table = merge(&[], 32, 0, Midi::None, 4, ALL);
        let listed = list(&table, Range::Shelf(0));
        assert_eq!(
            listed.hits.first().map(|hit| hit.legend.as_str()),
            Some("NO")
        );
        assert_eq!(listed.hits.last().map(|hit| hit.legend.as_str()), Some("?"));
        assert!(listed.hits.iter().all(|hit| hit.legend != "MO(0)"));
        assert!(listed.hits.iter().all(|hit| hit.legend != "Mute"));
        assert!(listed.hits.iter().all(|hit| hit.legend != "Boot"));
    }

    #[test]
    fn find_td_starts_at_the_first_tap_dance() {
        let table = merge(&[], 32, 0, Midi::None, 4, ALL);
        let listed = list(&table, Range::Find("td"));
        assert_eq!(listed.title, "search td");
        assert_eq!(
            listed.hits.first().map(|hit| hit.legend.as_str()),
            Some("TD(0)")
        );
        assert!(listed.hits.iter().any(|hit| hit.legend == "TD(25)"));
        let dances = |listed: &Listed| {
            listed
                .hits
                .iter()
                .filter(|hit| hit.legend.starts_with("TD("))
                .count()
        };
        assert_eq!(dances(&listed), 32);
        assert!(list(&table, Range::Find("td(25)"))
            .hits
            .iter()
            .any(|hit| hit.legend == "TD(25)"));
        assert_eq!(
            dances(&list(
                &merge(&[], 0, 0, Midi::None, 4, ALL),
                Range::Find("td")
            )),
            0
        );
        assert!(list(&table, Range::Find("")).hits.is_empty());
    }

    #[test]
    fn layers_follow_the_board_layer_count() {
        let layers = |count| merge(&[], 0, 0, Midi::None, count, ALL);
        let table = layers(2);
        let two = tokens(&table, 2);
        assert!(two.contains(&"MO(1)"));
        assert!(!two.contains(&"MO(2)"));
        assert!(!two.contains(&"LT2(kc)"));
        assert!(!two.contains(&"QK_TRI_LAYER_LOWER"));
        assert_eq!(two.len(), 1 + 7 * 2 + 2);
        let table = layers(4);
        let four = tokens(&table, 2);
        assert!(four.contains(&"QK_TRI_LAYER_LOWER"));
        assert!(four.contains(&"TO(3)"));
        let table = layers(20);
        let many = tokens(&table, 2);
        assert!(many.contains(&"MO(19)"));
        assert!(many.contains(&"LT15(kc)"));
        assert!(!many.contains(&"LT16(kc)"));
    }

    #[test]
    fn unsupported_features_are_hidden() {
        let gated = [
            "QK_LAYER_LOCK",
            "PDF(0)",
            "QK_CAPS_WORD_TOGGLE",
            "QK_REPEAT_KEY",
            "QK_ALT_REPEAT_KEY",
        ];
        let present = |features| {
            let table = merge(&[], 0, 0, Midi::None, 2, features);
            let mut shown = tokens(&table, 2);
            shown.extend(tokens(&table, 3));
            gated.iter().filter(|token| shown.contains(token)).count()
        };
        assert_eq!(present(Features::default()), 0);
        assert_eq!(present(ALL), gated.len());
        let caps_only = Features {
            caps_word: true,
            ..Features::default()
        };
        assert_eq!(present(caps_only), 1);
    }

    #[test]
    fn empty_tabs_are_omitted() {
        let table = merge(&[], 0, 0, Midi::None, 4, ALL);
        let names: Vec<_> = table.shelves.iter().map(|shelf| shelf.name).collect();
        assert_eq!(
            names,
            [
                "Basic",
                "ISO/JIS",
                "Layers",
                "Quantum",
                "Backlight",
                "App, Media and Mouse",
                "User",
                "Macro"
            ]
        );
        let user = list(&table, Range::Shelf(6));
        assert_eq!(user.hits.first().map(|hit| hit.code), Some(0x7E00));
        assert_eq!(user.hits.len(), 16);
        let macros = list(&table, Range::Shelf(7));
        assert_eq!(
            macros.hits.first().map(|hit| hit.legend.as_str()),
            Some("DM Rec 1")
        );
    }

    #[test]
    fn shelves_partition_the_entries() {
        let custom = [CustomCode {
            code: 0x7E00,
            name: "MyKey".to_string(),
        }];
        let table = merge(&custom, 2, 3, Midi::Advanced, 4, ALL);
        let mut legends = Vec::new();
        for index in 0..table.shelves.len() {
            legends.extend(
                list(&table, Range::Shelf(index))
                    .hits
                    .into_iter()
                    .map(|hit| hit.legend),
            );
        }
        let entries: Vec<_> = table
            .entries
            .iter()
            .map(|entry| entry.legend.clone())
            .collect();
        assert_eq!(legends, entries);
        assert_eq!(
            list(&table, Range::Shelf(0)).title,
            "[Basic] ISO/JIS Layers Quantum Backlight App, Media and Mouse MIDI Tap Dance User Macro"
        );
        let user = list(&table, Range::Shelf(8));
        assert_eq!(user.hits.len(), 1);
        let macros = list(&table, Range::Shelf(9));
        assert_eq!(macros.hits.first().map(|hit| hit.code), Some(0x7700));
        assert_eq!(
            macros.hits.get(2).map(|hit| hit.legend.as_str()),
            Some("M2")
        );
    }
}
