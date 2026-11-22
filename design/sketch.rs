//! Candidate sketch for lazyvial. Not the crate, and not an implementation.
//!
//! Public API, re-exported at the bottom:
//! `attach`, `Attached`, `MemoryBoard`, `App`, `Event`, `Step`, `Screen`,
//! `TestScreen`, `Error`, `Result`.
//!
//! Module tree. Visibility is the boundary.
//!
//! ```text
//! crate
//!   app.rs          Screen, TestScreen, App. Forwards events. Paints the string.
//!   session.rs      Event, Model, Session phases, fan_in, attach.
//!     vial.rs       Ask, Io, HID, MemoryBoard, Board, commit.
//!       layout.rs   KLE, layout-option filter, Place, render.
//!       catalog.rs  one keycode table.
//!   main.rs         Terminal. Not a library export.
//! ```
//!
//! A write crosses `App::handle` to `Model::handle` to `Board::commit`.
//! Command bytes stay in `vial`.

#![allow(dead_code)]

pub type Result<T> = std::result::Result<T, Error>;

/// Failures a caller can print. Variants carry no report bytes.
#[derive(Debug)]
pub enum Error {
    /// `attach` and `choose` found no Vial board.
    NoBoard,
    /// The board answered, and it has no usable Vial definition.
    /// The program prints this and does not invent a grid.
    NoLayout,
    /// Vial version outside -1 through 6, interpreted as `i32` from the `u32` bits.
    Unsupported { vial: i32 },
    /// The device or the tty went away.
    Disconnected,
    /// This store already has a live session. Drop that session before `open`.
    Busy,
    /// A report or a definition failed a check inside `vial`. The string is ours.
    Protocol(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoBoard => write!(f, "no Vial keyboard found"),
            Error::NoLayout => write!(f, "this keyboard has no Vial layout"),
            Error::Unsupported { vial } => {
                write!(f, "unsupported Vial protocol version {vial}")
            }
            Error::Disconnected => write!(f, "the keyboard disconnected"),
            Error::Busy => write!(f, "that keyboard is already open"),
            Error::Protocol(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

mod session {
    use crate::{Error, Result};

    /// What the binary and the tests send. `Model::handle` is the only match.
    #[derive(Debug)]
    pub enum Event {
        Left,
        Right,
        Up,
        Down,
        NextLayer,
        PrevLayer,
        Char(char),
        Backspace,
        Confirm,
        Cancel,
        /// One unlock poll. The binary's timer produces this. A person does not.
        Tick,
        Quit,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Step {
        Continue,
        Quit,
    }

    /// An opened chooser or a loaded board. No public fields. `App::new` consumes it.
    pub struct Attached {
        kind: Kind,
    }

    enum Kind {
        Ready(Session<Mapping>),
        Pick(Vec<vial::Candidate>),
    }

    /// The product loop. `app` can call `start`, `handle`, and `text`.
    /// It cannot name `Session`, `Place`, or `Ask`.
    pub(crate) struct Model {
        kind: ModelKind,
    }

    enum ModelKind {
        Pick {
            names: Vec<String>,
            candidates: Vec<vial::Candidate>,
            cursor: usize,
            failed: Option<usize>,
        },
        Map(Session<Mapping>),
        /// Query and cursor live here, not on the board.
        Search {
            session: Session<Mapping>,
            query: String,
            cursor: usize,
        },
        Hold(Session<Holding>),
    }

    /// Mapping has no pending code. The type is the invariant.
    struct Mapping;

    /// The one pending write. `poll` is the only path that commits it.
    struct Holding {
        place: vial::Place,
        code: u16,
        prompt: String,
    }

    struct Session<Phase> {
        board: vial::Board,
        /// 0-based. `text` prints `layer + 1`.
        layer: u8,
        selection: Option<vial::Place>,
        phase: Phase,
    }

    /// A code only `Board::search` can construct. The `u16` stays in this module.
    struct Hit {
        code: u16,
        legend: String,
    }

    impl Hit {
        fn legend(&self) -> &str {
            &self.legend
        }
    }

    /// Zero candidates: `NoBoard`. One candidate: load it. Many: the choose screen,
    /// and load waits for `Confirm`. `attach`, `open`, and `choose` all call this.
    fn fan_in(candidates: Vec<vial::Candidate>) -> Result<Attached> {
        let _ = candidates;
        // 0 -> Err(NoBoard)
        // 1 -> vial::load, Kind::Ready
        // n -> Kind::Pick, no definition read yet
        unimplemented!()
    }

    /// HID enumerate, then `fan_in`. Usage page `0xFF60`, usage `0x61`.
    /// A device that does not answer the vial id is skipped.
    /// Version bits are read as `i32`. Outside -1 through 6 is `Unsupported` at load.
    pub fn attach() -> Result<Attached> {
        fan_in(vial::enumerate()?)
    }

    impl Model {
        pub(crate) fn start(attached: Attached) -> Self {
            let _ = attached;
            // Ready -> Map, selection = first selectable place, layer = 0.
            // Pick -> Pick with cursor 0.
            unimplemented!()
        }

        /// Policy for every event. After this returns, `App` paints `text`.
        ///
        /// Quit returns `Step::Quit` and sends no report.
        /// Pick arrows move the cursor. Pick Confirm calls `vial::load`.
        /// `NoLayout` on that load sets `failed` and stays on the list.
        /// Map arrows call `Board::nudge`. Layer keys wrap.
        /// Map `Char` enters `Search`. Search arrows move the hit cursor.
        /// Search Confirm calls `Session::assign` on the highlighted `Hit`.
        /// Empty search Confirm does not write. Search Cancel returns to Map.
        /// Hold Tick calls `Session::poll`. Hold Cancel drops the pending code.
        /// Hold ignores every other event except Quit.
        pub(crate) fn handle(&mut self, event: Event) -> Result<Step> {
            let _ = event;
            unimplemented!()
        }

        /// The only string `App` paints. Pick uses `vial::pick_text`.
        /// A board uses `Board::text`.
        pub(crate) fn text(&self, cols: u16, rows: u16) -> String {
            let _ = (cols, rows);
            unimplemented!()
        }
    }

    impl Session<Mapping> {
        /// `begin_hold` then maybe `commit`. No selection leaves the session mapped
        /// and sends nothing. `Hit.code` is readable here and nowhere in `app`.
        fn assign(self, hit: Hit) -> Assign {
            let _ = hit;
            unimplemented!()
        }

        fn search(&self, query: &str) -> Vec<Hit> {
            // Board::search walks one vec. Wrap each found code in Hit.
            let _ = query;
            unimplemented!()
        }

        fn nudge(&mut self, dir: vial::Dir) {
            let _ = dir;
            // selection = board.nudge(layer, selection, dir)
            unimplemented!()
        }

        fn layer_step(&mut self, delta: i8) {
            let _ = delta;
            // wrap inside 0..layer_count. The count came from command 0x11.
            unimplemented!()
        }
    }

    impl Session<Holding> {
        /// Unlock poll, then `commit` of `phase.place` and `phase.code`.
        /// Still locked: stay in `Waiting` and do not write.
        fn poll(self) -> HoldStep {
            unimplemented!()
        }

        /// Drop the pending code. The board stays as it was. No report.
        fn cancel(self) -> Session<Mapping> {
            unimplemented!()
        }
    }

    enum Assign {
        Mapped(Session<Mapping>),
        Holding(Session<Holding>),
        Disconnected(Error),
    }

    enum HoldStep {
        Waiting(Session<Holding>),
        Mapped(Session<Mapping>),
        Disconnected(Error),
    }

    pub mod vial {
        use super::Attached;
        use crate::Result;
        use std::cell::RefCell;
        use std::sync::Arc;

        /// 32-byte message, without the HID report id.
        trait Io {
            fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]>;
        }

        /// Prepends `0x00` on the write and strips nothing from the 32-byte read.
        struct HidIo {
            _device: (),
        }

        impl Io for HidIo {
            fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
                let _ = report;
                unimplemented!()
            }
        }

        /// Speaks `Ask` through the same encode and decode as HID.
        struct FakeIo {
            store: Arc<RefCell<Store>>,
        }

        impl Io for FakeIo {
            fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
                let _ = report;
                // ask = decode_request(report)
                // reply = store.answer(ask)
                // encode_reply(ask, reply)
                // Unlock start sets unlock_armed. The next unlock poll clears locked.
                unimplemented!()
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
            open: bool,
        }

        /// Shares the store. A clone is the same keyboard, not a copy of the keys.
        /// `!Send` because the store is a `RefCell`. One thread. No mutex.
        #[derive(Clone)]
        pub struct MemoryBoard {
            store: Arc<RefCell<Store>>,
        }

        impl MemoryBoard {
            /// Parse `json`, reject a matrix address the matrix cannot hold, size the
            /// keymap to `KC_NO`, LZMA-compress the same bytes load will decompress.
            /// Default name `board`, one layer, layout-option dword 0, unlocked.
            pub fn from_definition(json: &str) -> Result<Self> {
                let _ = json;
                unimplemented!()
            }

            /// What command `0x11` will answer.
            pub fn layers(self, count: u8) -> Self {
                let _ = count;
                unimplemented!()
            }

            /// What get-keyboard-value `0x02` / id `0x02` will answer.
            pub fn layout_options(self, dword: u32) -> Self {
                let _ = dword;
                unimplemented!()
            }

            /// Status starts locked. Pairs are the hold, in the order poll prints.
            /// The first unlock poll after unlock start reports ready.
            pub fn locked(self, keys: &[(u8, u8)]) -> Self {
                let _ = keys;
                unimplemented!()
            }

            /// Status-line name, and the choose-list name.
            pub fn named(self, name: impl Into<String>) -> Self {
                let _ = name;
                unimplemented!()
            }

            pub fn open(&self) -> Result<Attached> {
                super::fan_in(vec![self.candidate()])
            }

            pub fn choose(boards: &[MemoryBoard]) -> Result<Attached> {
                super::fan_in(boards.iter().map(MemoryBoard::candidate).collect())
            }

            fn candidate(&self) -> Candidate {
                unimplemented!()
            }
        }

        pub(super) struct Candidate {
            name: String,
            io: Box<dyn Io>,
        }

        /// Skip non-Vial HID devices. Do not read the definition here.
        pub(super) fn enumerate() -> Result<Vec<Candidate>> {
            unimplemented!()
        }

        /// Id, version, definition, layout dword, layer count, keymap chunks, encoders,
        /// unlock status. Sets `Store.open` when the `Io` is a fake. Definition size 0,
        /// bad KLE, or a version outside -1 through 6 returns `Err` and no `Board`.
        pub(super) fn load(candidate: Candidate) -> Result<Board> {
            let _ = candidate;
            unimplemented!()
        }

        pub(super) struct Board {
            name: String,
            def: layout::Definition,
            catalog: catalog::Table,
            keymap: Keymap,
            options: u32,
            locked: bool,
            /// Clears `Store.open` on drop when this board came from a fake.
            _guard: Option<Arc<RefCell<Store>>>,
        }

        impl Drop for Board {
            fn drop(&mut self) {
                // `open = false` on the fake store. HID has no store.
                // Crash and normal quit take the same path. No lock report.
                let _ = &mut self._guard;
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
            /// One index. `((layer * rows) + row) * cols + col`.
            fn cell(&self, layer: u8, row: u8, col: u8) -> usize {
                let _ = (layer, row, col);
                unimplemented!()
            }
        }

        /// Private so `app` cannot match it. Response shape is a property of the ask.
        /// There is no lock ask, no buffer-set ask, and no lighting ask.
        enum Ask {
            LayerCount,
            LayoutOptions,
            KeymapChunk { offset: u16, len: u8 },
            SetKey { layer: u8, row: u8, col: u8, code: u16 },
            KeyboardId,
            DefinitionSize,
            Definition { block: u32 },
            GetEncoder { layer: u8, index: u8 },
            SetEncoder { layer: u8, index: u8, direction: u8, code: u16 },
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
                    Ask::LayerCount
                    | Ask::LayoutOptions
                    | Ask::KeymapChunk { .. }
                    | Ask::SetKey { .. } => Grammar::Echo,
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

        fn encode(ask: &Ask) -> [u8; 32] {
            let _ = (
                ask,
                CHUNK,
                GET_KEYBOARD_VALUE,
                LAYOUT_OPTIONS_ID,
                SET_KEYCODE,
                LAYER_COUNT,
                GET_BUFFER,
                VIAL,
                VIAL_ID,
                VIAL_DEF_SIZE,
                VIAL_DEF,
                VIAL_GET_ENCODER,
                VIAL_SET_ENCODER,
                VIAL_UNLOCK_STATUS,
                VIAL_UNLOCK_START,
                VIAL_UNLOCK_POLL,
            );
            unimplemented!()
        }

        /// Match `ask.grammar()`. Do not sniff a shared header.
        /// Echoed VIA commands keep their command byte. Vial id, definition, encoder,
        /// and unlock responses overwrite the front of the buffer.
        /// Keymap chunk bytes start at index 4. Set-key and buffer reads echo.
        /// Keyboard id is a little-endian version `u32` then 8 uid bytes.
        /// Definition blocks are raw compressed bytes, short on the last block.
        fn decode(ask: &Ask, resp: &[u8; 32]) -> Result<Reply> {
            let _ = (ask, resp);
            unimplemented!()
        }

        fn decode_request(report: &[u8; 32]) -> Result<Ask> {
            let _ = report;
            unimplemented!()
        }

        enum Reply {
            LayerCount(u8),
            LayoutOptions(u32),
            Chunk(Vec<u8>),
            Ack,
            Id { version: i32, uid: [u8; 8] },
            DefSize(u32),
            DefBlock(Vec<u8>),
            Encoder { cw: u16, ccw: u16 },
            Unlock { unlocked: bool, pairs: Vec<(u8, u8)> },
        }

        fn encode_reply(ask: &Ask, reply: &Reply) -> [u8; 32] {
            let _ = (ask, reply);
            unimplemented!()
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
            /// Locked, or `hold_first` for this code: send unlock start, return the hold,
            /// write nothing. Otherwise `None`. `KC_RESET` takes this path even when
            /// status already says ready.
            pub(super) fn begin_hold(&mut self, code: u16) -> Result<Option<Hold>> {
                let _ = code;
                unimplemented!()
            }

            /// The only set-key and set-encoder sender.
            /// Same cached code: send nothing, return `Same`.
            /// Status locked: send nothing, return `Holding`.
            /// Else send, and only then store the code in the one cell or encoder slot.
            /// `hold_first` is not re-checked here. `poll` must be able to finish it.
            pub(super) fn commit(&mut self, layer: u8, place: Place, code: u16) -> Result<Commit> {
                let _ = (layer, place, code);
                unimplemented!()
            }

            pub(super) fn poll_unlock(&mut self) -> Result<Option<Hold>> {
                // None means the board is ready.
                unimplemented!()
            }

            pub(super) fn search(&self, query: &str) -> Vec<catalog::Found> {
                let _ = query;
                unimplemented!()
            }

            pub(super) fn hold_first(&self, code: u16) -> bool {
                let _ = code;
                unimplemented!()
            }

            pub(super) fn nudge(&self, layer: u8, selection: Option<Place>, dir: Dir) -> Option<Place> {
                let _ = (layer, selection, dir);
                // Nearest selectable center in the facing half-plane.
                // Tie: smaller perpendicular distance. Decals are not targets.
                unimplemented!()
            }

            pub(super) fn layer_count(&self) -> u8 {
                unimplemented!()
            }

            /// Status line, optional hold line, optional search lines, then KLE rows.
            pub(super) fn text(&self, paint: &Paint<'_>) -> String {
                let _ = paint;
                unimplemented!()
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
            let _ = (names, cursor, failed, cols, rows);
            // layout::render_pick. Failed line is `> {name} unavailable`.
            unimplemented!()
        }

        mod layout {
            /// Matrix cell or one encoder direction. Direction 0 is clockwise.
            #[derive(Clone, Copy, PartialEq, Eq)]
            pub(in crate::session) enum Place {
                Matrix { row: u8, col: u8 },
                Encoder { index: u8, direction: u8 },
            }

            pub(super) struct Definition {
                pub(super) rows_n: u8,
                pub(super) cols_n: u8,
                /// KLE rows, metadata already applied onto each key. This is the line break.
                pub(super) rows: Vec<Vec<RawKey>>,
                pub(super) labels: Vec<LayoutGroup>,
                pub(super) custom: Vec<CustomCode>,
            }

            pub(super) struct LayoutGroup {
                pub(super) _name: String,
                pub(super) choices: Vec<String>,
            }

            pub(super) struct RawKey {
                pub(super) x: f64,
                pub(super) y: f64,
                pub(super) w: f64,
                pub(super) h: f64,
                pub(super) labels: [String; 12],
                pub(super) decal: bool,
            }

            pub(super) struct CustomCode {
                pub(super) code: u16,
                pub(super) name: String,
            }

            pub(super) struct Cap {
                place: Option<Place>,
                legend: String,
                selected: bool,
                decal: bool,
                x: f64,
                y: f64,
                w: f64,
                h: f64,
            }

            /// LZMA is the caller's job. This takes the decompressed JSON.
            /// Missing keymap, a non-array label group, label 0 that is not `row,col`
            /// or encoder `index,direction`, label 4 `e` with a bad direction,
            /// or a matrix address outside the matrix: `NoLayout`.
            /// Metadata objects between keys update `x`, `y`, `w`, `h` for the next key.
            pub(super) fn parse(json: &str) -> crate::Result<Definition> {
                let _ = json;
                unimplemented!()
            }

            /// `layouts.labels` in order. Bits are `(choices - 1).bit_length()`, low end first.
            /// No label 8: visible. Label 8 `index,choice` must match the decoded group.
            /// A bad index was already `NoLayout` in `parse`.
            pub(super) fn visible(def: &Definition, options: u32, key: &RawKey) -> bool {
                let _ = (def, options, key);
                unimplemented!()
            }

            /// Group by the KLE row vector. Sort the row by `x`.
            /// Column is `round(x * 4)`, clamped at 0. Overlap starts two spaces later.
            /// Selected token is `[legend]`. Decal is `(legend)`. Clip to `cols` and `rows`.
            /// Hidden keys never enter `caps`, so they leave an `x` gap and no token.
            pub(super) fn render_board(
                name: &str,
                layer_one_based: u8,
                layer_count: u8,
                locked: bool,
                caps: &[Cap],
                hold: Option<&str>,
                query: Option<&str>,
                hits: &[String],
                cursor: usize,
                cols: u16,
                rows: u16,
            ) -> String {
                let _ = (
                    name,
                    layer_one_based,
                    layer_count,
                    locked,
                    caps,
                    hold,
                    query,
                    hits,
                    cursor,
                    cols,
                    rows,
                );
                unimplemented!()
            }

            pub(super) fn render_pick(
                names: &[String],
                cursor: usize,
                failed: Option<usize>,
                cols: u16,
                rows: u16,
            ) -> String {
                let _ = (names, cursor, failed, cols, rows);
                unimplemented!()
            }
        }

        mod catalog {
            pub(super) struct Table {
                entries: Vec<Entry>,
            }

            struct Entry {
                token: String,
                legend: String,
                code: u16,
                /// True only for the `KC_RESET` row. The code number comes from this table.
                hold_first: bool,
            }

            pub(in crate::session) struct Found {
                pub(in crate::session) code: u16,
                pub(in crate::session) legend: String,
            }

            /// Static basic names plus custom codes, in that order, one vec.
            /// `KC_NO` legend `NO`. `KC_ESC` legend `Esc`.
            pub(super) fn merge(custom: &[super::layout::CustomCode]) -> Table {
                let _ = custom;
                unimplemented!()
            }

            /// Prefix before substring. Shorter legend before longer. Then vec order.
            /// Case-insensitive over token and legend.
            pub(super) fn search<'a>(table: &'a Table, query: &str) -> Vec<Found> {
                let _ = (table, query);
                unimplemented!()
            }

            pub(super) fn legend(table: &Table, code: u16) -> String {
                let _ = (table, code);
                // Unknown codes print as uppercase hex, four digits.
                unimplemented!()
            }

            pub(super) fn hold_first(table: &Table, code: u16) -> bool {
                let _ = (table, code);
                unimplemented!()
            }
        }
    }
}

mod app {
    use crate::session::{Attached, Event, Model, Step};
    use crate::Result;

    /// A backend receives the finished string. It does not receive keys or places.
    pub trait Screen {
        fn paint(&mut self, text: &str);
        fn cols(&self) -> u16;
        fn rows(&self) -> u16;
    }

    /// Remembers the last paint. Assertions read `text`.
    pub struct TestScreen {
        cols: u16,
        rows: u16,
        last: String,
    }

    impl TestScreen {
        pub fn new(cols: u16, rows: u16) -> Self {
            Self {
                cols,
                rows,
                last: String::new(),
            }
        }

        pub fn text(&self) -> &str {
            &self.last
        }
    }

    impl Screen for TestScreen {
        fn paint(&mut self, text: &str) {
            self.last = text.to_string();
        }

        fn cols(&self) -> u16 {
            self.cols
        }

        fn rows(&self) -> u16 {
            self.rows
        }
    }

    /// Shell. `new` paints once. `handle` asks `Model`, then paints again.
    pub struct App<S: Screen> {
        screen: S,
        model: Model,
    }

    impl<S: Screen> App<S> {
        pub fn new(attached: Attached, screen: S) -> Self {
            let model = Model::start(attached);
            let mut app = Self { screen, model };
            app.paint();
            app
        }

        pub fn handle(&mut self, event: Event) -> Result<Step> {
            let step = self.model.handle(event)?;
            self.paint();
            Ok(step)
        }

        pub fn screen(&self) -> &S {
            &self.screen
        }

        fn paint(&mut self) {
            let text = self.model.text(self.screen.cols(), self.screen.rows());
            self.screen.paint(&text);
        }
    }
}

/// Binary only. The library crate does not export this module.
mod binary {
    use crate::session::Event;
    use crate::app::Screen;
    use crate::Result;

    pub struct Terminal;

    impl Terminal {
        pub fn open() -> Result<Self> {
            unimplemented!()
        }

        /// One decoded `Event`. A closed tty is `Err(Disconnected)`.
        pub fn event() -> Result<Event> {
            unimplemented!()
        }
    }

    impl Screen for Terminal {
        fn paint(&mut self, text: &str) {
            let _ = text;
            unimplemented!()
        }

        fn cols(&self) -> u16 {
            unimplemented!()
        }

        fn rows(&self) -> u16 {
            unimplemented!()
        }
    }

    // fn main() -> Result<()> {
    //     let mut app = App::new(attach()?, Terminal::open()?);
    //     loop {
    //         if app.handle(Terminal::event()?)? == Step::Quit {
    //             return Ok(());
    //         }
    //     }
    // }
}

pub use app::{App, Screen, TestScreen};
pub use session::vial::MemoryBoard;
pub use session::{attach, Attached, Event, Step};
