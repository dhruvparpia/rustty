# Worktrees — discovery slice (#92)

This slice discovers existing Git worktrees from the current repository with `git worktree list --porcelain`. Each record carries root path, branch, detached state and availability. The palette lists worktrees and opens one as a tab at its root; unavailable roots are disabled.

Workspace-relative directories and explicit task instantiation are later slices. Task runs carry a project root namespace (`project`) in host metadata so same-name tasks in two worktrees do not share a live session. Normal non-Git directories are not inferred as worktrees.
