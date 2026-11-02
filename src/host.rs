//! Persistent session host (issue #46, docs/design-session-host.md).
//!
//! `rusttyhost.exe` owns managed ConPTY sessions per user, independently
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

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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

/// Pipe name base; the full name is `\\.\pipe\rusttyhost.<user>`.
/// The user suffix keeps two accounts on one machine apart; the real
/// boundary is the pipe DACL (current user only), not the name.
#[must_use]
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .map(|u| u.to_ascii_lowercase())
        .unwrap_or_else(|_| "user".to_string());
    format!(r"\\.\pipe\rusttyhost.{user}")
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
        match std::fs::read(dir.join("ids.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
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
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("ids.json.tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, bytes)?;
        let dst = dir.join("ids.json");
        std::fs::remove_file(&dst).ok();
        std::fs::rename(&tmp, dst)
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
/// RPC dispatch table. The `rusttyhost.exe` bin drives it; clients
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
    use crate::shell::{SpawnOptions, probe_shell};

    use super::{
        GRACEFUL_WAIT_MS, IdsState, MAX_EXITED_RECORDS, OPS, PROTOCOL_VERSION, SCHEMA,
        SessionRecord, error, host_state_dir, response,
    };

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
    }

    /// Shared host state. One `Mutex` guards it; requests are short.
    /// shortcut: terminate holds the lock up to GRACEFUL_WAIT_MS,
    /// upgrade when concurrent clients need terminate not to block them.
    pub struct App {
        ids: IdsState,
        ids_dir: std::path::PathBuf,
        cfg: Config,
        live: BTreeMap<u32, Managed>,
        exited: VecDeque<SessionRecord>,
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

        fn save_ids(&mut self) {
            // A failed persist is not fatal: in-memory ids keep moving
            // forward; the next save retries.
            self.ids.save(&self.ids_dir).ok();
        }

        fn records(&self) -> Vec<&SessionRecord> {
            let mut all: Vec<&SessionRecord> = self.exited.iter().collect();
            all.extend(self.live.values().map(|m| &m.record));
            all.sort_by_key(|r| r.alias);
            all
        }

        fn find_record(&self, alias: u32) -> Option<&SessionRecord> {
            self.live
                .get(&alias)
                .map(|m| &m.record)
                .or_else(|| self.exited.iter().find(|r| r.alias == alias))
        }

        fn push_exited(&mut self, record: SessionRecord) {
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
            g.save_ids();
            // Self-attach prevention (attach ADR): a shell started by the
            // host knows its alias and refuses to attach to its own session.
            opts.env
                .insert("RUSTTYTERM_SESSION".into(), alias.to_string());
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
            let managed = Managed {
                record: record.clone(),
                conpty,
                process: managed_process,
                job,
                subs: Vec::new(),
                controller: None,
                _reader: std::thread::spawn(move || {
                    let on_chunk = |chunk: &[u8]| {
                        // Answer the cursor-position query so the shell's
                        // input loop unblocks. shortcut: fixed 1;1 report
                        // (readline only needs a valid one); emulate
                        // queries against the attached console with #48.
                        if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                            let _ = query_writer.write(b"\x1b[1;1R");
                        }
                        // Hex keeps JSON frames pure ASCII; the client
                        // decodes and writes the bytes to its console.
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        let mut hex = String::with_capacity(chunk.len() * 2);
                        for b in chunk {
                            hex.push(HEX[usize::from(b >> 4)] as char);
                            hex.push(HEX[usize::from(b & 0xf)] as char);
                        }
                        if let Ok(mut app) = app_for_reader.lock()
                            && let Some(m) = app.live.get_mut(&alias)
                            && m.record.id == id
                        {
                            let (cols, rows) = (m.record.cols, m.record.rows);
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "session.output",
                                "params": {
                                    "alias": alias,
                                    "id": id,
                                    "data": hex,
                                    "cols": cols,
                                    "rows": rows,
                                    "schema": SCHEMA,
                                },
                            });
                            m.subs.retain(|(_, tx)| tx.try_send(note.clone()).is_ok());
                        }
                        // try_send fails only when a client is full or
                        // gone; the slow client is dropped (its pump
                        // closes the connection).
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

    /// Report a session exit (reader thread -> App). Only the reader of
    /// the session's *current* incarnation marks it; a terminated one
    /// was already recorded.
    fn exit_notifier(
        app: &Arc<Mutex<App>>,
        alias: u32,
        id: u64,
    ) -> Box<dyn Fn(Option<u32>) + Send + 'static> {
        let app = Arc::clone(app);
        Box::new(move |code| {
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
                }
                // Dropping Managed closes conpty + job: the tree goes
                // with it (the shell exited on its own by now).
            }
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
    /// The attach snapshot is the output from now on — scrollback replay
    /// is #48. shortcut: no replay, add it with the delta model.
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
        let mut g = app
            .lock()
            .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
        let m = g.live.get_mut(&alias).ok_or_else(|| {
            Fail::Code(
                "target-stale",
                format!("no live session with alias {alias}"),
            )
        })?;
        if !readonly && m.controller.is_some() && m.controller != Some(conn.token) && !takeover {
            return Err(Fail::Code(
                "session-busy",
                format!("session {alias} is controlled by another client (use --take-control)"),
            ));
        }
        m.subs.push((conn.token, conn.tx.clone()));
        let granted = if readonly {
            false
        } else {
            // takeover silently demotes the previous controller (ADR).
            m.controller = Some(conn.token);
            true
        };
        Ok(json!({
            "alias": alias,
            "id": m.record.id,
            "pid": m.record.pid,
            "cols": m.record.cols,
            "rows": m.record.rows,
            "state": "live",
            "controlGranted": granted,
            "schema": SCHEMA,
        }))
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
        let note = json!({
            "jsonrpc": "2.0",
            "method": "session.resize",
            "params": { "alias": alias, "id": m.record.id, "cols": cols, "rows": rows, "schema": SCHEMA },
        });
        m.subs.retain(|(_, tx)| tx.try_send(note.clone()).is_ok());
        Ok(json!({ "cols": cols, "rows": rows }))
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
                        "host": "rusttyhost",
                        "version": env!("CARGO_PKG_VERSION"),
                        "ops": OPS,
                    }))
                }
            }
            "session.create" => App::create_session(app, &params)
                .map(|r| json!({"schema": SCHEMA, "session": r.json()})),
            "session.list" => {
                let g = app
                    .lock()
                    .map_err(|_| Fail::Code("internal", "state lock poisoned".to_string()))?;
                Ok(
                    json!({"schema": SCHEMA, "sessions": g.records().iter().map(|r| r.json()).collect::<Vec<_>>() }),
                )
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
}

/// The client side: connect to the live host, spawning one on demand
/// (50 ms poll, 5 s budget per ADR §1). Frames are [`write_frame`] /
/// [`read_frame`] over the pipe.
#[cfg(windows)]
pub mod client {
    use std::io;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::{MAX_PAGE_BYTES, PROTOCOL_VERSION, pipe_name, read_frame, write_frame};

    /// A connected host session.
    pub struct Client {
        pipe: DupHandle,
        id: u64,
        /// Protocol version negotiated in `initialize`.
        pub protocol: u32,
    }

    /// Locate `rusttyhost.exe` beside this executable.
    #[must_use]
    pub fn host_exe_path() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let host = dir.join("rusttyhost.exe");
        host.exists().then_some(host)
    }

    #[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
    fn spawn_host() -> io::Result<()> {
        let host = host_exe_path().ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "rusttyhost.exe is not beside rusttyterm.exe",
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
    fn open_pipe() -> io::Result<DupHandle> {
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
        pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
            self.id += 1;
            let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
            write_frame(&mut self.pipe, &req)?;
            read_frame(&mut self.pipe, MAX_PAGE_BYTES)
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
            write_frame(&mut self.pipe, &req)
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
    impl io::Read for DupReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            overlapped_read(self.handle.0, buf)
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
        let dir = std::env::temp_dir().join(format!("rusttyhost-ids-{}", std::process::id()));
        ids.save(&dir).expect("save");
        let mut re = IdsState::load(&dir).expect("load");
        let (a3, i3) = re.allocate();
        assert_eq!((a3, i3), (3, 3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_allocator_is_an_error_never_a_reset() {
        let dir = std::env::temp_dir().join(format!("rusttyhost-bad-{}", std::process::id()));
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
}
