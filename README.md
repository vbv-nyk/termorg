# termorg

A terminal cockpit for supervising many long-running sessions — agents, dev servers, watchers — tiled at a glance, grouped by project, navigated mouse-first.

## The problem

Agent-supervision sprawl is real: ten Claude Code sessions across four projects, plus a dev server and a watcher per project, plus a scratch shell. tmux handles the multiplexing but its model (sessions → windows → panes) doesn't match the mental model of "this set of terminals belongs to project A." termorg makes the project group a first-class concept and the cockpit-style overview the default view.

## Status

v0 — planning complete, beginning implementation. The visual design direction lives in `design/termorg-design.html`. The current milestone is described in [First milestone](#first-milestone--one-terminal-detachable) below.

## Architecture

termorg is a single binary that behaves as either a **server** or a **client** depending on invocation, in the tmux style.

The **server** is a daemon that owns every PTY, runs a `vt100::Parser` per terminal, holds workspace state (groups, terminal metadata), and exposes a unix-domain-socket IPC.

The **client** is the TUI the user looks at. It connects to the server, receives a screen-state snapshot for each terminal it cares about, then receives diffs as PTY output changes. The client never touches PTYs directly.

This split is what enables three things at once: closing your terminal window doesn't kill your agents; you can SSH in and `termorg attach` to resume the same workspace; and multiple clients could in principle attach to the same server (single-attach is fine for v1).

### Decisions worth not relitigating

- **Server owns state.** All PTY ownership and vt100 parsing happens server-side. Clients are renderers + input forwarders.
- **Single binary.** `termorg` self-execs to spawn the server on first run; subsequent invocations attach.
- **Prefix key (default `Ctrl-Space`).** Inner programs (vim, htop, agents) eat `Esc` and most modifiers; the prefix is the one key reserved as the user's escape hatch out of focused mode. Configurable.
- **Grid mode vs focused mode.** Grid is navigation (cockpit keys are direct, mouse-first). Focused is passthrough (every key/mouse event flows to the PTY). The split is deliberate and not collapsible into a single mode.
- **Groups are exclusive.** A terminal belongs to exactly one group. No nesting.
- **No tile-splits in v0.** A terminal is one PTY, one tile. Splits double the complexity of resize and focus logic and are out of scope.

## Stack

| Concern | Choice |
|---|---|
| Language | Rust 2021 |
| TUI | `ratatui` + `crossterm` |
| PTY | `portable-pty` |
| Terminal emulation | `vt100` |
| IPC wire format | length-prefixed `bincode` over a unix socket |
| Daemonization | `nix` (fork + setsid) |
| Config | `toml` + `serde` |
| Errors | `anyhow` for the binary, `thiserror` if we ever split crates |

No `tokio` for v0 — `portable-pty` doesn't have first-class async readers, so threads + `mpsc` is simpler and scales fine to tens of terminals. Reconsider if the design ever calls for hundreds.

## First milestone — "one terminal, detachable"

Build the smallest thing that proves the server-client architecture works end-to-end. Everything later builds on this.

**Acceptance test:**

1. Run `termorg`. A server spawns (daemonized); a client attaches; you see `$SHELL` running in a full-screen ratatui viewport.
2. Type `sleep 60; echo done` and hit Enter.
3. Close the terminal window (don't quit the shell).
4. Open a new terminal. Run `termorg attach`. You see the same prompt. About a minute later, `done` appears.
5. Run `termorg kill`. The server exits cleanly.

If you can detach a `vim` session and reattach with the cursor in the same place, the milestone is done.

**In scope for this milestone (everything else is later):**

- Exactly one PTY. One client at a time.
- Full-screen rendering of that one terminal. No tiles, no sidebar, no groups, no design polish.
- Key passthrough: printable chars, arrows (honoring app-mode reported by `vt100::Parser`), common modifiers, Ctrl-Space to exit.
- No mouse passthrough yet.
- No config file yet. Defaults hardcoded.
- Hardcoded socket and PID file under `~/.local/state/termorg/`.

## Roadmap

- **Phase 2** — multi-terminal protocol and data model. Server holds `HashMap<TermId, Terminal>`. Client still renders one at a time but can switch.
- **Phase 3** — UI from the design: grid layout, sidebar, status bar. Mouse for selection. The cartographer's-console direction in `design/termorg-design.html` becomes real.
- **Phase 4** — groups: create, rename, delete, move terminals between. Sidebar with collapsible groups.
- **Phase 5** — polish: config file with custom keybindings, persistence of workspace metadata, mouse passthrough in focused mode (mode-aware via vt100), PTY resize on layout change.
- **Phase 6** — differentiators: cross-terminal search (`/`), "wants attention" heuristic, project auto-detection from cwd.

## Non-goals

- Not a general tmux replacement. Specifically a cockpit for many concurrent agents and dev sessions.
- No sixel or kitty graphics protocols in v0.
- No surviving server crash or machine reboot. (Children die with the server, same as tmux.)
- No remote orchestration beyond "SSH into the host running the server."
- Not Windows-first. `portable-pty` supports ConPTY, but unix-domain sockets don't exist on Windows; named pipes would be needed for cross-platform parity.

## Open questions

- Default scrollback depth per terminal (lean: 1000 lines, configurable).
- Default new-terminal command (lean: `$SHELL`).
- Should closing the last terminal also kill the server, or should the server linger? (lean: linger; explicit `termorg kill` to stop.)
- Where to surface "wants attention" — sidebar glyph only, or a status-bar count too?

## Proposed project layout

```
termorg/
├── Cargo.toml
├── README.md
├── LICENSE
├── design/
│   └── termorg-design.html          # visual direction (phase 3 onward)
└── src/
    ├── main.rs                       # CLI dispatch: termorg / attach / kill
    ├── daemon.rs                     # fork + setsid + pidfile
    ├── proto.rs                      # IPC types shared between server & client
    ├── server/
    │   ├── mod.rs
    │   ├── ipc.rs                    # unix socket listener
    │   ├── pty.rs                    # PTY spawn + reader thread
    │   └── session.rs                # Terminal state + vt100 parser
    └── client/
        ├── mod.rs
        ├── ui.rs                     # ratatui rendering
        └── input.rs                  # crossterm key → PTY bytes
```

## Notes for future contributors

This README is the source of truth for project decisions. The design HTML in `design/` shows the visual direction for phase 3 onward — it is not the spec for the current milestone. Build the foundation (phase 1) plain and full-screen before reaching for any of the cockpit UI.