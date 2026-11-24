use super::Attached;
use crate::{Error, Result};
use std::cell::RefCell;
use std::io::{Read, Write as _};
use std::sync::Arc;

mod catalog;
mod entries;
mod keycodes;
pub(super) use catalog::{Listed, Range};
pub(super) use entries::{Action, Combo, Spot, Tab, TapDance, MAX_DELAY};
pub(super) use layout::{Lower, Mode, ModeTone, Popup, PopupLine};
mod layout;

use catalog::{Features, Table};
use layout::{Cap, Definition, SheetCell, Tone};

/// 32-byte message, without the HID report id.
trait Io {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]>;
}

/// Prepends `0x00` on the write and strips nothing from the 32-byte read.
struct HidIo {
    device: hidapi::HidDevice,
}

impl Io for HidIo {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
        let mut packet = [0u8; 33];
        packet[1..].copy_from_slice(report);
        let wrote = self
            .device
            .write(&packet)
            .map_err(|_| Error::Disconnected)?;
        if wrote < packet.len() {
            return Err(Error::Disconnected);
        }
        let mut resp = [0u8; 32];
        let read = self
            .device
            .read_timeout(&mut resp, 500)
            .map_err(|_| Error::Disconnected)?;
        if read == 0 {
            return Err(Error::Disconnected);
        }
        Ok(resp)
    }
}

/// Speaks `Ask` through the same encode and decode as HID.
struct FakeIo {
    store: Arc<RefCell<Store>>,
}

impl Io for FakeIo {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
        let ask = decode_request(report)?;
        let reply = self.store.borrow_mut().answer(&ask)?;
        Ok(encode_reply(&ask, &reply))
    }
}

struct Store {
    name: String,
    compressed: Vec<u8>,
    layers: u8,
    macros: u8,
    /// The VIA macro buffer, `0x0D` reports its length.
    macro_buffer: Vec<u8>,
    /// Dynamic entries. Their lengths are the counts in the dynamic-entry reply.
    tap_dances: Vec<[u16; 5]>,
    combos: Vec<[u16; 5]>,
    alt_repeat: u8,
    feature_flags: u8,
    options: u32,
    locked: bool,
    unlock_pairs: Vec<(u8, u8)>,
    unlock_armed: bool,
    /// Polls that stay locked after unlock start. Zero means the next poll is ready.
    unlock_waits: u8,
    /// Row-major codes, length `layers * rows * cols`. Filled with `KC_NO`.
    matrix: Vec<u16>,
    /// `encoders[layer][index] = (clockwise, counterclockwise)`.
    encoders: Vec<Vec<(u16, u16)>>,
    rows: u8,
    cols: u8,
    encoder_count: usize,
    open: bool,
}

impl Store {
    fn answer(&mut self, ask: &Ask) -> Result<Reply> {
        match *ask {
            Ask::LayerCount => Ok(Reply::LayerCount(self.layers)),
            Ask::MacroCount => Ok(Reply::MacroCount(self.macros)),
            Ask::Entries => Ok(Reply::Entries {
                tap_dance: self.tap_dances.len() as u8,
                combo: self.combos.len() as u8,
                alt_repeat: self.alt_repeat,
                flags: self.feature_flags,
            }),
            Ask::MacroBufferSize => Ok(Reply::MacroBufferSize(self.macro_buffer.len() as u16)),
            Ask::MacroChunk { offset, len } => {
                let start = (offset as usize).min(self.macro_buffer.len());
                let end = (start + len as usize).min(self.macro_buffer.len());
                Ok(Reply::Chunk(self.macro_buffer[start..end].to_vec()))
            }
            Ask::SetMacroChunk { offset, len, data } => {
                // vial-qmk skips macro writes until the board is unlocked.
                if self.locked {
                    return Ok(Reply::Ack);
                }
                let start = offset as usize;
                let end = start + len as usize;
                if end > self.macro_buffer.len() {
                    return Err(Error::Protocol("macro write is outside the buffer"));
                }
                self.macro_buffer[start..end].copy_from_slice(&data[..len as usize]);
                Ok(Reply::Ack)
            }
            Ask::GetTapDance { index } => self
                .tap_dances
                .get(index as usize)
                .map(|words| Reply::Words(*words))
                .ok_or(Error::Protocol("tap dance index")),
            Ask::GetCombo { index } => self
                .combos
                .get(index as usize)
                .map(|words| Reply::Words(*words))
                .ok_or(Error::Protocol("combo index")),
            Ask::SetTapDance { index, words } | Ask::SetCombo { index, words } => {
                // Tap dances firewall every key; combos only the output.
                let tap_dance = matches!(ask, Ask::SetTapDance { .. });
                let mut words = words;
                for (slot, word) in words.iter_mut().enumerate() {
                    if (tap_dance && slot < 4) || (!tap_dance && slot == 4) {
                        *word = self.firewall(*word);
                    }
                }
                let list = if tap_dance {
                    &mut self.tap_dances
                } else {
                    &mut self.combos
                };
                let slot = list
                    .get_mut(index as usize)
                    .ok_or(Error::Protocol("dynamic entry index"))?;
                *slot = words;
                Ok(Reply::Ack)
            }
            Ask::LayoutOptions => Ok(Reply::LayoutOptions(self.options)),
            Ask::KeymapChunk { offset, len } => {
                let bytes = keymap_bytes(&self.matrix);
                let start = offset as usize;
                let mut chunk = vec![0u8; len as usize];
                if start < bytes.len() {
                    let end = (start + len as usize).min(bytes.len());
                    let n = end - start;
                    chunk[..n].copy_from_slice(&bytes[start..end]);
                }
                Ok(Reply::Chunk(chunk))
            }
            Ask::SetKey {
                layer,
                row,
                col,
                code,
            } => {
                let index = cell(layer, row, col, self.rows, self.cols);
                if index >= self.matrix.len() {
                    return Err(Error::Protocol("keycode is outside the matrix"));
                }
                self.matrix[index] = self.firewall(code);
                Ok(Reply::Ack)
            }
            Ask::KeyboardId => Ok(Reply::Id {
                version: FAKE_VERSION,
                uid: [0; 8],
            }),
            Ask::DefinitionSize => {
                let size = u32::try_from(self.compressed.len())
                    .map_err(|_| Error::Protocol("definition is too large"))?;
                Ok(Reply::DefSize(size))
            }
            Ask::Definition { block } => {
                let start = block as usize * 32;
                let mut chunk = Vec::new();
                if start < self.compressed.len() {
                    let end = (start + 32).min(self.compressed.len());
                    chunk.extend_from_slice(&self.compressed[start..end]);
                }
                Ok(Reply::DefBlock(chunk))
            }
            Ask::GetEncoder { layer, index } => {
                let slot = self
                    .encoders
                    .get(layer as usize)
                    .and_then(|row| row.get(index as usize))
                    .copied()
                    .ok_or(Error::Protocol("encoder is outside the layout"))?;
                Ok(Reply::Encoder {
                    cw: slot.0,
                    ccw: slot.1,
                })
            }
            Ask::SetEncoder {
                layer,
                index,
                direction,
                code,
            } => {
                let code = self.firewall(code);
                let slot = self
                    .encoders
                    .get_mut(layer as usize)
                    .and_then(|row| row.get_mut(index as usize))
                    .ok_or(Error::Protocol("encoder is outside the layout"))?;
                if direction == 0 {
                    slot.0 = code;
                } else if direction == 1 {
                    slot.1 = code;
                } else {
                    return Err(Error::Protocol("encoder direction"));
                }
                Ok(Reply::Ack)
            }
            Ask::UnlockStatus => Ok(self.unlock_reply()),
            Ask::UnlockStart => {
                self.unlock_armed = true;
                Ok(Reply::Ack)
            }
            Ask::UnlockPoll => {
                if self.unlock_armed {
                    if self.unlock_waits == 0 {
                        self.locked = false;
                        self.unlock_armed = false;
                    } else {
                        self.unlock_waits -= 1;
                    }
                }
                Ok(Reply::Polled {
                    unlocked: !self.locked,
                    left: self.unlock_waits,
                })
            }
        }
    }

    fn unlock_reply(&self) -> Reply {
        Reply::Unlock {
            unlocked: !self.locked,
            pairs: self.unlock_pairs.clone(),
        }
    }

    fn resize(&mut self, layers: u8) {
        self.layers = layers;
        let cells = cell_count(layers, self.rows, self.cols).unwrap_or(0);
        self.matrix = vec![0; cells];
        self.encoders = vec![vec![(0, 0); self.encoder_count]; layers as usize];
    }

    /// vial-qmk `vial_keycode_firewall`: a locked board stores `QK_BOOT` as `KC_NO`.
    fn firewall(&self, code: u16) -> u16 {
        if self.locked && code == QK_BOOT {
            0
        } else {
            code
        }
    }
}

/// The one code vial-qmk firewalls on a locked board. The fake mirrors it.
const QK_BOOT: u16 = 0x7C00;

/// Shares the store. A clone is the same keyboard, not a copy of the keys.
/// `!Send` because the store is a `RefCell`. One thread. No mutex.
#[derive(Clone)]
pub struct MemoryBoard {
    store: Arc<RefCell<Store>>,
}

impl MemoryBoard {
    /// Parse `json`, reject a matrix address the matrix cannot hold, size the
    /// keymap to `KC_NO`, and XZ-compress the same bytes load will decompress.
    /// Default name `board`, one layer, layout-option dword 0, unlocked.
    pub fn from_definition(json: &str) -> Result<Self> {
        let def = layout::parse(json)?;
        let rows = def.rows_n;
        let cols = def.cols_n;
        let encoder_count = layout::encoder_count(&def);
        let compressed = compress(json.as_bytes())?;
        let mut store = Store {
            name: "board".to_string(),
            compressed,
            layers: 1,
            macros: 0,
            macro_buffer: vec![0; 1024],
            tap_dances: Vec::new(),
            combos: Vec::new(),
            alt_repeat: 0,
            feature_flags: 0,
            options: 0,
            locked: false,
            unlock_pairs: Vec::new(),
            unlock_armed: false,
            unlock_waits: 0,
            matrix: Vec::new(),
            encoders: Vec::new(),
            rows,
            cols,
            encoder_count,
            open: false,
        };
        store.resize(1);
        Ok(Self {
            store: Arc::new(RefCell::new(store)),
        })
    }

    /// What command `0x11` will answer.
    pub fn layers(self, count: u8) -> Self {
        self.store.borrow_mut().resize(count);
        self
    }

    /// What command `0x0C` will answer. The macro buffer is 1024 bytes unless `macro_buffer` says otherwise.
    pub fn macros(self, count: u8) -> Self {
        self.store.borrow_mut().macros = count;
        self
    }

    /// What command `0x0D` will answer. The buffer starts zeroed, so every macro is empty.
    pub fn macro_buffer(self, bytes: u16) -> Self {
        self.store.borrow_mut().macro_buffer = vec![0; usize::from(bytes)];
        self
    }

    /// What Vial dynamic-entry count `0xFE 0x0D 0x00` will answer: `flags` bit 0 is Caps Word,
    /// bit 1 is Layer Lock, and a nonzero `alt_repeat` turns on the repeat keys.
    pub fn features(self, flags: u8, alt_repeat: u8) -> Self {
        let mut store = self.store.borrow_mut();
        store.feature_flags = flags;
        store.alt_repeat = alt_repeat;
        drop(store);
        self
    }

    /// The tap dance count in the Vial dynamic-entry reply. Every entry starts as `KC_NO` with term 0.
    pub fn tap_dances(self, count: u8) -> Self {
        self.store.borrow_mut().tap_dances = vec![[0; 5]; usize::from(count)];
        self
    }

    /// The combo count in the Vial dynamic-entry reply. Every combo starts as `KC_NO`.
    pub fn combos(self, count: u8) -> Self {
        self.store.borrow_mut().combos = vec![[0; 5]; usize::from(count)];
        self
    }

    /// What get-keyboard-value `0x02` / id `0x02` will answer.
    pub fn layout_options(self, dword: u32) -> Self {
        self.store.borrow_mut().options = dword;
        self
    }

    /// Status starts locked. Pairs are the hold, in the order poll prints.
    /// The first unlock poll after unlock start reports ready.
    pub fn locked(self, keys: &[(u8, u8)]) -> Self {
        let mut store = self.store.borrow_mut();
        store.locked = true;
        store.unlock_pairs = keys.to_vec();
        store.unlock_armed = false;
        drop(store);
        self
    }

    /// Status-line name, and the choose-list name.
    pub fn named(self, name: impl Into<String>) -> Self {
        self.store.borrow_mut().name = name.into();
        self
    }

    pub fn open(&self) -> Result<Attached> {
        super::fan_in(vec![self.candidate()])
    }

    pub fn choose(boards: &[MemoryBoard]) -> Result<Attached> {
        super::fan_in(boards.iter().map(MemoryBoard::candidate).collect())
    }

    fn candidate(&self) -> Candidate {
        let name = self.store.borrow().name.clone();
        Candidate {
            listing: Box::new(Listing {
                name,
                ..Listing::default()
            }),
            io: Some(Box::new(FakeIo {
                store: Arc::clone(&self.store),
            })),
            guard: Some(Arc::clone(&self.store)),
        }
    }
}

pub(super) struct Candidate {
    /// Boxed so a failed open can hand the candidate back without a large `Err`.
    pub(super) listing: Box<Listing>,
    io: Option<Box<dyn Io>>,
    guard: Option<Arc<RefCell<Store>>>,
}

/// What the choose screen shows for a board before it is opened. Empty fields are not shown.
#[derive(Default)]
pub(super) struct Listing {
    pub(super) name: String,
    pub(super) maker: String,
    /// `vid:pid` in hex.
    pub(super) usb: String,
    pub(super) serial: String,
}

/// Skip non-Vial HID devices. Do not read the definition here.
pub(super) fn enumerate() -> Result<Vec<Candidate>> {
    let api = hidapi::HidApi::new().map_err(|_| Error::Disconnected)?;
    let mut found = Vec::new();
    for info in api.device_list() {
        if info.usage_page() != 0xFF60 || info.usage() != 0x61 {
            continue;
        }
        let name = match info.product_string() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => "board".to_string(),
        };
        let Ok(device) = info.open_device(&api) else {
            continue;
        };
        let mut io = HidIo { device };
        let Ok(resp) = io.exchange(&encode(&Ask::KeyboardId)) else {
            continue;
        };
        if decode(&Ask::KeyboardId, &resp).is_err() {
            continue;
        }
        found.push(Candidate {
            listing: Box::new(Listing {
                name,
                maker: info.manufacturer_string().unwrap_or_default().to_string(),
                usb: format!("{:04x}:{:04x}", info.vendor_id(), info.product_id()),
                serial: info.serial_number().unwrap_or_default().to_string(),
            }),
            io: Some(Box::new(io)),
            guard: None,
        });
    }
    Ok(found)
}

/// Reads the whole board at once. Sets `Store.open` when the `Io` is a fake.
pub(super) fn load(mut candidate: Candidate) -> Result<Board> {
    let mut load = Load::begin(&candidate)?;
    while !load.step(&mut candidate)? {}
    Ok(load.finish(candidate))
}

/// What the keyboard holds, as last read or written. `Board::keymap` and `Board::entries` are the
/// draft the screen shows; `pending` is the difference.
struct Saved {
    matrix: Vec<u16>,
    encoders: Vec<Vec<(u16, u16)>>,
    tap_dances: Vec<TapDance>,
    combos: Vec<Combo>,
    macros: Vec<Vec<Action>>,
}

/// A board read one report per `step`, so the screen can show progress and take Esc between
/// reports. Over Bluetooth a report takes tens of milliseconds and a board takes a few hundred.
/// Order: id, dynamic-entry counts, definition, layout dword, layer count, macro count, keymap
/// chunks, encoders, unlock status, tap dances, combos, macro buffer.
pub(super) struct Load {
    phase: Phase,
    done: usize,
    percent: usize,
    version: i32,
    features: Features,
    tap_dance: u8,
    combo: u8,
    size: u32,
    compressed: Vec<u8>,
    def: Option<Definition>,
    options: u32,
    layers: u8,
    macros: u8,
    matrix: Vec<u16>,
    encoders: Vec<Vec<(u16, u16)>>,
    unlocked: bool,
    entries: Entries,
}

#[derive(Clone, Copy)]
enum Phase {
    Id,
    Entries,
    DefSize,
    Def { block: u32 },
    Options,
    Layers,
    MacroCount,
    Keymap { offset: usize },
    Encoder { layer: u8, index: usize },
    Unlock,
    TapDance(u8),
    Combo(u8),
    MacroSize,
    Macro { offset: usize },
    Done,
}

/// `what` names the part being read, with its own count when it has one.
pub(super) struct Progress {
    pub(super) what: String,
    pub(super) percent: usize,
}

impl Load {
    pub(super) fn begin(candidate: &Candidate) -> Result<Self> {
        if candidate
            .guard
            .as_ref()
            .is_some_and(|guard| guard.borrow().open)
        {
            return Err(Error::Busy);
        }
        if candidate.io.is_none() {
            return Err(Error::Disconnected);
        }
        Ok(Self {
            phase: Phase::Id,
            done: 0,
            percent: 0,
            version: 0,
            features: Features::default(),
            tap_dance: 0,
            combo: 0,
            size: 0,
            compressed: Vec::new(),
            def: None,
            options: 0,
            layers: 0,
            macros: 0,
            matrix: Vec::new(),
            encoders: Vec::new(),
            unlocked: false,
            entries: Entries::default(),
        })
    }

    /// One report. `true` once the board is fully read.
    pub(super) fn step(&mut self, candidate: &mut Candidate) -> Result<bool> {
        let io = candidate.io.as_deref_mut().ok_or(Error::Disconnected)?;
        self.phase = self.read(io)?;
        self.done += 1;
        // Each count is learned on the way, so the total grows early on. Hold the bar where it
        // got to rather than step back.
        self.percent = self
            .percent
            .max(self.done * 100 / self.total().max(self.done));
        Ok(matches!(self.phase, Phase::Done))
    }

    /// Reports for the whole board, counting only the parts sized so far.
    fn total(&self) -> usize {
        3 + (self.size as usize).div_ceil(32)
            + 3
            + (self.matrix.len() * 2).div_ceil(usize::from(CHUNK))
            + self.encoders.iter().map(Vec::len).sum::<usize>()
            + 1
            + usize::from(self.tap_dance)
            + usize::from(self.combo)
            + if self.macros > 0 {
                1 + self.entries.macro_memory.div_ceil(usize::from(CHUNK))
            } else {
                0
            }
    }

    pub(super) fn progress(&self) -> Progress {
        let blocks = (self.size as usize).div_ceil(32);
        let chunks = (self.matrix.len() * 2).div_ceil(usize::from(CHUNK));
        let encoders = self.encoders.iter().map(Vec::len).sum::<usize>();
        let macro_chunks = self.entries.macro_memory.div_ceil(usize::from(CHUNK));
        let part = |name: &str, at: usize, of: usize| format!("{name} {}/{of}", at.min(of));
        let what = match self.phase {
            Phase::Id | Phase::Entries | Phase::DefSize => "connecting".to_string(),
            Phase::Def { block } => part("layout", block as usize, blocks),
            Phase::Options | Phase::Layers | Phase::MacroCount => "layout".to_string(),
            Phase::Keymap { offset } => part("keymap", offset / usize::from(CHUNK), chunks),
            Phase::Encoder { layer, index } => {
                let per = encoders / usize::from(self.layers.max(1));
                part("encoders", usize::from(layer) * per + index, encoders)
            }
            Phase::Unlock => "lock state".to_string(),
            Phase::TapDance(index) => part(
                "tap dances",
                usize::from(index),
                usize::from(self.tap_dance),
            ),
            Phase::Combo(index) => part("combos", usize::from(index), usize::from(self.combo)),
            Phase::MacroSize => "macros".to_string(),
            Phase::Macro { offset } => part("macros", offset / usize::from(CHUNK), macro_chunks),
            Phase::Done => "done".to_string(),
        };
        Progress {
            what,
            percent: self.percent,
        }
    }

    /// Sends the report for the current phase and returns the next phase, skipping empty ones.
    fn read(&mut self, io: &mut dyn Io) -> Result<Phase> {
        Ok(match self.phase {
            Phase::Id => {
                let Reply::Id { version, .. } = exchange(io, &Ask::KeyboardId)? else {
                    return Err(Error::Protocol("keyboard id"));
                };
                if !(-1..=6).contains(&version) {
                    return Err(Error::Unsupported { vial: version });
                }
                self.version = version;
                // Vial asks for dynamic entries from protocol 4.
                if version >= 4 {
                    Phase::Entries
                } else {
                    Phase::DefSize
                }
            }
            Phase::Entries => {
                let Reply::Entries {
                    tap_dance,
                    combo,
                    alt_repeat,
                    flags,
                } = exchange(io, &Ask::Entries)?
                else {
                    return Err(Error::Protocol("dynamic entries"));
                };
                // Persistent default layer is unconditional from protocol 5.
                self.features = Features {
                    caps_word: flags & 0x01 != 0,
                    layer_lock: flags & 0x02 != 0,
                    repeat_key: alt_repeat > 0,
                    persistent_default_layer: self.version >= 5,
                };
                self.tap_dance = tap_dance;
                self.combo = combo;
                Phase::DefSize
            }
            Phase::DefSize => {
                let Reply::DefSize(size) = exchange(io, &Ask::DefinitionSize)? else {
                    return Err(Error::Protocol("definition size"));
                };
                if size == 0 {
                    return Err(Error::NoLayout);
                }
                self.size = size;
                self.compressed = Vec::with_capacity(size as usize);
                Phase::Def { block: 0 }
            }
            Phase::Def { block } => {
                let Reply::DefBlock(chunk) = exchange(io, &Ask::Definition { block })? else {
                    return Err(Error::Protocol("definition block"));
                };
                // The report is 32 bytes. The last block's tail is padding, so the declared size decides the cut.
                let take = (self.size as usize - self.compressed.len()).min(32);
                if chunk.len() < take {
                    return Err(Error::Protocol("definition block"));
                }
                self.compressed.extend_from_slice(&chunk[..take]);
                if self.compressed.len() < self.size as usize {
                    return Ok(Phase::Def { block: block + 1 });
                }
                let json = decompress(&self.compressed)?;
                let text = String::from_utf8(json).map_err(|_| Error::NoLayout)?;
                self.def = Some(layout::parse(&text)?);
                Phase::Options
            }
            Phase::Options => {
                let Reply::LayoutOptions(options) = exchange(io, &Ask::LayoutOptions)? else {
                    return Err(Error::Protocol("layout options"));
                };
                self.options = options;
                Phase::Layers
            }
            Phase::Layers => {
                let Reply::LayerCount(layers) = exchange(io, &Ask::LayerCount)? else {
                    return Err(Error::Protocol("layer count"));
                };
                let def = self.def.as_ref().ok_or(Error::NoLayout)?;
                let cells = cell_count(layers, def.rows_n, def.cols_n)?;
                // Keymap offsets are 16-bit.
                if cells * 2 > usize::from(u16::MAX) {
                    return Err(Error::Protocol("keymap size"));
                }
                self.matrix = vec![0u16; cells];
                self.encoders = vec![vec![(0, 0); layout::encoder_count(def)]; usize::from(layers)];
                self.layers = layers;
                Phase::MacroCount
            }
            Phase::MacroCount => {
                let Reply::MacroCount(macros) = exchange(io, &Ask::MacroCount)? else {
                    return Err(Error::Protocol("macro count"));
                };
                self.macros = macros;
                self.keymap_from(0)
            }
            Phase::Keymap { offset } => {
                let len = (self.matrix.len() * 2 - offset).min(usize::from(CHUNK)) as u8;
                let Reply::Chunk(chunk) = exchange(
                    io,
                    &Ask::KeymapChunk {
                        offset: offset as u16,
                        len,
                    },
                )?
                else {
                    return Err(Error::Protocol("keymap chunk"));
                };
                if chunk.len() < usize::from(len) {
                    return Err(Error::Protocol("keymap chunk"));
                }
                let (pairs, _) = chunk[..usize::from(len)].as_chunks::<2>();
                for (index, pair) in pairs.iter().enumerate() {
                    self.matrix[offset / 2 + index] = u16::from_be_bytes(*pair);
                }
                self.keymap_from(offset + usize::from(len))
            }
            Phase::Encoder { layer, index } => {
                let Reply::Encoder { cw, ccw } = exchange(
                    io,
                    &Ask::GetEncoder {
                        layer,
                        index: index as u8,
                    },
                )?
                else {
                    return Err(Error::Protocol("encoder"));
                };
                self.encoders[usize::from(layer)][index] = (cw, ccw);
                self.encoder_from(layer, index + 1)
            }
            Phase::Unlock => {
                let Reply::Unlock { unlocked, .. } = exchange(io, &Ask::UnlockStatus)? else {
                    return Err(Error::Protocol("unlock status"));
                };
                self.unlocked = unlocked;
                self.tap_dance_from(0)
            }
            Phase::TapDance(index) => {
                let Reply::Words(words) = exchange(io, &Ask::GetTapDance { index })? else {
                    return Err(Error::Protocol("tap dance"));
                };
                self.entries.tap_dances.push(TapDance::from_words(words));
                self.tap_dance_from(index + 1)
            }
            Phase::Combo(index) => {
                let Reply::Words(keys) = exchange(io, &Ask::GetCombo { index })? else {
                    return Err(Error::Protocol("combo"));
                };
                self.entries.combos.push(Combo { keys });
                self.combo_from(index + 1)
            }
            Phase::MacroSize => {
                let Reply::MacroBufferSize(memory) = exchange(io, &Ask::MacroBufferSize)? else {
                    return Err(Error::Protocol("macro buffer size"));
                };
                self.entries.macro_memory = usize::from(memory);
                self.macro_from(0)
            }
            Phase::Macro { offset } => {
                let len = (self.entries.macro_memory - offset).min(usize::from(CHUNK)) as u8;
                let Reply::Chunk(chunk) = exchange(
                    io,
                    &Ask::MacroChunk {
                        offset: offset as u16,
                        len,
                    },
                )?
                else {
                    return Err(Error::Protocol("macro buffer"));
                };
                self.entries.macro_bytes.extend_from_slice(&chunk);
                self.macro_from(offset + usize::from(len))
            }
            Phase::Done => Phase::Done,
        })
    }

    fn keymap_from(&self, offset: usize) -> Phase {
        if offset < self.matrix.len() * 2 {
            Phase::Keymap { offset }
        } else {
            self.encoder_from(0, 0)
        }
    }

    fn encoder_from(&self, layer: u8, index: usize) -> Phase {
        let per = self.encoders.first().map_or(0, Vec::len);
        if per == 0 || layer >= self.layers {
            Phase::Unlock
        } else if index < per {
            Phase::Encoder { layer, index }
        } else {
            self.encoder_from(layer + 1, 0)
        }
    }

    fn tap_dance_from(&self, index: u8) -> Phase {
        if index < self.tap_dance {
            Phase::TapDance(index)
        } else {
            self.combo_from(0)
        }
    }

    fn combo_from(&self, index: u8) -> Phase {
        if index < self.combo {
            Phase::Combo(index)
        } else if self.version >= 2 && self.macros > 0 {
            // The protocol 2 action codes are the only ones written here, so older boards get no macro tab.
            Phase::MacroSize
        } else {
            Phase::Done
        }
    }

    /// Stops once every macro has its NUL, as vial-gui does.
    fn macro_from(&self, offset: usize) -> Phase {
        let ended = self
            .entries
            .macro_bytes
            .iter()
            .filter(|byte| **byte == 0)
            .count();
        if offset < self.entries.macro_memory && ended < usize::from(self.macros) {
            Phase::Macro { offset }
        } else {
            Phase::Done
        }
    }

    /// The read board. Call after `step` returns `true`.
    pub(super) fn finish(mut self, mut candidate: Candidate) -> Board {
        let def = self.def.take().expect("a finished load has a definition");
        if self.version >= 2 && self.macros > 0 {
            self.entries.macros =
                entries::decode_macros(&self.entries.macro_bytes, usize::from(self.macros));
        }
        let guard = candidate.guard.clone();
        if let Some(guard) = &guard {
            guard.borrow_mut().open = true;
        }
        let keymap = Keymap {
            layers: self.layers,
            rows: def.rows_n,
            cols: def.cols_n,
            matrix: self.matrix,
            encoders: self.encoders,
        };
        let saved = Saved {
            matrix: keymap.matrix.clone(),
            encoders: keymap.encoders.clone(),
            tap_dances: self.entries.tap_dances.clone(),
            combos: self.entries.combos.clone(),
            macros: self.entries.macros.clone(),
        };
        Board {
            name: candidate.listing.name.clone(),
            catalog: catalog::merge(
                &def.custom,
                self.tap_dance,
                self.macros,
                def.midi,
                self.layers,
                self.features,
            ),
            def,
            keymap,
            options: self.options,
            locked: !self.unlocked,
            unlock: Vec::new(),
            unlock_left: None,
            entries: self.entries,
            saved,
            undo: Vec::new(),
            io: candidate.io.take().expect("a finished load keeps its io"),
            _guard: guard,
        }
    }
}

#[derive(Default)]
struct Entries {
    tap_dances: Vec<TapDance>,
    combos: Vec<Combo>,
    macros: Vec<Vec<Action>>,
    /// The device buffer as last read or written. A write sends only the chunks that differ.
    macro_bytes: Vec<u8>,
    macro_memory: usize,
}

pub(super) struct Board {
    name: String,
    def: Definition,
    catalog: Table,
    keymap: Keymap,
    options: u32,
    locked: bool,
    /// The keys the last unlock status asked for. Paint marks them during a hold.
    unlock: Vec<(u8, u8)>,
    /// The firmware's unlock countdown from the last poll. `None` before the first poll of a hold.
    unlock_left: Option<u8>,
    /// The draft. Edits land here; `commit` copies each written change into `saved`.
    entries: Entries,
    saved: Saved,
    /// What each staged change replaced, newest last.
    undo: Vec<Write>,
    io: Box<dyn Io>,
    /// Clears `Store.open` on drop when this board came from a fake.
    _guard: Option<Arc<RefCell<Store>>>,
}

impl Drop for Board {
    fn drop(&mut self) {
        if let Some(guard) = &self._guard {
            guard.borrow_mut().open = false;
        }
    }
}

struct Keymap {
    layers: u8,
    rows: u8,
    cols: u8,
    matrix: Vec<u16>,
    encoders: Vec<Vec<(u16, u16)>>,
}

impl Keymap {
    fn cell(&self, layer: u8, row: u8, col: u8) -> usize {
        cell(layer, row, col, self.rows, self.cols)
    }
}

/// Private so `app` cannot match it. Response shape is a property of the ask.
enum Ask {
    LayerCount,
    MacroCount,
    LayoutOptions,
    KeymapChunk {
        offset: u16,
        len: u8,
    },
    SetKey {
        layer: u8,
        row: u8,
        col: u8,
        code: u16,
    },
    KeyboardId,
    DefinitionSize,
    Definition {
        block: u32,
    },
    GetEncoder {
        layer: u8,
        index: u8,
    },
    SetEncoder {
        layer: u8,
        index: u8,
        direction: u8,
        code: u16,
    },
    UnlockStatus,
    UnlockStart,
    UnlockPoll,
    /// Vial dynamic-entry counts. The last byte carries the optional feature bits.
    Entries,
    GetTapDance {
        index: u8,
    },
    /// Little-endian on, hold, double tap, tap-hold, then the term.
    SetTapDance {
        index: u8,
        words: [u16; 5],
    },
    GetCombo {
        index: u8,
    },
    /// Little-endian, four inputs then the output.
    SetCombo {
        index: u8,
        words: [u16; 5],
    },
    MacroBufferSize,
    MacroChunk {
        offset: u16,
        len: u8,
    },
    SetMacroChunk {
        offset: u16,
        len: u8,
        data: [u8; CHUNK as usize],
    },
}

enum Grammar {
    /// The command byte stays at index 0.
    Echo,
    /// The response overwrites the front of the buffer.
    Overwrite,
}

impl Ask {
    fn grammar(&self) -> Grammar {
        match self {
            Ask::LayerCount
            | Ask::MacroCount
            | Ask::LayoutOptions
            | Ask::KeymapChunk { .. }
            | Ask::SetKey { .. }
            | Ask::MacroBufferSize
            | Ask::MacroChunk { .. }
            | Ask::SetMacroChunk { .. } => Grammar::Echo,
            Ask::KeyboardId
            | Ask::DefinitionSize
            | Ask::Definition { .. }
            | Ask::GetEncoder { .. }
            | Ask::SetEncoder { .. }
            | Ask::UnlockStatus
            | Ask::UnlockStart
            | Ask::UnlockPoll
            | Ask::Entries
            | Ask::GetTapDance { .. }
            | Ask::SetTapDance { .. }
            | Ask::GetCombo { .. }
            | Ask::SetCombo { .. } => Grammar::Overwrite,
        }
    }
}

const CHUNK: u8 = 28;
const GET_KEYBOARD_VALUE: u8 = 0x02;
const LAYOUT_OPTIONS_ID: u8 = 0x02;
const SET_KEYCODE: u8 = 0x05;
const MACRO_COUNT: u8 = 0x0C;
const MACRO_BUFFER_SIZE: u8 = 0x0D;
const MACRO_GET_BUFFER: u8 = 0x0E;
const MACRO_SET_BUFFER: u8 = 0x0F;
const LAYER_COUNT: u8 = 0x11;
const GET_BUFFER: u8 = 0x12;
const VIAL: u8 = 0xFE;
const VIAL_ID: u8 = 0x00;
const VIAL_DEF_SIZE: u8 = 0x01;
const VIAL_DEF: u8 = 0x02;
const VIAL_GET_ENCODER: u8 = 0x03;
const VIAL_SET_ENCODER: u8 = 0x04;
const VIAL_UNLOCK_STATUS: u8 = 0x05;
const VIAL_UNLOCK_START: u8 = 0x06;
const VIAL_UNLOCK_POLL: u8 = 0x07;
const VIAL_DYNAMIC_ENTRY_OP: u8 = 0x0D;
const DYNAMIC_ENTRY_COUNTS: u8 = 0x00;
const DYNAMIC_TAP_DANCE_GET: u8 = 0x01;
const DYNAMIC_TAP_DANCE_SET: u8 = 0x02;
const DYNAMIC_COMBO_GET: u8 = 0x03;
const DYNAMIC_COMBO_SET: u8 = 0x04;
const FAKE_VERSION: i32 = 6;
/// vial-qmk `VIAL_UNLOCK_COUNTER_MAX`: polls the unlock keys must stay held for.
const UNLOCK_COUNT: u8 = 50;

fn encode(ask: &Ask) -> [u8; 32] {
    let mut buf = [0u8; 32];
    match *ask {
        Ask::LayerCount => buf[0] = LAYER_COUNT,
        Ask::MacroCount => buf[0] = MACRO_COUNT,
        Ask::LayoutOptions => {
            buf[0] = GET_KEYBOARD_VALUE;
            buf[1] = LAYOUT_OPTIONS_ID;
        }
        Ask::KeymapChunk { offset, len } => {
            buf[0] = GET_BUFFER;
            buf[1] = (offset >> 8) as u8;
            buf[2] = (offset & 0xff) as u8;
            buf[3] = len;
        }
        Ask::SetKey {
            layer,
            row,
            col,
            code,
        } => {
            buf[0] = SET_KEYCODE;
            buf[1] = layer;
            buf[2] = row;
            buf[3] = col;
            buf[4] = (code >> 8) as u8;
            buf[5] = (code & 0xff) as u8;
        }
        Ask::KeyboardId => {
            buf[0] = VIAL;
            buf[1] = VIAL_ID;
        }
        Ask::DefinitionSize => {
            buf[0] = VIAL;
            buf[1] = VIAL_DEF_SIZE;
        }
        Ask::Definition { block } => {
            buf[0] = VIAL;
            buf[1] = VIAL_DEF;
            buf[2..6].copy_from_slice(&block.to_le_bytes());
        }
        Ask::GetEncoder { layer, index } => {
            buf[0] = VIAL;
            buf[1] = VIAL_GET_ENCODER;
            buf[2] = layer;
            buf[3] = index;
        }
        Ask::SetEncoder {
            layer,
            index,
            direction,
            code,
        } => {
            buf[0] = VIAL;
            buf[1] = VIAL_SET_ENCODER;
            buf[2] = layer;
            buf[3] = index;
            buf[4] = direction;
            buf[5] = (code >> 8) as u8;
            buf[6] = (code & 0xff) as u8;
        }
        Ask::UnlockStatus => {
            buf[0] = VIAL;
            buf[1] = VIAL_UNLOCK_STATUS;
        }
        Ask::UnlockStart => {
            buf[0] = VIAL;
            buf[1] = VIAL_UNLOCK_START;
        }
        Ask::UnlockPoll => {
            buf[0] = VIAL;
            buf[1] = VIAL_UNLOCK_POLL;
        }
        Ask::Entries => {
            buf[0] = VIAL;
            buf[1] = VIAL_DYNAMIC_ENTRY_OP;
            buf[2] = DYNAMIC_ENTRY_COUNTS;
        }
        Ask::GetTapDance { index } | Ask::GetCombo { index } => {
            buf[0] = VIAL;
            buf[1] = VIAL_DYNAMIC_ENTRY_OP;
            buf[2] = if matches!(ask, Ask::GetTapDance { .. }) {
                DYNAMIC_TAP_DANCE_GET
            } else {
                DYNAMIC_COMBO_GET
            };
            buf[3] = index;
        }
        Ask::SetTapDance { index, words } | Ask::SetCombo { index, words } => {
            buf[0] = VIAL;
            buf[1] = VIAL_DYNAMIC_ENTRY_OP;
            buf[2] = if matches!(ask, Ask::SetTapDance { .. }) {
                DYNAMIC_TAP_DANCE_SET
            } else {
                DYNAMIC_COMBO_SET
            };
            buf[3] = index;
            put_words(&mut buf[4..14], words);
        }
        Ask::MacroBufferSize => buf[0] = MACRO_BUFFER_SIZE,
        Ask::MacroChunk { offset, len } => {
            buf[0] = MACRO_GET_BUFFER;
            buf[1..3].copy_from_slice(&offset.to_be_bytes());
            buf[3] = len;
        }
        Ask::SetMacroChunk { offset, len, data } => {
            buf[0] = MACRO_SET_BUFFER;
            buf[1..3].copy_from_slice(&offset.to_be_bytes());
            buf[3] = len;
            buf[4..4 + CHUNK as usize].copy_from_slice(&data);
        }
    }
    buf
}

fn decode(ask: &Ask, resp: &[u8; 32]) -> Result<Reply> {
    match ask.grammar() {
        Grammar::Echo => decode_echo(ask, resp),
        Grammar::Overwrite => decode_overwrite(ask, resp),
    }
}

fn decode_echo(ask: &Ask, resp: &[u8; 32]) -> Result<Reply> {
    match *ask {
        Ask::LayerCount => Ok(Reply::LayerCount(resp[1])),
        Ask::MacroCount => Ok(Reply::MacroCount(resp[1])),
        Ask::LayoutOptions => {
            let dword = u32::from_be_bytes(resp[2..6].try_into().unwrap());
            Ok(Reply::LayoutOptions(dword))
        }
        Ask::KeymapChunk { len, .. } | Ask::MacroChunk { len, .. } => {
            if len > CHUNK {
                return Err(Error::Protocol("buffer chunk length"));
            }
            Ok(Reply::Chunk(resp[4..4 + len as usize].to_vec()))
        }
        Ask::SetKey { .. } | Ask::SetMacroChunk { .. } => Ok(Reply::Ack),
        Ask::MacroBufferSize => Ok(Reply::MacroBufferSize(u16::from_be_bytes([
            resp[1], resp[2],
        ]))),
        _ => Err(Error::Protocol("echo response")),
    }
}

fn decode_overwrite(ask: &Ask, resp: &[u8; 32]) -> Result<Reply> {
    match *ask {
        Ask::KeyboardId => {
            let version = i32::from_le_bytes(resp[0..4].try_into().unwrap());
            let mut uid = [0u8; 8];
            uid.copy_from_slice(&resp[4..12]);
            Ok(Reply::Id { version, uid })
        }
        Ask::DefinitionSize => Ok(Reply::DefSize(u32::from_le_bytes(
            resp[0..4].try_into().unwrap(),
        ))),
        Ask::Definition { .. } => Ok(Reply::DefBlock(resp.to_vec())),
        Ask::GetEncoder { .. } => Ok(Reply::Encoder {
            cw: u16::from_be_bytes([resp[0], resp[1]]),
            ccw: u16::from_be_bytes([resp[2], resp[3]]),
        }),
        Ask::SetEncoder { .. } | Ask::UnlockStart => Ok(Reply::Ack),
        Ask::UnlockStatus => Ok(Reply::Unlock {
            unlocked: resp[0] != 0,
            pairs: unlock_pairs(resp),
        }),
        Ask::UnlockPoll => Ok(Reply::Polled {
            unlocked: resp[0] != 0,
            left: resp[2],
        }),
        Ask::Entries => Ok(Reply::Entries {
            tap_dance: resp[0],
            combo: resp[1],
            alt_repeat: resp[3],
            flags: resp[31],
        }),
        // Byte 0 is the firmware's status. Zero is success.
        Ask::GetTapDance { .. } | Ask::GetCombo { .. } if resp[0] == 0 => {
            Ok(Reply::Words(words_from(&resp[1..11])))
        }
        Ask::SetTapDance { .. } | Ask::SetCombo { .. } if resp[0] == 0 => Ok(Reply::Ack),
        Ask::GetTapDance { .. }
        | Ask::GetCombo { .. }
        | Ask::SetTapDance { .. }
        | Ask::SetCombo { .. } => Err(Error::Protocol("the board refused a dynamic entry")),
        _ => Err(Error::Protocol("overwrite response")),
    }
}

fn decode_request(report: &[u8; 32]) -> Result<Ask> {
    match report[0] {
        LAYER_COUNT => Ok(Ask::LayerCount),
        MACRO_COUNT => Ok(Ask::MacroCount),
        MACRO_BUFFER_SIZE => Ok(Ask::MacroBufferSize),
        MACRO_GET_BUFFER | MACRO_SET_BUFFER => {
            let offset = u16::from_be_bytes([report[1], report[2]]);
            let len = report[3];
            if len > CHUNK {
                return Err(Error::Protocol("buffer chunk length"));
            }
            if report[0] == MACRO_GET_BUFFER {
                return Ok(Ask::MacroChunk { offset, len });
            }
            let mut data = [0u8; CHUNK as usize];
            data.copy_from_slice(&report[4..4 + CHUNK as usize]);
            Ok(Ask::SetMacroChunk { offset, len, data })
        }
        GET_KEYBOARD_VALUE if report[1] == LAYOUT_OPTIONS_ID => Ok(Ask::LayoutOptions),
        GET_BUFFER => {
            let len = report[3];
            if len > CHUNK {
                return Err(Error::Protocol("keymap chunk length"));
            }
            let offset = u16::from_be_bytes([report[1], report[2]]);
            Ok(Ask::KeymapChunk { offset, len })
        }
        SET_KEYCODE => Ok(Ask::SetKey {
            layer: report[1],
            row: report[2],
            col: report[3],
            code: u16::from_be_bytes([report[4], report[5]]),
        }),
        VIAL => match report[1] {
            VIAL_ID => Ok(Ask::KeyboardId),
            VIAL_DEF_SIZE => Ok(Ask::DefinitionSize),
            VIAL_DEF => Ok(Ask::Definition {
                block: u32::from_le_bytes(report[2..6].try_into().unwrap()),
            }),
            VIAL_GET_ENCODER => Ok(Ask::GetEncoder {
                layer: report[2],
                index: report[3],
            }),
            VIAL_SET_ENCODER => Ok(Ask::SetEncoder {
                layer: report[2],
                index: report[3],
                direction: report[4],
                code: u16::from_be_bytes([report[5], report[6]]),
            }),
            VIAL_UNLOCK_STATUS => Ok(Ask::UnlockStatus),
            VIAL_UNLOCK_START => Ok(Ask::UnlockStart),
            VIAL_UNLOCK_POLL => Ok(Ask::UnlockPoll),
            VIAL_DYNAMIC_ENTRY_OP => match report[2] {
                DYNAMIC_ENTRY_COUNTS => Ok(Ask::Entries),
                DYNAMIC_TAP_DANCE_GET => Ok(Ask::GetTapDance { index: report[3] }),
                DYNAMIC_COMBO_GET => Ok(Ask::GetCombo { index: report[3] }),
                DYNAMIC_TAP_DANCE_SET => Ok(Ask::SetTapDance {
                    index: report[3],
                    words: words_from(&report[4..14]),
                }),
                DYNAMIC_COMBO_SET => Ok(Ask::SetCombo {
                    index: report[3],
                    words: words_from(&report[4..14]),
                }),
                _ => Err(Error::Protocol("unknown dynamic entry command")),
            },
            _ => Err(Error::Protocol("unknown vial command")),
        },
        _ => Err(Error::Protocol("unknown command")),
    }
}

enum Reply {
    LayerCount(u8),
    MacroCount(u8),
    LayoutOptions(u32),
    Chunk(Vec<u8>),
    Ack,
    Id {
        version: i32,
        uid: [u8; 8],
    },
    DefSize(u32),
    DefBlock(Vec<u8>),
    Encoder {
        cw: u16,
        ccw: u16,
    },
    Unlock {
        unlocked: bool,
        pairs: Vec<(u8, u8)>,
    },
    /// vial-qmk: byte 2 counts down from 50 while every unlock key is held, and resets when one is not.
    Polled {
        unlocked: bool,
        left: u8,
    },
    Entries {
        tap_dance: u8,
        combo: u8,
        alt_repeat: u8,
        flags: u8,
    },
    /// A tap dance or combo record.
    Words([u16; 5]),
    MacroBufferSize(u16),
}

fn put_words(out: &mut [u8], words: [u16; 5]) {
    for (slot, word) in out.chunks_exact_mut(2).zip(words) {
        slot.copy_from_slice(&word.to_le_bytes());
    }
}

fn words_from(bytes: &[u8]) -> [u16; 5] {
    let mut words = [0u16; 5];
    for (word, pair) in words.iter_mut().zip(bytes.chunks_exact(2)) {
        *word = u16::from_le_bytes([pair[0], pair[1]]);
    }
    words
}

fn encode_reply(ask: &Ask, reply: &Reply) -> [u8; 32] {
    let mut buf = match ask.grammar() {
        Grammar::Echo => encode(ask),
        Grammar::Overwrite => [0u8; 32],
    };
    match (ask, reply) {
        (Ask::LayerCount, Reply::LayerCount(count))
        | (Ask::MacroCount, Reply::MacroCount(count)) => buf[1] = *count,
        (Ask::LayoutOptions, Reply::LayoutOptions(dword)) => {
            buf[2..6].copy_from_slice(&dword.to_be_bytes());
        }
        (Ask::KeymapChunk { len, .. } | Ask::MacroChunk { len, .. }, Reply::Chunk(data)) => {
            let n = (*len as usize).min(data.len()).min(CHUNK as usize);
            buf[4..4 + n].copy_from_slice(&data[..n]);
        }
        (Ask::SetKey { .. } | Ask::SetMacroChunk { .. }, Reply::Ack) => {}
        (Ask::MacroBufferSize, Reply::MacroBufferSize(size)) => {
            buf[1..3].copy_from_slice(&size.to_be_bytes());
        }
        (Ask::GetTapDance { .. } | Ask::GetCombo { .. }, Reply::Words(words)) => {
            put_words(&mut buf[1..11], *words);
        }
        (Ask::KeyboardId, Reply::Id { version, uid }) => {
            buf[0..4].copy_from_slice(&(*version as u32).to_le_bytes());
            buf[4..12].copy_from_slice(uid);
        }
        (Ask::DefinitionSize, Reply::DefSize(size)) => {
            buf[0..4].copy_from_slice(&size.to_le_bytes());
        }
        (Ask::Definition { .. }, Reply::DefBlock(data)) => {
            let n = data.len().min(32);
            buf[..n].copy_from_slice(&data[..n]);
        }
        (Ask::GetEncoder { .. }, Reply::Encoder { cw, ccw }) => {
            buf[0..2].copy_from_slice(&cw.to_be_bytes());
            buf[2..4].copy_from_slice(&ccw.to_be_bytes());
        }
        (Ask::SetEncoder { .. } | Ask::UnlockStart, Reply::Ack) => {}
        (Ask::UnlockStatus, Reply::Unlock { unlocked, pairs }) => {
            buf[0] = u8::from(*unlocked);
            // Unused combo slots are 255,255. A zero byte would be row 0, column 0.
            for slot in 0..15 {
                let (row, col) = pairs.get(slot).copied().unwrap_or((255, 255));
                buf[2 + slot * 2] = row;
                buf[3 + slot * 2] = col;
            }
        }
        (Ask::UnlockPoll, Reply::Polled { unlocked, left }) => {
            buf[0] = u8::from(*unlocked);
            buf[1] = u8::from(!*unlocked);
            buf[2] = *left;
        }
        (
            Ask::Entries,
            Reply::Entries {
                tap_dance,
                combo,
                alt_repeat,
                flags,
            },
        ) => {
            buf[0] = *tap_dance;
            buf[1] = *combo;
            buf[3] = *alt_repeat;
            buf[31] = *flags;
        }
        _ => {}
    }
    buf
}

/// `255,255` is an unused slot, not a key. Stop there.
fn unlock_pairs(resp: &[u8; 32]) -> Vec<(u8, u8)> {
    let mut pairs = Vec::new();
    for slot in 0..15 {
        let row = resp[2 + slot * 2];
        let col = resp[3 + slot * 2];
        if row == 255 && col == 255 {
            break;
        }
        pairs.push((row, col));
    }
    pairs
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Left,
    Right,
    Up,
    Down,
}

pub(in crate::session) use layout::Place;

/// One pending change. `Board::commit` is the only function that sends any of them.
#[derive(Clone, PartialEq)]
pub(in crate::session) enum Write {
    Key {
        layer: u8,
        place: Place,
        code: u16,
    },
    TapDance {
        index: u8,
        entry: TapDance,
    },
    Combo {
        index: u8,
        entry: Combo,
    },
    /// Every macro. The buffer is one blob, so a change to one macro rewrites the rest in place.
    Macros(Vec<Vec<Action>>),
}

impl Write {
    fn codes(&self) -> Vec<u16> {
        match self {
            Write::Key { code, .. } => vec![*code],
            Write::TapDance { entry, .. } => entry.keys.to_vec(),
            Write::Combo { entry, .. } => entry.keys.to_vec(),
            Write::Macros(macros) => macros
                .iter()
                .flatten()
                .filter_map(|action| match action {
                    Action::Tap(code) | Action::Down(code) | Action::Up(code) => Some(*code),
                    Action::Text(_) | Action::Delay(_) => None,
                })
                .collect(),
        }
    }
}

impl Board {
    fn exchange(&mut self, ask: &Ask) -> Result<Reply> {
        exchange(&mut *self.io, ask)
    }

    /// vial-qmk takes every key, encoder, tap dance, and combo write on a locked board, and only
    /// swaps `QK_BOOT` for `KC_NO` (`vial_keycode_firewall`). Macro buffer writes are skipped until
    /// unlocked. So the unlock hold is for those two cases, which is also when vial-gui asks.
    fn needs_unlock(&self, write: &Write) -> bool {
        matches!(write, Write::Macros(_))
            || write.codes().into_iter().any(|code| self.hold_first(code))
    }

    /// `None` when the write needs no unlock or the board already reports unlocked.
    pub(super) fn begin_hold(&mut self, write: &Write) -> Result<Option<Hold>> {
        if !self.needs_unlock(write) {
            return Ok(None);
        }
        let Reply::Unlock { unlocked, pairs } = self.exchange(&Ask::UnlockStatus)? else {
            return Err(Error::Protocol("unlock status"));
        };
        self.locked = !unlocked;
        self.unlock.clone_from(&pairs);
        self.unlock_left = None;
        if !self.locked {
            return Ok(None);
        }
        self.exchange(&Ask::UnlockStart)?;
        Ok(Some(Hold {
            prompt: self.hold_prompt(write, &pairs),
        }))
    }

    /// The only sender of set-key, set-encoder, tap dance, combo, and macro buffer writes.
    pub(super) fn commit(&mut self, write: &Write) -> Result<Commit> {
        let same = match write {
            Write::Key { layer, place, code } => self.saved_code_at(*layer, *place) == Some(*code),
            Write::TapDance { index, entry } => {
                self.saved.tap_dances.get(usize::from(*index)) == Some(entry)
            }
            Write::Combo { index, entry } => {
                self.saved.combos.get(usize::from(*index)) == Some(entry)
            }
            Write::Macros(macros) => self.saved.macros == *macros,
        };
        if same {
            return Ok(Commit::Same);
        }
        if self.locked && self.needs_unlock(write) {
            return Ok(Commit::Holding(Hold {
                prompt: String::new(),
            }));
        }
        match write {
            Write::Key { layer, place, code } => self.commit_key(*layer, *place, *code)?,
            Write::TapDance { index, entry } => {
                if usize::from(*index) >= self.saved.tap_dances.len() {
                    return Err(Error::Protocol("tap dance index"));
                }
                self.exchange(&Ask::SetTapDance {
                    index: *index,
                    words: entry.words(),
                })?;
                self.saved.tap_dances[usize::from(*index)] = *entry;
            }
            Write::Combo { index, entry } => {
                if usize::from(*index) >= self.saved.combos.len() {
                    return Err(Error::Protocol("combo index"));
                }
                self.exchange(&Ask::SetCombo {
                    index: *index,
                    words: entry.keys,
                })?;
                self.saved.combos[usize::from(*index)] = *entry;
            }
            Write::Macros(macros) => self.commit_macros(macros)?,
        }
        Ok(Commit::Stored)
    }

    fn commit_key(&mut self, layer: u8, place: Place, code: u16) -> Result<()> {
        match place {
            Place::Matrix { row, col } => {
                self.exchange(&Ask::SetKey {
                    layer,
                    row,
                    col,
                    code,
                })?;
                let index = self.keymap.cell(layer, row, col);
                if index >= self.saved.matrix.len() {
                    return Err(Error::Protocol("keycode is outside the matrix"));
                }
                self.saved.matrix[index] = code;
            }
            Place::Encoder { index, direction } => {
                self.exchange(&Ask::SetEncoder {
                    layer,
                    index,
                    direction,
                    code,
                })?;
                let slot = self
                    .saved
                    .encoders
                    .get_mut(layer as usize)
                    .and_then(|row| row.get_mut(index as usize))
                    .ok_or(Error::Protocol("encoder is outside the layout"))?;
                if direction == 0 {
                    slot.0 = code;
                } else {
                    slot.1 = code;
                }
            }
        }
        Ok(())
    }

    /// Sends only the 28-byte chunks that differ from what the board holds.
    fn commit_macros(&mut self, macros: &[Vec<Action>]) -> Result<()> {
        let bytes = entries::encode_macros(macros);
        if bytes.len() > self.entries.macro_memory {
            return Err(Error::Protocol("the macros do not fit the macro buffer"));
        }
        for (number, chunk) in bytes.chunks(usize::from(CHUNK)).enumerate() {
            let offset = number * usize::from(CHUNK);
            if self.entries.macro_bytes.get(offset..offset + chunk.len()) == Some(chunk) {
                continue;
            }
            let mut data = [0u8; CHUNK as usize];
            data[..chunk.len()].copy_from_slice(chunk);
            self.exchange(&Ask::SetMacroChunk {
                offset: offset as u16,
                len: chunk.len() as u8,
                data,
            })?;
        }
        let cache = &mut self.entries.macro_bytes;
        if cache.len() < bytes.len() {
            cache.resize(bytes.len(), 0);
        }
        cache[..bytes.len()].copy_from_slice(&bytes);
        self.saved.macros = macros.to_vec();
        Ok(())
    }

    /// Still locked. The hold line keeps the names `begin_hold` wrote, and the countdown moves.
    pub(super) fn poll_unlock(&mut self) -> Result<bool> {
        let Reply::Polled { unlocked, left } = self.exchange(&Ask::UnlockPoll)? else {
            return Err(Error::Protocol("unlock poll"));
        };
        self.locked = !unlocked;
        self.unlock_left = Some(left);
        Ok(self.locked)
    }

    fn hold_prompt(&self, write: &Write, pairs: &[(u8, u8)]) -> String {
        let (layer, next) = match write {
            Write::Key { layer, code, .. } => (*layer, catalog::legend(&self.catalog, *code)),
            Write::TapDance { index, .. } => (0, format!("TD({index})")),
            Write::Combo { index, .. } => (0, format!("combo {index}")),
            Write::Macros(_) => (0, "the macros".to_string()),
        };
        let keys = self.hold_names(layer, pairs);
        if keys.is_empty() {
            format!("hold the unlock keys to write {next}")
        } else {
            format!("hold {keys} to write {next}")
        }
    }

    pub(super) fn tap_dances(&self) -> &[TapDance] {
        &self.entries.tap_dances
    }

    pub(super) fn combos(&self) -> &[Combo] {
        &self.entries.combos
    }

    pub(super) fn macros(&self) -> &[Vec<Action>] {
        &self.entries.macros
    }

    /// Bytes the macros would take, and the buffer size.
    pub(super) fn macro_room(&self, macros: &[Vec<Action>]) -> (usize, usize) {
        (
            entries::encode_macros(macros).len(),
            self.entries.macro_memory,
        )
    }

    pub(super) fn legend(&self, code: u16) -> String {
        catalog::legend(&self.catalog, code)
    }

    /// Each editor the board has entries for, after the layer tabs.
    pub(super) fn editors(&self) -> Vec<Tab> {
        let mut tabs = Vec::new();
        if !self.entries.tap_dances.is_empty() {
            tabs.push(Tab::TapDance);
        }
        if !self.entries.combos.is_empty() {
            tabs.push(Tab::Combos);
        }
        if !self.entries.macros.is_empty() {
            tabs.push(Tab::Macros);
        }
        tabs
    }

    fn hold_names(&self, layer: u8, pairs: &[(u8, u8)]) -> String {
        let mut seen = Vec::new();
        let mut names = Vec::new();
        for &(row, col) in pairs {
            if row >= self.keymap.rows || col >= self.keymap.cols || seen.contains(&(row, col)) {
                continue;
            }
            seen.push((row, col));
            names.push(self.hold_key_name(layer, row, col));
        }
        names.join("+")
    }

    fn hold_key_name(&self, layer: u8, row: u8, col: u8) -> String {
        let place = Place::Matrix { row, col };
        for keys in &self.def.rows {
            for key in keys {
                if !layout::visible(&self.def, self.options, key)
                    || layout::place_of(key) != Some(place)
                {
                    continue;
                }
                if !key.labels[1].is_empty() {
                    return key.labels[1].clone();
                }
                // What the keyboard holds, not the draft: these are the keys to press now.
                let name =
                    catalog::legend(&self.catalog, self.saved_code_at(layer, place).unwrap_or(0));
                if !name.is_empty() && name != "NO" {
                    return name;
                }
                return format!("{row},{col}");
            }
        }
        format!("{row},{col}")
    }

    pub(super) fn list(&self, range: catalog::Range<'_>) -> catalog::Listed {
        catalog::list(&self.catalog, range)
    }

    pub(super) fn shelf_count(&self) -> usize {
        catalog::shelf_count(&self.catalog)
    }

    pub(super) fn hold_first(&self, code: u16) -> bool {
        catalog::hold_first(&self.catalog, code)
    }

    pub(super) fn nudge(&self, _layer: u8, selection: Option<Place>, dir: Dir) -> Option<Place> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let bands = layout::band_of(&targets.iter().map(|target| target.cy).collect::<Vec<_>>());
        let Some(current) =
            selection.and_then(|place| targets.iter().position(|target| target.place == place))
        else {
            return top_left(&targets, &bands);
        };
        let here = &targets[current];
        let band = bands[current];
        let chosen = match dir {
            Dir::Right => targets
                .iter()
                .enumerate()
                .filter(|(index, target)| bands[*index] == band && target.cx > here.cx)
                .min_by(|(_, a), (_, b)| a.cx.total_cmp(&b.cx)),
            Dir::Left => targets
                .iter()
                .enumerate()
                .filter(|(index, target)| bands[*index] == band && target.cx < here.cx)
                .max_by(|(_, a), (_, b)| a.cx.total_cmp(&b.cx)),
            Dir::Down => targets
                .iter()
                .enumerate()
                .filter(|(index, _)| bands[*index] == band + 1)
                .min_by(|(_, a), (_, b)| (a.cx - here.cx).abs().total_cmp(&(b.cx - here.cx).abs())),
            Dir::Up => {
                let Some(above) = band.checked_sub(1) else {
                    return Some(here.place);
                };
                targets
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| bands[*index] == above)
                    .min_by(|(_, a), (_, b)| {
                        (a.cx - here.cx).abs().total_cmp(&(b.cx - here.cx).abs())
                    })
            }
        };
        Some(chosen.map(|(_, target)| target.place).unwrap_or(here.place))
    }

    pub(super) fn layer_count(&self) -> u8 {
        self.keymap.layers
    }

    pub(super) fn text(&self, paint: &Paint<'_>) -> String {
        let frame = self.frame(paint);
        if let Some(spot) = paint.spot {
            return layout::render_sheet(&self.sheet(spot), frame);
        }
        let mut caps = Vec::new();
        for keys in &self.def.rows {
            for key in keys {
                if !layout::visible(&self.def, self.options, key) {
                    continue;
                }
                let place = layout::place_of(key);
                let code = place.and_then(|place| self.code_at(paint.layer, place));
                let legend = if !key.labels[1].is_empty() {
                    key.labels[1].clone()
                } else if place.is_some() {
                    catalog::legend(&self.catalog, code.unwrap_or(0))
                } else if !key.labels[0].is_empty() {
                    key.labels[0].clone()
                } else {
                    String::new()
                };
                let tone = if key.decal {
                    Tone::Decal
                } else if place.is_some() {
                    catalog::tone(code.unwrap_or(0))
                } else {
                    Tone::Key
                };
                let (x, _) = layout::world_point(key, key.x, key.y);
                caps.push(Cap {
                    place,
                    legend,
                    selected: place.is_some() && paint.selection == place,
                    unlock: paint.hold.is_some()
                        && self.locked
                        && matches!(place, Some(Place::Matrix { row, col }) if self.unlock.contains(&(row, col))),
                    changed: place.is_some_and(|place| {
                        self.code_at(paint.layer, place) != self.saved_code_at(paint.layer, place)
                    }),
                    decal: key.decal,
                    tone,
                    x,
                    y: layout::world_top(key),
                    w: key.w,
                    h: key.h,
                });
            }
        }
        let detail = paint.selection.map(|place| self.detail(paint.layer, place));
        layout::render_board(
            paint.layer.saturating_add(1),
            self.layer_count(),
            &caps,
            detail.as_ref(),
            frame,
        )
    }

    /// The selected key: its name and code, what it does, the same key on every layer, and
    /// what the keyboard holds when the draft differs.
    fn detail(&self, layer: u8, place: Place) -> layout::Detail {
        let code = self.code_at(layer, place).unwrap_or(0);
        let saved = self.saved_code_at(layer, place).unwrap_or(0);
        let legend = self.legend(code);
        layout::Detail {
            token: catalog::token(&self.catalog, code)
                .filter(|token| !token.eq_ignore_ascii_case(&legend)),
            legend,
            code,
            kind: catalog::kind(code),
            layers: (0..self.layer_count())
                .map(|other| {
                    let code = self.code_at(other, place).unwrap_or(0);
                    let changed = Some(code) != self.saved_code_at(other, place);
                    (self.legend(code), other == layer, changed)
                })
                .collect(),
            saved: (saved != code).then(|| self.legend(saved)),
        }
    }

    /// The header holds the tabs (`L1`.. then the editors), the lock state, and the unsaved count.
    fn frame<'a>(&self, paint: &Paint<'a>) -> layout::Frame<'a> {
        let editors = self.editors();
        let layers = usize::from(self.layer_count());
        let mut tabs: Vec<String> = (1..=layers).map(|n| format!("L{n}")).collect();
        tabs.extend(editors.iter().map(|tab| tab.name().to_string()));
        let active = match paint.spot {
            Some(spot) => layers + editors.iter().position(|tab| *tab == spot.tab).unwrap_or(0),
            None => usize::from(paint.layer),
        };
        layout::Frame {
            header: layout::Header {
                name: self.name.clone(),
                tabs,
                active,
                locked: Some(self.locked),
                unsaved: self.pending().len(),
            },
            mode: paint.mode,
            hold: paint.hold.map(|prompt| self.hold_line(prompt)),
            lower: paint.lower,
            popup: paint.popup,
            notice: paint.notice.map(str::to_string),
            footer: paint.footer,
            cols: paint.cols,
            rows: paint.rows,
        }
    }

    /// The prompt, then a bar that fills while the firmware counts the hold down from 50.
    /// A bar that stays empty means the board does not see every unlock key held.
    fn hold_line(&self, prompt: &str) -> String {
        let Some(left) = self.unlock_left.filter(|_| self.locked) else {
            return prompt.to_string();
        };
        let done = usize::from(UNLOCK_COUNT.saturating_sub(left)) * 10 / usize::from(UNLOCK_COUNT);
        format!("{prompt}  {}{}", "▰".repeat(done), "▱".repeat(10 - done))
    }

    fn sheet(&self, spot: Spot) -> layout::Sheet {
        let key = |code: u16, changed: bool| SheetCell {
            text: self.legend(code),
            tone: catalog::tone(code),
            changed,
        };
        let (header, rows): (Vec<&'static str>, Vec<(String, Vec<SheetCell>)>) = match spot.tab {
            Tab::TapDance => (
                vec!["tap", "hold", "double tap", "tap+hold", "term"],
                self.entries
                    .tap_dances
                    .iter()
                    .zip(&self.saved.tap_dances)
                    .enumerate()
                    .map(|(index, (entry, saved))| {
                        let mut cells: Vec<SheetCell> = (0..4)
                            .map(|slot| key(entry.keys[slot], entry.keys[slot] != saved.keys[slot]))
                            .collect();
                        cells.push(SheetCell {
                            text: format!("{} ms", entry.term),
                            tone: Tone::Key,
                            changed: entry.term != saved.term,
                        });
                        (format!("TD({index})"), cells)
                    })
                    .collect(),
            ),
            Tab::Combos => (
                vec!["key 1", "key 2", "key 3", "key 4", "output"],
                self.entries
                    .combos
                    .iter()
                    .zip(&self.saved.combos)
                    .enumerate()
                    .map(|(index, (entry, saved))| {
                        (
                            format!("combo {index}"),
                            (0..5)
                                .map(|slot| {
                                    key(entry.keys[slot], entry.keys[slot] != saved.keys[slot])
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            Tab::Macros => (
                Vec::new(),
                self.entries
                    .macros
                    .iter()
                    .zip(&self.saved.macros)
                    .enumerate()
                    .map(|(index, (actions, saved))| {
                        let changed = actions != saved;
                        let mut cells: Vec<SheetCell> = actions
                            .iter()
                            .map(|action| SheetCell {
                                changed,
                                ..self.action_cell(action)
                            })
                            .collect();
                        cells.push(SheetCell {
                            text: "+".to_string(),
                            tone: Tone::Trans,
                            changed: false,
                        });
                        (format!("M{index}"), cells)
                    })
                    .collect(),
            ),
        };
        let title = format!(
            "{} {}/{}",
            spot.tab.name().to_lowercase(),
            spot.row + 1,
            rows.len(),
        );
        let status = self.spot_status(spot, &header, &rows);
        layout::Sheet {
            title,
            header,
            rows,
            row: spot.row,
            col: spot.col,
            status,
        }
    }

    fn action_cell(&self, action: &Action) -> SheetCell {
        let (text, tone) = match action {
            Action::Text(text) => (format!("\"{text}\""), Tone::Key),
            Action::Tap(code) => (self.legend(*code), catalog::tone(*code)),
            Action::Down(code) => (format!("↓{}", self.legend(*code)), catalog::tone(*code)),
            Action::Up(code) => (format!("↑{}", self.legend(*code)), catalog::tone(*code)),
            Action::Delay(ms) => (format!("{ms} ms"), Tone::Trans),
        };
        SheetCell {
            text,
            tone,
            changed: false,
        }
    }

    fn spot_status(
        &self,
        spot: Spot,
        header: &[&str],
        rows: &[(String, Vec<SheetCell>)],
    ) -> String {
        let Some((label, cells)) = rows.get(spot.row) else {
            return String::new();
        };
        if spot.tab != Tab::Macros {
            let field = header.get(spot.col).copied().unwrap_or("");
            let value = cells.get(spot.col).map_or("", |cell| cell.text.as_str());
            return format!("{label} {field}: {value}");
        }
        let actions = &self.entries.macros[spot.row];
        let (used, memory) = self.macro_room(&self.entries.macros);
        let what = match actions.get(spot.col) {
            Some(Action::Text(text)) => format!("type \"{text}\""),
            Some(Action::Tap(code)) => format!("tap {}", self.legend(*code)),
            Some(Action::Down(code)) => format!("press {}", self.legend(*code)),
            Some(Action::Up(code)) => format!("release {}", self.legend(*code)),
            Some(Action::Delay(ms)) => format!("wait {ms} ms"),
            None => {
                return format!(
                    "{label} add a step: t tap  p press  r release  s text  w wait  ({used}/{memory} bytes)"
                )
            }
        };
        format!(
            "{label} step {}/{}: {what}  ({used}/{memory} bytes)",
            spot.col + 1,
            actions.len() + 1
        )
    }

    pub(super) fn first_selectable(&self) -> Option<Place> {
        let targets = self.targets();
        let bands = layout::band_of(&targets.iter().map(|target| target.cy).collect::<Vec<_>>());
        top_left(&targets, &bands)
    }

    pub(super) fn last_selectable(&self) -> Option<Place> {
        let targets = self.targets();
        let bands = layout::band_of(&targets.iter().map(|target| target.cy).collect::<Vec<_>>());
        bottom_right(&targets, &bands)
    }

    /// The draft code, which is what the screen shows.
    fn code_at(&self, layer: u8, place: Place) -> Option<u16> {
        code_in(
            &self.keymap.matrix,
            &self.keymap.encoders,
            &self.keymap,
            layer,
            place,
        )
    }

    fn saved_code_at(&self, layer: u8, place: Place) -> Option<u16> {
        code_in(
            &self.saved.matrix,
            &self.saved.encoders,
            &self.keymap,
            layer,
            place,
        )
    }

    /// Puts a change in the draft. Nothing reaches the keyboard until `commit`.
    pub(super) fn stage(&mut self, write: Write) {
        let before = self.draft_of(&write);
        if before == write {
            return;
        }
        self.apply(&write);
        self.undo.push(before);
    }

    /// Takes back the newest staged change. `false` when there is none.
    pub(super) fn undo(&mut self) -> bool {
        let Some(before) = self.undo.pop() else {
            return false;
        };
        self.apply(&before);
        true
    }

    /// The draft becomes what the keyboard holds.
    pub(super) fn discard(&mut self) {
        self.keymap.matrix.clone_from(&self.saved.matrix);
        self.keymap.encoders.clone_from(&self.saved.encoders);
        self.entries.tap_dances.clone_from(&self.saved.tap_dances);
        self.entries.combos.clone_from(&self.saved.combos);
        self.entries.macros.clone_from(&self.saved.macros);
        self.undo.clear();
    }

    /// Every write that would make the keyboard match the draft. Macros count as one.
    pub(super) fn pending(&self) -> Vec<Write> {
        let mut out = Vec::new();
        let cols = usize::from(self.keymap.cols);
        let cells = usize::from(self.keymap.rows) * cols;
        for (index, (draft, saved)) in self
            .keymap
            .matrix
            .iter()
            .zip(&self.saved.matrix)
            .enumerate()
        {
            if draft != saved && cells > 0 {
                let rest = index % cells;
                out.push(Write::Key {
                    layer: (index / cells) as u8,
                    place: Place::Matrix {
                        row: (rest / cols) as u8,
                        col: (rest % cols) as u8,
                    },
                    code: *draft,
                });
            }
        }
        for (layer, (draft, saved)) in self
            .keymap
            .encoders
            .iter()
            .zip(&self.saved.encoders)
            .enumerate()
        {
            for (index, (d, s)) in draft.iter().zip(saved).enumerate() {
                for (direction, dc, sc) in [(0u8, d.0, s.0), (1, d.1, s.1)] {
                    if dc != sc {
                        out.push(Write::Key {
                            layer: layer as u8,
                            place: Place::Encoder {
                                index: index as u8,
                                direction,
                            },
                            code: dc,
                        });
                    }
                }
            }
        }
        for (index, (draft, saved)) in self
            .entries
            .tap_dances
            .iter()
            .zip(&self.saved.tap_dances)
            .enumerate()
        {
            if draft != saved {
                out.push(Write::TapDance {
                    index: index as u8,
                    entry: *draft,
                });
            }
        }
        for (index, (draft, saved)) in self
            .entries
            .combos
            .iter()
            .zip(&self.saved.combos)
            .enumerate()
        {
            if draft != saved {
                out.push(Write::Combo {
                    index: index as u8,
                    entry: *draft,
                });
            }
        }
        if self.entries.macros != self.saved.macros {
            out.push(Write::Macros(self.entries.macros.clone()));
        }
        out
    }

    /// The draft value at the place `write` targets, as a write of its own.
    fn draft_of(&self, write: &Write) -> Write {
        match write {
            Write::Key { layer, place, .. } => Write::Key {
                layer: *layer,
                place: *place,
                code: self.code_at(*layer, *place).unwrap_or(0),
            },
            Write::TapDance { index, .. } => Write::TapDance {
                index: *index,
                entry: self.entries.tap_dances[usize::from(*index)],
            },
            Write::Combo { index, .. } => Write::Combo {
                index: *index,
                entry: self.entries.combos[usize::from(*index)],
            },
            Write::Macros(_) => Write::Macros(self.entries.macros.clone()),
        }
    }

    fn apply(&mut self, write: &Write) {
        match write {
            Write::Key { layer, place, code } => match *place {
                Place::Matrix { row, col } => {
                    let index = self.keymap.cell(*layer, row, col);
                    if let Some(slot) = self.keymap.matrix.get_mut(index) {
                        *slot = *code;
                    }
                }
                Place::Encoder { index, direction } => {
                    if let Some(slot) = self
                        .keymap
                        .encoders
                        .get_mut(usize::from(*layer))
                        .and_then(|row| row.get_mut(usize::from(index)))
                    {
                        if direction == 0 {
                            slot.0 = *code;
                        } else {
                            slot.1 = *code;
                        }
                    }
                }
            },
            Write::TapDance { index, entry } => {
                self.entries.tap_dances[usize::from(*index)] = *entry;
            }
            Write::Combo { index, entry } => self.entries.combos[usize::from(*index)] = *entry,
            Write::Macros(macros) => self.entries.macros.clone_from(macros),
        }
    }

    /// Popup lines for one change: a heading for where, then `was → becomes` per field.
    /// `from_saved` compares with what the keyboard holds, for the unsaved list on quit;
    /// otherwise with the draft, for the confirm before staging.
    pub(super) fn describe(&self, write: &Write, from_saved: bool) -> Vec<PopupLine> {
        let change = |was: String, now: String| PopupLine::Change(was, now);
        let key = |name: &str, was: u16, now: u16| {
            change(format!("{name}  {}", self.legend(was)), self.legend(now))
        };
        match write {
            Write::Key { layer, place, code } => {
                let at = match place {
                    Place::Matrix { row, col } => format!("key {row},{col}"),
                    Place::Encoder { index, direction } => {
                        let turn = if *direction == 0 { "cw" } else { "ccw" };
                        format!("encoder {index} {turn}")
                    }
                };
                let was = if from_saved {
                    self.saved_code_at(*layer, *place)
                } else {
                    self.code_at(*layer, *place)
                };
                vec![
                    PopupLine::Heading(format!("layer {} {at}", layer + 1)),
                    change(self.legend(was.unwrap_or(0)), self.legend(*code)),
                ]
            }
            Write::TapDance { index, entry } => {
                let list = if from_saved {
                    &self.saved.tap_dances
                } else {
                    &self.entries.tap_dances
                };
                let was = list[usize::from(*index)];
                let names = ["tap", "hold", "double tap", "tap+hold"];
                let mut lines = vec![PopupLine::Heading(format!("TD({index})"))];
                lines.extend(
                    (0..4)
                        .filter(|slot| was.keys[*slot] != entry.keys[*slot])
                        .map(|slot| key(names[slot], was.keys[slot], entry.keys[slot])),
                );
                if was.term != entry.term {
                    lines.push(change(
                        format!("term  {} ms", was.term),
                        format!("{} ms", entry.term),
                    ));
                }
                lines
            }
            Write::Combo { index, entry } => {
                let list = if from_saved {
                    &self.saved.combos
                } else {
                    &self.entries.combos
                };
                let was = list[usize::from(*index)];
                let names = ["key 1", "key 2", "key 3", "key 4", "output"];
                let mut lines = vec![PopupLine::Heading(format!("combo {index}"))];
                lines.extend(
                    (0..5)
                        .filter(|slot| was.keys[*slot] != entry.keys[*slot])
                        .map(|slot| key(names[slot], was.keys[slot], entry.keys[slot])),
                );
                lines
            }
            Write::Macros(macros) => {
                let list = if from_saved {
                    &self.saved.macros
                } else {
                    &self.entries.macros
                };
                let steps = |actions: &[Action]| {
                    let text: Vec<String> = actions
                        .iter()
                        .map(|action| self.action_cell(action).text)
                        .collect();
                    if text.is_empty() {
                        "(empty)".to_string()
                    } else {
                        text.join(" ")
                    }
                };
                macros
                    .iter()
                    .zip(list)
                    .enumerate()
                    .filter(|(_, (next, now))| next != now)
                    .flat_map(|(index, (next, now))| {
                        [
                            PopupLine::Heading(format!("M{index}")),
                            change(steps(now), steps(next)),
                        ]
                    })
                    .collect()
            }
        }
    }

    fn targets(&self) -> Vec<Target> {
        let mut targets = Vec::new();
        for row in &self.def.rows {
            for key in row {
                if key.decal || !layout::visible(&self.def, self.options, key) {
                    continue;
                }
                let Some(place) = layout::place_of(key) else {
                    continue;
                };
                let (x, _) = layout::world_point(key, key.x, key.y);
                targets.push(Target {
                    place,
                    cx: x,
                    cy: layout::world_top(key),
                });
            }
        }
        targets
    }
}

struct Target {
    place: Place,
    cx: f64,
    cy: f64,
}

fn top_left(targets: &[Target], bands: &[usize]) -> Option<Place> {
    targets
        .iter()
        .enumerate()
        .min_by(|(left, a), (right, b)| {
            bands[*left]
                .cmp(&bands[*right])
                .then(a.cx.total_cmp(&b.cx))
                .then(left.cmp(right))
        })
        .map(|(_, target)| target.place)
}

fn bottom_right(targets: &[Target], bands: &[usize]) -> Option<Place> {
    targets
        .iter()
        .enumerate()
        .max_by(|(left, a), (right, b)| {
            bands[*left]
                .cmp(&bands[*right])
                .then(a.cx.total_cmp(&b.cx))
                .then(left.cmp(right))
        })
        .map(|(_, target)| target.place)
}

pub(super) struct Hold {
    pub(super) prompt: String,
}

pub(super) enum Commit {
    Stored,
    Same,
    Holding(Hold),
}

pub(super) struct Paint<'a> {
    pub(super) layer: u8,
    pub(super) selection: Option<Place>,
    /// `None` draws the keymap. `Some` draws that tab's sheet.
    pub(super) spot: Option<Spot>,
    /// One line in place of the sheet status, after a write the board cannot take.
    pub(super) notice: Option<&'a str>,
    pub(super) hold: Option<&'a str>,
    pub(super) lower: Option<Lower<'a>>,
    pub(super) popup: Option<&'a Popup>,
    pub(super) mode: layout::Mode,
    pub(super) footer: &'a [(String, &'static str)],
    pub(super) cols: u16,
    pub(super) rows: u16,
}

/// What the chooser says on one card besides the board itself.
pub(super) enum CardNote<'a> {
    None,
    /// This board would not open, and why.
    Failed(usize, &'a str),
    /// The highlighted board is being read.
    Opening(Progress),
}

pub(super) fn pick_text(
    boards: &[Candidate],
    cursor: usize,
    note: CardNote<'_>,
    notice: Option<&str>,
    popup: Option<&Popup>,
    mode: layout::Mode,
    footer: &[(String, &'static str)],
    cols: u16,
    rows: u16,
) -> String {
    layout::render_pick(
        boards,
        cursor,
        note,
        layout::Frame {
            header: layout::Header {
                name: "choose a keyboard".to_string(),
                tabs: Vec::new(),
                active: 0,
                locked: None,
                unsaved: 0,
            },
            mode,
            hold: None,
            lower: None,
            popup,
            notice: notice.map(str::to_string),
            footer,
            cols,
            rows,
        },
    )
}

fn exchange(io: &mut dyn Io, ask: &Ask) -> Result<Reply> {
    let resp = io.exchange(&encode(ask))?;
    decode(ask, &resp)
}

fn cell(layer: u8, row: u8, col: u8, rows: u8, cols: u8) -> usize {
    ((layer as usize * rows as usize) + row as usize) * cols as usize + col as usize
}

/// The code at `place` in one copy of the keymap. `shape` supplies the matrix size.
fn code_in(
    matrix: &[u16],
    encoders: &[Vec<(u16, u16)>],
    shape: &Keymap,
    layer: u8,
    place: Place,
) -> Option<u16> {
    match place {
        Place::Matrix { row, col } => matrix.get(shape.cell(layer, row, col)).copied(),
        Place::Encoder { index, direction } => {
            let slot = encoders.get(usize::from(layer))?.get(usize::from(index))?;
            Some(if direction == 0 { slot.0 } else { slot.1 })
        }
    }
}

fn cell_count(layers: u8, rows: u8, cols: u8) -> Result<usize> {
    (layers as usize)
        .checked_mul(rows as usize)
        .and_then(|n| n.checked_mul(cols as usize))
        .ok_or(Error::Protocol("keymap size"))
}

fn keymap_bytes(matrix: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(matrix.len() * 2);
    for code in matrix {
        bytes.extend_from_slice(&code.to_be_bytes());
    }
    bytes
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
    encoder
        .write_all(bytes)
        .map_err(|_| Error::Protocol("definition compress failed"))?;
    encoder
        .finish()
        .map_err(|_| Error::Protocol("definition compress failed"))
}

fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = xz2::read::XzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|_| Error::NoLayout)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen without colors, as a person reads it.
    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                chars.by_ref().find(|c| *c == 'm');
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn opening_shows_progress_and_esc_goes_back() {
        let board = MemoryBoard::from_definition(
            r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
        )
        .unwrap()
        .named("slow");
        let mut candidates = vec![board.candidate()];
        // A board part-way read, as a slow Bluetooth board is between ticks.
        let mut load = Load::begin(&candidates[0]).unwrap();
        for _ in 0..3 {
            assert!(!load.step(&mut candidates[0]).unwrap());
        }
        let mut model = super::super::Model {
            kind: super::super::ModelKind::Open {
                candidates,
                cursor: 0,
                load,
            },
            notice: None,
        };
        let text = plain(&model.text(60, 12));
        assert!(text.contains("opening…"), "{text}");
        assert!(text.contains('▰') && text.contains("layout"), "{text}");
        model.handle(crate::Event::Cancel).unwrap();
        let text = plain(&model.text(60, 12));
        assert!(text.contains("stopped opening"), "{text}");
        assert!(text.contains("▌ slow"), "{text}");
        // The same board opens from the start again.
        model.handle(crate::Event::Confirm).unwrap();
        assert!(model.text(60, 12).contains("layer 1/"));
    }

    #[test]
    fn one_board_still_asks_which() {
        let board = MemoryBoard::from_definition(
            r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
        )
        .unwrap()
        .named("only");
        let attached = super::super::list_first(vec![board.candidate()]).unwrap();
        let text = plain(&super::super::Model::start(attached).text(40, 8));
        assert!(text.contains("choose"), "{text}");
        assert!(text.contains("▌ only"), "{text}");
        assert!(!text.contains("layer "), "{text}");
    }

    #[test]
    fn hold_names_survive_the_poll() {
        let board = MemoryBoard::from_definition(
            r#"{"matrix":{"rows":2,"cols":2},"layouts":{"keymap":[["0,0\n\n\n\n\n\n\n\nTab","0,1"]]}}"#,
        )
        .unwrap()
        .locked(&[(0, 0), (0, 1), (0, 0), (8, 0)]);
        board.store.borrow_mut().unlock_waits = 1;
        let mut app = crate::App::new(board.open().unwrap(), crate::TestScreen::new(60, 16));
        app.handle(crate::Event::Char('/')).unwrap();
        assert!(app.screen().text().contains("[Basic]"));
        for ch in "boot".chars() {
            app.handle(crate::Event::Char(ch)).unwrap();
        }
        app.handle(crate::Event::Confirm).unwrap();
        let text = plain(app.screen().text());
        assert!(text.contains("╭─ stage this change?"), "{text}");
        assert!(text.contains("NO  →  Boot"), "{text}");
        app.handle(crate::Event::Confirm).unwrap();
        // Staged only: the keyboard has nothing yet.
        assert_eq!(board.store.borrow().matrix[0], 0);
        assert!(app.screen().text().contains("1 unsaved"));
        for ch in ":w".chars() {
            app.handle(crate::Event::Char(ch)).unwrap();
        }
        app.handle(crate::Event::Confirm).unwrap();
        let text = app.screen().text();
        assert!(text.contains("hold Tab+0,1 to write Boot"), "{text}");
        assert!(!text.contains("8,0"), "{text}");
        app.handle(crate::Event::Tick).unwrap();
        let text = app.screen().text();
        assert!(text.contains("locked"), "{text}");
        assert!(text.contains("hold Tab+0,1 to write Boot"), "{text}");
        assert!(text.contains('▰'), "the countdown shows: {text}");
        assert!(!text.contains("2,0"), "{text}");
        // Corne's stock combo is Tab+Q. Held, it types into the terminal; none of it may act.
        for event in [
            crate::Event::Tab,
            crate::Event::Char('q'),
            crate::Event::Char('?'),
        ] {
            assert_eq!(app.handle(event).unwrap(), crate::Step::Continue);
        }
        app.handle(crate::Event::Tick).unwrap();
        let text = app.screen().text();
        assert!(text.contains("let go of the keys"), "{text}");
        // Still typing: `q` would quit the keymap, so it is dropped.
        assert_eq!(
            app.handle(crate::Event::Char('q')).unwrap(),
            crate::Step::Continue
        );
        app.handle(crate::Event::Tick).unwrap();
        assert!(app.screen().text().contains("let go of the keys"));
        app.handle(crate::Event::Tick).unwrap();
        let text = app.screen().text();
        assert!(text.contains("ready"), "{text}");
        // Written after the unlock, so the firmware kept it rather than firewalling it.
        assert_eq!(board.store.borrow().matrix[0], QK_BOOT);
    }
}
