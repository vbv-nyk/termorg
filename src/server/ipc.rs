use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg, TermId};
use crate::server::state::ServerState;
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};

/// Serialize `msg` and write it to `stream` as a length-prefixed frame.
pub fn send_msg<T: Serialize>(stream: &mut UnixStream, msg: &T) -> Result<()> {
    let bytes = bincode::serialize(msg).context("serialize")?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).context("write length prefix")?;
    stream.write_all(&bytes).context("write message")?;
    Ok(())
}

/// Read one length-prefixed frame from `stream` and deserialize it.
pub fn recv_msg<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .context("read length prefix")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).context("read message body")?;
    bincode::deserialize(&buf).context("deserialize")
}

/// Start the server: create initial terminal, bind the unix socket, serve clients.
pub fn run_server() -> Result<()> {
    let (output_tx, output_rx) = mpsc::channel::<(TermId, Vec<u8>)>();
    let mut state = ServerState::new(output_tx);

    // Start with a classic 80×24 terminal. The client sends a Resize with
    // the real dimensions as soon as it connects.
    state.new_terminal(80, 24)?;

    let sock_path = daemon::socket_path();
    let _ = fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).context("bind unix socket")?;

    loop {
        // While no client is attached, drain output into sessions so all
        // screen buffers stay current.
        while let Ok((id, bytes)) = output_rx.try_recv() {
            if let Some(session) = state.session_mut(id) {
                session.process(&bytes);
            }
        }

        let (stream, _) = listener.accept().context("accept client")?;
        let _ = serve_client(stream, &mut state, &output_rx);
    }
}

/// Handle one connected client until it detaches or all PTYs close.
fn serve_client(
    mut stream: UnixStream,
    state: &mut ServerState,
    output_rx: &mpsc::Receiver<(TermId, Vec<u8>)>,
) -> Result<()> {
    // The first message must be Attach.
    match recv_msg::<ClientMsg>(&mut stream)? {
        ClientMsg::Attach => {}
        other => anyhow::bail!("expected Attach, got {:?}", other),
    }

    // Tell the client which terminals exist and which is active.
    send_terminal_list(&mut stream, state)?;

    // Spawn a reader thread so client messages don't block the output loop.
    let mut reader_stream = stream.try_clone().context("clone stream")?;
    let (client_tx, client_rx) = mpsc::channel::<ClientMsg>();
    thread::spawn(move || loop {
        match recv_msg::<ClientMsg>(&mut reader_stream) {
            Ok(msg) => {
                if client_tx.send(msg).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    loop {
        // Wait up to 10 ms for any terminal's output.
        match output_rx.recv_timeout(Duration::from_millis(10)) {
            Ok((id, bytes)) => {
                if let Some(session) = state.session_mut(id) {
                    session.process(&bytes);
                }
                // Only forward to client if this is the active terminal.
                if state.active_id() == Some(id) {
                    send_msg(&mut stream, &ServerMsg::Output(bytes))?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }

        // Drain all client messages that arrived during the last 10 ms.
        loop {
            match client_rx.try_recv() {
                Ok(ClientMsg::Input(bytes)) => {
                    if let Some(pty) = state.active_pty_mut() {
                        pty.write_input(&bytes)?;
                    }
                }
                Ok(ClientMsg::Resize { cols, rows }) => {
                    if let Some(pty) = state.active_pty_mut() {
                        pty.resize(cols, rows)?;
                    }
                }
                Ok(ClientMsg::NewTerminal) => {
                    state.new_terminal(80, 24)?;
                    send_terminal_list(&mut stream, state)?;
                }
                Ok(ClientMsg::SwitchTo(id)) => {
                    if state.switch_to(id) {
                        send_terminal_list(&mut stream, state)?;
                    }
                }
                Ok(ClientMsg::CloseTerminal(id)) => {
                    state.close_terminal(id)?;
                    send_terminal_list(&mut stream, state)?;
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

/// Send the current terminal list to the client.
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
