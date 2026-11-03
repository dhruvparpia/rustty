# Validation matrix (issue #36)

What is validated how, on which versions, and with what result. Automated rows
run headless on every `cargo test` (CI-safe); desktop rows need a logged-in
Windows session and are run by hand; bench rows are `#[ignore]`d and run
individually with `cargo test --release -- --ignored --nocapture --exact <name>`.

## Toolchain

For the separate planned desktop platform track, see the
[cross-platform acceptance matrix](validation-cross-platform.md). The historical
Windows results below do not qualify macOS/Linux or the new platform interfaces.

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
clean HEAD); individual runs complete and report.

| Test | Numbers | Result |
| --- | --- | --- |
| `burst_of_10000_lines_paints_inside_the_budget` | 100 frames / 10 000 lines: 21.1 ms wall, 10.2 ms paint, 0.10 ms/frame, worst 0.22 ms, **0 allocations**; atlas 22 tiles, 50 416 glyph blits, 0 fallback runs; present floor 0.11 ms/frame | pass |
| `damage_frames_allocate_nothing` | 20 frames / 2 000 lines: 0.12 ms/frame, worst 0.19 ms, **0 allocations** | pass |
| `damage_frames_cover_only_the_dirty_rows` | one edited line + cursor move → 3 of 24 rows repainted; atlas 23 tiles, 23 rendered | pass |
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

## Known gaps

- The graphics marquee (progress animation) has no unit test; `a=;` key
  behaviour is covered by parser tests.
- Theme ids at and above `0xF100` collide with the palette-extension range.
- The perf-suite process-teardown AV on this machine is pre-existing; bench
  tests are run individually.
- Desktop rows marked "open" need the app installed (pwsh 7, Neovim, tmux,
  fzf) — automated fixtures already cover their wire shapes.
