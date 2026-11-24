use crate::{Error, Result};
use serde_json::Value;

use super::catalog::Found;

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
    pub(super) midi: Midi,
}

/// The definition's `vial.midi`. Vial shows the MIDI tab only for `basic` or `advanced`.
#[derive(Clone, Copy)]
pub(super) enum Midi {
    None,
    Basic,
    Advanced,
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
    /// Degrees. The key's `x` and `y` are in the rotated cluster, around `rotation_x`/`rotation_y`.
    pub(super) rotation_angle: f64,
    pub(super) rotation_x: f64,
    pub(super) rotation_y: f64,
    pub(super) labels: [String; 12],
    pub(super) decal: bool,
}

pub(super) struct CustomCode {
    pub(super) code: u16,
    pub(super) name: String,
}

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Key,
    None,
    Trans,
    Layer,
    Hold,
    Media,
    Raw,
    Decal,
}

pub(super) struct Cap {
    pub(super) place: Option<Place>,
    pub(super) legend: String,
    pub(super) selected: bool,
    /// One of the keys the unlock hold asks for.
    pub(super) unlock: bool,
    /// Staged and not yet written to the keyboard.
    pub(super) changed: bool,
    pub(super) decal: bool,
    pub(super) tone: Tone,
    /// World x, after rotation. Column start is `round(x * 5)`.
    pub(super) x: f64,
    /// World y, after rotation. A gap above half a key starts a new line.
    pub(super) y: f64,
    pub(super) w: f64,
    #[allow(dead_code)]
    pub(super) h: f64,
}

/// LZMA is the caller's job. This takes the decompressed JSON.
pub(super) fn parse(json: &str) -> Result<Definition> {
    let value: Value = serde_json::from_str(json).map_err(|_| Error::NoLayout)?;
    let rows_n = u8_field(&value, &["matrix", "rows"])?;
    let cols_n = u8_field(&value, &["matrix", "cols"])?;
    let labels = parse_labels(value.get("layouts").and_then(|v| v.get("labels")))?;
    let keymap = value
        .get("layouts")
        .and_then(|v| v.get("keymap"))
        .and_then(Value::as_array)
        .ok_or(Error::NoLayout)?;
    let rows = parse_keymap(keymap)?;
    let def = Definition {
        rows_n,
        cols_n,
        rows,
        labels,
        custom: parse_custom(value.get("customKeycodes")),
        midi: match value.pointer("/vial/midi").and_then(Value::as_str) {
            Some("basic") => Midi::Basic,
            Some("advanced") => Midi::Advanced,
            _ => Midi::None,
        },
    };
    validate(&def)?;
    Ok(def)
}

fn validate(def: &Definition) -> Result<()> {
    for row in &def.rows {
        for key in row {
            interpret(key, def.rows_n, def.cols_n)?;
            if key.labels[8].is_empty() {
                continue;
            }
            let (index, choice) = pair_u8(&key.labels[8]).ok_or(Error::NoLayout)?;
            let group = def.labels.get(index as usize).ok_or(Error::NoLayout)?;
            if choice as usize >= group.choices.len() {
                return Err(Error::NoLayout);
            }
        }
    }
    Ok(())
}

/// `None` is a decal. Anything else that is not a matrix cell or an encoder direction is `NoLayout`.
pub(super) fn interpret(key: &RawKey, rows: u8, cols: u8) -> Result<Option<Place>> {
    if key.decal {
        return Ok(None);
    }
    if key.labels[4] == "e" {
        let (index, direction) = pair_u8(&key.labels[0]).ok_or(Error::NoLayout)?;
        if direction > 1 {
            return Err(Error::NoLayout);
        }
        return Ok(Some(Place::Encoder { index, direction }));
    }
    let (row, col) = pair_u8(&key.labels[0]).ok_or(Error::NoLayout)?;
    if row >= rows || col >= cols {
        return Err(Error::NoLayout);
    }
    Ok(Some(Place::Matrix { row, col }))
}

pub(super) fn place_of(key: &RawKey) -> Option<Place> {
    if key.decal {
        return None;
    }
    if key.labels[4] == "e" {
        let (index, direction) = pair_u8(&key.labels[0])?;
        return Some(Place::Encoder { index, direction });
    }
    let (row, col) = pair_u8(&key.labels[0])?;
    Some(Place::Matrix { row, col })
}

pub(super) fn encoder_count(def: &Definition) -> usize {
    let mut count = 0usize;
    for row in &def.rows {
        for key in row {
            if let Some(Place::Encoder { index, .. }) = place_of(key) {
                count = count.max(index as usize + 1);
            }
        }
    }
    count
}

/// `layouts.labels` in order. Bits are `(choice_count - 1).bit_length()`, low end first.
/// No label 8: visible. Label 8 `index,choice` must match the decoded group.
pub(super) fn visible(def: &Definition, options: u32, key: &RawKey) -> bool {
    if key.labels[8].is_empty() {
        return true;
    }
    let Some((index, choice)) = pair_u8(&key.labels[8]) else {
        return false;
    };
    decoded_choice(&def.labels, options, index as usize) == Some(choice as u32)
}

fn decoded_choice(groups: &[LayoutGroup], options: u32, index: usize) -> Option<u32> {
    let mut shift = 0u32;
    for (i, group) in groups.iter().enumerate() {
        let choice_count = group.choices.len() as u32;
        let width = if choice_count == 0 {
            0
        } else {
            bit_length(choice_count - 1)
        };
        if i == index {
            if width == 0 {
                return Some(0);
            }
            if shift >= 32 {
                return None;
            }
            let mask = if width >= 32 {
                u32::MAX
            } else {
                (1u32 << width) - 1
            };
            return Some((options >> shift) & mask);
        }
        shift = shift.saturating_add(width);
    }
    None
}

fn bit_length(value: u32) -> u32 {
    u32::BITS - value.leading_zeros()
}

/// Everything around the main panel: the header bar, the status bar, and the options bar.
pub(super) struct Frame<'a> {
    pub(super) header: Header,
    /// The badge at the left of the status bar.
    pub(super) mode: Mode,
    /// The unlock prompt. It takes the status bar while a save waits for it.
    pub(super) hold: Option<String>,
    pub(super) lower: Option<Lower<'a>>,
    /// A box over the whole screen: help, or a question.
    pub(super) popup: Option<&'a Popup>,
    /// One amber line in place of the status: a refusal, a result, an unknown command.
    pub(super) notice: Option<String>,
    /// `(keys, what they do)`, drawn as key chips.
    pub(super) footer: &'a [(String, &'static str)],
    pub(super) cols: u16,
    pub(super) rows: u16,
}

/// The top line: the board, its tabs, and its state.
pub(super) struct Header {
    pub(super) name: String,
    pub(super) tabs: Vec<String>,
    pub(super) active: usize,
    /// `Some(true)` locked, `Some(false)` unlocked, `None` before a board is open.
    pub(super) locked: Option<bool>,
    pub(super) unsaved: usize,
}

/// What the keys do right now, as a badge: browsing, typing, or waiting on the keyboard.
#[derive(Clone, Copy)]
pub(in crate::session) struct Mode {
    pub(in crate::session) label: &'static str,
    pub(in crate::session) tone: ModeTone,
}

#[derive(Clone, Copy)]
pub(in crate::session) enum ModeTone {
    Browse,
    Input,
    Alert,
}

#[derive(Clone, Copy)]
pub(in crate::session) enum Lower<'a> {
    Search {
        title: &'a str,
        hits: &'a [Found],
        cursor: usize,
    },
    /// A one-line text or number field.
    Edit { title: &'a str, text: &'a str },
}

/// A box drawn over the screen, which dims behind it.
pub(in crate::session) struct Popup {
    pub(in crate::session) title: String,
    pub(in crate::session) lines: Vec<PopupLine>,
    /// First body line shown. The body scrolls when it is taller than the screen.
    pub(in crate::session) scroll: usize,
    /// Keys that answer it, drawn as chips under the body: `(key, what it does)`.
    pub(in crate::session) actions: Vec<(&'static str, &'static str)>,
    /// Amber frame for a question that loses or writes data; blue otherwise.
    pub(in crate::session) warn: bool,
}

#[derive(Clone)]
pub(in crate::session) enum PopupLine {
    Heading(String),
    /// A key and what it does, in two aligned columns.
    Pair(String, String),
    Text(String),
    /// A change: what it was, and what it becomes.
    Change(String, String),
    Blank,
}

/// One tap dance, combo, or macro tab. Cells already carry their legends.
pub(super) struct Sheet {
    pub(super) title: String,
    /// Column names. Empty for macros, whose rows are ragged.
    pub(super) header: Vec<&'static str>,
    pub(super) rows: Vec<(String, Vec<SheetCell>)>,
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) status: String,
}

pub(super) struct SheetCell {
    pub(super) text: String,
    pub(super) tone: Tone,
    /// Staged and not yet written to the keyboard.
    pub(super) changed: bool,
}

/// Bars across the top and bottom of the screen.
const BAR: Rgb = Rgb(24, 26, 36);
/// A board on the choose screen that the cursor is not on.
const CARD: Rgb = Rgb(30, 33, 44);
/// The brand chip at the left of the header.
const BRAND: Rgb = Rgb(187, 154, 247);
/// Mode badge for typing views: search, fields, the command line.
const INPUT: Rgb = Rgb(187, 154, 247);
const READY: Rgb = Rgb(158, 206, 106);

/// One run of text with its colors. Bars are built from these so every cell keeps the bar color.
type Seg = (String, Rgb, Rgb, bool);

fn seg(text: impl Into<String>, bg: Rgb, fg: Rgb, bold: bool) -> Seg {
    (text.into(), bg, fg, bold)
}

fn seg_width(segs: &[Seg]) -> usize {
    segs.iter().map(|(text, ..)| text.chars().count()).sum()
}

/// `left`, bar fill, `right`, exactly `cols` wide. `right` drops first, then `left` clips.
fn bar_line(left: &[Seg], right: &[Seg], cols: usize) -> String {
    fill_line(left, right, cols, BAR)
}

/// `bar_line` on any background.
fn fill_line(left: &[Seg], right: &[Seg], cols: usize, bg: Rgb) -> String {
    let right: &[Seg] = if seg_width(left) + seg_width(right) > cols {
        &[]
    } else {
        right
    };
    let fill_w = cols.saturating_sub(seg_width(left) + seg_width(right));
    let line: String = left
        .iter()
        .chain(std::iter::once(&seg(" ".repeat(fill_w), bg, TEXT, false)))
        .chain(right)
        .map(|(text, bg, fg, bold)| paint(text, *bg, *fg, *bold))
        .collect();
    take_visible(&line, cols)
}

/// ` lazyvial  board  L1 L2 Tap Dance  ...  ● 2 unsaved  ● ready `.
/// Tabs farthest from the active one drop first when the line is too narrow.
fn header_line(header: &Header, cols: usize) -> String {
    let mut left = vec![
        seg(" lazyvial ", BRAND, SELECTED_FG, true),
        seg(format!(" {} ", header.name), BAR, TEXT, true),
    ];
    let mut right = Vec::new();
    if header.unsaved > 0 {
        right.push(seg(
            format!(" ● {} unsaved ", header.unsaved),
            BAR,
            CHANGED,
            true,
        ));
    }
    match header.locked {
        Some(true) => right.push(seg(" ● locked ", BAR, UNLOCK, true)),
        Some(false) => right.push(seg(" ● ready ", BAR, READY, false)),
        None => {}
    }
    let room = cols.saturating_sub(seg_width(&left) + seg_width(&right) + 2);
    let tab = |index: usize| {
        let name = format!(" {} ", header.tabs[index]);
        if index == header.active {
            seg(name, SELECTED_BG, SELECTED_FG, true)
        } else {
            seg(name, BAR, DIM, false)
        }
    };
    let width = |first: usize, last: usize| {
        (first..=last)
            .map(|i| header.tabs[i].chars().count() + 3)
            .sum::<usize>()
            + 2
    };
    if !header.tabs.is_empty() {
        let (mut first, mut last) = (0, header.tabs.len() - 1);
        while first < last && width(first, last) > room {
            if header.active - first > last - header.active {
                first += 1;
            } else {
                last -= 1;
            }
        }
        left.push(seg("  ", BAR, DIM, false));
        if first > 0 {
            left.push(seg("…", BAR, DIM, false));
        }
        for index in first..=last {
            left.push(tab(index));
            left.push(seg(" ", BAR, DIM, false));
        }
        if last + 1 < header.tabs.len() {
            left.push(seg("…", BAR, DIM, false));
        }
    }
    bar_line(&left, &right, cols)
}

/// `[MODE] status text ...  ? help`. The status is already a run of segments on the bar.
fn status_line(mode: Mode, status: &[Seg], cols: usize) -> String {
    let badge = match mode.tone {
        ModeTone::Browse => SELECTED_BG,
        ModeTone::Input => INPUT,
        ModeTone::Alert => UNLOCK,
    };
    let mut left = vec![seg(format!(" {} ", mode.label), badge, SELECTED_FG, true)];
    left.push(seg(" ", BAR, TEXT, false));
    left.extend(status.iter().cloned());
    let right = [seg(" ? help ", BAR, DIM, false)];
    bar_line(&left, &right, cols)
}

/// Key chips: ` h ` on a key-colored chip, then what it does. Hints that do not fit drop off.
fn footer_line(hints: &[(String, &str)], cols: usize) -> String {
    let mut line = String::new();
    let mut used = 0;
    for (index, (keys, label)) in hints.iter().enumerate() {
        let gap = if index == 0 { "" } else { "  " };
        let width = gap.len() + keys.chars().count() + 2 + 1 + label.chars().count();
        if used + width > cols {
            break;
        }
        line.push_str(gap);
        line.push_str(&paint(&format!(" {keys} "), KEY_FACE, TEXT, true));
        line.push_str(&dye(&format!(" {label}"), DIM, false));
        used += width;
    }
    line
}

/// What the selected key does, for the spare rows under the keys.
pub(super) struct Detail {
    pub(super) legend: String,
    /// The QMK name, when the catalog lists this exact code.
    pub(super) token: Option<String>,
    pub(super) code: u16,
    pub(super) kind: String,
    /// This key on every layer: `(legend, is this layer, staged and unsaved)`.
    pub(super) layers: Vec<(String, bool, bool)>,
    /// What the keyboard holds, when the draft differs from it.
    pub(super) saved: Option<String>,
}

pub(super) fn render_board(
    layer_one_based: u8,
    layer_count: u8,
    caps: &[Cap],
    detail: Option<&Detail>,
    frame: Frame<'_>,
) -> String {
    let title = format!("layer {layer_one_based}/{layer_count}");
    let selected = caps
        .iter()
        .find(|cap| cap.selected && !cap.legend.is_empty())
        .map(|cap| {
            let at = match cap.place {
                Some(Place::Matrix { row, col }) => format!("  ·  key {row},{col}"),
                Some(Place::Encoder { index, direction }) => {
                    let turn = if direction == 0 { "cw" } else { "ccw" };
                    format!("  ·  encoder {index} {turn}")
                }
                None => String::new(),
            };
            let mut status = vec![seg(format!("selected {}", cap.legend), BAR, TEXT, true)];
            if cap.changed {
                status.push(seg("  ·  unsaved", BAR, CHANGED, false));
            }
            status.push(seg(at, BAR, DIM, false));
            status
        })
        .unwrap_or_default();
    compose(
        Top {
            title: &title,
            status: selected,
            body: Body::Keys(caps, detail),
        },
        frame,
    )
}

/// Three or four lines: a rule, the key and what it does, the key on each layer, and what the
/// keyboard holds when the draft differs. Each line fits `width`.
fn detail_lines(detail: &Detail, width: usize) -> Vec<String> {
    let mut lines = vec![dye(&"─".repeat(width), Rgb(44, 48, 62), false)];
    let mut head = paint(&format!(" {} ", detail.legend), KEY_FACE, TEXT, true);
    head.push_str("  ");
    if let Some(token) = &detail.token {
        head.push_str(&dye(token, TEXT, false));
        head.push_str(&dye("  ·  ", DIM, false));
    }
    head.push_str(&dye(&format!("0x{:04X}", detail.code), DIM, false));
    head.push_str(&dye("  ·  ", DIM, false));
    head.push_str(&dye(&detail.kind, DIM, false));
    lines.push(head);
    if detail.layers.len() > 1 {
        let mut row = dye("on each layer  ", DIM, false);
        for (index, (legend, here, changed)) in detail.layers.iter().enumerate() {
            let name = format!("L{} ", index + 1);
            let chip = format!(" {legend} ");
            if *here {
                row.push_str(&dye(&name, TEXT, true));
                row.push_str(&paint(&chip, SELECTED_BG, SELECTED_FG, true));
            } else {
                row.push_str(&dye(&name, DIM, false));
                let fg = if *changed { CHANGED } else { TEXT };
                row.push_str(&paint(&chip, KEY_FACE, fg, *changed));
            }
            row.push_str("  ");
        }
        lines.push(row);
    }
    if let Some(saved) = &detail.saved {
        lines.push(dye(
            &format!(
                "unsaved: the keyboard still has {saved}, :w writes {}",
                detail.legend
            ),
            CHANGED,
            false,
        ));
    }
    lines
        .into_iter()
        .map(|line| take_visible(&line, width))
        .collect()
}

pub(super) fn render_sheet(sheet: &Sheet, frame: Frame<'_>) -> String {
    compose(
        Top {
            title: &sheet.title,
            status: vec![seg(sheet.status.clone(), BAR, TEXT, false)],
            body: Body::Sheet(sheet),
        },
        frame,
    )
}

pub(super) fn render_pick<'a>(
    boards: &'a [super::Candidate],
    cursor: usize,
    note: super::CardNote<'a>,
    frame: Frame<'_>,
) -> String {
    let count = boards.len();
    let status = match (&note, boards.get(cursor)) {
        (super::CardNote::Opening(_), Some(board)) => vec![
            seg(format!("reading {}", board.listing.name), BAR, TEXT, true),
            seg(
                "  ·  Bluetooth takes a few seconds  ·  esc stops",
                BAR,
                DIM,
                false,
            ),
        ],
        _ => vec![seg(
            format!(
                "{count} Vial keyboard{} found",
                if count == 1 { "" } else { "s" }
            ),
            BAR,
            TEXT,
            false,
        )],
    };
    compose(
        Top {
            title: "keyboards",
            status,
            body: Body::Pick {
                boards,
                cursor,
                note,
            },
        },
        frame,
    )
}

struct Top<'a> {
    title: &'a str,
    /// What the status bar says when nothing more urgent does.
    status: Vec<Seg>,
    body: Body<'a>,
}

enum Body<'a> {
    Keys(&'a [Cap], Option<&'a Detail>),
    Sheet(&'a Sheet),
    Pick {
        boards: &'a [super::Candidate],
        cursor: usize,
        note: super::CardNote<'a>,
    },
}

struct Spend {
    header: bool,
    status: bool,
    footer: bool,
    board_top: bool,
    board_bottom: bool,
    lower_top: bool,
    lower_bottom: bool,
    lower_body: usize,
    board_pad: usize,
}

/// Key lines are reserved first, then the status bar, since the unlock prompt lives there.
/// Spare rows then buy the options bar, the header, the lower panel, and the panel borders.
fn spend(rows: u16, key_lines: usize, footer: bool, lower: bool) -> Spend {
    let mut spare = usize::from(rows).saturating_sub(key_lines);
    let take = |want: bool, spare: &mut usize| {
        let got = want && *spare > 0;
        if got {
            *spare -= 1;
        }
        got
    };
    let status = take(true, &mut spare);
    let footer = take(footer, &mut spare);
    let header = take(true, &mut spare);
    let lower_top = take(lower, &mut spare);
    let mut lower_body = 0;
    if lower {
        lower_body = spare.min(4);
        spare -= lower_body;
    }
    let board_top = take(true, &mut spare);
    let board_bottom = take(board_top, &mut spare);
    let lower_bottom = take(lower_top, &mut spare);
    let mut board_pad = 0;
    if lower {
        lower_body += spare;
    } else {
        board_pad = spare;
    }
    Spend {
        header,
        status,
        footer,
        board_top,
        board_bottom,
        lower_top,
        lower_bottom,
        lower_body,
        board_pad,
    }
}

/// Header bar, the main panel, the lower panel when there is one, the status bar, the options bar.
fn compose(top: Top<'_>, frame: Frame<'_>) -> String {
    let Frame {
        header,
        mode,
        hold,
        lower,
        popup,
        notice,
        footer,
        cols,
        rows,
    } = frame;
    let width = usize::from(cols);
    // The unlock prompt beats a notice, and a notice beats the usual status.
    let (mode, status) = match (hold, notice) {
        (Some(hold), _) => (
            Mode {
                label: "UNLOCK",
                tone: ModeTone::Alert,
            },
            vec![seg(hold, BAR, UNLOCK, true)],
        ),
        (None, Some(notice)) => (mode, vec![seg(notice, BAR, UNLOCK, true)]),
        (None, None) => (mode, top.status),
    };
    let key_lines = match &top.body {
        Body::Keys(caps, _) => paint_rows(caps, cols).len(),
        Body::Pick { boards, .. } => (boards.len() * 3).saturating_sub(1),
        // A sheet scrolls, so it takes what is left after the bars and borders, at least two lines.
        Body::Sheet(sheet) => {
            let around = 3 + usize::from(!footer.is_empty()) + if lower.is_some() { 6 } else { 0 };
            sheet_len(sheet).min(usize::from(rows).saturating_sub(around).max(2))
        }
    };
    let spent = spend(rows, key_lines, !footer.is_empty(), lower.is_some());
    let boxed = spent.board_top && spent.board_bottom;
    let side = boxed && cols >= 2;
    let key_cols = if side { cols.saturating_sub(2) } else { cols };
    let mut lines = Vec::new();
    if spent.header {
        lines.push(header_line(&header, width));
    }
    let body = board_body(&top.body, &spent, key_cols, key_lines);
    if spent.board_top {
        lines.extend(panel(top.title, &body, boxed, side, lower.is_none(), cols));
    } else {
        lines.extend(body);
    }
    if let Some(lower) = lower.filter(|_| spent.lower_top) {
        let lower_boxed = spent.lower_bottom;
        let lower_side = lower_boxed && cols >= 2;
        let lower_cols = if lower_side {
            cols.saturating_sub(2)
        } else {
            cols
        };
        lines.extend(panel(
            lower_title(&lower),
            &lower_body(&lower, spent.lower_body, lower_cols),
            lower_boxed,
            lower_side,
            true,
            cols,
        ));
    }
    if spent.status {
        lines.push(status_line(mode, &status, width));
    }
    if spent.footer {
        lines.push(footer_line(footer, width));
    }
    cover(&clip(&lines, cols, rows), popup, cols, rows)
}

fn board_body(body: &Body<'_>, spent: &Spend, key_cols: u16, key_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    match body {
        Body::Keys(caps, detail) => {
            // The keys, then the selected key's details when the spare rows hold them
            // with a blank line between. The group sits centered in the panel.
            let keys = paint_rows(caps, key_cols);
            let drawn = keys.iter().map(|line| visible_len(line)).max().unwrap_or(0);
            let indent = usize::from(key_cols).saturating_sub(drawn) / 2;
            let left = " ".repeat(indent);
            let detail = detail
                .map(|detail| {
                    detail_lines(detail, drawn.max(40).min(usize::from(key_cols) - indent))
                })
                // The details and a blank line above them.
                .filter(|lines| lines.len() < spent.board_pad)
                .unwrap_or_default();
            let pad = spent.board_pad
                - if detail.is_empty() {
                    0
                } else {
                    detail.len() + 1
                };
            let above = pad / 2;
            out.extend(std::iter::repeat_n(String::new(), above));
            out.extend(keys.into_iter().map(|line| format!("{left}{line}")));
            if !detail.is_empty() {
                out.push(String::new());
                out.extend(detail.into_iter().map(|line| format!("{left}{line}")));
            }
            out.extend(std::iter::repeat_n(String::new(), pad - above));
        }
        Body::Sheet(sheet) => {
            out.extend(sheet_lines(sheet, key_lines, usize::from(key_cols)));
            out.extend(std::iter::repeat_n(String::new(), spent.board_pad));
        }
        Body::Pick {
            boards,
            cursor,
            note,
        } => {
            let above = spent.board_pad / 2;
            out.extend(std::iter::repeat_n(String::new(), above));
            out.extend(pick_cards(boards, *cursor, note, usize::from(key_cols)));
            out.extend(std::iter::repeat_n(String::new(), spent.board_pad - above));
        }
    }
    out
}

fn sheet_len(sheet: &Sheet) -> usize {
    sheet.rows.len() + usize::from(!sheet.header.is_empty())
}

/// A header, then one line per entry. The window keeps the cursor row in view.
/// Fixed columns line up. Macro rows are ragged and slide left to keep the cursor cell in view.
fn sheet_lines(sheet: &Sheet, room: usize, width: usize) -> Vec<String> {
    let label_w = sheet
        .rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let ragged = sheet.header.is_empty();
    let widths: Vec<usize> = (0..sheet.header.len())
        .map(|col| {
            sheet
                .rows
                .iter()
                .filter_map(|(_, cells)| cells.get(col))
                .map(|cell| cell.text.chars().count() + 2)
                .chain([sheet.header[col].chars().count() + 2, 6])
                .max()
                .unwrap_or(6)
                .min(18)
        })
        .collect();
    let mut lines = Vec::new();
    let mut room = room;
    if !ragged && room > 0 {
        let mut header = format!(" {:label_w$} ", "");
        for (name, w) in sheet.header.iter().zip(&widths) {
            header.push_str(&format!(" {name:^w$}", w = *w));
        }
        lines.push(dye(&header, DIM, false));
        room -= 1;
    }
    let start = window_start(sheet.rows.len(), sheet.row, room);
    for (index, (label, cells)) in sheet.rows.iter().enumerate().skip(start).take(room) {
        let here = index == sheet.row;
        let mut line = if here {
            dye(&format!(" {label:label_w$} "), TEXT, true)
        } else {
            dye(&format!(" {label:label_w$} "), DIM, false)
        };
        let mut chips: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(col, cell)| {
                let w = if ragged {
                    cell.text.chars().count() + 2
                } else {
                    widths[col]
                };
                sheet_chip(cell, w, here && col == sheet.col)
            })
            .collect();
        if ragged && here {
            // Drop chips from the left until the cursor chip ends inside the panel.
            let budget = width.saturating_sub(label_w + 4);
            let mut first = 0;
            while first < sheet.col
                && chips[first..=sheet.col.min(chips.len() - 1)]
                    .iter()
                    .map(|chip| visible_len(chip) + 1)
                    .sum::<usize>()
                    > budget
            {
                first += 1;
            }
            if first > 0 {
                chips.drain(..first);
                chips.insert(0, dye("…", DIM, false));
            }
        }
        for chip in chips {
            line.push(' ');
            line.push_str(&chip);
        }
        lines.push(line);
    }
    lines
}

fn sheet_chip(cell: &SheetCell, width: usize, on: bool) -> String {
    let text: String = cell.text.chars().take(width.saturating_sub(2)).collect();
    let (bg, fg) = if on {
        (SELECTED_BG, SELECTED_FG)
    } else {
        let (bg, fg) = tone_colors(cell.tone);
        (bg, if cell.changed { CHANGED } else { fg })
    };
    fill(&text, width, bg, fg, on || cell.changed)
}

/// First index of a `room`-long window over `len` items that keeps `cursor` near the middle.
fn window_start(len: usize, cursor: usize, room: usize) -> usize {
    if len <= room || room == 0 {
        return 0;
    }
    cursor.saturating_sub(room / 2).min(len - room)
}

fn lower_title<'a>(lower: &Lower<'a>) -> &'a str {
    match lower {
        Lower::Search { title, .. } | Lower::Edit { title, .. } => title,
    }
}

fn lower_body(lower: &Lower<'_>, room: usize, width: u16) -> Vec<String> {
    let width = usize::from(width);
    match lower {
        Lower::Search { hits, cursor, .. } => search_grid(hits, *cursor, room, width),
        Lower::Edit { text, .. } => {
            vec![format!(
                " > {text}{}",
                dye_bg(" ", SELECTED_FG, SELECTED_BG)
            )]
        }
    }
}

/// Two lines per board, a blank line between: name and maker, then USB id and serial.
/// The cursor's card is lighter and carries an accent bar. While it opens, its second line is a
/// progress bar; a board that would not open says why in place of the maker.
fn pick_cards(
    boards: &[super::Candidate],
    cursor: usize,
    note: &super::CardNote<'_>,
    width: usize,
) -> Vec<String> {
    // At most 64 columns, centered, so a wide terminal does not stretch the cards.
    let inner = if width >= 24 {
        (width - 4).min(64)
    } else {
        width
    };
    let pad = " ".repeat((width - inner) / 2);
    let mut out = Vec::new();
    for (index, board) in boards.iter().enumerate() {
        let board = &board.listing;
        let on = index == cursor;
        let bg = if on { KEY_FACE } else { CARD };
        let edge = if on {
            seg("▌ ", bg, SELECTED_BG, true)
        } else {
            seg("  ", bg, DIM, false)
        };
        let name = seg(&board.name, bg, TEXT, on);
        let opening = match note {
            super::CardNote::Opening(progress) if on => Some(progress),
            _ => None,
        };
        let side = match note {
            super::CardNote::Failed(at, why) if *at == index => {
                seg(format!("✕ {why} "), bg, UNLOCK, true)
            }
            _ if opening.is_some() => seg("opening… ", bg, SELECTED_BG, true),
            _ => seg(format!("{} ", board.maker), bg, DIM, false),
        };
        let ids = [board.usb.as_str(), board.serial.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
        if index > 0 {
            out.push(String::new());
        }
        out.push(format!(
            "{pad}{}",
            fill_line(&[edge.clone(), name], &[side], inner, bg)
        ));
        let detail = match opening {
            Some(progress) => {
                let cells = 16;
                let filled = progress.percent.min(100) * cells / 100;
                vec![
                    edge,
                    seg("▰".repeat(filled), bg, SELECTED_BG, false),
                    seg("▱".repeat(cells - filled), bg, DIM, false),
                    seg(
                        format!("  {}%  {}", progress.percent, progress.what),
                        bg,
                        TEXT,
                        false,
                    ),
                ]
            }
            None => vec![edge, seg(ids, bg, DIM, false)],
        };
        out.push(format!("{pad}{}", fill_line(&detail, &[], inner, bg)));
    }
    out
}

/// Hits run down a column, then into the next, so Up and Down still walk the list in order.
/// The window slides by whole columns and keeps the cursor's column near the middle.
fn search_grid(hits: &[Found], cursor: usize, room: usize, width: usize) -> Vec<String> {
    if room == 0 {
        return Vec::new();
    }
    if hits.is_empty() {
        return vec![dye("   no match", DIM, false)];
    }
    let legend_w = hits
        .iter()
        .map(|hit| hit.legend.chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    let token_w = hits
        .iter()
        .filter_map(shown_token)
        .map(|token| token.chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    let cell = 3 + legend_w + if token_w > 0 { 2 + token_w } else { 0 } + 2;
    let across = (width / cell).max(1);
    let down = room.min(hits.len());
    let columns = hits.len().div_ceil(down);
    let cursor = cursor.min(hits.len() - 1);
    let first = if columns <= across {
        0
    } else {
        (cursor / down)
            .saturating_sub(across / 2)
            .min(columns - across)
    };
    (0..down)
        .map(|row| {
            (first..columns.min(first + across))
                .filter_map(|column| {
                    let index = column * down + row;
                    hits.get(index)
                        .map(|hit| hit_cell(hit, index == cursor, legend_w, token_w, cell))
                })
                .collect()
        })
        .collect()
}

/// The QMK name, when it says more than the legend.
fn shown_token(hit: &Found) -> Option<&str> {
    let token = hit.token.as_str();
    (!token.is_empty() && !token.eq_ignore_ascii_case(&hit.legend)).then_some(token)
}

fn hit_cell(hit: &Found, on: bool, legend_w: usize, token_w: usize, cell: usize) -> String {
    let mark = if on { " > " } else { "   " };
    let legend: String = hit.legend.chars().take(legend_w).collect();
    let token: String = shown_token(hit)
        .unwrap_or("")
        .chars()
        .take(token_w)
        .collect();
    let tail = if token_w > 0 {
        format!("  {token:<token_w$}")
    } else {
        String::new()
    };
    let gap = "  ";
    if on {
        let body = format!("{mark}{legend:<legend_w$}{tail}");
        return format!(
            "{}{gap}",
            dye_bg(&pad_visible(&body, cell - 2), SELECTED_FG, SELECTED_BG)
        );
    }
    format!("{mark}{legend:<legend_w$}{}{gap}", dye(&tail, DIM, false))
}

fn panel(
    title: &str,
    body: &[String],
    boxed: bool,
    side: bool,
    active: bool,
    cols: u16,
) -> Vec<String> {
    let cols = usize::from(cols);
    if cols == 0 || (!boxed && title.is_empty() && body.is_empty()) {
        return Vec::new();
    }
    let color = if active { SELECTED_BG } else { DIM };
    if !boxed {
        let mut lines = vec![take_visible(title, cols)];
        lines.extend(body.iter().map(|line| take_visible(line, cols)));
        return lines;
    }
    let inner = cols.saturating_sub(2);
    let mut lines = vec![dye(&top_rule(title, cols), color, active)];
    for line in body {
        if side {
            lines.push(side_line(line, inner, color, active));
        } else {
            lines.push(take_visible(line, cols));
        }
    }
    lines.push(dye(&bottom_rule(cols), color, active));
    lines
}

fn top_rule(title: &str, cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    let room = cols.saturating_sub(5);
    // Slide a too-wide title left so a bracketed active shelf stays on screen.
    let end = title.chars().position(|c| c == ']').map_or(0, |at| at + 1);
    let title: String = title
        .chars()
        .skip(end.saturating_sub(room))
        .take(room)
        .collect();
    let dashes = cols.saturating_sub(title.chars().count() + 5);
    format!("╭─ {title} {}╮", "─".repeat(dashes))
}

fn bottom_rule(cols: usize) -> String {
    if cols < 2 {
        return "╯".repeat(cols);
    }
    format!("╰{}╯", "─".repeat(cols - 2))
}

fn side_line(line: &str, inner: usize, color: Rgb, bold: bool) -> String {
    format!(
        "{}{}{}",
        dye("│", color, bold),
        pad_visible(line, inner),
        dye("│", color, bold)
    )
}

fn pad_visible(text: &str, width: usize) -> String {
    let shown = take_visible(text, width);
    let pad = width.saturating_sub(visible_len(&shown));
    format!("{shown}{}", " ".repeat(pad))
}

fn dye(text: &str, fg: Rgb, bold: bool) -> String {
    let weight = if bold { "1;" } else { "" };
    format!(
        "\u{1b}[{weight}38;2;{};{};{}m{text}\u{1b}[0m",
        fg.0, fg.1, fg.2
    )
}

fn dye_bg(text: &str, fg: Rgb, bg: Rgb) -> String {
    format!(
        "\u{1b}[38;2;{};{};{};48;2;{};{};{}m{text}\u{1b}[0m",
        fg.0, fg.1, fg.2, bg.0, bg.1, bg.2
    )
}

/// A physical row stays together when its keys sit within this many units of the row's top.
/// The Corne v4 inner keys drop 0.75u, and the next row starts 1u below that top.
const ROW_BAND: f64 = 0.85;

/// Band index for each y. Index 0 is the top row on screen.
pub(super) fn band_of(ys: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ys.len()).collect();
    order.sort_by(|&a, &b| ys[a].total_cmp(&ys[b]).then(a.cmp(&b)));
    let mut bands = vec![0; ys.len()];
    let mut origin = 0.0;
    let mut band = 0usize;
    let mut started = false;
    for index in order {
        if started && ys[index] - origin > ROW_BAND {
            band += 1;
            origin = ys[index];
        } else if !started {
            origin = ys[index];
            started = true;
        }
        bands[index] = band;
    }
    bands
}

fn paint_rows(caps: &[Cap], cols: u16) -> Vec<String> {
    if caps.is_empty() {
        return Vec::new();
    }
    let min_x = caps.iter().map(|cap| cap.x).fold(f64::INFINITY, f64::min);
    let max_right = caps
        .iter()
        .map(|cap| cap.x + cap.w.max(1.0))
        .fold(0.0, f64::max);
    let span = (max_right - min_x).max(1.0);
    let unit = cell_width(span, cols);
    let bands = band_of(&caps.iter().map(|cap| cap.y).collect::<Vec<_>>());
    let mut rows: Vec<Vec<&Cap>> = Vec::new();
    for (cap, band) in caps.iter().zip(bands) {
        if rows.len() <= band {
            rows.resize_with(band + 1, Vec::new);
        }
        rows[band].push(cap);
    }
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.extend(paint_row(row, cols, min_x, unit));
    }
    lines
}

/// Columns per 1u. The widest size that fits the board, then 5.
fn cell_width(span: f64, cols: u16) -> usize {
    let room = f64::from(cols);
    for unit in [8usize, 7, 6, 5] {
        if span * unit as f64 <= room {
            return unit;
        }
    }
    5
}

/// Three screen lines per key: header, main legend, filled body. A blank line separates rows.
/// Drawn width is one column short of the key's unit span, which leaves the gap.
fn paint_row(caps: &[&Cap], cols: u16, min_x: f64, unit: usize) -> [String; 3] {
    let width = cols as usize;
    let mut ordered: Vec<&Cap> = caps.to_vec();
    ordered.sort_by(|a, b| a.x.total_cmp(&b.x));
    let mut lines = [String::new(), String::new(), String::new()];
    let mut occupied = 0usize;
    for cap in ordered {
        let column = ((cap.x - min_x) * unit as f64).round().max(0.0) as usize;
        if width == 0 || column >= width || column < occupied {
            continue;
        }
        let drawn = drawn_width(cap.w, unit).min(width - column);
        let cells = filled(cap, drawn);
        for (line, cell) in lines.iter_mut().zip(cells) {
            place(line, column, &cell, width);
        }
        occupied = column + drawn;
    }
    lines.map(|line| take_visible(&line, width))
}

fn drawn_width(w: f64, unit: usize) -> usize {
    let full = (w.max(0.0) * unit as f64).round().max(1.0) as usize;
    let body = full.saturating_sub(1).max(1);
    if full >= 4 {
        body.max(3)
    } else {
        body
    }
}

fn place(line: &mut String, column: usize, text: &str, width: usize) {
    while visible_len(line) < column {
        line.push(' ');
    }
    let room = width.saturating_sub(column);
    line.push_str(&take_visible(text, room));
}

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

const TEXT: Rgb = Rgb(214, 220, 232);
const DIM: Rgb = Rgb(125, 134, 155);
const KEY_FACE: Rgb = Rgb(39, 44, 57);
const POPUP: Rgb = Rgb(22, 24, 33);
const SELECTED_BG: Rgb = Rgb(122, 162, 247);
const SELECTED_FG: Rgb = Rgb(15, 17, 23);
/// Amber marks the unlock keys and the hold line that names them.
const UNLOCK: Rgb = Rgb(224, 175, 104);
/// Green legend on a staged key or cell that the keyboard does not have yet.
const CHANGED: Rgb = Rgb(158, 206, 106);

fn colors(cap: &Cap) -> (Rgb, Rgb, Rgb) {
    if cap.selected {
        return (SELECTED_BG, SELECTED_FG, SELECTED_FG);
    }
    if cap.unlock {
        return (UNLOCK, SELECTED_FG, SELECTED_FG);
    }
    let (bg, fg) = tone_colors(cap.tone);
    if cap.changed {
        return (bg, CHANGED, CHANGED);
    }
    (bg, fg, DIM)
}

fn tone_colors(tone: Tone) -> (Rgb, Rgb) {
    match tone {
        Tone::Key => (KEY_FACE, TEXT),
        Tone::None => (Rgb(31, 31, 36), DIM),
        Tone::Trans => (Rgb(25, 28, 37), DIM),
        Tone::Layer => (Rgb(36, 58, 78), TEXT),
        Tone::Hold => (Rgb(52, 45, 72), TEXT),
        Tone::Media => (Rgb(35, 58, 50), TEXT),
        Tone::Raw => (Rgb(66, 38, 43), TEXT),
        Tone::Decal => (Rgb(25, 28, 37), DIM),
    }
}

fn filled(cap: &Cap, drawn: usize) -> [String; 3] {
    let (bg, fg, header_fg) = colors(cap);
    let label_cols = if cap.selected {
        drawn.saturating_sub(1)
    } else {
        drawn
    };
    let shown = if cap.decal {
        format!("({})", cap.legend)
    } else {
        cap.legend.clone()
    };
    let (header, main) = cap_lines(&shown, label_cols.max(1));
    [
        cell_line(&header, drawn, cap.selected, bg, header_fg),
        cell_line(&main, drawn, cap.selected, bg, fg),
        cell_line("", drawn, cap.selected, bg, fg),
    ]
}

fn cell_line(label: &str, drawn: usize, selected: bool, bg: Rgb, fg: Rgb) -> String {
    if selected {
        let rest = drawn.saturating_sub(1);
        format!(
            "{}{}",
            fill("▌", 1, bg, fg, true),
            fill(label, rest, bg, fg, true)
        )
    } else {
        fill(label, drawn, bg, fg, false)
    }
}

fn fill(text: &str, width: usize, bg: Rgb, fg: Rgb, bold: bool) -> String {
    let body = center(text, width);
    let weight = if bold { "1;" } else { "" };
    format!(
        "\u{1b}[{weight}38;2;{};{};{};48;2;{};{};{}m{body}\u{1b}[0m",
        fg.0, fg.1, fg.2, bg.0, bg.1, bg.2
    )
}

/// `MO(1)` is header `MO` and main `1`. `C-A` is header `C` and main `A`.
/// A legend with no split stays on the main line. Wider than the cap, it wraps at the space that
/// fits both halves most evenly, else at the middle.
fn cap_lines(legend: &str, room: usize) -> (String, String) {
    let room = room.max(1);
    if let Some((head, tail)) = legend.split_once('(') {
        if legend.ends_with(')') && !head.is_empty() {
            let main = tail.trim_end_matches(')').to_string();
            if !main.is_empty() && fits(head, room) && fits(&main, room) {
                return (head.to_string(), main);
            }
        }
    }
    if let Some((head, tail)) = legend.split_once('-') {
        if !head.is_empty()
            && !tail.is_empty()
            && !tail.contains('-')
            && fits(head, room)
            && fits(tail, room)
        {
            return (head.to_string(), tail.to_string());
        }
    }
    if fits(legend, room) {
        return (String::new(), legend.to_string());
    }
    let best = legend
        .match_indices(' ')
        .map(|(at, _)| (&legend[..at], &legend[at + 1..]))
        .filter(|(top, bottom)| fits(top, room) && fits(bottom, room))
        .min_by_key(|(top, bottom)| top.chars().count().max(bottom.chars().count()));
    if let Some((top, bottom)) = best {
        return (top.to_string(), bottom.to_string());
    }
    let mid = legend.chars().count().div_ceil(2);
    let top: String = legend.chars().take(mid).collect();
    let bottom: String = legend.chars().skip(mid).collect();
    (top, bottom)
}

fn fits(text: &str, room: usize) -> bool {
    text.chars().count() <= room
}

fn center(text: &str, width: usize) -> String {
    let text: String = text.chars().take(width).collect();
    let pad = width.saturating_sub(text.chars().count());
    let left = pad / 2;
    format!("{}{text}{}", " ".repeat(left), " ".repeat(pad - left))
}

fn visible_len(text: &str) -> usize {
    let mut len = 0usize;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            for ch in chars.by_ref() {
                if ch == 'm' {
                    break;
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

fn take_visible(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut seen = 0usize;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            out.push(ch);
            for ch in chars.by_ref() {
                out.push(ch);
                if ch == 'm' {
                    break;
                }
            }
            continue;
        }
        if seen >= width {
            break;
        }
        out.push(ch);
        seen += 1;
    }
    if out.contains('\u{1b}') && !out.ends_with("\u{1b}[0m") {
        out.push_str("\u{1b}[0m");
    }
    out
}

pub(in crate::session) fn clip(lines: &[String], cols: u16, rows: u16) -> String {
    lines
        .iter()
        .take(rows as usize)
        .map(|line| take_visible(line, cols as usize))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Draws `popup` over `frame`: the frame dims, the box sits in the middle with a shadow.
fn cover(frame: &str, popup: Option<&Popup>, cols: u16, rows: u16) -> String {
    let Some(popup) = popup else {
        return frame.to_string();
    };
    let cols = usize::from(cols);
    let rows = usize::from(rows);
    if cols < 8 || rows < 4 {
        return frame.to_string();
    }
    let boxed = popup_box(popup, cols, rows);
    stamp(frame, &boxed, cols, rows)
}

/// Blank columns between each side bar and the text.
const POPUP_PAD: usize = 2;
/// Widest body text before long lines wrap.
const POPUP_TEXT_MAX: usize = 60;

fn popup_box(popup: &Popup, cols: usize, rows: usize) -> Vec<Vec<Cell>> {
    let accent = if popup.warn { UNLOCK } else { SELECTED_BG };
    let key_w = popup
        .lines
        .iter()
        .filter_map(|line| match line {
            PopupLine::Pair(key, _) => Some(key.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let chips = action_chips(&popup.actions, accent);
    let chips_w = chips.len();
    let fixed_w = popup
        .lines
        .iter()
        .map(|line| match line {
            PopupLine::Heading(text) => text.chars().count(),
            PopupLine::Pair(_, label) => key_w + 3 + label.chars().count(),
            PopupLine::Change(was, now) => was.chars().count() + 5 + now.chars().count(),
            PopupLine::Text(_) | PopupLine::Blank => 0,
        })
        .chain([chips_w, popup.title.chars().count() + 4, 28])
        .max()
        .unwrap_or(28);
    let longest_text = popup
        .lines
        .iter()
        .map(|line| match line {
            PopupLine::Text(text) => text.chars().count(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let text_w = fixed_w.max(longest_text.min(POPUP_TEXT_MAX));
    // Keep three columns free for the shadow and a margin.
    let inner = (text_w + POPUP_PAD * 2).min(cols.saturating_sub(5)).max(10);
    let width = inner + 2;
    let body_w = inner - POPUP_PAD * 2;
    let lines = wrap_text(&popup.lines, body_w);
    // Border, a pad line above and below, and the chips with a pad line under them.
    let chrome = 4 + if chips_w > 0 { 2 } else { 0 };
    let room = rows.saturating_sub(chrome + 2).max(1);
    let total = lines.len();
    let scroll = popup.scroll.min(total.saturating_sub(room));
    let shown = &lines[scroll.min(total)..(scroll + room).min(total)];
    let gap = |n: usize| cells_of(&fill("", n, POPUP, TEXT, false));
    let side = |line: Vec<Cell>| {
        let mut row = cells_of(&dye("│", accent, true));
        row.extend(line);
        row.extend(cells_of(&dye("│", accent, true)));
        row
    };
    let mut out = vec![cells_of(&dye(
        &popup_top(&popup.title, width),
        accent,
        true,
    ))];
    out.push(side(gap(inner)));
    for line in shown {
        let mut row = gap(POPUP_PAD);
        row.extend(popup_line(line, key_w, body_w, accent));
        row.extend(gap(POPUP_PAD));
        out.push(side(row));
    }
    out.push(side(gap(inner)));
    if chips_w > 0 {
        // Chips sit right-aligned, the way dialog buttons do.
        let mut row = gap(inner.saturating_sub(chips_w + POPUP_PAD));
        row.extend(chips.into_iter().take(inner.saturating_sub(POPUP_PAD)));
        row.extend(gap(POPUP_PAD.min(inner.saturating_sub(row.len()))));
        out.push(side(row));
        out.push(side(gap(inner)));
    }
    let position = (total > room).then(|| {
        let last = (scroll + room).min(total);
        let up = if scroll > 0 { "↑" } else { " " };
        let down = if last < total { "↓" } else { " " };
        format!("{up}{down} {}–{last} of {total}", scroll + 1)
    });
    out.push(cells_of(&dye(
        &popup_bottom(position.as_deref(), width),
        accent,
        true,
    )));
    out
}

/// Text lines wider than `width` split at spaces; other lines pass through.
fn wrap_text(lines: &[PopupLine], width: usize) -> Vec<PopupLine> {
    let mut out = Vec::new();
    for line in lines {
        let PopupLine::Text(text) = line else {
            out.push(line.clone());
            continue;
        };
        let mut current = String::new();
        for word in text.split(' ') {
            let needed =
                current.chars().count() + usize::from(!current.is_empty()) + word.chars().count();
            if !current.is_empty() && needed > width {
                out.push(PopupLine::Text(std::mem::take(&mut current)));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        out.push(PopupLine::Text(current));
    }
    out
}

/// `╭─ title ────╮`.
fn popup_top(title: &str, width: usize) -> String {
    let title: String = title.chars().take(width.saturating_sub(6)).collect();
    let dashes = width.saturating_sub(title.chars().count() + 5);
    format!("╭─ {title} {}╮", "─".repeat(dashes))
}

/// `╰──── ↑↓ 3–20 of 64 ─╯` when the body scrolls, else a plain rule.
fn popup_bottom(position: Option<&str>, width: usize) -> String {
    match position {
        Some(text) if width > text.chars().count() + 6 => {
            let dashes = width - text.chars().count() - 5;
            format!("╰{} {text} ─╯", "─".repeat(dashes))
        }
        _ => format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
    }
}

/// One body line, exactly `width` cells on the popup background.
fn popup_line(line: &PopupLine, key_w: usize, width: usize, accent: Rgb) -> Vec<Cell> {
    let segments: Vec<(String, Rgb, bool)> = match line {
        PopupLine::Blank => Vec::new(),
        PopupLine::Heading(text) => vec![(text.to_uppercase(), accent, true)],
        PopupLine::Text(text) => vec![(text.clone(), TEXT, false)],
        PopupLine::Pair(key, label) => vec![
            (format!("{key:<key_w$}   "), TEXT, true),
            (label.clone(), DIM, false),
        ],
        PopupLine::Change(was, now) => vec![
            (was.clone(), DIM, false),
            ("  →  ".to_string(), accent, true),
            (now.clone(), CHANGED, true),
        ],
    };
    let mut row = Vec::new();
    for (text, fg, bold) in segments {
        let text: String = text.chars().take(width.saturating_sub(row.len())).collect();
        row.extend(cells_of(&paint(&text, POPUP, fg, bold)));
    }
    row.extend(cells_of(&fill(
        "",
        width.saturating_sub(row.len()),
        POPUP,
        TEXT,
        false,
    )));
    row
}

/// ` enter  write & quit   esc  stay `: each key a filled chip, its label after it.
fn action_chips(actions: &[(&str, &str)], accent: Rgb) -> Vec<Cell> {
    let mut row = Vec::new();
    for (index, (key, label)) in actions.iter().enumerate() {
        if index > 0 {
            row.extend(cells_of(&fill("", 3, POPUP, TEXT, false)));
        }
        row.extend(cells_of(&paint(
            &format!(" {key} "),
            accent,
            SELECTED_FG,
            true,
        )));
        row.extend(cells_of(&paint(&format!(" {label}"), POPUP, TEXT, false)));
    }
    row
}

/// `text` as is, on `bg`.
fn paint(text: &str, bg: Rgb, fg: Rgb, bold: bool) -> String {
    let weight = if bold { "1;" } else { "" };
    format!(
        "\u{1b}[{weight}38;2;{};{};{};48;2;{};{};{}m{text}\u{1b}[0m",
        fg.0, fg.1, fg.2, bg.0, bg.1, bg.2
    )
}

#[derive(Clone)]
struct Cell {
    ch: char,
    style: String,
}

/// Dims the frame, then lays `popup` in the middle with a shadow right and below it.
fn stamp(frame: &str, popup: &[Vec<Cell>], cols: usize, rows: usize) -> String {
    let mut grid: Vec<Vec<Cell>> = frame.split('\n').map(cells_of).collect();
    grid.truncate(rows);
    while grid.len() < rows {
        grid.push(Vec::new());
    }
    let blank = Cell {
        ch: ' ',
        style: String::new(),
    };
    for row in &mut grid {
        row.resize(cols, blank.clone());
        for cell in row.iter_mut() {
            cell.style = dim_style(&cell.style);
        }
    }
    let box_h = popup.len();
    let box_w = popup.iter().map(Vec::len).max().unwrap_or(0);
    let top = rows.saturating_sub(box_h) / 2;
    let left = cols.saturating_sub(box_w) / 2;
    let shadow = format!(
        "\u{1b}[38;2;40;44;56;48;2;{};{};{}m",
        SHADOW.0, SHADOW.1, SHADOW.2
    );
    // Two columns down the right side, one row along the bottom, offset like a drop shadow.
    for row in grid.iter_mut().take(top + box_h + 1).skip(top + 1) {
        for cell in row.iter_mut().skip(left + box_w).take(2) {
            cell.style.clone_from(&shadow);
        }
    }
    if let Some(row) = grid.get_mut(top + box_h) {
        for cell in row.iter_mut().take(left + box_w + 2).skip(left + 2) {
            cell.style.clone_from(&shadow);
        }
    }
    for (i, line) in popup.iter().enumerate() {
        let y = top + i;
        if y >= rows {
            break;
        }
        for (x, cell) in line.iter().enumerate() {
            if left + x < cols {
                grid[y][left + x] = cell.clone();
            }
        }
    }
    grid.into_iter()
        .map(|row| paint_cells(&row))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Behind a popup, under its shadow.
const SHADOW: Rgb = Rgb(6, 7, 10);

/// The same SGR style at 40% brightness and without bold, so the screen behind a popup recedes.
/// Plain cells get a dim foreground.
fn dim_style(style: &str) -> String {
    let body = style
        .strip_prefix("\u{1b}[")
        .and_then(|rest| rest.strip_suffix('m'))
        .unwrap_or("");
    let parts: Vec<&str> = body.split(';').filter(|part| !part.is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        if matches!(parts[i], "38" | "48") && parts.get(i + 1) == Some(&"2") && i + 4 < parts.len()
        {
            let scale = |part: &str| (part.parse::<u16>().unwrap_or(0) * 2 / 5).to_string();
            out.push(parts[i].to_string());
            out.push("2".to_string());
            out.extend(parts[i + 2..=i + 4].iter().map(|part| scale(part)));
            i += 5;
        } else {
            // Bold and anything else unknown drop out.
            i += 1;
        }
    }
    if !out.iter().any(|part| part == "38") {
        out.extend(["38", "2", "70", "76", "92"].map(str::to_string));
    }
    format!("\u{1b}[{}m", out.join(";"))
}

fn cells_of(line: &str) -> Vec<Cell> {
    let mut style = String::new();
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            let mut seq = String::from("\u{1b}");
            for ch in chars.by_ref() {
                seq.push(ch);
                if ch == 'm' {
                    break;
                }
            }
            if seq == "\u{1b}[0m" || seq == "\u{1b}[m" {
                style.clear();
            } else {
                style = seq;
            }
            continue;
        }
        out.push(Cell {
            ch,
            style: style.clone(),
        });
    }
    out
}

fn paint_cells(row: &[Cell]) -> String {
    let mut out = String::new();
    let mut current = String::new();
    for cell in row {
        if cell.style != current {
            if !current.is_empty() {
                out.push_str("\u{1b}[0m");
            }
            out.push_str(&cell.style);
            current.clone_from(&cell.style);
        }
        out.push(cell.ch);
    }
    if !current.is_empty() {
        out.push_str("\u{1b}[0m");
    }
    out
}

/// Highest corner after rotation. A negative angle drops the top-left below this.
pub(super) fn world_top(key: &RawKey) -> f64 {
    let corners = [
        (key.x, key.y),
        (key.x + key.w, key.y),
        (key.x, key.y + key.h),
        (key.x + key.w, key.y + key.h),
    ];
    corners
        .into_iter()
        .map(|(x, y)| world_point(key, x, y).1)
        .fold(f64::INFINITY, f64::min)
}

/// Top-left of a key in world units. Rotation is around `(rotation_x, rotation_y)`.
pub(super) fn world_point(key: &RawKey, px: f64, py: f64) -> (f64, f64) {
    if key.rotation_angle == 0.0 {
        return (px, py);
    }
    let rad = key.rotation_angle.to_radians();
    let dx = px - key.rotation_x;
    let dy = py - key.rotation_y;
    let (sin, cos) = rad.sin_cos();
    (
        key.rotation_x + dx * cos - dy * sin,
        key.rotation_y + dx * sin + dy * cos,
    )
}

struct Cursor {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    decal: bool,
    angle: f64,
    rx: f64,
    ry: f64,
    cluster_x: f64,
    cluster_y: f64,
    /// KLE alignment. 4 is center-front, the editor default.
    align: usize,
}

fn parse_keymap(rows: &[Value]) -> Result<Vec<Vec<RawKey>>> {
    let mut cursor = Cursor {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        decal: false,
        angle: 0.0,
        rx: 0.0,
        ry: 0.0,
        cluster_x: 0.0,
        cluster_y: 0.0,
        align: 4,
    };
    let mut out = Vec::new();
    for row in rows {
        let Some(items) = row.as_array() else {
            return Err(Error::NoLayout);
        };
        let mut keys = Vec::new();
        for item in items {
            if let Some(text) = item.as_str() {
                keys.push(RawKey {
                    x: cursor.x,
                    y: cursor.y,
                    w: cursor.w,
                    h: cursor.h,
                    rotation_angle: cursor.angle,
                    rotation_x: cursor.rx,
                    rotation_y: cursor.ry,
                    labels: place_labels(text, cursor.align),
                    decal: cursor.decal,
                });
                // `x` and `y` in a metadata object are offsets from the cursor, and `w` applies once.
                cursor.x += cursor.w;
                cursor.w = 1.0;
                cursor.h = 1.0;
                cursor.decal = false;
            } else if let Some(obj) = item.as_object() {
                let rotates =
                    obj.contains_key("r") || obj.contains_key("rx") || obj.contains_key("ry");
                if rotates && !keys.is_empty() {
                    return Err(Error::NoLayout);
                }
                if let Some(angle) = number(obj.get("r"))? {
                    cursor.angle = angle;
                }
                if let Some(rx) = number(obj.get("rx"))? {
                    cursor.rx = rx;
                    cursor.cluster_x = rx;
                    cursor.x = cursor.cluster_x;
                    cursor.y = cursor.cluster_y;
                }
                if let Some(ry) = number(obj.get("ry"))? {
                    cursor.ry = ry;
                    cursor.cluster_y = ry;
                    cursor.x = cursor.cluster_x;
                    cursor.y = cursor.cluster_y;
                }
                if let Some(x) = number(obj.get("x"))? {
                    cursor.x += x;
                }
                if let Some(y) = number(obj.get("y"))? {
                    cursor.y += y;
                }
                if let Some(w) = number(obj.get("w"))? {
                    cursor.w = w;
                }
                if let Some(h) = number(obj.get("h"))? {
                    cursor.h = h;
                }
                if let Some(decal) = obj.get("d") {
                    cursor.decal = decal.as_bool().ok_or(Error::NoLayout)?;
                }
                if let Some(align) = obj.get("a") {
                    let align = align.as_u64().ok_or(Error::NoLayout)?;
                    if align > 7 {
                        return Err(Error::NoLayout);
                    }
                    cursor.align = align as usize;
                }
            } else {
                return Err(Error::NoLayout);
            }
        }
        out.push(keys);
        cursor.y += 1.0;
        cursor.x = cursor.rx;
    }
    Ok(out)
}

/// KLE serial order, indexed by alignment. `-1` drops that field.
/// Alignment 4 is the default. Field 3 lands in label 8, field 8 in label 1, field 9 in label 4.
const LABEL_MAP: [[i8; 12]; 8] = [
    [0, 6, 2, 8, 9, 11, 3, 5, 1, 4, 7, 10],
    [1, 7, -1, -1, 9, 11, 4, -1, -1, -1, -1, 10],
    [3, -1, 5, -1, 9, 11, -1, -1, 4, -1, -1, 10],
    [4, -1, -1, -1, 9, 11, -1, -1, -1, -1, -1, 10],
    [0, 6, 2, 8, 10, -1, 3, 5, 1, 4, 7, -1],
    [1, 7, -1, -1, 10, -1, 4, -1, -1, -1, -1, -1],
    [3, -1, 5, -1, 10, -1, -1, -1, 4, -1, -1, -1],
    [4, -1, -1, -1, 10, -1, -1, -1, -1, -1, -1, -1],
];

fn place_labels(text: &str, align: usize) -> [String; 12] {
    let mut labels = std::array::from_fn(|_| String::new());
    let map = LABEL_MAP[align.min(7)];
    for (index, part) in text.split('\n').take(12).enumerate() {
        if part.is_empty() {
            continue;
        }
        let dest = map[index];
        if dest < 0 {
            continue;
        }
        labels[dest as usize] = part.to_string();
    }
    labels
}

fn parse_labels(value: Option<&Value>) -> Result<Vec<LayoutGroup>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(groups) = value.as_array() else {
        return Err(Error::NoLayout);
    };
    let mut out = Vec::new();
    for group in groups {
        let Some(parts) = group.as_array() else {
            return Err(Error::NoLayout);
        };
        if parts.is_empty() {
            return Err(Error::NoLayout);
        }
        let name = parts[0].as_str().ok_or(Error::NoLayout)?.to_string();
        let mut choices = Vec::new();
        for choice in parts.iter().skip(1) {
            choices.push(choice.as_str().ok_or(Error::NoLayout)?.to_string());
        }
        out.push(LayoutGroup {
            _name: name,
            choices,
        });
    }
    Ok(out)
}

fn parse_custom(value: Option<&Value>) -> Vec<CustomCode> {
    let Some(list) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, item) in list.iter().enumerate() {
        let short = item
            .get("shortName")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty());
        let name = short
            .or_else(|| item.get("name").and_then(Value::as_str))
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let code = 0x7E00u16.saturating_add(index as u16);
        // Vial's `shortName` breaks a cap across lines, as in `Next\nBT`. Every painter here is one line.
        out.push(CustomCode {
            code,
            name: name.split_whitespace().collect::<Vec<_>>().join(" "),
        });
    }
    out
}

fn u8_field(value: &Value, path: &[&str]) -> Result<u8> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key).ok_or(Error::NoLayout)?;
    }
    let number = cursor.as_u64().ok_or(Error::NoLayout)?;
    u8::try_from(number).map_err(|_| Error::NoLayout)
}

fn number(value: Option<&Value>) -> Result<Option<f64>> {
    match value {
        None => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or(Error::NoLayout),
    }
}

fn pair_u8(text: &str) -> Option<(u8, u8)> {
    let (left, right) = text.split_once(',')?;
    Some((left.trim().parse().ok()?, right.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(legend: &str, x: f64, selected: bool, tone: Tone) -> Cap {
        Cap {
            place: None,
            legend: legend.to_string(),
            selected,
            unlock: false,
            changed: false,
            decal: false,
            tone,
            x,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }

    #[test]
    fn wide_legends_wrap_at_a_space() {
        assert_eq!(cap_lines("Sec Unlk", 6), ("Sec".into(), "Unlk".into()));
        assert_eq!(cap_lines("SH Tap Tog", 6), ("SH Tap".into(), "Tog".into()));
        assert_eq!(cap_lines("Bootloader", 6), ("Bootl".into(), "oader".into()));
    }

    #[test]
    fn long_name_stays_on_the_status_line() {
        let caps = [
            cap("Space", 0.0, true, Tone::Key),
            cap("MO(1)", 7.0, false, Tone::Layer),
        ];
        let frame = Frame {
            header: Header {
                name: "board".to_string(),
                tabs: Vec::new(),
                active: 0,
                locked: Some(false),
                unsaved: 0,
            },
            mode: Mode {
                label: "KEYMAP",
                tone: ModeTone::Browse,
            },
            hold: None,
            lower: None,
            popup: None,
            notice: None,
            footer: &[],
            cols: 40,
            rows: 8,
        };
        let text = render_board(1, 1, &caps, None, frame);
        assert!(text.contains("selected Space"), "{text}");
        assert!(text.contains('▌'), "{text}");
        assert!(text.contains("48;2;122;162;247"), "{text}");
        assert!(text.contains("Spa"), "{text}");
        assert!(text.contains("ce"), "{text}");
        assert!(
            text.lines()
                .any(|line| line.contains("48;2;36;58;78") && line.contains("MO")),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line.contains("48;2;36;58;78") && line.contains('1')),
            "{text}"
        );
    }
}
