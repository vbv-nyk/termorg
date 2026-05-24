use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg};
use crate::server::ipc::{recv_msg, send_msg};
use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::app::{App, AppMode};
use super::draw::{draw_frame, focused_terminal_size, frame_layout, grid_terminal_size};
use super::handler::{compute_bottom_bar_hits, compute_sidebar_hits, handle_key, handle_mouse};

pub fn run_client() -> Result<()> {
    let sock_path = daemon::socket_path();
    let mut stream = UnixStream::connect(&sock_path)
        .context("could not connect to server — is termorg running?")?;

    send_msg(&mut stream, &ClientMsg::Attach)?;
    // Size the terminals for grid mode from the start.
    {
        let (tcols, trows) = terminal::size().context("get terminal size")?;
        let area = Rect::new(0, 0, tcols, trows);
        let layout = frame_layout(area);
        let (cols, rows) = grid_terminal_size(layout.main, 4); // assume ~4 tiles as default
        send_msg(&mut stream, &ClientMsg::ResizeAll { cols, rows })?;
    }

    let mut reader_stream = stream.try_clone().context("clone stream")?;
    let (server_tx, server_rx) = mpsc::channel::<ServerMsg>();
    thread::spawn(move || {
        while let Ok(msg) = recv_msg::<ServerMsg>(&mut reader_stream) {
            if server_tx.send(msg).is_err() {
                break;
            }
        }
    });

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen")?;
    terminal::enable_raw_mode().context("enable raw mode")?;

    let mut tui = Terminal::new(CrosstermBackend::new(stdout))
        .context("create ratatui terminal")?;

    let result = event_loop(&mut tui, &mut stream, &server_rx);

    let _ = terminal::disable_raw_mode();
    let _ = execute!(tui.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
    result
}

fn event_loop(
    tui: &mut Terminal<CrosstermBackend<io::Stdout>>,
    stream: &mut UnixStream,
    server_rx: &mpsc::Receiver<ServerMsg>,
) -> Result<()> {
    let mut app = App::new();

    loop {
        // 1. Drain server messages.
        loop {
            match server_rx.try_recv() {
                Ok(ServerMsg::TerminalList { active, ids }) => {
                    // If we switched to a different terminal, reset the local
                    // scroll offset counter — the new terminal starts at live view.
                    if app.active != Some(active) {
                        app.scroll_offset = 0;
                    }
                    app.active = Some(active);
                    app.ids = ids;
                    if app.selected.is_none() { app.selected = Some(active); }
                }
                Ok(ServerMsg::Snapshot(snap)) => {
                    app.snapshots.insert(snap.id, snap);
                }
                Ok(ServerMsg::GroupList(groups)) => {
                    for g in &groups { app.expanded_groups.insert(g.id); }
                    // Auto-select the first group if nothing is selected yet.
                    if app.selected_group.is_none() {
                        app.selected_group = groups.first().map(|g| g.id);
                    }
                    app.groups = groups;
                }
                Err(mpsc::TryRecvError::Empty)        => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // 2. Pre-compute hit areas for mouse handling.
        let size = tui.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let layout = frame_layout(area);
        // Tile areas only cover the terminals currently visible in the grid.
        let visible = app.visible_ids();
        let rects = super::draw::tile_rects(layout.main, visible.len());
        app.tile_areas = visible.iter().zip(rects.iter()).map(|(&id, &r)| (r, id)).collect();
        app.sidebar_hits = compute_sidebar_hits(&app, layout.sidebar);
        app.bottom_bar_hits = compute_bottom_bar_hits(&app, layout.bottom_bar);

        // 3. Draw.
        {
            let app_ref = &app;
            let layout_ref = &layout;
            tui.draw(|f| draw_frame(f, app_ref, layout_ref))?;
        }

        if app.quit { return Ok(()); }

        // 4. Handle input (~60 fps).
        if !event::poll(Duration::from_millis(16))? { continue; }

        match event::read()? {
            Event::Key(key)     => handle_key(&mut app, stream, key)?,
            Event::Mouse(mouse) => handle_mouse(&mut app, stream, mouse)?,
            Event::Resize(cols, rows) => {
                let area = Rect::new(0, 0, cols, rows);
                let layout = frame_layout(area);
                match &app.mode {
                    AppMode::Focused => {
                        let (tcols, trows) = focused_terminal_size(layout.main);
                        send_msg(stream, &ClientMsg::Resize { cols: tcols, rows: trows })?;
                    }
                    _ => {
                        let n = app.visible_ids().len().max(1);
                        let (tcols, trows) = grid_terminal_size(layout.main, n);
                        send_msg(stream, &ClientMsg::ResizeAll { cols: tcols, rows: trows })?;
                    }
                }
            }
            _ => {}
        }

        if app.quit { return Ok(()); }
    }
}
