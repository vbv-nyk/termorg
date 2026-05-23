use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread;

/// A live PTY with a running shell inside.
pub struct Pty {
    /// Write here to send bytes to the shell (keypresses).
    writer: Box<dyn Write + Send>,
    /// The master side of the PTY — kept alive for resize calls.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Kept alive so the child process is not reaped.
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pty {
    /// Spawn `$SHELL` inside a new PTY.
    ///
    /// Every chunk of output the shell produces is sent as a `Vec<u8>` on
    /// `output_tx`. The sender is moved into a background reader thread.
    pub fn spawn(cols: u16, rows: u16, output_tx: Sender<Vec<u8>>) -> Result<Self> {
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

        // Clone the reader before taking the writer — both come from the master.
        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;

        // Background thread: read PTY output and forward it down the channel.
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output_tx.send(buf[..n].to_vec()).is_err() {
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
}
