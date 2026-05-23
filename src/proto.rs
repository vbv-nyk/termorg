use serde::{Deserialize, Serialize};

/// Messages the client sends to the server.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Client wants to start receiving output.
    Attach,
    /// Bytes to write into the PTY (keypresses).
    Input(Vec<u8>),
    /// The client's viewport was resized.
    Resize { cols: u16, rows: u16 },
    /// Client is disconnecting cleanly.
    Detach,
}

/// Messages the server sends to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Raw bytes produced by the PTY — write these straight to the terminal.
    Output(Vec<u8>),
}
