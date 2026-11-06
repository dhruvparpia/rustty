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

## Project secrets (SECRET-001..012)

Automated rows use a fake `op` or stand-in children (`cmd.exe`, `findstr.exe`); none
touches a 1Password account. `op` is on 2.39.0 on this machine.

| Area | Coverage | Result |
| --- | --- | --- |
| Manifest, names, references | Literal values refused and never quoted, including through `toml`'s own error text; reserved names; case-only duplicates (`secrets::tests`) | pass |
| Vault naming, project identity | FNV vector, verbatim prefix and case ignored, awkward folder names, subfolder, linked worktree to main checkout, submodule stays separate | pass |
| `LockedSecret` | Page alignment, lock proved with `VirtualUnlock`, ciphertext at rest and after each use, re-encrypted after a panic, nested access refused, redacted `Debug`, working-set quota raised, WER exclusion on this build (`secure_mem::tests`) | pass |
| Token store | Real DPAPI round trip, no plaintext on disk, blob bound to its vault, names cannot leave the folder | pass |
| `setup`, `add`, `list`, `run` logic | Exact `op` arguments, call order, token never in output, files or arguments, value only on stdin, nothing starts on any failed or empty fetch, only the requested names (`secrets::tests::with_locked_memory`) | pass |
| Runner and launcher | Real children: stdin, stdout and stderr pipes, floods without deadlock, oversized output refused, token only in the child, 32-bit exit code, batch files with injection attempt, bare names never from the current directory, only the standard handles inherited (`secret_exec::tests`) | pass |
| Binary | Exit codes and refusals through `rustty.exe` for paths that stop before `op` (`tests/secrets_cli.rs`) | pass |
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

## Known gaps

- The graphics marquee (progress animation) has no unit test; `a=;` key
  behaviour is covered by parser tests.
- Theme ids at and above `0xF100` collide with the palette-extension range.
- The perf-suite process-teardown AV on this machine is pre-existing; bench
  tests are run individually.
- Desktop rows marked "open" need the app installed (pwsh 7, Neovim, tmux,
  fzf) — automated fixtures already cover their wire shapes.
