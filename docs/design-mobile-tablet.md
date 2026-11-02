# Mobile / tablet — design phase only

Roadmap v0.5 ("Mobile / tablet — design phase only"). A design document:
nothing here is implemented. SPEC.md stays the source of truth.

## Decision: one app, adaptive input — not a phone app

rusttyterm stays a native Win32/Direct2D app. The same binary runs on
Windows on ARM tablets (the ARM64 build, roadmap v0.5); touch changes the
*input layer*, never the renderer, parser, or PTY path. There is no
separate mobile binary, no XAML/WinUI rewrite, no Electron webview
(MVP exclusions stay excluded).

## Shape

- **Touch text selection**: long-press starts selection, draggable
  start/end handles snap to cell boundaries, double-tap selects a word,
  triple-tap a line. Copy goes through the existing clipboard path.
- **On-screen key row**: when a soft keyboard is present, a fixed row
  above it carries Esc, Tab, Ctrl, Alt, arrows, and a key that opens the
  full modifier layer. Ctrl/Alt act as latched modifiers (next key gets
  the modifier).
- **Gestures**: two-finger pinch changes font size (same clamp as
  `Ctrl+Plus`); two-finger scroll scrolls the viewport; swipe on the tab
  strip switches tabs. All gestures honour `prefers-reduced-motion`
  equivalents: the config `animations` switch.
- **Targets**: touch mode (auto-detected pointer) enforces >= 44 px
  targets: tab bar height, `+` button, split borders (the existing
  border-drag strips widen), search-strip controls. Mouse mode keeps the
  compact layout.
- **Quick terminal** becomes a top-docked edge window summoned by an
  edge swipe (it already exists since v0.2).

## Boundaries

- Layout, damage, and budgets are unchanged: 60 fps and zero hot-path
  allocations apply to touch frames too; gesture handling must not
  allocate per frame.
- DPI: Windows scales already apply; per-monitor DPI handling is the
  existing renderer path, no mobile-only scaling.
- No on-screen keyboard of our own; the Windows soft keyboard is
  augmented by the key row.

## Explicit non-goals

- A phone-first app, an app-store submission, ARM phone support.
- Touch-only layouts that hide the tab bar, splits, or menus.
- Cloud sync, session handoff between devices, SSH client mode.
- Pen/ink input beyond plain touch (no handwriting).

## Open questions (resolve before any implementation)

1. Soft-keyboard geometry: where exactly the key row lives when the
   Windows touch keyboard docks, and how it detects the dock.
2. Auto-hiding the key row in fullscreen.
3. Snap-layout behaviour on 10" tablets (the v0.2 snap layouts are a
   desktop feature).
