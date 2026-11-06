//! The Windows side of `rustty secrets` (SECRET-010, SECRET-011): finding and
//! running the real `op` over pipes, and starting the command through our own
//! `CreateProcessW` with an environment block wiped straight after.
//! `std::process::Command` cannot do this: it keeps copies of the environment
//! that nothing can wipe.
//!
//! There is deliberately no environment or config override for where `op` is
//! (SECRET-010), and `PATH` is never consulted for it. The per-user WinGet
//! location is still writable by the same Windows user, which is part of the
//! limit stated in SECRET-012.

use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::Once;

use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
};
use windows::core::{BOOL, GUID, PCWSTR, PWSTR};

use crate::program_search::{is_batch, is_runnable_file, resolve_command};
use crate::secrets::{
    CopyApproval, Launcher, OpCall, OpOutput, OpRunner, SecretsError, copy_prompt,
};
use crate::secure_mem::{LockedPages, LockedSecret, MAX_SECRET_LEN};

/// How much of `op`'s stderr is kept. The rest is read and dropped so the
/// child never blocks on a full pipe.
pub const MAX_CAPTURED_STDERR: usize = 64 * 1024;
/// The variable `op` reads a service-account token from.
pub const TOKEN_VARIABLE: &str = "OP_SERVICE_ACCOUNT_TOKEN";

/// A `windows` error as an `io::Error`.
fn io_error(e: &windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

/// Append `arg` quoted for `CommandLineToArgvW`.
pub fn push_quoted(out: &mut Vec<u16>, arg: &[u16]) {
    const SPACE: u16 = b' ' as u16;
    const TAB: u16 = b'\t' as u16;
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let needs_quotes = arg.is_empty() || arg.iter().any(|u| matches!(*u, SPACE | TAB | QUOTE));
    if !needs_quotes {
        out.extend_from_slice(arg);
        return;
    }
    out.push(QUOTE);
    let mut backslashes = 0usize;
    for unit in arg {
        if *unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        // Backslashes only need doubling when a quote follows them.
        let copies = if *unit == QUOTE {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        out.extend(std::iter::repeat_n(BACKSLASH, copies));
        out.push(*unit);
        backslashes = 0;
    }
    out.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    out.push(QUOTE);
}

/// The NUL-terminated command line for an executable.
#[must_use]
pub fn exe_command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    let program: Vec<u16> = program.as_os_str().encode_wide().collect();
    push_quoted(&mut line, &program);
    for arg in args {
        line.push(u16::from(b' '));
        let arg: Vec<u16> = arg.encode_wide().collect();
        push_quoted(&mut line, &arg);
    }
    line.push(0);
    line
}

/// True if `cmd.exe` could reinterpret these units even inside quotes: a
/// quote ends the quoting, `%` expands, and a line break ends the command.
fn unsafe_for_batch(units: &[u16]) -> bool {
    units
        .iter()
        .any(|u| matches!(*u, 0x22 | 0x25 | 0x0d | 0x0a | 0))
}

/// The NUL-terminated command line that runs a `.cmd`/`.bat` through
/// `cmd.exe`. `CreateProcessW` would do this implicitly with no quoting rules
/// at all, the "BatBadBut" injection. Every argument is quoted, and anything
/// quoting cannot make safe is refused.
///
/// # Errors
/// [`SecretsError::Launch`]; the message does not quote the argument.
pub fn batch_command_line(script: &Path, args: &[OsString]) -> Result<Vec<u16>, SecretsError> {
    let refused = || {
        SecretsError::Launch(
            "an argument or path holds a quote, a percent sign or a line break, which cannot be \
             passed safely to a batch file; run its interpreter directly"
                .to_owned(),
        )
    };
    let script: Vec<u16> = script.as_os_str().encode_wide().collect();
    if unsafe_for_batch(&script) {
        return Err(refused());
    }
    let mut line: Vec<u16> = "cmd.exe /e:ON /v:OFF /d /s /c \"\""
        .encode_utf16()
        .collect();
    line.extend_from_slice(&script);
    line.push(u16::from(b'"'));
    for arg in args {
        let arg: Vec<u16> = arg.encode_wide().collect();
        if unsafe_for_batch(&arg) {
            return Err(refused());
        }
        line.extend(" \"".encode_utf16());
        line.extend_from_slice(&arg);
        // A program behind the batch file parses `\"` as an escaped quote.
        let trailing = arg
            .iter()
            .rev()
            .take_while(|u| **u == u16::from(b'\\'))
            .count();
        line.extend(std::iter::repeat_n(u16::from(b'\\'), trailing));
        line.push(u16::from(b'"'));
    }
    line.push(u16::from(b'"'));
    line.push(0);
    Ok(line)
}

/// Where `op` may be, best first: the machine install (only an administrator
/// can change it), then the per-user WinGet locations. This list is the whole
/// search: neither `PATH` nor the current directory is consulted.
#[must_use]
pub fn op_candidates(
    program_files: &Path,
    local_app_data: &Path,
    winget_packages: &[PathBuf],
) -> Vec<PathBuf> {
    let mut candidates = vec![
        program_files.join("1Password CLI").join("op.exe"),
        program_files.join("WinGet").join("Links").join("op.exe"),
        local_app_data
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("op.exe"),
    ];
    candidates.extend(winget_packages.iter().map(|dir| dir.join("op.exe")));
    candidates
}

/// A known folder's path. These come from the user's profile, not from
/// environment variables that a parent process can set.
fn known_folder(id: &GUID) -> io::Result<PathBuf> {
    // Safety: `id` is a valid GUID, and the returned buffer is copied and then
    // freed with the allocator the API documents.
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).map_err(|e| io_error(&e))?;
        let path = OsString::from_wide(raw.as_wide());
        CoTaskMemFree(Some(raw.0.cast::<c_void>().cast_const()));
        Ok(PathBuf::from(path))
    }
}

/// SECRET-007: where tokens are kept: `%LOCALAPPDATA%\rustty\secrets`, from
/// the known folder rather than the environment variable.
///
/// # Errors
/// The known folder lookup failed.
pub fn token_store_dir() -> io::Result<PathBuf> {
    Ok(known_folder(&FOLDERID_LocalAppData)?
        .join("rustty")
        .join("secrets"))
}

/// A program in the Windows system directory, not found through `PATH` or
/// `COMSPEC`.
///
/// # Errors
/// The system directory could not be read.
pub fn system_program(name: &str) -> io::Result<PathBuf> {
    let mut buffer = [0u16; 1024];
    // Safety: the buffer is valid for its whole length.
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) };
    let len = usize::try_from(len).map_err(io::Error::other)?;
    if len == 0 || len > buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..len])).join(name))
}

/// The Windows folders, from the system rather than the environment: the system
/// folder, then Windows itself.
#[must_use]
pub fn windows_folders() -> Vec<PathBuf> {
    let folder = |buffer: &[u16], len: u32| {
        let len = usize::try_from(len).ok()?;
        (1..=buffer.len())
            .contains(&len)
            .then(|| PathBuf::from(OsString::from_wide(&buffer[..len])))
    };
    let mut buffer = [0u16; 1024];
    // Safety: the buffer is valid for its whole length.
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) };
    let system = folder(&buffer, len);
    // Safety: as above.
    let len = unsafe { GetSystemWindowsDirectoryW(Some(&mut buffer)) };
    let windows = folder(&buffer, len);
    system.into_iter().chain(windows).collect()
}

/// SECRET-010: find the real `op` with no override of any kind.
fn resolve_op() -> Result<PathBuf, SecretsError> {
    let unavailable = |e: io::Error| SecretsError::Op(format!("op could not be located: {e}"));
    let program_files = known_folder(&FOLDERID_ProgramFiles).map_err(unavailable)?;
    let local_app_data = known_folder(&FOLDERID_LocalAppData).map_err(unavailable)?;
    let packages_root = local_app_data
        .join("Microsoft")
        .join("WinGet")
        .join("Packages");
    let mut packages: Vec<PathBuf> = std::fs::read_dir(&packages_root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("AgileBits.1Password.CLI_")
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    packages.sort();
    op_candidates(&program_files, &local_app_data, &packages)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            SecretsError::Op(
                "the 1Password CLI was not found under Program Files or the WinGet folders. \
                 Install it with: winget install AgileBits.1Password.CLI. PATH is not searched \
                 on purpose, so a planted program cannot receive a token"
                    .to_owned(),
            )
        })
}

/// What a child may inherit from this process's environment. Applied to names
/// compared in upper case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvPolicy {
    /// The command for `secrets run`: everything but a stray token.
    Command,
    /// `op` under the owner's session: no service credentials, or `op` would
    /// use them instead. The owner's account choice stays.
    OpOwner,
    /// `op` holding a project token: no inherited `OP_*` at all, so nothing
    /// can divert it, such as `OP_CONNECT_HOST`.
    OpServiceAccount,
}

impl EnvPolicy {
    /// This policy drops the variable named `upper`.
    fn drops(self, upper: &str) -> bool {
        match self {
            Self::Command => upper == TOKEN_VARIABLE,
            Self::OpOwner => {
                matches!(
                    upper,
                    TOKEN_VARIABLE | "OP_CONNECT_HOST" | "OP_CONNECT_TOKEN"
                )
            }
            Self::OpServiceAccount => upper.starts_with("OP_"),
        }
    }
}

/// One variable on its way into a block.
struct EnvEntry<'a> {
    /// Name in UTF-16, as it will appear.
    name: Vec<u16>,
    /// Upper-cased name: the block's sort key.
    sort_key: Vec<u16>,
    /// The value.
    value: EnvValue<'a>,
}

/// A value on its way into a block.
enum EnvValue<'a> {
    /// An inherited value, already in UTF-16.
    Plain(Vec<u16>),
    /// A secret, and its length in UTF-16 units.
    Secret(&'a LockedSecret, usize),
}

/// `units` with ASCII lower case raised, for the block's sort order.
fn upper_units(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .map(|u| {
            if (0x61..=0x7a).contains(u) {
                u - 0x20
            } else {
                *u
            }
        })
        .collect()
}

/// A secret's UTF-16 length, refusing what cannot be an environment value.
fn secret_units(secret: &LockedSecret) -> io::Result<usize> {
    secret.with_plaintext(|plain| {
        let text = std::str::from_utf8(plain)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a secret is not text"))?;
        if text.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a secret holds a NUL character",
            ));
        }
        Ok(text.encode_utf16().count())
    })?
}

/// SECRET-011: build a `CreateProcessW` environment block in locked memory:
/// `inherited` filtered by `policy`, with `secrets` added or replacing a
/// variable of the same name ignoring case, sorted as Windows documents and
/// double-NUL terminated. The caller wipes it straight after the call.
///
/// # Errors
/// The memory could not be locked, or a secret is not valid text.
pub fn build_env_block(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    policy: EnvPolicy,
    secrets: &[(&str, &LockedSecret)],
) -> io::Result<LockedPages> {
    let mut entries = Vec::new();
    for (name, value) in inherited {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        let replaced = secrets
            .iter()
            .any(|(secret, _)| secret.eq_ignore_ascii_case(&upper));
        if policy.drops(&upper) || replaced {
            continue;
        }
        let name: Vec<u16> = name.encode_wide().collect();
        entries.push(EnvEntry {
            sort_key: upper_units(&name),
            name,
            value: EnvValue::Plain(value.encode_wide().collect()),
        });
    }
    for (name, secret) in secrets {
        let name: Vec<u16> = name.encode_utf16().collect();
        entries.push(EnvEntry {
            sort_key: upper_units(&name),
            name,
            value: EnvValue::Secret(secret, secret_units(secret)?),
        });
    }
    entries.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    // Each entry is name=value\0, and the block ends with one more \0. An
    // empty block still needs two.
    let mut units = 2usize;
    for entry in &entries {
        let value_units = match &entry.value {
            EnvValue::Plain(value) => value.len(),
            EnvValue::Secret(_, len) => *len,
        };
        units += entry.name.len() + 1 + value_units + 1;
    }
    let mut pages = LockedPages::new(units.saturating_mul(2))?;
    let wide = pages.as_wide_mut();
    let mut at = 0usize;
    for entry in &entries {
        wide[at..at + entry.name.len()].copy_from_slice(&entry.name);
        at += entry.name.len();
        wide[at] = u16::from(b'=');
        at += 1;
        match &entry.value {
            EnvValue::Plain(value) => {
                wide[at..at + value.len()].copy_from_slice(value);
                at += value.len();
            }
            EnvValue::Secret(secret, len) => {
                let slots = &mut wide[at..at + *len];
                secret.with_plaintext(|plain| {
                    // Validated by `secret_units`; a failure here leaves
                    // zeros, never a partial copy of anything else.
                    let text = std::str::from_utf8(plain).unwrap_or_default();
                    for (slot, unit) in slots.iter_mut().zip(text.encode_utf16()) {
                        *slot = unit;
                    }
                })?;
                at += *len;
            }
        }
        // The terminating NUL is already there: the pages start zeroed.
        at += 1;
    }
    Ok(pages)
}

/// An inheritable duplicate of `handle`, or `None` if there is nothing to pass
/// on. Duplicating leaves every other handle in this process non-inheritable.
fn inheritable_copy(handle: HANDLE) -> Option<OwnedHandle> {
    if handle.is_invalid() {
        return None;
    }
    let mut copy = HANDLE::default();
    // Safety: the pseudo-handle is always valid, `handle` is live or the call
    // fails, and `copy` is a valid out-parameter owned on success.
    unsafe {
        let process = GetCurrentProcess();
        DuplicateHandle(
            process,
            handle,
            process,
            &raw mut copy,
            0,
            true,
            DUPLICATE_SAME_ACCESS,
        )
        .ok()?;
        Some(OwnedHandle::from_raw_handle(copy.0))
    }
}

/// A NUL-terminated wide string.
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The attribute list saying "these handles and no others are inherited",
/// released on drop. `CreateProcessW` otherwise passes every inheritable
/// handle in the process to the child.
struct HandleList {
    /// Storage for the opaque list, kept `usize`-aligned.
    buffer: Vec<usize>,
    /// The handles the list points at; must stay in place while it lives.
    handles: Vec<HANDLE>,
}

impl HandleList {
    /// Build the list for `handles`, which must not be empty.
    fn new(handles: Vec<HANDLE>) -> io::Result<Self> {
        let mut size = 0usize;
        // Safety: the first call only reports the size needed (and fails with
        // ERROR_INSUFFICIENT_BUFFER, which is expected). The second writes
        // into a buffer of at least that size. `handles` is heap-allocated
        // and moved, not reallocated, into `Self`, so its address stays valid
        // for as long as the list does.
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &raw mut size);
            let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast::<c_void>());
            InitializeProcThreadAttributeList(Some(list), 1, None, &raw mut size)
                .map_err(|e| io_error(&e))?;
            let this = Self { buffer, handles };
            UpdateProcThreadAttribute(
                list,
                0,
                usize::try_from(PROC_THREAD_ATTRIBUTE_HANDLE_LIST).map_err(io::Error::other)?,
                Some(this.handles.as_ptr().cast::<c_void>()),
                size_of_val(this.handles.as_slice()),
                None,
                None,
            )
            .map_err(|e| io_error(&e))?;
            Ok(this)
        }
    }

    /// The list for `STARTUPINFOEXW`.
    const fn as_list(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.buffer.as_mut_ptr().cast::<c_void>())
    }
}

impl Drop for HandleList {
    fn drop(&mut self) {
        // Safety: the list was initialised in `new` and is not used again.
        unsafe { DeleteProcThreadAttributeList(self.as_list()) };
    }
}

/// Start `application` with exactly these three standard handles inherited and
/// nothing else. The kernel copies the environment during the call, so the
/// caller can wipe `environment` as soon as this returns.
fn spawn(
    application: &Path,
    command_line: &mut [u16],
    environment: &LockedPages,
    cwd: Option<&Path>,
    stdio: [HANDLE; 3],
) -> io::Result<OwnedHandle> {
    let application = wide_path(application);
    let cwd = cwd.map(wide_path);
    let cwd_ptr = cwd
        .as_ref()
        .map_or_else(PCWSTR::null, |dir| PCWSTR::from_raw(dir.as_ptr()));

    let copies = stdio.map(inheritable_copy);
    let raw = |copy: &Option<OwnedHandle>| {
        copy.as_ref()
            .map_or_else(HANDLE::default, |h| HANDLE(h.as_raw_handle()))
    };
    let inherited: Vec<HANDLE> = copies
        .iter()
        .flatten()
        .map(|h| HANDLE(h.as_raw_handle()))
        .collect();
    let inherit = !inherited.is_empty();
    let mut handle_list = if inherit {
        Some(HandleList::new(inherited)?)
    } else {
        None
    };

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb =
        u32::try_from(size_of::<STARTUPINFOEXW>()).map_err(io::Error::other)?;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = raw(&copies[0]);
    startup.StartupInfo.hStdOutput = raw(&copies[1]);
    startup.StartupInfo.hStdError = raw(&copies[2]);
    if let Some(list) = handle_list.as_mut() {
        startup.lpAttributeList = list.as_list();
    }

    let mut process = PROCESS_INFORMATION::default();
    // Safety: every pointer is valid for the call: the NUL-terminated
    // application path and directory, the mutable NUL-terminated command line,
    // the double-NUL-terminated UTF-16 block, and the start-up structure whose
    // attribute list and handles outlive it. Returned handles are taken over.
    unsafe {
        CreateProcessW(
            PCWSTR::from_raw(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            inherit,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            Some(environment.as_ptr()),
            cwd_ptr,
            &raw const startup.StartupInfo,
            &raw mut process,
        )
        .map_err(|e| io_error(&e))?;
        drop(OwnedHandle::from_raw_handle(process.hThread.0));
        Ok(OwnedHandle::from_raw_handle(process.hProcess.0))
    }
}

/// Wait for a process and return its full 32-bit exit code.
fn wait_for_exit(process: &OwnedHandle) -> io::Result<u32> {
    let handle = HANDLE(process.as_raw_handle());
    let mut code = 0u32;
    // Safety: `handle` stays open for the call and `code` is a valid
    // out-parameter.
    unsafe {
        let _ = WaitForSingleObject(handle, INFINITE);
        GetExitCodeProcess(handle, &raw mut code).map_err(|e| io_error(&e))?;
    }
    Ok(code)
}

/// A pipe as (read end, write end). Neither end is inheritable; [`spawn`]
/// duplicates only what the child needs.
fn pipe() -> io::Result<(File, File)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    // Safety: valid out-parameters; both handles are owned once it succeeds.
    unsafe {
        CreatePipe(&raw mut read, &raw mut write, None, 0).map_err(|e| io_error(&e))?;
        Ok((
            File::from(OwnedHandle::from_raw_handle(read.0)),
            File::from(OwnedHandle::from_raw_handle(write.0)),
        ))
    }
}

/// Read `reader` to its end, keeping at most [`MAX_CAPTURED_STDERR`] bytes.
fn read_capped(mut reader: File) -> String {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(count) = reader.read(&mut chunk) {
        if count == 0 {
            break;
        }
        let room = MAX_CAPTURED_STDERR.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..count.min(room)]);
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// A raw handle for a [`File`].
fn handle_of(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// Runs one specific `op` program over pipes. The path is a plain parameter:
/// production gets it only from [`SystemOp`], which has no override.
#[derive(Debug, Clone)]
pub struct ProgramOp {
    /// The program to run.
    program: PathBuf,
}

impl ProgramOp {
    /// Run `program`.
    #[must_use]
    pub const fn with_program(program: PathBuf) -> Self {
        Self { program }
    }
}

impl OpRunner for ProgramOp {
    fn run(&self, call: &OpCall<'_>) -> Result<OpOutput, SecretsError> {
        let start_failed = |e: io::Error| SecretsError::Op(format!("op could not be started: {e}"));
        let (policy, overlay) = match call.token {
            Some(token) => (EnvPolicy::OpServiceAccount, vec![(TOKEN_VARIABLE, token)]),
            None => (EnvPolicy::OpOwner, Vec::new()),
        };
        let mut environment = build_env_block(std::env::vars_os(), policy, &overlay)
            .map_err(|e| SecretsError::Memory(format!("secure memory failed: {e}")))?;

        let args: Vec<OsString> = call.args.iter().map(OsString::from).collect();
        let mut command_line = exe_command_line(&self.program, &args);
        let (mut stdout_read, stdout_write) = pipe().map_err(start_failed)?;
        let (stderr_read, stderr_write) = pipe().map_err(start_failed)?;
        // No payload means an empty stdin: `op` must never wait on a console.
        let (stdin_read, stdin_write) = if call.stdin.is_some() {
            let (read, write) = pipe().map_err(start_failed)?;
            (read, Some(write))
        } else {
            let null = OpenOptions::new()
                .read(true)
                .open("NUL")
                .map_err(start_failed)?;
            (null, None)
        };

        // The child's directory is op's own folder, never the project's: a DLL
        // planted in a project must not be near a process holding the token.
        let spawned = spawn(
            &self.program,
            &mut command_line,
            &environment,
            self.program.parent(),
            [
                handle_of(&stdin_read),
                handle_of(&stdout_write),
                handle_of(&stderr_write),
            ],
        );
        // SECRET-011: wipe straight after, whatever happened.
        environment.wipe_now();
        drop(environment);
        let process = spawned.map_err(start_failed)?;
        // Our copies of the child's ends must close, or reads never finish.
        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);

        let payload = call.stdin;
        let (stdout, stderr) = std::thread::scope(|scope| {
            let stderr = scope.spawn(move || read_capped(stderr_read));
            // A separate thread: `op` may fill its stdout before it has
            // finished reading stdin, and each side must be able to proceed.
            // The closure captures only the borrowed bytes.
            if let (Some(mut writer), Some(bytes)) = (stdin_write, payload) {
                scope.spawn(move || {
                    // A child that exits early breaks the pipe; its exit code
                    // reports the failure.
                    let _ = writer.write_all(bytes);
                });
            }
            let stdout = LockedSecret::read_from(&mut stdout_read, MAX_SECRET_LEN);
            if stdout.is_err() {
                // Safety: `process` is a live handle we own.
                let _ = unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 1) };
            }
            (stdout, stderr.join().unwrap_or_default())
        });
        let stdout = stdout.map_err(|e| {
            SecretsError::Op(format!("the output of op could not be held securely: {e}"))
        })?;
        let exit_code = wait_for_exit(&process)
            .map_err(|e| SecretsError::Op(format!("op could not be waited for: {e}")))?;
        Ok(OpOutput {
            exit_code,
            stdout,
            stderr,
        })
    }
}

/// The production runner: locates the real `op` on each run. It takes no path
/// from the environment or config (SECRET-010).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemOp;

impl OpRunner for SystemOp {
    fn run(&self, call: &OpCall<'_>) -> Result<OpOutput, SecretsError> {
        ProgramOp::with_program(resolve_op()?).run(call)
    }
}

/// The caption of the `secrets add` approval dialog.
pub const APPROVAL_CAPTION: &str = "rustty secrets";

/// The production [`CopyApproval`]: a native Yes/No box with No as the default.
/// With no desktop to show it on, it fails, and that counts as no.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeApproval;

impl CopyApproval for NativeApproval {
    fn approve(&self, from: &str, vault: &str, name: &str) -> bool {
        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
        let text = wide(&copy_prompt(from, vault, name));
        let caption = wide(APPROVAL_CAPTION);
        // Safety: both strings are NUL-terminated and outlive the call.
        let answer = unsafe {
            MessageBoxW(
                None,
                PCWSTR::from_raw(text.as_ptr()),
                PCWSTR::from_raw(caption.as_ptr()),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
            )
        };
        answer == IDYES
    }
}

/// Ctrl+C and Ctrl+Break go to the command, which decides what they mean. This
/// process only waits so that it can report the exit code.
unsafe extern "system" fn leave_interrupts_to_the_command(event: u32) -> BOOL {
    BOOL::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
}

/// The production [`Launcher`]: `CreateProcessW` with inherited stdio and an
/// environment block that is wiped straight after (SECRET-011).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLauncher;

impl Launcher for SystemLauncher {
    fn launch(
        &self,
        argv: &[OsString],
        secrets: Vec<(String, LockedSecret)>,
    ) -> Result<u32, SecretsError> {
        let failed = |e: io::Error| SecretsError::Launch(format!("the command did not start: {e}"));
        let Some((command, args)) = argv.split_first() else {
            return Err(SecretsError::Launch("no command was given".to_owned()));
        };
        let path_var = std::env::var_os("PATH");
        let file =
            resolve_command(command, path_var.as_deref(), is_runnable_file).ok_or_else(|| {
                SecretsError::Launch(format!(
                    "{} was not found as a .exe, .com, .bat or .cmd; the current directory is only \
                 searched for an explicit path such as .\\tool.exe",
                    Path::new(command).display()
                ))
            })?;
        let file = std::path::absolute(&file).map_err(failed)?;
        let (application, mut command_line) = if is_batch(&file) {
            (
                system_program("cmd.exe").map_err(failed)?,
                batch_command_line(&file, args)?,
            )
        } else {
            let line = exe_command_line(&file, args);
            (file, line)
        };

        let overlay: Vec<(&str, &LockedSecret)> = secrets
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect();
        let mut environment = build_env_block(std::env::vars_os(), EnvPolicy::Command, &overlay)
            .map_err(|e| SecretsError::Memory(format!("secure memory failed: {e}")))?;

        static HANDLER: Once = Once::new();
        HANDLER.call_once(|| {
            // Safety: the handler is a plain function that lives for the whole
            // process, and it touches no shared state. A failure only means
            // Ctrl+C also ends this process, which loses the exit code.
            let _ = unsafe { SetConsoleCtrlHandler(Some(leave_interrupts_to_the_command), true) };
        });

        // Safety: GetStdHandle only reads this process's standard handles; a
        // missing one comes back invalid and `spawn` skips it.
        let stdio = unsafe {
            [
                GetStdHandle(STD_INPUT_HANDLE).unwrap_or_default(),
                GetStdHandle(STD_OUTPUT_HANDLE).unwrap_or_default(),
                GetStdHandle(STD_ERROR_HANDLE).unwrap_or_default(),
            ]
        };
        let spawned = spawn(&application, &mut command_line, &environment, None, stdio);

        // SECRET-011: the moment the kernel has its copy, ours goes: the block
        // and every secret, before the wait, not after the command ends.
        environment.wipe_now();
        drop(environment);
        drop(overlay);
        drop(secrets);

        let process = spawned.map_err(failed)?;
        wait_for_exit(&process).map_err(failed)
    }
}

#[cfg(test)]
#[path = "secret_exec_tests.rs"]
mod tests;
