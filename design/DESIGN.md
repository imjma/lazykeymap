# lazyvial

## Problem

lazyvial is a terminal program that remaps a plugged-in Vial keyboard. The repo has no application code. The grounding file is the constraint set. A keycode is a `u16`. Matrix keys live in one row-major buffer of `layers * rows * cols` codes, big-endian on the wire. Encoder directions live in a separate get and set under the `0xFE` prefix. Firmware starts locked, and a locked board ignores keymap writes until the unlock hold finishes. The drawing is the KLE keymap inside the LZMA definition, minus keys the current layout-option dword hides. A board with no definition is a failure, and the program must not invent a grid. Writes are immediate. Quit leaves the keyboard as last written.

Lighting, macros, tap dance, combos, key overrides, QMK settings, layout-option edits, keymap import and export, and firmware flashing are out of this tool. The type layout has to make each of those a new private codec plus a new caller, so a later edit cannot grow them by adding a match arm the screen already touches.

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

Arrows move the selection. `NextLayer` and `PrevLayer` wrap the layer. A character opens search. `Backspace` edits the query. `Confirm` assigns the highlighted name to the selected place. `Cancel` closes search, or abandons a hold, without a write. `Tick` polls a hold. `Quit` drops the handle. Quit sends no lock command.

The painted string is the contract. Line 1 is `layer {n}/{count} {locked|ready} {name}` with `n` starting at 1. A hold inserts `hold {row},{col}+{row},{col}` under that, pairs in status-report order, `255,255` omitted. A search inserts `search {query}` and one legend per line, the highlighted legend prefixed with `> `. Then one line per KLE row. Tokens on a row are sorted by `x`. The selected legend is wrapped in `[]`. A decal is wrapped in `()`. A hidden key is absent. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. A choose screen is only `choose`, then one board name per line, the highlighted name prefixed with `> `. A confirm that cannot load a definition leaves the list up and appends `unavailable` to that name. Lines are clipped to the screen size and are not wrapped.

Search ranking is case-insensitive. A legend or token that starts with the query ranks before a substring. A shorter legend wins a tie. The remaining order is the catalog, basic names first and definition custom names after. `Confirm` assigns the highlighted hit. The fixture below has its first selectable key under the cursor, and a key whose legend is `Hid` with label 8 equal to `0,1`. The default layout-option dword is 0, which selects choice 0 of each group, so `Hid` is omitted.

```rust
fn remap_persists() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .locked(&[(0, 0), (0, 1)]);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("locked"));
    assert!(app.screen().text().contains("[NO]"));
    assert!(!app.screen().text().contains("Hid"));
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Char('s'))?;
    app.handle(lazyvial::Event::Char('c'))?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("hold 0,0+0,1"));
    app.handle(lazyvial::Event::Tick)?;
    assert!(app.screen().text().contains("[Esc]"));
    assert!(app.screen().text().contains("ready"));
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
    app.handle(lazyvial::Event::Down)?;
    assert!(app.screen().text().contains("> beta"));
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("layer 1/"));
    assert!(app.screen().text().contains("beta"));
    Ok(())
}
```

`choose` with one board skips the list and paints the layout. `choose` with none returns `Error::NoBoard`. `attach` uses that same decision. One real board loads immediately. Several real boards paint `choose`. Zero Vial boards return `Error::NoBoard`. One board whose id answers and whose definition does not return `Error::NoLayout`, and the binary prints that error. It does not draw a rectangle of empty keys.

The cap token is the keycode legend. `KC_NO` prints `NO`. `KC_ESC` prints `Esc`. `KC_A` prints `A`. A non-empty KLE label 1 replaces that legend. Tests live next to these fixtures.

`tests/fixtures/mini.json` is a 2 by 2 matrix. Group 0 is named `Side` and its choices are `Shown`, then `Hid`. The dword `0` selects choice 0. The key at `0,1` has label 1 `Hid` and label 8 `0,1`, so the painter omits it. The key at `0,0` is the first selectable key.

```json
{
  "name": "mini",
  "matrix": { "rows": 2, "cols": 2 },
  "layouts": {
    "labels": [["Side", "Shown", "Hid"]],
    "keymap": [["0,0", "0,1\nHid\n\n\n\n\n\n\n\n0,1"]]
  }
}
```

`tests/fixtures/encoder.json` is one clockwise encoder. Label 4 is `e`. Label 0 is `0,0`. An unlocked board writes that direction on `Confirm` with no `Tick`.

```rust
fn encoder_writes_clockwise() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/encoder.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 8));
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
  "layouts": { "keymap": [["0,0\n\n\n\ne"]] }
}
```

## Shape

The public API is `attach`, `Attached`, `MemoryBoard`, `App`, `Event`, `Step`, `Screen`, `TestScreen`, and `Error`. Interface depth is the point of the cut. That list hides device discovery, the 32-byte report codec, the two response grammars, LZMA, KLE, the layout-option filter, the keycode catalog, unlock, and the choice between a matrix write and an encoder write. The caller is left with events and the painted string, because those are the product loop. A `Screen` receives the finished string and cannot paint from a frame of its own.

`App::handle` paints. It does not decide what a key means. `Model::handle` in the session module is the only match on `Event`. `App` is the shell that copies `Model::text` onto the `Screen`, per boundary-discipline. `TestScreen` stores that string. `Terminal` prints it.

`Model` is a private enum of pick, map, search, and hold. Map and hold are `Session<Mapping>` and `Session<Holding>`. Search holds `Session<Mapping>` plus the query. The phase parameter is the pending write, per model-the-domain. `Holding` stores the place, the code, and the prompt. There is no `Option` of a pending code on the mapping session. `poll` exists only on `Session<Holding>`. `search` exists only on `Session<Mapping>`. `Hit` lives in the session module. Its `u16` is private, and only `search` constructs a `Hit`. `assign` consumes a `Hit`. The screen can pass a hit back. It cannot mint a code, so a keymap file has no constructor to target.

The keymap has one owner, the `Board` inside the session, per foundational-thinking. Paint derives a frame and drops it. The frame is not a second keymap. `Place` is a private enum, `Matrix { row, col }` or `Encoder { index, direction }`, with direction 0 clockwise and 1 counterclockwise. `commit` matches that enum and sends either set-key `0x05` or vial set-encoder `0x04`. Callers above `vial` name a `Place` the parser already built. They do not choose a command. `App` cannot name `Place`, `Board`, or `Ask`.

Modules are nested so the compiler keeps it that way, per encode-lessons-in-structure and type-system-discipline. `layout` and `catalog` sit inside `vial`. `vial` sits inside `session`. `commit`, `Ask`, and the report constants are `pub(super)` or private. `app` can call `Model`. `app` cannot call `commit`. Adding a lighting write means a new private ask, a new `Board` method, and a new arm in `Model::handle`. It cannot be a new variant in a public command enum, because that enum does not exist.

`fan_in` is the only device decision. `attach`, `MemoryBoard::open`, and `MemoryBoard::choose` all call it. Zero candidates is `NoBoard`. One candidate loads. Many candidates become the choose screen and load on `Confirm`.

`visible` drops keys the layout-option dword does not select, so the cap list the painter walks has no hidden key and no second filter flag. Groups come from `layouts.labels`, in order. Each group is `[name, choice, choice, ...]`. The group uses `(choice_count - 1).bit_length()` bits, packed from the low end of the `u32`. A key with no label 8 stays. A key with label 8 `index,choice` stays when that group decodes to `choice`. A label index the definition does not declare fails the open. The dword is read once, with VIA get-keyboard-value `0x02` and value id `0x02`, and stored on the board. Nothing in `session` or `app` can replace it. `MemoryBoard::layout_options` only sets the fake device's answer before open.

KLE metadata objects carry `x`, `y`, `w`, `h` onto the next key. Twelve labels are split on newlines. Label 0 is `row,col` for a normal key and `index,direction` when label 4 is `e`. A decal has no place. A matrix address outside `matrix.rows` or `matrix.cols` fails the open. The parser keeps KLE rows as the line breaks. Within a row, `render` sorts by `x` and starts each token at `round(x * 4)` columns, pushed two spaces right on overlap. `y`, `w`, and `h` stay on the cap for that one function. Negative `x` clamps the column to 0.

The catalog is one `Vec` built at load from the static basic table plus the definition's custom codes. `KC_RESET` is the only basic entry with `hold_first`. Its number comes from that table when the table is filled, and is checked against the QMK list rather than copied from the grounding note. `legend` is the short name. Search walks this vec once.

`begin_hold` runs on assign. It reads unlock status. If the board is locked, or the code is `hold_first`, it sends unlock start and returns the hold. It does not write the key. `commit` is the only function that sends set-key or set-encoder. If the cached code already equals the request, `commit` sends nothing and reports same, per make-operations-idempotent. If status is still locked, `commit` sends nothing and returns the hold. After a successful report, `commit` updates that one cell. `poll` sends unlock poll, then `commit` of the pending place. A second `Confirm` of the same code finds the cell already set. A crash drops the cache. The device keeps the last report that was acknowledged. The next open reads the buffer and the encoder slots again. Unlock-in-progress with no pending hit dies with the process.

The fake device is an `Io` beside HID. Both speak 32-byte reports. `Ask::grammar` returns `Echo` or `Overwrite`. `decode` matches that value. It does not sniff a shared header. `Echo` is layer count, layout options, the keymap chunk, and set-key. `Overwrite` is keyboard id, definition size, definition blocks, encoder get and set, and the unlock asks. The fake encodes replies with that same codec. `MemoryBoard::from_definition` parses the JSON, sizes the matrix to `KC_NO`, compresses the document, and serves the compressed bytes back through get-definition. Load always LZMA-decompresses, including in tests. HID prepends the `0x00` report id around the 32 bytes. The id never enters `encode`.

The store is `Arc<RefCell<...>>` and is `!Send`. One thread is the whole program, per separate-before-serializing-shared-state. `open` sets a flag. A second open returns `Busy`. Drop clears the flag. There is no mutex. The query string lives on the search phase, not on the board.

via-protocol 0.1.0 is rejected. The grounding file limits its known coverage to discovery, keymap buffer reads, and keycode name groups. Definition, unlock, encoders, and the layout-option dword were not established there. Those are this program. Taking the crate for the half it might cover, and writing the rest beside it, is two codecs. The usage-page `0xFF60` / usage `0x61` discovery and the basic keycode names are small enough to own here, in the same module as the asks those names are written through.

Out of scope stays out because the public API has no command trait, the private `Ask` enum has no lighting, macro, tap-dance, combo, override, QMK-setting, buffer-set, or lock variant, and layout options have no setter after open. Keymap import has no `Hit` constructor and no buffer write.

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
