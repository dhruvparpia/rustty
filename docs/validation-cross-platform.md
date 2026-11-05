# Cross-platform acceptance and traceability matrix

Contract: [Cross-platform SRS](srs-cross-platform.md), incorporated in SPEC §9.33.
All scenarios below are **not run** for this new platform contract. Existing
[Windows validation](validation-matrix.md) is historical evidence for its own
recorded commits; it does not establish these new gates.

## Required record format

Each execution records scenario/requirement IDs, commit and artifact SHA-256,
OS/version/architecture, hardware, display server/compositor, shell/application
versions, steps, expected/actual result, evidence link, date, tester and status
(`pass`, `fail`, `blocked`, `not run`). State the evidence class: static inspection,
cross-compile, simulated transport, native headless integration or real desktop.
Repeat desktop scenarios on every declared supported matrix entry; unsupported
combinations are explicitly listed, never silently counted as passing.

## Scenarios

| ID | Requirements | Reproduction and pass criterion | Gate / issue |
|---|---|---|---|
| CP-A01 | CP-ARCH-001, CP-ARCH-002 | Inspect shared API/dependency graph and exercise each backend boundary. No OS handles/events leak into shared contracts; ownership/cancellation/errors have concrete Windows and Unix callers. | G1 / #96 |
| CP-A02 | CP-ARCH-003, CP-QUAL-001 | Run required gates and common parser/action fixtures on declared targets; perform Windows input/render/session regression suite before/after extraction. Record failures separately from pre-existing issues. | G1 / #96; G4/G5 |
| CP-A03 | CP-ARCH-004 | Compare candidate frontends on one fixed text/input workload and record typography, accessibility feasibility, latency and resource results. ADR selects a justified backend without assuming Windows replacement. | G3 / #98 |
| CP-A04 | CP-DATA-001, CP-DATA-004, CP-PTY-001 | Spawn with Unicode/spaces/empty args, env overrides and cwd; inspect received values, TERM behavior and controlling TTY. Test missing executable/cwd and unrepresentable OS-native values. Values are exact or explicitly refused; no shell substitution/leaks. | G2 / #97 |
| CP-A05 | CP-DATA-002, CP-DATA-003 | Run two portable namespaces, default installation and read-only installation; inject unreadable/newer store, interrupted writes and disk-full. Namespaces remain isolated; data/IDs preserved; failures actionable. | G2 / #97 |
| CP-A06 | CP-PTY-002 | In a real shell run foreground/background jobs, Ctrl-C/Ctrl-Z/fg, an editor and alternate-screen app; resize repeatedly. Correct job receives input/resize; geometry and main-screen state survive. | G2 / #97 |
| CP-A07 | CP-PTY-003 | Child writes a final marker then exits; repeat with fragmented reads, a grandchild holding output open and PTY hangup/error variants. Marker drains before completion when available; timeout marks incomplete output; children reaped and descriptors bounded. | G2 / #97 |
| CP-A08 | CP-PTY-004, CP-PTY-005 | Terminate cooperative and unresponsive process groups; include an intentionally escaping descendant and an unrelated control process. Graceful/forced bounds and escape limitations match ADR; control process survives. Crash clients and then host; distinguish persistent survival, ephemeral cleanup and host-loss failure. | G2 / #97 |
| CP-A09 | CP-HOST-001, CP-HOST-002 | Race two host launches; attempt connection from another user/privilege context and hostile/stale endpoint paths. Exactly one authorized host wins; live/unrelated endpoints are untouched; unauthorized peers denied before mutation. | G2 / #97 |
| CP-A10 | CP-HOST-003, CP-HOST-004, CP-QUAL-006 | Exercise list/create/info/terminate with live, stale and invalid targets. Test current/previous compatible peers plus unsupported versions and newer files. Stable JSON/exit codes; no GUI or host replacement on failure; files retained. | G2 / #97; G4/G5 |
| CP-A11 | CP-HOST-005 | Flood output while a client stops reading; concurrently send input/resync and paste across request-size boundaries. Verify frame integrity, ordered bytes or explicit rejection, bounded queues and continued service to another client; cancellation completes within documented bound. | G2 / #97 |
| CP-A12 | CP-ATTACH-001, CP-ATTACH-002 | Attach mid-UTF-8/escape sequence and in alternate screen; race snapshot with output/resize, inject a revision gap, take control between two clients with watcher. Correct state installed once, resync explicit, one reply authority, watchers cannot write, stale IDs cannot redirect. | G2 / #97 |
| CP-A13 | CP-ATTACH-003, CP-ATTACH-004 | Snapshot caller stty/screen state; detach normally, fail setup after partial raw-mode entry, send handled signals, suspend/resume and resize with Unicode/paste/mouse. Caller restored and no duplicate input; uncatchable-signal limits documented. | G2 / #97 |
| CP-A14 | CP-UI-001, CP-UI-005 | On desktop open shell, tabs/splits and existing workspace; type, select, search, bookmark/export, detach/reattach and close views. Correct shared target/actions and no accidental termination or leaked UI-navigation keys. | G3 subset / #98; G4 / #99; G5 / #100 |
| CP-A15 | CP-UI-002 | Render combining/wide/emoji/fallback/ligature cases, select/hit-test them, change DPI/monitor/font, compare damaged/full repaint and simulate device loss. Cell positions/content agree; recovery repaints correctly. | G3 / #98; G5 / #100 |
| CP-A16 | CP-UI-003 | Use multiple real layouts, dead keys, IME preedit/commit/cancel, AltGr/Option and shortcut conflicts across focus changes. Committed text arrives exactly once; preedit/navigation do not leak. | G3 / #98; G5 / #100 |
| CP-A17 | CP-UI-004 | Copy/paste with clipboard contention/unavailability, Unicode and large multiline input. Terminal remains responsive; bytes are delivered correctly or explicit error is shown; managed session remains connected. | G3 / #98; G5 / #100 |
| CP-A18 | CP-UI-006 | Use VoiceOver on macOS and a supported Linux screen reader for text/selection, focus, pane/tab names and notifications. Complete keyboard-only shell/output workflows; record actual tool versions and results. | G4 / #99; G5 / #100 |
| CP-A19 | CP-UI-007, CP-MAC-001 | Launch from Finder/CLI with differing environments; verify default/login/custom shells, menus, activation/quit, notifications and file/URL/editor opening. Profile overrides honored, arguments intact and ownership semantics explicit. Repeat applicable services on Linux. | G4 / #99; G5 / #100 |
| CP-A20 | CP-MAC-002, CP-QUAL-005 | On each supported Mac architecture install signed/notarized artifact on a clean machine, relocate bundle, invoke CLI/host and upgrade with live sessions. Identity/hash/signatures match; no forced host replacement or data loss; document uninstall/portable behavior. | G4 / #99 |
| CP-A21 | CP-LINUX-001, CP-LINUX-002, CP-LINUX-003, CP-QUAL-005 | On declared Linux matrix install package, exercise XDG paths/desktop launch, clipboard/primary selection, IME, accessibility, fractional scaling and compositor-limited actions separately under Wayland/X11. Unsupported capabilities are explained; upgrade/uninstall preserves user state. | G5 / #100 |
| CP-A22 | CP-QUAL-002 | Run supported shells plus SSH/tmux/Neovim/fzf/coding-agent CLI through input, resize, colors, full-screen and paste scenarios. Record actual versions and observed behavior, not only parser fixtures. | G4 / #99; G5 / #100 |
| CP-A23 | CP-QUAL-003 | Measure SRS budgets under the documented hardware/workload protocol, including renderer allocations, latency, cold/warm startup and combined host/app resources. Record all results and resolve failures before qualification. | G4 / #99; G5 / #100 |
| CP-A24 | CP-QUAL-004 | Run 24-hour sustained/mixed output and repeated lifecycle cycles with stalled clients and persistence failures; sample memory/descriptors/processes. No unbounded growth or unresolved release-blocking loss/ownership/availability failure. | G4 / #99; G5 / #100 |

## Gate closure

- G1: CP-A01/A02 and D1 ADR, with Windows desktop evidence.
- G2: CP-A04–A13 on macOS and Linux, plus D2/D3 ADRs.
- G3: CP-A03, CP-A14 vertical-slice subset, CP-A15–A17 on macOS, plus D4 ADR.
- G4: full applicable CP-A02/A10/A14–A20/A22–A24, retained G2 evidence and D5.
- G5: full applicable CP-A02/A10/A14–A19/A21–A24, retained G2 evidence and D6.

Changed code invalidates affected evidence and requires rerunning relevant rows.
No gate closure follows merely from a linked issue being closed. Platform support
matrices (exact OS versions, CPUs, Linux desktops and package formats) remain
unresolved until D5/D6; they are release blockers, not presumed coverage.
