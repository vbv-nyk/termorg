use vt100::Parser;

/// The in-memory screen state for one PTY.
///
/// We feed raw PTY output bytes into the parser and it maintains a grid of
/// cells we can read at any time — even after the client disconnects.
pub struct Session {
    parser: Parser,
}

impl Session {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
        }
    }

    /// Feed bytes from the PTY into the screen buffer.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Borrow the current screen so callers can read cell content or cursor
    /// position.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn cols(&self) -> u16 {
        self.parser.screen().size().1
    }

    pub fn rows(&self) -> u16 {
        self.parser.screen().size().0
    }
}
