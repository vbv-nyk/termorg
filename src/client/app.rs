use crate::proto::{GroupId, GroupInfo, ScreenSnapshot, TermId, TermStatus};
use ratatui::layout::Rect;
use ratatui::style::Color as RColor;
use std::collections::{HashMap, HashSet};

use super::draw::{group_color_ratatui, loudest_status};

// ─── Mode ────────────────────────────────────────────────────────────────────

pub enum AppMode {
    Grid,
    Focused,
    /// Inline group rename — user is typing a new name for `group_id`.
    Renaming { group_id: GroupId, buffer: String },
    /// Inline terminal rename — user is typing a new name for `term_id`.
    RenamingTerminal { term_id: TermId, buffer: String },
}

// ─── Bottom bar hit target ───────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum BottomBarHit {
    RenameGroup,
    NewGroup,
    CloseTerminal,
    Quit,
}

// ─── Sidebar hit target ──────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum SidebarHit {
    GroupHeader(GroupId),
    /// The [+] button at the right of a group header row.
    AddTerminal(GroupId),
    Terminal(TermId),
    NewGroup,
}

// ─── App ─────────────────────────────────────────────────────────────────────

pub struct App {
    pub snapshots: HashMap<TermId, ScreenSnapshot>,
    pub ids: Vec<TermId>,
    pub active: Option<TermId>,
    pub groups: Vec<GroupInfo>,
    /// Highlighted terminal in grid mode.
    pub selected: Option<TermId>,
    pub expanded_groups: HashSet<GroupId>,
    pub mode: AppMode,
    /// Which group's terminals are shown in the grid.
    pub selected_group: Option<GroupId>,
    /// Updated each frame for tile mouse hit-testing.
    pub tile_areas: Vec<(Rect, TermId)>,
    /// Updated each frame for sidebar mouse hit-testing.
    pub sidebar_hits: Vec<(Rect, SidebarHit)>,
    /// Updated each frame for bottom bar mouse hit-testing.
    pub bottom_bar_hits: Vec<(Rect, BottomBarHit)>,
    pub quit: bool,
    /// Current scrollback offset being requested (lines above live view).
    /// The server tracks the actual parser offset; this is used to compute
    /// increments for Page-Up/Down and to drive the ScrollTo message.
    pub scroll_offset: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            snapshots: HashMap::new(),
            ids: Vec::new(),
            active: None,
            groups: Vec::new(),
            selected: None,
            expanded_groups: HashSet::new(),
            mode: AppMode::Grid,
            selected_group: None,
            tile_areas: Vec::new(),
            sidebar_hits: Vec::new(),
            bottom_bar_hits: Vec::new(),
            quit: false,
            scroll_offset: 0,
        }
    }

    /// Terminal IDs visible in the current grid view (filtered by `selected_group`).
    pub fn visible_ids(&self) -> Vec<TermId> {
        self.ids.iter().copied()
            .filter(|&id| self.snap_group(id) == self.selected_group)
            .collect()
    }

    pub fn snap_group(&self, id: TermId) -> Option<GroupId> {
        self.snapshots.get(&id)?.group
    }

    pub fn term_title(&self, id: TermId) -> String {
        self.snapshots
            .get(&id)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| format!("term {}", id.0 + 1))
    }

    pub fn term_status(&self, id: TermId) -> TermStatus {
        self.snapshots
            .get(&id)
            .map(|s| s.status)
            .unwrap_or(TermStatus::Quiet)
    }

    pub fn group_color(&self, id: GroupId) -> RColor {
        self.groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| group_color_ratatui(g.color))
            .unwrap_or(super::draw::pal::DIM)
    }

    pub fn term_accent_color(&self, id: TermId) -> RColor {
        self.snap_group(id)
            .map(|gid| self.group_color(gid))
            .unwrap_or(super::draw::pal::MUTE)
    }

    /// Loudest status across every terminal in a group.
    pub fn group_status(&self, gid: GroupId) -> TermStatus {
        let statuses: Vec<TermStatus> = self
            .ids
            .iter()
            .filter(|&&id| self.snap_group(id) == Some(gid))
            .map(|&id| self.term_status(id))
            .collect();
        loudest_status(&statuses)
    }
}
