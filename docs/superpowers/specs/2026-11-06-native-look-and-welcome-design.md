# Native look and startup crab: design

Date: 2026-11-06. Status: awaiting owner approval.

## Intent

The owner wants to run rustty and enjoy looking at it. Today it opens with a white
menu bar on a dark window, three stacked rows of chrome, text flush against the
edges and empty boxes where prompt icons belong.

The target is the owner's words: "ghostty with a bazzite like motd". That means a
quiet, seamless window, plus a welcome banner in the structure of Bazzite's terminal
MOTD: status, a command table and a changing tip. It carries the personality of
rustty's own icon, the crab whose shell is a terminal. The owner approved the banner
mockup and said the claws clack "perfectly".

## Goals

1. Opening rustty shows one seamless dark surface with tabs in the title bar.
2. On start, the first pane shows the crab banner. The claws snap twice, then stop.
3. Common prompts display correctly with no setup: icons appear, and Powerline
   segments join without seams.
4. Theme colours apply to ANSI output, so a theme looks like its name.

## Non-goals

- Other review findings: mouse-mode reset, background-colour erase, colon SGR forms,
  `OSC 133;D;0` and scoped search stay separate work.
- Custom drawing for box characters (U+2500-257F). Good follow-up, not needed here.
- Bundling a text font. Cascadia Mono stays the default.
- Splitting `window.rs` beyond what this work needs.
- Mica, acrylic, new themes or a settings redesign.

## Anti-goals

Any of these makes the work a failure, even if the rest looks right:

- Slower startup, or a shell that waits on the banner or animation.
- Any timer or repaint left running after the animation ends. An idle terminal stays
  idle (NFR-113/114).
- Losing native window behaviour: Snap Layouts, drag, resize, double-click maximize,
  screen-reader access or mixed-DPI correctness.
- Banner bytes reaching the shell, or any change to shell configuration.
- A crab that blocks work. It never appears on new tabs, splits, or a window opened while
  another rustty window is open.
- Network use. Tips ship in the binary. No telemetry.
- Electron or web UI. All of this is drawn by the existing native renderer.

## Approved design

### Window

Reference mockup, as approved by the owner:
[`2026-11-06-native-look-mockup.html`](2026-11-06-native-look-mockup.html).

- Tabs sit in the title bar on a single surface. The strip paints with the theme's
  `background`, so there is no band above the text. The active tab has a slightly lighter
  background (`#202328` on `copper-dark`, the mockup's value) and a 2-DIP `ui-accent` bar
  along its top. Inactive tabs have `ui-muted` text and no fill.
- The strip uses `tab-bar` only when the user sets that key explicitly. Builder's call
  how presets express this, including updating `tab_active` so copper matches the mockup.
- The `+` button follows the last tab, not the left edge as it does today.
- Minimize, maximize and close stay on the right, in theme colours.
- Tab titles use the Windows UI font (Segoe UI), not the terminal font.
- The strip is about 36 DIPs tall. The text area gains comfortable padding: new default
  `[window] padding` of about `{ x = 12, y = 10 }`, scaled for DPI. Explicit config wins.
- The Windows menu bar is no longer shown. Every command it holds stays reachable from
  the command palette. Tapping Alt opens the same menu as a popup, so keyboard users
  keep that route.

### Banner

Shown when rustty starts with no other rustty window open, in the focused pane, before
the shell starts. Each window is its own process, so this needs a per-user check across
processes, such as a named mutex held for the life of the GUI process. Once every rustty
window has closed, the next launch shows the crab again. If two launches race, one
showing it is fine. If the check fails, show no banner.

The layout, at the default 80-column window:

```
[crab, 26 cols x 8 rows]  rustty 0.6.0 · session host running · 2 sessions

                          Ctrl+Shift+P            Search every command
                          Ctrl+Shift+Enter        Split this pane
                          rustty --list-sessions  List running sessions
                          rustty --attach <n>     Reconnect to a session
                          welcome = false         Hide this banner

Tip: Ctrl+Shift+K labels links and paths so you can open them by keyboard.

```

The shell's own output and prompt follow beneath it. The descriptions are shortened
from the mockup so the banner fits in 80 columns.

**Colours**, all from the theme, so light themes work:

| Text | Colour |
|---|---|
| `rustty` | `ui-accent`, bold |
| Version and status | `ui-muted` |
| Commands | `ui-accent` on a slightly raised background (`#25282D` on `copper-dark`) |
| Descriptions | terminal foreground |
| `Tip:` | `ui-accent`, bold |
| Tip text | terminal foreground |

**When it appears**

| Launch | Banner |
|---|---|
| Plain launch, including restored tabs | Yes, focused pane only |
| A launch with a directory, `--profile` or `--theme` | Same as a plain launch |
| Any launch while another rustty window is open | No |
| New tab, split, quick terminal | No |
| `--workspace`, `--attach`, or a pane viewing a host session | No |
| `welcome = false` | No |

**Fit.** Width at least 78 columns: full banner. 52 to 77: no crab. Below 52, or fewer
than 14 rows: no banner. A tip that would not fit on one line is left out.

**Status line.** Version always. Host state and session count come from a background
query that never delays startup. If the answer has not arrived by the time the banner
is written, or there is no host, the line shows only what is known, and it may be filled
in later while the animation is still running.

**Tips.** A short list compiled into the binary, one chosen at random per launch. Each
tip must describe a real feature with its real default shortcut.

**The `rustty` commands must work as shown.** A portable install is not on PATH, so
rustty adds its own folder to the PATH of every shell it starts, in both GUI-owned and
host-owned sessions.

- This changes only that shell's environment: no registry edit, install step or admin
  rights, and nothing persists after the shell exits.
- The folder goes last, so a `rustty` the user already has on PATH wins. It is not added
  twice, and a profile's own PATH setting is extended rather than replaced.
- Under WSL the banner shows `rustty.exe`, because Linux shells need the extension. Both
  forms fit the 78-column tier.

**Config.** New top-level key `welcome = true` (default). `false` removes the banner
completely, including the animation and the host query. Documented in
`config.example.toml`.

### The crab

26 columns by 8 rows, using half-block characters, so 26 by 16 pixels. Colours:

| Character | Meaning | Colour |
|---|---|---|
| `l` | upper shell and claws | `#E58A4E` |
| `d` | lower shell and claws | `#D2642D` |
| `c` | the `>_` prompt | theme foreground, so it stays visible on light themes |
| `.` | empty | no colour |

Frames, left claw shown (mirror it for the right). Rows 0 to 5 of the body show only
where the claws do not:

```
Open          Closed        Body (rows 0-4 are blank)
...lll.       ..ll...       .......llllllllllll.......   row 5
.lll...       .lllll.       .ddd..ll..........ll..ddd.   row 6
lll...l       lll.lll       ..ddlll............llldd..   row 7
lll.lll       lll.lll       ...lll..c...........lll...   row 8
dddddl.       dddddl.       ..llll..ccc.........llll..   row 9
dddd...       dddd...       ..dddd....cc........dddd..   row 10
                            ..dddd..cc..........dddd..   row 11
                            ..dddd..c....ccccc..dddd..   row 12
                            ...ddd..............ddd...   row 13
                            ....dddddddddddddddddd....   row 14
                            .....dddddddddddddddd.....   row 15
```

### The clack

Sequence: open 420 ms, closed 130 ms, open 170 ms, closed 130 ms, open 170 ms, then
closed for good. About one second, each time the banner appears. The final frame stays in
scrollback as ordinary text.

- With Windows animation effects off (`SPI_GETCLIENTAREAANIMATION`), the closed frame
  appears with no motion.
- The shell's first output arrives within this second, so output alone must not cancel
  the animation. It stops and leaves the current frame when any of these happens: the
  crab's lines were overwritten or evicted, the pane entered the alt screen, the user
  typed, or the pane closed. It must never write over other content.
- Its timer is removed when the sequence ends or stops.

### Starting the shell under the banner

The banner is written into the pane's grid before the shell spawns, with the cursor on
the line below it. ConPTY already uses `PSEUDOCONSOLE_INHERIT_CURSOR` (`conpty.rs:146`).

This was tested on 2026-11-06 on Windows build 26300, with a throwaway test that wrote 8
lines to a pane, spawned the shell, resized to 100x30, then 60x20, then 100x30, and ran a
command. In cmd, Windows PowerShell, Git Bash and WSL, all 8 lines survived every step
and the prompt started below them. pwsh was not installed, so it is untested.

If a shell clears the screen on start, the banner goes with it, which is acceptable. If a
shell ever overlaps the banner, the desktop matrix treats that as a failure to fix or a
reason to skip the banner for that shell.

A second throwaway test on the same day started each shell with rustty's folder added to
the end of PATH. `rustty --version` printed `rustty 0.6.0` in cmd, Windows PowerShell and
a Git Bash login shell. Under WSL, `rustty` was not found and `rustty.exe` worked.

Seen in the first test and not investigated, so check whether they also happen without a
banner: after shrinking, the shell's text appeared a second time lower in the buffer until
the next grow. Git Bash also received `cho` when `echo` was sent right after a resize.

### Rendering

Both are required for the crab and prompts to look right:

- **Block characters.** Draw U+2580-259F as filled geometry rather than font glyphs.
  With the default line height of 1.2, font glyphs would probably leave gaps between
  rows. Powerline separators U+E0B0-E0B3 get the same treatment, so segments join.
- **Prompt icons.** Bundle Symbols Nerd Font Mono v3.5.1, the latest stable at the time
  of writing, embedded in the executable and used only as a fallback, so the user's
  chosen font still wins. It adds roughly 2.5 MB to a 4 MB executable. Include its
  licence in the package. Confirm the version again when adding it (AGENTS Rule 1).

### Theme

- With no `theme` key, a dark system now gets `copper-dark` and a light system gets
  `copper-light`. An explicit theme in config still wins, and other presets stay.
- Theme palettes must actually colour ANSI output. Today `Theme::palette` has no reader
  outside tests, and the parser hardcodes `ansi_palette()`, so every theme shows the same
  16 colours. After this work, output written under a theme uses that theme's palette.
  Recolouring text already on screen after a live theme switch is not required.

## Contract changes to make in the same work

AGENTS.md makes `docs/SPEC.md` the product contract, so these must change with the code:

- `MENU-001` to `MENU-005`: the menu stays available as an Alt popup, no longer a
  permanent bar.
- `SKIN-006`, and the claim in `docs/copper-skin.md` that caption controls stay
  Windows-owned, if the buttons end up drawn by rustty. Snap Layouts remain required.
- Default theme, in spec §14 and in the "default mapping remains unchanged" line of
  `docs/copper-skin.md`.
- What the `tab-bar` colour key means, wherever the skin requirements define it.
- New `WELCOME-*` requirements covering the banner, its config key and when it appears.
- A shell-environment requirement for the PATH addition, near SHELL-021.
- `config.example.toml`, `CHANGELOG.md` and the out-of-date `README.md` status text.

## Constraints

- Latest-stable rule, CI gates and the `unsafe` boundary in `AGENTS.md`. The CI gates
  must pass. Never panic in the render loop, parser or PTY read loop.
- Zero allocations in the render hot path.
- Windows 10 and 11. Unsupported DWM attributes fall back to system styling, as today.
- Tab bar options `always`, `auto` and `hidden`, tab bar positions, fullscreen, many
  tabs and narrow windows must all keep working. When the tab strip is hidden, a window
  must still have something to drag and its caption buttons.

## Build on

This work starts from branch `fix/ci-and-scrollback` (PR #117, open at the time of
writing), so the owner sees those fixes too. If #117 has merged, start from `main`.

## Left to the builder

- How the title bar is built: DWM-composited native buttons or buttons drawn by rustty.
  The requirements are above. Check how Direct2D and DWM interact under the buttons.
- How the crab frames are written to specific grid lines, and how overwriting is
  detected.
- How the theme palette reaches the parser.
- How proportional tab titles are measured. `tabbar::fit_title` assumes a cell width.
- The tip list, within the rule above.

## How we know it's done

- Automated: banner layout at each width tier, appearance rules for each launch type,
  animation stop conditions, no timer after the end, config key, every menu item present
  in the palette, tips fitting 78 columns, block-character pixels with no gaps at line
  height 1.2 and 1.0, palette reaching output.
- Desktop, recorded in `docs/validation-matrix.md`: the shells above; 100, 150 and 200%
  DPI; Snap Layouts, drag, maximize and fullscreen; light and dark Windows; Windows 10
  fallback if a machine is available; startup time and idle CPU compared with today.
- The owner opens it and likes it.
