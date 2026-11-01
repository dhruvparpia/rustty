//! App model: the ordered tab list with one active tab.
//!
//! This is the root of the data model the renderer and window layer will
//! drive. No windowing code lives here (spec §32 Step 4: data model
//! first).

use crate::shell::{ShellError, SpawnOptions};
use crate::tab::Tab;

/// The application state: tabs plus the active-tab index.
#[derive(Debug)]
pub struct App {
    tabs: Vec<Tab>,
    active: usize,
}

impl App {
    /// Create an app with a single tab.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab::new()],
            active: 0,
        }
    }

    /// Create an app whose first tab is sized for `cols` x `rows` with a
    /// `scrollback`-line history (config `scrollback`, SPEC §13).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            tabs: vec![Tab::with_size(cols, rows, scrollback)],
            active: 0,
        }
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
    /// The list is never empty, so this is always `Some`.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The active tab, mutably.
    #[must_use]
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Append a new tab and make it active; returns its index.
    pub fn add_tab(&mut self) -> usize {
        self.tabs.push(Tab::new());
        self.active = self.tabs.len() - 1;
        self.active
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
        // Keep the selection on the tab that took the closed slot.
        self.active = self.active.min(self.tabs.len() - 1);
        false
    }

    /// Resize the active tab's grid and pseudo-console (window resize
    /// handler entry point, spec §17).
    ///
    /// The grid always resizes; the error only reports a ConPTY resize
    /// failure, which the window layer logs and never panics on.
    pub fn resize_active(&mut self, cols: usize, rows: usize) -> Result<(), ShellError> {
        match self.active_tab_mut() {
            Some(tab) => tab.pane_mut().resize(cols, rows),
            None => Ok(()),
        }
    }

    /// Feed shell output bytes into the active pane, then sync tab titles.
    pub fn feed_active(&mut self, bytes: &[u8]) {
        if let Some(tab) = self.active_tab_mut() {
            tab.pane_mut().feed(bytes);
        }
        self.sync_tab_titles();
    }

    /// Start a shell in the active pane (TTY-001).
    pub fn spawn_active(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        match self.active_tab_mut() {
            Some(tab) => tab.pane_mut().spawn_shell(opts),
            None => Err(ShellError::NotRunning),
        }
    }

    /// Send keyboard/paste bytes to the active pane's shell.
    pub fn send_input_active(&mut self, bytes: &[u8]) -> Result<(), ShellError> {
        match self.active_tab_mut() {
            Some(tab) => tab.pane_mut().send_input(bytes),
            None => Err(ShellError::NotRunning),
        }
    }

    /// Drain every tab's shell reader events (the UI's periodic poll).
    ///
    /// Background tabs keep parsing while you work in another tab, so no
    /// output is lost and no tab blocks the UI thread.
    pub fn drain_shells(&mut self) {
        for tab in &mut self.tabs {
            tab.pane_mut().drain_shell();
        }
        self.sync_tab_titles();
    }

    /// Take the repaint hint across every tab: `true` when any grid changed
    /// since the last call, so the window layer repaints only on change.
    #[must_use]
    pub fn take_dirty(&mut self) -> bool {
        let mut dirty = false;
        for tab in &mut self.tabs {
            dirty |= tab.pane_mut().take_dirty();
        }
        dirty
    }

    /// Clear the active grid's damage flags after a damage repaint has put
    /// the dirty rows on screen ([`crate::render::Renderer::paint_damage`]).
    ///
    /// Background tabs keep their damage set: switching tabs forces a full
    /// repaint, so their pending rows are never lost.
    pub fn clear_grid_damage(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.pane_mut().grid_mut().clear_dirty_rows();
        }
    }

    /// Ask every shell to exit (window close, tab close — TTY-024).
    pub fn close_all_shells(&mut self) {
        for tab in &mut self.tabs {
            tab.pane_mut().close_shell();
        }
    }

    /// Copy each pane's OSC title onto its tab (tab-bar titles, spec §11).
    pub fn sync_tab_titles(&mut self) {
        for tab in &mut self.tabs {
            if let Some(title) = tab.pane().title() {
                let title = title.to_string();
                tab.set_title(title);
            }
        }
    }
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
    fn starts_with_one_tab() {
        let app = App::new();
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.active_index(), 0);
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
    fn resize_reaches_active_grid() {
        let mut app = App::new();
        // No shell attached yet: the grid resizes and the ConPTY leg
        // reports NotRunning (spec §17 chain, never a panic).
        assert!(matches!(
            app.resize_active(120, 30),
            Err(ShellError::NotRunning)
        ));
        let grid = app.active_tab().map(|t| t.pane().grid());
        assert_eq!(grid.map(|g| (g.cols, g.rows)), Some((120, 30)));
    }

    #[test]
    fn feed_active_writes_grid_and_syncs_title() {
        let mut app = App::new();
        app.feed_active(b"ls\x1b]0; ~/src\x07");
        assert_eq!(app.active_tab().map(|t| t.title()), Some(" ~/src"));
        assert_eq!(
            app.active_tab().map(|t| t.pane().grid().line_text(0)),
            Some(Some("ls".to_string()))
        );
    }

    #[test]
    fn drain_and_close_are_safe_with_no_processes() {
        let mut app = App::new();
        app.add_tab();
        app.drain_shells();
        app.close_all_shells();
        assert_eq!(app.tab_count(), 2);
    }
}
