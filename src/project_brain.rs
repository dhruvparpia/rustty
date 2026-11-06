//! A project's brain (BRAIN-001): plain files in `.rustty/brain` that agents read
//! first and keep up to date. The roadmap board reads `roadmap.md`.

use std::path::{Path, PathBuf};

/// The brain's folder inside a project.
pub const BRAIN_DIR: &str = ".rustty/brain";

const BRAIN_MD: &str = "# Project brain\n\nRead this first. Keep it short and true.\n\n## What this project is\n\n## How to work on it\n\n## Current state\n";
const ROADMAP_MD: &str = "# Roadmap\n\n## Idea bank\n\n## Backlog\n\n## In progress\n\n## Done\n";
const DECISIONS_MD: &str =
    "# Decisions\n\nOne entry per decision: what was decided, why, and the cost if it is wrong.\n";

/// The project a folder belongs to: the main checkout when it is one of our
/// worktrees, otherwise the nearest folder with `.git`, otherwise itself.
#[must_use]
pub fn project_root(dir: &Path) -> PathBuf {
    let mut parent = PathBuf::new();
    for part in dir.components() {
        if part.as_os_str() == crate::worktree::WORKTREE_DIR {
            return parent;
        }
        parent.push(part);
    }
    dir.ancestors()
        .find(|at| at.join(".git").exists())
        .unwrap_or(dir)
        .to_path_buf()
}

/// The brain folder of a project.
#[must_use]
pub fn brain_dir(root: &Path) -> PathBuf {
    root.join(BRAIN_DIR)
}

/// The roadmap file of a project.
#[must_use]
pub fn roadmap_path(root: &Path) -> PathBuf {
    brain_dir(root).join("roadmap.md")
}

/// Create any brain files that are missing, never touching one that exists.
/// Returns the files it created.
///
/// # Errors
/// The folder or a file cannot be written.
pub fn init(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dir = brain_dir(root);
    std::fs::create_dir_all(&dir)?;
    let mut made = Vec::new();
    for (name, text) in [
        ("BRAIN.md", BRAIN_MD),
        ("roadmap.md", ROADMAP_MD),
        ("decisions.md", DECISIONS_MD),
    ] {
        let path = dir.join(name);
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match created {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, text.as_bytes())?;
                made.push(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustty-brain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn init_creates_the_files_and_keeps_what_exists() {
        // Break caught: re-running init wiping someone's decisions.
        let root = temp("init");
        let made = init(&root).expect("init");
        assert_eq!(made.len(), 3);
        for name in ["BRAIN.md", "roadmap.md", "decisions.md"] {
            assert!(brain_dir(&root).join(name).is_file(), "{name}");
        }
        std::fs::write(brain_dir(&root).join("decisions.md"), "mine").expect("write");
        assert_eq!(init(&root).expect("again"), Vec::<PathBuf>::new());
        let kept = std::fs::read_to_string(brain_dir(&root).join("decisions.md")).expect("read");
        assert_eq!(kept, "mine");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_roadmap_has_every_board_column() {
        let board = crate::board::Board::parse(ROADMAP_MD);
        let mut copy = board.clone();
        assert!(copy.add(1, "x"));
        let text = copy.render();
        for column in crate::board::COLUMNS {
            assert_eq!(text.matches(&format!("## {column}\n")).count(), 1, "{text}");
        }
    }

    #[test]
    fn a_folder_inside_a_repository_belongs_to_it() {
        let root = temp("root");
        std::fs::create_dir_all(root.join(".git")).expect(".git");
        let deep = root.join("src").join("deep");
        std::fs::create_dir_all(&deep).expect("deep");
        assert_eq!(project_root(&deep), root);
        assert_eq!(project_root(&root), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn our_worktree_belongs_to_its_main_checkout() {
        // Break caught: each worker's board being a separate empty board.
        let root = temp("wt");
        std::fs::create_dir_all(root.join(".git")).expect(".git");
        let tree = root.join(".worktrees").join("fix-login").join("src");
        std::fs::create_dir_all(&tree).expect("tree");
        std::fs::write(root.join(".worktrees/fix-login/.git"), "gitdir: x").expect(".git file");
        assert_eq!(project_root(&tree), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_outside_any_repository_is_its_own_project() {
        let root = temp("plain");
        let found = project_root(&root);
        // A parent of the temp folder could be a repository on a dev machine,
        // so only check that the answer contains the folder.
        assert!(root.starts_with(&found), "{found:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
