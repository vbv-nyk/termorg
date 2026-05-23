use crate::proto::TermId;
use crate::server::pty::Pty;
use crate::server::session::Session;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::thread;

/// All terminal state owned by the server.
pub struct ServerState {
    ptys: HashMap<TermId, Pty>,
    sessions: HashMap<TermId, Session>,
    /// The terminal the client is currently looking at.
    active: Option<TermId>,
    /// Incremented each time a terminal is created.
    next_id: u32,
    /// Every PTY reader thread sends (TermId, bytes) here.
    output_tx: Sender<(TermId, Vec<u8>)>,
}

impl ServerState {
    pub fn new(output_tx: Sender<(TermId, Vec<u8>)>) -> Self {
        Self {
            ptys: HashMap::new(),
            sessions: HashMap::new(),
            active: None,
            next_id: 0,
            output_tx,
        }
    }

    /// Spawn a new terminal, add it to the maps, and return its ID.
    pub fn new_terminal(&mut self, cols: u16, rows: u16) -> Result<TermId> {
        let id = TermId(self.next_id);
        self.next_id += 1;

        // Per-PTY channel. A bridge thread forwards bytes into the shared
        // output channel tagged with this terminal's ID.
        let (pty_tx, pty_rx) = mpsc::channel::<Vec<u8>>();
        let shared_tx = self.output_tx.clone();
        thread::spawn(move || {
            while let Ok(bytes) = pty_rx.recv() {
                if shared_tx.send((id, bytes)).is_err() {
                    break;
                }
            }
        });

        let pty = Pty::spawn(cols, rows, pty_tx)?;
        let session = Session::new(cols, rows);

        self.ptys.insert(id, pty);
        self.sessions.insert(id, session);

        // First terminal becomes active automatically.
        if self.active.is_none() {
            self.active = Some(id);
        }

        Ok(id)
    }

    /// Close a terminal and remove it from the maps.
    pub fn close_terminal(&mut self, id: TermId) -> Result<()> {
        if !self.ptys.contains_key(&id) {
            bail!("terminal {:?} does not exist", id);
        }
        self.ptys.remove(&id);
        self.sessions.remove(&id);

        // If we closed the active terminal, switch to the first remaining one.
        if self.active == Some(id) {
            self.active = self.ptys.keys().next().copied();
        }

        Ok(())
    }

    /// Switch the active terminal. Returns false if the ID does not exist.
    pub fn switch_to(&mut self, id: TermId) -> bool {
        if self.ptys.contains_key(&id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    pub fn active_id(&self) -> Option<TermId> {
        self.active
    }

    pub fn terminal_ids(&self) -> Vec<TermId> {
        let mut ids: Vec<TermId> = self.ptys.keys().copied().collect();
        ids.sort_by_key(|t| t.0);
        ids
    }

    pub fn active_pty_mut(&mut self) -> Option<&mut Pty> {
        self.active.and_then(|id| self.ptys.get_mut(&id))
    }

    pub fn session_mut(&mut self, id: TermId) -> Option<&mut Session> {
        self.sessions.get_mut(&id)
    }

    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.active.and_then(|id| self.sessions.get_mut(&id))
    }
}
