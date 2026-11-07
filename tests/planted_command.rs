//! A bare program name never runs a file from the current folder (SHELL-030).
//! `CreateProcessW` with no application name searches there first, so a repository
//! could plant `claude.exe` and have it run for a boss. The planted file here is a
//! copy of `hostname.exe`, which prints a name and exits.
//!
//! This binary holds one test because it changes its process's working folder.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rustty::conpty::{ConPty, read_output_loop, wait_exit_code};

const PLANTED: &str = "rustty-planted-tool";

fn spawn(
    program: &str,
    args: &[&str],
) -> std::io::Result<(
    ConPty,
    rustty::conpty::SendHandle,
    rustty::conpty::SendHandle,
)> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    ConPty::spawn(program, &args, 80, 24, None, &BTreeMap::new(), None)
}

/// Be the terminal this console expects: it asks where the cursor is and waits for
/// the answer before the program starts, and it can't close while its output sits
/// unread.
fn serve_console(conpty: &ConPty) -> std::thread::JoinHandle<()> {
    conpty.write_input(b"\x1b[1;1R").expect("answered");
    let reader = conpty.output_reader().expect("reader");
    std::thread::spawn(move || {
        let _ = read_output_loop(&reader, |_| {});
    })
}

/// Run it to the end and return its exit code.
fn run(program: &str, args: &[&str]) -> std::io::Result<Option<i32>> {
    let (conpty, process, _thread) = spawn(program, args)?;
    let drain = serve_console(&conpty);
    let code = wait_exit_code(&process);
    drop(conpty);
    let _ = drain.join();
    Ok(code)
}

/// Whether it starts. Closing its console straight away ends whatever it was doing.
fn starts(program: &str, args: &[&str]) -> bool {
    let Ok((conpty, _process, _thread)) = spawn(program, args) else {
        return false;
    };
    let drain = serve_console(&conpty);
    drop(conpty);
    let _ = drain.join();
    true
}

struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        // Out of it first: Windows won't remove a process's working folder.
        let _ = std::env::set_current_dir(std::env::temp_dir());
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_bare_name_never_runs_a_file_planted_in_the_current_folder() {
    // Claude Code sets this for what it runs, and CreateProcessW honours it, which
    // would hide the attack. Nothing sets it for a window a person started.
    // Safety: this binary's only test, and it has started no other thread.
    unsafe { std::env::remove_var("NoDefaultCurrentDirectoryInExePath") };
    let root = std::env::var_os("SystemRoot").expect("SystemRoot");
    let harmless = Path::new(&root).join("System32").join("hostname.exe");
    let folder =
        Folder(std::env::temp_dir().join(format!("rustty-planted-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&folder.0);
    std::fs::create_dir_all(&folder.0).expect("folder");
    std::fs::copy(&harmless, folder.0.join(format!("{PLANTED}.exe"))).expect("copied");
    std::env::set_current_dir(&folder.0).expect("moved in");

    // The attack, with and without the extension.
    for name in [format!("{PLANTED}.exe"), PLANTED.to_string()] {
        let Err(error) = spawn(&name, &[]) else {
            panic!("{name} ran from the current folder");
        };
        assert_eq!(error.kind(), ErrorKind::NotFound, "{name}: {error}");
        let said = error.to_string();
        assert!(said.contains(&name) && said.contains("PATH"), "{said}");
    }

    // It is runnable, and an explicit path still reaches it: that is a choice.
    assert!(starts(&format!(".\\{PLANTED}.exe"), &[]));
    let absolute = folder.0.join(format!("{PLANTED}.exe"));
    assert!(starts(absolute.to_str().expect("text"), &[]));

    // Nothing else went missing: the programs the profiles name.
    assert_eq!(run("cmd.exe", &["/c", "exit 7"]).expect("cmd.exe"), Some(7));
    assert_eq!(run("cmd", &["/c", "exit 8"]).expect("cmd"), Some(8));
    assert!(starts(
        "powershell.exe",
        &["-NoProfile", "-Command", "exit"]
    ));
    assert!(starts("wsl.exe", &["--help"]));
    // App Execution Aliases are zero-byte reparse points in WindowsApps.
    let aliases = std::env::var_os("LOCALAPPDATA")
        .map(|dir| Path::new(&dir).join(r"Microsoft\WindowsApps"))
        .expect("LOCALAPPDATA");
    if aliases.join("winget.exe").symlink_metadata().is_ok() {
        assert!(starts("winget.exe", &["--version"]), "the alias");
    }
    for bash in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
    ] {
        if Path::new(bash).is_file() {
            assert!(starts(bash, &["--version"]), "{bash}");
        }
    }
}
