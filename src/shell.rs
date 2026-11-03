//! Shell process abstraction (spec §17, TTY-001..TTY-026).
//!
//! On Windows the backend is ConPTY (via the [`crate::conpty`] FFI
//! boundary); a dedicated reader thread pumps output chunks and the exit
//! code into a channel the UI polls. Off Windows the type compiles as an
//! inert stub so `cargo test` stays cross-platform.

use std::collections::BTreeMap;
#[cfg(windows)]
use std::collections::VecDeque;
use std::fmt;
use std::io;
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(windows)]
use std::sync::mpsc::{self, Receiver, Sender};

#[cfg(windows)]
use crate::config::Config;
#[cfg(windows)]
use crate::conpty;

/// GHSA-v5c6-5f42-9xqj: bytes of shell output a single pane may have in
/// flight (channel + UI backlog). The reader thread parks at the cap,
/// which back-pressures ConPTY and the shell producer; the terminal
/// stream stays ordered and lossless.
#[cfg(windows)]
const MAX_QUEUED_BYTES: usize = 4 << 20;

/// GHSA-v5c6-5f42-9xqj: byte budget one [`Shell::poll_events`] call
/// delivers, so one flooding tab bounds its own work per UI tick.
#[cfg(windows)]
const DRAIN_BUDGET_BYTES: usize = 1 << 20;

/// Bytes of terminal output carried by an event (notices carry none).
#[must_use]
pub const fn event_bytes(event: &ShellEvent) -> usize {
    match event {
        ShellEvent::Chunks(chunk) => chunk.len(),
        ShellEvent::Exited(_)
        | ShellEvent::Resize(..)
        | ShellEvent::ControlChanged(_)
        | ShellEvent::Notice(_) => 0,
    }
}

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
    /// A session view's host session was resized by its controller
    /// (issue #49): fit the grid to this geometry, never push back.
    Resize(u16, u16),
    /// A session view gained or lost controller rights (issue #49).
    ControlChanged(bool),
    /// A one-line human notice the pane shows inline (issue #49).
    Notice(&'static str),
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

    /// SHELLINT-013/014 (issue #38): when `shell-integration` is on, launch
    /// known shells (Git Bash, WSL bash, PowerShell) with the embedded
    /// rustty integration script. Unknown shells, and shells whose args
    /// already reference an rcfile/profile, are left untouched.
    pub fn apply_shell_integration(&mut self) {
        if self
            .args
            .iter()
            .any(|a| a.contains("rcfile") || a.contains("rustty"))
        {
            return;
        }
        let base = self
            .program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let dir = crate::config::config_path().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let Some(dir) = dir else { return };
        if base == "bash" || base == "bash.exe" {
            // Git Bash / MSYS2: source the script as the rcfile.
            let Some(path) = write_integration_script(
                &dir,
                "rustty-bash-rc.sh",
                include_str!("../assets/rustty-bash-rc.sh"),
            ) else {
                return;
            };
            self.args.push("--rcfile".to_string());
            self.args.push(path.to_string_lossy().into_owned());
        } else if base == "wsl" || base == "wsl.exe" {
            let Some(path) = write_integration_script(
                &dir,
                "rustty-bash-rc.sh",
                include_str!("../assets/rustty-bash-rc.sh"),
            ) else {
                return;
            };
            let Some(unix) = wsl_path(&path) else { return };
            self.args.extend([
                "--".to_string(),
                "bash".to_string(),
                "--rcfile".to_string(),
                unix,
            ]);
        } else if base.starts_with("pwsh") || base.starts_with("powershell") {
            let Some(path) = write_integration_script(
                &dir,
                "rustty-profile.ps1",
                include_str!("../assets/rustty-profile.ps1"),
            ) else {
                return;
            };
            self.args.extend([
                "-NoExit".to_string(),
                "-Command".to_string(),
                format!(". '{}'", path.to_string_lossy()),
            ]);
        }
    }
}

/// Write an embedded integration script into the config directory (kept
/// fresh on every spawn). `None` when the directory is not writable.
fn write_integration_script(
    dir: &std::path::Path,
    name: &str,
    body: &str,
) -> Option<std::path::PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, body).ok()?;
    Some(path)
}

/// `C:\dir\file` as the WSL mount path `/mnt/c/dir/file`.
fn wsl_path(path: &std::path::Path) -> Option<String> {
    let s = path.to_string_lossy().replace('\\', "/");
    let drive = s.as_bytes().first()?.to_ascii_lowercase();
    (s.as_bytes().get(1) == Some(&b':')).then(|| format!("/mnt/{}{}", drive as char, &s[2..]))
}

/// The quoting family of a shell program (issue #40: file drag/drop quotes
/// for the shell that will receive the text, not for Windows in general).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    /// powershell.exe / pwsh.exe
    PowerShell,
    /// cmd.exe
    Cmd,
    /// bash.exe (Git Bash, MSYS2, Cygwin)
    Bash,
    /// wsl.exe — the path must be mount-converted first
    Wsl,
    /// Anything else: paste the path unquoted.
    Other,
}

/// Classify a program by basename (same basenames `apply_shell_integration`
/// matches).
#[must_use]
pub fn shell_family(program: &str) -> ShellFamily {
    let base = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if base.starts_with("pwsh") || base.starts_with("powershell") {
        ShellFamily::PowerShell
    } else if base == "cmd" || base == "cmd.exe" {
        ShellFamily::Cmd
    } else if base == "bash" || base == "bash.exe" || base == "sh" || base == "sh.exe" {
        ShellFamily::Bash
    } else if base == "wsl" || base == "wsl.exe" {
        ShellFamily::Wsl
    } else {
        ShellFamily::Other
    }
}

/// Quote `path` for a drop into the shell `family`: a leading space (so the
/// cursor position is kept clear of the text) plus the shell-quoted path.
/// WSL gets the `/mnt/...` mount form when the path has a drive letter; UNC
/// and other paths paste as-is (WSL cannot mount them per-drop).
#[must_use]
pub fn quote_for_drop(family: ShellFamily, path: &str) -> String {
    let quoted = match family {
        ShellFamily::PowerShell => format!("'{}'", path.replace('\'', "''")),
        ShellFamily::Cmd => format!("\"{path}\""),
        ShellFamily::Bash => format!("'{}'", path.replace('\'', r"'\''")),
        ShellFamily::Wsl => {
            let unix = wsl_path(std::path::Path::new(path)).unwrap_or_else(|| path.to_string());
            format!("'{}'", unix.replace('\'', r"'\''"))
        }
        ShellFamily::Other => path.to_string(),
    };
    format!(" {quoted}")
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

#[cfg(windows)]
struct Inner {
    conpty: conpty::ConPty,
    /// Our half of the event channel. The reader thread holds a clone;
    /// keeping this sender alive is what makes [`Shell::poll_events`] see
    /// the queue as open while the shell runs.
    _tx: Sender<ShellEvent>,
}

/// A pane that renders a host session instead of a local shell
/// (issue #49): the pump thread owns the connection's read side and
/// forwards frames as [`ShellEvent`]s; input/resize go back as RPC
/// frames through `frames` only while `controller` is true.
#[cfg(windows)]
struct ViewState {
    alias: u32,
    /// True while this pane holds controller rights (shared with the
    /// pump, which clears it on a host demotion notice).
    controller: Arc<AtomicBool>,
    /// UI -> pump "please take control now" pings.
    /// UI -> writer-thread RPC frames (input, resize, takeover, resync).
    frame_tx: Sender<serde_json::Value>,
    /// The pump's read-handle cancel target, so `close` can wake it.
    read_handle: windows::Win32::Foundation::HANDLE,
}

/// Fallback shell probe (window milestone): Git Bash, then PowerShell 7,
/// then Windows PowerShell. Shared by the GUI's first tab and the session
/// host's default profile (issue #46). MSYS shells get `enable_pcon` in
/// the per-spawn environment; they only attach to a ConPTY with it.
#[cfg(windows)]
#[must_use]
pub fn probe_shell(cfg: &Config) -> SpawnOptions {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let (program, args) = if std::path::Path::new(r"C:\Program Files\Git\bin\bash.exe").exists() {
        (
            String::from(r"C:\Program Files\Git\bin\bash.exe"),
            vec![String::from("-l")],
        )
    } else if std::path::Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe").exists() {
        (
            String::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Vec::new(),
        )
    } else {
        let powershell = std::path::Path::new(&sys)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        (powershell.to_string_lossy().into_owned(), Vec::new())
    };
    let mut opts = SpawnOptions {
        program,
        args,
        cols: cfg.window.cols,
        rows: cfg.window.rows,
        cwd: None,
        env: std::collections::BTreeMap::new(),
    };
    if opts.program.contains("\\Git\\") {
        opts.env.insert("MSYS".into(), "enable_pcon".into());
        opts.env.insert("MSYS2".into(), "enable_pcon".into());
    }
    if cfg.shell_integration {
        opts.apply_shell_integration();
    }
    opts
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
    /// Bytes produced by the reader thread and not yet delivered to the
    /// UI (channel + backlog); the reader parks at [`MAX_QUEUED_BYTES`].
    #[cfg(windows)]
    queued: Arc<AtomicUsize>,
    /// Set on close so a parked reader thread drains and discards, never
    /// blocking the ConPTY shutdown path (GHSA-v5c6 acceptance).
    #[cfg(windows)]
    stopping: Arc<AtomicBool>,
    /// Events received beyond the per-tick drain budget, delivered first
    /// on the next poll so ordering is preserved (GHSA-v5c6).
    #[cfg(windows)]
    backlog: VecDeque<ShellEvent>,
    /// Some for a session-view pane (issue #49).
    #[cfg(windows)]
    view: Option<ViewState>,
    /// A session view is active (kept cross-platform for `is_view`).
    view_notice: bool,
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
    pub fn new() -> Self {
        Self {
            state: ShellState::Inactive,
            #[cfg(windows)]
            inner: None,
            #[cfg(windows)]
            rx: None,
            #[cfg(windows)]
            reader: None,
            #[cfg(windows)]
            queued: Arc::new(AtomicUsize::new(0)),
            #[cfg(windows)]
            stopping: Arc::new(AtomicBool::new(false)),
            #[cfg(windows)]
            backlog: VecDeque::new(),
            #[cfg(windows)]
            view: None,
            view_notice: false,
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
        // Re-spawn after close: fresh queue accounting (GHSA-v5c6).
        self.queued.store(0, Ordering::Relaxed);
        self.stopping.store(false, Ordering::Relaxed);
        self.backlog.clear();
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
            None,
        )
        .map_err(spawn_err)?;
        // The primary thread handle is not needed after spawn (WIN-001:
        // no handle leaks); dropping the SendHandle closes it.
        drop(thread);
        let reader_handle = conpty.output_reader().map_err(spawn_err)?;

        let (tx, rx) = mpsc::channel::<ShellEvent>();
        let pump_tx = tx.clone();
        let pump_queued = Arc::clone(&self.queued);
        let pump_stopping = Arc::clone(&self.stopping);
        // Terminal queries (CSI 6n, CSI c) are answered by the pane: the
        // parser emits ParserEvent::Respond with the real cursor position
        // (issue #21: replying here too produced duplicate, conflicting
        // CPR reports, and the chunk-local counter missed split queries).
        // Shells block at startup until a CPR arrives; the pane's reply is
        // what unblocks them (window-milestone liveness requirement).
        let reader = std::thread::spawn(move || {
            // Pump output until EOF/broken pipe; a hard failure is treated
            // like EOF so the pane always converges to Exited (TTY-026:
            // ConPTY failure must not crash the UI).
            let _ = conpty::read_output_loop(&reader_handle, |chunk| {
                // GHSA-v5c6-5f42-9xqj: park at the byte cap so ConPTY
                // back-pressures the producer; the stream stays ordered
                // and lossless. While stopping, drain the pipe and
                // discard, so ConPTY shutdown is never blocked by a full
                // queue.
                while !pump_stopping.load(Ordering::Relaxed)
                    && pump_queued.load(Ordering::Relaxed) + chunk.len() > MAX_QUEUED_BYTES
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if pump_stopping.load(Ordering::Relaxed) {
                    return;
                }
                pump_queued.fetch_add(chunk.len(), Ordering::Relaxed);
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

    /// Attach this pane as a *view* of live host session `alias`
    /// (issue #49): subscribe to its stream, render the snapshot, keep
    /// the snapshot current through resyncs, and — when `take_control`
    /// — hold (or demand) controller rights. Closing the view detaches;
    /// the host session keeps running.
    ///
    /// # Errors
    /// No host, attach refusals, thread setup failures.
    #[cfg(windows)]
    pub fn attach_view(&mut self, alias: u32, take_control: bool) -> Result<(), String> {
        if self.view.is_some() {
            return Err("this pane is already a session view".to_string());
        }
        if self.inner.is_some() {
            return Err("this pane already has a shell".to_string());
        }
        let mut client =
            crate::host::client::Client::connect_existing().map_err(|e| e.to_string())?;
        let resp = client
            .call(
                "session.attach",
                serde_json::json!({
                    "alias": alias,
                    "readOnly": !take_control,
                    "takeControl": take_control,
                }),
            )
            .map_err(|e| e.to_string())?;
        if let Some(err) = resp.get("error") {
            return Err(format!(
                "{}: {}",
                err.get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("error"),
                err.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            ));
        }
        let granted = resp
            .get("result")
            .and_then(|r| r.get("controlGranted"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let reader = client.reader().map_err(|e| e.to_string())?;
        let mut writer = client.writer().map_err(|e| e.to_string())?;
        // Keep the host connection open until the Shell drops (close
        // detaches; the host releases our control slot then).
        drop(client);
        let read_handle = reader.handle();

        let (tx, rx) = mpsc::channel::<ShellEvent>();
        // The attach answer paints the canvas before live frames flow.
        let result = resp.get("result");
        if let Some(snap) = result
            .and_then(|r| r.get("snapshot"))
            .filter(|v| v.is_object())
        {
            let bytes = crate::attach::snapshot_ansi(snap);
            self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
            let _ = tx.send(ShellEvent::Chunks(bytes));
        }
        let rev = result
            .and_then(|r| r.get("revision"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        for d in result
            .and_then(|r| r.get("deltas"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let hex = d.get(1).and_then(serde_json::Value::as_str).unwrap_or("");
            if let Ok(bytes) = view_unhex(hex) {
                self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
                let _ = tx.send(ShellEvent::Chunks(bytes));
            }
        }

        let (frame_tx, frame_rx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            while let Ok(frame) = frame_rx.recv() {
                if crate::host::write_frame(&mut writer, &frame).is_err() {
                    break; // the write side is gone; the pump ends next
                }
            }
        });

        let control = Arc::new(AtomicBool::new(granted));
        let pump_control = Arc::clone(&control);
        let pump_queued = Arc::clone(&self.queued);
        let pump_stopping = Arc::clone(&self.stopping);
        let pump_frame_tx = frame_tx.clone();
        let reader = std::thread::spawn(move || {
            let mut reader = reader;
            view_pump(
                &mut reader,
                &tx,
                pump_frame_tx,
                alias,
                rev,
                &pump_control,
                &pump_queued,
                &pump_stopping,
            );
            // Connection over (or the pane closed): the session is gone
            // for this pane; the UI converges on Exited.
            let _ = tx.send(ShellEvent::Exited(None));
        });

        self.view = Some(ViewState {
            alias,
            controller: control,
            frame_tx,
            read_handle,
        });
        self.view_notice = true;
        self.state = ShellState::Running;
        self.rx = Some(rx);
        self.reader = Some(reader);
        Ok(())
    }

    /// Session views need Windows (the host is Windows-only).
    #[cfg(not(windows))]
    pub fn attach_view(&mut self, _alias: u32, _take_control: bool) -> Result<(), String> {
        Err("session views require Windows".to_string())
    }

    /// Whether this pane renders a host session view (issue #49).
    #[must_use]
    pub const fn is_view(&self) -> bool {
        self.view_notice
    }

    /// Host alias of this pane's session view, if any.
    #[must_use]
    pub fn view_alias(&self) -> Option<u32> {
        #[cfg(windows)]
        return self.view.as_ref().map(|v| v.alias);
        #[cfg(not(windows))]
        return None;
    }

    /// Whether this pane's view currently holds controller rights.
    #[must_use]
    pub fn view_controller(&self) -> bool {
        #[cfg(windows)]
        return self
            .view
            .as_ref()
            .is_some_and(|v| v.controller.load(Ordering::Relaxed));
        #[cfg(not(windows))]
        return false;
    }

    /// Ask the pump to take controller rights now (issue #49); the
    /// answer arrives as a `ControlChanged` event. Send the RPC frame
    /// directly so a click is not queued behind a blocked output read.
    pub fn take_control(&self) {
        #[cfg(windows)]
        if let Some(v) = &self.view {
            let _ = v.frame_tx.send(serde_json::json!({
                "jsonrpc": "2.0", "id": "takeover", "method": "session.attach",
                "params": {"alias": v.alias, "readOnly": false, "takeControl": true},
            }));
        }
    }

    /// Write keyboard/paste bytes to the shell (TTY-004..TTY-017). A
    /// session view sends them to the host only while it holds
    /// controller rights (issue #49).
    #[cfg(windows)]
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), ShellError> {
        if let Some(v) = &self.view {
            if !v.controller.load(Ordering::Relaxed) {
                return Err(ShellError::NotRunning);
            }
            let mut data = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                data.push_str(&format!("{b:02x}"));
            }
            let frame = serde_json::json!({
                "jsonrpc": "2.0", "method": "session.input",
                "params": {"alias": v.alias, "data": data},
            });
            // shortcut: unbounded frame queue; a dead writer ends the
            // pump, upgrade to a bounded queue if input floods a dead pipe.
            return v.frame_tx.send(frame).map_err(|_| ShellError::NotRunning);
        }
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

    /// Resize the pseudo-console (TTY-003: resize must be delivered). A
    /// session view pushes its size to the host only while it holds
    /// controller rights; a watcher's grid follows the host's geometry
    /// instead (issue #49, pane-level gating).
    #[cfg(windows)]
    pub fn resize(&self, cols: usize, rows: usize) -> Result<(), ShellError> {
        if let Some(v) = &self.view {
            if !v.controller.load(Ordering::Relaxed) {
                return Err(ShellError::NotRunning);
            }
            let frame = serde_json::json!({
                "jsonrpc": "2.0", "method": "session.resize",
                "params": {"alias": v.alias, "cols": cols, "rows": rows},
            });
            return v.frame_tx.send(frame).map_err(|_| ShellError::NotRunning);
        }
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
    /// Drain shell events, honoring the per-tick byte budget
    /// ([`DRAIN_BUDGET_BYTES`]); the remainder waits in an ordered
    /// backlog (GHSA-v5c6: one flooding tab bounds its own UI work).
    #[cfg(windows)]
    pub fn poll_events(&mut self) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        let mut delivered = 0usize;
        let mut budget_open = true;
        let mut backlog = std::mem::take(&mut self.backlog);
        let Some(rx) = &self.rx else {
            self.backlog = backlog;
            return events;
        };
        loop {
            let event = match backlog.pop_front() {
                Some(ev) => ev,
                None => match rx.try_recv() {
                    Ok(ev) => ev,
                    Err(_) => break,
                },
            };
            let n = event_bytes(&event);
            self.queued.fetch_sub(n, Ordering::Relaxed);
            if budget_open && delivered + n <= DRAIN_BUDGET_BYTES {
                delivered += n;
                if let ShellEvent::Exited(code) = &event {
                    self.state = ShellState::Exited(*code);
                }
                events.push(event);
            } else {
                budget_open = false;
                backlog.push_back(event);
            }
        }
        self.backlog = backlog;
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
    #[allow(unsafe_code, reason = "FFI boundary (NFR-401): CancelIoEx")]
    pub fn close(&mut self) {
        // GHSA-v5c6: a reader parked at the byte cap must wake and drain
        // so the ConPTY shutdown path is never blocked by a full queue.
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(inner) = &mut self.inner {
            inner.conpty.close();
        }
        if let Some(v) = &self.view {
            // Wake the pump's blocking read; it ends, and dropping the
            // Shell closes the connection (the host detaches us).
            // Safety: FFI boundary (NFR-401); the handle outlives this call.
            unsafe {
                let _ = windows::Win32::System::IO::CancelIoEx(v.read_handle, None);
            }
        }
    }

    /// Close; inert on non-Windows.
    #[cfg(not(windows))]
    pub fn close(&mut self) {}
}

/// Decode host hex payloads (issue #49 view pump).
///
/// # Errors
/// Odd length, non-hex digits, or a length that does not fit `usize`.
#[cfg(windows)]
fn view_unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".to_string());
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16).ok_or("not hex")?;
        let lo = (pair[1] as char).to_digit(16).ok_or("not hex")?;
        out.push(u8::try_from(hi * 16 + lo).map_err(|_| "not hex".to_string())?);
    }
    Ok(out)
}

/// Send an output chunk to the UI, counting it against the pane's byte
/// budget; a saturated channel drops the frame and the pump resyncs.
#[cfg(windows)]
fn view_send_chunks(tx: &Sender<ShellEvent>, queued: &Arc<AtomicUsize>, bytes: Vec<u8>) {
    queued.fetch_add(bytes.len(), Ordering::Relaxed);
    let _ = tx.send(ShellEvent::Chunks(bytes));
}

/// Send a host snapshot + retained deltas to the UI and return the
/// revision they install (issue #49 view pump resync path).
#[cfg(windows)]
fn view_install_snapshot(
    result: &serde_json::Value,
    tx: &Sender<ShellEvent>,
    queued: &Arc<AtomicUsize>,
) -> u64 {
    let rev = result
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if let Some(snap) = result.get("snapshot").filter(|v| v.is_object()) {
        view_send_chunks(tx, queued, crate::attach::snapshot_ansi(snap));
    }
    for d in result
        .get("deltas")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let hex = d.get(1).and_then(serde_json::Value::as_str).unwrap_or("");
        if let Ok(bytes) = view_unhex(hex) {
            view_send_chunks(tx, queued, bytes);
        }
    }
    rev
}

/// The session-view pump (issue #49): read host frames and forward them
/// as events; on a revision gap request a snapshot resync over the same
/// connection; drain takeover requests from the UI. Ends on EOF, read
/// error, `session.exit`, or a cancelled read (pane close).
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::while_let_loop,
    reason = "the pump is one state machine over shared view state"
)]
#[cfg(windows)]
fn view_pump(
    rd: &mut crate::host::client::DupReader,
    tx: &Sender<ShellEvent>,
    frame_tx: Sender<serde_json::Value>,
    alias: u32,
    mut last_rev: u64,
    control: &Arc<AtomicBool>,
    queued: &Arc<AtomicUsize>,
    stopping: &Arc<AtomicBool>,
) {
    let mut resyncing = false;
    let mut pending: Vec<serde_json::Value> = Vec::new();
    loop {
        let frame = match crate::host::read_frame(rd, crate::host::MAX_PAGE_BYTES) {
            Ok(f) => f,
            Err(_) => break,
        };
        let method = frame.get("method").and_then(serde_json::Value::as_str);
        let params = frame.get("params");
        let rev = params
            .and_then(|p| p.get("revision"))
            .and_then(serde_json::Value::as_u64);
        match method {
            Some("session.output") | Some("session.resize") => {
                if resyncing {
                    pending.push(frame);
                    continue;
                }
                let Some(rev) = rev else { continue };
                if rev <= last_rev {
                    continue;
                }
                if rev > last_rev + 1 {
                    pending.push(frame);
                    let _ = frame_tx.send(serde_json::json!({
                        "jsonrpc": "2.0", "id": "resync", "method": "session.attach",
                        "params": {"alias": alias, "readOnly": true, "fromRevision": last_rev},
                    }));
                    resyncing = true;
                    continue;
                }
                last_rev = rev;
                if method == Some("session.output") {
                    let hex = params
                        .and_then(|p| p.get("data"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if let Ok(bytes) = view_unhex(hex) {
                        view_send_chunks(tx, queued, bytes);
                    }
                } else {
                    let cols = params
                        .and_then(|p| p.get("cols"))
                        .and_then(serde_json::Value::as_u64);
                    let rows = params
                        .and_then(|p| p.get("rows"))
                        .and_then(serde_json::Value::as_u64);
                    if let (Some(c), Some(r)) = (cols, rows) {
                        let c = u16::try_from(c).unwrap_or(0);
                        let r = u16::try_from(r).unwrap_or(0);
                        if c != 0 && r != 0 {
                            let _ = tx.send(ShellEvent::Resize(c, r));
                        }
                    }
                }
            }
            Some("session.control") => {
                if let Some(c) = params
                    .and_then(|p| p.get("controller"))
                    .and_then(serde_json::Value::as_bool)
                {
                    control.store(c, Ordering::Relaxed);
                    let _ = tx.send(ShellEvent::ControlChanged(c));
                }
            }
            Some("session.exit") => break,
            None => {
                let id = frame.get("id").and_then(serde_json::Value::as_str);
                if id == Some("resync") {
                    resyncing = false;
                    if let Some(result) = frame.get("result") {
                        let rev = view_install_snapshot(result, tx, queued);
                        last_rev = last_rev.max(rev);
                    }
                    let frames = std::mem::take(&mut pending);
                    for (idx, f) in frames.iter().enumerate() {
                        let p = f.get("params");
                        let Some(r) = p
                            .and_then(|p| p.get("revision"))
                            .and_then(serde_json::Value::as_u64)
                        else {
                            continue;
                        };
                        if r <= last_rev {
                            continue;
                        }
                        if r > last_rev + 1 {
                            for g in &frames[idx..] {
                                pending.push(g.clone());
                            }
                            let _ = frame_tx.send(serde_json::json!({
                                "jsonrpc": "2.0", "id": "resync", "method": "session.attach",
                                "params": {"alias": alias, "readOnly": true, "fromRevision": last_rev},
                            }));
                            resyncing = true;
                            break;
                        }
                        last_rev = r;
                        let m = f.get("method").and_then(serde_json::Value::as_str);
                        if m == Some("session.output") {
                            let hex = f
                                .get("params")
                                .and_then(|p| p.get("data"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("");
                            if let Ok(bytes) = view_unhex(hex) {
                                view_send_chunks(tx, queued, bytes);
                            }
                        } else if m == Some("session.resize") {
                            let cols = f
                                .get("params")
                                .and_then(|p| p.get("cols"))
                                .and_then(serde_json::Value::as_u64);
                            let rows = f
                                .get("params")
                                .and_then(|p| p.get("rows"))
                                .and_then(serde_json::Value::as_u64);
                            if let (Some(c), Some(r)) = (cols, rows) {
                                let c = u16::try_from(c).unwrap_or(0);
                                let r = u16::try_from(r).unwrap_or(0);
                                if c != 0 && r != 0 {
                                    let _ = tx.send(ShellEvent::Resize(c, r));
                                }
                            }
                        }
                    }
                } else if id == Some("takeover") {
                    let granted = frame
                        .get("result")
                        .and_then(|r| r.get("controlGranted"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    control.store(granted, Ordering::Relaxed);
                    let _ = tx.send(ShellEvent::ControlChanged(granted));
                }
            }
            _ => {}
        }
    }
    if !stopping.load(Ordering::Relaxed) {
        let _ = tx.send(ShellEvent::Notice("host connection lost"));
    }
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
    fn event_bytes_counts_only_output_chunks() {
        // GHSA-v5c6: the queue cap and the drain budget are byte-based.
        assert_eq!(event_bytes(&ShellEvent::Chunks(b"hello".to_vec())), 5);
        assert_eq!(event_bytes(&ShellEvent::Chunks(Vec::new())), 0);
        assert_eq!(event_bytes(&ShellEvent::Exited(Some(0))), 0);
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

    #[test]
    fn family_detection_by_program_basename() {
        assert_eq!(shell_family(r"C:\ps\pwsh.exe"), ShellFamily::PowerShell);
        assert_eq!(shell_family("powershell.exe"), ShellFamily::PowerShell);
        assert_eq!(shell_family("cmd.exe"), ShellFamily::Cmd);
        assert_eq!(
            shell_family(r"C:\Program Files\Git\bin\bash.exe"),
            ShellFamily::Bash
        );
        assert_eq!(shell_family("wsl.exe"), ShellFamily::Wsl);
        assert_eq!(shell_family("nvim"), ShellFamily::Other);
    }

    #[test]
    fn drop_quoting_matches_each_shell_family() {
        // Issue #40 acceptance: Git Bash, PowerShell, CMD and WSL quoting.
        assert_eq!(
            quote_for_drop(ShellFamily::PowerShell, r"C:\My Docs\it's.txt"),
            r" 'C:\My Docs\it''s.txt'"
        );
        assert_eq!(
            quote_for_drop(ShellFamily::Cmd, r"C:\My Docs\file.txt"),
            r#" "C:\My Docs\file.txt""#
        );
        assert_eq!(
            quote_for_drop(ShellFamily::Bash, "/home/me/it's.txt"),
            r" '/home/me/it'\''s.txt'"
        );
        assert_eq!(
            quote_for_drop(ShellFamily::Wsl, r"C:\Projects\App"),
            " '/mnt/c/Projects/App'"
        );
        // UNC has no mount form; WSL pastes it as-is.
        assert_eq!(
            quote_for_drop(ShellFamily::Wsl, r"\\server\share\file.txt"),
            r" '\\server\share\file.txt'"
        );
        assert_eq!(
            quote_for_drop(ShellFamily::Other, r"C:\x y\file.txt"),
            r" C:\x y\file.txt"
        );
    }
}
