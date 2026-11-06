//! The CLI against a fake window. Exit codes are the contract under test.

use std::io::Cursor;

use serde_json::{Value, json};

use super::*;
use crate::automation::client::fake::{FakeConnector, Shared, world};
use crate::automation::{ErrorCode, RpcError};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

fn parsed(args: &[&str]) -> Invocation {
    parse(&strings(args)).expect("valid arguments")
}

struct Ran {
    code: u8,
    out: String,
    err: String,
}

fn run_with(world: &Shared, args: &[&str], stdin: &str) -> Ran {
    let mut connector = FakeConnector::new(world);
    run_connected(args, stdin, &mut connector)
}

fn run_connected(args: &[&str], stdin: &str, connector: &mut FakeConnector) -> Ran {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = Cursor::new(stdin.as_bytes().to_vec());
    let code = run(&strings(args), connector, &mut input, &mut out, &mut err);
    Ran {
        code,
        out: String::from_utf8(out).expect("utf8"),
        err: String::from_utf8(err).expect("utf8"),
    }
}

fn writable() -> Shared {
    let shared = world();
    shared.borrow_mut().window.allow_writes = true;
    shared
}

fn first_ids(shared: &Shared) -> (u64, u64) {
    let world = shared.borrow();
    let tab = &world.window.app.tabs()[0];
    (tab.id(), tab.pane().id())
}

fn run_refused(args: &[&str], error: RpcError) -> Ran {
    let shared = world();
    let mut connector = FakeConnector::new(&shared);
    connector.refuse = Some(error);
    run_connected(args, "", &mut connector)
}

#[test]
fn window_and_json_are_accepted_anywhere_before_the_command() {
    for args in [
        ["--window", "9", "--json", "list"],
        ["list", "--window", "9", "--json"],
        ["--json", "list", "--window", "9"],
    ] {
        let invocation = parsed(&args);
        assert_eq!(invocation.window, Some(9));
        assert!(invocation.json);
        assert_eq!(invocation.verb, Verb::List);
    }
}

#[test]
fn open_takes_every_field_and_the_argv_after_the_separator() {
    let invocation = parsed(&[
        "open",
        "--dir",
        r"C:\my work\café",
        "--profile",
        "pwsh",
        "--name",
        "the builder",
        "--env",
        "A=1",
        "--env",
        "B=x=y",
        "--",
        "cargo",
        "test",
        "--window",
        "--json",
    ]);
    let Verb::Open(request) = invocation.verb else {
        panic!("not open");
    };
    assert_eq!(request.dir.as_deref(), Some(r"C:\my work\café"));
    assert_eq!(request.profile.as_deref(), Some("pwsh"));
    assert_eq!(request.name.as_deref(), Some("the builder"));
    assert_eq!(request.env.get("B").map(String::as_str), Some("x=y"));
    // Past `--` it all belongs to the command, flags included.
    assert_eq!(
        request.command,
        Some(strings(&["cargo", "test", "--window", "--json"]))
    );
    assert_eq!(invocation.window, None);
    assert!(!invocation.json);
}

#[test]
fn every_verb_parses() {
    assert_eq!(parsed(&["capabilities"]).verb, Verb::Capabilities);
    assert_eq!(parsed(&["close", "5"]).verb, Verb::Close { tab: 5 });
    assert_eq!(parsed(&["focus", "5"]).verb, Verb::FocusTab { tab: 5 });
    assert_eq!(
        parsed(&["focus", "--pane", "6"]).verb,
        Verb::FocusPane { pane: 6 }
    );
    assert_eq!(
        parsed(&["read", "6"]).verb,
        Verb::Read {
            pane: 6,
            scrollback: false,
            cursor: None,
            all: false,
        }
    );
    assert_eq!(
        parsed(&["read", "6", "--scrollback", "--cursor", "6:10", "--all"]).verb,
        Verb::Read {
            pane: 6,
            scrollback: true,
            cursor: Some("6:10".to_string()),
            all: true,
        }
    );
    let Verb::Split {
        pane,
        direction,
        request,
    } = parsed(&["split", "6", "--direction", "down", "--", "vim"]).verb
    else {
        panic!("not split");
    };
    assert_eq!((pane, direction.as_deref()), (6, Some("down")));
    assert_eq!(request.command, Some(strings(&["vim"])));
    assert_eq!(
        parsed(&["send", "6", "hello", "world", "--enter"]).verb,
        Verb::Send {
            pane: 6,
            text: Some("hello world".to_string()),
            enter: true,
        }
    );
    assert_eq!(
        parsed(&["send", "6", "--stdin"]).verb,
        Verb::Send {
            pane: 6,
            text: None,
            enter: false,
        }
    );
    assert_eq!(
        parsed(&["send", "6", "--", "--not-a-flag"]).verb,
        Verb::Send {
            pane: 6,
            text: Some("--not-a-flag".to_string()),
            enter: false,
        }
    );
    assert_eq!(
        parsed(&["watch", "--pane", "6", "--events", "bell,status-changed"]).verb,
        Verb::Watch {
            pane: Some(6),
            events: Some(strings(&["bell", "status-changed"])),
        }
    );
}

#[test]
fn bad_arguments_are_usage_errors() {
    for args in [
        vec![],
        vec!["nope"],
        vec!["close"],
        vec!["close", "x"],
        vec!["close", "5", "extra"],
        vec!["read"],
        vec!["read", "6", "--bogus"],
        vec!["send", "6"],
        vec!["send", "6", "--stdin", "text"],
        vec!["open", "--dir"],
        vec!["open", "--env", "NOEQUALS"],
        vec!["open", "extra"],
        vec!["open", "--"],
        vec!["list", "--window"],
        vec!["list", "--window", "x"],
        vec!["focus"],
        vec!["focus", "--pane"],
        vec!["split", "6", "--direction"],
    ] {
        let shared = world();
        let ran = run_with(&shared, &args, "");
        assert_eq!(ran.code, 2, "{args:?}");
        assert!(ran.err.starts_with("rustty: "), "{args:?}: {}", ran.err);
        assert_eq!(ran.err.lines().count(), 1, "{args:?}: {}", ran.err);
    }
}

#[test]
fn help_prints_the_grammar_and_succeeds_without_a_window() {
    for args in [vec!["--help"], vec!["-h"], vec!["help"]] {
        let shared = world();
        let mut connector = FakeConnector::new(&shared);
        connector.refuse = Some(RpcError::new(ErrorCode::NoInstance, "none"));
        let ran = run_connected(&args, "", &mut connector);
        assert_eq!(ran.code, 0, "{args:?}");
        assert!(ran.out.contains("rustty ctl"), "{}", ran.out);
        assert_eq!(connector.connects, 0);
    }
}

#[test]
fn every_failure_has_its_exit_code_and_a_one_line_message() {
    for code in crate::automation::ERROR_CODES {
        let ran = run_refused(&["list"], RpcError::new(*code, "it went wrong"));
        assert_eq!(ran.code, code.exit_code(), "{code:?}");
        assert_eq!(
            ran.err,
            format!("rustty: {}: it went wrong\n", code.as_str())
        );
        assert_eq!(ran.out, "");
    }
}

#[test]
fn json_mode_puts_the_error_on_stdout_too() {
    let ran = run_refused(
        &["--json", "list"],
        RpcError::new(ErrorCode::NoInstance, "no window"),
    );
    assert_eq!(ran.code, 5);
    let value: Value = serde_json::from_str(ran.out.trim()).expect("json");
    assert_eq!(value["error"]["code"], "no-instance");
    assert!(ran.err.contains("no-instance"));
}

#[test]
fn a_stale_target_exits_with_its_code() {
    let shared = writable();
    let ran = run_with(&shared, &["close", "12345"], "");
    assert_eq!(ran.code, 3);
    assert!(ran.err.starts_with("rustty: target-stale"), "{}", ran.err);
}

#[test]
fn a_denied_write_exits_with_its_code() {
    let shared = world();
    let (tab, _) = first_ids(&shared);
    let ran = run_with(&shared, &["close", &tab.to_string()], "");
    assert_eq!(ran.code, 4);
    assert_eq!(shared.borrow().window.app.tab_count(), 1);
}

#[test]
fn list_prints_a_table_or_json() {
    let shared = world();
    let (tab, pane) = first_ids(&shared);
    let ran = run_with(&shared, &["list"], "");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.contains(&tab.to_string()), "{}", ran.out);
    assert!(ran.out.contains(&pane.to_string()), "{}", ran.out);
    assert!(ran.out.contains("idle"), "{}", ran.out);
    let ran = run_with(&shared, &["list", "--json"], "");
    let value: Value = serde_json::from_str(ran.out.trim()).expect("json");
    assert_eq!(value["tabs"][0]["id"], tab);
}

#[test]
fn open_reports_the_new_ids() {
    let shared = writable();
    let ran = run_with(
        &shared,
        &["--json", "open", "--name", "x", "--", "a b", "c"],
        "",
    );
    assert_eq!(ran.code, 0, "{}", ran.err);
    let value: Value = serde_json::from_str(ran.out.trim()).expect("json");
    assert!(value["tab"].is_u64() && value["pane"].is_u64());
    assert_eq!(
        shared.borrow().window.opened[0].command,
        Some(strings(&["a b", "c"]))
    );
    let ran = run_with(&shared, &["open"], "");
    assert!(ran.out.contains("tab "), "{}", ran.out);
    assert!(ran.out.contains("pane "), "{}", ran.out);
}

#[test]
fn close_focus_and_split_reach_the_window() {
    let shared = writable();
    let (tab, pane) = first_ids(&shared);
    assert_eq!(run_with(&shared, &["split", &pane.to_string()], "").code, 0);
    assert_eq!(shared.borrow().window.app.tabs()[0].pane_count(), 2);
    assert_eq!(run_with(&shared, &["focus", &tab.to_string()], "").code, 0);
    assert_eq!(
        run_with(&shared, &["focus", "--pane", &pane.to_string()], "").code,
        0
    );
    assert_eq!(run_with(&shared, &["close", &tab.to_string()], "").code, 0);
    assert_eq!(shared.borrow().window.app.tab_count(), 0);
}

#[test]
fn capabilities_come_from_the_window() {
    let shared = world();
    let ran = run_with(&shared, &["capabilities"], "");
    assert_eq!(ran.code, 0);
    assert!(ran.out.contains("pane.send-text"), "{}", ran.out);
    let ran = run_with(&shared, &["capabilities", "--json"], "");
    let value: Value = serde_json::from_str(ran.out.trim()).expect("json");
    assert!(value["operations"].is_array());
}

fn feed(shared: &Shared, bytes: &[u8]) {
    let mut world = shared.borrow_mut();
    if let Some(mut pane) = world
        .window
        .app
        .tab_at_mut(0)
        .and_then(|tab| tab.pane_at_mut(0))
    {
        pane.feed(bytes);
    }
}

#[test]
fn read_prints_the_screens_text_exactly() {
    let shared = world();
    let (_, pane) = first_ids(&shared);
    feed(&shared, "héllo\r\nwörld".as_bytes());
    let ran = run_with(&shared, &["read", &pane.to_string()], "");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(ran.out, "héllo\nwörld\n");
}

#[test]
fn read_all_follows_every_page_and_prints_each_line_once() {
    let shared = world();
    shared.borrow_mut().window =
        crate::automation::ops::testkit::FakeWindow::sized(180, 10, 20_000);
    let (_, pane) = first_ids(&shared);
    let row = "\"".repeat(170);
    for i in 0..9000 {
        feed(&shared, format!("{i:05}{row}\r\n").as_bytes());
    }
    let ran = run_with(
        &shared,
        &["read", &pane.to_string(), "--scrollback", "--all"],
        "",
    );
    assert_eq!(ran.code, 0, "{}", ran.err);
    let rows: Vec<&str> = ran.out.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(rows.len(), 9000);
    for (i, row) in rows.iter().enumerate() {
        assert!(row.starts_with(&format!("{i:05}")), "{i}");
    }
    let reads = shared
        .borrow()
        .calls
        .iter()
        .filter(|(method, _)| method == "pane.read")
        .count();
    assert!(reads >= 2, "{reads}");
}

#[test]
fn read_without_all_prints_where_to_continue() {
    let shared = world();
    let (_, pane) = first_ids(&shared);
    feed(&shared, b"one\r\n");
    let ran = run_with(
        &shared,
        &["--json", "read", &pane.to_string(), "--scrollback"],
        "",
    );
    let value: Value = serde_json::from_str(ran.out.trim()).expect("json");
    assert!(value["cursor"].is_string());
    assert!(value["text"].as_str().expect("text").contains("one"));
}

#[test]
fn send_types_the_text_and_enter() {
    let shared = writable();
    let (_, pane) = first_ids(&shared);
    let ran = run_with(
        &shared,
        &["send", &pane.to_string(), "--enter", "--", "ls", "-la"],
        "",
    );
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(
        shared.borrow().window.sent,
        vec![(pane, b"ls -la\r".to_vec())]
    );
}

#[test]
fn send_reads_stdin_when_asked() {
    let shared = writable();
    let (_, pane) = first_ids(&shared);
    let ran = run_with(
        &shared,
        &["send", &pane.to_string(), "--stdin"],
        "piped ünicode\n",
    );
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(
        shared.borrow().window.sent[0].1,
        "piped ünicode\n".as_bytes()
    );
}

#[test]
fn send_refuses_stdin_that_is_not_text() {
    let shared = writable();
    let (_, pane) = first_ids(&shared);
    let mut connector = FakeConnector::new(&shared);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = Cursor::new(vec![0xFF, 0xFE, 0x41]);
    let args = strings(&["send", &pane.to_string(), "--stdin"]);
    let code = run(&args, &mut connector, &mut input, &mut out, &mut err);
    assert_eq!(code, 2);
    assert!(shared.borrow().window.sent.is_empty());
}

#[test]
fn a_long_send_is_split_on_character_boundaries_within_the_limit() {
    let text = "é".repeat(10_000);
    let chunks = split_text(&text, 8 * 1024);
    assert!(chunks.len() >= 3);
    for chunk in &chunks {
        assert!(chunk.len() <= 8 * 1024);
    }
    assert_eq!(chunks.concat(), "é".repeat(10_000));
    assert_eq!(split_text("", 8).len(), 0);
    assert_eq!(split_text("abc", 8), vec!["abc"]);
    // A character wider than the limit still moves forward instead of hanging.
    assert_eq!(split_text("é", 1), vec!["é"]);
}

#[test]
fn a_rate_limited_send_waits_as_told_and_carries_on() {
    let shared = writable();
    let (_, pane) = first_ids(&shared);
    shared
        .borrow_mut()
        .scripted
        .push_back(Err(RpcError::rate_limited(1)));
    let ran = run_with(&shared, &["send", &pane.to_string(), "x"], "");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert_eq!(shared.borrow().window.sent.len(), 1, "sent on the retry");
}

#[test]
fn a_send_that_fails_part_way_says_how_much_went_in() {
    // Break caught: a script retrying from the start and typing it all twice.
    let shared = writable();
    let (_, pane) = first_ids(&shared);
    let text = "x".repeat(9 * 1024);
    {
        let mut world = shared.borrow_mut();
        world.scripted.push_back(Ok(json!({"bytes": 8192})));
        world
            .scripted
            .push_back(Err(RpcError::closed(pane, "its shell has exited")));
    }
    let ran = run_with(&shared, &["send", &pane.to_string(), &text], "");
    assert_eq!(ran.code, 8);
    assert!(ran.err.contains("8192"), "{}", ran.err);
    assert_eq!(ran.err.lines().count(), 1, "{}", ran.err);
}

/// Watch until `limit` empty waits: the fake never ends a stream by itself.
fn watch(shared: &Shared, args: &[&str], limit: usize) -> Ran {
    let mut connector = FakeConnector::new(shared);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let invocation = parse(&strings(args)).expect("args");
    let mut transport = crate::automation::client::Connector::connect(&mut connector).expect("up");
    let code = run_watch(
        &invocation,
        transport.as_mut(),
        &mut out,
        &mut err,
        Some(limit),
    );
    Ran {
        code,
        out: String::from_utf8(out).expect("utf8"),
        err: String::from_utf8(err).expect("utf8"),
    }
}

#[test]
fn watch_prints_events_one_per_line_in_json_mode() {
    let shared = world();
    // Subscribing takes the baseline, so the change comes after it: the fake
    // transport scans on demand.
    let mut connector = FakeConnector::new(&shared);
    let mut transport = crate::automation::client::Connector::connect(&mut connector).expect("up");
    transport
        .call("events.subscribe", json!({}))
        .expect("subscribe");
    feed(&shared, b"\x07");
    let event = transport
        .next_event(std::time::Duration::ZERO)
        .expect("open")
        .expect("event");
    assert_eq!(event["event"], "bell");
    let mut out = Vec::new();
    print_event(&event, true, &mut out).expect("written");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(text.lines().count(), 1);
    let value: Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(value["event"], "bell");
}

#[test]
fn watch_prints_output_as_text_for_people() {
    let event =
        json!({"event": "pane-output", "seq": 1, "pane": 6, "text": "line one\nline two\n"});
    let mut out = Vec::new();
    print_event(&event, false, &mut out).expect("written");
    assert_eq!(
        String::from_utf8(out).expect("utf8"),
        "line one\nline two\n"
    );
    let event = json!({"event": "status-changed", "seq": 2, "pane": 6, "status": "needs-you", "detail": "Approve?"});
    let mut out = Vec::new();
    print_event(&event, false, &mut out).expect("written");
    let text = String::from_utf8(out).expect("utf8");
    assert!(
        text.contains("status-changed") && text.contains("needs-you"),
        "{text}"
    );
    assert!(text.contains("Approve?"), "{text}");
}

#[test]
fn watching_a_pane_ends_cleanly_when_it_closes() {
    let shared = world();
    {
        let mut guard = shared.borrow_mut();
        guard.window.app.split_active(crate::split::Dir::Vertical);
    }
    let pane = shared.borrow().window.app.tabs()[0]
        .pane_at(1)
        .map_or(0, |p| p.id());
    let mut connector = FakeConnector::new(&shared);
    let mut transport = crate::automation::client::Connector::connect(&mut connector).expect("up");
    let args = strings(&["watch", "--pane", &pane.to_string()]);
    let invocation = parse(&args).expect("args");
    let closer = std::rc::Rc::clone(&shared);
    let mut out = Vec::new();
    let mut err = Vec::new();
    // The limit stops a watch that doesn't end by itself, which the count catches.
    let mut idle = 0;
    let code = run_watch_with(
        &invocation,
        transport.as_mut(),
        &mut out,
        &mut err,
        Some(50),
        &mut || {
            idle += 1;
            closer.borrow_mut().window.app.close_tab_pane(0, 1);
        },
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert!(String::from_utf8_lossy(&out).contains("pane-closed"));
    assert!(idle < 5, "it kept watching after its pane closed: {idle}");
}

#[test]
fn a_stale_watch_target_exits_with_its_code() {
    let shared = world();
    let ran = watch(&shared, &["watch", "--pane", "999999"], 1);
    assert_eq!(ran.code, 3, "{}", ran.err);
}

#[test]
fn window_id_targeting_reaches_the_connector() {
    assert_eq!(parsed(&["--window", "77", "list"]).window, Some(77));
}
