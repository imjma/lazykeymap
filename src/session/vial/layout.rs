use crate::{Error, Result};
use serde_json::Value;

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
    pub(super) labels: [String; 12],
    pub(super) decal: bool,
}

pub(super) struct CustomCode {
    pub(super) code: u16,
    pub(super) name: String,
}

pub(super) struct Cap {
    /// KLE row. `y` is the key's position, so the row cannot be recovered from it.
    pub(super) row: usize,
    /// The token rule reads `selected`. The place stays so the cap is still the key.
    #[allow(dead_code)]
    pub(super) place: Option<Place>,
    pub(super) legend: String,
    pub(super) selected: bool,
    pub(super) decal: bool,
    pub(super) x: f64,
    /// Column start is `round(x * 4)`. Height and width stay on the cap with `y`.
    #[allow(dead_code)]
    pub(super) y: f64,
    #[allow(dead_code)]
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

pub(super) fn render_board(
    name: &str,
    layer_one_based: u8,
    layer_count: u8,
    locked: bool,
    caps: &[Cap],
    hold: Option<&str>,
    query: Option<&str>,
    hits: &[String],
    cursor: usize,
    cols: u16,
    rows: u16,
) -> String {
    let state = if locked { "locked" } else { "ready" };
    let mut lines = vec![format!(
        "layer {layer_one_based}/{layer_count} {state} {name}"
    )];
    if let Some(hold) = hold {
        lines.push(format!("hold {hold}"));
    }
    if let Some(query) = query {
        lines.push(format!("search {query}"));
        for (i, hit) in hits.iter().enumerate() {
            if i == cursor {
                lines.push(format!("> {hit}"));
            } else {
                lines.push(hit.clone());
            }
        }
    }
    lines.extend(paint_rows(caps, cols));
    clip(&lines, cols, rows)
}

pub(super) fn render_pick(
    names: &[String],
    cursor: usize,
    failed: Option<usize>,
    cols: u16,
    rows: u16,
) -> String {
    let mut lines = vec!["choose".to_string()];
    for (i, name) in names.iter().enumerate() {
        let mark = if i == cursor { "> " } else { "" };
        let bad = if failed == Some(i) {
            " unavailable"
        } else {
            ""
        };
        lines.push(format!("{mark}{name}{bad}"));
    }
    clip(&lines, cols, rows)
}

fn paint_rows(caps: &[Cap], cols: u16) -> Vec<String> {
    let Some(last) = caps.iter().map(|cap| cap.row).max() else {
        return Vec::new();
    };
    let mut grouped = vec![Vec::new(); last + 1];
    for cap in caps {
        grouped[cap.row].push(cap);
    }
    grouped.iter().map(|row| paint_row(row, cols)).collect()
}

fn paint_row(caps: &[&Cap], cols: u16) -> String {
    let width = cols as usize;
    let mut ordered: Vec<&Cap> = caps.to_vec();
    ordered.sort_by(|a, b| a.x.total_cmp(&b.x));
    let mut line = String::new();
    let mut occupied = 0usize;
    for cap in ordered {
        let mut column = (cap.x * 4.0).round() as i64;
        if column < 0 {
            column = 0;
        }
        let mut column = column as usize;
        if column < occupied {
            column = occupied + 2;
        }
        if width == 0 || column >= width {
            continue;
        }
        let token = token(cap);
        let room = width - column;
        let token: String = token.chars().take(room).collect();
        while line.chars().count() < column {
            line.push(' ');
        }
        line.push_str(&token);
        occupied = column + token.chars().count();
    }
    line.chars().take(width).collect()
}

fn token(cap: &Cap) -> String {
    if cap.decal {
        format!("({})", cap.legend)
    } else if cap.selected {
        format!("[{}]", cap.legend)
    } else {
        cap.legend.clone()
    }
}

fn clip(lines: &[String], cols: u16, rows: u16) -> String {
    lines
        .iter()
        .take(rows as usize)
        .map(|line| line.chars().take(cols as usize).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

struct Cursor {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    decal: bool,
}

fn parse_keymap(rows: &[Value]) -> Result<Vec<Vec<RawKey>>> {
    let mut cursor = Cursor {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        decal: false,
    };
    let mut out = Vec::new();
    for row in rows {
        let Some(items) = row.as_array() else {
            return Err(Error::NoLayout);
        };
        cursor.x = 0.0;
        cursor.w = 1.0;
        cursor.h = 1.0;
        cursor.decal = false;
        let mut keys = Vec::new();
        for item in items {
            if let Some(text) = item.as_str() {
                keys.push(RawKey {
                    x: cursor.x,
                    y: cursor.y,
                    w: cursor.w,
                    h: cursor.h,
                    labels: split_labels(text),
                    decal: cursor.decal,
                });
                // `x` and `y` in a metadata object are offsets from the cursor, and `w` applies once.
                cursor.x += cursor.w;
                cursor.w = 1.0;
                cursor.h = 1.0;
                cursor.decal = false;
            } else if let Some(obj) = item.as_object() {
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
            } else {
                return Err(Error::NoLayout);
            }
        }
        out.push(keys);
        cursor.y += 1.0;
    }
    Ok(out)
}

fn split_labels(text: &str) -> [String; 12] {
    let mut labels = std::array::from_fn(|_| String::new());
    for (i, part) in text.split('\n').take(12).enumerate() {
        labels[i] = part.to_string();
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
        out.push(CustomCode {
            code,
            name: name.to_string(),
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
