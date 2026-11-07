//! App model: the ordered tab list with one active tab.
//!
//! This is the root of the data model the renderer and window layer drive.
//! No windowing code lives here (spec §32 Step 4: data model first).
//!
//! Every tab is a pane tree ([`crate::tab::Tab`], SPEC SPLIT-001..015), so
//! the pane-wide operations (drain, dirty, close) walk every pane of every
//! tab, while the input operations (feed, spawn, send) target the focused
//! pane of the active tab. See [`crate::tab`] for the `RefCell` borrow rule.

use crate::color::Color;
use crate::shell::{ShellError, SpawnOptions};
use crate::split::{Dir, DropZone, Rect, Side};
use crate::tab::Tab;

/// Outcome of [`App::close_tab_pane`] (SPLIT-004/006 shell-death cascade):
/// the pane closed, the pane's tab closed with it, or the closed tab was
/// the last tab, so the window should quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneClose {
    /// The pane closed; its tab stays open.
    PaneClosed,
    /// The tab's last pane died, so the tab closed; other tabs remain.
    TabClosed,
    /// The closed tab was the last tab: quit the window.
    WindowQuit,
    /// The tab or pane index was out of range: nothing happened.
    NotFound,
}

/// The application state: tabs plus the active-tab index.
#[derive(Debug)]
pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    /// TAB-025: a pane that sets no title shows the directory it reported
    /// with OSC 7.
    show_cwd: bool,
    /// Colors for ANSI-16 output in every pane (the window's theme).
    palette: [Color; 16],
    /// This window's automation id: shells it starts learn it (AUTO-006).
    window_id: Option<u64>,
}

impl App {
    /// Create an app with a single tab.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab::new()],
            active: 0,
            show_cwd: true,
            palette: crate::color::ansi_palette(),
            window_id: None,
        }
    }

    /// Create an app whose first tab is sized for `cols` x `rows` with a
    /// `scrollback`-line history (config `scrollback`, SPEC §13).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            tabs: vec![Tab::with_size(cols, rows, scrollback)],
            active: 0,
            show_cwd: true,
            palette: crate::color::ansi_palette(),
            window_id: None,
        }
    }

    /// Set the automation id shells started from now on are told about.
    pub const fn set_window_id(&mut self, id: Option<u64>) {
        self.window_id = id;
    }

    /// Tell a shell about to start which window it belongs to (AUTO-006).
    pub fn apply_session_env(&self, opts: &mut SpawnOptions) {
        if let Some(id) = self.window_id {
            opts.add_window_env(id);
        }
    }

    /// Index of the tab with this id (AUTO-001). `None` once it has closed:
    /// ids are never reused, so a stale id can't find the tab in its slot.
    #[must_use]
    pub fn locate_tab(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id() == id)
    }

    /// Tab index and flatten index of the pane with this id (AUTO-001).
    #[must_use]
    pub fn locate_pane(&self, id: u64) -> Option<(usize, usize)> {
        self.tabs
            .iter()
            .enumerate()
            .find_map(|(t, tab)| tab.pane_index_of(id).map(|p| (t, p)))
    }

    /// Split a pane in any tab, keeping the selected tab and its focus, and
    /// return the new pane's id. The caller spawns its shell.
    pub fn split_tab_pane(&mut self, tab_idx: usize, pane_idx: usize, dir: Dir) -> Option<u64> {
        let tab = self.tabs.get_mut(tab_idx)?;
        let idx = tab.split_keep_focus(pane_idx, dir)?;
        let id = tab.pane_at(idx)?.id();
        self.apply_palette();
        self.sync_tab_titles();
        Some(id)
    }

    /// TAB-018: the active tab asks before its shells are closed.
    #[must_use]
    pub fn tab_confirm_close(&self) -> bool {
        self.active_tab().is_some_and(Tab::confirm_close)
    }

    /// Turn the cwd fallback in tab titles on or off (`tab-bar.show-cwd`).
    pub fn set_show_cwd(&mut self, on: bool) {
        self.show_cwd = on;
    }

    /// All tabs, in tab-bar order.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Index of the active tab.
    #[must_use]
    pub const fn active_index(&self) -> usize {
        self.active
    }

    /// Number of tabs.
    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The active tab.
    ///
    /// The list is never empty in a running app, so this is `Some` in
    /// practice.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The active tab, mutably.
    #[must_use]
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// A tab by index, mutably (rename, reorder and profile changes land on
    /// tabs other than the active one).
    #[must_use]
    pub fn tab_at_mut(&mut self, idx: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(idx)
    }

    /// Colors for ANSI-16 output in every pane, now and in panes opened later.
    pub fn set_palette(&mut self, palette: [Color; 16]) {
        self.palette = palette;
        self.apply_palette();
    }

    /// Change colors already written in every pane, for a new theme (THEME-016).
    /// When nothing changes, as between tabs sharing a theme, no line is touched.
    pub fn recolor(&mut self, map: &[(Color, Color)]) {
        if map.iter().all(|(from, to)| from == to) {
            return;
        }
        for tab in &mut self.tabs {
            tab.for_each_pane(|pane, _| pane.recolor(map));
        }
    }

    /// Push the current palette to every pane. Panes are few, and this only
    /// runs when a tab or pane opens or the theme changes.
    fn apply_palette(&mut self) {
        let palette = self.palette;
        for tab in &mut self.tabs {
            tab.for_each_pane(|pane, _| pane.set_palette(palette));
        }
    }

    /// Append a new tab (sized like the current one) and make it active;
    /// returns its index.
    pub fn add_tab(&mut self) -> usize {
        let (cols, rows, scrollback) = self.active_tab().map_or((80, 24, 1000), |tab| {
            (tab.cols(), tab.rows(), tab.scrollback())
        });
        self.tabs.push(Tab::with_size(cols, rows, scrollback));
        self.active = self.tabs.len() - 1;
        self.apply_palette();
        self.active
    }

    /// Append a tab sized `cols` x `rows` with `scrollback`-line history and
    /// make it active; returns its index.
    pub fn add_tab_sized(&mut self, cols: usize, rows: usize, scrollback: usize) -> usize {
        self.tabs.push(Tab::with_size(cols, rows, scrollback));
        self.active = self.tabs.len() - 1;
        self.apply_palette();
        self.active
    }

    /// Append a tab and set its profile label and tab color.
    pub fn add_tab_profile(
        &mut self,
        cols: usize,
        rows: usize,
        scrollback: usize,
        profile: impl Into<String>,
        color: Option<Color>,
    ) -> usize {
        let mut tab = Tab::with_size(cols, rows, scrollback);
        let profile = profile.into();
        tab.set_profile(profile.clone());
        tab.set_auto_title(profile);
        tab.set_color(color);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.apply_palette();
        self.active
    }

    /// Append a tab rebuilt from a saved pane shape and make it active;
    /// returns its index (TAB-026 session restore). The caller sets the
    /// title and spawns shells.
    pub fn add_tab_shape(
        &mut self,
        shape: &crate::session::Shape,
        cols: usize,
        rows: usize,
        scrollback: usize,
        profile: impl Into<String>,
        color: Option<Color>,
    ) -> usize {
        let mut tab = Tab::from_shape(shape, cols, rows, scrollback);
        let profile = profile.into();
        tab.set_profile(profile.clone());
        tab.set_auto_title(profile);
        tab.set_color(color);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.apply_palette();
        self.active
    }

    /// Index of the active tab (0 when there are none) for session save.
    #[must_use]
    pub const fn active_tab_index(&self) -> usize {
        self.active
    }

    /// Move the active tab left/right (TAB-017).
    pub fn move_active_tab(&mut self, delta: isize) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let len = self.tabs.len() as isize;
        let to = (self.active as isize + delta).rem_euclid(len) as usize;
        self.move_tab(self.active, to)
    }

    /// Move tab `from` to `to` (TAB-016/017).
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = to;
        true
    }

    /// Select the tab at `idx`; returns `false` when out of range.
    pub fn select_tab(&mut self, idx: usize) -> bool {
        if idx < self.tabs.len() {
            self.active = idx;
            true
        } else {
            false
        }
    }

    /// Close the tab at `idx`.
    ///
    /// Closing the last remaining tab reports `true` so the window layer
    /// can quit. Returns `false` when `idx` is out of range.
    pub fn close_tab(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() {
            return false;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            return true;
        }
        // Issue #19: closing an earlier tab shifts the selection left so
        // it stays on the same tab; closing the active tab keeps the
        // selection on the tab that took the slot (clamped below).
        if idx < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        false
    }

    // ---- focused-pane operations (SPEC §9/§10) -------------------------

    /// Number of panes in the active tab (1 when the tab is not split).
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.active_tab().map_or(0, Tab::pane_count)
    }

    /// The focused pane's box inside the active tab (SPLIT-003, WIN-004).
    #[must_use]
    pub fn active_rect(&self) -> Option<Rect> {
        self.active_tab().and_then(Tab::active_rect)
    }

    /// Resize the active tab to `cols` x `rows`: every pane's grid and
    /// pseudo-console follows its layout box, ratios kept (window resize
    /// handler entry point, spec §17, SPLIT-010).
    ///
    /// The grids always resize; the error only reports a ConPTY resize
    /// failure, which the window layer logs and never panics on.
    pub fn resize_active(&mut self, cols: usize, rows: usize) -> Result<(), ShellError> {
        match self.active_tab_mut() {
            Some(tab) => tab.resize(cols, rows),
            None => Ok(()),
        }
    }

    /// BELL-003: which tabs have an unhandled bell, one flag per tab.
    #[must_use]
    pub fn bell_flags(&self) -> Vec<bool> {
        self.tabs().iter().map(Tab::bell_pending).collect()
    }

    /// True when any tab has an unhandled bell.
    #[must_use]
    pub fn any_bell(&self) -> bool {
        self.tabs().iter().any(Tab::bell_pending)
    }

    /// Clear every pane's bell flag (the window layer rang once).
    pub fn take_bells(&mut self) {
        for tab in &mut self.tabs {
            tab.take_bell();
        }
    }

    /// Resize every tab to `cols` x `rows`. A per-profile font changes the
    /// grid the window fits (SHELL-019), so all tabs have to follow. The
    /// first ConPTY failure is returned; grids always resize.
    pub fn resize_all_tabs(&mut self, cols: usize, rows: usize) -> Result<(), ShellError> {
        let mut failed = None;
        for tab in &mut self.tabs {
            if let Err(e) = tab.resize(cols, rows)
                && failed.is_none()
            {
                failed = Some(e);
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Split the focused pane of the active tab (SPLIT-005):
    /// [`Dir::Vertical`] = new pane on the right, [`Dir::Horizontal`] = new
    /// pane below. The new pane is focused and returns its flatten index;
    /// the caller spawns its shell (SPLIT-007).
    pub fn split_active(&mut self, dir: Dir) -> Option<usize> {
        let tab = self.active_tab_mut()?;
        let idx = tab.split_active(dir)?;
        self.apply_palette();
        self.sync_tab_titles();
        Some(idx)
    }

    /// Split the focused pane of the active tab with the new pane on `side`
    /// (SPLIT-016). The new pane is focused and returns its flatten index; the
    /// caller spawns its shell.
    pub fn split_active_side(&mut self, side: Side) -> Option<usize> {
        let tab = self.active_tab_mut()?;
        let idx = tab.split_active_side(side)?;
        self.apply_palette();
        self.sync_tab_titles();
        Some(idx)
    }

    /// Move the focused pane of the active tab toward `side` (SPLIT-017).
    pub fn move_active_pane(&mut self, side: Side) -> bool {
        self.active_tab_mut().is_some_and(|t| t.move_active(side))
    }

    /// Drop one pane of the active tab onto another (SPLIT-017).
    pub fn move_pane(&mut self, moved: u64, target: u64, zone: DropZone) -> bool {
        let done = self
            .active_tab_mut()
            .is_some_and(|t| t.move_pane(moved, target, zone));
        if done {
            self.sync_tab_titles();
        }
        done
    }

    /// Close the focused pane of the active tab (SPLIT-006). `true` when it
    /// was the tab's last pane, so the caller closes the tab.
    pub fn close_active_pane(&mut self) -> bool {
        let quit = self.active_tab_mut().is_none_or(Tab::close_active);
        self.sync_tab_titles();
        quit
    }

    /// Close the pane at `idx` of the active tab (SPLIT-004, shell death).
    /// `true` when it was the tab's last pane, so the caller closes the tab.
    pub fn close_active_pane_at(&mut self, idx: usize) -> bool {
        let quit = self.active_tab_mut().is_none_or(|tab| tab.close_pane(idx));
        self.sync_tab_titles();
        quit
    }

    /// Close the pane at `idx` of the tab at `tab_idx` (SPLIT-004, shell
    /// death in a background tab). See [`PaneClose`] for what the result
    /// reports; closing the tab at the active index keeps the active
    /// selection valid.
    pub fn close_tab_pane(&mut self, tab_idx: usize, idx: usize) -> PaneClose {
        let alive = self
            .tabs
            .get(tab_idx)
            .is_some_and(|tab| tab.pane_count() > idx);
        if !alive {
            return PaneClose::NotFound;
        }
        // `idx` is in range, so `close_pane` reports the last-pane case.
        let last = self
            .tabs
            .get_mut(tab_idx)
            .is_some_and(|tab| tab.close_pane(idx));
        if !last {
            self.sync_tab_titles();
            return PaneClose::PaneClosed;
        }
        if self.close_tab(tab_idx) {
            PaneClose::WindowQuit
        } else {
            PaneClose::TabClosed
        }
    }

    /// Move pane focus inside the active tab (SPLIT-008). `false` when the
    /// tab is not split in that direction.
    pub fn focus_neighbor(&mut self, side: Side) -> bool {
        self.active_tab_mut().is_some_and(|tab| {
            let moved = tab.focus_neighbor(side);
            if moved {
                // Focus moved: the hollow/solid cursor swap repaints via
                // the caller's InvalidateRect, so drop the grid's dirty
                // flag rather than queue a second paint.
                let _ = tab.pane_mut().take_dirty();
            }
            moved
        })
    }

    /// Grow/shrink the focused pane by one [`crate::split::RATIO_STEP`]
    /// (SPLIT-009 keyboard pane resize). `true` when a split was adjusted.
    pub fn adjust_ratio(&mut self, dir: Dir, delta: f32) -> bool {
        self.active_tab_mut()
            .is_some_and(|tab| tab.adjust_ratio(dir, delta))
    }

    /// One keyboard pane-resize step (SPLIT-009, [`crate::split::RATIO_STEP`]).
    #[must_use]
    pub const fn ratio_step() -> f32 {
        Tab::ratio_step()
    }

    /// Focus the pane at `idx` of the active tab (mouse click, SPLIT-003).
    pub fn focus_pane(&mut self, idx: usize) -> bool {
        self.active_tab_mut().is_some_and(|tab| {
            let moved = tab.focus_index(idx);
            if moved {
                // See `focus_neighbor`: focus moves repaint through the
                // caller, so clear the grid's dirty flag here.
                let _ = tab.pane_mut().take_dirty();
            }
            moved
        })
    }

    /// Feed shell output bytes into the focused pane, then sync tab titles.
    pub fn feed_active(&mut self, bytes: &[u8]) {
        if let Some(tab) = self.active_tab_mut() {
            tab.pane_mut().feed(bytes);
        }
        self.sync_tab_titles();
    }

    /// Start a shell in the focused pane of the active tab, sized to its
    /// box (TTY-001, SPLIT-007: split panes get the tab's profile).
    pub fn spawn_active(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        let window_id = self.window_id;
        match self.active_tab_mut() {
            Some(tab) => {
                let (cols, rows) = tab
                    .active_rect()
                    .map_or((opts.cols, opts.rows), |r| (r.cols, r.rows));
                let mut opts = SpawnOptions {
                    cols,
                    rows,
                    ..opts.clone()
                };
                if let Some(id) = window_id {
                    opts.add_window_env(id);
                }
                tab.pane_mut().spawn_shell(&opts)
            }
            None => Err(ShellError::NotRunning),
        }
    }

    /// Send keyboard/paste bytes to the focused pane's shell.
    pub fn send_input_active(&mut self, bytes: &[u8]) -> Result<(), ShellError> {
        match self.active_tab_mut() {
            Some(tab) => tab.pane_mut().send_input(bytes),
            None => Err(ShellError::NotRunning),
        }
    }

    /// Route bytes to pane `idx` in the active tab.
    pub fn send_input_pane(&mut self, idx: usize, bytes: &[u8]) -> Result<(), ShellError> {
        match self.active_tab_mut().and_then(|tab| tab.pane_at_mut(idx)) {
            Some(mut pane) => pane.send_input(bytes),
            None => Err(ShellError::NotRunning),
        }
    }

    // ---- all-pane operations -------------------------------------------

    /// Drain every pane's shell reader events (the UI's periodic poll).
    ///
    /// Background tabs and unfocused panes keep parsing while you work
    /// elsewhere, so no output is lost and no tab blocks the UI thread.
    pub fn drain_shells(&mut self) {
        for tab in &mut self.tabs {
            tab.for_each_pane(|pane, _| pane.drain_shell());
        }
        self.sync_tab_titles();
    }

    /// Take the repaint hint across every pane: `true` when any grid changed
    /// since the last call, so the window layer repaints only on change.
    #[must_use]
    pub fn take_dirty(&mut self) -> bool {
        let mut dirty = false;
        for tab in &mut self.tabs {
            tab.for_each_pane(|pane, _| dirty |= pane.take_dirty());
        }
        dirty
    }

    /// Clear the active tab's grid damage flags after a damage repaint has
    /// put the dirty rows on screen ([`crate::render::Renderer::paint_damage`]).
    ///
    /// Background tabs keep their damage set: switching tabs forces a full
    /// repaint, so their pending rows are never lost.
    pub fn clear_grid_damage(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.for_each_pane(|pane, _| pane.grid_mut().clear_dirty_rows());
        }
    }

    /// Ask every shell to exit (window close, tab close — TTY-024).
    pub fn close_all_shells(&mut self) {
        for tab in &mut self.tabs {
            tab.for_each_pane(|pane, _| pane.close_shell());
        }
    }

    /// Copy the focused pane's OSC title onto its tab (tab-bar titles,
    /// spec §11, SPLIT-011/012: the tab shows what its focused pane runs).
    /// With `show_cwd` (TAB-025) a pane that sets no title shows the
    /// directory it reported with OSC 7.
    pub fn sync_tab_titles(&mut self) {
        let show_cwd = self.show_cwd;
        for tab in &mut self.tabs {
            // Read the pane through its own borrow, then release it, so the
            // tab is writable again below.
            let title = {
                let pane = tab.pane();
                pane.title().map(str::to_string).or_else(|| {
                    if show_cwd {
                        pane.cwd().and_then(last_path_component)
                    } else {
                        None
                    }
                })
            };
            if let Some(title) = title {
                tab.set_auto_title(title);
            }
        }
    }
}

/// Last directory component of a path, `/` or `\` separated (TAB-025).
fn last_path_component(path: &str) -> Option<String> {
    path.split(['/', '\\'])
        .rfind(|part| !part.is_empty() && !part.ends_with(':'))
        .map(str::to_string)
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_found_by_id_and_gone_after_closing() {
        // Break caught (#56): a stale id resolving to whatever took its slot.
        let mut app = App::with_size(80, 24, 100);
        app.add_tab_sized(80, 24, 100);
        let (first_tab, first_pane) = {
            let tab = &app.tabs()[0];
            (tab.id(), tab.pane().id())
        };
        let second_tab = app.tabs()[1].id();
        assert_eq!(app.locate_tab(first_tab), Some(0));
        assert_eq!(app.locate_tab(second_tab), Some(1));
        assert_eq!(app.locate_pane(first_pane), Some((0, 0)));
        assert!(!app.close_tab(0));
        assert_eq!(app.locate_tab(first_tab), None);
        assert_eq!(app.locate_pane(first_pane), None);
        assert_eq!(app.locate_tab(second_tab), Some(0));
        app.add_tab_sized(80, 24, 100);
        assert_ne!(app.tabs()[1].id(), first_tab, "ids are never reused");
    }

    #[test]
    fn splitting_a_background_tab_keeps_the_selection() {
        let mut app = App::with_size(80, 24, 100);
        app.add_tab_sized(80, 24, 100);
        app.select_tab(1);
        let fresh = app
            .split_tab_pane(0, 0, Dir::Vertical)
            .expect("split in the background tab");
        assert_eq!(app.active_index(), 1);
        assert_eq!(app.pane_count(), 1, "the active tab did not change");
        assert_eq!(app.locate_pane(fresh), Some((0, 1)));
        assert_eq!(app.split_tab_pane(5, 0, Dir::Vertical), None);
    }

    #[test]
    fn every_shell_a_window_starts_names_that_window() {
        // AUTO-006: RUSTTY_WINDOW selects a session's own window.
        let mut app = App::new();
        let mut opts = SpawnOptions::new("x", 80, 24);
        app.apply_session_env(&mut opts);
        assert!(!opts.env.contains_key("RUSTTY_WINDOW"));
        app.set_window_id(Some(42));
        app.apply_session_env(&mut opts);
        assert_eq!(
            opts.env.get("RUSTTY_WINDOW").map(String::as_str),
            Some("42")
        );
    }

    #[test]
    fn starts_with_one_tab() {
        let app = App::new();
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.active_index(), 0);
        assert_eq!(app.pane_count(), 1);
    }

    #[test]
    fn add_tab_becomes_active() {
        let mut app = App::new();
        let idx = app.add_tab();
        assert_eq!(idx, 1);
        assert_eq!(app.active_index(), 1);
        assert_eq!(app.tab_count(), 2);
    }

    #[test]
    fn add_tab_matches_the_active_tab_size() {
        let mut app = App::with_size(120, 30, 1000);
        app.add_tab();
        assert_eq!(
            app.active_tab().map(|t| (t.cols(), t.rows())),
            Some((120, 30))
        );
    }

    #[test]
    fn select_out_of_range_is_false() {
        let mut app = App::new();
        assert!(!app.select_tab(5));
        assert_eq!(app.active_index(), 0);
    }

    #[test]
    fn close_tab_keeps_selection_valid() {
        let mut app = App::new();
        app.add_tab();
        app.add_tab();
        app.select_tab(2);
        assert!(!app.close_tab(2));
        assert_eq!(app.tab_count(), 2);
        assert_eq!(app.active_index(), 1);
    }

    #[test]
    fn closing_last_tab_reports_quit() {
        let mut app = App::new();
        assert!(app.close_tab(0));
        assert_eq!(app.tab_count(), 0);
    }

    #[test]
    fn close_out_of_range_is_false() {
        let mut app = App::new();
        assert!(!app.close_tab(3));
    }

    #[test]
    fn resize_all_tabs_reaches_every_tab() {
        let mut app = App::with_size(80, 24, 1000);
        app.add_tab();
        // Tab 2 has no shell, so the ConPTY resize reports an error; the
        // grids of every tab must still follow.
        assert!(app.resize_all_tabs(100, 30).is_err());
        for tab in app.tabs() {
            assert_eq!((tab.cols(), tab.rows()), (100, 30));
        }
    }

    #[test]
    fn resize_reaches_every_pane_grid() {
        let mut app = App::with_size(80, 24, 1000);
        app.split_active(Dir::Vertical);
        // No shell attached yet: the grids resize and the ConPTY leg
        // reports NotRunning (spec §17 chain, never a panic).
        assert!(matches!(
            app.resize_active(120, 30),
            Err(ShellError::NotRunning)
        ));
        let sizes = app.active_tab().map(|tab| {
            let mut out = Vec::new();
            tab.for_each_pane_ref(|pane, _| out.push((pane.grid().cols, pane.grid().rows)));
            out
        });
        assert_eq!(sizes, Some(vec![(60, 29), (60, 29)]));
    }

    #[test]
    fn feed_active_writes_grid_and_syncs_title() {
        let mut app = App::new();
        app.feed_active(b"ls\x1b]0; ~/src\x07");
        assert_eq!(app.active_tab().map(|t| t.title()), Some(" ~/src"));
        let line = app
            .active_tab()
            .map(|t| t.pane().grid().line_text(0).unwrap_or_default());
        assert_eq!(line.as_deref(), Some("ls"));
    }

    #[test]
    fn split_then_feed_targets_the_new_pane() {
        let mut app = App::with_size(80, 24, 1000);
        assert_eq!(app.split_active(Dir::Vertical), Some(1));
        app.feed_active(b"right");
        let left = app
            .active_tab()
            .and_then(|t| t.pane_at(0))
            .map(|p| p.grid().line_text(0).unwrap_or_default());
        assert_eq!(left.as_deref(), Some(""));
        let right = app.active_tab().map(|t| t.pane().grid().line_text(0));
        assert_eq!(right.flatten().as_deref(), Some("right"));
    }

    #[test]
    fn close_active_pane_reports_last_pane() {
        let mut app = App::with_size(80, 24, 1000);
        app.split_active(Dir::Vertical);
        assert_eq!(app.pane_count(), 2);
        assert!(
            !app.close_active_pane(),
            "the sibling stays, so the tab stays open"
        );
        assert_eq!(app.pane_count(), 1);
        assert!(app.close_active_pane(), "last pane: caller closes the tab");
        assert_eq!(app.pane_count(), 1);
    }

    #[test]
    fn pane_focus_and_ratio_keys_walk_the_active_tab() {
        let mut app = App::with_size(80, 24, 1000);
        app.split_active(Dir::Vertical);
        assert!(app.focus_neighbor(Side::Left));
        let idx = app.active_tab().and_then(Tab::focused_index);
        assert_eq!(idx, Some(0));
        assert!(app.adjust_ratio(Dir::Vertical, Tab::ratio_step()));
        assert_eq!(app.active_rect().map(|r| r.cols), Some(44));
        assert!(!app.focus_neighbor(Side::Up));
    }

    #[test]
    fn tab_title_follows_the_focused_pane() {
        let mut app = App::with_size(80, 24, 1000);
        app.split_active(Dir::Vertical);
        app.feed_active(b"\x1b]0;right shell\x07");
        assert_eq!(app.active_tab().map(|t| t.title()), Some("right shell"));
        app.focus_pane(0);
        app.feed_active(b"\x1b]0;left shell\x07");
        assert_eq!(app.active_tab().map(|t| t.title()), Some("left shell"));
    }

    #[test]
    fn drain_and_close_are_safe_with_no_processes() {
        let mut app = App::new();
        app.add_tab();
        app.drain_shells();
        app.close_all_shells();
        assert_eq!(app.tab_count(), 2);
    }

    #[test]
    fn dirty_flags_cover_every_pane() {
        let mut app = App::with_size(80, 24, 1000);
        app.split_active(Dir::Vertical);
        let _ = app.take_dirty();
        assert!(!app.take_dirty(), "a fresh split frame is drained once");
        app.focus_pane(0);
        app.feed_active(b"x");
        assert!(app.take_dirty(), "an unfocused pane's output repaints");
    }

    #[test]
    fn a_recolor_reaches_every_tab_and_pane() {
        let (old, new) = (Color::rgb(1, 1, 1), Color::rgb(2, 2, 2));
        let mut palette = [Color::rgb(9, 9, 9); 16];
        palette[1] = old;
        let mut app = App::new();
        app.set_palette(palette);
        app.feed_active(b"\x1b[31mfirst");
        app.add_tab();
        assert!(app.split_active(Dir::Vertical).is_some());
        app.feed_active(b"\x1b[31msplit");
        app.recolor(&[(old, new)]);
        let first = app
            .tab_at_mut(0)
            .map(|t| t.pane().grid().row(0).unwrap().cells[0].fg);
        assert_eq!(first, Some(Some(new)));
        let split = app
            .active_tab()
            .map(|t| t.pane().grid().row(0).unwrap().cells[0].fg);
        assert_eq!(split, Some(Some(new)));
    }

    #[test]
    fn recoloring_to_the_same_colors_marks_nothing_to_repaint() {
        // Switching between tabs that share a theme must not walk every line.
        let same = Color::rgb(1, 1, 1);
        let mut app = App::new();
        let _ = app.take_dirty();
        app.recolor(&[(same, same)]);
        assert!(!app.take_dirty());
    }

    #[test]
    fn new_tabs_and_splits_use_the_app_palette() {
        // Break caught: a tab or split opened after a theme change keeping
        // the default colours, and existing panes not following the change.
        let palette = [Color::rgb(7, 8, 9); 16];
        let mut app = App::new();
        app.set_palette(palette);
        let first = app.active_tab().map(|t| t.pane().parser().palette);
        assert_eq!(first, Some(palette));
        app.add_tab();
        let added = app.active_tab().map(|t| t.pane().parser().palette);
        assert_eq!(added, Some(palette));
        assert!(app.split_active(Dir::Vertical).is_some());
        let split = app.active_tab().map(|t| t.pane().parser().palette);
        assert_eq!(split, Some(palette));
    }
}
