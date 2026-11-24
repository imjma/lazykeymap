# lazyvial

Remap a [Vial](https://get.vial.today) keyboard from the terminal: the keymap on every layer, tap dances, combos, and macros.

## Install

With Homebrew, on macOS or Linux:

```sh
brew tap imjma/lazyvial https://github.com/imjma/lazyvial
brew trust imjma/lazyvial
brew install lazyvial
```

Homebrew 7 refuses formulae from a tap until you trust it, hence the middle line. `brew upgrade lazyvial` picks up new releases.

From source, with a Rust toolchain:

```sh
cargo install --git https://github.com/imjma/lazyvial
```

On Linux, the build needs `libudev` (`libudev-dev` on Debian and Ubuntu) and `pkg-config`, and reading the keyboard needs a udev rule that gives your user access to its hidraw device, the same rule Vial asks for.

## Use

Plug in a Vial keyboard and run `lazyvial`. Pick the keyboard, then:

| Keys | Does |
|---|---|
| `h` `j` `k` `l`, arrows | move between keys |
| `tab`, `shift-tab` | next and previous layer, then the Tap Dance, Combos, and Macros tabs |
| `enter` or `/` | pick a keycode for the selected key |
| `u` | undo the last change |
| `:w` | write every change to the keyboard |
| `:wq`, `:q`, `:q!`, `:e!` | write and quit, quit, quit without writing, discard changes |
| `?` | every key, for the screen you are on |

Changes stay in a draft, shown in green, until `:w`. Quitting with unsaved changes asks first. A locked keyboard asks you to hold its unlock keys only when a change needs it: the bootloader key, or macros.

## License

MIT. See [LICENSE](LICENSE).

## Releasing

1. Bump `version` in `Cargo.toml`, commit, and push `main`.
2. Tag it and hash the tarball GitHub serves for the tag:

   ```sh
   git tag -a v0.2.0 -m "lazyvial 0.2.0" && git push origin v0.2.0
   curl -sL https://github.com/imjma/lazyvial/archive/refs/tags/v0.2.0.tar.gz | shasum -a 256
   ```

3. Put the new `url` and `sha256` in `Formula/lazyvial.rb`, commit, and push `main`.
