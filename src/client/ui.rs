use crate::client::input::{key_to_action, InputAction};
use crate::daemon;
use crate::proto::{ClientMsg, ServerMsg, TermId};
use crate::server::ipc::{recv_msg, send_msg};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Whether the next keypress is a PTY input or a termorg command.
enum Mode {
    /// All keypresses go to the active PTY.
    Passthrough,
    /// Ctrl-Space was pressed — next key is a termorg command.
    Prefix,
}

/// Client-side knowledge of the terminal list.
struct TermList {
    active: TermId,
    ids: Vec<TermId>,
}

impl TermList {
    fn next_id(&self) -> Option<TermId> {
        let pos = self.ids.iter().position(|&id| id == self.active)?;
        self.ids.get(pos + 1).copied()
    }

    fn prev_id(&self) -> Option<TermId> {
        let pos = self.ids.iter().position(|&id| id == self.active)?;
        if pos == 0 { None } else { self.ids.get(pos - 1).copied() }
    }
}

/// Connect to a running server and run the client event loop.
pub fn run_client() -> Result<()> {
    let sock_path = daemon::socket_path();
    let mut stream = UnixStream::connect(&sock_path)
        .context("could not connect to server — is termorg running?")?;

    send_msg(&mut stream, &ClientMsg::Attach)?;

    let (cols, rows) = terminal::size().context("get terminal size")?;
    send_msg(&mut stream, &ClientMsg::Resize { cols, rows })?;

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
    let mut mode = Mode::Passthrough;
    let mut terms: Option<TermList> = None;

    loop {
        // Process all server messages that arrived since the last iteration.
        loop {
            match server_rx.try_recv() {
                Ok(ServerMsg::Output(bytes)) => {
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
                Ok(ServerMsg::TerminalList { active, ids }) => {
                    // Print a brief status line so the user knows which
                    // terminal is active. \r\n is required in raw mode.
                    let idx = ids.iter().position(|&id| id == active)
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let total = ids.len();
                    write!(stdout, "\r\n[termorg: terminal {idx}/{total} — Ctrl-Space ? for help]\r\n")?;
                    stdout.flush()?;
                    terms = Some(TermList { active, ids });
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        if !event::poll(Duration::from_millis(10))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => match mode {
                Mode::Passthrough => match key_to_action(key) {
                    Some(InputAction::Forward(bytes)) => {
                        send_msg(stream, &ClientMsg::Input(bytes))?;
                    }
                    Some(InputAction::Prefix) => {
                        mode = Mode::Prefix;
                    }
                    None => {}
                },

                Mode::Prefix => {
                    mode = Mode::Passthrough;
                    match key.code {
                        // q — detach
                        KeyCode::Char('q') => {
                            let _ = send_msg(stream, &ClientMsg::Detach);
                            return Ok(());
                        }
                        // c — create new terminal
                        KeyCode::Char('c') => {
                            send_msg(stream, &ClientMsg::NewTerminal)?;
                        }
                        // n — switch to next terminal
                        KeyCode::Char('n') => {
                            if let Some(id) = terms.as_ref().and_then(|t| t.next_id()) {
                                send_msg(stream, &ClientMsg::SwitchTo(id))?;
                            }
                        }
                        // p — switch to previous terminal
                        KeyCode::Char('p') => {
                            if let Some(id) = terms.as_ref().and_then(|t| t.prev_id()) {
                                send_msg(stream, &ClientMsg::SwitchTo(id))?;
                            }
                        }
                        // 1-9 — switch to terminal by number
                        KeyCode::Char(d) if d.is_ascii_digit() && d != '0' => {
                            let n = (d as u32) - ('0' as u32) - 1;
                            if let Some(&id) = terms.as_ref().and_then(|t| t.ids.get(n as usize)) {
                                send_msg(stream, &ClientMsg::SwitchTo(id))?;
                            }
                        }
                        // ? — print help
                        KeyCode::Char('?') => {
                            write!(stdout,
                                "\r\n[termorg keys: c=new  n=next  p=prev  1-9=switch  q=detach]\r\n"
                            )?;
                            stdout.flush()?;
                        }
                        // anything else — forward the key as normal input
                        _ => {
                            if let Some(InputAction::Forward(bytes)) = key_to_action(key) {
                                send_msg(stream, &ClientMsg::Input(bytes))?;
                            }
                        }
                    }
                }
            },

            Event::Resize(cols, rows) => {
                send_msg(stream, &ClientMsg::Resize { cols, rows })?;
            }
            _ => {}
        }
    }
}
