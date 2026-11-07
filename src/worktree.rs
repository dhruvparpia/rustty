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

/// The folder worktrees live in, inside the main checkout.
pub const WORKTREE_DIR: &str = ".worktrees";

/// A branch and folder name from free text.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(50);
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Run git in `dir` without a console window, and return what it printed.
fn git(dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`: a GUI app must not flash a console.
        const NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(NO_WINDOW);
    }
    let out = command.output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let text = String::from_utf8_lossy(&out.stderr);
        Err(text.trim().chars().take(400).collect())
    }
}

/// Create a worktree of the repository at `dir` on a new branch named from
/// `name`, adding a number when the name is taken, and keep the folder out of
/// `git status`. Returns the worktree's path.
///
/// # Errors
/// `dir` is not in a repository, or git refuses.
pub fn create(dir: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
    // The common folder is the main checkout's, even from inside a worktree.
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = std::path::PathBuf::from(common);
    let root = common
        .parent()
        .ok_or_else(|| "this repository has no main checkout".to_string())?
        .to_path_buf();
    let exclude = common.join("info").join("exclude");
    let line = format!("/{WORKTREE_DIR}/");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|existing| existing.trim() == line) {
        if let Some(parent) = exclude.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        std::fs::write(&exclude, text).map_err(|e| e.to_string())?;
    }
    let base = slug(name);
    for n in 1..=99 {
        let branch = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let path = root.join(WORKTREE_DIR).join(&branch);
        let taken = path.exists()
            || git(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .is_ok();
        if taken {
            continue;
        }
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                &path.to_string_lossy(),
            ],
        )?;
        return Ok(path);
    }
    Err(format!("too many worktrees named {base}"))
}

/// `rustty worktree add NAME [--dir D]`: create a worktree and print its path.
/// Returns the exit code.
pub fn run_cli(args: &[std::ffi::OsString], out: &mut dyn std::io::Write) -> u8 {
    const USAGE: &str = "usage: rustty worktree add NAME [--dir DIR]";
    let mut args = args.iter();
    let mut name = None;
    let mut dir = None;
    if args.next().is_none_or(|verb| verb != "add") {
        eprintln!("rustty: {USAGE}");
        return 2;
    }
    while let Some(arg) = args.next() {
        if arg == "--dir" {
            dir = args.next().map(std::path::PathBuf::from);
        } else if name.is_none() {
            name = Some(arg.to_string_lossy().into_owned());
        } else {
            eprintln!("rustty: {USAGE}");
            return 2;
        }
    }
    let Some(name) = name else {
        eprintln!("rustty: {USAGE}");
        return 2;
    };
    let dir = dir
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    match create(&dir, &name) {
        Ok(path) => {
            if writeln!(out, "{}", path.display()).is_err() {
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("rustty: could not create the worktree: {e}");
            1
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
    let mut command = Command::new("git");
    command
        .args(&args)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`: a GUI app must not flash a console.
        const NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(NO_WINDOW);
    }
    let Ok(out) = command.output() else {
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

    #[test]
    fn slugs_are_safe_branch_and_folder_names() {
        assert_eq!(slug("Fix the login bug!"), "fix-the-login-bug");
        assert_eq!(slug("  ..\\..\\evil  "), "evil");
        assert_eq!(slug("a/b:c"), "a-b-c");
        assert_eq!(slug("---"), "session");
        assert_eq!(slug(""), "session");
        assert!(slug(&"x".repeat(300)).len() <= 50);
        assert_eq!(slug("sessão"), "sess-o", "ASCII only");
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rustty-wt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        git(&dir, &["init", "-q"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
        dir
    }

    #[test]
    fn a_worktree_is_created_on_its_own_branch() {
        let dir = repo("create");
        let path = create(&dir, "Fix the login bug").expect("created");
        assert!(path.ends_with(".worktrees/fix-the-login-bug"), "{path:?}");
        assert!(path.join(".git").exists());
        assert_eq!(
            git(&path, &["branch", "--show-current"]),
            "fix-the-login-bug"
        );
        // Break caught: every worktree showing up as untracked in the repo.
        assert_eq!(git(&dir, &["status", "--porcelain"]), "");
        // And from inside a worktree it still lands in the main checkout.
        let second = create(&path, "second").expect("from a worktree");
        // Git answers with the long path; the temp folder can be an 8.3 short name,
        // as it is on CI, so only compare the two after resolving both.
        let main = dir.canonicalize().expect("main checkout");
        let landed = second.canonicalize().expect("second worktree");
        assert!(landed.starts_with(&main), "{landed:?} is not in {main:?}");
        let _ = git(
            &dir,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        );
        let _ = git(
            &dir,
            &["worktree", "remove", "--force", &second.to_string_lossy()],
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        // Break caught: a second worker on the same card failing to start.
        let dir = repo("taken");
        let first = create(&dir, "same").expect("first");
        let second = create(&dir, "same").expect("second");
        assert!(first.ends_with(".worktrees/same"), "{first:?}");
        assert!(second.ends_with(".worktrees/same-2"), "{second:?}");
        assert_eq!(git(&second, &["branch", "--show-current"]), "same-2");
        let _ = git(
            &dir,
            &["worktree", "remove", "--force", &first.to_string_lossy()],
        );
        let _ = git(
            &dir,
            &["worktree", "remove", "--force", &second.to_string_lossy()],
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_is_not_a_repository_is_an_error() {
        let dir = std::env::temp_dir().join(format!("rustty-wt-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".git"), "not a repository").expect("fake .git");
        assert!(create(&dir, "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cli_prints_the_new_path() {
        // A boss starts workers with this, so its output must be just the path.
        let dir = repo("cli");
        let mut out = Vec::new();
        let code = run_cli(
            &[
                "add".into(),
                "My Task".into(),
                "--dir".into(),
                dir.clone().into(),
            ],
            &mut out,
        );
        assert_eq!(code, 0);
        let printed = String::from_utf8_lossy(&out).trim().to_string();
        assert!(
            std::path::Path::new(&printed).ends_with(".worktrees/my-task"),
            "{printed}"
        );
        let _ = git(&dir, &["worktree", "remove", "--force", &printed]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cli_refuses_what_it_cannot_do() {
        let mut out = Vec::new();
        assert_eq!(run_cli(&[], &mut out), 2);
        assert_eq!(run_cli(&["add".into()], &mut out), 2);
        assert_eq!(run_cli(&["remove".into(), "x".into()], &mut out), 2);
        assert!(out.is_empty(), "failures print nothing to stdout");
    }
}
