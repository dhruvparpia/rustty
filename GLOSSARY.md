# Glossary

## Project terms

### Workspace
A named, reusable launch template: tabs, pane split shapes, profiles, titles, focus, working directories and optional explicit startup commands. It is reopened on demand and never written by session restore.

### Task
An explicit named run definition: program argv, working directory, optional profile, dependency ids and an informational target layout. A task is not inferred from shell history or repository files.

### Session
A host-owned live process with a durable id and stable alias. It can outlive GUI and console clients. A session is not the same thing as a workspace or a task definition.

### Run
One task's session-backed execution. Runs use canonical session labels and reconnect by identity rather than palette row or list position.

### View
A GUI tab that attaches to a host session. Closing a view detaches it; terminating the session kills the shell.

### Controller / watcher
A controller can send input and resize the session. A watcher receives output only. Takeover demotes the previous controller to a watcher.

## Validation terms

### Desktop validation
Validation that needs a logged-in Windows desktop: Direct2D benches, keyboard/UIA checks and dogfooding. It is distinct from the headless `cargo test` suite.

### Windows in-scope
The Windows desktop issues that can be implemented and validated on this machine. Cross-platform frontend and Unix PTY work waits for real macOS/Linux machines.
