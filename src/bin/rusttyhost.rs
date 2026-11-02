//! `rusttyhost.exe` (issue #46, docs/design-session-host.md): the
//! per-user persistent session host. One instance at a time, bound by
//! creating the named pipe: a second instance probes the pipe, finds a
//! live host, and exits 0. Clients (`rusttyterm` session verbs, the
//! automation API later) connect, speak framed JSON (see
//! `rusttyterm::host`), and their death never kills a host-owned shell
//! (every session lives in its own kill-on-close job object).

// GUI subsystem: the host must never flash a console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use std::io;

#[cfg(windows)]
use rusttyterm::host::server::{App, PipeSecurity};

fn main() -> std::process::ExitCode {
    serve()
}

/// The whole host: non-Windows builds have nothing to host.
#[cfg(not(windows))]
fn serve() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}

#[cfg(windows)]
fn serve() -> std::process::ExitCode {
    use rusttyterm::host::pipe_name;

    // A console only exists when the user started the host by hand;
    // when spawned by a client, the client's redirected handles (if
    // any) must survive so host diagnostics land where the launcher
    // pointed them.
    rusttyterm::cli::attach_parent_console();

    let app = match App::new() {
        Ok(app) => std::sync::Arc::new(std::sync::Mutex::new(app)),
        Err(e) => {
            eprintln!("rusttyhost: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut security = match PipeSecurity::for_current_user() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rusttyhost: cannot build pipe security: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let name = pipe_name();
    match bind(&name, &mut security) {
        Ok(first) => serve_loop(&app, &mut security, &name, first),
        Err(HostExit::Taken) => std::process::ExitCode::SUCCESS,
        Err(HostExit::Failed(e)) => {
            eprintln!("rusttyhost: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Accept loop. `first` is the instance `bind` created: it is the first
/// connection target, and each new client gets a fresh instance created
/// after the previous one connects (the client's open binds to the
/// oldest listening instance, so the rotation order matters). A
/// `ConnectNamedPipe` timeout is not fatal: it's the beat at which the
/// idle-exit budget is checked.
#[cfg(windows)]
fn serve_loop(
    app: &std::sync::Arc<std::sync::Mutex<App>>,
    security: &mut PipeSecurity,
    name: &str,
    first: ClientPipe,
) -> std::process::ExitCode {
    use std::sync::atomic::Ordering;

    use rusttyterm::host::IDLE_EXIT_SECS;

    let mut current = first;
    loop {
        // Idle exit (ADR §1): no clients, no live sessions, budget spent.
        let quit = {
            let g = state(app);
            g.shutdown.load(Ordering::Relaxed)
                || (g.clients.load(Ordering::Relaxed) == 0
                    && !g.has_live()
                    && g.last_activity
                        .lock()
                        .map(|t| t.elapsed().as_secs() >= IDLE_EXIT_SECS)
                        .unwrap_or(false))
        };
        if quit {
            return std::process::ExitCode::SUCCESS;
        }
        match connect_named_pipe(current.raw()) {
            Ok(true) => {
                // A client is on `current`; peer identity (ADR §3)
                // decides whether it gets served.
                if security.peer_ok(current.raw()) {
                    state(app).clients.fetch_add(1, Ordering::Relaxed);
                    let thread_app = std::sync::Arc::clone(app);
                    let spawned =
                        std::thread::Builder::new()
                            .stack_size(4 << 20)
                            .spawn(move || {
                                client_thread(&thread_app, current);
                                App::reap_ephemeral(&thread_app);
                            });
                    if spawned.is_err() {
                        state(app).clients.fetch_sub(1, Ordering::Relaxed);
                    }
                } else {
                    // A rejected peer must be disconnected, not kept.
                    drop(current);
                }
            }
            Ok(false) => {
                // 5 s heartbeat: no client arrived, `current` is
                // listening; loop re-checks the idle budget.
            }
            Err(e) => {
                eprintln!("rusttyhost: connect failed ({e}); replacing instance");
                drop(current);
            }
        }
        // Fresh listening instance for the next client.
        current = match create_instance(name, security) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("rusttyhost: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
    }
}

/// Lock the shared state. A poisoned mutex means a request thread
/// panicked; the fields it guards are plain values, so the host keeps
/// serving rather than taking the user's shells down with it.
#[cfg(windows)]
fn state(app: &std::sync::Arc<std::sync::Mutex<App>>) -> std::sync::MutexGuard<'_, App> {
    app.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why `bind` refused: a live host owns the pipe, or creation failed.
#[cfg(windows)]
enum HostExit {
    Taken,
    Failed(std::io::Error),
}

/// A named-pipe handle (owns it; dropping closes it).
#[cfg(windows)]
struct ClientPipe(windows::core::Owned<windows::Win32::Foundation::HANDLE>);

#[cfg(windows)]
impl ClientPipe {
    fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        *self.0
    }
}

// A named-pipe handle is a kernel object, not thread-affine: the accept
// loop moves it to the client's thread.
// Safety: HANDLE values are valid in any thread.
#[cfg(windows)]
unsafe impl Send for ClientPipe {}

/// Probe an existing instance, then create the listening pipe. Creating
/// a second instance of a name succeeds, so a live host is found by
/// opening and connecting to it (ADR §1: single instance).
#[cfg(windows)]
fn bind(name: &str, security: &mut PipeSecurity) -> Result<ClientPipe, HostExit> {
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Pipes::*;
    use windows::core::PCWSTR;

    if probe_running(name) {
        return Err(HostExit::Taken);
    }
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let attrs = security.attributes();
    // Safety: `attrs` references the descriptor inside `security`, which
    // outlives this call; the handle is owned by ClientPipe next line.
    let pipe = unsafe {
        let r = CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(core::ptr::from_ref(&attrs)),
        );
        ClientPipe(windows::core::Owned::new(r))
    };
    if pipe.raw().is_invalid() {
        return Err(HostExit::Failed(io_last_error()));
    }
    Ok(pipe)
}

/// True when some instance already listens: open + connect succeeds.
#[cfg(windows)]
fn probe_running(name: &str) -> bool {
    use windows::Win32::Foundation::*;
    use windows::Win32::Storage::FileSystem::*;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // Safety: query handles, closed by the Owned drop.
    unsafe {
        let Ok(h) = CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        ) else {
            return false;
        };
        let probe = ClientPipe(windows::core::Owned::new(h));
        connect_named_pipe(probe.raw()).unwrap_or(false)
    }
}

/// Overlapped `ConnectNamedPipe` with a 5 s budget.
/// `Ok(true)`: a client is connected on `pipe`.
/// `Ok(false)`: the budget ran out — the pending connect is cancelled
/// and the instance is listening again (the loop retries it, which is
/// also the idle-exit heartbeat).
/// `Err`: the instance is unusable; the caller must close it and
/// create a fresh one. `ERROR_PIPE_CONNECTED` means the client arrived
/// before the call.
#[cfg(windows)]
fn connect_named_pipe(pipe: windows::Win32::Foundation::HANDLE) -> io::Result<bool> {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::IO::*;
    use windows::Win32::System::Pipes::*;
    use windows::Win32::System::Threading::*;

    // Safety: event + OVERLAPPED live through the wait and any cancel;
    // the pipe handle stays valid (owned by the caller).
    unsafe {
        let Ok(event) = CreateEventW(None, true, false, None) else {
            return Err(io_last_error());
        };
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let ok = ConnectNamedPipe(pipe, Some(core::ptr::from_mut(&mut ov)));
        let done = match ok {
            Ok(()) => Ok(true),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(true),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                if WaitForSingleObject(event, 5_000) == WAIT_OBJECT_0 {
                    let mut n = 0u32;
                    GetOverlappedResult(pipe, core::ptr::from_ref(&ov), &mut n, true)
                        .map(|()| Ok(true))
                        .map_err(|e| io::Error::from_raw_os_error(e.code().0))?
                } else {
                    // Timeout: cancel the connect so the event is no
                    // longer in flight, drain the result, keep the
                    // instance.
                    let _ = CancelIoEx(pipe, Some(core::ptr::from_ref(&ov)));
                    WaitForSingleObject(event, INFINITE);
                    let mut n = 0u32;
                    let _ = GetOverlappedResult(pipe, core::ptr::from_ref(&ov), &mut n, true);
                    Ok(false)
                }
            }
            Err(e) => {
                CloseHandle(event).ok();
                return Err(io::Error::from_raw_os_error(e.code().0));
            }
        };
        CloseHandle(event).ok();
        done
    }
}

/// Create one fresh listening instance of the pipe. The caller
/// connects it (`connect_overlapped`) and keeps it as the connection
/// target until a client arrives, then creates the next instance.
#[cfg(windows)]
fn create_instance(name: &str, security: &mut PipeSecurity) -> io::Result<ClientPipe> {
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Pipes::*;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let attrs = security.attributes();
    // Safety: same descriptor lifetime rule as bind.
    let client = unsafe {
        let r = CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(core::ptr::from_ref(&attrs)),
        );
        ClientPipe(windows::core::Owned::new(r))
    };
    if client.raw().is_invalid() {
        return Err(io_last_error());
    }
    Ok(client)
}

/// One client: framed requests in, framed responses out, until the pipe
/// closes. Every request touches `last_activity` (idle-exit budget).
#[cfg(windows)]
fn client_thread(app: &std::sync::Arc<std::sync::Mutex<App>>, client: ClientPipe) {
    use rusttyterm::host::MAX_REQUEST_BYTES;
    use rusttyterm::host::server::handle;

    let mut pipe = PipeStream {
        handle: client.raw(),
        _owner: client,
    };
    while let Ok(req) = rusttyterm::host::read_frame(&mut pipe, MAX_REQUEST_BYTES) {
        {
            let g = state(app);
            if let Ok(mut t) = g.last_activity.lock() {
                *t = std::time::Instant::now();
            }
        }
        let resp = handle(app, &req);
        if rusttyterm::host::write_frame(&mut pipe, &resp).is_err() {
            break;
        }
    }
    // Dropping `pipe` closes the client handle.
}

/// A named-pipe handle seen as a `Read + Write` stream.
#[cfg(windows)]
struct PipeStream {
    handle: windows::Win32::Foundation::HANDLE,
    _owner: ClientPipe,
}

#[cfg(windows)]
impl io::Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use windows::Win32::Foundation::*;
        use windows::Win32::Storage::FileSystem::*;
        use windows::Win32::System::IO::*;
        use windows::Win32::System::Threading::*;

        if buf.is_empty() {
            return Ok(0);
        }
        // Safety: synchronous read on a handle we own; the event and
        // OVERLAPPED live through the call.
        unsafe {
            let Ok(event) = CreateEventW(None, true, false, None) else {
                return Err(io_last_error());
            };
            let mut ov = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let mut done = 0u32;
            let started = ReadFile(
                self.handle,
                Some(buf),
                Some(core::ptr::from_mut(&mut done)),
                Some(core::ptr::from_mut(&mut ov)),
            );
            let mut n = usize::try_from(done).unwrap_or(0);
            if started.is_err() {
                let code = GetLastError();
                if code == ERROR_IO_PENDING {
                    WaitForSingleObject(event, INFINITE);
                    let ok = GetOverlappedResult(
                        self.handle,
                        core::ptr::from_ref(&ov),
                        &mut done,
                        false,
                    );
                    n = match ok {
                        Ok(()) => usize::try_from(done).unwrap_or(0),
                        Err(e) => {
                            CloseHandle(event).ok();
                            return Err(io_error(e));
                        }
                    };
                } else {
                    CloseHandle(event).ok();
                    return Err(io::Error::from_raw_os_error(code.0 as i32));
                }
            }
            CloseHandle(event).ok();
            // EOF (the client closed) is a zero read.
            Ok(n)
        }
    }
}

#[cfg(windows)]
impl io::Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use windows::Win32::Storage::FileSystem::WriteFile;

        if buf.is_empty() {
            return Ok(0);
        }
        // Safety: synchronous write on a handle we own.
        unsafe {
            let mut done = 0u32;
            WriteFile(
                self.handle,
                Some(buf),
                Some(core::ptr::from_mut(&mut done)),
                None,
            )
            .map_err(io_error)?;
            Ok(usize::try_from(done).unwrap_or(0))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
fn io_last_error() -> io::Error {
    use windows::Win32::Foundation::GetLastError;

    // Safety: last-error read, plain.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code.0 as i32)
}

#[cfg(windows)]
fn io_error(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}
