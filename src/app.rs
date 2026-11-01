//! App model: the ordered tab list with one active tab.
//!
//! This is the root of the data model the renderer and window layer will
//! drive. No windowing code lives here (spec §32 Step 4: data model
//! first).

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

    /// Resize the active tab's grid (window resize handler entry point).
    pub fn resize_active(&mut self, cols: usize, rows: usize) {
        if let Some(tab) = self.active_tab_mut() {
            tab.pane_mut().resize(cols, rows);
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
        app.resize_active(120, 30);
        let grid = app.active_tab().map(|t| t.pane().grid());
        assert_eq!(grid.map(|g| (g.cols, g.rows)), Some((120, 30)));
    }
}
