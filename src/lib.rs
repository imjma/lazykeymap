mod app;
mod session;

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
            Error::NoBoard => write!(
                f,
                "no Vial or ZMK Studio keyboard found. Connect it by USB or Bluetooth (wake it with a \
                 key press) and check it runs Vial firmware, or ZMK with Studio enabled; a ZMK board \
                 with its own USB id needs --port. On Linux, the hidraw device also needs a udev rule."
            ),
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

pub use app::{App, Screen, TestScreen};
pub use session::vial::MemoryBoard;
pub use session::{attach, attach_file, attach_port, Attached, Event, Step};
