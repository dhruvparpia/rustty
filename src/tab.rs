//! Tab model: tabs are mandatory in the MVP (spec §4, §8).
//!
//! Each tab owns exactly one [`Pane`] for now; v0.3 splits will attach a
//! pane tree to a tab later.

use crate::pane::Pane;

/// A terminal tab: one pane plus its tab-bar title.
#[derive(Debug)]
pub struct Tab {
    pane: Pane,
    title: String,
}

impl Tab {
    /// Create a tab with the default title.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pane: Pane::new(),
            title: String::from("New Tab"),
        }
    }

    /// Create a tab with a sized pane (see [`Pane::with_size`]).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            pane: Pane::with_size(cols, rows, scrollback),
            title: String::from("New Tab"),
        }
    }

    /// The tab's display title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set a user-fixed title (wins over auto-titles).
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// The pane hosted by this tab.
    #[must_use]
    pub fn pane(&self) -> &Pane {
        &self.pane
    }

    /// The pane hosted by this tab, mutably.
    #[must_use]
    pub fn pane_mut(&mut self) -> &mut Pane {
        &mut self.pane
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
}
