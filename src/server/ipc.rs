use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg, TermId};
use crate::server::pty::PtyEvent;
use crate::server::state::ServerState;
use anyhow::{Context, Result};
use nix::libc;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};

/// Set to true by the SIGTERM handler so the main loop can exit gracefully.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Install a SIGTERM handler that sets the global SHUTDOWN flag.
///
/// # Safety
/// This is called once at server startup before any other threads exist, so
/// there are no race conditions in accessing the signal handler state.
fn install_sigterm_handler() {
    let handler = SigHandler::Handler(handle_sigterm);
    let action = SigAction::new(handler, SaFlags::empty(), SigSet::empty());
    // SAFETY: called once at startup, signal-safe handler writes only to an AtomicBool.
    unsafe { signal::sigaction(Signal::SIGTERM, &action).expect("install SIGTERM handler") };
}

extern "C" fn handle_sigterm(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

pub fn send_msg<T: Serialize>(stream: &mut UnixStream, msg: &T) -> Result<()> {
    let bytes = bincode::serialize(msg).context("serialize")?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).context("write length prefix")?;
    stream.write_all(&bytes).context("write message")?;
    Ok(())
}

pub fn recv_msg<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).context("read length prefix")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).context("read message body")?;
    bincode::deserialize(&buf).context("deserialize")
}

pub fn run_server() -> Result<()> {
    // Install SIGTERM handler so the server can shut down gracefully.
    install_sigterm_handler();

    let (output_tx, output_rx) = mpsc::channel::<(TermId, PtyEvent)>();
    let mut state = ServerState::new(output_tx);

    // Restore saved layout, or start with a single fresh terminal.
    let layout_path = daemon::layout_path();
    let layout = crate::layout::load(&layout_path);
    if !layout.group.is_empty() || !layout.terminal.is_empty() {
        state.apply_layout(&layout)?;
    } else {
        state.new_terminal(80, 24)?;
    }

    let sock_path = daemon::socket_path();
    let _ = fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).context("bind unix socket")?;
    // Non-blocking so we can drain PTY output between accept attempts.
    listener.set_nonblocking(true).context("set nonblocking")?;

    loop {
        // Check if we received SIGTERM — if so, shut down gracefully.
        if SHUTDOWN.load(Ordering::Relaxed) {
            eprintln!("[termorg] received SIGTERM — saving layout and exiting");
            persist_layout(&state);
            let _ = fs::remove_file(&sock_path);
            return Ok(());
        }

        // Always drain PTY output so sessions never go stale while waiting for
        // a client connection.
        while let Ok((id, event)) = output_rx.try_recv() {
            match event {
                PtyEvent::Output(bytes) => {
                    if let Some(session) = state.session_mut(id) {
                        session.process(&bytes);
                    }
                }
                PtyEvent::Exited => {
                    if let Some(session) = state.session_mut(id) {
                        session.exited = true;
                    }
                }
            }
        }

        match listener.accept() {
            Ok((stream, _)) => {
                let _ = serve_client(stream, &mut state, &output_rx);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No client waiting — sleep briefly then loop to drain output again.
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                // Real error — log it and continue rather than crashing.
                eprintln!("[termorg] accept error: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn serve_client(
    mut stream: UnixStream,
    state: &mut ServerState,
    output_rx: &mpsc::Receiver<(TermId, PtyEvent)>,
) -> Result<()> {
    match recv_msg::<ClientMsg>(&mut stream)? {
        ClientMsg::Attach => {}
        other => anyhow::bail!("expected Attach, got {:?}", other),
    }

    send_terminal_list(&mut stream, state)?;
    send_group_list(&mut stream, state)?;
    // Send snapshots of every terminal so the client has a full picture
    // from the first frame (needed for grid mode).
    for snap in state.all_snapshots() {
        send_msg(&mut stream, &ServerMsg::Snapshot(snap))?;
    }

    let mut reader_stream = stream.try_clone().context("clone stream")?;
    let (client_tx, client_rx) = mpsc::channel::<ClientMsg>();
    thread::spawn(move || {
        while let Ok(msg) = recv_msg::<ClientMsg>(&mut reader_stream) {
            if client_tx.send(msg).is_err() {
                break;
            }
        }
    });

    loop {
        match output_rx.recv_timeout(Duration::from_millis(10)) {
            Ok((id, PtyEvent::Output(bytes))) => {
                if let Some(session) = state.session_mut(id) {
                    session.process(&bytes);
                }
                // Send a snapshot for every terminal that produces output —
                // not just the active one — so grid mode tiles stay current.
                if let Some(snap) = state.snapshot(id) {
                    send_msg(&mut stream, &ServerMsg::Snapshot(snap))?;
                }
            }
            Ok((id, PtyEvent::Exited)) => {
                // Shell exited — mark the session as dead and push a snapshot
                // so the client immediately sees the Error status indicator.
                if let Some(session) = state.session_mut(id) {
                    session.exited = true;
                }
                if let Some(snap) = state.snapshot(id) {
                    send_msg(&mut stream, &ServerMsg::Snapshot(snap))?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }

        loop {
            match client_rx.try_recv() {
                Ok(ClientMsg::Input(bytes)) => {
                    // Any keypress resets scrollback to live view so typing
                    // always returns the user to the current output.
                    state.scroll_active(0);
                    if let Some(pty) = state.active_pty_mut() {
                        pty.write_input(&bytes)?;
                    }
                }
                Ok(ClientMsg::Resize { cols, rows }) => {
                    state.resize_active(cols, rows)?;
                }
                Ok(ClientMsg::ResizeAll { cols, rows }) => {
                    state.resize_all(cols, rows)?;
                    // Send updated snapshots immediately so the client
                    // gets content at the new size without a separate request.
                    for snap in state.all_snapshots() {
                        send_msg(&mut stream, &ServerMsg::Snapshot(snap))?;
                    }
                }
                Ok(ClientMsg::NewTerminal { group_id, cols, rows }) => {
                    let id = state.new_terminal(cols.max(1), rows.max(1))?;
                    if let Some(gid) = group_id {
                        state.move_terminal(id, Some(gid));
                    }
                    state.switch_to(id);
                    send_terminal_list(&mut stream, state)?;
                    send_group_list(&mut stream, state)?;
                    send_active_snapshot(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::SwitchTo(id)) => {
                    if state.switch_to(id) {
                        send_terminal_list(&mut stream, state)?;
                        send_active_snapshot(&mut stream, state)?;
                    }
                }
                Ok(ClientMsg::CloseTerminal(id)) => {
                    state.close_terminal(id)?;
                    // Always keep at least one terminal alive so the client
                    // never ends up with an empty list and no active terminal.
                    if state.active_id().is_none() {
                        state.new_terminal(80, 24)?;
                    }
                    send_terminal_list(&mut stream, state)?;
                    send_active_snapshot(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::RequestSnapshots) => {
                    for snap in state.all_snapshots() {
                        send_msg(&mut stream, &ServerMsg::Snapshot(snap))?;
                    }
                }
                Ok(ClientMsg::RenameTerminal { id, title }) => {
                    state.rename_terminal(id, title);
                    persist_layout(state);
                }
                Ok(ClientMsg::NewGroup { name, color }) => {
                    state.new_group(name, color);
                    send_group_list(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::RenameGroup { id, name }) => {
                    state.rename_group(id, name);
                    send_group_list(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::MoveTerminal { term_id, group_id }) => {
                    state.move_terminal(term_id, group_id);
                    send_group_list(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::DeleteGroup(id)) => {
                    state.delete_group(id);
                    send_group_list(&mut stream, state)?;
                    persist_layout(state);
                }
                Ok(ClientMsg::ScrollTo { offset }) => {
                    // When a new input arrives while in scrollback, reset to live view
                    // so the user can always return to bottom by typing.
                    state.scroll_active(offset);
                    send_active_snapshot(&mut stream, state)?;
                }
                Ok(ClientMsg::Detach) | Err(mpsc::TryRecvError::Disconnected) => {
                    return Ok(());
                }
                Ok(ClientMsg::Attach) => {}
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}

fn send_active_snapshot(stream: &mut UnixStream, state: &ServerState) -> Result<()> {
    if let Some(id) = state.active_id() {
        if let Some(snap) = state.snapshot(id) {
            send_msg(stream, &ServerMsg::Snapshot(snap))?;
        }
    }
    Ok(())
}

fn send_terminal_list(stream: &mut UnixStream, state: &ServerState) -> Result<()> {
    if let Some(active) = state.active_id() {
        send_msg(
            stream,
            &ServerMsg::TerminalList {
                active,
                ids: state.terminal_ids(),
            },
        )?;
    }
    Ok(())
}

fn send_group_list(stream: &mut UnixStream, state: &ServerState) -> Result<()> {
    send_msg(stream, &ServerMsg::GroupList(state.groups()))
}

fn persist_layout(state: &ServerState) {
    let layout = state.to_layout();
    let _ = crate::layout::save(&daemon::layout_path(), &layout);
}
