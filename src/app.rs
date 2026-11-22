use crate::session::{Attached, Event, Model, Step};
use crate::Result;

/// A backend receives the finished string. It does not receive keys or places.
pub trait Screen {
    fn paint(&mut self, text: &str);
    fn cols(&self) -> u16;
    fn rows(&self) -> u16;
}

/// Remembers the last paint. Assertions read `text`.
pub struct TestScreen {
    cols: u16,
    rows: u16,
    last: String,
}

impl TestScreen {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            last: String::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.last
    }
}

impl Screen for TestScreen {
    fn paint(&mut self, text: &str) {
        self.last = text.to_string();
    }

    fn cols(&self) -> u16 {
        self.cols
    }

    fn rows(&self) -> u16 {
        self.rows
    }
}

/// Shell. `new` paints once. `handle` asks `Model`, then paints again.
pub struct App<S: Screen> {
    screen: S,
    model: Model,
}

impl<S: Screen> App<S> {
    pub fn new(attached: Attached, screen: S) -> Self {
        let model = Model::start(attached);
        let mut app = Self { screen, model };
        app.paint();
        app
    }

    pub fn handle(&mut self, event: Event) -> Result<Step> {
        let step = self.model.handle(event)?;
        self.paint();
        Ok(step)
    }

    pub fn screen(&self) -> &S {
        &self.screen
    }

    fn paint(&mut self) {
        let text = self.model.text(self.screen.cols(), self.screen.rows());
        self.screen.paint(&text);
    }
}
