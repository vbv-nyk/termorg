use serde::{Deserialize, Serialize};

/// A unique identifier for one terminal (PTY) managed by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TermId(pub u32);

/// Messages the client sends to the server.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Client wants to start receiving output.
    Attach,
    /// Bytes to write into the active PTY (keypresses).
    Input(Vec<u8>),
    /// The client's viewport was resized.
    Resize { cols: u16, rows: u16 },
    /// Client is disconnecting cleanly.
    Detach,
    /// Ask the server to spawn a new terminal and switch to it.
    NewTerminal,
    /// Switch the active terminal to the given ID.
    SwitchTo(TermId),
    /// Close the terminal with the given ID.
    CloseTerminal(TermId),
}

/// Messages the server sends to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Raw bytes produced by the active PTY — write these straight to the terminal.
    Output(Vec<u8>),
    /// Sent after Attach or a switch — tells the client which terminal is now
    /// active and what terminals exist.
    TerminalList {
        active: TermId,
        ids: Vec<TermId>,
    },
}
