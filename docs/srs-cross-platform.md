# Cross-platform desktop Software Requirements Specification

Date: 2026-11-03. Parent: [SPEC §9.33](SPEC.md#933-cross-platform-desktop-requirements-post-mvp).
Tracking: [#95](https://github.com/dhruvparpia/rustty/issues/95), children #96–#100.
Status: requirements baseline for owner-requested feature work; implementation
and desktop qualification pending. No release dates or platform-support claims.

## 1. Purpose and scope

RusTTY shall extend its shared Rust terminal engine to native desktop operation
on macOS and Linux while preserving its Windows frontend and supported contracts.
Native means a compiled desktop application using OS window/input services and
native/GPU rendering, with no browser engine or interpreted application runtime;
it does not require all custom terminal controls to be OS-provided widgets.

Users shall be able to launch local shells, use tabs/splits and output navigation,
and attach to locally managed sessions using the same terminal and operation
semantics. Platform-specific paths, shortcuts and integrations may differ when
documented. MIT, local-first operation, no telemetry, no required administrator
privileges and no required system service remain mandatory.

Scope is desktop Windows/macOS/Linux. Mobile, a shell implementation, a Linux
emulator, native tmux control mode, remote hosts/cross-machine attachment and new
competitor-research features are excluded from this track. Ordinary SSH/tmux
applications running inside a local PTY are compatibility targets.

## 2. Applicability and terminology

Every `shall` below is mandatory at its stated delivery gate. Gates are dependency
checkpoints, not release versions or calendar commitments:

| Gate | Owner issue | Deliverable |
|---|---|---|
| G1 | [#96](https://github.com/dhruvparpia/rustty/issues/96) | Platform interfaces with Windows regression evidence |
| G2 | [#97](https://github.com/dhruvparpia/rustty/issues/97) | Real Unix PTY, host and console attach on macOS and Linux |
| G3 | [#98](https://github.com/dhruvparpia/rustty/issues/98) | macOS frontend vertical slice |
| G4 | [#99](https://github.com/dhruvparpia/rustty/issues/99) | macOS daily-driver integration and release qualification |
| G5 | [#100](https://github.com/dhruvparpia/rustty/issues/100) | Linux desktop integration and release qualification |

G2 can inform G1 interfaces in parallel. G3 depends on usable G1/G2 slices; G4
depends on G3. G5 depends on G1/G2 and the frontend ADR, and is sequenced after
macOS qualification unless the owner changes the plan. G2 requires actual Unix
execution on both OS families, not just cross-compilation.

A **managed session** is a host-owned PTY and shell, not a GUI tab or arbitrary
PID. A **host namespace** identifies an installation/data namespace and effective
user on one machine; it must not accidentally share a host with another portable
installation. A **qualified platform** is an OS/version/architecture/display
configuration with recorded acceptance evidence for the released artifact.

| Existing contract | Cross-platform applicability |
|---|---|
| Parser/grid/search/selection/workspaces and action semantics | Shared; no platform-specific parser fork |
| WIN-* and Windows shell discovery | Windows only; Unix PTY/UI/platform requirements below replace API details |
| HOST-001/002 named pipe and GUI subsystem | Windows realization; Unix authenticated local socket and headless host |
| HOST-003/005/007 identity, verbs, persistence | Shared meaning and exit codes; platform data locations may differ |
| HOST-004/006 Job Objects and input closure | Windows realization; Unix process/termination limits in CP-PTY-003/004 |
| HOST-008 and host ADR minimum-version wording | CP-HOST-004 requires a mutually implemented version, not numeric minimum alone |
| NFR-217 no orphan shells | Local/ephemeral shells are cleaned up; persistent host-owned shells intentionally survive clients; escaped descendants have explicit limits |
| UIA-* and Win32 IME | User-visible accessibility/input goals apply; platform APIs differ |
| Portable ZIP and Windows config locations | Windows packaging; Unix portable mode and platform locations below |
| Historical v0.1–v0.5 labels and MVP exclusions | Historical Windows planning, not cross-platform release promises |

This extension does not retroactively mark any old requirement satisfied. Where
these platform mappings differ from a Windows-specific ADR, this extension
governs Unix scope; implementation ADRs must make the mapping explicit.

## 3. Architecture and shared data requirements

| ID | Requirement | Gate |
|---|---|---|
| CP-ARCH-001 | Shared parser, grid/history identity, selection/search, workspace models and action dispatch shall not depend on OS window handles or platform event structs. FFI shall remain isolated in documented platform modules. | G1 |
| CP-ARCH-002 | Backend boundaries shall specify PTY reads/writes/resize/exit, window/input, rendering/font metrics, clipboard/accessibility, paths/desktop services and local transport. Each boundary shall document ownership, cancellation, threading and error behavior. No fixed trait or crate layout is prescribed. | G1 |
| CP-ARCH-003 | Windows behavior shall remain available during extraction. Parser fixtures and shared action tests shall run across target OSs; Win32 desktop regressions shall be checked independently. | G1 |
| CP-ARCH-004 | A frontend ADR shall compare native frontends and a shared window/GPU/text stack using measured typography, input latency, resource use and accessibility feasibility. winit/wgpu/cosmic-text are candidates only. Windows renderer replacement requires its own evidence and decision. | G3 |
| CP-DATA-001 | Logical config/workspace/session schemas and stable target IDs shall retain shared meanings. Native executable/path values shall not be silently translated between Windows and Unix or substituted with a different shell/cwd. | G2 |
| CP-DATA-002 | Unix config/data/runtime paths and portable-mode precedence shall be documented before storage changes. Portable state shall be isolated by namespace; a read-only install shall produce an actionable error instead of writing to an unrelated location. | G2 |
| CP-DATA-003 | Persistence shall use non-destructive error handling and atomic replacement where supported. Unsupported newer schemas, unreadable files, disk-full/write failures and stale IDs shall not reset identity or overwrite user data. | G2 |
| CP-DATA-004 | Platform integration shall preserve supported argv/env/path values, including empty arguments, spaces and Unicode. OS-native values not representable by the current IPC schema shall be rejected explicitly before spawning, never lossily rewritten. The ADR shall define encoding and any future schema change. | G2 |

## 4. Unix PTY, host and attach requirements

| ID | Requirement | Gate |
|---|---|---|
| CP-PTY-001 | A Unix session shall own a real PTY with a controlling terminal and correct stdin/stdout/stderr, initial geometry, TERM capabilities, cwd, argv and environment. TERM/terminfo declarations shall advertise only implemented behavior. Spawn failure shall return a bounded error without leaking descriptors or claiming a running shell. | G2 |
| CP-PTY-002 | Resize shall apply to the correct live PTY and deliver the appropriate foreground-process-group notification. Interactive Ctrl-C/Ctrl-Z, foreground/background shell jobs, raw/full-screen applications and alternate-screen transitions shall behave according to Unix terminal semantics without duplicate input/signals. | G2 |
| CP-PTY-003 | The host shall own and reap its children, record exit status, and drain final PTY output before publishing stream completion/removing the target. EOF and OS-specific PTY hangup/error results shall be normalized; a grandchild holding a descriptor shall not cause an unbounded completion wait. The drain deadline and incomplete-output result shall be specified in the lifecycle ADR. | G2 |
| CP-PTY-004 | Explicit terminate shall use a documented bounded graceful phase (5 seconds unless separately revised), then force termination of the owned supported process scope. Unix signal/session/process-group behavior, including descendants that escape that scope, shall be documented and tested; a process-tree guarantee shall not be claimed from process groups alone. No unrelated process may be signaled. | G2 |
| CP-PTY-005 | Persistent sessions shall survive every client exit while their host lives; ephemeral sessions shall terminate after their last attachment ends. Host death/logout/reboot limits and possible escaped-child survival shall be documented. A failed host shall never silently respawn a replacement shell under an old target ID. | G2 |
| CP-HOST-001 | Unix IPC shall use local sockets with a private user-owned runtime directory and authenticated peer identity on each connection. Different users/privilege contexts shall not control a host by guessing an endpoint; the privilege policy shall be explicit and shall not silently elevate. | G2 |
| CP-HOST-002 | Single-instance startup shall be race-safe per host namespace. Stale endpoint cleanup shall verify ownership and liveness and shall not unlink an active or unrelated socket. Invalid runtime directories and path-length limits shall return actionable failures. | G2 |
| CP-HOST-003 | The shared request/stream envelope, canonical target identity and structured errors shall be reused. Command verbs and exit codes shall match HOST-005; list/info/errors shall not create a GUI or replace a live host. | G2 |
| CP-HOST-004 | Client and host shall negotiate a version both actually implement and required capabilities. Current/previous supported behavior shall have fixtures; no common version shall fail before mutation with exit 6. App, IPC, snapshot and storage versions shall be independent. An incompatible host with live sessions shall not be killed or auto-upgraded. | G2 |
| CP-HOST-005 | Outbound frames shall be serialized per connection; input ordering, cancellation and bounded queues shall be preserved. Existing 64-KiB request, 1-MiB stream-page and 4-MiB pending-client bounds shall not be weakened by the port. Larger input shall be chunked in order or explicitly rejected, not silently truncated. Stalled clients shall not block PTY drain or other clients. | G2 |
| CP-ATTACH-001 | Attach shall install an atomic snapshot and apply later ordered updates exactly once, preserving modes, decoder continuation, geometry and both screens. Stream gaps shall cause explicit resync or failure. The host shall remain the sole terminal-query reply authority for managed sessions. | G2 |
| CP-ATTACH-002 | Watchers shall not mutate the PTY; takeover shall atomically change controller ownership and propagate demotion. Input/resize shall target canonical session identity, never a list position or reused PID. | G2 |
| CP-ATTACH-003 | Console attach shall save/restore terminal settings and screen state on normal detach, setup failures and handled termination signals; restore shall also occur if entering raw mode only partially succeeds. SIGKILL/terminal disappearance limits shall be documented. | G2 |
| CP-ATTACH-004 | Console attach shall handle caller resize, Unicode input, paste and supported mouse/key sequences. Suspend/resume shall restore caller state while suspended and reestablish negotiated state on resume or exit with an explicit error. Capability limitations shall be reported without inventing unsupported protocol support. | G2 |

## 5. Frontend and platform integration requirements

CP-UI requirements apply to macOS at G3 or G4 as marked and Linux at G5.

| ID | Requirement | Gate |
|---|---|---|
| CP-UI-001 | A real desktop window shall launch/attach a shell, render output, accept input, resize, select/copy/paste and close with correct session ownership. Existing tab/split and shared action flows shall reuse core state. | G3/G5 |
| CP-UI-002 | Renderer metrics, cursor, selection and hit testing shall agree on Unicode cell/grapheme positions with fallback fonts, combining marks, wide characters and emoji. Logical size and device scale shall remain distinct across monitor changes; damage-only drawing and device-loss recovery shall preserve visible content. | G3/G5 |
| CP-UI-003 | Platform modifier conventions and configurable terminal shortcuts shall coexist. IME/dead-key composition shall commit text once, preedit shall not leak bytes to the shell, and focus transitions shall not duplicate keys. Keyboard layout changes and option/AltGr behavior shall have real desktop tests. | G3/G5 |
| CP-UI-004 | Clipboard reads/writes shall be explicit user actions and bounded/asynchronous with respect to terminal processing. Unavailable/contended clipboard, invalid text and large paste shall not freeze the UI or lose session ownership. Clipboard errors shall be visible. | G3/G5 |
| CP-UI-005 | Search, selection, bookmarks/export, workspace opening and supported session actions shall retain shared semantics. OS dialogs and focus transitions shall not send navigation keys into the shell accidentally. | G4/G5 |
| CP-UI-006 | Accessibility shall expose terminal text, selection, tab/pane names, focus and meaningful state changes through platform APIs. Keyboard-only operation and actual VoiceOver or Linux screen-reader navigation shall be demonstrated; exposing a window name alone is insufficient. | G4/G5 |
| CP-UI-007 | Shell discovery, file/URL/editor opening, notifications and theme preferences shall use platform services with validated arguments and clear failures. Loading a workspace shall not execute inferred commands from terminal output; startup commands remain explicit user configuration. | G4/G5 |
| CP-MAC-001 | The macOS backend shall handle application activation/quit, menu shortcuts, app-launch environment and default/login-shell behavior consistently. Closing a view shall distinguish detach from terminate. Profile overrides shall take precedence over shell discovery. | G4 |
| CP-MAC-002 | macOS releases shall declare minimum OS version and tested CPU architectures, provide a signed/notarized app artifact, and locate the companion host/CLI after installation or relocation. No claim of universal-binary support shall precede testing each included architecture. | G4 |
| CP-LINUX-001 | Linux releases shall declare distribution/runtime baseline, CPU architecture, desktop/compositor and Wayland/X11 coverage separately. Unsupported combinations shall not be advertised as qualified; a deliberate support-matrix reduction requires an explicit owner decision. | G5 |
| CP-LINUX-002 | Linux integration shall respect XDG paths, desktop entry/launch behavior and user permissions. Clipboard versus primary selection, input methods, notifications and accessibility transport shall have documented platform behavior. | G5 |
| CP-LINUX-003 | Wayland restrictions on global hotkeys/window placement shall be represented as capabilities with explanatory UI/errors. There shall be no silent insecure fallback or claim of unavailable quick-terminal behavior. Fractional scaling and X11 behavior shall be qualified separately. | G5 |

## 6. Qualification and release requirements

| ID | Requirement | Gate |
|---|---|---|
| CP-QUAL-001 | Every claimed platform shall pass required fmt, strict Clippy and tests, platform backend integration tests and real desktop acceptance. Records shall separate simulation, cross-compilation, native headless execution and desktop execution. | G4/G5; regression evidence at G1 |
| CP-QUAL-002 | The release shall demonstrate supported shells plus SSH, tmux, Neovim, fzf and representative coding-agent CLI behavior using recorded application versions. A passing parser fixture is not a substitute for live input/display behavior. | G4/G5 |
| CP-QUAL-003 | Existing SPEC NFR performance budgets shall remain the baseline: warm start <500 ms, cold start <1.5 s, idle CPU <2%, idle RAM <120 MB, five active tabs <350 MB, resize <200 ms and UI-to-shell input <10 ms. Measure GPU 60-FPS target, 10k-line burst <1 s and zero renderer-hot-path allocations. Any unmet budget requires an explicit documented scope/budget decision, not a hidden exception. | G4/G5 |
| CP-QUAL-004 | Stress evidence shall include 24-hour memory/descriptor/child-process trends and repeated create/close/attach/takeover cycles, output flood and disk/persistence faults. No unresolved release-blocking work-loss, identity, availability or ownership defect may be accepted by a green unit suite alone. | G4/G5 |
| CP-QUAL-005 | Artifacts shall carry consistent product version/build SHA and version/SHA/architecture/hash manifests. Published tags/assets shall be immutable. Clean install, upgrade, relocation/portable operation where supported and uninstall shall preserve stored user data and explicitly handle live hosts. No network/telemetry or background system service shall be required for normal use. | G4/G5 |
| CP-QUAL-006 | Platform schemas/CLI/API compatibility and downgrade refusal shall be tested. Migrations shall preserve newer/unreadable data, and installation shall never force-replace an incompatible live host. Platform addition alone does not require a major product version; supported-contract breaks follow #87. | G4/G5 |

For CP-QUAL-003 report hardware, OS, build profile/SHA, window geometry, font,
renderer, shell workload, measurement endpoints, repetitions and distributions
(at least median/p95/max where applicable). Report app and host resources
separately and combined; identify child-shell resources separately. Record cold
versus warm preparation and display refresh rate. Memory-trend evidence shall
distinguish configured retained history from unbounded growth. Binary size and
software-renderer targets in SPEC remain visible qualification items; their
applicability/measurement must be settled in the renderer/release ADR before a
platform is called qualified.

## 7. Acceptance and traceability

[Cross-platform validation matrix](validation-cross-platform.md) maps every ID
above to reproducible acceptance scenarios, owner issues and required evidence.
All new matrix rows start **not run**. Implementation PRs shall cite requirement
and scenario IDs, exact tested commits/artifacts and evidence locations. A skipped
or blocked mandatory row does not count as passed.

Windows [#94](https://github.com/dhruvparpia/rustty/issues/94) remains independent;
cross-platform work does not declare Windows defects fixed or Windows support
qualified. New platform advertising requires the corresponding G4/G5 evidence.

## 8. Design decisions required before implementation

| Decision | Owner | Must settle before |
|---|---|---|
| D1 Backend/module seams, shared input/action model, cancellation and thread ownership | #96 | Merging interface refactor |
| D2 Unix spawn/reap/drain/terminate mechanism, descendant limits and suspend/resume | #97 | Merging Unix lifecycle implementation |
| D3 Runtime endpoints, namespace isolation, path/argv encoding, peer identity and privilege policy on each Unix OS | #97 | Merging transport/storage implementation |
| D4 Renderer/window/font stack, accessibility approach, fallback and performance measurement | #98 with #100 | Committing to frontend backend/dependencies |
| D5 Minimum macOS/CPU targets, app/CLI layout, signing and portable-state resolution | #99 | Publishing a macOS preview |
| D6 Linux distribution/ABI/CPU and Wayland/X11 matrix, packaging and capability policy | #100 | Publishing a Linux preview |

An ADR shall name alternatives, chosen contracts, evidence, limitations and
requirement IDs. Open choices are not implementation approvals. Dependencies
must follow AGENTS.md latest-stable policy when added; this requirements change
adds no dependencies. Requirement amendments and owner decisions must be recorded
in CHANGELOG and the brain inbox, keeping the owner's canonical spec synchronized.
