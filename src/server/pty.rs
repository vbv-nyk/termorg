use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread;

/// Events sent from a PTY reader thread to the server's output channel.
pub enum PtyEvent {
    /// A chunk of output bytes from the PTY.
    Output(Vec<u8>),
    /// The PTY reached EOF — the shell process has exited.
    Exited,
}

/// A live PTY with a running shell inside.
pub struct Pty {
    /// Write here to send bytes to the shell (keypresses).
    writer: Box<dyn Write + Send>,
    /// The master side of the PTY — kept alive for resize calls.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Kept alive so the child process is not reaped.
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    /// PID of the shell process (the direct child we spawned).
    child_pid: Option<u32>,
}

impl Pty {
    /// Spawn `$SHELL` inside a new PTY.
    ///
    /// Every chunk of output the shell produces is forwarded as a `(TermId,
    /// Vec<u8>)` on `output_tx`. The sender is moved into a background reader
    /// thread.
    pub fn spawn(cols: u16, rows: u16, id: crate::proto::TermId, output_tx: Sender<(crate::proto::TermId, PtyEvent)>) -> Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let cmd = CommandBuilder::new(&shell);

        let child = pair.slave.spawn_command(cmd).context("spawn shell")?;
        let child_pid = child.process_id();

        // Clone the reader before taking the writer — both come from the master.
        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;

        // Background thread: read PTY output and forward it directly on the
        // shared output channel (tagged with the terminal id).
        // When the PTY reaches EOF (shell exits), sends a PtyEvent::Exited so
        // the server can mark the session as errored/dead.
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        // EOF or read error — the shell has exited.
                        let _ = output_tx.send((id, PtyEvent::Exited));
                        break;
                    }
                    Ok(n) => {
                        if output_tx.send((id, PtyEvent::Output(buf[..n].to_vec()))).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            writer,
            master: pair.master,
            _child: child,
            child_pid,
        })
    }

    /// Forward input bytes to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("write to pty")
    }

    /// Tell the PTY the viewport was resized.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pty")
    }

    /// Return the name of the foreground process running in this PTY, if any.
    ///
    /// When the shell is at the prompt (no command running) this returns None.
    /// When a command is running (e.g. "claude", "cargo") it returns Some("name").
    ///
    /// Implementation: reads /proc/{shell_pid}/task/{shell_pid}/children to find
    /// the direct child process the shell forked, then reads /proc/{child}/comm
    /// for its name. Linux-specific.
    pub fn foreground_process(&self) -> Option<String> {
        let pid = self.child_pid?;
        let children = std::fs::read_to_string(
            format!("/proc/{}/task/{}/children", pid, pid)
        ).ok()?;
        let child_pid: u32 = children.split_whitespace().next()?.parse().ok()?;
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", child_pid)).ok()?;
        Some(comm.trim().to_string())
    }
}
