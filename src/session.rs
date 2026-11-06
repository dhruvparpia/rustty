//! Session persistence (TAB-026, WINDOW-015/016/017): tabs, pane split
//! shapes, titles, focused panes and each pane's OSC 7 working directory
//! are written to `session.toml` beside the config when the window closes,
//! and reloaded at startup. Shells *respawn* in the saved directories —
//! the v0.2 roadmap says "reopen tabs/panes on relaunch", not reattach.
//!
//! The file is user-visible and hand-editable (it sits next to
//! `config.toml`), so the reader caps tab count and split depth, and a
//! file that fails to parse is reported and ignored: startup never fails
//! because of a session.

use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// Tabs a session file may define before it is rejected.
pub const MAX_TABS: usize = 64;
/// Split nesting the reader accepts (deeper splits collapse to a leaf).
pub const MAX_DEPTH: u32 = 64;

/// One saved pane-tree node (TAB-026). Mirrors [`crate::split::PaneTree`]:
/// `a` is the first child (left/top), `b` the second (right/bottom).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Shape {
    /// A pane; `cwd` is what the shell last reported with OSC 7.
    Leaf {
        /// Working directory to respawn this pane's shell in.
        #[serde(default)]
        cwd: Option<String>,
        /// The agent CLI this pane was running (RESTORE-002).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// A split: `dir` cuts the area, `ratio` is the first child's share.
    Split {
        /// Split orientation (SPLIT-001).
        dir: crate::split::Dir,
        /// First child's share of the axis (clamped on load).
        #[serde(default = "crate::split::default_ratio_serde")]
        ratio: f32,
        /// Left/top child.
        a: Box<Shape>,
        /// Right/bottom child.
        b: Box<Shape>,
    },
}

impl Shape {
    /// The cwd of this node when it is a leaf.
    #[must_use]
    pub fn leaf_cwd(&self) -> Option<&str> {
        match self {
            Self::Leaf { cwd, .. } => cwd.as_deref(),
            Self::Split { .. } => None,
        }
    }

    /// The saved agent of every leaf, in flatten order.
    pub fn leaf_agents(&self, out: &mut Vec<Option<String>>) {
        match self {
            Self::Leaf { agent, .. } => out.push(agent.clone()),
            Self::Split { a, b, .. } => {
                a.leaf_agents(out);
                b.leaf_agents(out);
            }
        }
    }

    /// Cwd of every leaf in flatten order (left-to-right, top-to-bottom),
    /// the same order as [`crate::split::PaneTree::flatten`].
    pub fn leaf_cwds(&self, out: &mut Vec<Option<String>>) {
        match self {
            Self::Leaf { cwd, .. } => out.push(cwd.clone()),
            Self::Split { a, b, .. } => {
                a.leaf_cwds(out);
                b.leaf_cwds(out);
            }
        }
    }
}

/// A saved tab: profile, title, focused pane, pane shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    /// Configured profile name (falls back to the first profile on load).
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
    /// The tab's pane tree (TAB-026).
    pub shape: Shape,
}

/// Saved outer window rectangle in physical pixels (WINDOW-015/016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

/// The whole saved run: tabs, focused tab, window position and size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Tabs in tab-bar order.
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    /// Focused tab index.
    #[serde(default)]
    pub focused_tab: usize,
    /// Window rectangle from the last exit, when saved.
    #[serde(default)]
    pub window: Option<WindowRect>,
}

/// Path of the session file: beside the config (portable mode wins,
/// CONFIG-020 logic shared with `config_path`).
#[must_use]
pub fn session_path() -> Option<PathBuf> {
    crate::config::config_path().map(|p| p.with_file_name("session.toml"))
}

/// Read the session file. `None` (missing, empty, oversized, unparseable)
/// means "start fresh"; a parse failure is reported on stderr, never fatal.
#[must_use]
pub fn load() -> Option<Session> {
    let path = session_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<Session>(&text) {
        Ok(session) if !session.tabs.is_empty() && session.tabs.len() <= MAX_TABS => Some(session),
        Ok(_) => None,
        Err(e) => {
            eprintln!("rustty: session file ignored: {e}");
            None
        }
    }
}

/// Write the session file, creating the directory when needed.
///
/// # Errors
///
/// Any I/O or serialization failure (the caller logs it; sessions are
/// best-effort and never block a quit).
pub fn save(session: &Session) -> anyhow::Result<()> {
    let path = session_path().context("no session path (APPDATA unset?)")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(session)?;
    std::fs::write(&path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::{Dir, PaneTree};

    fn split(dir: Dir, ratio: f32, a: Shape, b: Shape) -> Shape {
        Shape::Split {
            dir,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    fn leaf(cwd: Option<&str>) -> Shape {
        Shape::Leaf {
            cwd: cwd.map(str::to_owned),
            agent: None,
        }
    }

    #[test]
    fn session_round_trips_through_toml() {
        let session = Session {
            tabs: vec![
                SessionTab {
                    profile: "Git Bash".into(),
                    title: "build".into(),
                    title_fixed: true,
                    focused_pane: 1,
                    shape: split(Dir::Vertical, 0.25, leaf(Some("C:/src")), leaf(None)),
                },
                SessionTab {
                    profile: "PowerShell".into(),
                    title: "PowerShell".into(),
                    title_fixed: false,
                    focused_pane: 0,
                    shape: leaf(Some("C:/tmp")),
                },
            ],
            focused_tab: 0,
            window: Some(WindowRect {
                x: 10,
                y: 20,
                w: 1200,
                h: 800,
            }),
        };
        let text = toml::to_string(&session).expect("serializes");
        let back: Session = toml::from_str(&text).expect("parses");
        assert_eq!(back, session);
        assert!(text.contains("kind = \"split\""), "tagged shape: {text}");
    }

    #[test]
    fn shape_flatten_order_matches_the_tree() {
        // vsplit(a, hsplit(b, c)): flatten order is a, b, c.
        let shape = split(
            Dir::Vertical,
            0.5,
            leaf(Some("a")),
            split(Dir::Horizontal, 0.5, leaf(Some("b")), leaf(Some("c"))),
        );
        let mut cwds = Vec::new();
        shape.leaf_cwds(&mut cwds);
        assert_eq!(
            cwds,
            vec![Some("a".into()), Some("b".into()), Some("c".into())]
        );
    }

    #[test]
    fn from_shape_rebuilds_the_tree_the_shape_came_from() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let shape = split(
            Dir::Vertical,
            0.25,
            leaf(Some("C:/x")),
            split(Dir::Horizontal, 0.75, leaf(None), leaf(Some("C:/y"))),
        );
        let rebuilt =
            PaneTree::from_shape(&shape, &mut |cwd| Rc::new(RefCell::new(pane_with_cwd(cwd))));
        // The same tree built by splitting: a | (b over c) at the same ratios.
        let a = Rc::new(RefCell::new(pane_with_cwd(Some("x"))));
        let b = Rc::new(RefCell::new(pane_with_cwd(None)));
        let c = Rc::new(RefCell::new(pane_with_cwd(Some("y"))));
        let mut built = PaneTree::new(Rc::clone(&a));
        assert!(built.split(&a, Dir::Vertical, 0.25, Rc::clone(&b)));
        assert!(built.split(&b, Dir::Horizontal, 0.75, Rc::clone(&c)));
        assert_eq!(rebuilt.count(), 3);
        assert_eq!(rebuilt.layout(80, 24), built.layout(80, 24));
        // The leaf cwds survived into the rebuilt panes, in flatten order.
        let seen: Vec<Option<String>> = rebuilt
            .flatten()
            .iter()
            .map(|p| p.borrow().cwd().map(str::to_owned))
            .collect();
        assert_eq!(seen, vec![Some("C:/x".into()), None, Some("C:/y".into())]);
    }

    fn pane_with_cwd(cwd: Option<&str>) -> crate::pane::Pane {
        let mut p = crate::pane::Pane::new();
        if let Some(cwd) = cwd {
            // The OSC 7 form a real shell emits for a Windows drive path.
            p.feed(format!("\x1b]7;file:///{cwd}\x1b\\").as_bytes());
        }
        p
    }

    #[test]
    fn a_broken_session_is_ignored_not_fatal() {
        // The reader's job: bad text yields None through `load`'s match.
        let parsed = toml::from_str::<Session>("tabs = [1, 2]");
        assert!(parsed.is_err());
    }

    #[test]
    fn an_agent_survives_the_round_trip_in_pane_order() {
        // Break caught: a restart forgetting which pane was running an agent.
        let shape = Shape::Split {
            dir: crate::split::Dir::Vertical,
            ratio: 0.5,
            a: Box::new(Shape::Leaf {
                cwd: Some("C:/a".to_string()),
                agent: Some("claude".to_string()),
            }),
            b: Box::new(leaf(Some("C:/b"))),
        };
        let session = Session {
            tabs: vec![SessionTab {
                profile: "p".to_string(),
                title: String::new(),
                title_fixed: false,
                focused_pane: 0,
                shape,
            }],
            focused_tab: 0,
            window: None,
        };
        let text = toml::to_string(&session).expect("serializes");
        let back: Session = toml::from_str(&text).expect("parses");
        assert_eq!(back, session);
        let mut agents = Vec::new();
        back.tabs[0].shape.leaf_agents(&mut agents);
        assert_eq!(agents, vec![Some("claude".to_string()), None]);
    }

    #[test]
    fn a_session_file_from_before_agents_still_loads() {
        let text = "focused_tab = 0\n[[tabs]]\nprofile = \"p\"\n[tabs.shape]\nkind = \"leaf\"\ncwd = \"C:/a\"\n";
        let back: Session = toml::from_str(text).expect("parses");
        let mut agents = Vec::new();
        back.tabs[0].shape.leaf_agents(&mut agents);
        assert_eq!(agents, vec![None]);
    }
}
