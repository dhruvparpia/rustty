//! `rusttyterm` — a fast, native-feeling, MIT-licensed terminal emulator
//! for Windows (spec: [`docs/SPEC.md`](https://github.com/dhruvparpia/rusttyterm)).
//!
//! Data-model core: grid + scrollback, panes, shells, tabs, app state,
//! TOML config, themes. The ConPTY backend and the Direct2D renderer are
//! the next milestones (spec §32, Steps 5 and 8).

// NFR-401: unsafe code is isolated to FFI boundary modules (spec §15).
#![deny(unsafe_code)]
// CI forbids unwrap/expect in shipped code; tests are allowed to use them.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app;
pub mod color;
pub mod config;
pub mod grid;
// ConPTY FFI boundary (spec WIN-001). The crate-level `deny(unsafe_code)`
// below is lifted only for this module, so every unsafe block in the crate
// lives inside this file.
#[allow(unsafe_code, reason = "WIN-001: ConPTY is a raw Win32 FFI boundary")]
mod conpty;
pub mod pane;
pub mod parser;
pub mod shell;
pub mod tab;
pub mod theme;
