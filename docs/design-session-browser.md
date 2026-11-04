# Session browser — issue #88

The palette lists live and exited host sessions with profile, cwd, controller/watcher counts, exit state, and canonical `task#<id> <name>` labels. Live rows attach; exited rows are disabled because attach does not respawn them.

## Refresh

The registry is rebuilt from a fresh `session.list` snapshot every ~90 poll ticks (~3 s) while the palette is open. The selected command is preserved by command identity, and the typed query is reapplied. If the selected command disappears, selection falls back to the first hit.

## What stays later

Session-browser keyboard/desktop acceptance and dynamic row animation remain later slices. The refresh only re-reads host metadata; it does not spawn shells.
