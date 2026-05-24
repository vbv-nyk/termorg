# termorg

A terminal multiplexer/organizer written in Rust. A background daemon manages PTYs and state; a TUI client connects over a Unix domain socket. Built with `ratatui` (rendering), `crossterm` (input/raw mode), `portable-pty` (PTY management), `vt100` (VT100 parser), `bincode` + `serde` (IPC serialization), and `toml` (layout persistence).

## Build & run

```
cargo build --release          # build
cargo run                      # attach (starts daemon if not running)
cargo run -- kill              # stop the daemon
cargo run -- attach            # explicit attach
```

State files live at `~/.local/state/termorg/` (socket, pidfile, layout.toml).

## Architecture

```
main.rs          CLI entry point — fork/daemonize dance
daemon.rs        State-dir paths, fork helper, pidfile, server-liveness check
proto.rs         Shared types: TermId, GroupId, ScreenSnapshot, ClientMsg, ServerMsg
layout.rs        TOML persistence (groups + terminals)

server/
  ipc.rs         Unix socket listener, serve-client loop, message dispatch
  state.rs       ServerState — owns PTYs, Sessions, Groups, term→group mapping
  session.rs     vt100::Parser wrapper, snapshot generation, status timing
  pty.rs         portable-pty wrapper — spawn $SHELL, resize, foreground-process

client/
  ui.rs          ratatui event loop, server message drain, resize handling
  app.rs         App struct — client-side view state (snapshots, groups, mode)
  draw.rs        All ratatui rendering: top-bar, sidebar, grid, focused, bottom-bar
  handler.rs     Key + mouse dispatch; sidebar/bottom-bar hit-area computation
  input.rs       Crossterm key → raw bytes mapping
```

## Key design notes

- One client at a time. The server's outer loop is `try_recv PTY output → accept() → serve_client()`. While serving, PTY output is drained via `recv_timeout(10ms)`.
- `ScreenSnapshot` is the unit of currency: server sends one per terminal per output chunk.
- `AppMode` drives both rendering and input routing: `Grid` (overview, mouse-only), `Focused` (raw keystroke passthrough), `Renaming` (inline group rename).
- Hit areas for mouse clicks (`tile_areas`, `sidebar_hits`, `bottom_bar_hits`) are recomputed every frame and must stay in sync between `draw.rs` and `handler.rs`.
- Layout is persisted as TOML on every structural change (new terminal, new group, rename, close, move).
- `TermStatus::Error` is in the protocol but never set — process exit tracking is not implemented yet.
- Grid always uses 2 columns regardless of terminal count.
- New terminals always start at 80×24 regardless of current window size.
