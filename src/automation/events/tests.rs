//! Events against a fake window: everything is read off a scan, as on a tick.

use serde_json::{Value, json};

use crate::automation::ops::testkit::{FakeWindow, call, connect, err, ok};
use crate::automation::ops::{ClientId, Dispatcher};
use crate::automation::{ErrorCode, OUTPUT_CHUNK_BYTES, json_escaped_len};
use crate::split::Dir;

const CLIENT: ClientId = 1;

fn setup() -> (Dispatcher, FakeWindow) {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::new();
    connect(&mut d, &mut w, CLIENT);
    w.allow_writes = true;
    (d, w)
}

fn subscribe(d: &mut Dispatcher, w: &mut FakeWindow, params: Value) -> Value {
    ok(&call(d, w, CLIENT, "events.subscribe", params))
}

/// Everything one scan sends, as `(client, params)` pairs.
fn scan(d: &mut Dispatcher, w: &FakeWindow) -> Vec<(ClientId, Value)> {
    let mut out = Vec::new();
    d.scan(&w.app, &mut |client, frame| {
        assert_eq!(frame["method"], "event");
        out.push((client, frame["params"].clone()));
    });
    out
}

fn names(events: &[(ClientId, Value)]) -> Vec<String> {
    events
        .iter()
        .map(|(_, e)| e["event"].as_str().unwrap_or("?").to_string())
        .collect()
}

fn feed(w: &mut FakeWindow, tab: usize, pane: usize, bytes: &[u8]) {
    if let Some(mut p) = w.app.tab_at_mut(tab).and_then(|t| t.pane_at_mut(pane)) {
        p.feed(bytes);
    }
}

fn pane_id(w: &FakeWindow, tab: usize, pane: usize) -> u64 {
    w.app.tabs()[tab].pane_at(pane).map_or(0, |p| p.id())
}

#[test]
fn nothing_is_sent_with_no_subscription() {
    let (mut d, mut w) = setup();
    feed(&mut w, 0, 0, b"text\r\n\x07");
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn subscribing_reports_nothing_that_already_happened() {
    let (mut d, mut w) = setup();
    feed(&mut w, 0, 0, b"old\r\n");
    subscribe(
        &mut d,
        &mut w,
        json!({"events": ["pane-output", "tab-opened"]}),
    );
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn tabs_opening_and_closing_are_reported_with_stable_ids() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    let opened = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["tab-opened"]);
    assert_eq!(events[0].1["tab"], opened["tab"]);
    assert_eq!(events[0].1["panes"][0], opened["pane"]);

    ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.close",
        json!({"tab": opened["tab"]}),
    ));
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-closed", "tab-closed"]);
    assert_eq!(events[0].1["pane"], opened["pane"]);
    assert_eq!(events[0].1["tab"], opened["tab"]);
    assert_eq!(events[1].1["tab"], opened["tab"]);
}

#[test]
fn a_tab_opened_right_after_subscribing_is_not_lost() {
    // Break caught: the baseline taken at the next scan swallowing a tab opened
    // in the same tick as the subscribe.
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    assert_eq!(names(&scan(&mut d, &w)), ["tab-opened"]);
}

#[test]
fn a_pane_closing_in_a_split_is_reported_alone() {
    let (mut d, mut w) = setup();
    w.app.split_active(Dir::Vertical);
    let gone = pane_id(&w, 0, 1);
    subscribe(&mut d, &mut w, json!({}));
    w.app.close_tab_pane(0, 1);
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-closed"]);
    assert_eq!(events[0].1["pane"], gone);
}

#[test]
fn a_bell_is_reported_for_its_pane() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    let pane = pane_id(&w, 0, 0);
    feed(&mut w, 0, 0, b"\x07");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["bell"]);
    assert_eq!(events[0].1["pane"], pane);
    // The window clears the flag right after each scan.
    w.app.take_bells();
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn commands_report_start_and_finish_with_exit_code_and_duration() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    let pane = pane_id(&w, 0, 0);
    feed(
        &mut w,
        0,
        0,
        b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C\x07",
    );
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["command-started"]);
    assert_eq!(events[0].1["pane"], pane);
    let command = events[0].1["command"].clone();
    assert!(command.is_u64());

    feed(&mut w, 0, 0, b"out\r\n\x1b]133;D:3\x07");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["command-finished"]);
    assert_eq!(events[0].1["command"], command);
    assert_eq!(events[0].1["exitCode"], 3);
    assert!(events[0].1["durationMs"].is_u64());
    assert!(scan(&mut d, &w).is_empty(), "each is reported once");
}

#[test]
fn a_command_that_starts_and_ends_between_scans_reports_both_in_order() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    feed(
        &mut w,
        0,
        0,
        b"\x1b]133;B\x07\x1b]133;C\x07a\r\n\x1b]133;D:0\x07\x1b]133;B\x07\x1b]133;C\x07b\r\n\x1b]133;D:1\x07",
    );
    let events = scan(&mut d, &w);
    assert_eq!(
        names(&events),
        [
            "command-started",
            "command-finished",
            "command-started",
            "command-finished"
        ]
    );
    assert_eq!(events[1].1["exitCode"], 0);
    assert_eq!(events[3].1["exitCode"], 1);
}

#[test]
fn status_changes_come_from_the_sidebars_step_only() {
    // Break caught: scanning stepping activity itself and clearing "finished".
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    assert!(scan(&mut d, &w).is_empty());
    let pane = pane_id(&w, 0, 0);
    feed(&mut w, 0, 0, b"\x1b]9;Approve the edit?\x07");
    assert!(
        scan(&mut d, &w)
            .iter()
            .all(|(_, e)| e["event"] != "status-changed"),
        "no step yet, so no change"
    );
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.rail_step(false, 0);
    }
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["status-changed"]);
    assert_eq!(events[0].1["pane"], pane);
    assert_eq!(events[0].1["status"], "needs-you");
    assert_eq!(events[0].1["detail"], "Approve the edit?");
    assert_eq!(events[0].1["detailSource"], "notification");
    assert!(scan(&mut d, &w).is_empty());
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.rail_step(true, 10);
    }
    let events = scan(&mut d, &w);
    assert_eq!(events[0].1["status"], "working", "recent output, now seen");
    assert_eq!(events[0].1["detail"], "Working");
    assert_eq!(events[0].1["detailSource"], "none");
}

#[test]
fn a_new_notification_while_already_needing_you_is_a_change() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    feed(&mut w, 0, 0, b"\x1b]9;first\x07");
    if let Some(mut p) = w.app.tab_at_mut(0).and_then(|t| t.pane_at_mut(0)) {
        p.rail_step(false, 0);
    }
    scan(&mut d, &w);
    feed(&mut w, 0, 0, b"\x1b]9;second\x07");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["status-changed"]);
    assert_eq!(events[0].1["detail"], "second");
}

#[test]
fn output_arrives_in_order_as_whole_lines_without_escapes() {
    let (mut d, mut w) = setup();
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 0, b"\x1b[31mred\x1b[0m\r\nsecond\r\npartial");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-output"]);
    assert_eq!(events[0].1["pane"], pane);
    assert_eq!(events[0].1["text"], "red\nsecond\n");
    assert_eq!(events[0].1["skipped"], Value::Null);
    let from = events[0].1["fromLine"].as_u64().expect("from");
    let next = events[0].1["nextLine"].as_u64().expect("next");
    assert_eq!(next, from + 2);

    feed(&mut w, 0, 0, b" done\r\nthird\r\n");
    let events = scan(&mut d, &w);
    assert_eq!(events[0].1["text"], "partial done\nthird\n");
    assert_eq!(events[0].1["fromLine"].as_u64(), Some(next));
    let mut seqs: Vec<u64> = events
        .iter()
        .filter_map(|(_, e)| e["seq"].as_u64())
        .collect();
    seqs.dedup();
    assert_eq!(seqs.len(), events.len());
}

#[test]
fn output_starts_at_subscribing_for_an_existing_pane() {
    let (mut d, mut w) = setup();
    feed(&mut w, 0, 0, b"before\r\n");
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 0, b"after\r\n");
    let events = scan(&mut d, &w);
    assert_eq!(events[0].1["text"], "after\n");
}

#[test]
fn a_window_wide_output_subscription_gets_a_new_panes_output_from_its_first_line() {
    // Break caught: a boss opening a short-lived command and losing what it
    // printed before a per-pane subscribe could arrive.
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({"events": ["pane-output"]}));
    let opened = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    feed(&mut w, 1, 0, b"first line\r\n");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-output"]);
    assert_eq!(events[0].1["pane"], opened["pane"]);
    assert_eq!(events[0].1["text"], "first line\n");
}

#[test]
fn a_window_wide_subscription_leaves_out_output_unless_it_is_asked_for() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    feed(&mut w, 0, 0, b"text\r\n");
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn output_bursts_are_cut_into_bounded_chunks_that_lose_nothing() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::sized(200, 10, 60_000);
    connect(&mut d, &mut w, CLIENT);
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    let row = "y".repeat(190);
    for i in 0..5000 {
        feed(&mut w, 0, 0, format!("{i:05} {row}\r\n").as_bytes());
    }
    let mut seen = 0;
    let mut scans = 0;
    while seen < 5000 {
        let events = scan(&mut d, &w);
        assert!(!events.is_empty(), "stalled at {seen}");
        for (_, event) in events {
            let text = event["text"].as_str().expect("text");
            assert!(json_escaped_len(text) <= OUTPUT_CHUNK_BYTES);
            for line in text.lines() {
                assert!(line.starts_with(&format!("{seen:05} ")), "{line}");
                seen += 1;
            }
        }
        scans += 1;
    }
    assert!(scans >= 3, "one tick's output is bounded: {scans}");
}

#[test]
fn busy_panes_take_turns_when_the_scan_runs_out_of_time() {
    // Break caught: the first pane in a flood using every tick's time while the
    // second's output scrolls out of history unread.
    let (mut d, mut w) = setup();
    w.app.split_active(Dir::Vertical);
    let (a, b) = (pane_id(&w, 0, 0), pane_id(&w, 0, 1));
    subscribe(&mut d, &mut w, json!({"pane": a}));
    subscribe(&mut d, &mut w, json!({"pane": b}));
    for i in 0..4 {
        feed(&mut w, 0, 0, format!("a{i}\r\n").as_bytes());
        feed(&mut w, 0, 1, format!("b{i}\r\n").as_bytes());
    }
    d.set_output_budget(std::time::Duration::ZERO);
    let mut seen = Vec::new();
    for _ in 0..4 {
        let events = scan(&mut d, &w);
        assert_eq!(events.len(), 1, "out of time after one chunk: {events:?}");
        seen.push(events[0].1["pane"].as_u64().expect("pane"));
    }
    assert!(seen.contains(&a) && seen.contains(&b), "{seen:?}");
}

#[test]
fn output_lost_to_eviction_is_counted_never_silently_skipped() {
    let mut d = Dispatcher::new();
    let mut w = FakeWindow::sized(40, 5, 20);
    connect(&mut d, &mut w, CLIENT);
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    for i in 0..200 {
        feed(&mut w, 0, 0, format!("row {i}\r\n").as_bytes());
    }
    let events = scan(&mut d, &w);
    assert!(
        events[0].1["skipped"].as_u64().unwrap_or(0) > 100,
        "{events:?}"
    );
    let text = events[0].1["text"].as_str().expect("text");
    assert!(text.ends_with("row 199\n"), "{text}");
}

#[test]
fn output_waits_while_a_tui_owns_the_screen() {
    let (mut d, mut w) = setup();
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 0, b"before\r\n");
    assert_eq!(scan(&mut d, &w)[0].1["text"], "before\n");
    feed(&mut w, 0, 0, b"\x1b[?1049hTUI\r\nmore\r\nlines\r\n");
    assert!(scan(&mut d, &w).is_empty());
    feed(&mut w, 0, 0, b"\x1b[?1049lafter\r\n");
    let events = scan(&mut d, &w);
    let text = events[0].1["text"].as_str().expect("text");
    assert!(text.contains("after") && !text.contains("TUI"), "{text}");
}

#[test]
fn a_pane_subscription_ends_with_a_terminal_event_whatever_it_filtered() {
    let (mut d, mut w) = setup();
    w.app.split_active(Dir::Vertical);
    let gone = pane_id(&w, 0, 1);
    subscribe(
        &mut d,
        &mut w,
        json!({"pane": gone, "events": ["pane-output"]}),
    );
    w.app.close_tab_pane(0, 1);
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-closed"]);
    // That was its last event: the subscription is gone with the pane.
    assert!(!d.has_subscribers());
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn a_pane_subscription_sees_only_its_pane() {
    let (mut d, mut w) = setup();
    w.app.split_active(Dir::Vertical);
    let (a, b) = (pane_id(&w, 0, 0), pane_id(&w, 0, 1));
    subscribe(&mut d, &mut w, json!({"pane": a}));
    feed(&mut w, 0, 1, b"not this\r\n\x07");
    feed(&mut w, 0, 0, b"this\r\n");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-output"]);
    assert_eq!(events[0].1["pane"], a);
    assert_ne!(events[0].1["pane"], b);
    // A tab opening names no pane it is watching.
    ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    assert!(scan(&mut d, &w).is_empty());
}

#[test]
fn the_events_filter_is_honoured() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({"events": ["bell"]}));
    ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    feed(&mut w, 0, 0, b"\x07");
    assert_eq!(names(&scan(&mut d, &w)), ["bell"]);
    let frame = call(
        &mut d,
        &mut w,
        CLIENT,
        "events.subscribe",
        json!({"events": ["no-such-event"]}),
    );
    assert_eq!(err(&frame).code, ErrorCode::BadParams);
}

#[test]
fn subscribing_twice_to_a_pane_replaces_it() {
    // ADR §5: one subscription per client per pane.
    let (mut d, mut w) = setup();
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 0, b"once\r\n");
    assert_eq!(names(&scan(&mut d, &w)), ["pane-output"]);
}

#[test]
fn an_event_goes_to_a_client_once_however_many_subscriptions_match() {
    let (mut d, mut w) = setup();
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    subscribe(&mut d, &mut w, json!({}));
    feed(&mut w, 0, 0, b"\x07");
    assert_eq!(names(&scan(&mut d, &w)), ["bell"]);
}

#[test]
fn unsubscribing_stops_events_immediately() {
    // Acceptance (#58): cancellation stops the stream promptly.
    let (mut d, mut w) = setup();
    let pane = pane_id(&w, 0, 0);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 0, b"pending output\r\n");
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "events.unsubscribe",
        json!({"pane": pane}),
    ));
    assert_eq!(result["removed"], true);
    assert!(scan(&mut d, &w).is_empty(), "not even what was pending");
    let result = ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "events.unsubscribe",
        json!({"pane": pane}),
    ));
    assert_eq!(result["removed"], false, "unsubscribing twice is harmless");
}

#[test]
fn clients_are_independent_and_a_departed_one_gets_nothing() {
    let (mut d, mut w) = setup();
    connect(&mut d, &mut w, 2);
    subscribe(&mut d, &mut w, json!({}));
    ok(&call(
        &mut d,
        &mut w,
        2,
        "events.subscribe",
        json!({"events": ["bell"]}),
    ));
    feed(&mut w, 0, 0, b"\x07");
    let events = scan(&mut d, &w);
    let mut to: Vec<ClientId> = events.iter().map(|(c, _)| *c).collect();
    to.sort_unstable();
    assert_eq!(to, [CLIENT, 2]);
    w.app.take_bells();
    d.client_gone(2);
    feed(&mut w, 0, 0, b"\x07");
    let events = scan(&mut d, &w);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, CLIENT);
}

#[test]
fn seq_goes_up_across_events() {
    let (mut d, mut w) = setup();
    subscribe(&mut d, &mut w, json!({}));
    ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    feed(&mut w, 0, 0, b"\x07");
    let events = scan(&mut d, &w);
    assert_eq!(events.len(), 2);
    let first = events[0].1["seq"].as_u64().expect("seq");
    let second = events[1].1["seq"].as_u64().expect("seq");
    assert!(second > first);
}

#[test]
fn a_pane_events_order_stays_output_before_its_close() {
    // Break caught: the last lines a command printed arriving after it was
    // reported closed, or not at all.
    let (mut d, mut w) = setup();
    w.app.split_active(Dir::Vertical);
    let pane = pane_id(&w, 0, 1);
    subscribe(&mut d, &mut w, json!({"pane": pane}));
    feed(&mut w, 0, 1, b"last words\r\n");
    let events = scan(&mut d, &w);
    assert_eq!(names(&events), ["pane-output"]);
    w.app.close_tab_pane(0, 1);
    assert_eq!(names(&scan(&mut d, &w)), ["pane-closed"]);
}

#[test]
fn a_subscription_to_a_pane_that_vanished_before_a_scan_still_ends_cleanly() {
    let (mut d, mut w) = setup();
    let opened = ok(&call(&mut d, &mut w, CLIENT, "tab.open", json!({})));
    subscribe(&mut d, &mut w, json!({"pane": opened["pane"]}));
    ok(&call(
        &mut d,
        &mut w,
        CLIENT,
        "tab.close",
        json!({"tab": opened["tab"]}),
    ));
    assert_eq!(names(&scan(&mut d, &w)), ["pane-closed"]);
    assert!(!d.has_subscribers());
}
