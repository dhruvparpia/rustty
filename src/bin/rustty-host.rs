//! `rustty-host.exe` (issue #46, docs/design-session-host.md): the
//! per-user persistent session host. One instance at a time, bound by
//! creating the named pipe: a second instance probes the pipe, finds a
//! live host, and exits 0. Clients (`rustty` session verbs, the
//! automation API later) connect, speak framed JSON (see
//! `rustty::host`), and their death never kills a host-owned shell
//! (every session lives in its own kill-on-close job object).

// GUI subsystem: the host must never flash a console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use rustty::host::pipe::{ClientPipe, connect, create_instance};
#[cfg(windows)]
use rustty::host::server::{App, PipeSecurity};

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
    use rustty::host::pipe_name;

    // A console only exists when the user started the host by hand;
    // when spawned by a client, the client's redirected handles (if
    // any) must survive so host diagnostics land where the launcher
    // pointed them.
    rustty::cli::attach_parent_console();

    let app = match App::new() {
        Ok(app) => std::sync::Arc::new(std::sync::Mutex::new(app)),
        Err(e) => {
            eprintln!("rustty-host: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut security = match PipeSecurity::for_current_user() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rustty-host: cannot build pipe security: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let name = pipe_name();
    match bind(&name, &mut security) {
        Ok(first) => serve_loop(&app, &mut security, &name, first),
        Err(HostExit::Taken) => std::process::ExitCode::SUCCESS,
        Err(HostExit::Failed(e)) => {
            eprintln!("rustty-host: {e}");
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

    use rustty::host::IDLE_EXIT_SECS;

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
        match connect(current.raw()) {
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
                                // Per-connection attach identity (issue #47).
                                let token = rustty::host::server::new_token();
                                client_thread(&thread_app, current, token);
                                // Release is idempotent: client_thread already
                                // cleared this token before joining its pump.
                                // Now drop the client count, then the ephemeral rule.
                                rustty::host::server::detach_token(&thread_app, token);
                                state(&thread_app)
                                    .clients
                                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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
                eprintln!("rustty-host: connect failed ({e}); replacing instance");
                drop(current);
            }
        }
        // Fresh listening instance for the next client.
        current = match create_instance(name, security, false) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("rustty-host: {e}");
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

/// Probe an existing instance, then create the listening pipe. Creating
/// a second instance of a name succeeds, so a live host is found by
/// opening and connecting to it (ADR §1: single instance).
#[cfg(windows)]
fn bind(name: &str, security: &mut PipeSecurity) -> Result<ClientPipe, HostExit> {
    if probe_running(name) {
        return Err(HostExit::Taken);
    }
    create_instance(name, security, false).map_err(HostExit::Failed)
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
        let probe = ClientPipe::from_handle(h);
        connect(probe.raw()).unwrap_or(false)
    }
}

/// One client: framed requests in, framed responses out, until the pipe
/// closes. A second (pump) thread owns the write side of the handle so
/// streaming frames (output/input/resize) from other connections can
/// interleave safely. Every request but a session list touches
/// `last_activity` (idle-exit budget).
#[cfg(windows)]
fn client_thread(app: &std::sync::Arc<std::sync::Mutex<App>>, client: ClientPipe, token: u64) {
    use rustty::host::MAX_REQUEST_BYTES;
    use rustty::host::server::{Conn, handle, new_pump};

    // Single writer: every frame (responses + streams) goes out through
    // the pump, preserving frame order. The read thread only reads.
    let (wt, wr) = std::sync::mpsc::sync_channel(64);
    let pump = match new_pump(client.raw(), std::sync::Arc::clone(app), token, wr) {
        Ok(j) => j,
        Err(_) => return,
    };
    while let Ok(req) = rustty::host::read_frame(&mut &client, MAX_REQUEST_BYTES) {
        {
            let g = state(app);
            if rustty::host::counts_as_activity(&req)
                && let Ok(mut t) = g.last_activity.lock()
            {
                *t = std::time::Instant::now();
            }
        }
        let conn = Conn { token, tx: &wt };
        let resp = handle(app, &req, &conn);
        if wt.send(resp).is_err() {
            break; // the pump ended: the write side is gone
        }
    }
    // Dropping `client` closes the handle. Release the attach BEFORE
    // joining: the session's subscriptions hold sender clones, so the
    // pump's `rx` can only go idle once detach_token clears them (join
    // first would deadlock).
    drop(client);
    drop(wt);
    rustty::host::server::detach_token(app, token);
    let _ = pump.join();
}
