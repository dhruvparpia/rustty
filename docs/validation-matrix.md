# Validation matrix (issue #36)

What is validated how, on which versions, and with what result. Automated rows
run headless on every `cargo test` (CI-safe); desktop rows need a logged-in
Windows session and are run by hand; bench rows are `#[ignore]`d and run
individually with `cargo test --release -- --ignored --nocapture --exact <name>`.

## Toolchain

| Component | Version |
| --- | --- |
| rustc (stable) | 1.99.1 (2026-10-13), MSRV 1.99, edition 2024 |
| Target checks | `x86_64-pc-windows-msvc` (build + test), `aarch64-pc-windows-msvc` (check), `x86_64-unknown-linux-gnu` (check) |
| OS of the desktop runs | Windows 11 Home, Build 26300 |
| Shipped deps | latest stable at each dependency touch (see commit messages); dev-only `windows` `Win32_Storage_Xps` feature (0.62.2) exists solely for the bench's `PrintWindow` capture |

## Automated (headless, every `cargo test`)

| Area | Coverage | Result |
| --- | --- | --- |
| Parser wire shapes | 19 protocol fixtures (CSI, OSC BEL/ST, OSC 7, OSC 133, DSR/DA, 1049 alt screen, 25 cursor, DECSTBM, SGR, save/restore, UTF-8 + CJK + emoji, kitty APC/DCS/BEL, ignored DCS/APC/PM, controls, mouse modes) each fed whole, in every chunk size 1..len, and split at every byte offset; events, cursor and grid compared to the whole-feed run (`protocol_fixtures_survive_every_chunk_boundary`) | pass |
| Unit suite | 241 tests (grid, pane, app, config, graphics, parser, CLI, colour, portability) | pass |
| Integration suite | 8 + 4 tests (`tests/`) | pass |
| Damage row masks | damage repaint covers only dirty rows (`damage_frames_cover_only_the_dirty_rows`, desktop) | pass |

## Desktop benches (this machine, run individually)

Suite runs AV at process teardown on this machine (pre-existing, present at
clean HEAD); individual runs complete and report. Re-run on the available desktop
on 2026-11-03: burst, allocations and dirty-row runs printed their results before the
pre-existing teardown AV; device-loss and full-vs-damage comparisons exited clean.

| Test | Numbers | Result |
| --- | --- | --- |
| `burst_of_10000_lines_paints_inside_the_budget` | 100 frames / 10 000 lines: 22.2 ms wall, 8.5 ms paint, 0.09 ms/frame, worst 0.57 ms, **0 allocations**; atlas 21 tiles, 41 216 glyph blits, 0 fallback runs; present floor 0.14 ms/frame | pass |
| `damage_frames_allocate_nothing` | 20 frames / 2 000 lines: 0.19 ms/frame, worst 0.51 ms, **0 allocations**; atlas 21 tiles, 8 073 glyph blits, 0 fallback runs | pass |
| `damage_frames_cover_only_the_dirty_rows` | one edited line + cursor move → 3 of 24 rows repainted; atlas 22 tiles, 22 rendered | pass |
| `device_loss_recovery_repaints` | recreation forces a full repaint; atlas rebuilt; recovery frame blits cached glyphs | pass |
| `damage_repaint_matches_full_paint` | damage frame captured via `PrintWindow(PW_RENDERFULLCONTENT)` is pixel-identical to the full repaint of the same content | pass |

## Scenario matrix

| Scenario | Shell/app version | Automated | Desktop |
| --- | --- | --- | --- |
| PowerShell (in-box) | Windows PowerShell 5.1.26100.9711 | fixture corpus (title, resize, DSR, modes) | dogfooding (debug build) |
| PowerShell 7 | not installed on this machine; `winget install Microsoft.PowerShell` is the follow-up that kills the tab-title mojibake at source | fixture corpus | open |
| Git Bash | Git for Windows 2.49.0 | fixture corpus (OSC 0/2, 1049, UTF-8) | dogfooding |
| WSL | WSL 2.4.12.0 (Ubuntu-24.04, Ubuntu, docker-desktop) | fixture corpus (UTF-8/CJK/emoji, OSC 7, 133) | dogfooding |
| SSH client | OpenSSH_for_Windows_9.5p2 (LibreSSL 3.8.2) | fixture corpus (chunk-boundary splits model slow links) | dogfooding |
| Neovim | not installed on this machine | 1049/1000/1006, DECSTBM, SGR fixtures | open |
| tmux | not installed on this machine | 1049, SGR, mouse fixtures | open |
| fzf | not installed on this machine | 1049, mouse-mode fixtures | open |
| Agent CLIs (TUI over pipes) | n/a | full fixture corpus at every split | dogfooding |
| UTF-8 / CJK / emoji | n/a | fixtures: whole, every chunk size, every split | live typing dogfood |
| Resize (rows/cols, DPI) | Build 26300, per-monitor DPI | grid/pane resize tests | live resize dogfood |
| Sleep / wake, GPU/device recovery | n/a | — | `device_loss_recovery_repaints`, `damage_repaint_matches_full_paint` |

## Native look and startup crab (2026-11-06, build 26300, one 100% display)

Driven through the real window, with screenshots read by hand.

| Check | Result |
|---|---|
| cmd, Windows PowerShell, Git Bash, WSL | Banner appears, claws move, prompt sits below it, `rustty --version` (`rustty.exe` under WSL) works |
| pwsh 7 | Not run: not installed |
| Second window while one is open | No crab. Closing all windows brings it back |
| `welcome = false`, 60 columns, `copper-light` | No banner; text without crab; readable |
| Live host session | Status shows `session host running · 1 session` |
| Hit tests | Tab, `+`, empty strip, top edge, min, max, close report correctly; the corner of a maximized window is close |
| Caption button messages | Maximize, restore and minimize work; press and release on different buttons does nothing; close reaches the WINDOW-014 prompt, as before |
| Many tabs, fullscreen and back, Alt popup | Work |
| Startup | Window in about 62 ms, same as `fix/ci-and-scrollback` |
| Idle CPU over 30 s | 16-47 ms, against 31-94 ms before: no difference beyond noise |
| Memory | About 5-6 MB more, from the embedded font |
| Desktop benches, one at a time | All pass, with 0 allocations in damage frames |
| Snap Layouts flyout, real mouse drag and resize | Not run: needs a person. The window reports `HTMAXBUTTON`, `HTCAPTION` and the resize codes |
| 150% and 200% DPI, mixed DPI | Not run: one 100% display |
| Windows animation effects off | Not run live, to leave system settings alone. Covered by a unit test |
| Windows 10 | Not run: no machine |
| Narrator on the drawn caption | Not run |

## Known gaps

- The graphics marquee (progress animation) has no unit test; `a=;` key
  behaviour is covered by parser tests.
- Theme ids at and above `0xF100` collide with the palette-extension range.
- The perf-suite teardown crash came from destroying the window before the
  renderer. The benches now drop the renderer first. `device_loss_recovery_repaints`
  still leaves its window to process exit, so bench tests are run individually.
- Desktop rows marked "open" need the app installed (pwsh 7, Neovim, tmux,
  fzf) — automated fixtures already cover their wire shapes.
