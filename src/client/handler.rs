use crate::proto::{ClientMsg, GroupColor, TermId};
use crate::server::ipc::send_msg;
use anyhow::Result;
use crossterm::{event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind}, terminal};
use ratatui::layout::Rect;
use std::os::unix::net::UnixStream;

use super::app::{App, AppMode, BottomBarHit, SidebarHit};
use super::draw::{bottom_bar_specs, focused_terminal_size, frame_layout, grid_terminal_size, rect_contains};
use super::input::key_to_action;

// ─── Key dispatch ────────────────────────────────────────────────────────────

pub fn handle_key(
    app: &mut App,
    stream: &mut UnixStream,
    key: crossterm::event::KeyEvent,
) -> Result<()> {
    match &app.mode {
        // In focused mode: Ctrl-G goes back to grid, Ctrl-R renames the terminal,
        // Page-Up/Down scrolls the scrollback buffer, everything else goes to PTY.
        AppMode::Focused => {
            if key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL {
                // Reset scrollback when going to grid.
                if app.scroll_offset != 0 {
                    app.scroll_offset = 0;
                    send_msg(stream, &ClientMsg::ScrollTo { offset: 0 })?;
                }
                go_to_grid(app, stream)?;
                return Ok(());
            }
            if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::CONTROL {
                if let Some(id) = app.active {
                    let current = app.term_title(id);
                    app.mode = AppMode::RenamingTerminal { term_id: id, buffer: current };
                }
                return Ok(());
            }
            // Page-Up: scroll up by half a screen (12 lines).
            if key.code == KeyCode::PageUp {
                app.scroll_offset = app.scroll_offset.saturating_add(12);
                send_msg(stream, &ClientMsg::ScrollTo { offset: app.scroll_offset })?;
                return Ok(());
            }
            // Page-Down: scroll back toward live view.
            if key.code == KeyCode::PageDown {
                app.scroll_offset = app.scroll_offset.saturating_sub(12);
                send_msg(stream, &ClientMsg::ScrollTo { offset: app.scroll_offset })?;
                return Ok(());
            }
            if let Some(bytes) = key_to_action(key) {
                // Any regular key resets scrollback to live view before sending.
                if app.scroll_offset != 0 {
                    app.scroll_offset = 0;
                    // Server resets scrollback on Input, no extra message needed.
                }
                send_msg(stream, &ClientMsg::Input(bytes))?;
            }
        }
        AppMode::Grid => handle_key_grid(app, stream, key)?,
        // Renaming mode captures typed characters for the group name.
        AppMode::Renaming { .. } => handle_key_renaming(app, stream, key)?,
        AppMode::RenamingTerminal { .. } => handle_key_renaming_terminal(app, stream, key)?,
    }
    Ok(())
}

// ─── Grid mode keyboard shortcuts ────────────────────────────────────────────

fn handle_key_grid(
    app: &mut App,
    stream: &mut UnixStream,
    key: crossterm::event::KeyEvent,
) -> Result<()> {
    let visible = app.visible_ids();

    match key.code {
        // Arrow keys: navigate between tiles.
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
            if visible.is_empty() { return Ok(()); }
            let cols = super::draw::grid_cols_pub(visible.len());
            let current_pos = app.selected
                .and_then(|id| visible.iter().position(|&v| v == id))
                .unwrap_or(0);
            let new_pos = match key.code {
                KeyCode::Right => (current_pos + 1).min(visible.len() - 1),
                KeyCode::Left  => current_pos.saturating_sub(1),
                KeyCode::Down  => (current_pos + cols).min(visible.len() - 1),
                KeyCode::Up    => current_pos.saturating_sub(cols),
                _ => current_pos,
            };
            app.selected = visible.get(new_pos).copied();
        }
        // Enter: focus the selected terminal.
        KeyCode::Enter => {
            if let Some(id) = app.selected {
                focus_terminal(app, stream, id)?;
            }
        }
        // n: create a new terminal in the current group.
        KeyCode::Char('n') => {
            let (new_cols, new_rows) = if let Ok(tsize) = terminal::size() {
                let area = ratatui::layout::Rect::new(0, 0, tsize.0, tsize.1);
                let layout = frame_layout(area);
                focused_terminal_size(layout.main)
            } else {
                (80, 24)
            };
            send_msg(stream, &ClientMsg::NewTerminal {
                group_id: app.selected_group,
                cols: new_cols,
                rows: new_rows,
            })?;
            app.mode = AppMode::Focused;
        }
        // g: create a new group.
        KeyCode::Char('g') if key.modifiers == KeyModifiers::NONE => {
            let idx = app.groups.len();
            let color = [GroupColor::Ochre, GroupColor::Sage, GroupColor::Terra, GroupColor::Slate][idx % 4];
            send_msg(stream, &ClientMsg::NewGroup {
                name: format!("group-{}", idx + 1),
                color,
            })?;
        }
        // r: rename current group.
        KeyCode::Char('r') => {
            if let Some(gid) = app.selected_group {
                let current_name = app.groups.iter()
                    .find(|g| g.id == gid)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                app.mode = AppMode::Renaming { group_id: gid, buffer: current_name };
            }
        }
        // q or Ctrl-Q: quit.
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            let _ = send_msg(stream, &ClientMsg::Detach);
            app.quit = true;
        }
        _ => {}
    }
    Ok(())
}

// ─── Renaming mode keys ──────────────────────────────────────────────────────

fn handle_key_renaming(
    app: &mut App,
    stream: &mut UnixStream,
    key: crossterm::event::KeyEvent,
) -> Result<()> {
    let (group_id, buffer) = match &app.mode {
        AppMode::Renaming { group_id, buffer } => (*group_id, buffer.clone()),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Grid;
        }
        KeyCode::Enter => {
            let name = buffer.trim().to_string();
            if !name.is_empty() {
                send_msg(stream, &ClientMsg::RenameGroup { id: group_id, name })?;
            }
            app.mode = AppMode::Grid;
        }
        KeyCode::Backspace => {
            let mut b = buffer;
            b.pop();
            app.mode = AppMode::Renaming { group_id, buffer: b };
        }
        KeyCode::Char(c) => {
            let mut b = buffer;
            b.push(c);
            app.mode = AppMode::Renaming { group_id, buffer: b };
        }
        _ => {}
    }
    Ok(())
}

// ─── Terminal rename mode keys ───────────────────────────────────────────────

fn handle_key_renaming_terminal(
    app: &mut App,
    stream: &mut UnixStream,
    key: crossterm::event::KeyEvent,
) -> Result<()> {
    let (term_id, buffer) = match &app.mode {
        AppMode::RenamingTerminal { term_id, buffer } => (*term_id, buffer.clone()),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Focused;
        }
        KeyCode::Enter => {
            let name = buffer.trim().to_string();
            if !name.is_empty() {
                send_msg(stream, &ClientMsg::RenameTerminal { id: term_id, title: name })?;
            }
            app.mode = AppMode::Focused;
        }
        KeyCode::Backspace => {
            let mut b = buffer;
            b.pop();
            app.mode = AppMode::RenamingTerminal { term_id, buffer: b };
        }
        KeyCode::Char(c) => {
            let mut b = buffer;
            b.push(c);
            app.mode = AppMode::RenamingTerminal { term_id, buffer: b };
        }
        _ => {}
    }
    Ok(())
}

// ─── Mouse ───────────────────────────────────────────────────────────────────

pub fn handle_mouse(
    app: &mut App,
    stream: &mut UnixStream,
    mouse: crossterm::event::MouseEvent,
) -> Result<()> {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return Ok(());
    }
    let (col, row) = (mouse.column, mouse.row);

    // Bottom bar button click.
    let bottom_hits = app.bottom_bar_hits.clone();
    for (rect, hit) in bottom_hits {
        if !rect_contains(rect, col, row) { continue; }
        match hit {
            BottomBarHit::RenameGroup => {
                if let Some(gid) = app.selected_group {
                    let current_name = app.groups.iter()
                        .find(|g| g.id == gid)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    app.mode = AppMode::Renaming { group_id: gid, buffer: current_name };
                }
            }
            BottomBarHit::NewGroup => {
                let idx = app.groups.len();
                let color = [GroupColor::Ochre, GroupColor::Sage, GroupColor::Terra, GroupColor::Slate][idx % 4];
                send_msg(stream, &ClientMsg::NewGroup { name: format!("group-{}", idx + 1), color })?;
            }
            BottomBarHit::CloseTerminal => {
                if let Some(id) = app.active {
                    send_msg(stream, &ClientMsg::CloseTerminal(id))?;
                }
                go_to_grid(app, stream)?;
            }
            BottomBarHit::Quit => {
                let _ = send_msg(stream, &ClientMsg::Detach);
                app.quit = true;
            }
        }
        return Ok(());
    }

    // Tile click — single click focuses the terminal immediately.
    for &(rect, id) in &app.tile_areas {
        if rect_contains(rect, col, row) {
            focus_terminal(app, stream, id)?;
            return Ok(());
        }
    }

    // Sidebar click.
    let sidebar_hits = app.sidebar_hits.clone();
    let in_focused = matches!(&app.mode, AppMode::Focused);

    for (rect, hit) in sidebar_hits {
        if !rect_contains(rect, col, row) { continue; }

        match hit {
            SidebarHit::GroupHeader(gid) => {
                if app.selected_group == Some(gid) && !in_focused {
                    // Already viewing this group — collapse/expand the sidebar list.
                    if app.expanded_groups.contains(&gid) {
                        app.expanded_groups.remove(&gid);
                    } else {
                        app.expanded_groups.insert(gid);
                    }
                } else {
                    // Switch to this group and make sure it's expanded in the sidebar.
                    app.selected_group = Some(gid);
                    app.expanded_groups.insert(gid);
                    if in_focused {
                        go_to_grid(app, stream)?;
                    }
                }
            }
            SidebarHit::AddTerminal(gid) => {
                // Create a new terminal in this group and focus it.
                // Compute focused-mode size so the new terminal starts at the right size.
                let (new_cols, new_rows) = if let Ok(tsize) = terminal::size() {
                    let area = ratatui::layout::Rect::new(0, 0, tsize.0, tsize.1);
                    let layout = frame_layout(area);
                    focused_terminal_size(layout.main)
                } else {
                    (80, 24)
                };
                send_msg(stream, &ClientMsg::NewTerminal { group_id: Some(gid), cols: new_cols, rows: new_rows })?;
                app.selected_group = Some(gid);
                app.mode = AppMode::Focused;
            }
            SidebarHit::Terminal(id) => {
                // Always open the terminal in focus mode, whether we're in grid or focused.
                app.selected_group = app.snap_group(id);
                focus_terminal(app, stream, id)?;
            }
            SidebarHit::NewGroup => {
                let idx = app.groups.len();
                let color = [GroupColor::Ochre, GroupColor::Sage, GroupColor::Terra, GroupColor::Slate][idx % 4];
                send_msg(stream, &ClientMsg::NewGroup { name: format!("group-{}", idx + 1), color })?;
            }
        }
        return Ok(());
    }

    Ok(())
}

fn focus_terminal(app: &mut App, stream: &mut UnixStream, id: TermId) -> Result<()> {
    send_msg(stream, &ClientMsg::SwitchTo(id))?;
    if let Ok(tsize) = terminal::size() {
        let area = ratatui::layout::Rect::new(0, 0, tsize.0, tsize.1);
        let layout = frame_layout(area);
        let (cols, rows) = focused_terminal_size(layout.main);
        send_msg(stream, &ClientMsg::Resize { cols, rows })?;
    }
    app.mode = AppMode::Focused;
    Ok(())
}

fn go_to_grid(app: &mut App, stream: &mut UnixStream) -> Result<()> {
    if let Ok(tsize) = terminal::size() {
        let area = ratatui::layout::Rect::new(0, 0, tsize.0, tsize.1);
        let layout = frame_layout(area);
        // Use the number of VISIBLE terminals (current group), not all terminals.
        // ResizeAll also triggers a snapshot broadcast from the server.
        let n = app.visible_ids().len().max(1);
        let (cols, rows) = grid_terminal_size(layout.main, n);
        send_msg(stream, &ClientMsg::ResizeAll { cols, rows })?;
    }
    app.mode = AppMode::Grid;
    Ok(())
}

// ─── Sidebar hit areas ────────────────────────────────────────────────────────
//
// Must stay in sync with `draw::draw_sidebar`.
// Layout (per sidebar):
//   row 0: "GROUPS (N)"  or  "← click to go back"  (no hit zone)
//   row 1: blank line                               (no hit zone)
//   per group:
//     row y:   group header — left (W-3) = GroupHeader, right 3 = AddTerminal
//     rows …:  terminal rows (if expanded) = Terminal
//     row:     blank line                           (no hit zone)
//   ungrouped section (if any):
//     row:     "─ ungrouped ─"                      (no hit zone — TODO)
//     rows …:  terminal rows = Terminal
//     row:     blank line                           (no hit zone)
//   last row:  "+ new group" = NewGroup

pub fn compute_sidebar_hits(app: &App, sidebar: Rect) -> Vec<(Rect, SidebarHit)> {
    let mut hits = Vec::new();
    let mut y = sidebar.y;
    let bot = sidebar.y + sidebar.height;

    let full_row   = |y: u16| Rect::new(sidebar.x, y, sidebar.width, 1);
    let left_row   = |y: u16| Rect::new(sidebar.x, y, sidebar.width.saturating_sub(3), 1);
    let right_cell = |y: u16| Rect::new(sidebar.x + sidebar.width.saturating_sub(3), y, 3, 1);

    y += 1; // header line ("GROUPS (N)" or "← click to go back")
    y += 1; // blank line

    for group in &app.groups {
        if y >= bot { break; }
        hits.push((left_row(y),   SidebarHit::GroupHeader(group.id)));
        hits.push((right_cell(y), SidebarHit::AddTerminal(group.id)));
        y += 1;

        if app.expanded_groups.contains(&group.id) {
            for &id in &app.ids {
                if app.snap_group(id) != Some(group.id) { continue; }
                if y >= bot { break; }
                hits.push((full_row(y), SidebarHit::Terminal(id)));
                y += 1;
            }
        }
        y += 1; // blank line between groups
    }

    let ungrouped: Vec<TermId> = app.ids.iter().copied()
        .filter(|&id| app.snap_group(id).is_none())
        .collect();

    if !ungrouped.is_empty() {
        y += 1; // "─ ungrouped ─" header (no hit zone)
        for id in ungrouped {
            if y >= bot { break; }
            hits.push((full_row(y), SidebarHit::Terminal(id)));
            y += 1;
        }
        y += 1; // blank line
    }

    if y < bot {
        hits.push((full_row(y), SidebarHit::NewGroup));
    }

    hits
}

// ─── Bottom bar hit areas ─────────────────────────────────────────────────────
//
// Must stay in sync with `draw::draw_bottom_bar`.
// Buttons are rendered left-to-right with 1-char side padding + 2-char gap.

pub fn compute_bottom_bar_hits(app: &App, area: Rect) -> Vec<(Rect, BottomBarHit)> {
    let specs = bottom_bar_specs(app);
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    for (label, hit) in specs {
        let w = label.chars().count() as u16 + 2; // label + 1 pad each side
        if x + w < area.x + area.width {
            hits.push((Rect::new(x, area.y, w, 1), hit));
        }
        x += w + 2; // 2-char gap between buttons
    }
    hits
}
