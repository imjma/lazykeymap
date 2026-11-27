use crate::{Error, Result};
use std::mem;
use std::time::{Duration, Instant};

mod keys;

/// What the binary and the tests send. `Model::handle` is the only match.
#[derive(Debug)]
pub enum Event {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    NextLayer,
    PrevLayer,
    /// The Tab key. Moves to the next editor tab.
    Tab,
    /// Shift-Tab. Moves to the previous editor tab.
    BackTab,
    Char(char),
    Backspace,
    Confirm,
    Cancel,
    /// One unlock poll. The binary's timer produces this. A person does not.
    Tick,
    Quit,
    /// Ctrl-K: find anything. Space does the same on the board and the editor tabs.
    Palette,
    /// A mouse button or wheel at a screen cell, 0-based.
    Mouse {
        button: Mouse,
        col: u16,
        row: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    /// The left button: select what is under it; on what is already selected, open or stage it.
    Click,
    /// The right button: back, as Esc.
    Back,
    WheelUp,
    WheelDown,
    /// The pointer moved: what is under it lights up when a click would do something there.
    Hover,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Continue,
    /// Still reading a board. Send `Tick` again right away instead of waiting for the next one.
    Working,
    Quit,
}

/// How long one `Tick` keeps reading a board before the screen repaints.
const SLICE: Duration = Duration::from_millis(60);

/// An opened chooser or a loaded board. No public fields. `App::new` consumes it.
pub struct Attached {
    kind: Kind,
}

enum Kind {
    Ready(Session<Mapping>),
    Pick(Vec<vial::Candidate>),
}

/// The product loop. `app` can call `start`, `handle`, and `frame`.
pub(crate) struct Model {
    kind: ModelKind,
    /// One line for the last thing that happened: a save, a refusal, an unknown command.
    /// Cleared by the next key.
    notice: Option<String>,
    /// Where the last frame drew each clickable thing.
    zones: Vec<vial::Zone>,
    /// What the pointer is over and a click would act on, drawn lit.
    hover: Option<vial::Hit>,
}

enum ModelKind {
    Pick {
        candidates: Vec<vial::Candidate>,
        cursor: usize,
        /// The board that would not open, and why.
        failed: Option<(usize, String)>,
    },
    /// Reading the highlighted board. Esc goes back to the list.
    Open {
        candidates: Vec<vial::Candidate>,
        cursor: usize,
        load: vial::Load,
        /// The layer to show once open: 0 from the list, the edited one after a layer change.
        land: u8,
    },
    /// The keyboard stopped answering. The board stays on screen, drafts and all, under a
    /// pick-one list: reconnect and keep the changes, reconnect and revert them, start again,
    /// or quit.
    Lost {
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        /// The disconnect, or why the last try did not work.
        why: String,
        cursor: usize,
        /// Reading the keyboard again; `true` keeps the unsaved changes. Esc stops it.
        reconnect: Option<Box<(vial::Candidate, vial::Load, bool)>>,
    },
    Map(Session<Mapping>),
    /// A tap dance, combo, or macro tab.
    Sheet {
        session: Session<Mapping>,
        spot: vial::Spot,
    },
    /// Browse and cursor live here, not on the board.
    Search {
        session: Session<Mapping>,
        browse: Browse,
        cursor: usize,
        /// Set after a Vial `(kc)` pick. The next pick must be a basic key.
        wrap: Option<Wrap>,
        target: Target,
    },
    /// One text or number field of a sheet cell.
    Edit {
        session: Session<Mapping>,
        spot: vial::Spot,
        field: Field,
        text: String,
    },
    /// Saving, and this write needs the unlock first.
    Hold {
        session: Session<Holding>,
        /// The sheet to land on after the save. `None` is the keymap.
        back: Option<vial::Spot>,
        /// `:wq`: quit once every change is written.
        quit: bool,
    },
    /// Unlocked, and the unlock keys may still be down, typing into this terminal.
    /// Keys are dropped until a whole tick passes with none, then the save goes on.
    Release {
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        quit: bool,
        quiet: bool,
    },
    /// A yes-or-no before something happens.
    Ask {
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        question: Question,
    },
    /// Space or Ctrl-K: commands, layers, tabs, and every key on the board, filtered by `text`.
    /// `:` opens it with `text` = `:`, and a leading `:` lists only the commands, by name.
    Palette {
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        text: String,
        /// The row ↑↓ chose. Typing clears it; `palette_pick` says which row Enter runs.
        cursor: Option<usize>,
    },
    Help {
        under: Box<ModelKind>,
        scroll: usize,
    },
}

enum Question {
    /// Unsaved changes, and a quit.
    Quit {
        count: usize,
        lines: Vec<vial::PopupLine>,
    },
    /// `:e!` with unsaved changes.
    Discard {
        count: usize,
        lines: Vec<vial::PopupLine>,
    },
    /// `:layer-remove`, which changes the keyboard or file at once.
    RemoveLayer { layer: u8, name: String },
}

impl Question {
    fn popup(&self) -> vial::Popup {
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        let (title, intro, lines, actions, warn): (
            String,
            Option<String>,
            &[vial::PopupLine],
            Vec<_>,
            bool,
        ) = match self {
            Question::Quit { count, lines } => (
                format!("{count} unsaved change{}", plural(*count)),
                Some("The keyboard does not have these yet.".to_string()),
                lines,
                vec![
                    ("enter", "write & quit"),
                    ("d", "discard & quit"),
                    ("esc", "stay"),
                ],
                true,
            ),
            Question::Discard { count, lines } => (
                format!("discard {count} change{}?", plural(*count)),
                Some("These go back to what the keyboard holds.".to_string()),
                lines,
                vec![("enter", "discard"), ("esc", "keep")],
                true,
            ),
            Question::RemoveLayer { layer, name } => (
                format!("remove layer {layer} {name}?"),
                Some(
                    "It goes from the keyboard or the file at once; undo does not bring it back."
                        .to_string(),
                ),
                &[],
                vec![("enter", "remove"), ("esc", "keep")],
                true,
            ),
        };
        let mut body = Vec::new();
        if let Some(intro) = intro {
            body.push(vial::PopupLine::Text(intro));
            body.push(vial::PopupLine::Blank);
        }
        body.extend(lines.iter().cloned());
        vial::Popup {
            title,
            lines: body,
            scroll: 0,
            actions,
            warn,
            pinned: false,
        }
    }
}

/// Every unsaved change, as popup lines: what the keyboard has, and what the draft has.
fn pending_lines(board: &vial::Board) -> (usize, Vec<vial::PopupLine>) {
    let pending = board.pending();
    let lines = pending
        .iter()
        .flat_map(|write| board.describe(write, true))
        .collect();
    (pending.len(), lines)
}

/// Where a search pick goes.
enum Target {
    Key,
    Cell {
        spot: vial::Spot,
        /// A new macro step of this kind goes in at the cursor. `None` replaces the cell.
        insert: Option<Press>,
    },
}

impl Target {
    fn spot(&self) -> Option<vial::Spot> {
        match self {
            Target::Key => None,
            Target::Cell { spot, .. } => Some(*spot),
        }
    }
}

#[derive(Clone, Copy)]
enum Press {
    Tap,
    Down,
    Up,
}

impl Press {
    fn action(self, code: u16) -> vial::Action {
        match self {
            Press::Tap => vial::Action::Tap(code),
            Press::Down => vial::Action::Down(code),
            Press::Up => vial::Action::Up(code),
        }
    }
}

#[derive(Clone, Copy)]
enum Field {
    Term,
    Delay {
        insert: bool,
    },
    Text {
        insert: bool,
    },
    /// A keymap-file node's name or label.
    Name,
    /// A keymap-file property, by its column.
    Prop {
        col: usize,
    },
}

impl Field {
    fn title(self) -> &'static str {
        match self {
            Field::Term => "tapping term (ms)",
            Field::Delay { .. } => "delay (ms)",
            Field::Text { .. } => "text",
            Field::Name => "name",
            Field::Prop { .. } => "value",
        }
    }

    /// Digits for numbers, printable ASCII for text. The firmware types text byte by byte. A
    /// devicetree name takes letters, digits, `_`, and `-`; a value anything but what would end
    /// it early.
    fn accepts(self, text: &str, ch: char) -> bool {
        let max = match self {
            Field::Text { .. } => return (' '..='~').contains(&ch),
            Field::Name => return ch.is_ascii_alphanumeric() || ch == '_' || ch == '-',
            Field::Prop { .. } => return (' '..='~').contains(&ch) && !";<>\"{}&".contains(ch),
            Field::Term => 10_000,
            Field::Delay { .. } => u32::from(vial::MAX_DELAY),
        };
        ch.is_ascii_digit()
            && format!("{text}{ch}")
                .parse::<u32>()
                .is_ok_and(|value| value <= max)
    }
}

/// Mapping has no pending code. The type is the invariant.
struct Mapping;

/// The one pending write that needs the unlock first. `poll` is the only path that commits it.
struct Holding {
    write: vial::Write,
    prompt: String,
}

struct Session<Phase> {
    board: vial::Board,
    /// 0-based, as QMK and the tabs count.
    layer: u8,
    selection: Option<vial::Place>,
    phase: Phase,
}

/// A code only confirm builds from a listed hit. The `u16` stays in this module.
struct Hit {
    code: u16,
}

/// A Vial `(kc)` code waiting for the basic key that fills its low byte.
struct Wrap {
    code: u16,
    legend: String,
}

/// The search list, titled with the pending `(kc)` code when there is one.
fn search_list(session: &Session<Mapping>, browse: &Browse, wrap: &Option<Wrap>) -> vial::Listed {
    let mut listed = session.list(browse);
    if let Some(wrap) = wrap {
        listed.title = format!("{} > {}", wrap.legend, listed.title);
    }
    listed
}

struct ShelfIndex(usize);

/// A non-empty search token.
struct Query(String);

impl Query {
    fn as_str(&self) -> &str {
        &self.0
    }
}

enum Browse {
    Shelf(ShelfIndex),
    Find {
        query: Query,
        back: ShelfIndex,
    },
    /// A ZMK binding in columns, one per part, as Keymap Editor's chips: `query` is the parts
    /// left of the active column, then what is typed to narrow it.
    Part {
        parts: Parts,
        query: String,
    },
}

/// A binding's words, `&mt LSHIFT A`, one per column, and the active column. The active word
/// follows the highlight; words right of it are kept while they still fit. `at` is at most
/// `words.len()`, a column with nothing chosen yet.
struct Parts {
    words: Vec<String>,
    at: usize,
    /// The binding the list opened on. Its words stay valid where the lists do not hold them,
    /// as a `#define`d layer name.
    original: Vec<String>,
}

impl Parts {
    /// What the composer is given for column `i`: the behavior list, or the words left of it.
    fn prefix_of(&self, i: usize) -> String {
        match i {
            0 => "&".to_string(),
            _ => format!("{} ", self.words[..i].join(" ")),
        }
    }

    fn prefix(&self) -> String {
        self.prefix_of(self.at)
    }

    /// Sets the active column's word, as the highlight moves.
    fn choose(&mut self, word: &str) {
        match self.words.get_mut(self.at) {
            Some(slot) => *slot = word.to_string(),
            None => self.words.push(word.to_string()),
        }
    }
}

/// Column `i`'s values, narrowed by `filter`.
fn column_hits(session: &Session<Mapping>, parts: &Parts, i: usize, filter: &str) -> vial::Listed {
    let filter = if i == 0 {
        filter.trim_start_matches('&')
    } else {
        filter
    };
    session.board.list(vial::Range::Find(&format!(
        "{}{filter}",
        parts.prefix_of(i)
    )))
}

/// Where `word` is among `hits`: by its name, or by an alias the hit lists (`LSHFT` is the
/// `LSHIFT` row).
fn hit_of(hits: &[vial::Found], word: &str) -> Option<usize> {
    hits.iter()
        .position(|hit| hit.legend == word || hit.token.split(' ').any(|alias| alias == word))
}

/// Whether column `i` holds a value its behavior takes there.
fn word_valid(session: &Session<Mapping>, parts: &Parts, i: usize) -> bool {
    let Some(word) = parts.words.get(i) else {
        return false;
    };
    let unchanged =
        parts.words.get(..i) == parts.original.get(..i) && parts.original.get(i) == Some(word);
    unchanged || hit_of(&column_hits(session, parts, i, word).hits, word).is_some()
}

/// How many columns the binding has: the behavior, then each parameter it takes so far. A
/// parameter's own list can depend on the one before (`&bt BT_SEL` then a profile), so the
/// count stops one past the first column without a valid value.
fn column_count(session: &Session<Mapping>, parts: &Parts) -> usize {
    let mut count = 1;
    while count < 8 && word_valid(session, parts, count - 1) {
        let done = parts.words[..count].join(" ");
        if session.board.typed_complete(&format!("{done} ")) != Some(false) {
            return count;
        }
        count += 1;
    }
    count
}

impl Browse {
    fn type_char(self, ch: char) -> Self {
        let (mut text, back) = match self {
            Browse::Shelf(index) => (String::new(), index),
            Browse::Find { query, back } => (query.0, back),
            // `&` starts the binding over, from its behavior and with no parts kept.
            Browse::Part { parts, .. } if ch == '&' => {
                let parts = Parts {
                    words: Vec::new(),
                    at: 0,
                    original: parts.original,
                };
                let query = parts.prefix();
                return Browse::Part { parts, query };
            }
            Browse::Part { parts, mut query } => {
                query.push(ch);
                return Browse::Part { parts, query };
            }
        };
        text.push(ch);
        Browse::Find {
            query: Query(text),
            back,
        }
    }

    fn backspace(self) -> Self {
        match self {
            Browse::Shelf(index) => Browse::Shelf(index),
            Browse::Part { parts, mut query } => {
                if query.len() > parts.prefix().len() {
                    query.pop();
                }
                Browse::Part { parts, query }
            }
            Browse::Find { mut query, back } => {
                query.0.pop();
                if query.0.is_empty() {
                    Browse::Shelf(back)
                } else {
                    Browse::Find { query, back }
                }
            }
        }
    }

    /// The next group, or for a binding in columns the next of its `len` columns.
    fn shift_shelf(self, delta: i8, len: usize) -> Self {
        let ShelfIndex(index) = match self {
            Browse::Shelf(index) | Browse::Find { back: index, .. } => index,
            Browse::Part { mut parts, .. } => {
                let len = len.max(1) as isize;
                parts.at = (parts.at as isize + isize::from(delta)).rem_euclid(len) as usize;
                let query = parts.prefix();
                return Browse::Part { parts, query };
            }
        };
        let next = (index as isize + isize::from(delta)).rem_euclid(len as isize) as usize;
        Browse::Shelf(ShelfIndex(next))
    }

    fn range(&self) -> vial::Range<'_> {
        match self {
            Browse::Shelf(ShelfIndex(index)) => vial::Range::Shelf(*index),
            Browse::Find { query, .. } => vial::Range::Find(query.as_str()),
            Browse::Part { query, .. } => vial::Range::Find(query),
        }
    }
}

/// Zero candidates: `NoBoard`. One candidate: load it. Many: the choose screen.
fn fan_in(mut candidates: Vec<vial::Candidate>) -> Result<Attached> {
    match candidates.len() {
        0 => Err(Error::NoBoard),
        1 => {
            let board = vial::load(candidates.pop().expect("one candidate"))?;
            Ok(Attached {
                kind: Kind::Ready(Session::mapped(board)),
            })
        }
        _ => Ok(Attached {
            kind: Kind::Pick(candidates),
        }),
    }
}

/// Vial boards over HID and ZMK Studio boards over USB serial and Bluetooth, then the choose
/// screen. One board still waits for Enter. Zero boards is `NoBoard`.
pub fn attach() -> Result<Attached> {
    list_first(vial::enumerate()?)
}

/// `attach`, and the serial port at `path` is asked for ZMK Studio whatever its USB id.
/// It is asked again on every reconnect.
pub fn attach_port(path: &str) -> Result<Attached> {
    vial::add_port(path);
    attach()
}

/// A ZMK `.keymap` file, read and ready to edit; or a zmk-config folder: its one keymap, or
/// the choose screen when it has several. `:w` writes the file, and the connected ZMK Studio
/// keyboard too when it holds the same keymap.
pub fn attach_file(path: &std::path::Path) -> Result<Attached> {
    let mut candidates = vial::file_candidates(path)?;
    vial::link_files(&mut candidates);
    if candidates.len() == 1 {
        fan_in(candidates)
    } else {
        list_first(candidates)
    }
}

fn list_first(candidates: Vec<vial::Candidate>) -> Result<Attached> {
    if candidates.is_empty() {
        Err(Error::NoBoard)
    } else {
        Ok(Attached {
            kind: Kind::Pick(candidates),
        })
    }
}

impl Model {
    pub(crate) fn start(attached: Attached) -> Self {
        let kind = match attached.kind {
            Kind::Ready(session) => ModelKind::Map(session),
            // Even one board waits for Enter on its card; a locked Studio board then opens on
            // its unlock key by itself.
            Kind::Pick(candidates) => ModelKind::Pick {
                candidates,
                cursor: 0,
                failed: None,
            },
        };
        Self {
            kind,
            notice: None,
            zones: Vec::new(),
            hover: None,
        }
    }

    /// `cols` is the screen width: the key grid moves Up and Down a whole row of it.
    pub(crate) fn handle(&mut self, event: Event, cols: u16) -> Result<Step> {
        // A move only repaints the highlight: it keeps the notice, and an unlock still in
        // progress does not take it for a key.
        if let Event::Mouse {
            button: Mouse::Hover,
            col,
            row,
        } = event
        {
            self.hover = vial::hits_at(&self.zones, col, row)
                .into_iter()
                .find(|hit| **hit != vial::Hit::Lower)
                .filter(|hit| self.clickable(hit))
                .cloned();
            return Ok(Step::Continue);
        }
        if !matches!(event, Event::Tick) {
            self.notice = None;
        }
        // The keyboard takes over: the pointer's highlight goes until it moves again.
        if !matches!(event, Event::Tick | Event::Mouse { .. }) {
            self.hover = None;
        }
        if matches!(event, Event::Quit) {
            return self.quit();
        }
        // Keys typed by the still-held unlock keys. Drop them and wait for a quiet tick.
        if let ModelKind::Release { quiet, .. } = &mut self.kind {
            if !matches!(event, Event::Tick) {
                *quiet = false;
                return Ok(Step::Continue);
            }
        }
        if let Event::Mouse { button, col, row } = event {
            return self.mouse(button, col, row, cols);
        }
        self.act(keys::act(view_of(&self.kind), &event), cols)
    }

    fn act(&mut self, act: keys::Act, cols: u16) -> Result<Step> {
        match act {
            keys::Act::Nudge(dir) => self.arrow(dir, cols),
            keys::Act::Palette => self.open_palette(""),
            keys::Act::Shelf(delta) => self.shelf(delta),
            keys::Act::Page(delta) => self.page(delta, cols),
            keys::Act::Layer(delta) => self.layer(delta),
            keys::Act::Tab(delta) => self.tab(delta),
            keys::Act::End(end) => self.end(end),
            keys::Act::Type(ch) => self.char(ch),
            keys::Act::Backspace => self.backspace(),
            keys::Act::Clear => self.clear(),
            keys::Act::Insert(step) => self.insert(step),
            keys::Act::Reorder(delta) => self.reorder(delta),
            keys::Act::Undo => self.undo(),
            keys::Act::Command => self.open_palette(":"),
            keys::Act::Confirm => self.confirm(),
            keys::Act::Cancel => self.cancel(),
            keys::Act::Tick => self.tick(),
            keys::Act::Help => self.open_help(),
            keys::Act::CloseHelp => self.close_help(),
            keys::Act::Quit => self.quit(),
            keys::Act::Scroll(delta) => self.scroll_help(delta),
            keys::Act::Ignore => Ok(Step::Continue),
        }
    }

    /// The screen, and, kept for the next click, where each clickable thing is on it.
    pub(crate) fn frame(&mut self, cols: u16, rows: u16) -> String {
        let (text, zones) = vial::zones(
            &render(&self.kind, self.notice.as_deref(), None, cols, rows),
            self.hover.as_ref(),
        );
        self.zones = zones;
        text
    }

    fn mouse(&mut self, button: Mouse, col: u16, row: u16, cols: u16) -> Result<Step> {
        let hits: Vec<vial::Hit> = vial::hits_at(&self.zones, col, row)
            .into_iter()
            .cloned()
            .collect();
        match button {
            Mouse::Back => self.act(keys::act(view_of(&self.kind), &Event::Cancel), cols),
            Mouse::WheelUp | Mouse::WheelDown => {
                let down = button == Mouse::WheelDown;
                let delta = if down { 1 } else { -1 };
                match &self.kind {
                    // Over the board the wheel turns the layers; the key list scrolls under it.
                    ModelKind::Map(_) => self.layer(delta),
                    ModelKind::Search { .. } if !hits.contains(&vial::Hit::Lower) => {
                        Ok(Step::Continue)
                    }
                    ModelKind::Help { .. } => self.scroll_help(delta),
                    _ => {
                        let event = if down { Event::Down } else { Event::Up };
                        self.act(keys::act(view_of(&self.kind), &event), cols)
                    }
                }
            }
            Mouse::Click => match hits.into_iter().next() {
                Some(hit) => self.click(hit, cols),
                None => Ok(Step::Continue),
            },
            Mouse::Hover => Ok(Step::Continue),
        }
    }

    /// Selects what was clicked. A click on what is already selected does what Enter does
    /// there: opens the key list, edits the cell, opens the board, stages the pick.
    fn click(&mut self, hit: vial::Hit, cols: u16) -> Result<Step> {
        use vial::Hit;
        let view = view_of(&self.kind);
        match hit {
            Hit::Help if matches!(self.kind, ModelKind::Help { .. }) => return self.close_help(),
            Hit::Help => return self.open_help(),
            Hit::Hint(index) => {
                return match keys::footer_act(view, index) {
                    Some(act) => self.act(act, cols),
                    None => Ok(Step::Continue),
                }
            }
            Hit::Chip(key) => {
                return match chip_event(&key) {
                    Some(event) => self.act(keys::act(view, &event), cols),
                    None => Ok(Step::Continue),
                }
            }
            Hit::Tab(index) if matches!(self.kind, ModelKind::Map(_) | ModelKind::Sheet { .. }) => {
                return self.tab_to(|_, _| index)
            }
            // Another key on the board while the key list is open: the list moves to it.
            Hit::Key(place)
                if matches!(&self.kind, ModelKind::Search { session, target: Target::Key, .. }
                    if session.selection != Some(place)) =>
            {
                if let ModelKind::Search { mut session, .. } =
                    mem::replace(&mut self.kind, vacant())
                {
                    session.selection = Some(place);
                    self.kind = searching(session, Target::Key);
                }
                return Ok(Step::Continue);
            }
            _ => {}
        }
        let again = match (&mut self.kind, hit) {
            (ModelKind::Pick { cursor, .. }, Hit::Card(index))
            | (
                ModelKind::Lost {
                    cursor,
                    reconnect: None,
                    ..
                },
                Hit::Choice(index),
            ) => mem::replace(cursor, index) == index,
            (ModelKind::Map(session), Hit::Key(place)) => {
                session.selection.replace(place) == Some(place)
            }
            (ModelKind::Map(session), Hit::Layer(layer)) => {
                session.layer = layer;
                false
            }
            (ModelKind::Sheet { session, spot }, Hit::Cell(row, col)) => {
                let again = spot.row == row && spot.col == col;
                spot.row = row;
                spot.col = col;
                clamp_col(&session.board, spot);
                again
            }
            (
                ModelKind::Search {
                    session,
                    browse,
                    cursor,
                    ..
                },
                hit,
            ) => match (&mut *browse, hit) {
                (Browse::Shelf(ShelfIndex(at)), Hit::Shelf(shelf)) => {
                    let delta = shelf as isize - *at as isize;
                    *browse = Browse::Shelf(ShelfIndex(*at))
                        .shift_shelf(delta as i8, session.board.shelf_count());
                    *cursor = 0;
                    false
                }
                (Browse::Part { parts, .. }, Hit::Part(column, index)) => {
                    let again = parts.at == column && *cursor == index;
                    let delta = column as isize - parts.at as isize;
                    if delta != 0 {
                        shift_column(session, browse, cursor, delta as i8);
                    }
                    *cursor = index;
                    follow(session, browse, index);
                    again
                }
                (Browse::Part { .. }, _) => false,
                (_, Hit::Pick(index)) => mem::replace(cursor, index) == index,
                _ => false,
            },
            (
                ModelKind::Palette {
                    session,
                    text,
                    cursor,
                    ..
                },
                Hit::Choice(index),
            ) => {
                // The popup lists a window of the rows; its first is where `palette_popup` starts.
                let len = palette(&session.board, text).len();
                let pick = palette_pick(text, len, *cursor);
                let row = pick.unwrap_or(0).saturating_sub(PALETTE_ROWS - 1) + index;
                *cursor = Some(row);
                pick == Some(row)
            }
            _ => false,
        };
        if again {
            self.confirm()
        } else {
            Ok(Step::Continue)
        }
    }

    /// Whether `click` does something with `hit` here, so hovering it lights it.
    fn clickable(&self, hit: &vial::Hit) -> bool {
        use vial::Hit;
        match hit {
            Hit::Help => true,
            Hit::Lower => false,
            Hit::Hint(index) => keys::footer_act(view_of(&self.kind), *index).is_some(),
            Hit::Chip(key) => chip_event(key).is_some(),
            Hit::Tab(_) => matches!(self.kind, ModelKind::Map(_) | ModelKind::Sheet { .. }),
            Hit::Key(_) => matches!(
                self.kind,
                ModelKind::Map(_)
                    | ModelKind::Search {
                        target: Target::Key,
                        ..
                    }
            ),
            Hit::Layer(_) => matches!(self.kind, ModelKind::Map(_)),
            Hit::Cell(..) => matches!(self.kind, ModelKind::Sheet { .. }),
            Hit::Card(_) => matches!(self.kind, ModelKind::Pick { .. }),
            Hit::Pick(_) | Hit::Shelf(_) | Hit::Part(..) => {
                matches!(self.kind, ModelKind::Search { .. })
            }
            Hit::Choice(_) => matches!(
                self.kind,
                ModelKind::Palette { .. }
                    | ModelKind::Lost {
                        reconnect: None,
                        ..
                    }
            ),
        }
    }

    /// `q`, `:q`, or Ctrl-C. Unsaved changes ask first; asking again quits.
    fn quit(&mut self) -> Result<Step> {
        // The choices already say what quitting drops.
        if matches!(
            self.kind,
            ModelKind::Ask {
                question: Question::Quit { .. },
                ..
            } | ModelKind::Lost { .. }
        ) {
            return Ok(Step::Quit);
        }
        let kind = mem::replace(&mut self.kind, vacant());
        let (session, back) = match into_session(kind) {
            Some(found) => found,
            None => return Ok(Step::Quit),
        };
        let (count, lines) = pending_lines(&session.board);
        if count == 0 {
            return Ok(Step::Quit);
        }
        self.kind = ModelKind::Ask {
            session,
            back,
            question: Question::Quit { count, lines },
        };
        Ok(Step::Continue)
    }

    fn undo(&mut self) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Map(session) => {
                if !session.board.undo() {
                    self.notice = Some("nothing to undo".to_string());
                }
            }
            ModelKind::Sheet { session, spot } => {
                if session.board.undo() {
                    clamp_col(&session.board, spot);
                } else {
                    self.notice = Some("nothing to undo".to_string());
                }
            }
            _ => {}
        }
        Ok(Step::Continue)
    }

    fn open_palette(&mut self, text: &str) -> Result<Step> {
        let (session, back) = match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Map(session) => (session, None),
            ModelKind::Sheet { session, spot } => (session, Some(spot)),
            other => {
                self.kind = other;
                return Ok(Step::Continue);
            }
        };
        self.kind = ModelKind::Palette {
            session,
            back,
            text: text.to_string(),
            cursor: None,
        };
        Ok(Step::Continue)
    }

    /// Enter on a Ctrl-K row.
    fn go(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        goal: Goal,
        arg: &str,
    ) -> Result<Step> {
        let mut session = session;
        self.kind = match goal {
            Goal::Run(command) => return self.run_command(session, back, command, arg),
            Goal::Layer(layer) => {
                session.layer = layer;
                ModelKind::Map(session)
            }
            Goal::Jump(vial::Jump::Key { layer, place }) => {
                session.layer = layer;
                session.selection = Some(place);
                ModelKind::Map(session)
            }
            Goal::Jump(vial::Jump::Spot(spot)) => ModelKind::Sheet { session, spot },
        };
        Ok(Step::Continue)
    }

    /// `arg` is what was typed after the command's name, as the name in `:layer-rename Sym`.
    fn run_command(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        command: Cmd,
        arg: &str,
    ) -> Result<Step> {
        let mut session = session;
        match command {
            Cmd::Write => return self.save(session, back, false),
            Cmd::WriteQuit => return self.save(session, back, true),
            Cmd::ForceQuit => return Ok(Step::Quit),
            Cmd::Quit => {
                self.kind = back_to(session, back);
                return self.quit();
            }
            Cmd::Discard => {
                let (count, lines) = pending_lines(&session.board);
                if count == 0 {
                    self.notice = Some("nothing to discard".to_string());
                } else {
                    self.kind = ModelKind::Ask {
                        session,
                        back,
                        question: Question::Discard { count, lines },
                    };
                    return Ok(Step::Continue);
                }
            }
            Cmd::Undo => {
                if !session.board.undo() {
                    self.notice = Some("nothing to undo".to_string());
                }
            }
            Cmd::Help => {
                self.kind = back_to(session, back);
                return self.open_help();
            }
            Cmd::RowAdd | Cmd::RowRemove => {
                let Some(
                    spot @ vial::Spot {
                        tab: vial::Tab::Section(kind),
                        ..
                    },
                ) = back
                else {
                    self.notice = Some("open a keymap file's tab first".to_string());
                    self.kind = back_to(session, back);
                    return Ok(Step::Continue);
                };
                if kind.rows_fixed() {
                    self.notice = Some("encoder rows are the layers: use :layer-add".to_string());
                    self.kind = back_to(session, back);
                    return Ok(Step::Continue);
                }
                let mut rows = session.board.section(kind).to_vec();
                let row = if command == Cmd::RowAdd {
                    let name = if arg.is_empty() {
                        let base = kind.name().to_lowercase();
                        (rows.len()..)
                            .map(|n| format!("{base}_{n}"))
                            .find(|name| rows.iter().all(|row| row.name != *name))
                            .unwrap_or(base)
                    } else {
                        arg.to_string()
                    };
                    if !name
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    {
                        self.notice = Some("a name takes letters, digits, and _".to_string());
                        self.kind = back_to(session, back);
                        return Ok(Step::Continue);
                    }
                    rows.push(kind.blank(&name));
                    rows.len() - 1
                } else {
                    if spot.row < rows.len() {
                        rows.remove(spot.row);
                    }
                    spot.row.saturating_sub(1)
                };
                let at = vial::Spot {
                    row,
                    col: 0,
                    ..spot
                };
                self.land(session, vial::Write::Section(kind, rows), Some(at));
                return Ok(Step::Continue);
            }
            Cmd::LayerAdd
            | Cmd::LayerRemove
            | Cmd::LayerRename
            | Cmd::LayerLeft
            | Cmd::LayerRight => {
                let layer = session.layer;
                let last = session.board.layer_count().saturating_sub(1);
                // Removing goes to the keyboard or the file at once, with no undo: ask first.
                if command == Cmd::LayerRemove {
                    let name = session.board.layer_name(layer).unwrap_or("").to_string();
                    self.kind = ModelKind::Ask {
                        session,
                        back,
                        question: Question::RemoveLayer { layer, name },
                    };
                    return Ok(Step::Continue);
                }
                let change = match command {
                    Cmd::LayerAdd => Ok((vial::LayerOp::Add, last + 1)),
                    Cmd::LayerRemove => Ok((vial::LayerOp::Remove(layer), layer.saturating_sub(1))),
                    Cmd::LayerRename if arg.is_empty() => {
                        Err("type the name after it: :layer-rename Sym")
                    }
                    Cmd::LayerRename => Ok((vial::LayerOp::Rename(layer, arg.to_string()), layer)),
                    Cmd::LayerLeft if layer == 0 => Err("this is the first layer"),
                    Cmd::LayerLeft => Ok((
                        vial::LayerOp::Move {
                            from: layer,
                            to: layer - 1,
                        },
                        layer - 1,
                    )),
                    Cmd::LayerRight if layer >= last => Err("this is the last layer"),
                    _ => Ok((
                        vial::LayerOp::Move {
                            from: layer,
                            to: layer + 1,
                        },
                        layer + 1,
                    )),
                };
                match change {
                    Ok((op, land)) => return self.change_layers(session, back, op, land),
                    Err(why) => self.notice = Some(why.to_string()),
                }
            }
            Cmd::Layout => return self.switch_layout(session, back, arg),
        }
        self.kind = back_to(session, back);
        Ok(Step::Continue)
    }

    /// Sends a layer change, then reads the keyboard again and lands on `land`.
    fn change_layers(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        op: vial::LayerOp,
        land: u8,
    ) -> Result<Step> {
        let mut session = session;
        if let Err(err) = session.board.edit_layers(&op) {
            if matches!(err, Error::Disconnected) {
                return Ok(self.lose(session, back, &err));
            }
            self.notice = Some(err.to_string());
            self.kind = back_to(session, back);
            return Ok(Step::Continue);
        }
        self.reread(session, land)
    }

    /// `:layout` lists physical layouts; `:layout 1` or `:layout five` switches and re-reads.
    fn switch_layout(
        &mut self,
        mut session: Session<Mapping>,
        back: Option<vial::Spot>,
        arg: &str,
    ) -> Result<Step> {
        let names = session.board.layouts();
        if names.is_empty() {
            self.notice = Some("this keyboard has one layout".to_string());
            self.kind = back_to(session, back);
            return Ok(Step::Continue);
        }
        let at = session.board.layout_at();
        if arg.is_empty() {
            self.notice = Some(layout_list(&names, at));
            self.kind = back_to(session, back);
            return Ok(Step::Continue);
        }
        let index = match layout_pick(&names, arg) {
            Ok(index) => index,
            Err(why) => {
                self.notice = Some(why);
                self.kind = back_to(session, back);
                return Ok(Step::Continue);
            }
        };
        if index == at {
            self.notice = Some(format!("already {at} {}", names[at]));
            self.kind = back_to(session, back);
            return Ok(Step::Continue);
        }
        let land = session.layer;
        if let Err(err) = session.board.set_layout(index) {
            if matches!(err, Error::Disconnected) {
                return Ok(self.lose(session, back, &err));
            }
            self.notice = Some(err.to_string());
            self.kind = back_to(session, back);
            return Ok(Step::Continue);
        }
        self.notice = Some(format!("layout {index} {}", names[index]));
        self.reread(session, land)
    }

    /// Reads the open keyboard again and lands on `land`.
    fn reread(&mut self, session: Session<Mapping>, land: u8) -> Result<Step> {
        let candidate = session.board.reopen();
        match vial::Load::begin(&candidate) {
            Ok(load) => {
                self.kind = ModelKind::Open {
                    candidates: vec![candidate],
                    cursor: 0,
                    load,
                    land,
                };
                self.pump()
            }
            Err(err) => {
                self.kind = ModelKind::Pick {
                    candidates: vec![candidate],
                    cursor: 0,
                    failed: Some((0, why_not(&err))),
                };
                Ok(Step::Continue)
            }
        }
    }

    /// Writes every staged change. A write that needs the unlock parks here as a hold, and the
    /// release after it calls `save` again for the rest.
    fn save(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        quit: bool,
    ) -> Result<Step> {
        let mut session = session;
        let writes = session.board.pending();
        let count = writes.len();
        for write in writes {
            match session.write(write) {
                Assign::Mapped(next) => session = next,
                Assign::Holding(held) => {
                    self.kind = ModelKind::Hold {
                        session: held,
                        back,
                        quit,
                    };
                    return Ok(Step::Continue);
                }
                Assign::Disconnected(err, session) => return Ok(self.lose(session, back, &err)),
            }
        }
        if quit {
            return Ok(Step::Quit);
        }
        let to = session.board.writes_to();
        self.notice = Some(match count {
            0 => "nothing to write".to_string(),
            1 => format!("wrote 1 change to {to}"),
            _ => format!("wrote {count} changes to {to}"),
        });
        self.kind = back_to(session, back);
        Ok(Step::Continue)
    }

    /// Puts one change in the draft and lands on its sheet. Macros that would not fit the
    /// board's buffer stay out, and the notice says why.
    fn land(&mut self, session: Session<Mapping>, write: vial::Write, spot: Option<vial::Spot>) {
        let mut session = session;
        if let vial::Write::Macros(macros) = &write {
            let (used, memory) = session.board.macro_room(macros);
            if used > memory {
                self.notice = Some(format!(
                    "not staged: the macros need {used} bytes and the board holds {memory}"
                ));
                self.kind = back_to(session, spot);
                return;
            }
        }
        session.board.stage(write);
        self.kind = back_to(session, spot);
    }

    fn arrow(&mut self, dir: vial::Dir, cols: u16) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Pick {
                candidates, cursor, ..
            } => move_cursor(cursor, candidates.len(), dir),
            ModelKind::Open { .. } => {}
            ModelKind::Lost {
                session,
                cursor,
                reconnect: None,
                ..
            } => {
                let unsaved = session.board.pending().len();
                move_cursor(cursor, Choice::list(unsaved).len(), dir);
            }
            ModelKind::Lost { .. } => {}
            ModelKind::Map(session) => session.nudge(dir),
            ModelKind::Sheet { session, spot, .. } => {
                match dir {
                    vial::Dir::Up | vial::Dir::Down => {
                        move_cursor(&mut spot.row, sheet_rows(&session.board, spot.tab), dir);
                    }
                    vial::Dir::Left | vial::Dir::Right => {
                        let cols = sheet_cols(&session.board, *spot);
                        move_cursor(&mut spot.col, cols, dir);
                    }
                }
                clamp_col(&session.board, spot);
            }
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                if matches!(browse, Browse::Part { .. }) {
                    // A binding in columns: up and down choose, left and right change column.
                    match dir {
                        vial::Dir::Up | vial::Dir::Down => {
                            let len = session.list(browse).hits.len();
                            move_cursor(cursor, len, dir);
                            follow(session, browse, *cursor);
                        }
                        vial::Dir::Left => shift_column(session, browse, cursor, -1),
                        vial::Dir::Right => shift_column(session, browse, cursor, 1),
                    }
                } else {
                    let hits = session.list(browse).hits;
                    // The Basic group is a keyboard: arrows go to the key beside, above, below.
                    let keyboard = matches!(browse, Browse::Shelf(ShelfIndex(0)));
                    match keyboard
                        .then(|| vial::basic_step(&hits, *cursor, cols, dir))
                        .flatten()
                    {
                        Some(next) => *cursor = next,
                        None => {
                            grid_step(cursor, hits.len(), vial::search_across(&hits, cols), dir)
                        }
                    }
                }
            }
            ModelKind::Palette {
                session,
                text,
                cursor,
                ..
            } => {
                let len = palette(&session.board, text).len();
                // Nothing picked yet: the first arrow lands on the first row.
                *cursor = match palette_pick(text, len, *cursor) {
                    None => (len > 0).then_some(0),
                    Some(mut at) => {
                        move_cursor(&mut at, len, dir);
                        Some(at)
                    }
                };
            }
            ModelKind::Edit { .. }
            | ModelKind::Hold { .. }
            | ModelKind::Release { .. }
            | ModelKind::Ask { .. }
            | ModelKind::Help { .. } => {}
        }
        Ok(Step::Continue)
    }

    fn layer(&mut self, delta: i8) -> Result<Step> {
        if let ModelKind::Map(session) = &mut self.kind {
            session.layer_step(delta);
        }
        Ok(Step::Continue)
    }

    /// One tab per layer, then each editor the board has entries for. A sheet starts at its first cell.
    fn tab(&mut self, delta: i8) -> Result<Step> {
        // After `:` Tab fills in the picked command's name, or the first match's.
        if let ModelKind::Palette {
            session,
            text,
            cursor,
            ..
        } = &mut self.kind
        {
            if text.starts_with(':') {
                let rows = palette(&session.board, text);
                let pick = palette_pick(text, rows.len(), *cursor).unwrap_or(0);
                if let Some((_, _, Goal::Run(command))) = rows.get(pick) {
                    *text = format!(":{}", command_name(*command));
                    *cursor = None;
                }
            }
            return Ok(Step::Continue);
        }
        self.tab_to(|at, count| {
            (at as isize + isize::from(delta)).rem_euclid(count as isize) as usize
        })
    }

    /// To the tab `pick` chooses from the current one and the count.
    fn tab_to(&mut self, pick: impl FnOnce(usize, usize) -> usize) -> Result<Step> {
        let (mut session, at) = match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Map(session) => {
                let at = usize::from(session.layer);
                (session, at)
            }
            ModelKind::Sheet { session, spot, .. } => {
                let editors = session.board.editors();
                let at = usize::from(session.board.layer_count())
                    + editors.iter().position(|tab| *tab == spot.tab).unwrap_or(0);
                (session, at)
            }
            other => {
                self.kind = other;
                return Ok(Step::Continue);
            }
        };
        let layers = usize::from(session.board.layer_count());
        let editors = session.board.editors();
        let count = (layers + editors.len()).max(1);
        let next = pick(at, count).min(count - 1);
        self.kind = if next < layers {
            session.layer = next as u8;
            ModelKind::Map(session)
        } else {
            ModelKind::Sheet {
                session,
                spot: vial::Spot {
                    tab: editors[next - layers],
                    row: 0,
                    col: 0,
                },
            }
        };
        Ok(Step::Continue)
    }

    fn char(&mut self, ch: char) -> Result<Step> {
        if matches!(self.kind, ModelKind::Ask { .. }) {
            let ModelKind::Ask {
                session,
                back,
                question,
            } = mem::replace(&mut self.kind, vacant())
            else {
                unreachable!("checked above");
            };
            return self.answer(session, back, question, ch);
        }
        match &mut self.kind {
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.type_char(ch);
                *cursor = 0;
                follow(session, browse, 0);
            }
            ModelKind::Edit { field, text, .. } => {
                if field.accepts(text, ch) {
                    text.push(ch);
                }
            }
            ModelKind::Palette { text, cursor, .. } => {
                if !ch.is_control() {
                    text.push(ch);
                    *cursor = None;
                }
            }
            _ => {}
        }
        Ok(Step::Continue)
    }

    fn page(&mut self, delta: i8, cols: u16) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Pick {
                candidates, cursor, ..
            } => shift(cursor, candidates.len(), delta),
            ModelKind::Open { .. } | ModelKind::Lost { .. } => {}
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                // Four rows of the grid at a time; ten values of a column.
                let hits = session.list(browse).hits;
                let step = match browse {
                    Browse::Part { .. } => 10,
                    _ => vial::search_across(&hits, cols) * 4,
                };
                let last = hits.len().saturating_sub(1);
                *cursor = if delta < 0 {
                    cursor.saturating_sub(step)
                } else {
                    (*cursor + step).min(last)
                };
                follow(session, browse, *cursor);
            }
            ModelKind::Sheet { session, spot, .. } => {
                shift(&mut spot.row, sheet_rows(&session.board, spot.tab), delta);
                clamp_col(&session.board, spot);
            }
            ModelKind::Help { scroll, under } => {
                shift(scroll, help_lines(under).len(), delta);
            }
            ModelKind::Map(session) => session.layer_step(delta),
            ModelKind::Edit { .. }
            | ModelKind::Hold { .. }
            | ModelKind::Release { .. }
            | ModelKind::Ask { .. }
            | ModelKind::Palette { .. } => {}
        }
        Ok(Step::Continue)
    }

    fn end(&mut self, which: keys::End) -> Result<Step> {
        let pick = |len: usize| match which {
            keys::End::First => 0,
            keys::End::Last => len.saturating_sub(1),
        };
        match &mut self.kind {
            ModelKind::Pick {
                candidates, cursor, ..
            } => *cursor = pick(candidates.len()),
            ModelKind::Open { .. } | ModelKind::Lost { .. } => {}
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                *cursor = pick(session.list(browse).hits.len());
                follow(session, browse, *cursor);
            }
            ModelKind::Sheet { session, spot, .. } => {
                spot.row = pick(sheet_rows(&session.board, spot.tab));
                clamp_col(&session.board, spot);
            }
            ModelKind::Help { scroll, under } => *scroll = pick(help_lines(under).len()),
            ModelKind::Map(session) => {
                session.selection = match which {
                    keys::End::First => session.board.first_selectable(),
                    keys::End::Last => session.board.last_selectable(),
                };
            }
            ModelKind::Edit { .. }
            | ModelKind::Hold { .. }
            | ModelKind::Release { .. }
            | ModelKind::Ask { .. }
            | ModelKind::Palette { .. } => {}
        }
        Ok(Step::Continue)
    }

    fn open_help(&mut self) -> Result<Step> {
        if matches!(self.kind, ModelKind::Help { .. }) {
            return Ok(Step::Continue);
        }
        let under = mem::replace(&mut self.kind, vacant());
        self.kind = ModelKind::Help {
            under: Box::new(under),
            scroll: 0,
        };
        Ok(Step::Continue)
    }

    fn close_help(&mut self) -> Result<Step> {
        match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Help { under, .. } => self.kind = *under,
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    fn scroll_help(&mut self, delta: i8) -> Result<Step> {
        if let ModelKind::Help { scroll, under } = &mut self.kind {
            let len = help_lines(under).len();
            let step = i32::from(delta);
            let next = *scroll as i32 + step;
            *scroll = next.clamp(0, len.saturating_sub(1) as i32) as usize;
        }
        Ok(Step::Continue)
    }

    fn backspace(&mut self) -> Result<Step> {
        match &mut self.kind {
            // Backspace with nothing typed closes the key list, for a keyboard without Esc.
            ModelKind::Search {
                browse: Browse::Shelf(_),
                ..
            } => return self.cancel(),
            // The same for a part with nothing typed for it.
            ModelKind::Search {
                browse: Browse::Part { parts, query },
                ..
            } if *query == parts.prefix() => return self.cancel(),
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.backspace();
                *cursor = 0;
                follow(session, browse, 0);
            }
            ModelKind::Edit { text, .. } => {
                text.pop();
            }
            // Backspace on an empty line closes it, as in vim after `:`.
            ModelKind::Palette { text, .. } if text.is_empty() || text == ":" => {
                return self.cancel()
            }
            ModelKind::Palette { text, cursor, .. } => {
                text.pop();
                *cursor = None;
            }
            _ => {}
        }
        Ok(Step::Continue)
    }

    fn shelf(&mut self, delta: i8) -> Result<Step> {
        if let ModelKind::Search {
            session,
            browse,
            cursor,
            ..
        } = &mut self.kind
        {
            if matches!(browse, Browse::Part { .. }) {
                shift_column(session, browse, cursor, delta);
            } else {
                let count = session.board.shelf_count();
                if count > 0 {
                    let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                    *browse = old.shift_shelf(delta, count);
                }
                *cursor = 0;
            }
        }
        Ok(Step::Continue)
    }

    fn confirm(&mut self) -> Result<Step> {
        match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Pick {
                candidates,
                cursor,
                failed,
            } => return self.open_pick(candidates, cursor, failed),
            lost @ ModelKind::Lost { .. } => {
                self.kind = lost;
                return self.choose_lost();
            }
            ModelKind::Map(session) => self.kind = searching(session, Target::Key),
            ModelKind::Sheet { session, spot } => match flag_toggle(&session.board, spot) {
                Some(write) => self.land(session, write, Some(spot)),
                None => self.kind = open_cell(session, spot),
            },
            ModelKind::Palette {
                session,
                back,
                text,
                cursor,
            } => {
                let rows = palette(&session.board, &text);
                match palette_pick(&text, rows.len(), cursor) {
                    Some(at) => return self.go(session, back, rows[at].2, command_arg(&text)),
                    // Typed, and nothing matches: the popup says so and stays.
                    None if rows.is_empty() => {
                        self.kind = ModelKind::Palette {
                            session,
                            back,
                            text,
                            cursor,
                        }
                    }
                    // Nothing typed, nothing picked: Enter only closes, as `:` then Enter in vim.
                    None => self.kind = back_to(session, back),
                }
            }
            ModelKind::Search {
                session,
                browse,
                cursor,
                wrap,
                target,
            } => self.confirm_search(session, browse, cursor, wrap, target),
            ModelKind::Edit {
                session,
                spot,
                field,
                text,
            } => match field_write(&session, spot, field, &text) {
                Some((write, after)) => self.land(session, write, Some(after)),
                None => self.kind = back_to(session, Some(spot)),
            },
            ModelKind::Ask {
                session,
                back,
                question,
            } => return self.answer(session, back, question, 'y'),
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    /// Starts reading the highlighted board. A board that cannot open stays on the list with why.
    fn open_pick(
        &mut self,
        candidates: Vec<vial::Candidate>,
        cursor: usize,
        failed: Option<(usize, String)>,
    ) -> Result<Step> {
        match candidates.get(cursor).map(vial::Load::begin) {
            Some(Ok(load)) => {
                self.kind = ModelKind::Open {
                    candidates,
                    cursor,
                    load,
                    land: 0,
                };
                self.pump()
            }
            Some(Err(err)) => {
                self.kind = ModelKind::Pick {
                    candidates,
                    cursor,
                    failed: Some((cursor, why_not(&err))),
                };
                Ok(Step::Continue)
            }
            None => {
                self.kind = ModelKind::Pick {
                    candidates,
                    cursor,
                    failed,
                };
                Ok(Step::Continue)
            }
        }
    }

    /// Reads reports for one slice, then hands back `Working` so the screen repaints and Esc
    /// gets through. A board that reads fast opens inside the first slice.
    fn pump(&mut self) -> Result<Step> {
        let ModelKind::Open {
            mut candidates,
            cursor,
            mut load,
            land,
        } = mem::replace(&mut self.kind, vacant())
        else {
            return Ok(Step::Continue);
        };
        let until = Instant::now() + SLICE;
        loop {
            match load.step(&mut candidates[cursor]) {
                Ok(_) if load.ready() => {
                    let board = load.finish(candidates.remove(cursor));
                    let more = board.reading().is_some();
                    let mut session = Session::mapped(board);
                    session.layer = land.min(session.board.layer_count().saturating_sub(1));
                    self.kind = ModelKind::Map(session);
                    return Ok(if more { Step::Working } else { Step::Continue });
                }
                Ok(_) if Instant::now() < until => {}
                Ok(_) => {
                    self.kind = ModelKind::Open {
                        candidates,
                        cursor,
                        load,
                        land,
                    };
                    return Ok(Step::Working);
                }
                Err(err) => {
                    self.kind = ModelKind::Pick {
                        candidates,
                        cursor,
                        failed: Some((cursor, why_not(&err))),
                    };
                    return Ok(Step::Continue);
                }
            }
        }
    }

    /// `x` and Backspace on a sheet: a key cell becomes `KC_NO`, a macro step goes away.
    fn clear(&mut self) -> Result<Step> {
        let kind = mem::replace(&mut self.kind, vacant());
        let ModelKind::Sheet { session, spot, .. } = kind else {
            self.kind = kind;
            return Ok(Step::Continue);
        };
        let board = &session.board;
        let write = match spot.tab {
            vial::Tab::TapDance if spot.col < 4 => {
                let mut entry = board.tap_dances()[spot.row];
                entry.keys[spot.col] = 0;
                Some(vial::Write::TapDance {
                    index: spot.row as u8,
                    entry,
                })
            }
            vial::Tab::Combos => {
                let mut entry = board.combos()[spot.row];
                entry.keys[spot.col] = 0;
                Some(vial::Write::Combo {
                    index: spot.row as u8,
                    entry,
                })
            }
            vial::Tab::Macros if spot.col < board.macros()[spot.row].len() => {
                let mut macros = board.macros().to_vec();
                macros[spot.row].remove(spot.col);
                Some(vial::Write::Macros(macros))
            }
            vial::Tab::Section(kind) => {
                let slot = board
                    .section(kind)
                    .get(spot.row)
                    .map(|row| vial::section_slot(kind, row, spot.col));
                match slot {
                    Some(vial::Slot::Column(col)) => {
                        let blank = match kind.columns()[col].cell {
                            vial::Cell::Binding => "&none",
                            _ => "",
                        };
                        Some(section_edit(board, kind, spot.row, |row| {
                            row.cells[col] = blank.to_string();
                        }))
                    }
                    Some(vial::Slot::Tail(index)) => {
                        Some(section_edit(board, kind, spot.row, |row| {
                            row.tail.remove(index);
                        }))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        match write {
            Some(write) => self.land(session, write, Some(spot)),
            None => self.kind = ModelKind::Sheet { session, spot },
        }
        Ok(Step::Continue)
    }

    /// A new macro step at the cursor. Keys go through search, text and delays through a field.
    fn insert(&mut self, step: keys::Step) -> Result<Step> {
        let kind = mem::replace(&mut self.kind, vacant());
        let ModelKind::Sheet { session, spot, .. } = kind else {
            self.kind = kind;
            return Ok(Step::Continue);
        };
        if let vial::Tab::Section(kind) = spot.tab {
            // A section's bindings take a tap only: `t`, as in the key list.
            self.kind = if kind.tail().is_some()
                && matches!(step, keys::Step::Tap)
                && spot.col > kind.columns().len()
            {
                searching(
                    session,
                    Target::Cell {
                        spot,
                        insert: Some(Press::Tap),
                    },
                )
            } else {
                ModelKind::Sheet { session, spot }
            };
            return Ok(Step::Continue);
        }
        if spot.tab != vial::Tab::Macros {
            self.kind = ModelKind::Sheet { session, spot };
            return Ok(Step::Continue);
        }
        let press = |press| Target::Cell {
            spot,
            insert: Some(press),
        };
        self.kind = match step {
            keys::Step::Tap => searching(session, press(Press::Tap)),
            keys::Step::Down => searching(session, press(Press::Down)),
            keys::Step::Up => searching(session, press(Press::Up)),
            keys::Step::Text => ModelKind::Edit {
                session,
                spot,
                field: Field::Text { insert: true },
                text: String::new(),
            },
            keys::Step::Delay => ModelKind::Edit {
                session,
                spot,
                field: Field::Delay { insert: true },
                text: "100".to_string(),
            },
        };
        Ok(Step::Continue)
    }

    /// Moves the macro step under the cursor one place left or right.
    fn reorder(&mut self, delta: i8) -> Result<Step> {
        let kind = mem::replace(&mut self.kind, vacant());
        let ModelKind::Sheet { session, spot, .. } = kind else {
            self.kind = kind;
            return Ok(Step::Continue);
        };
        // The movable cells: macro steps, or a section row's bindings after its properties.
        let (offset, len) = match spot.tab {
            vial::Tab::Macros => (0, session.board.macros()[spot.row].len()),
            vial::Tab::Section(kind) => (
                1 + kind.columns().len(),
                session
                    .board
                    .section(kind)
                    .get(spot.row)
                    .map_or(0, |row| row.tail.len()),
            ),
            _ => (0, 0),
        };
        let at = spot.col as isize - offset as isize;
        let to = at + isize::from(delta);
        if at < 0 || at as usize >= len || to < 0 || to as usize >= len {
            self.kind = ModelKind::Sheet { session, spot };
            return Ok(Step::Continue);
        }
        let write = match spot.tab {
            vial::Tab::Section(kind) => section_edit(&session.board, kind, spot.row, |row| {
                row.tail.swap(at as usize, to as usize);
            }),
            _ => {
                let mut macros = session.board.macros().to_vec();
                macros[spot.row].swap(at as usize, to as usize);
                vial::Write::Macros(macros)
            }
        };
        let spot = vial::Spot {
            col: (to + offset as isize) as usize,
            ..spot
        };
        self.land(session, write, Some(spot));
        Ok(Step::Continue)
    }

    fn cancel(&mut self) -> Result<Step> {
        match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Lost {
                session,
                back,
                cursor,
                reconnect: Some(_),
                ..
            } => {
                self.kind = ModelKind::Lost {
                    session,
                    back,
                    why: "reconnect stopped".to_string(),
                    cursor,
                    reconnect: None,
                };
            }
            ModelKind::Open {
                candidates, cursor, ..
            } => {
                self.notice = Some("stopped opening the keyboard".to_string());
                self.kind = ModelKind::Pick {
                    candidates,
                    cursor,
                    failed: None,
                };
            }
            ModelKind::Search {
                session, target, ..
            } => self.kind = back_to(session, target.spot()),
            ModelKind::Edit { session, spot, .. } => self.kind = back_to(session, Some(spot)),
            ModelKind::Hold { session, back, .. } => {
                let session = session.cancel();
                let count = session.board.pending().len();
                self.notice = Some(format!("save stopped, {count} change(s) still unsaved"));
                self.kind = back_to(session, back);
            }
            ModelKind::Ask {
                session,
                back,
                question,
            } => return self.answer(session, back, question, 'n'),
            ModelKind::Palette { session, back, .. } => self.kind = back_to(session, back),
            ModelKind::Help { under, .. } => self.kind = *under,
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    /// `y` or Enter says yes, `n` or Esc says no. The quit question also takes `w` to write
    /// then quit and `d` to quit without writing. Any other key leaves the question up.
    fn answer(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        question: Question,
        ch: char,
    ) -> Result<Step> {
        let mut session = session;
        match (question, ch) {
            (Question::RemoveLayer { layer, .. }, 'y' | 'Y') => {
                let op = vial::LayerOp::Remove(layer);
                return self.change_layers(session, back, op, layer.saturating_sub(1));
            }
            (Question::RemoveLayer { .. }, 'n' | 'N') => self.kind = back_to(session, back),
            (Question::Quit { .. }, 'w' | 'W' | 'y' | 'Y') => {
                return self.save(session, back, true)
            }
            (Question::Quit { .. }, 'd' | 'D') => return Ok(Step::Quit),
            (Question::Discard { count, .. }, 'y' | 'Y') => {
                session.board.discard();
                self.notice = Some(format!("discarded {count} staged change(s)"));
                self.kind = back_to(session, back);
            }
            (Question::Quit { .. } | Question::Discard { .. }, 'n' | 'N') => {
                self.kind = back_to(session, back);
            }
            (question, _) => {
                self.kind = ModelKind::Ask {
                    session,
                    back,
                    question,
                };
            }
        }
        Ok(Step::Continue)
    }

    fn tick(&mut self) -> Result<Step> {
        if matches!(self.kind, ModelKind::Open { .. }) {
            return self.pump();
        }
        // A Studio card waiting for its unlock key: ask again, and open the moment it is in.
        if let ModelKind::Pick {
            failed: Some((at, why)),
            ..
        } = &self.kind
        {
            if why == LOCKED_CARD {
                let at = *at;
                let ModelKind::Pick {
                    candidates, failed, ..
                } = mem::replace(&mut self.kind, vacant())
                else {
                    unreachable!("matched above");
                };
                return self.open_pick(candidates, at, failed);
            }
        }
        if matches!(
            self.kind,
            ModelKind::Lost {
                reconnect: Some(_),
                ..
            }
        ) {
            return self.pump_reconnect();
        }
        if let Some(board) = reading_board(&mut self.kind) {
            let until = Instant::now() + SLICE;
            loop {
                match board.read_rest() {
                    Ok(true) => return Ok(Step::Continue),
                    Ok(false) if Instant::now() < until => {}
                    Ok(false) => return Ok(Step::Working),
                    Err(err) => {
                        let (session, back) = into_session(mem::replace(&mut self.kind, vacant()))
                            .expect("a view with a board");
                        return Ok(self.lose(session, back, &err));
                    }
                }
            }
        }
        let kind = mem::replace(&mut self.kind, vacant());
        let (session, back, quit) = match kind {
            ModelKind::Hold {
                session,
                back,
                quit,
            } => (session, back, quit),
            ModelKind::Release {
                session,
                back,
                quit,
                quiet: true,
            } => return self.save(session, back, quit),
            ModelKind::Release {
                session,
                back,
                quit,
                ..
            } => {
                self.kind = ModelKind::Release {
                    session,
                    back,
                    quit,
                    quiet: true,
                };
                return Ok(Step::Continue);
            }
            other => {
                self.kind = other;
                return Ok(Step::Continue);
            }
        };
        match session.poll() {
            HoldStep::Waiting(session) => {
                self.kind = ModelKind::Hold {
                    session,
                    back,
                    quit,
                };
            }
            HoldStep::Mapped(session) => {
                self.kind = ModelKind::Release {
                    session,
                    back,
                    quit,
                    quiet: true,
                };
            }
            HoldStep::Disconnected(err, session) => return Ok(self.lose(session, back, &err)),
        }
        Ok(Step::Continue)
    }

    /// The keyboard stopped answering. Its board stays on screen under the choices.
    fn lose(&mut self, session: Session<Mapping>, back: Option<vial::Spot>, err: &Error) -> Step {
        self.kind = ModelKind::Lost {
            session,
            back,
            why: match err {
                Error::Disconnected => "the keyboard stopped answering".to_string(),
                other => other.to_string(),
            },
            cursor: 0,
            reconnect: None,
        };
        Step::Continue
    }

    /// Acts on the highlighted choice. Reconnecting finds the same keyboard by name, USB id,
    /// and serial; a keyboard that is not back yet says so and the choices stay.
    fn choose_lost(&mut self) -> Result<Step> {
        let (mut session, back, mut why, cursor) = match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Lost {
                session,
                back,
                why,
                cursor,
                reconnect: None,
            } => (session, back, why, cursor),
            other => {
                self.kind = other;
                return Ok(Step::Continue);
            }
        };
        let unsaved = session.board.pending().len();
        let choice = Choice::list(unsaved).get(cursor).copied();
        // The old handle must close first: a HID device still held does not enumerate again.
        if matches!(
            choice,
            Some(Choice::Keep | Choice::Revert | Choice::Restart)
        ) {
            session.board.disown();
        }
        match choice {
            Some(Choice::Quit) => return Ok(Step::Quit),
            Some(Choice::Restart) => match session.board.find_again() {
                Ok(candidates) if !candidates.is_empty() => {
                    self.kind = ModelKind::Pick {
                        candidates,
                        cursor: 0,
                        failed: None,
                    };
                    return Ok(Step::Continue);
                }
                Ok(_) => why = "no keyboard answers yet: wake it or plug it in".to_string(),
                Err(err) => why = err.to_string(),
            },
            Some(choice @ (Choice::Keep | Choice::Revert)) => {
                let found = session
                    .board
                    .find_again()
                    .map(|all| all.into_iter().find(|found| session.board.is(found)));
                match found {
                    Ok(Some(candidate)) => match vial::Load::begin(&candidate) {
                        Ok(load) => {
                            self.kind = ModelKind::Lost {
                                session,
                                back,
                                why,
                                cursor,
                                reconnect: Some(Box::new((
                                    candidate,
                                    load,
                                    choice == Choice::Keep,
                                ))),
                            };
                            return self.pump_reconnect();
                        }
                        Err(err) => why = why_not(&err),
                    },
                    Ok(None) => {
                        why = "not back yet: wake the keyboard or plug it in, then try again"
                            .to_string();
                    }
                    Err(err) => why = err.to_string(),
                }
            }
            None => {}
        }
        self.kind = ModelKind::Lost {
            session,
            back,
            why,
            cursor,
            reconnect: None,
        };
        Ok(Step::Continue)
    }

    /// Reads the keyboard again a slice at a time. Keeping the changes waits for the tap dances,
    /// combos, and macros too, since the changes may be to them. The new board takes the old
    /// layer and selected key, and with `keep` every unsaved change as a staged one.
    fn pump_reconnect(&mut self) -> Result<Step> {
        let (session, back, why, cursor, mut candidate, mut load, keep) =
            match mem::replace(&mut self.kind, vacant()) {
                ModelKind::Lost {
                    session,
                    back,
                    why,
                    cursor,
                    reconnect: Some(reconnect),
                } => {
                    let (candidate, load, keep) = *reconnect;
                    (session, back, why, cursor, candidate, load, keep)
                }
                other => {
                    self.kind = other;
                    return Ok(Step::Continue);
                }
            };
        let until = Instant::now() + SLICE;
        loop {
            match load.step(&mut candidate) {
                Ok(done) if done || (!keep && load.ready()) => {
                    let mut board = load.finish(candidate);
                    let kept = if keep {
                        session.board.pending()
                    } else {
                        Vec::new()
                    };
                    let count = kept.len();
                    for write in kept {
                        board.stage(write);
                    }
                    let more = board.reading().is_some();
                    let layer = if session.layer < board.layer_count() {
                        session.layer
                    } else {
                        0
                    };
                    let fresh = Session {
                        selection: session.selection.or(board.first_selectable()),
                        board,
                        layer,
                        phase: Mapping,
                    };
                    self.notice = Some(match count {
                        0 => "reconnected".to_string(),
                        1 => "reconnected, 1 unsaved change kept".to_string(),
                        _ => format!("reconnected, {count} unsaved changes kept"),
                    });
                    self.kind = if keep {
                        back_to(fresh, back)
                    } else {
                        ModelKind::Map(fresh)
                    };
                    return Ok(if more { Step::Working } else { Step::Continue });
                }
                Ok(_) if Instant::now() < until => {}
                Ok(_) => {
                    self.kind = ModelKind::Lost {
                        session,
                        back,
                        why,
                        cursor,
                        reconnect: Some(Box::new((candidate, load, keep))),
                    };
                    return Ok(Step::Working);
                }
                Err(err) => {
                    self.kind = ModelKind::Lost {
                        session,
                        back,
                        why: why_not(&err),
                        cursor,
                        reconnect: None,
                    };
                    return Ok(Step::Continue);
                }
            }
        }
    }
}

/// The open board, for the background read of its entries. Not during an unlock or a save,
/// whose ticks belong to them.
fn reading_board(kind: &mut ModelKind) -> Option<&mut vial::Board> {
    match kind {
        ModelKind::Map(session)
        | ModelKind::Sheet { session, .. }
        | ModelKind::Search { session, .. }
        | ModelKind::Edit { session, .. }
        | ModelKind::Ask { session, .. }
        | ModelKind::Palette { session, .. } => Some(&mut session.board),
        ModelKind::Help { under, .. } => reading_board(under),
        _ => None,
    }
}

/// The session under any view that has one, and the sheet that view came from.
/// `None` on the chooser, which has no board yet.
fn into_session(kind: ModelKind) -> Option<(Session<Mapping>, Option<vial::Spot>)> {
    match kind {
        ModelKind::Map(session) => Some((session, None)),
        ModelKind::Sheet { session, spot } | ModelKind::Edit { session, spot, .. } => {
            Some((session, Some(spot)))
        }
        ModelKind::Search {
            session, target, ..
        } => Some((session, target.spot())),
        ModelKind::Hold { session, back, .. } => Some((session.cancel(), back)),
        ModelKind::Release { session, back, .. }
        | ModelKind::Ask { session, back, .. }
        | ModelKind::Palette { session, back, .. }
        | ModelKind::Lost { session, back, .. } => Some((session, back)),
        ModelKind::Help { under, .. } => into_session(*under),
        ModelKind::Pick { .. } | ModelKind::Open { .. } => None,
    }
}

/// The event a popup chip's key sends: `enter`, `esc`, `tab`, or one letter. `j k` and `↑↓`
/// name more than one key, so they are labels, not buttons.
fn chip_event(key: &str) -> Option<Event> {
    Some(match key {
        "enter" => Event::Confirm,
        "esc" => Event::Cancel,
        "tab" => Event::Tab,
        one if one.chars().count() == 1 => Event::Char(one.chars().next()?),
        _ => return None,
    })
}

/// The key list for a key or cell. On a ZMK board or keymap file it opens on the binding's
/// last part, as Keymap Editor's parameter chip; on Vial, on the first group, with the key's
/// own keycode highlighted when that group has it.
fn searching(session: Session<Mapping>, target: Target) -> ModelKind {
    let mut browse = binding_parts(&session, &target).unwrap_or(Browse::Shelf(ShelfIndex(0)));
    let mut cursor = land_on(&session, &mut browse);
    if let (Browse::Shelf(_), Target::Key, Some(place)) = (&browse, &target, session.selection) {
        if let Some(code) = session.board.key_code(session.layer, place) {
            let hits = session.list(&browse).hits;
            cursor = hits.iter().position(|hit| hit.code == code).unwrap_or(0);
        }
    }
    ModelKind::Search {
        session,
        browse,
        cursor,
        wrap: None,
        target,
    }
}

/// The parts of the binding under the cursor, `None` where bindings are not typed (Vial).
/// A binding not written as `&…`, such as a `#define` call, starts at the behavior list.
fn binding_parts(session: &Session<Mapping>, target: &Target) -> Option<Browse> {
    let board = &session.board;
    board.typed_complete("&")?;
    let text = match target {
        Target::Key => session
            .selection
            .and_then(|place| board.key_text(session.layer, place)),
        Target::Cell {
            spot:
                spot @ vial::Spot {
                    tab: vial::Tab::Section(kind),
                    ..
                },
            insert: None,
        } => board.section(*kind).get(spot.row).and_then(|row| {
            match vial::section_slot(*kind, row, spot.col) {
                vial::Slot::Column(col) => row.cells.get(col).cloned(),
                vial::Slot::Tail(index) => row.tail.get(index).cloned(),
                _ => None,
            }
        }),
        Target::Cell { .. } => None,
    };
    let words: Vec<String> = text
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let words = if words.first().is_some_and(|word| word.starts_with('&')) {
        words
    } else {
        Vec::new()
    };
    let mut parts = Parts {
        at: 0,
        original: words.clone(),
        words,
    };
    parts.at = column_count(session, &parts) - 1;
    Some(Browse::Part {
        query: parts.prefix(),
        parts,
    })
}

/// The hit for the active column's word, so a column opens on what the binding has.
fn current_hit(session: &Session<Mapping>, browse: &Browse) -> usize {
    let Browse::Part { parts, .. } = browse else {
        return 0;
    };
    parts
        .words
        .get(parts.at)
        .and_then(|word| hit_of(&session.list(browse).hits, word))
        .unwrap_or(0)
}

/// The highlight chooses: the active column's word becomes the hit at `cursor`.
fn follow(session: &Session<Mapping>, browse: &mut Browse, cursor: usize) {
    if !matches!(browse, Browse::Part { .. }) {
        return;
    }
    let Some(word) = session
        .list(browse)
        .hits
        .get(cursor)
        .map(|hit| hit.legend.clone())
    else {
        return;
    };
    if let Browse::Part { parts, .. } = browse {
        parts.choose(&word);
    }
}

/// Moves to the next or previous column, wrapping, and highlights what it holds.
fn shift_column(session: &Session<Mapping>, browse: &mut Browse, cursor: &mut usize, delta: i8) {
    let Browse::Part { parts, .. } = browse else {
        return;
    };
    let count = column_count(session, parts);
    let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
    *browse = old.shift_shelf(delta, count);
    *cursor = land_on(session, browse);
}

/// The highlight for the active column: what it holds, or, when it holds nothing the column
/// takes, its first value, chosen, so Enter always moves the binding on.
fn land_on(session: &Session<Mapping>, browse: &mut Browse) -> usize {
    let Browse::Part { parts, .. } = browse else {
        return 0;
    };
    if !word_valid(session, parts, parts.at) {
        follow(session, browse, 0);
    }
    current_hit(session, browse)
}

fn back_to(session: Session<Mapping>, spot: Option<vial::Spot>) -> ModelKind {
    match spot {
        None => ModelKind::Map(session),
        Some(mut spot) => {
            clamp_col(&session.board, &mut spot);
            ModelKind::Sheet { session, spot }
        }
    }
}

/// Enter on a sheet cell: a key opens search, a number or text opens its field.
fn open_cell(session: Session<Mapping>, spot: vial::Spot) -> ModelKind {
    let board = &session.board;
    let field = match spot.tab {
        vial::Tab::TapDance if spot.col == 4 => {
            Some((Field::Term, board.tap_dances()[spot.row].term.to_string()))
        }
        vial::Tab::Macros => match board.macros()[spot.row].get(spot.col) {
            Some(vial::Action::Text(text)) => Some((Field::Text { insert: false }, text.clone())),
            Some(vial::Action::Delay(ms)) => Some((Field::Delay { insert: false }, ms.to_string())),
            Some(_) => None,
            // The append slot takes a tap.
            None => {
                return searching(
                    session,
                    Target::Cell {
                        spot,
                        insert: Some(Press::Tap),
                    },
                )
            }
        },
        vial::Tab::Section(kind) => {
            let Some(row) = board.section(kind).get(spot.row) else {
                return ModelKind::Sheet { session, spot };
            };
            match vial::section_slot(kind, row, spot.col) {
                vial::Slot::Name if kind == vial::Kind::Encoders => {
                    return ModelKind::Sheet { session, spot };
                }
                vial::Slot::Name => Some((Field::Name, row.name.clone())),
                vial::Slot::Column(col) if kind.columns()[col].cell == vial::Cell::Binding => None,
                vial::Slot::Column(col) => Some((Field::Prop { col }, row.cells[col].clone())),
                vial::Slot::Tail(_) => None,
                vial::Slot::Append => {
                    return searching(
                        session,
                        Target::Cell {
                            spot,
                            insert: Some(Press::Tap),
                        },
                    )
                }
            }
        }
        _ => None,
    };
    match field {
        Some((field, text)) => ModelKind::Edit {
            session,
            spot,
            field,
            text,
        },
        None => searching(session, Target::Cell { spot, insert: None }),
    }
}

/// Enter on a flag cell of a keymap-file section sets or clears it at once.
fn flag_toggle(board: &vial::Board, spot: vial::Spot) -> Option<vial::Write> {
    let vial::Tab::Section(kind) = spot.tab else {
        return None;
    };
    let row = board.section(kind).get(spot.row)?;
    let vial::Slot::Column(col) = vial::section_slot(kind, row, spot.col) else {
        return None;
    };
    if kind.columns()[col].cell != vial::Cell::Flag {
        return None;
    }
    Some(section_edit(board, kind, spot.row, |row| {
        row.cells[col] = if row.cells[col].is_empty() {
            "yes".to_string()
        } else {
            String::new()
        }
    }))
}

impl Model {
    /// Enter in the key list stages the highlighted hit at once: the board already previews it,
    /// and `u` takes it back. A `(kc)` code first asks for the basic key that fills it.
    fn confirm_search(
        &mut self,
        session: Session<Mapping>,
        browse: Browse,
        cursor: usize,
        wrap: Option<Wrap>,
        target: Target,
    ) {
        let mut session = session;
        // A ZMK binding in columns: Enter stages it once every column holds a value its
        // behavior takes, and otherwise moves to the first column still to choose.
        let columns = match &browse {
            Browse::Part { parts, .. } => {
                let count = column_count(&session, parts);
                let missing = (0..count).find(|&i| !word_valid(&session, parts, i));
                let text = parts.words[..count.min(parts.words.len())].join(" ");
                Some((count, missing, text))
            }
            _ => None,
        };
        if let Some((count, missing, text)) = columns {
            let done = missing.is_none()
                && session.board.typed_complete(&format!("{text} ")) == Some(true);
            let mut browse = match browse {
                Browse::Part { mut parts, .. } if !done => {
                    parts.at = missing.unwrap_or(count - 1);
                    Browse::Part {
                        query: parts.prefix(),
                        parts,
                    }
                }
                browse => {
                    match self.stage_binding(session, &target, text) {
                        Ok(()) => return,
                        Err(back) => session = back,
                    }
                    browse
                }
            };
            let cursor = land_on(&session, &mut browse);
            self.kind = ModelKind::Search {
                session,
                browse,
                cursor,
                wrap,
                target,
            };
            return;
        }
        let found = session.list(&browse).hits.into_iter().nth(cursor);
        let code = match (found, &wrap) {
            // Vial fills a `(kc)` code with a basic key only.
            (Some(found), Some(outer)) if found.code <= 0xFF => outer.code | found.code,
            (Some(found), None) if found.masked => {
                self.kind = ModelKind::Search {
                    session,
                    browse: Browse::Shelf(ShelfIndex(0)),
                    cursor: 0,
                    wrap: Some(Wrap {
                        code: found.code,
                        legend: found.legend,
                    }),
                    target,
                };
                return;
            }
            (Some(found), None) => found.code,
            _ => {
                self.kind = ModelKind::Search {
                    session,
                    browse,
                    cursor,
                    wrap,
                    target,
                };
                return;
            }
        };
        let hit = Hit { code };
        let (write, after) = match target {
            Target::Key => {
                let Some(place) = session.selection else {
                    self.kind = ModelKind::Map(session);
                    return;
                };
                let layer = session.layer;
                (
                    vial::Write::Key {
                        layer,
                        place,
                        code: hit.code,
                    },
                    None,
                )
            }
            Target::Cell { spot, insert } => {
                let (write, after) = cell_write(&session, spot, insert, hit);
                (write, Some(after))
            }
        };
        self.land(session, write, after);
    }

    /// Stages a typed binding on the key or section cell the list was opened for. The session
    /// comes back when it has nowhere to go.
    fn stage_binding(
        &mut self,
        mut session: Session<Mapping>,
        target: &Target,
        text: String,
    ) -> std::result::Result<(), Session<Mapping>> {
        let code = session.board.intern(&text);
        match (target, code, session.selection) {
            (Target::Key, Some(code), Some(place)) => {
                let layer = session.layer;
                self.land(session, vial::Write::Key { layer, place, code }, None);
                Ok(())
            }
            (
                Target::Cell {
                    spot:
                        spot @ vial::Spot {
                            tab: vial::Tab::Section(kind),
                            ..
                        },
                    insert,
                },
                ..,
            ) => {
                let (write, after) =
                    section_text(&session.board, *kind, *spot, insert.is_some(), text);
                self.land(session, write, Some(after));
                Ok(())
            }
            _ => {
                self.notice = Some("a typed binding goes on a key or a file's tab".to_string());
                Err(session)
            }
        }
    }
}

/// A macro step given a new key: a press or a release stays one, anything else becomes a tap.
/// Staging and the key list's preview both use this, so the preview is what Enter stages.
fn replaced_step(step: &vial::Action, code: u16) -> vial::Action {
    match step {
        vial::Action::Down(_) => vial::Action::Down(code),
        vial::Action::Up(_) => vial::Action::Up(code),
        _ => vial::Action::Tap(code),
    }
}

/// A picked key for a tap dance or combo field, or a replaced or added macro step.
/// Returns the change and where the cursor goes once it is staged.
fn cell_write(
    session: &Session<Mapping>,
    spot: vial::Spot,
    insert: Option<Press>,
    hit: Hit,
) -> (vial::Write, vial::Spot) {
    let board = &session.board;
    let code = hit.code;
    let (write, spot) = match spot.tab {
        vial::Tab::TapDance => {
            let mut entry = board.tap_dances()[spot.row];
            entry.keys[spot.col.min(3)] = code;
            let index = spot.row as u8;
            (vial::Write::TapDance { index, entry }, spot)
        }
        vial::Tab::Combos => {
            let mut entry = board.combos()[spot.row];
            entry.keys[spot.col.min(4)] = code;
            let index = spot.row as u8;
            (vial::Write::Combo { index, entry }, spot)
        }
        vial::Tab::Macros => {
            let mut macros = board.macros().to_vec();
            let steps = &mut macros[spot.row];
            let at = spot.col.min(steps.len());
            let col = match insert {
                // The cursor moves past the new step, so the next insert follows it.
                Some(press) => {
                    steps.insert(at, press.action(code));
                    at + 1
                }
                None => {
                    if let Some(step) = steps.get_mut(at) {
                        *step = replaced_step(step, code);
                    }
                    at
                }
            };
            (vial::Write::Macros(macros), vial::Spot { col, ..spot })
        }
        vial::Tab::Section(kind) => match board.binding_text(code) {
            Some(text) => section_text(board, kind, spot, insert.is_some(), text),
            None => (
                vial::Write::Section(kind, board.section(kind).to_vec()),
                spot,
            ),
        },
    };
    (write, spot)
}

/// A binding for a section cell: a binding property, a tail binding replaced, or one added at
/// the cursor. The cursor moves past an added one, so the next follows it.
fn section_text(
    board: &vial::Board,
    kind: vial::Kind,
    spot: vial::Spot,
    insert: bool,
    text: String,
) -> (vial::Write, vial::Spot) {
    let mut rows = board.section(kind).to_vec();
    let mut col = spot.col;
    if let Some(row) = rows.get_mut(spot.row) {
        match vial::section_slot(kind, row, spot.col) {
            vial::Slot::Column(index) => row.cells[index] = text,
            vial::Slot::Tail(index) if !insert => row.tail[index] = text,
            vial::Slot::Tail(index) => {
                row.tail.insert(index, text);
                col += 1;
            }
            vial::Slot::Append => {
                row.tail.push(text);
                col += 1;
            }
            vial::Slot::Name => {}
        }
    }
    (vial::Write::Section(kind, rows), vial::Spot { col, ..spot })
}

/// One section row changed by `edit`, staged as the whole section.
fn section_edit(
    board: &vial::Board,
    kind: vial::Kind,
    row: usize,
    edit: impl FnOnce(&mut vial::Record),
) -> vial::Write {
    let mut rows = board.section(kind).to_vec();
    if let Some(record) = rows.get_mut(row) {
        edit(record);
    }
    vial::Write::Section(kind, rows)
}

/// Enter in a field: the change, and where the cursor goes. `None` when there is nothing
/// to stage, as with an empty new text step. An empty existing text step is removed.
fn field_write(
    session: &Session<Mapping>,
    spot: vial::Spot,
    field: Field,
    text: &str,
) -> Option<(vial::Write, vial::Spot)> {
    let number = text.parse::<u16>().unwrap_or(0);
    let board = &session.board;
    match field {
        Field::Name | Field::Prop { .. } => {
            let vial::Tab::Section(kind) = spot.tab else {
                return None;
            };
            if matches!(field, Field::Name) && text.is_empty() {
                return None;
            }
            let write = section_edit(board, kind, spot.row, |row| match field {
                Field::Prop { col } => row.cells[col] = text.trim().to_string(),
                _ => row.name = text.to_string(),
            });
            Some((write, spot))
        }
        Field::Term => {
            let mut entry = board.tap_dances()[spot.row];
            entry.term = number;
            let index = spot.row as u8;
            Some((vial::Write::TapDance { index, entry }, spot))
        }
        Field::Delay { insert } | Field::Text { insert } => {
            let step = match field {
                Field::Delay { .. } => Some(vial::Action::Delay(number.min(vial::MAX_DELAY))),
                _ if text.is_empty() => None,
                _ => Some(vial::Action::Text(text.to_string())),
            };
            let mut macros = board.macros().to_vec();
            let steps = &mut macros[spot.row];
            let at = spot.col.min(steps.len());
            let col = match (step, insert) {
                (None, true) => return None,
                (None, false) => {
                    if at < steps.len() {
                        steps.remove(at);
                    }
                    at
                }
                (Some(step), true) => {
                    steps.insert(at, step);
                    at + 1
                }
                (Some(step), false) => {
                    if let Some(slot) = steps.get_mut(at) {
                        *slot = step;
                    }
                    at
                }
            };
            Some((vial::Write::Macros(macros), vial::Spot { col, ..spot }))
        }
    }
}

fn sheet_rows(board: &vial::Board, tab: vial::Tab) -> usize {
    match tab {
        vial::Tab::TapDance => board.tap_dances().len(),
        vial::Tab::Combos => board.combos().len(),
        vial::Tab::Macros => board.macros().len(),
        vial::Tab::Section(kind) => board.section(kind).len(),
    }
}

/// Five fields for tap dances and combos. A macro row is its steps plus the append slot; a
/// section row its name, its properties, then its bindings and their append slot.
fn sheet_cols(board: &vial::Board, spot: vial::Spot) -> usize {
    match spot.tab {
        vial::Tab::TapDance | vial::Tab::Combos => 5,
        vial::Tab::Macros => board.macros().get(spot.row).map_or(0, Vec::len) + 1,
        vial::Tab::Section(kind) => board
            .section(kind)
            .get(spot.row)
            .map_or(1, |row| vial::section_width(kind, row)),
    }
}

fn clamp_col(board: &vial::Board, spot: &mut vial::Spot) {
    spot.row = spot.row.min(sheet_rows(board, spot.tab).saturating_sub(1));
    spot.col = spot.col.min(sheet_cols(board, *spot).saturating_sub(1));
}

/// The chooser's card text for a board that would not open. A sleeping Bluetooth board just
/// stops answering, so that case says how to retry.
fn why_not(err: &Error) -> String {
    match err {
        Error::Disconnected => "no answer: wake it, then enter".to_string(),
        Error::Locked => LOCKED_CARD.to_string(),
        other => other.to_string(),
    }
}

/// A Studio board's card while it waits for its unlock key. `tick` asks again on each tick.
const LOCKED_CARD: &str = "🔒 press the keyboard's Studio unlock key: it opens by itself";

/// The hold prompt while a save waits for a Studio board's unlock key.
const LOCKED_SAVE: &str = "locked: press the keyboard's Studio unlock key to finish writing";

fn vacant() -> ModelKind {
    ModelKind::Pick {
        candidates: Vec::new(),
        cursor: 0,
        failed: None,
    }
}

fn view_of(kind: &ModelKind) -> keys::View {
    match kind {
        ModelKind::Pick { .. } => keys::View::Pick,
        // Esc is the only key while a board reads.
        ModelKind::Open { .. }
        | ModelKind::Lost {
            reconnect: Some(_), ..
        } => keys::View::Hold,
        ModelKind::Lost { .. } => keys::View::Lost,
        ModelKind::Map(_) => keys::View::Map,
        ModelKind::Sheet { session, .. } if session.board.reading().is_some() => {
            keys::View::Reading
        }
        ModelKind::Sheet { .. } => keys::View::Sheet,
        ModelKind::Search { .. } => keys::View::Search,
        ModelKind::Edit { .. } => keys::View::Edit,
        ModelKind::Hold { .. } | ModelKind::Release { .. } => keys::View::Hold,
        ModelKind::Ask { .. } => keys::View::Ask,
        ModelKind::Palette { .. } => keys::View::Palette,
        ModelKind::Help { .. } => keys::View::Help,
    }
}

fn help_lines(under: &ModelKind) -> Vec<vial::PopupLine> {
    keys::cheatsheet(view_of(under))
}

/// What a Ctrl-K row does.
#[derive(Clone, Copy)]
enum Goal {
    /// A `:` command, run as if typed.
    Run(Cmd),
    Layer(u8),
    Jump(vial::Jump),
}

/// A `:` command.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Write,
    WriteQuit,
    Undo,
    Discard,
    Help,
    Quit,
    ForceQuit,
    LayerAdd,
    LayerRemove,
    LayerRename,
    LayerLeft,
    LayerRight,
    Layout,
    RowAdd,
    RowRemove,
}

/// Every `:` command: its names (Tab fills in the first), and what it does.
/// The `:` rows of the find-anything list and its `run` group both read this.
const COMMANDS: [(Cmd, &[&str], &str); 15] = [
    (
        Cmd::Write,
        &["w", "write"],
        "write every change to the keyboard",
    ),
    (Cmd::WriteQuit, &["wq", "x"], "write, then quit"),
    (Cmd::Undo, &["u", "undo"], "undo the last change"),
    (
        Cmd::Discard,
        &["e!", "discard"],
        "discard every unsaved change",
    ),
    (Cmd::Help, &["help", "h"], "show every key"),
    (
        Cmd::Quit,
        &["q", "quit"],
        "quit, asking first when changes are unsaved",
    ),
    (Cmd::ForceQuit, &["q!"], "quit without writing"),
    (Cmd::LayerAdd, &["layer-add"], "add a layer after the last"),
    (Cmd::LayerRemove, &["layer-remove"], "remove this layer"),
    (
        Cmd::LayerRename,
        &["layer-rename"],
        "name this layer: :layer-rename Sym",
    ),
    (
        Cmd::LayerLeft,
        &["layer-left"],
        "move this layer one place left",
    ),
    (
        Cmd::LayerRight,
        &["layer-right"],
        "move this layer one place right",
    ),
    (
        Cmd::Layout,
        &["layout"],
        "list physical layouts, or switch: :layout 1",
    ),
    (
        Cmd::RowAdd,
        &["add"],
        "on a keymap file's tab: add a combo, macro, or behavior: :add name",
    ),
    (
        Cmd::RowRemove,
        &["remove"],
        "on a keymap file's tab: remove this row",
    ),
];

/// The name Tab fills in after `:`.
fn command_name(command: Cmd) -> &'static str {
    COMMANDS
        .iter()
        .find(|(at, ..)| *at == command)
        .map_or("", |(_, names, _)| names[0])
}

/// What follows the command's name on the `:` line, trimmed.
fn command_arg(text: &str) -> &str {
    text.strip_prefix(':')
        .and_then(|line| line.trim_start().split_once(char::is_whitespace))
        .map_or("", |(_, rest)| rest.trim())
}

fn layout_list(names: &[String], at: usize) -> String {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            if index == at {
                format!("{index} {name}*")
            } else {
                format!("{index} {name}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// An index, or the one name that starts with `arg`.
fn layout_pick(names: &[String], arg: &str) -> std::result::Result<usize, String> {
    if let Ok(index) = arg.parse::<usize>() {
        return if index < names.len() {
            Ok(index)
        } else {
            Err(format!("layouts are 0..{}", names.len().saturating_sub(1)))
        };
    }
    let needle = arg.to_lowercase();
    let hits: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            let name = name.to_lowercase();
            name == needle || name.starts_with(&needle)
        })
        .map(|(index, _)| index)
        .collect();
    match *hits.as_slice() {
        [index] => Ok(index),
        [] => Err(format!("no layout {arg}")),
        _ => Err(format!("{arg} matches more than one layout")),
    }
}

/// After `:`: each command with a name that starts with `typed`'s first word, every one when
/// nothing is typed. An exact name sorts first, so `:q` runs quit and not `:q!`; then table
/// order, so `:wr` writes. The rest of `typed` is the command's argument.
fn command_rows(typed: &str) -> Vec<(&'static str, String, Goal)> {
    let typed = typed.split_whitespace().next().unwrap_or("");
    let names = |names: &[&str]| -> String {
        names
            .iter()
            .map(|name| format!(":{name}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Every command's width, not just the matches', so the columns stay put while typing.
    let width = COMMANDS
        .iter()
        .map(|(_, list, _)| names(list).chars().count())
        .max()
        .unwrap_or(0);
    let mut hits: Vec<_> = COMMANDS
        .iter()
        .filter(|(_, list, _)| list.iter().any(|name| name.starts_with(typed)))
        .collect();
    hits.sort_by_key(|(_, list, _)| !list.contains(&typed));
    hits.into_iter()
        .map(|(command, list, what)| {
            let label = format!("{:<width$}   {what}", names(list));
            ("commands", label, Goal::Run(*command))
        })
        .collect()
}

/// The row Enter runs: the one ↑↓ chose, else the first once something is typed. With nothing
/// typed, nothing is picked, so Space then Enter cannot run a command by accident.
fn palette_pick(text: &str, len: usize, cursor: Option<usize>) -> Option<usize> {
    let typed = !text.trim_start_matches(':').trim().is_empty();
    cursor.or(typed.then_some(0)).filter(|&at| at < len)
}

/// Rows the popup shows at once. The window follows the cursor.
const PALETTE_ROWS: usize = 12;

/// The popup's rows for `query`, as `(group, label, goal)`. A leading `:` lists commands by name
/// (`command_rows`). Otherwise every word of the query must appear in the row, and keys on the
/// board join once something is typed; there are hundreds.
fn palette(board: &vial::Board, query: &str) -> Vec<(&'static str, String, Goal)> {
    if let Some(typed) = query.strip_prefix(':') {
        return command_rows(typed.trim());
    }
    let mut rows: Vec<(&'static str, String, String, Goal)> = COMMANDS
        .iter()
        .map(|(command, names, what)| {
            (
                "run",
                format!("{what}   :{}", names[0]),
                String::new(),
                Goal::Run(*command),
            )
        })
        .collect();
    rows.extend((0..board.layer_count()).map(|layer| {
        (
            "go to",
            format!("layer {layer}   L{layer}"),
            String::new(),
            Goal::Layer(layer),
        )
    }));
    rows.extend(board.editors().into_iter().map(|tab| {
        let spot = vial::Spot {
            tab,
            row: 0,
            col: 0,
        };
        (
            "go to",
            tab.name().to_string(),
            String::new(),
            Goal::Jump(vial::Jump::Spot(spot)),
        )
    }));
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if !words.is_empty() {
        rows.extend(
            board
                .jumps()
                .into_iter()
                .map(|(jump, label, more)| ("on the board", label, more, Goal::Jump(jump))),
        );
    }
    rows.into_iter()
        .filter(|(_, label, more, _)| {
            let hay = format!("{label} {more}").to_lowercase();
            words.iter().all(|word| hay.contains(word.as_str()))
        })
        .map(|(group, label, _, goal)| (group, label, goal))
        .collect()
}

/// The query line, then a window of hits under group headings. After `:` it is the command line.
fn palette_popup(
    rows: &[(&'static str, String, Goal)],
    text: &str,
    pick: Option<usize>,
) -> vial::Popup {
    let command = text.starts_with(':');
    // Padded so the box keeps one width while the hits change under it.
    let mut lines = vec![
        vial::PopupLine::Text(format!("{:<60}", format!("› {text}▏"))),
        vial::PopupLine::Blank,
    ];
    let start = pick.unwrap_or(0).saturating_sub(PALETTE_ROWS - 1);
    let mut group = "";
    for (index, (in_group, label, _)) in rows.iter().enumerate().skip(start).take(PALETTE_ROWS) {
        if *in_group != group {
            group = in_group;
            lines.push(vial::PopupLine::Heading(group.to_string()));
        }
        lines.push(vial::PopupLine::Choice(label.clone(), pick == Some(index)));
    }
    if rows.is_empty() {
        lines.push(vial::PopupLine::Text(if command {
            format!("no command starts with {}", text.trim())
        } else {
            "no match".to_string()
        }));
    } else if rows.len() > PALETTE_ROWS {
        lines.push(vial::PopupLine::Blank);
        lines.push(vial::PopupLine::Text(format!(
            "{} of {}",
            pick.unwrap_or(0) + 1,
            rows.len()
        )));
    }
    let (title, actions) = if command {
        (
            "command",
            vec![("tab", "complete"), ("enter", "run"), ("esc", "close")],
        )
    } else {
        (
            "find anything",
            vec![("↑↓", "move"), ("enter", "go"), ("esc", "close")],
        )
    };
    vial::Popup {
        title: title.to_string(),
        lines,
        scroll: 0,
        actions,
        warn: false,
        pinned: true,
    }
}

/// The status bar badge for each view. Typing views are `Input`, waiting on the keyboard is `Alert`.
fn mode_of(kind: &ModelKind) -> vial::Mode {
    use vial::ModeTone::{Alert, Browse, Input};
    let (label, tone) = match kind {
        ModelKind::Pick { .. } => ("CHOOSE", Browse),
        ModelKind::Open { .. } => ("OPENING", Browse),
        ModelKind::Lost {
            reconnect: Some(_), ..
        } => ("RECONNECTING", Alert),
        ModelKind::Lost { .. } => ("DISCONNECTED", Alert),
        ModelKind::Map(_) => ("KEYMAP", Browse),
        ModelKind::Sheet { spot, .. } => (
            match spot.tab {
                vial::Tab::TapDance => "TAP DANCE",
                vial::Tab::Combos => "COMBOS",
                vial::Tab::Macros | vial::Tab::Section(vial::Kind::Macros) => "MACROS",
                vial::Tab::Section(vial::Kind::Combos) => "COMBOS",
                vial::Tab::Section(vial::Kind::Behaviors) => "BEHAVIORS",
                vial::Tab::Section(vial::Kind::Conditional) => "CONDITIONAL",
                vial::Tab::Section(vial::Kind::Encoders) => "ENCODERS",
            },
            Browse,
        ),
        ModelKind::Search { .. } => ("SEARCH", Input),
        ModelKind::Edit { .. } => ("EDIT", Input),
        ModelKind::Palette { text, .. } if text.starts_with(':') => ("COMMAND", Input),
        ModelKind::Palette { .. } => ("FIND", Input),
        ModelKind::Ask { .. } => ("CONFIRM", Alert),
        ModelKind::Hold { .. } | ModelKind::Release { .. } => ("UNLOCK", Alert),
        ModelKind::Help { under, .. } => return mode_of(under),
    };
    vial::Mode { label, tone }
}

/// The one painter. `popup` draws over whatever `kind` paints.
fn render(
    kind: &ModelKind,
    notice: Option<&str>,
    popup: Option<&vial::Popup>,
    cols: u16,
    rows: u16,
) -> String {
    let footer = keys::footer(view_of(kind));
    let mode = mode_of(kind);
    let chrome = Chrome {
        notice,
        popup,
        mode,
        footer: &footer,
        cols,
        rows,
    };
    match kind {
        ModelKind::Pick {
            candidates,
            cursor,
            failed,
        } => vial::pick_text(
            candidates,
            *cursor,
            match failed {
                Some((index, why)) => vial::CardNote::Failed(*index, why),
                None => vial::CardNote::None,
            },
            notice,
            popup,
            mode,
            &footer,
            cols,
            rows,
        ),
        ModelKind::Open {
            candidates,
            cursor,
            load,
            ..
        } => vial::pick_text(
            candidates,
            *cursor,
            vial::CardNote::Opening(load.progress()),
            notice,
            popup,
            mode,
            &footer,
            cols,
            rows,
        ),
        ModelKind::Lost {
            session,
            back,
            why,
            cursor,
            reconnect,
        } => {
            let unsaved = session.board.pending().len();
            let mut lines = vec![vial::PopupLine::Text(why.clone()), vial::PopupLine::Blank];
            let actions = match reconnect {
                Some(reconnect) => {
                    let load = &reconnect.1;
                    let progress = load.progress();
                    lines.push(vial::PopupLine::Text(format!(
                        "reading it again… {}%  {}",
                        progress.percent, progress.what
                    )));
                    vec![("esc", "stop")]
                }
                None => {
                    lines.extend(Choice::list(unsaved).iter().enumerate().map(
                        |(index, choice)| {
                            vial::PopupLine::Choice(choice.label(unsaved), index == *cursor)
                        },
                    ));
                    vec![("j k", "move"), ("enter", "choose")]
                }
            };
            let popup = vial::Popup {
                title: "keyboard disconnected".to_string(),
                lines,
                scroll: 0,
                actions,
                warn: true,
                pinned: false,
            };
            let chrome = Chrome {
                popup: Some(&popup),
                ..chrome
            };
            session.paint(*back, None, None, &chrome)
        }
        ModelKind::Map(session) => session.paint(None, None, None, &chrome),
        ModelKind::Sheet { session, spot } => session.paint(Some(*spot), None, None, &chrome),
        ModelKind::Search {
            session,
            browse,
            cursor,
            wrap,
            target,
        } => {
            if let Browse::Part { parts, query } = browse {
                let count = column_count(session, parts);
                let filter = query.get(parts.prefix().len()..).unwrap_or_default();
                let columns: Vec<vial::Column> = (0..count)
                    .map(|i| {
                        let active = i == parts.at;
                        let listed =
                            column_hits(session, parts, i, if active { filter } else { "" });
                        let selected = if active {
                            Some(*cursor)
                        } else {
                            parts
                                .words
                                .get(i)
                                .and_then(|word| hit_of(&listed.hits, word))
                        };
                        vial::Column {
                            title: listed.title,
                            hits: listed.hits,
                            selected,
                        }
                    })
                    .collect();
                let text = parts.words[..count.min(parts.words.len())].join(" ");
                let lower = vial::Lower::Columns {
                    title: &text,
                    query: filter,
                    columns: &columns,
                    active: parts.at,
                };
                return session.paint(target.spot(), None, Some(lower), &chrome);
            }
            let listed = search_list(session, browse, wrap);
            let query = match browse {
                Browse::Find { query, .. } => query.as_str(),
                Browse::Part { query, .. } => query.as_str(),
                Browse::Shelf(_) => "",
            };
            // What Enter would stage, by the same rules as `confirm_search` and `cell_write`:
            // after a `(kc)` pick only a basic key fills it, and a macro step keeps its kind.
            // A binding being typed has no keycode until it is staged.
            let preview = listed
                .hits
                .get(*cursor)
                .filter(|_| !query.starts_with('&'))
                .and_then(|hit| match wrap {
                    Some(outer) => (hit.code <= 0xFF).then_some(outer.code | hit.code),
                    None => Some(hit.code),
                })
                .and_then(|code| match target {
                    Target::Cell {
                        spot,
                        insert: Some(press),
                    } if spot.tab == vial::Tab::Macros => Some(vial::Preview::Step {
                        action: press.action(code),
                        insert: true,
                    }),
                    Target::Cell { spot, insert: None } if spot.tab == vial::Tab::Macros => {
                        let step = session.board.macros().get(spot.row)?.get(spot.col)?;
                        Some(vial::Preview::Step {
                            action: replaced_step(step, code),
                            insert: false,
                        })
                    }
                    _ => Some(vial::Preview::Code(code)),
                });
            let lower = vial::Lower::Search {
                title: &listed.title,
                query,
                hits: &listed.hits,
                cursor: *cursor,
                preview: preview.as_ref(),
                keyboard: matches!(browse, Browse::Shelf(ShelfIndex(0))),
            };
            session.paint(target.spot(), None, Some(lower), &chrome)
        }
        ModelKind::Edit {
            session,
            spot,
            field,
            text,
        } => {
            let lower = vial::Lower::Edit {
                title: field.title(),
                text,
            };
            session.paint(Some(*spot), None, Some(lower), &chrome)
        }
        ModelKind::Hold { session, back, .. } => {
            session.paint(*back, Some(session.phase.prompt.as_str()), None, &chrome)
        }
        ModelKind::Release { session, back, .. } => {
            session.paint(*back, Some("unlocked, let go of the keys"), None, &chrome)
        }
        ModelKind::Ask {
            session,
            back,
            question,
        } => {
            let popup = question.popup();
            let chrome = Chrome {
                popup: Some(&popup),
                ..chrome
            };
            session.paint(*back, None, None, &chrome)
        }
        ModelKind::Palette {
            session,
            back,
            text,
            cursor,
        } => {
            let rows = palette(&session.board, text);
            let pick = palette_pick(text, rows.len(), *cursor);
            let popup = palette_popup(&rows, text, pick);
            let chrome = Chrome {
                popup: Some(&popup),
                ..chrome
            };
            session.paint(*back, None, None, &chrome)
        }
        ModelKind::Help { under, scroll } => {
            let popup = vial::Popup {
                title: "help".to_string(),
                lines: help_lines(under),
                scroll: *scroll,
                actions: vec![("j k", "scroll"), ("esc", "close")],
                warn: false,
                pinned: false,
            };
            render(under, notice, Some(&popup), cols, rows)
        }
    }
}

/// What every board paint shares: the notice, the popup, the bars, and the size.
struct Chrome<'a> {
    notice: Option<&'a str>,
    popup: Option<&'a vial::Popup>,
    mode: vial::Mode,
    footer: &'a [(String, &'static str)],
    cols: u16,
    rows: u16,
}

fn shift(cursor: &mut usize, len: usize, delta: i8) {
    if len == 0 {
        *cursor = 0;
        return;
    }
    let step = 8_i32 * i32::from(delta.signum());
    let next = *cursor as i32 + step;
    let max = len.saturating_sub(1) as i32;
    *cursor = next.clamp(0, max) as usize;
}

/// One step in the key grid, `across` hits a row: Left and Right to the neighbour, Up and Down
/// a row, staying put at an edge. Down onto a short last row lands on its last hit.
fn grid_step(cursor: &mut usize, len: usize, across: usize, dir: vial::Dir) {
    let Some(last) = len.checked_sub(1) else {
        *cursor = 0;
        return;
    };
    *cursor = match dir {
        vial::Dir::Left => cursor.saturating_sub(1),
        vial::Dir::Right => (*cursor + 1).min(last),
        vial::Dir::Up if *cursor >= across => *cursor - across,
        vial::Dir::Down if *cursor / across < last / across => (*cursor + across).min(last),
        vial::Dir::Up | vial::Dir::Down => *cursor,
    };
}

fn move_cursor(cursor: &mut usize, len: usize, dir: vial::Dir) {
    if len == 0 {
        *cursor = 0;
        return;
    }
    match dir {
        vial::Dir::Left | vial::Dir::Up => *cursor = cursor.saturating_sub(1),
        vial::Dir::Right | vial::Dir::Down => {
            if *cursor + 1 < len {
                *cursor += 1;
            }
        }
    }
}

impl<Phase> Session<Phase> {
    fn paint(
        &self,
        spot: Option<vial::Spot>,
        hold: Option<&str>,
        lower: Option<vial::Lower<'_>>,
        chrome: &Chrome<'_>,
    ) -> String {
        self.board.text(&vial::Paint {
            layer: self.layer,
            selection: self.selection,
            spot,
            notice: chrome.notice,
            hold,
            lower,
            popup: chrome.popup,
            mode: chrome.mode,
            footer: chrome.footer,
            cols: chrome.cols,
            rows: chrome.rows,
        })
    }
}

impl Session<Mapping> {
    fn mapped(board: vial::Board) -> Self {
        let selection = board.first_selectable();
        Self {
            board,
            layer: 0,
            selection,
            phase: Mapping,
        }
    }

    /// Unlock first when the board asks for it, then commit.
    fn write(mut self, write: vial::Write) -> Assign {
        let hold = match self.board.begin_hold(&write) {
            Ok(hold) => hold,
            Err(err) => return Assign::Disconnected(err, self),
        };
        if let Some(hold) = hold {
            return Assign::Holding(self.holding(write, hold.prompt));
        }
        match self.board.commit(&write) {
            Ok(vial::Commit::Stored | vial::Commit::Same) => Assign::Mapped(self),
            Ok(vial::Commit::Holding(hold)) => Assign::Holding(self.holding(write, hold.prompt)),
            // A Studio board that locked itself while idle: wait for its unlock key, then go on.
            Err(Error::Locked) => Assign::Holding(self.holding(write, LOCKED_SAVE.to_string())),
            Err(err) => Assign::Disconnected(err, self),
        }
    }

    fn holding(self, write: vial::Write, prompt: String) -> Session<Holding> {
        Session {
            board: self.board,
            layer: self.layer,
            selection: self.selection,
            phase: Holding { write, prompt },
        }
    }

    fn list(&self, browse: &Browse) -> vial::Listed {
        self.board.list(browse.range())
    }

    fn nudge(&mut self, dir: vial::Dir) {
        self.selection = self.board.nudge(self.layer, self.selection, dir);
    }

    fn layer_step(&mut self, delta: i8) {
        let count = self.board.layer_count() as i16;
        if count <= 0 {
            return;
        }
        let next = (self.layer as i16 + delta as i16).rem_euclid(count);
        self.layer = next as u8;
    }
}

impl Session<Holding> {
    fn poll(mut self) -> HoldStep {
        match self.board.poll_unlock() {
            Err(err) => HoldStep::Disconnected(err, self.cancel()),
            Ok(true) => HoldStep::Waiting(self),
            Ok(false) => match self.board.commit(&self.phase.write) {
                // The lock state can lag the key; ask again on the next tick.
                Err(Error::Locked) => HoldStep::Waiting(self),
                Err(err) => HoldStep::Disconnected(err, self.cancel()),
                Ok(vial::Commit::Holding(hold)) => {
                    if !hold.prompt.is_empty() {
                        self.phase.prompt = hold.prompt;
                    }
                    HoldStep::Waiting(self)
                }
                Ok(vial::Commit::Stored | vial::Commit::Same) => HoldStep::Mapped(self.cancel()),
            },
        }
    }

    fn cancel(self) -> Session<Mapping> {
        Session {
            board: self.board,
            layer: self.layer,
            selection: self.selection,
            phase: Mapping,
        }
    }
}

enum Assign {
    Mapped(Session<Mapping>),
    Holding(Session<Holding>),
    /// The keyboard stopped answering. The draft is still on the session.
    Disconnected(Error, Session<Mapping>),
}

enum HoldStep {
    Waiting(Session<Holding>),
    Mapped(Session<Mapping>),
    Disconnected(Error, Session<Mapping>),
}

/// One answer to a keyboard that stopped answering.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Read the keyboard again and stage the unsaved changes on it.
    Keep,
    /// Read the keyboard again and drop the unsaved changes.
    Revert,
    /// Back to the keyboard list.
    Restart,
    Quit,
}

impl Choice {
    /// Revert only means something when there is something to revert.
    fn list(unsaved: usize) -> &'static [Choice] {
        if unsaved > 0 {
            &[Choice::Keep, Choice::Revert, Choice::Restart, Choice::Quit]
        } else {
            &[Choice::Keep, Choice::Restart, Choice::Quit]
        }
    }

    fn label(self, unsaved: usize) -> String {
        let changes = format!(
            "{unsaved} unsaved change{}",
            if unsaved == 1 { "" } else { "s" }
        );
        match self {
            Choice::Keep if unsaved > 0 => format!("reconnect and keep the {changes}"),
            Choice::Keep => "reconnect".to_string(),
            Choice::Revert => format!("reconnect and revert the {changes}"),
            Choice::Restart if unsaved > 0 => {
                format!("start again from the keyboard list, dropping the {changes}")
            }
            Choice::Restart => "start again from the keyboard list".to_string(),
            Choice::Quit if unsaved > 0 => format!("quit without the {changes}"),
            Choice::Quit => "quit".to_string(),
        }
    }
}

pub mod vial;
