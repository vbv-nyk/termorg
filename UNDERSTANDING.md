# Understanding termorg — A Systems Programming Primer

This document is written for you: someone who can write application-level code
(functions, classes, HTTP requests, databases) but has never touched the layer
below that — operating system primitives, file descriptors, signals, and the
like.  Every concept is explained from first principles before connecting it to
the termorg source code.

---

## Table of Contents

1. [What Is a Terminal?](#1-what-is-a-terminal)
2. [What Is a PTY?](#2-what-is-a-pty-pseudo-terminal)
3. [VT100 and ANSI Escape Codes](#3-vt100-and-ansi-escape-codes)
4. [Unix Domain Sockets](#4-unix-domain-sockets)
5. [Daemons and Background Processes](#5-daemons-and-background-processes)
6. [The termorg Architecture End-to-End](#6-the-termorg-architecture-end-to-end)
7. [The Rendering Pipeline](#7-the-rendering-pipeline)
8. [State and Persistence](#8-state-and-persistence)
9. [The Threading Model](#9-the-threading-model)
10. [Glossary](#10-glossary)

---

## 1. What Is a Terminal?

### The Physical Machine

In the 1960s and 70s, computers were room-sized machines shared by many people.
Each person sat at a **terminal** — a physical device with a keyboard and a
screen (or a teleprinter before screens were cheap).  The terminal was not a
computer; it was an input/output device connected to the main computer by a
serial wire.

```
  ┌──────────────┐         serial cable         ┌────────────────┐
  │  Your        │ ──────────────────────────── │  Mainframe     │
  │  Terminal    │   bytes go both ways          │  Computer      │
  │  (keyboard + │                               │                │
  │   screen)    │                               │                │
  └──────────────┘                               └────────────────┘
```

The terminal's job:
- **Input**: when you press a key, encode it as a byte (or sequence of bytes)
  and send it down the wire.
- **Output**: receive bytes from the computer, display them on screen.

### Software Terminal Emulators

Today's computers are powerful enough to run many programs simultaneously on
one machine.  Instead of physical terminals we have **terminal emulators** —
software programs that *pretend* to be the old physical hardware.

Examples: GNOME Terminal, iTerm2, Alacritty, Kitty, Windows Terminal.

A terminal emulator:
1. Draws a window with a grid of characters.
2. Forwards your keypresses as bytes to whatever program is running inside it.
3. Reads bytes that the program outputs and updates the grid accordingly.

The program running inside — usually a shell like `bash` or `zsh` — has no
idea it is talking to software rather than real hardware.  It still sends the
same kinds of bytes it always would, and the emulator interprets them.

### How termorg Fits In

termorg is a **terminal multiplexer** — software that lets multiple terminal
sessions share a single window, keep running when you disconnect, and be
reorganised into groups.  It is in the same family as `tmux` and `screen`.

termorg uses your existing terminal emulator (whatever you started it from) as
the "glass" — ratatui draws the UI there.  Internally it runs its own terminal
emulators (vt100 parsers) for each managed terminal.

---

## 2. What Is a PTY (Pseudo-Terminal)?

### The Problem

Imagine you write a program that reads text from the keyboard and prints
colored output.  How does it know to emit color?  Most programs check whether
stdout is a *terminal* — if yes, use ANSI color codes; if no (e.g. when piped
to a file), emit plain text.

How does a program check? It calls the C function `isatty(fd)`, which asks the
kernel "is file descriptor `fd` connected to a terminal device?"

Now suppose termorg wants to run `bash` as a child process and capture all its
output.  If termorg just used a regular pipe (`pipe()`), bash would see
`isatty() == false`, disable colors, disable line editing (readline), and
behave differently from how it does in a real terminal.  This is bad.

### The Solution: PTY

A **pseudo-terminal (PTY)** is a kernel object that looks like a real terminal
to the program using it, but is actually controlled by another process.

A PTY has two sides:
- **Master side**: your program (termorg's server) holds this.  You read output
  from it and write input to it.
- **Slave side**: the shell gets this.  When the shell calls `isatty()`, the
  kernel returns `true`.  When the shell writes "hello\n", the bytes appear on
  the master side.

```
  ┌─────────────────────────────────────────────────────────┐
  │                    Linux Kernel                          │
  │                                                         │
  │   ┌──────────────────────────────────────────────────┐  │
  │   │              PTY (pseudo-terminal)                │  │
  │   │                                                   │  │
  │   │  master fd ◄──────────────────── slave fd        │  │
  │   │  (termorg reads/writes here)      (bash uses this)│  │
  │   └──────────────────────────────────────────────────┘  │
  └─────────────────────────────────────────────────────────┘
         ▲  bytes flowing both ways  ▲
         │                           │
  ┌──────┴────┐               ┌──────┴────┐
  │  termorg  │               │   bash    │
  │  (server) │               │  (child)  │
  └───────────┘               └───────────┘
```

### What Happens When You Type `ls --color`

Step by step, at the byte level:

1. You press `l`.  Your physical keyboard sends a scan code.  Your OS
   translates it to ASCII byte `0x6c`.  termorg's client (which is in raw
   mode) reads that byte with crossterm's event system.

2. termorg's client sends `ClientMsg::Input(vec![0x6c])` to the server over the
   Unix socket.

3. The server receives it and calls `pty.write_input(&[0x6c])` — this writes
   the byte to the master side of the PTY.

4. The kernel routes it through the PTY to the slave side.  bash (which has the
   slave side as its stdin) reads `l`.

5. bash echoes it back: it writes `l` to its stdout (the slave side).  The
   kernel puts this on the master side.

6. termorg's reader thread (in `pty.rs`) reads the echo from the master side
   and sends it to the session's vt100 parser.

7. You finish typing `ls --color` and press Enter.  Enter is `\r` (0x0d).

8. bash sees the complete command, forks a child, runs `ls --color`.  `ls`
   calls `isatty()` on stdout, gets `true` (because it has the slave side!),
   and emits ANSI color codes mixed with filenames.

9. `ls` output flows: slave → kernel → master → reader thread → vt100 parser.

10. The vt100 parser interprets the ANSI codes and updates its internal grid of
    colored cells.

11. On the next draw cycle, termorg's client renders that grid as ratatui Spans
    with colors — so you see colored output.

### In the termorg Source Code

`src/server/pty.rs` — the `Pty::spawn()` function:
- Calls `native_pty_system().openpty()` to create master+slave.
- Spawns `$SHELL` with the slave side as its controlling terminal.
- Grabs the master side for reading (in a background thread) and writing.

---

## 3. VT100 and ANSI Escape Codes

### The Problem: Terminals Are Dumb Devices

The original terminals could only do two things: show characters and move the
cursor forward.  They couldn't move the cursor backward, clear a line, or show
colors.

As terminals got more capable, manufacturers added special byte sequences —
**escape sequences** — to control these features.  DEC (Digital Equipment
Corporation) standardized many of them in their VT100 terminal in 1978.  The
ANSI standards committee later formalized and extended them.

### The ESC Byte

Every ANSI escape sequence starts with byte `0x1b` — the ASCII *Escape*
character, commonly written as `ESC` or `\x1b`.

The most common sequence type is **CSI** (Control Sequence Introducer):
`ESC [` followed by parameters and a letter.

Examples:

| Sequence         | Meaning                        |
|------------------|--------------------------------|
| `\x1b[31m`       | Set foreground color to red    |
| `\x1b[0m`        | Reset all attributes           |
| `\x1b[1m`        | Bold                           |
| `\x1b[2J`        | Clear entire screen            |
| `\x1b[H`         | Move cursor to top-left (home) |
| `\x1b[3;10H`     | Move cursor to row 3, column 10|
| `\x1b[A`         | Move cursor up one line        |

So when `ls --color` outputs a red filename, the bytes on the wire look like:

```
\x1b[31m  f  i  l  e  n  a  m  e  \x1b[0m
```

The terminal emulator sees `\x1b[31m`, switches to red, draws "filename" in
red, then sees `\x1b[0m` and resets.

### The vt100 Crate

Instead of writing our own ANSI parser (hundreds of edge cases), termorg uses
the `vt100` crate.

`vt100::Parser` maintains an internal **grid of cells** — a 2D array, one
`Cell` per character position.  Each cell stores:
- The character to display
- Foreground color
- Background color
- Bold, italic, underline flags

When bytes flow in from the PTY, you call `parser.process(bytes)`.  The parser
walks through the bytes, and whenever it sees an escape sequence, it updates the
grid.  When it sees a plain character, it puts it in the current cursor cell and
advances the cursor.

After processing, you read the grid via `parser.screen()`:
```rust
let screen = parser.screen();
let cell = screen.cell(row, col);    // One cell
let (cursor_row, cursor_col) = screen.cursor_position();
```

### Scrollback

vt100 also supports a scrollback buffer.  When the terminal is full and a new
line comes in, the top row doesn't disappear — it gets pushed into a scrollback
deque.  termorg creates parsers with 1000 scrollback lines:

```rust
Parser::new(rows, cols, 1000 /* scrollback */)
```

You can "scroll" the view by calling `parser.set_scrollback(n)` — this offsets
the viewport `n` lines upward into history without discarding the live screen.

### In the termorg Source Code

`src/server/session.rs` — the `Session` struct wraps a `vt100::Parser`.
`Session::process()` feeds bytes in.  `Session::snapshot()` reads the current
grid into a `ScreenSnapshot` (the struct we send to the client over the socket).

---

## 4. Unix Domain Sockets

### What Is a Socket?

A socket is a communication endpoint — like a telephone.  Two programs can
each hold one end of a socket pair and exchange data.

**TCP sockets** (what most network code uses) go through the kernel's networking
stack and can cross machine boundaries.  They need an IP address and port.

**Unix domain sockets** stay entirely within a single machine.  Instead of an
IP address, they have a *file path*.  They are:
- Faster (no network overhead)
- More secure (file permissions control access)
- Simpler to use for local IPC

termorg uses a Unix domain socket at `~/.local/state/termorg/server.sock`.

### How They Work

```
  Server (daemon):                    Client:
  ┌──────────────┐                   ┌──────────────┐
  │ bind(path)   │                   │              │
  │ listen()     │                   │              │
  │ accept() ────┼───────────────────┼► connect()   │
  │              │  socket connected │              │
  │ read()/write │ ◄────────────────►│ read()/write │
  └──────────────┘                   └──────────────┘
```

1. The server calls `bind()` to claim the path, then `listen()` to say "accept
   connections here".
2. Clients call `connect(path)` to connect.
3. After `accept()` returns a new stream, both sides can `read()` and `write()`.

### Length-Prefix Framing

Raw sockets are **streams** — bytes arrive in arbitrary-sized chunks.  There is
no built-in concept of "messages".  If you send a 100-byte message and the
receiver calls `read()`, it might get 40 bytes, then 60 bytes in separate calls.

To send *messages*, we need **framing**.  termorg uses **length-prefix framing**:
before every message, send a 4-byte big-endian integer saying how many bytes
follow.

```
Wire format:
  ┌──────────────────┬──────────────────────────┐
  │  4 bytes (u32)   │  N bytes (the message)   │
  │  length = N      │  bincode-serialized data  │
  └──────────────────┴──────────────────────────┘
```

The receiver:
1. Reads exactly 4 bytes → parse the length N.
2. Reads exactly N bytes → the message.

This is implemented in `src/server/ipc.rs` as `send_msg()` and `recv_msg()`.

### Bincode Serialization

Rust structs and enums can't be sent over a socket directly (they live in RAM
as bits arranged by the Rust compiler in an unspecified format).  We need to
**serialize** them into a byte sequence that can be sent, then **deserialize**
on the other side back into a struct.

JSON is one format you've probably used.  termorg uses **bincode**, which is
more compact and faster but not human-readable.

```rust
// Sending:
let msg = ClientMsg::Input(vec![0x41]);  // 'A'
let bytes = bincode::serialize(&msg)?;   // → compact binary
stream.write_all(&bytes)?;

// Receiving:
let msg: ClientMsg = bincode::deserialize(&bytes)?;
```

The structs that can be serialized are marked with `#[derive(Serialize, Deserialize)]`
in `src/proto.rs`.

---

## 5. Daemons and Background Processes

### What Is a Process?

When you run a program, the OS creates a **process** — an isolated execution
context with its own memory, file descriptors, and CPU time.  Every process has
a numeric ID called a **PID** (process ID).

Processes form a tree.  When you run `bash`, bash is a child of your terminal.
When bash runs `ls`, ls is a child of bash.

### The `fork()` System Call

`fork()` is how Unix creates new processes.  It duplicates the current process
— creating an identical copy with a new PID.  The original is the **parent**;
the copy is the **child**.

After `fork()` returns, both parent and child continue running the same code,
but `fork()` returns *different values* to each:
- Parent receives the child's PID (a positive number).
- Child receives 0.

```rust
match unsafe { fork() } {
    ForkResult::Parent { child } => {
        // We are the parent (original process)
        println!("I spawned child PID {}", child);
    }
    ForkResult::Child => {
        // We are the child (copy)
        println!("I am the new child");
    }
}
```

### Becoming a Daemon

A **daemon** is a background process that runs independently of any terminal.
The traditional way to create one:

1. **Fork** — parent exits, child keeps running.  Since the parent is gone, the
   shell thinks the command finished and returns the prompt to the user.

2. **setsid()** — the child calls `setsid()` to become the **session leader**
   of a new session.  This disconnects it from the controlling terminal (the
   terminal window you started it from).  If that terminal closes, the daemon
   won't receive the `SIGHUP` signal that would normally kill it.

3. **Redirect stdio** — stdin and stdout are connected to `/dev/null` (a file
   that discards everything written to it and returns EOF on read).  The daemon
   has no terminal, so these would be dangling handles otherwise.

4. **Redirect stderr to a log file** — instead of `/dev/null`, we redirect
   stderr to `~/.local/state/termorg/server.log`.  This captures all
   `eprintln!()` calls and Rust panic messages for debugging.

### In the termorg Source Code

`src/daemon.rs` — the `daemonize()` function does all of this.

```
user types: termorg attach
                │
                ▼
         main() calls attach()
                │
                ▼
         daemon::daemonize()
              fork()
              │
    ┌─────────┴─────────┐
    │ Parent             │ Child
    │ (DaemonRole::      │ (DaemonRole::
    │  Client)           │  Server)
    │                    │
    │ waits for socket   │ setsid()
    │ to appear          │ redirect stdio
    │                    │ write pidfile
    │                    │ server::ipc::run_server()
    ▼                    │ (blocks forever)
    client::ui::run_client()
```

### The Pidfile Pattern

How does a new `termorg attach` know if a server is already running?  It reads
the **pidfile** — a file at `~/.local/state/termorg/server.pid` containing
just the PID number.

If the file exists and `/proc/{pid}` exists (on Linux, each running process has
a directory in `/proc`), the server is alive.  If not, start a new one.

---

## 6. The termorg Architecture End-to-End

Let's trace exactly what happens from the moment you type `termorg` in your
shell to when you see a rendered terminal grid.

### Step 1: Entry Point

```
termorg  (no args) → attach()
```

`src/main.rs::main()` reads command-line arguments.  No arguments (or `attach`)
→ calls `attach()`.

### Step 2: Fork Decision

`attach()` calls `daemon::server_is_running()`.  If no server is alive:
- Calls `daemon::daemonize()`.
- `fork()` splits the process.
- **Parent** (client role): waits up to 2 seconds for the socket file to appear.
- **Child** (server role): calls `server::ipc::run_server()`.

If a server is already running: skip straight to the client.

### Step 3: Daemon Child Starts

In `run_server()`:
1. Install a SIGTERM handler (sets an atomic flag when the signal arrives).
2. Create an mpsc channel: `(output_tx, output_rx)`.  This is the pipe through
   which PTY reader threads send output bytes to the server loop.
3. Create `ServerState` — the central data structure owning all PTYs, sessions,
   and groups.
4. Load `~/.local/state/termorg/layout.toml` — if it exists, recreate the
   previous arrangement of groups and terminals.  Otherwise spawn one fresh
   terminal.
5. Bind the Unix domain socket at `server.sock`.
6. Set the socket to non-blocking mode.
7. Enter the main server loop.

### Step 4: Server Main Loop (Waiting for Clients)

```
while true:
  check SHUTDOWN flag → if set, save layout, remove socket, exit
  drain output_rx (PTY output → vt100 parsers)
  try accept() a new client connection
  if WouldBlock: sleep 10ms, continue
  if success: serve_client(stream)
```

The key insight: even while no client is connected, the server keeps draining
PTY output.  So the vt100 parsers stay current — when you reconnect, you get
fresh snapshots, not stale ones.

### Step 5: Spawning a Terminal

When a terminal is created (either from layout or `ClientMsg::NewTerminal`):

`ServerState::new_terminal(cols, rows)`:
1. `Pty::spawn(cols, rows, id, output_tx.clone())`:
   - Opens a PTY pair (master + slave).
   - Spawns `$SHELL` with slave as its stdin/stdout/stderr.
   - Spawns a **reader thread**: loops on `master.read()`, sends
     `(id, PtyEvent::Output(bytes))` on `output_tx`.
   - On EOF (shell exit), sends `(id, PtyEvent::Exited)`.
2. Creates a `Session` with a fresh `vt100::Parser`.

### Step 6: Client Connects

The parent process (client) calls `client::ui::run_client()`:
1. Opens a `UnixStream` to `server.sock`.
2. Sends `ClientMsg::Attach`.
3. Asks for the terminal size with `terminal::size()`, computes what size the
   grid tiles should be, sends `ClientMsg::ResizeAll { cols, rows }`.
4. Enters alternate screen mode (hides the shell's own output area).
5. Enables raw mode (intercepts all keypresses before the shell sees them).
6. Creates a ratatui `Terminal` backed by crossterm.
7. Spawns a **server reader thread**: loops on `recv_msg()` from the socket,
   forwards `ServerMsg` values to an mpsc channel.
8. Enters the **event loop**.

### Step 7: Initial Handshake (Server Side)

When the server receives `ClientMsg::Attach`:
- Sends `ServerMsg::TerminalList { active, ids }`.
- Sends `ServerMsg::GroupList(groups)`.
- Sends a `ServerMsg::Snapshot(snap)` for **every** terminal — so the client
  can draw the full grid from the very first frame.

### Step 8: Event Loop (~60 fps)

Each iteration:
1. Drain the server message channel → update `App` state (snapshots, lists).
2. Compute mouse hit areas (tile positions, sidebar rows, bottom bar buttons).
3. Draw the frame with ratatui.
4. Poll for input events with a 16ms timeout (≈60fps).
5. If a key event arrives: `handle_key()` or `handle_mouse()`.

### Step 9: A Keypress → PTY Write

You press `a` while in focused mode:
1. crossterm generates `KeyEvent { code: Char('a'), modifiers: NONE }`.
2. `handle_key()` → `AppMode::Focused` branch → calls `key_to_action(key)`.
3. `key_to_action` returns `Some(vec![0x61])` (ASCII 'a').
4. `send_msg(stream, &ClientMsg::Input(vec![0x61]))` — serialized to bytes,
   length-prefixed, written to the socket.
5. Server receives it, calls `pty.write_input(&[0x61])`.
6. The byte flows through the PTY to bash's stdin.

### Step 10: Output → Snapshot → Re-render

1. Bash echoes `a` and maybe produces more output.
2. PTY reader thread reads bytes from master, sends them to `output_rx`.
3. Server's `serve_client()` loop drains `output_rx`:
   - Calls `session.process(bytes)` → vt100 parser updates its grid.
   - Calls `state.snapshot(id)` → reads the grid into a `ScreenSnapshot`.
   - Sends `ServerMsg::Snapshot(snap)` over the socket.
4. Client's server reader thread receives the snapshot, puts it in the channel.
5. Next event loop iteration: drain channel → `app.snapshots.insert(id, snap)`.
6. Draw frame: `draw_focused()` reads `app.snapshots[active]` → renders cells
   as ratatui Spans with colors.

---

## 7. The Rendering Pipeline

### ratatui

**ratatui** is a library for building text-based UIs (TUIs) in Rust.  It works
on the concept of *immediate mode* rendering: every frame you describe the
entire UI from scratch, ratatui diffs the new description against the previous
frame, and only sends the changed bytes to the terminal.

### The Alternate Screen

Most terminal UIs use the **alternate screen** — a separate framebuffer in your
terminal emulator.  When you enter it:
- Your existing shell output scrolls behind the scenes (preserved).
- The TUI draws into the blank alternate screen.

When you exit (Ctrl-G or q), the alternate screen is discarded and your
original shell output reappears exactly as it was.  crossterm does this with:

```rust
execute!(stdout, EnterAlternateScreen)   // on entry
execute!(stdout, LeaveAlternateScreen)   // on exit
```

### Raw Mode

Normally, when you type in a terminal, the *line discipline* (a kernel layer)
buffers characters until Enter, handles backspace, and only then sends the line
to the program.  This is **cooked mode**.

In **raw mode**, every keypress is sent to your program immediately, without
buffering.  This is necessary for a TUI — you want to respond to arrow keys,
Ctrl combinations, and special keys instantly.

```rust
terminal::enable_raw_mode()   // enter raw mode
terminal::disable_raw_mode()  // restore cooked mode (always do this on exit!)
```

### A Frame

When ratatui renders a frame, it:
1. Calls your `draw` closure with a `Frame`.
2. Your code calls `f.render_widget(widget, area)` for each thing to display.
3. ratatui computes the minimal set of terminal escape codes to update the
   screen from the previous frame.
4. Sends those codes to stdout.

### The termorg Rendering Structure

```
┌─────────────────────────────────────────────────────┐
│  top bar (1 row): termorg · group · terminal name   │
├────────────┬────────────────────────────────────────┤
│  sidebar   │                                        │
│  (24 cols) │        main area                       │
│            │  (grid of tiles, or focused terminal)  │
│  GROUPS    │                                        │
│  ▾ dev  ●  │                                        │
│    term1 · │                                        │
│  ▾ ops  ✱  │                                        │
│    term2 ● │                                        │
│            │                                        │
├────────────┴────────────────────────────────────────┤
│  bottom bar (1 row): action buttons                 │
└─────────────────────────────────────────────────────┘
```

`draw_frame()` in `src/client/draw.rs` orchestrates this:
- `draw_top_bar()` — breadcrumb navigation
- `draw_sidebar()` — group list with status indicators
- `draw_grid()` or `draw_focused()` — the main content area
- `draw_bottom_bar()` — action buttons or rename prompt

### Spans, Lines, Text

ratatui text is built from nested containers:

- **`Span`**: a string fragment with a single style (fg color, bg color,
  modifiers like bold/italic).
- **`Line`**: a horizontal sequence of `Span`s — one row of text.
- **`Text`**: a vertical sequence of `Line`s — multiple rows.
- **`Paragraph`**: a widget that renders a `Text` into a `Rect`.

```rust
let line = Line::from(vec![
    Span::styled("hello ", Style::default().fg(Color::Green)),
    Span::styled("world", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
]);
```

### Snapshot → Text

`snapshot_to_text()` in `draw.rs` converts a `ScreenSnapshot` into a ratatui
`Text`:
- For each row of cells → one `Line`.
- For each cell → one `Span` with the cell's character and colors.

Colors are translated from termorg's `proto::Color` enum (which mirrors what
vt100 exposes: Default, Indexed 0-255, or RGB) to ratatui's `Color` enum.

---

## 8. State and Persistence

### What Gets Saved

When the layout changes (new group, new terminal, rename, move), the server
immediately calls `persist_layout()`, which serializes the current state to
`~/.local/state/termorg/layout.toml`.

The TOML file stores:
```toml
[[group]]
name = "dev"
color = "Sage"

[[group]]
name = "ops"
color = "Ochre"

[[terminal]]
title = "claude"
group = "dev"
group_index = 0   # authoritative: position of the group in the list above

[[terminal]]
title = ""
group = "ops"
group_index = 1
```

### What Is NOT Saved

- The actual terminal content (screen cells, scrollback history).
- The shell's working directory.
- What command was running.
- Environment variables.

On server restart, each terminal is recreated as a *fresh* shell session.  The
layout (groups and terminal titles) is restored, but you're at a blank prompt,
not where you left off.

### Why `group_index` Matters

Groups are matched by their *position* in the `[[group]]` array, not by name.
If two groups both happen to be named "dev" (which is possible), using name
lookup would always assign terminals to the first one.  The `group_index` field
eliminates this ambiguity.

Older layout files without `group_index` fall back to name-based matching
gracefully.

---

## 9. The Threading Model

Understanding which thread owns what prevents a class of bugs (data races,
deadlocks).  Here is every thread termorg creates:

```
                           PROCESS
  ┌────────────────────────────────────────────────────────────────┐
  │                                                                │
  │  Main thread (server)                                          │
  │  ├── run_server() main loop                                    │
  │  ├── owns: ServerState, listener socket, output_rx             │
  │  └── calls serve_client() when a client connects               │
  │                                                                │
  │  PTY reader thread (one per terminal)    ← pty.rs              │
  │  ├── loops: master.read() → output_tx.send()                   │
  │  └── on EOF: sends PtyEvent::Exited                            │
  │                                                                │
  │  Client reader thread (one per connection)   ← ipc.rs          │
  │  ├── loops: recv_msg(socket) → client_tx.send()                │
  │  └── lives only while a client is connected                    │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘

                           CLIENT PROCESS
  ┌────────────────────────────────────────────────────────────────┐
  │                                                                │
  │  Main thread (client)                                          │
  │  ├── event_loop(): draw → poll events → handle input           │
  │  └── owns: App, tui terminal, stream (write half)              │
  │                                                                │
  │  Server reader thread (one)    ← ui.rs                         │
  │  ├── loops: recv_msg(socket) → server_tx.send()                │
  │  └── feeds ServerMsg into the event loop's channel             │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘
```

### The mpsc Channels

`mpsc` = **m**ulti-**p**roducer, **s**ingle-**c**onsumer channel.  Like a
message queue: many senders, one receiver.

**`output_tx` / `output_rx`** (in server):
- Multiple senders: one per PTY reader thread (each has a clone of `output_tx`).
- One receiver: the server main thread (`output_rx`).
- Carries: `(TermId, PtyEvent)` — which terminal produced the event.

**`client_tx` / `client_rx`** (per connection):
- One sender: the client reader thread.
- One receiver: the main server loop.
- Carries: `ClientMsg` — commands from the connected client.

**`server_tx` / `server_rx`** (in client):
- One sender: the server reader thread.
- One receiver: the event loop on the main thread.
- Carries: `ServerMsg` — updates from the server.

### Why Threads at All?

- The PTY reader thread must block on `read()` waiting for shell output.  If
  the main thread blocked there, it couldn't accept new connections.
- The client reader thread must block on `recv_msg()` waiting for commands.  If
  the main thread blocked there, it couldn't process PTY output or time out.
- Channels let us use blocking reads in background threads while the main thread
  uses non-blocking `try_recv()` in a polling loop.

---

## 10. Glossary

**Alternate screen**: A second framebuffer in a terminal emulator.  TUIs use it
so they don't corrupt the user's shell output.  Entered with an escape sequence,
exited by another.

**ANSI escape codes**: Byte sequences starting with `\x1b[` that control
terminal formatting: colors, cursor movement, clearing screen.  Standardized by
ANSI X3.64.

**bincode**: A binary serialization format for Rust.  Converts structs/enums to
compact bytes.  Not human-readable; faster and smaller than JSON.

**Cell**: One character position in a terminal grid.  Stores the character,
foreground color, background color, and style flags (bold, italic, underline).

**Cooked mode / line discipline**: The kernel's default terminal input mode.
Buffers characters until Enter; handles backspace automatically.  The opposite
of raw mode.

**crossterm**: A Rust crate that handles terminal raw mode, alternate screen,
and reading input events (keys, mouse) in a cross-platform way.

**Daemon**: A background process with no controlling terminal.  Stays alive
after the user's shell session ends.

**ESC (escape byte)**: ASCII byte `0x1b`.  Every ANSI/VT100 escape sequence
starts with it.

**File descriptor (fd)**: An integer handle to an open file, socket, PTY side,
or pipe.  When a process opens a file, the kernel returns an fd.  `0` = stdin,
`1` = stdout, `2` = stderr.

**fork()**: A system call that duplicates the current process.  Both parent and
child continue running.  Used to create the daemon child.

**mpsc channel**: Rust's `std::sync::mpsc` — a thread-safe queue where multiple
producers can send values that one consumer drains.

**PID**: Process ID.  A unique integer the kernel assigns to each running process.

**Pidfile**: A small file containing a PID, used to check if a background
service is already running.

**PTY (pseudo-terminal)**: A kernel object that simulates a real terminal.  Has
a master side (your program) and a slave side (the shell).  Programs using the
slave see `isatty() == true`.

**ratatui**: A Rust library for building terminal user interfaces (TUIs).  Works
by describing the UI each frame and sending only diffs to the terminal.

**Raw mode**: Terminal input mode where every keypress is sent to the program
immediately, without buffering or processing.  Required for TUIs.

**Scrollback**: Lines of output that have scrolled above the visible terminal
screen.  Stored in a ring buffer; accessible by scrolling up.

**setsid()**: A system call that makes the calling process the leader of a new
session, detaching it from any controlling terminal.

**Span**: In ratatui, a string with a single uniform style (color, bold, etc.).

**Unix domain socket**: A socket that lives on the filesystem (identified by a
file path) and connects two processes on the same machine.  Faster and more
private than TCP sockets.

**VT100**: A physical terminal made by DEC in 1978.  Its escape sequence
vocabulary became the de facto standard; what we now call "ANSI escape codes"
are largely the VT100 sequence set plus extensions.

**vt100 (crate)**: A Rust library that parses ANSI/VT100 byte streams and
maintains an in-memory grid of cells representing the current terminal screen.

**Widget**: In ratatui, an object that knows how to draw itself into a `Rect`
(a rectangular region of the screen).  Examples: `Paragraph`, `Block`, `List`.

---

*Document written to reflect the state of the termorg codebase after the
production-readiness pass of 2026-05-24.*
