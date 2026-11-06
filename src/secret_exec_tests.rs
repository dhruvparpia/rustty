//! Tests for `secret_exec` (SECRET-010, SECRET-011). Real child processes, but
//! never the real `op` against an account: the children are `cmd.exe` and
//! `findstr.exe` from System32. The one test that runs the real `op` is
//! ignored by default and only asks for `--version`.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::secrets::{EXIT_LAUNCH, OpCall, OpOutput};
use crate::secure_mem::LockedSecret;

const TOKEN: &str = "ops_THROWAWAY_TEST_TOKEN_0123456789";

#[test]
#[ignore = "opens a message box; run by hand"]
fn the_approval_dialog_names_both_ends_and_no_is_no() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetDlgItemTextW, IDNO, PostMessageW, WM_COMMAND,
    };
    let answer = std::thread::spawn(|| {
        NativeApproval.approve("op://Private/Item/field", "rustty-demo-12345678", "API_KEY")
    });
    let caption: Vec<u16> = APPROVAL_CAPTION.encode_utf16().chain(Some(0)).collect();
    let mut found = None;
    for _ in 0..200 {
        // Safety: the caption is NUL-terminated.
        if let Ok(hwnd) = unsafe { FindWindowW(None, PCWSTR::from_raw(caption.as_ptr())) } {
            found = Some(hwnd);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let hwnd = found.expect("the dialog appeared");
    let mut text = [0u16; 1024];
    // Safety: the buffer is valid for its length; 0xFFFF is a message box's text.
    let len = unsafe { GetDlgItemTextW(hwnd, 0xFFFF, &mut text) };
    let text = String::from_utf16_lossy(&text[..len as usize]);
    // Answer the box this test opened, and nothing else.
    // Safety: posting a message to a window handle that was just found.
    unsafe { PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(IDNO.0 as usize), LPARAM(0)) }.unwrap();
    assert!(!answer.join().unwrap(), "No is no");
    assert!(text.contains("op://Private/Item/field"), "{text}");
    assert!(text.contains("rustty-demo-12345678"), "{text}");
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustty-secret-exec-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().collect()
}

fn from_wide(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn system(program: &str) -> PathBuf {
    system_program(program).unwrap()
}

fn plaintext(secret: &LockedSecret) -> String {
    secret
        .with_plaintext(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap()
}

fn cmd_op() -> ProgramOp {
    ProgramOp::with_program(system("cmd.exe"))
}

fn run_op(op: &ProgramOp, args: &[&str], token: Option<&LockedSecret>) -> OpOutput {
    op.run(&OpCall {
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        token,
        stdin: None,
    })
    .unwrap()
}

// ---- command lines -----------------------------------------------------

#[test]
fn arguments_are_quoted_for_command_line_to_argv() {
    for (raw, quoted) in [
        ("plain", "plain"),
        (r"C:\plain\path", r"C:\plain\path"),
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("a\"b", "\"a\\\"b\""),
        (r"a\ b\", r#""a\ b\\""#),
        ("a\\\"b c", "\"a\\\\\\\"b c\""),
        ("tab\there", "\"tab\there\""),
    ] {
        let mut out = Vec::new();
        push_quoted(&mut out, &wide(raw));
        assert_eq!(from_wide(&out), quoted, "for {raw:?}");
    }
}

#[test]
fn exe_command_line_is_program_then_arguments() {
    let args = [OsString::from("a b"), OsString::from("c")];
    let line = exe_command_line(Path::new(r"C:\Program Files\x.exe"), &args);
    assert_eq!(line.last(), Some(&0));
    assert_eq!(
        from_wide(&line[..line.len() - 1]),
        r#""C:\Program Files\x.exe" "a b" c"#
    );
}

#[test]
fn batch_command_line_quotes_everything_for_cmd() {
    let args = [
        OsString::from("a b"),
        OsString::from(r"C:\dir\"),
        OsString::new(),
        OsString::from("x&y"),
    ];
    let line = batch_command_line(Path::new(r"C:\t\s.cmd"), &args).unwrap();
    assert_eq!(
        from_wide(&line[..line.len() - 1]),
        r#"cmd.exe /e:ON /v:OFF /d /s /c ""C:\t\s.cmd" "a b" "C:\dir\\" "" "x&y"""#
    );
}

#[test]
fn batch_command_line_refuses_what_cmd_would_reinterpret() {
    for bad in ["a\"b", "%PATH%", "a\rb", "a\nb"] {
        let args = [OsString::from(bad)];
        let err = batch_command_line(Path::new(r"C:\t\s.cmd"), &args)
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_LAUNCH));
        assert!(!err.to_string().contains(bad), "{err}");
    }
    assert!(batch_command_line(Path::new(r"C:\100%\s.cmd"), &[]).is_err());
}

// ---- resolving the command ---------------------------------------------

/// `resolve_command` over a fixed set of files, case-insensitive.
fn resolve(cmd: &str, path_var: &str, files: &[&str]) -> Option<PathBuf> {
    resolve_command(OsStr::new(cmd), Some(OsStr::new(path_var)), |p| {
        files
            .iter()
            .any(|f| p.to_string_lossy().eq_ignore_ascii_case(f))
    })
}

#[test]
fn a_bare_command_is_searched_on_path_in_order_with_extensions() {
    let files = [r"C:\b\tool.exe", r"C:\c\tool.exe"];
    assert_eq!(
        resolve("tool", r"C:\a;C:\b;C:\c", &files),
        Some(PathBuf::from(r"C:\b\tool.exe"))
    );
    assert_eq!(
        resolve("tool", r"C:\a", &[r"C:\a\tool.cmd"]),
        Some(PathBuf::from(r"C:\a\tool.cmd"))
    );
    assert_eq!(
        resolve("tool", r"C:\a", &[r"C:\a\tool.cmd", r"C:\a\tool.exe"]),
        Some(PathBuf::from(r"C:\a\tool.exe")),
        "an exe wins over a script of the same name"
    );
    assert_eq!(
        resolve("tool.cmd", r"C:\a", &[r"C:\a\tool.cmd"]),
        Some(PathBuf::from(r"C:\a\tool.cmd"))
    );
    assert_eq!(
        resolve("tool.v2", r"C:\a", &[r"C:\a\tool.v2.exe"]),
        Some(PathBuf::from(r"C:\a\tool.v2.exe"))
    );
}

#[test]
fn a_bare_command_is_never_taken_from_the_current_directory() {
    // Only an explicit relative path may run something from there.
    let cwd_file = std::env::current_dir()
        .unwrap()
        .join("planted.exe")
        .to_string_lossy()
        .into_owned();
    assert_eq!(resolve("planted", r"C:\a", &[&cwd_file]), None);
    assert_eq!(resolve("planted", "", &[&cwd_file, "planted.exe"]), None);
}

#[test]
fn an_explicit_path_skips_path_and_only_runnable_files_count() {
    assert_eq!(
        resolve(r"C:\x\tool", r"C:\a", &[r"C:\x\tool.exe", r"C:\a\tool.exe"]),
        Some(PathBuf::from(r"C:\x\tool.exe"))
    );
    assert_eq!(
        resolve(r".\tool.exe", r"C:\a", &[r".\tool.exe"]),
        Some(PathBuf::from(r".\tool.exe"))
    );
    assert_eq!(resolve("notes.txt", r"C:\a", &[r"C:\a\notes.txt"]), None);
    assert_eq!(resolve(r"C:\x\run.ps1", r"C:\a", &[r"C:\x\run.ps1"]), None);
    assert_eq!(resolve("missing", r"C:\a", &[]), None);
}

#[test]
fn batch_files_are_recognised_by_extension() {
    assert!(is_batch(Path::new(r"C:\a\x.cmd")));
    assert!(is_batch(Path::new(r"C:\a\x.BAT")));
    assert!(!is_batch(Path::new(r"C:\a\x.exe")));
    assert!(!is_batch(Path::new(r"C:\a\cmd")));
}

// ---- resolving op ------------------------------------------------------

#[test]
fn op_candidates_prefer_the_machine_install_and_never_consult_path() {
    let candidates = op_candidates(
        Path::new(r"C:\Program Files"),
        Path::new(r"C:\Users\u\AppData\Local"),
        &[PathBuf::from(
            r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\AgileBits.1Password.CLI_x",
        )],
    );
    assert_eq!(
        candidates,
        [
            PathBuf::from(r"C:\Program Files\1Password CLI\op.exe"),
            PathBuf::from(r"C:\Program Files\WinGet\Links\op.exe"),
            PathBuf::from(r"C:\Users\u\AppData\Local\Microsoft\WinGet\Links\op.exe"),
            PathBuf::from(
                r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\AgileBits.1Password.CLI_x\op.exe"
            ),
        ]
    );
}

#[test]
fn known_folders_do_not_come_from_the_environment() {
    // A tampered LOCALAPPDATA cannot be observed from here without racing
    // other tests; what can be pinned is that the lookup succeeds and that
    // the store lands in the expected place under that known folder.
    let dir = token_store_dir().unwrap();
    assert!(dir.ends_with(Path::new(r"rustty\secrets")), "{dir:?}");
    assert!(dir.is_absolute());
}

// ---- the environment block ---------------------------------------------

fn block_entries(pages: &mut crate::secure_mem::LockedPages) -> Vec<String> {
    let units = pages.as_wide_mut();
    let mut entries = Vec::new();
    let mut start = 0;
    loop {
        let len = units[start..].iter().position(|u| *u == 0).unwrap();
        if len == 0 {
            return entries;
        }
        entries.push(from_wide(&units[start..start + len]));
        start += len + 1;
    }
}

fn inherited(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
}

#[test]
fn block_is_sorted_and_secrets_replace_inherited_values_case_insensitively() {
    let secret = LockedSecret::from_slice("sécret".as_bytes()).unwrap();
    let mut block = build_env_block(
        inherited(&[
            ("Zed", "1"),
            ("=C:", r"C:\x"),
            ("my_key", "old"),
            ("alpha", "2"),
        ]),
        EnvPolicy::Command,
        &[("MY_KEY", &secret)],
    )
    .unwrap();
    assert_eq!(
        block_entries(&mut block),
        [r"=C:=C:\x", "alpha=2", "MY_KEY=sécret", "Zed=1"]
    );
}

#[test]
fn block_wipes_to_all_zeros() {
    let secret = LockedSecret::from_slice(b"value").unwrap();
    let mut block = build_env_block(inherited(&[]), EnvPolicy::Command, &[("A", &secret)]).unwrap();
    block.wipe_now();
    assert!(block.as_mut_slice().iter().all(|b| *b == 0));
}

#[test]
fn an_empty_environment_is_still_a_valid_block() {
    let mut block = build_env_block(inherited(&[]), EnvPolicy::Command, &[]).unwrap();
    assert_eq!(&block.as_wide_mut()[..2], &[0, 0]);
}

#[test]
fn a_stray_token_never_reaches_the_command() {
    let mut block = build_env_block(
        inherited(&[("op_service_account_token", "x"), ("KEEP", "1")]),
        EnvPolicy::Command,
        &[],
    )
    .unwrap();
    assert_eq!(block_entries(&mut block), ["KEEP=1"]);
}

#[test]
fn the_service_account_run_of_op_ignores_inherited_op_settings() {
    // OP_CONNECT_HOST would divert op from the account; OP_DEBUG would make
    // it chatty; neither may survive into a run holding the token.
    let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
    let mut block = build_env_block(
        inherited(&[
            ("OP_CONNECT_HOST", "http://attacker"),
            ("OP_CONNECT_TOKEN", "x"),
            ("OP_DEBUG", "true"),
            ("OP_SESSION_abc", "x"),
            ("OP_SERVICE_ACCOUNT_TOKEN", "other"),
            ("KEEP", "1"),
        ]),
        EnvPolicy::OpServiceAccount,
        &[(TOKEN_VARIABLE, &token)],
    )
    .unwrap();
    assert_eq!(
        block_entries(&mut block),
        [
            "KEEP=1".to_owned(),
            format!("OP_SERVICE_ACCOUNT_TOKEN={TOKEN}")
        ]
    );
}

#[test]
fn the_owner_run_of_op_keeps_account_choice_but_drops_service_credentials() {
    let mut block = build_env_block(
        inherited(&[
            ("OP_ACCOUNT", "me"),
            ("OP_CONNECT_HOST", "x"),
            ("OP_CONNECT_TOKEN", "x"),
            ("OP_SERVICE_ACCOUNT_TOKEN", "x"),
        ]),
        EnvPolicy::OpOwner,
        &[],
    )
    .unwrap();
    assert_eq!(block_entries(&mut block), ["OP_ACCOUNT=me"]);
}

#[test]
fn a_command_gets_no_1password_settings_and_no_automation_token() {
    // Break caught: an owner's OP_SESSION_ or a boss's write token reaching whatever
    // `secrets run` starts, which is exactly the program the secret is for.
    let mut block = build_env_block(
        inherited(&[
            ("KEEP", "yes"),
            ("OP_SESSION_abc", "x"),
            ("op_account", "x"),
            ("OP_CONNECT_HOST", "x"),
            ("OP_SERVICE_ACCOUNT_TOKEN", "x"),
            ("RUSTTY_AUTOMATION_TOKEN", "x"),
            ("rustty_automation_token", "x"),
            ("RUSTTY_WINDOW", "7"),
        ]),
        EnvPolicy::Command,
        &[],
    )
    .unwrap();
    assert_eq!(block_entries(&mut block), ["KEEP=yes", "RUSTTY_WINDOW=7"]);
}

#[test]
fn op_gets_neither_automation_variable() {
    for policy in [EnvPolicy::OpOwner, EnvPolicy::OpServiceAccount] {
        let mut block = build_env_block(
            inherited(&[
                ("KEEP", "yes"),
                ("RUSTTY_AUTOMATION_TOKEN", "x"),
                ("rustty_window", "7"),
            ]),
            policy,
            &[],
        )
        .unwrap();
        assert_eq!(block_entries(&mut block), ["KEEP=yes"], "{policy:?}");
    }
}

#[test]
fn a_command_that_is_not_found_is_not_quoted() {
    // Break caught: a secret pasted where the command goes coming back in an error
    // that ends up in a log (SECRET-002).
    let argv = [OsString::from("sk-live-not-a-program-0123456789")];
    let error = SystemLauncher.launch(&argv, Vec::new()).unwrap_err();
    assert_eq!(error.exit_code(), u32::from(EXIT_LAUNCH));
    let said = error.to_string();
    assert!(!said.contains("sk-live"), "{said}");
    assert!(said.contains("not found"), "{said}");
}

#[test]
fn a_secret_that_is_not_text_fails_the_block() {
    let secret = LockedSecret::from_slice(&[0xff, 0xfe]).unwrap();
    assert!(build_env_block(inherited(&[]), EnvPolicy::Command, &[("A", &secret)]).is_err());
}

// ---- running a stand-in op over pipes ----------------------------------

#[test]
fn the_runner_captures_stdout_and_the_exit_code() {
    let output = run_op(&cmd_op(), &["/c", "echo", "hello"], None);
    assert_eq!(output.exit_code, 0);
    assert_eq!(plaintext(&output.stdout).trim_end(), "hello");

    let output = run_op(&cmd_op(), &["/c", "exit", "7"], None);
    assert_eq!(output.exit_code, 7);
    assert!(output.stdout.is_empty());
}

#[test]
fn the_token_reaches_only_the_child_environment() {
    let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
    let output = run_op(
        &cmd_op(),
        &["/c", "echo", "%OP_SERVICE_ACCOUNT_TOKEN%"],
        Some(&token),
    );
    assert_eq!(plaintext(&output.stdout).trim_end(), TOKEN);
    assert_eq!(
        std::env::var_os(TOKEN_VARIABLE),
        None,
        "not in this process"
    );

    // Without a token the variable is simply absent in the child.
    let output = run_op(
        &cmd_op(),
        &["/c", "echo", "%OP_SERVICE_ACCOUNT_TOKEN%"],
        None,
    );
    assert_eq!(
        plaintext(&output.stdout).trim_end(),
        "%OP_SERVICE_ACCOUNT_TOKEN%"
    );
}

#[test]
fn the_runner_feeds_stdin_and_a_missing_stdin_is_empty() {
    let op = ProgramOp::with_program(system("findstr.exe"));
    let args = vec!["/r".to_owned(), ".".to_owned()];
    let output = op
        .run(&OpCall {
            args: args.clone(),
            token: None,
            stdin: Some(b"line-one\r\nline-two\r\n"),
        })
        .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(plaintext(&output.stdout), "line-one\r\nline-two\r\n");

    // No stdin: findstr ends at once with "no match" instead of hanging.
    let output = op
        .run(&OpCall {
            args,
            token: None,
            stdin: None,
        })
        .unwrap();
    assert_eq!(output.exit_code, 1);
}

#[test]
fn a_payload_bigger_than_a_pipe_buffer_does_not_deadlock() {
    let op = ProgramOp::with_program(system("findstr.exe"));
    let line = "x".repeat(79) + "\n";
    let payload = line.repeat(500);
    let output = op
        .run(&OpCall {
            args: vec!["/r".to_owned(), ".".to_owned()],
            token: None,
            stdin: Some(payload.as_bytes()),
        })
        .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout.len(), payload.len());
}

#[test]
fn stderr_is_captured_and_a_flood_of_it_does_not_block_the_child() {
    let output = run_op(&cmd_op(), &["/c", "echo boom 1>&2 & exit /b 3"], None);
    assert_eq!(output.exit_code, 3);
    assert!(output.stderr.contains("boom"), "{}", output.stderr);

    let dir = TempDir::new();
    let flood = dir.path().join("flood.txt");
    std::fs::write(&flood, "e".repeat(300_000)).unwrap();
    let command = format!("type {} 1>&2", flood.display());
    let output = run_op(&cmd_op(), &["/c", &command], None);
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.len() <= MAX_CAPTURED_STDERR);
}

#[test]
fn oversized_stdout_is_refused_and_the_child_does_not_linger() {
    let dir = TempDir::new();
    let big = dir.path().join("big.txt");
    std::fs::write(&big, "v".repeat(crate::secure_mem::MAX_SECRET_LEN + 1)).unwrap();
    let command = format!("type {}", big.display());
    let err = cmd_op()
        .run(&OpCall {
            args: vec!["/c".to_owned(), command],
            token: None,
            stdin: None,
        })
        .err()
        .unwrap();
    assert_eq!(err.exit_code(), u32::from(crate::secrets::EXIT_OP));
    assert!(!err.to_string().contains("vvv"), "{err}");
}

#[test]
fn a_missing_program_is_an_op_error() {
    let dir = TempDir::new();
    let op = ProgramOp::with_program(dir.path().join("nope.exe"));
    let err = op
        .run(&OpCall {
            args: Vec::new(),
            token: None,
            stdin: None,
        })
        .err()
        .unwrap();
    assert_eq!(err.exit_code(), u32::from(crate::secrets::EXIT_OP));
}

#[test]
fn op_runs_outside_the_project_folder() {
    // A DLL planted in a project must not be near a process holding the token.
    let output = run_op(&cmd_op(), &["/c", "cd"], None);
    let cwd = plaintext(&output.stdout);
    assert_eq!(
        cwd.trim_end().to_ascii_lowercase(),
        system("cmd.exe")
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase()
    );
}

// ---- launching the command ---------------------------------------------

#[test]
fn launch_sets_the_environment_and_returns_the_exit_code() {
    let dir = TempDir::new();
    let out = dir.path().join("out.txt");
    let secret = LockedSecret::from_slice(b"launched-value").unwrap();
    let script = format!("echo %RUSTTY_TEST_LAUNCH%>{} & exit /b 7", out.display());
    let argv = vec![
        system("cmd.exe").into_os_string(),
        OsString::from("/c"),
        OsString::from(script),
    ];
    let code = SystemLauncher
        .launch(&argv, vec![("RUSTTY_TEST_LAUNCH".to_owned(), secret)])
        .unwrap();
    assert_eq!(code, 7);
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().trim_end(),
        "launched-value"
    );
    assert_eq!(std::env::var_os("RUSTTY_TEST_LAUNCH"), None);
}

#[test]
fn launch_keeps_a_full_32_bit_exit_code() {
    let argv = vec![
        system("cmd.exe").into_os_string(),
        OsString::from("/c"),
        OsString::from("exit 3221225477"),
    ];
    assert_eq!(
        SystemLauncher.launch(&argv, Vec::new()).unwrap(),
        0xC000_0005
    );
}

#[test]
fn launch_finds_a_bare_command_on_path() {
    // cmd.exe is on every PATH through System32.
    let argv = vec![
        OsString::from("cmd"),
        OsString::from("/c"),
        OsString::from("exit 4"),
    ];
    assert_eq!(SystemLauncher.launch(&argv, Vec::new()).unwrap(), 4);
}

#[test]
fn launch_runs_a_batch_file_without_letting_arguments_escape() {
    let dir = TempDir::new();
    let folder = dir.path().join("a b");
    std::fs::create_dir_all(&folder).unwrap();
    let script = folder.join("s.cmd");
    std::fs::write(
        &script,
        "@>\"%~dp0out.txt\" echo \"[%~1]\"[%RUSTTY_TEST_BATCH%]\r\n@exit /b 5\r\n",
    )
    .unwrap();
    let marker = dir.path().join("pwned.txt");
    let injected = format!("v&type nul>{}", marker.display());
    let secret = LockedSecret::from_slice(b"batch-value").unwrap();
    let argv = vec![script.into_os_string(), OsString::from(&injected)];
    let code = SystemLauncher
        .launch(&argv, vec![("RUSTTY_TEST_BATCH".to_owned(), secret)])
        .unwrap();
    assert_eq!(code, 5);
    assert!(!marker.exists(), "an argument must not run as a command");
    let written = std::fs::read_to_string(folder.join("out.txt")).unwrap();
    assert!(written.contains("batch-value"), "{written}");
    assert!(written.contains(&injected), "passed whole: {written}");
}

#[test]
fn launch_refuses_a_batch_argument_it_cannot_quote() {
    let dir = TempDir::new();
    let script = dir.path().join("s.cmd");
    std::fs::write(&script, "@exit /b 0\r\n").unwrap();
    let argv = vec![script.into_os_string(), OsString::from("a\"&calc")];
    let err = SystemLauncher.launch(&argv, Vec::new()).err().unwrap();
    assert_eq!(err.exit_code(), u32::from(EXIT_LAUNCH));
}

#[test]
fn launch_reports_a_command_it_cannot_find() {
    let argv = vec![OsString::from("rustty-no-such-command-xyz")];
    let err = SystemLauncher.launch(&argv, Vec::new()).err().unwrap();
    assert_eq!(err.exit_code(), u32::from(EXIT_LAUNCH));
    assert!(SystemLauncher.launch(&[], Vec::new()).is_err());
}

#[test]
fn a_child_inherits_only_its_standard_handles() {
    // An inheritable handle elsewhere in this process, as another thread or
    // library might hold. If the child inherited it, the read below would not
    // see the end of the pipe until the child exited.
    let (mut read, write) = pipe().unwrap();
    let inheritable = inheritable_copy(handle_of(&write)).unwrap();
    drop(write);

    let cmd = system("cmd.exe");
    let args = [
        OsString::from("/c"),
        OsString::from("ping -n 6 127.0.0.1 >NUL"),
    ];
    let mut line = exe_command_line(&cmd, &args);
    let block = build_env_block(std::env::vars_os(), EnvPolicy::Command, &[]).unwrap();
    let null = OpenOptions::new().read(true).open("NUL").unwrap();
    let handles = [handle_of(&null); 3];
    let process = spawn(&cmd, &mut line, &block, None, handles).unwrap();
    drop(inheritable);

    let started = std::time::Instant::now();
    let mut buffer = [0u8; 16];
    let count = read.read(&mut buffer).unwrap();
    let waited = started.elapsed();
    // Safety: a live handle we own; a test only.
    let _ = unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 0) };
    assert_eq!(count, 0, "the only writer is gone");
    // The child runs for about five seconds. If it had inherited the handle,
    // the read would have lasted as long as the child did.
    assert!(waited.as_secs() < 3, "read waited {waited:?} for the child");
}

// ---- the whole of `secrets run`, with only op faked ---------------------

/// A stand-in `op` that answers every `read` with one value, and checks it was
/// given the token.
struct ReadOnlyOp;

impl OpRunner for ReadOnlyOp {
    fn run(&self, call: &OpCall<'_>) -> Result<OpOutput, crate::secrets::SecretsError> {
        assert_eq!(call.args[0], "read");
        assert_eq!(call.token.map(plaintext).as_deref(), Some(TOKEN));
        Ok(OpOutput {
            exit_code: 0,
            stdout: LockedSecret::from_slice(b"end-to-end-value").unwrap(),
            stderr: String::new(),
        })
    }
}

#[test]
fn secrets_run_gives_the_real_child_the_value_and_never_the_token() {
    use crate::secrets::{Deps, FileTokenStore, Manifest, Project, SecretsVerb, TokenStore};
    use crate::secrets::{execute, stored_reference};

    let dir = TempDir::new();
    let root = dir.path().join("project");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let project = Project::resolve(Some(&root)).unwrap();
    let mut manifest = Manifest::default();
    for name in ["WANTED", "NOT_WANTED"] {
        manifest
            .secrets
            .insert(name.to_owned(), stored_reference(&project.vault, name));
    }
    manifest.save(&project.root).unwrap();
    let store = FileTokenStore::new(dir.path().join("tokens"));
    let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
    store.save(&project.vault, &token).unwrap();

    let out = dir.path().join("env.txt");
    let script = format!(
        "(echo [%WANTED%] & echo [%NOT_WANTED%] & echo [%OP_SERVICE_ACCOUNT_TOKEN%])>{} & exit /b 9",
        out.display()
    );
    let verb = SecretsVerb::Run {
        only: vec!["WANTED".to_owned()],
        dir: Some(root),
        argv: vec![
            system("cmd.exe").into_os_string(),
            OsString::from("/c"),
            OsString::from(script),
        ],
    };
    let deps = Deps {
        op: &ReadOnlyOp,
        store: &store,
        launcher: &SystemLauncher,
        // `run` never asks.
        approval: &NativeApproval,
    };
    let (mut printed, mut errors) = (Vec::new(), Vec::new());
    let code = execute(&verb, &deps, &mut printed, &mut errors);
    assert_eq!(code, 9, "{}", String::from_utf8_lossy(&errors));

    let seen = std::fs::read_to_string(&out).unwrap();
    assert!(seen.contains("[end-to-end-value]"), "{seen}");
    assert!(seen.contains("[%NOT_WANTED%]"), "not requested: {seen}");
    assert!(seen.contains("[%OP_SERVICE_ACCOUNT_TOKEN%]"), "{seen}");
    assert!(!seen.contains(TOKEN), "{seen}");
    assert!(printed.is_empty());
}

// ---- the real op, version only -----------------------------------------

/// Proves the production resolver and spawn path against the installed `op`
/// without touching the account: `--version` is one of the two allowed calls.
/// Run by hand: `cargo test --lib real_op_version -- --ignored`.
#[test]
#[ignore = "needs the 1Password CLI installed; asks only for --version"]
fn real_op_version_runs_through_the_production_path() {
    let output = SystemOp
        .run(&OpCall {
            args: vec!["--version".to_owned()],
            token: None,
            stdin: None,
        })
        .unwrap();
    assert_eq!(output.exit_code, 0, "{}", output.stderr);
    let version = plaintext(&output.stdout);
    assert!(version.trim_end().starts_with("2."), "{version}");
}
