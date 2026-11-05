# Settings provider — searchable slice (#91)

This slice exposes global and per-profile setting names, effective values, source and restart flags through the native command palette. Settings rows now open an in-app overlay: Up/Down selects a row, typed text builds a TOML value, and Enter applies a plain top-level key through `replace_plain`. Table/profile edits remain later slices. Keybinding rows still open the native Preferences editor.

Applying/canceling individual values is now implemented for plain, table and profile scalar keys through `replace_plain`, `replace_section` and `replace_profile`; each preserves trailing comments and writes atomically. External-edit conflict handling is implemented by capturing the config mtime when the overlay opens and refusing Enter when the file changed externally; the config is reloaded and the user retries. The overlay is exposed to UIA as `Node::Settings` with bounds, selected row/count and typed value.
