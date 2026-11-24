# lazyvial

## Problem

lazyvial is a terminal program that remaps a plugged-in Vial keyboard. The repo has no application code. The grounding file is the constraint set. A keycode is a `u16`. Matrix keys live in one row-major buffer of `layers * rows * cols` codes, big-endian on the wire. Encoder directions live in a separate get and set under the `0xFE` prefix. Firmware starts locked. A locked board still takes key, encoder, tap dance, and combo writes; it only stores `QK_BOOT` as `KC_NO` and skips macro buffer writes (vial-qmk `vial_keycode_firewall` and `via.c`). The grounding file's "writes while locked do not stick" was wrong. The drawing is the KLE keymap inside the LZMA definition, minus keys the current layout-option dword hides. A board with no definition is a failure, and the program must not invent a grid. Edits are staged in a draft and reach the keyboard on `:w`. Quit asks when the draft has changes, and otherwise leaves the keyboard as last written.

Tap dance, combo, and macro editing are in, as tabs beside the keymap (see Editors). Lighting, key overrides, alt repeat keys, QMK settings, layout-option edits, keymap import and export, and firmware flashing are out of this tool. The type layout has to make each of those a new private codec plus a new caller, so a later edit cannot grow them by adding a match arm the screen already touches.

## Usage (caller's view)

The library call is `App`. The caller passes events and reads the painted string. `attach` opens the only Vial board, or returns a chooser when several answer. `MemoryBoard` is the same `Attached` value with no USB device. `Terminal` belongs to the binary. The library never sees a tty event.

```rust
use lazyvial::{App, Step};

fn main() -> lazyvial::Result<()> {
    let mut app = App::new(lazyvial::attach()?, Terminal::open()?);
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

- The header bar is one filled line: a ` lazyvial ` brand chip, the board name, the tabs (`L1`.. then `Tap Dance`, `Combos`, `Macros`, the active one filled), and at the right `● N unsaved` when the draft differs from the keyboard and `● locked` or `● ready`. On a narrow screen the tabs farthest from the active one drop first, marked `…`. The chooser's header says `choose a keyboard` and has no tabs or state.
- The chooser shows each board as a two-line card, centered and at most 64 columns wide: the product name with the manufacturer at the right, then `vid:pid · serial` from HID enumeration, so two boards with the same name still differ. The highlighted card is lighter and carries an accent bar `▌`. A board that opens without a Vial layout shows `✕ no Vial layout` in place of the manufacturer and stays on the list.
- The main panel is a rounded box titled `layer {n}/{count}`, with `n` starting at 1, or `tap dance 1/4`, `combos 1/2`, `macros 1/4` with the cursor row. The keycaps sit centered in it. During a hold the keys the unlock names are painted amber.
- Under the keycaps, when the panel has rows to spare, the selected key's details: a rule, then the legend chip, its QMK name when that says more than the legend (`KC_ESCAPE`), the code (`0x0029`), and what it does (`basic key`, `mod-tap: hold for the modifier, tap for the key`, `layer 1 (tab L2) while held`; QMK and the legends count layers from 0, the tabs from 1, so both are named). Then `on each layer  L1 Esc  L2 NO ...`, the same key on every layer with this layer filled and staged ones green, when the board has more than one layer. Then, when this key is staged, `unsaved: the keyboard still has {saved}, :w writes {draft}`. The details take spare rows only, so a short screen or an open search drops them before anything else.
- The status bar starts with a mode badge (`KEYMAP`, `TAP DANCE`, `SEARCH`, `EDIT`, `COMMAND`, `CONFIRM`, `UNLOCK`, `CHOOSE`): blue while browsing, purple while typing, amber while the keyboard or a question waits. After it comes, in order of urgency, the unlock prompt `hold {row},{col}+{row},{col} to write {what}` with its countdown bar (pairs in status-report order, `255,255` omitted), a notice (amber), or the usual status: `selected {legend}  ·  key {row},{col}` (or `encoder {index} cw|ccw`), with `·  unsaved` when that key is staged, or the sheet cell under the cursor. `? help` sits at the right.
- The options bar is key chips (` h ` on a key-colored chip, then what it does). Hints that share a label merge, as in `left/right group`, and hints that do not fit drop off whole.

Rows go to the key lines first, then the status bar, so the unlock prompt survives the smallest screen, then the options bar, the header, the lower panel, and the borders. A short screen drops the boxes before it drops a key line. Search is a panel under the board, so the keys stay on screen. Its hits run down a column, then into the next, so Up and Down still walk the list in order. Each hit shows its QMK name beside the legend when the two differ. A query with no hits shows `no match`. Help is a popup drawn over that picture, not a panel in the stack. An open search title brackets the active shelf, as in `[Basic] ISO/JIS Layers Quantum Backlight ...`. A query makes it `search {query}`. The highlighted hit is prefixed with `> `. The chooser is one panel titled `keyboards`, one board name per line, the highlighted name prefixed with `> `, and its status bar counts the boards found. A confirm that cannot load a definition leaves the list up and appends ` unavailable` to that name. A decal is wrapped in `()`. A hidden key is absent. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. Lines are clipped to the screen size and are not wrapped.

Search ranking is case-insensitive. A legend or token that starts with the query ranks before a substring. A shorter legend wins a tie. The remaining order is the catalog, tab by tab. `Confirm` assigns the highlighted hit. The fixture below has its first selectable key under the cursor, and a key whose legend is `Hid` with label 8 equal to `0,1`. The default layout-option dword is 0, which selects choice 0 of each group, so `Hid` is omitted.

```rust
fn remap_persists() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .locked(&[(0, 0), (0, 1)]);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("locked"));
    assert!(app.screen().text().contains("[NO]"));
    assert!(!app.screen().text().contains("Hid"));
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Char('s'))?;
    app.handle(lazyvial::Event::Char('c'))?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("NO → Esc"));
    app.handle(lazyvial::Event::Confirm)?;
    for ch in ":w".chars() {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("[Esc]"));
    drop(app);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("[Esc]"));
    Ok(())
}
```

A second `open` while the first `App` still holds the board returns `Error::Busy`. Drop releases it. The second `open` reads the stored codes through the same command codec as the first, so a paint-only cache cannot satisfy the last assertion.

```rust
fn pick_when_two() -> lazyvial::Result<()> {
    let alpha = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .named("alpha");
    let beta = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/other.json"))?
        .named("beta");
    let mut app = lazyvial::App::new(
        lazyvial::MemoryBoard::choose(&[alpha, beta])?,
        lazyvial::TestScreen::new(40, 12),
    );
    assert!(app.screen().text().contains("choose"));
    assert!(app.screen().text().contains("> alpha"));
    app.handle(lazyvial::Event::Char('j'))?;
    assert!(app.screen().text().contains("> beta"));
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("layer 1/"));
    assert!(app.screen().text().contains("beta"));
    Ok(())
}
```

`choose` with one board skips the list and paints the layout. `choose` with none returns `Error::NoBoard`. `attach` paints `choose` whenever at least one board answers, including when that list has one name. Enter loads the highlighted board. Zero Vial boards return `Error::NoBoard`. One board whose id answers and whose definition does not return `Error::NoLayout`, and the binary prints that error. It does not draw a rectangle of empty keys.

The cap token is the keycode legend. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. `KC_A` prints `A`. A non-empty KLE label 1 replaces that legend. Tests live next to these fixtures.

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
fn encoder_writes_clockwise() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/encoder.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 8));
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('a'))?;
    app.handle(lazyvial::Event::Confirm)?;
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

The top line is a tab bar: one tab per layer (`L1`, `L2`, ...), then `Tap Dance`, `Combos`, and `Macros`, each only when its count is nonzero. `Tab` and `Shift-Tab` (`Event::Tab`, `Event::BackTab`) walk the whole bar, so they step through the layers and then the editors. A board with one layer and no entries draws no tab bar. In search, `Tab` and `Shift-Tab` step through the keycode groups, like Left and Right.

Each editor tab is a sheet: one row per entry, a cursor cell, and a status line naming that cell. `h` `j` `k` `l` and the arrows move. `Enter` edits the cell: a key cell opens the same search panel the keymap uses, and the pick lands in that cell; a number or text cell opens a one-line field (`Enter` saves, `Esc` cancels). `x` and `Backspace` clear a key cell to `KC_NO` or remove a macro step.

- Tap dance rows are `TD(n)` with tap, hold, double tap, tap+hold, and the tapping term (0 to 10000 ms). Wire: `0xFE 0x0D 0x01/0x02 n`, five little-endian `u16`, status in byte 0 of the reply.
- Combo rows are `combo n` with four input keys and the output. Wire: `0xFE 0x0D 0x03/0x04 n`, same shape.
- Macro rows are `M0`..`Mn`, one chip per step and a trailing `+`. `t` `p` `r` add a tap, press, or release through search; `s` adds text; `w` adds a wait. Each insert goes in at the cursor and the cursor moves past it, so repeated inserts append in order. `H` and `L` move a step. Text is printable ASCII, since the firmware types bytes and `0x00` ends a macro while `0x01` starts a code. The buffer is VIA `0x0C` count, `0x0D` size, `0x0E` read, `0x0F` write, with the protocol 2 step codes from vial-gui `macro_action.py`. Boards below Vial protocol 2 get no macro tab.

The unlock hold is only for the two writes a locked board refuses: a change that contains `QK_BOOT` (keymap, encoder, tap dance, or combo output) and any macro buffer write. That is also when vial-gui asks. `Board::needs_unlock` is that rule; `begin_hold` reads unlock status only then, and starts the hold with `hold {keys} to write {what}` when the board is locked. Everything else writes at once, locked or not. `Cancel` abandons the hold.

The unlock keys are real keys, so holding them types into this terminal (Corne's stock combo is matrix 0,0 + 0,1, Tab and Q). During the hold only Esc acts; `q` and `?` do nothing, and Ctrl-C still quits. vial-qmk counts down from 50, one step per poll that comes more than 100 ms after the last counted one while every unlock key is down, and resets the count on any other poll. The binary therefore ticks every 200 ms on a fixed clock, as Vial's client does, whether or not keys arrive in between; about ten seconds of holding unlocks. Poll byte 2 is that count, drawn as a ten-step bar after the prompt, so an empty bar means the board does not see every key held. Once a poll reports unlocked the write goes out, the screen says `unlocked, let go of the keys`, and every key is dropped until one whole tick passes with none, so the repeats of the held keys never reach the next view.

## Draft and write

Edits are staged, not written. The board keeps two copies: `saved`, what the keyboard holds as last read or written, and the draft (`keymap` and `entries`), which is everything the screen shows. `Board::stage` changes the draft and pushes what it replaced onto an undo stack. `Board::pending` is the difference, one `Write` per changed key, encoder direction, tap dance, or combo, and one for all macros. The header shows `● N unsaved` while it is not empty, and changed keys and cells draw their legend green until written.

A keycode picked in search asks before it is staged, in a popup titled `stage this change?`: a heading for where (`LAYER 1 KEY 0,1`, `TD(0)`, `COMBO 2`, `M1`), then one `was → becomes` line per changed field, as in `hold  NO → LCtrl`. Enter or `y` stages it; Esc or `n` drops it. Field edits (tapping term, delay, text), `x`, and step moves stage directly, since Enter in the field or the key itself already is the confirmation, and `u` takes any of them back.

Every question and the help list are popups (`layout::Popup`): a rounded box in the middle of the screen with a one-cell drop shadow, while everything behind it dims to 40% so it reads as background. The body is lines of four kinds: a heading in the frame color, a key and its meaning in two aligned columns, plain text that wraps at 60 columns, and a change with the old value dim and the new one green. The keys that answer sit at the bottom right as filled chips (`enter stage`, `esc cancel`), so the options bar is empty while a question is up. Help is blue; a question that writes or throws away changes is amber. A body taller than the screen scrolls with `j`/`k`, and the bottom border shows `↑↓ 3–20 of 98`.

The command line opens with `:` or `;` on the keymap or a sheet:

| Command | Does |
|---|---|
| `:w` | write every staged change to the keyboard |
| `:wq`, `:x` | write, then quit |
| `:q` | quit; asks first when changes are unsaved |
| `:q!` | quit without writing |
| `:e!` | discard every staged change, after a popup listing them |
| `:u` | undo, same as `u` |

`q`, `:q`, and Ctrl-C with unsaved changes open a popup titled `N unsaved changes` that lists each one as the keyboard has it and as the draft has it: Enter or `w` writes then quits, `d` quits without writing, Esc stays, and a second `q` or Ctrl-C quits. `:e!` lists the same way and asks before discarding; with nothing staged it says `nothing to discard`. Dropping an `App` without `:w` loses the draft; the keyboard keeps what it had.

`:w` walks `pending` in order through `Session::write`, the same `begin_hold` and `commit` path as before. A write that needs the unlock parks the save in a hold; after the release, `save` runs again and picks up what is still pending, so a stopped save (Esc in the hold) leaves exactly the unwritten changes staged. The unlock prompt names keys by what the keyboard holds, not the draft, since those are the keys under your fingers. `Board::commit` is the only sender for all four kinds of write and copies each one into `saved`. A macro write sends only the 28-byte chunks that differ from the last read or write. Macros that would not fit the buffer are not staged; the status line says how many bytes they need and how many the board holds.

Adjacent text steps share bytes on the wire, so they come back as one step after a reopen, as in vial-gui.

## Shape

The public API is `attach`, `Attached`, `MemoryBoard`, `App`, `Event`, `Step`, `Screen`, `TestScreen`, and `Error`. Interface depth is the point of the cut. That list hides device discovery, the 32-byte report codec, the two response grammars, LZMA, KLE, the layout-option filter, the keycode catalog, unlock, and the choice between a matrix write and an encoder write. The caller is left with events and the painted string, because those are the product loop. A `Screen` receives the finished string and cannot paint from a frame of its own.

`App::handle` paints. It does not decide what a key means. `Model::handle` in the session module is the only match on `Event`. `App` is the shell that copies `Model::text` onto the `Screen`, per boundary-discipline. `TestScreen` stores that string. `Terminal` prints it.

`Model` is a private enum of pick, map, search, and hold. Map and hold are `Session<Mapping>` and `Session<Holding>`. Search holds `Session<Mapping>` plus a browse value, either a shelf or a non-empty token. The phase parameter is the pending write, per model-the-domain. `Holding` stores the place, the code, and the prompt. There is no `Option` of a pending code on the mapping session. `poll` exists only on `Session<Holding>`. `list` exists only on `Session<Mapping>`. `Hit` lives in the session module. Its `u16` is private, and only `list` supplies the code that becomes a `Hit`. `assign` consumes a `Hit`. The screen can pass a hit back. It cannot mint a code, so a keymap file has no constructor to target.

The keymap has one owner, the `Board` inside the session, per foundational-thinking. Paint derives a frame and drops it. The frame is not a second keymap. `Place` is a private enum, `Matrix { row, col }` or `Encoder { index, direction }`, with direction 0 clockwise and 1 counterclockwise. `commit` matches that enum and sends either set-key `0x05` or vial set-encoder `0x04`. Callers above `vial` name a `Place` the parser already built. They do not choose a command. `App` cannot name `Place`, `Board`, or `Ask`.

Modules are nested so the compiler keeps it that way, per encode-lessons-in-structure and type-system-discipline. `layout` and `catalog` sit inside `vial`. `vial` sits inside `session`. `commit`, `Ask`, and the report constants are `pub(super)` or private. `app` can call `Model`. `app` cannot call `commit`. Adding a lighting write means a new private ask, a new `Board` method, and a new arm in `Model::handle`. It cannot be a new variant in a public command enum, because that enum does not exist.

`fan_in` is the only device decision. `attach`, `MemoryBoard::open`, and `MemoryBoard::choose` all call it. Zero candidates is `NoBoard`. One candidate loads. Many candidates become the choose screen and load on `Confirm`.

Opening from the choose screen reads one report per `Load::step`. Each `Tick` reads for about 60 ms, then returns `Step::Working`; the binary sends the next `Tick` at once, still taking keys between slices. Over Bluetooth a report takes tens of milliseconds and a 4-layer board with tap dances and combos takes about five seconds, so the open card shows a progress bar, the part being read (`keymap 14/40`), and `esc stops`. Esc returns to the list. A board that fails to open stays on the list with the reason on its card; a board that stops answering, as a sleeping Bluetooth board does, says `no answer: wake it, then enter`. The percentage never steps back as counts are learned.

The board opens once the keymap, encoders, and lock state are in (`Load::ready`); on a Bluetooth Cornix that is about 3 of the 5 seconds. The tap dances, combos, and macro buffer follow in `Board::read_rest`, one slice per `Tick`, while the keymap is usable. The header shows `◌ reading tap dances` meanwhile, without a count, so the screen repaints only when the part changes: a repaint between reports is a gap, and over Bluetooth a gap makes the next report take about half a second. Their tabs appear only once all three are read, so no editor ever shows a half-read list, and their saved copies arrive equal to the draft, so nothing looks unsaved. Background reads pause during an unlock hold and a save, whose ticks belong to them.

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
