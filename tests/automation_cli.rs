//! `rustty ctl` and `rustty mcp` as real processes, with no window to find. Each
//! run has its own empty `%APPDATA%`, so it can never reach a window someone has
//! open, and each must exit by itself: neither verb may ever open a window.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::Write;
use std::path::PathBuf;
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

fn rustty(dir: &PathBuf, args: &[&str]) -> Command {
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
