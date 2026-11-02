# Workspace templates — storage schema and migration policy (#37)

Design record for SPEC `WORKSPACE-001..009`. A **workspace** is a named,
reusable launch template: tabs, pane split shapes, profiles, tab titles,
per-pane working directories, focus, and optional explicit per-pane startup
commands. Workspaces are *templates*: they are opened on demand and saved on
demand. They are never written by session restore, and closing a window never
overwrites one (WORKSPACE-002).

## File

`workspaces.toml` sits beside `config.toml` (portable mode wins, same
resolution as `session.toml`). The file is user-visible and hand-editable, so
the reader is forgiving on load and strict on write.

## Schema (version 1)

```toml
version = 1

[[workspace]]
name = "web"
focused_tab = 0

[[workspace.tab]]
profile = "Git Bash"
title = "server"
title_fixed = false
focused_pane = 0

[workspace.tab.shape]
kind = "split"
dir = "vertical"
ratio = 0.5

[workspace.tab.shape.a]
kind = "leaf"
cwd = "C:/src/web"
command = ["npm", "run", "dev"]

[workspace.tab.shape.b]
kind = "leaf"
cwd = "C:/src/web"
```

- `version` (u32): schema version. A v1 file missing the key is read as 1.
- `[[workspace]]`: array of templates. Each has `name` (exact string,
  compared case-insensitively for duplicate detection), `focused_tab`, and a
  `tab` array.
- `[[workspace.tab]]`: mirrors a session tab — `profile` (a config/built-in
  profile name), `title`, `title_fixed`, `focused_pane`, `shape`.
- `shape`: the same tagged `kind = "leaf" | "split"` tree as `session.toml`
  (`dir`, `ratio`, children `a`/`b`), plus on a leaf an optional
  `command = [argv...]`. `argv[0]` is the program, the rest are arguments;
  the pane's profile still supplies environment and starting-directory
  defaults, overridden by the leaf `cwd`.
- Caps: ≤ 64 workspaces, ≤ 64 tabs per workspace (`session::MAX_TABS`),
  split depth ≤ 64 (`session::MAX_DEPTH`, collapses to a leaf exactly as a
  deep `session.toml` split does). A file over a tab/workspace cap is
  reported and ignored, never written back.

## Semantics

- **Commands are explicit template data.** The UI capture path writes leaf
  `cwd` only and never a `command`; a saved *session* has no command field,
  so ordinary session output can never become a startup command
  (WORKSPACE-006).
- **Open**: template → in-memory session (commands kept beside it, consumed
  once at spawn). A missing profile falls back to the first profile, a
  missing directory or executable fails only that pane's spawn, with the
  error reported and unaffected panes opening (WORKSPACE-007).
- **Save / Save As** capture the live window (layout, profiles, titles,
  focus, cwds). **Overwrite** targets the workspace this window opened from.
- **Names**: kept byte-exact (Unicode, spaces); duplicates rejected
  case-insensitively so Jump List tasks and menu entries never collide.

## Versioning and migration (WORKSPACE-009)

- The writer always emits the current version (1).
- The reader accepts `version` ≤ current. Today current = 1, so only
  `version = 1` (or the key missing) loads.
- A file with a **newer** version than this rusttyterm understands is
  reported ("workspaces file is version N; this rusttyterm understands
  version 1") and treated as empty: it is **never rewritten, migrated
  silently, or clobbered**. Mutating operations (save/rename/delete) refuse
  to touch a store they could not read.
- A file that fails to parse is reported and treated the same way:
  readable-then-ignored, never rewritten (same contract as `session.toml`).
- Future versions: add fields with serde defaults (v1 readers ignore
  unknown fields? No — v1 refuses to read v≥2 files, above); a bump to v2
  adds a `migrate_v1_to_v2` step in `workspace.rs` that rewrites the file
  once, atomically (write `workspaces.toml.tmp`, rename), and documents the
  field changes here.

## CLI / UI surface

- `rusttyterm --workspace <name>`: open the template in a new window; a
  missing template is reported (message box) and a plain window opens.
- `rusttyterm --workspaces`: print template names to stdout.
- File → Workspaces: `Open <name>…` (one item per template, replaces this
  window's tabs after confirmation), `Save As New…`, `Overwrite "<current>"`,
  `Rename…`, and a Delete submenu. The name prompt is a native modal dialog
  (built `DLGTEMPLATE` + edit + OK/Cancel).
- Jump List: one task per template (`--workspace "<name>"`).
