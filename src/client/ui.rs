use crate::client::input::{key_to_action, InputAction};
use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg};
use crate::server::ipc::{recv_msg, send_msg};
use anyhow::{Context, Result};
use crossterm::event::{self, Event};
use crossterm::terminal;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Connect to a running server and run the client event loop.
pub fn run_client() -> Result<()> {
    let sock_path = daemon::socket_path();
    let mut stream = UnixStream::connect(&sock_path)
        .context("could not connect to server — is termorg running?")?;

    // Handshake: tell the server we are attaching.
    send_msg(&mut stream, &ClientMsg::Attach)?;

    // Immediately send our real terminal size so the PTY is resized to match.
    let (cols, rows) = terminal::size().context("get terminal size")?;
    send_msg(&mut stream, &ClientMsg::Resize { cols, rows })?;

    // Background thread: read ServerMsg frames and forward them on a channel.
    let mut reader_stream = stream.try_clone().context("clone stream")?;
    let (server_tx, server_rx) = mpsc::channel::<ServerMsg>();
    thread::spawn(move || loop {
        match recv_msg::<ServerMsg>(&mut reader_stream) {
            Ok(msg) => {
                if server_tx.send(msg).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    // Raw mode: the terminal stops processing keys itself and hands every
    // byte directly to us. Must be undone before we exit.
    terminal::enable_raw_mode().context("enable raw mode")?;
    let result = event_loop(&mut stream, &server_rx);
    let _ = terminal::disable_raw_mode();

    result
}

fn event_loop(
    stream: &mut UnixStream,
    server_rx: &mpsc::Receiver<ServerMsg>,
) -> Result<()> {
    let mut stdout = io::stdout();

    loop {
        // Write all server output that arrived since the last iteration.
        loop {
            match server_rx.try_recv() {
                Ok(ServerMsg::Output(bytes)) => {
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Wait up to 10 ms for a keyboard or resize event.
        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => match key_to_action(key) {
                    Some(InputAction::Forward(bytes)) => {
                        send_msg(stream, &ClientMsg::Input(bytes))?;
                    }
                    Some(InputAction::Quit) => {
                        let _ = send_msg(stream, &ClientMsg::Detach);
                        return Ok(());
                    }
                    None => {}
                },
                Event::Resize(cols, rows) => {
                    send_msg(stream, &ClientMsg::Resize { cols, rows })?;
                }
                _ => {}
            }
        }
    }
}
