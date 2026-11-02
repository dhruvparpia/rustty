# Plugin system / MCP — design phase only

Roadmap v0.5 ("Plugin system / MCP — design phase only"). This is a design
document, not a contract: nothing here is implemented and no key is in
`src/config.rs`. SPEC.md stays the source of truth.

## Decision: one mechanism, MCP over stdio

A plugin is a **separate process** that speaks
[MCP](https://modelcontextprotocol.io) (JSON-RPC 2.0) over its stdin/stdout.
rustty is the MCP host. There is no in-process plugin ABI: no DLL
loading, no embedded scripting language, no stable-Rust-plugin interface.

Why:

- A crashing plugin kills its own process, never the terminal (the render
  loop must never panic; a third-party DLL breaks that promise).
- MCP is a published protocol with existing SDKs (Rust, TypeScript,
  Python); a private ABI is ours to design, version, and document forever.
- One transport means one permission model, one config key, one test
  harness.

## Shape

- Config: `[[plugin]]` entries name an executable, args, and an enable
  flag. Only executables listed in the config are ever started; there is
  no plugin directory scan, download, or update mechanism.
- Startup: each enabled plugin launches with the window; a plugin that
  fails the MCP handshake within 5 s is marked dead and reported once.
- Plugins expose **tools**; rustty exposes its own capabilities as
  MCP server tools (read visible grid text, read scrollback, send text to
  a pane, open/close/focus tabs and panes, query config).
- Every plugin tool that *writes* (send text, open tab, change config)
  requires a per-plugin grant in config, e.g.
  `grants = ["pane.send-text", "tab.open"]`. No grants: read-only tools
  only. First use of an ungranted capability is a native dialog, and the
  answer is written to config.

## Boundaries

- No telemetry, no network by the host on a plugin's behalf (spec "No
  telemetry, no phone-home").
- Plugins read terminal state only; they are never in the PTY path. A
  plugin cannot inject bytes that look like user keystrokes to the shell —
  `pane.send-text` is a distinct, grantable, visibly-logged operation.
- Clipboard access stays behind the existing Windows privacy prompt.

## Explicit non-goals

- An in-process plugin API (DLL, COM, Rust trait objects).
- A plugin marketplace, signing, or update pipeline.
- Lua/JS/WASM scripting inside the terminal.
- Plugins rendering pixels (no custom overlay drawing).

## Open questions (resolve before any implementation)

1. Do plugins get scrollback beyond the UIA cap of 500 lines, and is a
   large read streamed?
2. `pane.send-text` rate limit to stop a plugin flooding a shell.
3. Whether a plugin can subscribe to events (tab opened, bell) via MCP
   notifications, and what the event list is.
