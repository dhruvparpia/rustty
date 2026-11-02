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
pub mod cli;
pub mod color;
pub mod config;
pub mod grid;
// Glyph atlas FFI boundary (spec §20 "custom glyph cache / custom font
// atlas", §21): Direct2D calls on the atlas' bitmap render target.
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "SPEC §20: the glyph atlas is a Direct2D COM FFI boundary"
)]
pub mod glyph_cache;
// ConPTY FFI boundary (spec WIN-001). The crate-level `deny(unsafe_code)`
// below is lifted only for this module, so every unsafe block in the crate
// lives inside the FFI-boundary files.
#[cfg(windows)]
#[allow(unsafe_code, reason = "WIN-001: ConPTY is a raw Win32 FFI boundary")]
pub mod conpty;
// Clipboard FFI boundary (spec WIN-004).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-004: the clipboard is a raw Win32 FFI boundary"
)]
pub mod clipboard;
// Jump List COM boundary (roadmap v0.2: Windows Jump List).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "roadmap v0.2: the Jump List is a raw Win32/COM FFI boundary"
)]
pub mod jumplist;
/// Keyboard encoding is pure Rust (no FFI): Win32 key events in, VT bytes
/// out (spec WIN-005).
pub mod keyboard;
pub mod pane;
pub mod parser;
// Render hot-path benchmark (NFR-113/NFR-114): tests only, ignored by
// default because Direct2D needs a desktop session.
#[cfg(all(test, windows))]
#[allow(
    unsafe_code,
    reason = "NFR-113: the bench drives the Direct2D/Win32 render hot path"
)]
mod perf_bench;
pub mod portable;
/// Terminal search over screen + scrollback (SPEC SEARCH-001..015).
pub mod search;
pub mod selection;
pub mod session;
pub mod shell;
/// Pane tree for splits (SPEC SPLIT-001..015): pure Rust tree math.
pub mod split;
// Direct2D/DirectWrite renderer FFI boundary (spec WIN-002, WIN-003).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-002/WIN-003: Direct2D/DirectWrite are raw Win32 COM FFI boundaries"
)]
pub mod render;
pub mod tab;
pub mod tabbar;
// UIA-001..005 / WIN-007: the UI Automation COM provider is an FFI boundary.
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-007: the UIA COM provider is a raw Win32 FFI boundary"
)]
pub mod uia;
// THEME-010: reading the system dark/light setting is a Win32 registry call.
#[allow(
    unsafe_code,
    reason = "THEME-010: the system theme check is a Win32 registry FFI boundary"
)]
pub mod theme;
// Win32 window + message loop FFI boundary (spec WIN-001, WIN-005).
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "WIN-001/WIN-005: the Win32 message loop is a raw FFI boundary"
)]
pub mod window;
