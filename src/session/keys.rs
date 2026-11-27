use super::vial::{Dir, PopupLine};
use super::Event;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Pick,
    Map,
    /// A tap dance, combo, or macro tab.
    Sheet,
    /// An editor tab whose entries are still being read: no cells to move to or edit yet.
    Reading,
    Search,
    /// A text or number field on a sheet.
    Edit,
    /// A yes-or-no question.
    Ask,
    Hold,
    Help,
    /// The keyboard stopped answering: reconnect, revert, start again, or quit.
    Lost,
    /// Ctrl-K: one list of commands, places, and keys, filtered as you type.
    Palette,
    /// `:export`: tick the layers that go in the image.
    Export,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    First,
    Last,
}

/// A new macro step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Tap,
    Down,
    Up,
    Text,
    Delay,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    Nudge(super::vial::Dir),
    Page(i8),
    Layer(i8),
    Tab(i8),
    End(End),
    Type(char),
    Backspace,
    Clear,
    Insert(Step),
    Reorder(i8),
    Undo,
    /// `:` or `;`: the find-anything list, opened at `:` so it lists the commands.
    Command,
    Confirm,
    Cancel,
    Quit,
    Help,
    CloseHelp,
    Scroll(i8),
    Tick,
    Ignore,
    Shelf(i8),
    /// Open the Ctrl-K list.
    Palette,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Backspace,
    Confirm,
    Cancel,
    PrevLayer,
    NextLayer,
    Tab,
    BackTab,
    Char(char),
    Palette,
}

struct Row {
    view: View,
    key: Key,
    act: Act,
    name: &'static str,
    label: &'static str,
    show: bool,
}

const ROWS: &[Row] = &[
    Row {
        view: View::Sheet,
        key: Key::Char('h'),
        act: Act::Nudge(Dir::Left),
        name: "h",
        label: "move",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('j'),
        act: Act::Nudge(Dir::Down),
        name: "j",
        label: "move",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('k'),
        act: Act::Nudge(Dir::Up),
        name: "k",
        label: "move",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('l'),
        act: Act::Nudge(Dir::Right),
        name: "l",
        label: "move",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Left,
        act: Act::Nudge(Dir::Left),
        name: "left",
        label: "move",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "move",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "move",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Right,
        act: Act::Nudge(Dir::Right),
        name: "right",
        label: "move",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('g'),
        act: Act::End(End::First),
        name: "g",
        label: "first",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('G'),
        act: Act::End(End::Last),
        name: "G",
        label: "last",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Home,
        act: Act::End(End::First),
        name: "home",
        label: "first",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::End,
        act: Act::End(End::Last),
        name: "end",
        label: "last",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::PageUp,
        act: Act::Page(-1),
        name: "ctrl-u",
        label: "page",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::PageDown,
        act: Act::Page(1),
        name: "ctrl-d",
        label: "page",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "edit",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('x'),
        act: Act::Clear,
        name: "x",
        label: "clear",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Backspace,
        act: Act::Clear,
        name: "bspc",
        label: "clear",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('t'),
        act: Act::Insert(Step::Tap),
        name: "t",
        label: "add tap",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('p'),
        act: Act::Insert(Step::Down),
        name: "p",
        label: "add press",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('r'),
        act: Act::Insert(Step::Up),
        name: "r",
        label: "add release",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('s'),
        act: Act::Insert(Step::Text),
        name: "s",
        label: "add text",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('w'),
        act: Act::Insert(Step::Delay),
        name: "w",
        label: "add wait",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('H'),
        act: Act::Reorder(-1),
        name: "H",
        label: "step left",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('L'),
        act: Act::Reorder(1),
        name: "L",
        label: "step right",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Tab,
        act: Act::Tab(1),
        name: "tab",
        label: "tabs",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::BackTab,
        act: Act::Tab(-1),
        name: "shift-tab",
        label: "tabs",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('?'),
        act: Act::Help,
        name: "?",
        label: "help",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('q'),
        act: Act::Quit,
        name: "q",
        label: "quit",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char('u'),
        act: Act::Undo,
        name: "u",
        label: "undo",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char(':'),
        act: Act::Command,
        name: ":",
        label: "command",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Char(';'),
        act: Act::Command,
        name: ";",
        label: "command",
        show: false,
    },
    Row {
        view: View::Reading,
        key: Key::Tab,
        act: Act::Tab(1),
        name: "tab",
        label: "tabs",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::BackTab,
        act: Act::Tab(-1),
        name: "shift-tab",
        label: "tabs",
        show: false,
    },
    Row {
        view: View::Reading,
        key: Key::Char('?'),
        act: Act::Help,
        name: "?",
        label: "help",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::Char('q'),
        act: Act::Quit,
        name: "q",
        label: "quit",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::Char('u'),
        act: Act::Undo,
        name: "u",
        label: "undo",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::Char(':'),
        act: Act::Command,
        name: ":",
        label: "command",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::Char(';'),
        act: Act::Command,
        name: ";",
        label: "command",
        show: false,
    },
    Row {
        view: View::Ask,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "yes",
        // The popup draws its own key chips.
        show: false,
    },
    Row {
        view: View::Ask,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "no",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Char('j'),
        act: Act::Nudge(Dir::Down),
        name: "j",
        label: "down",
        // The popup draws its own key chips.
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Char('k'),
        act: Act::Nudge(Dir::Up),
        name: "k",
        label: "up",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Char(' '),
        act: Act::Type(' '),
        name: "space",
        label: "tick or untick the layer",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Char('a'),
        act: Act::Type('a'),
        name: "a",
        label: "tick every layer, or none",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "save the ticked layers",
        show: false,
    },
    Row {
        view: View::Export,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "cancel",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Tab,
        act: Act::Tab(1),
        name: "tab",
        label: "complete a command",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::BackTab,
        act: Act::Tab(-1),
        name: "shift-tab",
        label: "complete a command",
        show: false,
    },
    Row {
        view: View::Edit,
        key: Key::Backspace,
        act: Act::Backspace,
        name: "bspc",
        label: "delete",
        show: true,
    },
    Row {
        view: View::Edit,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "save",
        show: true,
    },
    Row {
        view: View::Edit,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "cancel",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('h'),
        act: Act::Nudge(Dir::Left),
        name: "h",
        label: "left",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('l'),
        act: Act::Nudge(Dir::Right),
        name: "l",
        label: "right",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('k'),
        act: Act::Nudge(Dir::Up),
        name: "k",
        label: "up",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('j'),
        act: Act::Nudge(Dir::Down),
        name: "j",
        label: "down",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Left,
        act: Act::Nudge(Dir::Left),
        name: "left",
        label: "left",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Right,
        act: Act::Nudge(Dir::Right),
        name: "right",
        label: "right",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char('g'),
        act: Act::End(End::First),
        name: "g",
        label: "first",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char('G'),
        act: Act::End(End::Last),
        name: "G",
        label: "last",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Home,
        act: Act::End(End::First),
        name: "home",
        label: "first",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::End,
        act: Act::End(End::Last),
        name: "end",
        label: "last",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char('['),
        act: Act::Layer(-1),
        name: "[",
        label: "prev",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char(']'),
        act: Act::Layer(1),
        name: "]",
        label: "next",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::PrevLayer,
        act: Act::Layer(-1),
        name: "p",
        label: "",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::NextLayer,
        act: Act::Layer(1),
        name: "n",
        label: "",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::PageUp,
        act: Act::Layer(-1),
        name: "ctrl-u",
        label: "prev",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::PageDown,
        act: Act::Layer(1),
        name: "ctrl-d",
        label: "next",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char('/'),
        act: Act::Confirm,
        name: "/",
        label: "find",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "catalog",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('?'),
        act: Act::Help,
        name: "?",
        label: "help",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char('q'),
        act: Act::Quit,
        name: "q",
        label: "quit",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "close",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Tab,
        act: Act::Tab(1),
        name: "tab",
        label: "tabs",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::BackTab,
        act: Act::Tab(-1),
        name: "shift-tab",
        label: "tabs",
        show: false,
    },
    Row {
        view: View::Map,
        key: Key::Char('u'),
        act: Act::Undo,
        name: "u",
        label: "undo",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char(':'),
        act: Act::Command,
        name: ":",
        label: "command",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char(';'),
        act: Act::Command,
        name: ";",
        label: "command",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Char('j'),
        act: Act::Nudge(Dir::Down),
        name: "j",
        label: "down",
        show: true,
    },
    Row {
        view: View::Pick,
        key: Key::Char('k'),
        act: Act::Nudge(Dir::Up),
        name: "k",
        label: "up",
        show: true,
    },
    Row {
        view: View::Pick,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Left,
        act: Act::Nudge(Dir::Left),
        name: "left",
        label: "left",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Right,
        act: Act::Nudge(Dir::Right),
        name: "right",
        label: "right",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Char('g'),
        act: Act::End(End::First),
        name: "g",
        label: "first",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Char('G'),
        act: Act::End(End::Last),
        name: "G",
        label: "last",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Home,
        act: Act::End(End::First),
        name: "home",
        label: "first",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::End,
        act: Act::End(End::Last),
        name: "end",
        label: "last",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::PageUp,
        act: Act::Page(-1),
        name: "ctrl-u",
        label: "page",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::PageDown,
        act: Act::Page(1),
        name: "ctrl-d",
        label: "page",
        show: false,
    },
    Row {
        view: View::Pick,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "open",
        show: true,
    },
    Row {
        view: View::Pick,
        key: Key::Char('q'),
        act: Act::Quit,
        name: "q",
        label: "quit",
        show: true,
    },
    Row {
        view: View::Pick,
        key: Key::Char('?'),
        act: Act::Help,
        name: "?",
        label: "help",
        show: true,
    },
    Row {
        view: View::Lost,
        key: Key::Char('j'),
        act: Act::Nudge(Dir::Down),
        name: "j",
        label: "down",
        show: true,
    },
    Row {
        view: View::Lost,
        key: Key::Char('k'),
        act: Act::Nudge(Dir::Up),
        name: "k",
        label: "up",
        show: true,
    },
    Row {
        view: View::Lost,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Lost,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Lost,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "choose",
        show: true,
    },
    Row {
        view: View::Lost,
        key: Key::Char('q'),
        act: Act::Quit,
        name: "q",
        label: "quit",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::Left,
        act: Act::Nudge(Dir::Left),
        name: "←→↑↓",
        label: "move",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Right,
        act: Act::Nudge(Dir::Right),
        name: "right",
        label: "move",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::Tab,
        act: Act::Shelf(1),
        name: "tab",
        label: "group/part",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::BackTab,
        act: Act::Shelf(-1),
        name: "shift-tab",
        label: "group/part",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::PageUp,
        act: Act::Page(-1),
        name: "ctrl-u",
        label: "page",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::PageDown,
        act: Act::Page(1),
        name: "ctrl-d",
        label: "page",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Home,
        act: Act::End(End::First),
        name: "home",
        label: "first",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::End,
        act: Act::End(End::Last),
        name: "end",
        label: "last",
        show: false,
    },
    Row {
        view: View::Search,
        key: Key::Backspace,
        act: Act::Backspace,
        name: "bspc",
        label: "edit",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "assign",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Cancel,
        act: Act::Cancel,
        // Ctrl-[ is Esc in every terminal, for a keyboard that has no Esc key.
        name: "esc/ctrl-[",
        label: "close",
        show: true,
    },
    Row {
        view: View::Search,
        key: Key::Char('?'),
        act: Act::Help,
        name: "?",
        label: "help",
        show: true,
    },
    // The unlock keys are real keys, so holding them types into this terminal. Corne's stock
    // combo is Tab+Q, so `q` cannot quit here. Only Esc cancels. Ctrl-C still quits.
    Row {
        view: View::Hold,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "cancel",
        show: true,
    },
    Row {
        view: View::Help,
        key: Key::Char('j'),
        act: Act::Scroll(1),
        name: "j",
        label: "down",
        show: true,
    },
    Row {
        view: View::Help,
        key: Key::Char('k'),
        act: Act::Scroll(-1),
        name: "k",
        label: "up",
        show: true,
    },
    Row {
        view: View::Help,
        key: Key::Down,
        act: Act::Scroll(1),
        name: "down",
        label: "down",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::Up,
        act: Act::Scroll(-1),
        name: "up",
        label: "up",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::PageDown,
        act: Act::Page(1),
        name: "ctrl-d",
        label: "page",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::PageUp,
        act: Act::Page(-1),
        name: "ctrl-u",
        label: "page",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::Char('g'),
        act: Act::End(End::First),
        name: "g",
        label: "first",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::Home,
        act: Act::End(End::First),
        name: "home",
        label: "first",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::Char('G'),
        act: Act::End(End::Last),
        name: "G",
        label: "last",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::End,
        act: Act::End(End::Last),
        name: "end",
        label: "last",
        show: false,
    },
    Row {
        view: View::Help,
        key: Key::Cancel,
        act: Act::CloseHelp,
        name: "esc",
        label: "close",
        show: true,
    },
    Row {
        view: View::Help,
        key: Key::Char('q'),
        act: Act::CloseHelp,
        name: "q",
        label: "close",
        show: true,
    },
    Row {
        view: View::Help,
        key: Key::Char('?'),
        act: Act::CloseHelp,
        name: "?",
        label: "close",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Char(' '),
        act: Act::Palette,
        name: "space",
        label: "find anything",
        show: true,
    },
    Row {
        view: View::Map,
        key: Key::Palette,
        act: Act::Palette,
        name: "ctrl-k",
        label: "find anything",
        show: false,
    },
    Row {
        view: View::Sheet,
        key: Key::Char(' '),
        act: Act::Palette,
        name: "space",
        label: "find anything",
        show: true,
    },
    Row {
        view: View::Sheet,
        key: Key::Palette,
        act: Act::Palette,
        name: "ctrl-k",
        label: "find anything",
        show: false,
    },
    Row {
        view: View::Reading,
        key: Key::Char(' '),
        act: Act::Palette,
        name: "space",
        label: "find anything",
        show: true,
    },
    Row {
        view: View::Reading,
        key: Key::Palette,
        act: Act::Palette,
        name: "ctrl-k",
        label: "find anything",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Up,
        act: Act::Nudge(Dir::Up),
        name: "up",
        label: "move",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Down,
        act: Act::Nudge(Dir::Down),
        name: "down",
        label: "move",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Confirm,
        act: Act::Confirm,
        name: "enter",
        label: "go",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Backspace,
        act: Act::Backspace,
        name: "backspace",
        label: "delete",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Cancel,
        act: Act::Cancel,
        name: "esc",
        label: "close",
        show: false,
    },
    Row {
        view: View::Palette,
        key: Key::Palette,
        act: Act::Cancel,
        name: "ctrl-k",
        label: "close",
        show: false,
    },
];

fn key_of(event: &Event) -> Option<Key> {
    Some(match event {
        Event::Left => Key::Left,
        Event::Right => Key::Right,
        Event::Up => Key::Up,
        Event::Down => Key::Down,
        Event::PageUp => Key::PageUp,
        Event::PageDown => Key::PageDown,
        Event::Home => Key::Home,
        Event::End => Key::End,
        Event::Backspace => Key::Backspace,
        Event::Confirm => Key::Confirm,
        Event::Cancel => Key::Cancel,
        Event::PrevLayer => Key::PrevLayer,
        Event::NextLayer => Key::NextLayer,
        Event::Tab => Key::Tab,
        Event::BackTab => Key::BackTab,
        Event::Char(ch) => Key::Char(*ch),
        Event::Palette => Key::Palette,
        Event::Tick | Event::Quit | Event::Mouse { .. } => return None,
    })
}

pub(super) fn act(view: View, event: &Event) -> Act {
    if matches!(event, Event::Tick) {
        return Act::Tick;
    }
    let Some(key) = key_of(event) else {
        return Act::Ignore;
    };
    for row in ROWS {
        if row.view == view && row.key == key {
            return row.act;
        }
    }
    if let Key::Char(ch) = key {
        if matches!(view, View::Search | View::Edit | View::Ask | View::Palette) {
            return Act::Type(ch);
        }
    }
    Act::Ignore
}

/// The options bar: `(keys, what they do)`. Hints that share a label merge, as `left/right`.
pub(super) fn footer(view: View) -> Vec<(String, &'static str)> {
    let mut merged: Vec<(&'static str, Vec<&str>)> = Vec::new();
    for row in ROWS.iter().filter(|row| row.view == view && row.show) {
        match merged.iter_mut().find(|(label, _)| *label == row.label) {
            Some((_, names)) => names.push(row.name),
            None => merged.push((row.label, vec![row.name])),
        }
    }
    merged
        .into_iter()
        .map(|(label, names)| (names.join("/"), label))
        .collect()
}

/// What clicking the `index`th options-bar hint does: its act, when every key it merges does the
/// same thing (`h/j/k/l move` does four things, so it is not a button).
pub(super) fn footer_act(view: View, index: usize) -> Option<Act> {
    let (_, label) = footer(view).into_iter().nth(index)?;
    // `←→↑↓ move` stands for four arrows, whatever its one row says.
    if label == "move" {
        return None;
    }
    let mut acts = ROWS
        .iter()
        .filter(|row| row.view == view && row.show && row.label == label)
        .map(|row| row.act);
    let act = acts.next()?;
    acts.all(|other| other == act).then_some(act)
}

/// The help popup body: the group names, then each group, the current view first.
pub(super) fn cheatsheet(current: View) -> Vec<PopupLine> {
    let views = [
        View::Map,
        View::Search,
        View::Pick,
        View::Hold,
        View::Help,
        View::Sheet,
        View::Edit,
        View::Ask,
        View::Palette,
        View::Export,
        View::Lost,
    ];
    let mut lines = vec![PopupLine::Text(
        views
            .iter()
            .map(|view| title(*view))
            .collect::<Vec<_>>()
            .join(" · "),
    )];
    let mut ordered = vec![current];
    for view in views {
        if view != current {
            ordered.push(view);
        }
    }
    for (at, view) in ordered.into_iter().enumerate() {
        lines.push(PopupLine::Blank);
        lines.push(PopupLine::Heading(title(view).to_string()));
        lines.extend(group(view));
        // The mouse works in every view, so it follows the one on screen.
        if at == 0 {
            lines.push(PopupLine::Blank);
            lines.push(PopupLine::Heading("mouse".to_string()));
            for (what, does) in [
                (
                    "click",
                    "select a key, cell, tab, or hint; in the key list, another key",
                ),
                ("click again", "open the key list, or stage the pick"),
                ("wheel", "change layer on the board, scroll a list"),
                ("right-click", "back, as esc"),
                ("shift-drag", "select text, as without the mouse"),
            ] {
                lines.push(PopupLine::Pair(what.to_string(), does.to_string()));
            }
        }
    }
    lines
}

fn title(view: View) -> &'static str {
    match view {
        View::Map => "board",
        View::Search => "search",
        View::Pick => "choose",
        View::Hold => "hold",
        View::Help => "help",
        View::Sheet => "tap dance, combos, macros",
        View::Reading => "still reading",
        View::Edit => "field",
        View::Ask => "confirm",
        View::Lost => "disconnected",
        View::Palette => "find anything",
        View::Export => "export",
    }
}

fn group(view: View) -> Vec<PopupLine> {
    let mut items: Vec<(Act, &str, Vec<&str>)> = Vec::new();
    for row in ROWS
        .iter()
        .filter(|row| row.view == view && !row.label.is_empty())
    {
        if let Some((_, _, names)) = items
            .iter_mut()
            .find(|(act, label, _)| *act == row.act && *label == row.label)
        {
            names.push(row.name);
            continue;
        }
        items.push((row.act, row.label, vec![row.name]));
    }
    items
        .into_iter()
        .map(|(_, label, names)| PopupLine::Pair(names.join(" "), label.to_string()))
        .collect()
}
