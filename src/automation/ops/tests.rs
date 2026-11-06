//! The dispatcher against a fake window: a real [`App`], no shells, no HWND.

use serde_json::{Value, json};

use super::testkit::{FakeWindow, call, connect, err, ok};
use super::*;
use crate::automation::{ErrorCode, MAX_PAGE_BYTES, OPERATIONS, RpcError, Token};

const CLIENT: ClientId = 1;

fn setup() -> (Dispatcher, FakeWindow) {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    connect(&mut d, &mut w, CLIENT);
    (d, w)
}

fn first_pane(w: &FakeWindow) -> u64 {
    w.app.tabs()[0].pane().id()
}

fn first_tab(w: &FakeWindow) -> u64 {
    w.app.tabs()[0].id()
}

#[test]
fn nothing_works_before_initialize() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    let frame = call(&mut d, &mut w, CLIENT, "list", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    assert!(err(&frame).message.contains("initialize"));
}

#[test]
fn initialize_reports_version_capabilities_window_and_grant() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "initialize",
        json!({"protocolVersion": 1, "clientName": "test"}),
    );
    let result = ok(&frame);
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["window"], w.window_id());
    assert_eq!(result["grants"]["writes"], false);
    assert!(result["capabilities"]["operations"].is_array());
}

#[test]
fn a_version_mismatch_is_a_hard_error_and_leaves_the_connection_uninitialized() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    for version in [json!(0), json!(99)] {
        let frame = call(
            &mut d,
            &mut w,
            CLIENT,
            "initialize",
            json!({"protocolVersion": version}),
        );
        assert_eq!(err(&frame).code, ErrorCode::ProtocolMismatch);
    }
    let frame = call(&mut d, &mut w, CLIENT, "list", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    // A version that isn't a number is a malformed request.
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "initialize",
        json!({"protocolVersion": "1"}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
}

#[test]
fn a_connection_initializes_once() {
    // Break caught: guessing tokens over one connection, or changing protocol
    // part way through.
    let (mut d, mut w) = setup();
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "initialize",
        json!({"protocolVersion": 1}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
}

#[test]
fn every_read_works_with_no_grant_and_every_write_is_refused() {
    let (mut d, mut w) = setup();
    let (pane, tab) = (first_pane(&w), first_tab(&w));
    for (method, params) in [
        ("capabilities", json!({})),
        ("list", json!({})),
        ("pane.read", json!({"pane": pane})),
        ("events.subscribe", json!({})),
        ("events.unsubscribe", json!({})),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, method, params);
        assert!(frame.get("result").is_some(), "{method}: {frame}");
    }
    for (method, params) in [
        ("tab.open", json!({})),
        ("tab.close", json!({"tab": tab})),
        ("tab.focus", json!({"tab": tab})),
        ("pane.split", json!({"pane": pane})),
        ("pane.focus", json!({"pane": pane})),
        ("pane.send-text", json!({"pane": pane, "text": "x"})),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, method, params);
        assert_eq!(err(&frame).code, ErrorCode::DeniedGrant, "{method}");
    }
    assert_eq!(w.app.tab_count(), 1, "nothing happened");
    assert!(w.sent.is_empty());
    assert!(w.opened.is_empty());
}

#[test]
fn the_grant_check_covers_every_write_in_the_table() {
    // Break caught: a new write op added to the table but not gated.
    let (mut d, mut w) = setup();
    for op in OPERATIONS.iter().filter(|op| op.write) {
        let frame = call(&mut d, &mut w, CLIENT, op.name, json!({}));
        assert_eq!(err(&frame).code, ErrorCode::DeniedGrant, "{}", op.name);
    }
}

#[test]
fn the_config_key_grants_every_connection_and_can_be_turned_off() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let frame = call(&mut d, &mut w, CLIENT, "tab.open", json!({}));
    assert!(frame.get("result").is_some(), "{frame}");
    // It takes effect on the next request, including for existing connections.
    w.allow_writes = false;
    let frame = call(&mut d, &mut w, CLIENT, "tab.open", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::DeniedGrant);
}

#[test]
fn the_token_grants_only_the_connection_that_showed_it() {
    let token = Token::from_random_bytes(&[7; 32]);
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    w.token = Some(token.clone());
    let frame = call(
        &mut d,
        &mut w,
        1,
        "initialize",
        json!({"protocolVersion": 1, "token": token.as_str()}),
    );
    assert_eq!(ok(&frame)["grants"]["writes"], true);
    connect(&mut d, &mut w, 2);

    let frame = call(&mut d, &mut w, 1, "tab.open", json!({}));
    assert!(frame.get("result").is_some(), "{frame}");
    let frame = call(&mut d, &mut w, 2, "tab.open", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::DeniedGrant);
}

#[test]
fn a_wrong_token_still_reads_but_cannot_write() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    w.token = Some(Token::from_random_bytes(&[7; 32]));
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "initialize",
        json!({"protocolVersion": 1, "token": "wrong"}),
    );
    let text = frame.to_string();
    assert_eq!(ok(&frame)["grants"]["writes"], false);
    assert!(!text.contains("wrong"), "the token is never echoed: {text}");
    assert!(
        call(&mut d, &mut w, CLIENT, "list", json!({}))
            .get("result")
            .is_some()
    );
    let frame = call(&mut d, &mut w, CLIENT, "tab.open", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::DeniedGrant);
}

#[test]
fn a_window_with_no_token_accepts_none() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "initialize",
        json!({"protocolVersion": 1, "token": ""}),
    );
    assert_eq!(ok(&frame)["grants"]["writes"], false);
}

#[test]
fn list_describes_tabs_and_panes_with_canonical_ids() {
    let (mut d, mut w) = setup();
    w.app.add_tab_sized(80, 24, 100);
    w.app.select_tab(0);
    if let Some(mut pane) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        pane.feed(b"\x1b]0;\xE2\xA0\x82 build\x07\x1b]7;file:///C:/work/api\x07");
    }
    let result = ok(&call(&mut d, &mut w, CLIENT, "list", json!({})));
    assert_eq!(result["window"], w.window_id());
    let tabs = result["tabs"].as_array().expect("tabs");
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0]["id"], first_tab(&w));
    assert_eq!(tabs[0]["number"], 1);
    assert_eq!(tabs[1]["number"], 2);
    assert_eq!(tabs[0]["active"], true);
    assert_eq!(tabs[1]["active"], false);
    assert_eq!(tabs[0]["status"], "idle");
    assert_eq!(tabs[0]["detail"], "Ready");
    let pane = &tabs[0]["panes"][0];
    assert_eq!(pane["id"], first_pane(&w));
    assert_eq!(pane["path"], ".0");
    assert_eq!(
        pane["title"], "build",
        "the spinner is the sidebar's to strip"
    );
    assert_eq!(pane["status"], "idle");
    assert_eq!(pane["detail"], "Ready");
    assert_eq!(pane["project"], "api");
    assert_eq!(pane["cwd"], "C:/work/api");
    assert_eq!(pane["agent"], Value::Null);
    assert_eq!(pane["active"], true);
    // A fake pane never started a shell: not running is what `exited` reports.
    assert_eq!(pane["exited"], true);
    assert_eq!(pane["exitCode"], Value::Null);
}

#[test]
fn a_pane_with_no_folder_falls_back_to_the_windows_home_project() {
    let (mut d, mut w) = setup();
    let result = ok(&call(&mut d, &mut w, CLIENT, "list", json!({})));
    assert_eq!(result["tabs"][0]["panes"][0]["project"], "home-project");
    assert_eq!(result["tabs"][0]["panes"][0]["cwd"], Value::Null);
}

#[test]
fn list_reads_the_status_the_sidebar_computed_and_never_steps_it() {
    // Break caught: list calling rail_step, which clears "finished" and
    // attention while the sidebar still shows them, and moves the Working timer.
    let (mut d, mut w) = setup();
    if let Some(mut pane) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        pane.feed(b"\x07");
        assert_eq!(pane.rail_step(false, 0), crate::rail::Status::NeedsYou);
    }
    for _ in 0..3 {
        let result = ok(&call(&mut d, &mut w, CLIENT, "list", json!({})));
        assert_eq!(result["tabs"][0]["panes"][0]["status"], "needs-you");
        assert_eq!(result["tabs"][0]["status"], "needs-you");
        assert_eq!(result["tabs"][0]["panes"][0]["detail"], "Needs you");
    }
    let pane = w.app.tabs()[0].pane();
    assert!(pane.needs_attention(), "attention is still there to see");
}

#[test]
fn a_tab_shows_its_most_urgent_pane_and_a_notification_message() {
    let (mut d, mut w) = setup();
    w.app.split_active(crate::split::Dir::Vertical);
    if let Some(mut pane) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(1)) {
        pane.feed(b"\x1b]9;Approve the edit?\x07");
        pane.rail_step(false, 0);
    }
    let result = ok(&call(&mut d, &mut w, CLIENT, "list", json!({})));
    let tab = &result["tabs"][0];
    assert_eq!(tab["status"], "needs-you");
    assert_eq!(tab["detail"], "Approve the edit?");
    assert_eq!(tab["panes"][0]["status"], "idle");
    assert_eq!(tab["panes"][1]["detail"], "Approve the edit?");
}

#[test]
fn stale_ids_are_refused_and_echoed_for_every_target() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let stale = 123_456_789;
    for (method, params) in [
        ("pane.read", json!({"pane": stale})),
        ("events.subscribe", json!({"pane": stale})),
        ("tab.close", json!({"tab": stale})),
        ("tab.focus", json!({"tab": stale})),
        ("pane.split", json!({"pane": stale})),
        ("pane.focus", json!({"pane": stale})),
        ("pane.send-text", json!({"pane": stale, "text": "x"})),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, method, params);
        let error = err(&frame);
        assert_eq!(error.code, ErrorCode::TargetStale, "{method}");
        assert_eq!(error.target, Some(stale), "{method}");
    }
    assert!(w.sent.is_empty());
}

#[test]
fn a_closed_id_is_stale_forever_even_when_its_slot_is_taken() {
    // Break caught (#56): a stale id acting on the tab that moved into its slot.
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let opened = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    let (tab, pane) = (
        opened["tab"].as_u64().expect("tab"),
        opened["pane"].as_u64().expect("pane"),
    );
    let closed = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.close",
        json!({"tab": tab}),
    ));
    assert_eq!(closed["tab"], tab);
    let other = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    assert_ne!(other["tab"], tab);
    assert_ne!(other["pane"], pane);
    for (method, params) in [
        ("tab.focus", json!({"tab": tab})),
        ("pane.read", json!({"pane": pane})),
        ("pane.send-text", json!({"pane": pane, "text": "x"})),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, method, params);
        assert_eq!(err(&frame).code, ErrorCode::TargetStale, "{method}");
    }
    assert!(w.sent.is_empty(), "nothing typed into the new tab");
}

#[test]
fn tab_open_passes_every_field_and_returns_resolvable_ids() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.open",
        json!({
            "dir": r"C:\work\café space",
            "profile": "pwsh",
            "name": "builder",
            "command": ["cargo", "test", "--lib"],
            "env": {"A": "1", "B": "two words"},
        }),
    ));
    let request = &w.opened[0];
    assert_eq!(request.dir.as_deref(), Some(r"C:\work\café space"));
    assert_eq!(request.profile.as_deref(), Some("pwsh"));
    assert_eq!(request.name.as_deref(), Some("builder"));
    assert_eq!(
        request.command.as_deref(),
        Some(&["cargo".to_string(), "test".to_string(), "--lib".to_string()][..])
    );
    assert_eq!(request.env.get("B").map(String::as_str), Some("two words"));
    let tab = result["tab"].as_u64().expect("tab");
    let pane = result["pane"].as_u64().expect("pane");
    assert_eq!(w.app.locate_tab(tab), Some(1));
    assert_eq!(w.app.locate_pane(pane), Some((1, 0)));
}

#[test]
fn a_session_cant_be_pointed_at_another_window_through_env() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.open",
        json!({"env": {"rustty_window": "1", "KEEP": "x"}}),
    );
    let env = &w.opened[0].env;
    assert_eq!(env.len(), 1, "{env:?}");
    assert!(env.contains_key("KEEP"));
}

#[test]
fn malformed_spawn_requests_are_bad_params_and_open_nothing() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    for params in [
        json!({"cwd": "C:\\oops"}),
        json!({"dir": 5}),
        json!({"dir": ""}),
        json!({"dir": "a\0b"}),
        json!({"profile": 1}),
        json!({"name": ""}),
        json!({"command": []}),
        json!({"command": "cargo test"}),
        json!({"command": ["cargo", 1]}),
        json!({"command": [""]}),
        json!({"command": ["a\0b"]}),
        json!({"env": "A=1"}),
        json!({"env": {"A": 1}}),
        json!({"env": {"": "x"}}),
        json!({"env": {"A=B": "x"}}),
        json!({"env": {"A": "x\0y"}}),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, "tab.open", params.clone());
        assert_eq!(err(&frame).code, ErrorCode::BadParams, "{params}");
    }
    assert!(w.opened.is_empty());
    assert_eq!(w.app.tab_count(), 1);
}

#[test]
fn a_failed_spawn_comes_back_as_the_windows_error() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    w.fail_open = Some(RpcError::bad_params("cannot start nope.exe"));
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.open",
        json!({"command": ["nope.exe"]}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    assert!(err(&frame).message.contains("nope.exe"));
}

#[test]
fn close_and_focus_find_their_target_by_id_whatever_is_selected() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let first = first_tab(&w);
    let second = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})))["tab"]
        .as_u64()
        .expect("tab");
    assert_eq!(w.app.active_index(), 0, "opened in the background");
    ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.focus",
        json!({"tab": second}),
    ));
    assert_eq!(w.app.active_index(), 1);
    ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.close",
        json!({"tab": first}),
    ));
    assert_eq!(w.app.tab_count(), 1);
    assert_eq!(w.app.locate_tab(second), Some(0));
}

#[test]
fn pane_focus_selects_its_tab_too() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let opened = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    let pane = opened["pane"].as_u64().expect("pane");
    let split = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.split",
        json!({"pane": pane}),
    ));
    assert_eq!(split["tab"], opened["tab"]);
    let fresh = split["pane"].as_u64().expect("pane");
    assert_ne!(fresh, pane);
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.focus",
        json!({"pane": fresh}),
    ));
    assert_eq!(result["pane"], fresh);
    assert_eq!(result["tab"], opened["tab"]);
    assert_eq!(w.app.active_index(), 1);
    assert_eq!(w.app.tabs()[1].pane().id(), fresh);
}

#[test]
fn pane_split_takes_a_direction_and_refuses_others() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let pane = first_pane(&w);
    let split = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.split",
        json!({"pane": pane, "direction": "down", "dir": "C:\\x", "command": ["a"], "env": {"K": "V"}}),
    ));
    assert_eq!(split["tab"], first_tab(&w));
    assert_eq!(w.splits[0].1, crate::split::Dir::Horizontal);
    assert_eq!(w.splits[0].0.dir.as_deref(), Some("C:\\x"));
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.split",
        json!({"pane": pane, "direction": "sideways"}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    call(&mut d, &mut w, CLIENT, "pane.split", json!({"pane": pane}));
    assert_eq!(
        w.splits[1].1,
        crate::split::Dir::Vertical,
        "right by default"
    );
}

#[test]
fn send_text_reaches_the_pane_as_utf8() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let pane = first_pane(&w);
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.send-text",
        json!({"pane": pane, "text": "héllo\r"}),
    ));
    assert_eq!(result["bytes"], "héllo\r".len());
    assert_eq!(w.sent, vec![(pane, "héllo\r".as_bytes().to_vec())]);
}

#[test]
fn send_text_over_the_burst_says_to_split_it() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let pane = first_pane(&w);
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.send-text",
        json!({"pane": pane, "text": "x".repeat(8 * 1024 + 1)}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    assert!(w.sent.is_empty());
    for params in [
        json!({"pane": pane}),
        json!({"pane": pane, "text": 5}),
        json!({"text": "x"}),
        json!({"pane": "1", "text": "x"}),
        json!({"pane": -1, "text": "x"}),
    ] {
        let frame = call(&mut d, &mut w, CLIENT, "pane.send-text", params.clone());
        assert_eq!(err(&frame).code, ErrorCode::BadParams, "{params}");
    }
}

#[test]
fn send_text_is_rate_limited_per_pane_and_reconnecting_does_not_refill_it() {
    // Break caught: a CLI that opens a new connection per call getting a fresh
    // burst each time, so the limit never applies to it.
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let pane = first_pane(&w);
    let chunk = "x".repeat(8 * 1024);
    ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.send-text",
        json!({"pane": pane, "text": chunk}),
    ));
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.send-text",
        json!({"pane": pane, "text": "x"}),
    );
    let error = err(&frame);
    assert_eq!(error.code, ErrorCode::RateLimited);
    assert!(error.retry_after_ms.is_some_and(|ms| ms >= 1));
    assert_eq!(w.sent.len(), 1, "the excess was not typed");

    d.client_gone(CLIENT);
    connect(&mut d, &mut w, 2);
    let frame = call(
        &mut d,
        &mut w,
        2,
        "pane.send-text",
        json!({"pane": pane, "text": "x"}),
    );
    assert_eq!(err(&frame).code, ErrorCode::RateLimited);

    // Another pane has its own.
    w.app.add_tab_sized(80, 24, 100);
    let other = w.app.tabs()[1].pane().id();
    ok(&call(
        &mut d,
        &mut w,
        2,
        "pane.send-text",
        json!({"pane": other, "text": "x"}),
    ));
}

#[test]
fn a_refused_send_comes_back_from_the_window() {
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    let pane = first_pane(&w);
    w.fail_send = Some(RpcError::closed(pane, "its shell has exited"));
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.send-text",
        json!({"pane": pane, "text": "x"}),
    );
    assert_eq!(err(&frame).code, ErrorCode::TargetClosed);
}

#[test]
fn delivering_text_to_a_pane_with_no_shell_is_target_closed() {
    let mut pane = crate::pane::Pane::new();
    let error = deliver_text(&mut pane, b"x").expect_err("no shell");
    assert_eq!(error.code, ErrorCode::TargetClosed);
    assert_eq!(error.target, Some(pane.id()));
    assert_eq!(pane.automation_input_age(), None, "nothing was typed");
}

#[test]
fn unknown_methods_and_malformed_requests_are_answered_not_dropped() {
    let (mut d, mut w) = setup();
    let frame = call(&mut d, &mut w, CLIENT, "no.such", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    let frame = call(&mut d, &mut w, CLIENT, "list", json!([1, 2]));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    let frame = d
        .handle(&mut w, CLIENT, &json!({"id": 9, "params": {}}), 0)
        .expect("a reply");
    assert_eq!(frame["id"], 9);
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
}

#[test]
fn a_notification_gets_no_reply() {
    let (mut d, mut w) = setup();
    assert_eq!(
        d.handle(&mut w, CLIENT, &json!({"method": "list"}), 0),
        None
    );
}

#[test]
fn unknown_params_are_refused_rather_than_ignored() {
    // Break caught: `{"cwd": ...}` silently opening a session in the wrong place.
    let (mut d, mut w) = setup();
    let frame = call(&mut d, &mut w, CLIENT, "list", json!({"verbose": true}));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    assert!(err(&frame).message.contains("verbose"));
    let frame = call(&mut d, &mut w, CLIENT, "pane.read", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    assert!(err(&frame).message.contains("pane"));
}

#[test]
fn replies_carry_the_requests_id() {
    let (mut d, mut w) = setup();
    for id in [json!(7), json!("abc"), Value::Null] {
        let frame = d
            .handle(&mut w, CLIENT, &json!({"id": id, "method": "list"}), 0)
            .expect("a reply");
        assert_eq!(frame["id"], id);
        assert_eq!(frame["jsonrpc"], "2.0");
    }
}

#[test]
fn reading_the_screen_gets_the_live_rows_wherever_the_person_scrolled() {
    // Break caught: a boss reading old history because someone scrolled back.
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        for i in 0..200 {
            p.feed(format!("line {i}\r\n").as_bytes());
        }
        p.grid_mut().scroll_up(50);
    }
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane}),
    ));
    let text = result["text"].as_str().expect("text");
    assert!(text.contains("line 199"), "{text}");
    assert!(!text.contains("line 100\n"), "{text}");
    assert_eq!(result["pane"], pane);
    assert_eq!(result["altScreen"], false);
    assert_eq!(result["rows"], 24);
    assert_eq!(result["cols"], 80);
    assert!(
        !text.ends_with("\n\n"),
        "blank rows below are trimmed: {text:?}"
    );
}

#[test]
fn reading_the_screen_shows_the_alternate_screen_when_it_is_up() {
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.feed(b"shell text\r\n\x1b[?1049hTUI here");
    }
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane}),
    ));
    assert_eq!(result["altScreen"], true);
    assert_eq!(result["text"], "TUI here\n");
}

#[test]
fn scrollback_reads_page_through_everything_in_order() {
    let (mut d, mut w) = setup_sized(120, 10, 5000);
    let pane = first_pane(&w);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        for i in 0..4000 {
            p.feed(format!("row {i}\r\n").as_bytes());
        }
    }
    let mut all = String::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut params = json!({"pane": pane, "scrollback": true});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let result = ok(&call(&mut d, &mut w, CLIENT, "pane.read", params));
        all.push_str(result["text"].as_str().expect("text"));
        cursor = result["cursor"].as_str().map(str::to_string);
        pages += 1;
        if result["more"] == false {
            break;
        }
    }
    let rows: Vec<&str> = all.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(rows.len(), 4000);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(*row, format!("row {i}"));
    }
    assert_eq!(pages, 1, "all of that fits one page");
}

fn setup_sized(cols: usize, rows: usize, scrollback: usize) -> (Dispatcher, FakeWindow) {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::sized(cols, rows, scrollback);
    connect(&mut d, &mut w, CLIENT);
    (d, w)
}

#[test]
fn every_page_fits_the_frame_cap_even_when_all_of_it_escapes() {
    // Break caught: a megabyte of `"` doubling past the 1 MiB frame cap, so
    // the write fails and the client hangs waiting for the reply.
    let (mut d, mut w) = setup_sized(180, 10, 20_000);
    let pane = first_pane(&w);
    let line = "\"".repeat(170);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        for _ in 0..12_000 {
            p.feed(format!("{line}\r\n").as_bytes());
        }
    }
    let mut cursor: Option<String> = None;
    let mut total = 0;
    let mut pages = 0;
    loop {
        let mut params = json!({"pane": pane, "scrollback": true});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let frame = call(&mut d, &mut w, CLIENT, "pane.read", params);
        let size = serde_json::to_vec(&frame).expect("json").len();
        assert!(size <= MAX_PAGE_BYTES, "{size}");
        let result = ok(&frame);
        total += result["text"]
            .as_str()
            .expect("text")
            .lines()
            .filter(|l| !l.is_empty())
            .count();
        cursor = result["cursor"].as_str().map(str::to_string);
        pages += 1;
        if result["more"] == false {
            break;
        }
    }
    assert!(pages >= 3, "{pages}");
    assert_eq!(total, 12_000);
}

#[test]
fn scrollback_reads_the_main_history_while_a_tui_is_up() {
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.feed(b"before the tui\r\n\x1b[?1049hTUI here");
    }
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true}),
    ));
    let text = result["text"].as_str().expect("text");
    assert!(text.contains("before the tui"), "{text}");
    assert!(!text.contains("TUI here"), "{text}");
}

#[test]
fn a_kept_cursor_reads_what_arrived_later() {
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.feed(b"one\r\ntwo\r\n");
    }
    let first = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true}),
    ));
    assert_eq!(first["more"], false);
    let cursor = first["cursor"].as_str().expect("cursor").to_string();
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.feed(b"three\r\n");
    }
    let second = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true, "cursor": cursor}),
    ));
    let text = second["text"].as_str().expect("text");
    assert!(text.contains("three") && !text.contains("one"), "{text:?}");
}

#[test]
fn lines_already_evicted_are_counted_not_faked() {
    let (mut d, mut w) = setup_sized(40, 5, 10);
    let pane = first_pane(&w);
    let cursor = encode_cursor(pane, 0);
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        for i in 0..100 {
            p.feed(format!("row {i}\r\n").as_bytes());
        }
    }
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true, "cursor": cursor}),
    ));
    assert!(result["skipped"].as_u64().unwrap_or(0) > 0, "{result}");
    assert!(result["text"].as_str().expect("text").contains("row 99"));
}

#[test]
fn a_cursor_from_another_pane_is_refused() {
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true, "cursor": encode_cursor(pane + 1, 0)}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
    // A cursor only makes sense for scrollback.
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "cursor": encode_cursor(pane, 0)}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
}

#[test]
fn a_cursor_past_the_end_is_an_empty_page() {
    let (mut d, mut w) = setup();
    let pane = first_pane(&w);
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "pane.read",
        json!({"pane": pane, "scrollback": true, "cursor": encode_cursor(pane, 1_000_000)}),
    ));
    assert_eq!(result["text"], "");
    assert_eq!(result["more"], false);
}

#[test]
fn the_window_going_away_during_a_request_is_target_closed() {
    // Break caught: closing the last tab destroys the window under the dispatcher.
    let (mut d, mut w) = setup();
    w.allow_writes = true;
    w.gone = true;
    let frame = call(&mut d, &mut w, CLIENT, "list", json!({}));
    assert_eq!(err(&frame).code, ErrorCode::TargetClosed);
}
