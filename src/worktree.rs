//! Worktree discovery slice (#92): existing Git worktrees from the current
//! repository, with branch, path and detached/available state. Opening a worktree
//! creates a tab at its root; workspace/task instantiation is a later slice.

/// Worktrees one discovery list may keep.
pub const MAX_WORKTREES: usize = 64;

/// One discovered worktree.
#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    /// Root directory.
    pub root: String,
    /// Branch name, `None` when detached or unavailable metadata.
    pub branch: Option<String>,
    /// Detached HEAD.
    pub detached: bool,
    /// Root directory exists.
    pub available: bool,
}

impl Default for Worktree {
    fn default() -> Self {
        Self::new()
    }
}

impl Worktree {
    /// Default empty worktree record.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: String::new(),
            branch: None,
            detached: false,
            available: false,
        }
    }

    /// Display label for palette rows.
    #[must_use]
    pub fn label(&self) -> String {
        if self.detached {
            format!("{} (detached)", self.root)
        } else if self.branch.is_some() {
            format!("{} ({:?})", self.root.clone(), self.branch)
        } else {
            self.root.clone()
        }
    }
}

/// Parse `git worktree list --porcelain` text.
fn parse_porcelain(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut current = Worktree::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current.root = path.to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current.branch = Some(branch.to_string());
        } else if line == "detached" {
            current.detached = true;
        } else if line.is_empty() && !current.root.is_empty() {
            current.available = std::path::Path::new(&current.root).is_dir();
            out.push(current.clone());
            current = Worktree::new();
        }
    }
    if !current.root.is_empty() {
        current.available = std::path::Path::new(&current.root).is_dir();
        out.push(current.clone());
    }
    out.truncate(MAX_WORKTREES);
    out
}

/// Discover existing worktrees from the current repository.
#[must_use]
pub fn discover() -> Vec<Worktree> {
    use std::process::{Command, Stdio};
    let dir_result = std::env::current_dir();
    let Ok(root) = dir_result else {
        return Vec::new();
    };
    let args: Vec<String> = vec![
        "-C".into(),
        root.as_os_str().to_string_lossy().into_owned(),
        "worktree".into(),
        "list".into(),
        "--porcelain".into(),
    ];
    let Ok(out) = Command::new("git")
        .args(&args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    parse_porcelain(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        "worktree C:/repo
HEAD 123
branch refs/heads/main

worktree C:/repo/web
HEAD 456
branch refs/heads/web
"
        .to_string()
    }

    #[test]
    fn porcelain_parses_branch_path_and_detached() {
        let records = parse_porcelain(&fixture());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].root, "C:/repo".to_string());
        assert_eq!(records[0].branch, Some("refs/heads/main".to_string()));
        assert_eq!(records[1].root, "C:/repo/web".to_string());
        assert_eq!(records[1].branch, Some("refs/heads/web".to_string()));
    }

    #[test]
    fn detached_and_unavailable_are_explicit() {
        let records = parse_porcelain("worktree C:/gone\nHEAD 123\ndetached\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].root, "C:/gone".to_string());
        assert!(records[0].detached);
        assert!(!records[0].available);
        assert_eq!(records[0].branch, None);
    }

    #[test]
    fn cap_bounds_the_list() {
        let records = parse_porcelain(&fixture());
        assert!(records.len() <= MAX_WORKTREES);
    }
}
