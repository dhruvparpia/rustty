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
// lives inside the FFI-boundary files.
#[allow(unsafe_code, reason = "WIN-001: ConPTY is a raw Win32 FFI boundary")]
pub mod conpty;
// Clipboard FFI boundary (spec WIN-004).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-004: the clipboard is a raw Win32 FFI boundary"
)]
pub mod clipboard;
/// Keyboard encoding is pure Rust (no FFI): Win32 key events in, VT bytes
/// out (spec WIN-005).
pub mod keyboard;
pub mod pane;
pub mod parser;
pub mod selection;
pub mod shell;
// Direct2D/DirectWrite renderer FFI boundary (spec WIN-002, WIN-003).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-002/WIN-003: Direct2D/DirectWrite are raw Win32 COM FFI boundaries"
)]
pub mod render;
pub mod tab;
pub mod tabbar;
pub mod theme;
// Win32 window + message loop FFI boundary (spec WIN-001, WIN-005).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-001/WIN-005: the Win32 message loop is a raw FFI boundary"
)]
pub mod window;
