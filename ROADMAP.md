# rusttyterm Roadmap

Release plan derived from [`docs/SPEC.md`](docs/SPEC.md) (sections 24–27).
Versions ship in order; features move down only by explicit decision.

The owner-approved next-work sequence is recorded in
[Post-v0.5 — Daily-driver excellence and project workflows](#post-v05--daily-driver-excellence-and-project-workflows)
and [master backlog #45](https://github.com/dhruvparpia/rusttyterm/issues/45).

## v0.1 — MVP

A native Windows terminal that developers use daily.

| Category | Feature | Priority |
|---|---|---|
| Core | Tabs: add, close, switch, reorder, `+` button, tab close destroys shell | P0 |
| Core | Tab bar always visible (1 tab shown with `+`; hide-tab-bar setting exists) | P0 |
| Core | ConPTY shell backend; default shell = Git Bash, configurable per profile | P0 |
| Core | DirectWrite + Direct2D renderer, GPU-accelerated | P0 |
| Core | VT parser: SGR 16/256/true color, common CSI/OSC, UTF-8 | P0 |
| Core | TOML config + profiles; default config location | P0 |
| Core | Ghostty-compatible keybindings (tab new/close/next/prev, copy/paste, scroll, fullscreen, quit) | P0 |
| Core | Scrollable output + scrollback; visual bell | P0 |
| Core | Theme presets (Predawn and friends); theme in config | P0 |
| Core | Windows Jump List (native taskbar API) | P1 |
| Perf | 60 fps under load (glamor stress), 10k-line burst in < 1 s | P0 |
| Perf | Zero allocations in render hot path | P0 |
| Perf | Fast startup, low idle RAM vs Windows Terminal | P1 |
| Ship | Portable ZIP packaging; MIT LICENSE in package | P0 |
| Ship | No telemetry, no phone-home | P0 |

**Explicitly NOT in MVP:** splits, shell integration, full screen-reader
accessibility, IME, session restore, quick terminal, theme browser, graphics
protocols, plugin system, Electron/web UI of any kind.

## v0.2 — Ghostty feature parity, part 1

| Feature | Notes |
|---|---|
| Splits | Pane tree (spec §9/§10); panes are not tabs; close pane = destroy process |
| Visual effects | Blending, transparency, blur (DWM), animation — Direct2D side |
| Window controls | Resize, fullscreen, maximize, snap layouts (spec §8) |
| Themes | Preset browser, live theme switching |
| Session restore | Reopen tabs/panes on relaunch |
| Desktop notifications | Windows toast |
| Quick terminal | Hotkey summon from screen edge |
| Windows Jump List | Full implementation (MVP carries the P1 item) |

## v0.3 — Linux

| Feature | Notes |
|---|---|
| Wayland backend | **TBC — confirm compositor target list before starting** |
| X11 backend | Baseline support for all X11 compositors (spec §25) |
| Wayland protocol parity | `fractional-scale-v1` required; viewport, IME, clipboard (spec §26) |
| Linux packaging | deb/rpm + portable builds |

## v0.4 — Ghostty parity

Full feature parity with Ghostty, verified against the spec parity matrix (§27).
Remaining accessibility (screen reader matrix), IME parity, key-binding parity,
terminal capability parity.

## v0.5+ — Ecosystem

| Feature | Status |
|---|---|
| Graphics protocols (kitty, 6bit SGR) | Done: kitty subset (transmit/put/delete/query, PNG/JPEG via WIC); 6bit SGR renders as text (SGR 256 + block chars) |
| Plugin system / MCP | Design phase done: [`docs/design-plugins-mcp.md`](docs/design-plugins-mcp.md); implementation not scheduled |
| Mobile / tablet | Design phase done: [`docs/design-mobile-tablet.md`](docs/design-mobile-tablet.md); implementation not scheduled |
| Windows ARM64 | Cross-check gate in CI: done. Release ZIP blocked: needs the HostX64/arm64 MSVC tools on the build box |
| macOS backend | Open — no committed date |

## Post-v0.5 — Daily-driver excellence and project workflows

Approved. Build an exceptional native Windows terminal for managing
an entire development project: reliable sessions, reusable workspaces,
command-aware output and first-class Windows integration. Preserve native UI,
local-first operation, portable support and no telemetry.

These are planned stages, not shipped features or new version/date commitments.
[Master backlog #45](https://github.com/dhruvparpia/rusttyterm/issues/45) tracks
execution; linked issues contain acceptance criteria and dependencies. Reconcile
new behavior and configuration schemas with `docs/SPEC.md` before implementation.

### Stage 0 — Daily-driver reliability

| Work | Deliverable | Tracking |
|---|---|---|
| Compatibility and recovery gates | Repeatable Windows matrix for PowerShell, Git Bash, WSL, SSH, Neovim, tmux, fzf and interactive coding-agent CLIs; Unicode, IME, AltGr, clipboard, resize, mixed DPI, background exits, sleep/wake and graphics recovery | [#36](https://github.com/dhruvparpia/rusttyterm/issues/36) |
| Correctness and responsiveness | Triage existing defects; prioritize session identity, terminal replies, content preservation, rendering and resource/lifecycle correctness; measure input latency, startup, memory/handles and sustained output | Individual bug issues, coordinated by [#36](https://github.com/dhruvparpia/rusttyterm/issues/36) |

Record actual desktop results separately from portable tests and cross-compilation.
Compare damage-only rendering against full-frame references and exercise protocol
chunk boundaries. Security remediation remains in the private advisory workflow.

### Stage 1 — Discoverability and project workflows

| Feature | First deliverables | Tracking |
|---|---|---|
| Native command palette | Fuzzy action/profile/tab search, shortcut discovery and conflict diagnostics; shared action registry (shipped in #39), then pane/theme/settings/workspace providers | [#39](https://github.com/dhruvparpia/rusttyterm/issues/39) |
| Named project workspaces | Save/open reusable tab and pane layouts with profiles, directories and focus; templates independent of last-session restore; explicit startup tasks and Jump List/CLI entry points; schema in [design-workspaces.md](docs/design-workspaces.md) | [#37](https://github.com/dhruvparpia/rusttyterm/issues/37) |
| Links and editor navigation | OSC 8 hyperlinks with pointer open/copy shipped in #41; remaining: diagnostic file:line:column links, configured editor launch and keyboard hints; correct Unicode, quoting and cwd resolution (child issues) | [#41](https://github.com/dhruvparpia/rusttyterm/issues/41) |
| Windows development workflows | WSL/PowerShell/Visual Studio shell discovery, Open terminal here, shell-aware file drag/drop and clear Windows/WSL/remote session identity (shipped in #40) | [#40](https://github.com/dhruvparpia/rusttyterm/issues/40) |

The initial palette can ship while workspace storage is designed. Fix cwd and
session-identity defects before building launch/navigation workflows on them.

### Stage 2 — Understand and navigate output

| Feature | First deliverables | Tracking |
|---|---|---|
| Command-aware scrollback | Opt-in OSC 7/133 integration, bounded command metadata, previous/next command, copy command output, duration/exit status and background completion notifications (shipped in #38); search/export of one command's output fold into large-output mode below | [#38](https://github.com/dhruvparpia/rusttyterm/issues/38) |
| Large-output mode | Incremental bounded search scan with follow-output pause/resume shipped in #42 slice 1; remaining: search results with context, stable bookmarks, streaming export, optional disk-backed history with explicit retention and disk-full behavior (child issues) | [#42](https://github.com/dhruvparpia/rusttyterm/issues/42) |

Share stable history anchors between command metadata, search and bookmarks.
Deliver responsive in-memory navigation before adding disk-backed storage. Shell
integration stays explicit and opt-in, consistent with the existing spec.

### Stage 3 — Persistent sessions and automation

| Feature | First deliverables | Tracking |
|---|---|---|
| Persistent running sessions / console attach | `rusttyterm --attach <n>` connects from another console to the same live PTY; session discovery, headless creation, detach/reattach, GUI views, read-only watchers, explicit control transfer and bounded recovery | [#43](https://github.com/dhruvparpia/rusttyterm/issues/43), [attach design](docs/design-session-attach.md) |
| Local automation API / CLI / MCP | Stable window/tab/pane/session IDs, versioned operations, structured lifecycle/completion events and streamed capture; MCP adapter over the same API | [#44](https://github.com/dhruvparpia/rusttyterm/issues/44) |

Persistent sessions retain live processes; current session restore respawns them.
Resolve lifecycle/backpressure behavior before implementing persistence. Design
stable IDs early; useful local automation over live windows can ship before a
persistent-session host. Reuse the capability model in
[`docs/design-plugins-mcp.md`](docs/design-plugins-mcp.md).

The next major persistent-session milestone has five delivery slices:

1. **Host and identity:** per-user session owner, durable IDs and stable numeric
   aliases, versioned local IPC and headless session creation.
2. **Console attach:** list sessions, `--attach <n>` inside the invoking console,
   interactive input/resize and a local detach escape that restores the caller.
3. **State continuity:** atomic screen/mode snapshot plus ordered updates; one
   terminal-query reply owner, bounded history and output draining while detached.
4. **Shared views:** attach the native GUI to the same session; read-only watchers,
   explicit controller takeover and one authoritative terminal geometry.
5. **Recovery:** reconnect/resynchronize, client-crash survival, clear host failure
   and version-mismatch behavior, saturated-output shutdown and real Windows gates.

The attach target is a managed PTY/pane, not a tab index or an arbitrary process
ID. Persistent sessions preserve live processes while their host survives; reboot
and host-process failure do not promise live-process restoration. Detailed CLI
semantics, ownership rules and acceptance scenarios are in the
[persistent-session/console-attach design](docs/design-session-attach.md).

## Performance budgets (all versions)

- 60 fps sustained under `glamor`-style load; damage-only redraw.
- 10k-line burst rendered in < 1 s.
- Zero allocations in the render hot path.
- Startup and idle-RAM at or better than Windows Terminal.
- Tab switch, tab create, tab close: perceptibly instant (< 16 ms).
- Config reload: hot, < 100 ms, no window restart.
