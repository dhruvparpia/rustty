# Settings provider — searchable slice (#91)

This slice exposes global and per-profile setting names, effective values, source and restart flags through the native command palette. Keybindings are listed with conflict diagnostics from the shared registry. Selecting a row opens `config.toml` in the native Preferences editor.

Applying/canceling individual values and external-edit conflict handling remain later slices. A `write_text_atomic` helper is available for the future editor: it preserves comments when the editor supplies text containing them and keeps the original on failure.
