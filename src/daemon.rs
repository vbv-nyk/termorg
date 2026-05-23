use anyhow::{Context, Result};
use nix::unistd::{dup2, fork, getpid, setsid, ForkResult, Pid};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::fs;

pub fn state_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home).join(".local/state/termorg")
}

pub fn socket_path() -> PathBuf {
    state_dir().join("server.sock")
}

pub fn pid_path() -> PathBuf {
    state_dir().join("server.pid")
}

/// Which side of the fork we are on after `daemonize()` returns.
pub enum DaemonRole {
    /// We are the daemon child: run the server.
    Server,
    /// We are the original process: connect as the client.
    Client,
}

/// Fork the process. The child becomes the daemon (setsid + redirect stdio).
/// The parent returns `DaemonRole::Client` and keeps running so it can
/// connect to the server right after.
pub fn daemonize() -> Result<DaemonRole> {
    fs::create_dir_all(state_dir()).context("create state dir")?;

    match unsafe { fork().context("fork")? } {
        ForkResult::Parent { .. } => {
            // The original process becomes the client — do not exit.
            return Ok(DaemonRole::Client);
        }
        ForkResult::Child => {}
    }

    // --- daemon child from here ---

    setsid().context("setsid")?;

    let devnull = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .context("open /dev/null")?;
    let fd = devnull.as_raw_fd();
    dup2(fd, 0).context("dup2 stdin")?;
    dup2(fd, 1).context("dup2 stdout")?;
    dup2(fd, 2).context("dup2 stderr")?;

    fs::write(pid_path(), getpid().to_string()).context("write pidfile")?;

    Ok(DaemonRole::Server)
}

/// Returns true if a server process from the pidfile is still alive.
///
/// We check `/proc/<pid>` rather than connecting to the socket — a connection
/// probe would be accepted by the server and corrupt its accept loop.
pub fn server_is_running() -> bool {
    if let Some(pid) = read_pid() {
        std::path::Path::new(&format!("/proc/{}", pid.as_raw())).exists()
    } else {
        false
    }
}

/// Read the PID from the pidfile. Returns None if no server is running.
pub fn read_pid() -> Option<Pid> {
    let text = fs::read_to_string(pid_path()).ok()?;
    let n: i32 = text.trim().parse().ok()?;
    Some(Pid::from_raw(n))
}
