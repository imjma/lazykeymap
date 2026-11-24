#[test]
fn remap_persists() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .locked(&[(0, 0), (0, 1)]);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("locked"));
    assert!(app.screen().text().contains('▌'));
    assert!(app.screen().text().contains("NO"));
    assert!(!app.screen().text().contains("Hid"));
    // vial-qmk takes a plain keycode on a locked board. No unlock.
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Char('s'))?;
    app.handle(lazyvial::Event::Char('c'))?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("stage this change?"));
    assert!(app.screen().text().contains("Esc"));
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("1 unsaved"));
    app.handle(lazyvial::Event::Char(':'))?;
    app.handle(lazyvial::Event::Char('w'))?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(!app.screen().text().contains("hold "));
    assert!(!app.screen().text().contains("unsaved"));
    assert!(app.screen().text().contains('▌'));
    assert!(app.screen().text().contains("Esc"));
    drop(app);
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains('▌'));
    assert!(app.screen().text().contains("Esc"));
    Ok(())
}

#[test]
fn hold_marks_the_unlock_keys_on_a_full_board() -> lazyvial::Result<()> {
    let amber = "48;2;224;175;104";
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?
        .locked(&[(0, 0), (0, 1)]);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(!app.screen().text().contains(amber));
    for event in [
        lazyvial::Event::Char('/'),
        lazyvial::Event::Char('b'),
        lazyvial::Event::Char('o'),
        lazyvial::Event::Char('o'),
        lazyvial::Event::Char('t'),
        lazyvial::Event::Confirm,
        lazyvial::Event::Confirm,
        lazyvial::Event::Char(':'),
        lazyvial::Event::Char('w'),
        lazyvial::Event::Confirm,
    ] {
        app.handle(event)?;
    }
    // Amber key faces, not the UNLOCK badge on the status bar.
    let amber_keys = |text: &str| {
        text.lines()
            .any(|line| line.contains(amber) && !line.contains("UNLOCK"))
    };
    let text = app.screen().text();
    assert!(text.contains("hold 0,0+0,1"), "{text}");
    assert!(amber_keys(text), "{text}");
    app.handle(lazyvial::Event::Tick)?;
    assert!(!amber_keys(app.screen().text()));
    Ok(())
}

#[test]
fn pick_when_two() -> lazyvial::Result<()> {
    let alpha =
        lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?.named("alpha");
    let beta =
        lazyvial::MemoryBoard::from_definition(include_str!("fixtures/other.json"))?.named("beta");
    let mut app = lazyvial::App::new(
        lazyvial::MemoryBoard::choose(&[alpha, beta])?,
        lazyvial::TestScreen::new(40, 12),
    );
    assert!(app.screen().text().contains("choose"));
    assert!(app.screen().text().contains("> alpha"));
    app.handle(lazyvial::Event::Char('j'))?;
    assert!(app.screen().text().contains("> beta"));
    app.handle(lazyvial::Event::Left)?;
    assert!(app.screen().text().contains("> alpha"));
    app.handle(lazyvial::Event::Right)?;
    assert!(app.screen().text().contains("> beta"));
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("layer 1/"));
    assert!(app.screen().text().contains("beta"));
    Ok(())
}

#[test]
fn encoder_writes_clockwise() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/encoder.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 8));
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('a'))?;
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains('▌'));
    assert!(
        app.screen().text().contains("selected A"),
        "{}",
        app.screen().text()
    );
    Ok(())
}

#[test]
fn tick_does_not_repaint() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    let paints = app.screen().paints();
    app.handle(lazyvial::Event::Tick)?;
    assert_eq!(app.screen().paints(), paints);
    Ok(())
}

#[test]
fn neighbors_keep_a_space() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":2},"layouts":{"keymap":[["0,0","0,1"]]}}"#,
    )?;
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 8));
    let line = app
        .screen()
        .text()
        .lines()
        .find(|line| line.contains('▌') && line.contains("NO"))
        .expect("selected key");
    assert!(line.contains("48;2;31;31;36"), "{line}");
    assert!(line.contains("\u{1b}[0m "), "{line}");
    assert!(line.contains("NO"), "{line}");
    Ok(())
}

#[test]
fn pulled_row_shares_a_line() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":2},"layouts":{"keymap":[["0,0"],[{"y":-1,"x":1},"0,1"]]}}"#,
    )?;
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 8));
    let key_lines: Vec<_> = app
        .screen()
        .text()
        .lines()
        .filter(|line| line.contains("NO") && !line.contains("selected "))
        .collect();
    assert_eq!(key_lines.len(), 1, "{}", app.screen().text());
    Ok(())
}

#[test]
fn right_space_stays_on_the_thumb_row() -> lazyvial::Result<()> {
    // Cornix right Space: top-left is 1u below the thumb row, the key's top is not.
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":2},"layouts":{"keymap":[[{"y":3.3},"0,0\n\n\n\n\n\n\n\nRow"],[{"r":-23,"rx":5.005815,"ry":4.741697,"x":2.634185,"y":0.638303},"0,1\n\n\n\n\n\n\n\nSpace"]]}}"#,
    )?;
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(80, 16));
    let text = app.screen().text();
    assert!(
        text.lines()
            .any(|line| line.contains("Space") && line.contains("Row")),
        "{text}"
    );
    Ok(())
}

#[test]
fn tap_dance_shows_up_in_search() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(
        board.tap_dances(32).open()?,
        lazyvial::TestScreen::new(40, 16),
    );
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('t'))?;
    app.handle(lazyvial::Event::Char('d'))?;
    assert!(
        app.screen().text().contains("> TD(0)"),
        "{}",
        app.screen().text()
    );
    for _ in 0..25 {
        app.handle(lazyvial::Event::Down)?;
    }
    assert!(
        app.screen().text().contains("> TD(25)"),
        "{}",
        app.screen().text()
    );
    Ok(())
}

#[test]
fn enter_opens_the_catalog() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Confirm)?;
    let text = app.screen().text();
    assert!(text.contains("[Basic]"), "{text}");
    assert!(text.contains("> NO"), "{text}");
    app.handle(lazyvial::Event::Down)?;
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(
        app.screen().text().contains("selected TRNS"),
        "{}",
        app.screen().text()
    );
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Backspace)?;
    let text = app.screen().text();
    assert!(text.contains("[Basic]"), "{text}");
    assert!(text.contains("> NO"), "{text}");
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Char('s'))?;
    app.handle(lazyvial::Event::Char('c'))?;
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(
        app.screen().text().contains("selected Esc"),
        "{}",
        app.screen().text()
    );
    Ok(())
}

#[test]
fn arrows_follow_the_drawn_keys() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(120, 24));
    let start_row = bracket_row(app.screen().text());
    let start_col = bracket_col(app.screen().text());
    app.handle(lazyvial::Event::Right)?;
    assert_eq!(bracket_row(app.screen().text()), start_row);
    assert!(bracket_col(app.screen().text()) > start_col);
    app.handle(lazyvial::Event::Left)?;
    assert_eq!(bracket_col(app.screen().text()), start_col);
    app.handle(lazyvial::Event::Down)?;
    let mid = bracket_row(app.screen().text());
    assert!(mid > start_row);
    app.handle(lazyvial::Event::Down)?;
    app.handle(lazyvial::Event::Down)?;
    let low = bracket_row(app.screen().text());
    assert!(low > mid, "{}", app.screen().text());
    app.handle(lazyvial::Event::Up)?;
    assert!(bracket_row(app.screen().text()) < low);
    Ok(())
}

fn help_sits_on_the_board(text: &str) -> bool {
    text.lines().any(|line| {
        let Some(prefix) = line.split("╭─ help ").next() else {
            return false;
        };
        line.contains("╭─ help ") && visible_columns(prefix) > 0
    })
}

fn bracket_row(text: &str) -> usize {
    text.lines()
        .position(|line| line.contains('▌'))
        .expect(text)
}

fn bracket_col(text: &str) -> usize {
    let line = text.lines().find(|line| line.contains('▌')).expect(text);
    let prefix = line.split('▌').next().unwrap_or("");
    visible_columns(prefix)
}

fn visible_columns(text: &str) -> usize {
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

#[test]
fn layer_key_keeps_its_color() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":3},"layouts":{"keymap":[["0,0","0,1","0,2"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.layers(2).open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('/'))?;
    for ch in ['m', 'o', '(', '1', ')'] {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    assert!(
        app.screen().text().contains("> MO(1)"),
        "{}",
        app.screen().text()
    );
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Right)?;
    let text = app.screen().text();
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
    assert!(text.contains('▌'), "{text}");
    assert!(text.contains("48;2;122;162;247"), "{text}");
    assert!(text.contains("48;2;31;31;36"), "{text}");
    Ok(())
}

#[test]
fn vim_moves_like_arrows_on_corne() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(120, 24));
    let start_row = bracket_row(app.screen().text());
    let start_col = bracket_col(app.screen().text());
    app.handle(lazyvial::Event::Char('l'))?;
    assert_eq!(bracket_row(app.screen().text()), start_row);
    assert!(bracket_col(app.screen().text()) > start_col);
    app.handle(lazyvial::Event::Char('h'))?;
    assert_eq!(bracket_col(app.screen().text()), start_col);
    app.handle(lazyvial::Event::Char('j'))?;
    let down = bracket_row(app.screen().text());
    assert!(down > start_row);
    app.handle(lazyvial::Event::Char('k'))?;
    assert!(bracket_row(app.screen().text()) < down);
    app.handle(lazyvial::Event::Char('G'))?;
    assert!(bracket_row(app.screen().text()) > start_row);
    app.handle(lazyvial::Event::Char('g'))?;
    assert_eq!(bracket_row(app.screen().text()), start_row);
    assert_eq!(bracket_col(app.screen().text()), start_col);
    Ok(())
}

#[test]
fn map_typing_x_does_not_open_search() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('x'))?;
    let text = app.screen().text();
    assert!(!text.lines().any(|line| line == "search"), "{text}");
    assert!(
        !text.lines().any(|line| line.starts_with("search ")),
        "{text}"
    );
    Ok(())
}

#[test]
fn feature_bits_gate_keys() -> lazyvial::Result<()> {
    let json = r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#;
    let search = |board: lazyvial::MemoryBoard, query: &str| -> lazyvial::Result<String> {
        let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
        app.handle(lazyvial::Event::Char('/'))?;
        for ch in query.chars() {
            app.handle(lazyvial::Event::Char(ch))?;
        }
        Ok(app.screen().text().to_string())
    };
    for query in ["layer lock", "caps word", "repeat"] {
        let off = search(lazyvial::MemoryBoard::from_definition(json)?, query)?;
        assert!(!off.contains("> "), "{off}");
        let on = search(
            lazyvial::MemoryBoard::from_definition(json)?.features(0b11, 1),
            query,
        )?;
        assert!(on.contains("> "), "{on}");
    }
    Ok(())
}

#[test]
fn shelves_switch_and_typing_searches() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(
        board.macros(2).features(0b10, 0).tap_dances(32).open()?,
        lazyvial::TestScreen::new(40, 16),
    );
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Right)?;
    let iso = app.screen().text();
    assert!(iso.contains("[ISO/JIS]"), "{iso}");
    app.handle(lazyvial::Event::Right)?;
    let layers = app.screen().text();
    assert!(layers.contains("[Layers]"), "{layers}");
    assert!(layers.contains("> Layer Lock"), "{layers}");
    app.handle(lazyvial::Event::Right)?;
    let quantum = app.screen().text();
    assert!(quantum.contains("[Quantum]"), "{quantum}");
    assert!(quantum.contains("> Bootloader"), "{quantum}");
    for _ in 0..4 {
        app.handle(lazyvial::Event::Left)?;
    }
    let macros = app.screen().text();
    assert!(macros.contains("[Macro]"), "{macros}");
    assert!(macros.contains("> M0"), "{macros}");
    app.handle(lazyvial::Event::Left)?;
    app.handle(lazyvial::Event::Left)?;
    let dance = app.screen().text();
    assert!(dance.contains("[Tap Dance]"), "{dance}");
    assert!(dance.contains("> TD(0)"), "{dance}");
    app.handle(lazyvial::Event::Char('t'))?;
    app.handle(lazyvial::Event::Char('d'))?;
    let found = app.screen().text();
    assert!(found.contains("search td"), "{found}");
    assert!(found.contains("> TD(0)"), "{found}");

    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Left)?;
    app.handle(lazyvial::Event::Left)?;
    app.handle(lazyvial::Event::Left)?;
    let text = app.screen().text();
    assert!(text.contains("[App, Media and Mouse]"), "{text}");
    Ok(())
}

#[test]
fn kc_code_takes_a_basic_key() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.layers(3).open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('/'))?;
    for ch in "lt1(".chars() {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    app.handle(lazyvial::Event::Confirm)?;
    let text = app.screen().text();
    assert!(text.contains("LT1(kc) > [Basic]"), "{text}");
    for ch in "mo(2".chars() {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    app.handle(lazyvial::Event::Confirm)?;
    let text = app.screen().text();
    assert!(text.contains("LT1(kc) > search mo(2"), "{text}");
    for _ in 0..4 {
        app.handle(lazyvial::Event::Backspace)?;
    }
    for ch in "esc".chars() {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::Confirm)?;
    let text = app.screen().text();
    assert!(text.contains("selected L1(Esc)"), "{text}");
    Ok(())
}

#[test]
fn slash_opens_search() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('/'))?;
    let text = app.screen().text();
    assert!(text.contains("[Basic]"), "{text}");
    Ok(())
}

#[test]
fn help_overlays_bindings() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(120, 80));
    app.handle(lazyvial::Event::Char('?'))?;
    let text = app.screen().text();
    assert!(text.lines().any(|line| line.contains("╭─ help ")), "{text}");
    assert!(help_sits_on_the_board(&text), "{text}");
    assert!(text.contains("layer "), "{text}");
    assert!(text.contains("board · search · choose · hold"), "{text}");
    assert!(text.contains("left"), "{text}");
    assert!(text.contains("assign"), "{text}");
    assert!(text.contains("choose"), "{text}");
    assert!(text.contains("cancel"), "{text}");
    app.handle(lazyvial::Event::Cancel)?;
    let back = app.screen().text();
    assert!(!back.lines().any(|line| line == "help"), "{back}");
    assert!(back.contains('▌'), "{back}");
    app.handle(lazyvial::Event::Char('/'))?;
    app.handle(lazyvial::Event::Char('e'))?;
    app.handle(lazyvial::Event::Char('?'))?;
    let during = app.screen().text();
    assert!(
        during.lines().any(|line| line.contains("╭─ help ")),
        "{during}"
    );
    assert!(help_sits_on_the_board(&during), "{during}");
    assert!(during.contains("layer "), "{during}");
    assert!(during.contains("assign"), "{during}");
    app.handle(lazyvial::Event::Cancel)?;
    assert!(app.screen().text().contains("search e"));
    app.handle(lazyvial::Event::Cancel)?;
    app.handle(lazyvial::Event::Char('?'))?;
    app.handle(lazyvial::Event::Char('q'))?;
    let closed = app.screen().text();
    assert!(!closed.lines().any(|line| line == "help"), "{closed}");
    assert!(closed.contains('▌'), "{closed}");
    assert_eq!(
        app.handle(lazyvial::Event::Char('q'))?,
        lazyvial::Step::Quit
    );
    Ok(())
}

#[test]
fn q_quits_from_map() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert_eq!(
        app.handle(lazyvial::Event::Char('q'))?,
        lazyvial::Step::Quit
    );
    Ok(())
}

#[test]
fn q_types_in_search() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 16));
    app.handle(lazyvial::Event::Char('/'))?;
    assert_eq!(
        app.handle(lazyvial::Event::Char('q'))?,
        lazyvial::Step::Continue
    );
    assert!(app.screen().text().contains("search q"));
    Ok(())
}

#[test]
fn layers_wrap_with_brackets() -> lazyvial::Result<()> {
    let board =
        lazyvial::MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?.layers(2);
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("layer 1/"));
    app.handle(lazyvial::Event::Char(']'))?;
    assert!(app.screen().text().contains("layer 2/"));
    app.handle(lazyvial::Event::Char('['))?;
    assert!(app.screen().text().contains("layer 1/"));
    Ok(())
}

#[test]
fn catalog_endpoints_with_home_and_end() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{"matrix":{"rows":1,"cols":1},"layouts":{"keymap":[["0,0"]]}}"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(40, 24));
    app.handle(lazyvial::Event::Confirm)?;
    app.handle(lazyvial::Event::End)?;
    assert!(app.screen().text().contains("> ?"));
    app.handle(lazyvial::Event::Home)?;
    assert!(app.screen().text().contains("> NO"));
    Ok(())
}

#[test]
fn corne_draws_four_rows() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?;
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(120, 24));
    let text = app.screen().text();
    assert!(text.contains("selected NO"), "{text}");
    let key_lines: Vec<_> = text
        .lines()
        // Key rows only: not the status bar, and not the key details under the board.
        .filter(|line| line.contains("NO") && !line.contains("selected ") && !line.contains("0x"))
        .collect();
    assert_eq!(key_lines.len(), 4, "{text}");
    let nos: usize = key_lines
        .iter()
        .map(|line| line.matches("NO").count())
        .sum();
    assert_eq!(nos, 46, "{text}");
    assert!(key_lines.iter().any(|line| line.contains("   ")), "{text}");
    Ok(())
}

#[test]
fn multi_line_custom_names_stay_on_one_line() -> lazyvial::Result<()> {
    let board = lazyvial::MemoryBoard::from_definition(
        r#"{
          "name": "bt",
          "matrix": { "rows": 1, "cols": 1 },
          "customKeycodes": [{ "name": "BT_NXT", "shortName": "Next\nBT" }],
          "layouts": { "keymap": [["0,0"]] }
        }"#,
    )?;
    let mut app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    for ch in "/next bt".chars() {
        app.handle(lazyvial::Event::Char(ch))?;
    }
    let text = app.screen().text();
    assert!(text.contains("> Next BT"), "{text}");
    assert!(text.lines().count() <= 16, "{text}");
    Ok(())
}
