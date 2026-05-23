use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg};
use crate::server::pty::Pty;
use crate::server::session::Session;
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
    stream.read_exact(&mut len_buf).context("read length prefix")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).context("read message body")?;
    bincode::deserialize(&buf).context("deserialize")
}

/// Start the server: spawn a PTY, bind the unix socket, and serve clients.
pub fn run_server() -> Result<()> {
    let (pty_tx, pty_rx) = mpsc::channel::<Vec<u8>>();

    // Start with a classic 80×24 terminal. The client sends a Resize with
    // the real dimensions as soon as it connects.
    let mut pty = Pty::spawn(80, 24, pty_tx)?;
    let mut session = Session::new(80, 24);

    let sock_path = daemon::socket_path();
    let _ = fs::remove_file(&sock_path); // remove stale socket from a prior run
    let listener = UnixListener::bind(&sock_path).context("bind unix socket")?;

    loop {
        // While no client is attached, drain PTY output into the session so
        // the screen buffer stays up to date.
        while let Ok(bytes) = pty_rx.try_recv() {
            session.process(&bytes);
        }

        let (stream, _) = listener.accept().context("accept client")?;
        // Ignore client errors (disconnect, bad handshake, etc.) — keep running.
        let _ = serve_client(stream, &mut pty, &mut session, &pty_rx);
    }
}

/// Handle one connected client until it detaches or the PTY closes.
fn serve_client(
    mut stream: UnixStream,
    pty: &mut Pty,
    session: &mut Session,
    pty_rx: &mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    // The first message must be Attach.
    match recv_msg::<ClientMsg>(&mut stream)? {
        ClientMsg::Attach => {}
        other => anyhow::bail!("expected Attach, got {:?}", other),
    }

    // Spawn a reader thread so we can receive client messages without
    // blocking the PTY-output forwarding loop.
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

    // Main loop: forward PTY output to the client and process incoming
    // client messages.
    loop {
        // Block for up to 10 ms waiting for PTY output.
        match pty_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(bytes) => {
                session.process(&bytes);
                send_msg(&mut stream, &ServerMsg::Output(bytes))?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Shell exited — server has nothing left to do.
                return Ok(());
            }
        }

        // Drain all client messages that arrived during the last 10 ms.
        loop {
            match client_rx.try_recv() {
                Ok(ClientMsg::Input(bytes)) => pty.write_input(&bytes)?,
                Ok(ClientMsg::Resize { cols, rows }) => pty.resize(cols, rows)?,
                Ok(ClientMsg::Detach) | Err(mpsc::TryRecvError::Disconnected) => {
                    return Ok(());
                }
                Ok(ClientMsg::Attach) => {} // ignore duplicate attach
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}
