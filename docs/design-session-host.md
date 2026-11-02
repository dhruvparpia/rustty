# ADR: Session host lifetime, ownership and transport (#43 / #46)

Resolves the five items `docs/design-session-attach.md` requires settled
before the production host is built. Binding for #46–#50. Coordinate with
`docs/design-automation-api.md`: the session host reuses its envelope,
bounds and authorization style; the attach operations join the same
capability list, not a second control surface.

## 1. Host lifetime, launch race, idle policy, namespace

- The host is a separate executable, `rustty-host.exe`, installed beside
  `rustty.exe` (same directory: portable namespace travels with the
  app; no registry, no services, no scheduled tasks).
- Any client that needs a host tries to connect first, then spawns
  `rustty-host.exe --serve` once. The single-instance race is settled by
  the pipe name: the listener that binds `\\.\pipe\rustty-host.<user>` wins;
  every loser (process and client) exits 0 immediately. There is exactly
  one host per user per session, by construction.
- Ready handshake: client polls the pipe (50 ms interval, 5 s budget) and
  completes the `initialize` exchange from the automation envelope. A host
  that cannot bind within its own startup window exits with a log line.
- Idle shutdown: with zero sessions, zero attachments and no GUI client
  connected, the host exits after 10 minutes. GUI-owned persistent
  sessions count as "GUI client connected" while their window lives.
- One host crash domain: the host never panics (same crate rules), and a
  host death is visible to clients as `host-gone`, never as a silent
  reconnect loop.

## 2. Transport

- One listen pipe, one connection per client, frames = 4-byte
  little-endian length + UTF-8 JSON, reusing the automation envelope
  (versioned `initialize`, the same structured error table, 64 KiB request
  cap). Stream frames (snapshots, deltas, capture) are the same envelope
  with an opaque `stream` id and `seq`; a frame's payload page is capped
  at 1 MiB.
- Host→client streams have a 4 MiB pending budget per client. Overrun
  policy: the host marks that stream `resync-required` and sends a fresh
  snapshot at the current revision (attach semantics), or disconnects a
  client that will not resync. The PTY reader is never blocked by a slow
  client; host-side retention is bounded (item 3 of #48).
- Cancellation: every streaming request takes a `req_id`; `cancel {req_id}`
  is honored within one page. Disconnect is cancellation of everything.
- Local-only: named pipes are already local; no TCP socket exists in this
  design.

## 3. Same-user and integrity/elevation boundary

- Authorization is Windows-native on connect: impersonate, require the
  client token's user SID == the host's SID, **and** require the client's
  token integrity level == the host's. A non-elevated client is refused
  control of an elevated host with the structured error
  `integrity-mismatch`; the host never silently elevates.
- Running the GUI elevated and attaching from a normal console is the
  documented unsupported combination; the CLI reports it as
  `integrity-mismatch`, not "not found".

## 4. Process and job ownership

- The host creates each session's ConPTY with its **own job object**
  (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), one job per session. The GUI and
  console clients are not in that job: killing a client can never kill a
  host-owned shell.
- `--terminate-session` closes the ConPTY, then terminates the job tree
  (graceful: close + bounded wait; `--force`: immediate job term).
  Termination is the only path that kills session descendants, and it is
  always explicit.
- Host exit (idle policy, sign-out, explicit) terminates every session
  job; this is the documented lifetime limit, never advertised as
  survive-reboot.

## 5. Upgrade compatibility

- Every host and client embeds a protocol version (u32). `initialize`
  negotiates `min(client, host)` and the host supports that version plus
  one back. Incompatible client → `protocol-mismatch`, exit code defined,
  and **the client never starts, replaces, signals or kills a host it
  cannot speak to** (the spawn-race in item 1 binds: a losing client
  process exits before it can harm an old host).
- `rustty --session-info <n> --json` exposes the live host's protocol
  version, so a mismatched user gets a visible "restart the host after
  updating" instruction; a host with live sessions is never auto-replaced.

## Consequences for the slices

- #46 builds `rustty-host.exe`: pipe listener, envelope, identity/alias
  allocator (u64 ids, never reused, same rule as the automation ADR;
  numeric alias allocated atomically in the host namespace, persisted
  under `%APPDATA%\rustty\host\`), headless ConPTY creation in its
  own job, `--list-sessions`/`--session-info`, and the client-death
  demonstration.
- #47 adds the console frontend over the same frames; #48 adds
  revisions/snapshots; #49 makes the GUI a client of the host; #50
  stress-gates everything.
