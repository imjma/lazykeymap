use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, MoveToNextLine, Show};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use lazyvial::{App, Event, Screen, Step};

struct Terminal;

impl Terminal {
    fn open() -> lazyvial::Result<Self> {
        terminal::enable_raw_mode().map_err(|_| lazyvial::Error::Disconnected)?;
        if execute!(io::stdout(), EnterAlternateScreen, Hide).is_err() {
            let _ = terminal::disable_raw_mode();
            return Err(lazyvial::Error::Disconnected);
        }
        Ok(Self)
    }

    /// The next key, or `Tick` once `due` passes. Ticks keep their cadence while keys stream in:
    /// holding the unlock keys auto-repeats into the terminal, and the unlock only counts polls
    /// that keep coming.
    fn event(due: Instant) -> lazyvial::Result<Event> {
        loop {
            let wait = due.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return Ok(Event::Tick);
            }
            let ready = event::poll(wait).map_err(|_| lazyvial::Error::Disconnected)?;
            if !ready {
                return Ok(Event::Tick);
            }
            let term = event::read().map_err(|_| lazyvial::Error::Disconnected)?;
            if let Some(event) = map_term(term) {
                return Ok(event);
            }
        }
    }
}

/// vial-qmk counts an unlock poll only when more than 100 ms passed since the last counted one,
/// and resets the countdown otherwise. Vial's own client polls every 200 ms.
const TICK: Duration = Duration::from_millis(200);

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

impl Screen for Terminal {
    fn paint(&mut self, text: &str) {
        let mut out = io::stdout();
        let _ = execute!(out, MoveTo(0, 0));
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                let _ = execute!(out, MoveToNextLine(1));
            }
            let _ = execute!(out, Clear(ClearType::UntilNewLine));
            let _ = write!(out, "{line}");
        }
        let _ = execute!(out, Clear(ClearType::FromCursorDown));
        let _ = out.flush();
    }

    fn cols(&self) -> u16 {
        terminal::size().map(|(cols, _)| cols).unwrap_or(80)
    }

    fn rows(&self) -> u16 {
        terminal::size().map(|(_, rows)| rows).unwrap_or(24)
    }
}

fn map_term(event: TermEvent) -> Option<Event> {
    let TermEvent::Key(key) = event else {
        return None;
    };
    if key.kind == KeyEventKind::Release {
        return None;
    }
    Some(map_key(key))
}

fn map_key(key: KeyEvent) -> Event {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Event::Quit,
            KeyCode::Char('u') => Event::PageUp,
            KeyCode::Char('d') => Event::PageDown,
            KeyCode::Char('p') => Event::Up,
            KeyCode::Char('n') => Event::Down,
            _ => Event::Tick,
        };
    }
    match key.code {
        KeyCode::Left => Event::Left,
        KeyCode::Tab => Event::Tab,
        KeyCode::BackTab => Event::BackTab,
        KeyCode::Right => Event::Right,
        KeyCode::Up => Event::Up,
        KeyCode::Down => Event::Down,
        KeyCode::PageUp => Event::PageUp,
        KeyCode::PageDown => Event::PageDown,
        KeyCode::Home => Event::Home,
        KeyCode::End => Event::End,
        KeyCode::Enter => Event::Confirm,
        KeyCode::Esc => Event::Cancel,
        KeyCode::Backspace => Event::Backspace,
        KeyCode::Char(ch) => Event::Char(ch),
        _ => Event::Tick,
    }
}

fn run() -> lazyvial::Result<()> {
    let attached = lazyvial::attach()?;
    let mut app = App::new(attached, Terminal::open()?);
    let mut due = Instant::now() + TICK;
    loop {
        let event = Terminal::event(due)?;
        if matches!(event, Event::Tick) {
            due = Instant::now() + TICK;
        }
        if app.handle(event)? == Step::Quit {
            return Ok(());
        }
    }
}

const HELP: &str = "\
lazyvial: remap a Vial keyboard from the terminal

usage: lazyvial [--help | --version]

Run it with a Vial keyboard plugged in. Inside, ? lists every key.";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("-V" | "--version") => {
            println!("lazyvial {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("-h" | "--help") => {
            println!("{HELP}");
            return;
        }
        Some(other) => {
            eprintln!("lazyvial: unknown argument {other}\n\n{HELP}");
            std::process::exit(2);
        }
        None => {}
    }
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
