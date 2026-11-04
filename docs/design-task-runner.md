# TaskRunner seam — #89 architecture

`TaskRunner` is a deep module: a small interface (`run`, `stop`, `restart`) holds task-store lookup, dependency readiness, canonical run reconnect/create and explicit outcomes. The window layer is an adapter that renders palette rows and calls the seam; it no longer owns task lifecycle JSON parsing or dependency startup.

## Seam

The seam is `TaskRunner`, not `window.rs`. Its interface is:

- input: a `Task` id, a `TaskStore` dependency and a `HostAdapter`
- output: an explicit `Outcome` (`Started`, `AlreadyRunning`, `Stopped`, `MissingTask`, `NotLive`, `DependencyFailed`, `HostFailed`)

## Adapters

The `HostAdapter` has two concrete adapters:

- the real host client adapter (`real_host_adapter`)
- a synthetic in-memory adapter used by tests

That is the two-adapter signal that the seam is real, not hypothetical.

## Tests

The tests cross the `TaskRunner` interface with a synthetic host: missing task, missing dependency, dependency start then target, reconnect, stop and restart. The real host adapter is used only at the window boundary.

## What stays behind the seam

Dependency cycle validation stays in `TaskStore`; atomic store policy stays in the store module; palette row rendering stays in the window adapter. The task lifecycle itself is one module.
