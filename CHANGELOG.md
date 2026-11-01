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

## [0.1.0] — v0.1 MVP (planned)

See `ROADMAP.md` for the MVP feature table.
