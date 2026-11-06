# Project Specification & Software Requirements Specification

## Project: RustyTerm *(placeholder name)*  
**Type:** Lightweight native Windows terminal emulator  
**Primary language:** Rust  
**License:** MIT  
**Primary platform:** Windows 10/11  
**Key product goal:** Ghostty-like terminal features, tabs required, bash/TTY-friendly, open-source, lightweight, native

---

# 1. Document Purpose

This document is the initial **full specification** and **Software Requirements Specification** for a Rust-based, open-source Windows terminal emulator intended to match Ghostty in important user-facing features, especially:

- tabs
- splits
- themes
- fast native rendering
- rich terminal/TTY behavior
- shell integration
- keyboard/mouse support
- lightweight performance
- open-source licensing
- no telemetry

The product is **Windows-first**, not cross-platform in MVP.

The product is **not a full POSIX shell**, not a replacement for Linux, and not an Electron terminal.

---

# 2. Product Statement

RustyTerm is a native Windows terminal emulator written in Rust.

It launches shells using Windows ConPTY, including Git Bash, WSL, MSYS2, Cygwin, PowerShell, CMD, and custom shells. It provides a modern, lightweight terminal UI with tabs as a required feature, rich ANSI/VT terminal behavior, native font rendering, scrollback, search, copy/paste, themes, profiles, and Ghostty-like configuration.

The long-term target is **Ghostty-like feature parity on Windows**, implemented natively using Windows APIs rather than a web stack.

---

# 3. Product Scope

## In scope

A Windows terminal emulator that:

- runs natively on Windows 10/11
- uses Rust
- supports tabs
- supports shell profiles
- supports bash-like shells
- supports TTY-style terminal behavior through ConPTY
- renders terminal text efficiently
- supports ANSI/VT escape sequences
- supports themes, fonts, config files
- supports copy/paste
- supports scrollback
- supports search
- supports splits in post-MVP
- supports shell integration in later versions
- is open-source under MIT
- has no telemetry
- supports portable mode

## Out of scope for MVP

- full cross-platform support
- Electron/web rendering
- a custom bash-compatible shell
- full POSIX PTY semantics
- full POSIX signal/job-control semantics
- full Ghostty parity immediately
- session restore
- plugin system
- global quick terminal
- Sixel/Kitty/iTerm graphics protocols
- multiplexer parity
- full accessibility support
- full IME support

These are not rejected as permanent non-goals, but they are not MVP requirements.

---

# 4. Goals

## G-1: Lightweight native terminal

The terminal must feel fast, native, responsive, and lightweight.

## G-2: Ghostty-like feature target

Match Ghostty in terms of core terminal features:

- tabs
- splits
- themes
- config
- performance
- shell integration
- ligatures
- search
- mouse support
- accessibility
- no telemetry

Full parity is staged, not required for MVP.

## G-3: Tabs are mandatory

Tabs are a required product feature.

The product is not acceptable as a single-window terminal.

## G-4: Bash/TTY-friendly behavior

The terminal must support realistic terminal behavior for:

- bash
- vim
- nano
- less
- top
- htop
- git
- ssh
- TUI apps
- shell scripts
- multiline paste
- resize
- Ctrl-C
- Ctrl-D
- Ctrl-Z
- Ctrl-S/Ctrl-Q
- bracketed paste

## G-5: Open-source

The project must be suitable for community development.

Required:

- public repository
- MIT license
- CONTRIBUTING guide
- SECURITY policy
- issue templates
- CI
- public roadmap
- no CLA

## G-6: Privacy

No telemetry, no analytics, no remote config fetching.

---

# 5. Non-Goals

## NG-1: Not a web terminal

No Electron.

No webview.

No web UI stack.

No JavaScript terminal renderer.

## NG-2: Not a full Linux emulator

No Linux kernel.

No full `/dev/ptmx`.

No full POSIX tty implementation.

## NG-3: Not a shell replacement

The product launches shells. It does not implement bash.

## NG-4: Not a full POSIX job-control system

Windows ConPTY approximates terminal behavior. Full POSIX job control is a best-effort, backend-dependent feature.

## NG-5: Not a full Ghostty clone

The goal is Ghostty-like behavior on Windows, not byte-for-byte compatibility.

---

# 6. Definitions

## Terminal

A program that interprets shell output as a grid of characters and sends keyboard input to the shell.

## Shell

A child process such as bash, PowerShell, CMD, zsh, fish, WSL bash, Git Bash, MSYS2 bash, or Cygwin bash.

## TTY

A terminal-like interface with cooked/raw modes, cursor control, line editing, signals, resize, and TUI app support.

## PTY

A pseudo-terminal. On native Windows, the practical equivalent used by this project is ConPTY.

## ConPTY

Windows Pseudoconsole / ConPTY, a Windows API for terminal pseudo-console behavior.

## Tab

A top-level session container. A tab contains one terminal pane, and in later versions a tree of split panes.

## Pane

A terminal viewport. A tab can contain one pane in MVP. Splits add multiple panes.

## Grid

The terminal character grid, including scrollback, current screen, styles, colors, cursor state, and Unicode cells.

## Ghostty-like

A set of user-facing features and qualities:

- native tabs
- splits
- themes
- rich config
- shell integration
- fast rendering
- ligatures
- search
- mouse reporting
- shell title/working directory
- no telemetry
- performance-focused UX

Ghostty is a trademark of its authors. This project is not an official Ghostty product.

---

# 7. System Architecture Overview

## Recommended architecture

```text
RustyTerm.exe
  Win32 window
    Tab bar
    Pane grid renderer
    Direct2D renderer
    DirectWrite font layout
  Terminal state
    Tabs
    Panes
    Grid buffers
    Scrollback
    Cursor state
    ANSI parser
    Shell integration state
  Shell sessions
    ConPTY child processes
    Stdin/stdout/stderr pipes
    Resize events
    Exit events
  Input
    Keyboard
    Mouse
    IME
    Clipboard
    Key map
  Config
    TOML config
    profiles
    themes
    key bindings
    settings
```

## Thread model

The system uses isolated responsibilities:

1. **UI thread**
   - window messages
   - tab bar
   - keyboard events
   - mouse events
   - IME
   - UI actions
   - rendering commands

2. **Shell process thread**
   - ConPTY child process management
   - stdin/stdout/stderr I/O
   - child exit detection
   - resize delivery

3. **Parser thread**
   - incoming bytes
   - UTF-8 decoding
   - ANSI/VT parsing
   - terminal state updates

4. **Renderer thread**
   - grid snapshots
   - Direct2D drawing
   - cursor rendering
   - selection rendering
   - tab bar rendering

5. **Config thread**
   - config load
   - hot reload
   - validation
   - theme loading

Communication is by message queues/channels, not shared mutable global state.

## Core data flow

```text
User keypress
  -> input manager
  -> keymap
  -> shell stdin
  -> ConPTY
  -> child shell
  -> child output
  -> ConPTY output
  -> parser
  -> grid update
  -> renderer
  -> screen
```

Resize flow:

```text
User/window resize
  -> UI computes new cols/rows
  -> resize pane grid
  -> resize ConPTY
  -> child TUI app redraws
  -> parser updates grid
  -> renderer draws
```

Tab flow:

```text
New tab
  -> create tab state
  -> spawn shell session
  -> create pane/grid
  -> add tab bar entry
  -> focus pane
```

Close tab flow:

```text
Close tab
  -> confirm if busy
  -> signal child
  -> close ConPTY
  -> remove grid/session
  -> update tab bar
  -> close app if last tab closed, per setting
```

---

# 8. Requirements Specification

## 8.1 Requirements classification

- **P0:** Required for MVP.
- **P1:** Required for first Ghostty-like feature set.
- **P2:** Required for full Ghostty-like parity target.
- **P3:** Stretch/open-source future.

Requirement priority is product priority, not implementation order.

---

# 9. Functional Requirements

## 9.1 Product core requirements

| ID | Requirement | Priority |
|---|---|---|
| FR-100 | The product shall be a native Windows terminal emulator written primarily in Rust. | P0 |
| FR-101 | The product shall be open-source under the MIT license. | P0 |
| FR-102 | The product shall not require administrator privileges to run. | P0 |
| FR-103 | The product shall support portable execution from an extracted folder. | P0 |
| FR-104 | The product shall not include telemetry. | P0 |
| FR-105 | The product shall not use Electron. | P0 |
| FR-106 | The product shall not use a web view as its primary UI. | P0 |
| FR-107 | The product shall support x86_64 Windows. | P0 |
| FR-108 | The product should support ARM64 Windows. | P1 |
| FR-109 | The product shall work on Windows 10 and Windows 11. | P0 |

---

## 9.2 Tabs requirements

Tabs are mandatory.

| ID | Requirement | Priority |
|---|---|---|
| TAB-001 | The terminal shall display a tab bar. | P0 |
| TAB-002 | The terminal shall allow the user to create a new tab. | P0 |
| TAB-003 | The terminal shall allow the user to close a tab. | P0 |
| TAB-004 | The terminal shall allow the user to focus a tab. | P0 |
| TAB-005 | The terminal shall support unlimited tabs subject to system resources. | P0 |
| TAB-006 | The terminal shall support keyboard shortcuts for tab creation. | P0 |
| TAB-007 | The terminal shall support keyboard shortcuts for tab closing. | P0 |
| TAB-008 | The terminal shall support keyboard shortcuts for next/previous tab. | P0 |
| TAB-009 | The terminal shall support keyboard shortcuts for jumping to tab 1-9. | P0 |
| TAB-010 | Each tab shall have a title. | P0 |
| TAB-011 | The tab title shall display the profile name by default. | P0 |
| TAB-012 | The tab title shall update from shell OSC title sequences. | P0 |
| TAB-013 | The tab title shall be user-renamable. | P1 |
| TAB-014 | Tabs shall support tab colors. | P1 |
| TAB-015 | Tab colors shall be configurable by profile. | P1 |
| TAB-016 | Tabs shall be reorderable by drag. | P0 |
| TAB-017 | Tabs shall be reorderable by keyboard command. | P2 |
| TAB-018 | The terminal shall ask for confirmation when closing a tab with active shell activity. | P0 |
| TAB-019 | Closing a tab shall close the associated shell session. | P0 |
| TAB-020 | Closing the last tab shall terminate the app, subject to a configurable setting. | P0 |
| TAB-021 | The tab bar shall theme with the active color scheme. | P1 |
| TAB-022 | The tab bar shall support top and bottom placement. | P1 |
| TAB-023 | The tab bar shall hide automatically when only one tab is open, subject to setting. | P1 |
| TAB-024 | The tab bar shall show shell/profile indicator for each tab. | P1 |
| TAB-025 | The tab bar shall show working directory when shell integration is enabled. | P2 |
| TAB-026 | Tabs shall persist across app restarts. | P3 |
| TAB-027 | A tab may be dragged out to a separate window. | P3 |
| TAB-028 | A tab may contain split panes. | P1 |
| TAB-029 | A tab shall preserve scrollback and pane state across resize. | P0 |
| TAB-030 | A tab shall preserve state across config reload. | P2 |

---

## 9.3 Split requirements

Splits are not MVP, but are required for Ghostty-like parity.

| ID | Requirement | Priority |
|---|---|---|
| SPLIT-001 | The terminal shall support vertical split of a pane. | P1 |
| SPLIT-002 | The terminal shall support horizontal split of a pane. | P1 |
| SPLIT-003 | A tab shall support multiple panes. | P1 |
| SPLIT-004 | The user shall be able to focus a pane. | P1 |
| SPLIT-005 | The user shall be able to close a pane. | P1 |
| SPLIT-006 | Pane close shall confirm if the pane has an active shell. | P1 |
| SPLIT-007 | Panes shall inherit the profile of the active tab. | P1 |
| SPLIT-008 | Panes shall support keyboard pane focus movement. | P1 |
| SPLIT-009 | Panes shall support keyboard pane resize. | P2 |
| SPLIT-010 | Pane layout shall persist when a pane closes. | P2 |
| SPLIT-011 | Pane titles shall be visible. | P2 |
| SPLIT-012 | Pane titles shall update from shell OSC title. | P2 |
| SPLIT-013 | Pane titles shall update from shell integration working directory. | P2 |
| SPLIT-014 | Split layout shall be themeable. | P2 |
| SPLIT-015 | Pane borders shall be themeable. | P2 |

---

## 9.4 Shell support requirements

| ID | Requirement | Priority |
|---|---|---|
| SHELL-001 | The terminal shall launch shell child processes using Windows ConPTY. | P0 |
| SHELL-002 | The terminal shall support Git Bash. | P0 |
| SHELL-003 | The terminal shall support WSL bash. | P0 |
| SHELL-004 | The terminal shall support PowerShell. | P0 |
| SHELL-005 | The terminal shall support CMD. | P0 |
| SHELL-006 | The terminal shall support MSYS2 bash. | P1 |
| SHELL-007 | The terminal shall support Cygwin bash. | P1 |
| SHELL-008 | The terminal shall support zsh. | P1 |
| SHELL-009 | The terminal shall support fish. | P1 |
| SHELL-010 | The terminal shall support custom shell commands. | P1 |
| SHELL-011 | The terminal shall auto-detect installed shells. | P0 |
| SHELL-012 | The terminal shall allow profiles to define shell command, args, environment, and starting directory. | P0 |
| SHELL-013 | The terminal shall set TERM for shells. | P0 |
| SHELL-014 | The terminal shall set COLORTERM=truecolor. | P0 |
| SHELL-015 | The terminal shall set TERM_PROGRAM. | P0 |
| SHELL-016 | The terminal shall set TERM_PROGRAM_VERSION. | P0 |
| SHELL-017 | The terminal shall allow shell-specific profile overrides. | P1 |
| SHELL-018 | The terminal shall detect shell exit and clean up tab state. | P0 |
| SHELL-019 | The terminal shall support starting shell in a user-defined working directory. | P0 |
| SHELL-020 | The terminal shall support `~`, `%USERPROFILE%`, and Windows paths appropriately. | P0 |
| SHELL-021 | The terminal shall pass environment variables to the shell. | P0 |
| SHELL-022 | The terminal shall avoid injecting unsafe environment variables into shells. | P0 |

Recommended shell defaults:

```text
TERM=xterm-256color
COLORTERM=truecolor
TERM_PROGRAM=RustyTerm
TERM_PROGRAM_VERSION=<version>
```

---

## 9.5 ConPTY/TTY requirements

| ID | Requirement | Priority |
|---|---|---|
| TTY-001 | The terminal shall use ConPTY for Windows shell I/O. | P0 |
| TTY-002 | The terminal shall support child stdin/stdout/stderr piping. | P0 |
| TTY-003 | The terminal shall resize the ConPTY when the pane grid resizes. | P0 |
| TTY-004 | The terminal shall support Ctrl-C. | P0 |
| TTY-005 | The terminal shall support Ctrl-D. | P0 |
| TTY-006 | The terminal shall support Ctrl-Z. | P0 |
| TTY-007 | The terminal shall support Ctrl-Break where available. | P1 |
| TTY-008 | The terminal shall support Ctrl-Backslash where available. | P1 |
| TTY-009 | The terminal shall support Ctrl-S/Ctrl-Q flow control. | P0 |
| TTY-010 | The terminal shall support raw mode for TUI apps. | P0 |
| TTY-011 | The terminal shall support cooked mode for shells. | P0 |
| TTY-012 | The terminal shall support shell line editing. | P0 |
| TTY-013 | The terminal shall support Ctrl-A, Ctrl-E, Ctrl-K, Ctrl-U, Ctrl-W in shells that use them. | P0 |
| TTY-014 | The terminal shall support Ctrl-R history search. | P0 |
| TTY-015 | The terminal shall support Ctrl-P/Ctrl-N history navigation. | P0 |
| TTY-016 | The terminal shall support Ctrl-L clear screen in shells supporting it. | P0 |
| TTY-017 | The terminal shall support bracketed paste. | P0 |
| TTY-018 | The terminal shall support alt-screen mode. | P0 |
| TTY-019 | The terminal shall support scroll regions. | P1 |
| TTY-020 | The terminal shall support mouse reporting modes. | P1 |
| TTY-021 | The terminal shall support DECCKM application cursor keys. | P1 |
| TTY-022 | The terminal shall support DECCOLM 132-column mode where reasonable. | P2 |
| TTY-023 | The terminal shall support job control best-effort depending on shell backend. | P2 |
| TTY-024 | The terminal shall close child process cleanly when tab closes. | P0 |
| TTY-025 | The terminal shall handle child exit without crash. | P0 |
| TTY-026 | The terminal shall handle ConPTY failure gracefully. | P0 |
| TTY-027 | The terminal shall not require full POSIX PTY semantics. | NFR constraint |

---

## 9.6 TUI acceptance requirements

The product must support common terminal/TUI applications.

| ID | Requirement | Priority |
|---|---|---|
| TUI-001 | vim shall open, edit, move cursor, and exit correctly. | P0 |
| TUI-002 | nano shall open, edit, move cursor, and exit correctly. | P0 |
| TUI-003 | less shall open, scroll, search, and exit correctly. | P0 |
| TUI-004 | top shall render and update correctly. | P0 |
| TUI-005 | htop shall render and update correctly. | P0 |
| TUI-006 | git diff shall render with colors. | P0 |
| TUI-007 | git log shall render with colors. | P0 |
| TUI-008 | watch shall redraw correctly on resize. | P0 |
| TUI-009 | ssh shall render correctly. | P0 |
| TUI-010 | TUI apps shall redraw correctly on window resize. | P0 |
| TUI-011 | Ctrl-C shall exit TUI apps that use SIGINT-like behavior. | P0 |
| TUI-012 | Ctrl-Z shall suspend/signal where supported by backend. | P2 |
| TUI-013 | Alt-screen restore shall return to normal screen correctly. | P0 |
| TUI-014 | Colors shall render correctly in TUI apps. | P0 |
| TUI-015 | 256-color apps shall render correctly. | P0 |
| TUI-016 | True-color apps shall render correctly. | P0 |
| TUI-017 | Unicode text shall render as best as practical. | P1 |

---

## 9.7 Terminal parser requirements

| ID | Requirement | Priority |
|---|---|---|
| PARSER-001 | The terminal shall parse UTF-8 text. | P0 |
| PARSER-002 | The terminal shall handle split UTF-8 sequences safely. | P0 |
| PARSER-003 | The terminal shall handle split ANSI/VT sequences safely. | P0 |
| PARSER-004 | The terminal shall parse CSI sequences. | P0 |
| PARSER-005 | The terminal shall parse OSC sequences. | P0 |
| PARSER-006 | The terminal shall parse ESC sequences. | P0 |
| PARSER-007 | The terminal shall handle BEL. | P0 |
| PARSER-008 | The terminal shall handle cursor movement sequences. | P0 |
| PARSER-009 | The terminal shall handle cursor save/restore. | P0 |
| PARSER-010 | The terminal shall handle erase line. | P0 |
| PARSER-011 | The terminal shall handle erase screen. | P0 |
| PARSER-012 | The terminal shall handle scroll regions. | P1 |
| PARSER-013 | The terminal shall handle 16 ANSI colors. | P0 |
| PARSER-014 | The terminal shall handle 256 colors. | P0 |
| PARSER-015 | The terminal shall handle 24-bit true color. | P0 |
| PARSER-016 | The terminal shall handle bold. | P0 |
| PARSER-017 | The terminal shall handle italic. | P0 |
| PARSER-018 | The terminal shall handle underline. | P0 |
| PARSER-019 | The terminal shall handle inverse video. | P0 |
| PARSER-020 | The terminal shall handle cursor blink. | P0 |
| PARSER-021 | The terminal shall handle cursor hide/show. | P0 |
| PARSER-022 | The terminal shall handle window title OSC 0/2. | P0 |
| PARSER-023 | The terminal shall handle icon title OSC 1. | P1 |
| PARSER-024 | The terminal shall handle bracketed paste mode. | P0 |
| PARSER-025 | The terminal shall handle alt-screen mode. | P0 |
| PARSER-026 | The terminal shall handle mouse reporting modes. | P1 |
| PARSER-027 | The terminal shall handle device status request sequences. | P1 |
| PARSER-028 | The terminal shall handle OSC 8 hyperlinks. | P2 |
| PARSER-029 | The terminal shall handle OSC 7 working directory. | P2 |
| PARSER-030 | The terminal shall handle OSC 133 shell integration sequences. | P2 |
| PARSER-031 | The terminal shall ignore unknown sequences without crashing. | P0 |
| PARSER-032 | The parser shall not panic on malformed input. | P0 |
| PARSER-033 | The parser shall not allow unbounded memory growth from malformed input. | P0 |
| PARSER-034 | The parser shall preserve scrollback during resize. | P0 |
| PARSER-035 | The parser shall preserve alt-screen state during resize. | P0 |
| PARSER-036 | The parser shall process input from multiple shell threads safely. | P0 |

---

## 9.8 Rendering requirements

| ID | Requirement | Priority |
|---|---|---|
| RENDER-001 | The terminal shall render text in a monospaced grid. | P0 |
| RENDER-002 | The terminal shall use native Windows font rendering. | P0 |
| RENDER-003 | The terminal shall use DirectWrite for font discovery and layout. | P0 |
| RENDER-004 | The terminal shall use Direct2D for rendering. | P0 |
| RENDER-005 | The terminal shall use GPU acceleration when available. | P0 |
| RENDER-006 | The terminal shall fall back to software rendering when GPU acceleration is unavailable. | P0 |
| RENDER-007 | The terminal shall support configurable font family. | P0 |
| RENDER-008 | The terminal shall support font fallback. | P0 |
| RENDER-009 | The terminal shall support Cascadia Mono. | P0 |
| RENDER-010 | The terminal shall support Cascadia Code. | P1 |
| RENDER-011 | The terminal shall support configurable font size. | P0 |
| RENDER-012 | The terminal shall support configurable line height. | P0 |
| RENDER-013 | The terminal shall support configurable letter spacing. | P2 |
| RENDER-014 | The terminal shall support cursor block. | P0 |
| RENDER-015 | The terminal shall support cursor beam. | P0 |
| RENDER-016 | The terminal shall support cursor underline. | P0 |
| RENDER-017 | The terminal shall support blinking cursor. | P0 |
| RENDER-018 | The terminal shall support cell background color. | P0 |
| RENDER-019 | The terminal shall support foreground color. | P0 |
| RENDER-020 | The terminal shall support selection highlight. | P0 |
| RENDER-021 | The terminal shall support copy highlight. | P0 |
| RENDER-022 | The terminal shall support search highlight. | P1 |
| RENDER-023 | The terminal shall support scrollback rendering. | P0 |
| RENDER-024 | The terminal shall support scrollbar. | P0 |
| RENDER-025 | The terminal shall support resize rendering. | P0 |
| RENDER-026 | The terminal shall support DPI scaling. | P0 |
| RENDER-027 | The terminal shall support per-monitor DPI. | P0 |
| RENDER-028 | The terminal shall support high DPI. | P0 |
| RENDER-029 | The terminal shall support Unicode width cells. | P1 |
| RENDER-030 | The terminal shall support double-width glyphs best-effort. | P1 |
| RENDER-031 | The terminal shall support emoji best-effort. | P2 |
| RENDER-032 | The terminal shall support CJK text best-effort. | P2 |
| RENDER-033 | The terminal shall support font ligatures. | P1 |
| RENDER-034 | The terminal shall support configurable ligatures setting. | P1 |
| RENDER-035 | The terminal shall avoid UI blocking during fast output. | P0 |
| RENDER-036 | The renderer shall not allocate unbounded memory for scrollback. | P0 |

---

## 9.9 Keyboard requirements

| ID | Requirement | Priority |
|---|---|---|
| KEY-001 | The terminal shall support standard keyboard input. | P0 |
| KEY-002 | The terminal shall translate key events into shell input correctly. | P0 |
| KEY-003 | The terminal shall support Ctrl key input. | P0 |
| KEY-004 | The terminal shall support Alt key input. | P0 |
| KEY-005 | The terminal shall support Esc key input. | P0 |
| KEY-006 | The terminal shall support arrow keys. | P0 |
| KEY-007 | The terminal shall support function keys. | P0 |
| KEY-008 | The terminal shall support Home/End keys. | P0 |
| KEY-009 | The terminal shall support Insert/Delete keys. | P0 |
| KEY-010 | The terminal shall support Page Up/Page Down keys. | P0 |
| KEY-011 | The terminal shall support Backspace. | P0 |
| KEY-012 | The terminal shall support Enter. | P0 |
| KEY-013 | The terminal shall support Tab. | P0 |
| KEY-014 | The terminal shall support Shift+Enter when supported by app. | P2 |
| KEY-015 | The terminal shall support Ctrl+Shift+C copy. | P0 |
| KEY-016 | The terminal shall support Ctrl+Shift+V paste. | P0 |
| KEY-017 | The terminal shall support Ctrl+Shift+T new tab. | P0 |
| KEY-018 | The terminal shall support Ctrl+Shift+W close tab. | P0 |
| KEY-019 | The terminal shall support Ctrl+Tab next tab. | P0 |
| KEY-020 | The terminal shall support Ctrl+Shift+Tab previous tab. | P0 |
| KEY-021 | The terminal shall support Ctrl+Shift+1..9 tab focus. | P0 |
| KEY-022 | The terminal shall support Ctrl+F search. | P1 |
| KEY-023 | The terminal shall support Ctrl+Shift+D vertical split. | P1 |
| KEY-024 | The terminal shall support Ctrl+Shift+E horizontal split. | P1 |
| KEY-025 | The terminal shall support customizable key bindings. | P1 |
| KEY-026 | The terminal shall support IME composition input. | P2 |
| KEY-027 | The terminal shall avoid sending raw modifier-only events to shell. | P0 |
| KEY-028 | The terminal shall handle keyboard locale layouts correctly. | P1 |
| KEY-029 | The terminal shall support keyboard accessibility navigation. | P2 |

---

## 9.10 Mouse requirements

| ID | Requirement | Priority |
|---|---|---|
| MOUSE-001 | The terminal shall support mouse wheel scroll. | P0 |
| MOUSE-002 | The terminal shall support mouse drag selection. | P0 |
| MOUSE-003 | The terminal shall support click selection. | P0 |
| MOUSE-004 | The terminal shall support copy on selection, configurable. | P1 |
| MOUSE-005 | The terminal shall support mouse focus on pane/tab click. | P0 |
| MOUSE-006 | The terminal shall support middle-click paste. | P1 |
| MOUSE-007 | The terminal shall support right-click context menu. | P1 |
| MOUSE-008 | The terminal shall support bracketed paste for mouse-driven input. | P1 |
| MOUSE-009 | The terminal shall support mouse reporting modes. | P1 |
| MOUSE-010 | The terminal shall support mouse reporting coordinate encoding. | P1 |
| MOUSE-011 | The terminal shall support mouse reporting button press/release. | P1 |
| MOUSE-012 | The terminal shall support mouse reporting scroll. | P1 |
| MOUSE-013 | Mouse selection shall copy full lines where applicable. | P0 |
| MOUSE-014 | Selection shall preserve line breaks. | P0 |
| MOUSE-015 | Selection shall include trailing spaces configurable. | P1 |
| MOUSE-016 | The terminal shall support scroll region selection. | P1 |

---

## 9.11 Clipboard requirements

| ID | Requirement | Priority |
|---|---|---|
| CLIP-001 | The terminal shall support copying selected text. | P0 |
| CLIP-002 | The terminal shall support pasting clipboard text. | P0 |
| CLIP-003 | Paste shall support multiline text. | P0 |
| CLIP-004 | Paste shall support bracketed paste when enabled. | P0 |
| CLIP-005 | Clipboard text shall be handled as UTF-8 where supported. | P0 |
| CLIP-006 | Clipboard operations shall not block UI. | P0 |
| CLIP-007 | Clipboard failures shall not crash the app. | P0 |
| CLIP-008 | Paste shall preserve newline conventions. | P0 |
| CLIP-009 | Copy shall preserve newline conventions. | P0 |
| CLIP-010 | Clipboard access shall occur only on explicit user action. | P0 |
| CLIP-011 | Clipboard shall not be accessed for telemetry. | P0 |

---

## 9.12 Scrollback requirements

| ID | Requirement | Priority |
|---|---|---|
| SCROLL-001 | The terminal shall support scrollback. | P0 |
| SCROLL-002 | Default scrollback shall be 10,000 lines. | P0 |
| SCROLL-003 | Scrollback shall be configurable. | P0 |
| SCROLL-004 | Maximum scrollback shall be at least 100,000 lines. | P0 |
| SCROLL-005 | Scrollback shall support keyboard scroll. | P0 |
| SCROLL-006 | Scrollback shall support mouse wheel scroll. | P0 |
| SCROLL-007 | Scrollback shall support jump to bottom. | P0 |
| SCROLL-008 | Scrollback shall support jump to top. | P0 |
| SCROLL-009 | Scroll position shall persist during output. | P0 |
| SCROLL-010 | New output shall not force scroll to bottom if user is scrolled up, configurable. | P1 |
| SCROLL-011 | Resize shall preserve scroll position best-effort. | P0 |
| SCROLL-012 | Scrollback shall support search. | P1 |
| SCROLL-013 | Scrollback shall support copy selection. | P0 |
| SCROLL-014 | Scrollback shall support Unicode text. | P1 |
| SCROLL-015 | Scrollback shall not grow unbounded. | P0 |

---

## 9.13 Theme requirements

| ID | Requirement | Priority |
|---|---|---|
| THEME-001 | The terminal shall support color themes. | P0 |
| THEME-002 | The terminal shall define foreground color. | P0 |
| THEME-003 | The terminal shall define background color. | P0 |
| THEME-004 | The terminal shall define ANSI 16 palette. | P0 |
| THEME-005 | The terminal shall define cursor color. | P0 |
| THEME-006 | The terminal shall define selection color. | P0 |
| THEME-007 | The terminal shall define tab bar colors. | P1 |
| THEME-008 | The terminal shall define pane border colors. | P1 |
| THEME-009 | The terminal shall support named theme files. | P1 |
| THEME-010 | The terminal shall support system dark/light theme detection. | P1 |
| THEME-011 | The terminal shall support per-profile themes. | P1 |
| THEME-012 | The terminal shall support theme hot reload. | P2 |
| THEME-013 | Themes shall be validated on load. | P0 |
| THEME-014 | Invalid theme colors shall fall back safely. | P0 |
| THEME-015 | Themes shall support 256-color palette remapping. | P1 |
| SKIN-001 | RusTTY shall offer paired `copper-dark` and `copper-light` native skins with charcoal/warm-paper surfaces and restrained copper focus accents. Existing themes and explicit profile overrides remain selectable. | P1 |
| SKIN-002 | UI foreground, secondary text, accent and separator colors shall be independently overrideable from terminal foreground/cursor/ANSI colors using `ui-foreground`, `ui-muted`, `ui-accent` and `ui-border`. Omitted keys shall derive compatible colors from the selected theme. | P1 |
| SKIN-003 | Tabs, palette, search, bookmark and pane-title chrome shall use shared semantic UI colors. Active tabs and selected palette/bookmark rows shall have a visible positional indicator as well as a changed background. | P1 |
| SKIN-004 | Copper presets shall meet 4.5:1 normal/secondary text contrast against their chrome surfaces and 3:1 focus-indicator contrast. Selection/search text shall remain readable. Custom colors remain user-controlled and are validated for syntax. | P1 |
| SKIN-005 | Skin changes shall use the existing theme resolution and reload paths, preserve terminal content/actions and reuse render buffers. Validation shall include dark/light, top/bottom tabs, mixed DPI, narrow windows, keyboard focus and Unicode fallback on Windows. | P1 |
| SKIN-006 | The native caption shall follow the effective skin using supported DWM attributes, retain native window controls/drag/resize, and fall back to system rendering when an attribute is unsupported. Tab titles, profile icons and close controls shall be drawn separately. | P1 |

---

## 9.14 Font requirements

| ID | Requirement | Priority |
|---|---|---|
| FONT-001 | The terminal shall use DirectWrite for font rendering. | P0 |
| FONT-002 | The terminal shall support configurable font family. | P0 |
| FONT-003 | The terminal shall support font fallback chains. | P0 |
| FONT-004 | The terminal shall support user-installed fonts. | P0 |
| FONT-005 | The terminal shall support system fonts. | P0 |
| FONT-006 | The terminal shall support Cascadia Mono. | P0 |
| FONT-007 | The terminal shall support Cascadia Code. | P1 |
| FONT-008 | The terminal shall support font size. | P0 |
| FONT-009 | The terminal shall support line height. | P0 |
| FONT-010 | The terminal shall support font ligatures. | P1 |
| FONT-011 | The terminal shall support ligatures as a configurable setting. | P1 |
| FONT-012 | The terminal shall support high DPI font scaling. | P0 |
| FONT-013 | The terminal shall support per-monitor DPI. | P0 |
| FONT-014 | The terminal shall support Unicode shaping best-effort. | P1 |
| FONT-015 | The terminal shall support CJK fonts. | P2 |
| FONT-016 | The terminal shall support emoji fonts best-effort. | P2 |
| FONT-017 | Font discovery shall not crash on invalid fonts. | P0 |
| FONT-018 | Missing fonts shall fall back to a default font. | P0 |

---

## 9.15 Configuration requirements

| ID | Requirement | Priority |
|---|---|---|
| CONFIG-001 | The terminal shall support TOML configuration. | P0 |
| CONFIG-002 | The terminal shall support profiles. | P0 |
| CONFIG-003 | The terminal shall support themes. | P0 |
| CONFIG-004 | The terminal shall support font settings. | P0 |
| CONFIG-005 | The terminal shall support scrollback settings. | P0 |
| CONFIG-006 | The terminal shall support theme settings. | P0 |
| CONFIG-007 | The terminal shall support key bindings. | P1 |
| CONFIG-008 | The terminal shall support copy-on-select setting. | P1 |
| CONFIG-009 | The terminal shall support cursor blink setting. | P0 |
| CONFIG-010 | The terminal shall support cursor shape setting. | P0 |
| CONFIG-011 | The terminal shall support window opacity setting. | P1 |
| CONFIG-012 | The terminal shall support tab bar settings. | P1 |
| CONFIG-013 | The terminal shall support split settings. | P1 |
| CONFIG-014 | The terminal shall support shell integration settings. | P2 |
| CONFIG-015 | Config shall be validated on load. | P0 |
| CONFIG-016 | Invalid config shall produce clear errors. | P0 |
| CONFIG-017 | Invalid config shall not crash the app. | P0 |
| CONFIG-018 | Config shall support hot reload. | P2 |
| CONFIG-019 | Config reload shall preserve tabs best-effort. | P2 |
| CONFIG-020 | Portable mode shall store config beside executable. | P0 |
| CONFIG-021 | Normal mode shall store config under user AppData. | P0 |
| CONFIG-022 | Config shall not require admin. | P0 |
| CONFIG-023 | Config shall support custom profile selectors. | P0 |
| CONFIG-024 | Config shall support starting directory. | P0 |
| CONFIG-025 | Config shall support starting shell. | P0 |
| CONFIG-026 | Config shall support environment variables. | P0 |
| CONFIG-027 | Config shall support shell args. | P0 |
| CONFIG-028 | Config shall support per-profile colors. | P1 |
| CONFIG-029 | Config shall support per-profile fonts. | P1 |
| CONFIG-030 | Config shall support per-profile tab colors. | P1 |

Recommended config format:

```toml
theme = "catppuccin-mocha"
font = "Cascadia Mono"
font-size = 11
line-height = 1.2
scrollback = 10000
copy-on-select = false
cursor-shape = "block"
cursor-blink = true
opacity = 100
editor = "code --goto {file}:{line}:{column}"

[window]
padding = { x = 8, y = 8 }
tab-bar = "auto"

[defaults]
profile = "Git Bash"

[[profile]]
name = "Git Bash"
command = "C:\\Program Files\\Git\\bin\\bash.exe"
args = ["-l"]
starting-directory = "~"
theme = "catppuccin-mocha"
tab-color = "#89b4fa"

[[profile]]
name = "WSL Ubuntu"
command = "wsl.exe"
args = ["--distribution", "Ubuntu"]
starting-directory = "~"
theme = "gruvbox-dark"

[[profile]]
name = "PowerShell"
command = "pwsh.exe"
args = ["-NoLogo"]
starting-directory = "~"
theme = "one-dark"
```

---

## 9.16 Search requirements

| ID | Requirement | Priority |
|---|---|---|
| SEARCH-001 | The terminal shall support search. | P1 |
| SEARCH-002 | Search shall be invoked with Ctrl+F. | P1 |
| SEARCH-003 | Search shall operate over current screen and scrollback. | P1 |
| SEARCH-004 | Search shall highlight matches. | P1 |
| SEARCH-005 | Search shall support next match. | P1 |
| SEARCH-006 | Search shall support previous match. | P1 |
| SEARCH-007 | Search shall support case-sensitive toggle. | P1 |
| SEARCH-008 | Search shall support whole-word toggle. | P1 |
| SEARCH-009 | Search shall support regex. | P2 |
| SEARCH-010 | Search shall support per-pane search. | P1 |
| SEARCH-011 | Search shall support per-tab search. | P1 |
| SEARCH-012 | Search shall support global search. | P2 |
| SEARCH-013 | Search shall not block UI. | P1 |
| SEARCH-014 | Search shall handle Unicode text. | P1 |
| SEARCH-015 | Search shall not grow unbounded. | P1 |

---

## 9.17 Shell integration requirements

| ID | Requirement | Priority |
|---|---|---|
| SHELLINT-001 | The terminal shall support shell integration scripts for bash. | P2 |
| SHELLINT-002 | The terminal shall support shell integration scripts for zsh. | P2 |
| SHELLINT-003 | The terminal shall support shell integration scripts for fish. | P2 |
| SHELLINT-004 | The terminal shall support shell integration scripts for PowerShell. | P2 |
| SHELLINT-005 | The terminal shall support shell integration scripts for CMD best-effort. | P3 |
| SHELLINT-006 | The terminal shall support OSC 133 command tracking. | P2 |
| SHELLINT-007 | The terminal shall support shell working directory tracking. | P2 |
| SHELLINT-008 | The terminal shall support tab title from working directory. | P2 |
| SHELLINT-009 | The terminal shall support pane title from working directory. | P2 |
| SHELLINT-010 | The terminal shall support command start/end detection. | P2 |
| SHELLINT-011 | The terminal shall support prompt detection. | P2 |
| SHELLINT-012 | The terminal shall support semantic terminal API. | P3 |
| SHELLINT-013 | Shell integration shall be opt-in. | P2 |
| SHELLINT-014 | Shell integration shall be disabled by default in MVP. | P0 |
| SHELLINT-015 | Shell integration shall not inject code into shells automatically. | P0 |
| SHELLINT-016 | Shell integration shall require explicit config. | P0 |
| SHELLINT-017 | Shell integration shall work with Git Bash. | P2 |
| SHELLINT-018 | Shell integration shall work with WSL bash. | P2 |
| SHELLINT-019 | Shell integration shall work with PowerShell. | P2 |
| SHELLINT-020 | Shell integration shall not break plain shells. | P0 |
| SHELLINT-021 | The terminal shall track command blocks (bounds, cwd, exit, duration) from OSC 133 marks, in a bounded ring. | P2 |
| SHELLINT-022 | The terminal shall navigate the scroll to the previous/next command block (keyboard, rebindable). | P2 |
| SHELLINT-023 | The terminal shall copy the last finished command's output block to the clipboard. | P2 |
| SHELLINT-024 | The terminal shall toast when a command longer than a configurable threshold finishes while the window is backgrounded; 0 disables. | P2 |
| SHELLINT-025 | Each command shall earn at most one completion notification. | P2 |
| SHELLINT-026 | Entering the alt screen shall close the running command's scrollback block. | P2 |

Recommended shell integration:

```text
OSC 133
OSC 7
OSC 0/1/2
TERM_PROGRAM
TERM_PROGRAM_VERSION
TERM
COLORTERM
```

---

## 9.18 Window requirements

| ID | Requirement | Priority |
|---|---|---|
| WINDOW-001 | The terminal shall create a native Win32 window. | P0 |
| WINDOW-002 | The terminal shall support resizable windows. | P0 |
| WINDOW-003 | The terminal shall support maximized windows. | P0 |
| WINDOW-004 | The terminal shall support minimized windows. | P0 |
| WINDOW-005 | The terminal shall support fullscreen mode. | P1 |
| WINDOW-006 | The terminal shall support window title. | P0 |
| WINDOW-007 | The terminal shall support per-monitor DPI awareness. | P0 |
| WINDOW-008 | The terminal shall support high DPI scaling. | P0 |
| WINDOW-009 | The terminal shall support window snapping. | P0 |
| WINDOW-010 | The terminal shall support window opacity. | P1 |
| WINDOW-011 | The terminal shall support window padding. | P0 |
| WINDOW-012 | The terminal shall support background color. | P0 |
| WINDOW-013 | The terminal shall support system theme integration. | P1 |
| WINDOW-014 | The terminal shall support window close confirmation for multiple tabs. | P0 |
| WINDOW-015 | The terminal shall restore window position on restart. | P3 |
| WINDOW-016 | The terminal shall restore window size on restart. | P3 |
| WINDOW-017 | The terminal shall restore tabs on restart. | P3 |
| WINDOW-018 | The terminal shall support multiple windows. | P2 |
| WINDOW-019 | Multiple windows shall share tab manager state. | P2 |
| WINDOW-020 | Window state shall survive config reload. | P2 |

---

## 9.19 Menu requirements

| ID | Requirement | Priority |
|---|---|---|
| MENU-001 | The terminal shall provide a File menu. | P1 |
| MENU-002 | The terminal shall provide an Edit menu. | P1 |
| MENU-003 | The terminal shall provide a View menu. | P1 |
| MENU-004 | The terminal shall provide a Window menu. | P1 |
| MENU-005 | The terminal shall provide a Help menu. | P1 |
| MENU-006 | The menu shall include New Tab. | P1 |
| MENU-007 | The menu shall include Close Tab. | P1 |
| MENU-008 | The menu shall include Profile selector. | P1 |
| MENU-009 | The menu shall include Theme selector. | P1 |
| MENU-010 | The menu shall include Split commands. | P1 |
| MENU-011 | The menu shall include Copy/Paste. | P1 |
| MENU-012 | The menu shall include Search. | P1 |
| MENU-013 | The menu shall include Preferences. | P2 |
| MENU-014 | The menu shall include About. | P1 |
| MENU-015 | The menu shall include keyboard shortcuts. | P1 |

---

## 9.20 Bell/notification requirements

| ID | Requirement | Priority |
|---|---|---|
| BELL-001 | The terminal shall support BEL. | P0 |
| BELL-002 | Bell shall be visual and/or audible, configurable. | P0 |
| BELL-003 | Bell shall flash tab. | P1 |
| BELL-004 | Bell shall flash pane. | P1 |
| BELL-005 | Bell shall support system notification when backgrounded. | P2 |
| BELL-006 | Bell shall support silent mode. | P1 |
| BELL-007 | Bell shall support visual-only mode. | P1 |
| BELL-008 | Bell shall support audible mode. | P1 |
| BELL-009 | Bell shall not block shell. | P0 |

---

## 9.21 Accessibility requirements

| ID | Requirement | Priority |
|---|---|---|
| ACCESS-001 | The terminal shall support keyboard-only operation. | P1 |
| ACCESS-002 | The terminal shall support Windows UI Automation. | P2 |
| ACCESS-003 | The terminal shall expose accessible text content. | P2 |
| ACCESS-004 | The tab bar shall be accessible. | P2 |
| ACCESS-005 | The pane grid shall be accessible. | P2 |
| ACCESS-006 | Menus shall be accessible. | P2 |
| ACCESS-007 | Settings shall be accessible. | P2 |
| ACCESS-008 | Search UI shall be accessible. | P2 |
| ACCESS-009 | High contrast color schemes shall be supported. | P1 |
| ACCESS-010 | Color contrast shall meet recommended contrast guidance. | P1 |
| ACCESS-011 | Font size shall be adjustable. | P0 |
| ACCESS-012 | DPI scaling shall not break UI. | P0 |
| ACCESS-013 | Screen reader output shall expose terminal text. | P2 |
| ACCESS-014 | Keyboard shortcuts shall be documented. | P0 |
| ACCESS-015 | Accessible name shall be provided for tabs/panes. | P2 |

---

## 9.22 IME/localization requirements

| ID | Requirement | Priority |
|---|---|---|
| I18N-001 | The terminal shall support IME input. | P2 |
| I18N-002 | IME composition shall not corrupt shell input. | P2 |
| I18N-003 | IME composition shall render visually in the terminal. | P2 |
| I18N-004 | The terminal shall support CJK text. | P2 |
| I18N-005 | The terminal shall support Korean/Japanese/Chinese IME. | P2 |
| I18N-006 | UI strings shall be externalized. | P1 |
| I18N-007 | The terminal shall support localization files. | P1 |
| I18N-008 | The terminal shall support RTL text best-effort. | P2 |
| I18N-009 | The terminal shall support Unicode normalization. | P1 |
| I18N-010 | The terminal shall support grapheme clusters. | P1 |

---

## 9.23 Security requirements

| ID | Requirement | Priority |
|---|---|---|
| SEC-001 | The terminal shall not execute shell commands from terminal titles. | P0 |
| SEC-002 | OSC title strings shall be sanitized for UI display. | P0 |
| SEC-003 | OSC hyperlink targets shall require explicit user interaction. | P2 |
| SEC-004 | OSC hyperlink targets shall be validated/sanitized. | P2 |
| SEC-005 | The terminal shall not auto-open links. | P0 |
| SEC-006 | The terminal shall not open links from shell integration. | P2 |
| SEC-007 | Clipboard access shall be explicit. | P0 |
| SEC-008 | Clipboard access shall not send data remotely. | P0 |
| SEC-009 | Config shall not require admin. | P0 |
| SEC-010 | Shell child processes shall run with the same user privileges. | P0 |
| SEC-011 | The terminal shall not require administrator elevation. | P0 |
| SEC-012 | The terminal shall not install system services. | P0 |
| SEC-013 | The terminal shall not modify system registry. | P0 |
| SEC-014 | The terminal shall not modify system shell integrations. | P0 |
| SEC-015 | Config reload shall be validated. | P0 |
| SEC-016 | Config shall not allow arbitrary code execution by default. | P0 |
| SEC-017 | Shell profiles shall not inject code into system shell config. | P0 |
| SEC-018 | OSC 133 integration shall be opt-in. | P2 |
| SEC-019 | The parser shall reject unbounded memory growth. | P0 |
| SEC-020 | The parser shall handle hostile ANSI input safely. | P0 |
| SEC-021 | The parser shall not interpret terminal output as native shell commands. | P0 |
| SEC-022 | Hyperlinks shall be rendered only for valid OSC 8 URLs. | P2 |
| SEC-023 | The terminal shall sanitize control characters in UI titles. | P0 |
| SEC-024 | The terminal shall support code signing in release builds. | P2 |
| SEC-025 | The terminal shall not auto-download remote config. | P0 |
| SEC-026 | The terminal shall not auto-update by default. | P0 |
| SEC-027 | Crash logs shall be local unless explicitly sent. | P1 |
| SEC-028 | Crash logs shall not include shell secrets by default. | P1 |
| SEC-029 | Crash logs shall redact clipboard content. | P1 |
| SEC-030 | The terminal shall not expose shell input/output to telemetry. | P0 |

---

## 9.24 Privacy requirements

| ID | Requirement | Priority |
|---|---|---|
| PRIV-001 | The product shall not include telemetry. | P0 |
| PRIV-002 | The product shall not send usage analytics. | P0 |
| PRIV-003 | The product shall not send shell content. | P0 |
| PRIV-004 | The product shall not send clipboard content. | P0 |
| PRIV-005 | The product shall not send config content. | P0 |
| PRIV-006 | The product shall not send crash data without consent. | P1 |
| PRIV-007 | The product shall not auto-check for updates. | P0 |
| PRIV-008 | Update checking shall be opt-in. | P1 |
| PRIV-009 | Privacy policy shall be published. | P0 |
| PRIV-010 | Privacy policy shall state no telemetry. | P0 |

---

## 9.25 Open-source requirements

| ID | Requirement | Priority |
|---|---|---|
| OSS-001 | The project shall be public. | P0 |
| OSS-002 | The project shall use MIT license. | P0 |
| OSS-003 | The project shall include LICENSE. | P0 |
| OSS-004 | The project shall include README. | P0 |
| OSS-005 | The project shall include CONTRIBUTING.md. | P0 |
| OSS-006 | The project shall include SECURITY.md. | P0 |
| OSS-007 | The project shall include CODE_OF_CONDUCT.md. | P1 |
| OSS-008 | The project shall include issue templates. | P0 |
| OSS-009 | The project shall include PR template. | P1 |
| OSS-010 | The project shall include ROADMAP.md. | P0 |
| OSS-011 | The project shall include CHANGELOG.md. | P1 |
| OSS-012 | The project shall not require a CLA. | P0 |
| OSS-013 | The project shall have CI. | P0 |
| OSS-014 | CI shall build the project. | P0 |
| OSS-015 | CI shall run tests. | P0 |
| OSS-016 | CI shall run cargo fmt. | P0 |
| OSS-017 | CI shall run cargo clippy. | P0 |
| OSS-018 | CI shall run cargo check. | P0 |
| OSS-019 | CI shall check docs. | P1 |
| OSS-020 | Releases shall be reproducible. | P1 |

---

## 9.26 Packaging/distribution requirements

| ID | Requirement | Priority |
|---|---|---|
| PKG-001 | The product shall provide a portable ZIP. | P0 |
| PKG-002 | The portable ZIP shall run without installation. | P0 |
| PKG-003 | The portable ZIP shall not require admin. | P0 |
| PKG-004 | Portable mode shall use local config by default. | P0 |
| PKG-005 | Normal install mode shall use AppData config. | P0 |
| PKG-006 | Releases shall include checksums. | P1 |
| PKG-007 | Releases shall include GitHub release artifacts. | P1 |
| PKG-008 | The project shall support winget. | P1 |
| PKG-009 | The project shall support MSIX, optional. | P3 |
| PKG-010 | Releases shall support x86_64. | P0 |
| PKG-011 | Releases shall support ARM64. | P1 |
| PKG-012 | Releases shall include source archives. | P1 |
| PKG-013 | Releases shall include release notes. | P1 |
| PKG-014 | Releases shall be signed, when practical. | P2 |

---

## 9.27 Workspace requirements

Named workspaces are reusable launch templates (#37, roadmap Stage 1). They
are stored independently of last-session restore; the schema and migration
policy are in [docs/design-workspaces.md](design-workspaces.md).

| ID | Requirement | Priority |
|---|---|---|
| WORKSPACE-001 | The terminal shall support named workspaces that preserve tab/pane layout, profiles, working directories and focus. | P2 |
| WORKSPACE-002 | Workspace templates shall be stored independently of last-session restore; launching or closing a session shall never overwrite a template. | P2 |
| WORKSPACE-003 | The user shall create/update/rename/delete/open workspaces through the native menu. | P2 |
| WORKSPACE-004 | The CLI shall open a named workspace (`--workspace <name>`) and list names (`--workspaces`); create/update/rename/delete run through the UI and the documented hand-editable file. | P2 |
| WORKSPACE-005 | Named workspaces shall appear as Jump List tasks. | P2 |
| WORKSPACE-006 | A pane may carry an explicit startup command (argv) configured in the template; ordinary saved session output shall never become a startup command. | P2 |
| WORKSPACE-007 | A missing profile/directory/executable at open time shall produce a useful error and leave unaffected panes opening. | P2 |
| WORKSPACE-008 | Workspace names, directories and commands shall round-trip Unicode, spaces, WSL profiles, nested splits and repeated launches. | P2 |
| WORKSPACE-009 | The storage schema shall be versioned; a newer file version shall be reported and left untouched, never silently rewritten. | P2 |

## 9.28 Command palette requirements

The command palette (#39, roadmap Stage 1) is a keyboard-first, fuzzy
launcher for every command. One action registry drives the palette, the
native menus and the keybindings, so a command listed anywhere is the
same action everywhere.

| ID | Requirement | Priority |
|---|---|---|
| PALETTE-001 | Ctrl+Shift+P shall open and close the palette; while open, typed text edits the query (WM_CHAR/IME commits included), Up/Down select, Enter runs, and Esc closes and restores terminal focus. | P1 |
| PALETTE-002 | Each row shall display its shortcut: a user `[keybindings]` binding wins over the built-in default, and duplicate bindings for one action shall be reported at config load. | P1 |
| PALETTE-003 | A disabled action shall remain visible with a plain-language reason (e.g. needs 2+ panes, no command history, tab already active). | P1 |
| PALETTE-004 | Typing in the palette shall never reach the PTY; while open, every keydown is consumed by the overlay. | P1 |
| PALETTE-005 | The hit list shall be responsive and bounded (case-insensitive fuzzy subsequence scoring, capped hits and visible rows) with a bounded recently-run history. | P1 |

## 9.29 Environment discovery, open-terminal-here and drag-drop requirements

Discovery (#40, roadmap Stage 1) finds the shells the user has installed
and adds them to the profile list without touching the registry
(SEC-013): WSL distributions (`wsl.exe -l -q`), the newest installed
PowerShell, and Visual Studio developer shells (`vswhere`).

| ID | Requirement | Priority |
|---|---|---|
| DISCOVERY-001 | Discovery shall produce stable profile identities (e.g. "WSL: <distro>", "Developer PowerShell"); re-running discovery shall never duplicate a profile. | P1 |
| DISCOVERY-002 | Merge priority shall be config profile > discovered > built-in; a user profile with the same name shall never be overwritten. | P1 |
| DISCOVERY-003 | Discovery shall run off the UI thread: startup never waits on a probe, and late results merge into the live profile list and Jump List. | P1 |
| DISCOVERY-004 | A failed probe shall be nonfatal and explainable (the profile just does not appear); "Reload Config" re-runs discovery. | P1 |
| DISCOVERY-005 | Discovery shall never modify the registry or install shell extensions (SEC-013/NFR-308). | P0 |
| OPENHERE-001 | A positional directory argument (`rustty <dir>`, "Open terminal here") shall open the first tab in that directory; a non-directory shall be reported and the default location used. Unicode, spaces and supported UNC paths shall work. | P1 |
| DRAG-001 | Dropping files onto the grid shall insert their paths at the shell prompt, quoted for the receiving shell family (Git Bash, PowerShell, CMD, WSL). | P1 |
| DRAG-002 | Dropping folders onto the tab bar shall open one new tab per folder in that directory. | P1 |
| DRAG-003 | The drop shall use the target tab's profile (quoting rules and cloned cwd), not a hard-coded shell. | P1 |
| IDENT-001 | A session whose OSC 7 host is not this machine shall be labelled (tab title suffix " @host" and UIA tab HelpText " · host:"); local and unknown sessions shall show nothing, and the app shall never guess identity from prompt text. | P1 |

## 9.30 Hyperlink requirements

OSC 8 support (#41, roadmap Stage 1, slice 1). Diagnostic
`file:line:column` recognition and editor launch are LINK-006..008
(#51, slice 2). Keyboard hint mode is LINK-009 (#52, slice 3).

| ID | Requirement | Priority |
|---|---|---|
| LINK-001 | OSC 8 shall open a link pen (params ignored, empty URI closes, SGR RST closes); printed cells carry the link id, so the target survives wrap, scrollback eviction, selection and resize (PARSER-028). | P1 |
| LINK-002 | The URI table is bounded (1024 entries, deduplicated); past the cap new links render as links with no openable target, keeping memory flat against hostile output. | P1 |
| LINK-003 | OSC 8 cells render in the theme `link` colour (config key `link`) with an underline, and only for real OSC 8 state (SEC-022); auto-detected text must stay visually distinct. | P1 |
| LINK-004 | Opening a link shall require explicit user interaction: Ctrl+click, or context menu "Open Link" (SEC-003). Only `http`/`https` targets open; every other scheme is copy-only (SEC-004). | P1 |
| LINK-005 | A plain click (no drag) on a link cell, and context menu "Copy Link", copy the URI to the clipboard. | P1 |
| LINK-006 | Diagnostic references (`file:line:column`) in compiler and stack-trace output (cargo/clippy, tsc, python, gcc/WSL styles; drive letters, UNC, spaces and Unicode paths) shall be recognized by scanning the clicked line on demand: nothing is stored on cells, so auto-detected text never takes the OSC 8 link paint path (SEC-022). Paths are anchored by pattern shape, not naive splitting; junk (URLs, times, host:port, mid-word text) is not recognized. | P1 |
| LINK-007 | A relative diagnostic path shall resolve against the pane's OSC 7 cwd; `/mnt/<drive>/…` shall map to `<D>:\…`; Windows absolute paths pass through. Other POSIX paths and sessions on a remote OSC 7 host (IDENT-001) are copy-path-only, never launched. | P1 |
| LINK-008 | Ctrl+click, or context menu "Open in Editor", shall launch the command from the config key `editor` (default `code --goto {file}:{line}:{column}`; `{file}` `{line}` `{column}` placeholders, whitespace-split into argv; empty disables launching, LINK-004's explicit-interaction rule). Non-openable targets (remote host, no cwd, disabled editor) copy the reference text. A failed launch shall be ignored; plain-click behaviour is unchanged. | P1 |
| LINK-009 | One binding (built-in Ctrl+Shift+K, overridable via `[keybindings]` action `hint-mode`, and the command palette "Hint Mode") shall enter keyboard hint mode: every OSC 8 link and auto-detected file reference visible in the focused pane's viewport gets a short typed label drawn as overlay paint only (the grid is never modified, so selection and copy stay uncorrupted). Label keys select (prefix-free labels), Enter opens (http/https links via the browser, openable references via the `editor` command, LINK-004/008), `c` copies the target text, `u` copies a URL, Esc dismisses. While open every keystroke is swallowed (held keys included) and dismiss leaks none. Auto-detected references render in a badge colour distinct from OSC 8 links (SEC-022); references overlapping a link get no label. Scrollback (off-screen) targets are out of scope. | P1 |

---

## 9.31 Large-output search and reading requirements

Issue #42 slice 1 (roadmap "Large-output mode"). Issue #53 slice 2 ships
search context, stable bookmarks and streaming export. Optional disk-backed
history is the remaining #42 scope (#54) and will be specified with that
issue.

| ID | Requirement | Priority |
|---|---|---|
| SEARCH-016 | The search scan shall be incremental and cancellable: one bounded chunk of lines (2048) runs per tick on a 33 ms timer, so typing, scrolling and shell output stay responsive while a scan of megabytes of scrollback is in flight (SEARCH-013). | P1 |
| SEARCH-017 | Each restart (query, option, tab switch, history rewind) bumps a scan generation and clears prior results; a partial result from an older generation is never shown. | P1 |
| SEARCH-018 | While a scan is still running or stopped at the match cap, the count carries a "+" suffix so the user knows more matches may exist. | P1 |
| SEARCH-019 | Lines the shell appends after a scan are searched incrementally (the scan resumes at the tail); when history rewinds (scrollback eviction, alt-screen swap, clear) the scan restarts, so no result points at a line that no longer exists. | P1 |
| SEARCH-020 | Appending output keeps the results already shown, so the selected match survives new output. | P1 |
| SEARCH-021 | A rescan (e.g. a case toggle) that finds the same match restores the user's selection to it. | P2 |
| SEARCH-022 | Scrolling away from the live edge pauses the follow-the-output snap; the reading position survives a burst of shell output. Returning to the live edge (wheel to the bottom, search jump, command jump) resumes it. | P1 |
| SEARCH-023 | Search results shall show configurable context lines above and below the current match (`[search] context-lines`, clamped 0..16); the result view reads those lines from the grid without re-scanning the query. | P1 |
| BKMK-001 | A bookmark shall anchor on the grid's absolute line id, added at the current search match or the focused pane's cursor and toggled off when already present; the list is sorted by anchor and capped at 256. | P1 |
| BKMK-002 | The bookmark overlay shall list bookmarks one row per bookmark with the selected row visible, and Up/Down/Enter/d/Esc drive it while every key is swallowed; the overlay takes the strip from search/palette/hint. | P1 |
| BKMK-003 | An anchor evicted from scrollback shall be marked and never jumped to; the bookmark list must not silently repoint a dead anchor. | P1 |
| EXPORT-001 | Exporting the current selection shall stream line by line to a file chosen via a Save As dialog, with peak memory bounded to one line (no full history clone). | P1 |
| EXPORT-002 | Exporting the last finished command's output shall stream its block to a file chosen via a Save As dialog, clamped to the kept history and bounded to one line in memory. | P1 |

## 9.32 Session host requirements

Issue #46 (roadmap "Session host"; ADR `docs/design-session-host.md`,
contract `docs/design-session-attach.md`). The per-user `rustty-host`
owns managed PTYs independently of any client. Console attach
(#47), streaming (#48/#58), lifecycle UI (#49) and automation
(#56–#59) build on these and will be specified with their issues.

| ID | Requirement | Priority |
|---|---|---|
| HOST-001 | A per-user session host binary (`rustty-host`) serves a per-user named pipe; the pipe's DACL grants access to the current user only, and remote clients are rejected. The host is a GUI-subsystem process: it never flashes a console. | P1 |
| HOST-002 | Single instance: the first process to create the pipe owns it; a second instance probes the pipe, finds the live host, and exits 0. The host exits by itself after an idle budget (10 min) with no clients and no live sessions. Host death is visible to clients as a host-unavailable exit, never as a silent success. | P1 |
| HOST-003 | Sessions get durable u64 ids and stable numeric aliases, persisted atomically under the config dir. A closed alias keeps answering "stale" for the host's process lifetime and across restarts; a stale number is never bound to an unrelated live session. | P1 |
| HOST-004 | The host owns every managed PTY in its own kill-on-close job object: a client process dying (crash included) never kills a host-owned shell, and explicit termination cleans up the whole process tree and its handles. | P1 |
| HOST-005 | Headless management verbs: `--list-sessions`, `--new-session` (with `--name`, `--profile`, `--cwd`), `--session-info <n>`, `--terminate-session <n>`, and versioned JSON via `--json` (schema `host-session/v1`). Records carry name, profile, cwd, geometry, live/exited state, pid and exit code. A failing verb never opens a window and uses stable exit codes: 2 usage, 3 not found, 5 host unavailable, 6 protocol incompatible. | P1 |
| HOST-006 | Graceful termination closes the shell's input (stdin EOF) and waits a bounded grace period (5 s) before terminating the job, so shells get a chance to exit on their own. | P1 |
| HOST-007 | Lifecycle is explicit at creation: persistent sessions survive client detach (the shell keeps running after `--new-session --detach` and after any client exits); ephemeral sessions terminate when the last client detaches. The host never adopts a process it did not spawn. | P1 |
| HOST-008 | Protocol version negotiation: client and host agree on `min(client, host)` at `initialize`; an incompatible host exits 6 with a clear message. A client never replaces, kills, or reconfigures a live host to force a version. | P1 |

---

## 9.33 Project secrets requirements

`rustty secrets` gives each project one small 1Password vault with its own
read-only service account, so agents can fetch the secrets they need without a
1Password approval prompt for every sub-shell. A service account cannot reach a
Private vault, and its vault access cannot change after creation, so each
project needs its own.

**What this does not protect against (SECRET-012).** The threat considered is a
compromised agent running as the same Windows user. This design limits the
damage: a project's token reads one vault, and `run` hands a command only the
names it asks for. It cannot prevent the damage. Code running as the same
Windows user can still reach these secrets: it can decrypt the token file with
DPAPI, because DPAPI protects against other users and not against this one; it
can read a running process's memory and environment; and it can replace a
per-user `op.exe`. A command given a secret can print it. A separate Windows
user for agents is the real boundary, and it is not built.

| ID | Requirement | Priority |
|---|---|---|
| SECRET-001 | The project manifest `.rustty/secrets.toml` maps a NAME to an `op://` reference under a `[secrets]` table and holds no values. It may be committed. Unknown keys are refused. | P2 |
| SECRET-002 | Names shall be safe as Windows environment variables (`[A-Za-z_][A-Za-z0-9_]*`, at most 255 characters, unique ignoring case) and shall not be `PATH`, `PATHEXT`, `SYSTEMROOT`, `WINDIR`, `COMSPEC` or start with `OP_`. References shall match the documented `op://vault/item/[section/]field[?query]` syntax with only letters, digits, `-`, `_`, `.` and spaces in a segment. Anything else is refused, so a literal secret cannot be stored in the manifest. No error message quotes a rejected value, a manifest line or a stray argument. | P2 |
| SECRET-003 | A project's vault is named `rustty-<slug>-<hash>`: the folder name as `[a-z0-9-]`, at most 32 characters, and 8 hex digits of FNV-1a over the canonical project path (no verbatim prefix, lower case). The project is the nearest ancestor of the working directory or `--dir` holding a manifest or `.git`. A linked Git worktree resolves to its main checkout, so all worktrees of a repository share one project. Moving a project folder changes its vault name: run `setup` and `add` again. | P2 |
| SECRET-004 | Secret memory shall be page-aligned `VirtualAlloc` pages held with `VirtualLock`, and zeroed by a volatile write when dropped. Locking fails closed: the working-set quota is raised once, and memory that still cannot be locked is never used. | P2 |
| SECRET-005 | Secret pages should be excluded from Windows Error Reporting dumps with `WerRegisterExcludedMemoryBlock`, looked up at run time because it needs Windows 10 2004 (FR-109). This is best effort and does not affect other dump tools. | P2 |
| SECRET-006 | `LockedSecret` shall be encrypted with `CryptProtectMemory` (same-process) at rest, reachable only through a `with_plaintext` closure, and re-encrypted when that closure ends, including by a panic. It has no `Display`, and its `Debug` shows neither contents nor length. Off Windows every operation fails: there is no insecure fallback. | P2 |
| SECRET-007 | Each project's service-account token is stored encrypted with DPAPI `CryptProtectData`, with the vault name as entropy, in `<vault>.token` under `%LOCALAPPDATA%\rustty\secrets`, taken from the known folder and not the environment variable. It is never in the project folder or the config. | P2 |
| SECRET-008 | `rustty secrets setup [--dir D] [--json]` runs `op vault get <vault>`, `op vault create <vault>` if that failed, and `op service-account create <vault> --vault <vault>:read_items --raw`, and stores the token from stdout. The token is never printed or logged. The store and manifest are checked before anything is created, because `op` shows the token only once. A project already holding a token is left alone. `secrets` is a verb only as the first argument (`.\secrets` opens a folder) and a malformed verb never opens a window. Stable exit codes across the verbs: 2 usage, 3 not found, 8 manifest, 9 `op`, 10 token store, 11 secure memory, 12 command did not start. | P2 |
| SECRET-009 | `rustty secrets add NAME --from op://... [--dir D] [--json]` runs `op read --no-newline <from>` and `op item create --vault <vault> --format json -`, under the owner's own 1Password session because the service account is read-only, and records `op://<vault>/<NAME>/password`. The value travels only through `op`'s stdout and stdin and locked memory in between, never a command line. A name already in the manifest is refused. `rustty secrets list [--dir D] [--json]` shows names and references, never values, and does not call `op`. | P2 |
| SECRET-010 | `rustty secrets run --only A,B [--dir D] -- cmd args...` fetches only the named secrets with `op read --no-newline <reference>`, with the token only in that child's `OP_SERVICE_ACCOUNT_TOKEN`, and returns the command's full 32-bit exit code. Names are required: there is no default of all. A name not in the manifest, a reference outside the project's vault, a failed read or an empty, non-text or oversized value stops it before the command starts. `op` is located only in `Program Files\1Password CLI`, `Program Files\WinGet\Links` and the per-user WinGet folders. There shall be no environment or config override for its path, and neither `PATH` nor the current directory is searched for it. It runs from its own folder with inherited `OP_*` settings removed. | P2 |
| SECRET-011 | The command starts through `CreateProcessW` with an environment block built in locked memory and wiped straight after the call, along with each secret, before the wait. It inherits stdio and no other handle, and its working directory is unchanged by `--dir`. A bare command name is searched on the absolute `PATH` entries only, never the current directory. A `.cmd` or `.bat` runs through `cmd.exe` from the system directory with every argument quoted, and an argument holding a quote, a percent sign or a line break is refused. | P2 |
| SECRET-012 | The documentation shall state the limit above plainly. This feature shall not be described as protecting secrets from code running as the same Windows user. | P2 |

---

# 10. Non-Functional Requirements

## NFR-1: Performance

| ID | Requirement | Target |
|---|---|---|
| NFR-100 | Startup time shall be under 500 ms for warm start. | P0 |
| NFR-101 | Startup time shall be under 1.5 s for cold start. | P0 |
| NFR-102 | Startup target shall be 200 ms warm. | Stretch |
| NFR-103 | Idle CPU shall be under 2% on a modern machine. | P0 |
| NFR-104 | Idle RAM shall be under 120 MB. | P0 |
| NFR-105 | Idle RAM target shall be under 80 MB. | Stretch |
| NFR-106 | 5 tabs with active shells shall use under 350 MB. | P0 |
| NFR-107 | Scrollback of 10,000 lines shall not cause UI stalls. | P0 |
| NFR-108 | Fast output shall not freeze UI. | P0 |
| NFR-109 | Resize shall complete within 200 ms. | P0 |
| NFR-110 | Key input latency shall be under 10 ms UI-to-shell, target 5 ms. | P0 |
| NFR-111 | Binary size shall be under 20 MB. | P0 |
| NFR-112 | Binary size target shall be under 15 MB. | Stretch |
| NFR-113 | GPU rendering shall support 60 FPS target. | P0 |
| NFR-114 | Software rendering shall support 30 FPS target. | P0 |

Performance tests:

```bash
cat large_text.txt
yes
python -c 'print("x" * 100000000)'
git log --all
vim
htop
top
watch ls
```

---

## NFR-2: Stability

| ID | Requirement | Target |
|---|---|---|
| NFR-200 | The product shall not crash on valid shell output. | P0 |
| NFR-201 | The product shall not crash on invalid ANSI. | P0 |
| NFR-202 | The product shall not crash on invalid UTF-8. | P0 |
| NFR-203 | The product shall not crash on huge input. | P0 |
| NFR-204 | The product shall not crash on child exit. | P0 |
| NFR-205 | The product shall not crash on shell exit. | P0 |
| NFR-206 | The product shall not crash on resize. | P0 |
| NFR-207 | The product shall not crash on clipboard failure. | P0 |
| NFR-208 | The product shall not leak memory over 24 hours. | P0 |
| NFR-209 | The product shall handle tab close under stress. | P0 |
| NFR-210 | The product shall handle config reload under stress. | P0 |
| NFR-211 | The product shall handle rapid tab creation. | P0 |
| NFR-212 | The product shall handle rapid tab close. | P0 |
| NFR-213 | The product shall handle window close while shell running. | P0 |
| NFR-214 | The product shall handle ConPTY failure. | P0 |
| NFR-215 | The product shall handle DirectWrite failure. | P0 |
| NFR-216 | The product shall handle Direct2D failure. | P0 |
| NFR-217 | The product shall not leave orphan child shells on app exit. | P0 |

---

## NFR-3: Lightweight requirements

| ID | Requirement | Target |
|---|---|---|
| NFR-300 | No Electron. | P0 |
| NFR-301 | No Node runtime. | P0 |
| NFR-302 | No Python runtime. | P0 |
| NFR-303 | No browser engine. | P0 |
| NFR-304 | No heavy framework dependency for core UI. | P0 |
| NFR-305 | Portable ZIP shall be usable. | P0 |
| NFR-306 | No admin required. | P0 |
| NFR-307 | No system service required. | P0 |
| NFR-308 | No registry modification required. | P0 |
| NFR-309 | No system shell integration by default. | P0 |

---

## NFR-4: Maintainability

| ID | Requirement | Target |
|---|---|---|
| NFR-400 | Core code shall be Rust. | P0 |
| NFR-401 | Unsafe code shall be isolated to FFI boundaries. | P0 |
| NFR-402 | Parser shall have unit tests. | P0 |
| NFR-403 | Config shall have unit tests. | P0 |
| NFR-404 | Tab model shall have unit tests. | P0 |
| NFR-405 | Rendering shall have integration tests. | P1 |
| NFR-406 | Project shall use cargo fmt. | P0 |
| NFR-407 | Project shall pass clippy. | P0 |
| NFR-408 | Project shall have docs. | P0 |
| NFR-409 | Public APIs shall have rustdoc. | P1 |
| NFR-410 | Module boundaries shall be documented. | P0 |

---

## NFR-5: Security

| ID | Requirement | Target |
|---|---|---|
| NFR-500 | No telemetry. | P0 |
| NFR-501 | No remote code execution. | P0 |
| NFR-502 | No admin required. | P0 |
| NFR-503 | No system service. | P0 |
| NFR-504 | No registry modification. | P0 |
| NFR-505 | No shell integration by default. | P0 |
| NFR-506 | OSC titles sanitized. | P0 |
| NFR-507 | OSC hyperlinks opt-in and validated. | P2 |
| NFR-508 | Clipboard explicit only. | P0 |
| NFR-509 | Crash logs local by default. | P1 |
| NFR-510 | Config validation required. | P0 |
| NFR-511 | Parser fuzz-safe. | P1 |

---

# 11. Ghostty Feature Parity Matrix

The goal is Ghostty-like parity on Windows.

| Ghostty-like feature | RustyTerm target | Priority |
|---|---|---|
| Tabs | Full tabs: create/close/focus/reorder/title/color/keyboard | P0 |
| Splits | Vertical/horizontal split panes | P1 |
| Themes | TOML themes, palettes, tab colors, system theme | P0/P1 |
| Config | TOML, profiles, themes, fonts, scrollback | P0 |
| Hot reload | Config reload without app restart | P2 |
| Native rendering | DirectWrite/Direct2D, GPU acceleration | P0 |
| Fast performance | Native Rust, no Electron | P0 |
| No telemetry | No analytics | P0 |
| MIT license | MIT | P0 |
| Open source | Public repo, MIT, CI, docs | P0 |
| Ligatures | DirectWrite font ligatures | P1 |
| Search | Ctrl+F, scrollback search, regex | P1/P2 |
| Mouse support | Selection, copy, mouse reporting, bracketed paste | P0/P1 |
| Shell title | OSC 0/2, tab title update | P0 |
| Working directory | OSC 7 / shell integration | P2 |
| OSC 133 | Shell integration command tracking | P2 |
| Accessibility | UIA, keyboard-only, screen reader support | P2 |
| IME | Windows IME composition | P2 |
| Quick terminal | Global hotkey quick terminal | P3 |
| Session restore | Restore tabs/windows | P3 |
| Plugin API | Extensible plugin system | P3 |
| Multiplexer | tmux-like session management | P3 |
| Graphics protocols | Sixel/Kitty/iTerm | P3 |
| Cross-platform | macOS/Linux parity | P3 |
| Native menus | Win32 menus | P1 |
| Opacity/blur | Window opacity, optional blur | P1/P2 |
| Tab persistence | Session restore | P3 |
| High DPI | Per-monitor DPI | P0 |
| Unicode | Graphemes, CJK, emoji best-effort | P1/P2 |

Full Ghostty-like parity is achieved when P0, P1, and P2 requirements are complete.

P3 features are optional stretch.

---

# 12. Data Model Requirements

## Required data structures

```rust
App
  windows: Vec<Window>
  tabs: Vec<Tab>
  panes: Vec<Pane>
  config: Config
  keymap: Keymap
  theme: Theme
```

```rust
Window
  tabs: Vec<TabId>
  focused_tab: TabId
  window_state: WindowState
```

```rust
Tab
  id: TabId
  panes: PaneTree
  focused_pane: PaneId
  title: String
  color: Color
  profile_id: ProfileId
```

```rust
Pane
  id: PaneId
  grid: Grid
  shell: ShellSession
  title: String
  color: Color
  font: FontConfig
```

```rust
ShellSession
  child: ChildProcess
  conpty: PseudoConsole
  stdin: Pipe
  stdout: Pipe
  stderr: Pipe
  cols: u16
  rows: u16
  running: bool
```

```rust
Grid
  cols: usize
  rows: usize
  cells: Vec<Cell>
  scrollback: Vec<Vec<Cell>>
  cursor: Cursor
  modes: TerminalModes
  alt_screen: Option<Grid>
```

```rust
Cell
  grapheme: String
  fg: Color
  bg: Color
  style: CellStyle
  width: CellWidth
  hyperlink: Option<String>
```

```rust
Cursor
  col: usize
  row: usize
  shape: CursorShape
  visible: bool
  blink: bool
  color: Color
```

```rust
Config
  profile: Profile
  theme: Theme
  font: FontConfig
  scrollback: usize
  opacity: u8
  keybindings: Vec<KeyBinding>
  tab_bar: TabBarConfig
  split: SplitConfig
  shell_integration: ShellIntegrationConfig
```

```rust
Profile
  name: String
  command: PathBuf
  args: Vec<String>
  starting_directory: PathBuf
  env: HashMap<String, String>
  theme: Theme
  font: FontConfig
  tab_color: Color
```

---

# 13. Config Schema Requirements

## Required top-level fields

```toml
theme
font
font-size
line-height
ligatures
scrollback
opacity
cursor-shape
cursor-blink
copy-on-select
tab-bar
starting-directory
starting-profile
```

## Required profile fields

```toml
name
command
args
starting-directory
env
theme
font
font-size
tab-color
```

## Required theme fields

```toml
name
foreground
background
cursor
selection
black
red
green
yellow
blue
magenta
cyan
white
bright_black
bright_red
bright_green
bright_yellow
bright_blue
bright_magenta
bright_cyan
bright_white
```

## Required tab bar fields

```toml
visible
placement
show-icons
show-profiles
show-cwd
close-button
tab-width
tab-padding
```

## Required split fields

```toml
visible
padding
border
border-color
focus-color
inactive-color
keyboard-shortcuts
```

## Required search fields

```toml
visible
highlight-color
case-sensitive-default
whole-word-default
regex-default
```

## Required shell integration fields

```toml
enabled
mode
command-tracking
working-directory
prompt-detection
osc-133
```

## Example complete config

```toml
theme = "catppuccin-mocha"
font = "Cascadia Mono"
font-size = 11
line-height = 1.2
ligatures = true
scrollback = 10000
opacity = 100
cursor-shape = "block"
cursor-blink = true
copy-on-select = false
starting-profile = "Git Bash"

[tab-bar]
visible = "auto"
placement = "top"
show-profiles = true
show-cwd = false
close-button = true

[split]
visible = true
padding = 8
border = true

[search]
visible = true
case-sensitive-default = false
whole-word-default = false
regex-default = false

[[profile]]
name = "Git Bash"
command = "C:\\Program Files\\Git\\bin\\bash.exe"
args = ["-l"]
starting-directory = "~"
theme = "catppuccin-mocha"
tab-color = "#89b4fa"

[[profile]]
name = "WSL Ubuntu"
command = "wsl.exe"
args = ["--distribution", "Ubuntu"]
starting-directory = "~"
theme = "gruvbox-dark"

[[profile]]
name = "PowerShell"
command = "pwsh.exe"
args = ["-NoLogo"]
starting-directory = "~"
theme = "one-dark"

[[profile]]
name = "CMD"
command = "cmd.exe"
args = []
starting-directory = "~"
theme = "solarized-dark"
```

---

# 14. Interface Requirements

## UIA-001: Tab bar interface

The tab bar shall expose:

- tab title
- tab index
- tab color
- close button
- profile name
- working directory, when available
- shell activity state

Accessible properties:

```text
Name
Position
ControlType
KeyboardShortcut
HelpText
ExpandCollapse
```

## UIA-002: Terminal grid interface

The grid shall expose:

- current screen text
- scrollback text, best-effort
- cursor position
- selection text
- accessible role as text area/document

## UIA-003: Menu interface

Menus shall expose:

- commands
- keyboard shortcuts
- enabled/disabled state
- checked state where applicable

## UIA-004: Search interface

Search UI shall expose:

- search field
- match count
- current match
- next button
- previous button
- close button
- case toggle
- regex toggle

The command palette (PALETTE-001..005) shall expose an equivalent node: an
editable element named "Command palette" whose value carries the query and
the current/total match position, present only while the palette is open.

## UIA-005: Window interface

Window shall expose:

- title
- tabs
- panes
- active pane
- app version

---

# 15. Windows API Requirements

## WIN-001: ConPTY

Use:

```text
CreatePseudoConsole
ResizePseudoConsole
ClosePseudoConsole
CreatePipe
ReadFile
WriteFile
```

Requirements:

- pipe handles must not leak
- child process must receive pipe ends
- parent must not retain pipe handles
- ConPTY resize must be delivered
- ConPTY close must be graceful
- ConPTY failure must not crash UI

## WIN-002: DirectWrite

Use for:

- font collection
- font family
- font face
- font metrics
- text layout
- ligatures
- glyph analysis
- text shaping
- DPI scaling

## WIN-003: Direct2D

Use for:

- window rendering
- background fill
- text glyph rendering
- cursor rendering
- selection rendering
- search highlight
- tab bar rendering
- split pane rendering

## WIN-004: Clipboard

Use Windows clipboard APIs.

Requirements:

- text clipboard
- unicode text
- multiline paste
- clipboard close/end
- avoid clipboard contention
- avoid UI thread blocking

## WIN-005: Keyboard

Use Windows keyboard messages.

Requirements:

- WM_KEYDOWN
- WM_KEYUP
- WM_CHAR
- WM_SYSKEYDOWN
- WM_SYSCHAR
- WM_IME_...
- modifier handling
- Unicode text
- dead keys best-effort

## WIN-006: IME

Use IME APIs.

Requirements:

- composition string display
- composition commit
- conversion state
- no shell corruption
- CJK/Japanese/Korean input support

## WIN-007: UI Automation

Use UI Automation providers.

Requirements:

- accessible tab bar
- accessible pane
- accessible menus
- accessible search
- accessible config UI, later

---

# 16. Terminal Behavior Requirements

## Parser behavior

The parser shall be tolerant.

Valid behavior:

- accept ANSI/VT output
- accept malformed escape sequences
- accept split UTF-8
- accept split ANSI
- accept huge input
- accept unknown sequences
- accept OSC strings
- accept OSC 0/1/2 title strings
- accept OSC 133
- accept OSC 7
- accept OSC 8
- accept mouse mode sequences
- accept alt-screen sequences

The parser shall not:

- panic
- allocate unbounded memory
- execute shell commands
- execute native code
- spawn processes
- modify config automatically
- access clipboard automatically
- send data to network

---

# 17. TTY/ConPTY Behavior Requirements

## Required shell behavior

The product shall support:

```bash
bash
bash -l
vim
nano
less
top
htop
git
ssh
git diff
watch ls
```

PowerShell:

```powershell
pwsh
cmd
```

WSL:

```bash
wsl
wsl --distribution Ubuntu
```

Git Bash:

```bash
C:\Program Files\Git\bin\bash.exe -l
```

## Required key behavior

```text
Ctrl-C: interrupt shell/TUI app
Ctrl-D: EOF/exit shell when applicable
Ctrl-Z: suspend/signal where supported
Ctrl-L: clear screen
Ctrl-A: line start
Ctrl-E: line end
Ctrl-K: kill line
Ctrl-U: clear line
Ctrl-W: delete word
Ctrl-R: reverse history search
Ctrl-P: previous command
Ctrl-N: next command
Ctrl-S: pause output where supported
Ctrl-Q: resume output where supported
Ctrl-Shift+C: copy
Ctrl-Shift+V: paste
```

## Required resize behavior

When terminal window resizes:

1. UI computes new pane size.
2. Pane grid resizes.
3. ConPTY receives resize.
4. Child shell/TUI app receives resize.
5. TUI app redraws.
6. Parser updates grid.
7. Renderer redraws.

No UI freeze required.

---

# 18. Acceptance Criteria

## MVP Acceptance: A-1 — Tabs work

Scenario:

1. Launch app.
2. Click New Tab.
3. Tab 2 opens with default shell.
4. Type `ls`.
5. Output renders.
6. Resize window.
7. Output remains readable.
8. Close tab 2.
9. App returns to tab 1.
10. Close tab 1.
11. App exits cleanly.

Pass criteria:

- no crash
- no orphan shell
- no tab bar desync
- no memory leak over repeated tab creation/closing

---

## MVP Acceptance: A-2 — Bash/Git Bash works

Scenario:

1. Launch Git Bash profile.
2. Run:

```bash
pwd
ls
echo hello
cat somefile
```

3. Resize.
4. Ctrl-C interrupt a running command.
5. Exit shell.

Pass criteria:

- bash works
- Ctrl-C works
- resize works
- exit clean

---

## MVP Acceptance: A-3 — vim works

Scenario:

1. Launch Git Bash.
2. Run:

```bash
vim file.txt
```

3. Enter insert mode.
4. Type text.
5. Exit insert mode.
6. Save.
7. Quit.
8. Resize while vim open.
9. Ctrl-C exits vim.

Pass criteria:

- vim does not render mangled
- arrow keys work
- resize redraws
- Ctrl-C exits
- terminal restores after vim exits

---

## MVP Acceptance: A-4 — less works

Scenario:

1. Launch bash.
2. Run:

```bash
cat large_file.txt | less
```

3. Scroll.
4. Search.
5. Quit.
6. Resize.

Pass criteria:

- less opens
- scroll works
- search works
- resize works
- alt-screen restores

---

## MVP Acceptance: A-5 — top/htop works

Scenario:

1. Launch bash.
2. Run:

```bash
top
htop
watch ls
```

3. Resize.
4. Ctrl-C exit.

Pass criteria:

- redraws correctly
- no garbled output
- Ctrl-C exits
- resize redraws

---

## MVP Acceptance: A-6 — colors work

Scenario:

Run:

```bash
printf "\e[31mRED\e[0m\n"
ls --color=auto
git diff
```

Pass criteria:

- ANSI colors render
- 256 colors render
- true color renders
- git diff renders

---

## MVP Acceptance: A-7 — copy/paste works

Scenario:

1. Select text.
2. Copy.
3. Paste into editor.
4. Paste multiline text into shell.
5. Bracketed paste mode enabled.
6. Paste multiline into vim.

Pass criteria:

- copy preserves lines
- paste preserves lines
- multiline paste safe
- no shell injection
- no UI freeze

---

## MVP Acceptance: A-8 — scrollback works

Scenario:

Run:

```bash
yes
```

Stop after scrollback fills.

Pass criteria:

- UI remains responsive
- scrollback usable
- search future-ready
- no memory explosion
- resize preserves state

---

## P1 Acceptance: A-9 — Splits work

Scenario:

1. Launch tab.
2. Create vertical split.
3. Create horizontal split.
4. Run vim in split.
5. Run bash in split.
6. Resize pane.
7. Close pane.
8. Close tab.

Pass criteria:

- splits work
- focus works
- resize works
- close clean

---

## P1 Acceptance: A-10 — Search works

Scenario:

1. Run command that produces many lines.
2. Ctrl+F.
3. Search text.
4. Next match.
5. Previous match.
6. Case-sensitive toggle.
7. Regex toggle.

Pass criteria:

- search highlights
- scrollback search works
- Unicode works
- no UI freeze

---

## P2 Acceptance: A-11 — Shell integration works

Scenario:

Enable bash shell integration.

Run commands:

```bash
pwd
cd /c
cd /tmp
git status
```

Pass criteria:

- working directory tracked
- tab title updates
- pane title updates
- command tracking works
- no shell breakage

---

## P2 Acceptance: A-12 — Accessibility works

Scenario:

Use keyboard-only navigation:

1. Create tab.
2. Rename tab.
3. Open search.
4. Search text.
5. Use UIA inspector.
6. Use screen reader.

Pass criteria:

- UIA exposes tabs/panes
- screen reader announces content
- keyboard-only operation works
- focus order works

---

# 19. Roadmap

## v0.1 — Foundation

Goals:

- native Rust app
- Win32 window
- DirectWrite/Direct2D
- ConPTY
- basic ANSI parser
- tabs
- basic config
- portable ZIP

Requirements:

```text
FR-100
FR-101
FR-102
FR-103
FR-104
FR-105
FR-106
FR-107
FR-109
TAB-001..TAB-020
SHELL-001..SHELL-016
TTY-001..TTY-018
PARSER-001..PARSER-036
RENDER-001..RENDER-028
KEY-001..KEY-021
MOUSE-001..MOUSE-014
CLIP-001..CLIP-011
SCROLL-001..SCROLL-015
CONFIG-001..CONFIG-030
THEME-001..THEME-014
FONT-001..FONT-018
NFR-100..NFR-114
NFR-200..NFR-217
SEC-001..SEC-030
PRIV-001..PRIV-010
OSS-001..OSS-020
PKG-001..PKG-11
```

Definition of Done:

```text
Git Bash tab launches.
Tab creation/closing works.
Tab reorder works.
Ctrl-C works.
Ctrl-D works.
vim works.
less works.
top works.
Resize works.
Copy/paste works.
Portable mode works.
No telemetry.
CI green.
MIT license.
```

---

## v0.2 — Ghostty Basics

Goals:

- themes
- profiles
- key bindings
- search
- splits
- menus
- ligatures
- bracketed paste
- mouse reporting
- tab colors
- pane titles

Requirements:

```text
SPLIT-001..SPLIT-015
SEARCH-001..SEARCH-015
THEME-007..THEME-014
KEY-022..KEY-025
MOUSE-006..MOUSE-013
FONT-010..FONT-014
TAB-013..TAB-017
TAB-021..TAB-024
SHELL-017..SHELL-022
CONFIG-007..CONFIG-013
MENU-001..MENU-015
PARSER-019..PARSER-021
TTY-019..TTY-021
```

Definition of Done:

```text
Splits work.
Search works.
Themes work.
Key bindings work.
Ligatures work.
Mouse reporting works.
Menus work.
Tab colors work.
Pane titles work.
```

---

## v0.3 — Shell Integration & Polish

Goals:

- shell integration
- OSC 133
- working directory
- command tracking
- tab title by directory
- accessibility
- IME
- config hot reload
- performance polish

Requirements:

```text
SHELLINT-001..SHELLINT-020
TAB-025
PARSER-028..PARSER-030
ACCESS-001..ACCESS-015
I18N-001..I18N-010
CONFIG-018..CONFIG-019
WINDOW-012..WINDOW-020
SEC-018
SEC-022
PRIV-006..PRIV-010
```

Definition of Done:

```text
bash shell integration works.
Working directory shown.
Command tracking works.
Accessibility works.
IME works.
Config hot reload works.
Ghostty-like UX polish achieved.
```

---

## v0.4 — Ghostty-like Parity

Goals:

Full Ghostty-like feature parity on Windows.

Definition of Done:

```text
P0: complete
P1: complete
P2: complete
```

Parity target:

```text
tabs
splits
themes
config
shell integration
search
mouse
ligatures
accessibility
IME
performance
no telemetry
MIT
open-source
portable
native Windows UI
```

---

## v0.5 — Stretch

Optional:

```text
session restore
quick terminal
plugin API
multiplexer
graphics protocols
cross-platform
macOS
Linux
global hotkeys
native window decorations
advanced accessibility
session sharing
tab persistence
window state restore
```

---

# 20. Technical Stack Recommendation

## Recommended stack

```text
Language: Rust
License: MIT
Platform: Windows 10/11
UI: Win32
Rendering: Direct2D
Fonts: DirectWrite
Shell: ConPTY
Config: TOML
Parser: custom Rust ANSI/VT parser
Tabs: custom tab manager
Search: custom search engine
Clipboard: Win32 clipboard
Accessibility: Windows UI Automation
IME: Win32 IME APIs
Packaging: portable ZIP first
```

## Recommended Rust crates

Core:

```text
windows
serde
toml
crossbeam-channel
tracing
anyhow
thiserror
```

Optional terminal helper crates:

```text
alacritty_core
vt100
termwarp
```

Evaluation plan:

1. Build custom parser.
2. Use `alacritty_core` as reference.
3. Use `vt100` for test vectors.
4. Use `termwarp` for TUI testing reference.
5. Avoid dependency lock-in if custom parser is cleaner.

Rendering:

```text
Direct2D via windows crate
DirectWrite via windows crate
custom glyph cache
custom font atlas
```

Cross-platform UI:

Not MVP.

If later:

```text
winit
wgpu
cosmic-text
```

---

# 21. Project Structure

Proposed Rust workspace layout:

```text
rustyterm/
  Cargo.toml
  README.md
  LICENSE
  CONTRIBUTING.md
  SECURITY.md
  ROADMAP.md
  CHANGELOG.md
  docs/
    spec.md
    srs.md
    architecture.md
    testing.md
  src/
    main.rs
    app/
      app.rs
      window.rs
      events.rs
    config/
      config.rs
      profile.rs
      theme.rs
      keymap.rs
      schema.rs
    ui/
      tab_bar.rs
      menu.rs
      search_bar.rs
      settings.rs
    terminal/
      grid.rs
      cell.rs
      cursor.rs
      modes.rs
      parser.rs
      parser_state.rs
      unicode.rs
      ansi.rs
      osc.rs
      csi.rs
    shell/
      conpty.rs
      child.rs
      shell_session.rs
      shell_integration.rs
      env.rs
      profile_launcher.rs
    render/
      renderer.rs
      direct2d.rs
      directwrite.rs
      font.rs
      glyph_cache.rs
      selection.rs
      cursor_renderer.rs
    input/
      keyboard.rs
      mouse.rs
      ime.rs
      keymap.rs
      clipboard.rs
    accessibility/
      uia.rs
      accessible_grid.rs
      accessible_tabs.rs
    packaging/
      portable.rs
      install.rs
    tests/
      parser_tests.rs
      grid_tests.rs
      tab_tests.rs
      shell_tests.rs
      render_tests.rs
```

---

# 22. Testing Requirements

## Unit tests

Required:

- parser unit tests
- UTF-8 unit tests
- ANSI parser tests
- OSC parser tests
- CSI parser tests
- grid tests
- cursor tests
- selection tests
- scrollback tests
- config tests
- profile tests
- theme tests
- keymap tests
- tab model tests
- shell session lifecycle tests
- resize logic tests
- copy/paste logic tests

## Integration tests

Required:

- launch shell
- run commands
- resize
- Ctrl-C
- Ctrl-D
- exit shell
- tab close
- tab create
- tab reorder
- copy/paste
- scrollback
- theme load
- config reload

## Acceptance tests

Required:

```bash
vim
nano
less
top
htop
git
git diff
git log
watch ls
ssh
```

PowerShell tests:

```powershell
pwsh
cmd
```

WSL tests:

```bash
wsl
wsl --distribution Ubuntu
```

Git Bash tests:

```bash
C:\Program Files\Git\bin\bash.exe -l
```

## Stress tests

Required:

```bash
yes
cat large_text.txt
python -c 'print("x" * 100000000)'
git log --all
vim
htop
watch ls
```

Stress operations:

- create 100 tabs
- close 100 tabs
- resize rapidly
- paste huge text
- invalid ANSI
- random UTF-8
- random ANSI fuzz
- child exit
- ConPTY close
- config reload
- theme switch

## Fuzz tests

Required:

- random ANSI input
- random UTF-8
- random OSC strings
- random CSI
- random ESC sequences
- split sequences across boundaries

Pass criteria:

- no panic
- no unbounded memory
- graceful recovery
- no crash

---

# 23. Performance Requirements Summary

## Startup

Target:

```text
Warm startup: 200 ms target, 500 ms max
Cold startup: 500 ms target, 1.5 s max
```

## Memory

Target:

```text
Idle: under 120 MB, target 80 MB
5 tabs: under 350 MB
10 tabs: under 600 MB
```

## CPU

Target:

```text
Idle: under 2%
Scrollback: under 20% during stress
```

## Input latency

Target:

```text
Keyboard input: under 10 ms UI-to-shell
Rendering: 60 FPS GPU, 30 FPS software
```

## Binary size

Target:

```text
Under 20 MB
Target 15 MB
```

---

# 24. Security Requirements Summary

The product must be safe against:

- ANSI injection
- title injection
- OSC injection
- malformed UTF-8
- malformed CSI
- OSC hyperlink abuse
- clipboard abuse
- config-based arbitrary command execution
- child process leak
- ConPTY leak
- unbounded memory growth
- parser panic
- renderer panic
- crash reporting leaks
- telemetry leaks
- auto-update abuse

Security rules:

```text
No code execution from terminal titles.
No auto-link opening.
No telemetry.
No admin.
No system service.
No registry modification.
No shell integration by default.
No clipboard access without user action.
No config auto-download.
No update auto-check.
```

`rustty secrets` (§9.33) limits what a compromised agent can reach. It is not a
boundary against code running as the same Windows user (SECRET-012).

---

# 25. Accessibility Requirements Summary

Required for Ghostty-like parity:

```text
Keyboard-only operation
UI Automation support
Accessible tab bar
Accessible pane grid
Accessible search UI
Accessible menus
High contrast theme support
Adjustable font size
High DPI
Per-monitor DPI
Screen reader text exposure
Focus management
```

---

# 26. Privacy Requirements Summary

Required:

```text
No telemetry
No analytics
No shell content collection
No clipboard collection
No config collection
No auto-update
No remote config
No remote shell integration
Crash logs local by default
Explicit consent for diagnostics
```

---

# 27. Open-Source Requirements Summary

Required:

```text
MIT license
Public repo
README
LICENSE
CONTRIBUTING
SECURITY
ROADMAP
CHANGELOG
Issue templates
PR template
CI
cargo fmt
cargo clippy
cargo check
cargo test
Release artifacts
Docs
No CLA
No telemetry
Privacy policy
```

---

# 28. Packaging Requirements Summary

MVP:

```text
Portable ZIP
No admin
No install
Local portable config
GitHub releases
Checksums
```

P1:

```text
winget
Signed builds
Release notes
Source archive
```

P3:

```text
MSIX
winget signing
winget auto-publish
```

---

# 29. Recommended Canonical Decisions

To move forward, lock these decisions:

## Decision 1: Product type

**Native Windows terminal emulator.**

Not a full POSIX environment.

Not a shell.

Not Electron.

## Decision 2: Primary language

**Rust.**

Reason:

- performance
- safety
- open-source adoption
- native Windows APIs
- good ecosystem
- maintainability

## Decision 3: License

**MIT.**

## Decision 4: Tabs

**Mandatory in MVP.**

Tabs are not stretch.

## Decision 5: Shell backend

**ConPTY.**

Native Windows practical tty.

## Decision 6: Shell targets

MVP:

```text
Git Bash
WSL
PowerShell
CMD
```

Stretch:

```text
MSYS2
Cygwin
zsh
fish
custom shells
```

## Decision 7: Rendering

MVP:

```text
DirectWrite + Direct2D
```

Stretch:

```text
wgpu
cosmic-text
```

## Decision 8: Config format

**TOML.**

## Decision 9: Telemetry

**None.**

## Decision 10: Ghostty parity

Full parity target:

```text
v0.4
```

MVP parity:

```text
tabs
native performance
no telemetry
MIT
config
themes
shell profiles
basic ANSI
copy/paste
scrollback
resize
TUI apps
```

---

# 30. SRS Summary Table

| Section | Status |
|---|---|
| Product vision | Defined |
| Platform | Windows 10/11 |
| Language | Rust |
| License | MIT |
| UI | Win32 native |
| Rendering | DirectWrite/Direct2D |
| Shell | ConPTY |
| Tabs | Required MVP |
| Splits | P1 |
| Themes | P0/P1 |
| Config | TOML, P0 |
| Profiles | P0 |
| Shell integration | P2 |
| Search | P1/P2 |
| Accessibility | P2 |
| IME | P2 |
| Quick terminal | P3 |
| Telemetry | None |
| Open-source | Required |
| Packaging | Portable ZIP MVP |
| Ghostty parity | v0.4 target |

---

# 31. SRS Approval Criteria

The specification is ready for implementation when these are accepted:

1. Rust is chosen.
2. MIT is chosen.
3. Windows-first is accepted.
4. Tabs are mandatory.
5. ConPTY is accepted as the Windows tty mechanism.
6. Git Bash/WSL/PowerShell/CMD are accepted MVP shells.
7. DirectWrite/Direct2D is accepted MVP renderer.
8. TOML is accepted config format.
9. No telemetry is mandatory.
10. Ghostty parity is staged to v0.4.
11. Portable ZIP is accepted MVP packaging.
12. MVP excludes shell integration.
13. MVP excludes splits.
14. MVP excludes full accessibility.
15. MVP excludes IME.
16. MVP excludes session restore.
17. MVP excludes graphics protocols.
18. MVP excludes Electron/web UI.
19. MVP excludes full POSIX shell implementation.
20. MVP excludes full Ghostty parity.

---

# 32. Next Steps

## Step 1: Freeze MVP scope

Freeze the MVP list:

```text
Tabs
ConPTY
Git Bash/WSL/PowerShell/CMD
ANSI parser
DirectWrite/Direct2D
TOML config
Profiles
Themes
Scrollback
Copy/paste
Keyboard
Mouse selection
Resize
Ctrl-C/Ctrl-D
Clean exit
Portable mode
MIT license
No telemetry
CI
```

## Step 2: Choose repository name

Recommend:

```text
rustyterm
openwindows-terminal
rust-terminal
tabterm
```

## Step 3: Create repo skeleton

Initialize:

```bash
cargo init rustyterm
```

Create:

```text
src/
docs/
.github/
tests/
```

Add:

```text
LICENSE
README.md
CONTRIBUTING.md
SECURITY.md
ROADMAP.md
CHANGELOG.md
Cargo.toml
rustfmt.toml
clippy.toml
.github/workflows/ci.yml
```

## Step 4: Implement tab model first

Start with:

```rust
struct App
struct Tab
struct Pane
struct Grid
struct ShellSession
struct Config
struct Profile
struct Theme
```

Do not begin with rendering until tab/shell/grid model exists.

## Step 5: Implement ConPTY prototype

Target:

```text
Create ConPTY
Spawn Git Bash
Read bytes
Print raw bytes to console
Write keyboard input
Resize
Close
```

This validates the hardest part.

## Step 6: Implement parser prototype

Target:

```text
UTF-8 decode
CSI parser
OSC parser
cursor movement
erase line
erase screen
16 colors
256 colors
true color
```

Do not add rendering until parser has unit tests.

## Step 7: Render MVP

Target:

```text
Win32 window
DirectWrite font
Direct2D grid
tab bar
cursor
selection
scrollback
```

## Step 8: Polish tabs

Target:

```text
create
close
focus
reorder
title
color
keyboard
close confirmation
theme
```

---

# 33. Final Specification Position

This project makes sense as a Rust, MIT-licensed, native Windows terminal emulator.

Rust is appropriate because it supports:

- native performance
- safety
- Windows APIs
- ConPTY
- DirectWrite/Direct2D
- low resource usage
- open-source contribution
- maintainable parser architecture
- low-risk FFI boundaries

Tabs are mandatory.

Ghostty-like feature parity should be a staged target:

```text
MVP: tabs + native performance + shell profiles + ANSI/TTY
v0.2: splits + search + themes + ligatures + mouse reporting
v0.3: shell integration + accessibility + IME + polish
v0.4: Ghostty-like parity
v0.5: stretch features
```
