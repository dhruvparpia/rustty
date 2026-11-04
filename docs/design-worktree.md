# Worktrees — discovery slice (#92)

This slice discovers existing Git worktrees from the current repository with `git worktree list --porcelain`. Each record carries root path, branch, detached state and availability. The palette lists worktrees and opens one as a tab at its root; unavailable roots are disabled.

Workspace-relative directories, explicit task instantiation and reopened-task identity are later slices. Normal non-Git directories are not inferred as worktrees.
