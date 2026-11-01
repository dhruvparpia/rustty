//! `rusttyterm` — a fast, native-feeling, MIT-licensed terminal emulator
//! for Windows (spec: [`docs/SPEC.md`](https://github.com/dhruvparpia/rusttyterm)).
//!
//! Data-model core: grid + scrollback, panes, shells, tabs, app state,
//! TOML config, themes. The ConPTY backend and the Direct2D renderer are
//! the next milestones (spec §32, Steps 5 and 8).

#![forbid(unsafe_code)]
// CI forbids unwrap/expect in shipped code; tests are allowed to use them.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app;
pub mod color;
pub mod config;
pub mod grid;
pub mod pane;
pub mod shell;
pub mod tab;
pub mod theme;
