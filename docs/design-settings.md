# Settings provider — searchable slice (#91)

This slice exposes global and per-profile setting names, effective values, source and restart flags through the native command palette. Keybindings are listed with conflict diagnostics from the shared registry. Selecting a row opens `config.toml` in the native Preferences editor.

Applying/canceling individual values, preserving TOML comments, atomic round-trip edits and external-edit conflict handling are later slices. The provider does not rewrite the config file here.
