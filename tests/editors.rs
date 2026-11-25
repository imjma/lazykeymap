use lazyvial::{App, Event, MemoryBoard, Step, TestScreen};

fn press(app: &mut App<TestScreen>, keys: &str) -> lazyvial::Result<()> {
    for ch in keys.chars() {
        app.handle(Event::Char(ch))?;
    }
    Ok(())
}

/// The sheet row that mentions `needle`, not the status bar under it that names the cursor cell.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .rfind(|line| line.contains(needle) && line.contains('│'))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{text}"))
}

/// The screen without colors, as a person reads it.
fn plain(app: &App<TestScreen>) -> String {
    let mut out = String::new();
    let mut chars = app.screen().text().chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            chars.by_ref().find(|c| *c == 'm');
        } else {
            out.push(ch);
        }
    }
    out
}

/// `:w` and Enter.
fn write(app: &mut App<TestScreen>) -> lazyvial::Result<()> {
    press(app, ":w")?;
    app.handle(Event::Confirm)?;
    Ok(())
}

fn board() -> lazyvial::Result<MemoryBoard> {
    Ok(
        MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
            .tap_dances(2)
            .combos(2)
            .macros(2),
    )
}

#[test]
fn key_list_previews_on_sheet_cells_in_amber() -> lazyvial::Result<()> {
    let amber = "48;2;224;175;104";
    let board = board()?;
    let mut app = App::new(board.open()?, TestScreen::new(100, 30));
    // A tap dance key cell: the pick replaces it, in amber, before Enter.
    app.handle(Event::Tab)?;
    app.handle(Event::Confirm)?;
    press(&mut app, "esc")?;
    let raw = app.screen().text().to_string();
    assert!(line_with(&raw, "TD(0)").contains(amber), "{raw}");
    assert!(plain(&app).contains("NO → Esc"), "{}", plain(&app));
    app.handle(Event::Cancel)?;
    assert!(!app.screen().text().contains(amber));
    assert!(!app.screen().text().contains("unsaved"));
    // A new macro step: it goes in at the cursor, in amber, and the status says so.
    app.handle(Event::BackTab)?;
    app.handle(Event::BackTab)?;
    press(&mut app, "t")?;
    press(&mut app, "a")?;
    let text = plain(&app);
    assert!(text.contains("new step → A"), "{text}");
    assert!(
        line_with(app.screen().text(), "M0").contains(amber),
        "{}",
        app.screen().text()
    );
    app.handle(Event::Confirm)?;
    assert!(plain(&app).contains("1 unsaved"), "{}", plain(&app));
    Ok(())
}

#[test]
fn tap_dance_keys_and_term_reach_the_board() -> lazyvial::Result<()> {
    let board = board()?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::Tab)?;
    assert!(
        app.screen().text().contains("tap dance 1/2"),
        "{}",
        app.screen().text()
    );
    app.handle(Event::Down)?;
    app.handle(Event::Confirm)?;
    press(&mut app, "esc")?;
    app.handle(Event::Confirm)?;
    for _ in 0..4 {
        app.handle(Event::Right)?;
    }
    app.handle(Event::Confirm)?;
    app.handle(Event::Backspace)?;
    press(&mut app, "250")?;
    app.handle(Event::Confirm)?;
    // Two fields of one tap dance are one write.
    assert!(
        app.screen().text().contains("1 unsaved"),
        "{}",
        app.screen().text()
    );
    write(&mut app)?;
    assert!(
        app.screen().text().contains("wrote 1 change"),
        "{}",
        app.screen().text()
    );
    drop(app);

    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::Tab)?;
    let text = app.screen().text().to_string();
    let row = line_with(&text, "TD(1) ");
    assert!(row.contains("Esc") && row.contains("250 ms"), "{text}");
    assert!(!line_with(&text, "TD(0) ").contains("Esc"), "{text}");
    Ok(())
}

#[test]
fn locked_board_takes_a_combo_without_unlocking() -> lazyvial::Result<()> {
    let board = board()?.locked(&[(0, 0)]);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::Tab)?;
    app.handle(Event::Tab)?;
    for _ in 0..4 {
        app.handle(Event::Right)?;
    }
    app.handle(Event::Confirm)?;
    press(&mut app, "f13")?;
    app.handle(Event::Confirm)?;
    write(&mut app)?;
    let text = app.screen().text().to_string();
    assert!(!text.contains("hold "), "{text}");
    assert!(line_with(&text, "combo 0 ").contains("F13"), "{text}");
    drop(app);

    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    app.handle(Event::BackTab)?;
    assert!(line_with(app.screen().text(), "combo 0 ").contains("F13"));
    Ok(())
}

#[test]
fn locked_board_writes_macros_after_the_unlock() -> lazyvial::Result<()> {
    let board = board()?.locked(&[(0, 0)]);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    press(&mut app, "s")?;
    press(&mut app, "hi")?;
    app.handle(Event::Confirm)?;
    write(&mut app)?;
    let text = app.screen().text().to_string();
    assert!(text.contains("hold 0,0 to write the macros"), "{text}");
    app.handle(Event::Tick)?;
    assert!(app.screen().text().contains("let go of the keys"));
    app.handle(Event::Tick)?;
    let text = app.screen().text().to_string();
    assert!(text.contains("ready"), "{text}");
    drop(app);

    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    assert!(line_with(app.screen().text(), "M0 ").contains("\"hi\""));
    Ok(())
}

#[test]
fn macro_steps_reach_the_board() -> lazyvial::Result<()> {
    let board = board()?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    app.handle(Event::Down)?;
    press(&mut app, "s")?;
    press(&mut app, "hi")?;
    app.handle(Event::Confirm)?;
    press(&mut app, "t")?;
    press(&mut app, "a")?;
    app.handle(Event::Confirm)?;
    press(&mut app, "w")?;
    app.handle(Event::Confirm)?;
    // Move the tap in front of the text.
    app.handle(Event::Left)?;
    app.handle(Event::Left)?;
    press(&mut app, "H")?;
    write(&mut app)?;
    drop(app);

    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    let text = app.screen().text().to_string();
    let row = line_with(&text, "M1 ");
    let tap = row.find(" A ").expect(row);
    let typed = row.find("\"hi\"").expect(row);
    assert!(tap < typed && row.contains("100 ms"), "{text}");
    assert!(!line_with(&text, "M0 ").contains("hi"), "{text}");
    Ok(())
}

#[test]
fn macros_that_overflow_the_buffer_are_refused() -> lazyvial::Result<()> {
    let board = board()?.macro_buffer(8);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    press(&mut app, "s")?;
    press(&mut app, "too long for eight")?;
    app.handle(Event::Confirm)?;
    assert!(
        app.screen().text().contains("not staged"),
        "{}",
        app.screen().text()
    );
    write(&mut app)?;
    drop(app);

    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    app.handle(Event::BackTab)?;
    assert!(!app.screen().text().contains("too long"));
    Ok(())
}

#[test]
fn picks_stage_at_once_and_wait_for_write() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    // One Enter stages the pick; the board previewed it already.
    press(&mut app, "/esc")?;
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(
        text.contains("selected Esc") && text.contains("1 unsaved"),
        "{text}"
    );
    // The changed key is tinted until written, once the cursor leaves it.
    let board2 = MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?;
    let mut corne = App::new(board2.open()?, TestScreen::new(120, 30));
    press(&mut corne, "/esc")?;
    corne.handle(Event::Confirm)?;
    // Green legend on the key face, as opposed to the header's unsaved count.
    let tint = "38;2;158;206;106;48;2;39;44;57";
    assert!(!corne.screen().text().contains(tint));
    press(&mut corne, "l")?;
    assert!(corne.screen().text().contains(tint));
    // Closing the app without `:w` loses the draft.
    drop(app);
    let app = App::new(board.open()?, TestScreen::new(80, 20));
    assert!(!app.screen().text().contains("Esc"));
    Ok(())
}

#[test]
fn undo_and_discard_drop_staged_changes() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    for query in ["/esc", "/tab"] {
        press(&mut app, query)?;
        app.handle(Event::Confirm)?;
    }
    assert!(app.screen().text().contains("selected Tab"));
    press(&mut app, "u")?;
    assert!(app.screen().text().contains("selected Esc"));
    press(&mut app, ":e!")?;
    app.handle(Event::Confirm)?;
    // Discarding asks first, and Esc keeps the changes.
    assert!(plain(&app).contains("discard 1 change?"), "{}", plain(&app));
    app.handle(Event::Cancel)?;
    assert!(app.screen().text().contains("1 unsaved"));
    press(&mut app, ":e!")?;
    app.handle(Event::Confirm)?;
    app.handle(Event::Confirm)?;
    let text = app.screen().text().to_string();
    assert!(
        text.contains("discarded 1") && !text.contains("unsaved"),
        "{text}"
    );
    assert!(!text.contains("Esc"), "{text}");
    press(&mut app, ":e!")?;
    app.handle(Event::Confirm)?;
    assert!(app.screen().text().contains("nothing to discard"));
    press(&mut app, "u")?;
    assert!(app.screen().text().contains("nothing to undo"));
    press(&mut app, ":nope")?;
    app.handle(Event::Confirm)?;
    // An unknown command keeps the line open and says so.
    let text = plain(&app);
    assert!(text.contains("no command starts with :nope"), "{text}");
    assert!(text.contains("COMMAND"), "{text}");
    Ok(())
}

#[test]
fn command_line_picks_exact_names_then_prefixes() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 24));
    press(&mut app, "/esc")?;
    app.handle(Event::Confirm)?;
    // `q` is quit, which asks about the unsaved change, not `q!`, which would not.
    press(&mut app, ":q")?;
    assert_eq!(app.handle(Event::Confirm)?, Step::Continue);
    assert!(plain(&app).contains("1 unsaved change"), "{}", plain(&app));
    app.handle(Event::Cancel)?;
    // Space then Enter picks nothing, so it only closes: nothing is written.
    app.handle(Event::Char(' '))?;
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(
        text.contains("1 unsaved") && !text.contains("find anything"),
        "{text}"
    );
    // A `:` typed into the find list makes it the command line; Backspace from `:` closes it.
    press(&mut app, " :")?;
    assert!(plain(&app).contains("COMMAND"), "{}", plain(&app));
    app.handle(Event::Backspace)?;
    assert!(!plain(&app).contains("COMMAND"), "{}", plain(&app));
    // Tab fills in the first match; a prefix alone runs it too.
    press(&mut app, ":wr")?;
    app.handle(Event::Tab)?;
    assert!(plain(&app).contains("› :w▏"), "{}", plain(&app));
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(text.contains("wrote 1 change"), "{text}");
    Ok(())
}

#[test]
fn quit_with_unsaved_changes_asks() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?;
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    press(&mut app, "/esc")?;
    app.handle(Event::Confirm)?;
    assert_eq!(app.handle(Event::Char('q'))?, Step::Continue);
    let text = plain(&app);
    assert!(
        text.contains("1 unsaved change") && text.contains("NO  →  Esc"),
        "{text}"
    );
    // Esc stays; `w` writes then quits.
    app.handle(Event::Cancel)?;
    assert_eq!(app.handle(Event::Char('q'))?, Step::Continue);
    assert_eq!(app.handle(Event::Char('w'))?, Step::Quit);
    drop(app);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    assert!(app.screen().text().contains("Esc"));

    // `d` quits without writing; `:q!` too.
    press(&mut app, "/tab")?;
    app.handle(Event::Confirm)?;
    assert_eq!(app.handle(Event::Quit)?, Step::Continue);
    assert_eq!(app.handle(Event::Char('d'))?, Step::Quit);
    drop(app);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    assert!(app.screen().text().contains("selected Esc"));
    press(&mut app, ":q")?;
    assert_eq!(app.handle(Event::Confirm)?, Step::Quit);
    Ok(())
}

#[test]
fn tab_walks_layers_then_editors_and_search_groups() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/mini.json"))?
        .layers(3)
        .tap_dances(1);
    let mut app = App::new(board.open()?, TestScreen::new(80, 20));
    for title in ["layer 1/3", "layer 2/3", "tap dance 1/1", "layer 0/3"] {
        app.handle(Event::Tab)?;
        assert!(
            app.screen().text().contains(title),
            "{}",
            app.screen().text()
        );
    }
    app.handle(Event::BackTab)?;
    assert!(app.screen().text().contains("tap dance 1/1"));
    app.handle(Event::BackTab)?;
    app.handle(Event::Char('/'))?;
    assert!(app.screen().text().contains("[Basic]"));
    app.handle(Event::Tab)?;
    assert!(
        app.screen().text().contains("[ISO/JIS]"),
        "{}",
        app.screen().text()
    );
    app.handle(Event::BackTab)?;
    assert!(app.screen().text().contains("[Basic]"));
    Ok(())
}

#[test]
fn key_details_fill_the_spare_rows() -> lazyvial::Result<()> {
    let board = MemoryBoard::from_definition(include_str!("fixtures/corne.json"))?.layers(3);
    let mut app = App::new(board.open()?, TestScreen::new(110, 32));
    let text = plain(&app);
    assert!(text.contains("KC_NO  ·  0x0000  ·  does nothing"), "{text}");
    assert!(
        text.contains("on each layer  L0  NO   L1  NO   L2  NO"),
        "{text}"
    );
    // Stage MO(1) on L0 and look at it from L1: the row shows it, and says what it does.
    press(&mut app, "/mo(1")?;
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(text.contains("layer 1 while held"), "{text}");
    assert!(
        text.contains("unsaved: the keyboard still has NO, :w writes MO(1)"),
        "{text}"
    );
    app.handle(Event::Tab)?;
    let text = plain(&app);
    assert!(text.contains("L0  MO(1)   L1  NO"), "{text}");
    assert!(!text.contains("unsaved: the keyboard"), "{text}");
    // A short screen keeps the keys and drops the details.
    drop(app);
    let short = App::new(board.open()?, TestScreen::new(110, 18));
    let text = plain(&short);
    assert!(
        text.contains("selected NO") && !text.contains("0x0000"),
        "{text}"
    );
    Ok(())
}

/// A tap dance staged with `tap Esc`, then the keyboard stops answering on `:w`.
fn lost_with_one_change(board: &MemoryBoard) -> lazyvial::Result<App<TestScreen>> {
    let mut app = App::new(board.open()?, TestScreen::new(100, 24));
    app.handle(Event::Tab)?;
    app.handle(Event::Confirm)?;
    press(&mut app, "esc")?;
    app.handle(Event::Confirm)?;
    board.unplug();
    write(&mut app)?;
    Ok(app)
}

#[test]
fn reconnect_keeps_unsaved_changes() -> lazyvial::Result<()> {
    let board = board()?;
    let mut app = lost_with_one_change(&board)?;
    let text = plain(&app);
    assert!(text.contains("keyboard disconnected"), "{text}");
    assert!(
        text.contains("▸ reconnect and keep the 1 unsaved change"),
        "{text}"
    );
    // Still asleep: the choices stay and say why.
    app.handle(Event::Confirm)?;
    assert!(plain(&app).contains("not back yet"), "{}", plain(&app));
    board.plug();
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(
        text.contains("reconnected, 1 unsaved change kept"),
        "{text}"
    );
    assert!(
        text.contains("tap dance 1/2") && text.contains("1 unsaved"),
        "{text}"
    );
    write(&mut app)?;
    assert!(plain(&app).contains("wrote 1 change"), "{}", plain(&app));
    drop(app);
    // The keyboard has it: a fresh open shows the tap dance with no unsaved change.
    let mut again = App::new(board.open()?, TestScreen::new(100, 24));
    again.handle(Event::Tab)?;
    let text = plain(&again);
    assert!(line_with(&text, "TD(0)").contains("Esc"), "{text}");
    assert!(!text.contains("unsaved"), "{text}");
    Ok(())
}

#[test]
fn reconnect_can_revert_or_start_again() -> lazyvial::Result<()> {
    let board = board()?;
    let mut app = lost_with_one_change(&board)?;
    board.plug();
    app.handle(Event::Down)?;
    assert!(
        plain(&app).contains("▸ reconnect and revert the 1 unsaved change"),
        "{}",
        plain(&app)
    );
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(
        text.contains("reconnected") && !text.contains("unsaved"),
        "{text}"
    );
    drop(app);

    let mut app = lost_with_one_change(&board)?;
    board.plug();
    app.handle(Event::Down)?;
    app.handle(Event::Down)?;
    app.handle(Event::Confirm)?;
    let text = plain(&app);
    assert!(
        text.contains("choose a keyboard") && text.contains("▌ board"),
        "{text}"
    );
    drop(app);
    // q on the choices quits without asking again.
    let mut app = lost_with_one_change(&board)?;
    assert_eq!(app.handle(Event::Char('q'))?, Step::Quit);
    Ok(())
}
