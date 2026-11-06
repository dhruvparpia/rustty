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
| Launch through Explorer and `Start-Process` | Stays up. Before the fix it panicked on every start from the portable folder |
| Startup | Window in about 62 ms, same as `fix/ci-and-scrollback` |
| Idle CPU over 30 s | 16-47 ms, against 31-94 ms before: no difference beyond noise |
| Memory | About 5-6 MB more, from the embedded font |
| Desktop benches, one at a time | All pass, with 0 allocations in damage frames |
| Snap Layouts flyout, real mouse drag and resize | Not run: needs a person. The window reports `HTMAXBUTTON`, `HTCAPTION` and the resize codes |
| 150% and 200% DPI, mixed DPI | Not run: one 100% display |
| Windows animation effects off | Not run live, to leave system settings alone. Covered by a unit test |
| Windows 10 | Not run: no machine |

## Real Claude Code in the sidebar (STATUS-001, STATUS-002), 2026-11-06

Claude Code 2.1.319 in a scratch folder, in a test window that was not in the foreground.

| Check | Result |
|---|---|
| Trust prompt | Row reads "Trust this folder?" in the Needs you colour |
| Permission prompt | Row reads "Do you want to create a.txt?" in the Needs you colour |
| Working, then Ready, then Finished while unseen | As expected, in an earlier run |
| Text after a link or startup | No longer underlined |
| OSC 7501 reply crossing ConPTY | A probe reads the reply, as key events. The console answers DA1 itself and first, so a program that treats DA1 as "no reply coming" concludes there is no support. Claude Code still shows its title glyph, so it most likely does this. The reports themselves are covered by unit tests only |
| Display after a redraw with a startup notice above the prompt | **Open bug:** some text garbled and a line doubled. Not diagnosed |
| Answering a prompt, then the row clearing | Not confirmed live: the test's Esc did not reach Claude Code. A unit test covers the screen changing |
| Codex, Gemini | Not run. Their questions are not read off the screen |

## Roadmap board, project brain, worktree sessions, 2026-11-06

Driven through the real window against a scratch Git repository.

| Check | Result |
|---|---|
| Create Project Brain | Three files created, message shown. Unit test: existing files are kept |
| Add cards, long title | Cards saved to `roadmap.md`, long title ends in an ellipsis |
| Drag a card to another column | Moves, file updated |
| Enter on a card | Session opens titled with the card, which moves to In progress |
| W on a card | Worktree on a new branch, session opens in it and groups under the project; `git status` does not list `.worktrees` |
| Shift+arrows | Not run live: needs real modifier state. Unit test covers the move |
| Editing `roadmap.md` while the board is open | Not run live. The file is read again before every key |
| A real keyboard | Not run: keys were sent as window messages |

## Restart safety (RESTORE-001..003), 2026-11-06

Driven through the real window, with a copy of `ping.exe` named `claude.exe` as the agent. No real restart was run: the shutdown messages were sent by script and the processes then killed.

| Check | Result |
|---|---|
| Periodic save with an agent running and no shell folder report | `cwd` and `agent = "claude"` in `session.toml` after 40 s |
| `WM_QUERYENDSESSION` | Answers yes and saves at once |
| Hard kill, then relaunch | Row reads "Interrupted. Click to resume claude" |
| Click the row | `claude --continue` runs in the saved folder |
| Real Windows restart, `RegisterApplicationRestart` | Not run |
| Codex, or agents that run as plain `node.exe` | Not run. `node.exe` agents such as Gemini are not detected |
| Interrupted panes with the sidebar hidden | No way to resume them: the offer is only in the sidebar |

## Sessions sidebar (RAIL-001..005), 2026-11-06, Windows 11, 100% DPI

Driven through the real window, with screenshots read by hand. Status was checked with a spinner in the title, `OSC 9` and a finished job, not with a real agent CLI.

| Check | Result |
|---|---|
| New session, switch by row, hide and show, command palette beside the sidebar | Work |
| Under 720 DIP | Sidebar collapses, tab strip returns |
| Fullscreen | Sidebar collapses as soon as it starts and comes back on exit |
| Needs you, Working, Finished, Ready, Background rows | Each shows with the right text |
| Resizing for 4 s with two sessions | Both stay Ready. Before the fix, they showed Working, then Finished |
| Click a background session | Opens under its name, typed text reaches it |
| Click a background session another window controls | Not run live. The decision has a unit test |
| Crab beside the sidebar | Clacks, banner fits |
| Close a session from its glyph | Asks first, then closes, in the real window. The hover highlight is proved by `hovering_a_sidebar_row_paints_only_the_sidebar`, since a synthetic mouse message cannot hold a hover with the real pointer elsewhere |
| Launch through Explorer | Stays up. Started with `explorer.exe <path>`, the shell opens in `system32`, so the project heading reads `system32` |
| `OSC 9;9` directory report with the window in the background | Row stays Ready. A unit test covers the other numbered commands |
| `TERM_PROGRAM` inside WSL (Ubuntu 24.04) | `wsl-sees-rustty` |
| Hide with Ctrl+Shift+S, then save an unrelated config change | Stays hidden |
| `session-rail = false` saved while running | Sidebar goes at once, tab strip returns |
| Working dot | Colour steps through a pulse in the real window |
| CPU over 30 s with one session Working | 63 ms, against 234 ms when each pulse repainted the whole window |
| Bell with one pane | Row flashes: `hovering_a_sidebar_row_paints_only_the_sidebar`. Not rung in the real window |
| Selection and mouse-report drags past the sidebar, folder drop on it | Not run live. `drag_x` has a unit test; the drop change is one condition |
| A real agent CLI (Claude Code, Codex) | Not run: its signals were reproduced by script |
| Startup, 5 launches | Median 62 ms with the sidebar on and off |
| Idle CPU over 30 s | On: 63-78 ms. Off: 31-63 ms. About 0.2% of one core at most, near the noise |
| Sidebar frames | `damage_frames_with_the_sidebar_allocate_nothing`: 0 allocations in 20 frames |
| Desktop benches, one at a time | All pass |
| 150% and 200% DPI, Windows 10, ARM64 run | Not run |
| Caption buttons through MSAA (`OBJID_TITLEBAR`) | Minimize, maximize and close report the rectangles they are drawn and clicked at; context help is invisible. Before the fix they reported a 1-pixel-tall strip |
| Caption buttons through UI Automation | Not exposed: rustty's provider shows tabs and panes only. v0.5.1 with native buttons behaved the same, so this is not new |
| Double-click on maximize | Maximizes, then restores |
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
