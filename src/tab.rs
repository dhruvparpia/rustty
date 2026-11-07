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
use crate::split::{
    DEFAULT_RATIO, Dir, DropZone, PaneTree, RATIO_STEP, Rect, Side, content_box, neighbor,
};

/// A terminal tab: a pane tree, the focused leaf, and the tab-bar title.
#[derive(Debug)]
pub struct Tab {
    /// Stable identity (AUTO-001): assigned at creation, never reused.
    id: u64,
    tree: PaneTree,
    /// Tree leaves in flatten order, kept in step with `layout` (index `i`
    /// of `panes` owns `layout[i]`).
    panes: Vec<Rc<RefCell<Pane>>>,
    /// Focused leaf (SPLIT-005: a new split focuses the new pane).
    active: Rc<RefCell<Pane>>,
    /// Each pane's whole box at `cols` x `rows`, in flatten order, its title row
    /// included. The boxes tile the tab.
    frames: Vec<Rect>,
    /// The part of each frame the pane's text uses, in flatten order.
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
    /// Ask before closing this tab's shells (TAB-018).
    confirm_close: bool,
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
        let frames = tree.layout(cols, rows);
        let layout = frames.clone();
        Self {
            id: crate::automation::next_id(),
            tree,
            panes,
            active: pane,
            frames,
            layout,
            cols,
            rows,
            scrollback,
            title: String::from("New Tab"),
            title_fixed: false,
            profile: String::new(),
            color: None,
            confirm_close: true,
        }
    }

    /// Stable identity (AUTO-001): assigned at creation, never reused.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Flatten index of the pane with this id, `None` when it is not here.
    #[must_use]
    pub fn pane_index_of(&self, id: u64) -> Option<usize> {
        self.panes.iter().position(|pane| pane.borrow().id() == id)
    }

    /// Each pane's dot path from the tab root, in flatten order (the human
    /// alias of the automation ADR §1).
    #[must_use]
    pub fn pane_paths(&self) -> Vec<String> {
        self.tree.paths()
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

    /// TAB-018: does closing this tab's shells need a confirmation?
    #[must_use]
    pub const fn confirm_close(&self) -> bool {
        self.confirm_close
    }

    /// The confirm flag a profile brings (TAB-018).
    pub fn set_confirm(&mut self, close: bool) {
        self.confirm_close = close;
    }

    /// Tab color (TAB-014/015/021: per-profile colour).
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

    /// A handle to the focused pane that does not keep it alive.
    #[must_use]
    pub fn focused_pane_weak(&self) -> std::rc::Weak<RefCell<Pane>> {
        Rc::downgrade(&self.active)
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

    /// The box each pane's text fills, in flatten order (renderer, hit-test).
    #[must_use]
    pub fn layout(&self) -> &[Rect] {
        &self.layout
    }

    /// Each pane's whole box, title row included, in flatten order. These tile the
    /// tab, so borders and neighbours are worked out from them.
    #[must_use]
    pub fn frames(&self) -> &[Rect] {
        &self.frames
    }

    /// Flatten index of the pane whose title row holds the cell `(col, row)`.
    #[must_use]
    pub fn header_at(&self, col: usize, row: usize) -> Option<usize> {
        self.frames
            .iter()
            .zip(&self.layout)
            .position(|(frame, text)| frame.contains(col, row) && !text.contains(col, row))
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

    /// Split the focused pane with the new pane on `side` (SPLIT-016). The new
    /// pane is focused and sized to its box; the caller spawns its shell.
    pub fn split_active_side(&mut self, side: Side) -> Option<usize> {
        let fresh = Rc::new(RefCell::new(Pane::with_size(1, 1, self.scrollback)));
        if !self
            .tree
            .split_side(&self.active, side, DEFAULT_RATIO, Rc::clone(&fresh))
        {
            return None;
        }
        self.active = fresh;
        self.relayout();
        let _ = self.apply_layout();
        self.focused_index()
    }

    /// Move the focused pane toward `side` (SPLIT-017): it trades places with the
    /// neighbour there, or with none, takes that whole edge of the tab. `false`
    /// when it already has that edge to itself.
    pub fn move_active(&mut self, side: Side) -> bool {
        let Some(idx) = self.focused_index() else {
            return false;
        };
        let active = Rc::clone(&self.active);
        let moved = match neighbor(&self.frames, idx, side) {
            Some(other) => {
                let other = Rc::clone(&self.panes[other]);
                self.tree.swap(&active, &other)
            }
            None if self.owns_edge(idx, side) => false,
            None => self.tree.dock(&active, side),
        };
        if moved {
            self.after_move();
        }
        moved
    }

    /// Drop the pane with id `moved` onto the pane with id `target` (SPLIT-017).
    /// The moved pane is focused. `false`, changing nothing, unless they are two
    /// different panes of this tab.
    pub fn move_pane(&mut self, moved: u64, target: u64, zone: DropZone) -> bool {
        let (Some(from), Some(to)) = (self.pane_index_of(moved), self.pane_index_of(target)) else {
            return false;
        };
        let (from, to) = (Rc::clone(&self.panes[from]), Rc::clone(&self.panes[to]));
        let done = match zone {
            DropZone::Swap => self.tree.swap(&from, &to),
            DropZone::Side(side) => self.tree.move_beside(&from, &to, side),
        };
        if done {
            self.active = from;
            self.after_move();
        }
        done
    }

    /// Whether pane `idx` already spans the whole of `side` of the tab.
    fn owns_edge(&self, idx: usize, side: Side) -> bool {
        let Some(f) = self.frames.get(idx) else {
            return true;
        };
        match side {
            Side::Left => f.col == 0 && f.rows == self.rows,
            Side::Right => f.col + f.cols == self.cols && f.rows == self.rows,
            Side::Up => f.row == 0 && f.cols == self.cols,
            Side::Down => f.row + f.rows == self.rows && f.cols == self.cols,
        }
    }

    /// Give every pane its new box. Panes can trade places without changing size,
    /// so each is also marked for repainting.
    fn after_move(&mut self) {
        self.relayout();
        let _ = self.apply_layout();
        self.for_each_pane(|pane, _| {
            pane.grid_mut().mark_all_dirty();
            pane.mark_dirty();
        });
    }

    /// Split the pane at flatten index `idx` and leave focus where it is:
    /// automation splitting a pane must not move the person's typing to another
    /// shell. Returns the new pane's flatten index; the caller spawns its shell.
    pub fn split_keep_focus(&mut self, idx: usize, dir: Dir) -> Option<usize> {
        let target = Rc::clone(self.panes.get(idx)?);
        let fresh = Rc::new(RefCell::new(Pane::with_size(1, 1, self.scrollback)));
        if !self
            .tree
            .split(&target, dir, DEFAULT_RATIO, Rc::clone(&fresh))
        {
            return None;
        }
        self.relayout();
        let _ = self.apply_layout();
        self.panes.iter().position(|pane| Rc::ptr_eq(pane, &fresh))
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
        // Focus the pane that took the closed slot (SPLIT-004), but only
        // when the focused pane itself closed (issue #34: closing a
        // background pane must not steal focus from the selected pane).
        let idx = idx.min(self.tree.count() - 1);
        self.relayout();
        let keep = self.panes.iter().any(|p| Rc::ptr_eq(p, &self.active));
        if !keep {
            self.active = Rc::clone(&self.panes[idx]);
        }
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
        match idx.and_then(|i| neighbor(&self.frames, i, side)) {
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
        self.frames = self.tree.layout(self.cols, self.rows);
        let panes = self.panes.len();
        self.layout.clear();
        self.layout
            .extend(self.frames.iter().map(|frame| content_box(*frame, panes)));
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

    /// Rebuild a tab from a saved pane shape (TAB-026 session restore).
    /// Panes start inactive; the caller spawns shells with
    /// [`Tab::spawn_panes_at`].
    #[must_use]
    pub fn from_shape(
        shape: &crate::session::Shape,
        cols: usize,
        rows: usize,
        scrollback: usize,
    ) -> Self {
        let tree = PaneTree::from_shape(shape, &mut |_| {
            Rc::new(RefCell::new(Pane::with_size(cols, rows, scrollback)))
        });
        let panes = tree.flatten();
        let frames = tree.layout(cols, rows);
        let layout = frames
            .iter()
            .map(|frame| content_box(*frame, panes.len()))
            .collect();
        Self {
            id: crate::automation::next_id(),
            active: panes[0].clone(), // from_shape always leaves at least one leaf
            tree,
            panes,
            frames,
            layout,
            cols,
            rows,
            scrollback,
            title: String::from("New Tab"),
            title_fixed: false,
            profile: String::new(),
            color: None,
            confirm_close: true,
        }
    }

    /// Serializable shape of this tab's pane tree with each pane's OSC 7
    /// cwd (TAB-026 session save).
    #[must_use]
    pub fn shape(&self) -> crate::session::Shape {
        self.tree.shape()
    }

    /// Start a shell in every pane that has none, overriding the working
    /// directory with `cwds[i]` (flatten order) where present (TAB-026).
    pub fn spawn_panes_at(
        &mut self,
        opts: &SpawnOptions,
        cwds: &[Option<String>],
    ) -> Result<(), ShellError> {
        self.spawn_panes_cmds(opts, cwds, &[])
    }

    /// Like [`Tab::spawn_panes_at`], and where `cmds[i]` (flatten order)
    /// is `Some(argv)`, spawn that argv instead of the profile's program:
    /// `argv[0]` is the program, the rest are arguments (WORKSPACE-006).
    pub fn spawn_panes_cmds(
        &mut self,
        opts: &SpawnOptions,
        cwds: &[Option<String>],
        cmds: &[Option<Vec<String>>],
    ) -> Result<(), ShellError> {
        let mut first_error = None;
        let mut idx = 0_usize;
        self.for_each_pane(|pane, rect| {
            if pane.shell().state() == ShellState::Inactive {
                let mut opts = opts.clone();
                opts.cols = rect.cols;
                opts.rows = rect.rows;
                if let Some(Some(cwd)) = cwds.get(idx) {
                    opts.cwd = Some(cwd.clone());
                }
                if let Some(Some(argv)) = cmds.get(idx)
                    && let Some((program, rest)) = argv.split_first()
                {
                    opts.program = program.clone();
                    opts.args = rest.to_vec();
                }
                if let Err(e) = pane.spawn_shell(&opts) {
                    first_error.get_or_insert(e);
                }
            }
            idx += 1;
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
    use crate::split::DropZone;

    #[test]
    fn ids_survive_split_resize_and_closing_a_neighbour() {
        // Break caught (#56): an id moving to another pane when the layout changes.
        let mut tab = Tab::with_size(80, 24, 100);
        let tab_id = tab.id();
        let first = tab.pane().id();
        assert_ne!(tab_id, first, "tabs and panes share one counter");
        tab.split_active(Dir::Vertical);
        let second = tab.pane().id();
        assert_ne!(first, second);
        let _ = tab.resize(100, 30);
        assert_eq!(tab.pane_index_of(first), Some(0));
        assert_eq!(tab.pane_index_of(second), Some(1));
        tab.close_pane(0);
        assert_eq!(tab.id(), tab_id);
        assert_eq!(tab.pane_index_of(first), None);
        assert_eq!(tab.pane_index_of(second), Some(0));
        assert_eq!(tab.pane().id(), second);
    }

    #[test]
    fn splitting_another_pane_leaves_focus_where_it_was() {
        // Break caught: a boss splitting a pane moving the owner's keystrokes
        // to a different shell.
        let mut tab = Tab::with_size(80, 24, 100);
        let first = tab.pane().id();
        tab.split_active(Dir::Vertical);
        let focused = tab.pane().id();
        let fresh = tab.split_keep_focus(0, Dir::Horizontal).expect("split");
        assert_eq!(tab.pane().id(), focused);
        assert_eq!(tab.pane_count(), 3);
        let fresh_id = tab.pane_at(fresh).map(|p| p.id()).expect("pane");
        assert!(fresh_id != first && fresh_id != focused);
        assert_eq!(tab.split_keep_focus(9, Dir::Vertical), None);
        assert_eq!(tab.pane_count(), 3);
    }

    #[test]
    fn pane_paths_follow_the_tree() {
        // ADR §1: panes keep a dot path from the tab root.
        let mut tab = Tab::with_size(80, 24, 100);
        assert_eq!(tab.pane_paths(), vec![".0"]);
        tab.split_active(Dir::Vertical);
        assert_eq!(tab.pane_paths(), vec![".0", ".1"]);
        tab.split_active(Dir::Horizontal);
        assert_eq!(tab.pane_paths(), vec![".0", ".1.0", ".1.1"]);
    }

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
    fn user_rename_wins_over_auto_titles() {
        let mut tab = Tab::new();
        tab.set_title("my tab");
        tab.set_auto_title("PowerShell");
        assert_eq!(tab.title(), "my tab");
        assert!(tab.title_is_fixed());
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
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (40, 23));
        let left = tab.pane_at(0).expect("left pane");
        assert_eq!((left.grid().cols, left.grid().rows), (40, 23));
    }

    #[test]
    fn horizontal_split_stacks_panes() {
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Horizontal).expect("split");
        let top = tab.pane_at(0).expect("top pane");
        assert_eq!((top.grid().cols, top.grid().rows), (80, 11));
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (80, 11));
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
    fn closing_a_background_pane_keeps_the_selected_pane_focused() {
        // Issue #34: pane 1 exits while pane 0 is selected.
        let mut tab = Tab::with_size(80, 24, 1000);
        tab.split_active(Dir::Vertical).expect("split");
        tab.focus_index(0);
        assert!(!tab.close_pane(1), "one pane is left, the tab stays");
        assert_eq!(tab.focused_index(), Some(0), "focus stays on pane 0");
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
    fn split_panes_keep_a_title_row_above_their_text() {
        // Break caught: the title was drawn over row 0, hiding the first line of
        // every pane, so `Windows PowerShell` vanished from a new split.
        let mut tab = Tab::with_size(80, 24, 100);
        let whole = Rect {
            col: 0,
            row: 0,
            cols: 80,
            rows: 24,
        };
        assert_eq!(tab.layout(), &[whole], "one pane has no title row");
        assert_eq!(tab.header_at(3, 0), None);
        tab.split_active(Dir::Vertical).expect("split");
        assert_eq!(tab.frames()[0], Rect { cols: 40, ..whole });
        assert_eq!(
            tab.layout()[0],
            Rect {
                row: 1,
                cols: 40,
                rows: 23,
                ..whole
            }
        );
        assert_eq!(
            tab.pane().grid().rows,
            23,
            "the shell is told the real size"
        );
        assert_eq!(tab.header_at(3, 0), Some(0));
        assert_eq!(tab.header_at(50, 0), Some(1));
        assert_eq!(tab.header_at(3, 1), None, "row 1 is text");
        tab.close_pane(1);
        assert_eq!(tab.layout(), &[whole], "the row goes when the split does");
        assert_eq!(tab.pane().grid().rows, 24);
    }

    #[test]
    fn panes_above_and_below_are_still_neighbours() {
        // The title row must not open a gap in keyboard focus movement.
        let mut tab = Tab::with_size(80, 24, 100);
        tab.split_active(Dir::Horizontal).expect("split");
        assert_eq!(tab.focused_index(), Some(1));
        assert!(tab.focus_neighbor(Side::Up));
        assert_eq!(tab.focused_index(), Some(0));
        assert!(tab.focus_neighbor(Side::Down));
        assert_eq!(tab.focused_index(), Some(1));
    }

    #[test]
    fn split_toward_a_side_focuses_the_new_pane_there() {
        let mut tab = Tab::with_size(80, 24, 100);
        let old = tab.pane().id();
        assert_eq!(tab.split_active_side(Side::Left), Some(0));
        assert_ne!(tab.pane().id(), old);
        assert_eq!(tab.pane_index_of(old), Some(1));
    }

    #[test]
    fn moving_toward_a_neighbour_swaps_with_it_and_keeps_focus() {
        let mut tab = Tab::with_size(80, 24, 100);
        let left = tab.pane().id();
        tab.split_active(Dir::Vertical).expect("split");
        let right = tab.pane().id();
        assert!(tab.move_active(Side::Left));
        assert_eq!(tab.pane_index_of(right), Some(0));
        assert_eq!(tab.pane_index_of(left), Some(1));
        assert_eq!(tab.pane().id(), right, "the moved pane keeps the keyboard");
    }

    #[test]
    fn moving_with_no_neighbour_takes_that_whole_edge() {
        // Side by side, then up: the only way from a row of panes to a stack.
        let mut tab = Tab::with_size(80, 24, 100);
        tab.split_active(Dir::Vertical).expect("split");
        let right = tab.pane().id();
        assert!(tab.move_active(Side::Up));
        assert_eq!(tab.pane_index_of(right), Some(0));
        assert_eq!(tab.frames()[0].cols, 80, "across the whole tab");
        assert_eq!(tab.pane().grid().cols, 80);
        assert_eq!(tab.pane().grid().rows, 11);
    }

    #[test]
    fn moving_at_the_edge_it_already_owns_does_nothing() {
        let mut tab = Tab::with_size(80, 24, 100);
        tab.split_active(Dir::Vertical).expect("split");
        let before = tab.frames().to_vec();
        assert!(!tab.move_active(Side::Right));
        assert_eq!(tab.frames(), before.as_slice());
    }

    #[test]
    fn a_lone_pane_has_nowhere_to_move() {
        let mut tab = Tab::with_size(80, 24, 100);
        assert!(!tab.move_active(Side::Left));
    }

    #[test]
    fn dropping_a_pane_moves_it_and_focuses_it() {
        let mut tab = Tab::with_size(90, 24, 100);
        let a = tab.pane().id();
        tab.split_active(Dir::Vertical).expect("split");
        let b = tab.pane().id();
        tab.split_active(Dir::Horizontal).expect("split");
        let c = tab.pane().id();
        // Drop a below c.
        assert!(tab.move_pane(a, c, DropZone::Side(Side::Down)));
        assert_eq!(tab.pane().id(), a);
        assert_eq!(tab.pane_index_of(b), Some(0));
        assert_eq!(tab.pane_index_of(c), Some(1));
        assert_eq!(tab.pane_index_of(a), Some(2));
        assert_eq!(tab.frames()[0].cols, 90, "b has the top row to itself");
        // Drop b on the middle of a: they trade places.
        assert!(tab.move_pane(b, a, DropZone::Swap));
        assert_eq!(tab.pane_index_of(a), Some(0));
        assert_eq!(tab.pane_index_of(b), Some(2));
    }

    #[test]
    fn dropping_on_itself_or_an_unknown_pane_changes_nothing() {
        let mut tab = Tab::with_size(80, 24, 100);
        let a = tab.pane().id();
        tab.split_active(Dir::Vertical).expect("split");
        assert!(!tab.move_pane(a, a, DropZone::Swap));
        assert!(!tab.move_pane(a, u64::MAX, DropZone::Swap));
        assert!(!tab.move_pane(u64::MAX, a, DropZone::Swap));
        assert_eq!(tab.pane_count(), 2);
    }

    #[test]
    fn a_swap_of_equal_panes_still_repaints() {
        // Break caught: equal boxes mean no resize, so nothing told the renderer
        // the panes had changed places and the old pictures stayed up.
        let mut tab = Tab::with_size(80, 24, 100);
        tab.split_active(Dir::Vertical).expect("split");
        tab.for_each_pane(|pane, _| {
            let _ = pane.grid_mut().take_dirty_rows();
        });
        assert!(tab.move_active(Side::Left));
        tab.for_each_pane(|pane, _| assert!(pane.grid().all_dirty()));
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
        assert_eq!((left.grid().cols, left.grid().rows), (50, 29));
        assert_eq!((tab.pane().grid().cols, tab.pane().grid().rows), (50, 29));
    }

    #[test]
    fn for_each_visits_every_pane_with_its_box() {
        let mut tab = Tab::with_size(80, 20, 1000);
        tab.split_active(Dir::Horizontal).expect("split");
        let mut boxes = Vec::new();
        tab.for_each_pane(|_, rect| boxes.push((rect.col, rect.row, rect.rows)));
        assert_eq!(boxes, vec![(0, 1, 9), (0, 11, 9)]);
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
        let opts = SpawnOptions::new("rustty-no-such-program.exe", 1, 1);
        let err = tab.spawn_panes(&opts);
        assert!(err.is_err(), "both panes were asked to spawn and failed");
        assert_eq!(tab.pane_count(), 2);
        let mut states = Vec::new();
        tab.for_each_pane_ref(|p, _| states.push(p.shell().state()));
        assert_eq!(states, vec![ShellState::Inactive, ShellState::Inactive]);
    }

    #[test]
    fn shape_round_trips_through_a_rebuilt_tab() {
        let mut tab = Tab::with_size(80, 24, 100);
        tab.split_active(Dir::Vertical).expect("split");
        tab.split_active(Dir::Horizontal).expect("split");
        let shape = tab.shape();
        let rebuilt = Tab::from_shape(&shape, 80, 24, 100);
        assert_eq!(rebuilt.pane_count(), 3);
        assert_eq!(rebuilt.layout(), tab.layout());
        assert_eq!(rebuilt.shape(), shape);
    }
}
