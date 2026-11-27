# lazykeymap

Remap a [Vial](https://get.vial.today) or [ZMK Studio](https://zmk.dev/docs/features/studio) keyboard from the terminal: the keymap on every layer, and on Vial also tap dances, combos, and macros.

## Install

With Homebrew, on macOS or Linux:

```sh
brew tap imjma/lazykeymap https://github.com/imjma/lazykeymap
brew trust imjma/lazykeymap
brew install lazykeymap
```

Homebrew 7 refuses formulae from a tap until you trust it, hence the middle line. `brew upgrade lazykeymap` picks up new releases.

From source, with a Rust toolchain:

```sh
cargo install --git https://github.com/imjma/lazykeymap
```

On Linux, the build needs `libudev` and `libdbus` (`libudev-dev` and `libdbus-1-dev` on Debian and Ubuntu) and `pkg-config`, and reading the keyboard needs a udev rule that gives your user access to its hidraw device, the same rule Vial asks for. A ZMK board's serial port needs your user in the `dialout` group (`uucp` on Arch); ZMK over Bluetooth goes through BlueZ.

### Arch Linux and Omarchy

Build it with Cargo from Arch's own packages (Omarchy already has `systemd-libs` and `dbus`):

```sh
sudo pacman -S --needed rust pkgconf
cargo install --locked --git https://github.com/imjma/lazykeymap --tag v0.6.0
```

It lands in `~/.cargo/bin`; add that to your `PATH` if `lazykeymap` is not found. With Rust from `mise` instead, `mise use -g rust` and leave `rust` out of the `pacman` line. To update, run the `cargo install` line again with the new tag and `--force`.

Then let your user reach the keyboard:

```sh
# Vial keyboards: the udev rule Vial asks for. Replug the keyboard after.
echo 'KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{serial}=="*vial:f64c2b3c*", MODE="0660", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/60-lazykeymap-vial.rules
sudo udevadm control --reload && sudo udevadm trigger

# ZMK Studio keyboards over USB: the serial-port group. Log out and back in after.
sudo usermod -aG uucp "$USER"
```

A ZMK keyboard over Bluetooth needs no setup beyond pairing it (Omarchy's Bluetooth menu, or `bluetoothctl`).

## Use

Plug in a Vial keyboard, or connect a ZMK keyboard built with Studio support by USB or Bluetooth, and run `lazykeymap`. To edit a ZMK keymap file instead, name it or its zmk-config folder: `lazykeymap config/corne.keymap`. Pick the keyboard, then:

| Keys | Does |
|---|---|
| `h` `j` `k` `l`, arrows | move between keys |
| `tab`, `shift-tab` | next and previous layer, then the Tap Dance, Combos, and Macros tabs (on a keymap file: Combos, Macros, Behaviors, Conditional, and Encoders) |
| `enter` or `/` | pick a keycode for the selected key. On Vial the Basic group is drawn as a keyboard, as in Vial: arrows move to the key beside, above, or below, and `NO`, `TRNS`, and the keys a keyboard has no room for follow under it |
| `u` | undo the last change |
| `:w` | write every change to the keyboard |
| `:wq`, `:q`, `:q!`, `:e!` | write and quit, quit, quit without writing, discard changes |
| `?` | every key, for the screen you are on |
| `space` or `ctrl-k` | find anything: run a command, go to a layer or tab, or find a key on any layer by name. `:` opens the same list showing only commands |
| `:layer-add`, `:layer-remove`, `:layer-rename Sym`, `:layer-left`, `:layer-right` | add a layer, remove this one, name it, or move it one place (ZMK Studio and keymap files) |
| `:layout`, `:layout 1` | list a ZMK board's physical layouts, or switch to one; the keyboard is read again |
| `:e config/corne.keymap` | on a ZMK Studio keyboard: link the keymap file (or zmk-config folder) it runs, so `:w` writes both; the board is read again from the file |
| `:export`, `:export light`, `:export dark ~/corne.svg` | tick the layers to draw (all start ticked; `space` or a click ticks one, `a` all or none), then `enter` saves them as one SVG image, dark or light, at the path or as `<board>-keymap-<theme>.svg` in the current folder; unsaved changes are drawn as staged |

The mouse works too. What a click would act on lights up under the pointer. Click a key, a tab, a cell, a board, or a hint in the bottom bar to select it; click what is already selected to do what Enter does there (open the key list, edit the cell, open the board). In the key list one click shows the keycode on the key in amber and a second click stages it, so a stray click changes nothing; clicking another key on the board moves the list to that key. The wheel changes layer over the board and scrolls a list under it; right-click is `esc`. Hold Shift (Option in some macOS terminals) to select text as usual.

Changes stay in a draft, shown in green, until `:w`. Quitting with unsaved changes asks first. A locked keyboard asks you to hold its unlock keys only when a change needs it: the bootloader key, or macros.

### ZMK Studio

lazykeymap talks to ZMK the way [ZMK Studio](https://github.com/zmkfirmware/zmk-studio) does, over the board's USB serial port or over Bluetooth. The firmware needs `CONFIG_ZMK_STUDIO=y`, plus the `studio-rpc-usb-uart` snippet for USB, as in the [ZMK Studio docs](https://zmk.dev/docs/features/studio).

- USB: lazykeymap opens only serial ports with ZMK's default USB id (`1d50:615e`) or a maker name containing `ZMK`, so it never resets some other device on a serial port. For a board with its own USB id, name its port: `lazykeymap --port /dev/cu.usbmodem1101` (macOS), `/dev/ttyACM0` (Linux), or `COM3` (Windows).
- Bluetooth: pair the keyboard with the computer first and make Bluetooth its active output; Studio answers only on the output the keyboard is typing on. On macOS, the first run asks to let your terminal use Bluetooth. On Windows, the board is found only while it advertises.
- Studio starts locked. Pick the keyboard with Enter; its card then says `press the keyboard's Studio unlock key: it opens by itself`, and pressing the key bound to `&studio_unlock` opens it, no second Enter. Quitting locks the board again, which is the firmware default (`CONFIG_ZMK_STUDIO_LOCK_ON_DISCONNECT`). A board that locks itself after a long idle holds `:w` until the same key is pressed, then finishes the write.
- A board that will not open: run `LAZYKEYMAP_TRACE=trace.txt lazykeymap`, try again, and attach `trace.txt` to an issue. It has one line per Studio call, with how long it took and how it ended.
- Each key shows its ZMK binding, as Keymap Editor does: `&kp A` is `A`, and any other behavior sits over its parameters, with layers by name and modifiers as ⇧⌃⌥⌘ (`mo` over `Sym`, `lt Sym` over `Space`, `mt` over `⇧ B`). The line under the board spells the binding out, `&mt LSHIFT B`.
- Enter on a key opens its binding in columns, one per part, as Keymap Editor's chips: the behavior, then each parameter, named by role (`hold · modifier`, `tap · key`, `layer`). The panel's title is the binding being built. ←/→ (or Tab) move between columns and ↑/↓ choose, live; typing narrows the active column. The behavior column lists every behavior the keyboard reported, its own hold-taps too; a parameter column lists what it takes: keys once each by the name keymaps use (`N1`, `LSHIFT`, `BSPC`, with their aliases beside and searchable), layers by name, or commands. Changing one column keeps the others while they still fit, so `&mt LSHIFT A` takes a new tap key with ↓ and Enter. Enter stages the binding once every column holds a value, and otherwise moves to the first column still to choose; typing `&` starts the binding over. Parameters use ZMK's keymap names (`&bt BT_SEL 1`, `&out OUT_USB`); mouse move, mouse scroll, and external power, which ZMK sends no parameter list for, take keymap text such as `MOVE_X(-2500)`, `SCRL_DOWN`, or `EP_TOG`.
- The key list offers only what the keyboard can store. `:w` saves each change to the keyboard's flash, like Studio's Save.
- Layers can be added, removed, renamed, and moved with the `:layer-` commands; the keyboard keeps each change at once and is read again. Write or discard staged key changes first. `:layout` lists the board's physical layouts; `:layout 1` or `:layout five` switches and reads the keyboard again.

### ZMK keymap files

`lazykeymap config/corne.keymap` edits the file the way [Keymap Editor](https://github.com/nickcoutsos/keymap-editor) does, in the terminal: `:w` writes the file, and you build and flash as usual. Only the bytes of what changed are rewritten: comments, `#define`s, and the grid the layers are written in stay as they are, and a binding that grows widens its column.

With the ZMK Studio keyboard that runs this keymap connected (USB or Bluetooth), lazykeymap links the two: the screen shows the file, and `:w` writes each key to the keyboard first, then to the file, so the keyboard changes now and the file stays the source for your next build. They link when the keyboard holds the same keymap: the same layers, the same keys, and at every key the same binding (a behavior the file calls `&hm` and Studio calls by its display name is matched up from the keymap itself). The card then reads `corne + Corne`. When they differ, the card says where (`not linked to Corne: layer 1 key 2: the file has &kp Q, the keyboard &kp Y`) and only the file is edited. Combos, macros, and behaviors are written to the file only, since Studio cannot change them; flash to get them onto the keyboard.

Already in a ZMK Studio keyboard? `:e ~/zmk-config/config/corne.keymap` does the same from inside: it compares, links, and reads the board again from the file, or says where they differ and leaves the board as it was. Write or discard unsaved changes first.

- The layout is drawn from `config/<keyboard>.json` or `config/info.json` (Keymap Editor's layout file, QMK `info.json` keys), else from how the first layer's bindings are laid out in the file.
- Keys show and edit as on a Studio board: each key is its binding, behavior over parameters, and Enter opens it in columns, one per part. A cell on the Combos, Macros, Behaviors, Conditional, or Encoders tab that holds a binding edits the same way.
- The behavior column offers ZMK's own behaviors and the file's hold-taps and macros; a parameter column offers keys, modifiers, layers by name, Bluetooth, output, underglow, and power commands, profiles, mouse buttons, mouse move, and mouse scroll. Typing narrows the active column; `&thm`, `enter`, `LSH`, `enter`, `B`, `enter` stages `&thm LSHIFT B`. A modifier function such as `LG(C)` or `LC(LS(TAB))` is a hit when it is a real key. A word the lists do not hold, such as a `#define`d layer, stays as written while its column is left alone.
- A zmk-config folder with several keymaps opens the choose screen, one card per keymap.
- `:layer-remove` asks first, since it changes the keyboard or file at once.
- A `#define` used as bindings, such as `HRML(A, S, D, F)`, shows as the keys it stands for. Editing one of them writes the invocation out as plain bindings; leaving them alone keeps the `#define`.
- Tabs after the layers edit the file's other parts, one row per node: Combos (binding, key positions, timeout, layers, prior idle, slow release), Macros (wait and tap times, then the bindings), Behaviors (compatible, binding cells, flavor, tapping term, quick tap, prior idle, hold-trigger positions and on-release, mods, then the bindings), Conditional (if-layers, then-layer), and Encoders (each layer's `sensor-bindings`). `enter` edits a cell, `x` clears it, `t` adds a binding at the cursor, `H` `L` move one, `enter` on a flag sets or clears it. `:add name` adds a row and `:remove` removes one.
- A macro whose bindings use `&macro_param_1to1` and the like becomes a one- or two-parameter macro by itself.
- Renaming a behavior or macro renames every `&` that uses it. One still in use is not removed, nor is a layer that bindings still point at; moving or removing a layer renumbers the `&mo`, `&lt`, `&tog`, `&to`, `&sl` bindings, combo `layers`, and conditional layers that point past it.

## License

MIT. See [LICENSE](LICENSE). The keycode table is generated by `scripts/keycodes.py`, which takes only keycode identifiers and numbers from QMK; its legends and grouping are this project's own. The ZMK key-name table is generated by `scripts/zmk_keys.py` from ZMK's MIT-licensed `keys.h`.

## Releasing

1. Bump `version` in `Cargo.toml`, commit, and push `main`.
2. Tag it and hash the tarball GitHub serves for the tag:

   ```sh
   git tag -a v0.2.0 -m "lazykeymap 0.2.0" && git push origin v0.2.0
   curl -sL https://github.com/imjma/lazykeymap/archive/refs/tags/v0.2.0.tar.gz | shasum -a 256
   ```

3. Put the new `url` and `sha256` in `Formula/lazykeymap.rb`, and the new tag in the Arch section's `cargo install` line above, then commit and push `main`.
