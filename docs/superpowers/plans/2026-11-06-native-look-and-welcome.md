# Native Look and Startup Crab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make rustty open as one seamless dark window with tabs in the title bar and greet the user with a clacking crab banner on start.

**Architecture:** Pure, portable logic goes in small new modules (`cellart`, `caption`, `welcome`) with unit tests. `render.rs` and `window.rs` only call them. The banner is ordinary terminal text fed through `Pane::feed` before the shell spawns. The animation is a bounded state machine that rewrites a few cells and stops when those cells are no longer what it wrote.

**Tech Stack:** Rust 1.99.1, `windows` 0.62 (Direct2D, DirectWrite, DWM, Win32), Symbols Nerd Font Mono.

**Spec:** `docs/superpowers/specs/2026-11-06-native-look-and-welcome-design.md`. Approved mockup: `docs/superpowers/specs/2026-11-06-native-look-mockup.html`. Read both before starting.

## Global Constraints

- AGENTS.md applies: latest stable dependencies, `#![deny(unsafe_code)]` outside FFI modules with a reasoned `allow`, doc comments on public items, no panics in the render loop, parser or PTY read loop.
- Gate before every commit: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings -D clippy::unwrap_used -D clippy::expect_used`, `cargo test`, and `cargo check --target x86_64-unknown-linux-gnu --all-targets`.
- Run tests with `APPDATA` and `LOCALAPPDATA` pointing at a scratch folder. Host tests call `App::new()`, which otherwise uses real user state.
- Zero allocations in the render hot path.
- No network use, no telemetry, no web UI.
- Commit on branch `feat/native-look`. End each commit message with the attribution line from the session. Do not push until the owner asks.
- Each task updates the spec rows, `config.example.toml` and `CHANGELOG.md` entries for its own behaviour.

## Review Focus

1. **A burst of output during the clack** (a profile script printing thousands of lines): the crab must stop cleanly and never write over other text. Test in Task 9.
2. **A light Windows theme:** the `>_` and all banner text stay readable. Test in Task 8.
3. **The top-right corner of a maximized window:** the very last pixel closes the window, and the top edge is not a dead zone. Test in Task 5.
4. **`ESC c`, or a new tab or split after a theme change:** colours still follow the current theme. Tests in Task 1.
5. **A profile that already sets `Path`, in any letter case:** it is extended once, not replaced or duplicated. Test in Task 7.

---

### Task 1: Copper default, and theme palettes that colour output

**Files:**
- Modify: `src/parser.rs`, `src/pane.rs`, `src/app.rs`, `src/theme.rs`, `src/config.rs`, `src/window.rs` (the four `s.theme =` sites at about 786, 5814, 5937 and 6315), `src/color.rs`
- Test: `src/parser_tests.rs`, `src/app.rs`, `src/theme.rs`
- Docs: `docs/copper-skin.md`, `docs/SPEC.md` (§14 default theme, THEME-010, SKIN rows)

**Interfaces:**
- Produces: `Parser.palette: [Color; 16]`, default `ansi_palette()`; `Pane::set_palette(&mut self, [Color; 16])`; `App::set_palette(&mut self, [Color; 16])`, which updates every pane and is remembered for new tabs and splits; `Color::mix(self, other: Color, t: f32) -> Color`; `Theme::strip_color(&self) -> Color`; `UiOverrides.strip: Option<Color>`.

- [ ] **Step 1: Create the branch and commit the design**

```bash
git switch -c feat/native-look
git add docs/superpowers
git commit -m "docs: native look and startup crab design and plan"
```

- [ ] **Step 2: Write the failing tests**

```rust
// parser_tests.rs
#[test] fn sgr_16_colors_use_the_parser_palette() {
    // p.palette[1] = Color::rgb(1, 2, 3); feed "\x1b[31m"
    assert_eq!(p.pen_fg, Some(Color::rgb(1, 2, 3)));
}
#[test] fn sgr_bright_and_256_low_indices_use_the_parser_palette() {
    // palette[9] via "\x1b[91m", and palette[2] via "\x1b[38;5;2m"
}
#[test] fn sgr_256_cube_ignores_the_palette() {
    // "\x1b[38;5;196m" equals ansi256(196)
}
#[test] fn reset_keeps_the_palette() {
    // set palette, feed "\x1bc", then "\x1b[31m" still gives the custom colour
}
// app.rs
#[test] fn new_tabs_and_splits_use_the_app_palette() {}
// theme.rs
#[test] fn default_and_unknown_themes_are_copper_dark() {}
#[test] fn light_system_maps_to_copper_light() {} // extract the mapping as a pure fn(bool) -> &str
#[test] fn strip_is_the_background_unless_tab_bar_is_set_explicitly() {
    // every preset: strip_color() == background; config with tab-bar = "#010203": that value
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test --lib palette` and `cargo test --lib theme::`
Expected: failures or compile errors for the missing items.

- [ ] **Step 4: Implement**

Parser lines 1207-1210 and 1168 read `self.palette`; `reset` restores it after `Self::new()`. `DEFAULT_THEME = "copper-dark"`, and `system_preset` returns `copper-light` for a light system. Set copper-dark `tab_active` to `#202328` as in the mockup, then re-check the existing SKIN-004 contrast tests. An explicit `[theme] tab-bar` fills `ui.strip`. Update existing tests that name `predawn` as the default, and keep `predawn` as a preset. Call `App::set_palette` at all four theme sites. Host parsers keep the default palette; note this in the changelog.

- [ ] **Step 5: Run the gate, then commit**

Expected: all pass. Commit: `feat(theme): copper default and theme palettes colour output`.

---

### Task 2: Native block and Powerline drawing

**Files:**
- Create: `src/cellart.rs` (portable, registered in `lib.rs`)
- Modify: `src/render.rs` (`paint_row_text` at about 2307), `src/perf_bench.rs`
- Docs: `docs/SPEC.md` (font section)

**Interfaces:**
- Produces: `cellart::shape(ch: char) -> Option<Shape>`, where `Shape` is `Fill { rects: [Rect01; 4], count: u8, alpha: f32 }` or `Triangle(PowerlineSide)`, and rectangles are fractions of a cell. No allocation.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn upper_half_block_fills_the_top_half() {
    // shape('▀') is one rect: x 0..1, y 0..0.5, alpha 1
}
#[test] fn lower_half_and_full_blocks() {} // '▄' y 0.5..1, '█' 0..1
#[test] fn quadrants_cover_the_right_corners() {} // '▘' and '▟' (three rects)
#[test] fn eighths_and_shades() {} // '▁' y 0.875..1, '▏' x 0..0.125, '░' '▒' '▓' alpha 0.25, 0.5, 0.75
#[test] fn every_block_element_has_a_shape() {} // all of U+2580..=U+259F
#[test] fn powerline_separators_are_triangles() {} // U+E0B0..=U+E0B3; 'a' is None
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib cellart`
Expected: compile error, module missing.

- [ ] **Step 3: Implement `cellart`, then draw it in `paint_row_text`**

Check `cellart::shape` first in the row loop and in the run-folding loop, so a shaped cell is never drawn as text. Snap rectangle edges to whole device pixels so neighbouring rows share an edge. Triangles use `ID2D1PathGeometry` objects cached on `Renderer` and rebuilt when metrics change. Filled rects use a brush with `alpha` applied through `Color::mix` toward the cell background.

- [ ] **Step 4: Add a desktop pixel test to `perf_bench.rs`**

`#[ignore]` test `block_rows_have_no_gaps`: paint a column of `█` over 6 rows at line heights 1.0 and 1.2, capture with the existing `PrintWindow` helper, and assert that no scanline in the column has the background colour.

- [ ] **Step 5: Verify**

Run the gate, then `cargo test --release --lib -- --ignored --nocapture --exact perf_bench::block_rows_have_no_gaps --test-threads=1`
Expected: pass. Commit: `feat(render): draw block and Powerline characters as geometry`.

---

### Task 3: Bundled symbols font as a fallback

**Files:**
- Create: `assets/fonts/SymbolsNerdFontMono-Regular.ttf`, `assets/fonts/LICENSE`
- Modify: `src/render.rs` (`make_format` at about 305), `src/glyph_cache.rs` if it builds its own format, `packaging/build-portable.ps1` (copy the licence), `README.md`
- Docs: `docs/SPEC.md` (FONT-003 note)

**Interfaces:**
- Produces: `render::symbol_fallback(dwrite: &IDWriteFactory) -> Option<IDWriteFontFallback>`, built once. `None` means today's behaviour, with a logged line.

- [ ] **Step 1: Fetch the font**

Confirm the latest stable release with `gh api repos/ryanoasis/nerd-fonts/releases/latest --jq .tag_name` (v3.5.1 when the spec was written). Download `NerdFontsSymbolsOnly.zip` into a new empty folder in the scratchpad, extract it, and copy the Mono TTF and licence into `assets/fonts/`. Record the version in the commit message.

- [ ] **Step 2: Write the failing Windows tests**

```rust
#[cfg(windows)] #[test] fn powerline_and_icon_glyphs_map_to_the_bundled_font() {
    // MapCharacters on U+E0A0 and U+F115 with the fallback, starting from "Cascadia Mono":
    // the mapped family name contains "Symbols Nerd Font"
}
#[cfg(windows)] #[test] fn a_font_with_the_glyph_keeps_it() {
    // 'A' from "Cascadia Mono": no fallback is reported (mapped length 0 or the same family)
}
#[cfg(windows)] #[test] fn non_symbol_text_still_uses_system_fallback() {
    // U+4E2D does not map to the symbols font
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test --lib symbol`
Expected: compile error.

- [ ] **Step 4: Implement**

Embed with `include_bytes!` under `cfg(windows)`. Load through `IDWriteFactory5::CreateInMemoryFontFileLoader` into a private collection. Build with `IDWriteFontFallbackBuilder`: map the private-use ranges U+E000-F8FF and U+F0000-FFFFD to the symbols family, then add the system fallback. Apply through `IDWriteTextFormat1::SetFontFallback` to every terminal format and to `TextLayout`s created from them. Wide glyphs stay clipped to the cell.

- [ ] **Step 5: Verify**

Run the gate. Expected: pass. Check `target/release/rustty.exe` grew by roughly 2.5 MB. Commit: `feat(render): bundle Symbols Nerd Font Mono <version> as a fallback`.

---

### Task 4: Quiet tab strip and more padding

**Files:**
- Modify: `src/tabbar.rs`, `src/render.rs` (`paint_tab_bar` at about 1830), `src/window.rs` (callers of `tabbar::` functions), `src/config.rs` (`Padding` default)
- Docs: `docs/SPEC.md` (SKIN-003, WINDOW-011, tab bar rows), `docs/copper-skin.md`

**Interfaces:**
- Consumes: `Theme::strip_color` (Task 1).
- Produces: `tabbar::TAB_BAR_HEIGHT = 36.0`; `tabbar::UI_CHAR_WIDTH = 7.0`, an estimated Segoe UI advance passed wherever `cell_width` was passed for tabs; `layout_into` places tabs from x = 0 and `plus` directly after the last visible tab.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn plus_follows_the_last_tab() {
    // two 100-wide tabs: tabs start at 0, plus starts at the end of tab 2
}
#[test] fn plus_stays_inside_the_bar_when_tabs_overflow() {}
#[test] fn hit_test_finds_tabs_and_plus_in_the_new_order() {}
#[test] fn default_padding_is_twelve_by_ten() {} // Padding::default() x = 12, y = 10
#[test] fn explicit_zero_padding_is_honoured() {} // [window] padding = { x = 0, y = 0 }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib tabbar` and `cargo test --lib padding`
Expected: new tests fail. Update existing tests that assert `+` at the left.

- [ ] **Step 3: Implement**

`paint_tab_bar`: fill the strip with `strip_color()`, with no bottom border line, because the strip must be seamless. The active tab fills `tab_active` with a 2-DIP `ui.accent` bar on top. Inactive tabs have `ui.muted` text and no fill. Titles use a new `IDWriteTextFormat` with Segoe UI at 12.5 DIP, ellipsis trimming and vertical centring. Replace `tabbar::fit_title` use with that trimming if it is then unused. Draw a tab icon only when the profile configures one; remove the default shell glyph, which the mockup does not have. Keep tab colours, bell flash and marquee behaviour.

- [ ] **Step 4: Verify**

Run the gate. Expected: pass. Commit: `feat(ui): seamless tab strip and default window padding`.

---

### Task 5: Tabs in the title bar

**Files:**
- Create: `src/caption.rs` (portable, registered in `lib.rs`)
- Modify: `src/window.rs` (`wnd_proc`, `init_window`, `toggle_fullscreen`, `cell_at`, `bar_top`, `bar_height`), `src/render.rs` (`grid_top`, `tab_bar_top`, a new strip paint)
- Docs: `docs/SPEC.md` (SKIN-006), `docs/copper-skin.md` (caption claims)

**Interfaces:**
- Consumes: `tabbar` from Task 4.
- Produces, in `caption`: `BUTTON_WIDTH = 46.0`, `STRIP_HEIGHT = 36.0`, `DRAG_GAP = 46.0`; `enum Hit { Client, Caption, Min, Max, Close, TopEdge }`; `hit_test(x: f32, y: f32, width: f32, ctx: Ctx) -> Hit`, with `Ctx { maximized: bool, resize_border: f32, tabs_end: f32 }` where `tabs_end` is the end of the `+` button; `tab_area_width(width: f32) -> f32`, the client width minus the three buttons and `DRAG_GAP`.

**Decision:** rustty draws the three buttons and answers `WM_NCHITTEST` with `HTMINBUTTON`, `HTMAXBUTTON` and `HTCLOSE`, because the opaque Direct2D target would cover DWM's native buttons. `HTMAXBUTTON` is what gives Windows 11 its Snap Layouts flyout.

- [ ] **Step 1: Write the failing tests**

```rust
// at width 1000: Close 954..1000, Max 908..954, Min 862..908
#[test] fn buttons_are_on_the_right() {}
#[test] fn top_right_corner_of_a_maximized_window_is_close() {
    // x = 999.5, y = 0, maximized: Hit::Close
}
#[test] fn top_edge_resizes_unless_maximized() {
    // y = 2, x = 500, resize_border 6: TopEdge; maximized: Caption
}
#[test] fn empty_strip_after_the_tabs_drags_the_window() {} // x = tabs_end + 1: Caption
#[test] fn tabs_and_plus_stay_clickable() {} // x < tabs_end: Client
#[test] fn below_the_strip_is_client() {} // y = 36: Client
#[test] fn narrow_window_keeps_all_three_buttons() {} // width 200: Close 154..200; tab_area_width >= 0
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib caption`
Expected: compile error.

- [ ] **Step 3: Implement `caption`**

Run: `cargo test --lib caption`
Expected: pass.

- [ ] **Step 4: Wire the frame in `wnd_proc`**

- `WM_NCCALCSIZE` with `wparam` true, on non-quick, non-fullscreen windows: call `DefWindowProcW`, then restore `rgrc[0].top`. When maximized, inset the top by `GetSystemMetricsForDpi(SM_CYSIZEFRAME) + SM_CXPADDEDBORDER`.
- `WM_NCHITTEST`: `DefWindowProcW` first; if it returns `HTCLIENT`, map `caption::hit_test` to the matching `HT*` code.
- `WM_NCLBUTTONDOWN` and `WM_NCLBUTTONUP` on the three button codes: consume them and send `SC_MINIMIZE`, `SC_MAXIMIZE`, `SC_RESTORE` or `SC_CLOSE` on release over the same button. `HTCAPTION` falls through, so drag, double-click maximize and the system menu stay native.
- `WM_NCMOUSEMOVE` and `WM_NCMOUSELEAVE` (`TrackMouseEvent` with `TME_NONCLIENT`): hover state.
- `init_window`: `DwmExtendFrameIntoClientArea` with a 1-pixel top margin, then `SetWindowPos` with `SWP_FRAMECHANGED`.

- [ ] **Step 5: Paint and lay out the strip**

The strip is always `STRIP_HEIGHT` on normal windows and 0 on quick and fullscreen windows. Top-placed visible tabs sit inside it. With tabs hidden, `auto` with one tab, or bottom placement, it shows the window title in `ui.muted` on the left. Pass `caption::tab_area_width` to `layout_into`. Buttons use the existing icon font glyphs U+E921, U+E922 or U+E923 for restore, and U+E8BB. Hover uses a raised fill; Close hover uses `#C42B1C` with white. Update `grid_top`, `cell_at`, and every use of `bar_height` and `bar_top` so mouse cells stay correct.

- [ ] **Step 6: Check on the desktop**

Launch the release build. Verify drag, double-click, Snap flyout, all resize edges, maximize and restore, fullscreen and back, `tab-bar = "hidden"`, `"auto"` and bottom placement, and quick terminal unchanged. Expected: all native behaviour intact.

- [ ] **Step 7: Run the gate, then commit**

Commit: `feat(ui): tabs in the title bar with drawn caption buttons`.

---

### Task 6: Menu bar becomes an Alt popup

**Files:**
- Modify: `src/window.rs` (`build_menu` at 340, `SetMenu` at 1688 and 1937, `WM_SYSKEYDOWN` at 1703, new `WM_SYSKEYUP`), `src/palette.rs` if rows are missing
- Create pure helper in: `src/keyboard.rs`
- Docs: `docs/SPEC.md` (MENU-001..005), `README.md`

**Interfaces:**
- Produces: `keyboard::AltTap` with `key_down(vk: u16, ctrl: bool)`, `key_up(vk: u16) -> bool` (true when a lone Alt was tapped) and `cancel()`; `window::MENU_COMMANDS: &[(usize, &str)]`, each menu id with its palette action label, used to build the menu's fixed items.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn lone_alt_tap_is_detected() {}
#[test] fn alt_with_another_key_is_not_a_tap() {} // Alt+X, Alt+Tab
#[test] fn altgr_is_not_a_tap() {} // ctrl held
#[test] fn mouse_click_during_alt_cancels() {}
#[cfg(windows)] #[test] fn every_menu_command_has_a_palette_action() {
    // each MENU_COMMANDS label exists in the palette's action registry
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib alt_` and `cargo test --lib every_menu_command`
Expected: compile errors.

- [ ] **Step 3: Implement**

Remove both `SetMenu` calls. `build_menu` returns a popup root, built with `CreatePopupMenu`, and is rebuilt when profiles or themes change, as today. A lone Alt tap opens it with `TrackPopupMenu` at the strip's left edge. Results go through the existing `WM_COMMAND` path, and `WM_INITMENUPOPUP` keeps working. Add palette rows for any command the parity test finds missing. Alt combinations must still reach the terminal.

- [ ] **Step 4: Run the gate, then commit**

Commit: `feat(ui): replace the menu bar with an Alt popup`.

---

### Task 7: `rustty` on PATH inside its own shells

**Files:**
- Modify: `src/shell.rs`, `src/window.rs` (`spec_opts` at 5744), `src/host.rs` (`create_session` at about 880)
- Docs: `docs/SPEC.md` (new row near SHELL-021)

**Interfaces:**
- Produces: `shell::path_with_dir(current: &str, dir: &str) -> Option<String>`, `None` when already present; `SpawnOptions::add_own_dir(&mut self, dir: &str, inherited_path: &str)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn dir_is_appended_last() {
    assert_eq!(path_with_dir(r"C:\a;C:\b", r"C:\r"), Some(r"C:\a;C:\b;C:\r".into()));
}
#[test] fn existing_dir_is_not_added_twice() {
    // case and a trailing backslash are ignored: "C:\\A\\;C:\\b" with "c:\\a" gives None
}
#[test] fn empty_path_gets_just_the_dir() {}
#[test] fn profile_path_is_extended_whatever_its_case() {
    // env {"Path": "C:\\mine"}: stays one key, value "C:\\mine;C:\\r"
}
#[test] fn without_a_profile_path_the_inherited_one_is_extended() {}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib path_with_dir` and `cargo test --lib profile_path`
Expected: compile errors.

- [ ] **Step 3: Implement and wire**

Use `std::env::current_exe()` for the folder; on failure, skip silently. Call it in `spec_opts` and in host `create_session`.

- [ ] **Step 4: Check on the desktop**

Launch rustty and run `rustty --version` in cmd, PowerShell and Git Bash, and `rustty.exe --version` in WSL. In cmd, run `set path` and confirm one `Path` entry that ends with rustty's folder. Expected: `rustty 0.6.0` each time.

- [ ] **Step 5: Run the gate, then commit**

Commit: `feat(shell): add rustty's folder to the PATH of its shells`.

---

### Task 8: Banner content and layout

**Files:**
- Create: `src/welcome.rs` (portable, registered in `lib.rs`)
- Modify: `src/pane.rs`, `src/config.rs`, `src/settings.rs`, `src/color.rs`
- Docs: `docs/SPEC.md` (new `WELCOME-*` rows), `config.example.toml`

**Interfaces:**
- Consumes: `Color::mix` (Task 1).
- Produces, in `welcome`:
  - `enum Frame { Open, Closed }`
  - `struct Colors { accent, muted, foreground, raised: Color }`, with `Colors::from_theme(&Theme)` and `raised = background.mix(foreground, 0.07)`
  - `struct Banner<'a> { version: &'a str, cols: usize, rows: usize, exe: &'a str, tip: &'a str, colors: Colors, frame: Frame }`
  - `Banner::render(&self) -> Option<String>`, ANSI text ending at the start of a fresh line, `None` when it does not fit
  - `crab_cells(frame: Frame, c_color: Color) -> [[Cell; 26]; 8]`
  - `exe_for_program(program: &str) -> &'static str`, `rustty.exe` for `wsl` and `wsl.exe`, else `rustty`
  - `pick_tip(seed: u64) -> &'static str`, `TIPS: &[&str]`
  - `struct Launch { quick, workspace, attach, view, welcome_enabled, first_instance: bool }`, with `should_show(&Launch) -> bool`
- Produces: `Pane::fit_grid(&mut self, cols: usize, rows: usize)`, which resizes grid and parser with no shell. Config key `welcome: bool`, default true.

- [ ] **Step 1: Write the failing tests**

Tests feed `render()` into `Pane::with_size(cols, rows, 100)` and read the grid. Use the crab frames and strings from the spec.

```rust
#[test] fn crab_half_blocks_match_the_frames() {
    // row 0 col 3 of Open: top 'l' over bottom 'l' from the claw rows gives the expected
    // char, fg and bg. Rule: both empty ' '; top only '▀'; bottom only '▄';
    // same colour '█'; two colours '▀' fg top, bg bottom.
}
#[test] fn open_and_closed_differ_only_in_the_top_three_cell_rows() {}
#[test] fn prompt_uses_the_given_colour() {} // the 'c' cells use c_color
#[test] fn full_banner_at_78_cols() {
    // col 28 of row 0 starts "rustty 0.6.0"; row 2 "Ctrl+Shift+P" with "Search every command"
    // at col 52; row 6 "welcome = false"; "Tip: " at col 0 of row 9; cursor on row 11, col 0
}
#[test] fn crab_drops_at_77_cols() {} // no half blocks; "rustty 0.6.0" at col 0
#[test] fn banner_drops_below_52_cols_or_14_rows() {} // None at (51, 24) and (80, 13)
#[test] fn wsl_uses_the_exe_name_and_still_fits() {} // exe "rustty.exe" at 78 cols: no text past col 77
#[test] fn every_tip_fits_on_one_line() {} // "Tip: " + tip <= 78 chars, for all TIPS
#[test] fn tip_is_left_out_when_it_would_wrap() {} // a 70-char tip at 60 cols: no "Tip:" row
#[test] fn banner_leaves_the_pen_reset() {} // pen_fg, pen_bg None and pen_attrs empty afterwards
#[test] fn banner_never_writes_to_the_shell() {} // Pane has no shell; rendering emits no query sequences
#[test] fn light_theme_stays_readable() {
    // Colors::from_theme(copper-light): accent, muted and foreground on background and raised
    // meet the SKIN-004 ratios: 4.5:1 for text, 3:1 for accent
}
#[test] fn appearance_rules() {
    // table from the spec: plain yes; quick, workspace, attach, view, disabled or
    // not first_instance: no
}
#[test] fn welcome_config_key_defaults_on_and_parses_false() {}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib welcome`
Expected: compile error.

- [ ] **Step 3: Implement**

Tips: write about eight, each naming a real feature with its real default shortcut from `palette.rs` or `config.example.toml`, starting with the spec's `Ctrl+Shift+K` tip. Render with truecolor SGR, positioning columns with `CSI n G`. Status at this stage is the version only; Task 9 adds the rest.

- [ ] **Step 4: Run the gate, then commit**

Commit: `feat(welcome): banner content, layout and appearance rules`.

---

### Task 9: The clack, and showing it on start

**Files:**
- Modify: `src/welcome.rs`, `src/grid.rs`, `src/pane.rs`, `src/tab.rs`, `src/host.rs` (client), `src/window.rs` (`init_window` at about 1925, `WM_TIMER`, `WindowState`)
- Docs: `docs/validation-matrix.md`, `README.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: `Banner`, `crab_cells`, `should_show`, `Pane::fit_grid` (Task 8).
- Produces:
  - `Grid::cell_abs_mut(&mut self, abs: usize, col: usize) -> Option<&mut Cell>`, marks the line dirty and does not bump the search revision
  - `Pane::input_seen(&self) -> bool`, set by `send_input` and not by terminal replies; `Pane::mark_dirty(&mut self)`; `Pane::in_alt(&self) -> bool`
  - `Tab::focused_pane_weak(&self) -> Weak<RefCell<Pane>>`
  - `welcome::Clack::new(anchor_abs: usize, c_color: Color, animate: bool, status: Option<Receiver<Option<usize>>>, status_at: (usize, usize), colors: Colors)`
  - `Clack::step(&mut self, pane: &mut Pane) -> Option<Duration>`, the delay until the next step, or `None` when finished
  - `welcome::status_text(sessions: Option<usize>) -> String`
  - `host::client::live_session_count() -> Option<usize>`, using `connect_existing`, so it never starts a host
  - `welcome::InstanceGuard::acquire(name: &str) -> Option<InstanceGuard>`, Windows only: a named mutex, `None` when it already exists or creation fails

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn sequence_is_open_closed_open_closed_open_closed() {
    // delays in order 420, 130, 170, 130, 170 ms, then None; final cells are the Closed frame
}
#[test] fn scrolling_shell_output_does_not_stop_the_clack() {
    // feed lines below the banner between steps: every step still applies
}
#[test] fn overwritten_crab_stops_and_is_left_alone() {
    // overwrite a claw cell, step: None, and the overwritten cell is unchanged
}
#[test] fn burst_that_evicts_the_crab_stops_safely() {
    // small scrollback, feed 10,000 lines mid-sequence: None, no panic, later lines intact
}
#[test] fn cleared_screen_stops_the_clack() {} // "\x1b[2J\x1b[H": None, screen stays blank
#[test] fn alt_screen_stops_the_clack() {}
#[test] fn typing_stops_the_clack() {}
#[test] fn without_animation_the_frame_never_changes() {
    // animate false: same delays, so the status can still arrive; crab stays Closed
}
#[test] fn status_is_filled_in_when_it_arrives() {
    // Some(2) gives " · session host running · 2 sessions"; Some(1) gives "1 session"
}
#[test] fn no_host_leaves_the_version_alone() {} // None: text unchanged
#[test] fn status_is_not_written_over_other_text() {}
#[cfg(windows)] #[test] fn second_instance_is_refused_until_the_first_is_gone() {
    // unique name: acquire is Some, then None; drop the first, acquire is Some again
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib welcome::`
Expected: failures or compile errors.

- [ ] **Step 3: Implement `Clack` and the helpers**

Before each write, compare the claw cells with the frame last written. Any difference, `None` from the grid, `in_alt`, or `input_seen` ends the sequence without writing. The first step only compares and schedules, because the banner renders `Open` when animating.

- [ ] **Step 4: Wire `init_window`**

Acquire `InstanceGuard` in `window::run` only, and keep it for the life of the process, so CLI verbs and the host never take it. Name: `Local\rustty-gui`.

In the non-restored path, and before `spawn_restored` in the restored path: if `should_show`, call `fit_grid` with the real size, `feed` the rendered banner to the focused pane, then spawn the shell. Note that `spawn_active` currently runs before `resize_active`, and the banner must be written at the final size. Start a named thread that sends `live_session_count()` over a channel. Read `SPI_GETCLIENTAREAANIMATION` for `animate`.

Store the `Clack` and the pane `Weak` in `WindowState`. Drive them with a new `CLACK_TIMER`, using the delay from each `step`. On `None`, or when the pane is gone, call `KillTimer` and drop both. After each step, mark the pane dirty and invalidate.

- [ ] **Step 5: Run the gate, then commit**

Commit: `feat(welcome): clacking crab on start`.

---

### Task 10: Desktop check and hand over

**Files:**
- Modify: `docs/validation-matrix.md`, `README.md` (out-of-date status text), `AGENTS.md` only if the owner agrees

- [ ] **Step 1: Run the desktop matrix and record real results**

For cmd, Windows PowerShell, Git Bash and WSL (pwsh if installed): the banner appears, the claws clack twice, the prompt sits below it, and it survives a resize. Also: 100, 150 and 200% DPI if available; light and dark Windows; a second window shows no crab; closing all windows brings it back; `welcome = false`; Windows animation effects off. Record anything not run as not run.

- [ ] **Step 2: Investigate the two spike oddities**

With and without the banner, shrink the window and check for duplicated text, then send a command straight after a resize in Git Bash and check for a lost first character. File an issue for each one that reproduces. Fix neither in this branch.

- [ ] **Step 3: Measure the anti-goals**

Compare with `fix/ci-and-scrollback`: time from launch to first prompt, with and without the banner. After the clack, `POLL_TIMER` and blink should be the only active timers, and idle CPU should match. Expected: no meaningful difference.

- [ ] **Step 4: Run the final gate and build**

Run the gate, then `powershell -ExecutionPolicy Bypass -File packaging/build-portable.ps1`.
Expected: all pass, and a ZIP is written with the font licence inside.

- [ ] **Step 5: Show the owner**

Launch it for them and capture a screenshot of the window. The owner's approval is the acceptance test. Do not push until they ask.
