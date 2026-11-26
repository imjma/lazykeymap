//! Editing a ZMK `.keymap` file end to end: the screen, the key list, a section tab, and `:w`.

use lazykeymap::{App, Event, TestScreen};

const KEYMAP: &str = include_str!("fixtures/corne.keymap");

fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn typed(app: &mut App<TestScreen>, text: &str) {
    for ch in text.chars() {
        app.handle(Event::Char(ch)).unwrap();
    }
}

#[test]
fn a_keymap_file_is_edited_in_place() {
    let dir = std::env::temp_dir().join(format!("lazykeymap-file-{}", std::process::id()));
    let config = dir.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let path = config.join("corne.keymap");
    std::fs::write(&path, KEYMAP).unwrap();

    // The zmk-config folder opens its one keymap.
    let mut app = App::new(
        lazykeymap::attach_file(&dir).unwrap(),
        TestScreen::new(130, 34),
    );
    let text = plain(app.screen().text());
    assert!(
        text.contains("L0 Base") && text.contains("Combos"),
        "{text}"
    );

    // A binding put together in the key list, a step at a time: the behavior, then each
    // parameter, as Keymap Editor's pickers do.
    app.handle(Event::Char('l')).unwrap();
    app.handle(Event::Char('/')).unwrap();
    for step in ["&thm", "LSH", "B"] {
        typed(&mut app, step);
        let text = plain(app.screen().text());
        // After the behavior, the list asks for its first parameter: a key, as `&thm` holds `&kp`.
        assert!(step != "LSH" || text.contains("key 1/2"), "{text}");
        app.handle(Event::Confirm).unwrap();
    }

    // A new combo on the Combos tab, after the seven layers.
    for _ in 0..7 {
        app.handle(Event::Tab).unwrap();
    }
    assert!(plain(app.screen().text()).contains("combo_lbkt"));
    typed(&mut app, ":add combo_q");
    app.handle(Event::Confirm).unwrap();
    typed(&mut app, ":w");
    app.handle(Event::Confirm).unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    let expected_key = KEYMAP.replacen("&kp Q          ", "&thm LSHIFT B  ", 1);
    assert!(written.starts_with(&expected_key[..expected_key.find("    combos {").unwrap()]));
    assert!(
        written.contains("        combo_q {\n            bindings = <&none>;\n"),
        "{written}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn one_part_of_a_binding_is_changed_and_the_rest_kept() {
    let dir = std::env::temp_dir().join(format!("lazykeymap-part-{}", std::process::id()));
    let config = dir.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let path = config.join("corne.keymap");
    std::fs::write(&path, KEYMAP).unwrap();
    let mut app = App::new(
        lazykeymap::attach_file(&dir).unwrap(),
        TestScreen::new(130, 34),
    );
    // `&thm LSHIFT A`: the list opens on its tap key, as Keymap Editor's parameter chip.
    typed(&mut app, "jl");
    app.handle(Event::Confirm).unwrap();
    let text = plain(app.screen().text());
    assert!(text.contains("&thm  LSHIFT  [A]"), "{text}");
    // Back to the hold modifier; LCTRL replaces it and the tap key stays.
    app.handle(Event::BackTab).unwrap();
    assert!(plain(app.screen().text()).contains("&thm  [LSHIFT]  A"));
    typed(&mut app, "LCTRL");
    app.handle(Event::Confirm).unwrap();
    typed(&mut app, ":w");
    app.handle(Event::Confirm).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("&thm CAPS ESC  &thm LCTRL A   &thm LCTRL S"),
        "{written}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_config_with_two_keymaps_asks_which() {
    let dir = std::env::temp_dir().join(format!("lazykeymap-two-{}", std::process::id()));
    let config = dir.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("corne.keymap"), KEYMAP).unwrap();
    std::fs::write(config.join("sweep.keymap"), KEYMAP).unwrap();
    let app = App::new(
        lazykeymap::attach_file(&dir).unwrap(),
        TestScreen::new(80, 20),
    );
    let text = plain(app.screen().text());
    assert!(
        text.contains("corne") && text.contains("sweep") && text.contains("choose"),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
