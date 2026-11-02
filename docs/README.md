# Documentation

| File | Purpose |
|---|---|
| [`SPEC.md`](SPEC.md) | The full product specification / SRS (source of truth) |
| [`design-plugins-mcp.md`](design-plugins-mcp.md) | Plugin system / MCP design (roadmap v0.5, design phase only) |
| [`design-mobile-tablet.md`](design-mobile-tablet.md) | Mobile / tablet design (roadmap v0.5, design phase only) |

Conventions (spec §31):

- Product docs are English; plain sentences; one topic per file.
- Config docs stay in sync with the `src/config.rs` schema.
- Keybinding docs mirror the Ghostty-compatible default set.
- Performance docs state budgets (60 fps, 10k-line burst < 1 s, zero
  hot-path allocations) and how they are measured.

When SPEC.md is amended in the repo, keep it a faithful copy of the owner's
canonical spec and note the change in `CHANGELOG.md`.
