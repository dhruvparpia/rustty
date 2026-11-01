//! Tab model: tabs are mandatory (spec §4, §8) and each tab owns a pane
//! tree ([`crate::split`], SPEC SPLIT-001..015) plus its tab-bar title.
//!
//! The tree's leaves are [`Rc<RefCell<Pane>>`]; the tab remembers which leaf
//! is focused, caches the tree's [`crate::split::Rect`] layout for its
//! current size, and keeps the pane grids pinned to that layout. Layout
//! order (left-to-right, top-to-bottom) is shared with the renderer and the
//! mouse hit-test, so index `i` means the same pane everywhere.
//!
//! # Borrow rule
//!
//! Panes live in `RefCell`s. `pane()` / `pane_mut()` / `for_each_pane()`
//! borrow them, and a `RefCell` panics when a borrow overlaps a conflicting
//! one. Code in this crate must never hold a pane borrow across a call back
//! into [`Tab`], [`crate::app::App`] or the renderer.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use crate::color::Color;
use crate::pane::Pane;
use crate::shell::{ShellError, ShellState, SpawnOptions};
use crate::split::{DEFAULT_RATIO, Dir, PaneTree, RATIO_STEP, Rect, Side, neighbor};

/// A terminal tab: a pane tree, the focused leaf, and the tab-bar title.
#[derive(Debug)]
pub struct Tab {
    tree: PaneTree,
    /// Tree leaves in flatten order, kept in step with `layout` (index `i`
    /// of `panes` owns `layout[i]`).
    panes: Vec<Rc<RefCell<Pane>>>,
    /// Focused leaf (SPLIT-005: a new split focuses the new pane).
    active: Rc<RefCell<Pane>>,
    /// Cached layout of `tree` at `cols` x `rows`, in flatten order.
    layout: Vec<Rect>,
    cols: usize,
    rows: usize,
    scrollback: usize,
    title: String,
    /// True when `set_title` pinned the title against OSC/profile updates.
    title_fixed: bool,
    /// Configured profile name shown in the tab bar and used for new splits.
    profile: String,
    /// Tab color from the profile/theme.
    color: Option<Color>,
    /// Ask before closing this tab's shells (TAB-014).
    confirm_close: bool,
    /// Warn when a new process starts in this tab unasked (TAB-021).
    confirm_new_process: bool,
}

impl Tab {
    /// Create a tab with one pane and the default title.
    #[must_use]
    pub fn new() -> Self {
        Self::with_size(80, 24, 1000)
    }

    /// Create a tab with a sized pane (see [`Pane::with_size`]).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        let pane = Rc::new(RefCell::new(Pane::with_size(cols, rows, scrollback)));
        let tree = PaneTree::new(Rc::clone(&pane));
        let panes = tree.flatten();
        let layout = tree.layout(cols, rows);
        Self {
            tree,
            panes,
            active: pane,
            layout,
            cols,
            rows,
            scrollback,
            title: String::from("New Tab"),
            title_fixed: false,
            profile: String::new(),
            color: None,
            confirm_close: true,
            confirm_new_process: true,
        }
    }

    /// The tab's display title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set a user-fixed title (wins over auto-titles, TAB-013).
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.title_fixed = true;
    }

    /// Set an automatic title (profile, cwd, shell title). Ignored once the
    /// user renamed the tab.
    pub fn set_auto_title(&mut self, title: impl Into<String>) {
        if !self.title_fixed {
            self.title = title.into();
        }
    }

    /// True after a user rename.
    #[must_use]
    pub fn title_is_fixed(&self) -> bool {
        self.title_fixed
    }

    /// Profile label for the tab bar (TAB-024).
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Set profile label.
    pub fn set_profile(&mut self, profile: impl Into<String>) {
        self.profile = profile.into();
    }

    /// TAB-014: does closing this tab's shells need a confirmation?
    #[must_use]
    pub const fn confirm_close(&self) -> bool {
        self.confirm_close
    }

    /// TAB-021: warn when a new process starts in this tab unasked.
    #[must_use]
    pub const fn confirm_new_process(&self) -> bool {
        self.confirm_new_process
    }

    /// The confirm flags a profile brings (TAB-014/021).
    pub fn set_confirm(&mut self, close: bool, new_process: bool) {
        self.confirm_close = close;
        self.confirm_new_process = new_process;
    }

    /// Tab color (TAB-014/015/021).
    #[must_use]
    pub fn color(&self) -> Option<Color> {
        self.color
    }

    /// Set tab color.
    pub fn set_color(&mut self, color: Option<Color>) {
        self.color = color;
    }

    /// Scrollback used for new panes in this tab.
    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    // ---- panes ----------------------------------------------------------

    /// The focused pane.
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when the pane is already mutably borrowed.
    #[must_use]
    pub fn pane(&self) -> Ref<'_, Pane> {
        self.active.borrow()
    }

    /// The focused pane, mutably.
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when the pane is already borrowed.
    #[must_use]
    pub fn pane_mut(&mut self) -> RefMut<'_, Pane> {
        self.active.borrow_mut()
    }

    /// The pane at flatten index `idx`, `None` when out of range.
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when the pane is already mutably borrowed.
    #[must_use]
    pub fn pane_at(&self, idx: usize) -> Option<Ref<'_, Pane>> {
        self.panes.get(idx).map(|pane| pane.borrow())
    }

    /// The pane at flatten index `idx`, mutably.
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when the pane is already borrowed.
    #[must_use]
    pub fn pane_at_mut(&mut self, idx: usize) -> Option<RefMut<'_, Pane>> {
        self.panes.get(idx).map(|pane| pane.borrow_mut())
    }

    /// Visit every pane with its layout box, in flatten order (SPLIT-002:
    /// the paint hot path uses this; it allocates nothing).
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when a pane is already borrowed.
    pub fn for_each_pane(&mut self, mut f: impl FnMut(&mut Pane, Rect)) {
        self.tree.for_each(&self.layout, |_, pane, rect| {
            f(&mut pane.borrow_mut(), rect)
        });
    }

    /// Visit every pane immutably with its layout box, in flatten order
    /// (the renderer's paint walk, SPLIT-002: allocation-free).
    ///
    /// # Panics
    ///
    /// Panics (through `RefCell`) when a pane is mutably borrowed.
    pub fn for_each_pane_ref(&self, mut f: impl FnMut(&Pane, Rect)) {
        self.tree
            .for_each(&self.layout, |_, pane, rect| f(&pane.borrow(), rect));
    }

    /// BELL-003/004: true when any pane in this tab rang the bell.
    #[must_use]
    pub fn bell_pending(&self) -> bool {
        let mut any = false;
        self.for_each_pane_ref(&mut |pane: &Pane, _rect: Rect| any |= pane.bell_pending());
        any
    }

    /// Clear the bell flag of every pane in this tab.
    pub fn take_bell(&mut self) {
        self.for_each_pane(&mut |pane: &mut Pane, _rect: Rect| pane.take_bell());
    }

    /// Number of panes in the tab (1 before any split, SPLIT-001).
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Flatten index of the focused pane (`None` only in a tree the tab
    /// cannot produce: focus is always one of its panes).
    #[must_use]
    pub fn focused_index(&self) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| Rc::ptr_eq(pane, &self.active))
    }

    /// The focused pane's box (mouse routing, spawn size).
    #[must_use]
    pub fn active_rect(&self) -> Option<Rect> {
        self.focused_index()
            .and_then(|i| self.layout.get(i).copied())
    }

    /// The cached layout, in flatten order (renderer, hit-test).
    #[must_use]
    pub fn layout(&self) -> &[Rect] {
        &self.layout
    }

    // ---- split operations (SPEC §9/§10) --------------------------------

    /// Split the focused pane (SPLIT-005): `Dir::Vertical` puts a new pane
    /// on the right, `Dir::Horizontal` below, at the default half ratio.
    /// The new pane is focused and sized to its box.
    ///
    /// Returns its flatten index; the caller spawns the shell into it
    /// (SPLIT-007: same profile as the tab).
    pub fn split_active(&mut self, dir: Dir) -> Option<usize> {
        let fresh = Rc::new(RefCell::new(Pane::with_size(1, 1, self.scrollback)));
        if !self
            .tree
            .split(&self.active, dir, DEFAULT_RATIO, Rc::clone(&fresh))
        {
            return None;
        }
        self.active = fresh;
        self.relayout();
        // SPLIT-005: both halves take their new boxes right away.
        let _ = self.apply_layout();
        self.focused_index()
    }

    /// Close the focused pane (SPLIT-006): its sibling takes its box and
    /// focus moves to the pane at the same slot. `true` when the tab has
    /// one pane left, so the caller closes the tab.
    pub fn close_active(&mut self) -> bool {
        let idx = self.focused_index().unwrap_or(0);
        self.close_pane(idx)
    }

    /// Close the pane at `idx` (shell death, SPLIT-004). `true` when the tab
    /// has one pane left, so the caller closes the tab.
    pub fn close_pane(&mut self, idx: usize) -> bool {
        if self.panes.len() <= 1 {
            return true;
        }
        let Some(target) = self.panes.get(idx).cloned() else {
            return false;
        };
        self.tree.close(&target);
        // Focus the pane that took the closed slot (SPLIT-004).
        let idx = idx.min(self.tree.count() - 1);
        self.relayout();
        self.active = Rc::clone(&self.panes[idx]);
        let _ = self.apply_layout();
        false
    }

    /// Focus the pane at `idx` (mouse click, SPLIT-003). `false` when out of
    /// range; focus never leaves the tab.
    pub fn focus_index(&mut self, idx: usize) -> bool {
        if self.panes.get(idx).is_none() {
            return false;
        }
        self.active = Rc::clone(&self.panes[idx]);
        true
    }

    /// Move focus to the neighbouring pane on `side` (SPLIT-008 keyboard
    /// pane focus). `false` when there is no touching neighbour.
    pub fn focus_neighbor(&mut self, side: Side) -> bool {
        let idx = self.focused_index();
        match idx.and_then(|i| neighbor(&self.layout, i, side)) {
            Some(target) => self.focus_index(target),
            None => false,
        }
    }

    /// Grow/shrink the focused pane by one [`crate::split::RATIO_STEP`]
    /// through the nearest split of `dir` (SPLIT-009). `true` when a split
    /// of that orientation was adjusted.
    pub fn adjust_ratio(&mut self, dir: Dir, delta: f32) -> bool {
        if self.tree.adjust_ratio(&self.active, dir, delta) {
            self.relayout();
            let _ = self.apply_layout();
            true
        } else {
            false
        }
    }

    /// One keyboard pane-resize step (SPLIT-009, [`crate::split::RATIO_STEP`]).
    #[must_use]
    pub const fn ratio_step() -> f32 {
        RATIO_STEP
    }

    // ---- size -----------------------------------------------------------

    /// Resize the tab area to `cols` x `rows` (window resize, spec §17):
    /// recompute the layout and resize every pane's grid and ConPTY to its
    /// box. Split ratios are kept (SPLIT-010). A ConPTY resize failure is
    /// reported, never a panic; the first one is returned.
    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<(), ShellError> {
        self.cols = cols;
        self.rows = rows;
        self.relayout();
        self.apply_layout()
    }

    /// Tab area width in cells.
    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Tab area height in cells.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Recompute the cached panes and layout from the tree.
    fn relayout(&mut self) {
        self.panes = self.tree.flatten();
        self.layout = self.tree.layout(self.cols, self.rows);
    }

    /// Resize every pane to its layout box; panes already at their box are
    /// left untouched (their selection and ConPTY stay as they are).
    fn apply_layout(&mut self) -> Result<(), ShellError> {
        let mut first_error = None;
        self.for_each_pane(|pane, rect| {
            if (pane.grid().cols, pane.grid().rows) == (rect.cols, rect.rows) {
                return;
            }
            if let Err(e) = pane.resize(rect.cols, rect.rows) {
                first_error.get_or_insert(e);
            }
        });
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Start a shell in every pane that has none (new tab, split pane).
    pub fn spawn_panes(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        let mut first_error = None;
        self.for_each_pane(|pane, rect| {
            if pane.shell().state() != ShellState::Inactive {
                return;
            }
            let opts = SpawnOptions {
                cols: rect.cols,
                rows: rect.rows,
                ..opts.clone()
            };
            if let Err(e) = pane.spawn_shell(&opts) {
                first_error.get_or_insert(e);
            }
        });
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Default for Tab {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_title() {
        assert_eq!(Tab::new().title(), "New Tab");
    }

    #[test]
    fn title_can_be_set() {
        let mut tab = Tab::new();
        tab.set_title("build");
        assert_eq!(tab.title(), "build");
    }

    #[test]
    fn one_pane_fills_the_tab() {
        let tab = Tab::with_size(80, 24, 1000);
        assert_eq!(tab.pane_count(), 1);
        assert_eq!(tab.focused_index(), Some(0));
        assert_eq!(
            tab.layout(),
            &[Rect {
                col: 0,
                row: 0,
                cols: 80,
                rows: 24
            }]
        );
    }

    #[test]
    fn split_focuses_the_new_pane_and_sizes_both() {
        let mut tab = Tab::with_size(80, 24, 1000);
        let idx = tab.split_active(Dir::Vertical).expect("split");
        assert_eq!(idx, 1);
        assert_eq!(tab.pane_count(), 2);
        assert_eq!(tab.focused_index(), Some(1), "SPLIT-005: new pane focused");
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (40, 24));
        let left = tab.pane_at(0).expect("left pane");
        assert_eq!((left.grid().cols, left.grid().rows), (40, 24));
    }

    #[test]
    fn horizontal_split_stacks_panes() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Horizontal).expect("split");
        let top = tab.pane_at(0).expect("top pane");
        assert_eq!((top.grid().cols, top.grid().rows), (80, 12));
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (80, 12));
    }

    #[test]
    fn close_active_returns_to_the_sibling() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        assert!(!tab.close_active(), "one pane is left, the tab stays");
        assert_eq!(tab.pane_count(), 1);
        assert_eq!(tab.focused_index(), Some(0));
        assert!(tab.close_active(), "last pane: caller closes the tab");
    }

    #[test]
    fn close_pane_out_of_range_is_false() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        assert!(!tab.close_pane(9));
        assert_eq!(tab.pane_count(), 2);
    }

    #[test]
    fn focus_neighbor_walks_the_layout() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        assert_eq!(tab.focused_index(), Some(1));
        assert!(tab.focus_neighbor(Side::Left));
        assert_eq!(tab.focused_index(), Some(0));
        assert!(!tab.focus_neighbor(Side::Left), "no pane left of the edge");
        assert!(tab.focus_neighbor(Side::Right));
        assert_eq!(tab.focused_index(), Some(1));
        assert!(!tab.focus_neighbor(Side::Up), "no pane above");
    }

    #[test]
    fn adjust_ratio_resizes_the_focused_pane() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        assert!(tab.adjust_ratio(Dir::Vertical, -Tab::ratio_step()));
        assert_eq!(tab.pane().grid().cols, 36, "right pane shrinks");
        {
            let left = tab.pane_at(0).expect("left pane");
            assert_eq!(left.grid().cols, 44, "left pane grows");
        }
        assert!(!tab.adjust_ratio(Dir::Horizontal, Tab::ratio_step()));
    }

    #[test]
    fn resize_keeps_ratios_and_resizes_every_pane() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        let _ = tab.resize(100, 30);
        assert_eq!(tab.layout()[0].cols, 50, "SPLIT-010: ratio kept");
        let left = tab.pane_at(0).expect("left pane");
        assert_eq!((left.grid().cols, left.grid().rows), (50, 30));
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (50, 30));
    }

    #[test]
    fn for_each_visits_every_pane_with_its_box() {
        let mut tab = Tab::with_size(80, 20, 1000);
        tab.split_active(Dir::Horizontal).expect("split");
        let mut boxes = Vec::new();
        tab.for_each_pane(|_, rect| boxes.push((rect.col, rect.row, rect.rows)));
        assert_eq!(boxes, vec![(0, 0, 10), (0, 10, 10)]);
    }

    #[test]
    fn feed_goes_to_the_focused_pane() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        tab.pane_mut().feed(b"right pane");
        {
            let left = tab.pane_at(0).expect("left pane");
            assert_eq!(left.grid().line_text(0).as_deref(), Some(""));
        }
        let right = tab.pane();
        assert_eq!(right.grid().line_text(0).as_deref(), Some("right pane"));
    }

    #[test]
    fn spawn_panes_reports_a_spawn_failure_without_a_panic() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        // A program that does not exist: the spawn leg fails for every pane,
        // never a panic, and no live shell/ConPTY is left behind.
        let opts = SpawnOptions::new("rusttyterm-no-such-program.exe", 1, 1);
        let err = tab.spawn_panes(&opts);
        assert!(err.is_err(), "both panes were asked to spawn and failed");
        assert_eq!(tab.pane_count(), 2);
        let mut states = Vec::new();
        tab.for_each_pane_ref(|p, _| states.push(p.shell().state()));
        assert_eq!(states, vec![ShellState::Inactive, ShellState::Inactive]);
    }
}
