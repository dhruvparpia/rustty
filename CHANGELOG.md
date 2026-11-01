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
- Split panes: pane tree, vertical/horizontal splits, pane focus and ratio
  keys, mouse routing, themeable borders and pane titles, shell-death
  cascade (SPLIT-001..015).
- Search: Ctrl+F overlay; plain, whole-word and regex search over the active
  tab's screen and scrollback; match highlights, next/previous navigation
  that scrolls the viewport, case/word/regex toggles, and a capped result
  list (SEARCH-001..015). Adds `regex` 1.13.1.
- Native Win32 menus: File/Edit/View/Window/Help with New Tab, Close Tab,
  built-in shell/profile selector, theme selector, split commands, copy/paste,
  search, window controls, Preferences, About, and visible shortcuts
  (MENU-001..015).
- Mouse reporting: `1000`/`1002`/`1003` click, drag, and motion modes; SGR and
  urxvt coordinate encodings; press, release, drag, scroll, right-click context
  menu, and middle-click reporting routing (MOUSE-006..013, PARSER-026,
  TTY-020).
- Shell profiles: the built-in shells (cmd, PowerShell, pwsh, Git Bash, WSL)
  merged with `[[profile]]` entries, a profile selector in the Window menu and
  on the menu bar, `starting-profile`, and per-profile command, args, working
  directory, environment, theme, tab colour and scrollback (SHELL-017, SHELL-019,
  SHELL-021, TAB-011/015, CONFIG-023..028). The working directory and the
  environment reach `CreateProcessW`, so the shell starts where the profile says
  and the profile's variables replace the inherited ones.
- Tab bar behaviour: strip at the top or bottom (TAB-022), `always`/`auto`/
  `hidden` visibility that re-flows the grid and the ConPTY (TAB-023), the hover
  close button, drag-to-reorder (TAB-016), keyboard reorder
  (`Ctrl+Shift+PageUp`/`PageDown`, TAB-017), inline rename (TAB-013,
  `Ctrl+Shift+R`), profile prefix (TAB-024) and per-tab colour (TAB-014).
- Custom key bindings: `[keybindings]` maps `Ctrl+Shift+…`-style specs to
  actions (tabs, panes, split, search, copy/paste, profile and theme cycling,
  window); a user binding replaces the built-in one (CONFIG-007).
- Window and cursor config: `opacity` through a layered window (CONFIG-011),
  `cursor-shape` (CONFIG-010) and `cursor-blink` (CONFIG-009), `copy-on-select`
  (CONFIG-008), the `[search]` option defaults applied when the strip opens,
  `[split] border` (CONFIG-013), `[window.padding]` — the grid is inset by it
  while the tab bar stays full width (WINDOW-011) — and `[tab-bar]` placement,
  `show-profiles` and `close-button` (CONFIG-012).

### Fixed
- `[[profile]]` entries in the config were rejected: the parser read them from
  a `profiles` key while `docs/SPEC.md` spells it `profile`. The spec spelling
  is now the key, and `profiles` is accepted as an alias.
- A key binding that used the Win/Super key was accepted and then never fired;
  it is now a startup error, because the dispatcher matches Ctrl/Shift/Alt.
- `config.example.toml` is now covered by a test, so the documented keys and
  the parser cannot drift apart.

## [0.1.0] — v0.1 MVP (planned)

See `ROADMAP.md` for the MVP feature table.
