//! ConPTY FFI boundary (spec WIN-001, TTY-001..TTY-003, TTY-024..TTY-026).
//!
//! This module is the only place in the crate where `unsafe` is allowed
//! (NFR-401). It wraps `CreatePseudoConsole`, `ResizePseudoConsole`,
//! `ClosePseudoConsole`, `CreatePipe`, `ReadFile` and `WriteFile` behind a
//! safe-owning type, [`ConPty`], plus the blocking pipe-read loop the shell
//! reader thread runs.
//!
//! Handle ownership (WIN-001 "parent must not retain pipe handles"):
//!
//! * Four anonymous-pipe ends are created. The two ends handed to
//!   `CreatePseudoConsole` are closed by the parent immediately after the
//!   call — ConPTY owns them from that point, and `ClosePseudoConsole`
//!   closes them.
//! * The parent keeps exactly two handles: the input write end (keyboard
//!   bytes go in) and the output read end (grid bytes come out). Both are
//!   `windows::core::Owned<HANDLE>`, so every path — spawn failure
//!   included — closes them; no leaks.

use std::collections::BTreeMap;
use std::io;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE,
    INVALID_HANDLE_VALUE, SetLastError, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, PSEUDOCONSOLE_INHERIT_CURSOR,
    ResizePseudoConsole,
};
use windows::Win32::System::JobObjects::AssignProcessToJobObject;
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{HRESULT, Owned, PCWSTR, PWSTR};

/// An owned raw HANDLE safe to move onto a worker thread.
///
/// Dropping it closes the handle via the windows crate's `Free` impl for HANDLE.
pub struct SendHandle(Owned<HANDLE>);

impl SendHandle {
    /// The raw handle value, valid while this `SendHandle` is alive.
    #[must_use]
    pub fn raw(&self) -> HANDLE {
        *self.0
    }

    /// Wrap an owned handle (e.g. a job object) for cross-thread ownership.
    #[must_use]
    pub fn from_owned(owned: Owned<HANDLE>) -> Self {
        Self(owned)
    }
}

// Safety: the payload is a single owned kernel handle. Handles are not tied to a
// process thread, and this type is the sole owner, so moving it between threads is
// sound (the reader thread and the UI thread never share the same handle object).
unsafe impl Send for SendHandle {}
// Safety: only `raw` and `wait_exit_code` read it through a shared reference. The
// handle value never changes, and waiting on or querying one process handle from
// two threads is allowed. It is closed only when the last owner drops it.
unsafe impl Sync for SendHandle {}

/// The ConPTY input side, owned by one worker thread.
///
/// Created by [`ConPty::input_writer`]; writes keyboard/query-reply bytes
/// into the shell's input (same path as [`ConPty::write_input`]).
// ConPty handles are kernel handles (not thread-affine); the session host
// moves a whole ConPty to its reader thread (HOST-004).
// Safety: HANDLE/HPCON values are valid in any thread.
unsafe impl Send for ConPty {}

/// A cloneable writer for the pseudo-console input pipe (used by panes
/// that write from the UI thread).
pub struct ConPtyWriter(SendHandle);

impl ConPtyWriter {
    /// Write bytes into the shell's input.
    pub fn write(&self, bytes: &[u8]) -> Result<(), io::Error> {
        // Safety: WriteFile against our own duplicated pipe handle.
        unsafe { WriteFile(*self.0.0, Some(bytes), None, None).map_err(|e| io_error(&e)) }
    }
}

/// A live ConPTY session holding the two parent-side pipe handles.
pub struct ConPty {
    input_write: Owned<HANDLE>,
    output_read: Owned<HANDLE>,
    conpty: HPCON,
}

impl ConPty {
    /// Spawn `program` (with `args`) attached to a new pseudo-console of
    /// `cols` x `rows`, returning the session and the child handles.
    ///
    /// The second returned handle is the primary thread handle (close it
    /// right away); the first is the process handle (the reader thread
    /// waits on it). `cols`/`rows` must be 1..=i16::MAX.
    ///
    /// HOST-004: when `job` is `Some`, the child is created suspended,
    /// assigned to that job object, and resumed, so no grandchild can be
    /// spawned before it is inside the job (the session host uses one
    /// kill-on-close job per session).
    pub fn spawn(
        program: &str,
        args: &[String],
        cols: usize,
        rows: usize,
        cwd: Option<&str>,
        env: &BTreeMap<String, String>,
        job: Option<HANDLE>,
    ) -> Result<(Self, SendHandle, SendHandle), io::Error> {
        if cols == 0 || rows == 0 || cols > i16::MAX as usize || rows > i16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal size must be 1..=i16::MAX in each dimension",
            ));
        }
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };

        // Safety: plain Win32 FFI per the MSDN ConPTY sample. Every handle
        // created below is closed on every failure path (WIN-001).
        unsafe {
            SetLastError(WIN32_ERROR(0));

            // Pipe 1: parent writes -> ConPTY reads (child stdin).
            let (con_in_read, parent_input_write) = create_pipe()?;
            // Pipe 2: ConPTY writes -> parent reads (child stdout).
            let (parent_output_read, con_out_write) = create_pipe()?;

            // Hand one end of each pipe to ConPTY; ConPTY owns these two
            // ends from the call on, so the parent closes them right after
            // (WIN-001: parent must not retain passed pipe handles).
            let pcon = match CreatePseudoConsole(
                size,
                con_in_read,
                con_out_write,
                PSEUDOCONSOLE_INHERIT_CURSOR,
            ) {
                Ok(pcon) => {
                    CloseHandle(con_in_read).ok();
                    CloseHandle(con_out_write).ok();
                    pcon
                }
                Err(e) => {
                    CloseHandle(con_in_read).ok();
                    CloseHandle(con_out_write).ok();
                    CloseHandle(parent_input_write).ok();
                    CloseHandle(parent_output_read).ok();
                    return Err(io_error(&e));
                }
            };

            // Extended startup info carrying the pseudo-console handle.
            // MSDN: the probe call with a null list fails with
            // ERROR_INSUFFICIENT_BUFFER and reports the buffer size in
            // `attr_size`. That failure is the documented sizing step, not
            // an error (ConPTY sample checks for exactly this code).
            let mut attr_size = 0usize;
            if let Err(e) = InitializeProcThreadAttributeList(
                Some(LPPROC_THREAD_ATTRIBUTE_LIST(null_mut())),
                1,
                None,
                &mut attr_size,
            ) {
                let is_size_probe = (e.code().0 & 0xFFFF) == ERROR_INSUFFICIENT_BUFFER.0 as i32;
                if !is_size_probe || attr_size == 0 {
                    ClosePseudoConsole(pcon);
                    CloseHandle(parent_input_write).ok();
                    CloseHandle(parent_output_read).ok();
                    return Err(io_error(&e));
                }
            }
            let mut attr_list: Vec<u8> = vec![0; attr_size];
            let attr = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list.as_mut_ptr().cast());
            if let Err(e) = InitializeProcThreadAttributeList(Some(attr), 1, None, &mut attr_size) {
                DeleteProcThreadAttributeList(attr);
                ClosePseudoConsole(pcon);
                CloseHandle(parent_input_write).ok();
                CloseHandle(parent_output_read).ok();
                return Err(io_error(&e));
            }
            // PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE carries the HPCON value
            // itself, passed as a pointer-sized value (MSDN ConPTY sample).
            let update = UpdateProcThreadAttribute(
                attr,
                0,
                usize::try_from(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE).unwrap_or(0),
                Some(pcon.0 as *const core::ffi::c_void),
                size_of::<HPCON>(),
                None,
                None,
            );
            if let Err(e) = update {
                DeleteProcThreadAttributeList(attr);
                ClosePseudoConsole(pcon);
                CloseHandle(parent_input_write).ok();
                CloseHandle(parent_output_read).ok();
                return Err(io_error(&e));
            }

            let mut cmdline = wide_command_line(program, args);
            // SHELL-019/021: working directory and environment overrides for
            // this pane. The block starts from the inherited environment
            // (SHELL-022: nothing is injected on top of what the user set).
            let cwd_wide = cwd.map(|dir| {
                dir.encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<u16>>()
            });
            let cwd_ptr = cwd_wide
                .as_ref()
                .map_or(std::ptr::null(), |dir| dir.as_ptr());
            let env_block = wide_environment(env);
            let mut startup = STARTUPINFOEXW::default();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            // The std handles are set explicitly (to invalid): the child must
            // not inherit the parent's own stdio, it talks to the
            // pseudo-console through the attribute list only.
            // Same convention as portable-pty/WezTerm.
            startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
            startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
            startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
            startup.lpAttributeList = attr;

            let mut process = PROCESS_INFORMATION::default();
            let flags = EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | if job.is_some() {
                    CREATE_SUSPENDED
                } else {
                    Default::default()
                };
            let spawned = CreateProcessW(
                PWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                Some(env_block.as_ptr() as *const core::ffi::c_void),
                PCWSTR::from_raw(cwd_ptr),
                core::ptr::from_ref(&startup.StartupInfo),
                &mut process,
            );

            DeleteProcThreadAttributeList(attr);

            if let Err(e) = spawned {
                ClosePseudoConsole(pcon);
                CloseHandle(parent_input_write).ok();
                CloseHandle(parent_output_read).ok();
                return Err(io_error(&e));
            }

            if let Some(job) = job {
                // HOST-004: the child is suspended; it joins the job
                // before any code of its own runs. An assignment failure
                // kills the suspended child (no orphan) and unwinds.
                if let Err(e) = AssignProcessToJobObject(job, process.hProcess) {
                    TerminateProcess(process.hProcess, 1).ok();
                    CloseHandle(process.hThread).ok();
                    CloseHandle(process.hProcess).ok();
                    ClosePseudoConsole(pcon);
                    CloseHandle(parent_input_write).ok();
                    CloseHandle(parent_output_read).ok();
                    return Err(io_error(&e));
                }
                ResumeThread(process.hThread);
            }

            Ok((
                Self {
                    input_write: own(parent_input_write),
                    output_read: own(parent_output_read),
                    conpty: pcon,
                },
                SendHandle(own(process.hProcess)),
                SendHandle(own(process.hThread)),
            ))
        }
    }

    /// Write keyboard/paste bytes to the shell's input (TTY-004..TTY-017).
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), io::Error> {
        // Safety: WriteFile against our own pipe handle.
        unsafe { WriteFile(*self.input_write, Some(bytes), None, None).map_err(|e| io_error(&e)) }
    }

    /// Resize the pseudo-console (TTY-003: "resize must be delivered").
    /// HOST-006: close the input pipe write end so the shell sees stdin
    /// EOF (the graceful-termination attempt). Idempotent: the handle is
    /// replaced with a null one `Drop` tolerates.
    pub fn close_input(&mut self) {
        // Safety: closing our own write handle; CloseHandle(NULL) just
        // fails harmlessly on repeat calls.
        unsafe {
            CloseHandle(*self.input_write).ok();
            *self.input_write = HANDLE(std::ptr::null_mut());
        }
    }

    /// Resize the pseudo-console to `cols` x `rows`.
    pub fn resize(&self, cols: usize, rows: usize) -> Result<(), io::Error> {
        let size = COORD {
            X: cols.min(i16::MAX as usize) as i16,
            Y: rows.min(i16::MAX as usize) as i16,
        };
        // Safety: ResizePseudoConsole against our own ConPTY handle.
        unsafe { ResizePseudoConsole(self.conpty, size).map_err(|e| io_error(&e)) }
    }

    /// Signal EOF to the shell and release the pseudo-console.
    ///
    /// `ClosePseudoConsole` closes the ConPTY-side pipe ends, so the child
    /// sees stdin EOF and exits; the reader thread observes output EOF.
    /// Safe to call more than once (TTY-024: graceful close).
    pub fn close(&mut self) {
        let pcon = std::mem::replace(&mut self.conpty, HPCON(0));
        if pcon.0 != 0 {
            // Safety: ClosePseudoConsole on a handle we own.
            unsafe { ClosePseudoConsole(pcon) }
        }
    }

    /// The input side, for a worker thread that must write query replies.
    ///
    /// The reader answers terminal queries (CSI 6n) that arrive on the
    /// output stream; those answers are written back into the shell's
    /// input, exactly like keyboard bytes.
    pub fn input_writer(&self) -> Result<ConPtyWriter, io::Error> {
        // Safety: plain DuplicateHandle of our own pipe write end.
        unsafe {
            let me = GetCurrentProcess();
            let mut dup = HANDLE::default();
            DuplicateHandle(
                me,
                *self.input_write,
                me,
                core::ptr::from_mut(&mut dup),
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .map(|()| ConPtyWriter(SendHandle(own(dup))))
            .map_err(|e| io_error(&e))
        }
    }

    /// A duplicate of the output-read handle, for the reader thread.
    pub fn output_reader(&self) -> Result<SendHandle, io::Error> {
        // Safety: plain DuplicateHandle of our own pipe read end.
        unsafe {
            let me = GetCurrentProcess();
            let mut dup = HANDLE::default();
            DuplicateHandle(
                me,
                *self.output_read,
                me,
                core::ptr::from_mut(&mut dup),
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .map(|()| SendHandle(own(dup)))
            .map_err(|e| io_error(&e))
        }
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        self.close();
    }
}

/// Block until the child process exits, then return its exit code.
///
/// The process id behind `process` (RESTORE-002): 0 when Windows will not say.
#[must_use]
pub fn process_id(process: &SendHandle) -> u32 {
    // Safety: the handle stays open for the length of the call.
    unsafe { windows::Win32::System::Threading::GetProcessId(*process.0) }
}

/// Run this on the reader thread after the output pipe reports EOF
/// (TTY-025: report the exit code to the pane). `STILL_ACTIVE` (259) is
/// filtered out so a half-dead handle yields `None`, never a bogus code.
pub fn wait_exit_code(process: &SendHandle) -> Option<i32> {
    // Safety: wait + query against the child process handle we own.
    unsafe {
        let handle = *process.0;
        let _ = WaitForSingleObject(handle, INFINITE);
        let mut code = 0u32;
        if GetExitCodeProcess(handle, &mut code).is_ok() && code != 259 {
            Some(code as i32)
        } else {
            None
        }
    }
}

/// Block reading the output pipe, yielding every chunk that arrives.
///
/// The loop ends on EOF (shell gone / ConPTY closed) or a pipe error; the
/// bool is `true` on a normal EOF. A broken ConPTY surfaces as an exit,
/// never a crash (TTY-026).
pub fn read_output_loop<F>(reader: &SendHandle, mut on_chunk: F) -> Result<bool, io::Error>
where
    F: FnMut(&[u8]),
{
    // Safety: ReadFile against our own pipe read end; the buffer is valid
    // for the read length and `got` is a live u32 for the out-param.
    unsafe {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut got = 0u32;
            match ReadFile(
                *reader.0,
                Some(&mut buf),
                Some(core::ptr::from_mut(&mut got)),
                None,
            ) {
                Ok(()) => {
                    if got == 0 {
                        return Ok(true);
                    }
                    on_chunk(&buf[..usize::try_from(got).unwrap_or(0)]);
                }
                Err(e) => {
                    if e.code() == ERROR_BROKEN_PIPE_CODE {
                        return Ok(true);
                    }
                    return Err(io_error(&e));
                }
            }
        }
    }
}

/// `ERROR_BROKEN_PIPE` (109) as the `HRESULT` shape `Error::code()` returns.
const ERROR_BROKEN_PIPE_CODE: HRESULT = HRESULT::from_win32(109);

fn create_pipe() -> Result<(HANDLE, HANDLE), io::Error> {
    // Safety: out-params are valid; handles are owned by the caller next.
    unsafe {
        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        CreatePipe(&mut read, &mut write, None, 8 * 1024).map_err(|e| io_error(&e))?;
        Ok((read, write))
    }
}

/// Take ownership of a raw HANDLE the caller has just created.
///
/// # Safety
/// `handle` must be a live, uniquely-owned handle (fresh `CreatePipe` /
/// `CreateProcessW` output).
unsafe fn own(handle: HANDLE) -> Owned<HANDLE> {
    // Safety: contract of the caller of this private helper.
    unsafe { Owned::new(handle) }
}

/// Build a wide command line: the program path, then each argument,
/// quoting anything with spaces (the classic CommandLineToArgvW shape).
fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        let mut out = String::new();
        out.push('"');
        out.push('"');
        return out;
    }
    let needs = arg
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
        || arg.contains("\\");
    if !needs {
        return arg.to_string();
    }
    let mut out = String::new();
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else if c == '"' {
            let n = backslashes * 2 + 1;
            for _ in 0..n {
                out.push('\\');
            }
            out.push('"');
            backslashes = 0;
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push(c);
            backslashes = 0;
        }
    }
    let n = backslashes * 2;
    for _ in 0..n {
        out.push('\\');
    }
    out.push('"');
    out.to_string()
}

fn wide_command_line(program: &str, args: &[String]) -> Vec<u16> {
    let mut s = quote_arg(program);
    for arg in args {
        s.push(' ');
        s.push_str(&quote_arg(arg));
    }
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The `CreateProcessW` environment block: the inherited environment with
/// the profile's overrides applied (case-insensitive, Windows convention),
/// as `KEY=VALUE\0` pairs terminated by an extra NUL.
fn wide_environment(env: &BTreeMap<String, String>) -> Vec<u16> {
    environment_text(std::env::vars(), env)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// The block's text. The automation variables are never inherited (AUTO-007):
/// they name one session's window and grant, so a window or host started from
/// inside a boss session must not pass them on to every shell it opens. A
/// session that should have them gets them explicitly, in `env`.
fn environment_text(
    inherited: impl Iterator<Item = (String, String)>,
    env: &BTreeMap<String, String>,
) -> String {
    let mut block = String::new();
    for (key, value) in inherited {
        let private = [crate::automation::ENV_WINDOW, crate::automation::ENV_TOKEN]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&key));
        if !private && !env.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            block.push_str(&key);
            block.push('=');
            block.push_str(&value);
            block.push('\0');
        }
    }
    for (key, value) in env {
        block.push_str(key);
        block.push('=');
        block.push_str(value);
        block.push('\0');
    }
    block
}

fn io_error(e: &windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_variables_are_never_inherited() {
        // Break caught: a window started from a boss session handing that
        // session's token to every shell it opens, and a host session naming
        // a window it has nothing to do with.
        let inherited = vec![
            ("PATH".to_string(), "p".to_string()),
            ("RUSTTY_AUTOMATION_TOKEN".to_string(), "leak".to_string()),
            ("rustty_window".to_string(), "9".to_string()),
        ];
        let mut env = BTreeMap::new();
        let block = environment_text(inherited.clone().into_iter(), &env);
        assert!(block.contains("PATH=p"));
        assert!(!block.contains("leak"));
        assert!(!block.to_ascii_uppercase().contains("RUSTTY_WINDOW"));
        // What this session was explicitly given still goes through.
        env.insert("RUSTTY_WINDOW".to_string(), "5".to_string());
        env.insert("RUSTTY_AUTOMATION_TOKEN".to_string(), "mine".to_string());
        let block = environment_text(inherited.into_iter(), &env);
        assert!(block.contains("RUSTTY_WINDOW=5"));
        assert!(block.contains("RUSTTY_AUTOMATION_TOKEN=mine"));
        assert!(!block.contains("leak"));
    }

    /// SHELL-021: the profile's environment replaces the inherited value
    /// (case-insensitively) and the block stays double-NUL terminated.
    #[test]
    fn profile_env_replaces_inherited_and_terminates_the_block() {
        let mut env = BTreeMap::new();
        env.insert("rustty_test_var".to_string(), "profile".to_string());
        let block = wide_environment(&env);
        let text: String = String::from_utf16_lossy(&block);
        let pairs: Vec<&str> = text.split('\0').collect();
        let hits: Vec<&&str> = pairs
            .iter()
            .filter(|p| p.to_ascii_lowercase().starts_with("rustty_test_var="))
            .collect();
        assert_eq!(hits, vec![&"rustty_test_var=profile"]);
        assert!(text.ends_with("\0\0"), "block must end with an empty pair");
    }
}
