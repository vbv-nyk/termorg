use crate::proto::{Cell, Color, GroupId, ScreenSnapshot, TermId, TermStatus};
use std::time::Instant;
use vt100::Parser;

/// How many lines of scrollback to keep per terminal.
const SCROLLBACK_LINES: usize = 1000;

pub struct Session {
    parser: Parser,
    /// Override title set by the user. Empty means "use the vt100 title".
    pub title: String,
    /// When we last received any bytes from the PTY.
    last_output_at: Option<Instant>,
    /// True when the shell process has exited.
    pub exited: bool,
    /// Current scrollback offset (0 = live view, >0 = scrolled up into history).
    pub scroll_offset: usize,
}

impl Session {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, SCROLLBACK_LINES),
            title: String::new(),
            last_output_at: None,
            exited: false,
            scroll_offset: 0,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.last_output_at = Some(Instant::now());
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
    }

    /// Set the scrollback offset — how many lines above the bottom to show.
    /// 0 returns to the live (bottom) view.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset;
        self.parser.set_scrollback(offset);
    }

    pub fn status(&self) -> TermStatus {
        // Error overrides everything else — the process is gone.
        if self.exited {
            return TermStatus::Error;
        }
        match self.last_output_at {
            None => TermStatus::Quiet,
            Some(t) => {
                let secs = t.elapsed().as_secs();
                if secs < 2 {
                    TermStatus::Active
                } else if secs < 30 {
                    TermStatus::Attention
                } else {
                    TermStatus::Quiet
                }
            }
        }
    }

    pub fn snapshot(&self, id: TermId, group: Option<GroupId>, fg_process: Option<String>) -> ScreenSnapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();

        let cells = (0..rows)
            .map(|row| {
                (0..cols)
                    .map(|col| {
                        screen
                            .cell(row, col)
                            .map(|c| Cell {
                                ch: c.contents().chars().next().unwrap_or(' '),
                                fg: convert_color(c.fgcolor()),
                                bg: convert_color(c.bgcolor()),
                                bold: c.bold(),
                                italic: c.italic(),
                                underline: c.underline(),
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect();

        let (cursor_row, cursor_col) = screen.cursor_position();

        // Priority: user-set title > foreground process > vt100 OSC title > generic fallback.
        let title = if !self.title.is_empty() {
            self.title.clone()
        } else if let Some(fg) = fg_process {
            fg
        } else {
            let vt_title = screen.title();
            if !vt_title.is_empty() {
                vt_title.to_string()
            } else {
                format!("term {}", id.0 + 1)
            }
        };

        ScreenSnapshot {
            id,
            cols,
            rows,
            cells,
            cursor_row,
            cursor_col,
            title,
            status: self.status(),
            group,
            scroll_offset: self.scroll_offset,
        }
    }
}

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
