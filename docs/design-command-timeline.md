# Command timeline — current-session metadata (#90)

The timeline uses the pane's OSC 133 command tracker, not shell history inference. Each retained command has a canonical id, block start, output start, end, cwd, start/end timestamps, exit code and running state.

The palette exposes the most recent retained commands with exit status and duration. Jump, copy and export actions take the canonical command id, so an evicted or stale command cannot retarget a different block. Unknown command text/status stays unknown. Search within one command's output is now implemented: finished-command palette rows open a search overlay scoped to that command's pane and `[output_start_abs, end_abs)` range. Durable cross-restart indexing remains later.
