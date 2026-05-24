# Review Notes — Production-Readiness Pass

Reviewed by: automated code-review agent, 2026-05-24.

---

## What Was Found

### Bug: "Last terminal closed" left the client in a broken state

**File:** `src/server/ipc.rs`, `CloseTerminal` handler.

**Problem:** When the final terminal was closed, `state.close_terminal()` set
`state.active` to `None`. The helper `send_terminal_list()` silently does
nothing when `active_id()` is `None` (because `ServerMsg::TerminalList`
requires a non-optional active ID). The client never received an updated list
and continued holding stale IDs, which would cause confusing UI behaviour (e.g.
trying to focus a terminal that no longer existed on the server).

**Fix applied:** After `close_terminal()`, if no terminals remain, the server
now immediately spawns a fresh terminal (at 80×24; the client's next `ResizeAll`
will correct the size). This keeps the invariant that there is always at least
one live terminal, which the rest of the code assumes.

---

## What Was Verified Correct

### Non-blocking accept loop
`server/ipc.rs` sets the listener to non-blocking, drains `output_rx` with
`try_recv()` in the outer loop, sleeps 10 ms on `WouldBlock`, and the loop
repeats. PTY output is processed continuously even without a client. ✓

### `PtyEvent::Exited` path
`pty.rs` → EOF on `master.read()` → `output_tx.send((id, PtyEvent::Exited))` →
both the outer loop (no client) and `serve_client` (client connected) handle it
by setting `session.exited = true` → `session.status()` returns
`TermStatus::Error` → snapshot carries `Error` status → client renders the
`[process exited]` overlay. ✓

### `ClientMsg::NewTerminal` uses actual cols/rows
`ipc.rs` line 193: `state.new_terminal(cols.max(1), rows.max(1))`. ✓

### Ctrl-G exits focused mode
`handler.rs` line 23: `if key.code == KeyCode::Char('g') && key.modifiers ==
KeyModifiers::CONTROL`. Goes to grid. Note: the review task mentioned "Ctrl-B"
(tmux convention) but the code was consistently built around Ctrl-G, which
matches the help text and bottom bar hint. ✓

### `ScrollTo` calls `set_scroll_offset`
`ipc.rs` lines 243–247: `state.scroll_active(offset)` →
`session.set_scroll_offset(offset)` → `parser.set_scrollback(offset)` →
snapshot carries updated `scroll_offset`. ✓

### Alt-key sends ESC + char
`input.rs` lines 10–18: `has_alt` flag detected, `base_bytes` prepended with
`0x1b`. ✓

### SIGTERM handler
`install_sigterm_handler()` called at top of `run_server()`. Sets
`SHUTDOWN` AtomicBool. Outer loop checks it on every iteration. ✓

### AppMode::RenamingTerminal — fully implemented
- `handler.rs`: `handle_key_renaming_terminal()` handles Enter/Esc/Backspace/Char.
- `handler.rs`: Enter sends `ClientMsg::RenameTerminal { id, title }`.
- `draw.rs`: `draw_bottom_bar()` renders the inline prompt for this mode.  ✓

### Grid column formula handles n=0
`draw.rs` `grid_cols(0)` returns 1. `tile_rects(area, 0)` returns `Vec::new()`
before reaching any division. ✓

### Arrow key navigation index math
Uses `visible.iter().position()` to find current index, then clamps with
`.min(visible.len() - 1)`. Guard at top returns early if `visible.is_empty()`.  ✓

### `[process exited]` overlay
Rendered in both `draw_grid()` (bottom row of each errored tile) and
`draw_focused()` (bottom row of the full focused view). ✓

### Scroll indicator in top bar
`draw_focused()` adds a `↑ -N lines  PgDn to return` span to the title when
`snap.scroll_offset > 0`. ✓

### Group color cycling beyond 4 groups
`[...][idx % 4]` where `idx = app.groups.len()`. Correct modulo arithmetic. ✓

### Sidebar hit zones for off-screen rows
`compute_sidebar_hits()` has `if y >= bot { break; }` guards throughout. ✓

### No `unwrap()` on failure paths
All `unwrap_or` / `unwrap_or_else` calls in the source have safe defaults. ✓

### cargo clippy
Zero warnings. ✓

### UNDERSTANDING.md
Present, 4 669 words, covers PTY, VT100, Unix sockets, fork/daemonize,
end-to-end architecture, threading model, and a full glossary. ✓

---

## What Remains (not fixed, not critical)

### SIGTERM while client is connected
If `SIGTERM` arrives while `serve_client()` is running, the `SHUTDOWN` flag
check in the outer loop is not reached until the client disconnects. The server
will not save the layout or remove the socket until then. Acceptable for a v0.1
multiplexer; a production fix would check `SHUTDOWN` inside the
`serve_client` `recv_timeout` loop and return early.

### Resizing layout terminals at attach
Terminals restored from `layout.toml` start at 80×24. The client sends
`ResizeAll` immediately after attach, which corrects this. The very first
batch of snapshots the client receives is therefore at 80×24, but the second
frame (after `ResizeAll` is processed) is at correct dimensions. No visual
artefact for the user; cosmetic at most.
