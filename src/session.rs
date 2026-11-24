use crate::{Error, Result};
use std::mem;

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
pub(crate) struct Model {
    kind: ModelKind,
    /// One line for the last thing that happened: a save, a refusal, an unknown command.
    /// Cleared by the next key.
    notice: Option<String>,
}

enum ModelKind {
    Pick {
        names: Vec<String>,
        candidates: Vec<vial::Candidate>,
        cursor: usize,
        failed: Option<usize>,
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
    /// `:` or `;`, then a command and Enter.
    Command {
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        text: String,
    },
    Help {
        under: Box<ModelKind>,
        scroll: usize,
    },
}

enum Question {
    /// A picked keycode. Yes stages it; the keyboard gets it on `:w`.
    Change {
        write: vial::Write,
        /// Where the sheet cursor lands after a yes.
        after: Option<vial::Spot>,
        lines: Vec<vial::PopupLine>,
    },
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
            Question::Change { lines, .. } => (
                "stage this change?".to_string(),
                None,
                lines,
                vec![("enter", "stage"), ("esc", "cancel")],
                false,
            ),
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
    /// 0-based. `text` prints `layer + 1`.
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
            Kind::Pick(candidates) => {
                let names = candidates
                    .iter()
                    .map(|candidate| candidate.name.clone())
                    .collect();
                ModelKind::Pick {
                    names,
                    candidates,
                    cursor: 0,
                    failed: None,
                }
            }
        };
        Self { kind, notice: None }
    }

    pub(crate) fn handle(&mut self, event: Event) -> Result<Step> {
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
            keys::Act::Nudge(dir) => self.arrow(dir),
            keys::Act::Shelf(delta) => self.shelf(delta),
            keys::Act::Page(delta) => self.page(delta),
            keys::Act::Layer(delta) => self.layer(delta),
            keys::Act::Tab(delta) => self.tab(delta),
            keys::Act::End(end) => self.end(end),
            keys::Act::Type(ch) => self.char(ch),
            keys::Act::Backspace => self.backspace(),
            keys::Act::Clear => self.clear(),
            keys::Act::Insert(step) => self.insert(step),
            keys::Act::Reorder(delta) => self.reorder(delta),
            keys::Act::Undo => self.undo(),
            keys::Act::Command => self.open_command(),
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
        if matches!(
            self.kind,
            ModelKind::Ask {
                question: Question::Quit { .. },
                ..
            }
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

    fn open_command(&mut self) -> Result<Step> {
        self.kind = match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Map(session) => ModelKind::Command {
                session,
                back: None,
                text: String::new(),
            },
            ModelKind::Sheet { session, spot } => ModelKind::Command {
                session,
                back: Some(spot),
                text: String::new(),
            },
            other => other,
        };
        Ok(Step::Continue)
    }

    /// `:w` write, `:wq` or `:x` write and quit, `:q` quit, `:q!` quit without writing,
    /// `:e!` discard every change, `:u` undo.
    fn run_command(
        &mut self,
        session: Session<Mapping>,
        back: Option<vial::Spot>,
        text: &str,
    ) -> Result<Step> {
        let mut session = session;
        match text.trim() {
            "w" | "write" => return self.save(session, back, false),
            "wq" | "x" => return self.save(session, back, true),
            "q!" => return Ok(Step::Quit),
            "q" | "quit" => {
                self.kind = back_to(session, back);
                return self.quit();
            }
            "e!" | "discard" => {
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
            "u" | "undo" => {
                if !session.board.undo() {
                    self.notice = Some("nothing to undo".to_string());
                }
            }
            "" => {}
            other => {
                self.notice = Some(format!(
                    "not a command: :{other}   (:w write · :wq write and quit · :q quit · :q! quit without writing · :e! discard)"
                ));
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
                Assign::Disconnected(err) => return Err(err),
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

    fn arrow(&mut self, dir: vial::Dir) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Pick { names, cursor, .. } => move_cursor(cursor, names.len(), dir),
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
                let len = session.list(browse).hits.len();
                move_cursor(cursor, len, dir);
            }
            ModelKind::Edit { .. }
            | ModelKind::Hold { .. }
            | ModelKind::Release { .. }
            | ModelKind::Ask { .. }
            | ModelKind::Command { .. }
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
            ModelKind::Command { text, .. } => {
                if !ch.is_control() {
                    text.push(ch);
                }
            }
            _ => {}
        }
        Ok(Step::Continue)
    }

    fn page(&mut self, delta: i8) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Pick { names, cursor, .. } => shift(cursor, names.len(), delta),
            ModelKind::Search {
                session,
                browse,
                cursor,
                ..
            } => {
                let len = session.list(browse).hits.len();
                shift(cursor, len, delta);
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
            | ModelKind::Command { .. } => {}
        }
        Ok(Step::Continue)
    }

    fn end(&mut self, which: keys::End) -> Result<Step> {
        let pick = |len: usize| match which {
            keys::End::First => 0,
            keys::End::Last => len.saturating_sub(1),
        };
        match &mut self.kind {
            ModelKind::Pick { names, cursor, .. } => *cursor = pick(names.len()),
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
            | ModelKind::Command { .. } => {}
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
            ModelKind::Search { browse, cursor, .. } => {
                let old = mem::replace(browse, Browse::Shelf(ShelfIndex(0)));
                *browse = old.backspace();
                *cursor = 0;
            }
            ModelKind::Edit { text, .. } => {
                text.pop();
            }
            // Backspace on an empty command line closes it, as in vim.
            ModelKind::Command { text, .. } if !text.is_empty() => {
                text.pop();
            }
            ModelKind::Command { .. } => return self.cancel(),
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
                names,
                candidates,
                cursor,
                failed,
            } => {
                self.kind = ModelKind::Pick {
                    names,
                    candidates,
                    cursor,
                    failed,
                };
                return self.confirm_pick();
            }
            ModelKind::Map(session) => self.kind = searching(session, Target::Key),
            ModelKind::Sheet { session, spot } => self.kind = open_cell(session, spot),
            ModelKind::Search {
                session,
                browse,
                cursor,
                wrap,
                target,
            } => self.kind = confirm_search(session, browse, cursor, wrap, target),
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
            ModelKind::Command {
                session,
                back,
                text,
            } => return self.run_command(session, back, &text),
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    fn confirm_pick(&mut self) -> Result<Step> {
        let ModelKind::Pick {
            names,
            mut candidates,
            cursor,
            failed,
        } = mem::replace(&mut self.kind, vacant())
        else {
            return Ok(Step::Continue);
        };
        if cursor >= candidates.len() {
            self.kind = ModelKind::Pick {
                names,
                candidates,
                cursor,
                failed,
            };
            return Ok(Step::Continue);
        }
        let candidate = candidates.remove(cursor);
        match vial::load_keep(candidate) {
            Ok(board) => {
                self.kind = ModelKind::Map(Session::mapped(board));
                Ok(Step::Continue)
            }
            Err((Error::NoLayout, candidate)) => {
                candidates.insert(cursor, candidate);
                self.kind = ModelKind::Pick {
                    names,
                    candidates,
                    cursor,
                    failed: Some(cursor),
                };
                Ok(Step::Continue)
            }
            Err((err, candidate)) => {
                candidates.insert(cursor, candidate);
                self.kind = ModelKind::Pick {
                    names,
                    candidates,
                    cursor,
                    failed,
                };
                Err(err)
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
            ModelKind::Command { session, back, .. } => self.kind = back_to(session, back),
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
            (Question::Change { write, after, .. }, 'y' | 'Y') => {
                self.land(session, write, after.or(back));
            }
            (Question::Quit { .. }, 'w' | 'W' | 'y' | 'Y') => {
                return self.save(session, back, true)
            }
            (Question::Quit { .. }, 'd' | 'D') => return Ok(Step::Quit),
            (Question::Discard { count, .. }, 'y' | 'Y') => {
                session.board.discard();
                self.notice = Some(format!("discarded {count} staged change(s)"));
                self.kind = back_to(session, back);
            }
            (
                Question::Change { .. } | Question::Quit { .. } | Question::Discard { .. },
                'n' | 'N',
            ) => {
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
            HoldStep::Disconnected(err) => return Err(err),
        }
        Ok(Step::Continue)
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
        | ModelKind::Command { session, back, .. } => Some((session, back)),
        ModelKind::Help { under, .. } => into_session(*under),
        ModelKind::Pick { .. } => None,
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

fn confirm_search(
    session: Session<Mapping>,
    browse: Browse,
    cursor: usize,
    wrap: Option<Wrap>,
    target: Target,
) -> ModelKind {
    let found = session.list(&browse).hits.into_iter().nth(cursor);
    let code = match (found, &wrap) {
        // Vial fills a `(kc)` code with a basic key only.
        (Some(found), Some(outer)) if found.code <= 0xFF => outer.code | found.code,
        (Some(found), None) if found.masked => {
            return ModelKind::Search {
                session,
                browse: Browse::Shelf(ShelfIndex(0)),
                cursor: 0,
                wrap: Some(Wrap {
                    code: found.code,
                    legend: found.legend,
                }),
                target,
            };
        }
        (Some(found), None) => found.code,
        _ => {
            return ModelKind::Search {
                session,
                browse,
                cursor,
                wrap,
                target,
            }
        }
    };
    let hit = Hit { code };
    let (write, back, after) = match target {
        Target::Key => {
            let Some(place) = session.selection else {
                return ModelKind::Map(session);
            };
            let layer = session.layer;
            (
                vial::Write::Key {
                    layer,
                    place,
                    code: hit.code,
                },
                None,
                None,
            )
        }
        Target::Cell { spot, insert } => {
            let (write, after) = cell_write(&session, spot, insert, hit);
            (write, Some(spot), Some(after))
        }
    };
    let lines = session.board.describe(&write, false);
    ModelKind::Ask {
        session,
        back,
        question: Question::Change {
            write,
            after,
            lines,
        },
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
                        *step = match step {
                            vial::Action::Down(_) => vial::Action::Down(code),
                            vial::Action::Up(_) => vial::Action::Up(code),
                            _ => vial::Action::Tap(code),
                        };
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

fn vacant() -> ModelKind {
    ModelKind::Pick {
        names: Vec::new(),
        candidates: Vec::new(),
        cursor: 0,
        failed: None,
    }
}

fn view_of(kind: &ModelKind) -> keys::View {
    match kind {
        ModelKind::Pick { .. } => keys::View::Pick,
        ModelKind::Map(_) => keys::View::Map,
        ModelKind::Sheet { .. } => keys::View::Sheet,
        ModelKind::Search { .. } => keys::View::Search,
        ModelKind::Edit { .. } => keys::View::Edit,
        ModelKind::Hold { .. } | ModelKind::Release { .. } => keys::View::Hold,
        ModelKind::Ask { .. } => keys::View::Ask,
        ModelKind::Command { .. } => keys::View::Command,
        ModelKind::Help { .. } => keys::View::Help,
    }
}

fn help_lines(under: &ModelKind) -> Vec<vial::PopupLine> {
    keys::cheatsheet(view_of(under))
}

/// The status bar badge for each view. Typing views are `Input`, waiting on the keyboard is `Alert`.
fn mode_of(kind: &ModelKind) -> vial::Mode {
    use vial::ModeTone::{Alert, Browse, Input};
    let (label, tone) = match kind {
        ModelKind::Pick { .. } => ("CHOOSE", Browse),
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
        ModelKind::Command { .. } => ("COMMAND", Input),
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
            names,
            cursor,
            failed,
            ..
        } => vial::pick_text(names, *cursor, *failed, popup, mode, &footer, cols, rows),
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
            let lower = vial::Lower::Search {
                title: &listed.title,
                hits: &listed.hits,
                cursor: *cursor,
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
        ModelKind::Command {
            session,
            back,
            text,
        } => {
            let line = format!(":{text}");
            let lower = vial::Lower::Edit {
                title: "command  :w write · :wq write and quit · :q quit · :q! quit without writing · :e! discard",
                text: &line,
            };
            session.paint(*back, None, Some(lower), &chrome)
        }
        ModelKind::Help { under, scroll } => {
            let popup = vial::Popup {
                title: "help".to_string(),
                lines: help_lines(under),
                scroll: *scroll,
                actions: vec![("j k", "scroll"), ("esc", "close")],
                warn: false,
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
            Err(err) => return Assign::Disconnected(err),
        };
        if let Some(hold) = hold {
            return Assign::Holding(self.holding(write, hold.prompt));
        }
        match self.board.commit(&write) {
            Ok(vial::Commit::Stored | vial::Commit::Same) => Assign::Mapped(self),
            Ok(vial::Commit::Holding(hold)) => Assign::Holding(self.holding(write, hold.prompt)),
            Err(err) => Assign::Disconnected(err),
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
            Err(err) => HoldStep::Disconnected(err),
            Ok(true) => HoldStep::Waiting(self),
            Ok(false) => match self.board.commit(&self.phase.write) {
                Err(err) => HoldStep::Disconnected(err),
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
    Disconnected(Error),
}

enum HoldStep {
    Waiting(Session<Holding>),
    Mapped(Session<Mapping>),
    Disconnected(Error),
}

pub mod vial;
