use super::Attached;
use crate::{Error, Result};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::sync::Arc;

mod catalog;
mod layout;

use catalog::Table;
use layout::{Cap, Definition};

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
    options: u32,
    locked: bool,
    unlock_pairs: Vec<(u8, u8)>,
    unlock_armed: bool,
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
                if self.locked {
                    return Ok(Reply::Ack);
                }
                let index = cell(layer, row, col, self.rows, self.cols);
                if index >= self.matrix.len() {
                    return Err(Error::Protocol("keycode is outside the matrix"));
                }
                self.matrix[index] = code;
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
                if self.locked {
                    return Ok(Reply::Ack);
                }
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
                    self.locked = false;
                    self.unlock_armed = false;
                }
                Ok(self.unlock_reply())
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
}

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
            options: 0,
            locked: false,
            unlock_pairs: Vec::new(),
            unlock_armed: false,
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
            name,
            io: Some(Box::new(FakeIo {
                store: Arc::clone(&self.store),
            })),
            guard: Some(Arc::clone(&self.store)),
        }
    }
}

pub(super) struct Candidate {
    pub(super) name: String,
    io: Option<Box<dyn Io>>,
    guard: Option<Arc<RefCell<Store>>>,
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
            name,
            io: Some(Box::new(io)),
            guard: None,
        });
    }
    Ok(found)
}

/// Id, version, definition, layout dword, layer count, keymap chunks, encoders,
/// unlock status. Sets `Store.open` when the `Io` is a fake.
pub(super) fn load(candidate: Candidate) -> Result<Board> {
    match open_board(candidate) {
        Ok(board) => Ok(board),
        Err((err, _)) => Err(err),
    }
}

pub(super) fn load_keep(candidate: Candidate) -> std::result::Result<Board, (Error, Candidate)> {
    open_board(candidate)
}

fn open_board(mut candidate: Candidate) -> std::result::Result<Board, (Error, Candidate)> {
    if let Some(guard) = &candidate.guard {
        if guard.borrow().open {
            return Err((Error::Busy, candidate));
        }
    }
    let Some(mut io) = candidate.io.take() else {
        return Err((Error::Disconnected, candidate));
    };
    let loaded = match read_board(&mut *io) {
        Ok(loaded) => loaded,
        Err(err) => {
            candidate.io = Some(io);
            return Err((err, candidate));
        }
    };
    let guard = candidate.guard.clone();
    if let Some(guard) = &guard {
        guard.borrow_mut().open = true;
    }
    Ok(Board {
        name: candidate.name.clone(),
        def: loaded.def,
        catalog: loaded.catalog,
        keymap: loaded.keymap,
        options: loaded.options,
        locked: loaded.locked,
        io,
        _guard: guard,
    })
}

struct Loaded {
    def: Definition,
    catalog: Table,
    keymap: Keymap,
    options: u32,
    locked: bool,
}

fn read_board(io: &mut dyn Io) -> Result<Loaded> {
    let Reply::Id { version, .. } = exchange(io, &Ask::KeyboardId)? else {
        return Err(Error::Protocol("keyboard id"));
    };
    if !(-1..=6).contains(&version) {
        return Err(Error::Unsupported { vial: version });
    }
    let Reply::DefSize(size) = exchange(io, &Ask::DefinitionSize)? else {
        return Err(Error::Protocol("definition size"));
    };
    if size == 0 {
        return Err(Error::NoLayout);
    }
    let mut compressed = Vec::with_capacity(size as usize);
    let mut remaining = size;
    let mut block = 0u32;
    while remaining > 0 {
        let Reply::DefBlock(chunk) = exchange(io, &Ask::Definition { block })? else {
            return Err(Error::Protocol("definition block"));
        };
        // The report is 32 bytes. The last block's tail is padding, so the declared size decides the cut.
        let take = (remaining as usize).min(32);
        if chunk.len() < take {
            return Err(Error::Protocol("definition block"));
        }
        compressed.extend_from_slice(&chunk[..take]);
        remaining -= take as u32;
        block += 1;
    }
    let json = decompress(&compressed)?;
    let text = String::from_utf8(json).map_err(|_| Error::NoLayout)?;
    let def = layout::parse(&text)?;
    let Reply::LayoutOptions(options) = exchange(io, &Ask::LayoutOptions)? else {
        return Err(Error::Protocol("layout options"));
    };
    let Reply::LayerCount(layers) = exchange(io, &Ask::LayerCount)? else {
        return Err(Error::Protocol("layer count"));
    };
    let rows = def.rows_n;
    let cols = def.cols_n;
    let cells = cell_count(layers, rows, cols)?;
    let mut matrix = vec![0u16; cells];
    let byte_len = cells.checked_mul(2).ok_or(Error::Protocol("keymap size"))?;
    let mut offset = 0usize;
    while offset < byte_len {
        let len = (byte_len - offset).min(CHUNK as usize) as u8;
        let offset_u16 = u16::try_from(offset).map_err(|_| Error::Protocol("keymap size"))?;
        let Reply::Chunk(chunk) = exchange(
            io,
            &Ask::KeymapChunk {
                offset: offset_u16,
                len,
            },
        )?
        else {
            return Err(Error::Protocol("keymap chunk"));
        };
        if chunk.len() < len as usize {
            return Err(Error::Protocol("keymap chunk"));
        }
        for index in 0..(len as usize / 2) {
            let byte = index * 2;
            let code = u16::from_be_bytes([chunk[byte], chunk[byte + 1]]);
            let cell_index = (offset / 2) + index;
            if cell_index < matrix.len() {
                matrix[cell_index] = code;
            }
        }
        offset += len as usize;
    }
    let encoder_count = layout::encoder_count(&def);
    let mut encoders = vec![vec![(0u16, 0u16); encoder_count]; layers as usize];
    for layer in 0..layers {
        for index in 0..encoder_count {
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
            encoders[layer as usize][index] = (cw, ccw);
        }
    }
    let Reply::Unlock { unlocked, .. } = exchange(io, &Ask::UnlockStatus)? else {
        return Err(Error::Protocol("unlock status"));
    };
    Ok(Loaded {
        catalog: catalog::merge(&def.custom),
        keymap: Keymap {
            layers,
            rows,
            cols,
            matrix,
            encoders,
        },
        options,
        locked: !unlocked,
        def,
    })
}

pub(super) struct Board {
    name: String,
    def: Definition,
    catalog: Table,
    keymap: Keymap,
    options: u32,
    locked: bool,
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
            Ask::LayerCount | Ask::LayoutOptions | Ask::KeymapChunk { .. } | Ask::SetKey { .. } => {
                Grammar::Echo
            }
            Ask::KeyboardId
            | Ask::DefinitionSize
            | Ask::Definition { .. }
            | Ask::GetEncoder { .. }
            | Ask::SetEncoder { .. }
            | Ask::UnlockStatus
            | Ask::UnlockStart
            | Ask::UnlockPoll => Grammar::Overwrite,
        }
    }
}

const CHUNK: u8 = 28;
const GET_KEYBOARD_VALUE: u8 = 0x02;
const LAYOUT_OPTIONS_ID: u8 = 0x02;
const SET_KEYCODE: u8 = 0x05;
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
const FAKE_VERSION: i32 = 6;

fn encode(ask: &Ask) -> [u8; 32] {
    let mut buf = [0u8; 32];
    match *ask {
        Ask::LayerCount => buf[0] = LAYER_COUNT,
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
        Ask::LayoutOptions => {
            let dword = u32::from_be_bytes(resp[2..6].try_into().unwrap());
            Ok(Reply::LayoutOptions(dword))
        }
        Ask::KeymapChunk { len, .. } => {
            if len > CHUNK {
                return Err(Error::Protocol("keymap chunk length"));
            }
            Ok(Reply::Chunk(resp[4..4 + len as usize].to_vec()))
        }
        Ask::SetKey { .. } => Ok(Reply::Ack),
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
        Ask::UnlockStatus | Ask::UnlockPoll => Ok(Reply::Unlock {
            unlocked: resp[0] != 0,
            pairs: unlock_pairs(resp),
        }),
        _ => Err(Error::Protocol("overwrite response")),
    }
}

fn decode_request(report: &[u8; 32]) -> Result<Ask> {
    match report[0] {
        LAYER_COUNT => Ok(Ask::LayerCount),
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
            _ => Err(Error::Protocol("unknown vial command")),
        },
        _ => Err(Error::Protocol("unknown command")),
    }
}

enum Reply {
    LayerCount(u8),
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
}

fn encode_reply(ask: &Ask, reply: &Reply) -> [u8; 32] {
    let mut buf = match ask.grammar() {
        Grammar::Echo => encode(ask),
        Grammar::Overwrite => [0u8; 32],
    };
    match (ask, reply) {
        (Ask::LayerCount, Reply::LayerCount(count)) => buf[1] = *count,
        (Ask::LayoutOptions, Reply::LayoutOptions(dword)) => {
            buf[2..6].copy_from_slice(&dword.to_be_bytes());
        }
        (Ask::KeymapChunk { len, .. }, Reply::Chunk(data)) => {
            let n = (*len as usize).min(data.len()).min(CHUNK as usize);
            buf[4..4 + n].copy_from_slice(&data[..n]);
        }
        (Ask::SetKey { .. }, Reply::Ack) => {}
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
        (Ask::UnlockStatus | Ask::UnlockPoll, Reply::Unlock { unlocked, pairs }) => {
            buf[0] = u8::from(*unlocked);
            // Unused combo slots are 255,255. A zero byte would be row 0, column 0.
            for slot in 0..15 {
                let (row, col) = pairs.get(slot).copied().unwrap_or((255, 255));
                buf[2 + slot * 2] = row;
                buf[3 + slot * 2] = col;
            }
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

fn pairs_text(pairs: &[(u8, u8)]) -> String {
    pairs
        .iter()
        .map(|(row, col)| format!("{row},{col}"))
        .collect::<Vec<_>>()
        .join("+")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Left,
    Right,
    Up,
    Down,
}

pub(in crate::session) use layout::Place;

impl Board {
    fn exchange(&mut self, ask: &Ask) -> Result<Reply> {
        exchange(&mut *self.io, ask)
    }

    pub(super) fn begin_hold(&mut self, code: u16) -> Result<Option<Hold>> {
        let Reply::Unlock { unlocked, pairs } = self.exchange(&Ask::UnlockStatus)? else {
            return Err(Error::Protocol("unlock status"));
        };
        self.locked = !unlocked;
        if self.locked || self.hold_first(code) {
            self.exchange(&Ask::UnlockStart)?;
            return Ok(Some(Hold {
                prompt: pairs_text(&pairs),
            }));
        }
        Ok(None)
    }

    /// The only set-key and set-encoder sender.
    pub(super) fn commit(&mut self, layer: u8, place: Place, code: u16) -> Result<Commit> {
        if self.code_at(layer, place) == Some(code) {
            return Ok(Commit::Same);
        }
        if self.locked {
            return Ok(Commit::Holding(Hold {
                prompt: String::new(),
            }));
        }
        match place {
            Place::Matrix { row, col } => {
                self.exchange(&Ask::SetKey {
                    layer,
                    row,
                    col,
                    code,
                })?;
                let index = self.keymap.cell(layer, row, col);
                if index >= self.keymap.matrix.len() {
                    return Err(Error::Protocol("keycode is outside the matrix"));
                }
                self.keymap.matrix[index] = code;
            }
            Place::Encoder { index, direction } => {
                self.exchange(&Ask::SetEncoder {
                    layer,
                    index,
                    direction,
                    code,
                })?;
                let slot = self
                    .keymap
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
        Ok(Commit::Stored)
    }

    pub(super) fn poll_unlock(&mut self) -> Result<Option<Hold>> {
        let Reply::Unlock { unlocked, pairs } = self.exchange(&Ask::UnlockPoll)? else {
            return Err(Error::Protocol("unlock poll"));
        };
        self.locked = !unlocked;
        if self.locked {
            Ok(Some(Hold {
                prompt: pairs_text(&pairs),
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn search(&self, query: &str) -> Vec<catalog::Found> {
        catalog::search(&self.catalog, query)
    }

    pub(super) fn hold_first(&self, code: u16) -> bool {
        catalog::hold_first(&self.catalog, code)
    }

    pub(super) fn nudge(&self, _layer: u8, selection: Option<Place>, dir: Dir) -> Option<Place> {
        let targets = self.targets();
        let Some(current) =
            selection.and_then(|place| targets.iter().position(|target| target.place == place))
        else {
            return targets.first().map(|target| target.place);
        };
        let origin = (targets[current].cx, targets[current].cy);
        let mut best: Option<(f64, f64, usize, Place)> = None;
        for (index, target) in targets.iter().enumerate() {
            if index == current {
                continue;
            }
            let point = (target.cx, target.cy);
            if !facing(dir, origin, point) {
                continue;
            }
            let primary = dist2(origin, point);
            let perp = perpendicular(dir, origin, point);
            let replace = match best {
                None => true,
                Some((best_primary, best_perp, best_index, _)) => nearer(
                    (primary, perp, index),
                    (best_primary, best_perp, best_index),
                ),
            };
            if replace {
                best = Some((primary, perp, index, target.place));
            }
        }
        best.map(|(_, _, _, place)| place).or(selection)
    }

    pub(super) fn layer_count(&self) -> u8 {
        self.keymap.layers
    }

    pub(super) fn text(&self, paint: &Paint<'_>) -> String {
        let mut caps = Vec::new();
        for (row, keys) in self.def.rows.iter().enumerate() {
            for key in keys {
                if !layout::visible(&self.def, self.options, key) {
                    continue;
                }
                let place = layout::place_of(key);
                let legend = if !key.labels[1].is_empty() {
                    key.labels[1].clone()
                } else if let Some(place) = place {
                    catalog::legend(&self.catalog, self.code_at(paint.layer, place).unwrap_or(0))
                } else if !key.labels[0].is_empty() {
                    key.labels[0].clone()
                } else {
                    String::new()
                };
                caps.push(Cap {
                    row,
                    place,
                    legend,
                    selected: place.is_some() && paint.selection == place,
                    decal: key.decal,
                    x: key.x,
                    y: key.y,
                    w: key.w,
                    h: key.h,
                });
            }
        }
        layout::render_board(
            &self.name,
            paint.layer.saturating_add(1),
            self.layer_count(),
            self.locked,
            &caps,
            paint.hold,
            paint.query,
            paint.hits,
            paint.cursor,
            paint.cols,
            paint.rows,
        )
    }

    pub(super) fn first_selectable(&self) -> Option<Place> {
        self.targets().first().map(|target| target.place)
    }

    fn code_at(&self, layer: u8, place: Place) -> Option<u16> {
        match place {
            Place::Matrix { row, col } => {
                let index = self.keymap.cell(layer, row, col);
                self.keymap.matrix.get(index).copied()
            }
            Place::Encoder { index, direction } => {
                let slot = self
                    .keymap
                    .encoders
                    .get(layer as usize)?
                    .get(index as usize)?;
                Some(if direction == 0 { slot.0 } else { slot.1 })
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
                targets.push(Target {
                    place,
                    cx: key.x + key.w / 2.0,
                    cy: key.y + key.h / 2.0,
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

fn nearer(candidate: (f64, f64, usize), best: (f64, f64, usize)) -> bool {
    candidate
        .0
        .total_cmp(&best.0)
        .then(candidate.1.total_cmp(&best.1))
        .then(candidate.2.cmp(&best.2))
        == Ordering::Less
}

fn facing(dir: Dir, from: (f64, f64), to: (f64, f64)) -> bool {
    match dir {
        Dir::Right => to.0 > from.0,
        Dir::Left => to.0 < from.0,
        Dir::Down => to.1 > from.1,
        Dir::Up => to.1 < from.1,
    }
}

fn dist2(from: (f64, f64), to: (f64, f64)) -> f64 {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    dx * dx + dy * dy
}

fn perpendicular(dir: Dir, from: (f64, f64), to: (f64, f64)) -> f64 {
    match dir {
        Dir::Left | Dir::Right => (to.1 - from.1).abs(),
        Dir::Up | Dir::Down => (to.0 - from.0).abs(),
    }
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
    pub(super) hold: Option<&'a str>,
    pub(super) query: Option<&'a str>,
    pub(super) hits: &'a [String],
    pub(super) cursor: usize,
    pub(super) cols: u16,
    pub(super) rows: u16,
}

pub(super) fn pick_text(
    names: &[String],
    cursor: usize,
    failed: Option<usize>,
    cols: u16,
    rows: u16,
) -> String {
    layout::render_pick(names, cursor, failed, cols, rows)
}

fn exchange(io: &mut dyn Io, ask: &Ask) -> Result<Reply> {
    let resp = io.exchange(&encode(ask))?;
    decode(ask, &resp)
}

fn cell(layer: u8, row: u8, col: u8, rows: u8, cols: u8) -> usize {
    ((layer as usize * rows as usize) + row as usize) * cols as usize + col as usize
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
