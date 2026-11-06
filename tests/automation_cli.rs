//! `rustty ctl` and `rustty mcp` as real processes, with no window to find. Each
//! run has its own empty `%APPDATA%`, so it can never reach a window someone has
//! open, and each must exit by itself: neither verb may ever open a window.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rustty-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn rustty(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustty"));
    command
        .args(args)
        .env("APPDATA", dir)
        .env("LOCALAPPDATA", dir)
        .env_remove("RUSTTY_WINDOW")
        .env_remove("RUSTTY_AUTOMATION_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(args: &[&str]) -> Output {
    let dir = scratch();
    let output = rustty(&dir, args).output().expect("ran");
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn ctl_with_no_window_says_so_and_exits_with_its_code() {
    let output = run(&["ctl", "list"]);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.starts_with("rustty: no-instance: "), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(output.stdout.is_empty());
}

#[test]
fn ctl_json_reports_the_error_on_stdout_too() {
    let output = run(&["ctl", "--json", "list"]);
    assert_eq!(output.status.code(), Some(5));
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "no-instance");
}

#[test]
fn ctl_usage_errors_exit_two_without_looking_for_a_window() {
    for args in [
        vec!["ctl"],
        vec!["ctl", "nope"],
        vec!["ctl", "close"],
        vec!["ctl", "list", "--window", "x"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
}

#[test]
fn ctl_help_succeeds_and_documents_the_exit_codes() {
    let output = run(&["ctl", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("target-stale") && text.contains("watch"),
        "{text}"
    );
}

#[test]
fn a_session_whose_window_is_gone_gets_an_error_not_another_window() {
    let dir = scratch();
    let output = rustty(&dir, &["ctl", "list"])
        .env("RUSTTY_WINDOW", "12345")
        .output()
        .expect("ran");
    // No windows at all here, so it is no-instance; with others it is stale.
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Which encoder shows the token, against a real pipe server run by this test: it
/// needs no window and no desktop. Nothing else can reach the server, because
/// discovery is the scratch `%APPDATA%` and the pipe takes this user only.
#[cfg(windows)]
mod token {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    use rustty::automation::pipe::{Server, process_start_time};
    use rustty::automation::{
        DiscoveryEntry, ID_PID_SHIFT, RpcError, discovery_dir, error_frame, load_discovery,
        next_id, write_discovery,
    };
    use serde_json::{Value, json};

    use super::{rustty, scratch};

    const SECRET: &str = "the-window-token-for-this-test";

    /// The first request `child` sends, answered with an error so it ends quickly.
    fn first_request(server: &Server, child: &mut Child) -> Value {
        let start = Instant::now();
        loop {
            if let Some((client, request)) = server.next_request(&|_| true) {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                server.send(client, &error_frame(id, &RpcError::bad_params("test over")));
                return request;
            }
            let done = child.try_wait().expect("polled").is_some();
            assert!(!done, "it ended without connecting");
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "it never connected"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Runs `rustty mcp` holding the token, asks it to reach window `id`, and says
    /// whether anything it sent arrived at `attacker`. Answers the attacker's
    /// connection so a client that fell for it would go on to send its token.
    fn mcp_reaches_the_attacker(dir: &Path, id: u64, attacker: &Server) -> bool {
        let mut child = rustty(dir, &["mcp"])
            .env("RUSTTY_WINDOW", id.to_string())
            .env("RUSTTY_AUTOMATION_TOKEN", SECRET)
            .stdin(Stdio::piped())
            .spawn()
            .expect("started");
        let mut stdin = child.stdin.take().expect("stdin");
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list", "arguments": {}}}),
        ] {
            writeln!(stdin, "{message}").expect("written");
        }
        drop(stdin);
        let mut reached = false;
        let start = Instant::now();
        while child.try_wait().expect("polled").is_none() {
            if let Some((client, request)) = attacker.next_request(&|_| true) {
                reached = true;
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                attacker.send(client, &error_frame(id, &RpcError::bad_params("got you")));
            }
            assert!(start.elapsed() < Duration::from_secs(30), "mcp never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        reached || attacker.next_request(&|_| true).is_some()
    }

    /// A pipe this test controls, as an attacker's would be: its name, and its
    /// server. The server's own discovery file goes in a folder clients don't read.
    fn attacker_pipe(dir: &Path) -> (Server, String) {
        let private = dir.join("attacker");
        let server = Server::start(next_id(), false, Some(private.clone())).expect("server");
        let own = load_discovery(&private, &|_| true);
        let pipe = own.first().expect("published").pipe.clone();
        (server, pipe)
    }

    /// A process to play the real window: its pid is what the id says.
    fn victim() -> Child {
        std::process::Command::new("cmd.exe")
            .args(["/c", "pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("started")
    }

    fn victim_id(pid: u32) -> u64 {
        (u64::from(pid) << ID_PID_SHIFT) + 1
    }

    #[test]
    fn a_fake_file_with_a_windows_id_and_the_attackers_pid_gets_nothing() {
        // The attack: write a discovery file naming the real window's id, with
        // this process's own pid, start time and pipe. A client whose RUSTTY_WINDOW
        // is that id would connect here and hand over its token.
        let dir = scratch();
        let mut window = victim();
        let (attacker, pipe) = attacker_pipe(&dir);
        let own_pid = std::process::id();
        let fake = DiscoveryEntry {
            id: victim_id(window.id()),
            pipe,
            pid: own_pid,
            start_time: process_start_time(own_pid).expect("own start time"),
            quick: false,
            version: "0".to_string(),
        };
        write_discovery(&discovery_dir(&dir), &fake).expect("planted");
        let reached = mcp_reaches_the_attacker(&dir, fake.id, &attacker);
        let _ = window.kill();
        let _ = window.wait();
        drop(attacker);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!reached, "the attacker's pipe was told something");
    }

    #[test]
    fn a_fake_file_with_the_windows_own_pid_and_the_attackers_pipe_gets_nothing() {
        // The same attack with every field true but the pipe: the identity check
        // on the connection is what stops this one.
        let dir = scratch();
        let mut window = victim();
        let (attacker, pipe) = attacker_pipe(&dir);
        let fake = DiscoveryEntry {
            id: victim_id(window.id()),
            pipe,
            pid: window.id(),
            start_time: process_start_time(window.id()).expect("its start time"),
            quick: false,
            version: "0".to_string(),
        };
        write_discovery(&discovery_dir(&dir), &fake).expect("planted");
        let reached = mcp_reaches_the_attacker(&dir, fake.id, &attacker);
        let _ = window.kill();
        let _ = window.wait();
        drop(attacker);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!reached, "the attacker's pipe was told something");
    }

    fn serve(dir: &Path) -> (Server, u64) {
        let id = next_id();
        let server = Server::start(id, false, Some(discovery_dir(dir))).expect("server");
        (server, id)
    }

    #[test]
    fn ctl_ignores_the_token_in_its_environment() {
        // Break caught: any command an agent runs inside a boss session getting
        // writes just by inheriting the variable.
        let dir = scratch();
        let (server, id) = serve(&dir);
        let mut child = rustty(&dir, &["ctl", "list"])
            .env("RUSTTY_WINDOW", id.to_string())
            .env("RUSTTY_AUTOMATION_TOKEN", SECRET)
            .spawn()
            .expect("started");
        let request = first_request(&server, &mut child);
        assert_eq!(request["method"], "initialize");
        assert!(request["params"].get("token").is_none(), "{request}");
        let _ = child.wait_with_output();
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_shows_the_token_to_its_own_window() {
        let dir = scratch();
        let (server, id) = serve(&dir);
        let mut child = rustty(&dir, &["mcp"])
            .env("RUSTTY_WINDOW", id.to_string())
            .env("RUSTTY_AUTOMATION_TOKEN", SECRET)
            .stdin(Stdio::piped())
            .spawn()
            .expect("started");
        let mut stdin = child.stdin.take().expect("stdin");
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list", "arguments": {}}}),
        ] {
            writeln!(stdin, "{message}").expect("written");
        }
        let request = first_request(&server, &mut child);
        assert_eq!(request["method"], "initialize");
        assert_eq!(request["params"]["token"], SECRET);
        drop(stdin);
        let _ = child.wait_with_output();
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Feed `rustty mcp` these messages, then end its input.
fn mcp(messages: &[Value]) -> Vec<Value> {
    let dir = scratch();
    let mut child = rustty(&dir, &["mcp"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("started");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for message in messages {
            writeln!(stdin, "{message}").expect("written");
        }
    }
    let output = child.wait_with_output().expect("exited");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout holds only MCP messages"))
        .collect()
}

#[test]
fn mcp_serves_a_handshake_client_with_no_window_open() {
    let out = mcp(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "list", "arguments": {}}}),
    ]);
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[0]["result"]["protocolVersion"], "2025-11-25");
    let tools = out[1]["result"]["tools"].as_array().expect("tools");
    assert!(tools.iter().any(|t| t["name"] == "pane_send-text"));
    assert_eq!(out[2]["result"]["isError"], true);
    assert_eq!(
        out[2]["result"]["structuredContent"]["error"]["code"],
        "no-instance"
    );
}

#[test]
fn mcp_serves_a_current_client_with_no_window_open() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let out = mcp(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": meta}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
    ]);
    assert_eq!(out[0]["result"]["supportedVersions"][0], "2026-07-28");
    assert_eq!(out[1]["result"]["resultType"], "complete");
}
