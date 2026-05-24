use crate::layout::{self, GroupEntry, Layout, TerminalEntry};
use crate::proto::{GroupColor, GroupId, GroupInfo, ScreenSnapshot, TermId};
use crate::server::pty::{Pty, PtyEvent};
use crate::server::session::Session;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::mpsc::Sender;

pub struct ServerState {
    ptys: HashMap<TermId, Pty>,
    sessions: HashMap<TermId, Session>,
    active: Option<TermId>,
    next_id: u32,
    next_group_id: u32,
    groups: HashMap<GroupId, GroupInfo>,
    /// Which group each terminal belongs to.
    term_groups: HashMap<TermId, GroupId>,
    output_tx: Sender<(TermId, PtyEvent)>,
}

impl ServerState {
    pub fn new(output_tx: Sender<(TermId, PtyEvent)>) -> Self {
        Self {
            ptys: HashMap::new(),
            sessions: HashMap::new(),
            active: None,
            next_id: 0,
            next_group_id: 0,
            groups: HashMap::new(),
            term_groups: HashMap::new(),
            output_tx,
        }
    }

    // ─── Terminals ───────────────────────────────────────────────────────────

    pub fn new_terminal(&mut self, cols: u16, rows: u16) -> Result<TermId> {
        let id = TermId(self.next_id);
        self.next_id += 1;

        // Pass the shared output channel directly to the PTY reader thread —
        // no intermediate channel needed, which halves the thread count.
        let pty = Pty::spawn(cols, rows, id, self.output_tx.clone())?;
        let session = Session::new(cols, rows);

        self.ptys.insert(id, pty);
        self.sessions.insert(id, session);

        if self.active.is_none() {
            self.active = Some(id);
        }

        Ok(id)
    }

    pub fn close_terminal(&mut self, id: TermId) -> Result<()> {
        if !self.ptys.contains_key(&id) {
            bail!("terminal {:?} does not exist", id);
        }
        self.ptys.remove(&id);
        self.sessions.remove(&id);
        self.term_groups.remove(&id);

        if self.active == Some(id) {
            self.active = self.ptys.keys().next().copied();
        }

        Ok(())
    }

    pub fn switch_to(&mut self, id: TermId) -> bool {
        if self.ptys.contains_key(&id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    /// Resize every PTY and its session parser to the same dimensions.
    pub fn resize_all(&mut self, cols: u16, rows: u16) -> Result<()> {
        let ids: Vec<TermId> = self.ptys.keys().copied().collect();
        for id in ids {
            if let Some(pty) = self.ptys.get(&id) {
                pty.resize(cols, rows)?;
            }
            if let Some(session) = self.sessions.get_mut(&id) {
                session.resize(cols, rows);
            }
        }
        Ok(())
    }

    /// Resize the active PTY and its session's vt100 parser.
    pub fn resize_active(&mut self, cols: u16, rows: u16) -> Result<()> {
        let id = match self.active {
            Some(id) => id,
            None => return Ok(()),
        };
        if let Some(pty) = self.ptys.get_mut(&id) {
            pty.resize(cols, rows)?;
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.resize(cols, rows);
        }
        Ok(())
    }

    pub fn rename_terminal(&mut self, id: TermId, title: String) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.title = title;
        }
    }

    /// Set the scrollback offset for the active terminal.
    /// `offset` = 0 means live view (bottom), >0 means scrolled into history.
    pub fn scroll_active(&mut self, offset: usize) {
        if let Some(id) = self.active {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.set_scroll_offset(offset);
            }
        }
    }

    // ─── Groups ──────────────────────────────────────────────────────────────

    pub fn new_group(&mut self, name: String, color: GroupColor) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        self.groups.insert(id, GroupInfo { id, name, color });
        id
    }

    pub fn delete_group(&mut self, id: GroupId) {
        self.groups.remove(&id);
        // Unassign every terminal that belonged to this group.
        self.term_groups.retain(|_, gid| *gid != id);
    }

    pub fn rename_group(&mut self, id: GroupId, name: String) {
        if let Some(g) = self.groups.get_mut(&id) {
            g.name = name;
        }
    }

    pub fn move_terminal(&mut self, term_id: TermId, group_id: Option<GroupId>) {
        match group_id {
            Some(gid) if self.groups.contains_key(&gid) => {
                self.term_groups.insert(term_id, gid);
            }
            _ => {
                self.term_groups.remove(&term_id);
            }
        }
    }

    // ─── Accessors ───────────────────────────────────────────────────────────

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

    pub fn snapshot(&self, id: TermId) -> Option<ScreenSnapshot> {
        let group = self.term_groups.get(&id).copied();
        let fg = self.ptys.get(&id).and_then(|p| p.foreground_process());
        self.sessions.get(&id).map(|s| s.snapshot(id, group, fg))
    }

    pub fn all_snapshots(&self) -> Vec<ScreenSnapshot> {
        let mut snaps: Vec<ScreenSnapshot> = self
            .sessions
            .iter()
            .map(|(&id, session)| {
                let group = self.term_groups.get(&id).copied();
                let fg = self.ptys.get(&id).and_then(|p| p.foreground_process());
                session.snapshot(id, group, fg)
            })
            .collect();
        snaps.sort_by_key(|s| s.id.0);
        snaps
    }

    pub fn groups(&self) -> Vec<GroupInfo> {
        let mut gs: Vec<GroupInfo> = self.groups.values().cloned().collect();
        gs.sort_by_key(|g| g.id.0);
        gs
    }

    // ─── Persistence ─────────────────────────────────────────────────────────

    /// Recreate groups and terminals from a saved layout.
    /// Call this on server startup instead of `new_terminal(80, 24)`.
    pub fn apply_layout(&mut self, layout: &Layout) -> Result<()> {
        // Build groups in order and remember their assigned GroupIds.
        // We use the position index in the saved list rather than name lookup so
        // that two groups with the same name are kept distinct.
        let mut group_ids: Vec<GroupId> = Vec::new();
        for entry in &layout.group {
            let gid = self.new_group(entry.name.clone(), layout::color_from_str(&entry.color));
            group_ids.push(gid);
        }

        for entry in &layout.terminal {
            // Terminals start at 80×24; the client will send ResizeAll immediately
            // on attach and correct this to the real viewport dimensions.
            let id = self.new_terminal(80, 24)?;
            if !entry.title.is_empty() {
                self.rename_terminal(id, entry.title.clone());
            }
            // Prefer the numeric `group_index` field (authoritative); fall back to
            // name-based lookup for files saved by older versions of termorg.
            let group_idx = entry.group_index.or_else(|| {
                entry.group.as_ref().and_then(|name| {
                    layout.group.iter().position(|g| g.name == *name)
                })
            });
            if let Some(idx) = group_idx {
                if let Some(&gid) = group_ids.get(idx) {
                    self.move_terminal(id, Some(gid));
                }
            }
        }
        Ok(())
    }

    /// Serialise current groups and terminals into a saveable layout.
    pub fn to_layout(&self) -> Layout {
        let mut groups_sorted: Vec<&GroupInfo> = self.groups.values().collect();
        groups_sorted.sort_by_key(|g| g.id.0);

        let group = groups_sorted
            .iter()
            .map(|g| GroupEntry {
                name: g.name.clone(),
                color: layout::color_to_str(g.color).to_string(),
            })
            .collect();

        let mut term_ids: Vec<TermId> = self.sessions.keys().copied().collect();
        term_ids.sort_by_key(|t| t.0);

        let terminal = term_ids
            .iter()
            .map(|&id| {
                let title = self
                    .sessions
                    .get(&id)
                    .map(|s| s.title.clone())
                    .unwrap_or_default();
                // Find the group this terminal belongs to.
                let group_gid = self.term_groups.get(&id).copied();
                let group_name = group_gid
                    .and_then(|gid| self.groups.get(&gid))
                    .map(|g| g.name.clone());
                // Store the group's position index in the sorted group list so
                // apply_layout can match by index instead of name — this correctly
                // handles two groups that share the same display name.
                let group_index = group_gid.and_then(|gid| {
                    groups_sorted.iter().position(|g| g.id == gid)
                });
                TerminalEntry { title, group: group_name, group_index }
            })
            .collect();

        Layout { group, terminal }
    }
}
