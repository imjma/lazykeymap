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

/// The product loop. `app` can call `start`, `handle`, and `text`.
pub(crate) struct Model {
    kind: ModelKind,
    /// One line for the last thing that happened: a save, a refusal, an unknown command.
    /// Cleared by the next key.
    notice: Option<String>,
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
    Delay { insert: bool },
    Text { insert: bool },
}

impl Field {
    fn title(self) -> &'static str {
        match self {
            Field::Term => "tapping term (ms)",
            Field::Delay { .. } => "delay (ms)",
            Field::Text { .. } => "text",
        }
    }

    /// Digits for numbers, printable ASCII for text. The firmware types text byte by byte.
    fn accepts(self, text: &str, ch: char) -> bool {
        let max = match self {
            Field::Text { .. } => return (' '..='~').contains(&ch),
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
    Find { query: Query, back: ShelfIndex },
}

impl Browse {
    fn type_char(self, ch: char) -> Self {
        let (mut text, back) = match self {
            Browse::Shelf(index) => (String::new(), index),
            Browse::Find { query, back } => (query.0, back),
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

    fn shift_shelf(self, delta: i8, len: usize) -> Self {
        let ShelfIndex(index) = match self {
            Browse::Shelf(index) | Browse::Find { back: index, .. } => index,
        };
        let next = (index as isize + isize::from(delta)).rem_euclid(len as isize) as usize;
        Browse::Shelf(ShelfIndex(next))
    }

    fn range(&self) -> vial::Range<'_> {
        match self {
            Browse::Shelf(ShelfIndex(index)) => vial::Range::Shelf(*index),
            Browse::Find { query, .. } => vial::Range::Find(query.as_str()),
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

/// HID enumerate, then the choose screen. Usage page `0xFF60`, usage `0x61`.
/// One board still waits for Enter. Zero boards is `NoBoard`.
pub fn attach() -> Result<Attached> {
    list_first(vial::enumerate()?)
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
            Kind::Pick(candidates) => ModelKind::Pick {
                candidates,
                cursor: 0,
                failed: None,
            },
        };
        Self { kind, notice: None }
    }

    /// `cols` is the screen width: the key grid moves Up and Down a whole row of it.
    pub(crate) fn handle(&mut self, event: Event, cols: u16) -> Result<Step> {
        if !matches!(event, Event::Tick) {
            self.notice = None;
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
        match keys::act(view_of(&self.kind), &event) {
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

    pub(crate) fn text(&self, cols: u16, rows: u16) -> String {
        render(&self.kind, self.notice.as_deref(), None, cols, rows)
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
    ) -> Result<Step> {
        let mut session = session;
        self.kind = match goal {
            Goal::Run(command) => return self.run_command(session, back, command),
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

    fn run_command(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        command: Cmd,
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
        }
        self.kind = back_to(session, back);
        Ok(Step::Continue)
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
        self.notice = Some(match count {
            0 => "nothing to write".to_string(),
            1 => "wrote 1 change to the keyboard".to_string(),
            _ => format!("wrote {count} changes to the keyboard"),
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
                let hits = session.list(browse).hits;
                grid_step(cursor, hits.len(), vial::search_across(&hits, cols), dir);
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
        let next = (at as isize + isize::from(delta)).rem_euclid(count as isize) as usize;
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
            ModelKind::Search { browse, cursor, .. } => {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.type_char(ch);
                *cursor = 0;
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
                // Four rows of the grid at a time.
                let hits = session.list(browse).hits;
                let step = vial::search_across(&hits, cols) * 4;
                let last = hits.len().saturating_sub(1);
                *cursor = if delta < 0 {
                    cursor.saturating_sub(step)
                } else {
                    (*cursor + step).min(last)
                };
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
            } => *cursor = pick(session.list(browse).hits.len()),
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
            ModelKind::Search { browse, cursor, .. } => {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.backspace();
                *cursor = 0;
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
            let count = session.board.shelf_count();
            if count > 0 {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.shift_shelf(delta, count);
            }
            *cursor = 0;
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
            ModelKind::Sheet { session, spot } => self.kind = open_cell(session, spot),
            ModelKind::Palette {
                session,
                back,
                text,
                cursor,
            } => {
                let rows = palette(&session.board, &text);
                match palette_pick(&text, rows.len(), cursor) {
                    Some(at) => return self.go(session, back, rows[at].2),
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
                    self.kind = ModelKind::Map(Session::mapped(board));
                    return Ok(if more { Step::Working } else { Step::Continue });
                }
                Ok(_) if Instant::now() < until => {}
                Ok(_) => {
                    self.kind = ModelKind::Open {
                        candidates,
                        cursor,
                        load,
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
        let len = if spot.tab == vial::Tab::Macros {
            session.board.macros()[spot.row].len()
        } else {
            0
        };
        let to = spot.col as isize + isize::from(delta);
        if spot.col >= len || to < 0 || to as usize >= len {
            self.kind = ModelKind::Sheet { session, spot };
            return Ok(Step::Continue);
        }
        let mut macros = session.board.macros().to_vec();
        macros[spot.row].swap(spot.col, to as usize);
        let spot = vial::Spot {
            col: to as usize,
            ..spot
        };
        self.land(session, vial::Write::Macros(macros), Some(spot));
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
                Ok(_) => why = "no Vial keyboard answers yet: wake it or plug it in".to_string(),
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

fn searching(session: Session<Mapping>, target: Target) -> ModelKind {
    ModelKind::Search {
        session,
        browse: Browse::Shelf(ShelfIndex(0)),
        cursor: 0,
        wrap: None,
        target,
    }
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
    };
    (write, spot)
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
    }
}

/// Five fields for tap dances and combos. A macro row is its steps plus the append slot.
fn sheet_cols(board: &vial::Board, spot: vial::Spot) -> usize {
    match spot.tab {
        vial::Tab::TapDance | vial::Tab::Combos => 5,
        vial::Tab::Macros => board.macros().get(spot.row).map_or(0, Vec::len) + 1,
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
        other => other.to_string(),
    }
}

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
}

/// Every `:` command: its names (Tab fills in the first), and what it does.
/// The `:` rows of the find-anything list and its `run` group both read this.
const COMMANDS: [(Cmd, &[&str], &str); 7] = [
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
];

/// The name Tab fills in after `:`.
fn command_name(command: Cmd) -> &'static str {
    COMMANDS
        .iter()
        .find(|(at, ..)| *at == command)
        .map_or("", |(_, names, _)| names[0])
}

/// After `:`: each command with a name that starts with `typed`, every one when nothing is typed.
/// An exact name sorts first, so `:q` runs quit and not `:q!`; then table order, so `:wr` writes.
fn command_rows(typed: &str) -> Vec<(&'static str, String, Goal)> {
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
                vial::Tab::Macros => "MACROS",
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
            let listed = search_list(session, browse, wrap);
            let query = match browse {
                Browse::Find { query, .. } => query.as_str(),
                Browse::Shelf(_) => "",
            };
            // What Enter would stage, by the same rules as `confirm_search` and `cell_write`:
            // after a `(kc)` pick only a basic key fills it, and a macro step keeps its kind.
            let preview = listed
                .hits
                .get(*cursor)
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
