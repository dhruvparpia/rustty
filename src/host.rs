//! Persistent session host (issue #46, docs/design-session-host.md).
//!
//! `rustty-host.exe` owns managed ConPTY sessions per user, independently
//! of any GUI/console client: a client's death never kills a host-owned
//! shell (each session lives in its own kill-on-close job object), and
//! termination is always explicit. Transport is one local named pipe,
//! 4-byte little-endian length + UTF-8 JSON frames, carrying the
//! versioned JSON-RPC 2.0-style envelope shared with the automation ADR
//! (`docs/design-automation-api.md`): string error codes from a fixed
//! table, 64 KiB request cap, 1 MiB page cap.
//!
//! Identities: durable u64 ids plus stable numeric aliases, allocated
//! monotonically and persisted under the config directory so a closed
//! alias is never silently recycled.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

static IDS_IO_LOCK: std::sync::Mutex<u8> = std::sync::Mutex::new(0);

/// Protocol version this build speaks. Hosts support this version and
/// one step back; `initialize` negotiates `min(client, host)` (HOST-008).
pub const PROTOCOL_VERSION: u32 = 1;
/// Request frame cap per the automation ADR §3 (64 KiB).
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// One response/stream page cap per the automation ADR §3 (1 MiB).
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
/// Idle budget: zero live sessions and zero clients for this long ends
/// the host (ADR §1).
pub const IDLE_EXIT_SECS: u64 = 600;
/// Graceful-termination wait before the job-object tree kill (ADR §4).
pub const GRACEFUL_WAIT_MS: u64 = 5_000;
/// Bounded exited-session records kept for inspection
/// (docs/design-session-attach.md, "retain bounded exited-session metadata").
#[cfg(windows)]
const MAX_EXITED_RECORDS: usize = 32;

/// JSON payload schema tag: every list/info/create result carries it so
/// scripts can pin the shape ("versioned JSON", #46 acceptance).
pub const SCHEMA: &str = "host-session/v1";

/// Attach-snapshot schema tag (issue #48, docs/design-session-attach.md):
/// the canonical terminal model rendered as grid spans, never raw
/// historic PTY bytes.
pub const SNAPSHOT_SCHEMA: &str = "host-snapshot/v1";

/// Operations this host serves (clients discover, never hardcode).
pub const OPS: &[&str] = &[
    "initialize",
    "session.create",
    "session.list",
    "session.info",
    "session.terminate",
    "session.attach",
    "session.input",
    "session.resize",
    "session.detach",
    "host.shutdown",
];

// Stable CLI exit codes (docs/design-session-attach.md "Define stable CLI
// exit codes"). 0 is success.
/// Usage / malformed option.
pub const EXIT_USAGE: u8 = 2;
/// Session not found.
pub const EXIT_NOT_FOUND: u8 = 3;
/// No host reachable and one could not be spawned.
pub const EXIT_HOST_UNAVAILABLE: u8 = 5;
/// Protocol version incompatible with the live host.
pub const EXIT_PROTOCOL: u8 = 6;
/// The session is controlled by another client.
pub const EXIT_BUSY: u8 = 7;

/// Pipe name base; the full name is `\\.\pipe\rustty-host.<user>`.
/// The user suffix keeps two accounts on one machine apart; the real
/// boundary is the pipe DACL (current user only), not the name.
#[must_use]
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .map(|u| u.to_ascii_lowercase())
        .unwrap_or_else(|_| "user".to_string());
    format!(r"\\.\pipe\rustty-host.{user}")
}

/// Directory holding the persistent id/alias allocator:
/// `<config dir>/host`, portable-mode aware through [`crate::config::config_path`].
#[must_use]
pub fn host_state_dir() -> Option<PathBuf> {
    crate::config::config_path().and_then(|p| p.parent().map(|d| d.join("host")))
}

/// Persisted identity allocator state: u64 ids and numeric aliases are
/// monotonic per user and never reused, even across host restarts
/// (HOST-003 / "never bind a stale number to an unrelated live session").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdsState {
    /// Next durable u64 id to hand out.
    pub next_id: u64,
    /// Next numeric alias to hand out.
    pub next_alias: u32,
    /// alias -> durable id, kept for the process lifetime and across
    /// restarts; closed aliases stay so a stale alias answers "stale",
    /// never retargets a newer session.
    pub aliases: BTreeMap<u32, u64>,
}

impl Default for IdsState {
    /// A fresh registry: ids and aliases start at 1 (alias 0 is
    /// reserved, the CLI's "no alias given" sentinel).
    fn default() -> Self {
        Self {
            next_id: 1,
            next_alias: 1,
            aliases: BTreeMap::new(),
        }
    }
}

impl IdsState {
    /// Load from `<dir>/ids.json`; missing or unreadable file starts at
    /// id 1, alias 1. Lost registry state is not silently reset to
    /// reuse old aliases: a corrupt file is an error for the host, which
    /// refuses to start (explicit recovery, not silent recycling).
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be parsed.
    pub fn load(dir: &Path) -> io::Result<Self> {
        let binding = &IDS_IO_LOCK;
        let Ok(_guard) = binding.lock() else {
            return Err(io::Error::other("ids lock failed"));
        };
        let bytes = match std::fs::read(dir.join("ids.json")) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let tmp = dir.join("ids.json.tmp");
                let old = dir.join("ids.json.old");
                match std::fs::read(&tmp) {
                    Ok(b) => b,
                    Err(_) => match std::fs::read(&old) {
                        Ok(b) => b,
                        Err(_) => return Ok(Self::default()),
                    },
                }
            }
            Err(e) => return Err(e),
        };
        match serde_json::from_slice(&bytes) {
            Ok(store) => Ok(store),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }

    /// Persist atomically (write temp + rename) so a crash never leaves
    /// a half-written allocator.
    ///
    /// # Errors
    ///
    /// Propagates any filesystem failure; the host keeps serving with
    /// the in-memory state.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let binding = &IDS_IO_LOCK;
        let Ok(_guard) = binding.lock() else {
            return Err(io::Error::other("ids lock failed"));
        };
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("ids.json.tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, bytes)?;
        let dst = dir.join("ids.json");
        let backup = dir.join("ids.json.old");
        std::fs::remove_file(&backup).ok();
        if dst.exists() {
            std::fs::rename(&dst, &backup)?;
        }
        std::fs::rename(&tmp, dst)?;
        std::fs::remove_file(&backup).ok();
        Ok(())
    }

    /// Allocate the next (alias, durable id) pair and record the binding.
    pub fn allocate(&mut self) -> (u32, u64) {
        let alias = self.next_alias;
        let id = self.next_id;
        self.next_alias += 1;
        self.next_id += 1;
        self.aliases.insert(alias, id);
        (alias, id)
    }

    /// The durable id an alias was ever bound to, if any.
    #[must_use]
    pub fn resolve(&self, alias: u32) -> Option<u64> {
        self.aliases.get(&alias).copied()
    }
}

/// One session's metadata (the "name, profile, cwd, geometry, live/exited
/// state" the #46 acceptance names). Serialized into every payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Durable u64 id.
    pub id: u64,
    /// Stable numeric alias.
    pub alias: u32,
    /// Display name (label only, never a lookup key).
    pub name: String,
    /// Profile name, or "default" for the probe-shell profile.
    pub profile: String,
    /// Working directory the shell was spawned in.
    pub cwd: String,
    /// Program the shell runs.
    pub program: String,
    /// Columns at creation.
    pub cols: usize,
    /// Rows at creation.
    pub rows: usize,
    /// OS process id of the shell.
    pub pid: u32,
    /// Persistent sessions outlive every client; ephemeral ones end with
    /// their last client (ADR §3).
    pub persistent: bool,
    /// "live" while the shell runs, "exited" after.
    pub state: String,
    /// Creation time (unix seconds).
    pub created_unix_secs: u64,
    /// Exit code once exited.
    pub exit_code: Option<u32>,
    /// Canonical task id for task-run identity, presentation-only name otherwise.
    pub task_id: Option<u32>,
    /// Project root/namespace for the task run (issue #92).
    pub project: Option<String>,
}

impl SessionRecord {
    /// JSON shape used in payloads.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "alias": self.alias,
            "name": self.name,
            "profile": self.profile,
            "cwd": self.cwd,
            "program": self.program,
            "cols": self.cols,
            "rows": self.rows,
            "pid": self.pid,
            "persistent": self.persistent,
            "state": self.state,
            "createdUnixSecs": self.created_unix_secs,
            "exitCode": self.exit_code,
            "taskId": self.task_id,
            "project": self.project,
        })
    }
}

/// Wrap a result in the versioned envelope.
#[must_use]
pub fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Wrap a fixed-table error code in the envelope. `code` is the string
/// code (automation ADR §3); the echoed id lets a client match.
#[must_use]
pub fn error(id: Value, code: &str, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Write one frame: 4-byte little-endian length + UTF-8 JSON.
///
/// # Errors
///
/// Propagates the writer's errors.
pub fn write_frame(w: &mut impl Write, v: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if bytes.len() > MAX_PAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds page cap",
        ));
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)
}

/// Read one frame, enforcing `max` on the declared length.
///
/// # Errors
///
/// Unexpected EOF, an over-long frame, or invalid UTF-8/JSON.
pub fn read_frame(r: &mut impl Read, max: usize) -> io::Result<Value> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame over request cap",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// `WriteFile` with an event + wait, the overlapped pattern every pipe
/// handle in this project uses: a blocking `ReadFile` on a synchronous
/// handle locks the file object, and a concurrent writer on the same
/// handle (or a duplicate) is starved until the next read completes
/// (measured: the pump's response only went out when the next request
/// arrived).
///
/// # Errors
/// Transport errors.
#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
pub fn overlapped_write(
    handle: windows::Win32::Foundation::HANDLE,
    buf: &[u8],
) -> io::Result<usize> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GetLastError};
    use windows::Win32::Storage::FileSystem::WriteFile;
    use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

    if buf.is_empty() {
        return Ok(0);
    }
    // Safety: event + OVERLAPPED live through the wait; `handle` is a
    // valid pipe handle opened with FILE_FLAG_OVERLAPPED by the caller.
    unsafe {
        let Ok(event) = CreateEventW(None, true, false, None) else {
            return Err(io::Error::last_os_error());
        };
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut done = 0u32;
        let started = WriteFile(
            handle,
            Some(buf),
            Some(core::ptr::from_mut(&mut done)),
            Some(core::ptr::from_mut(&mut ov)),
        );
        let n = if started.is_ok() {
            done
        } else if GetLastError() == ERROR_IO_PENDING {
            WaitForSingleObject(event, INFINITE);
            GetOverlappedResult(handle, core::ptr::from_ref(&ov), &mut done, false)
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            done
        } else {
            let code = GetLastError();
            CloseHandle(event).ok();
            return Err(io::Error::from_raw_os_error(code.0 as i32));
        };
        CloseHandle(event).ok();
        Ok(usize::try_from(n).unwrap_or(0))
    }
}

/// The Windows session host: App state, per-session job objects, the
/// RPC dispatch table. The `rustty-host.exe` bin drives it; clients
/// speak through [`client`].
#[cfg(windows)]
pub mod server {
    use std::collections::{BTreeMap, VecDeque};
    use std::io;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0,
    };
    use windows::Win32::Security::*;
    use windows::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
    use windows::Win32::System::Threading::*;
    use windows::core::{Owned, PCWSTR};

    use crate::config::Config;
    use crate::conpty::{self, ConPty, SendHandle};
    use crate::discovery;
    use crate::grid::Grid;
    use crate::parser::{Parser, ParserEvent};
    use crate::shell::{SpawnOptions, probe_shell};

    use super::{
        GRACEFUL_WAIT_MS, IdsState, MAX_EXITED_RECORDS, OPS, PROTOCOL_VERSION, SCHEMA,
        SNAPSHOT_SCHEMA, SessionRecord, error, host_state_dir, response,
    };

    /// Bytes of raw PTY output the host keeps for a resuming client
    /// (issue #48). Hex doubles in a frame, so a resume stays under the
    /// 1 MiB page cap.
    const MAX_HISTORY_BYTES: usize = 128 * 1024;

    /// Lowercase hex, pure-ASCII JSON frames.
    fn hex_encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push(HEX[usize::from(b >> 4)] as char);
            s.push(HEX[usize::from(b & 0xf)] as char);
        }
        s
    }

    /// The host's canonical terminal model per attached session (issue
    /// #48, docs/design-session-attach.md §"State synchronization"): the
    /// same parser + grid a pane runs, fed from the PTY reader even with
    /// no client attached. It answers terminal queries itself (exactly
    /// one reply per query) and is the source of attach snapshots, so
    /// the host never replays raw historic bytes as an attach.
    pub struct Model {
        parser: Parser,
        grid: Grid,
        /// The main-screen grid while an alt-screen app runs.
        main_grid: Option<Grid>,
        in_alt: bool,
        title: Option<String>,
        cwd: Option<String>,
        /// Bumped once per PTY read chunk and per resize; a client
        /// resumes from a revision with retained deltas or an installed
        /// snapshot, never both for the same interval.
        revision: u64,
        /// Raw PTY chunks keyed to the revision that produced them
        /// (a resume window, not the scrollback: history is
        /// negotiated separately).
        history: VecDeque<(u64, Vec<u8>)>,
        history_bytes: usize,
    }

    impl Model {
        fn new(cols: usize, rows: usize) -> Self {
            Self {
                parser: Parser::new(),
                grid: Grid::with_size(cols, rows, 0),
                main_grid: None,
                in_alt: false,
                title: None,
                cwd: None,
                revision: 0,
                history: VecDeque::new(),
                history_bytes: 0,
            }
        }

        /// Feed one PTY chunk; mirrors `Pane::feed` (pane.rs) minus the
        /// shell-integration bookkeeping. Returns the DSR/DA replies the
        /// shell expects, so detached sessions answer queries alone.
        fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
            let mut replies = Vec::new();
            let mut pos = 0usize;
            while pos < bytes.len() {
                let outcome = self.parser.advance(&mut self.grid, &bytes[pos..]);
                let step = outcome.consumed.min(bytes.len() - pos).max(1);
                pos += step;
                for event in outcome.events {
                    match event {
                        ParserEvent::Title(t) => self.title = Some(t),
                        ParserEvent::Cwd(url) => self.cwd = Some(url),
                        ParserEvent::EnterAltScreen => {
                            if !self.in_alt {
                                let (cols, rows) = (self.grid.cols, self.grid.rows);
                                let fresh = Grid::with_size(cols, rows, 0);
                                self.main_grid = Some(std::mem::replace(&mut self.grid, fresh));
                                self.in_alt = true;
                            }
                        }
                        ParserEvent::ExitAltScreen => {
                            if self.in_alt {
                                if let Some(main) = self.main_grid.take() {
                                    self.grid = main;
                                    let (cols, rows) = (self.grid.cols, self.grid.rows);
                                    self.grid.resize(cols, rows);
                                }
                                self.in_alt = false;
                            }
                        }
                        ParserEvent::ClearScrollback => self.grid.clear_scrollback(),
                        ParserEvent::Respond(reply) => replies.extend_from_slice(&reply),
                        // Watchers need no host-side bookkeeping for these.
                        ParserEvent::Hyperlink(_)
                        | ParserEvent::Bell
                        | ParserEvent::PromptStart
                        | ParserEvent::CommandStart
                        | ParserEvent::CommandOutput
                        | ParserEvent::PromptEnd(_)
                        | ParserEvent::CursorVisibility(_)
                        | ParserEvent::Graphics(_) => {}
                    }
                }
            }
            self.parser.fit(&self.grid);
            replies
        }

        /// Record a delivered chunk under the revision that follows it,
        /// evicting from the front to stay inside the byte budget.
        fn push_history(&mut self, after_rev: u64, chunk: &[u8]) {
            if chunk.is_empty() {
                return;
            }
            self.history_bytes += chunk.len();
            self.history.push_back((after_rev, chunk.to_vec()));
            while self.history_bytes > MAX_HISTORY_BYTES
                && let Some((_, chunk)) = self.history.pop_front()
            {
                self.history_bytes -= chunk.len();
            }
        }

        /// True when a client at revision `from` resumes exactly: every
        /// revision in `from + 1..=revision` is retained, so `deltas`
        /// sends each one once and skips nothing.
        fn covers(&self, from: u64) -> bool {
            from <= self.revision
                && (from == self.revision
                    || self
                        .history
                        .front()
                        .is_some_and(|(rev, _)| *rev <= from + 1))
        }

        /// Deltas that carry a client at `from` up to `revision`.
        fn deltas(&self, from: u64) -> Vec<Value> {
            self.history
                .iter()
                .filter(|(rev, _)| *rev > from)
                .map(|(rev, bytes)| json!([rev, hex_encode(bytes)]))
                .collect()
        }

        /// The grid + state at the current revision, as SGR-restoring
        /// spans (`host-snapshot/v1`). A fresh grid is a repaint of the
        /// same shape, so snapshot bytes and live bytes converge.
        fn snapshot(&self) -> Value {
            let lines: Vec<Value> = grid_lines(&self.grid);
            let main_lines = self
                .main_grid
                .as_ref()
                .map_or(Value::Null, |g| json!(grid_lines(g)));
            let mouse = match self.parser.mouse_protocol {
                crate::parser::MouseProtocol::Off => "off",
                crate::parser::MouseProtocol::Click => "click",
                crate::parser::MouseProtocol::Drag => "drag",
                crate::parser::MouseProtocol::Any => "any",
            };
            json!({
                "schema": SNAPSHOT_SCHEMA,
                "revision": self.revision,
                "cols": self.grid.cols,
                "rows": self.grid.rows,
                "inAlt": self.in_alt,
                "mainLines": main_lines,
                "mainCols": self.main_grid.as_ref().map_or(0, |g| g.cols),
                "mainRows": self.main_grid.as_ref().map_or(0, |g| g.rows),
                "cursor": {
                    "row": self.parser.cursor_row,
                    "col": self.parser.cursor_col,
                    "visible": self.parser.cursor_visible,
                },
                "modes": {
                    "autoWrap": self.parser.auto_wrap,
                    "bracketedPaste": self.parser.bracketed_paste,
                    "appCursorKeys": self.parser.application_cursor_keys,
                    "mouse": mouse,
                    "mouseSgr": self.parser.mouse_sgr_coordinates,
                    "mouseUrxvt": self.parser.mouse_urxvt_coordinates,
                },
                "pen": pen_value(&self.parser),
                "parser": parser_value(&self.parser),
                "title": self.title,
                "cwd": self.cwd,
                "lines": lines,
            })
        }

        /// Keep the model's geometry in step with a PTY resize: the
        /// revision bump tells clients the repaint bytes are in the
        /// stream.
        fn resize(&mut self, cols: usize, rows: usize) {
            self.grid.resize(cols, rows);
            if let Some(ref mut main) = self.main_grid {
                main.resize(cols, rows);
            }
            self.parser.fit(&self.grid);
            self.revision += 1;
            // Keep the delta stream contiguous: this revision carries
            // no PTY bytes, so record an empty chunk for it.
            self.history.push_back((self.revision, Vec::new()));
        }
    }

    /// One grid row as run-length `[text, fg, bg, attrs]` spans, split
    /// only at style changes. Blanks between words stay inside the run
    /// so a client painting the spans in order reproduces the row
    /// exactly, and the blank tail (default style) overwrites any
    /// stale content past the text.
    fn spans(line: &crate::grid::Line) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        let mut cur = String::new();
        let mut style: Option<(Option<String>, Option<String>, u8)> = None;
        for cell in &line.cells {
            if cell.continuation {
                continue;
            }
            let st = (
                cell.fg.map(|c| c.to_string()),
                cell.bg.map(|c| c.to_string()),
                cell.attributes.flags(),
            );
            if style != Some(st.clone()) {
                if !cur.is_empty() {
                    let (fg, bg, attrs) = style.take().unwrap_or_default();
                    out.push(json!([std::mem::take(&mut cur), fg, bg, attrs]));
                }
                style = Some(st);
            }
            let text = crate::grid::cell_text(cell);
            cur.push_str(&text);
        }
        let (fg, bg, attrs) = style.unwrap_or_default();
        out.push(json!([cur, fg, bg, attrs]));
        out
    }

    /// One host-owned session: metadata + every kernel handle the shell
    /// needs (conpty, process, kill-on-close job, reader thread).
    pub struct Managed {
        /// The session's public record (list/info output).
        pub record: SessionRecord,
        conpty: ConPty,
        /// Handle used by terminate (poll + job kill).
        process: SendHandle,
        job: SendHandle,
        /// Reader thread handle; detached at drop.
        _reader: std::thread::JoinHandle<()>,
        /// Attached clients: per-connection token + queue of frames to
        /// write (output chunks, input, resize, exit). The connection's
        /// pump thread is the only writer of its pipe.
        subs: Vec<(u64, std::sync::mpsc::SyncSender<Value>)>,
        /// Token of the client with input rights; `None` = uncontrolled.
        controller: Option<u64>,
        /// Canonical terminal model + revision history (issue #48).
        model: Model,
    }

    /// Shared host state. One `Mutex` guards it; requests are short.
    /// shortcut: terminate holds the lock up to GRACEFUL_WAIT_MS,
    /// upgrade when concurrent clients need terminate not to block them.
    pub struct App {
        ids: IdsState,
        ids_dir: std::path::PathBuf,
        cfg: Config,
        live: BTreeMap<u32, Managed>,
        pub(crate) exited: VecDeque<SessionRecord>,
        /// Set by `host.shutdown`; the serve loop exits at the next tick.
        pub shutdown: Arc<AtomicBool>,
        /// Connected clients; idle exit needs zero.
        pub clients: Arc<AtomicUsize>,
        /// Last request time; the idle-exit budget is measured from here.
        pub last_activity: Arc<Mutex<Instant>>,
    }

    /// Errors that map onto the fixed error-code table.
    enum Fail {
        Code(&'static str, String),
    }

    impl App {
        /// Load persisted ids + config, start empty.
        ///
        /// # Errors
        ///
        /// Allocator file present but corrupt (explicit recovery, per the
        /// attach design), or config invalid.
        pub fn new() -> anyhow::Result<Self> {
            let ids_dir = host_state_dir()
                .ok_or_else(|| anyhow::anyhow!("no %%APPDATA%%; host needs a state directory"))?;
            Ok(Self {
                ids: IdsState::load(&ids_dir)?,
                ids_dir,
                cfg: Config::load()?,
                live: BTreeMap::new(),
                exited: VecDeque::new(),
                shutdown: Arc::new(AtomicBool::new(false)),
                clients: Arc::new(AtomicUsize::new(0)),
                last_activity: Arc::new(Mutex::new(Instant::now())),
            })
        }

        fn save_ids(&mut self) -> io::Result<()> {
            self.ids.save(&self.ids_dir)
        }

        fn records(&self) -> Vec<&SessionRecord> {
            let mut all: Vec<&SessionRecord> = self.exited.iter().collect();
            all.extend(self.live.values().map(|m| &m.record));
            all.sort_by_key(|r| r.alias);
            all
        }

        pub(crate) fn find_record(&self, alias: u32) -> Option<&SessionRecord> {
            self.live
                .get(&alias)
                .map(|m| &m.record)
                .or_else(|| self.exited.iter().find(|r| r.alias == alias))
        }

        pub(crate) fn push_exited(&mut self, record: SessionRecord) {
            self.exited.push_back(record);
            while self.exited.len() > MAX_EXITED_RECORDS {
                self.exited.pop_front();
            }
        }
    }

    /// Kill-on-close job object (ADR §4): closing the host's job handle
    /// kills the whole tree, and a client's death never touches it.
    fn create_kill_job() -> io::Result<SendHandle> {
        // Safety: plain kernel object creation; handle owned next line.
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::default())
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                core::ptr::from_ref(&info).cast::<core::ffi::c_void>(),
                u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
            );
            if set.is_err() {
                CloseHandle(job).ok();
                return Err(io::Error::last_os_error());
            }
            Ok(SendHandle::from_owned(Owned::new(job)))
        }
    }

    /// Resolve a profile name against config + discovery; `None` is the
    /// default probe shell.
    fn resolve_profile(cfg: &Config, name: Option<&str>) -> Result<SpawnOptions, Fail> {
        let Some(n) = name else {
            return Ok(probe_shell(cfg));
        };
        if let Some(p) = cfg.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(n)) {
            let mut env = p.env.clone();
            add_msys_pcon(&p.command, &mut env);
            return Ok(SpawnOptions {
                program: p.command.clone(),
                args: p.args.clone(),
                cols: 0,
                rows: 0,
                cwd: p.expanded_dir().ok(),
                env,
            });
        }
        if let Some(f) = discovery::discover()
            .into_iter()
            .find(|f| f.name.eq_ignore_ascii_case(n))
        {
            let mut env = std::collections::BTreeMap::new();
            add_msys_pcon(&f.program, &mut env);
            return Ok(SpawnOptions {
                program: f.program,
                args: f.args,
                cols: 0,
                rows: 0,
                cwd: None,
                env,
            });
        }
        Err(Fail::Code("bad-params", format!("unknown profile {n:?}")))
    }

    /// MSYS shells only attach to a ConPTY with `enable_pcon`.
    fn add_msys_pcon(program: &str, env: &mut std::collections::BTreeMap<String, String>) {
        if program.to_lowercase().contains("\\git\\") {
            env.insert("MSYS".into(), "enable_pcon".into());
            env.insert("MSYS2".into(), "enable_pcon".into());
        }
    }

    impl App {
        /// Spawn a host-owned session and register it.
        fn create_session(app: &Arc<Mutex<Self>>, params: &Value) -> Result<SessionRecord, Fail> {
            let mut g = app
                .lock()
                .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
            let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80);
            let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24);
            let persistent = params
                .get("persistent")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let project = params
                .get("project")
                .and_then(Value::as_str)
                .map(str::to_string);
            let task_id = params
                .get("taskId")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok());
            if let Some(task_id) = task_id {
                for m in g.live.values() {
                    if m.record.task_id == Some(task_id) && m.record.project == project {
                        return Ok(m.record.clone());
                    }
                }
            }
            let profile_name = params.get("profile").and_then(Value::as_str);
            let mut opts = resolve_profile(&g.cfg, profile_name)?;
            if let Some(dir) = params.get("cwd").and_then(Value::as_str) {
                if !std::path::Path::new(dir).is_dir() {
                    return Err(Fail::Code(
                        "bad-params",
                        format!("{dir:?} is not a directory"),
                    ));
                }
                opts.cwd = Some(dir.to_string());
            }
            if let Some(program) = params.get("program").and_then(Value::as_str) {
                opts.program = program.to_string();
            }
            if let Some(args) = params.get("args").and_then(Value::as_array) {
                let mut converted: Vec<String> = Vec::new();
                for arg in args.iter() {
                    if let Some(value) = arg.as_str() {
                        converted.push(value.to_string());
                    }
                }
                opts.args = converted;
            }
            opts.cols = usize::try_from(cols).unwrap_or(0);
            opts.rows = usize::try_from(rows).unwrap_or(0);
            if !(1..=i16::MAX as usize).contains(&opts.cols)
                || !(1..=i16::MAX as usize).contains(&opts.rows)
            {
                return Err(Fail::Code(
                    "bad-params",
                    "cols/rows must be 1..=32767".to_string(),
                ));
            }

            let (alias, id) = g.ids.allocate();
            if let Err(e) = g.save_ids() {
                g.ids.aliases.remove(&alias);
                g.ids.next_alias = alias;
                g.ids.next_id = id;
                return Err(Fail::Code(
                    "internal",
                    format!("identity persistence failed: {e}"),
                ));
            }
            // Self-attach prevention (attach ADR): a shell started by the
            // host knows its alias and refuses to attach to its own session.
            opts.env.insert("RUSTTY_SESSION".into(), alias.to_string());
            let job = create_kill_job().map_err(|e| Fail::Code("internal", e.to_string()))?;
            let spawned = ConPty::spawn(
                &opts.program,
                &opts.args,
                opts.cols,
                opts.rows,
                opts.cwd.as_deref(),
                &opts.env,
                Some(job.raw()),
            );
            let (conpty, process, thread) = match spawned {
                Ok(v) => v,
                Err(e) => {
                    g.ids.aliases.remove(&alias);
                    return Err(Fail::Code("internal", format!("spawn failed: {e}")));
                }
            };
            drop(thread); // primary thread handle; the process handle stays.
            // Safety: GetProcessId on the child process handle we own.
            let pid = unsafe { GetProcessId(process.raw()) };
            let record = SessionRecord {
                id,
                alias,
                name: params
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        profile_name.map(str::to_string).unwrap_or_else(|| {
                            std::path::Path::new(&opts.program)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "shell".to_string())
                        })
                    }),
                profile: profile_name.unwrap_or("default").to_string(),
                cwd: opts.cwd.clone().unwrap_or_default(),
                program: opts.program.clone(),
                cols: opts.cols,
                rows: opts.rows,
                pid,
                persistent,
                state: "live".into(),
                created_unix_secs: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                exit_code: None,
                task_id,
                project: project.clone(),
            };

            // Second process handle for the terminate path (poll/kill);
            // the reader thread keeps the original.
            let term = unsafe {
                match OpenProcess(
                    PROCESS_ACCESS_RIGHTS(PROCESS_TERMINATE.0 | SYNCHRONIZE.0),
                    false,
                    pid,
                ) {
                    Ok(h) => Owned::new(h),
                    Err(_) => Owned::new(process.raw()),
                }
            };
            let managed_process = SendHandle::from_owned(term);
            let reader = conpty
                .output_reader()
                .map_err(|e| Fail::Code("internal", e.to_string()))?;
            // A headless session has no pane to answer terminal queries,
            // and bash/readline blocks on a cursor-position query (CSI 6n)
            // until a report arrives; the reader thread answers it.
            let query_writer = conpty
                .input_writer()
                .map_err(|e| Fail::Code("internal", e.to_string()))?;
            let app_for_reader = Arc::clone(app);
            let exit = exit_notifier(app, alias, id);
            // The conhost server keeps the output pipe's write end open
            // after the client app exits (measured: shell dead, no output
            // EOF), so EOF alone can hang exit forever. Watch the process
            // handle directly; the first caller (watcher or EOF) reports,
            // the second is a no-op (issue #48, acceptance 7).
            // Safety: duplicate our own process handle for the watcher.
            let managed = Managed {
                record: record.clone(),
                conpty,
                process: managed_process,
                job,
                subs: Vec::new(),
                controller: None,
                model: Model::new(record.cols, record.rows),
                _reader: std::thread::spawn(move || {
                    let on_chunk = |chunk: &[u8]| {
                        let hex = hex_encode(chunk);
                        let mut replies = Vec::new();
                        if let Ok(mut guard) = app_for_reader.lock()
                            && let Some(m) = guard.live.get_mut(&alias)
                            && m.record.id == id
                        {
                            // The host model is the terminal: it parses
                            // every chunk and answers terminal queries
                            // itself (exactly one reply, even detached).
                            replies = m.model.feed(chunk);
                            m.model.revision += 1;
                            let rev = m.model.revision;
                            m.model.push_history(rev, chunk);
                            let (cols, rows) = (m.record.cols, m.record.rows);
                            // A saturated watcher is asked to resync
                            // (best effort: its queue is what is full)
                            // and then dropped; its pump closes that
                            // connection, so no client stalls the rest.
                            let mouse = match m.model.parser.mouse_protocol {
                                crate::parser::MouseProtocol::Off => "off",
                                crate::parser::MouseProtocol::Click => "click",
                                crate::parser::MouseProtocol::Drag => "drag",
                                crate::parser::MouseProtocol::Any => "any",
                            };
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "session.output",
                                "params": {
                                    "alias": alias,
                                    "id": id,
                                    "data": hex,
                                    "cols": cols,
                                    "rows": rows,
                                    "revision": rev,
                                    "schema": SCHEMA,
                                    "mouse": mouse,
                                    "mouseSgr": m.model.parser.mouse_sgr_coordinates,
                                    "mouseUrxvt": m.model.parser.mouse_urxvt_coordinates,
                                },
                            });
                            let resync = json!({
                                "jsonrpc": "2.0",
                                "method": "session.resync",
                                "params": { "alias": alias, "id": id, "schema": SCHEMA },
                            });
                            // A saturated subscriber keeps its
                            // subscription: the frame it cannot keep up
                            // with is dropped, and the revision gap in
                            // the stream tells it to resync from a fresh
                            // snapshot (session.resync is a best-effort
                            // nudge through the same full queue). A
                            // client that is gone closes its own
                            // connection when its pump write fails.
                            for (_, tx) in &m.subs {
                                if tx.try_send(note.clone()).is_err() {
                                    let _ = tx.try_send(resync.clone());
                                }
                            }
                        }
                        // Replies go to the PTY input outside the lock.
                        for reply in replies.chunks(64 * 1024) {
                            let _ = query_writer.write(reply);
                        }
                    };
                    let _ = conpty::read_output_loop(&reader, on_chunk);
                    let code = conpty::wait_exit_code(&process);
                    exit(code.map(|c| c.cast_unsigned()));
                }),
            };
            g.live.insert(alias, managed);
            Ok(record)
        }

        /// Terminate one session: graceful (stdin EOF, then the job tree
        /// kill after the budget) or forced (tree kill now). ADR §4.
        fn terminate_session(
            app: &Arc<Mutex<Self>>,
            params: &Value,
        ) -> Result<SessionRecord, Fail> {
            let alias = params
                .get("alias")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| Fail::Code("bad-params", "alias (number) required".to_string()))?;
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut g = app
                .lock()
                .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
            let known = g.ids.resolve(alias).is_some_and(|id| {
                g.live.contains_key(&alias) || g.exited.iter().any(|r| r.id == id)
            });
            if !known {
                return Err(Fail::Code(
                    "target-stale",
                    format!("no session with alias {alias}"),
                ));
            }
            let Some(mut m) = g.live.remove(&alias) else {
                let rec = g
                    .find_record(alias)
                    .cloned()
                    .ok_or_else(|| Fail::Code("target-stale", "session gone".to_string()))?;
                return Err(Fail::Code(
                    "target-closed",
                    format!(
                        "session {alias} already exited (exitCode {:?})",
                        rec.exit_code
                    ),
                ));
            };
            if force {
                // Safety: job handle we own; kills the whole tree.
                unsafe {
                    TerminateJobObject(m.job.raw(), 1).ok();
                }
            } else {
                m.conpty.close_input(); // shell sees stdin EOF
                let deadline = Instant::now() + Duration::from_millis(GRACEFUL_WAIT_MS);
                while Instant::now() < deadline {
                    // Safety: poll our own process handle.
                    if unsafe { WaitForSingleObject(m.process.raw(), 200) } == WAIT_OBJECT_0 {
                        break;
                    }
                }
                // Safety: still alive after the budget: tree kill.
                unsafe {
                    TerminateJobObject(m.job.raw(), 1).ok();
                }
            }
            let code = conpty::wait_exit_code(&m.process).map(|c| c.cast_unsigned());
            m.record.state = "exited".into();
            m.record.exit_code = code;
            let record = m.record.clone();
            g.push_exited(record.clone());
            let exit_note = json!({
                "jsonrpc": "2.0",
                "method": "session.exit",
                "params": { "alias": alias, "id": record.id, "exitCode": code, "schema": SCHEMA },
            });
            for (_, tx) in &m.subs {
                let _ = tx.try_send(exit_note.clone());
            }
            drop(m); // closes conpty + job handles (tree already down)
            // The reader thread finds no live session and no-ops.
            Ok(record)
        }

        /// True when at least one session is live (the idle-exit check).
        #[must_use]
        pub fn has_live(&self) -> bool {
            !self.live.is_empty()
        }

        /// Ephemeral rule (ADR §3): when the last client detaches, every
        /// live ephemeral session terminates (its clients are gone).
        pub fn reap_ephemeral(app: &Arc<Mutex<Self>>) {
            let ephemeral: Vec<u32> = {
                let Ok(g) = app.lock() else { return };
                if g.clients.load(Ordering::Relaxed) != 0 {
                    return;
                }
                g.live
                    .values()
                    .filter(|m| !m.record.persistent)
                    .map(|m| m.record.alias)
                    .collect()
            };
            for alias in ephemeral {
                Self::terminate_session(app, &json!({"alias": alias, "force": true})).ok();
            }
        }
    }

    /// Report a session exit (process watcher and reader thread -> App).
    /// Both callers share one `Arc`; the live-and-matching-id check makes
    /// the second call a no-op. The `Managed` drop (which closes the
    /// ConPTY) runs with the state lock released: `ClosePseudoConsole`
    /// blocks until the conhost server exits, and that exit needs the
    /// reader thread to finish (issue #48, acceptance 7).
    fn exit_notifier(
        app: &Arc<Mutex<App>>,
        alias: u32,
        id: u64,
    ) -> Arc<dyn Fn(Option<u32>) + Send + Sync + 'static> {
        let app = Arc::clone(app);
        Arc::new(move |code| {
            let mut dead = None;
            if let Ok(mut app) = app.lock() {
                let live = app.live.get(&alias).is_some_and(|m| m.record.id == id);
                if live && let Some(mut m) = app.live.remove(&alias) {
                    m.record.state = "exited".into();
                    m.record.exit_code = code;
                    let exit_note = json!({
                        "jsonrpc": "2.0",
                        "method": "session.exit",
                        "params": {
                            "alias": alias,
                            "id": id,
                            "exitCode": code,
                            "schema": SCHEMA,
                        },
                    });
                    for (_, tx) in &m.subs {
                        let _ = tx.try_send(exit_note.clone());
                    }
                    app.push_exited(m.record.clone());
                    dead = Some(m);
                }
            }
            drop(dead);
        })
    }

    /// One request in, one response out (the client thread loop).
    /// Unknown methods and malformed params answer `bad-params`.
    pub fn handle(app: &Arc<Mutex<App>>, req: &Value, conn: &Conn<'_>) -> Value {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        match dispatch(app, req, conn) {
            Ok(v) => response(id, v),
            Err(Fail::Code(code, msg)) => error(id, code, &msg),
        }
    }

    /// Per-connection identity: the attach token (control ownership +
    /// subscription key) and the queue the connection's pump drains.
    pub struct Conn<'a> {
        /// Unique token for this connection.
        pub token: u64,
        /// Frame queue; `session.attach` registers a clone of it.
        pub tx: &'a SyncSender<Value>,
    }

    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

    /// Unique per-connection token (issue #47).
    #[must_use]
    pub fn new_token() -> u64 {
        NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
    }

    /// Start the connection's writer thread: the sole writer of the
    /// (duplicated) pipe handle. It drains `rx` — output/input/resize
    /// frames from other threads — and when the queue closes (the
    /// connection thread ended) it releases the attach and closes the
    /// handle.
    ///
    /// # Errors
    /// IO error duplicating the handle.
    pub fn new_pump(
        pipe: HANDLE,
        app: Arc<Mutex<App>>,
        token: u64,
        rx: Receiver<Value>,
    ) -> io::Result<std::thread::JoinHandle<()>> {
        // Safety: duplicate our own pipe handle with the same access.
        let dup = {
            let mut h = HANDLE::default();
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    pipe,
                    GetCurrentProcess(),
                    &mut h,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
            }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            // Safety: h is the fresh handle we just duplicated and now own.
            unsafe { SendHandle::from_owned(Owned::new(h)) }
        };
        Ok(std::thread::spawn(move || {
            let mut conn_pipe = DupConn { handle: dup };
            while let Ok(frame) = rx.recv() {
                if crate::host::write_frame(&mut conn_pipe, &frame).is_err() {
                    break;
                }
            }
            // The connection thread ended: release control + subscription.
            let req = json!({
                "jsonrpc": "2.0", "id": serde_json::Value::Null,
                "method": "session.detach", "params": { "token": token },
            });
            let (tx, _drain) = sync_channel(1);
            let conn = Conn { token, tx: &tx };
            handle(&app, &req, &conn);
            // Dropping `conn_pipe` closes the duplicated handle.
        }))
    }

    /// Attach: subscribe to a live session's stream; with `readOnly`
    /// false, take controller rights (busy → `session-busy`; `takeControl`
    /// demotes the current controller to a watcher, per the ADR).
    /// With `fromRevision` the client resumes from retained output when
    /// the window covers it; otherwise the result carries the model's
    /// snapshot — the host never replays raw historic bytes as an attach
    /// (issue #48). Subscribing is idempotent per connection token, so
    /// a mid-stream resync over the same connection never doubles the
    /// stream.
    fn session_attach(
        app: &Arc<Mutex<App>>,
        params: &Value,
        conn: &Conn<'_>,
    ) -> Result<Value, Fail> {
        let alias = param_alias(params)?;
        let readonly = params
            .get("readOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let takeover = params
            .get("takeControl")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let from = params.get("fromRevision").and_then(Value::as_u64);
        let mut g = app
            .lock()
            .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
        let m = g.live.get_mut(&alias).ok_or_else(|| {
            Fail::Code(
                "target-stale",
                format!("no live session with alias {alias}"),
            )
        })?;
        // Remember who we demote so they can be told (issue #49).
        let demoted = if takeover && m.controller.is_some() && m.controller != Some(conn.token) {
            m.controller
        } else {
            None
        };
        if !readonly && m.controller.is_some() && m.controller != Some(conn.token) && !takeover {
            return Err(Fail::Code(
                "session-busy",
                format!("session {alias} is controlled by another client (use --take-control)"),
            ));
        }
        if !m.subs.iter().any(|(t, _)| *t == conn.token) {
            m.subs.push((conn.token, conn.tx.clone()));
        }
        let granted = if readonly {
            false
        } else {
            // takeover demotes the previous controller (ADR) and tells
            // them so their UI can stop sending (issue #49).
            if let Some((_, tx)) = demoted.and_then(|old| m.subs.iter().find(|(t, _)| *t == old)) {
                let _ = tx.try_send(json!({
                    "method": "session.control",
                    "params": {
                        "alias": alias,
                        "id": m.record.id,
                        "controller": false,
                        "schema": SCHEMA,
                    },
                }));
            }
            m.controller = Some(conn.token);
            true
        };
        let mut result = json!({
            "alias": alias,
            "id": m.record.id,
            "pid": m.record.pid,
            "cols": m.record.cols,
            "rows": m.record.rows,
            "state": "live",
            "controlGranted": granted,
            "schema": SCHEMA,
        });
        match from {
            Some(rf) if m.model.covers(rf) => {
                result["resumed"] = json!(true);
                result["revision"] = json!(m.model.revision);
                result["deltas"] = json!(m.model.deltas(rf));
            }
            _ => {
                result["resumed"] = json!(false);
                result["revision"] = json!(m.model.revision);
                result["snapshot"] = m.model.snapshot();
            }
        }
        Ok(result)
    }

    /// Input bytes (hex) to the PTY. Controller only.
    fn session_input(
        app: &Arc<Mutex<App>>,
        params: &Value,
        conn: &Conn<'_>,
    ) -> Result<Value, Fail> {
        let alias = param_alias(params)?;
        let data = params
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Fail::Code("bad-params", "data (hex string) required".to_string()))?;
        let bytes = hex_decode(data)?;
        if bytes.len() > crate::host::MAX_REQUEST_BYTES {
            return Err(Fail::Code(
                "bad-params",
                "input frame too large".to_string(),
            ));
        }
        let mut g = app
            .lock()
            .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
        let m = g
            .live
            .get_mut(&alias)
            .ok_or_else(|| Fail::Code("target-stale", "session gone".to_string()))?;
        if m.controller != Some(conn.token) {
            return Err(Fail::Code(
                "session-busy",
                "not the controller of this session".to_string(),
            ));
        }
        m.conpty
            .write_input(&bytes)
            .map_err(|e| Fail::Code("internal", e.to_string()))?;
        Ok(json!({ "written": bytes.len() }))
    }

    /// Resize the PTY + record; forwarded to every watcher. Controller only.
    fn session_resize(
        app: &Arc<Mutex<App>>,
        params: &Value,
        conn: &Conn<'_>,
    ) -> Result<Value, Fail> {
        let alias = param_alias(params)?;
        let cols = param_u16(params, "cols")?;
        let rows = param_u16(params, "rows")?;
        let mut g = app
            .lock()
            .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
        let m = g
            .live
            .get_mut(&alias)
            .ok_or_else(|| Fail::Code("target-stale", "session gone".to_string()))?;
        if m.controller != Some(conn.token) {
            return Err(Fail::Code(
                "session-busy",
                "not the controller of this session".to_string(),
            ));
        }
        m.conpty
            .resize(usize::from(cols), usize::from(rows))
            .map_err(|e| Fail::Code("internal", e.to_string()))?;
        m.record.cols = usize::from(cols);
        m.record.rows = usize::from(rows);
        // The model repaints at the new geometry in the same revision
        // stream, so watchers see the resize bytes between revisions.
        m.model.resize(usize::from(cols), usize::from(rows));
        let rev = m.model.revision;
        let note = json!({
            "jsonrpc": "2.0",
            "method": "session.resize",
            "params": { "alias": alias, "id": m.record.id, "cols": cols, "rows": rows, "revision": rev, "schema": SCHEMA },
        });
        // A saturated subscriber drops this frame; its resync (the
        // attach result) carries the new geometry.
        for (_, tx) in &m.subs {
            let _ = tx.try_send(note.clone());
        }
        Ok(json!({ "cols": cols, "rows": rows, "revision": rev }))
    }

    /// Release a connection's attach everywhere: unregister its stream
    /// and free the controller slot it held.
    pub fn detach_token(app: &Arc<Mutex<App>>, token: u64) -> Value {
        if let Ok(mut g) = app.lock() {
            for m in g.live.values_mut() {
                m.subs.retain(|(t, _)| *t != token);
                if m.controller == Some(token) {
                    m.controller = None;
                }
            }
        }
        json!({ "detached": true })
    }

    fn param_alias(params: &Value) -> Result<u32, Fail> {
        params
            .get("alias")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| Fail::Code("bad-params", "alias (number) required".to_string()))
    }

    fn param_u16(params: &Value, key: &str) -> Result<u16, Fail> {
        params
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| Fail::Code("bad-params", format!("{key} (number > 0) required")))
    }

    /// The method table; `handle` attaches the envelope id.
    fn dispatch(app: &Arc<Mutex<App>>, req: &Value, conn: &Conn<'_>) -> Result<Value, Fail> {
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| Fail::Code("bad-params", "missing method".to_string()))?;
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
        let result: Result<Value, Fail> = match method {
            "initialize" => {
                let wanted = params.get("protocol").and_then(Value::as_u64).unwrap_or(0);
                let wanted = u32::try_from(wanted).unwrap_or(0);
                let negotiated = wanted.min(PROTOCOL_VERSION);
                if negotiated == 0 {
                    Err(Fail::Code(
                        "protocol-mismatch",
                        format!(
                            "client protocol {wanted} unsupported (host speaks {PROTOCOL_VERSION})"
                        ),
                    ))
                } else {
                    Ok(json!({
                        "protocol": negotiated,
                        "host": "rustty-host",
                        "version": env!("CARGO_PKG_VERSION"),
                        "ops": OPS,
                        "snapshotSchema": SNAPSHOT_SCHEMA,
                    }))
                }
            }
            "session.create" => App::create_session(app, &params)
                .map(|r| json!({"schema": SCHEMA, "session": r.json()})),
            "session.list" => {
                let g = app
                    .lock()
                    .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
                let sessions = g
                    .records()
                    .iter()
                    .map(|r| {
                        let mut controllers = 0usize;
                        let mut watchers = 0usize;
                        if let Some(m) = g.live.get(&r.alias) {
                            if m.controller.is_some() {
                                controllers = 1;
                            }
                            watchers = m.subs.len() - controllers;
                        }
                        let mut rec = r.json();
                        rec["controllers"] = json!(controllers);
                        rec["watchers"] = json!(watchers);
                        rec
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"schema": SCHEMA, "sessions": sessions }))
            }
            "session.info" => {
                let g = app
                    .lock()
                    .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
                let alias = params
                    .get("alias")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| {
                        Fail::Code("bad-params", "alias (number) required".to_string())
                    })?;
                g.find_record(alias)
                    .map(|r| json!({"schema": SCHEMA, "session": r.json()}))
                    .ok_or_else(|| {
                        Fail::Code("target-stale", format!("no session with alias {alias}"))
                    })
            }
            "session.terminate" => App::terminate_session(app, &params)
                .map(|r| json!({"schema": SCHEMA, "session": r.json()})),
            "session.attach" => session_attach(app, &params, conn),
            "session.input" => session_input(app, &params, conn),
            "session.resize" => session_resize(app, &params, conn),
            "session.detach" => Ok(detach_token(app, conn.token)),
            "host.shutdown" => {
                let g = app
                    .lock()
                    .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
                g.shutdown.store(true, Ordering::SeqCst);
                Ok(json!({"schema": SCHEMA, "shuttingDown": true}))
            }
            _ => Err(Fail::Code(
                "bad-params",
                format!("unknown method {method:?}"),
            )),
        };
        result
    }

    /// Query one information class from a token.
    fn query_token(tok: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u8>> {
        // Safety: standard two-pass token query; buffers stay alive.
        unsafe {
            let mut needed = 0u32;
            let _ = GetTokenInformation(tok, class, None, 0, &mut needed);
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buf = vec![0u8; usize::try_from(needed).unwrap_or(0)];
            GetTokenInformation(
                tok,
                class,
                Some(buf.as_mut_ptr().cast::<core::ffi::c_void>()),
                needed,
                &mut needed,
            )
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            Ok(buf)
        }
    }

    /// The user SID and integrity level of the current process.
    fn own_identity() -> io::Result<(Vec<u8>, u32)> {
        // Safety: query against our own process token; buffers owned here.
        unsafe {
            let mut tok = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok)
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            let user = query_token(tok, TokenUser)?;
            let integrity = query_token(tok, TokenIntegrityLevel)?;
            CloseHandle(tok).ok();
            let tu = &*(user.as_ptr().cast::<TOKEN_USER>());
            let ml = &*(integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
            let sid_len =
                usize::try_from(GetSidLengthRequired(*GetSidSubAuthorityCount(tu.User.Sid)))
                    .unwrap_or(0);
            let off = (tu.User.Sid.0 as usize) - (user.as_ptr() as usize);
            let sid = user
                .get(off..off + sid_len)
                .ok_or(io::Error::other("token layout"))?
                .to_vec();
            let count = *GetSidSubAuthorityCount(ml.Label.Sid);
            let rid = *GetSidSubAuthority(ml.Label.Sid, u32::from(count.saturating_sub(1)));
            Ok((sid, rid))
        }
    }

    /// Pipe DACL granting the current user read/write, plus the
    /// SECURITY_ATTRIBUTES that carry it (ADR §2: current-user access,
    /// local-only). Kept alive by the returned buffers.
    pub struct PipeSecurity {
        sd: SECURITY_DESCRIPTOR,
        _acl: Vec<u8>,
        /// Current-user SID, granted RW on the pipe DACL.
        pub user_sid: Vec<u8>,
        /// Current-process integrity RID; peers must match it.
        pub integrity_rid: u32,
    }

    impl PipeSecurity {
        /// Build the descriptor for the current user.
        ///
        /// # Errors
        ///
        /// Token query failures.
        pub fn for_current_user() -> io::Result<Self> {
            let (sid, integrity_rid) = own_identity()?;
            let sid_len = sid.len();
            let acl_size = u32::try_from(8 + 8 + sid_len).unwrap_or(u32::MAX);
            let mut acl = vec![0u8; usize::try_from(acl_size).unwrap_or(0)];
            let mut sd = SECURITY_DESCRIPTOR::default();
            // One ACCESS_ALLOWED ACE for GENERIC_READ|GENERIC_WRITE,
            // built by advapi32 (no hand-assembled ACE bytes).
            unsafe {
                let sd_ptr = PSECURITY_DESCRIPTOR(core::ptr::from_mut(&mut sd).cast());
                InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION)
                    .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                InitializeAcl(acl.as_mut_ptr().cast(), acl_size, ACL_REVISION)
                    .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                AddAccessAllowedAce(
                    acl.as_mut_ptr().cast(),
                    ACL_REVISION,
                    0xC000_0000u32, // GENERIC_READ | GENERIC_WRITE
                    PSID(sid.as_ptr().cast_mut().cast()),
                )
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                SetSecurityDescriptorDacl(sd_ptr, true, Some(acl.as_ptr().cast()), false)
                    .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            }
            Ok(Self {
                sd,
                _acl: acl,
                user_sid: sid,
                integrity_rid,
            })
        }

        /// SECURITY_ATTRIBUTES pointing at the descriptor. The
        /// `PipeSecurity` must outlive every use.
        #[must_use]
        pub fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
                    .unwrap_or(u32::MAX),
                lpSecurityDescriptor: core::ptr::from_mut(&mut self.sd).cast(),
                bInheritHandle: false.into(),
            }
        }

        /// Same user SID and same integrity level as us (ADR §3):
        /// normal clients never control an elevated shell, elevated
        /// clients get `integrity-mismatch`, and vice versa.
        #[must_use]
        pub fn peer_ok(&self, pipe: HANDLE) -> bool {
            // Safety: all handles are short-lived query handles we close.
            unsafe {
                let mut pid = 0u32;
                if GetNamedPipeClientProcessId(pipe, &mut pid).is_err() {
                    return false;
                }
                let Ok(peer) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                    return false;
                };
                let mut tok = HANDLE::default();
                if OpenProcessToken(peer, TOKEN_QUERY, &mut tok).is_err() {
                    CloseHandle(peer).ok();
                    return false;
                }
                let ok = match (
                    query_token(tok, TokenUser),
                    query_token(tok, TokenIntegrityLevel),
                ) {
                    (Ok(user), Ok(integrity)) => {
                        let tu = &*(user.as_ptr().cast::<TOKEN_USER>());
                        let ml = &*(integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
                        let count = *GetSidSubAuthorityCount(ml.Label.Sid);
                        let rid =
                            *GetSidSubAuthority(ml.Label.Sid, u32::from(count.saturating_sub(1)));
                        let own = PSID(self.user_sid.as_ptr().cast_mut().cast());
                        EqualSid(tu.User.Sid, own).is_ok() && rid == self.integrity_rid
                    }
                    _ => false,
                };
                CloseHandle(tok).ok();
                CloseHandle(peer).ok();
                ok
            }
        }
    }

    /// A duplicated connection handle used by the pump thread; it only
    /// writes (the connection thread owns the reads).
    pub struct DupConn {
        handle: SendHandle,
    }

    impl io::Write for DupConn {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            super::overlapped_write(self.handle.raw(), buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Decode a hex string (session.input payloads).
    fn hex_decode(s: &str) -> Result<Vec<u8>, Fail> {
        let bad = || {
            Fail::Code(
                "bad-params",
                "data must be an even-length hex string".to_string(),
            )
        };
        let bytes = s.as_bytes();
        if !bytes.len().is_multiple_of(2) {
            return Err(bad());
        }
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks(2) {
            let Ok(text) = std::str::from_utf8(pair) else {
                return Err(bad());
            };
            let Ok(v) = u8::from_str_radix(text, 16) else {
                return Err(bad());
            };
            out.push(v);
        }
        Ok(out)
    }

    fn grid_lines(grid: &Grid) -> Vec<Value> {
        (0..grid.rows)
            .map(|r| {
                grid.row(r)
                    .map_or_else(|| json!([]), |line| json!(spans(line)))
            })
            .collect()
    }

    fn pen_value(p: &Parser) -> Value {
        json!({
            "fg": p.pen_fg.map_or(Value::Null, |c| json!(c.to_string())),
            "bg": p.pen_bg.map_or(Value::Null, |c| json!(c.to_string())),
            "attrs": p.pen_attrs.flags(),
            "link": p.pen_link,
        })
    }

    fn saved_value(p: &Parser) -> Value {
        if let Some(sc) = p.saved {
            json!({
                "row": sc.row,
                "col": sc.col,
                "fg": sc.fg.map_or(Value::Null, |c| json!(c.to_string())),
                "bg": sc.bg.map_or(Value::Null, |c| json!(c.to_string())),
                "attrs": sc.attrs.flags(),
            })
        } else {
            Value::Null
        }
    }

    fn state_name(p: &Parser) -> String {
        match p.state {
            crate::parser::State::Ground => "ground".to_string(),
            crate::parser::State::Escape => "escape".to_string(),
            crate::parser::State::EscapeIntermediate => "escape-intermediate".to_string(),
            crate::parser::State::Csi => "csi".to_string(),
            crate::parser::State::Osc => "osc".to_string(),
            crate::parser::State::OscEscape => "osc-escape".to_string(),
            crate::parser::State::KittyDetect => "kitty-detect".to_string(),
            crate::parser::State::Kitty => "kitty".to_string(),
            crate::parser::State::KittyEscape => "kitty-escape".to_string(),
            crate::parser::State::Ignore => "ignore".to_string(),
            crate::parser::State::IgnoreEscape => "ignore-escape".to_string(),
        }
    }

    fn parser_value(p: &Parser) -> Value {
        json!({
            "state": state_name(p),
            "params": hex_encode(&p.params),
            "private": hex_encode(&p.private),
            "intermediates": hex_encode(&p.intermediates),
            "osc": hex_encode(&p.osc),
            "utf8Pending": hex_encode(&p.utf8_pending),
            "stringUtf8": p.string_utf8,
            "saved": saved_value(p),
            "scrollTop": p.scroll_top,
            "scrollBottom": p.scroll_bottom,
            "regionFollowsBottom": p.region_follows_bottom,
            "wrapPending": p.wrap_pending,
            "pen": pen_value(p),
            "linkUris": p.link_uris,
        })
    }

    #[cfg(test)]
    mod model_tests {
        use super::*;

        #[test]
        fn model_answers_dsr_from_the_real_cursor() {
            let mut m = Model::new(80, 24);
            // The reply event arrives in the feed that consumes the
            // query (parser.rs dsr_and_da_respond).
            assert_eq!(m.feed(b"\x1b[5;1H\x1b[6n"), b"\x1b[5;1R");
            assert_eq!(m.feed(b"\x1b[c"), b"\x1b[?6c");
        }

        #[test]
        fn model_swaps_the_alt_screen_and_back() {
            let mut m = Model::new(80, 24);
            m.feed(b"main line");
            m.feed(b"\x1b[?1049halt screen");
            assert!(m.in_alt);
            assert!(m.main_grid.is_some());
            // Spans cover the whole row (blank tail included), so trim.
            let row_text = |s: &Value| -> String {
                s["lines"][0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|sp| sp[0].as_str().unwrap_or(""))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            };
            assert_eq!(row_text(&m.snapshot()), "alt screen");
            m.feed(b"\x1b[?1049l");
            assert!(!m.in_alt);
            assert_eq!(row_text(&m.snapshot()), "main line");
        }

        #[test]
        fn history_stays_inside_the_byte_budget() {
            let mut m = Model::new(80, 24);
            let chunk = vec![b'x'; 4096];
            for i in 1..=64u64 {
                m.push_history(i, &chunk);
                m.revision = i;
            }
            assert!(m.history_bytes <= MAX_HISTORY_BYTES);
            assert!(!m.covers(0));
            assert!(m.covers(64));
        }

        #[test]
        fn resize_keeps_the_delta_stream_contiguous() {
            let mut m = Model::new(80, 24);
            // Simulate the reader loop: rev bump + history push per chunk.
            m.revision = 1;
            m.push_history(1, b"aa");
            m.resize(120, 35);
            m.revision = 3;
            m.push_history(3, b"bb");
            // The resize revision carries no PTY bytes; a client at the
            // pre-resize revision must still receive every revision
            // once, in order (an empty chunk stands in for the resize).
            assert!(m.covers(1));
            let d = m.deltas(1);
            let revs: Vec<u64> = d.iter().map(|e| e[0].as_u64().unwrap()).collect();
            assert_eq!(revs, vec![2, 3]);
            assert_eq!(d[0][1].as_str().unwrap(), "");
            assert_eq!(d[1][1].as_str().unwrap(), "6262");
        }

        #[test]
        fn resume_window_is_exact() {
            let mut m = Model::new(80, 24);
            m.push_history(1, b"aaa");
            m.push_history(2, b"bbb");
            m.revision = 2;
            // 0 -> history starts at 1: resumes exactly; 1 -> starts
            // at 2: exact; 2 -> nothing to send; 3 -> future: refuse.
            assert!(m.covers(0) && m.covers(1) && m.covers(2));
            assert!(!m.covers(3));
            assert_eq!(m.deltas(1).len(), 1);
            assert_eq!(m.deltas(1)[0][0].as_u64().unwrap(), 2);
            assert_eq!(m.deltas(1)[0][1].as_str().unwrap(), "626262");
        }

        #[test]
        fn snapshot_carries_the_canvas_and_the_live_state() {
            let mut m = Model::new(80, 24);
            m.feed(b"\x1b[1;1H\x1b[31mred\x1b[0m tail");
            m.feed(b"\x1b]0;a title\x07");
            let s = m.snapshot();
            assert_eq!(s["schema"], SNAPSHOT_SCHEMA);
            assert_eq!(s["cols"], 80);
            assert_eq!(s["rows"], 24);
            assert_eq!(s["title"], "a title");
            // The red run is its own span; ANSI-16 red is this
            // project's palette entry (color.rs ansi_palette), spelled
            // uppercase by Color's Display.
            assert_eq!(s["lines"][0][0][0].as_str().unwrap(), "red");
            assert_eq!(s["lines"][0][0][1].as_str().unwrap(), "#D7005F");
            assert_eq!(s["lines"][0][1][1], Value::Null);
            assert_eq!(s["cursor"]["row"], 0);
            assert_eq!(s["cursor"]["col"], 8);
            assert_eq!(s["modes"]["autoWrap"], true);
        }

        #[test]
        fn resize_bumps_the_revision() {
            let mut m = Model::new(80, 24);
            m.resize(120, 35);
            assert_eq!(m.revision, 1);
            let s = m.snapshot();
            assert_eq!(s["cols"], 120);
            assert_eq!(s["rows"], 35);
        }
        #[test]
        fn alt_resize_keeps_main_geometry() {
            let mut m = Model::new(10, 3);
            m.feed(b"main");
            m.feed(b"\x1b[?1049h alt");
            m.resize(20, 5);
            m.feed(b"\x1b[?1049l");
            assert_eq!(m.grid.cols, 20);
            assert_eq!(m.grid.rows, 5);
        }

        #[test]
        fn snapshot_keeps_pen_and_utf8_pending() {
            let mut m = Model::new(80, 24);
            m.feed(b"\x1b[31mX");
            m.feed(b"\xC3");
            let s = m.snapshot();
            assert_eq!(s["pen"]["fg"].as_str().unwrap(), "#D7005F");
            assert_eq!(s["parser"]["utf8Pending"].as_str().unwrap(), "c3");
        }

        #[test]
        fn snapshot_keeps_main_lines_while_in_alt() {
            let mut m = Model::new(10, 3);
            m.feed(b"main");
            m.feed(b"\x1b[?1049h alt");
            let s = m.snapshot();
            assert!(s["mainLines"].is_array());
            assert_eq!(
                s["mainLines"][0][0][0]
                    .as_str()
                    .unwrap()
                    .trim_end()
                    .to_string(),
                "main"
            );
        }

        #[test]
        fn snapshot_keeps_wide_and_combining_text() {
            let mut m = Model::new(10, 2);
            m.feed("界e\u{301}X".as_bytes());
            let s = m.snapshot();
            let row_text = |snap: &Value| -> String {
                snap["lines"][0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|sp| sp[0].as_str().unwrap_or(""))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            };
            assert_eq!(
                row_text(&s),
                "界e\u{301}X",
                "wide continuation and combining stay"
            );
        }
    }
}

/// The client side: connect to the live host, spawning one on demand
/// (50 ms poll, 5 s budget per ADR §1). Frames are [`write_frame`] /
/// [`read_frame`] over the pipe.
#[cfg(windows)]
pub mod client {
    use std::io;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use std::sync::Mutex;

    use super::{MAX_PAGE_BYTES, PROTOCOL_VERSION, pipe_name, read_frame, write_frame};

    /// A connected host session.
    pub struct Client {
        pipe: DupHandle,
        id: u64,
        /// Protocol version negotiated in `initialize`.
        pub protocol: u32,
        /// Stream notifications buffered while waiting for an RPC reply.
        pub pending: Vec<Value>,
        /// Shared frame-write lock, held across the whole length-prefixed frame.
        pub lock: Arc<Mutex<u8>>,
    }

    /// Locate `rustty-host.exe` beside this executable.
    #[must_use]
    pub fn host_exe_path() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let host = dir.join("rustty-host.exe");
        host.exists().then_some(host)
    }

    #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
    fn spawn_host() -> io::Result<()> {
        let host = host_exe_path().ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "rustty-host.exe is not beside rustty.exe",
        ))?;
        // The host is a GUI-subsystem exe (no console flashes) and must
        // outlive us, so spawn it detached. `bInheritHandles=FALSE` is
        // required: PowerShell marks native-pipeline stdout pipe handles
        // inheritable, and a daemon holding one open keeps the reader
        // waiting for EOF forever (measured: `cmd | Out-Null` hangs).
        // Safety: FFI boundary (NFR-401); the buffers live through the
        // call and the child handles are closed.
        unsafe {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
            };
            use windows::core::{PCWSTR, PWSTR};

            let to_wide =
                |s: String| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
            let app = to_wide(host.display().to_string());
            let mut cmd = to_wide(format!("\"{}\" --serve", host.display()));
            let si = STARTUPINFOW {
                cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(0),
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessW(
                PCWSTR(app.as_ptr()),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                false,
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            CloseHandle(pi.hProcess).ok();
            CloseHandle(pi.hThread).ok();
        }
        Ok(())
    }

    /// Open the host pipe read+write, overlapped: the attach frontend
    /// reads on a dedicated thread while its main thread writes, and a
    /// blocking read on a synchronous handle starves a concurrent
    /// writer (see [`super::overlapped_write`]).
    #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
    pub fn open_pipe() -> io::Result<DupHandle> {
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
            OPEN_EXISTING,
        };

        let name = pipe_name();
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // Safety: FFI boundary (NFR-401); `wide` lives through the call.
        unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE::default(),
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0),
                Some(HANDLE::default()),
            )
            .map(DupHandle)
            // System HRESULTs carry the Win32 code in the low 16 bits
            // (connect()'s retry loop matches ERROR_FILE_NOT_FOUND = 2).
            .map_err(|e| io::Error::from_raw_os_error(i32::from(e.code().0 as u16)))
        }
    }

    impl Client {
        /// Connect to the live host without spawning one; error when no
        /// host answers (palette session list, issue #49). Drop the
        /// client to return the host's client count to its prior value.
        ///
        /// # Errors
        ///
        /// Pipe unavailability or handshake failure.
        pub fn connect_existing() -> io::Result<Self> {
            Self::handshake(open_pipe()?)
        }

        /// Connect to the live host, spawning one when none answers;
        /// 50 ms poll, 5 s budget (ADR §1 ready handshake).
        ///
        /// # Errors
        ///
        /// Pipe unavailability, spawn failure, handshake failure.
        pub fn connect() -> io::Result<Self> {
            if let Ok(pipe) = open_pipe() {
                return Self::handshake(pipe);
            }
            spawn_host()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match open_pipe() {
                    Ok(pipe) => return Self::handshake(pipe),
                    Err(e) if e.raw_os_error() == Some(2) && Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    // The pipe exists by now: the spawn timed out.
                    Err(e) if Instant::now() < deadline => return Err(e),
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "the session host did not become ready within 5s",
                        ));
                    }
                }
            }
        }

        fn handshake(pipe: DupHandle) -> io::Result<Self> {
            let mut me = Self {
                pipe,
                id: 0,
                protocol: 0,
                pending: Vec::new(),
                lock: Arc::new(Mutex::new(0)),
            };
            let resp = me.call("initialize", json!({"protocol": PROTOCOL_VERSION}))?;
            if let Some(err) = resp.get("error") {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!(
                        "{}: {}",
                        err.get("code").and_then(Value::as_str).unwrap_or("error"),
                        err.get("message").and_then(Value::as_str).unwrap_or("")
                    ),
                ));
            }
            me.protocol = resp
                .get("result")
                .and_then(|r| r.get("protocol"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            Ok(me)
        }

        /// One request/response round trip. The response envelope is
        /// returned whole; callers inspect `result` / `error`.
        ///
        /// # Errors
        ///
        /// Transport errors. Protocol-level errors come back in the
        /// envelope.
        /// Write one frame under the shared lock.
        pub fn write_locked(&mut self, req: &Value) -> io::Result<()> {
            let Ok(_guard) = self.lock.lock() else {
                return Err(io::Error::other("write lock failed"));
            };
            write_frame(&mut self.pipe, req)
        }

        /// One request/response round trip; intervening stream frames are
        /// buffered until the response id matches.
        pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
            self.id += 1;
            let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
            self.write_locked(&req)?;
            loop {
                let frame = read_frame(&mut self.pipe, MAX_PAGE_BYTES)?;
                let id = frame.get("id").and_then(Value::as_u64);
                if id == Some(self.id) {
                    return Ok(frame);
                }
                self.pending.push(frame.clone());
            }
        }

        /// Fire-and-forget request (the attach frontend's input/resize
        /// frames). The answer is ignored: it arrives on the reader
        /// thread as an unsolicited response and is dropped there.
        ///
        /// # Errors
        ///
        /// Transport errors.
        pub fn send(&mut self, method: &str, params: Value) -> io::Result<()> {
            self.id += 1;
            let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
            self.write_locked(&req)
        }

        /// A read handle for the connection, for a dedicated reader
        /// thread while this thread keeps writing.
        ///
        /// # Errors
        ///
        /// Handle duplication failure.
        #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
        pub fn reader(&self) -> io::Result<DupReader> {
            use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
            use windows::Win32::System::Threading::GetCurrentProcess;

            // Safety: duplicate our own pipe handle with the same access.
            let dup = unsafe {
                let mut h = HANDLE::default();
                DuplicateHandle(
                    GetCurrentProcess(),
                    self.pipe.0,
                    GetCurrentProcess(),
                    &mut h,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                h
            };
            Ok(DupReader {
                handle: std::sync::Arc::new(DupHandle(dup)),
            })
        }

        /// A write handle for the connection, so a reader thread can
        /// send (a resync request) while this thread keeps writing.
        ///
        /// # Errors
        ///
        /// Handle duplication failure.
        #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
        pub fn writer(&self) -> io::Result<DupWriter> {
            use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
            use windows::Win32::System::Threading::GetCurrentProcess;

            // Safety: duplicate our own pipe handle with the same access.
            let dup = unsafe {
                let mut h = HANDLE::default();
                DuplicateHandle(
                    GetCurrentProcess(),
                    self.pipe.0,
                    GetCurrentProcess(),
                    &mut h,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                h
            };
            Ok(DupWriter {
                handle: std::sync::Arc::new(DupHandle(dup)),
            })
        }
    }

    /// Sendable owner of a duplicated pipe handle.
    /// Safety: kernel HANDLE values are valid in any thread.
    #[cfg(windows)]
    pub struct DupHandle(windows::Win32::Foundation::HANDLE);
    #[cfg(windows)]
    unsafe impl Send for DupHandle {}
    // Safety: every I/O call uses its own event and completes before
    // the call returns; concurrent read (reader thread) + write (main
    // thread) on one pipe is the design (overlapped handles).
    #[cfg(windows)]
    unsafe impl Sync for DupHandle {}
    #[cfg(windows)]
    impl Drop for DupHandle {
        fn drop(&mut self) {
            // Safety: closing the handle we duplicated.
            unsafe {
                windows::Win32::Foundation::CloseHandle(self.0).ok();
            }
        }
    }

    /// `ReadFile` with an event + wait (the overlapped pattern; see
    /// [`super::overlapped_write`]). A zero-length result is EOF.
    #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
    fn overlapped_read(
        handle: windows::Win32::Foundation::HANDLE,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
        use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

        if buf.is_empty() {
            return Ok(0);
        }
        // Safety: event + OVERLAPPED live through the wait; `handle` is
        // a valid pipe handle opened with FILE_FLAG_OVERLAPPED.
        unsafe {
            let Ok(event) = CreateEventW(None, true, false, None) else {
                return Err(io::Error::last_os_error());
            };
            let mut ov = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let mut done = 0u32;
            let started = ReadFile(
                handle,
                Some(buf),
                Some(core::ptr::from_mut(&mut done)),
                Some(core::ptr::from_mut(&mut ov)),
            );
            let n = if started.is_ok() {
                done
            } else if GetLastError() == ERROR_IO_PENDING {
                WaitForSingleObject(event, INFINITE);
                GetOverlappedResult(handle, core::ptr::from_ref(&ov), &mut done, false)
                    .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                done
            } else {
                let code = GetLastError();
                CloseHandle(event).ok();
                return Err(io::Error::from_raw_os_error(code.0 as i32));
            };
            CloseHandle(event).ok();
            Ok(usize::try_from(n).unwrap_or(0))
        }
    }

    #[cfg(windows)]
    impl io::Read for DupHandle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            overlapped_read(self.0, buf)
        }
    }

    #[cfg(windows)]
    impl io::Write for DupHandle {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            super::overlapped_write(self.0, buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The connection's read side (blocking).
    #[cfg(windows)]
    pub struct DupReader {
        handle: std::sync::Arc<DupHandle>,
    }

    #[cfg(windows)]
    impl DupReader {
        /// The raw pipe handle, for `CancelIoEx` so a close can wake a
        /// blocking read (issue #49 view pump).
        #[must_use]
        pub fn handle(&self) -> windows::Win32::Foundation::HANDLE {
            self.handle.0
        }
    }

    #[cfg(windows)]
    impl io::Read for DupReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            overlapped_read(self.handle.0, buf)
        }
    }

    /// The connection's write side (blocking, overlapped).
    #[cfg(windows)]
    pub struct DupWriter {
        handle: std::sync::Arc<DupHandle>,
    }

    #[cfg(windows)]
    impl io::Write for DupWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            super::overlapped_write(self.handle.0, buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_aliases_persist_bindings() {
        let mut ids = IdsState::default();
        let (a1, i1) = ids.allocate();
        let (a2, i2) = ids.allocate();
        assert_eq!((a1, i1), (1, 1));
        assert_eq!((a2, i2), (2, 2));
        assert_eq!(ids.resolve(a1), Some(i1));
        // A reload keeps the allocator moving forward: no reuse.
        let dir = std::env::temp_dir().join(format!("rustty-host-ids-{}", std::process::id()));
        ids.save(&dir).expect("save");
        let mut re = IdsState::load(&dir).expect("load");
        let (a3, i3) = re.allocate();
        assert_eq!((a3, i3), (3, 3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_allocator_is_an_error_never_a_reset() {
        let dir = std::env::temp_dir().join(format!("rustty-host-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("ids.json"), b"not json").expect("write");
        assert!(IdsState::load(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frames_round_trip_and_capped_reads_reject() {
        let v = json!({"method": "session.list", "params": {"x": "y"}, "id": 7});
        let mut buf = Vec::new();
        write_frame(&mut buf, &v).expect("write");
        let got = read_frame(&mut buf.as_slice(), MAX_REQUEST_BYTES).expect("read");
        assert_eq!(got, v);
        // A declared length over the cap is refused without allocating.
        let mut big = (MAX_REQUEST_BYTES as u32 + 1).to_le_bytes().to_vec();
        big.extend_from_slice(b"{}");
        assert!(read_frame(&mut big.as_slice(), MAX_REQUEST_BYTES).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn takeover_demotes_the_old_controller_and_tells_them() {
        use std::sync::mpsc::sync_channel;

        use server::{App, Conn};

        let app = std::sync::Arc::new(std::sync::Mutex::new(App::new().expect("app")));
        let req = json!({"method": "session.create", "params": {"cols": 80, "rows": 24}, "id": 1});
        let (tx, _drain) = sync_channel(8);
        let conn = Conn { token: 1, tx: &tx };
        let created = server::handle(&app, &req, &conn);
        let alias = created["result"]["session"]["alias"]
            .as_u64()
            .expect("alias");

        // token 2 takes over from token 1 (both subscribed, as in #49).
        let (tx1, rx1) = sync_channel(64);
        let c1 = Conn { token: 2, tx: &tx1 };
        let v = json!({"method": "session.attach", "params": {"alias": alias}, "id": 2});
        assert!(server::handle(&app, &v, &c1)["result"]["controlGranted"].as_bool() == Some(true));
        let (tx2, rx2) = sync_channel(64);
        let c2 = Conn { token: 3, tx: &tx2 };
        let v = json!({
            "method": "session.attach", "id": 3,
            "params": {"alias": alias, "takeControl": true},
        });
        assert!(server::handle(&app, &v, &c2)["result"]["controlGranted"].as_bool() == Some(true));

        // The demoted controller gets a session.control frame in its queue.
        let mut saw_control = false;
        while let Ok(frame) = rx1.try_recv() {
            if frame["method"] == "session.control"
                && frame["params"]["controller"].as_bool() == Some(false)
            {
                assert_eq!(frame["params"]["alias"].as_u64(), Some(alias));
                saw_control = true;
            }
        }
        assert!(saw_control, "demoted client must be notified");

        // The superseded controller's input is refused; the new one's is not.
        let v = json!({
            "method": "session.input", "id": 4,
            "params": {"alias": alias, "data": "78"},
        });
        let resp = server::handle(&app, &v, &c1);
        assert_eq!(resp["error"]["code"].as_str(), Some("session-busy"));
        let resp = server::handle(&app, &v, &c2);
        assert!(
            resp.get("result").is_some(),
            "new controller's input accepted"
        );

        // The demoted controller's resize is refused too (issue #50).
        let v = json!({
            "method": "session.resize", "id": 5,
            "params": {"alias": alias, "cols": 100, "rows": 30},
        });
        let resp = server::handle(&app, &v, &c1);
        assert_eq!(resp["error"]["code"].as_str(), Some("session-busy"));
        drop(rx2);

        // The unified browser list carries controller/watcher counts.
        let v = json!({"method": "session.list", "id": 7});
        let binding = server::handle(&app, &v, &conn);
        let rec = &binding["result"]["sessions"].as_array().expect("list")[0];
        assert_eq!(rec["controllers"].as_u64(), Some(1), "one controller");
        assert_eq!(rec["watchers"].as_u64(), Some(1), "one demoted watcher");

        // Clean up the live session (kills the shell via the job).
        let v = json!({"method": "session.terminate", "params": {"alias": alias}, "id": 6});
        server::handle(&app, &v, &conn);
    }

    #[cfg(windows)]
    #[test]
    fn attach_statuses_are_explicit_and_never_respawn() {
        use std::sync::mpsc::sync_channel;

        use server::{App, Conn};

        let app = std::sync::Arc::new(std::sync::Mutex::new(App::new().expect("app")));
        let (tx, _drain) = sync_channel(8);
        let conn = Conn { token: 1, tx: &tx };

        // Unknown alias: explicit target-stale, never a silent spawn.
        let v = json!({"method": "session.attach", "params": {"alias": 999_999}, "id": 1});
        let resp = server::handle(&app, &v, &conn);
        assert_eq!(resp["error"]["code"].as_str(), Some("target-stale"));

        // Create and terminate a session, then attach to the exited one.
        let req = json!({"method": "session.create", "params": {"cols": 80, "rows": 24}, "id": 2});
        let alias = server::handle(&app, &req, &conn)["result"]["session"]["alias"]
            .as_u64()
            .expect("alias");
        let v = json!({"method": "session.terminate", "params": {"alias": alias}, "id": 3});
        server::handle(&app, &v, &conn);

        let v = json!({"method": "session.attach", "params": {"alias": alias}, "id": 4});
        let resp = server::handle(&app, &v, &conn);
        assert_eq!(resp["error"]["code"].as_str(), Some("target-stale"));

        // The record survives as "exited"; no live session was respawned.
        let v = json!({"method": "session.info", "params": {"alias": alias}, "id": 5});
        let rec = &server::handle(&app, &v, &conn)["result"]["session"];
        assert_eq!(rec["state"].as_str(), Some("exited"));
        assert!(rec.get("exitCode").is_some(), "exit code recorded");
        let v = json!({"method": "session.list", "id": 6});
        let sessions = server::handle(&app, &v, &conn)["result"]["sessions"]
            .as_array()
            .expect("list")
            .clone();
        assert_eq!(sessions.len(), 1, "exited record only, no respawn");
        assert_eq!(sessions[0]["state"].as_str(), Some("exited"));
    }

    #[cfg(windows)]
    #[test]
    fn exited_history_stays_bounded() {
        use server::App;

        let mut app = App::new().expect("app");
        for n in 0..(MAX_EXITED_RECORDS + 8) {
            let record = SessionRecord {
                id: n as u64,
                alias: n as u32,
                name: format!("s{n}"),
                profile: "default".to_string(),
                cwd: ".".to_string(),
                program: "shell".to_string(),
                cols: 80,
                rows: 24,
                pid: n as u32,
                persistent: true,
                state: "exited".to_string(),
                created_unix_secs: 0,
                exit_code: Some(0),
                task_id: None,
                project: None,
            };
            app.push_exited(record);
        }
        assert_eq!(app.exited.len(), MAX_EXITED_RECORDS, "ring stays bounded");
        // Oldest records fall off; the newest survive.
        assert!(app.find_record(0).is_none(), "oldest record evicted");
        assert!(
            app.find_record((MAX_EXITED_RECORDS + 7) as u32).is_some(),
            "newest record kept"
        );
    }

    #[cfg(windows)]
    #[test]
    fn protocol_negotiation_picks_the_min_and_rejects_the_unusable() {
        use std::sync::mpsc::sync_channel;

        use server::{App, Conn};

        let app = std::sync::Arc::new(std::sync::Mutex::new(App::new().expect("app")));
        let (tx, _drain) = sync_channel(8);
        let conn = Conn { token: 1, tx: &tx };

        // Newer client than host: negotiates down to the host's version.
        let v = json!({"method": "initialize", "params": {"protocol": 99}, "id": 1});
        let resp = server::handle(&app, &v, &conn);
        assert_eq!(resp["result"]["protocol"].as_u64(), Some(1));

        // Client speaks nothing the host knows: explicit mismatch, and the
        // CLI maps that to exit code 6 (attach never degrades silently).
        let v = json!({"method": "initialize", "params": {"protocol": 0}, "id": 2});
        let resp = server::handle(&app, &v, &conn);
        assert_eq!(resp["error"]["code"].as_str(), Some("protocol-mismatch"));
    }

    #[cfg(windows)]
    #[test]
    fn stress_repeated_attach_detach_keeps_the_session_live() {
        use std::sync::mpsc::sync_channel;

        use server::{App, Conn};

        let app = std::sync::Arc::new(std::sync::Mutex::new(App::new().expect("app")));
        let (tx, _drain) = sync_channel(8);
        let conn = Conn { token: 1, tx: &tx };
        let req = json!({"method": "session.create", "params": {"cols": 80, "rows": 24}, "id": 1});
        let alias = server::handle(&app, &req, &conn)["result"]["session"]["alias"]
            .as_u64()
            .expect("alias");
        let v = json!({"method": "session.info", "params": {"alias": alias}, "id": 2});
        let pid = server::handle(&app, &v, &conn)["result"]["session"]["pid"]
            .as_u64()
            .expect("pid");

        // 30 attach/reattach cycles from distinct clients (crash-reconnect
        // shape: each token takes control, detaches, next token reclaims).
        for n in 0..30u32 {
            let (atx, arx) = sync_channel(64);
            let ac = Conn {
                token: 1000 + u64::from(n),
                tx: &atx,
            };
            let v = json!({
                "method": "session.attach", "id": 3 + n,
                "params": {"alias": alias, "takeControl": true},
            });
            let resp = server::handle(&app, &v, &ac);
            assert!(
                resp["result"]["controlGranted"].as_bool() == Some(true),
                "cycle {n}: attach granted"
            );
            let v = json!({"method": "session.detach", "id": 100 + n});
            server::handle(&app, &v, &ac);
            drop(arx);
        }

        // Same shell process, still live: no respawn across 30 reconnects.
        let v = json!({"method": "session.info", "params": {"alias": alias}, "id": 999});
        let rec = &server::handle(&app, &v, &conn)["result"]["session"];
        assert_eq!(rec["pid"].as_u64(), Some(pid), "shell pid preserved");
        assert_eq!(rec["state"].as_str(), Some("live"));

        let v = json!({"method": "session.terminate", "params": {"alias": alias}, "id": 1000});
        server::handle(&app, &v, &conn);
    }
    #[cfg(windows)]
    #[test]
    fn explicit_program_and_args_create_session() {
        use std::sync::mpsc::sync_channel;

        use server::{App, Conn};

        let app = std::sync::Arc::new(std::sync::Mutex::new(App::new().expect("app")));
        let (tx, _drain) = sync_channel(8);
        let conn = Conn { token: 1, tx: &tx };
        let req = json!({
            "method": "session.create",
            "params": {
                "cols": 80, "rows": 24,
                "program": "cmd.exe",
                "args": ["/c", "echo", "task"],
                "name": "task#1 dev",
                "persistent": true,
            },
            "id": 1,
        });
        let resp = server::handle(&app, &req, &conn);
        let rec = &resp["result"]["session"];
        assert_eq!(
            rec["program"].as_str(),
            Some("cmd.exe"),
            "explicit program reaches the host"
        );
        let Some(alias) = rec["alias"].as_u64() else {
            panic!("create returned no alias");
        };
        let v = json!({"method": "session.terminate", "params": {"alias": alias}, "id": 2});
        server::handle(&app, &v, &conn);
    }
}
