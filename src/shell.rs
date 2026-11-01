//! Shell process abstraction.
//!
//! The real backend is ConPTY (spec §7, §32 Step 5); it is not wired up
//! yet. This module fixes the shape of the API the ConPTY prototype will
//! implement: spawn, write input, read output, resize, close.

/// Lifecycle state of a shell process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellState {
    /// No process has been spawned for this pane.
    Inactive,
    /// Spawn requested; the ConPTY hand-shake is in progress.
    Spawning,
    /// Process is running and attached.
    Running,
    /// Process exited with the given code (None when unknown).
    Exited(Option<i32>),
}

/// A shell process behind the pane.
///
/// Stub: holds state only. The ConPTY prototype replaces the internals
/// while keeping these method names.
#[derive(Debug)]
pub struct Shell {
    state: ShellState,
}

impl Shell {
    /// Create an inactive shell (nothing spawned yet).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ShellState::Inactive,
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ShellState {
        self.state
    }

    /// Request spawn of the default shell via ConPTY.
    ///
    /// Stub: records [`ShellState::Spawning`]; the ConPTY prototype will
    /// perform the real spawn.
    pub fn spawn(&mut self) {
        if self.state == ShellState::Inactive {
            self.state = ShellState::Spawning;
        }
    }

    /// Forward keyboard input bytes to the shell.
    ///
    /// Stub: drops the bytes until the ConPTY prototype lands.
    pub fn write_input(&mut self, _bytes: &[u8]) {}

    /// Tell the shell its window size changed (ConPTY resize).
    ///
    /// Stub: no-op until the ConPTY prototype lands.
    pub fn resize(&mut self, _cols: usize, _rows: usize) {}

    /// Close the shell process.
    pub fn close(&mut self) {
        self.state = ShellState::Exited(None);
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_moves_inactive_to_spawning() {
        let mut shell = Shell::new();
        assert_eq!(shell.state(), ShellState::Inactive);
        shell.spawn();
        assert_eq!(shell.state(), ShellState::Spawning);
        // A second spawn must not change a running/spawning state.
        shell.spawn();
        assert_eq!(shell.state(), ShellState::Spawning);
    }

    #[test]
    fn close_records_exit() {
        let mut shell = Shell::new();
        shell.close();
        assert_eq!(shell.state(), ShellState::Exited(None));
    }
}
