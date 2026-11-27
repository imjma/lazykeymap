//! A keymap file and the ZMK Studio keyboard running it, edited as one. The file is what the
//! screen shows; each key written goes to the keyboard first, then to the file, so a keyboard
//! that is locked or refuses the binding leaves the file as it was.

use std::cell::RefCell;

use super::{decode, decode_request, encode, file, Ask, Io, Kind, LayerOp, Record, Reply};
use crate::{Error, Result};

pub(super) struct Linked {
    file: Box<dyn Io>,
    /// In a cell so `speaks` can ask the keyboard to read a binding, which may add a keycode.
    board: RefCell<Box<dyn Io>>,
    /// File behavior labels and the keyboard's tokens for them, as `("&hm", "&homerow_mods")`:
    /// Studio names a behavior by its display name, a keymap by its node label.
    aliases: Vec<(String, String)>,
}

/// Why a file and a keyboard did not link, and both of them back.
type Unlinked = (String, Box<dyn Io>, Box<dyn Io>);

/// `file` and `board` as one, when both hold the same keymap: the same layers, the same keys,
/// and at each key a binding the keyboard reads as what it has.
pub(super) fn link(
    mut file: Box<dyn Io>,
    mut board: Box<dyn Io>,
) -> std::result::Result<Linked, Unlinked> {
    match compare(&mut *file, &mut *board) {
        Ok(aliases) => Ok(Linked {
            file,
            board: RefCell::new(board),
            aliases,
        }),
        Err(why) => Err((why, file, board)),
    }
}

fn talk(io: &mut dyn Io, ask: &Ask) -> Result<Reply> {
    decode(ask, &io.exchange(&encode(ask))?)
}

fn compare(
    file: &mut dyn Io,
    board: &mut dyn Io,
) -> std::result::Result<Vec<(String, String)>, String> {
    let read = |io: &mut dyn Io| -> Result<()> {
        while io.busy()?.is_some() {}
        // A keymap file reads itself on this ask.
        talk(io, &Ask::KeyboardId).map(drop)
    };
    read(board).map_err(|err| format!("could not read the keyboard: {err}"))?;
    read(file).map_err(|err| format!("could not read the file: {err}"))?;
    let (ours, theirs) = (file.keymap(), board.keymap());
    if ours.len() != theirs.len() {
        return Err(format!(
            "the keyboard has {} layers, the file {}",
            theirs.len(),
            ours.len()
        ));
    }
    if let Some((a, b)) = ours.iter().zip(&theirs).find(|(a, b)| a.len() != b.len()) {
        return Err(format!(
            "the keyboard has {} keys, the file {}",
            b.len(),
            a.len()
        ));
    }
    let pairs = || ours.iter().zip(&theirs).flat_map(|(a, b)| a.iter().zip(b));
    // Where the labels differ, the keyboard's is the file's under another name.
    let mut aliases: Vec<(String, String)> = Vec::new();
    for (&mine, &its) in pairs() {
        let (Some(mine), Some(its)) = (file.binding_text(mine), board.binding_text(its)) else {
            continue;
        };
        let (mine, its) = (label(&mine), label(&its));
        if mine != its && !aliases.iter().any(|(known, _)| known == mine) {
            aliases.push((mine.to_string(), its.to_string()));
        }
    }
    // Then every key, strictly: the file's binding, read by the keyboard, is what it has.
    for (layer, (a, b)) in ours.iter().zip(&theirs).enumerate() {
        for (key, (&mine, &its)) in a.iter().zip(b).enumerate() {
            let text = file.binding_text(mine);
            let read = text
                .as_deref()
                .and_then(|text| board.intern(&translate(&aliases, text)));
            if read.map(|(code, _)| code) != Some(its) {
                return Err(format!(
                    "layer {layer} key {key}: the file has {}, the keyboard {}",
                    text.as_deref().unwrap_or("a binding it cannot read"),
                    board.binding_text(its).as_deref().unwrap_or("another"),
                ));
            }
        }
    }
    Ok(aliases)
}

/// The behavior, `&` and all.
fn label(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or("")
}

/// `text` with its behavior renamed to the keyboard's token for it.
fn translate(aliases: &[(String, String)], text: &str) -> String {
    let head = label(text);
    match aliases.iter().find(|(mine, _)| mine == head) {
        Some((_, its)) => format!("{its}{}", &text[head.len()..]),
        None => text.to_string(),
    }
}

impl Linked {
    /// The keyboard's keycode for the file's `code`.
    fn board_code(&self, code: u16) -> Option<u16> {
        let text = self.file.binding_text(code)?;
        let text = translate(&self.aliases, &text);
        self.board.borrow_mut().intern(&text).map(|(code, _)| code)
    }
}

impl Io for Linked {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
        match decode_request(report)? {
            Ask::SetKey {
                layer,
                row,
                col,
                code,
            } => {
                let theirs = self.board_code(code).ok_or(Error::Protocol(
                    "the keyboard has no behavior for that binding; the file is unchanged",
                ))?;
                let set = Ask::SetKey {
                    layer,
                    row,
                    col,
                    code: theirs,
                };
                self.board.get_mut().exchange(&encode(&set))?;
                self.file.exchange(report)
            }
            // The lock is the keyboard's; a file has none.
            Ask::UnlockStatus | Ask::UnlockStart | Ask::UnlockPoll => {
                self.board.get_mut().exchange(report)
            }
            _ => self.file.exchange(report),
        }
    }

    fn speaks(&self, code: u16) -> bool {
        self.file.speaks(code) && self.board_code(code).is_some()
    }

    fn busy(&mut self) -> Result<Option<String>> {
        if let Some(what) = self.board.get_mut().busy()? {
            return Ok(Some(what));
        }
        self.file.busy()
    }

    fn layer_names(&self) -> Vec<String> {
        self.file.layer_names()
    }

    fn keymap(&self) -> Vec<Vec<u16>> {
        self.file.keymap()
    }

    fn binding_text(&self, code: u16) -> Option<String> {
        self.file.binding_text(code)
    }

    /// Combos, macros, and behaviors live in the file only: Studio cannot change them.
    fn sections(&self) -> Vec<(Kind, Vec<Record>)> {
        self.file.sections()
    }

    fn write_section(&mut self, kind: Kind, rows: &[Record]) -> Result<Vec<Record>> {
        self.file.write_section(kind, rows)
    }

    /// The keyboard first, so a refused change leaves the file as it was.
    fn edit_layers(&mut self, op: &LayerOp) -> Result<()> {
        self.board.get_mut().edit_layers(op)?;
        self.file.edit_layers(op)
    }

    fn intern(&mut self, text: &str) -> Option<(u16, String)> {
        self.file.intern(text)
    }

    fn writes_to(&self) -> &'static str {
        "the file and the keyboard"
    }

    fn complete(&self, typed: &str) -> Option<file::Completion> {
        self.file.complete(typed)
    }
}
