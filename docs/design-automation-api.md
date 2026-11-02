# ADR: Automation identity, IPC schema and capability grants (#44 slice 0, #55)

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

- Per-window named pipe `\\.\pipe\rusttyterm.<user>.<process-id>.<window-seq>`.
  Discovery: a per-user directory `%APPDATA%\rusttyterm\ipc\` holding one
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
