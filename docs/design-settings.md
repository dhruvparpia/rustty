# Settings provider — searchable slice (#91)

This slice exposes global and per-profile setting names, effective values, source and restart flags through the native command palette. Settings rows now open an in-app overlay: Up/Down selects a row, typed text builds a TOML value, and Enter applies a plain top-level key through `replace_plain`. Table/profile edits remain later slices. Keybinding rows still open the native Preferences editor.

Applying/canceling individual values and external-edit conflict handling remain later slices. A `write_text_atomic` helper is available for the future editor: it preserves comments when the editor supplies text containing them and keeps the original on failure.
