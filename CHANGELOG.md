# Changelog

All notable changes to `rusttyterm` are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - unreleased feature work

Release policy: `v0.5.0` remains immutable; `v0.5.1` is the compatible repair. Feature work targets minors. `packaging/check-release.ps1` checks tag/version/SHA and CI fetches tags and runs it for `v0.5.1`.

### Added

- Local automation API (AUTO-001..015): `rustty ctl <verb>` and `rustty mcp` control a running
  window over a per-window pipe, so a script or a coding agent can start sessions, read their
  output, type into them and see whether they need attention. Reading needs
  `automation-reads = true`, and writing `automation-writes = true`; a window reads both once, when
  it starts, and its token gives a boss session the same reads and writes to the sessions it opened.
  See [the design record](docs/design-automation-api.md).
- Per-project 1Password secrets (SECRET-001..012): `rustty secrets setup`, `add`, `list` and
  `run --only A,B -- cmd` give each project its own small vault and read-only service account, so
  agents stop prompting for approval in every sub-shell. Values are held in locked, encrypted memory
  and the started command's environment block is wiped straight after it starts. This limits damage
  and cannot prevent it: code running as the same Windows user can still reach these secrets, and a
  command given a secret can print it. A separate Windows user for agents is the real boundary, and
  it is not built.
- Copper native dark/light skins, independent UI color overrides, native caption
  Fix: caption uses the window background instead of the tab-bar strip, title icon slots are reserved only when an icon is drawn, and the active-tab marker is 3 DIPs.
  colors and positional active-tab/palette/bookmark indicators. See
  [Copper skin](docs/copper-skin.md) for configuration and Windows validation.
- Session browser slice (#88): palette rows list live and exited host sessions with profile, cwd,
  controller/watcher counts, status, and a disabled reason for exited targets. The palette registry refreshes every ~3 s while open, keeping the selected command and typed query.
- Settings overlay UIA node (#91): `Node::Settings` exposes overlay bounds, selected setting/count and the typed TOML value, with keyboard focus and help text.
- Project task runner slice (#89): `tasks.toml` schema, dependency validation, palette Run/Stop/Restart
  actions, host explicit program/args, and canonical `task#<id> <name>` session labels.
- Palette providers slice (#93): focus pane, open saved workspace, and switch theme rows reuse the
  shared action registry. Settings navigation waits for the #91 editor destination.
- Command timeline slice (#90): palette rows expose retained command ids, exit status and duration; jump/copy/export actions use canonical command ids from the active pane. Finished-command rows also open a search overlay scoped to that command's output range. Durable cross-restart indexing remains later.
- TaskRunner seam (#89 architecture): task lifecycle moved from `window.rs` into a deep `TaskRunner` module with a `HostAdapter` seam, explicit outcomes, and synthetic host tests.

### Changed

- A pane now closes when its program exits even if the pseudo-console keeps its output pipe open,
  as it does on current Windows builds, where `exit` used to leave the pane open and blank.
- The permanent menu bar is gone. Tap Alt to open the same menu as a popup; every menu
  command is also in the command palette, including minimize, maximize, close window, about
  and saving workspaces.
- Tabs now sit in the title bar on one seamless surface, with rustty drawing the minimize,
  maximize and close buttons in theme colors. Dragging, resizing and Snap Layouts stay native.
- The tab strip is quieter: taller, with no divider line, Windows UI font titles, the `+`
  button after the last tab, and no default shell icon. The window has 12 by 10 DIPs of
  padding by default.
- Copper is the default skin: `copper-dark` on a dark system and `copper-light` on a light
  one. An explicit theme in config still wins. The tab strip now matches the terminal
  background unless `tab-bar` is set explicitly.

### Added

- A Boss Agent: "Start Boss Agent" in the command palette opens a Claude Code session that can
  split work across sessions in their own worktrees, watch them, and report back. It stops before
  merging, never pushes, and leaves permission prompts to you. `rustty worktree add NAME` creates a
  worktree and prints its path.
- The sidebar now shows when Claude Code is waiting on you, with its actual question, read from its
  screen. Programs can also report their status directly through OSC 7501.
- Fixed: a window started without activation, such as from a "Run minimized" shortcut, had no
  renderer and could not draw.
- Fixed: `CSI > 4 m`, which Claude Code sends at startup, turned on underline for all later text.
  A private marker now never runs as the plain command.
- Worktree sessions, a project brain and a roadmap board: "New Session in Worktree" gives a session
  its own branch and folder; "Create Project Brain" adds plain files agents can read in
  `.rustty/brain`; Ctrl+Shift+R shows `roadmap.md` as a board where each card can open a session.
- Restart safety: the session is saved when Windows shuts down and every 30 seconds, and panes that
  were running Claude Code or Codex come back as Interrupted, with a click to resume. Sidebar
  projects now follow an agent's real folder, even when the shell reports none.
- A sessions sidebar, on by default: every session grouped by project with what it is doing
  (needs you, working, finished, ready, exited), plus background host sessions you can click to
  open and type in (read-only if another window controls them). It reads signals agent CLIs already send. Ctrl+Shift+S or `session-rail = false` turns it
  off, which brings the tab strip back. Closing a session from its row asks first when it is
  working or waiting for you, or when the profile asks.
- `TERM_PROGRAM`, `TERM_PROGRAM_VERSION` and `COLORTERM` also reach WSL shells, through `WSLENV`.
- Shells see `TERM_PROGRAM=rustty`, `TERM_PROGRAM_VERSION` and `COLORTERM=truecolor`.
- A welcome banner when rustty starts with no other rustty window open: your crab snaps its
  claws twice beside the session status, key commands and a tip. `welcome = false` removes it.
- Shells started by rustty get its folder at the end of their PATH, so `rustty` commands
  work inside them with a portable install. Nothing persistent changes.
- Symbols Nerd Font Mono v3.5.1 is embedded as a fallback, so prompt icons work with no
  setup. It adds about 2.6 MB to the executable. A chosen font that has the glyph keeps it.

### Fixed

- A normal launch no longer crashes. After a GUI launch released its console, printing any
  diagnostic could panic, or land in whatever now held that handle number. A portable install
  hit this on every start through the jump list error.
- Bold text is bold and italic text is italic. They were swapped.
- The window is sized so the configured columns and rows fit inside the padding.
- Words are evenly spaced. The grid now matches the font's own advance instead of rounding
  it up, which stretched the gaps between words.
- Block characters and Powerline separators are drawn as geometry, so rows and prompt
  segments join with no gaps at any line height.
- Theme palettes now colour ANSI output, so each theme shows its own 16 colors. Text
  already on screen keeps its colors after a theme switch, and a session host's snapshot
  of existing text uses the default palette.
- Reading scrollback: the view now keeps its text while output arrives, and line
  edits (erase, insert, delete) apply to the live screen, never the history on screen.
  Images placed while scrolled back anchor to the live cursor.
- Finished commands survive full-screen apps: alt-screen output and `CSI 3J` no
  longer retire main-screen command records.
- `ESC c` and compound modes such as `CSI ?1049;25h` give the same result however the
  output is chunked.
- A full-screen `DECSTBM` such as `CSI r` no longer pins the scroll region across a
  window grow, and line feed and cursor down below a region move down instead of up.
- Search results refresh when a line edit changes existing text.
- Tab icons and close controls are painted separately from Unicode titles.
- Windows task-host adapter is gated to Windows so portable core checks compile.
- Task runs now bootstrap the host when absent, use host-side task identity (`taskId`), launch transitive dependencies topologically, and idempotently return the same live run on concurrent create (#102–#105).
- Host snapshots now serialize wide-cell text without continuation spaces and keep combining marks (#106).
- Search resumes completed history during ordinary output and maps text offsets back to wide/combining grid columns (#107, #113).
- Console attach tracks live mouse mode changes from output frames and resync snapshots (#108).
- Wide-cell overwrite repair, no-wrap right-margin cursor handling, combining-mark dirty/revision bumps and right-margin attachment are covered by parser tests (#109–#112).
- Renderer fallback encodes cell text directly into the reused UTF-16 buffer instead of allocating a String per Unicode cell (#114).
- Palette construction reuses one session snapshot for browser and task rows instead of one session-list query per task (#115).
- Worktree discovery slice (#92): existing Git worktrees appear with branch/path/detached/available state and open as tabs at their roots. Task runs now carry a project root namespace, so same-name tasks in two worktrees stay isolated. Relative task `cwd` is resolved against the project/worktree root.
- Settings provider slice (#91): palette rows expose global/profile setting values, source and restart flags, plus keybinding conflict diagnostics. Settings rows open an in-app overlay: Up/Down select, type a TOML value, Enter applies plain, table and profile scalar keys atomically while preserving comments. An external config edit is detected by mtime, reloaded, and Enter refuses the stale write. Keybinding rows still open the native Preferences editor.

## [0.5.1] - 2026-11-03

Compatible repair line; `v0.5.0` remains immutable and is not moved.


### Added

- Large-output slice 2 (#53): search result context lines (`[search] context-lines`),
  a bookmark overlay anchored to absolute history lines with eviction marking,
  and streaming selection/last-command export through a native Save As dialog.

### Fixed

- Attach snapshots now restore terminal modes, the active pen, partial UTF-8 bytes
  and both screens; resizing in the alternate screen also resizes the saved main
  screen.
- GUI Take Control sends `readOnly:false`; console/GUI resync discards frames already
  in the installed snapshot and applies newer output/resize frames once.
- Diagnostic references use character columns for Python tracebacks, keep quoted
  editor paths as one token, and map WSL `/mnt/c` cwd to `C:\` before joining relative
  paths.
- Clear scrollback now preserves the live viewport's absolute line ids.
- Workspace mutation refuses an unreadable store instead of replacing it with an empty one.
- Windows argv quoting preserves empty args, embedded quotes/backslashes and trailing slashes.
- OSC 7 scripts emit valid percent-encoded file URIs with the hostname/path separator.
- Clipboard writes open with the owning window HWND.
- Palette fuzzy matching advances on Unicode boundaries; command/bookmark anchors use the
  live screen, not the scrolled viewport.
- Console attach replays initial host stream frames before input, shares one frame-write lock,
  keeps UTF-8 surrogate pairs and repeated key counts, and leaves the alternate screen on detach.
- Host stream notifications are buffered by request id; identity persistence is serialized and
  rolls back on a save failure.
- Raw input frames are capped at 24 KiB so hex/JSON stays below the host request cap.
- Search restarts when existing line/cell content is overwritten instead of returning stale matches.
- Wide characters occupy two columns; combining marks attach to the base cell and continuation
  columns are skipped by text snapshots.

## [0.5.0] - 2026-11-02

The v0.2–v0.5 features accumulated here (those milestones were tagged as
plain commits); this is the first release whose tag and `Cargo.toml`
version agree.

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
- `theme = "..."` now also names a theme file: `themes/<name>.toml` beside the
  config is loaded, and `[theme]` is a table of per-colour overrides
  (`background`, `foreground`, `cursor`, `selection`, `search-highlight`,
  `tab-bar`, `tab-active`, `split-border`, `split-focus`, `palette`) applied on
  top of it (CONFIG-003, THEME-009). An unknown key in a theme file, a palette
  that is not 16 colors, and a color that is not `#RRGGBB` are reported on
  stderr and skipped, keeping the color that already worked (THEME-013/014).

- `[split] padding` is drawn: pane content is offset by the configured pixel
  gap, so the border sits inside it (SPLIT-014, CONFIG-013).
- Per-profile `font`, `font-size` and `line-height` are applied: the renderer is
  rebuilt and every tab is resized to the grid the window fits with the new
  metrics — at startup, on tab creation, and on profile cycling (SHELL-019).
- OSC 7 working-directory reporting: with `tab-bar.show-cwd` (TAB-025) a tab whose
  shell sets no title shows the last directory of the path the shell reported,
  percent-decoded and drive-path aware.
- OSC 133 prompt markers: a pane knows when its shell is at a prompt, when a command
  is running, and the exit code of the last command.
- Close confirmations (TAB-018, SPLIT-006, WINDOW-014): `confirm-close` in
  `[[profile]]`. Closing a tab whose shell is busy, closing a split pane whose shell
  runs, closing the last tab, and quitting ask in a native dialog; the busy wording
  comes from the OSC 133 state.
- Bell: `BEL` is decoded and announced. `bell = "silent" | "visual" | "audible" |
  "both"` picks the mode (BELL-002/006/007/008); the visual bell flashes the tabs
  that rang and the focused pane's border for 300 ms (BELL-003/004), the audible
  bell plays the system bell (BELL-008), and a bell in a background window raises a
  Windows toast attributed to rusttyterm by its AUMID (BELL-005). Ringing never
  blocks the PTY read: the sound call returns straight away (BELL-009). Adds the
  `windows` `UI_Notifications` and `Data_Xml_Dom` features.
- System dark/light detection (THEME-010): with no `theme` key the app reads the
  Windows `AppsUseLightTheme` registry value — light mode gets the new
  `solarized-light` preset, dark mode gets `predawn`. A `theme` in the config, and
  a per-profile theme, both win over the system. Adds the `windows`
  `Win32_System_Registry` feature.
- Config hot reload (CONFIG-018): the app notices `config.toml` changing and
  applies theme (including the system dark/light choice), colours, fonts, window
  padding, split border and padding, tab bar placement and labels, cursor shape
  and blink, opacity, key bindings, bell mode, search defaults and the profile
  list — without restarting any shell. A config that no longer parses is reported
  on stderr and the running one is kept. `scrollback` and `starting-profile`
  only reach the next tab.
- Session restore (TAB-026, WINDOW-015/016/017): the run's tabs, split
  layouts, titles, focused panes and each pane's last OSC 7 directory are
  written to `session.toml` beside `config.toml` on exit, and respawned at
  startup — shells start in their saved directories (no reattach, per the
  roadmap). The window position and size are saved too and re-applied when
  they sit on a live monitor. Opt out with `session-restore = false`. A
  session file that does not parse is reported and ignored.
- Windows Jump List (roadmap v0.2, native taskbar API): the taskbar icon's
  right-click menu gets "New window" plus one "New <profile> window" task per
  configured profile. Built in memory through `ICustomDestinationList`
  (no hand-written `.lnk` files); failures are reported on stderr and never
  block startup.
- Real CLI parsing: `--profile <name>` picks the starting profile (what the
  Jump List tasks launch with), `--theme <name>` overrides the configured
  theme, `-h`/`--help` prints usage, `--version` prints the version.
  Unknown arguments are reported and ignored. No new dependency.
- Quick terminal (roadmap v0.2 "Hotkey summon from screen edge"): an opt-in
  `[quick-terminal]` section turns a system-wide hotkey (default
  `ctrl+grave`, registered with `RegisterHotKey`) into a frameless drop-down
  shell. It spans the monitor's work-area width, is `height` rows tall, docks
  flush to `edge = "top" | "bottom"`, and hides (never closes) on the hotkey
  again, on `close-quick` (a `[keybindings]` action), and — with
  `autohide = true` (the default) — when it loses focus. Its shell survives
  every dismissal. It off-lists the taskbar (`WS_EX_TOOLWINDOW`), stays
  topmost, and is off by default because a global hotkey is invasive. A hotkey
  another app grabbed is reported on stderr; the terminal still starts.
- Theme preset browser, live switching (roadmap v0.2): the View→Themes menu
  now lists every built-in preset *and* every theme file (`*.toml`) in the
  theme directory, checks the active theme, and applies the pick immediately.
  A theme file that fails to parse is reported and falls back to predawn.
- Documented skip: per-pane Gaussian background blur (the "acrylic" style)
  is not on the v0.2 roadmap; `opacity` (WINDOW-010) covers translucency.
- IME composition (WIN-006, KEY-026, roadmap v0.4): the IME composition
  window is positioned at the caret (`ImmSetCompositionWindow`), the
  committed string is read from `GCS_RESULTSTR` and sent to the shell, the
  rename buffer, or the search query, `ImmNotifyIME` completes the
  composition, and `WM_IME_CHAR` is handled as a fallback commit path.
  UTF-16 surrogate pairs decode to astral characters; unpaired surrogates
  are dropped, so a partial string never sends garbage (no shell
  corruption).
- Font ligatures (FONT-010/011, RENDER-033/034; v0.2 scope, landed in the
  v0.4 parity sweep): ligature text
  is shaped as whole runs through DirectWrite with `calt`/`liga`/`clig`
  requested explicitly, so programming ligatures (`=>`, `!=`, `fi`, ...)
  render. The per-cell glyph atlas cannot form a ligature, so it is bypassed
  while ligatures are on; `ligatures = false` goes back to atlas blits with
  the ligature features switched off. `font-features` now accepts `calt 0`,
  `calt=0` and comma lists (`calt=0,liga=0`, `calt,liga`), and is applied
  after the `ligatures` switch so a hand-written tag wins.
- UI Automation provider (WIN-007, UIA-001..005, ACCESS-002..005): screen
  readers see the window (name, version), the tab bar with one tab element
  per tab (name, selected, runtime id), one grid per pane exposing the
  visible text plus the last 500 scrollback lines, and the search strip
  while it is open. `WM_GETOBJECT` answers `UiaRootObjectId`; the tree is
  rebuilt live on every call, so no cached state can drift from the frames.
- Key-binding parity (KEY-025, a11y "adjustable font size", CONFIG-018):
  new `[keybindings]` actions `increase-font-size`, `decrease-font-size`,
  `reset-font-size`, `reload-config` and `clear-screen` (Ghostty's own
  spellings work too), and built-in `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0`
  font zoom. Zoom is clamped to the configured font-size bounds and a
  refused font leaves the window unchanged, the same rule as profile fonts.
- Kitty graphics protocol subset (spec "graphics protocols", roadmap v0.5):
  `ESC _ G` / `ESC P G` /
  `0x90 G` strings with transmit (`a=t`, chunked via `m=1/2`), placement
  (`a=p`, cursor-anchored, transient `t=1` placements drop when they scroll
  out of the viewport), delete (`a=d`, by id or all), and capability queries
  (`a=I`/`a=Q`) answered with `OK` / `IMGNOTFOUND`. PNG/JPEG payloads are
  decoded through WIC and cached per image id; `f=24`/`f=32` raw pixels need
  `w=`/`h=`. Images draw above the text, scroll with the viewport, and are
  cleared when a pane enters/leaves the alt screen or clears scrollback.
  Out of scope: compression, file storage, animation, cropping, z-index.
- 6-bit SGR graphics (the block-character "sixel" look from `ls --color`
  charts and ANSI art) needed no work: SGR 256-colour cells and the Unicode
  block characters already render, so those programs just work.
- Design-phase documents for the last v0.5 ecosystem items:
  `docs/design-plugins-mcp.md` (plugins as out-of-process MCP servers with
  per-capability grants) and `docs/design-mobile-tablet.md` (adaptive
  touch input for Windows on ARM). Design only; no implementation
  scheduled.

### Fixed

- The v0.1 "CLI stub" never parsed arguments, and a later commit message
  claimed `--theme`/`--profile` that were never implemented. Both flags now
  exist (see Added). `--config` deliberately does not: CONFIG-021 (config
  under `%APPDATA%`, or beside the exe in portable mode) and CONFIG-022
  (no admin) are what the spec asks for, and they work.
- `[[profile]]` entries in the config were rejected: the parser read them from
  a `profiles` key while `docs/SPEC.md` spells it `profile`. The spec spelling
  is now the key, and `profiles` is accepted as an alias.
- A key binding that used the Win/Super key was accepted and then never fired;
  it is now a startup error, because the dispatcher matches Ctrl/Shift/Alt.
- `config.example.toml` is now covered by a test, so the documented keys and
  the parser cannot drift apart.
- Close confirmations were labelled `TAB-014`/`TAB-021` in code and docs. The spec
  spells tab close confirmation `TAB-018`, pane close `SPLIT-006` and window close
  `WINDOW-014` (`TAB-014` is tab colours, `TAB-021` is tab-bar theming). The
  `confirm-new-process` key went with them: nothing ever prompted on it and no
  requirement asks for it.

## [0.1.0] — v0.1 MVP (planned)

See `ROADMAP.md` for the MVP feature table.
