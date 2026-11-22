use std::io::{self, Write};
use std::time::Duration;

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

    fn event() -> lazyvial::Result<Event> {
        loop {
            let ready = event::poll(Duration::from_millis(100))
                .map_err(|_| lazyvial::Error::Disconnected)?;
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

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

impl Screen for Terminal {
    fn paint(&mut self, text: &str) {
        let mut out = io::stdout();
        let _ = execute!(out, MoveTo(0, 0), Clear(ClearType::All));
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                let _ = execute!(out, MoveToNextLine(1));
            }
            let _ = write!(out, "{line}");
        }
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
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Event::Quit;
    }
    match key.code {
        KeyCode::Left => Event::Left,
        KeyCode::Right => Event::Right,
        KeyCode::Up => Event::Up,
        KeyCode::Down => Event::Down,
        KeyCode::Enter => Event::Confirm,
        KeyCode::Esc => Event::Cancel,
        KeyCode::Backspace => Event::Backspace,
        KeyCode::Char('[') => Event::PrevLayer,
        KeyCode::Char(']') => Event::NextLayer,
        KeyCode::Char(ch) => Event::Char(ch),
        _ => Event::Tick,
    }
}

fn run() -> lazyvial::Result<()> {
    let attached = lazyvial::attach()?;
    let mut app = App::new(attached, Terminal::open()?);
    loop {
        if app.handle(Terminal::event()?)? == Step::Quit {
            return Ok(());
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
