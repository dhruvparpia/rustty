//! End to end against a real window. Ignored by default: it needs a desktop, and
//! it opens a window (not activated, so it can't take anyone's keystrokes). Run by
//! hand, one at a time:
//!
//! ```text
//! cargo test --test automation_live -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Safety rules this keeps: `%APPDATA%` and `%LOCALAPPDATA%` point at a scratch
//! folder, so it uses none of the person's settings, and discovery can't find any
//! window they have open. It only ever stops the one process it started.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustty::automation::client::Transport;
use rustty::automation::pipe::{PipeTransport, alive, open};
use rustty::automation::{DiscoveryEntry, ErrorCode, discovery_dir, load_discovery};
use serde_json::{Value, json};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SW_SHOWNOACTIVATE,
};
use windows::core::{PCWSTR, PWSTR};

const WAIT: Duration = Duration::from_secs(30);

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A rustty window this test started. Dropping it stops that process only.
struct TestWindow {
    process: HANDLE,
    pid: u32,
    scratch: PathBuf,
}

impl TestWindow {
    fn start(tag: &str, config: &str) -> Self {
        let scratch =
            std::env::temp_dir().join(format!("rustty-live-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("rustty")).expect("scratch");
        write_config(&scratch, config);

        let mut block: Vec<u16> = Vec::new();
        for (key, value) in std::env::vars() {
            let private = [
                "APPDATA",
                "LOCALAPPDATA",
                "RUSTTY_WINDOW",
                "RUSTTY_AUTOMATION_TOKEN",
            ]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&key));
            if !private {
                block.extend(format!("{key}={value}\0").encode_utf16());
            }
        }
        let dir = scratch.display();
        block.extend(format!("APPDATA={dir}\0LOCALAPPDATA={dir}\0\0").encode_utf16());

        let exe = env!("CARGO_BIN_EXE_rustty");
        let mut line: Vec<u16> = format!("\"{exe}\"")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let startup = STARTUPINFOW {
            cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).expect("size"),
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: u16::try_from(SW_SHOWNOACTIVATE.0).expect("show"),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        // Safety: every pointer outlives the call; the block is NUL-terminated
        // UTF-16, as CREATE_UNICODE_ENVIRONMENT says it is.
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_UNICODE_ENVIRONMENT,
                Some(block.as_ptr().cast()),
                PCWSTR::null(),
                &startup,
                &mut info,
            )
            .expect("started the window");
            CloseHandle(info.hThread).expect("closed the thread handle");
        }
        Self {
            process: info.hProcess,
            pid: info.dwProcessId,
            scratch,
        }
    }

    fn entry(&self) -> DiscoveryEntry {
        let dir = discovery_dir(&self.scratch);
        wait_for("the window's discovery file", || {
            load_discovery(&dir, &alive)
                .into_iter()
                .find(|entry| entry.pid == self.pid)
        })
    }

    fn connect(&self) -> PipeTransport {
        let mut transport = open(&self.entry()).expect("connected");
        transport
            .call(
                "initialize",
                json!({"protocolVersion": 1, "clientName": "live test"}),
            )
            .expect("initialized");
        transport
    }

    /// `rustty ctl` run the way a script would.
    fn ctl(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rustty"))
            .arg("ctl")
            .args(args)
            .env("APPDATA", &self.scratch)
            .env("LOCALAPPDATA", &self.scratch)
            .env_remove("RUSTTY_WINDOW")
            .env_remove("RUSTTY_AUTOMATION_TOKEN")
            .stdin(Stdio::null())
            .output()
            .expect("ran ctl")
    }

    /// Whether this test's window has the foreground. It must never: anything the
    /// person types would go to it. The person changing windows is fine, which is
    /// why this looks at who owns the foreground rather than whether it changed.
    fn took_the_foreground(&self) -> bool {
        let mut pid = 0u32;
        // Safety: reading the foreground window; `pid` is a real out-param.
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
        pid == self.pid
    }

    fn exited(&self) -> bool {
        // Safety: the handle is ours and open.
        unsafe { WaitForSingleObject(self.process, 0) }.0 == 0
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // Safety: this handle is the process this test started, and nothing else.
        unsafe {
            if !self.exited() {
                let _ = TerminateProcess(self.process, 1);
            }
            let _ = CloseHandle(self.process);
        }
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

fn write_config(scratch: &Path, text: &str) {
    std::fs::write(scratch.join("rustty").join("config.toml"), text).expect("config");
}

const BASE_CONFIG: &str = "welcome = false\nsession-restore = false\n";

fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("ctl --json prints JSON")
}

fn read_screen(window: &TestWindow, pane: u64) -> String {
    let output = window.ctl(&["read", &pane.to_string()]);
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn wait_screen(window: &TestWindow, pane: u64, text: &str) -> String {
    wait_for(&format!("{text:?} on the screen"), || {
        let screen = read_screen(window, pane);
        screen.contains(text).then_some(screen)
    })
}

/// Grant writes through the config key, and wait for the window to pick it up.
fn allow_writes(window: &TestWindow) {
    write_config(
        &window.scratch,
        &format!("{BASE_CONFIG}automation-writes = true\n"),
    );
    wait_for("the config reload", || {
        let init = open(&window.entry()).ok()?.call(
            "initialize",
            json!({"protocolVersion": 1, "clientName": "probe"}),
        );
        (init.ok()?["grants"]["writes"] == true).then_some(())
    });
}

fn open_cmd(window: &TestWindow) -> (u64, u64) {
    let dir = window.scratch.to_string_lossy().into_owned();
    let opened = json_of(&window.ctl(&[
        "--json",
        "open",
        "--dir",
        &dir,
        "--name",
        "worker",
        "--env",
        "MARK=live-mark",
        "--",
        "cmd.exe",
        "/k",
        "prompt $G",
    ]));
    (
        opened["tab"].as_u64().expect("tab"),
        opened["pane"].as_u64().expect("pane"),
    )
}

/// The events and reads a boss depends on, with `config` deciding the sidebar.
fn boss_flow(tag: &str, config: &str) {
    let window = TestWindow::start(tag, config);
    let entry = window.entry();
    assert_eq!(entry.pid, window.pid);
    let raw = std::fs::read_to_string(
        discovery_dir(&window.scratch).join(format!("{}-{}.json", entry.pid, entry.id)),
    )
    .expect("discovery file");
    assert!(!raw.to_ascii_lowercase().contains("token"), "{raw}");

    // Only the window this test started can be found.
    let listed = json_of(&window.ctl(&["--json", "list"]));
    assert_eq!(listed["window"], entry.id);
    let first_tab = listed["tabs"][0]["id"].as_u64().expect("tab");

    // Reads need no grant. Writes do, and are refused with their exit code.
    let refused = window.ctl(&["open"]);
    assert_eq!(refused.status.code(), Some(4), "{refused:?}");

    allow_writes(&window);
    let (tab, pane) = open_cmd(&window);

    // It opened in the background: the selection stayed put.
    let listed = json_of(&window.ctl(&["--json", "list"]));
    let tabs = listed["tabs"].as_array().expect("tabs");
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0]["id"], first_tab);
    assert_eq!(tabs[0]["active"], true);
    assert_eq!(tabs[1]["id"], tab);
    assert_eq!(tabs[1]["active"], false);
    assert_eq!(tabs[1]["title"], "worker");

    // The session gets the environment it was given, and its own window's id.
    wait_screen(&window, pane, ">");
    let sent = window.ctl(&[
        "send",
        &pane.to_string(),
        "--enter",
        "--",
        "echo [%MARK%] [%RUSTTY_WINDOW%] [%RUSTTY_AUTOMATION_TOKEN%]",
    ]);
    assert!(sent.status.success(), "{sent:?}");
    let screen = wait_screen(&window, pane, "[live-mark]");
    assert!(screen.contains(&format!("[{}]", entry.id)), "{screen}");
    assert!(
        screen.contains("[%RUSTTY_AUTOMATION_TOKEN%]"),
        "a normal session must not get the token: {screen}"
    );

    // Events: output in order, and the status the sidebar logic computes.
    let mut events = window.connect();
    events
        .call("events.subscribe", json!({"pane": pane}))
        .expect("subscribe");
    let sent = window.ctl(&[
        "send",
        &pane.to_string(),
        "--enter",
        "--",
        "echo first-line& ping -n 5 127.0.0.1 >nul& echo last-line",
    ]);
    assert!(sent.status.success(), "{sent:?}");
    let mut output = String::new();
    let mut statuses = Vec::new();
    let start = Instant::now();
    let has_line = |output: &str, wanted: &str| output.lines().any(|line| line == wanted);
    while !(has_line(&output, "last-line")
        && statuses.iter().any(|s| s == "finished" || s == "idle"))
    {
        assert!(
            start.elapsed() < WAIT,
            "output {output:?}, statuses {statuses:?}"
        );
        match events.next_event(Duration::from_millis(100)).expect("open") {
            Some(event) if event["event"] == "pane-output" => {
                output.push_str(event["text"].as_str().expect("text"));
            }
            Some(event) if event["event"] == "status-changed" => {
                assert_eq!(event["pane"], pane);
                statuses.push(event["status"].as_str().expect("status").to_string());
            }
            _ => {}
        }
    }
    let lines: Vec<&str> = output.lines().collect();
    let first = lines
        .iter()
        .position(|l| *l == "first-line")
        .expect("first");
    let last = lines.iter().position(|l| *l == "last-line").expect("last");
    assert!(first < last, "{output}");
    assert!(statuses.iter().any(|s| s == "working"), "{statuses:?}");

    // Scrollback paging reads the same text back.
    let history = window.ctl(&["read", &pane.to_string(), "--scrollback", "--all"]);
    let history = String::from_utf8_lossy(&history.stdout);
    assert!(history.contains("first-line"), "{history}");

    // Split, focus: the new pane answers by its own id, ids don't move.
    let split = json_of(&window.ctl(&[
        "--json",
        "split",
        &pane.to_string(),
        "--direction",
        "down",
        "--",
        "cmd.exe",
        "/k",
        "prompt $G",
    ]));
    let fresh = split["pane"].as_u64().expect("pane");
    assert_ne!(fresh, pane);
    assert_eq!(split["tab"], tab);
    wait_screen(&window, fresh, ">");
    assert!(
        read_screen(&window, pane).contains("first-line"),
        "still the same pane"
    );
    assert!(
        window
            .ctl(&["focus", "--pane", &fresh.to_string()])
            .status
            .success()
    );
    let listed = json_of(&window.ctl(&["--json", "list"]));
    assert_eq!(listed["tabs"][1]["active"], true);
    assert_eq!(listed["tabs"][1]["panes"][1]["active"], true);
    assert!(
        window
            .ctl(&["focus", &first_tab.to_string()])
            .status
            .success()
    );

    // A program that exits closes its pane, with its exit code, as a final event.
    let mut watcher = window.connect();
    watcher
        .call("events.subscribe", json!({"pane": fresh}))
        .expect("subscribe");
    let sent = window.ctl(&["send", &fresh.to_string(), "--enter", "--", "exit 7"]);
    assert!(sent.status.success(), "{sent:?}");
    let start = Instant::now();
    let mut seen = Vec::new();
    let closed = loop {
        match watcher
            .next_event(Duration::from_millis(100))
            .expect("open")
        {
            Some(event) if event["event"] == "pane-closed" => break event,
            Some(event) => seen.push(event),
            None => {}
        }
        assert!(
            start.elapsed() < WAIT,
            "no pane-closed.\nevents: {seen:?}\nlist: {}\nscreen: {}",
            String::from_utf8_lossy(&window.ctl(&["--json", "list"]).stdout),
            read_screen(&window, fresh),
        );
    };
    assert_eq!(closed["pane"], fresh);
    assert_eq!(closed["exitCode"], 7);

    // Its id is stale forever, with its exit code, and nothing else got the text.
    let stale = window.ctl(&["send", &fresh.to_string(), "hello"]);
    assert_eq!(stale.status.code(), Some(3), "{stale:?}");

    // A short-lived command that fails to start is an error, and leaves no tab.
    let bad = window.ctl(&["open", "--", "C:\\no\\such\\program.exe"]);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    let listed = json_of(&window.ctl(&["--json", "list"]));
    assert_eq!(listed["tabs"].as_array().map(Vec::len), Some(2));

    assert!(
        !window.took_the_foreground(),
        "the test window must never take the foreground"
    );

    assert!(window.ctl(&["close", &tab.to_string()]).status.success());
    let stale = window.ctl(&["read", &pane.to_string()]);
    assert_eq!(stale.status.code(), Some(3), "{stale:?}");

    // Closing the last tab closes the window, and the reply still gets out.
    let closed = window.ctl(&["--json", "close", &first_tab.to_string()]);
    let closed = json_of(&closed);
    assert_eq!(closed["closed"], true);
    wait_for("the window's process to end", || {
        window.exited().then_some(())
    });
    let gone = window.ctl(&["list"]);
    assert_eq!(gone.status.code(), Some(5), "{gone:?}");
    assert!(
        load_discovery(&discovery_dir(&window.scratch), &|_| true).is_empty(),
        "its discovery file went with it"
    );
}

#[test]
#[ignore = "opens a window; run by hand"]
fn a_boss_can_run_a_session_with_the_sidebar_on() {
    boss_flow("rail-on", &format!("{BASE_CONFIG}session-rail = true\n"));
}

#[test]
#[ignore = "opens a window; run by hand"]
fn a_boss_can_run_a_session_with_the_sidebar_off() {
    // Break caught: statuses only ever being stepped by the sidebar's tick, so
    // turning it off would leave every session reporting idle forever.
    boss_flow("rail-off", &format!("{BASE_CONFIG}session-rail = false\n"));
}

#[test]
#[ignore = "opens a window; run by hand"]
fn stale_and_wrong_tokens_get_no_writes_on_a_real_window() {
    let window = TestWindow::start("token", BASE_CONFIG);
    let entry = window.entry();
    let mut transport = open(&entry).expect("connected");
    let init = transport
        .call(
            "initialize",
            json!({"protocolVersion": 1, "clientName": "live", "token": "not-the-token"}),
        )
        .expect("initialized");
    assert_eq!(init["grants"]["writes"], false);
    assert_eq!(init["window"], entry.id);
    let error = transport.call("tab.open", json!({})).expect_err("refused");
    assert_eq!(error.code, ErrorCode::DeniedGrant);
    assert!(transport.call("list", json!({})).is_ok());
}

#[test]
#[ignore = "opens a window; run by hand"]
fn a_command_that_ends_reports_its_output_and_exit_code() {
    // What a boss most needs from `tab.open` with a command: subscribe first, so
    // nothing printed before a per-pane subscribe could arrive is missed.
    let window = TestWindow::start(
        "oneshot",
        &format!("{BASE_CONFIG}automation-writes = true\n"),
    );
    let mut events = window.connect();
    events
        .call(
            "events.subscribe",
            json!({"events": ["pane-output", "pane-closed", "tab-closed"]}),
        )
        .expect("subscribe");
    let opened = events
        .call(
            "tab.open",
            json!({"command": ["cmd.exe", "/c", "echo from-the-command& exit 3"]}),
        )
        .expect("opened");
    let pane = &opened["pane"];
    let start = Instant::now();
    let mut output = String::new();
    let closed = loop {
        match events.next_event(Duration::from_millis(100)).expect("open") {
            Some(event) if event["event"] == "pane-closed" && &event["pane"] == pane => {
                break event;
            }
            Some(event) if event["event"] == "pane-output" && &event["pane"] == pane => {
                output.push_str(event["text"].as_str().expect("text"));
            }
            _ => {}
        }
        assert!(
            start.elapsed() < WAIT,
            "no pane-closed.\noutput: {output:?}\nlist: {}",
            String::from_utf8_lossy(&window.ctl(&["--json", "list"]).stdout),
        );
    };
    assert_eq!(closed["exitCode"], 3, "{closed}");
    assert!(output.contains("from-the-command"), "{output:?}");
}

/// One JSON-RPC exchange with a running `rustty mcp`.
fn ask(
    stdin: &mut impl Write,
    stdout: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    writeln!(stdin, "{request}").expect("written");
    stdin.flush().expect("flushed");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    let reply: Value = serde_json::from_str(&line).expect("one JSON message per line");
    assert_eq!(reply["id"], id, "{reply}");
    reply
}

#[test]
#[ignore = "opens a window; run by hand"]
fn mcp_drives_a_real_window_end_to_end() {
    let window = TestWindow::start("mcp", &format!("{BASE_CONFIG}automation-writes = true\n"));
    let entry = window.entry();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustty"))
        .arg("mcp")
        .env("APPDATA", &window.scratch)
        .env("LOCALAPPDATA", &window.scratch)
        .env("RUSTTY_WINDOW", entry.id.to_string())
        .env_remove("RUSTTY_AUTOMATION_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("started");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = ask(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "live", "version": "1"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");

    let tools = ask(&mut stdin, &mut stdout, 2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in ["list", "pane_read", "tab_open", "pane_send-text"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }

    let call = |stdin: &mut std::process::ChildStdin,
                stdout: &mut BufReader<std::process::ChildStdout>,
                id,
                name: &str,
                arguments: Value| {
        let reply = ask(
            stdin,
            stdout,
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        );
        assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
        reply["result"]["structuredContent"].clone()
    };

    let listed = call(&mut stdin, &mut stdout, 3, "list", json!({}));
    assert_eq!(listed["window"], entry.id);
    let opened = call(
        &mut stdin,
        &mut stdout,
        4,
        "tab_open",
        json!({"name": "via-mcp", "command": ["cmd.exe", "/k", "prompt $G"]}),
    );
    let pane = opened["pane"].as_u64().expect("pane");
    call(
        &mut stdin,
        &mut stdout,
        5,
        "pane_send-text",
        json!({"pane": pane, "text": "echo typed-through-mcp\r"}),
    );
    let mut id = 6;
    let screen = wait_for("the echoed text", || {
        id += 1;
        let read = call(
            &mut stdin,
            &mut stdout,
            id,
            "pane_read",
            json!({"pane": pane}),
        );
        let text = read["text"].as_str().unwrap_or_default().to_string();
        text.lines()
            .any(|line| line.trim_end() == "typed-through-mcp")
            .then_some(text)
    });
    assert!(screen.contains("typed-through-mcp"), "{screen}");

    drop(stdin);
    let status = child.wait().expect("exited");
    assert_eq!(status.code(), Some(0), "ending its input ends it cleanly");
}
