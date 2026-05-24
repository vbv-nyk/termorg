mod client;
mod daemon;
mod layout;
mod proto;
mod server;

use anyhow::{bail, Context, Result};
use daemon::DaemonRole;
use nix::sys::signal::{kill, Signal};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        None | Some("attach") => attach(),
        Some("kill") => kill_server(),
        Some("--version") | Some("-V") | Some("version") => {
            println!("termorg {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") | Some("help") => {
            println!("termorg {} — terminal organizer / multiplexer", env!("CARGO_PKG_VERSION"));
            println!();
            println!("USAGE:");
            println!("  termorg [attach]   Start the server (if not running) and attach a client");
            println!("  termorg kill       Stop the running server");
            println!("  termorg --version  Print version");
            println!("  termorg --help     Print this help");
            println!();
            println!("KEYBOARD SHORTCUTS:");
            println!("  Grid mode:");
            println!("    Arrow keys   Navigate between terminals");
            println!("    Enter        Focus the selected terminal");
            println!("    n            New terminal in current group");
            println!("    g            New group");
            println!("    r            Rename current group");
            println!("    q / Q        Quit");
            println!();
            println!("  Focused mode:");
            println!("    Ctrl-G       Return to grid view");
            println!("    Ctrl-R       Rename this terminal");
            println!("    Page-Up      Scroll back through output history");
            println!("    Page-Down    Scroll forward (back to live view)");
            println!();
            println!("STATE DIR: ~/.local/state/termorg/");
            Ok(())
        }
        Some(unknown) => bail!("unknown subcommand: {unknown}\nRun 'termorg --help' for usage."),
    }
}

/// Start a server if none is running, then connect as a client.
fn attach() -> Result<()> {
    if !daemon::server_is_running() {
        match daemon::daemonize()? {
            DaemonRole::Server => {
                // We are the daemon child. Run the server — this blocks forever.
                return server::ipc::run_server();
            }
            DaemonRole::Client => {
                // We are the original process. Wait for the server to be ready.
                wait_for_server()?;
            }
        }
    }

    client::ui::run_client()
}

/// Wait until the server's socket file appears (up to 2 s).
///
/// We check for the file rather than connecting — a probe connection would be
/// accepted by the server and interfere with its accept loop.
fn wait_for_server() -> Result<()> {
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(100));
        if daemon::socket_path().exists() {
            return Ok(());
        }
    }
    bail!("server did not start within 2 seconds")
}

/// Send SIGTERM to the running server.
fn kill_server() -> Result<()> {
    let pid = daemon::read_pid().context("no server running (pidfile not found)")?;
    kill(pid, Signal::SIGTERM).context("kill server")?;
    println!("termorg server stopped.");
    Ok(())
}
