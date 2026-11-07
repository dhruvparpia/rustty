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
| Bare program names (SHELL-030) | A planted copy of `hostname.exe` in the current folder doesn't run, with or without `.exe`, while an explicit path still reaches it; relative `PATH` entries skipped; `cmd.exe`, `powershell.exe`, `wsl.exe`, the `winget.exe` App Execution Alias and Git Bash's paths still start (`tests/planted_command.rs`, `program_search::tests`). The test clears `NoDefaultCurrentDirectoryInExePath`, which Claude Code sets and which `CreateProcessW` honours, or the attack doesn't reproduce under it | pass |
| Unit suite | 241 tests (grid, pane, app, config, graphics, parser, CLI, colour, portability) | pass |
| Integration suite | 8 + 4 tests (`tests/`) | pass |
| Damage row masks | damage repaint covers only dirty rows (`damage_frames_cover_only_the_dirty_rows`, desktop) | pass |

## Project secrets (SECRET-001..012)

Automated rows use a fake `op` or stand-in children (`cmd.exe`, `findstr.exe`); none
touches a 1Password account. `op` is on 2.39.0 on this machine.

**What these results do not show (SECRET-012).** Project separation limits mistakes
and casual misuse. It does not stop code running as the same Windows user, which can
name any project. A separate Windows user is the real boundary, and it is not built.

| Area | Coverage | Result |
| --- | --- | --- |
| Manifest, names, references | Literal values refused and never quoted, including through `toml`'s own error text; reserved names; case-only duplicates (`secrets::tests`) | pass |
| Vault naming, project identity | FNV vector, verbatim prefix and case ignored, awkward folder names, subfolder, linked worktree to main checkout, submodule stays separate | pass |
| `LockedSecret` | Page alignment, lock proved with `VirtualUnlock`, ciphertext at rest and after each use, re-encrypted after a panic, nested access refused, redacted `Debug`, working-set quota raised, WER exclusion on this build (`secure_mem::tests`) | pass |
| Token store | Real DPAPI round trip, no plaintext on disk, blob bound to its vault, names cannot leave the folder | pass |
| `setup`, `add`, `list`, `run` logic | Exact `op` arguments, call order, token never in output, files or arguments, value only on stdin, nothing starts on any failed or empty fetch, only the requested names (`secrets::tests::with_locked_memory`) | pass |
| Runner and launcher | Real children: stdin, stdout and stderr pipes, floods without deadlock, oversized output refused, token only in the child, 32-bit exit code, batch files with injection attempt, bare names never from the current directory, only the standard handles inherited (`secret_exec::tests`) | pass |
| Binary | Exit codes and refusals through `rustty.exe` for paths that stop before `op` (`tests/secrets_cli.rs`) | pass |
| Forged `.git` file | One naming a real worktree's folder or one with no back-pointer stays its own project; a worktree made by real Git still resolves (`a_forged_git_file_cant_borrow_another_projects_vault`, `a_worktree_made_by_real_git_still_resolves_to_its_main_checkout`) | pass |
| `add` approval | No means nothing read and exit 13; asked before the first `op` call, naming source and vault; never reached by a refused add (`secrets::tests`, fake dialog). The real box: text read back and No answered (`the_approval_dialog_names_both_ends_and_no_is_no`, ignored, desktop) | pass |
| Real `op` | `op --version` through the production resolver and spawn path (`real_op_version_runs_through_the_production_path`, ignored by default, run by hand) | pass |

Not verified, because no test may touch the owner's real account. To be run by the
owner:

| Step | What it proves | Result |
| --- | --- | --- |
| `rustty secrets setup` | `op vault get/create` and `op service-account create --raw` accept these arguments; `--raw` prints only the token; re-running after a partial failure works (a second account with the same name is untested) | open |
| `rustty secrets add NAME --from op://...` | `op item create -` accepts the JSON template (`LOGIN` with a concealed `password` field, the shape in 1Password's docs; `op item template get` was off limits); the recorded reference resolves | open |
| `rustty secrets run --only NAME -- cmd` | The service-account token reads the value with no approval prompt, and the value reaches the command | open |
| `.cmd` shims through `run` (for example `npm`) | Real shims behave with the quoting; covered only with a test script so far | open |
| Ctrl+C during `run` | The command receives it and its exit code is still returned | open |
| Crash dump inspection | A WER dump leaves out the secret pages; only the registration call is checked | open |

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

## Boss Agent (BOSS-001), 2026-11-06

One real run: Claude Code 2.1.319 as boss in a scratch Git repository, launched from the palette in a test window, told to use one worker for a one-file task. It ran in auto mode, which the test window inherited.

| Check | Result |
|---|---|
| Boss tab opens with the MCP tools | Works. A stock MCP client, Claude Code, listed and called the tools |
| Worker started through `rustty worktree add` and `tab_open` | Worktree `.worktrees/hello` on its own branch, row in the sidebar |
| Worker's trust prompt | Did not appear: the new folder is inside the trusted project |
| Boss watches the worker and reviews its diff | Reported the commit and diff correctly |
| Stops before merging | Asked the person. `master` unchanged, nothing pushed |
| Write token on disk | `mcp.json` holds only `${RUSTTY_AUTOMATION_TOKEN}` |
| Permission prompts in the default mode | Not run. Write tools were approved by auto mode in this run |
| After this run: inline `--mcp-config` and `--append-system-prompt`, `--strict-mcp-config`, `--permission-mode manual`, the Yes/No dialog naming the folder | Not run end to end, since no test may start the real `claude` as a session. Flags checked against `claude --help` 2.1.319, whose help lists `manual` but whose parser also accepts `default`. The command line is parsed back with `CommandLineToArgvW` and is about 2,800 characters with ordinary paths. To be run by the owner |
| A worker asking a permission question, several workers at once, a real merge | Not run |
| Boss after a restart | Not supported: a resumed `claude --continue` has neither the tools nor a token. Start a new boss |

## Shortcuts pressed as keys, 2026-11-06

`tests/keyboard_live.rs` sets Ctrl and Shift in a keyboard state shared only with the test window, then posts the key, so the person's focus is left alone. Earlier tests went through the menu, which is why `Ctrl+Shift+Enter` could be dead from 2026-11-01 until now.

| Check | Result |
|---|---|
| `Ctrl+Shift+Enter` | Splits right. Failed before the fix |
| `Ctrl+Shift+-` | Splits below |
| Dragging a pane's title, posted as mouse messages | Middle swaps, edge docks, click only focuses, Escape cancels. Not yet tried with a physical mouse |
| `rustty <folder>` with a saved session | Saved tab kept, folder's tab in front. Failed before the fix |
| Every other shortcut through this path | Not run. Most built-in shortcuts have never been pressed as keys by a test |
| A person's real keyboard | Not run |

## Skinned dialogs (DIALOG-001), 2026-11-06

`tests/dialog_live.rs` drives real dialogs, posting keys to the focused control so they go through the dialog's own message loop. Screenshots were read by eye in `copper-dark` and `copper-light`.

| Check | Result |
|---|---|
| Enter on a question | Takes the safe answer |
| Tab, then Enter | Takes the button moved to. Fails if `answer_for` ignores the focus |
| Esc from any button, close box, a button's letter | Work |
| Field | Starting text selected, typing replaces it, cancel returns nothing |
| Paints in the theme's colors | Pixels checked in a capture |
| Text at 4.5:1 in every preset | Unit test. It caught `solarized-light`, whose chrome text is too weak, so dialogs fall back to black or white there |
| Layout at 100% to 300% scale | Unit test on the numbers. Not seen on a real high-DPI display |
| A real keyboard and mouse, hover state | Not run: the pointer was left alone |
| A screen reader | Not run. The controls are real, so their names should read |
| High contrast mode | Not handled: dialogs keep the theme's colors. Not run |
| Windows 10 title bar | Not run: it may stay system-colored |

## Boss Agent after the security fixes, 2026-11-06

A second real run, on the final launch: inline config and brief, `--strict-mcp-config`, `--permission-mode manual`, and the folder dialog. Keys were sent as window messages in the order a real keypress produces them.

| Check | Result |
|---|---|
| Folder dialog, then trust prompt | Both shown. The first attempt found a real bug: a palette command that opens a dialog swallowed the next key, here the Enter on the trust prompt. Fixed. There is no dedicated test: this run is the proof |
| Boss in manual mode | Status line reads "manual mode on" |
| `tab_open` permission prompt | Shown before the worker starts. Row reads "Do you want to proceed?" in the Needs you colour |
| Worker and result | Own worktree and branch, commit with only `hello.txt`, boss checked the diff |
| Stops before merging | Did not merge or push, and left the worktree and tab open until told |
| Token scoping on a real window | Dispatcher tests only |
| A worker's permission prompt refused by the server | Dispatcher tests only: this worker was not asked anything, since it ran in the owner's default mode |

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

## Local automation API (AUTO-001..015), 2026-11-06, build 26300

Live and bench runs are at commit 5427954, on top of the COM fix d6e5ee1. The commit
after it only adds this section and removes temp folders the tests left behind.

Live runs are `cargo test --test automation_live -- --ignored --test-threads=1`, ignored
by default because they open a window. Each window runs against a scratch `%APPDATA%`
and `%LOCALAPPDATA%`, so it uses none of the person's settings and discovery can't find
their windows. It starts without activation, and only its own process is stopped.

| Check | Result | How |
| --- | --- | --- |
| Dispatcher, grants, ids, stale targets, rate limit, paging, events | pass | Headless against a real `App` with a fake window (`automation::ops`, `automation::events`) |
| Concurrent clients, a client vanishing mid-request, a flooding client, a client that stops reading, oversized and malformed frames, a squatted pipe name, the wrong process on a pipe, the token over a pipe | pass | Real named pipe with the test thread as the window (`automation::pipe`) |
| MCP over stdio, both eras | pass | The real stdio loop on a fake window (`automation::mcp`), and the real binary with no window (`tests/automation_cli.rs`) |
| CLI parsing and exit codes | pass | Fake window (`automation::ctl`), and the real binary (`tests/automation_cli.rs`). Written alongside the code rather than before it, so checked by mutation: 8 of 8 deliberate breaks caught |
| A boss's whole flow, sidebar on and sidebar off | pass | Live: reads, a background tab, its environment, the token absent, output in order, status changes, scrollback, split, focus, `exit 7`, stale ids, a program that won't start, closing the last tab, the reply still arriving, the discovery file going |
| A command that ends reports its output and exit code | pass | Live: subscribe, `tab.open`, `pane-output`, then `pane-closed` with `exitCode` 3 |
| `rustty mcp` driving a real window | pass | Live: handshake, `tools/list`, `list`, `tab_open`, `pane_send-text`, `pane_read` |
| A wrong token gets no writes or reads | pass | Live |
| Editing the config never grants anything | pass | Live: `editing_the_config_never_turns_writes_on` proves the reload landed by a bigger font changing `cols`, and `reads_need_a_grant_on_a_real_window_and_editing_the_file_doesnt_give_one` |
| Reads need a grant | pass | Dispatcher tests for each grant, and live: `ctl list` and `ctl watch` exit 4 naming the key, `capabilities` stays open |
| A spoofed discovery file gets no token | pass | The real binary against a real attacker's pipe, no window needed (`tests/automation_cli.rs`), and live against a real window. The attacker's pipe received nothing |
| A token reaches only what it opened, and can't answer a permission, trust or auth wait | pass | Dispatcher tests. Not on a real window: its token comes only from Start Boss Agent, which needs the real `claude` |
| Which encoder shows the token | pass | The real `rustty ctl` and `rustty mcp` against a real pipe server (`tests/automation_cli.rs`) |
| Hostile clients | pass | Live: protocol `u32::MAX`, 127 levels of objects, 32 silent connections losing their slots after 5 s. The nesting overflowed the reader's stack in a debug build before the fix |
| `detailSource`, a report hiding a question, cleaned names and captions | pass | Dispatcher, event and pane tests; `detailSource` in the live flow |
| The sidebar heading says so | pass | `PrintWindow` read by eye with `automation-writes = true`: "rustty · automation on" |
| Windows opened through a launcher asking for no activation stay in the background while the launcher's terminal holds the foreground | pass | The flows above, with foreground changes logged. Before: `SetFocus` at start activated them, and the discovery probes opened console windows in the default terminal |
| Session host route | Known exception, not a pass: `--attach --take-control`, `session.input`, `session.terminate` and `host.shutdown` need no grant (AUTO-007), so the grants in this table don't cover host sessions | Not changed, not tested |
| Painting | pass | `PrintWindow` read by eye: sidebar rows, titles, grid text sent through the API, and "Typed by automation" on the row |
| The window never took the foreground | pass | Checked in the flow above and at the capture |
| A shell's exit is noticed (AUTO-015) | pass | Real ConPTY, headless. Failed before the fix, as did ff51b46 and 0e7dae6 under the same launch |

| Bench (release) | Numbers | Result |
| --- | --- | --- |
| `a_full_page_read_stays_well_inside_a_tick` | Worst read 4.2 ms; 9.6 MB of history in 74 pages. Before reads stopped on time: 34.6 ms for one 1 MiB page | pass |
| `starting_the_server_costs_the_window_little` | 1.0 ms worst of 20, on the UI thread at window start | pass |

Every other desktop bench was also run individually on this commit and passes, with
0 allocations in the burst, damage and sidebar frames.

**Not run:**

- `open_boss_tab`, live. It has no caller until the boss exists. Its parts are covered:
  the token over a real pipe, and `add_boss_token` and the environment scrub headless.
- A stock MCP client (Claude Code, Claude Desktop, the inspector). The client side here
  is a test harness speaking the published wire format, in both eras.
- The modern `subscriptions/listen` stream and legacy event notifications against a real
  window: covered against the fake window only.
- `slow-client` against a real window. Covered over a real pipe with a test thread as
  the window, which is the same code on the same side of it.
- A dialog or a drag holding writes back, live. The holding is covered over a real pipe;
  the two conditions in the window that trigger it are not.
- Elevated windows and WSL sessions.
- Linux and arm64 run nothing: they are `cargo check` only.

## Known gaps

- The graphics marquee (progress animation) has no unit test; `a=;` key
  behaviour is covered by parser tests.
- Theme ids at and above `0xF100` collide with the palette-extension range.
- The perf-suite teardown crash came from destroying the window before the
  renderer. The benches now drop the renderer first. `device_loss_recovery_repaints`
  still leaves its window to process exit, so bench tests are run individually.
- Desktop rows marked "open" need the app installed (pwsh 7, Neovim, tmux,
  fzf) — automated fixtures already cover their wire shapes.
