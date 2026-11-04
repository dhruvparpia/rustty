# Command timeline — current-session metadata (#90)

The timeline uses the pane's OSC 133 command tracker, not shell history inference. Each retained command has a canonical id, block start, output start, end, cwd, start/end timestamps, exit code and running state.

The palette exposes the most recent retained commands with exit status and duration. Jump, copy and export actions take the canonical command id, so an evicted or stale command cannot retarget a different block. Unknown command text/status stays unknown. Search within one command's output and durable cross-restart indexing are later slices.
