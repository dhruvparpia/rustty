# Changelog

All notable changes to `rusttyterm` are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Repository scaffold: MIT license, README, CONTRIBUTING, SECURITY, ROADMAP, CI
  (fmt/clippy/test on `windows-latest`), full product spec in `docs/SPEC.md`.
- Tab/grid/shell data model: `App`, `Tab`, `Pane`, `Grid`, `ShellSession`,
  `Config`, `Profile`, `Theme` (spec §32, Step 4).
- TOML config loader with profiles and `predawn` theme preset.
- CLI stub (`rusttyterm` prints version and theme list).
- ConPTY shells in a Win32 window with tabs, clipboard, keyboard encoding
  (WIN-001, WIN-004, WIN-005).
- Direct2D/DirectWrite renderer: damage-only redraw, zero-allocation paint
  hot path, glyph atlas blit cache (WIN-002/003, NFR-113/114, SPEC §20).
- Mouse selection, fullscreen, tab bar driven by config; font/theme wiring
  (WIN-003, TAB-010, CFG/THEME/FONT).
- Portable mode: `config.toml` beside the exe wins over `%APPDATA%`
  (CONFIG-020, PKG-004). `packaging/build-portable.ps1` builds the portable
  ZIP (PKG-001..003); CI uploads it as a build artifact.

## [0.1.0] — v0.1 MVP (planned)

See `ROADMAP.md` for the MVP feature table.
