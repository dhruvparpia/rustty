# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| Latest release | ✅ |
| Older releases | ❌ |

`rustty` is pre-release; only the latest commit on `main` is supported.

## Reporting a vulnerability

Open a **private** GitHub security advisory on the repository:

<https://github.com/dhruvparpia/rustty/security/advisories/new>

If private advisories are not available, email the maintainer directly
(find the address on the maintainer's GitHub profile) with the subject line
`rustty: security`.

Please do **not** post open issues containing exploitable details.

## What we care about

- Parsing and PTY handling (crash/UB reachable from shell output is a security
  issue for a terminal emulator).
- Config file parsing (TOML decoding of untrusted files).
- Anything that turns received terminal output into code execution.
- Telemetry/phone-home: `rustty` has none by policy. A report that we shipped
  any is treated as a vulnerability.

## Response

Expect acknowledgement within 7 days. Fixes ship in the next release; severity
and impact decide the timeline. No bug bounty program.
