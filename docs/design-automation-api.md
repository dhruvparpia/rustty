# ADR: Automation identity, IPC schema and capability grants (#44 slice 0, #55)

**Status: accepted by the owner, 2026-11-06.** Implemented; the requirements are
SPEC §9.34 (AUTO-001..015). Where the amendments at the end of this record differ
from the text above them, the amendments win.

Design record for the local automation API. Accepted decisions are binding for
#56 (IPC server), #57 (CLI), #58 (events) and #59 (MCP adapter). One
operations surface: the CLI, the raw IPC and the MCP adapter are three
encoders over the same operations; none of them may re-implement targets,
authorization or bounds. `docs/design-plugins-mcp.md` keeps the plugin
transport; this ADR answers its three open questions.

## 1. Stable identities

- Every window, tab, pane and (later) session carries a **u64 id assigned at
  creation, monotonically increasing per process, never reused**. A closed
  target keeps answering "stale" forever; ids are never recycled, so a
  long-lived client can never act on the wrong object.
- Human aliases on top: tabs keep their 1-based tab number, panes keep a
  dot path from the tab root (`.0`, `.1.0`), windows keep the HWND. Aliases
  are conveniences; every response reports the canonical id.
- Stale/unknown ids produce the structured error `target-stale` (see §3),
  with the id echoed. Targets are never resolved by name-matching.
- Grid absolute line ids (already stable per pane) are the anchors for
  capture ranges, bookmarks and scrollback cursors.

## 2. Transport and authorization

- Per-window named pipe `\\.\pipe\rustty.<user>.<process-id>.<window-seq>`.
  Discovery: a per-user directory `%APPDATA%\rustty\ipc\` holding one
  small JSON file per live window (id, pipe name, pid, start time); files are
  deleted on clean exit and pruned when the pid is dead.
- Authorization is Windows-native: the server impersonates the client and
  requires the client token's **user SID to equal the server's own SID**.
  Same user = allowed; anything else is refused at connect. No tokens,
  no registry keys, no network listeners.
- The pipe server runs on its own thread. Grid access happens only on the
  window's message pump: requests are queued, drained each tick with a
  bounded per-tick budget (same pattern as the search timer, #42), and
  replies are written by the IPC thread. A client that stops reading can
  only stall its own queue; the terminal never blocks.

## 3. Versioned request/response envelope

JSON-RPC 2.0 (same base as MCP, so the adapter is a thin encoder):

- `initialize {protocolVersion, clientName} -> {protocolVersion, capabilities}`.
  Mismatch is a hard `protocol-mismatch` error; the server supports the
  version it shipped with and the one before (one-step-back compatibility).
- Requests: `{method, params, id}`. Results are JSON. All ids in responses
  are canonical u64.
- Errors are structured: `{code, message, target?}` with a fixed code table:
  `no-instance`, `target-stale`, `target-closed`, `denied-grant`, `timeout`,
  `slow-client`, `bad-params`, `protocol-mismatch`, `internal`.
  The CLI maps each code to a fixed exit code (documented in #57).
- Bounds: 64 KiB request frame; 1 MiB response page. Reads beyond a page
  stream via an opaque `cursor` token (`read-scrollback {pane, cursor}`);
  cursors are cheap (a line id) and expire with the pane.
- Per-client outbound queue: 4 MiB. Overflow closes that client's connection
  with `slow-client`; the server never buffers unboundedly.

## 4. Capabilities and grants

- Capability discovery: `capabilities {}` lists operations and their grant
  requirements; clients never hardcode a second list.
- Read operations (list, read-visible, read-scrollback, capture) need no
  grant. Write operations (`tab.open`, `tab.close`, `pane.split`,
  `pane.focus`, `pane.send-text`, `config.query`-adjacent changes) are
  grant-gated.
- CLI/IPC clients running as the same user are granted reads implicitly;
  writes require the config key `automation-writes = true` (default false),
  reported as `denied-grant` when off.
- MCP plugins keep the per-plugin `grants = [...]` model from
  `docs/design-plugins-mcp.md`, enforced by the same server check.

## 5. Answers to the open questions in the plugin design

1. **Scrollback beyond the UIA 500-line cap:** yes — reads are paginated
   (1 MiB pages with cursors, §3). Large reads are streamed, never one blob.
2. **`pane.send-text` rate limit:** token bucket per plugin, 64 KiB/s
   sustained, 8 KiB burst; excess returns `denied-grant`-adjacent
   `rate-limited` and the injection is visibly logged in the tab.
3. **Event subscriptions:** yes, as MCP notifications, over a fixed list:
   `tab-opened`, `tab-closed`, `pane-closed`, `bell`,
   `command-started`/`command-finished` (exit code, duration; needs the #38
   OSC 133 markers), `pane-output` (ordered, bounded per §3). One
   subscription per client per pane; overflow disconnects the subscriber
   (`slow-client`), never the terminal.

## Consequences

- #56 builds the identity table, pipe server and dispatcher; #57 encodes
  the CLI; #58 attaches the event list to the pump; #59 is the MCP encoder.
- The same identities feed the attach epic (#43) session aliases; identity
  design is shared, implementation is not.
- Nothing here touches the PTY read loop, the parser, or the render loop.

## Amendments (2026-11-06, while implementing)

1. **Per-window write token** (owner's direction). Besides `automation-writes`, a
   window generates a random token at start. Passed in `initialize` as `token`, it
   grants writes to that connection only. A session the window starts for a boss
   gets it as `RUSTTY_AUTOMATION_TOKEN`, through a helper no client can reach. It is
   compared in constant time, never logged, never in a file, and sent only to the
   window `RUSTTY_WINDOW` names. Shells never inherit it, nor `RUSTTY_WINDOW`: a
   window or host started from inside a boss session would otherwise pass them to
   everything it opens. Same-user code can read a process's environment, so this
   scopes a grant; it is not a boundary against other code running as this user.
2. **The send-text bucket is per pane, not per plugin** (§5). No plugin identity
   exists yet, and a CLI opening a connection per call would get a fresh burst each
   time, so a per-connection bucket would not limit anything. Per pane still does
   what §5 wants: nothing can flood a shell. A call carries at most one burst.
3. **Ids carry the process id in their high bits** (§1). Ids per process alone let
   a pane id from one window name a different pane in another, when `--window`
   changes or the window a client resolves to does. Every value stays below 2^53, the
   integers JSON clients read exactly.
4. **Injection is logged on the sidebar row, not in the tab** (§5). Text written
   into the grid would desynchronise it from the program's own screen. The row says
   "Typed by automation" for 5 s. With the sidebar off, nothing shows this.
5. **The identity check is the host's** (§2): the peer process's token, not
   impersonation, comparing user SID and also integrity level. The first pipe
   instance refuses a name already taken, and a client checks the server's process
   and identity before sending anything, since it may be about to send a token.
6. **`pane-output` is whole lines of text read back from the grid** (§5), not raw
   bytes, which would mean tapping the read path. Escapes are already gone. The line
   the cursor is on waits until it is finished or the program exits. While a program
   holds the alternate screen there are no lines to report: use `pane.read`. Lines
   evicted before they were sent are counted in `skipped`.
7. **Added to the fixed lists:** `status-changed {pane, status, detail}` (§5),
   `tab.focus` (§4) and the error `rate-limited` in the table (§3 and §5).
8. **Writes wait while a dialog is up or the window is being dragged** (§2). Code
   behind a confirmation box resumes assuming the tabs didn't change. Reads carry on,
   and a client that gives up takes its queued requests with it.
9. **MCP revision and names** (§3, §5). `rustty mcp` speaks `2026-07-28`, which has
   no handshake, and also serves `initialize` for `2025-11-25` and earlier. Dots in
   operation names become underscores in tool names (`pane_send-text`), because
   clients that pass tools to a model API commonly refuse dots. Events go out as
   `notifications/message` to handshake clients, and on a `subscriptions/listen`
   stream with the `com.rustty/events` extension to current ones.
10. **Reads stop on time as well as size** (§3). A full 1 MiB page held the UI thread
    for about 35 ms when measured, more than a tick, against the bounded per-tick work
    §2 requires. A read now stops after 4 ms and says `more`, and the pane-output scan
    shares 4 ms per tick, with busy panes taking turns. 1 MiB stays the cap on a page.
11. **Writes can't be turned on by editing a file** (§4). `automation-writes` is read
    once, when the window starts, and a hot reload never changes it: otherwise an
    agent that can write a file could grant itself writes with nothing for the person
    to notice. While it is on, the sidebar heading says "rustty · automation on".
12. **The token reaches fewer places** (amendment 1). `rustty ctl` ignores
    `RUSTTY_AUTOMATION_TOKEN` and only `rustty mcp` reads it, so running a command
    inside a boss session is not enough to write. `tab.open` and `pane.split` refuse
    it in `env`, and it is never in `WSLENV`. The boss starts with
    `--strict-mcp-config` and `--permission-mode default`. Anything the boss session
    starts itself still inherits the variable and can read it: this scopes a grant,
    and is not a boundary against code running as this user.
