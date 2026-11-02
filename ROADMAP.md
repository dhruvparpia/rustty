# rusttyterm Roadmap

Release plan derived from [`docs/SPEC.md`](docs/SPEC.md) (sections 24–27).
Versions ship in order; features move down only by explicit decision.

## v0.1 — MVP (target: 2026-12-31 → 2027-Q1)

A native Windows terminal that developers use daily.

| Category | Feature | Priority |
|---|---|---|
| Core | Tabs: add, close, switch, reorder, `+` button, tab close destroys shell | P0 |
| Core | Tab bar always visible (1 tab shown with `+`; hide-tab-bar setting exists) | P0 |
| Core | ConPTY shell backend; default shell = Git Bash, configurable per profile | P0 |
| Core | DirectWrite + Direct2D renderer, GPU-accelerated | P0 |
| Core | VT parser: SGR 16/256/true color, common CSI/OSC, UTF-8 | P0 |
| Core | TOML config + profiles; default config location | P0 |
| Core | Ghostty-compatible keybindings (tab new/close/next/prev, copy/paste, scroll, fullscreen, quit) | P0 |
| Core | Scrollable output + scrollback; visual bell | P0 |
| Core | Theme presets (Predawn and friends); theme in config | P0 |
| Core | Windows Jump List (native taskbar API) | P1 |
| Perf | 60 fps under load (glamor stress), 10k-line burst in < 1 s | P0 |
| Perf | Zero allocations in render hot path | P0 |
| Perf | Fast startup, low idle RAM vs Windows Terminal | P1 |
| Ship | Portable ZIP packaging; MIT LICENSE in package | P0 |
| Ship | No telemetry, no phone-home | P0 |

**Explicitly NOT in MVP:** splits, shell integration, full screen-reader
accessibility, IME, session restore, quick terminal, theme browser, graphics
protocols, plugin system, Electron/web UI of any kind.

## v0.2 — Ghostty feature parity, part 1 (target: 2027-Q2)

| Feature | Notes |
|---|---|
| Splits | Pane tree (spec §9/§10); panes are not tabs; close pane = destroy process |
| Visual effects | Blending, transparency, blur (DWM), animation — Direct2D side |
| Window controls | Resize, fullscreen, maximize, snap layouts (spec §8) |
| Themes | Preset browser, live theme switching |
| Session restore | Reopen tabs/panes on relaunch |
| Desktop notifications | Windows toast |
| Quick terminal | Hotkey summon from screen edge |
| Windows Jump List | Full implementation (MVP carries the P1 item) |

## v0.3 — Linux (target: 2027-Q3)

| Feature | Notes |
|---|---|
| Wayland backend | **TBC — confirm compositor target list before starting** |
| X11 backend | Baseline support for all X11 compositors (spec §25) |
| Wayland protocol parity | `fractional-scale-v1` required; viewport, IME, clipboard (spec §26) |
| Linux packaging | deb/rpm + portable builds |

## v0.4 — Ghostty parity (target: 2027-Q4 / 2028-Q1)

Full feature parity with Ghostty, verified against the spec parity matrix (§27).
Remaining accessibility (screen reader matrix), IME parity, key-binding parity,
terminal capability parity.

## v0.5+ — Ecosystem (2028+)

| Feature | Status |
|---|---|
| Graphics protocols (kitty, 6bit SGR) | Done 2026-11-02: kitty subset (transmit/put/delete/query, PNG/JPEG via WIC); 6bit SGR renders as text (SGR 256 + block chars) |
| Plugin system / MCP | Design phase done: [`docs/design-plugins-mcp.md`](docs/design-plugins-mcp.md); implementation not scheduled |
| Mobile / tablet | Design phase done: [`docs/design-mobile-tablet.md`](docs/design-mobile-tablet.md); implementation not scheduled |
| Windows ARM64 | Done 2026-11-02: cross-check gate in CI; ARM64 release ZIP ships with the v0.5 packaging |
| macOS backend | Open — no committed date |

## Performance budgets (all versions)

- 60 fps sustained under `glamor`-style load; damage-only redraw.
- 10k-line burst rendered in < 1 s.
- Zero allocations in the render hot path.
- Startup and idle-RAM at or better than Windows Terminal.
- Tab switch, tab create, tab close: perceptibly instant (< 16 ms).
- Config reload: hot, < 100 ms, no window restart.
