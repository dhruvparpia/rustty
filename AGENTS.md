# Rules for AI agents and contributors working on rusttyghost (rusttyterm)

Read `docs/SPEC.md` before changing behaviour. The spec is the product contract;
`ROADMAP.md` is the release plan.

## Rule 1: only latest-stable packages

Every dependency, tool and toolchain component this project uses must be the
**latest stable** release at the time you add or touch it. No old components, no
"whatever is already installed", no pinned legacy versions.

Concretely, when adding or changing a dependency:

1. Find the current stable version (crates.io / the vendor's own release page), not
   the version that happens to be in the local cargo cache.
2. Use that version in `Cargo.toml`, then `cargo update` and commit `Cargo.lock`.
3. Confirm the current stable toolchain (`rustup update stable`, `rustc -V`) and keep
   `rust-version` (MSRV) in `Cargo.toml` equal to it. MSRV caps cargo's version
   resolution, so a stale MSRV silently pins every crate to an older release.
4. Never add a pre-release, `-alpha`/`-beta`/`-rc`, yanked, or git/dev-pin dependency.
   Yanked or superseded versions are removed with `cargo update`, not kept.
5. Prefer crates that are actively maintained (recent release, no unmaintained
   advisory in `cargo audit`) over feature-comparable abandoned crates.
6. When a dependency must be updated in a way that changes the API, port the code to
   the new API in the same change. Do not carry compatibility shims for old versions.
7. Record what you checked in the commit message: toolchain version and each crate
   version you validated.

`cargo outdated` (`cargo install cargo-outdated`) is the check to run before a
release, and at least once per milestone.

## Rule 2: CI gates are hard gates

All of these must pass before a commit is pushed:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo test
```

- `#![deny(unsafe_code)]` at crate level. `unsafe` lives only in FFI-boundary modules
  (NFR-401), each with an `#[allow(unsafe_code, reason = "...")]` naming the spec ID.
- Never panic in the render loop, the parser, or the PTY read loop.
- Public items need doc comments (`missing_docs` warns, and `-D warnings` makes it fatal).

## Rule 3: keep the memory current

After a decision, a root cause, or a project state change, write a note in the brain
vault (`inbox/`, one topic per file, see the vault's own rules) and tell the owner the
file name. Never edit `wiki/`.

## Rule 4: don't invent scope

MVP exclusions stay excluded: splits, shell integration, IME, session restore,
graphics protocols, telemetry, Electron/web UI. Tabs are mandatory.
