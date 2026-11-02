//! Shell process abstraction (spec §17, TTY-001..TTY-026).
//!
//! On Windows the backend is ConPTY (via the [`crate::conpty`] FFI
//! boundary); a dedicated reader thread pumps output chunks and the exit
//! code into a channel the UI polls. Off Windows the type compiles as an
//! inert stub so `cargo test` stays cross-platform.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
#[cfg(windows)]
use std::sync::mpsc::{self, Receiver, Sender};

#[cfg(windows)]
use crate::conpty;

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

/// Things the shell reader thread reports back to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    /// A chunk of ConPTY output bytes (feed them to the parser in order).
    Chunks(Vec<u8>),
    /// The process is gone; the code is `None` when it could not be read.
    Exited(Option<i32>),
}

/// What [`Shell::spawn`] needs to start a pane's process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnOptions {
    /// Program to run (absolute path or name on PATH).
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// Initial pseudo-console width in columns.
    pub cols: usize,
    /// Initial pseudo-console height in rows.
    pub rows: usize,
    /// Working directory for the child process (SHELL-019/020).
    pub cwd: Option<String>,
    /// Environment variables added to the inherited environment
    /// (SHELL-021).
    pub env: BTreeMap<String, String>,
}

impl SpawnOptions {
    /// Options for `program` (no args) at `cols` x `rows`.
    pub fn new(program: impl Into<String>, cols: usize, rows: usize) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cols,
            rows,
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

/// Why a shell operation failed.
#[derive(Debug)]
pub enum ShellError {
    /// ConPTY/CreateProcessW failed.
    Spawn {
        /// Program that failed to start.
        program: String,
        /// Underlying OS error.
        source: io::Error,
    },
    /// No shell is attached (spawn first).
    NotRunning,
    /// This build target has no ConPTY backend (non-Windows).
    Unsupported(&'static str),
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, source } => write!(f, "spawning {program}: {source}"),
            Self::NotRunning => write!(f, "shell is not running"),
            Self::Unsupported(what) => write!(f, "not supported on this platform: {what}"),
        }
    }
}

impl std::error::Error for ShellError {}

/// Count `CSI 6n` (cursor position query) sequences in an output chunk.
///
/// A shell that sends this query blocks until the terminal answers with a
/// CPR report; the reader thread answers each occurrence (TTY-004 path).
/// Pure byte-scan, so it is unit-tested on every platform.
#[must_use]
pub fn count_cursor_position_queries(chunk: &[u8]) -> usize {
    chunk.windows(4).filter(|w| *w == b"\x1b[6n").count()
}

#[cfg(windows)]
struct Inner {
    conpty: conpty::ConPty,
    /// Our half of the event channel. The reader thread holds a clone;
    /// keeping this sender alive is what makes [`Shell::poll_events`] see
    /// the queue as open while the shell runs.
    _tx: Sender<ShellEvent>,
}

/// A shell process behind the pane.
///
/// The UI side owns the receiver; the reader thread owns a sender clone
/// and reports output chunks plus the exit event (TTY-025).
pub struct Shell {
    state: ShellState,
    #[cfg(windows)]
    inner: Option<Inner>,
    #[cfg(windows)]
    rx: Option<Receiver<ShellEvent>>,
    #[cfg(windows)]
    reader: Option<std::thread::JoinHandle<()>>,
}

impl fmt::Debug for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut built = f.debug_struct("Shell");
        built.field("state", &self.state);
        #[cfg(windows)]
        built.field("attached", &self.inner.is_some());
        built.finish()
    }
}

impl Shell {
    /// Create an inactive shell (nothing spawned yet).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ShellState::Inactive,
            #[cfg(windows)]
            inner: None,
            #[cfg(windows)]
            rx: None,
            #[cfg(windows)]
            reader: None,
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ShellState {
        self.state
    }

    /// Whether a process is attached.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self.state, ShellState::Running)
    }

    /// Spawn the shell on a ConPTY of the requested size (TTY-001, TTY-002).
    ///
    /// A failed spawn leaves the pane inert (`Inactive`) so a later
    /// [`Tab::spawn_panes`] retry can ask again (TTY-026: no half-open
    /// session, no stuck `Spawning` state).
    #[cfg(windows)]
    pub fn spawn(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        if self.is_running() {
            return Err(ShellError::NotRunning);
        }
        self.state = ShellState::Spawning;
        match self.attach_console(opts) {
            Ok(()) => {
                self.state = ShellState::Running;
                Ok(())
            }
            Err(e) => {
                self.state = ShellState::Inactive;
                self.inner = None;
                self.rx = None;
                self.reader = None;
                Err(e)
            }
        }
    }

    /// Build the ConPTY session and its pump thread; the caller owns the
    /// `Spawning`/`Running`/`Inactive` transitions.
    #[cfg(windows)]
    fn attach_console(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        let spawn_err = |e: std::io::Error| ShellError::Spawn {
            program: opts.program.clone(),
            source: e,
        };

        let (conpty, process, thread) = conpty::ConPty::spawn(
            &opts.program,
            &opts.args,
            opts.cols,
            opts.rows,
            opts.cwd.as_deref(),
            &opts.env,
        )
        .map_err(spawn_err)?;
        // The primary thread handle is not needed after spawn (WIN-001:
        // no handle leaks); dropping the SendHandle closes it.
        drop(thread);
        let reader_handle = conpty.output_reader().map_err(spawn_err)?;

        let (tx, rx) = mpsc::channel::<ShellEvent>();
        let pump_tx = tx.clone();
        // The shell answers terminal queries (CSI 6n) inline: cmd.exe and
        // bash query the cursor position at startup and block until the
        // terminal replies with a CPR report.
        let reply_tx = conpty.input_writer().map_err(spawn_err)?;
        let reader = std::thread::spawn(move || {
            // Pump output until EOF/broken pipe; a hard failure is treated
            // like EOF so the pane always converges to Exited (TTY-026:
            // ConPTY failure must not crash the UI).
            let _ = conpty::read_output_loop(&reader_handle, |chunk| {
                let n = count_cursor_position_queries(chunk);
                for _ in 0..n {
                    // Answer "you are at row 1, col 1"; shells use this
                    // only as a "is a terminal there" probe.
                    let _ = reply_tx.write(b"\x1b[1;1R");
                }
                let _ = pump_tx.send(ShellEvent::Chunks(chunk.to_vec()));
            });
            let code = conpty::wait_exit_code(&process);
            let _ = pump_tx.send(ShellEvent::Exited(code));
        });

        self.inner = Some(Inner { conpty, _tx: tx });
        self.rx = Some(rx);
        self.reader = Some(reader);
        Ok(())
    }

    /// Spawn the shell; non-Windows builds have no ConPTY backend.
    #[cfg(not(windows))]
    pub fn spawn(&mut self, _opts: &SpawnOptions) -> Result<(), ShellError> {
        Err(ShellError::Unsupported("ConPTY requires Windows"))
    }

    /// Write keyboard/paste bytes to the shell (TTY-004..TTY-017).
    #[cfg(windows)]
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), ShellError> {
        self.inner
            .as_ref()
            .ok_or(ShellError::NotRunning)?
            .conpty
            .write_input(bytes)
            .map_err(|_| ShellError::NotRunning)
    }

    /// Write input; inert on non-Windows.
    #[cfg(not(windows))]
    pub fn write_input(&self, _bytes: &[u8]) -> Result<(), ShellError> {
        Err(ShellError::NotRunning)
    }

    /// Resize the pseudo-console (TTY-003: resize must be delivered).
    #[cfg(windows)]
    pub fn resize(&self, cols: usize, rows: usize) -> Result<(), ShellError> {
        self.inner
            .as_ref()
            .ok_or(ShellError::NotRunning)?
            .conpty
            .resize(cols, rows)
            .map_err(|_| ShellError::NotRunning)
    }

    /// Resize; inert on non-Windows.
    #[cfg(not(windows))]
    pub fn resize(&self, _cols: usize, _rows: usize) -> Result<(), ShellError> {
        Err(ShellError::NotRunning)
    }

    /// Drain every pending reader-thread event, in order.
    ///
    /// An `Exited` event moves the state to [`ShellState::Exited`].
    #[cfg(windows)]
    pub fn poll_events(&mut self) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(event) = rx.try_recv() {
                if let ShellEvent::Exited(code) = event {
                    self.state = ShellState::Exited(code);
                }
                events.push(event);
            }
        }
        events
    }

    /// No events on non-Windows (nothing is running).
    #[cfg(not(windows))]
    pub fn poll_events(&mut self) -> Vec<ShellEvent> {
        Vec::new()
    }

    /// Signal EOF and shut the shell down (TTY-024: graceful close).
    ///
    /// The exit code arrives later through [`Shell::poll_events`].
    #[cfg(windows)]
    pub fn close(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.conpty.close();
        }
    }

    /// Close; inert on non-Windows.
    #[cfg(not(windows))]
    pub fn close(&mut self) {}
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.close();
        #[cfg(windows)]
        {
            // Dropping our sender closes the channel once the reader is
            // done; join so the reader thread never outlives the Shell.
            drop(self.rx.take());
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_shell_is_inactive() {
        let shell = Shell::new();
        assert_eq!(shell.state(), ShellState::Inactive);
        assert!(!shell.is_running());
    }

    #[test]
    fn cursor_position_queries_are_counted() {
        assert_eq!(count_cursor_position_queries(b""), 0);
        assert_eq!(count_cursor_position_queries(b"plain text"), 0);
        assert_eq!(count_cursor_position_queries(b"\x1b[6n"), 1);
        assert_eq!(
            count_cursor_position_queries(b"\x1b[?1h\x1b[6ntext\x1b[6n"),
            2
        );
        // Similar-looking sequences must not count.
        assert_eq!(count_cursor_position_queries(b"\x1b[6;1R\x1b[5n"), 0);
    }

    #[test]
    fn inert_ops_report_not_running() {
        let mut shell = Shell::new();
        assert!(matches!(
            shell.write_input(b"x"),
            Err(ShellError::NotRunning)
        ));
        assert!(matches!(shell.resize(80, 24), Err(ShellError::NotRunning)));
        assert!(shell.poll_events().is_empty());
        shell.close();
        assert_eq!(shell.state(), ShellState::Inactive);
    }
}
