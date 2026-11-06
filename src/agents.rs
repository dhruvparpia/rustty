//! Which agent CLI a pane is running (RESTORE-002), so a restart can offer to
//! resume it. Detection reads the process tree under the pane's shell. An agent
//! that runs as a plain `node.exe` cannot be told apart by name and is not found.

/// One running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub parent: u32,
    /// Executable file name.
    pub exe: String,
}

/// The agent an executable name belongs to.
#[must_use]
pub fn agent_from_exe(exe: &str) -> Option<&'static str> {
    let name = exe.to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        _ => None,
    }
}

/// The command that continues `agent`'s latest conversation in this folder.
#[must_use]
pub fn resume_command(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude --continue"),
        "codex" => Some("codex resume --last"),
        _ => None,
    }
}

/// The first agent at or below `root` in the process tree.
#[must_use]
pub fn find_agent(table: &[Proc], root: u32) -> Option<&'static str> {
    find_agent_process(table, root).map(|(agent, _)| agent)
}

/// The first agent at or below `root`, with its process id.
#[must_use]
pub fn find_agent_process(table: &[Proc], root: u32) -> Option<(&'static str, u32)> {
    let mut seen = vec![root];
    let mut next = 0;
    while let Some(&pid) = seen.get(next) {
        next += 1;
        for proc in table {
            if proc.pid == pid
                && let Some(agent) = agent_from_exe(&proc.exe)
            {
                return Some((agent, pid));
            }
            if proc.parent == pid && !seen.contains(&proc.pid) {
                seen.push(proc.pid);
            }
        }
    }
    None
}

/// A process's working directory in the form panes keep: forward slashes and
/// no trailing slash, except at a drive root.
#[must_use]
pub fn clean_process_cwd(path: &str) -> Option<String> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') {
        Some(format!("{trimmed}/"))
    } else {
        Some(trimmed.to_string())
    }
}

/// What one pane's process tree holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    /// The pane's shell process.
    pub root: u32,
    /// The agent CLI at or below it.
    pub agent: Option<&'static str>,
    /// The folder of that agent, or of the shell when there is no agent.
    pub cwd: Option<String>,
}

/// Look at the process tree under `root`.
#[must_use]
pub fn inspect(table: &[Proc], root: u32) -> Inspection {
    let found = find_agent_process(table, root);
    Inspection {
        root,
        agent: found.map(|(agent, _)| agent),
        cwd: process_cwd(found.map_or(root, |(_, pid)| pid)),
    }
}

/// Every running process, or nothing when the system will not say.
#[cfg(windows)]
#[allow(unsafe_code, reason = "RESTORE-002: Toolhelp process snapshot FFI")]
#[must_use]
pub fn process_table() -> Vec<Proc> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut table = Vec::new();
    // Safety: the snapshot handle is checked and closed here, and `entry` is
    // sized as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return table;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            table.push(Proc {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    table
}

/// A process's working directory, read from its process block (RESTORE-002).
/// The layout is undocumented but has been stable for every 64-bit Windows;
/// anything unexpected reads as "don't know". A 32-bit target is not supported.
#[cfg(windows)]
#[allow(unsafe_code, reason = "RESTORE-002: reading a child process's PEB")]
#[must_use]
pub fn process_cwd(pid: u32) -> Option<String> {
    use std::ffi::c_void;
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    /// `PEB.ProcessParameters` on 64-bit Windows.
    const PARAMETERS: usize = 0x20;
    /// `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath` on 64-bit Windows.
    const CURRENT_DIRECTORY: usize = 0x38;
    /// Longest working directory Windows allows.
    const MAX_BYTES: usize = 32_768 * 2;

    fn read(process: HANDLE, at: usize, into: &mut [u8]) -> Option<()> {
        // Safety: `into` is valid for its length, and the kernel checks the
        // address in the target process.
        unsafe {
            ReadProcessMemory(
                process,
                at as *const c_void,
                into.as_mut_ptr().cast(),
                into.len(),
                None,
            )
        }
        .ok()
    }
    fn read_cwd(process: HANDLE) -> Option<String> {
        let mut basic = PROCESS_BASIC_INFORMATION::default();
        // Safety: `basic` is the size the information class asks for.
        unsafe {
            NtQueryInformationProcess(
                process,
                ProcessBasicInformation,
                (&raw mut basic).cast(),
                u32::try_from(std::mem::size_of::<PROCESS_BASIC_INFORMATION>()).ok()?,
                std::ptr::null_mut(),
            )
        }
        .ok()
        .ok()?;
        let mut pointer = [0u8; 8];
        read(
            process,
            basic.PebBaseAddress as usize + PARAMETERS,
            &mut pointer,
        )?;
        let parameters = usize::try_from(u64::from_le_bytes(pointer)).ok()?;
        // UNICODE_STRING: u16 length, u16 capacity, padding, then the pointer.
        let mut header = [0u8; 16];
        read(
            process,
            parameters.checked_add(CURRENT_DIRECTORY)?,
            &mut header,
        )?;
        let length = usize::from(u16::from_le_bytes([header[0], header[1]]));
        if length == 0 || length > MAX_BYTES || length % 2 != 0 {
            return None;
        }
        let buffer = usize::try_from(u64::from_le_bytes(header[8..16].try_into().ok()?)).ok()?;
        let mut bytes = vec![0u8; length];
        read(process, buffer, &mut bytes)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
    // Safety: the handle is checked and closed here.
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?;
    let path = read_cwd(process);
    // Safety: `process` came from OpenProcess above.
    let _ = unsafe { CloseHandle(process) };
    clean_process_cwd(&path?)
}

/// A process's working directory: unknown on other platforms for now.
#[cfg(not(windows))]
#[must_use]
pub fn process_cwd(_pid: u32) -> Option<String> {
    None
}

/// Every running process: nothing on other platforms for now.
#[cfg(not(windows))]
#[must_use]
pub fn process_table() -> Vec<Proc> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, exe: &str) -> Proc {
        Proc {
            pid,
            parent,
            exe: exe.to_string(),
        }
    }

    #[test]
    fn known_agents_are_found_by_name() {
        assert_eq!(agent_from_exe("claude.exe"), Some("claude"));
        assert_eq!(agent_from_exe("CLAUDE.EXE"), Some("claude"));
        assert_eq!(agent_from_exe("codex.exe"), Some("codex"));
        assert_eq!(agent_from_exe("node.exe"), None);
        assert_eq!(agent_from_exe("claude-helper.exe"), None);
    }

    #[test]
    fn every_known_agent_can_resume() {
        assert_eq!(resume_command("claude"), Some("claude --continue"));
        assert_eq!(resume_command("codex"), Some("codex resume --last"));
        assert_eq!(resume_command("vim"), None);
    }

    #[test]
    fn the_agent_is_found_below_the_shell() {
        let table = [
            proc(1, 0, "explorer.exe"),
            proc(10, 1, "pwsh.exe"),
            proc(20, 10, "cmd.exe"),
            proc(30, 20, "claude.exe"),
            proc(40, 1, "codex.exe"),
        ];
        assert_eq!(find_agent(&table, 10), Some("claude"));
        assert_eq!(find_agent(&table, 30), Some("claude"), "the root counts");
        assert_eq!(find_agent(&table, 20), Some("claude"));
    }

    #[test]
    fn another_sessions_agent_is_not_ours() {
        // Break caught: resuming an agent in the wrong session.
        let table = [
            proc(10, 1, "pwsh.exe"),
            proc(11, 1, "pwsh.exe"),
            proc(30, 11, "claude.exe"),
        ];
        assert_eq!(find_agent(&table, 10), None);
        assert_eq!(find_agent(&table, 99), None);
    }

    #[test]
    fn a_cycle_in_reused_process_ids_does_not_hang() {
        // Windows reuses ids, so a parent link can point anywhere.
        let table = [proc(10, 20, "pwsh.exe"), proc(20, 10, "cmd.exe")];
        assert_eq!(find_agent(&table, 10), None);
    }

    #[test]
    fn a_process_folder_reads_like_a_reported_one() {
        assert_eq!(
            clean_process_cwd(r"C:\src\rust\").as_deref(),
            Some("C:/src/rust")
        );
        assert_eq!(clean_process_cwd(r"C:\").as_deref(), Some("C:/"));
        assert_eq!(clean_process_cwd("").as_deref(), None);
        assert_eq!(clean_process_cwd("  ").as_deref(), None);
    }

    #[cfg(windows)]
    #[test]
    fn this_process_folder_can_be_read() {
        // Break caught: a Windows layout change silently losing every resume.
        let want = std::env::current_dir().expect("current dir");
        let want = clean_process_cwd(&want.to_string_lossy());
        assert_eq!(process_cwd(std::process::id()), want);
    }
}
