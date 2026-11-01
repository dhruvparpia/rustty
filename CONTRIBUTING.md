# Contributing to rusttyterm

Thanks for helping. This project is MIT-licensed and developed in the open;
issues and pull requests are the way work happens.

## Ground rules from the spec

The product requirements live in [`docs/SPEC.md`](docs/SPEC.md); the release plan
in [`ROADMAP.md`](ROADMAP.md). The non-negotiables:

- **Rust only.** No C/C++ core (FFI to Windows APIs is fine), no interpreted runtime, no Electron/web UI.
- **MIT license, no telemetry, no phone-home.**
- **Tabs ship in the MVP.** Splits are explicitly *not* in the MVP.
- Windows first. Renderer is DirectWrite + Direct2D; shell backend is ConPTY.

## Dev setup

1. Install the **latest stable** Rust toolchain (`rustup update stable`, MSRV **1.99**):
   <https://rustup.rs>.
2. `git clone` the repo, then `cargo build`.

Every dependency must be the latest stable release — see [AGENTS.md](AGENTS.md),
Rule 1. Check with `cargo outdated` before each milestone and release.

Windows GNU-toolchain users need MinGW-w64 in `PATH` (MSVC is the default and is
what CI uses: `x86_64-pc-windows-msvc`).

Every day:

```sh
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo test
```

CI enforces the clippy flags above (including `unwrap_used`/`expect_used`). In
UI/backend code, prefer `anyhow::Result` at the call boundary and handle errors at
the top level; never panic in the render loop, parser, or PTY read loop.

## Project layout

```
src/lib.rs        public data-model API (App, Tab, Pane, Grid, ShellSession, Config, Profile, Theme)
src/app.rs        application state: tab collection, active tab, commands
src/grid.rs       grid, viewport, scrollback, dirty tracking
src/pane.rs       pane tree, split directions, fullscreen toggle
src/shell.rs      shell session API (ConPTY backend lands here)
src/config.rs     TOML config, profiles, schema
src/theme.rs      theme model + presets
src/main.rs       CLI entry point
tests/            integration tests (public API only)
docs/             SPEC.md, docs index
```

New subsystems land as new modules (for example `src/pty/` for ConPTY,
`src/parser/` for the VT parser, `src/render/` for Direct2D). Keep them behind the
public API in `lib.rs`.

## Testing conventions

- **Unit tests**: inline `#[cfg(test)] mod tests` at the bottom of each module.
- **Integration tests**: top-level `tests/` directory, exercising the public API
  only.
- The VT parser must be unit-tested against byte fixtures **before** it is wired
  into rendering (spec §32, Step 6). Parser test fixtures live in `tests/fixtures/`.
- Every bug fix ships with a test that fails without the fix.

## Pull requests

- One concern per PR; keep them small and reviewable.
- Title: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- PRs that touch performance must say so, and include before/after numbers when
  measurable.
- CI (fmt, clippy, tests on `windows-latest`) must be green.
- Update `CHANGELOG.md` under **Unreleased** for user-visible changes.

## Behavior references

When a spec section says "match Ghostty/WT behavior", the reference implementation
is the user-visible behavior, not the code. Don't copy other projects' source —
implement against the spec, and link the spec section in the PR description.
