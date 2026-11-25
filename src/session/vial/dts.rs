//! Devicetree source for ZMK keymaps, with source spans so an edit rewrites only what changed.

use std::ops::Range;

use crate::{Error, Result};

/// A parsed `.keymap`; every span indexes into `text`.
#[derive(Clone, Debug)]
pub(super) struct Doc {
    pub(super) text: String,
    /// Children of every `/ { ... };` root plus top-level `&label { ... };` overlays, in file order.
    pub(super) nodes: Vec<Node>,
    pub(super) defines: Vec<Define>,
}

#[derive(Clone, Debug)]
pub(super) struct Define {
    pub(super) name: String,
    /// `Some` for function-like `#define F(a,b)`.
    pub(super) params: Option<Vec<String>>,
    /// Rest of the line(s): continuations joined with a space, trailing `//` comment stripped, trimmed.
    pub(super) body: String,
    /// Where `body` is in the file. `None` when it spans continued lines.
    pub(super) body_span: Option<Range<usize>>,
}

#[derive(Clone, Debug)]
pub(super) struct Node {
    pub(super) label: Option<String>,
    pub(super) name: String,
    /// First byte of the label or name through the closing `};`.
    pub(super) span: Range<usize>,
    /// `span` widened to whole lines (incl. a trailing `//` comment and the newline).
    pub(super) line_span: Range<usize>,
    /// Whitespace before the node on its first line.
    pub(super) indent: String,
    /// Just after `{` to just before `}`.
    pub(super) body: Range<usize>,
    pub(super) props: Vec<Prop>,
    pub(super) children: Vec<Node>,
}

#[derive(Clone, Debug)]
pub(super) struct Prop {
    pub(super) name: String,
    /// Name through the `;`.
    pub(super) span: Range<usize>,
    pub(super) line_span: Range<usize>,
    /// `None` for a boolean flag property.
    pub(super) value: Option<Value>,
}

#[derive(Clone, Debug)]
pub(super) struct Value {
    pub(super) parts: Vec<Part>,
}

#[derive(Clone, Debug)]
pub(super) enum Part {
    /// The tokens between `<` and `>`.
    Cells { tokens: Vec<Token> },
    /// `text` is the raw (unescaped) contents between the quotes.
    Str { text: String },
    /// Anything else, as a `&label` phandle or a bare number; nothing here reads it.
    Other,
}

#[derive(Clone, Debug)]
pub(super) struct Token {
    pub(super) text: String,
    pub(super) span: Range<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct Edit {
    pub(super) range: Range<usize>,
    pub(super) text: String,
}

impl Doc {
    pub(super) fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

impl Node {
    #[cfg(test)]
    pub(super) fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|n| n.name == name)
    }

    pub(super) fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
}

impl Prop {
    /// Every token of every `Cells` part, in order.
    pub(super) fn cells(&self) -> Vec<&Token> {
        let Some(value) = &self.value else {
            return Vec::new();
        };
        value
            .parts
            .iter()
            .flat_map(|p| match p {
                Part::Cells { tokens, .. } => tokens.as_slice(),
                _ => &[],
            })
            .collect()
    }
}

/// Groups cell tokens into ZMK bindings: each starts at an `&` token and runs to the
/// last token before the next one. Tokens before the first `&` are ignored.
pub(super) fn bindings(tokens: &[&Token]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut open = false;
    for t in tokens {
        if t.text.starts_with('&') {
            out.push(t.span.clone());
            open = true;
        } else if open {
            out.last_mut().unwrap().end = t.span.end;
        }
    }
    out
}

/// Applies non-overlapping edits, in any order.
pub(super) fn apply(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    debug_assert!(edits.windows(2).all(|w| w[0].range.end <= w[1].range.start));
    let mut out = text.to_string();
    for e in edits.iter().rev() {
        out.replace_range(e.range.clone(), &e.text);
    }
    out
}

pub(super) fn parse(text: &str) -> Result<Doc> {
    let mut p = Parser {
        s: text.as_bytes(),
        pos: 0,
        defines: Vec::new(),
    };
    let mut nodes = Vec::new();
    loop {
        p.skip();
        match p.peek() {
            None => break,
            Some(b'/') => {
                p.pos += 1;
                p.skip();
                p.expect(b'{')?;
                let (_, mut children, _) = p.body()?;
                p.close()?;
                nodes.append(&mut children);
            }
            Some(b'&') => nodes.push(p.node()?),
            Some(_) => return Err(Error::Protocol("keymap: unexpected top-level text")),
        }
    }
    Ok(Doc {
        text: text.to_string(),
        nodes,
        defines: p.defines,
    })
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    defines: Vec<Define>,
}

const DIRECTIVES: &[&str] = &[
    "include", "define", "undef", "if", "ifdef", "ifndef", "elif", "else", "endif", "pragma",
    "error", "warning", "line",
];

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"_-,#@.&+?".contains(&c)
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn str(&self, r: Range<usize>) -> String {
        String::from_utf8_lossy(&self.s[r]).into_owned()
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        if self.peek() != Some(c) {
            return Err(Error::Protocol("keymap: unexpected character"));
        }
        self.pos += 1;
        Ok(())
    }

    /// Consumes `}` `;`.
    fn close(&mut self) -> Result<()> {
        self.expect(b'}')?;
        self.skip();
        self.expect(b';')
    }

    fn line_end(&self, from: usize) -> usize {
        self.s[from..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(self.s.len(), |i| from + i)
    }

    /// Skips whitespace, comments and preprocessor lines (collecting `#define`s).
    fn skip(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else if self.s[self.pos..].starts_with(b"//") {
                self.pos = self.line_end(self.pos);
            } else if self.s[self.pos..].starts_with(b"/*") {
                self.pos = self.s[self.pos + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(self.s.len(), |i| self.pos + 4 + i);
            } else if c == b'#' && self.directive().is_some() {
                self.preprocessor();
            } else {
                break;
            }
        }
    }

    /// The directive word after `#`, if this `#` starts a preprocessor line.
    fn directive(&self) -> Option<&str> {
        let mut i = self.pos + 1;
        while matches!(self.s.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        let start = i;
        while self.s.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
            i += 1;
        }
        let word = std::str::from_utf8(&self.s[start..i]).ok()?;
        let ends = !self.s.get(i).is_some_and(|&c| is_ident(c));
        (ends && DIRECTIVES.contains(&word)).then_some(word)
    }

    fn preprocessor(&mut self) {
        let is_define = self.directive() == Some("define");
        let start = self.pos;
        let mut end = self.line_end(start);
        while end > start && self.s[..end].ends_with(b"\\") && end < self.s.len() {
            end = self.line_end(end + 1);
        }
        self.pos = end;
        if !is_define {
            return;
        }
        let line = self
            .str(start..end)
            .replace("\\\r\n", " ")
            .replace("\\\n", " ");
        let rest = line.trim_start_matches('#').trim_start();
        let rest = rest["define".len()..].trim_start();
        let name_len = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let (name, mut rest) = rest.split_at(name_len);
        let mut params = None;
        if let Some(r) = rest.strip_prefix('(') {
            let close = r.find(')').unwrap_or(r.len());
            params = Some(
                r[..close]
                    .split(',')
                    .map(|a| a.trim().to_string())
                    .collect(),
            );
            rest = r.get(close + 1..).unwrap_or("");
        }
        let body = rest.find("//").map_or(rest, |i| &rest[..i]).trim();
        // `line` is the source as written unless continuations were joined.
        let raw = self.str(start..end);
        let body_span = (raw == line).then(|| {
            let at = start + (body.as_ptr() as usize - line.as_ptr() as usize);
            at..at + body.len()
        });
        self.defines.push(Define {
            name: name.to_string(),
            params,
            body: body.to_string(),
            body_span,
        });
    }

    fn ident(&mut self) -> Result<Range<usize>> {
        let start = self.pos;
        while self.peek().is_some_and(is_ident) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(Error::Protocol("keymap: expected a name"));
        }
        Ok(start..self.pos)
    }

    /// Widens `span` to whole lines when only whitespace (and a trailing `//` comment) surrounds it.
    fn line_span(&self, span: &Range<usize>) -> (Range<usize>, String) {
        let line_start = self.s[..span.start]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |i| i + 1);
        let lead = &self.s[line_start..span.start];
        let alone = lead.iter().all(|c| matches!(c, b' ' | b'\t'));
        let start = if alone { line_start } else { span.start };
        let indent = if alone {
            self.str(line_start..span.start)
        } else {
            String::new()
        };
        let eol = self.line_end(span.end);
        let tail = std::str::from_utf8(&self.s[span.end..eol])
            .unwrap_or("x")
            .trim();
        let end = if tail.is_empty() || tail.starts_with("//") {
            (eol + 1).min(self.s.len())
        } else {
            span.end
        };
        (start..end, indent)
    }

    /// Parses props and child nodes up to (not consuming) `}`.
    fn body(&mut self) -> Result<(Range<usize>, Vec<Node>, Vec<Prop>)> {
        let start = self.pos;
        let (mut nodes, mut props) = (Vec::new(), Vec::new());
        loop {
            self.skip();
            match self.peek() {
                None => return Err(Error::Protocol("keymap: unclosed node")),
                Some(b'}') => return Ok((start..self.pos, nodes, props)),
                Some(_) => {}
            }
            let item_start = self.pos;
            let name = self.ident()?;
            self.skip();
            let after_name = self.pos;
            match self.peek() {
                Some(b'{' | b':') => {
                    self.pos = item_start;
                    nodes.push(self.node()?);
                }
                Some(b';') => {
                    self.pos += 1;
                    props.push(self.prop(name, item_start, None));
                }
                Some(b'=') => {
                    self.pos += 1;
                    let value = self.value()?;
                    props.push(self.prop(name, item_start, Some(value)));
                }
                _ => {
                    self.pos = after_name;
                    return Err(Error::Protocol("keymap: expected `{`, `=` or `;`"));
                }
            }
        }
    }

    fn prop(&self, name: Range<usize>, start: usize, value: Option<Value>) -> Prop {
        let span = start..self.pos;
        let (line_span, _) = self.line_span(&span);
        Prop {
            name: self.str(name),
            span,
            line_span,
            value,
        }
    }

    fn node(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut name = self.ident()?;
        self.skip();
        let mut label = None;
        if self.peek() == Some(b':') {
            self.pos += 1;
            self.skip();
            label = Some(self.str(name));
            name = self.ident()?;
            self.skip();
        }
        self.expect(b'{')?;
        let (body, children, props) = self.body()?;
        self.close()?;
        let span = start..self.pos;
        let (line_span, indent) = self.line_span(&span);
        Ok(Node {
            label,
            name: self.str(name),
            span,
            line_span,
            indent,
            body,
            props,
            children,
        })
    }

    /// Parses comma-separated parts through the `;`.
    fn value(&mut self) -> Result<Value> {
        let mut parts = Vec::new();
        self.skip();
        loop {
            self.skip();
            match self.peek() {
                Some(b'<') => {
                    self.pos += 1;
                    parts.push(self.cells()?);
                }
                Some(b'"') => {
                    self.pos += 1;
                    let s = self.pos;
                    while let Some(c) = self.peek() {
                        match c {
                            b'"' => break,
                            b'\\' => self.pos += 2,
                            _ => self.pos += 1,
                        }
                    }
                    self.expect(b'"')?;
                    parts.push(Part::Str {
                        text: self.str(s..self.pos - 1),
                    });
                }
                Some(_) => {
                    while self.peek().is_some_and(|c| c != b',' && c != b';') {
                        self.pos += 1;
                    }
                    parts.push(Part::Other);
                }
                None => return Err(Error::Protocol("keymap: unterminated property")),
            }
            self.skip();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b';') => {
                    self.pos += 1;
                    return Ok(Value { parts });
                }
                _ => return Err(Error::Protocol("keymap: expected `,` or `;`")),
            }
        }
    }

    /// Tokenizes a cell list after `<` through `>`; parentheses nest within a token.
    fn cells(&mut self) -> Result<Part> {
        let mut tokens = Vec::new();
        loop {
            self.skip();
            match self.peek() {
                None => return Err(Error::Protocol("keymap: unclosed `<`")),
                Some(b'>') => {
                    self.pos += 1;
                    return Ok(Part::Cells { tokens });
                }
                Some(_) => {}
            }
            let s = self.pos;
            let mut depth = 0usize;
            while let Some(c) = self.peek() {
                match c {
                    b'(' => depth += 1,
                    b')' => depth = depth.saturating_sub(1),
                    b'>' if depth == 0 => break,
                    c if depth == 0 && c.is_ascii_whitespace() => break,
                    _ => {}
                }
                self.pos += 1;
            }
            tokens.push(Token {
                text: self.str(s..self.pos),
                span: s..self.pos,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORNE: &str = include_str!("../../../tests/fixtures/corne.keymap");

    fn layer_bindings(doc: &Doc, layer: &str) -> Vec<String> {
        let prop = doc
            .node("keymap")
            .unwrap()
            .child(layer)
            .unwrap()
            .prop("bindings")
            .unwrap();
        bindings(&prop.cells())
            .into_iter()
            .map(|r| doc.text[r].to_string())
            .collect()
    }

    #[test]
    fn parses_corne_structure() {
        let doc = parse(CORNE).unwrap();
        for name in [
            "behaviors",
            "combos",
            "macros",
            "keymap",
            "conditional_layers",
        ] {
            assert!(doc.node(name).is_some(), "{name}");
        }
        let layers: Vec<_> = doc
            .node("keymap")
            .unwrap()
            .children
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(
            layers,
            [
                "default_layer",
                "SYM",
                "NUM_NAV",
                "BT_FUNC",
                "layer_mouse",
                "ColmakDH",
                "numpad"
            ]
        );
        let name = doc
            .node("keymap")
            .unwrap()
            .child("default_layer")
            .unwrap()
            .prop("display-name");
        let Some(Value { parts, .. }) = &name.unwrap().value else {
            panic!()
        };
        assert!(matches!(&parts[..], [Part::Str { text, .. }] if text == "Base"));
    }

    #[test]
    fn groups_bindings() {
        let doc = parse(CORNE).unwrap();
        let b = layer_bindings(&doc, "default_layer");
        assert_eq!(b.len(), 42);
        assert_eq!(b[0], "&kp TAB");
        assert_eq!(b[12], "&thm CAPS ESC");
        assert_eq!(b[28], "&thm LG(V) V");
        assert!(b.contains(&"&lt 6 SPACE".to_string()));
        assert_eq!(layer_bindings(&doc, "BT_FUNC").len(), 42);
    }

    #[test]
    fn parses_behavior_and_macro() {
        let doc = parse(CORNE).unwrap();
        let thm = &doc.node("behaviors").unwrap().children[0];
        assert_eq!(thm.label.as_deref(), Some("thm"));
        assert_eq!(thm.name, "timerless_homerow_mods");
        assert_eq!(thm.prop("#binding-cells").unwrap().cells()[0].text, "2");
        assert!(thm.prop("hold-trigger-on-release").unwrap().value.is_none());
        let parts = &thm.prop("bindings").unwrap().value.as_ref().unwrap().parts;
        assert_eq!(
            parts
                .iter()
                .filter(|p| matches!(p, Part::Cells { .. }))
                .count(),
            2
        );

        let m = doc.node("macros").unwrap().child("colemak_layer").unwrap();
        let b = m.prop("bindings").unwrap();
        assert_eq!(b.value.as_ref().unwrap().parts.len(), 7);
        assert!(b.cells().iter().any(|t| t.text == "MACRO_PLACEHOLDER"));
        let combo = doc.node("combos").unwrap().child("combo_tab_prev").unwrap();
        assert_eq!(
            combo.prop("bindings").unwrap().cells()[1].text,
            "LC(LS(TAB))"
        );
    }

    #[test]
    fn collects_defines() {
        let doc = parse(CORNE).unwrap();
        let hrml = doc.defines.iter().find(|d| d.name == "HRML").unwrap();
        assert_eq!(hrml.params.as_deref().unwrap(), ["k1", "k2", "k3", "k4"]);
        assert!(hrml.body.starts_with("&thm LCTRL k1"));
        let mv = doc
            .defines
            .iter()
            .find(|d| d.name == "ZMK_POINTING_DEFAULT_MOVE_VAL")
            .unwrap();
        assert_eq!((mv.params.as_ref(), mv.body.as_str()), (None, "2500"));
        assert_eq!(&CORNE[hrml.body_span.clone().unwrap()], hrml.body);
    }

    #[test]
    fn splices_one_binding() {
        let doc = parse(CORNE).unwrap();
        let prop = doc
            .node("keymap")
            .unwrap()
            .child("default_layer")
            .unwrap()
            .prop("bindings")
            .unwrap();
        let range = bindings(&prop.cells())[1].clone();
        assert_eq!(&CORNE[range.clone()], "&kp Q");
        let out = apply(
            CORNE,
            vec![Edit {
                range: range.clone(),
                text: "&kp A".into(),
            }],
        );
        assert_eq!(out[..range.start], CORNE[..range.start]);
        assert_eq!(out[range.end..], CORNE[range.end..]);
        assert_eq!(
            layer_bindings(&parse(&out).unwrap(), "default_layer")[1],
            "&kp A"
        );
    }

    #[test]
    fn deletes_a_node() {
        let doc = parse(CORNE).unwrap();
        let combos = doc.node("combos").unwrap();
        let victim = combos.child("combo_pipe").unwrap();
        assert_eq!(victim.indent, "        ");
        let out = apply(
            CORNE,
            vec![Edit {
                range: victim.line_span.clone(),
                text: String::new(),
            }],
        );
        let after = parse(&out).unwrap();
        let left = after.node("combos").unwrap();
        assert_eq!(left.children.len(), combos.children.len() - 1);
        assert!(left.child("combo_pipe").is_none());
    }

    #[test]
    fn parses_overlay_and_tricky_comments() {
        let src = "#include <x.h>\n#define F(a) \\\n  &kp a // c\n/* { not a node */\n&kp { flavor = \"x\"; };\n/ { a { b = <1>, <F(2)>; c; }; };\n";
        let doc = parse(src).unwrap();
        assert_eq!(doc.nodes[0].name, "&kp");
        assert_eq!(doc.nodes[1].name, "a");
        assert_eq!(doc.defines[0].body, "&kp a");
        assert_eq!(doc.nodes[1].prop("b").unwrap().cells()[1].text, "F(2)");
    }
}
