//! Workspace templates (WORKSPACE-001..009, roadmap Stage 1, #37): named,
//! reusable launch layouts — tabs, pane split shapes, profiles, tab titles,
//! per-pane working directories, focus, and optional explicit per-pane
//! startup commands. Schema and migration policy:
//! [`docs/design-workspaces.md`](https://github.com/dhruvparpia/rustty).
//!
//! Templates live in `workspaces.toml` beside `config.toml` (portable mode
//! wins, the same resolution as `session.toml`). They are *templates*: the
//! session-restore writer never touches this file, and closing a window
//! never overwrites a template (WORKSPACE-002). The file is user-visible
//! and hand-editable: the reader is forgiving, and a store that cannot be
//! read (newer version, unparseable, over cap) is reported and **never**
//! rewritten — every mutating operation refuses to touch it (WORKSPACE-009).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::{self, MAX_DEPTH, MAX_TABS};
use crate::split::Dir;

/// Schema version this rustty writes (WORKSPACE-009).
pub const VERSION: u32 = 1;
/// Workspaces the store may hold before writes are refused.
pub const MAX_WORKSPACES: usize = 64;

fn current_version() -> u32 {
    VERSION
}

/// One saved pane-tree node: the [`session::Shape`] tree plus an optional
/// explicit startup `command` (argv) on a leaf (WORKSPACE-006). Commands
/// are template data only — the session-capture path never writes one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WsShape {
    /// A pane: `cwd` to spawn in, and the argv to spawn (profile default
    /// when `command` is absent).
    Leaf {
        /// Working directory for this pane's shell.
        #[serde(default)]
        cwd: Option<String>,
        /// Explicit startup argv: `argv[0]` is the program, the rest are
        /// arguments.
        #[serde(default)]
        command: Option<Vec<String>>,
    },
    /// A split: `dir` cuts the area, `ratio` is the first child's share.
    Split {
        /// Split orientation (SPLIT-001).
        dir: Dir,
        /// First child's share of the axis (clamped on load).
        #[serde(default = "crate::split::default_ratio_serde")]
        ratio: f32,
        /// Left/top child.
        a: Box<WsShape>,
        /// Right/bottom child.
        b: Box<WsShape>,
    },
}

impl WsShape {
    /// The cwd of this node when it is a leaf.
    #[must_use]
    pub fn leaf_cwd(&self) -> Option<&str> {
        match self {
            Self::Leaf { cwd, .. } => cwd.as_deref(),
            Self::Split { .. } => None,
        }
    }

    /// Command of every leaf in flatten order (same order as
    /// [`session::Shape::leaf_cwds`]); `None` entries mean "use the
    /// profile's program".
    pub fn leaf_commands(&self, out: &mut Vec<Option<Vec<String>>>) {
        match self {
            Self::Leaf { command, .. } => out.push(command.clone()),
            Self::Split { a, b, .. } => {
                a.leaf_commands(out);
                b.leaf_commands(out);
            }
        }
    }

    /// Convert a session shape (no commands) into a template shape.
    #[must_use]
    pub fn from_session(shape: &session::Shape) -> Self {
        match shape {
            session::Shape::Leaf { cwd } => Self::Leaf {
                cwd: cwd.clone(),
                command: None,
            },
            session::Shape::Split { dir, ratio, a, b } => Self::Split {
                dir: *dir,
                ratio: *ratio,
                a: Box::new(Self::from_session(a)),
                b: Box::new(Self::from_session(b)),
            },
        }
    }

    /// Convert to a session shape, dropping commands (they are consumed at
    /// spawn, not stored in the tab tree). Splits deeper than
    /// [`MAX_DEPTH`] collapse to a leaf, exactly as
    /// [`crate::split::PaneTree::from_shape`] does for a hand-edited file.
    #[must_use]
    pub fn to_session(&self) -> session::Shape {
        self.to_session_at(0)
    }

    fn to_session_at(&self, depth: u32) -> session::Shape {
        match self {
            Self::Leaf { cwd, .. } => session::Shape::Leaf { cwd: cwd.clone() },
            Self::Split { dir, ratio, a, b } if depth >= MAX_DEPTH => {
                session::Shape::Leaf { cwd: None }
            }
            Self::Split { dir, ratio, a, b } => session::Shape::Split {
                dir: *dir,
                ratio: *ratio,
                a: Box::new(a.to_session_at(depth + 1)),
                b: Box::new(b.to_session_at(depth + 1)),
            },
        }
    }
}

/// One saved template tab: mirrors [`session::SessionTab`] plus leaf
/// commands inside `shape`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsTab {
    /// Configured profile name (falls back to the first profile on open).
    pub profile: String,
    /// Tab-bar title as saved.
    #[serde(default)]
    pub title: String,
    /// True when the title was pinned by a user rename (TAB-013).
    #[serde(default)]
    pub title_fixed: bool,
    /// Focused pane's flatten index.
    #[serde(default)]
    pub focused_pane: usize,
    /// The tab's pane tree, with optional per-leaf commands.
    pub shape: WsShape,
}

impl WsTab {
    /// The session-tab view of this tab (commands dropped; they are kept
    /// beside and consumed at spawn).
    #[must_use]
    pub fn to_session(&self) -> session::SessionTab {
        session::SessionTab {
            profile: self.profile.clone(),
            title: self.title.clone(),
            title_fixed: self.title_fixed,
            focused_pane: self.focused_pane,
            shape: self.shape.to_session(),
        }
    }
}

/// One named template: tabs, focused tab (WORKSPACE-001).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Exact name; compared case-insensitively for lookups and duplicates.
    pub name: String,
    /// Focused tab index.
    #[serde(default)]
    pub focused_tab: usize,
    /// Tabs in tab-bar order.
    #[serde(default)]
    pub tab: Vec<WsTab>,
}

impl Workspace {
    /// Capture a workspace template from a live session snapshot. UI
    /// capture carries no commands: the template records layout,
    /// directories and focus, not what the panes happen to be running
    /// (WORKSPACE-006).
    #[must_use]
    pub fn from_session(name: &str, session: &session::Session) -> Self {
        Self {
            name: name.to_string(),
            focused_tab: session.focused_tab,
            tab: session
                .tabs
                .iter()
                .map(|st| WsTab {
                    profile: st.profile.clone(),
                    title: st.title.clone(),
                    title_fixed: st.title_fixed,
                    focused_pane: st.focused_pane,
                    shape: WsShape::from_session(&st.shape),
                })
                .collect(),
        }
    }

    /// The session view of this template: the same tabs the window's
    /// session restore rebuilds, without the window rectangle.
    #[must_use]
    pub fn as_session(&self) -> session::Session {
        session::Session {
            tabs: self.tab.iter().map(WsTab::to_session).collect(),
            focused_tab: self.focused_tab,
            window: None,
        }
    }
}

/// The whole `workspaces.toml` document (schema version 1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    /// Schema version; a v1 file missing the key reads as 1.
    #[serde(default = "current_version")]
    pub version: u32,
    /// Templates in file order.
    #[serde(default)]
    pub workspace: Vec<Workspace>,
}

impl Store {
    /// Case-insensitive index of the template named `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<usize> {
        let want = name.to_lowercase();
        self.workspace
            .iter()
            .position(|w| w.name.to_lowercase() == want)
    }
}

/// Outcome of reading a store: usable, or a human-readable reason the
/// store must not be rewritten (WORKSPACE-009).
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// Readable store; a missing or empty file reads as an empty store.
    Store(Store),
    /// Unreadable: report this and refuse every mutation of the file.
    Unusable(String),
}

/// Path of the store: beside the config (portable mode wins, the same
/// rule as `session.toml`).
#[must_use]
pub fn store_path() -> Option<PathBuf> {
    crate::config::config_path().map(|p| p.with_file_name("workspaces.toml"))
}

/// Read the store at `path`. Never fails: the reason text is for the user.
#[must_use]
pub fn read_store(path: &Path) -> Loaded {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Loaded::Store(Store::default());
    };
    if text.trim().is_empty() {
        return Loaded::Store(Store::default());
    }
    match toml::from_str::<Store>(&text) {
        Ok(store) if store.version > VERSION => Loaded::Unusable(format!(
            "workspaces file is version {}; this rustty understands version {VERSION}",
            store.version
        )),
        Ok(store) if store.workspace.len() > MAX_WORKSPACES => Loaded::Unusable(format!(
            "workspaces file has more than {MAX_WORKSPACES} workspaces and will not be touched"
        )),
        Ok(store) => Loaded::Store(store),
        Err(e) => Loaded::Unusable(format!("workspaces file ignored: {e}")),
    }
}

fn write_store(path: &Path, store: &Store) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(store)?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Load the store, refusing to continue (with the reason) when it cannot
/// be read — every mutation goes through this so an unreadable file is
/// never clobbered (WORKSPACE-009).
fn load_for_write(path: &Path) -> anyhow::Result<Store> {
    match read_store(path) {
        Loaded::Store(mut store) => {
            store.version = VERSION;
            Ok(store)
        }
        Loaded::Unusable(reason) => anyhow::bail!("{reason}"),
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("workspace name is empty");
    }
    Ok(())
}

/// The template named `name` (case-insensitive), or `None` when absent,
/// empty, or over the tab cap (a hand-edited template is then reported by
/// the opener as a plain miss, WORKSPACE-007).
#[must_use]
pub fn workspace_at(path: &Path, name: &str) -> Option<Workspace> {
    let Loaded::Store(store) = read_store(path) else {
        return None;
    };
    store
        .find(name)
        .map(|i| store.workspace[i].clone())
        .filter(|ws| !ws.tab.is_empty() && ws.tab.len() <= MAX_TABS)
}

/// All template names in file order; empty when the store is unreadable.
#[must_use]
pub fn names_at(path: &Path) -> Vec<String> {
    let Loaded::Store(store) = read_store(path) else {
        return Vec::new();
    };
    store.workspace.iter().map(|w| w.name.clone()).collect()
}

/// Template names from the default store (for the Jump List and
/// `--workspaces`).
#[must_use]
pub fn names() -> Vec<String> {
    store_path().map(|p| names_at(&p)).unwrap_or_default()
}

/// Save `ws`, replacing a same-named template (case-insensitively) in
/// place. Names are kept byte-exact. Refuses a store it cannot read.
///
/// # Errors
///
/// Empty name, store full, or any I/O/serialization failure.
pub fn save_ws(path: &Path, ws: &Workspace) -> anyhow::Result<()> {
    validate_name(&ws.name)?;
    let mut store = load_for_write(path)?;
    match store.find(&ws.name) {
        Some(i) => store.workspace[i] = ws.clone(),
        None => {
            if store.workspace.len() >= MAX_WORKSPACES {
                anyhow::bail!("workspace store is full ({MAX_WORKSPACES} templates)");
            }
            store.workspace.push(ws.clone());
        }
    }
    write_store(path, &store)
}

/// Delete the template named `name` (case-insensitive); absent is a
/// no-op. Refuses a store it cannot read.
///
/// # Errors
///
/// Any I/O/serialization failure.
pub fn delete_ws(path: &Path, name: &str) -> anyhow::Result<()> {
    let mut store = load_for_write(path)?;
    if let Some(i) = store.find(name) {
        store.workspace.remove(i);
        write_store(path, &store)?;
    }
    Ok(())
}

/// Rename `old` to `new` (names kept byte-exact, duplicates rejected
/// case-insensitively). Refuses a store it cannot read.
///
/// # Errors
///
/// Empty new name, `old` absent, `new` taken, or any I/O/serialization
/// failure.
pub fn rename_ws(path: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    validate_name(new)?;
    let mut store = load_for_write(path)?;
    let i = store
        .find(old)
        .ok_or_else(|| anyhow::anyhow!("no workspace named '{old}'"))?;
    if let Some(j) = store.find(new)
        && j != i
    {
        anyhow::bail!("a workspace named '{new}' already exists");
    }
    store.workspace[i].name = new.to_string();
    write_store(path, &store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::Dir;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustty-ws-{}-{tag}.toml", std::process::id()))
    }

    fn leaf(cwd: Option<&str>, command: Option<&[&str]>) -> WsShape {
        WsShape::Leaf {
            cwd: cwd.map(str::to_owned),
            command: command.map(|argv| argv.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    fn template() -> Workspace {
        Workspace {
            name: "Web".into(),
            focused_tab: 0,
            tab: vec![WsTab {
                profile: "Git Bash".into(),
                title: "server".into(),
                title_fixed: true,
                focused_pane: 1,
                shape: WsShape::Split {
                    dir: Dir::Vertical,
                    ratio: 0.25,
                    a: Box::new(leaf(Some("C:/src/web"), Some(&["npm", "run", "dev"]))),
                    b: Box::new(leaf(Some("C:/src/web"), None)),
                },
            }],
        }
    }

    #[test]
    fn store_round_trips_with_commands_and_shape() {
        let path = temp_path("roundtrip");
        let _clean = std::fs::remove_file(&path);
        save_ws(&path, &template()).expect("saves");
        let Loaded::Store(store) = read_store(&path) else {
            panic!("fresh store must be readable");
        };
        assert_eq!(store.version, VERSION);
        assert_eq!(store.workspace, vec![template()]);
        let text = std::fs::read_to_string(&path).expect("reads back");
        // toml::to_string_pretty wraps arrays over several lines, so check
        // the array key and its items, not the exact single-line form.
        assert!(text.contains("command = ["), "{text}");
        assert!(text.contains("\"npm\""), "{text}");
        assert!(text.contains("\"run\""), "{text}");
        assert!(text.contains("\"dev\""), "{text}");
        assert!(text.contains("kind = \"leaf\""), "{text}");
        assert!(text.contains("version = 1"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn as_session_drops_commands_keeps_cwds_and_focus() {
        let ws = template();
        let session = ws.as_session();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.tabs[0].title, "server");
        assert!(session.tabs[0].title_fixed);
        assert_eq!(session.tabs[0].focused_pane, 1);
        let mut cwds = Vec::new();
        session.tabs[0].shape.leaf_cwds(&mut cwds);
        assert_eq!(
            cwds,
            vec![Some("C:/src/web".into()), Some("C:/src/web".into())]
        );
        let mut cmds = Vec::new();
        for t in &ws.tab {
            t.shape.leaf_commands(&mut cmds);
        }
        assert_eq!(
            cmds[0],
            Some(vec!["npm".into(), "run".into(), "dev".into()])
        );
        assert_eq!(cmds[1], None);
    }

    #[test]
    fn v1_file_without_version_key_reads() {
        let path = temp_path("nover");
        let _clean = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            "[[workspace]]\nname = \"a\"\n\n[[workspace.tab]]\nprofile = \"PowerShell\"\n\n[workspace.tab.shape]\nkind = \"leaf\"\n",
        )
        .expect("writes fixture");
        let Loaded::Store(store) = read_store(&path) else {
            panic!("version key is optional in v1");
        };
        assert_eq!(store.version, 1);
        assert_eq!(store.workspace.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn newer_version_is_reported_and_never_clobbered() {
        let path = temp_path("newer");
        let _clean = std::fs::remove_file(&path);
        let future = "version = 2\n";
        std::fs::write(&path, future).expect("writes fixture");
        let Loaded::Unusable(reason) = read_store(&path) else {
            panic!("version 2 must be refused");
        };
        assert!(reason.contains("version 2"), "{reason}");
        assert!(save_ws(&path, &template()).is_err());
        assert!(rename_ws(&path, "a", "b").is_err());
        assert!(delete_ws(&path, "a").is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), future);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn broken_file_blocks_every_mutation() {
        let path = temp_path("broken");
        let _clean = std::fs::remove_file(&path);
        let junk = "not = = toml";
        std::fs::write(&path, junk).expect("writes fixture");
        assert!(matches!(read_store(&path), Loaded::Unusable(_)));
        assert!(save_ws(&path, &template()).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), junk);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_replaces_case_insensitively_and_rejects_empty_names() {
        let path = temp_path("dupes");
        let _clean = std::fs::remove_file(&path);
        save_ws(&path, &template()).expect("first save");
        let mut renamed = template();
        renamed.name = "web".into();
        renamed.focused_tab = 3;
        save_ws(&path, &renamed).expect("second save updates in place");
        let Loaded::Store(store) = read_store(&path) else {
            panic!("store must be readable");
        };
        assert_eq!(store.workspace.len(), 1, "case-insensitive replace");
        assert_eq!(store.workspace[0].focused_tab, 3);
        assert!(store.workspace[0].name == "web", "kept byte-exact");
        assert!(workspace_at(&path, "WEB").is_some());
        let mut blank = template();
        blank.name = "  ".into();
        assert!(save_ws(&path, &blank).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rename_and_delete_behave() {
        let path = temp_path("rename");
        let _clean = std::fs::remove_file(&path);
        save_ws(&path, &template()).expect("saves");
        assert!(rename_ws(&path, "web", "WebApp").is_ok());
        assert!(workspace_at(&path, "webapp").is_some());
        assert!(
            rename_ws(&path, "webapp", "WEB").is_ok(),
            "same name, case-only"
        );
        assert!(rename_ws(&path, "gone", "x").is_err());
        save_ws(
            &path,
            &Workspace {
                name: "two".into(),
                ..template()
            },
        )
        .expect("second");
        assert!(
            rename_ws(&path, "two", "web").is_err(),
            "duplicate rejected"
        );
        assert!(delete_ws(&path, "WEB").is_ok());
        assert!(delete_ws(&path, "web").is_ok(), "absent is a no-op");
        assert_eq!(names_at(&path), vec!["two".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deep_splits_collapse_like_the_session_reader() {
        let mut shape = leaf(Some("x"), None);
        for _ in 0..(MAX_DEPTH + 8) {
            shape = WsShape::Split {
                dir: Dir::Vertical,
                ratio: 0.5,
                a: Box::new(leaf(None, None)),
                b: Box::new(shape),
            };
        }
        let as_shape = shape.to_session();
        let mut cwds = Vec::new();
        as_shape.leaf_cwds(&mut cwds);
        assert_eq!(cwds.len() as u32, MAX_DEPTH + 1);
    }
}
