# Project tasks — schema and lifecycle (#89)

A **task** is an explicit, named project run: argv, working directory, optional profile, dependency ids and an informational target layout. Tasks are not inferred from shell history or repository files. The store is `tasks.toml` beside `config.toml` (portable mode wins).

## File and schema (version 1)

```toml
version = 1

[[task]]
id = 1
name = "dev"
argv = ["npm", "run", "dev"]
cwd = "C:/src/web"
profile = "Git Bash"
layout = "web"
deps = []

[[task]]
id = 2
name = "test"
argv = ["npm", "test"]
cwd = "C:/src/web"
deps = [1]
```

- `version` (u32): current writer emits 1; a missing key reads as 1.
- `id`: positive, unique, canonical. It is never a list position.
- `name`: display label; duplicate names are rejected case-insensitively.
- `argv`: at least one item; `argv[0]` is the program.
- `cwd`, `profile`, `layout`: optional. `layout` is only a hint.
- `deps`: ids that must be live before the task starts.
- Caps: `MAX_TASKS = 64`.

## Validation

`validate_tasks` rejects duplicate ids, duplicate names, empty argv and missing/self dependencies. A Kahn topological pass rejects dependency cycles before any process is started.

## Session identity

A task run is a host-owned persistent session named `task#<id> <name>`. The canonical label lets a reopened GUI reconnect the same run; it does not resolve by palette row or list position.

The host `session.create` accepts explicit `program` and `args`, so task argv reaches ConPTY without being converted into shell text.

## Palette actions

The palette lists `Run Task`, `Stop Task` and `Restart Task` from the store. Run starts dependencies first and then opens the task view; it reconnects a live task instead of launching a duplicate. Stop terminates the live task session. Restart stops the live run and re-runs it. Exited or missing targets are explicit failures.

## Migration and ownership

A newer version or an unreadable store is reported and never rewritten. Hand edits are the supported creation path for this first slice. The store is independent of `workspaces.toml` and `session.toml`; it never captures shell output as task data.
