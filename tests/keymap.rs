#[test]
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
    let app = lazyvial::App::new(board.open()?, lazyvial::TestScreen::new(60, 16));
    assert!(app.screen().text().contains("[Esc]"));
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
    app.handle(lazyvial::Event::Down)?;
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
    app.handle(lazyvial::Event::Char('a'))?;
    app.handle(lazyvial::Event::Confirm)?;
    assert!(app.screen().text().contains("[A]"));
    Ok(())
}
