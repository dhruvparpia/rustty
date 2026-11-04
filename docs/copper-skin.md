# Copper native skin

Select **copper-dark** or **copper-light** from the theme menu/palette, or put
this before any section header in `config.toml`:

```toml
theme = "copper-dark"
```

Dark uses graphite surfaces and warm copper focus accents; light uses warm
paper, dark ink and deep copper. Both use readable secondary labels and a
positional active-tab/selected-row indicator. Native caption buttons, window
dragging and resizing remain Windows-owned. The caption follows the skin background, with
custom text and frame colors where supported; unsupported DWM attributes fall back to system styling.

To customize chrome independently, replace the plain theme key with a table:

```toml
[theme]
name = "copper-dark"
ui-foreground = "#F0EAE2"
ui-muted = "#B6B0A8"
ui-accent = "#E0A477"
ui-border = "#454B54"
```

These four keys affect application chrome, not terminal foreground, cursor or
ANSI colors. Presets include their own terminal palette. Existing presets,
per-profile themes and theme-file/config override precedence still apply.
Saving the config reloads colors; no shell restart is needed. The default and
automatic system-theme mapping remain unchanged.

## Validation

Automated contrast checks require 4.5:1 for primary/secondary chrome text and
terminal default text on background/selection/search surfaces, plus 3:1 for
focus accents. User overrides can intentionally depart from these ratios.

Windows desktop acceptance (owner-run):

- Switch both skins through menu, palette, profile and config reload; caption
  and client colors must agree. Check Windows 10 fallback and Windows 11.
- Check top/bottom/hidden tabs, many tabs, narrow windows, Unicode titles and
  marquee hover; icon/title/close targets must stay separate and clickable.
- Check palette/bookmarks/search, keyboard focus and pane titles in both skins.
- Move between 100%, 150% and 200% DPI displays; maximize, restore, drag, resize
  and enter/exit fullscreen. Native caption controls must remain functional.
- Verify ANSI-colored tools, selection, search matches and custom profiles.

Source checks do not substitute for these desktop checks.
