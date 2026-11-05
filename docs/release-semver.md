# Release policy — SemVer and immutable artifacts (#87)

## Versioning

This project is pre-1.0. A patch release (`0.5.1`) is a compatible repair; a minor (`0.6.0`) adds features or documents an explicit break. After 1.0, a supported-contract break requires a major release.

Feature work targets minors. The `0.5.x` repair line stays for compatible fixes. Published tags and assets are immutable: `v0.5.0` is not moved; `v0.5.1` is the compatible repair.

## Version sources

`Cargo.toml` is the single source for the client version. The host protocol, snapshot schema and persistence schemas are versioned independently:
- IPC: `PROTOCOL_VERSION` + `host-session/v1`;
- snapshot: `host-snapshot/v1`;
- persistence: `tasks.toml`, `workspaces.toml`, `session.toml` each carry their own `version`.

Negotiation supports the current protocol and one step back. A client that speaks an unsupported protocol gets an explicit `protocol-mismatch`; a newer host is never replaced to force a version.

## Beta/RC workflow

A changed candidate gets a new identifier, e.g. `0.6.0-beta.1`, `0.6.0-beta.2`, `0.6.0-rc.1`. Do not move a published beta/RC tag; publish a new one.

## Release checks

`packaging/check-release.ps1` checks a local tag:
- tag exists;
- tag version matches `Cargo.toml` at that commit;
- no duplicate tag names;
- optional local ZIP SHA matches its `.sha256`.

CI fetches tags and runs this check for `v0.5.1`. It does not replace or retag an immutable release.
