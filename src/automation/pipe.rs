//! The per-window automation pipe (ADR §2, AUTO-003/004): a server the window
//! owns, and the client end the encoders use. Same pipe security, identity check,
//! framing and overlapped I/O as the session host, from [`crate::host`].
//!
//! Threads: one accepts connections, and each connection has a reader and a
//! writer. The window never touches the pipe. It takes requests from a bounded
//! per-client inbox and puts frames in a bounded per-client [`Outbox`], neither
//! of which can make it wait on a client. A client that stops reading stalls its
//! own writer thread, then overflows its outbox and is cut off (`slow-client`).

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
use windows::Win32::System::IO::CancelIoEx;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::client::{CALL_TIMEOUT, Transport};
use super::ops::ClientId;
use super::{
    CLIENT_INBOX_CAP, DiscoveryEntry, ErrorCode, MAX_CLIENTS, MAX_PAGE_BYTES, MAX_REQUEST_BYTES,
    OUTBOUND_CAP, Outbox, Push, RpcError, Token, entry_problem, error_frame, load_discovery,
    pipe_name, write_discovery,
};
use crate::host::client::{DupHandle, open_named_pipe};
use crate::host::pipe::{ClientPipe, connect, create_instance};
use crate::host::server::PipeSecurity;
use crate::host::{encode_frame, overlapped_read, overlapped_write, read_frame, write_frame};

/// How long a cut-off client's stuck write gets before it is cancelled.
const CUT_OFF_GRACE: Duration = Duration::from_millis(250);
/// How long closing a window waits for replies already queued to go out.
const SHUTDOWN_FLUSH: Duration = Duration::from_millis(250);
/// How long a connection gets to send `initialize`, so connections that say nothing
/// can't hold all of [`MAX_CLIENTS`]. After that a quiet client may stay: it might be
/// waiting on events.
pub const INITIALIZE_DEADLINE: Duration = Duration::from_secs(5);
/// How long a client keeps trying while every pipe instance is busy.
const OPEN_BUSY_WAIT: Duration = Duration::from_secs(2);
/// `ERROR_PIPE_BUSY`.
const PIPE_BUSY: i32 = 231;
/// `STILL_ACTIVE`, the exit code of a process that hasn't exited.
const STILL_ACTIVE: u32 = 259;

static WINDOW_SEQ: AtomicU64 = AtomicU64::new(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // These hold plain queues: a panicked thread leaves nothing half-done that
    // matters more than keeping the terminal's automation alive.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A new write token from the OS's random source, or `None` if it failed: then
/// the window has no token rather than a guessable one (AUTO-007).
#[must_use]
pub fn new_token() -> Option<Token> {
    let mut bytes = [0u8; 32];
    // Safety: the buffer is valid for its whole length, and the flag selects the
    // system RNG, which takes no algorithm handle.
    let status = unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    status.is_ok().then(|| Token::from_random_bytes(&bytes))
}

enum ProcessProbe {
    Gone,
    Unknown,
    Started(u64),
}

fn probe_process(pid: u32) -> ProcessProbe {
    // Safety: a short-lived query handle, closed below; the out-params are real.
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) if e.code() == ERROR_INVALID_PARAMETER.to_hresult() => {
                return ProcessProbe::Gone;
            }
            // It exists and we may not look: an elevated window, say.
            Err(_) => return ProcessProbe::Unknown,
        };
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE;
        let mut created = FILETIME::default();
        let mut unused = FILETIME::default();
        let mut unused2 = FILETIME::default();
        let mut unused3 = FILETIME::default();
        let times = GetProcessTimes(
            handle,
            &mut created,
            &mut unused,
            &mut unused2,
            &mut unused3,
        );
        CloseHandle(handle).ok();
        if !running {
            return ProcessProbe::Gone;
        }
        if times.is_err() {
            return ProcessProbe::Unknown;
        }
        ProcessProbe::Started(
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        )
    }
}

/// When a process started, `None` if it can't be told.
#[must_use]
pub fn process_start_time(pid: u32) -> Option<u64> {
    match probe_process(pid) {
        ProcessProbe::Started(time) => Some(time),
        _ => None,
    }
}

/// Whether a discovery file's window could still be running: its process exists
/// and is the same process, not a later one given the same pid. When that can't
/// be checked the file stays; a client won't trust it (`entry_problem`) and the
/// connection's identity check still applies.
#[must_use]
pub fn alive(entry: &DiscoveryEntry) -> bool {
    match probe_process(entry.pid) {
        ProcessProbe::Gone => false,
        ProcessProbe::Unknown => true,
        ProcessProbe::Started(time) => entry.start_time == 0 || time == entry.start_time,
    }
}

struct Client {
    id: ClientId,
    pipe: ClientPipe,
    inbox: Mutex<VecDeque<Value>>,
    inbox_space: Condvar,
    out: Mutex<Outbox>,
    out_ready: Condvar,
    /// A frame has left the outbox and isn't fully written yet.
    writing: AtomicBool,
    done: AtomicBool,
    cut_at: Mutex<Option<Instant>>,
    connected_at: Instant,
    /// It has sent `initialize`, whether or not that went on to succeed.
    initialize_seen: AtomicBool,
}

impl Client {
    fn new(id: ClientId, pipe: ClientPipe) -> Self {
        Self {
            id,
            pipe,
            inbox: Mutex::new(VecDeque::new()),
            inbox_space: Condvar::new(),
            out: Mutex::new(Outbox::new(OUTBOUND_CAP)),
            out_ready: Condvar::new(),
            writing: AtomicBool::new(false),
            done: AtomicBool::new(false),
            cut_at: Mutex::new(None),
            connected_at: Instant::now(),
            initialize_seen: AtomicBool::new(false),
        }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Stop serving this client: drop its backlog, say this frame last, and let
    /// its writer finish up. A write stuck on a client that isn't reading gets
    /// [`CUT_OFF_GRACE`], then [`Server::reap`] cancels it.
    fn cut_off(&self, last: Option<Vec<u8>>) {
        {
            let mut out = lock(&self.out);
            out.close();
            if let Some(last) = last {
                out.set_final(last);
            }
        }
        let mut cut_at = lock(&self.cut_at);
        cut_at.get_or_insert_with(Instant::now);
        self.out_ready.notify_all();
    }

    /// The connection is over: forget its queues and wake both its threads.
    /// Queued requests from a client that left never run (AUTO-009).
    fn finish(&self, shared: &Shared) {
        if !self.done.swap(true, Ordering::AcqRel) {
            lock(&shared.gone).push(self.id);
        }
        lock(&self.out).close();
        lock(&self.inbox).clear();
        self.out_ready.notify_all();
        self.inbox_space.notify_all();
        self.pipe.cancel_io();
    }

    fn flushed(&self) -> bool {
        self.is_done() || (lock(&self.out).is_empty() && !self.writing.load(Ordering::Acquire))
    }
}

struct Shared {
    stop: AtomicBool,
    clients: Mutex<Vec<Arc<Client>>>,
    gone: Mutex<Vec<ClientId>>,
    turn: AtomicUsize,
    next_client: AtomicU64,
    /// [`INITIALIZE_DEADLINE`], which tests shorten.
    initialize_deadline: Mutex<Duration>,
}

impl Shared {
    /// Cut off connections that have said nothing useful for too long.
    fn expire_uninitialized(&self) {
        let deadline = *lock(&self.initialize_deadline);
        for client in lock(&self.clients).iter() {
            if client.is_done()
                || client.initialize_seen.load(Ordering::Acquire)
                || client.connected_at.elapsed() < deadline
                || lock(&client.cut_at).is_some()
            {
                continue;
            }
            let error = RpcError::new(
                ErrorCode::Timeout,
                "this connection did not send initialize in time",
            );
            client.cut_off(encode_frame(&error_frame(Value::Null, &error)).ok());
        }
    }
}

fn read_loop(client: &Client, shared: &Shared) {
    while !client.is_done() {
        match read_frame(&mut &client.pipe, MAX_REQUEST_BYTES) {
            Ok(request) => {
                if request.get("method").and_then(Value::as_str) == Some("initialize") {
                    client.initialize_seen.store(true, Ordering::Release);
                }
                let mut inbox = lock(&client.inbox);
                // Full: stop reading, and the pipe pushes back on this client alone.
                while inbox.len() >= CLIENT_INBOX_CAP && !client.is_done() {
                    inbox = client
                        .inbox_space
                        .wait_timeout(inbox, Duration::from_millis(100))
                        .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
                }
                if client.is_done() {
                    break;
                }
                inbox.push_back(request);
            }
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                // Framing can't be trusted past this: answer, then hang up.
                let error = RpcError::bad_params(
                    "the request is over the 64 KiB frame cap, nested too deeply or not valid JSON",
                );
                client.cut_off(encode_frame(&error_frame(Value::Null, &error)).ok());
                return;
            }
            Err(_) => break,
        }
    }
    client.finish(shared);
}

fn write_loop(client: &Client, shared: &Shared) {
    loop {
        let frame = {
            let mut out = lock(&client.out);
            loop {
                if let Some(frame) = out.pop() {
                    client.writing.store(true, Ordering::Release);
                    break Some(frame);
                }
                if out.is_closed() || client.is_done() {
                    break None;
                }
                out = client
                    .out_ready
                    .wait_timeout(out, Duration::from_millis(200))
                    .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
            }
        };
        let Some(frame) = frame else { break };
        let written = (&client.pipe).write_all(&frame);
        client.writing.store(false, Ordering::Release);
        if written.is_err() {
            break;
        }
    }
    client.finish(shared);
}

fn serve_connection(shared: &Arc<Shared>, pipe: ClientPipe) {
    let id = shared.next_client.fetch_add(1, Ordering::Relaxed);
    let client = Arc::new(Client::new(id, pipe));
    let spawn = |name: &str, body: fn(&Client, &Shared)| {
        let client = Arc::clone(&client);
        let shared = Arc::clone(shared);
        std::thread::Builder::new()
            .name(name.to_string())
            .stack_size(256 * 1024)
            .spawn(move || body(&client, &shared))
    };
    lock(&shared.clients).push(Arc::clone(&client));
    if spawn("rustty-automation-read", read_loop).is_err()
        || spawn("rustty-automation-write", write_loop).is_err()
    {
        client.finish(shared);
    }
}

/// What the accept thread reports once the pipe exists.
type Ready = io::Result<Option<PathBuf>>;

fn accept_loop(
    shared: &Arc<Shared>,
    name: &str,
    entry: &DiscoveryEntry,
    discovery: Option<PathBuf>,
    ready: &Sender<Ready>,
) {
    let mut security = match PipeSecurity::for_current_user() {
        Ok(security) => security,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // Refuses a name something else already holds (AUTO-004).
    let mut current = match create_instance(name, &mut security, true) {
        Ok(pipe) => pipe,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let path = discovery.and_then(|dir| {
        let _ = load_discovery(&dir, &alive);
        write_discovery(&dir, entry).ok()
    });
    if ready.send(Ok(path)).is_err() {
        return;
    }
    while !shared.stop.load(Ordering::Acquire) {
        match connect(current.raw()) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(_) => {
                // Replace the instance; the next one is made before this closes
                // so the name is never free for something else to take.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        // The next listener exists before this one is handed off, for the same reason.
        let next = loop {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            match create_instance(name, &mut security, false) {
                Ok(next) => break next,
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        };
        let accepted = std::mem::replace(&mut current, next);
        // Make room first, so silent connections can't turn this one away.
        shared.expire_uninitialized();
        let serving = lock(&shared.clients)
            .iter()
            .filter(|client| !client.is_done())
            .count();
        // A refused peer is dropped here, which disconnects it (ADR §2).
        if security.peer_ok(accepted.raw()) && serving < MAX_CLIENTS {
            serve_connection(shared, accepted);
        }
    }
}

/// One window's automation server. Dropping it removes the discovery file, gives
/// queued replies a moment to go out, and disconnects everyone.
pub struct Server {
    shared: Arc<Shared>,
    discovery_file: Option<PathBuf>,
}

impl Server {
    /// Start serving window `id` and publish it in `discovery`.
    ///
    /// # Errors
    ///
    /// The pipe could not be created. The window carries on without automation.
    pub fn start(id: u64, quick: bool, discovery: Option<PathBuf>) -> io::Result<Self> {
        let pid = std::process::id();
        let seq = WINDOW_SEQ.fetch_add(1, Ordering::Relaxed);
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        let name = pipe_name(&user, pid, seq);
        let entry = DiscoveryEntry {
            id,
            pipe: name.clone(),
            pid,
            start_time: process_start_time(pid).unwrap_or(0),
            quick,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            clients: Mutex::new(Vec::new()),
            gone: Mutex::new(Vec::new()),
            turn: AtomicUsize::new(0),
            next_client: AtomicU64::new(1),
            initialize_deadline: Mutex::new(INITIALIZE_DEADLINE),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("rustty-automation".to_string())
            .stack_size(256 * 1024)
            .spawn(move || accept_loop(&thread_shared, &name, &entry, discovery, &ready_tx))?;
        let discovery_file = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(io::Error::other)??;
        Ok(Self {
            shared,
            discovery_file,
        })
    }

    fn client(&self, id: ClientId) -> Option<Arc<Client>> {
        lock(&self.shared.clients)
            .iter()
            .find(|client| client.id == id)
            .cloned()
    }

    /// The next request `allow` lets through, taking turns between clients so a
    /// busy one can't starve the rest. A held request also holds everything
    /// behind it from that client, so one client's requests never reorder.
    pub fn next_request(&self, allow: &dyn Fn(&Value) -> bool) -> Option<(ClientId, Value)> {
        let clients = lock(&self.shared.clients);
        let count = clients.len();
        let start = self.shared.turn.load(Ordering::Relaxed);
        for offset in 0..count {
            let index = (start + offset) % count;
            let client = &clients[index];
            if client.is_done() {
                continue;
            }
            let mut inbox = lock(&client.inbox);
            if inbox.front().is_some_and(allow) {
                let request = inbox.pop_front()?;
                client.inbox_space.notify_one();
                self.shared.turn.store(index + 1, Ordering::Relaxed);
                return Some((client.id, request));
            }
        }
        None
    }

    /// Queue a frame for a client. Never waits on the client: past its 4 MiB
    /// cap it is cut off with `slow-client` instead.
    pub fn send(&self, client: ClientId, frame: &Value) {
        let Some(client) = self.client(client) else {
            return;
        };
        let bytes = encode_frame(frame).or_else(|_| {
            let id = frame.get("id").cloned().unwrap_or(Value::Null);
            let error = RpcError::new(ErrorCode::Internal, "the reply was over the page cap");
            encode_frame(&error_frame(id, &error))
        });
        let Ok(bytes) = bytes else { return };
        let pushed = lock(&client.out).push(bytes);
        match pushed {
            Push::Queued => client.out_ready.notify_all(),
            Push::Closed => {}
            Push::Overflow => {
                let error = RpcError::new(
                    ErrorCode::SlowClient,
                    "this client read too slowly and was disconnected",
                );
                client.cut_off(encode_frame(&error_frame(Value::Null, &error)).ok());
            }
        }
    }

    /// The clients that have left since the last call.
    #[must_use]
    pub fn take_gone(&self) -> Vec<ClientId> {
        std::mem::take(&mut *lock(&self.shared.gone))
    }

    /// Housekeeping for the window's tick. Never waits: cancels writes stuck on
    /// clients being cut off, wakes threads of finished connections and forgets
    /// connections whose threads have ended, which closes their handles.
    pub fn reap(&self) {
        self.shared.expire_uninitialized();
        let mut clients = lock(&self.shared.clients);
        for client in clients.iter() {
            let overdue = lock(&client.cut_at).is_some_and(|at| at.elapsed() >= CUT_OFF_GRACE);
            if overdue || client.is_done() {
                client.pipe.cancel_io();
            }
        }
        clients.retain(|client| !client.is_done() || Arc::strong_count(client) > 1);
    }

    /// Shorten [`INITIALIZE_DEADLINE`] for a test.
    #[cfg(test)]
    pub(crate) fn set_initialize_deadline(&self, deadline: Duration) {
        *lock(&self.shared.initialize_deadline) = deadline;
    }

    /// Connections being served.
    #[must_use]
    pub fn client_count(&self) -> usize {
        lock(&self.shared.clients)
            .iter()
            .filter(|client| !client.is_done())
            .count()
    }

    /// Whether any request is waiting.
    #[must_use]
    pub fn has_queued(&self) -> bool {
        lock(&self.shared.clients)
            .iter()
            .any(|client| !lock(&client.inbox).is_empty())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(path) = &self.discovery_file {
            let _ = std::fs::remove_file(path);
        }
        // The reply to the `tab.close` that closed this window is in here.
        let start = Instant::now();
        while start.elapsed() < SHUTDOWN_FLUSH
            && !lock(&self.shared.clients).iter().all(|c| c.flushed())
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        // The accept thread can outlive this by a heartbeat, so let go of the
        // connections here: their handles close as their threads end.
        let clients = std::mem::take(&mut *lock(&self.shared.clients));
        for client in &clients {
            client.finish(&self.shared);
        }
    }
}

/// Reads frames off a client handle.
struct ReadEnd<'a>(&'a DupHandle);

impl io::Read for ReadEnd<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        overlapped_read(self.0.raw(), buf)
    }
}

/// Writes frames to a client handle.
struct WriteEnd<'a>(&'a DupHandle);

impl Write for WriteEnd<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        overlapped_write(self.0.raw(), buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The client end of a window's pipe. A thread reads frames as they arrive, so a
/// stream of events can't back up while the caller is busy.
pub struct PipeTransport {
    pipe: Arc<DupHandle>,
    frames: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
    pending: VecDeque<Value>,
    next_id: u64,
    timeout: Duration,
}

fn connection_ended() -> RpcError {
    RpcError::new(ErrorCode::TargetClosed, "the window closed the connection")
}

/// Open a window's pipe and check who is on the other end before saying anything
/// to it (AUTO-004). The security flags stop a server impersonating this client.
///
/// # Errors
///
/// `no-instance` when the entry can't be trusted, the window is gone or the pipe
/// isn't really its.
pub fn open(entry: &DiscoveryEntry) -> Result<PipeTransport, RpcError> {
    let flags = SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0;
    let unreachable = |why: String| {
        RpcError::new(
            ErrorCode::NoInstance,
            format!("window {} can't be reached: {why}", entry.id),
        )
    };
    // Before the pipe is touched: opening a name is itself something a server sees.
    if let Some(problem) = entry_problem(entry, process_start_time(entry.pid)) {
        return Err(unreachable(problem.to_string()));
    }
    let start = Instant::now();
    let pipe = loop {
        match open_named_pipe(&entry.pipe, flags) {
            Ok(pipe) => break pipe,
            Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && start.elapsed() < OPEN_BUSY_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(unreachable(e.to_string())),
        }
    };
    let security = PipeSecurity::for_current_user().map_err(|e| unreachable(e.to_string()))?;
    if !security.server_ok(pipe.raw(), entry.pid) {
        return Err(unreachable(
            "the process on that pipe is not the window it claims to be".to_string(),
        ));
    }
    let pipe = Arc::new(pipe);
    let (tx, frames) = mpsc::channel();
    let read_side = Arc::clone(&pipe);
    let reader = std::thread::Builder::new()
        .name("rustty-automation-client".to_string())
        .spawn(move || {
            while let Ok(frame) = read_frame(&mut ReadEnd(&read_side), MAX_PAGE_BYTES) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| unreachable(e.to_string()))?;
    Ok(PipeTransport {
        pipe,
        frames,
        reader: Some(reader),
        pending: VecDeque::new(),
        next_id: 1,
        timeout: CALL_TIMEOUT,
    })
}

impl PipeTransport {
    /// How long [`Transport::call`] waits for an answer.
    pub const fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn write_request(&mut self, method: &str, params: Value) -> io::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        write_frame(&mut WriteEnd(&self.pipe), &request)?;
        Ok(id)
    }

    /// Sort an incoming frame. `Some` for what the caller was waiting on: an
    /// answer to `id`, or an error the server sent with no id (`slow-client`).
    fn sort_frame(&mut self, frame: Value, id: Option<u64>) -> Option<Result<Value, RpcError>> {
        if frame.get("method").and_then(Value::as_str) == Some("event") {
            self.pending
                .push_back(frame.get("params").cloned().unwrap_or(Value::Null));
            return None;
        }
        let reply_id = frame.get("id").and_then(Value::as_u64);
        let unattributed = frame.get("id").is_none_or(Value::is_null);
        if reply_id.is_some() && reply_id == id {
            return Some(match frame.get("error") {
                Some(error) => Err(RpcError::from_value(error)),
                None => Ok(frame.get("result").cloned().unwrap_or(Value::Null)),
            });
        }
        if unattributed && let Some(error) = frame.get("error") {
            return Some(Err(RpcError::from_value(error)));
        }
        // The late answer to a call that already timed out.
        None
    }

    #[cfg(test)]
    fn send_only(&mut self, method: &str, params: Value) -> io::Result<u64> {
        self.write_request(method, params)
    }

    #[cfg(test)]
    fn write_raw(&self, bytes: &[u8]) -> io::Result<()> {
        WriteEnd(&self.pipe).write_all(bytes)
    }
}

impl Transport for PipeTransport {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self
            .write_request(method, params)
            .map_err(|_| connection_ended())?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(wait) {
                Ok(frame) => {
                    if let Some(answer) = self.sort_frame(frame, Some(id)) {
                        return answer;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(RpcError::new(
                        ErrorCode::Timeout,
                        format!("the window did not answer {method} in time"),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(connection_ended()),
            }
        }
    }

    fn next_event(&mut self, wait: Duration) -> Result<Option<Value>, RpcError> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(left) {
                Ok(frame) => {
                    if let Some(Err(error)) = self.sort_frame(frame, None) {
                        return Err(error);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => return Err(connection_ended()),
            }
        }
    }
}

impl Drop for PipeTransport {
    /// Hang up now, so the window drops any requests still queued from us. The
    /// reader holds the handle open, so wake it, repeatedly in case it was
    /// between reads when the first cancel went out.
    fn drop(&mut self) {
        let start = Instant::now();
        while let Some(reader) = &self.reader {
            if reader.is_finished() || start.elapsed() > Duration::from_millis(200) {
                break;
            }
            // Safety: the handle is valid while `self.pipe` lives. With no
            // OVERLAPPED this cancels what is pending and fails harmlessly if
            // nothing is.
            let _ = unsafe { CancelIoEx(self.pipe.raw(), None) };
            std::thread::sleep(Duration::from_millis(1));
        }
        self.reader = None;
    }
}

#[cfg(test)]
mod tests;
