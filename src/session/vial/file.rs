//! A ZMK `.keymap` file, served to `Load` and `Board` as Vial reports, the way `zmk::Zmk` serves a
//! Studio keyboard. Bindings that QMK has a keycode for show as that keycode; every other one is
//! a custom keycode from `0x7E00` named by its own text, so `&thm LCTRL A` stays exactly that.
//! A write replaces only the bytes of the binding it changes: comments, `#define`s, and the
//! grid the file was laid out in stay as they were.

use super::dts::{self, Doc, Edit, Node, Prop};
use super::sections::{Cell, Kind, Record};
use super::{
    compress, decode_request, encode_reply, keymap_bytes, zmk, zmk_keys, Ask, Candidate, Io,
    LayerOp, Listing, Reply,
};
use crate::{Error, Result};
use std::ops::Range;
use std::path::{Path, PathBuf};

const CUSTOM: u16 = 0x7E00;
const CUSTOM_ROOM: usize = 0x200;

/// ZMK's behavior labels whose bindings have a QMK keycode, with the `display-name` that
/// `zmk::qmk_code` keys them by, and how many parameters each takes.
const LABELS: [(&str, &str, usize); 16] = [
    ("kp", "Key Press", 1),
    ("mt", "Mod-Tap", 2),
    ("lt", "Layer-Tap", 2),
    ("mo", "Momentary Layer", 1),
    ("tog", "Toggle Layer", 1),
    ("to", "To Layer", 1),
    ("sl", "Sticky Layer", 1),
    ("sk", "Sticky Key", 1),
    ("trans", "Transparent", 0),
    ("none", "None", 0),
    ("bootloader", "Bootloader", 0),
    ("sys_reset", "Reset", 0),
    ("gresc", "Grave/Escape", 0),
    ("caps_word", "Caps Word", 0),
    ("key_repeat", "Key Repeat", 0),
    ("mkp", "Mouse Key Press", 1),
];

/// Behaviors whose first parameter is a layer index.
const LAYER_LABELS: [&str; 5] = ["mo", "tog", "to", "sl", "lt"];

/// zmk/app/include/dt-bindings/zmk/pointing.h button names; the first of each value prints.
const BUTTONS: [(&str, u32); 8] = [
    ("LCLK", 1),
    ("RCLK", 2),
    ("MCLK", 4),
    ("MB4", 8),
    ("MB5", 16),
    ("MB1", 1),
    ("MB2", 2),
    ("MB3", 4),
];

/// Bindings with no QMK keycode that a ZMK keyboard usually has. The key list offers these
/// beside the ones already in the file.
const EXTRAS: [&str; 13] = [
    "&bt BT_SEL 0",
    "&bt BT_SEL 1",
    "&bt BT_SEL 2",
    "&bt BT_SEL 3",
    "&bt BT_SEL 4",
    "&bt BT_NXT",
    "&bt BT_PRV",
    "&bt BT_CLR",
    "&bt BT_CLR_ALL",
    "&out OUT_USB",
    "&out OUT_BLE",
    "&out OUT_TOG",
    "&studio_unlock",
];

/// The `.keymap` file at `path`, ready to load; for a folder, each of `config/*.keymap`, else
/// each of its own `*.keymap`, for the choose screen.
pub(in crate::session) fn candidates(path: &Path) -> Result<Vec<Candidate>> {
    Ok(keymaps_in(path)?
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "keymap".to_string());
            Candidate {
                listing: Box::new(Listing {
                    name,
                    maker: "keymap file".to_string(),
                    usb: path.display().to_string(),
                    serial: String::new(),
                }),
                io: Some(Box::new(FileIo {
                    path,
                    doc: None,
                    custom: Vec::new(),
                    compressed: Vec::new(),
                })),
                guard: None,
            }
        })
        .collect())
}

fn keymaps_in(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    for dir in [path.join("config"), path.to_path_buf()] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|file| file.extension().is_some_and(|ext| ext == "keymap"))
            .collect();
        if !found.is_empty() {
            found.sort();
            return Ok(found);
        }
    }
    Err(Error::Protocol("no .keymap file in that folder"))
}

struct FileIo {
    path: PathBuf,
    /// `None` until the first report, and again after a layer change, so the next load reads
    /// the file afresh.
    doc: Option<Doc>,
    /// Bindings with no QMK keycode, by their text. Index `n` is keycode `CUSTOM + n`.
    custom: Vec<String>,
    /// The Vial definition made from the layout file, XZ-compressed.
    compressed: Vec<u8>,
}

impl Io for FileIo {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
        let ask = decode_request(report)?;
        let reply = self.answer(&ask)?;
        Ok(encode_reply(&ask, &reply))
    }

    fn speaks(&self, code: u16) -> bool {
        self.text_of(code).is_some()
    }

    fn layer_names(&self) -> Vec<String> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        layers(doc)
            .iter()
            .map(|layer| {
                layer
                    .prop("display-name")
                    .and_then(string)
                    .unwrap_or_else(|| layer.name.clone())
            })
            .collect()
    }

    fn edit_layers(&mut self, op: &LayerOp) -> Result<()> {
        let doc = self
            .doc
            .take()
            .ok_or(Error::Protocol("the keymap is not read"))?;
        let text = layer_edit(&doc, op)?;
        self.save(&text)
    }

    fn intern(&mut self, text: &str) -> Option<(u16, String)> {
        let doc = self.doc.as_ref()?;
        let tokens: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        if tokens
            .first()
            .is_none_or(|first| !first.starts_with('&') || first.len() < 2)
        {
            return None;
        }
        let text = tokens.join(" ");
        if let Some(code) = native(doc, &tokens) {
            return Some((code, String::new()));
        }
        let index = match self.custom.iter().position(|known| *known == text) {
            Some(index) => index,
            None if self.custom.len() < CUSTOM_ROOM => {
                self.custom.push(text.clone());
                self.custom.len() - 1
            }
            None => return None,
        };
        Some((CUSTOM + index as u16, custom_name(&text)))
    }

    fn writes_to(&self) -> &'static str {
        "the file"
    }

    fn complete(&self, typed: &str) -> Option<Completion> {
        Some(complete(self.doc.as_ref()?, typed))
    }

    fn binding_text(&self, code: u16) -> Option<String> {
        self.text_of(code)
    }

    fn sections(&self) -> Vec<(Kind, Vec<Record>)> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        Kind::ALL
            .iter()
            .map(|&kind| (kind, read_section(doc, kind)))
            .collect()
    }

    fn write_section(&mut self, kind: Kind, rows: &[Record]) -> Result<Vec<Record>> {
        if self.doc.is_none() {
            self.read()?;
        }
        let doc = self.doc()?;
        let text = section_edit(doc, kind, rows)?;
        let updated = dts::parse(&text)?;
        let fresh = read_section(&updated, kind);
        self.save(&text)?;
        self.doc = Some(updated);
        Ok(fresh)
    }
}

impl FileIo {
    fn answer(&mut self, ask: &Ask) -> Result<Reply> {
        Ok(match *ask {
            Ask::KeyboardId => {
                self.read()?;
                Reply::Id {
                    version: 4,
                    uid: [0; 8],
                }
            }
            // ZMK has Caps Word and Key Repeat on every board.
            Ask::Entries => Reply::Entries {
                tap_dance: 0,
                combo: 0,
                alt_repeat: 1,
                flags: 1,
            },
            Ask::DefinitionSize => Reply::DefSize(
                u32::try_from(self.compressed.len())
                    .map_err(|_| Error::Protocol("layout is too large"))?,
            ),
            Ask::Definition { block } => {
                let start = (block as usize * 32).min(self.compressed.len());
                let end = (start + 32).min(self.compressed.len());
                Reply::DefBlock(self.compressed[start..end].to_vec())
            }
            Ask::LayoutOptions => Reply::LayoutOptions(0),
            Ask::LayerCount => Reply::LayerCount(
                u8::try_from(layers(self.doc()?).len())
                    .map_err(|_| Error::Protocol("layer count"))?,
            ),
            Ask::MacroCount => Reply::MacroCount(0),
            Ask::KeymapChunk { offset, len } => {
                let doc = self.doc()?;
                let codes: Vec<u16> = layers(doc)
                    .iter()
                    .flat_map(|layer| binding_texts(doc, layer.prop("bindings")))
                    .map(|(_, tokens)| self.code_of(doc, &tokens).unwrap_or(0))
                    .collect();
                let bytes = keymap_bytes(&codes);
                let start = usize::from(offset).min(bytes.len());
                let end = (start + usize::from(len)).min(bytes.len());
                let mut chunk = bytes[start..end].to_vec();
                chunk.resize(usize::from(len), 0);
                Reply::Chunk(chunk)
            }
            Ask::SetKey {
                layer, col, code, ..
            } => {
                self.set(layer, col, code)?;
                Reply::Ack
            }
            // A file has no lock.
            Ask::UnlockStatus => Reply::Unlock {
                unlocked: true,
                pairs: Vec::new(),
            },
            Ask::UnlockStart => Reply::Ack,
            Ask::UnlockPoll => Reply::Polled {
                unlocked: true,
                left: 0,
            },
            _ => return Err(Error::Protocol("a keymap file has no Vial entries")),
        })
    }

    fn doc(&self) -> Result<&Doc> {
        self.doc
            .as_ref()
            .ok_or(Error::Protocol("the keymap is not read"))
    }

    /// The file, its layers, the custom keycodes, and the definition drawn from the layout.
    fn read(&mut self) -> Result<()> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|_| Error::Protocol("could not read the keymap file"))?;
        let doc = dts::parse(&text)?;
        let layer_nodes = layers(&doc);
        let keys = layer_nodes
            .first()
            .map(|layer| binding_texts(&doc, layer.prop("bindings")).len())
            .unwrap_or(0);
        if keys == 0 || keys > usize::from(u8::MAX) {
            return Err(Error::NoLayout);
        }
        if layer_nodes
            .iter()
            .any(|layer| binding_texts(&doc, layer.prop("bindings")).len() != keys)
        {
            return Err(Error::Protocol("the keymap's layers differ in length"));
        }

        self.custom.clear();
        let mut texts: Vec<String> = layer_nodes
            .iter()
            .flat_map(|layer| binding_texts(&doc, layer.prop("bindings")))
            .filter(|(_, tokens)| native(&doc, tokens).is_none())
            .map(|(_, tokens)| tokens.join(" "))
            .collect();
        texts.extend(EXTRAS.iter().map(|text| text.to_string()));
        // Behaviors and macros of this file that take no parameters.
        for section in ["behaviors", "macros"] {
            let Some(section) = doc.node(section) else {
                continue;
            };
            for node in &section.children {
                let cells = node
                    .prop("#binding-cells")
                    .map(|prop| prop.cells().first().map(|t| t.text.clone()));
                if let (Some(label), Some(Some(cells))) = (&node.label, cells) {
                    if cells == "0" {
                        texts.push(format!("&{label}"));
                    }
                }
            }
        }
        for text in texts {
            if !self.custom.contains(&text) && self.custom.len() < CUSTOM_ROOM {
                self.custom.push(text);
            }
        }

        let rows = match layout_rows(&self.path, keys) {
            Some(rows) => rows,
            None => grid_rows(&doc, layer_nodes[0], keys),
        };
        let custom: Vec<_> = self
            .custom
            .iter()
            .map(|text| serde_json::json!({ "name": custom_name(text) }))
            .collect();
        let json = serde_json::json!({
            "matrix": { "rows": 1, "cols": keys },
            "layouts": { "keymap": rows },
            "customKeycodes": custom,
        });
        self.compressed = compress(json.to_string().as_bytes())?;
        self.doc = Some(doc);
        Ok(())
    }

    /// The keycode a binding shows as: its QMK keycode, or its custom keycode.
    fn code_of(&self, doc: &Doc, tokens: &[String]) -> Option<u16> {
        if let Some(code) = native(doc, tokens) {
            return Some(code);
        }
        let text = tokens.join(" ");
        self.custom
            .iter()
            .position(|known| *known == text)
            .map(|index| CUSTOM + index as u16)
    }

    /// The text a keycode is written as.
    fn text_of(&self, code: u16) -> Option<String> {
        if (CUSTOM..CUSTOM + CUSTOM_ROOM as u16).contains(&code) {
            return self.custom.get(usize::from(code - CUSTOM)).cloned();
        }
        let count = self.doc.as_ref().map_or(0, |doc| layers(doc).len());
        let (display, p1, p2) = zmk::qmk_binding(code, |index| {
            (usize::from(index) < count).then_some(u32::from(index))
        })?;
        let &(label, ..) = LABELS.iter().find(|(_, name, _)| *name == display)?;
        let key = |value: u32| zmk_keys::name(value);
        let params = match label {
            "kp" | "sk" => vec![key(p1)?],
            "mt" => vec![key(p1)?, key(p2)?],
            "lt" => vec![p1.to_string(), key(p2)?],
            "mo" | "tog" | "to" | "sl" => vec![p1.to_string()],
            "mkp" => vec![BUTTONS.iter().find(|b| b.1 == p1)?.0.to_string()],
            _ => Vec::new(),
        };
        Some(
            std::iter::once(format!("&{label}"))
                .chain(params)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    /// Writes one key: the binding's bytes, padded to its old width so the grid stays aligned
    /// unless it is the last on its line.
    fn set(&mut self, layer: u8, position: u8, code: u16) -> Result<()> {
        let text = self
            .text_of(code)
            .ok_or(Error::Protocol("a keymap file has no binding for that key"))?;
        let doc = self.doc()?;
        let node = *layers(doc)
            .get(usize::from(layer))
            .ok_or(Error::Protocol("layer"))?;
        let spans = binding_texts(doc, node.prop("bindings"));
        let (range, _) = spans
            .get(usize::from(position))
            .ok_or(Error::Protocol("key position"))?;
        let at = usize::from(position);
        let group: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter(|(_, (span, _))| span == range)
            .map(|(index, _)| index)
            .collect();
        // A `#define` invocation is one span for every binding it expanded to. Editing one
        // unfolds the define; the others keep the text the expansion produced.
        if group.len() > 1 {
            let joined = group
                .iter()
                .map(|&index| {
                    if index == at {
                        text.clone()
                    } else {
                        spans[index].1.join(" ")
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");
            let updated = dts::apply(
                &doc.text,
                vec![Edit {
                    range: range.clone(),
                    text: padded(&doc.text, range, joined),
                }],
            );
            self.save(&updated)?;
            self.doc = Some(dts::parse(&updated)?);
            return Ok(());
        }
        let edits = if text.chars().count() <= doc.text[range.clone()].chars().count() {
            vec![Edit {
                range: range.clone(),
                text: padded(&doc.text, range, text),
            }]
        } else {
            // Longer than the slot it had: lay the layer's grid out again around it. A
            // `#define` invocation is one cell of the grid, however many keys it stands for.
            let mut ranges: Vec<Range<usize>> = Vec::new();
            let mut texts: Vec<String> = Vec::new();
            for (index, (span, _)) in spans.iter().enumerate() {
                if ranges.last() == Some(span) {
                    continue;
                }
                ranges.push(span.clone());
                texts.push(if index == at {
                    text.clone()
                } else {
                    doc.text[span.clone()].to_string()
                });
            }
            realign(&doc.text, &ranges, &texts)
        };
        let updated = dts::apply(&doc.text, edits);
        self.save(&updated)?;
        self.doc = Some(dts::parse(&updated)?);
        Ok(())
    }

    /// Writes the whole file and forgets the parse, so the next load reads it again.
    fn save(&mut self, text: &str) -> Result<()> {
        std::fs::write(&self.path, text)
            .map_err(|_| Error::Protocol("could not write the keymap file"))?;
        self.doc = None;
        Ok(())
    }
}

/// `text` widened with spaces to the width it replaces, unless only a line end follows.
fn padded(source: &str, range: &Range<usize>, text: String) -> String {
    let old = source[range.clone()].chars().count();
    let ends_line = source[range.end..]
        .chars()
        .next()
        .is_none_or(|next| next == '\n' || next == '\r');
    if ends_line || text.chars().count() >= old {
        text
    } else {
        format!("{text:<old$}")
    }
}

/// Edits that lay bindings out as a grid again with `texts` in place of what `spans` hold.
/// Each column starts where it did, or further right when the column before it grew; a line
/// is rewritten from its indent to its last binding, so comments after it stay.
fn realign(source: &str, spans: &[Range<usize>], texts: &[String]) -> Vec<Edit> {
    let line_start = |at: usize| source[..at].rfind('\n').map_or(0, |i| i + 1);
    let column = |at: usize| source[line_start(at)..at].chars().count();
    // Columns are the distinct start columns the bindings had.
    let mut starts: Vec<usize> = spans.iter().map(|span| column(span.start)).collect();
    starts.sort_unstable();
    starts.dedup();
    let slot_of = |at: usize| starts.iter().position(|&s| s == column(at)).unwrap_or(0);
    let mut widths = vec![0usize; starts.len()];
    for (span, text) in spans.iter().zip(texts) {
        let slot = slot_of(span.start);
        widths[slot] = widths[slot].max(text.chars().count());
    }
    let mut placed = starts.clone();
    for slot in 1..placed.len() {
        placed[slot] = placed[slot].max(placed[slot - 1] + widths[slot - 1] + 2);
    }
    let mut edits = Vec::new();
    let mut index = 0;
    while index < spans.len() {
        let line = line_start(spans[index].start);
        let mut end = index;
        while end + 1 < spans.len() && line_start(spans[end + 1].start) == line {
            end += 1;
        }
        let indent_is_blank = source[line..spans[index].start].trim().is_empty();
        let from = if indent_is_blank {
            line
        } else {
            spans[index].start
        };
        let mut out = String::new();
        let mut at = if indent_is_blank {
            0
        } else {
            column(spans[index].start)
        };
        for i in index..=end {
            let target = placed[slot_of(spans[i].start)];
            if target > at {
                out.push_str(&" ".repeat(target - at));
                at = target;
            } else if i > index {
                out.push(' ');
                at += 1;
            }
            out.push_str(&texts[i]);
            at += texts[i].chars().count();
        }
        edits.push(Edit {
            range: from..spans[end].end,
            text: out,
        });
        index = end + 1;
    }
    edits
}

/// The layer nodes of `keymap`, in order: every child with a `bindings` property.
fn layers(doc: &Doc) -> Vec<&Node> {
    doc.node("keymap")
        .map(|keymap| {
            keymap
                .children
                .iter()
                .filter(|node| node.prop("bindings").is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// Each binding of a property: its span and its tokens. A `#define` invocation expands
/// here, and every binding it produced keeps the invocation's span, so an edit of one
/// unfolds the define and an untouched one leaves the file as it was.
fn binding_texts(doc: &Doc, prop: Option<&Prop>) -> Vec<(Range<usize>, Vec<String>)> {
    let Some(prop) = prop else {
        return Vec::new();
    };
    let tokens = prop.cells();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(expanded) = expand_call(doc, &tokens[i].text, 0) {
            for words in expanded {
                out.push((tokens[i].span.clone(), words));
            }
            i += 1;
            continue;
        }
        if tokens[i].text.starts_with('&') {
            let start = tokens[i].span.start;
            let mut end = tokens[i].span.end;
            let mut words = vec![tokens[i].text.clone()];
            i += 1;
            while i < tokens.len()
                && !tokens[i].text.starts_with('&')
                && expand_call(doc, &tokens[i].text, 0).is_none()
            {
                words.push(tokens[i].text.clone());
                end = tokens[i].span.end;
                i += 1;
            }
            out.push((start..end, words));
        } else {
            i += 1;
        }
    }
    out
}

/// The bindings a `#define` invocation stands for, or `None` when `text` is not one.
fn expand_call(doc: &Doc, text: &str, depth: usize) -> Option<Vec<Vec<String>>> {
    if depth > 4 {
        return None;
    }
    let (name, args) = call_of(text)?;
    let define = doc.defines.iter().find(|define| define.name == name)?;
    let body = match (&define.params, &args) {
        (None, None) => define.body.clone(),
        (Some(params), Some(args)) if params.len() == args.len() => {
            substitute(&define.body, params, args)
        }
        _ => return None,
    };
    let mut words = split_words(&body);
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if let Some(nested) = expand_call(doc, &words[i], depth + 1) {
            out.extend(nested);
            i += 1;
            continue;
        }
        if words[i].starts_with('&') {
            let mut binding = vec![words[i].clone()];
            i += 1;
            while i < words.len()
                && !words[i].starts_with('&')
                && expand_call(doc, &words[i], depth + 1).is_none()
            {
                binding.push(std::mem::take(&mut words[i]));
                i += 1;
            }
            out.push(binding);
        } else {
            i += 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

fn call_of(text: &str) -> Option<(&str, Option<Vec<String>>)> {
    let name_end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    if name_end == 0 {
        return None;
    }
    let name = &text[..name_end];
    let rest = text[name_end..].trim();
    if rest.is_empty() {
        return Some((name, None));
    }
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    Some((name, Some(split_args(inner))))
}

fn split_args(inner: &str) -> Vec<String> {
    let mut args = Vec::new();
    let (mut start, mut depth) = (0, 0i32);
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    args.push(inner[start..].trim().to_string());
    args
}

fn substitute(body: &str, params: &[String], args: &[String]) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        out.push_str(&rest[..i]);
        let ident: String = rest[i..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let len = ident.len();
        if let Some(at) = params.iter().position(|param| param == &ident) {
            out.push_str(&args[at]);
        } else {
            out.push_str(&ident);
        }
        rest = &rest[i + len..];
    }
    out.push_str(rest);
    out
}

fn split_words(body: &str) -> Vec<String> {
    word_spans(body)
        .into_iter()
        .map(|span| body[span].to_string())
        .collect()
}

/// Each whitespace-separated word of `body`; parentheses keep `LC(LS(TAB))` one word.
fn word_spans(body: &str) -> Vec<Range<usize>> {
    let bytes = body.as_bytes();
    let mut words = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        let mut depth = 0i32;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                c if depth == 0 && c.is_ascii_whitespace() => break,
                _ => {}
            }
            i += 1;
        }
        words.push(start..i);
    }
    words
}

fn string(prop: &Prop) -> Option<String> {
    prop.value
        .as_ref()?
        .parts
        .iter()
        .find_map(|part| match part {
            dts::Part::Str { text, .. } => Some(text.clone()),
            _ => None,
        })
}

/// A number, or an object-like `#define` that is one.
fn number(doc: &Doc, word: &str) -> Option<u32> {
    if let Ok(value) = word.parse() {
        return Some(value);
    }
    doc.defines
        .iter()
        .find(|define| define.params.is_none() && define.name == word)
        .and_then(|define| {
            define
                .body
                .trim_matches(|c| c == '(' || c == ')')
                .parse()
                .ok()
        })
}

/// The QMK keycode a binding's tokens stand for, when it turns back into the same binding.
fn native(doc: &Doc, tokens: &[String]) -> Option<u16> {
    let (head, params) = tokens.split_first()?;
    let label = head.strip_prefix('&')?;
    let &(_, display, arity) = LABELS.iter().find(|(known, ..)| *known == label)?;
    if params.len() != arity {
        return None;
    }
    let key = |word: &String| zmk_keys::parse(word);
    let (p1, p2) = match label {
        "kp" | "sk" => (key(&params[0])?, 0),
        "mt" => (key(&params[0])?, key(&params[1])?),
        "lt" => (number(doc, &params[0])?, key(&params[1])?),
        "mo" | "tog" | "to" | "sl" => (number(doc, &params[0])?, 0),
        "mkp" => (BUTTONS.iter().find(|b| b.0 == params[0])?.1, 0),
        _ => (0, 0),
    };
    let count = layers(doc).len();
    let code = zmk::qmk_code(display, p1, p2, |layer| {
        u16::try_from(layer)
            .ok()
            .filter(|l| usize::from(*l) < count)
    })?;
    let back = zmk::qmk_binding(code, |index| Some(u32::from(index)))?;
    (back == (display, p1, p2)).then_some(code)
}

/// What a parameter takes, as Keymap Editor's behavior list names them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
    /// A key: `A`, `LC(TAB)`.
    Key,
    /// A modifier key, for `&mt`.
    Mod,
    Layer,
    /// One of a behavior's commands; some take a number after.
    Command(&'static [(&'static str, usize)]),
    /// A Bluetooth profile.
    Profile,
    Button,
    /// Anything; the key names are offered.
    Any,
}

const BT: &[(&str, usize)] = &[
    ("BT_SEL", 1),
    ("BT_DISC", 1),
    ("BT_NXT", 0),
    ("BT_PRV", 0),
    ("BT_CLR", 0),
    ("BT_CLR_ALL", 0),
];
const OUT: &[(&str, usize)] = &[
    ("OUT_USB", 0),
    ("OUT_BLE", 0),
    ("OUT_TOG", 0),
    ("OUT_NONE", 0),
];
const EXT_POWER: &[(&str, usize)] = &[("EP_ON", 0), ("EP_OFF", 0), ("EP_TOG", 0)];
const RGB: &[(&str, usize)] = &[
    ("RGB_TOG", 0),
    ("RGB_ON", 0),
    ("RGB_OFF", 0),
    ("RGB_HUI", 0),
    ("RGB_HUD", 0),
    ("RGB_SAI", 0),
    ("RGB_SAD", 0),
    ("RGB_BRI", 0),
    ("RGB_BRD", 0),
    ("RGB_SPI", 0),
    ("RGB_SPD", 0),
    ("RGB_EFF", 0),
    ("RGB_EFR", 0),
];
const BACKLIGHT: &[(&str, usize)] = &[
    ("BL_ON", 0),
    ("BL_OFF", 0),
    ("BL_TOG", 0),
    ("BL_INC", 0),
    ("BL_DEC", 0),
    ("BL_CYCLE", 0),
    ("BL_SET", 1),
];
const MOVE: &[(&str, usize)] = &[
    ("MOVE_UP", 0),
    ("MOVE_DOWN", 0),
    ("MOVE_LEFT", 0),
    ("MOVE_RIGHT", 0),
];
const SCROLL: &[(&str, usize)] = &[
    ("SCRL_UP", 0),
    ("SCRL_DOWN", 0),
    ("SCRL_LEFT", 0),
    ("SCRL_RIGHT", 0),
];

/// ZMK's own behaviors, as Keymap Editor lists them: label, what it is, and what each
/// parameter takes.
const BEHAVIORS: [(&str, &str, &[Param]); 27] = [
    ("kp", "key press", &[Param::Key]),
    ("mt", "mod-tap", &[Param::Mod, Param::Key]),
    ("lt", "layer-tap", &[Param::Layer, Param::Key]),
    ("mo", "momentary layer", &[Param::Layer]),
    ("tog", "toggle layer", &[Param::Layer]),
    ("to", "to layer", &[Param::Layer]),
    ("sl", "sticky layer", &[Param::Layer]),
    ("sk", "sticky key", &[Param::Key]),
    ("kt", "key toggle", &[Param::Key]),
    ("mkp", "mouse button", &[Param::Button]),
    ("mmv", "mouse move", &[Param::Command(MOVE)]),
    ("msc", "mouse scroll", &[Param::Command(SCROLL)]),
    ("bt", "bluetooth", &[Param::Command(BT)]),
    ("out", "output selection", &[Param::Command(OUT)]),
    ("rgb_ug", "underglow", &[Param::Command(RGB)]),
    ("ext_power", "external power", &[Param::Command(EXT_POWER)]),
    ("bl", "backlight", &[Param::Command(BACKLIGHT)]),
    ("trans", "transparent", &[]),
    ("none", "nothing", &[]),
    ("bootloader", "bootloader", &[]),
    ("sys_reset", "reset", &[]),
    ("soft_off", "soft off", &[]),
    ("caps_word", "caps word", &[]),
    ("key_repeat", "key repeat", &[]),
    ("gresc", "grave escape", &[]),
    ("studio_unlock", "studio unlock", &[]),
    ("macro_param_1to1", "macro parameter", &[]),
];

/// The hits for a binding being typed, a title saying what they are, and whether the typed
/// text already has every parameter its behavior takes.
pub(super) type Completion = (String, Vec<(String, String)>, bool);

/// The behaviors this file can bind: ZMK's own, then its behaviors and macros, each with what
/// its parameters take. A hold-tap's parameters take what its two bindings take.
fn behaviors(doc: &Doc) -> Vec<(String, String, Vec<Param>)> {
    let mut out: Vec<(String, String, Vec<Param>)> = BEHAVIORS
        .iter()
        .map(|(label, about, params)| (label.to_string(), about.to_string(), params.to_vec()))
        .collect();
    for section in ["behaviors", "macros"] {
        let Some(section) = doc.node(section) else {
            continue;
        };
        for node in &section.children {
            let Some(label) = &node.label else {
                continue;
            };
            let cells: usize = node
                .prop("#binding-cells")
                .and_then(|prop| prop.cells().first().and_then(|t| t.text.parse().ok()))
                .unwrap_or(0);
            let refs: Vec<String> = node
                .prop("bindings")
                .map(|prop| {
                    binding_texts(doc, Some(prop))
                        .into_iter()
                        .map(|(_, tokens)| tokens.join(" "))
                        .collect()
                })
                .unwrap_or_default();
            let params = (0..cells)
                .map(|i| match refs.get(i).map(String::as_str) {
                    Some("&mo" | "&tog" | "&to" | "&sl") => Param::Layer,
                    Some("&kp" | "&sk") => Param::Key,
                    _ => Param::Any,
                })
                .collect();
            out.retain(|(known, ..)| known != label);
            out.push((
                label.clone(),
                format!("{} {}", section.name, node.name),
                params,
            ));
        }
    }
    out
}

/// What may come next in `typed`, a binding being typed after `&`: first the behaviors, then
/// the values each parameter takes. Values that start with what is typed come first.
fn complete(doc: &Doc, typed: &str) -> Completion {
    let words: Vec<&str> = typed.split_whitespace().collect();
    let (done, partial) = if typed.ends_with(' ') || words.is_empty() {
        (&words[..], "")
    } else {
        (&words[..words.len() - 1], words[words.len() - 1])
    };
    let known = behaviors(doc);
    let rank = |items: Vec<(String, String)>, partial: &str| {
        let needle = partial.to_lowercase();
        let mut hits: Vec<(u8, (String, String))> = items
            .into_iter()
            .filter_map(|(token, about)| {
                let hay = token.to_lowercase();
                if hay == needle {
                    Some((0, (token, about)))
                } else if hay.starts_with(&needle) {
                    Some((1, (token, about)))
                } else if hay.contains(&needle) || about.to_lowercase().contains(&needle) {
                    Some((2, (token, about)))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by_key(|(rank, _)| *rank);
        hits.into_iter()
            .map(|(_, hit)| hit)
            .take(300)
            .collect::<Vec<_>>()
    };
    let Some(head) = done.first() else {
        let items = known
            .iter()
            .map(|(label, about, _)| (format!("&{label}"), about.clone()))
            .collect();
        let partial = partial.trim_start_matches('&');
        let items = rank(items, &format!("&{partial}"));
        return ("behavior".to_string(), items, false);
    };
    let label = head.trim_start_matches('&');
    let Some((_, about, params)) = known.iter().find(|(known, ..)| known == label) else {
        return (format!("{label}: type its parameters"), Vec::new(), true);
    };
    // A command that takes a number adds that parameter.
    let mut params = params.clone();
    if let (Some(Param::Command(commands)), Some(command)) = (params.first(), done.get(1)) {
        if commands
            .iter()
            .any(|(name, extra)| name == command && *extra > 0)
        {
            params.push(Param::Profile);
        }
    }
    let at = done.len() - 1;
    let Some(param) = params.get(at) else {
        return (format!("&{label}"), Vec::new(), partial.is_empty());
    };
    let names = |list: &[&str]| {
        list.iter()
            .map(|n| (n.to_string(), String::new()))
            .collect()
    };
    let (what, items): (&str, Vec<(String, String)>) = match param {
        Param::Key | Param::Any => {
            let mut items: Vec<(String, String)> = zmk_keys::KEYS
                .iter()
                .map(|(name, _)| (name.to_string(), String::new()))
                .collect();
            // `LG(C)` is not a table name. If it parses, it is the hit, so Enter does not take another key.
            let typed = partial.trim();
            if typed.contains('(') && zmk_keys::parse(typed).is_some() {
                items.insert(0, (typed.to_string(), String::new()));
            }
            ("key", items)
        }
        Param::Mod => (
            "modifier",
            names(&[
                "LSHIFT", "LCTRL", "LALT", "LGUI", "RSHIFT", "RCTRL", "RALT", "RGUI",
            ]),
        ),
        Param::Layer => (
            "layer",
            layers(doc)
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    let name = node
                        .prop("display-name")
                        .and_then(string)
                        .unwrap_or_else(|| node.name.clone());
                    (index.to_string(), name)
                })
                .collect(),
        ),
        Param::Command(commands) => (
            "command",
            commands
                .iter()
                .map(|(name, _)| (name.to_string(), String::new()))
                .collect(),
        ),
        Param::Profile => ("profile", names(&["0", "1", "2", "3", "4"])),
        Param::Button => ("button", names(&["LCLK", "RCLK", "MCLK", "MB4", "MB5"])),
    };
    let title = format!("&{label} ({about}): {what} {}/{}", at + 1, params.len());
    (title, rank(items, partial), false)
}

/// The key-list name of a custom binding: its text without the `&`.
fn custom_name(text: &str) -> String {
    text.trim_start_matches('&').to_string()
}

/// KLE rows, one per key, from `<keymap>.json` or `info.json` beside the keymap: the layout
/// format Keymap Editor reads (QMK `info.json` keys: `x`, `y`, `w`, `h`, `r`, `rx`, `ry`).
fn layout_rows(keymap: &Path, keys: usize) -> Option<Vec<serde_json::Value>> {
    let dir = keymap.parent()?;
    let stem = keymap.file_stem()?.to_string_lossy();
    let text = [format!("{stem}.json"), "info.json".to_string()]
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let layout = json
        .get("layouts")?
        .as_object()?
        .values()
        .next()?
        .get("layout")?
        .as_array()?;
    if layout.len() < keys {
        return None;
    }
    Some(
        layout
            .iter()
            .take(keys)
            .enumerate()
            .map(|(position, key)| {
                let at = |name: &str, default: f64| {
                    key.get(name)
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(default)
                };
                let (rx, ry) = (at("rx", 0.0), at("ry", 0.0));
                serde_json::json!([
                    {
                        "r": at("r", 0.0), "rx": rx, "ry": ry,
                        "x": at("x", 0.0) - rx, "y": at("y", 0.0) - ry,
                        "w": at("w", 1.0), "h": at("h", 1.0),
                    },
                    format!("0,{position}"),
                ])
            })
            .collect(),
    )
}

/// Without a layout file: the grid the first layer is written in, one row per line, shorter
/// lines centered under the longest.
fn grid_rows(doc: &Doc, layer: &Node, keys: usize) -> Vec<serde_json::Value> {
    let spans = binding_texts(doc, layer.prop("bindings"));
    let line_of = |at: usize| doc.text[..at].matches('\n').count();
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut last = None;
    for (position, (span, _)) in spans.iter().enumerate().take(keys) {
        let line = line_of(span.start);
        if last != Some(line) {
            lines.push(Vec::new());
            last = Some(line);
        }
        lines.last_mut().expect("pushed above").push(position);
    }
    let widest = lines.iter().map(Vec::len).max().unwrap_or(0);
    let mut rows = Vec::new();
    for (y, line) in lines.iter().enumerate() {
        let offset = (widest - line.len()) as f64 / 2.0;
        for (x, position) in line.iter().enumerate() {
            rows.push(serde_json::json!([
                { "rx": 0, "ry": 0, "x": offset + x as f64, "y": y },
                format!("0,{position}"),
            ]));
        }
    }
    rows
}

/// The file's text after a layer change. Layer numbers in bindings, combos, and conditional
/// layers follow the layers they meant.
fn layer_edit(doc: &Doc, op: &LayerOp) -> Result<String> {
    let nodes = layers(doc);
    let count = nodes.len();
    let at = |index: u8| {
        nodes
            .get(usize::from(index))
            .copied()
            .ok_or(Error::Protocol("layer"))
    };
    let mut edits = Vec::new();
    match op {
        LayerOp::Rename(index, name) => {
            let node = at(*index)?;
            let quoted = name.replace('"', "'");
            match node.prop("display-name") {
                Some(prop) => edits.push(Edit {
                    range: prop.span.clone(),
                    text: format!("display-name = \"{quoted}\";"),
                }),
                None => {
                    let line_end = doc.text[node.body.start..]
                        .find('\n')
                        .map_or(node.body.start, |i| node.body.start + i + 1);
                    edits.push(Edit {
                        range: line_end..line_end,
                        text: format!("{}display-name = \"{quoted}\";\n", child_indent(doc, node)),
                    });
                }
            }
        }
        LayerOp::Add => {
            let last = *nodes.last().ok_or(Error::NoLayout)?;
            let number = count;
            let mut name = format!("layer_{number}");
            while nodes.iter().any(|node| node.name == name) {
                name.push('_');
            }
            let copy = blank_layer(doc, last, &name, &format!("Layer {number}"));
            let end = last.line_span.end.max(last.span.end);
            let lead = if doc.text[..end].ends_with('\n') {
                ""
            } else {
                "\n"
            };
            edits.push(Edit {
                range: end..end,
                text: format!("{lead}\n{}{copy}\n", last.indent),
            });
        }
        LayerOp::Remove(index) => {
            at(*index)?;
            if count == 1 {
                return Err(Error::Protocol("the keymap needs one layer"));
            }
            if references(doc)
                .iter()
                .any(|(_, layer)| *layer == u32::from(*index))
            {
                return Err(Error::Protocol(
                    "bindings still use that layer: change them first",
                ));
            }
            let index = u32::from(*index);
            let renumbered = dts::parse(&dts::apply(
                &doc.text,
                renumber(doc, |layer| if layer > index { layer - 1 } else { layer }),
            ))?;
            let node = layers(&renumbered)[index as usize];
            // The blank line before it goes too, as `:layer-add` put one there.
            let mut start = node.line_span.start;
            if renumbered.text[..start].ends_with("\n\n") {
                start -= 1;
            }
            let edit = Edit {
                range: start..node.line_span.end,
                text: String::new(),
            };
            return Ok(dts::apply(&renumbered.text, vec![edit]));
        }
        // Studio moves a layer; the commands here only ever move one place, which is a swap.
        LayerOp::Move { from, to } => {
            at(*from)?;
            at(*to)?;
            if from.abs_diff(*to) != 1 {
                return Err(Error::Protocol("a layer moves one place at a time"));
            }
            let (from, to) = (u32::from(*from), u32::from(*to));
            let renumbered = dts::parse(&dts::apply(
                &doc.text,
                renumber(doc, |layer| match layer {
                    l if l == from => to,
                    l if l == to => from,
                    l => l,
                }),
            ))?;
            let nodes = layers(&renumbered);
            let (first, second) = (nodes[from.min(to) as usize], nodes[from.max(to) as usize]);
            let swap = vec![
                Edit {
                    range: first.span.clone(),
                    text: renumbered.text[second.span.clone()].to_string(),
                },
                Edit {
                    range: second.span.clone(),
                    text: renumbered.text[first.span.clone()].to_string(),
                },
            ];
            return Ok(dts::apply(&renumbered.text, swap));
        }
    }
    Ok(dts::apply(&doc.text, edits))
}

/// The whitespace before a node's first property, or the node's own plus four spaces.
fn child_indent(doc: &Doc, node: &Node) -> String {
    node.props
        .first()
        .map(|prop| {
            let line = doc.text[..prop.span.start].rfind('\n').map_or(0, |i| i + 1);
            doc.text[line..prop.span.start].to_string()
        })
        .filter(|indent| indent.trim().is_empty())
        .unwrap_or_else(|| format!("{}    ", node.indent))
}

/// A copy of `layer` named `name`, every binding `&trans` in the same grid.
fn blank_layer(doc: &Doc, layer: &Node, name: &str, display: &str) -> String {
    let base = layer.span.start;
    let mut edits = Vec::new();
    // The name, without the label: labels are unique.
    let head_end = doc.text[base..layer.body.start]
        .find('{')
        .map_or(base, |i| base + i);
    edits.push(Edit {
        range: base..head_end,
        text: format!("{name} "),
    });
    for prop in &layer.props {
        match prop.name.as_str() {
            "bindings" | "sensor-bindings" => {
                let spans = binding_texts(doc, Some(prop));
                let mut seen: Vec<Range<usize>> = Vec::new();
                for (span, _) in &spans {
                    if seen.iter().any(|known| known == span) {
                        continue;
                    }
                    let count = spans.iter().filter(|(other, _)| other == span).count();
                    let text = if count == 1 {
                        "&trans".to_string()
                    } else {
                        vec!["&trans"; count].join("  ")
                    };
                    edits.push(Edit {
                        text: padded(&doc.text, span, text),
                        range: span.clone(),
                    });
                    seen.push(span.clone());
                }
            }
            "display-name" | "label" => edits.push(Edit {
                range: prop.span.clone(),
                text: format!("{} = \"{display}\";", prop.name),
            }),
            _ => {}
        }
    }
    let shifted = edits
        .into_iter()
        .map(|edit| Edit {
            range: edit.range.start - base..edit.range.end - base,
            text: edit.text,
        })
        .collect();
    dts::apply(&doc.text[layer.span.clone()], shifted)
}

/// Every place a layer number is written: the first parameter of layer behaviors (and of
/// hold-taps that hold a layer), in bindings and in `#define` bodies, combo `layers`, and
/// conditional `if-layers` / `then-layer`. Each is the token's span and its number;
/// `#define` names and macro parameters are left alone.
fn references(doc: &Doc) -> Vec<(Range<usize>, u32)> {
    let mut layer_labels: Vec<String> = LAYER_LABELS.iter().map(|l| l.to_string()).collect();
    if let Some(behaviors) = doc.node("behaviors") {
        for node in &behaviors.children {
            let holds_layer = node
                .prop("bindings")
                .and_then(|prop| prop.cells().first().map(|t| t.text.clone()))
                .is_some_and(|first| {
                    LAYER_LABELS
                        .iter()
                        .any(|label| first == format!("&{label}"))
                });
            if let (true, Some(label)) = (holds_layer, &node.label) {
                layer_labels.push(label.clone());
            }
        }
    }
    let mut out = Vec::new();
    let mut visit = |node: &Node, out: &mut Vec<(Range<usize>, u32)>| {
        for prop in &node.props {
            match prop.name.as_str() {
                "bindings" | "sensor-bindings" => {
                    let tokens = prop.cells();
                    for span in dts::bindings(&tokens) {
                        let inside: Vec<_> = tokens
                            .iter()
                            .filter(|t| t.span.start >= span.start && t.span.end <= span.end)
                            .collect();
                        let is_layer = inside.first().is_some_and(|head| {
                            layer_labels.iter().any(|l| head.text == format!("&{l}"))
                        });
                        if let (true, Some(param)) = (is_layer, inside.get(1)) {
                            if let Ok(layer) = param.text.parse() {
                                out.push((param.span.clone(), layer));
                            }
                        }
                    }
                }
                "layers" | "if-layers" | "then-layer" => {
                    for token in prop.cells() {
                        if let Ok(layer) = token.text.parse() {
                            out.push((token.span.clone(), layer));
                        }
                    }
                }
                _ => {}
            }
        }
    };
    fn walk(
        node: &Node,
        out: &mut Vec<(Range<usize>, u32)>,
        visit: &mut impl FnMut(&Node, &mut Vec<(Range<usize>, u32)>),
    ) {
        visit(node, out);
        for child in &node.children {
            walk(child, out, visit);
        }
    }
    for node in &doc.nodes {
        walk(node, &mut out, &mut visit);
    }
    // `#define NAV &mo 2`: the same rule inside a define body, over continued lines too.
    for define in &doc.defines {
        let Some(body) = &define.body_span else {
            continue;
        };
        // Comments and `\` line joins become spaces of the same length, so offsets hold.
        let source = &doc.text[body.clone()];
        let mut clean = String::with_capacity(source.len());
        for line in source.split_inclusive('\n') {
            let code = line.find("//").map_or(line, |i| &line[..i]);
            clean.push_str(&code.replace('\\', " "));
            clean.extend(std::iter::repeat_n(' ', line.len() - code.len()));
        }
        let words = word_spans(&clean);
        for (at, word) in words.iter().enumerate() {
            let head = &clean[word.clone()];
            if !layer_labels.iter().any(|l| head == format!("&{l}")) {
                continue;
            }
            let Some(param) = words.get(at + 1) else {
                continue;
            };
            let span = body.start + param.start..body.start + param.end;
            if let Ok(layer) = doc.text[span.clone()].parse() {
                out.push((span, layer));
            }
        }
    }
    out
}

/// Edits that rewrite every layer number through `map`.
fn renumber(doc: &Doc, map: impl Fn(u32) -> u32) -> Vec<Edit> {
    references(doc)
        .into_iter()
        .filter_map(|(range, layer)| {
            let to = map(layer);
            (to != layer).then(|| Edit {
                range,
                text: to.to_string(),
            })
        })
        .collect()
}

/// The rows of one section as the file has them.
fn read_section(doc: &Doc, kind: Kind) -> Vec<Record> {
    section_nodes(doc, kind)
        .into_iter()
        .enumerate()
        .map(|(id, node)| Record {
            id: Some(id),
            name: match kind {
                Kind::Encoders => node
                    .prop("display-name")
                    .and_then(string)
                    .unwrap_or_else(|| node.name.clone()),
                _ => node.label.clone().unwrap_or_else(|| node.name.clone()),
            },
            cells: kind
                .columns()
                .iter()
                .map(|column| cell_text(doc, node.prop(column.name), column.cell))
                .collect(),
            tail: kind
                .tail()
                .map(|tail| {
                    binding_texts(doc, node.prop(tail))
                        .into_iter()
                        .map(|(_, tokens)| tokens.join(" "))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The nodes a section's rows are: the section node's children, or the layers for encoders.
fn section_nodes(doc: &Doc, kind: Kind) -> Vec<&Node> {
    match kind {
        Kind::Encoders => layers(doc),
        _ => doc
            .node(kind.node())
            .map(|section| section.children.iter().collect())
            .unwrap_or_default(),
    }
}

/// A property as one cell's text; empty when the node does not have it.
fn cell_text(doc: &Doc, prop: Option<&Prop>, cell: Cell) -> String {
    let Some(prop) = prop else {
        return String::new();
    };
    match cell {
        Cell::Flag => "yes".to_string(),
        Cell::Str => string(prop).unwrap_or_default(),
        Cell::Binding => binding_texts(doc, Some(prop))
            .first()
            .map(|(_, tokens)| tokens.join(" "))
            .unwrap_or_default(),
        Cell::Cells => prop
            .cells()
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// How a property is written: `name = <28 29>;`, `name = "balanced";`, `name;`.
fn prop_line(name: &str, cell: Cell, value: &str) -> String {
    match cell {
        Cell::Flag => format!("{name};"),
        Cell::Str => format!("{name} = \"{}\";", value.replace('"', "'")),
        Cell::Binding | Cell::Cells => format!("{name} = <{value}>;"),
    }
}

/// How a binding list is written: behaviors one binding per `< >`, as ZMK's own files do;
/// macros and sensors all in one.
fn tail_line(kind: Kind, name: &str, tail: &[String]) -> String {
    if kind == Kind::Behaviors {
        let parts: Vec<String> = tail.iter().map(|binding| format!("<{binding}>")).collect();
        format!("{name} = {};", parts.join(", "))
    } else {
        format!("{name} = <{}>;", tail.join(" "))
    }
}

/// A macro's `compatible` and `#binding-cells` for the parameters its bindings take, from
/// `&macro_param_1to1` and the like.
fn macro_kind(tail: &[String]) -> (&'static str, &'static str) {
    let params = tail
        .iter()
        .filter_map(|binding| binding.strip_prefix("&macro_param_"))
        .filter_map(|rest| rest.chars().next()?.to_digit(10))
        .max()
        .unwrap_or(0);
    match params {
        0 => ("zmk,behavior-macro", "0"),
        1 => ("zmk,behavior-macro-one-param", "1"),
        _ => ("zmk,behavior-macro-two-param", "2"),
    }
}

/// The file's text with one section made to match `rows`: changed properties rewritten,
/// removed nodes deleted, new ones added at the end of the section. A label that changes is
/// changed in every binding that uses it.
fn section_edit(doc: &Doc, kind: Kind, rows: &[Record]) -> Result<String> {
    let nodes = section_nodes(doc, kind);
    let saved = read_section(doc, kind);
    let mut edits = Vec::new();
    let mut renames = Vec::new();
    for (id, (node, was)) in nodes.iter().zip(&saved).enumerate() {
        let Some(now) = rows.iter().find(|row| row.id == Some(id)) else {
            if kind.rows_fixed() {
                continue;
            }
            if kind.labelled() && used_elsewhere(doc, &was.name, node) {
                return Err(Error::Protocol(
                    "bindings still use that behavior: change them first",
                ));
            }
            edits.push(Edit {
                range: node.line_span.clone(),
                text: String::new(),
            });
            continue;
        };
        if now == was {
            continue;
        }
        if now.name != was.name && !kind.rows_fixed() {
            let head_end = doc.text[node.span.start..node.body.start]
                .find('{')
                .map_or(node.span.start, |i| node.span.start + i);
            let head = if kind.labelled() {
                renames.push((format!("&{}", was.name), format!("&{}", now.name)));
                format!("{}: {} ", now.name, node.name)
            } else {
                format!("{} ", now.name)
            };
            edits.push(Edit {
                range: node.span.start..head_end,
                text: head,
            });
        }
        for (index, column) in kind.columns().iter().enumerate() {
            if now.cells[index] != was.cells[index] {
                edits.extend(prop_edit(
                    doc,
                    node,
                    column.name,
                    column.cell,
                    &now.cells[index],
                ));
            }
        }
        if let Some(tail) = kind.tail() {
            if now.tail != was.tail {
                let line = tail_line(kind, tail, &now.tail);
                edits.extend(prop_edit_line(doc, node, tail, Some(line)));
                if kind == Kind::Macros {
                    let (compatible, cells) = macro_kind(&now.tail);
                    edits.extend(prop_edit(doc, node, "compatible", Cell::Str, compatible));
                    edits.extend(prop_edit(doc, node, "#binding-cells", Cell::Cells, cells));
                }
            }
        }
    }
    let added: Vec<&Record> = rows.iter().filter(|row| row.id.is_none()).collect();
    if !added.is_empty() {
        edits.push(new_nodes(doc, kind, &added)?);
    }
    let text = dts::apply(&doc.text, edits);
    if renames.is_empty() {
        return Ok(text);
    }
    // Labels change in a second pass, so they never overlap a rewritten property.
    let renamed = dts::parse(&text)?;
    let mut edits = Vec::new();
    walk_props(&renamed, &mut |prop| {
        for token in prop.cells() {
            if let Some((_, to)) = renames.iter().find(|(from, _)| token.text == *from) {
                edits.push(Edit {
                    range: token.span.clone(),
                    text: to.clone(),
                });
            }
        }
    });
    Ok(dts::apply(&renamed.text, edits))
}

/// Every property of every node, depth first.
fn walk_props<'a>(doc: &'a Doc, visit: &mut impl FnMut(&'a Prop)) {
    fn walk<'a>(node: &'a Node, visit: &mut impl FnMut(&'a Prop)) {
        for prop in &node.props {
            visit(prop);
        }
        for child in &node.children {
            walk(child, visit);
        }
    }
    for node in &doc.nodes {
        walk(node, visit);
    }
}

/// Whether `&label` appears in any binding outside `node`.
fn used_elsewhere(doc: &Doc, label: &str, node: &Node) -> bool {
    let reference = format!("&{label}");
    let mut used = false;
    walk_props(doc, &mut |prop| {
        let inside = prop.span.start >= node.span.start && prop.span.end <= node.span.end;
        if !inside && prop.cells().iter().any(|token| token.text == reference) {
            used = true;
        }
    });
    used
}

/// The edit that gives `node` property `name` the value `value`, removing it when empty.
fn prop_edit(doc: &Doc, node: &Node, name: &str, cell: Cell, value: &str) -> Option<Edit> {
    let line = (!value.is_empty()).then(|| prop_line(name, cell, value));
    prop_edit_line(doc, node, name, line)
}

/// Replaces, removes (`None`), or adds the property line for `name` in `node`.
fn prop_edit_line(doc: &Doc, node: &Node, name: &str, line: Option<String>) -> Option<Edit> {
    match (node.prop(name), line) {
        (Some(prop), Some(line)) => Some(Edit {
            range: prop.span.clone(),
            text: line,
        }),
        (Some(prop), None) => Some(Edit {
            range: prop.line_span.clone(),
            text: String::new(),
        }),
        (None, Some(line)) => {
            // Before the line that closes the node.
            let close = node.body.end;
            let line_start = doc.text[..close].rfind('\n').map_or(0, |i| i + 1);
            if doc.text[line_start..close].trim().is_empty() {
                Some(Edit {
                    range: line_start..line_start,
                    text: format!("{}{line}\n", child_indent(doc, node)),
                })
            } else {
                Some(Edit {
                    range: close..close,
                    text: format!(" {line} "),
                })
            }
        }
        (None, None) => None,
    }
}

/// The text of new nodes, inserted at the end of the section, or a new section before the
/// keymap when the file has none.
fn new_nodes(doc: &Doc, kind: Kind, rows: &[&Record]) -> Result<Edit> {
    if kind.rows_fixed() {
        return Err(Error::Protocol("encoder rows are the layers"));
    }
    let section = doc.node(kind.node());
    let keymap = doc.node("keymap").ok_or(Error::NoLayout)?;
    let indent = match section {
        Some(section) => section
            .children
            .first()
            .map(|child| child.indent.clone())
            .unwrap_or_else(|| format!("{}    ", section.indent)),
        None => format!("{}    ", keymap.indent),
    };
    let inner = format!("{indent}    ");
    let mut nodes = Vec::new();
    for row in rows {
        let head = if kind.labelled() {
            format!("{}: {}", row.name, row.name)
        } else {
            row.name.clone()
        };
        let mut text = format!("{indent}{head} {{\n");
        if kind == Kind::Macros {
            let (compatible, cells) = macro_kind(&row.tail);
            text.push_str(&format!("{inner}compatible = \"{compatible}\";\n"));
            text.push_str(&format!("{inner}#binding-cells = <{cells}>;\n"));
        }
        for (column, value) in kind.columns().iter().zip(&row.cells) {
            if !value.is_empty() {
                let line = prop_line(column.name, column.cell, value);
                text.push_str(&format!("{inner}{line}\n"));
            }
        }
        if let Some(tail) = kind.tail() {
            text.push_str(&format!("{inner}{}\n", tail_line(kind, tail, &row.tail)));
        }
        text.push_str(&format!("{indent}}};\n"));
        nodes.push(text);
    }
    // A blank line before each node, as ZMK's own files have.
    let body: String = nodes.iter().map(|node| format!("\n{node}")).collect();
    match section {
        Some(section) => {
            let close = section.body.end;
            let line_start = doc.text[..close].rfind('\n').map_or(0, |i| i + 1);
            let at = if doc.text[line_start..close].trim().is_empty() {
                line_start
            } else {
                close
            };
            Ok(Edit {
                range: at..at,
                text: body,
            })
        }
        None => {
            let outer = &keymap.indent;
            let compatible = kind
                .compatible()
                .map(|c| format!("{indent}compatible = \"{c}\";\n"))
                .unwrap_or_default();
            let at = doc.text[..keymap.span.start]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            Ok(Edit {
                range: at..at,
                text: format!(
                    "{outer}{} {{\n{compatible}{body}{outer}}};\n\n",
                    kind.node()
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{load, Place, Write};
    use super::*;

    const KEYMAP: &str = include_str!("../../../tests/fixtures/corne.keymap");

    fn doc() -> Doc {
        dts::parse(KEYMAP).unwrap()
    }

    /// A fresh copy of the fixture in its own folder, so tests do not share a file.
    fn copy(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazykeymap-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corne.keymap");
        std::fs::write(&path, KEYMAP).unwrap();
        path
    }

    #[test]
    fn a_key_write_changes_only_that_binding() {
        let path = copy("key");
        let mut board = load(candidates(&path).unwrap().remove(0)).unwrap();
        let place = Place::Matrix { row: 0, col: 1 };
        // `&thm LSHIFT A` has no QMK keycode: it reads as a custom one named by its text.
        assert_eq!(
            board.legend(board.code_at(0, Place::Matrix { row: 0, col: 13 }).unwrap()),
            "thm LSHIFT A"
        );
        board
            .commit(&Write::Key {
                layer: 0,
                place,
                code: 0x001D,
            })
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, KEYMAP.replacen("&kp Q    ", "&kp Z    ", 1));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn every_binding_in_the_fixture_loads() {
        let path = copy("all");
        let board = load(candidates(&path).unwrap().remove(0)).unwrap();
        let doc = doc();
        let mut missing = Vec::new();
        for (layer, node) in layers(&doc).iter().enumerate() {
            for (col, (_, tokens)) in binding_texts(&doc, node.prop("bindings"))
                .into_iter()
                .enumerate()
            {
                let text = tokens.join(" ");
                let code = board
                    .code_at(
                        layer as u8,
                        Place::Matrix {
                            row: 0,
                            col: col as u8,
                        },
                    )
                    .unwrap();
                if code == 0 && text != "&none" {
                    missing.push(format!("L{layer}:{col} {text}"));
                }
            }
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn a_define_expands_until_one_of_its_keys_is_edited() {
        let dir = std::env::temp_dir().join(format!("lazykeymap-def-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.keymap");
        std::fs::write(
            &path,
            "#define PAIR(a, b) &kp a  &kp b\n#define ONE &kp Q\n/ {\n    keymap {\n        compatible = \"zmk,keymap\";\n        default_layer {\n            bindings = <PAIR(A, B) ONE>;\n        };\n    };\n};\n",
        )
        .unwrap();
        let mut board = load(candidates(&path).unwrap().remove(0)).unwrap();
        assert_eq!(
            board.code_at(0, Place::Matrix { row: 0, col: 0 }).unwrap(),
            0x0004
        );
        assert_eq!(
            board.code_at(0, Place::Matrix { row: 0, col: 1 }).unwrap(),
            0x0005
        );
        assert_eq!(
            board.code_at(0, Place::Matrix { row: 0, col: 2 }).unwrap(),
            0x0014
        );
        board
            .commit(&Write::Key {
                layer: 0,
                place: Place::Matrix { row: 0, col: 2 },
                code: 0x001D,
            })
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("PAIR(A, B)"), "{written}");
        board
            .commit(&Write::Key {
                layer: 0,
                place: Place::Matrix { row: 0, col: 0 },
                code: 0x001D,
            })
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("bindings = <&kp Z  &kp B  &kp Z>;"),
            "{written}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_longer_binding_widens_its_column() {
        let doc = doc();
        let node = layers(&doc)[0];
        let spans: Vec<Range<usize>> = binding_texts(&doc, node.prop("bindings"))
            .into_iter()
            .map(|(span, _)| span)
            .collect();
        let mut texts: Vec<String> = spans
            .iter()
            .map(|s| doc.text[s.clone()].to_string())
            .collect();
        texts[1] = "&thm LC(LSHIFT) LC(LS(TAB))".to_string();
        let text = dts::apply(&doc.text, realign(&doc.text, &spans, &texts));
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("&kp W") || l.contains("&thm LSHIFT A"))
            .collect();
        // `W` and `A`, the next column on two lines, still start together.
        let w = lines[0].find("&kp W").unwrap();
        let a = lines[1].find("&thm LCTRL S").unwrap();
        assert_eq!(w, a, "{text}");
    }

    #[test]
    fn moving_a_layer_moves_the_numbers_that_point_at_it() {
        let text = layer_edit(&doc(), &LayerOp::Move { from: 5, to: 6 }).unwrap();
        let moved = dts::parse(&text).unwrap();
        let names: Vec<&str> = layers(&moved).iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names[5..], ["numpad", "ColmakDH"]);
        // Base reached numpad as 6 and Colemak as 5; now the other way round.
        assert!(text.contains("&lt 5 SPACE"), "{text}");
        assert!(text.contains("&tog 6"));
        assert!(!text.contains("&tog 5"));
        // Not layer numbers: the tri-layer still reads 1, 2, 3.
        assert!(text.contains("if-layers = <1 2>") && text.contains("then-layer = <3>"));
    }

    #[test]
    fn moving_a_layer_renumbers_define_bodies() {
        let source = KEYMAP.replacen(
            "#define HRML",
            "#define NUMS &tog 6  // numpad 6\n#define PAD &kp A \\\n    &tog 6 // numpad 6\n#define HRML",
            1,
        );
        let text = layer_edit(
            &dts::parse(&source).unwrap(),
            &LayerOp::Move { from: 5, to: 6 },
        )
        .unwrap();
        assert!(text.contains("#define NUMS &tog 5  // numpad 6"), "{text}");
        // Continued over a line, and the `6` in the comment is not a layer.
        assert!(
            text.contains("#define PAD &kp A \\\n    &tog 5 // numpad 6"),
            "{text}"
        );
        // `k1` in HRML is a parameter, not a layer, and the body is untouched.
        assert!(text.contains("#define HRML(k1,k2,k3,k4) &thm LCTRL k1"));
    }

    #[test]
    fn a_layer_still_in_use_stays() {
        // `&tog 4` on two layers points at the mouse layer.
        assert!(layer_edit(&doc(), &LayerOp::Remove(4)).is_err());
        let added = layer_edit(&doc(), &LayerOp::Add).unwrap();
        let removed = layer_edit(&dts::parse(&added).unwrap(), &LayerOp::Remove(7)).unwrap();
        assert_eq!(removed, KEYMAP);
    }

    #[test]
    fn sections_change_only_the_rows_that_changed() {
        let doc = doc();
        let mut rows = read_section(&doc, Kind::Combos);
        assert_eq!(rows.len(), 18);
        assert_eq!(rows[4].cells[0], "&kp LC(LS(TAB))");
        rows[0].cells[0] = "&kp B".to_string();
        rows[0].cells[5] = "yes".to_string();
        rows.retain(|row| row.name != "combo_esc");
        let mut new = Kind::Combos.blank("combo_new");
        new.cells[1] = "1 2".to_string();
        rows.push(new);
        let text = section_edit(&doc, Kind::Combos, &rows).unwrap();
        let after = read_section(&dts::parse(&text).unwrap(), Kind::Combos);
        assert_eq!(after.len(), 18);
        assert_eq!(after[0].cells[0], "&kp B");
        assert_eq!(after[0].cells[5], "yes");
        assert!(after.iter().all(|row| row.name != "combo_esc"));
        let last = after.last().unwrap();
        assert_eq!(
            (last.name.as_str(), last.cells[1].as_str()),
            ("combo_new", "1 2")
        );
        // The keymap and every other section are untouched.
        let start = KEYMAP.find("    macros {").unwrap();
        assert!(text.contains(&KEYMAP[start..]));
    }

    #[test]
    fn a_macro_takes_the_parameters_its_bindings_use() {
        let doc = doc();
        let mut rows = read_section(&doc, Kind::Macros);
        let mut new = Kind::Macros.blank("pick");
        new.tail = vec![
            "&macro_param_1to1".to_string(),
            "&kp MACRO_PLACEHOLDER".to_string(),
        ];
        rows.push(new);
        let text = section_edit(&doc, Kind::Macros, &rows).unwrap();
        assert!(text.contains(
            "pick: pick {\n            compatible = \"zmk,behavior-macro-one-param\";\n            #binding-cells = <1>;"
        ), "{text}");
    }

    #[test]
    fn mouse_move_and_scroll_are_in_the_behavior_list() {
        let doc = doc();
        let (_, hits, _) = complete(&doc, "&");
        assert!(hits.iter().any(|(token, _)| token == "&mmv"));
        assert!(hits.iter().any(|(token, _)| token == "&msc"));
        let (_, hits, done) = complete(&doc, "&msc ");
        assert!(!done);
        assert!(hits.iter().any(|(token, _)| token == "SCRL_LEFT"));
        let (_, _, done) = complete(&doc, "&msc SCRL_LEFT ");
        assert!(done);
        let (_, hits, _) = complete(&doc, "&mmv ");
        assert!(hits.iter().any(|(token, _)| token == "MOVE_UP"));
        let (_, hits, _) = complete(&doc, "&kp LG(C)");
        assert_eq!(hits.first().map(|(token, _)| token.as_str()), Some("LG(C)"));
        let (_, hits, _) = complete(&doc, "&kp LC(LS(TAB))");
        assert_eq!(
            hits.first().map(|(token, _)| token.as_str()),
            Some("LC(LS(TAB))")
        );
    }

    #[test]
    fn renaming_a_macro_renames_its_uses_and_a_used_one_stays() {
        let doc = doc();
        let mut rows = read_section(&doc, Kind::Macros);
        rows[0].name = "todo_list".to_string();
        let text = section_edit(&doc, Kind::Macros, &rows).unwrap();
        assert!(text.contains("todo_list: todo {") && text.contains("bindings = <&todo_list>;"));
        assert!(!text.contains("&todo>"));

        let mut rows = read_section(&doc, Kind::Macros);
        rows.retain(|row| row.name != "hello");
        assert!(section_edit(&doc, Kind::Macros, &rows).is_err());
    }
}
