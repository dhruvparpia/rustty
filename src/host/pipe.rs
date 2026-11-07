//! Named-pipe server plumbing shared by `rustty-host` and the per-window
//! automation server (AUTO-003): one way to create an instance, wait for a
//! client and read or write its frames. Every handle is overlapped; see
//! [`super::overlapped_write`] for why.

use std::io;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Owned, PCWSTR};

use super::server::PipeSecurity;

/// How long [`connect`] waits before reporting no client, which doubles as the
/// accept loop's heartbeat.
const CONNECT_WAIT_MS: u32 = 5_000;

/// The last Win32 error as an [`io::Error`].
#[must_use]
pub fn last_error() -> io::Error {
    // Safety: reading the thread's last error has no preconditions.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code.0 as i32)
}

/// A server end of a named pipe. Dropping it closes the handle.
pub struct ClientPipe(Owned<HANDLE>);

// Safety: a kernel handle is valid on any thread, and every read and write here
// waits on its own event, so a reader and a writer can share one connection.
unsafe impl Send for ClientPipe {}
// Safety: as above; concurrent overlapped I/O on one handle is the design.
unsafe impl Sync for ClientPipe {}

impl ClientPipe {
    /// Take ownership of a handle.
    #[must_use]
    pub fn from_handle(handle: HANDLE) -> Self {
        // Safety: the caller hands over a handle it owns, or an invalid one,
        // which `is_invalid` reports and `Owned` tolerates.
        Self(unsafe { Owned::new(handle) })
    }

    /// The raw handle, valid while `self` lives.
    #[must_use]
    pub fn raw(&self) -> HANDLE {
        *self.0
    }

    /// Wake any read or write in progress on this handle, on any thread.
    pub fn cancel_io(&self) {
        // Safety: the handle is valid while `self` lives. With no OVERLAPPED
        // this cancels everything pending; it fails harmlessly if nothing is.
        let _ = unsafe { CancelIoEx(self.raw(), None) };
    }
}

impl io::Read for &ClientPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        super::overlapped_read(self.raw(), buf)
    }
}

impl io::Write for &ClientPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        super::overlapped_write(self.raw(), buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Create one listening instance of `name`, current-user only and local only.
/// `first` also refuses the name if anything already holds it, so a process that
/// got there first can't pose as this server.
///
/// # Errors
///
/// The Win32 error from `CreateNamedPipeW`.
pub fn create_instance(
    name: &str,
    security: &mut PipeSecurity,
    first: bool,
) -> io::Result<ClientPipe> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let attrs = security.attributes();
    let mode = if first {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED
    };
    // Safety: `attrs` references the descriptor inside `security`, which
    // outlives this call; `wide` is NUL-terminated and outlives it too.
    let pipe = ClientPipe::from_handle(unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(core::ptr::from_ref(&attrs)),
        )
    });
    if pipe.raw().is_invalid() {
        return Err(last_error());
    }
    Ok(pipe)
}

/// Overlapped `ConnectNamedPipe` with a 5 s budget.
/// `Ok(true)`: a client is connected on `pipe`.
/// `Ok(false)`: the budget ran out; the pending connect is cancelled and the
/// instance is still listening (the loop retries it, which is also its heartbeat).
/// `Err`: the instance is unusable; close it and create a fresh one.
/// `ERROR_PIPE_CONNECTED` means the client arrived before the call.
///
/// # Errors
///
/// Event creation or connect failures.
pub fn connect(pipe: HANDLE) -> io::Result<bool> {
    // Safety: event + OVERLAPPED live through the wait and any cancel; the pipe
    // handle stays valid (owned by the caller).
    unsafe {
        let Ok(event) = CreateEventW(None, true, false, None) else {
            return Err(last_error());
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
                if WaitForSingleObject(event, CONNECT_WAIT_MS) == WAIT_OBJECT_0 {
                    let mut n = 0u32;
                    GetOverlappedResult(pipe, core::ptr::from_ref(&ov), &mut n, true)
                        .map(|()| true)
                        .map_err(|e| io::Error::from_raw_os_error(e.code().0))
                } else {
                    // Timeout: cancel the connect so the event is no longer in
                    // flight, drain the result, keep the instance.
                    let _ = CancelIoEx(pipe, Some(core::ptr::from_ref(&ov)));
                    WaitForSingleObject(event, INFINITE);
                    let mut n = 0u32;
                    let _ = GetOverlappedResult(pipe, core::ptr::from_ref(&ov), &mut n, true);
                    Ok(false)
                }
            }
            Err(e) => Err(io::Error::from_raw_os_error(e.code().0)),
        };
        CloseHandle(event).ok();
        done
    }
}
