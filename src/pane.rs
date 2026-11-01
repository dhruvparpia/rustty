//! Pane: the split leaf that will hold a grid + shell.
//!
//! Splits are out of MVP scope (spec §4), so a pane is a 1:1 owner of one
//! [`Grid`] and one [`Shell`] for now. The split tree in v0.3 will reuse
//! this type unchanged.

use crate::grid::Grid;
use crate::shell::Shell;

/// A single terminal pane (one grid, one shell process).
#[derive(Debug)]
pub struct Pane {
    grid: Grid,
    shell: Shell,
}

impl Pane {
    /// Create a pane with a fresh grid and an inactive shell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: Grid::new(),
            shell: Shell::new(),
        }
    }

    /// The pane's grid.
    #[must_use]
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// The pane's grid, mutably (the parser writes through this).
    #[must_use]
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// The pane's shell.
    #[must_use]
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// The pane's shell, mutably (input/resize/close go through this).
    #[must_use]
    pub fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }

    /// Resize the pane's grid to `cols` x `rows`.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

impl Default for Pane {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pane_is_inactive() {
        let pane = Pane::new();
        assert!(pane.shell().state() == crate::shell::ShellState::Inactive);
        assert_eq!(pane.grid().cols, 80);
    }
}
