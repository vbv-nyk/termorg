use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TermId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(pub u32);

/// One of the four earth-tone accent colours assigned to a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupColor {
    Ochre,
    Sage,
    Terra,
    Slate,
}

/// A named project group that terminals can belong to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub id: GroupId,
    pub name: String,
    pub color: GroupColor,
}

/// Lifecycle state of one terminal, derived from recent output activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TermStatus {
    /// Output arrived in the last 2 s.
    Active,
    /// Had output but went quiet — likely waiting for input.
    Attention,
    /// Process exited with a non-zero status.
    Error,
    /// No output for 30 s+, or never had any.
    Quiet,
}

/// A terminal colour — matches what vt100 cells can express.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One cell in a terminal grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// A full snapshot of one terminal's screen at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    pub id: TermId,
    pub cols: u16,
    pub rows: u16,
    /// Row-major: `cells[row][col]`.
    pub cells: Vec<Vec<Cell>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    /// Human-readable title (set by the inner program or the user).
    pub title: String,
    /// Current lifecycle state.
    pub status: TermStatus,
    /// Which group this terminal belongs to, if any.
    pub group: Option<GroupId>,
    /// How many lines above live view this snapshot is (0 = live, >0 = scrollback).
    pub scroll_offset: usize,
}

/// Messages the client sends to the server.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Attach,
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
    NewTerminal { group_id: Option<GroupId>, cols: u16, rows: u16 },
    SwitchTo(TermId),
    CloseTerminal(TermId),
    RequestSnapshots,
    /// Set a custom display title for a terminal.
    RenameTerminal { id: TermId, title: String },
    /// Create a new project group.
    NewGroup { name: String, color: GroupColor },
    /// Rename an existing group.
    RenameGroup { id: GroupId, name: String },
    /// Move a terminal into a group (or out of all groups if group_id is None).
    MoveTerminal { term_id: TermId, group_id: Option<GroupId> },
    /// Delete a group (terminals become ungrouped).
    DeleteGroup(GroupId),
    /// Resize every terminal at once (used when entering grid mode).
    ResizeAll { cols: u16, rows: u16 },
    /// Scroll the active terminal's viewport into scrollback history.
    /// `offset` is the number of lines above the bottom to show (0 = live view).
    ScrollTo { offset: usize },
}

/// Messages the server sends to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    TerminalList {
        active: TermId,
        ids: Vec<TermId>,
    },
    Snapshot(ScreenSnapshot),
    GroupList(Vec<GroupInfo>),
}
