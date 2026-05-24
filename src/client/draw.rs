use crate::proto::{Cell, Color, GroupColor, ScreenSnapshot, TermId, TermStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color as RColor, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};
use super::app::{App, AppMode, BottomBarHit};

// ─── Palette ─────────────────────────────────────────────────────────────────

pub mod pal {
    use ratatui::style::Color;
    pub const BG:     Color = Color::Rgb(15,  18,  23 );
    pub const BG_ELV: Color = Color::Rgb(22,  26,  32 );
    pub const FG:     Color = Color::Rgb(226, 218, 196);
    pub const DIM:    Color = Color::Rgb(128, 120, 102);
    pub const MUTE:   Color = Color::Rgb(74,  70,  64 );
    pub const QUIET:  Color = Color::Rgb(100, 96,  88 );
    #[allow(dead_code)]
    pub const RULE:   Color = Color::Rgb(41,  43,  50 );
    pub const OCHRE:  Color = Color::Rgb(212, 165, 116);
    pub const SAGE:   Color = Color::Rgb(148, 172, 133);
    pub const TERRA:  Color = Color::Rgb(201, 120, 90 );
    pub const SLATE:  Color = Color::Rgb(122, 146, 173);
    pub const ACTIVE: Color = Color::Rgb(159, 187, 133);
    pub const ATTN:   Color = Color::Rgb(224, 167, 93 );
    pub const ERROR:  Color = Color::Rgb(210, 119, 101);
}

pub const SIDEBAR_W: u16 = 24;

// ─── Frame layout ────────────────────────────────────────────────────────────

pub struct FrameLayout {
    pub top_bar:    Rect,
    pub sidebar:    Rect,
    pub main:       Rect,
    pub bottom_bar: Rect,
}

pub fn frame_layout(area: Rect) -> FrameLayout {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_W), Constraint::Min(1)])
        .split(outer[1]);

    FrameLayout {
        top_bar:    outer[0],
        sidebar:    content[0],
        main:       content[1],
        bottom_bar: outer[2],
    }
}

// ─── Top-level draw ──────────────────────────────────────────────────────────

pub fn draw_frame(f: &mut ratatui::Frame, app: &App, layout: &FrameLayout) {
    let bg = Block::default().style(Style::default().bg(pal::BG));
    f.render_widget(bg, f.area());

    draw_top_bar(f, app, layout.top_bar);

    draw_sidebar(f, app, layout.sidebar);

    match &app.mode {
        AppMode::Grid | AppMode::Renaming { .. } => draw_grid(f, app, layout.main),
        AppMode::Focused | AppMode::RenamingTerminal { .. } => draw_focused(f, app, layout.main),
    }

    draw_bottom_bar(f, app, layout.bottom_bar);
}

// ─── Top bar ─────────────────────────────────────────────────────────────────

fn draw_top_bar(f: &mut ratatui::Frame, app: &App, area: Rect) {
    // Breadcrumb: termorg · group-name (or "all terminals")
    let group_crumb = match app.selected_group {
        Some(gid) => app.groups.iter()
            .find(|g| g.id == gid)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "—".to_string()),
        None => "ungrouped".to_string(),
    };

    let left = match &app.mode {
        AppMode::Focused => Line::from(vec![
            Span::styled("● ", Style::default().fg(pal::OCHRE)),
            Span::styled("termorg", Style::default().fg(pal::FG)),
            Span::styled(" · ", Style::default().fg(pal::MUTE)),
            Span::styled(group_crumb, Style::default().fg(pal::DIM)),
            Span::styled(" · ", Style::default().fg(pal::MUTE)),
            Span::styled(
                app.active.map(|id| app.term_title(id)).unwrap_or_default(),
                Style::default().fg(pal::FG),
            ),
        ]),
        _ => Line::from(vec![
            Span::styled("● ", Style::default().fg(pal::OCHRE)),
            Span::styled("termorg", Style::default().fg(pal::FG)),
            Span::styled(" · ", Style::default().fg(pal::MUTE)),
            Span::styled(group_crumb, Style::default().fg(pal::FG)),
        ]),
    };

    let all_statuses: Vec<TermStatus> = app.ids.iter().map(|&id| app.term_status(id)).collect();
    let (agg_glyph, agg_color) = status_glyph_color(loudest_status(&all_statuses));

    let right = Line::from(vec![
        Span::styled(agg_glyph, Style::default().fg(agg_color)),
        Span::raw(" "),
    ]);

    f.render_widget(
        Paragraph::new(left).style(Style::default().bg(pal::BG_ELV).fg(pal::FG)),
        area,
    );

    let right_width: u16 = 4;
    if area.width > right_width {
        let right_area = Rect { x: area.x + area.width - right_width, y: area.y, width: right_width, height: 1 };
        f.render_widget(Paragraph::new(right).style(Style::default().bg(pal::BG_ELV)), right_area);
    }
}

// ─── Sidebar ─────────────────────────────────────────────────────────────────

pub fn draw_sidebar(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let w = area.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("GROUPS", Style::default().fg(pal::DIM).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{:>width$}", format!("({})", app.groups.len()), width = w.saturating_sub(7)),
            Style::default().fg(pal::MUTE),
        ),
    ]));
    lines.push(Line::from(""));

    for group in &app.groups {
        let gcolor = group_color_ratatui(group.color);
        let is_selected = app.selected_group == Some(group.id);
        let expanded = app.expanded_groups.contains(&group.id);
        let arrow = if expanded { "▾" } else { "▸" };
        let (glyph, sc) = status_glyph_color(app.group_status(group.id));
        let header_bg = if is_selected { pal::BG_ELV } else { pal::BG };

        // Layout: ▾(1) _(1) name(w-7) _(1)glyph(1) __(2)+(1)
        // The name is padded to exactly (w-7) chars so "  +" always lands at
        // the last 3 columns, matching the AddTerminal hit zone.
        let name_cols = w.saturating_sub(7);
        let raw_name = truncate_str(&group.name, name_cols);
        let padded_name = format!("{:<width$}", raw_name, width = name_cols);

        let active_in_group = app.active
            .map(|aid| app.snap_group(aid) == Some(group.id))
            .unwrap_or(false);
        let mut name_style = Style::default().fg(pal::FG).bg(header_bg);
        if active_in_group {
            name_style = name_style.fg(gcolor).add_modifier(Modifier::BOLD);
        }

        lines.push(Line::from(vec![
            Span::styled(arrow.to_string(), Style::default().fg(gcolor).bg(header_bg)),
            Span::raw(" "),
            Span::styled(padded_name, name_style),
            Span::styled(format!(" {}", glyph), Style::default().fg(sc).bg(header_bg)),
            Span::styled("  +", Style::default().fg(pal::MUTE).bg(header_bg)),
        ]));

        if expanded {
            for &id in &app.ids {
                if app.snap_group(id) != Some(group.id) { continue; }
                let title = app.term_title(id);
                let (tglyph, tc) = status_glyph_color(app.term_status(id));
                let is_active = app.active == Some(id);
                let is_sel = app.selected == Some(id);
                let bg = if is_sel || is_active { pal::BG_ELV } else { pal::BG };
                let name_col = if is_active { gcolor } else { pal::FG };
                let mut name_style = Style::default().fg(name_col).bg(bg);
                if is_active { name_style = name_style.add_modifier(Modifier::BOLD); }
                let tname = truncate_str(&title, w.saturating_sub(8));

                lines.push(Line::from(vec![
                    Span::styled("  ▎ ".to_string(), Style::default().fg(gcolor)),
                    Span::styled(tname, name_style),
                    Span::styled(format!(" {}", tglyph), Style::default().fg(tc).bg(bg)),
                ]));
            }
        }

        lines.push(Line::from(""));
    }

    // Ungrouped section.
    let ungrouped: Vec<TermId> = app.ids.iter().copied()
        .filter(|&id| app.snap_group(id).is_none())
        .collect();

    if !ungrouped.is_empty() {
        let header_bg = if app.selected_group.is_none() { pal::BG_ELV } else { pal::BG };
        lines.push(Line::from(vec![
            Span::styled("─ ungrouped ─".to_string(), Style::default().fg(pal::MUTE).bg(header_bg)),
        ]));
        for id in ungrouped {
            let title = app.term_title(id);
            let (tglyph, tc) = status_glyph_color(app.term_status(id));
            let is_active = app.active == Some(id);
            let is_sel = app.selected == Some(id);
            let bg = if is_sel || is_active { pal::BG_ELV } else { pal::BG };
            let mut name_style = Style::default().fg(pal::FG).bg(bg);
            if is_active { name_style = name_style.add_modifier(Modifier::BOLD); }
            let tname = truncate_str(&title, w.saturating_sub(6));

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(tname, name_style),
                Span::styled(format!(" {}", tglyph), Style::default().fg(tc).bg(bg)),
            ]));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled("+ new group", Style::default().fg(pal::MUTE)),
    ]));

    f.render_widget(Paragraph::new(lines).style(Style::default().bg(pal::BG).fg(pal::FG)), area);
}

// ─── Grid mode ───────────────────────────────────────────────────────────────

fn draw_grid(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let visible = app.visible_ids();

    if visible.is_empty() {
        // Show a hint when the selected group has no terminals yet.
        let hint = match app.selected_group {
            Some(_) => "No terminals in this group.\nClick [+] next to a group to add one.",
            None    => "No ungrouped terminals.\nClick [+] next to a group to add one.",
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(pal::DIM).bg(pal::BG)),
            area,
        );
        return;
    }

    let rects = tile_rects(area, visible.len());

    for (i, &id) in visible.iter().enumerate() {
        let Some(&tile) = rects.get(i) else { continue };

        let accent = app.term_accent_color(id);
        let border_color = if app.selected == Some(id) {
            pal::FG
        } else if app.active == Some(id) {
            accent
        } else {
            pal::QUIET
        };

        let title_str = app.term_title(id);
        let (glyph, sc) = status_glyph_color(app.term_status(id));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Line::from(vec![
                Span::styled("▎ ", Style::default().fg(accent)),
                Span::styled(truncate_str(&title_str, 14), Style::default().fg(pal::FG)),
                Span::raw(" "),
                Span::styled(glyph, Style::default().fg(sc)),
            ]));

        let inner = block.inner(tile);
        f.render_widget(block, tile);

        if let Some(snap) = app.snapshots.get(&id) {
            // Clip to tile dimensions so wide/tall PTYs don't overflow.
            let text = snapshot_to_text_clipped(snap, inner.width as usize, inner.height as usize);
            f.render_widget(Paragraph::new(text), inner);

            // Show "[process exited]" overlay at the bottom of errored tiles.
            if snap.status == TermStatus::Error && inner.height > 0 {
                let notice = Span::styled(
                    " [process exited] ",
                    Style::default().fg(pal::BG).bg(pal::ERROR),
                );
                let notice_area = Rect {
                    x: inner.x,
                    y: inner.y + inner.height - 1,
                    width: inner.width.min(18),
                    height: 1,
                };
                f.render_widget(Paragraph::new(Line::from(notice)), notice_area);
            }
        }
    }
}

// ─── Focused mode ────────────────────────────────────────────────────────────

fn draw_focused(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let Some(id) = app.active else { return };
    let Some(snap) = app.snapshots.get(&id) else { return };

    let accent = app.term_accent_color(id);
    let (glyph, sc) = status_glyph_color(snap.status);

    // Build title spans; add a scrollback indicator when not at live view.
    let mut title_spans = vec![
        Span::styled("▎ ", Style::default().fg(accent)),
        Span::styled(snap.title.clone(), Style::default().fg(pal::FG)),
        Span::styled(format!(" {}", glyph), Style::default().fg(sc)),
    ];
    if snap.scroll_offset > 0 {
        title_spans.push(Span::styled(
            format!("  ↑ -{} lines  PgDn to return", snap.scroll_offset),
            Style::default().fg(pal::ATTN),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Line::from(title_spans));

    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(snapshot_to_text(snap)), inner);

    // Show "[process exited]" at the bottom of focused mode when the shell exited.
    if snap.status == TermStatus::Error && inner.height > 0 {
        let notice = Span::styled(
            " [process exited] Press Ctrl-G to return to grid ",
            Style::default().fg(pal::BG).bg(pal::ERROR),
        );
        let notice_area = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width.min(50),
            height: 1,
        };
        f.render_widget(Paragraph::new(Line::from(notice)), notice_area);
    }

    // Only show cursor when at live view (scrolled = cursor is off-screen anyway).
    if snap.scroll_offset == 0 && snap.status != TermStatus::Error {
        let cx = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
        let cy = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
        f.set_cursor_position((cx, cy));
    }
}

// ─── Bottom bar ──────────────────────────────────────────────────────────────

/// The list of action buttons shown in the bottom bar for a given app state.
/// Each entry is (display_label, hit_target).
/// Must stay in sync with `compute_bottom_bar_hits` in handler.rs.
pub fn bottom_bar_specs(app: &App) -> Vec<(&'static str, BottomBarHit)> {
    match &app.mode {
        AppMode::Grid => {
            let mut v: Vec<(&'static str, BottomBarHit)> = Vec::new();
            if app.selected_group.is_some() {
                v.push(("✎ rename group", BottomBarHit::RenameGroup));
            }
            v.push(("+ new group", BottomBarHit::NewGroup));
            v.push(("✕ quit", BottomBarHit::Quit));
            v
        }
        AppMode::Focused => vec![
            ("✕ close terminal", BottomBarHit::CloseTerminal),
            ("✕ quit", BottomBarHit::Quit),
        ],
        AppMode::Renaming { .. } | AppMode::RenamingTerminal { .. } => vec![],
    }
}

fn draw_bottom_bar(f: &mut ratatui::Frame, app: &App, area: Rect) {
    // Both rename modes share the same inline-input bar appearance.
    let rename_buffer = match &app.mode {
        AppMode::Renaming { buffer, .. } => Some(("RENAME GROUP", buffer.clone())),
        AppMode::RenamingTerminal { buffer, .. } => Some(("RENAME TERM", buffer.clone())),
        _ => None,
    };
    if let Some((label, buffer)) = rename_buffer {
        let line = Line::from(vec![
            Span::styled(format!(" {} ", label), Style::default().fg(pal::OCHRE).bg(pal::BG_ELV)),
            Span::raw("  "),
            Span::styled(buffer, Style::default().fg(pal::FG)),
            Span::styled("█", Style::default().fg(pal::OCHRE)),
            Span::styled("  ⏎ confirm  Esc cancel", Style::default().fg(pal::DIM)),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(pal::BG_ELV)),
            area,
        );
        return;
    }

    // Render clickable buttons by replaying the same positions used in compute_bottom_bar_hits.
    let specs = bottom_bar_specs(app);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut x = 1u16;
    for (label, _) in &specs {
        let w = label.chars().count() as u16 + 2; // 1 pad on each side
        // Only render if it fits in the bar.
        if x + w < area.width {
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(pal::FG).bg(pal::BG_ELV),
            ));
            spans.push(Span::raw("  "));
            x += w + 2;
        }
    }

    if let AppMode::Focused = &app.mode {
        spans.push(Span::styled(
            "  Ctrl-G: grid  Ctrl-R: rename  PgUp/PgDn: scroll",
            Style::default().fg(pal::MUTE),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(pal::BG_ELV).fg(pal::DIM)),
        area,
    );
}

// ─── Tile layout ─────────────────────────────────────────────────────────────

/// Choose a column count that makes a balanced grid for `n` tiles.
///
/// Rules:
///   1  → 1 column
///   2  → 2 columns
///   3-4 → 2 columns
///   5-6 → 3 columns
///   7+  → ceil(sqrt(n)) columns
pub fn grid_cols_pub(n: usize) -> usize {
    grid_cols(n)
}

fn grid_cols(n: usize) -> usize {
    match n {
        0 => 1,
        1 => 1,
        2..=4 => 2,
        5..=6 => 3,
        _ => (n as f64).sqrt().ceil() as usize,
    }
}

pub fn tile_rects(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 { return Vec::new(); }
    let cols = grid_cols(n);
    let rows = n.div_ceil(cols);
    let col_pct = 100 / cols as u16;
    let row_pct = 100 / rows as u16;

    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Percentage(row_pct); rows])
        .split(area);

    let mut tiles = Vec::with_capacity(n);
    'outer: for row_area in row_areas.iter() {
        let cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Percentage(col_pct); cols])
            .split(*row_area);
        for cell in cells.iter() {
            tiles.push(*cell);
            if tiles.len() == n { break 'outer; }
        }
    }
    tiles
}

// ─── Snapshot → Text ─────────────────────────────────────────────────────────

pub fn snapshot_to_text(snap: &ScreenSnapshot) -> Text<'static> {
    Text::from(
        snap.cells
            .iter()
            .map(|row| Line::from(row.iter().map(cell_to_span).collect::<Vec<_>>()))
            .collect::<Vec<_>>(),
    )
}

/// Like `snapshot_to_text` but clips to at most `max_cols` columns and
/// `max_rows` rows (showing the bottom of the screen, which is where the
/// cursor is). Prevents a wide/tall PTY from overflowing a smaller tile.
fn snapshot_to_text_clipped(snap: &ScreenSnapshot, max_cols: usize, max_rows: usize) -> Text<'static> {
    let start = snap.cells.len().saturating_sub(max_rows);
    Text::from(
        snap.cells[start..]
            .iter()
            .map(|row| {
                let spans: Vec<Span> = row.iter()
                    .take(max_cols)
                    .map(cell_to_span)
                    .collect();
                Line::from(spans)
            })
            .collect::<Vec<_>>(),
    )
}

fn cell_to_span(cell: &Cell) -> Span<'static> {
    let mut style = Style::default()
        .fg(proto_color(cell.fg))
        .bg(proto_color(cell.bg));
    if cell.bold      { style = style.add_modifier(Modifier::BOLD);       }
    if cell.italic    { style = style.add_modifier(Modifier::ITALIC);     }
    if cell.underline { style = style.add_modifier(Modifier::UNDERLINED); }
    Span::styled(cell.ch.to_string(), style)
}

// ─── Colour helpers ───────────────────────────────────────────────────────────

fn proto_color(c: Color) -> RColor {
    match c {
        Color::Default      => RColor::Reset,
        Color::Indexed(n)   => RColor::Indexed(n),
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
    }
}

pub fn group_color_ratatui(c: GroupColor) -> RColor {
    match c {
        GroupColor::Ochre => pal::OCHRE,
        GroupColor::Sage  => pal::SAGE,
        GroupColor::Terra => pal::TERRA,
        GroupColor::Slate => pal::SLATE,
    }
}

// ─── Status helpers ───────────────────────────────────────────────────────────

pub fn status_glyph_color(s: TermStatus) -> (&'static str, RColor) {
    match s {
        TermStatus::Active    => ("●", pal::ACTIVE),
        TermStatus::Attention => ("✱", pal::ATTN),
        TermStatus::Error     => ("✗", pal::ERROR),
        TermStatus::Quiet     => ("·", pal::QUIET),
    }
}

pub fn loudest_status(statuses: &[TermStatus]) -> TermStatus {
    if      statuses.contains(&TermStatus::Error)     { TermStatus::Error     }
    else if statuses.contains(&TermStatus::Active)    { TermStatus::Active    }
    else if statuses.contains(&TermStatus::Attention) { TermStatus::Attention }
    else                                              { TermStatus::Quiet     }
}

// ─── String helpers ───────────────────────────────────────────────────────────

pub fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

pub fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

// ─── Terminal size helpers ────────────────────────────────────────────────────

/// Usable (cols, rows) for a terminal rendered in focused mode inside `main`.
pub fn focused_terminal_size(main: Rect) -> (u16, u16) {
    (main.width.saturating_sub(2), main.height.saturating_sub(2))
}

/// Usable (cols, rows) for a terminal rendered in a grid tile.
pub fn grid_terminal_size(main: Rect, n: usize) -> (u16, u16) {
    let rects = tile_rects(main, n);
    let tile = rects.first().copied().unwrap_or(main);
    (tile.width.saturating_sub(2), tile.height.saturating_sub(2))
}
