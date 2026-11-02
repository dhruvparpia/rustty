//! Per-command metadata built from OSC 133 prompt/command transitions
//! (SHELLINT-006/010/013-019, roadmap Stage 2 / issue #38).
//!
//! A shell integration script wraps each command in `OSC 133;A` (prompt),
//! `OSC 133;B` (command accepted), `OSC 133;C` (first output) and
//! `OSC 133;D;<exit>` (finished). This module turns those transitions, plus
//! the grid's monotonic absolute line ids, into bounded records of every
//! command: where its block and output begin and end, its working
//! directory, start/end time and exit status. It drives previous/next
//! command navigation, copy/export of one command's output, duration and
//! success display, and background completion notifications.
//!
//! Recording is opt-in (SHELLINT-013/014): a pane only feeds this tracker
//! when shell integration is enabled. Shells that emit no OSC 133 simply
//! leave it empty.

use std::collections::VecDeque;

/// Default bound on retained command records; the oldest are dropped once
/// full, mirroring how the grid evicts scrollback.
pub const DEFAULT_CAP: usize = 512;

/// One command, from `OSC 133;B` to the matching `OSC 133;D`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Monotonic id, stable for the life of the pane.
    pub id: u64,
    /// Working directory the shell reported when the command started.
    pub cwd: Option<String>,
    /// Absolute grid line where the command's block begins (its prompt).
    pub start_abs: usize,
    /// Absolute grid line of the first output line, once `133;C` is seen.
    pub output_start_abs: usize,
    /// Absolute grid line where the command ended (`133;D`), `None` while
    /// it is still running.
    pub end_abs: Option<usize>,
    /// Unix milliseconds at start.
    pub started_at_ms: u64,
    /// Unix milliseconds at end, `None` while running.
    pub ended_at_ms: Option<u64>,
    /// Exit code from `OSC 133;D;<code>`, `None` when unknown.
    pub exit: Option<i32>,
    /// True from `133;B` until the matching `133;D` (or interruption).
    pub running: bool,
}

impl Command {
    /// Wall-clock duration in milliseconds once finished, else `None`.
    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.ended_at_ms
            .map(|end| end.saturating_sub(self.started_at_ms))
    }

    /// `Some(true)` for a clean exit, `Some(false)` for a non-zero exit,
    /// `None` while running or when the shell reported no code.
    #[must_use]
    pub fn success(&self) -> Option<bool> {
        self.exit.map(|code| code == 0)
    }
}

/// Bounded, ordered history of tracked commands for one pane.
#[derive(Debug, Clone)]
pub struct CommandTracker {
    cap: usize,
    seq: u64,
    ring: VecDeque<Command>,
}

impl Default for CommandTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandTracker {
    /// A tracker keeping up to [`DEFAULT_CAP`] records.
    #[must_use]
    pub fn new() -> Self {
        Self::with_cap(DEFAULT_CAP)
    }

    /// A tracker keeping up to `cap` records (at least one).
    #[must_use]
    pub fn with_cap(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            seq: 0,
            ring: VecDeque::new(),
        }
    }

    /// Current unix time in milliseconds (0 when the clock is before 1970).
    #[must_use]
    pub fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// `OSC 133;A`: the shell is at a prompt. A command left open (a shell
    /// that emitted `133;B`/`C` but no `D`, e.g. an interrupted one) is
    /// closed as unknown so its output is not attributed to the next.
    pub fn prompt_start(&mut self, abs: usize, at_ms: u64) {
        if let Some(open) = self.ring.back_mut().filter(|c| c.running) {
            open.running = false;
            open.end_abs = Some(abs);
            open.ended_at_ms = Some(at_ms);
        }
    }

    /// `OSC 133;B`: a command was accepted and starts running. Opens a new
    /// record; its block starts at `abs` (the prompt line it was typed on).
    pub fn command_start(&mut self, abs: usize, cwd: Option<String>, at_ms: u64) {
        // A stray `133;B` without a prior `D` closes the previous open one.
        self.prompt_start(abs, at_ms);
        self.seq += 1;
        self.ring.push_back(Command {
            id: self.seq,
            cwd,
            start_abs: abs,
            output_start_abs: abs,
            end_abs: None,
            started_at_ms: at_ms,
            ended_at_ms: None,
            exit: None,
            running: true,
        });
        while self.ring.len() > self.cap {
            self.ring.pop_front();
        }
    }

    /// `OSC 133;C`: the running command produced its first output line.
    /// Stamps the output start; the first `;C` wins.
    pub fn command_output(&mut self, abs: usize) {
        if let Some(open) = self
            .ring
            .back_mut()
            .filter(|c| c.running && c.end_abs.is_none() && c.output_start_abs == c.start_abs)
        {
            open.output_start_abs = abs;
        }
    }

    /// `OSC 133;D;<code>`: the running command finished with `exit`.
    pub fn prompt_end(&mut self, abs: usize, exit: Option<i32>, at_ms: u64) {
        if let Some(open) = self.ring.back_mut().filter(|c| c.running) {
            open.running = false;
            open.end_abs = Some(abs);
            open.ended_at_ms = Some(at_ms);
            open.exit = exit;
        }
    }

    /// Close every running record as interrupted: used when the pane
    /// enters or leaves the alternate screen, where the running command's
    /// output belongs to the alt app, not to the scrollback block.
    pub fn interrupt_running(&mut self, abs: usize, at_ms: u64) {
        for cmd in &mut self.ring {
            if cmd.running {
                cmd.running = false;
                cmd.end_abs = Some(abs);
                cmd.ended_at_ms = Some(at_ms);
                cmd.exit = None;
            }
        }
    }

    /// Drop records whose whole block has scrolled out of the grid buffer.
    /// `first_abs` is the grid's oldest retained absolute line; a command
    /// that ended before it can no longer be shown, copied, or jumped to.
    pub fn note_eviction(&mut self, first_abs: usize) {
        while self
            .ring
            .front()
            .is_some_and(|c| c.end_abs.is_some_and(|end| end < first_abs))
        {
            self.ring.pop_front();
        }
    }

    /// All records, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.ring.iter()
    }

    /// Number of retained records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// True when no command has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// The running command, if any.
    #[must_use]
    pub fn running(&self) -> Option<&Command> {
        self.ring.back().filter(|c| c.running)
    }

    /// The most recently finished command, if any.
    #[must_use]
    pub fn last_finished(&self) -> Option<&Command> {
        self.ring.iter().rev().find(|c| !c.running)
    }

    /// Block-start line of the command just above absolute line `from`
    /// (previous-command navigation). `None` when already at the top.
    #[must_use]
    pub fn prev_boundary(&self, from: usize) -> Option<usize> {
        self.ring
            .iter()
            .rev()
            .map(|c| c.start_abs)
            .find(|b| *b < from)
    }

    /// Block-start line of the command at or below absolute line `from`
    /// (next-command navigation).
    #[must_use]
    pub fn next_boundary(&self, from: usize) -> Option<usize> {
        self.ring.iter().map(|c| c.start_abs).find(|b| *b > from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_then_end_records_one_command() {
        let mut t = CommandTracker::new();
        t.command_start(10, Some("C:/src".into()), 1000);
        t.command_output(11);
        t.prompt_end(20, Some(0), 2500);
        assert_eq!(t.len(), 1);
        let c = t.last_finished().expect("one finished");
        assert_eq!(c.id, 1);
        assert_eq!(c.cwd.as_deref(), Some("C:/src"));
        assert_eq!(c.start_abs, 10);
        assert_eq!(c.output_start_abs, 11);
        assert_eq!(c.end_abs, Some(20));
        assert_eq!(c.duration_ms(), Some(1500));
        assert_eq!(c.success(), Some(true));
    }

    #[test]
    fn nonzero_exit_is_not_success() {
        let mut t = CommandTracker::new();
        t.command_start(0, None, 0);
        t.prompt_end(3, Some(2), 10);
        assert_eq!(t.last_finished().and_then(Command::success), Some(false));
    }

    #[test]
    fn running_has_no_duration_or_status() {
        let mut t = CommandTracker::new();
        t.command_start(0, None, 100);
        assert!(t.running().is_some());
        assert!(t.last_finished().is_none());
        assert_eq!(t.running().and_then(Command::duration_ms), None);
    }

    #[test]
    fn prompt_start_closes_an_interrupted_command_as_unknown() {
        let mut t = CommandTracker::new();
        t.command_start(0, None, 100); // B seen, no D (e.g. Ctrl-C)
        t.prompt_start(5, 300); // A seen: prompt returned
        assert!(t.running().is_none());
        let c = t.last_finished().expect("closed");
        assert_eq!(c.exit, None);
        assert_eq!(c.end_abs, Some(5));
    }

    #[test]
    fn command_output_only_stamps_the_first_output_line() {
        let mut t = CommandTracker::new();
        t.command_start(4, None, 0);
        t.command_output(5);
        t.command_output(9); // ignored
        assert_eq!(t.running().map(|c| c.output_start_abs), Some(5));
    }

    #[test]
    fn tracker_is_bounded_and_keeps_the_newest() {
        let mut t = CommandTracker::with_cap(3);
        for i in 0..10u64 {
            t.command_start(usize::try_from(i).unwrap() * 2, None, i);
            t.prompt_end(usize::try_from(i).unwrap() * 2 + 1, Some(0), i + 1);
        }
        assert_eq!(t.len(), 3);
        let ids: Vec<u64> = t.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![8, 9, 10]);
    }

    #[test]
    fn eviction_drops_commands_fully_scrolled_away() {
        let mut t = CommandTracker::new();
        t.command_start(0, None, 0);
        t.prompt_end(4, Some(0), 1); // block [0..4]
        t.command_start(10, None, 2);
        t.prompt_end(14, Some(0), 3); // block [10..14]
        t.note_eviction(5); // lines below 5 are gone
        assert_eq!(t.len(), 1);
        assert_eq!(t.iter().next().map(|c| c.start_abs), Some(10));
    }

    #[test]
    fn alt_screen_closes_running_commands_as_unattributed() {
        let mut t = CommandTracker::new();
        t.command_start(0, None, 0);
        t.interrupt_running(3, 50); // entered alt screen mid-command
        assert!(t.running().is_none());
        assert_eq!(t.last_finished().and_then(Command::success), None);
    }

    #[test]
    fn prev_and_next_boundary_walk_block_starts() {
        let mut t = CommandTracker::new();
        for start in [0, 10, 20] {
            t.command_start(start, None, 0);
            t.prompt_end(start + 5, Some(0), 1);
        }
        assert_eq!(t.prev_boundary(15), Some(10));
        assert_eq!(t.prev_boundary(0), None);
        assert_eq!(t.next_boundary(0), Some(10));
        assert_eq!(t.next_boundary(20), None);
    }

    #[test]
    fn no_integration_events_leaves_the_tracker_empty() {
        let t = CommandTracker::new();
        assert!(t.is_empty());
        assert!(t.last_finished().is_none());
        assert_eq!(t.prev_boundary(100), None);
    }
}
