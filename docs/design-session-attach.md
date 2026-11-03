# Persistent sessions and console attach

Next major milestone, tracked by [#43](https://github.com/dhruvparpia/rustty/issues/43).
Owner-requested entry point: **`rustty --attach <n>` from another console**.
This is a feature/architecture specification; the commands below are planned,
not implemented. No version number or release date is assigned yet.

## User experience

A developer starts a managed shell in rustty, runs a build or opens Neovim,
then connects to that same session from PowerShell in Windows Terminal, another
rustty window, or another supported local console. Attaching preserves the
shell PID, current directory, environment, jobs and terminal state. Detaching
returns to the caller's shell without stopping the managed session.

One attach target is **one managed PTY/pane**, not a tab index or OS process ID.
Tabs, splits and workspaces arrange views of those sessions. The first milestone
attaches to individual sessions; full workspace-layout attach can follow.

## Planned CLI contract

| Command | Behavior |
|---|---|
| `rustty --list-sessions` | List session number, name, profile, cwd when known, state, controller and viewer count |
| `rustty --list-sessions --json` | Versioned structured output for scripts; include durable ID and exit status |
| `rustty --new-session --profile "PowerShell" --name build` | Create a persistent session and attach in the invoking console |
| `rustty --new-session --profile "PowerShell" --name build --detach` | Create a persistent session without a view; print its session number |
| `rustty --attach 3` | Attach interactively to existing session 3 in the invoking console |
| `rustty --attach 3 --read-only` | Watch the session without acquiring input or resize ownership |
| `rustty --attach 3 --take-control` | Explicitly transfer input/resize ownership from the current controller |
| `rustty --attach 3 --window` | Open a native GUI view of the same session |
| `rustty --session-info 3 --json` | Inspect metadata, geometry, attachments and exit state |
| `rustty --terminate-session 3` | Request graceful termination; report completion or timeout |
| `rustty --terminate-session 3 --force` | Explicitly force termination according to the documented process-tree policy |

These auxiliary spellings are the proposed first CLI contract. Resolve grammar
and compatibility with #44 before implementation. `--attach <n>` is the required
user-facing entry point. A future parser may also accept durable IDs; names are
display labels initially, avoiding ambiguous duplicate-name lookup.

- Invalid/missing IDs, incompatible modes and malformed options fail visibly;
  never fall back to starting a new shell or a GUI window.
- An unknown session is not auto-created. `--attach` with `--profile` is an error:
  attaching does not replace the original process environment or profile.
- Interactive console attach requires terminal input/output. Redirected output
  uses a separate bounded capture operation from #44, not interactive VT output.
- Define stable CLI exit codes for usage, not found, busy/controller conflict,
  host unavailable, protocol incompatibility and successful detach. Exited shell
  status must be available in structured metadata, distinct from client errors.
- Show session IDs in GUI pane chrome, context menus and palette; provide
  **Copy attach command**, **Detach view**, **Watch**, and **Terminate session**.

## Identity and lifetime

- Durable opaque session ID plus a human-friendly numeric alias `<n>`.
- Allocate aliases atomically within a per-user host namespace; never bind a stale
  number to an unrelated live session. Persist allocator state; if registry state
  is lost, require explicit recovery rather than silent alias recycling.
- Rename/reorder tabs and split/focus changes do not change a session ID.
- Initial delivery supports sessions created by rustty's session host. It does
  not adopt arbitrary external consoles or recover already-lost ConPTY handles.
- Introduce persistent mode explicitly. Legacy ephemeral sessions keep their
  documented destructive close behavior. Persistent views clearly label detach
  versus terminate; last-view close does not terminate their shell.
- Choose session mode at creation. Any future promotion of a legacy GUI-owned
  ConPTY must have an explicit supported migration path; do not imply that saving
  a PID makes it attachable.
- GUI and console client exit/crash leave persistent shells alive while their
  owner host lives. Host crash, Windows sign-out or reboot may end them. Never
  advertise live-process survival across those events as session restore.
- Retain bounded exited-session metadata/output for inspection. Never silently
  respawn an exited process on attach; expose a separate restart action later.

## Ownership and host architecture

Prototype a **per-user session-host process** owning ConPTY handles, child
processes, the output reader, canonical terminal model and bounded history.
GUI and console clients subscribe to state and submit input. The host is the
sole reader of PTY output and sole producer of terminal-generated replies.

Resolve these details in an ADR before building the production host:

1. Host launch/ready handshake, single-instance races, idle shutdown policy and
   portable installation namespace.
2. Windows named-pipe transport with current-user access, local-only endpoints,
   framed/versioned messages, payload bounds and cancellation.
3. Same-user integrity/elevation boundaries: normal clients must not silently
   gain control of elevated shells; unsupported elevation combinations fail.
4. Process/job ownership so killing a client cannot kill the host-owned shell,
   while explicit termination cleans up handles/descendants predictably.
5. Upgrade compatibility: clients negotiate capabilities/version; incompatible
   upgrades fail without replacing a live host or killing its sessions.

Avoid embedding the host in the GUI lifecycle. Keep disk session-restore templates
separate from the live host registry: a saved layout cannot prove a shell exists.

## Console frontend

`--attach` runs **inside the caller's terminal**, not in a new GUI by default.
Provide a console client that renders host terminal state into supported VT
sequences, collects input and reports viewport changes.

- Save and restore caller console modes, code page where changed, cursor and
  alternate-screen state on normal detach and recoverable errors.
- Handle Unicode text, Ctrl+C, navigation/function keys, paste, mouse reporting
  and resize. Forward session input; Ctrl+C must not accidentally kill the host.
- Proposed local escape prefix: **Ctrl+] then d** detaches, **Ctrl+] then ?** shows
  help, and **Ctrl+] then Ctrl+]** sends a literal Ctrl+]. Make the prefix
  configurable and test it with TUIs and nested SSH/tmux sessions.
- Attachment must remain foreground/synchronous to the invoking shell. Account
  for Windows GUI-subsystem executable launch behavior and console handle setup;
  choose a console launcher/entry-point design during the prototype.
- Advertise a supported-client matrix and negotiate rendering capabilities.
  Preserve the session's TERM contract across attachments rather than changing
  it to match each viewer. Unsupported graphics have an explicit fallback;
  text-only attach must remain usable.
- Prevent self-attachment to the containing session; define handling of nested
  attachment chains to avoid feedback loops and confusing local escape routing.

## Multiple attachments and geometry

- Support several read-only watchers and **one input controller** per session.
- Ordinary attach acquires control if free; if busy, report the controller and
  require `--read-only` or explicit `--take-control` rather than stealing focus.
- Takeover changes the previous controller into a watcher and visibly notifies
  both clients. Host-side ownership tokens reject delayed input from the old
  controller; transfer is atomic with resize authority.
- Only the controller sets ConPTY dimensions. Watchers display the canonical
  geometry using clipping/panning; their resizes do not fight the controller.
- On controller disconnect, release control and retain last geometry. A watcher
  can explicitly request control; do not automatically promote an arbitrary one.
- Concurrent collaborative typing and shared clipboard are later opt-ins, not
  implicit consequences of attaching a second view.

## State synchronization and backpressure

1. Host assigns monotonically increasing output/model revision numbers.
2. Attach atomically obtains a snapshot at revision R plus subsequent changes.
3. Client installs that snapshot and consumes ordered changes after R without a
   lost or duplicate interval, even while the shell continuously writes.
4. Snapshot covers main/alternate screen, cursor, modes, colors, titles and known
   command/cwd metadata; negotiate graphics and history separately.
5. Reconnection resumes from an available revision or explicitly resynchronizes
   from a new snapshot if retained deltas were evicted.

Do not replay raw historic PTY bytes as an attach mechanism. They may contain
terminal queries, notifications or other side effects; queries get exactly one
reply from the host, including when no client is attached. Console frontend
responses to its own rendering/probing must not leak into shell input.

Bound PTY queues, per-client pending data, snapshots, history and image storage.
An attached slow watcher cannot stall other clients: disconnect/resynchronize a
lagging viewer according to policy while preserving authoritative model state.
Detached output continues draining under bounded retention; interactive prompts
wait for user input normally. Test cancellation/shutdown under saturation so
backpressure cannot deadlock ConPTY close.

## Delivery slices and exit criteria

| Slice | Exit criteria |
|---|---|
| [A — Host and identity prototype (#46)](https://github.com/dhruvparpia/rustty/issues/46) | Headless managed shell, stable number/ID, versioned discovery/IPC; client death does not kill shell |
| [B — Single-console attach (#47)](https://github.com/dhruvparpia/rustty/issues/47) | `--list-sessions`, `--attach n`, input/resize, detach escape and caller-console restoration work end-to-end |
| [C — State continuity (#48)](https://github.com/dhruvparpia/rustty/issues/48) | Attach into running Neovim/build; correct snapshot/delta ordering, single query-reply owner, bounded detached history |
| [D — GUI integration and shared views (#49)](https://github.com/dhruvparpia/rustty/issues/49) | GUI attaches to host sessions; copy attach command, watchers, explicit takeover and deterministic geometry |
| [E — Recovery and release gates (#50)](https://github.com/dhruvparpia/rustty/issues/50) | Reconnect, stale records, host incompatibility, shutdown stress, accessibility and documented client matrix |

Release acceptance must include actual Windows desktop/console runs:

- Start a persistent session in GUI, record PID/cwd and a live counter, detach,
  attach from another PowerShell/Windows Terminal console; same process continues.
- Create a headless session, list it, attach, detach back to the caller's prompt,
  then reattach. Caller terminal modes and subsequent commands remain normal.
- Attach while output is continuously changing; compare host/client model state
  and revisions for omissions/duplicates, then repeat during resize.
- Attach/detach with Neovim, fzf, tmux over SSH, Unicode/IME/AltGr and bracketed
  paste. Distinguish client-terminal limitations from session corruption.
- Two consoles plus GUI: only the controller writes/resizes; takeover revokes old
  input; slow watchers do not freeze the controller or grow memory indefinitely.
- Kill the GUI/client during a long-running job; reattach and verify PID/job
  continuity. Test client loss during handshake, takeover and snapshot transfer.
- Terminate while output is saturated; verify bounded completion, child/handle
  cleanup and correct exited-session status. Simulate host failure honestly.
- Run no-client terminal queries and repeated attach cycles; no duplicate replies,
  repeated notification/paste side effects or leaked handles/threads.

Dependencies: reliability #36; stable automation identities #44; workspace #37
integration is useful but not required for the first `--attach` proof. This design
initially targets local Windows sessions. Network access and remote-machine attach
need a separate transport/authentication design and are not implied by `<n>`.

## Shipped limits (v0.5)

What the shipped feature does not do, stated so no one discovers it by accident:

- **Local Windows managed sessions only.** The host spawns and owns every
  session it lists. It never adopts an already-running console program, a
  terminal window it did not create, or a session on another machine.
- **The host process is the durability boundary.** A session survives every
  client (GUI or CLI) crash while the host lives. If the host process itself
  dies — crash, forced kill, sign-out, reboot — its shells die with it (they
  live in the host's job object) and are not respawned. Attaching afterwards
  gives an explicit "host unavailable" (exit 5); the next client starts a
  fresh host with an empty session set.
- **No network, no authentication.** The named pipe is local and open to the
  logged-in user's session only. Anyone who can attach can control the shell.
  Cross-machine attach needs the separate transport design named above.
- **Bounded history.** Exited sessions keep their metadata in a ring of the
  most recent 32 records; older ones fall off. Detached-output replay covers
  the most recent 128 KiB of PTY output per session; resuming beyond that
  point gets a fresh full snapshot, not a partial delta.
- **Protocol is negotiated down, never up.** A newer client attaches to an
  older host at the host's version; a client that speaks nothing the host
  knows gets an explicit protocol-mismatch (exit 6), never a degraded
  silent attach.
