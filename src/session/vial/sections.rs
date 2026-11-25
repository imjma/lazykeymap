//! The parts of a ZMK keymap file besides the layers, edited as sheets: combos, macros,
//! behaviors, conditional layers, and encoder bindings. A row is one devicetree node; a cell is
//! one of its properties, kept as the text the file has, so an untouched cell is written back
//! exactly as it was.

/// A section of the keymap file, in tab order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::session) enum Kind {
    Combos,
    Macros,
    Behaviors,
    Conditional,
    Encoders,
}

/// How a cell is written and edited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::session) enum Cell {
    /// One binding, `<&kp A>`, picked from the key list.
    Binding,
    /// Numbers or names between `< >`, typed: `28 29`, `(MOD_LGUI|MOD_LSFT)`.
    Cells,
    /// A quoted string, typed.
    Str,
    /// A property that is there or not.
    Flag,
}

pub(in crate::session) struct Column {
    /// The devicetree property.
    pub(in crate::session) name: &'static str,
    pub(in crate::session) cell: Cell,
}

/// One node. `cells` follow `Kind::columns`; an empty string is a property the node does not
/// have, and a flag that is set is `yes`. `tail` is the binding list after them, for kinds that
/// have one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::session) struct Record {
    /// Which node this was, in file order, when it was read; `None` for one added here.
    pub(in crate::session) id: Option<usize>,
    /// The node's label where it has one (what bindings use, as `&todo`), else its name.
    pub(in crate::session) name: String,
    pub(in crate::session) cells: Vec<String>,
    pub(in crate::session) tail: Vec<String>,
}

const fn column(name: &'static str, cell: Cell) -> Column {
    Column { name, cell }
}

const COMBOS: [Column; 6] = [
    column("bindings", Cell::Binding),
    column("key-positions", Cell::Cells),
    column("timeout-ms", Cell::Cells),
    column("layers", Cell::Cells),
    column("require-prior-idle-ms", Cell::Cells),
    column("slow-release", Cell::Flag),
];

const MACROS: [Column; 2] = [
    column("wait-ms", Cell::Cells),
    column("tap-ms", Cell::Cells),
];

const BEHAVIORS: [Column; 9] = [
    column("compatible", Cell::Str),
    column("#binding-cells", Cell::Cells),
    column("flavor", Cell::Str),
    column("tapping-term-ms", Cell::Cells),
    column("quick-tap-ms", Cell::Cells),
    column("require-prior-idle-ms", Cell::Cells),
    column("hold-trigger-key-positions", Cell::Cells),
    column("hold-trigger-on-release", Cell::Flag),
    column("mods", Cell::Cells),
];

const CONDITIONAL: [Column; 2] = [
    column("if-layers", Cell::Cells),
    column("then-layer", Cell::Cells),
];

impl Kind {
    pub(in crate::session) const ALL: [Kind; 5] = [
        Kind::Combos,
        Kind::Macros,
        Kind::Behaviors,
        Kind::Conditional,
        Kind::Encoders,
    ];

    pub(in crate::session) fn name(self) -> &'static str {
        match self {
            Kind::Combos => "Combos",
            Kind::Macros => "Macros",
            Kind::Behaviors => "Behaviors",
            Kind::Conditional => "Conditional",
            Kind::Encoders => "Encoders",
        }
    }

    /// The node the rows live under: `/ { combos { ... }; }`. Encoders are the layers.
    pub(in crate::session) fn node(self) -> &'static str {
        match self {
            Kind::Combos => "combos",
            Kind::Macros => "macros",
            Kind::Behaviors => "behaviors",
            Kind::Conditional => "conditional_layers",
            Kind::Encoders => "keymap",
        }
    }

    /// `compatible` of the section node, when this tool has to make one.
    pub(in crate::session) fn compatible(self) -> Option<&'static str> {
        match self {
            Kind::Combos => Some("zmk,combos"),
            Kind::Conditional => Some("zmk,conditional-layers"),
            _ => None,
        }
    }

    pub(in crate::session) fn columns(self) -> &'static [Column] {
        match self {
            Kind::Combos => &COMBOS,
            Kind::Macros => &MACROS,
            Kind::Behaviors => &BEHAVIORS,
            Kind::Conditional => &CONDITIONAL,
            Kind::Encoders => &[],
        }
    }

    /// The binding-list property after the columns, shown as a ragged row.
    pub(in crate::session) fn tail(self) -> Option<&'static str> {
        match self {
            Kind::Macros | Kind::Behaviors => Some("bindings"),
            Kind::Encoders => Some("sensor-bindings"),
            Kind::Combos | Kind::Conditional => None,
        }
    }

    /// Encoder rows are the layers, which `:layer-add` and `:layer-remove` own.
    pub(in crate::session) fn rows_fixed(self) -> bool {
        self == Kind::Encoders
    }

    /// Behaviors and macros are bound by label (`&todo`); combos and conditional layers are
    /// only ever named.
    pub(in crate::session) fn labelled(self) -> bool {
        matches!(self, Kind::Macros | Kind::Behaviors)
    }

    /// A new row named `name`, with what ZMK needs before it builds.
    pub(in crate::session) fn blank(self, name: &str) -> Record {
        let mut cells = vec![String::new(); self.columns().len()];
        let mut tail = Vec::new();
        match self {
            Kind::Combos => {
                cells[0] = "&none".to_string();
                cells[2] = "50".to_string();
            }
            Kind::Behaviors => {
                cells[0] = "zmk,behavior-hold-tap".to_string();
                cells[1] = "2".to_string();
                cells[2] = "balanced".to_string();
                cells[3] = "200".to_string();
                tail = vec!["&kp".to_string(), "&kp".to_string()];
            }
            Kind::Macros | Kind::Conditional | Kind::Encoders => {}
        }
        Record {
            id: None,
            name: name.to_string(),
            cells,
            tail,
        }
    }
}

/// Where a sheet column lands in a record: the name, a property, a tail binding, or the slot
/// after the tail that appends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::session) enum Slot {
    Name,
    Column(usize),
    Tail(usize),
    Append,
}

/// The slot under sheet column `col` of `record`.
pub(in crate::session) fn slot(kind: Kind, record: &Record, col: usize) -> Slot {
    let columns = kind.columns().len();
    match col {
        0 => Slot::Name,
        c if c <= columns => Slot::Column(c - 1),
        c if kind.tail().is_some() && c - 1 - columns < record.tail.len() => {
            Slot::Tail(c - 1 - columns)
        }
        _ => Slot::Append,
    }
}

/// Sheet columns of `record`: the name, each property, then the tail and its append slot.
pub(in crate::session) fn width(kind: Kind, record: &Record) -> usize {
    1 + kind.columns().len()
        + match kind.tail() {
            Some(_) => record.tail.len() + 1,
            None => 0,
        }
}
