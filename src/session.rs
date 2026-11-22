use crate::{Error, Result};
use std::mem;

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

/// HID enumerate, then `fan_in`. Usage page `0xFF60`, usage `0x61`.
pub fn attach() -> Result<Attached> {
    fan_in(vial::enumerate()?)
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
        Self { kind }
    }

    pub(crate) fn handle(&mut self, event: Event) -> Result<Step> {
        if matches!(event, Event::Quit)
            || matches!((&self.kind, &event), (ModelKind::Map(_), Event::Char('q')))
        {
            return Ok(Step::Quit);
        }
        match event {
            Event::Left => self.arrow(vial::Dir::Left),
            Event::Right => self.arrow(vial::Dir::Right),
            Event::Up => self.arrow(vial::Dir::Up),
            Event::Down => self.arrow(vial::Dir::Down),
            Event::NextLayer => self.layer(1),
            Event::PrevLayer => self.layer(-1),
            Event::Char(ch) => self.char(ch),
            Event::Backspace => self.backspace(),
            Event::Confirm => self.confirm(),
            Event::Cancel => self.cancel(),
            Event::Tick => self.tick(),
            Event::Quit => Ok(Step::Quit),
        }
    }

    pub(crate) fn text(&self, cols: u16, rows: u16) -> String {
        match &self.kind {
            ModelKind::Pick {
                names,
                cursor,
                failed,
                ..
            } => vial::pick_text(names, *cursor, *failed, cols, rows),
            ModelKind::Map(session) => session.paint(None, None, &[], 0, cols, rows),
            ModelKind::Search {
                session,
                query,
                cursor,
            } => {
                let hits = session.search(query);
                let legends: Vec<String> =
                    hits.iter().map(|hit| hit.legend().to_string()).collect();
                let cursor = if legends.is_empty() {
                    0
                } else {
                    (*cursor).min(legends.len() - 1)
                };
                session.paint(None, Some(query.as_str()), &legends, cursor, cols, rows)
            }
            ModelKind::Hold(session) => session.paint(
                Some(session.phase.prompt.as_str()),
                None,
                &[],
                0,
                cols,
                rows,
            ),
        }
    }

    fn arrow(&mut self, dir: vial::Dir) -> Result<Step> {
        match &mut self.kind {
            ModelKind::Pick { names, cursor, .. } => move_cursor(cursor, names.len(), dir),
            ModelKind::Map(session) => session.nudge(dir),
            ModelKind::Search {
                session,
                query,
                cursor,
            } => {
                let len = session.search(query).len();
                move_cursor(cursor, len, dir);
            }
            ModelKind::Hold(_) => {}
        }
        Ok(Step::Continue)
    }

    fn layer(&mut self, delta: i8) -> Result<Step> {
        if let ModelKind::Map(session) = &mut self.kind {
            session.layer_step(delta);
        }
        Ok(Step::Continue)
    }

    fn char(&mut self, ch: char) -> Result<Step> {
        match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Map(session) => {
                self.kind = ModelKind::Search {
                    session,
                    query: ch.to_string(),
                    cursor: 0,
                };
            }
            ModelKind::Search {
                session, mut query, ..
            } => {
                query.push(ch);
                self.kind = ModelKind::Search {
                    session,
                    query,
                    cursor: 0,
                };
            }
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    fn backspace(&mut self) -> Result<Step> {
        if let ModelKind::Search { query, cursor, .. } = &mut self.kind {
            query.pop();
            *cursor = 0;
        }
        Ok(Step::Continue)
    }

    fn confirm(&mut self) -> Result<Step> {
        if matches!(self.kind, ModelKind::Pick { .. }) {
            return self.confirm_pick();
        }
        if matches!(self.kind, ModelKind::Search { .. }) {
            return self.confirm_search();
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

    fn confirm_search(&mut self) -> Result<Step> {
        let ModelKind::Search {
            session,
            query,
            cursor,
        } = mem::replace(&mut self.kind, vacant())
        else {
            return Ok(Step::Continue);
        };
        if query.is_empty() {
            self.kind = ModelKind::Search {
                session,
                query,
                cursor,
            };
            return Ok(Step::Continue);
        }
        let hits = session.search(&query);
        let Some(hit) = hits.into_iter().nth(cursor) else {
            self.kind = ModelKind::Search {
                session,
                query,
                cursor,
            };
            return Ok(Step::Continue);
        };
        match session.assign(hit) {
            Assign::Mapped(session) => self.kind = ModelKind::Map(session),
            Assign::Holding(session) => self.kind = ModelKind::Hold(session),
            Assign::Disconnected(err) => return Err(err),
        }
        Ok(Step::Continue)
    }

    fn cancel(&mut self) -> Result<Step> {
        match mem::replace(&mut self.kind, vacant()) {
            ModelKind::Search { session, .. } => self.kind = ModelKind::Map(session),
            ModelKind::Hold(session) => self.kind = ModelKind::Map(session.cancel()),
            other => self.kind = other,
        }
        Ok(Step::Continue)
    }

    fn tick(&mut self) -> Result<Step> {
        let kind = mem::replace(&mut self.kind, vacant());
        let ModelKind::Hold(session) = kind else {
            self.kind = kind;
            return Ok(Step::Continue);
        };
        match session.poll() {
            HoldStep::Waiting(session) => self.kind = ModelKind::Hold(session),
            HoldStep::Mapped(session) => self.kind = ModelKind::Map(session),
            HoldStep::Disconnected(err) => return Err(err),
        }
        Ok(Step::Continue)
    }
}

fn vacant() -> ModelKind {
    ModelKind::Pick {
        names: Vec::new(),
        candidates: Vec::new(),
        cursor: 0,
        failed: None,
    }
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

    fn assign(mut self, hit: Hit) -> Assign {
        let Some(place) = self.selection else {
            return Assign::Mapped(self);
        };
        let code = hit.code;
        let hold = match self.board.begin_hold(code) {
            Ok(hold) => hold,
            Err(err) => return Assign::Disconnected(err),
        };
        if let Some(hold) = hold {
            return Assign::Holding(Session {
                board: self.board,
                layer: self.layer,
                selection: self.selection,
                phase: Holding {
                    place,
                    code,
                    prompt: hold.prompt,
                },
            });
        }
        match self.board.commit(self.layer, place, code) {
            Ok(vial::Commit::Stored | vial::Commit::Same) => Assign::Mapped(self),
            Ok(vial::Commit::Holding(hold)) => Assign::Holding(Session {
                board: self.board,
                layer: self.layer,
                selection: self.selection,
                phase: Holding {
                    place,
                    code,
                    prompt: hold.prompt,
                },
            }),
            Err(err) => Assign::Disconnected(err),
        }
    }

    fn search(&self, query: &str) -> Vec<Hit> {
        self.board
            .search(query)
            .into_iter()
            .map(|found| Hit {
                code: found.code,
                legend: found.legend,
            })
            .collect()
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

    fn paint(
        &self,
        hold: Option<&str>,
        query: Option<&str>,
        hits: &[String],
        cursor: usize,
        cols: u16,
        rows: u16,
    ) -> String {
        self.board.text(&vial::Paint {
            layer: self.layer,
            selection: self.selection,
            hold,
            query,
            hits,
            cursor,
            cols,
            rows,
        })
    }
}

impl Session<Holding> {
    fn poll(mut self) -> HoldStep {
        let place = self.phase.place;
        let code = self.phase.code;
        match self.board.poll_unlock() {
            Err(err) => HoldStep::Disconnected(err),
            Ok(Some(hold)) => {
                self.phase.prompt = hold.prompt;
                HoldStep::Waiting(self)
            }
            Ok(None) => match self.board.commit(self.layer, place, code) {
                Err(err) => HoldStep::Disconnected(err),
                Ok(vial::Commit::Holding(hold)) => {
                    if !hold.prompt.is_empty() {
                        self.phase.prompt = hold.prompt;
                    }
                    HoldStep::Waiting(self)
                }
                Ok(vial::Commit::Stored | vial::Commit::Same) => HoldStep::Mapped(Session {
                    board: self.board,
                    layer: self.layer,
                    selection: self.selection,
                    phase: Mapping,
                }),
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

    fn paint(
        &self,
        hold: Option<&str>,
        query: Option<&str>,
        hits: &[String],
        cursor: usize,
        cols: u16,
        rows: u16,
    ) -> String {
        self.board.text(&vial::Paint {
            layer: self.layer,
            selection: self.selection,
            hold,
            query,
            hits,
            cursor,
            cols,
            rows,
        })
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
