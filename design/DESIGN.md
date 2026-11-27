# lazykeymap

## Problem

lazykeymap is a terminal program that remaps a plugged-in Vial keyboard. The repo has no application code. The grounding file is the constraint set. A keycode is a `u16`. Matrix keys live in one row-major buffer of `layers * rows * cols` codes, big-endian on the wire. Encoder directions live in a separate get and set under the `0xFE` prefix. Firmware starts locked. A locked board still takes key, encoder, tap dance, and combo writes; it only stores `QK_BOOT` as `KC_NO` and skips macro buffer writes (vial-qmk `vial_keycode_firewall` and `via.c`). The grounding file's "writes while locked do not stick" was wrong. The drawing is the KLE keymap inside the LZMA definition, minus keys the current layout-option dword hides. A board with no definition is a failure, and the program must not invent a grid. Edits are staged in a draft and reach the keyboard on `:w`. Quit asks when the draft has changes, and otherwise leaves the keyboard as last written.

Tap dance, combo, and macro editing are in, as tabs beside the keymap (see Editors). Lighting, key overrides, alt repeat keys, QMK settings, layout-option edits, keymap import and export, and firmware flashing are out of this tool. The type layout has to make each of those a new private codec plus a new caller, so a later edit cannot grow them by adding a match arm the screen already touches.

## Usage (caller's view)

The library call is `App`. The caller passes events and reads the painted string. `attach` opens the only Vial board, or returns a chooser when several answer. `MemoryBoard` is the same `Attached` value with no USB device. `Terminal` belongs to the binary. The library never sees a tty event.

```rust
use lazykeymap::{App, Step};

fn main() -> lazykeymap::Result<()> {
    let mut app = App::new(lazykeymap::attach()?, Terminal::open()?);
    loop {
        if app.handle(Terminal::event()?)? == Step::Quit {
            return Ok(());
        }
    }
}
```

On the map, `h` `j` `k` `l` and the arrow keys move the selection. `g` jumps to the first key. `G` jumps to the last. `[` and `]` wrap the layer. `/` and `Confirm` open search. A letter with no binding does nothing. `q` quits, and asks first when changes are unsaved (see Draft and write). `u` undoes the last staged change. `:` or `;` opens the command line. `?` opens a help box over the current screen. The board stays visible around it. The box title is `help`. The list names board, search, choose, hold, and help, then every shortcut in those groups. The group you were in is first. `j` and `k` scroll the list inside the box. `q` on that screen closes it and stays in the program. `Quit` (Ctrl-C) quits from every view, with the same unsaved-changes question.

In search, characters append to the query, including `j`, `k`, and `q`. `?` opens the same list. Up and Down move the highlighted hit. Left and Right switch shelves and clear the query. `Backspace` edits the query. `Confirm` assigns that hit. `Cancel` closes search.

`Cancel` also abandons a hold without a write, and does nothing on the map. `Tick` polls a hold. Quit sends no lock command. On the chooser, `j` and `k` move, the arrow keys move, `Confirm` opens the highlighted board, and `q` quits.

The painted string is the contract. The screen is five bands, top to bottom: the header bar, the main panel, the lower panel when search or a field is open, the status bar, and the options bar.

- The header bar is one filled line: a ` lazykeymap ` brand chip, the board name, the tabs (`L0`.. then `Tap Dance`, `Combos`, `Macros`, the active one filled), and at the right `● N unsaved` when the draft differs from the keyboard and `● locked` or `● ready`. On a narrow screen the tabs farthest from the active one drop first, marked `…`. The chooser's header says `choose a keyboard` and has no tabs or state.
- The chooser shows each board as a two-line card, centered and at most 64 columns wide: the product name with the manufacturer at the right, then `vid:pid · serial` from HID enumeration, so two boards with the same name still differ. The highlighted card is lighter and carries an accent bar `▌`. A board that opens without a Vial layout shows `✕ no Vial layout` in place of the manufacturer and stays on the list.
- The main panel is a rounded box titled `layer {n}/{count}`, with `n` starting at 0 as QMK counts, or `tap dance 1/4`, `combos 1/2`, `macros 1/4` with the cursor row. The keycaps sit centered in it. During a hold the keys the unlock names are painted amber.
- Under the keycaps, when the panel has rows to spare, the selected key's details: a rule, then the legend chip, its QMK name when that says more than the legend (`KC_ESCAPE`), the code (`0x0029`), and what it does (`basic key`, `mod-tap: hold for the modifier, tap for the key`, `layer 1 while held`; QMK, the legends, and the tabs all count layers from 0). Then `on each layer  L0 Esc  L1 NO ...`, the same key on every layer with this layer filled and staged ones green, when the board has more than one layer. Then, when this key is staged, `unsaved: the keyboard still has {saved}, :w writes {draft}`. The details take spare rows only, so a short screen or an open search drops them before anything else.
- The status bar starts with a mode badge (`KEYMAP`, `TAP DANCE`, `SEARCH`, `EDIT`, `COMMAND`, `CONFIRM`, `UNLOCK`, `CHOOSE`): blue while browsing, purple while typing, amber while the keyboard or a question waits. After it comes, in order of urgency, the unlock prompt `hold {row},{col}+{row},{col} to write {what}` with its countdown bar (pairs in status-report order, `255,255` omitted), a notice (amber), or the usual status: `selected {legend}  ·  key {row},{col}` (or `encoder {index} cw|ccw`), with `·  unsaved` when that key is staged, or the sheet cell under the cursor. `? help` sits at the right.
- The options bar is key chips (` h ` on a key-colored chip, then what it does). Hints that share a label merge, as in `ctrl-u/ctrl-d page`, and hints that do not fit drop off whole.

Rows go to the key lines first, then the status bar, so the unlock prompt survives the smallest screen, then the options bar, the header, the lower panel, and the borders. A short screen drops the boxes before it drops a key line. Search, the key list, is a panel under the board, not the find-anything popup, so the key being changed stays on screen: as the highlight moves, that key draws the highlighted keycode in amber and the status bar reads `NO → C  ·  enter to stage`, so the change shows before Enter (`Lower::Search::preview`, after a `(kc)` pick only once a basic key fills it). Esc drops the preview with nothing staged; one Enter stages the pick. The panel's first line is the query, as in the find list: `› esc▏`, or `› ▏ type to search every group` before anything is typed. Its hits run along a row, then onto the next, as text reads: Left and Right step to the neighbour, Up and Down move a whole row (`layout::search_across`, which the drawing and the keys share, so Down lands under the cursor), Ctrl-U and Ctrl-D four rows. Tab and Shift-Tab switch the group. Each hit shows its QMK name beside the legend when the two differ. A query with no hits shows `no match`. Esc closes the list, and so does Ctrl-[, which every terminal sends as Esc, and Backspace with nothing typed, so a keyboard with no Esc key can still leave it. Typing `esc` finds the Esc keycode itself. Help is a popup drawn over that picture, not a panel in the stack. An open search title brackets the active shelf, as in `[Basic] ISO/JIS Layers Quantum Backlight ...`. A query makes it `search {query}`. The highlighted hit is prefixed with `> `. The chooser is one panel titled `keyboards`, one board name per line, the highlighted name prefixed with `> `, and its status bar counts the boards found. A confirm that cannot load a definition leaves the list up and appends ` unavailable` to that name. A decal is wrapped in `()`. A hidden key is absent. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. Lines are clipped to the screen size and are not wrapped.

The Basic group is drawn as vial-gui draws it: an ANSI keyboard (`layout::ANSI`, keys in quarter units with a cluster tag), the widest of 100%, 80% (no numpad), or 70% (no navigation block) that fits the panel at five columns a unit (`basic_plan`). A key the board does not store is drawn dim and is not a stop; a legend wider than its key shows its last word (`Num 7` is `7`). The hits with no key on the picture (`NO`, `TRNS`, F13 and up, the shifted symbols, and the numpad or navigation keys a narrow picture dropped) follow under it as caps too, one width wide enough for the longest legend, as vial-gui's buttons beside its keyboard (`rest_caps`). The cursor is still an index into the group's hits, and opening the list on a key starts it on that key's keycode when the Basic group has it (`searching`), so the key's own key is lit. On the keyboard, Left and Right step along the row and Up and Down go to the nearest key centre in the next row (`basic_step`, which plans the same picture the drawing does); Down from the bottom row enters the caps under it, which step as a grid, and Up from their first row comes back to the bottom row. The ISO/JIS group stays a grid.

Search ranking is case-insensitive and goes by the key before its QMK name: a legend equal to the query, then a legend that starts with it, then a QMK name equal to it or starting with it, read without its `KC_` or `QK_` (every QMK name starts with `KC_`, so `k` would otherwise match every key alike), then a substring of either. `k` lists `K` first, then `KO On`, `KB Mute`; `kc_k` and `kp_1` still find `KC_K` and `KC_KP_1`. A shorter legend wins a tie. The remaining order is the catalog, tab by tab. `Confirm` assigns the highlighted hit. The fixture below has its first selectable key under the cursor, and a key whose legend is `Hid` with label 8 equal to `0,1`. The default layout-option dword is 0, which selects choice 0 of each group, so `Hid` is omitted.

```rust
fn remap_persists() -> lazykeymap::Result<()> {
    let board = lazykeymap::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .locked(&[(0, 0), (0, 1)]);
    let mut app = lazykeymap::App::new(board.open()?, lazykeymap::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("locked"));
    assert!(app.screen().text().contains("[NO]"));
    assert!(!app.screen().text().contains("Hid"));
    app.handle(lazykeymap::Event::Char('/'))?;
    app.handle(lazykeymap::Event::Char('e'))?;
    app.handle(lazykeymap::Event::Char('s'))?;
    app.handle(lazykeymap::Event::Char('c'))?;
    app.handle(lazykeymap::Event::Confirm)?;
    assert!(app.screen().text().contains("NO → Esc"));
    app.handle(lazykeymap::Event::Confirm)?;
    for ch in ":w".chars() {
        app.handle(lazykeymap::Event::Char(ch))?;
    }
    app.handle(lazykeymap::Event::Confirm)?;
    assert!(app.screen().text().contains("[Esc]"));
    drop(app);
    let mut app = lazykeymap::App::new(board.open()?, lazykeymap::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("[Esc]"));
    Ok(())
}
```

A second `open` while the first `App` still holds the board returns `Error::Busy`. Drop releases it. The second `open` reads the stored codes through the same command codec as the first, so a paint-only cache cannot satisfy the last assertion.

```rust
fn pick_when_two() -> lazykeymap::Result<()> {
    let alpha = lazykeymap::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .named("alpha");
    let beta = lazykeymap::MemoryBoard::from_definition(include_str!("fixtures/other.json"))?
        .named("beta");
    let mut app = lazykeymap::App::new(
        lazykeymap::MemoryBoard::choose(&[alpha, beta])?,
        lazykeymap::TestScreen::new(40, 12),
    );
    assert!(app.screen().text().contains("choose"));
    assert!(app.screen().text().contains("> alpha"));
    app.handle(lazykeymap::Event::Char('j'))?;
    assert!(app.screen().text().contains("> beta"));
    app.handle(lazykeymap::Event::Confirm)?;
    assert!(app.screen().text().contains("layer 0/"));
    assert!(app.screen().text().contains("beta"));
    Ok(())
}
```

`choose` with one board skips the list and paints the layout. `choose` with none returns `Error::NoBoard`. `attach` paints `choose` whenever at least one board answers, including when that list has one name: opening a keyboard is always the user's Enter. Enter loads the highlighted board. Zero Vial boards return `Error::NoBoard`. One board whose id answers and whose definition does not return `Error::NoLayout`, and the binary prints that error. It does not draw a rectangle of empty keys.

The cap token is the keycode legend. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. `KC_A` prints `A`. A non-empty KLE label 1 replaces that legend. Composite codes name their modifiers as Vial does (`catalog::mod_name`: `LCtl`, `RGui`, `LCtl+Sft`, `Meh`, `Hyper`): a mod-tap is `RGui(J)`, the modifier over the tap; a key with modifiers held is `LGui+C`, `LGui+` over `C`; a layer-tap is `LT1(Esc)`; a one-shot mod is `OSM(LSft)`. On ZMK a key in modifier functions shows them as symbols, `LG(C)` as `⌘C`, unless QMK names that key itself (`EXCL` is `!`). Tests live next to these fixtures.

Key strings are KLE's serialized legends. The default center-front alignment stores label 8 on the fourth line, label 1 on the ninth line, and label 4 on the tenth line.

`tests/fixtures/mini.json` is a 2 by 2 matrix. Group 0 is named `Side` and its choices are `Shown`, then `Hid`. The dword `0` selects choice 0. The key at `0,1` has label 1 `Hid` and label 8 `0,1`, so the painter omits it. The key at `0,0` is the first selectable key.

```json
{
  "name": "mini",
  "matrix": { "rows": 2, "cols": 2 },
  "layouts": {
    "labels": [["Side", "Shown", "Hid"]],
    "keymap": [["0,0", "0,1\n\n\n0,1\n\n\n\n\nHid"]]
  }
}
```

`tests/fixtures/encoder.json` is one clockwise encoder. Label 4 is `e`. Label 0 is `0,0`. An unlocked board writes that direction on `Confirm` with no `Tick`.

```rust
fn encoder_writes_clockwise() -> lazykeymap::Result<()> {
    let board = lazykeymap::MemoryBoard::from_definition(include_str!("fixtures/encoder.json"))?;
    let mut app = lazykeymap::App::new(board.open()?, lazykeymap::TestScreen::new(40, 8));
    app.handle(lazykeymap::Event::Char('/'))?;
    app.handle(lazykeymap::Event::Char('a'))?;
    app.handle(lazykeymap::Event::Confirm)?;
    assert!(app.screen().text().contains("[A]"));
    Ok(())
}
```

```json
{
  "name": "enc",
  "matrix": { "rows": 1, "cols": 1 },
  "layouts": { "keymap": [["0,0\n\n\n\n\n\n\n\n\ne"]] }
}
```

## Editors

The top line is a tab bar: one tab per layer (`L0`, `L1`, ...), then `Tap Dance`, `Combos`, and `Macros`, each only when its count is nonzero. `Tab` and `Shift-Tab` (`Event::Tab`, `Event::BackTab`) walk the whole bar, so they step through the layers and then the editors. A board with one layer and no entries draws no tab bar. In search, `Tab` and `Shift-Tab` step through the keycode groups; the arrows move inside the group.

Each editor tab is a sheet: one row per entry, a cursor cell, and a status line naming that cell. `h` `j` `k` `l` and the arrows move. `Enter` edits the cell: a key cell opens the same search panel the keymap uses, and the pick lands in that cell; a number or text cell opens a one-line field (`Enter` saves, `Esc` cancels). `x` and `Backspace` clear a key cell to `KC_NO` or remove a macro step.

- Tap dance rows are `TD(n)` with tap, hold, double tap, tap+hold, and the tapping term (0 to 10000 ms). Wire: `0xFE 0x0D 0x01/0x02 n`, five little-endian `u16`, status in byte 0 of the reply.
- Combo rows are `combo n` with four input keys and the output. Wire: `0xFE 0x0D 0x03/0x04 n`, same shape.
- Macro rows are `M0`..`Mn`, one chip per step and a trailing `+`. `t` `p` `r` add a tap, press, or release through search; `s` adds text; `w` adds a wait. Each insert goes in at the cursor and the cursor moves past it, so repeated inserts append in order. `H` and `L` move a step. Text is printable ASCII, since the firmware types bytes and `0x00` ends a macro while `0x01` starts a code. The buffer is VIA `0x0C` count, `0x0D` size, `0x0E` read, `0x0F` write, with the protocol 2 step codes from vial-gui `macro_action.py`. Boards below Vial protocol 2 get no macro tab.

The unlock hold is only for the two writes a locked board refuses: a change that contains `QK_BOOT` (keymap, encoder, tap dance, or combo output) and any macro buffer write. That is also when vial-gui asks. `Board::needs_unlock` is that rule; `begin_hold` reads unlock status only then, and starts the hold with `hold {keys} to write {what}` when the board is locked. Everything else writes at once, locked or not. `Cancel` abandons the hold.

The unlock keys are real keys, so holding them types into this terminal (Corne's stock combo is matrix 0,0 + 0,1, Tab and Q). During the hold only Esc acts; `q` and `?` do nothing, and Ctrl-C still quits. vial-qmk counts down from 50, one step per poll that comes more than 100 ms after the last counted one while every unlock key is down, and resets the count on any other poll. The binary therefore ticks every 200 ms on a fixed clock, as Vial's client does, whether or not keys arrive in between; about ten seconds of holding unlocks. Poll byte 2 is that count, drawn as a ten-step bar after the prompt, so an empty bar means the board does not see every key held. Once a poll reports unlocked the write goes out, the screen says `unlocked, let go of the keys`, and every key is dropped until one whole tick passes with none, so the repeats of the held keys never reach the next view.

## ZMK Studio

`vial::zmk::Zmk` is one more `Io`. Like `FakeIo`, it decodes each Vial report into an `Ask` and answers it, here by ZMK Studio RPC: protobuf (zmkfirmware/zmk-studio-messages) in SOF `0xAB` / ESC `0xAC` / EOF `0xAD` frames. `Framed` numbers the requests, cuts the frames, and matches answers; under it a `Link` only moves bytes, and there are two:

- `Serial`, the board's USB CDC port. `enumerate` opens only ports with ZMK's default USB id (`1d50:615e`), a maker naming ZMK, or the one `--port` named (`attach_port`, kept for reconnects), since opening a serial port resets an Arduino or a printer. A port that answers `core.get_device_info` within 300 ms is a board.
- `Ble`, the Studio GATT characteristic `00000001-0196-6107-c967-c5cfb1c2482a`: writes with response in pieces of MTU − 3 bytes, answers by indication. btleplug finds the paired boards the system is connected to by the service UUID (`retrieve_peripherals`; a 2 s scan where that is unsupported), on one tokio runtime thread. Finding adapters is capped at 1.5 s, because macOS never answers while Bluetooth is off or the terminal may not use it, and all of Bluetooth at 8 s. Studio answers only on the keyboard's active output, so a board typing over USB is found on serial.

Nothing above `Io` knows the keyboard is ZMK, so `Load`, `Board`, the draft, undo, and the screen are shared.

- Before `Load` sends its first report it calls `Io::busy` until it returns `None`; `Zmk` reads one piece per call (lock state, the behavior list, each behavior's details, `keymap.get_keymap`, then `keymap.get_physical_layouts` and the definition) and names the next piece, which the opening card shows. Over Bluetooth this takes seconds, and the screen repaints and takes Esc between pieces. A locked board fails the open with `Error::Locked`; its card reads `press the keyboard's Studio unlock key: it opens by itself` (`LOCKED_CARD`), and each `Tick` starts the open again (one `get_lock_state` call), so the unlock key opens the board with no Enter. The port stays open across those tries. The firmware default (`CONFIG_ZMK_STUDIO_LOCK_ON_DISCONNECT`) locks again when the client disconnects, so the unlock has to happen while this process holds the port; closing the program and opening it again asks for the key once more. A write the board refuses as locked (it locked itself while idle) becomes the same hold a Vial unlock uses (`Session::write`, prompt `LOCKED_SAVE`): each tick asks the lock state, and the write goes on once the key is pressed.
- A call's wait is how long the keyboard may stay quiet, reset by every arrival, not a cap on the whole reply: a keymap comes over Bluetooth about 20 bytes per indication. Bluetooth writes give up after 3 s. `LAZYKEYMAP_TRACE=file` logs each call.
- The definition is made here: one KLE row per key of the active physical layout (centi-units and centi-degrees, divided by 100), matrix `1 × keys`, label `0,position`.
- A binding maps to the QMK keycode that does the same thing, found by the behavior's `display-name` (`Key Press`, `Mod-Tap`, `Layer-Tap`, `Momentary Layer`, ...). Layer parameters are layer ids; QMK's layer number is the layer's index. A keycode stands for a binding only if it maps back to that exact binding. Every other binding on the keymap, then every binding whose parameters are all named values or small ranges (Bluetooth, outputs, lighting, no-parameter behaviors), becomes a `customKeycodes` entry from `0x7E00`, named from the behavior metadata.
- A query starting with `&` is a binding being put together from that same metadata (`Zmk::complete` / `Zmk::intern`). Known display names use the keymap-file tokens (`Key Press` is `&kp`); the keyboard's own behaviors are slugged. Enter takes the highlighted hit, and once every parameter the behavior reported is filled the binding is staged. Studio sends constants by display name and value; the first parameter of `&bt`, `&out`, `&rgb_ug`, `&bl`, `&ext_power` and `&mkp` is typed as ZMK's header name for that value (`COMMANDS`), so `Select Profile` (3) is `BT_SEL` and the binding reads `&bt BT_SEL 1`, as in a keymap. Other constants are their display name without spaces.
- ZMK sends no parameter list for `&mmv`, `&msc` and `&ext_power`. For a behavior without one, the composer takes up to two parameters as keymap text (`bare_param`): a number, `MOVE_X(n)`, `MOVE_Y(n)`, or `MOVE_UP`/`SCRL_DOWN`/`EP_TOG` at ZMK's default speeds. Without parameters the behavior is staged with zeros, as Studio does.
- ZMK keys and cells are edited in columns, one per part, as Keymap Editor's chips (`Browse::Part` in `session.rs`, drawn by `layout::columns_body` from `Lower::Columns`). `Parts` holds one word per column, from `Io::binding_text` (`Zmk::text` on Studio, the file's own text on a keymap), the active column, and the binding it opened on. Column `i` lists what the composer gives for the words left of it (`column_hits`); its title is the parameter's role (`file::role`: `hold · modifier`, `tap · key`, else what it takes). The active word follows the highlight (`follow`), so ↑/↓ and typing choose live and the columns right of it redraw; `column_count` stops one past the first word that is not in its column's list, because a later list can depend on it (`&bt BT_SEL`). A word kept from the opening binding stays valid where no list holds it (`word_valid`), so a `#define`d layer survives an edit to the tap key. Enter stages `words.join(" ")` when every column is valid and the composer calls it complete, and otherwise moves to the first column to choose, which takes its first value (`land_on`). Typing `&` starts over with no words. Keys are listed once each by the name keymaps use (`zmk_keys::listed`: keys.h's first name, or its first alias when that is longer than six), with the other names as the hit's searchable aside. A binding not written as `&…`, such as a `#define` call, opens on the behavior column. Vial boards have no binding text and keep the QMK groups.
- On a ZMK board or file, `Board::legend` reads the binding text: `&kp X` is the key's short legend (`Bksp`, `LShift`); another behavior is `label(params)`, with the first parameter of `mo`/`to`/`tog`/`sl`/`lt` by layer name and bare modifiers as ⇧⌃⌥⌘ (`mt(⇧ A)`). `layout::cap_lines` puts the label over the parameters, or the label and first parameter (cut to fit) over the last. The info line shows the binding text instead of a QMK code and kind (`Detail::code` is `None`).
- `Io::speaks` says which keycodes the keyboard can store, and `Load::finish` drops the rest from the key list with `catalog::retain`.
- `SetKey` sends `keymap.set_layer_binding` and then `keymap.save_changes`, since Vial writes are permanent and Studio's are staged. ZMK saves only changed bindings, so one save per key writes no more flash than one save at the end.
- Tap dance, combo, macro, and encoder asks are refused; the entry counts are 0, so their tabs never show. Protocol 4 keeps persistent default layer keys out and lets the entry flags carry Caps Word and Key Repeat.
- Layer names come from Studio (`Io::layer_names`) and show in the tabs (`L1 Sym`) and the panel title. `:layer-add`, `:layer-remove`, `:layer-rename NAME`, `:layer-left`, and `:layer-right` go through `Io::edit_layers` (`LayerOp`): Studio's `add_layer`, `remove_layer`, `set_layer_props`, and `move_layer`, then `save_changes`. They refuse while keys are staged, because the keymap is read again after: `Board::reopen` hands the open link back as a candidate, and the `Open` state reads it and lands on the layer the change was about. A command's argument is whatever follows its name on the `:` line (`command_arg`).
- `:layout` lists the physical layouts Studio reported (`Io::layouts`); `:layout 1` or `:layout five` calls `set_active_physical_layout`, saves, and reads the keyboard again, the same way a layer change does. A board with one layout, a keymap file, and a Vial board say so.

## ZMK keymap files

`lazykeymap KEYMAP` (`attach_file`) edits a `.keymap` file the way Keymap Editor does. `vial::file::FileIo` is one more `Io`, so the keymap, layers, draft, undo, and screen are the ones above; `:w` writes the file.

- `dts` parses devicetree with byte spans for every node, property, and cell token, skipping comments and preprocessor lines (it keeps `#define`s). Every write is a list of splices (`dts::apply`) on the original text, then a parse of the result, so whatever was not edited stays byte for byte.
- A binding's text maps to a QMK keycode through the same `zmk::qmk_code` / `zmk::qmk_binding` the Studio backend uses, keyed by behavior label (`kp`, `mt`, `lt`, `mo`, ...) and ZMK key names (`zmk_keys`, generated from `keys.h`). Only a keycode that maps back to the same behavior and parameters stands for a binding; every other binding is a custom keycode named by its own text (`Io::intern`, which adds it to the key list).
- A query starting with `&` in the key list is a binding being put together. `Io::complete` lists what may come next: behaviors, then each parameter's values, from Keymap Editor's behavior schema for ZMK's own (`file::BEHAVIORS`: key, modifier, layer, command, profile, button) and, for the file's hold-taps and macros, `#binding-cells` with what their `bindings` hold. Enter takes the highlighted hit; once the behavior has every parameter, the binding is staged on the key or the section cell. A folder with several keymaps opens the choose screen with one card each, and `:layer-remove` asks first (`Question::RemoveLayer`).
- A key write replaces that binding's bytes, padded to the old width; one that no longer fits lays the layer's grid out again (`realign`), keeping each column's start where it can.
- The layout comes from `<keymap>.json` or `info.json` beside the file (Keymap Editor's and QMK's key list), else from the lines the first layer is written on.
- Layer changes renumber what points at layers: the first parameter of `mo`, `lt`, `tog`, `to`, `sl` and of hold-taps whose first binding is one of those, combo `layers`, and conditional `if-layers` / `then-layer`. A layer that something still points at is not removed.
- `sections` models the rest as sheets (`Tab::Section`): Combos, Macros, Behaviors, Conditional, and Encoders. A row is a node; a cell is a property kept as the file's text (`Record`), typed by `Cell` (one binding, `< >` cells, a string, a flag), with an optional ragged list of bindings after the columns. A section changes as one `Write::Section`, like `Write::Macros`; `FileIo::write_section` rewrites only changed properties, removes and adds nodes, sets a macro's `compatible` and `#binding-cells` from its `&macro_param_` bindings, and renames every `&label` use when a behavior or macro is renamed. A behavior or macro still in use is not removed. After a write the file's rows replace both the draft and the saved copy, since new nodes now have places in the file.
- A `#define` used as a binding expands while editing (`expand_call`): `HRML(A, B, C, D)` is four keys, and an object-like define is the bindings in its body. Editing one of those keys rewrites the invocation as the expanded bindings; leaving them alone keeps the `#define`. Layer changes renumber layer numbers inside define bodies too (`#define NAV &mo 2`), across `\` continuations and not inside `//` comments. The layout is not generated from a matrix transform.
- A file and the Studio keyboard that runs it link into one `Io` (`vial::linked::Linked`), so a change lands on both and nothing above `Io` knows there are two. `attach_file` reads each connected Studio board whole (`link_files`, before the screen opens) and compares its keymap with each file's through `Io::keymap` and `binding_text`, never through either side's types: the same layer and key counts, then at every key the file's binding, read by the keyboard (`intern`), must be the keyboard's keycode there. Studio names a behavior by its display name and a keymap by its node label, so before that check the labels that differ at the same key are learned as aliases (`&hm` is `&homerow_mods`) and used to translate every binding sent later; a label that would map two ways fails the check. The file stays what the screen reads (layers, sections, completion, `binding_text`). A key write goes to the keyboard first, as the file's text interned there, then to the file: a locked keyboard (`Error::Locked`, the unlock wait) or a refused binding leaves the file unchanged, so the two never part on a key. `speaks` asks both, so the key list offers only what the keyboard can take. Layer edits go to both, the keyboard first. Sections go to the file only; Studio has no call for them. A file that does not link opens alone, and its card says why at the first difference.
- The same link is made from inside, on an open Studio board: `:e PATH` (`Model::open_file`, `Board::link_file`) takes the board's own `Io`, links it with the file at `PATH` (a `.keymap`, or a zmk-config folder with one), and on success reads the board again, as `:layout` does, so layer names, the editor tabs, and bindings now come from the file. On a mismatch the `Io` goes back unchanged and the notice says where; nothing is read again. Unsaved changes refuse it first, as layer changes do: the board is read again after. Only a board whose `Io` writes to `the keyboard` and lists its keymap qualifies, which is a Studio board and not a Vial board, a file, or a board already linked. `:e!` stays discard: an exact name sorts first in the command list.

## Find anything

Space, or Ctrl-K, opens one list over the board or a sheet, titled `find anything`, with a query line on top (`› esc▏`). Space is free on the board and the editor tabs, and is one thumb press, as the pickers in LazyVim and Helix; inside the list it types a space, which separates query words. Ctrl-K stays for the habit. Cmd-K is not bound: a terminal program only sees Cmd through the kitty keyboard protocol, and Terminal.app, iTerm2, and Ghostty keep Cmd-K to clear the screen.

The rows come in three groups. `run` is the `:` commands, each with its command (`write changes to the keyboard   :w`); Enter runs it through `run_command`, as if typed, so the two never disagree. `go to` is each layer and each editor tab. `on the board` is `Board::jumps`: every key on every layer that is not `KC_NO` or `KC_TRNS`, as `Esc  ·  L1 key 0,0`, then each tap dance, combo, and macro row with its keys, once the background read has them. It joins only once something is typed, since a board has hundreds. A row matches when every word of the query is in its label or its QMK name, so `esc l1` finds Esc on layer 1. Enter on a key goes to its layer and selects it; on an entry, opens its sheet at that row. Up and Down (Ctrl-P, Ctrl-N) move; the box shows twelve rows and follows the cursor, with `n of m` under them. Esc or Ctrl-K closes it. While it is open the background read keeps going.

## Mouse

The binary turns on mouse capture and sends `Event::Mouse { button, col, row }`: a left click, a right click, or a wheel turn. Nothing is hit-tested by geometry. The drawing wraps each clickable thing in a zero-width mark, `ESC[>{hit}m … ESC[<m` (`layout::mark`), at the place that draws it: a header tab, a key cap's three lines, a layer chip under the key, a sheet cell, a board card, a key-list hit (grid, Basic keyboard, and the caps under it), a group name in the panel title, a binding column's value, a popup's pick-one line and answer chips, a footer hint, `? help`, and the whole lower panel. Every width helper already skips escapes up to `m`, so marks ride through padding, borders, clipping (`take_visible` keeps escapes past the edge, so a clipped mark keeps its end), and the popup's cell grid (`Cell::mark`, `Cell::after`). `Model::frame` renders, takes the marks out (`layout::zones`), and keeps where each landed; the terminal never sees one. A popup clears the marks of what it covers, so it takes every click, as a modal does.

A click goes to the innermost mark under it (`hits_at`). It selects what it lands on; a click on what is already selected does what Enter does there (`Model::click`): opens the key list on a key, edits a cell, opens a board, runs a find-anything row, stages a key-list pick. Two clicks to stage, not one, because a pick changes the keymap: the first shows it in amber on the key, as the arrows do, and a stray click costs nothing. A footer hint runs its `Act` through the same dispatch as its key (`keys::footer_act`); a hint that merges different acts, `h/j/k/l move`, is not a button. A popup chip sends its key's event (`enter`, `esc`, `tab`, or one letter). The wheel changes layer over the board, scrolls the key list or columns when it is over the lower panel (the `Lower` mark), and moves a sheet, the choose list, find anything, or help. Right-click is Esc. Shift-drag still selects text: terminals pass it through mouse capture.

While the key list is open on a key, a click on another key on the board moves the list there, as Esc, the arrows, and Enter would, in one step: the list opens again through `searching`, on that key's own keycode or binding. The group shown goes back to the first, as any opening does. Hover is drawn, not hit-tested: the binary turns pointer motion into `Mouse::Hover`, `Model` keeps the innermost hit under it when a click would act on it there (`Model::clickable`: a hint that is not a button, a `j k` chip, the lower panel, or a key during a sheet does not light), and `layout::zones` lights that hit's runs as it takes the marks out (`lift`): every style inside the run gets its background 28 steps lighter, and text with no background of its own gets the key face 28 steps lighter, so whatever is hovered reads as one solid chip. An underline was tried first and read badly: it striped a key cap's three lines and ran under a list cell's padding. A move repaints only when the lit hit changes, since `App` skips a frame equal to the last. A move keeps the notice and is not a key to the unlock wait; a key press drops the light until the pointer moves again.

## Draft and write

Edits are staged, not written. The board keeps two copies: `saved`, what the keyboard holds as last read or written, and the draft (`keymap` and `entries`), which is everything the screen shows. `Board::stage` changes the draft and pushes what it replaced onto an undo stack. `Board::pending` is the difference, one `Write` per changed key, encoder direction, tap dance, or combo, and one for all macros. The header shows `● N unsaved` while it is not empty, and changed keys and cells draw their legend green until written.

A keycode picked in search stages at once on Enter: the key list already previewed it, nothing reaches the keyboard before `:w`, and `u` takes it back. The preview (`vial::Preview`) lands where Enter would: on the board, the selected key; on a sheet, the tap dance or combo cell, drawn as an amber chip, with `NO → Esc  ·  enter to stage` on the status line. In a macro, a replaced step keeps its kind (`↓A` stays a press, `replaced_step`, which staging uses too), and a new step shows where it goes in, with `new step → A`. Field edits (tapping term, delay, text), `x`, and step moves stage directly too, since Enter in the field or the key itself already is the confirmation.

Every question and the help list are popups (`layout::Popup`): a rounded box in the middle of the screen with a one-cell drop shadow, while everything behind it dims to 40% so it reads as background. The body is lines of four kinds: a heading in the frame color, a key and its meaning in two aligned columns, plain text that wraps at 60 columns, and a change with the old value dim and the new one green. The keys that answer sit at the bottom right as filled chips (`enter discard`, `esc keep`), so the options bar is empty while a question is up. Help is blue; a question that writes or throws away changes is amber. A body taller than the screen scrolls with `j`/`k`, and the bottom border shows `↑↓ 3–20 of 98`.

The command line is the same popup. `:` or `;` on the keymap or a sheet opens it with the query `:`, and a `:` typed first into the find list does the same, as `>` does in VS Code's palette. After `:` the title is `command`, the mode badge `COMMAND`, and the rows are only the commands, each with its names and what it does, filtered by name prefix instead of words. An exact name sorts first, so `:q` is quit and not `:q!`; the rest keep table order, so `:wr` writes. The highlighted row is what Enter runs: the one ↑↓ chose, else the first once anything is typed. With nothing typed, after `:` or after Space, nothing is highlighted and Enter only closes, so Space then Enter cannot write by accident. Tab fills in the highlighted command's first name. A line no command starts with stays open and says `no command starts with :xyz`. Esc closes it, and so does Backspace on an empty line or on the bare `:`, as in vim. One table, `COMMANDS` in `session.rs`, holds the names and what each does; the `:` rows and the `run` group both read it, and Enter on either goes through `run_command`.

| Command | Does |
|---|---|
| `:w`, `:write` | write every staged change to the keyboard |
| `:wq`, `:x` | write, then quit |
| `:q`, `:quit` | quit; asks first when changes are unsaved |
| `:q!` | quit without writing |
| `:e!`, `:discard` | discard every staged change, after a popup listing them |
| `:u`, `:undo` | undo, same as `u` |
| `:help`, `:h` | the help popup, same as `?` |

`q`, `:q`, and Ctrl-C with unsaved changes open a popup titled `N unsaved changes` that lists each one as the keyboard has it and as the draft has it: Enter or `w` writes then quits, `d` quits without writing, Esc stays, and a second `q` or Ctrl-C quits. `:e!` lists the same way and asks before discarding; with nothing staged it says `nothing to discard`. Dropping an `App` without `:w` loses the draft; the keyboard keeps what it had.

`:w` walks `pending` in order through `Session::write`, the same `begin_hold` and `commit` path as before. A write that needs the unlock parks the save in a hold; after the release, `save` runs again and picks up what is still pending, so a stopped save (Esc in the hold) leaves exactly the unwritten changes staged. The unlock prompt names keys by what the keyboard holds, not the draft, since those are the keys under your fingers. `Board::commit` is the only sender for all four kinds of write and copies each one into `saved`. A macro write sends only the 28-byte chunks that differ from the last read or write. Macros that would not fit the buffer are not staged; the status line says how many bytes they need and how many the board holds.

Adjacent text steps share bytes on the wire, so they come back as one step after a reopen, as in vial-gui.

## Shape

The public API is `attach`, `Attached`, `MemoryBoard`, `App`, `Event`, `Mouse`, `Step`, `Screen`, `TestScreen`, and `Error`. Interface depth is the point of the cut. That list hides device discovery, the 32-byte report codec, the two response grammars, LZMA, KLE, the layout-option filter, the keycode catalog, unlock, and the choice between a matrix write and an encoder write. The caller is left with events and the painted string, because those are the product loop. A `Screen` receives the finished string and cannot paint from a frame of its own.

`App::handle` paints. It does not decide what a key or a click means. `Model::handle` in the session module is the only match on `Event`. `App` is the shell that copies `Model::frame` onto the `Screen`, per boundary-discipline; the click zones stay in `Model`, so `App` cannot name a hit. `TestScreen` stores that string. `Terminal` prints it.

`Model` is a private enum of pick, map, search, and hold. Map and hold are `Session<Mapping>` and `Session<Holding>`. Search holds `Session<Mapping>` plus a browse value, either a shelf or a non-empty token. The phase parameter is the pending write, per model-the-domain. `Holding` stores the place, the code, and the prompt. There is no `Option` of a pending code on the mapping session. `poll` exists only on `Session<Holding>`. `list` exists only on `Session<Mapping>`. `Hit` lives in the session module. Its `u16` is private, and only `list` supplies the code that becomes a `Hit`. `assign` consumes a `Hit`. The screen can pass a hit back. It cannot mint a code, so a keymap file has no constructor to target.

The keymap has one owner, the `Board` inside the session, per foundational-thinking. Paint derives a frame and drops it. The frame is not a second keymap. `Place` is a private enum, `Matrix { row, col }` or `Encoder { index, direction }`, with direction 0 clockwise and 1 counterclockwise. `commit` matches that enum and sends either set-key `0x05` or vial set-encoder `0x04`. Callers above `vial` name a `Place` the parser already built. They do not choose a command. `App` cannot name `Place`, `Board`, or `Ask`.

Modules are nested so the compiler keeps it that way, per encode-lessons-in-structure and type-system-discipline. `layout` and `catalog` sit inside `vial`. `vial` sits inside `session`. `commit`, `Ask`, and the report constants are `pub(super)` or private. `app` can call `Model`. `app` cannot call `commit`. Adding a lighting write means a new private ask, a new `Board` method, and a new arm in `Model::handle`. It cannot be a new variant in a public command enum, because that enum does not exist.

`fan_in` is the only device decision. `attach`, `MemoryBoard::open`, and `MemoryBoard::choose` all call it. Zero candidates is `NoBoard`. One candidate loads. Many candidates become the choose screen and load on `Confirm`.

Opening from the choose screen reads one report per `Load::step`. Each `Tick` reads for about 60 ms, then returns `Step::Working`; the binary sends the next `Tick` at once, still taking keys between slices. Over Bluetooth a report takes tens of milliseconds and a 4-layer board with tap dances and combos takes about five seconds, so the open card shows a progress bar, the part being read (`keymap 14/40`), and `esc stops`. Esc returns to the list. A board that fails to open stays on the list with the reason on its card; a board that stops answering, as a sleeping Bluetooth board does, says `no answer: wake it, then enter`. The percentage never steps back as counts are learned.

The board opens once the keymap, encoders, and lock state are in (`Load::ready`); on a Bluetooth Cornix that is about 3 of the 5 seconds. The tap dances, combos, and macro buffer follow in `Board::read_rest`, one slice per `Tick`, while the keymap is usable. The header shows `◌ reading tap dances` meanwhile, without a count, so the screen repaints only when the part changes: a repaint between reports is a gap, and over Bluetooth a gap makes the next report take about half a second. Their tabs appear at once from the counts the read already has (`Board::editors`), marked `Tap Dance…` until all three are read. An unread tab shows a `reading` panel instead of its sheet: `lazykeymap is still reading this keyboard`, `◌ now {part}`, again without a count, and that the keymap works meanwhile. Its keys are `keys::View::Reading`: tabs, help, quit, undo, and the command line, so no edit key reaches a list that is not there yet. Once read, the tab becomes its sheet in place, and the saved copies arrive equal to the draft, so nothing looks unsaved. Background reads pause during an unlock hold and a save, whose ticks belong to them.

A report that fails after the board is open (a save, an unlock poll, or the background read) no longer quits. The board stays on screen with its draft, under a `keyboard disconnected` popup with a pick-one list (`j`/`k`, Enter): reconnect and keep the unsaved changes, reconnect and revert them, start again from the keyboard list, or quit. Without unsaved changes it is reconnect, start again, or quit. `q` quits at once, since the list already says what quitting drops. Reconnecting first closes the old device (`Board::disown`): a HID device still held does not enumerate again. It then looks for the same keyboard by name, maker, USB id, and serial; a keyboard not back yet says `not back yet: wake the keyboard or plug it in, then try again` and the list stays. Found, it reads the board again with progress in the popup, and Esc stops that. Keeping the changes waits for the tap dances, combos, and macros too, then stages every old unsaved change on the fresh board, so `:w` writes them; the layer and selected key carry over. `MemoryBoard::unplug` and `plug` make the fake device stop and start answering.

`visible` drops keys the layout-option dword does not select, so the cap list the painter walks has no hidden key and no second filter flag. Groups come from `layouts.labels`, in order. Each group is `[name, choice, choice, ...]`. The group uses `(choice_count - 1).bit_length()` bits, packed from the low end of the `u32`. A key with no label 8 stays. A key with label 8 `index,choice` stays when that group decodes to `choice`. A label index the definition does not declare fails the open. The dword is read once, with VIA get-keyboard-value `0x02` and value id `0x02`, and stored on the board. Nothing in `session` or `app` can replace it. `MemoryBoard::layout_options` only sets the fake device's answer before open.

KLE metadata objects carry `x`, `y`, `w`, `h` onto the next key. Twelve labels are split on newlines. Label 0 is `row,col` for a normal key and `index,direction` when label 4 is `e`. A decal has no place. A matrix address outside `matrix.rows` or `matrix.cols` fails the open. The parser keeps KLE rows as the line breaks. Within a row, `render` sorts by `x` and starts each token at `round(x * 4)` columns, pushed two spaces right on overlap. `y`, `w`, and `h` stay on the cap for that one function. Negative `x` clamps the column to 0.

The catalog is one `Vec` built at load. Shelves are ranges in that vec and are the Vial keycode tabs, in Vial's order: `Basic`, `ISO/JIS`, `Layers`, `Quantum`, `Backlight`, `App, Media and Mouse`, `MIDI`, `Tap Dance`, `User`, and `Macro`. The static rows live in `vial/keycodes.rs`, generated by `scripts/keycodes.py` from a QMK checkout: QMK supplies only identifiers and codes; shelf membership and legends are written in the script. Regenerate with `python3 scripts/keycodes.py ~/qmk_firmware > src/session/vial/keycodes.rs`. MIDI shows when the definition's `vial.midi` is `basic` or `advanced`. `User` is the definition's custom keycodes, or `QK_KB_0`–`QK_KB_15` when it has none. `Macro` is `M0` up to the count VIA command `0x0C` reports, then the dynamic-macro keys. Layers is built from the layer count command `0x11` reports, as Vial does: Layer Lock, `QK_TRI_LAYER_LOWER`/`QK_TRI_LAYER_UPPER` when there are at least 4 layers, then `MO`, `DF`, `PDF`, `TG`, `TT`, `OSL`, `TO` for each layer, then `LT` for the first 16. A layer code outside that count has no catalog row, so its legend is the hex number. From Vial protocol 4, load sends dynamic-entry count `0xFE 0x0D 0x00`, as Vial does. Byte 0 is the tap dance count, so `Tap Dance` is `TD(0)` up to it. Bit 0 of byte 31 shows Caps Word, bit 1 shows Layer Lock, and a nonzero alt-repeat count (byte 3) shows Repeat and Alt Repeat. `PDF(n)` shows from protocol 5. Below protocol 4 there are no tap dances and all four gated keys stay hidden. An empty tab is omitted. A title too wide for the panel slides left so the bracketed shelf stays visible. An empty query shows the current shelf. A token ranks the whole vec. A `(kc)` row (`LSFT(kc)`, `LT0(kc)`, `LCTL_T(kc)`, ...) does not assign: confirm keeps search open on `Basic` with the title prefixed `{legend} > `, and the next confirm of a code at or below `0xFF` assigns the two OR'd together. `QK_BOOTLOADER` (`QK_BOOT`) is the only entry with `hold_first`. `legend` is the short name.

`begin_hold` runs on assign, and only for a write `needs_unlock` names. It reads unlock status. If the board is locked, it sends unlock start and returns the hold. It does not write the key. `commit` is the only function that sends set-key or set-encoder. If the cached code already equals the request, `commit` sends nothing and reports same, per make-operations-idempotent. If status is still locked and the write needs the unlock, `commit` sends nothing and returns the hold. After a successful report, `commit` updates that one cell. `poll` sends unlock poll, then `commit` of the pending write. A second `Confirm` of the same code finds the cell already set. A crash drops the cache. The device keeps the last report that was acknowledged. The next open reads the buffer and the encoder slots again. Unlock-in-progress with no pending write dies with the process.

The fake device is an `Io` beside HID. Both speak 32-byte reports. `Ask::grammar` returns `Echo` or `Overwrite`. `decode` matches that value. It does not sniff a shared header. `Echo` is layer count, macro count, layout options, the keymap chunk, and set-key. `Overwrite` is keyboard id, definition size, definition blocks, encoder get and set, the unlock asks, and the dynamic-entry counts. The fake encodes replies with that same codec. `MemoryBoard::from_definition` parses the JSON, sizes the matrix to `KC_NO`, compresses the document, and serves the compressed bytes back through get-definition. `MemoryBoard::macros` sets the macro count, 0 by default. `MemoryBoard::tap_dances` sets the dynamic-entry tap dance count, 0 by default. `MemoryBoard::features(flags, alt_repeat)` sets the dynamic-entry feature bits and alt-repeat count, both 0 by default. Load always LZMA-decompresses, including in tests. HID prepends the `0x00` report id around the 32 bytes. The id never enters `encode`.

The store is `Arc<RefCell<...>>` and is `!Send`. One thread is the whole program, per separate-before-serializing-shared-state. `open` sets a flag. A second open returns `Busy`. Drop clears the flag. There is no mutex. The query string lives on the search phase, not on the board.

via-protocol 0.1.0 is rejected. The grounding file limits its known coverage to discovery, keymap buffer reads, and keycode name groups. Definition, unlock, encoders, and the layout-option dword were not established there. Those are this program. Taking the crate for the half it might cover, and writing the rest beside it, is two codecs. The usage-page `0xFF60` / usage `0x61` discovery and the basic keycode names are small enough to own here, in the same module as the asks those names are written through.

Out of scope stays out because the public API has no command trait, the private `Ask` enum has no lighting, override, QMK-setting, keymap-buffer-set, or lock variant, and layout options have no setter after open. Keymap import has no `Hit` constructor and no buffer write.

## Synthesis decision

The base is the Grok sketch in `.audit/arena/grok/`. The cross-judge scored it 30, the Grok 4.6 sketch 23, and the Composer sketch 19. The parent read agreed. The product loop is `App::handle` to `Model::handle` to `Board::commit`. Browse, search, and hold are phases, so a pending write cannot sit on the mapping session as an `Option`.

Two grafts landed in this file. `Ask::grammar` names the two response shapes, taken from the Composer router's split and folded into the existing `Ask` enum. The fixtures above name the cap text, taken from the need to prove the layout-option filter without a second public frame type.

These grafts were rejected. A public `Frame` and a public `FakeUsb` from the Grok 4.6 sketch would publish the geometry and the 32-byte port. A second `Input` type would sit beside `Event`. The Composer `Command` enum and the `commit_selection` helpers are a second way to write a key, and `RemapApp` keeps its own search and unlock flags. A `testkit` feature flag splits a crate that has one binary. `via-protocol` 0.1.0 stays rejected. Its public methods include lighting and tap dance, and it does not cover unlock or encoders.

`Tick` before `KC_RESET` stays, including when the board already reports ready. Quit sends no lock command. One `RefCell` store and `Error::Busy` stay. The painted string stays the only screen contract. `round(x * 4)` lives in one private render function, which is the layout model a later change replaces.

## Tradeoffs accepted

- We accept an opaque event loop in exchange for a caller who cannot issue a raw write, a buffer import, or a layout-option change.
- We accept phase types that only the session module names in exchange for `poll` and `search` being uncallable in the wrong phase.
- We accept an LZMA round trip inside `MemoryBoard` in exchange for one definition decoder on the test path and the device path.
- We accept a clipped monospace string in exchange for one painter and assertions that read the same text the person reads.
- We accept `round(x * 4)` column placement, and a push on overlap, in exchange for KLE `x` surviving into the only painter without a second layout type.
- We accept reading the layout-option dword only at open in exchange for no setter anywhere above the fake-device builder.
- We accept quit without vial lock `0x08` in exchange for `Drop` sending no report.
- We accept `Error::Busy` on a second open in exchange for one writer and no mutex.
- We accept `Error::Protocol(&'static str)` with no report bytes in exchange for keeping wire dumps out of the public error.
- We accept our own keycode table in exchange for one name list the screen and the write path share.
- We accept a `Tick` before `KC_RESET` lands, even when the board already reports ready, in exchange for one hold path inside `begin_hold`.

## Alternatives considered

A transport module, a codec module, a domain module, and a UI module loses on interface depth. Each layer either re-exposes the report or grows a pass-through with the same arguments. The caller of "set key" still has to know lock, encoders, and chunk size. Nesting the codec under the session hides those, at the cost of a larger `vial` module.

A public `Session<Mapping>` the test matches through `assign` and `poll` is a real machine, and it lost for interface depth. The test would coordinate the hold, which is the policy `Model::handle` already finishes. Publishing `Session` also publishes a second way to remap beside `App`.

A public `Command` enum the TUI reduces loses because a new Vial tab is a new variant plus a new arm in the match the screen already has. That is the shortest edit, and it is the edit this program has to make expensive. The private `Ask` enum exists so the request and its response grammar stay one match. It is not reachable from `app`.

via-protocol 0.1.0 as the HID crate, with a local addon for definition, unlock, and encoders, loses for the same reason. Two writers of the report layout will drift. The grounding file does not establish the addon surface as already solved.

## Implementation reconciliation

None. This sketch has not been implemented.

## Open questions and risks

- Is the layout-option dword VIA get-keyboard-value command `0x02`, value id `0x02`, big-endian in response bytes 2 through 5? The sketch assumes that packing because other multi-byte VIA fields in the grounding file are big-endian. If vial-gui uses a different slice, only the private decoder changes.
- Is `layouts.labels` one array per group, name first and choices after, packed from the low bits with `(choices - 1).bit_length()`? If the reference client uses a different width, only `visible` changes. Hidden keys stay omitted from the cap list either way.
- Should `KC_RESET` on a board that already reports ready write on `Confirm`, or keep the `Tick` the sketch uses for every hold?
- Should quit send vial lock subcommand `0x08`? The sketch sends nothing on quit.
- For nudge, the sketch picks the nearest selectable center in the facing half-plane, then the smaller perpendicular distance. Decals are not targets. Is that the movement a staggered or split layout should have?

The definition container is XZ. `vial_generate_definition.py` calls `lzma.compress` with no format argument, and that default is the XZ container, whose first bytes are `fd 7z XZ`. vial-gui calls `lzma.decompress`, which accepts that container. One decoder serves the fake board and a real board. A different container fails every open.

## Next implementation step

Implement the private `Ask` codec and the `MemoryBoard` exchange until the reopen assertion in the second call site passes, with the painter emitting the status line and the key rows that assertion reads.
