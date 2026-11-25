use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, MoveToNextLine, Show};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use lazykeymap::{App, Event, Screen, Step};

struct Terminal;

impl Terminal {
    fn open() -> lazykeymap::Result<Self> {
        terminal::enable_raw_mode().map_err(|_| lazykeymap::Error::Disconnected)?;
        if execute!(io::stdout(), EnterAlternateScreen, Hide).is_err() {
            let _ = terminal::disable_raw_mode();
            return Err(lazykeymap::Error::Disconnected);
        }
        Ok(Self)
    }
    /// A waiting key, else `Tick` without waiting.
    fn event_now() -> lazykeymap::Result<Event> {
        while event::poll(Duration::ZERO).map_err(|_| lazykeymap::Error::Disconnected)? {
            let term = event::read().map_err(|_| lazykeymap::Error::Disconnected)?;
            if let Some(event) = map_term(term) {
                return Ok(event);
            }
        }
        Ok(Event::Tick)
    }

    /// The next key, or `Tick` once `due` passes. Ticks keep their cadence while keys stream in:
    /// holding the unlock keys auto-repeats into the terminal, and the unlock only counts polls
    /// that keep coming.
    fn event(due: Instant) -> lazykeymap::Result<Event> {
        loop {
            let wait = due.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return Ok(Event::Tick);
            }
            let ready = event::poll(wait).map_err(|_| lazykeymap::Error::Disconnected)?;
            if !ready {
                return Ok(Event::Tick);
            }
            let term = event::read().map_err(|_| lazykeymap::Error::Disconnected)?;
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
            KeyCode::Char('k') => Event::Palette,
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

/// Where to find the keyboard: every connected one (plus a named serial port), or a file.
enum Source<'a> {
    Connected(Option<&'a str>),
    File(&'a str),
}

fn run(source: Source<'_>) -> lazykeymap::Result<()> {
    let attached = match source {
        Source::File(path) => lazykeymap::attach_file(std::path::Path::new(path))?,
        Source::Connected(port) => {
            // Asking each HID device for its Vial id can take a moment over Bluetooth, and each
            // USB serial port and Bluetooth keyboard gets a short wait for a ZMK Studio answer.
            eprint!("looking for Vial and ZMK keyboards…");
            let attached = match port {
                Some(path) => lazykeymap::attach_port(path),
                None => lazykeymap::attach(),
            };
            eprint!("\r\x1b[K");
            attached?
        }
    };
    let mut app = App::new(attached, Terminal::open()?);
    let mut due = Instant::now() + TICK;
    // While a board is being read, ask for the next slice at once, still taking keys between.
    let mut working = false;
    loop {
        let event = if working {
            Terminal::event_now()?
        } else {
            Terminal::event(due)?
        };
        let tick = matches!(event, Event::Tick);
        if tick {
            due = Instant::now() + TICK;
        }
        match app.handle(event)? {
            Step::Quit => return Ok(()),
            Step::Working => working = true,
            Step::Continue => working &= !tick,
        }
    }
}

const HELP: &str = "\
lazykeymap: remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal

usage: lazykeymap [--port PATH] [--help | --version]
       lazykeymap KEYMAP

Run it with a Vial keyboard, or a ZMK Studio keyboard on USB or Bluetooth, connected.
Inside, ? lists every key.

  KEYMAP       edit a ZMK .keymap file, or the one in a zmk-config folder, instead;
               :w writes the file (e.g. config/corne.keymap)
  --port PATH  also ask this serial port for ZMK Studio, for a board whose USB id
               and maker are not ZMK's (e.g. /dev/cu.usbmodem1101, /dev/ttyACM0, COM3)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => Source::Connected(None),
        ["-V" | "--version"] => {
            println!("lazykeymap {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        ["-h" | "--help"] => {
            println!("{HELP}");
            return;
        }
        ["--port", path] => Source::Connected(Some(path)),
        [path] if !path.starts_with('-') => Source::File(path),
        [other, ..] => {
            eprintln!("lazykeymap: unknown argument {other}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    if let Err(err) = run(source) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
