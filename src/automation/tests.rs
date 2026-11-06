//! Tests of the pure automation core: everything here runs without a window.

use super::*;

#[test]
fn ids_only_go_up_and_stay_safe_in_json() {
    // Break caught: a JavaScript MCP client rounding an id above 2^53 onto a
    // different pane.
    let a = next_id();
    let b = next_id();
    assert!(b > a);
    assert!(a < (1 << 53), "{a}");
    // The window's process is in the id, so an id from one process can't
    // name a target in another.
    assert_eq!(a >> ID_PID_SHIFT, u64::from(std::process::id()));
}

#[test]
fn every_error_has_a_fixed_distinct_exit_code() {
    // The owner's scripts branch on these numbers: changing one is a breaking
    // change, so they are written out here on purpose.
    let table = [
        ("no-instance", 5),
        ("target-stale", 3),
        ("target-closed", 8),
        ("denied-grant", 4),
        ("timeout", 9),
        ("slow-client", 10),
        ("bad-params", 2),
        ("protocol-mismatch", 6),
        ("internal", 1),
        ("rate-limited", 11),
    ];
    assert_eq!(ERROR_CODES.len(), table.len());
    for (name, exit) in table {
        let code = ErrorCode::parse(name).expect(name);
        assert_eq!(code.as_str(), name);
        assert_eq!(code.exit_code(), exit, "{name}");
    }
    assert_eq!(ErrorCode::parse("nope"), None);
    // Shared meanings match the session verbs' exit codes.
    assert_eq!(
        ErrorCode::TargetStale.exit_code(),
        crate::host::EXIT_NOT_FOUND
    );
    assert_eq!(
        ErrorCode::NoInstance.exit_code(),
        crate::host::EXIT_HOST_UNAVAILABLE
    );
    assert_eq!(
        ErrorCode::ProtocolMismatch.exit_code(),
        crate::host::EXIT_PROTOCOL
    );
    assert_eq!(ErrorCode::BadParams.exit_code(), crate::host::EXIT_USAGE);
}

#[test]
fn an_error_frame_echoes_the_id_and_target_and_round_trips() {
    let error = RpcError::stale(77);
    let frame = error_frame(json!(5), &error);
    assert_eq!(frame["id"], 5);
    assert_eq!(frame["error"]["code"], "target-stale");
    assert_eq!(frame["error"]["target"], 77);
    let back = RpcError::from_value(&frame["error"]);
    assert_eq!(back, error);

    let limited = RpcError::rate_limited(250);
    let back = RpcError::from_value(&limited.to_value());
    assert_eq!(back.retry_after_ms, Some(250));
    assert_eq!(back.code, ErrorCode::RateLimited);

    // An error we can't read is still an error, never success.
    assert_eq!(
        RpcError::from_value(&json!("garbage")).code,
        ErrorCode::Internal
    );
}

#[test]
fn capabilities_list_every_operation_once_with_its_grant() {
    let caps = capabilities();
    let ops = caps["operations"].as_array().expect("operations");
    let names: Vec<&str> = ops.iter().filter_map(|op| op["name"].as_str()).collect();
    assert_eq!(
        names,
        [
            "initialize",
            "capabilities",
            "list",
            "pane.read",
            "events.subscribe",
            "events.unsubscribe",
            "tab.open",
            "tab.close",
            "tab.focus",
            "pane.split",
            "pane.focus",
            "pane.send-text",
        ]
    );
    for op in ops {
        let name = op["name"].as_str().unwrap_or_default();
        let write = [
            "tab.open",
            "tab.close",
            "tab.focus",
            "pane.split",
            "pane.focus",
            "pane.send-text",
        ]
        .contains(&name);
        let read = ["list", "pane.read", "events.subscribe"].contains(&name);
        let wanted = if write {
            "write"
        } else if read {
            "read"
        } else {
            "none"
        };
        assert_eq!(op["grant"], wanted, "{name}");
        assert_eq!(is_write_op(name), write, "{name}");
        assert_eq!(op["params"]["type"], "object", "{name}");
        assert!(
            !op["description"].as_str().unwrap_or("").is_empty(),
            "{name}"
        );
    }
    assert!(!is_write_op("no-such-op"));
}

#[test]
fn capabilities_list_the_events_and_errors() {
    let caps = capabilities();
    let events: Vec<&str> = caps["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        events,
        [
            "tab-opened",
            "tab-closed",
            "pane-closed",
            "bell",
            "command-started",
            "command-finished",
            "pane-output",
            "status-changed",
        ]
    );
    let errors = caps["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), ERROR_CODES.len());
    assert_eq!(caps["protocolVersion"], PROTOCOL_VERSION);
}

#[test]
fn the_token_is_not_part_of_any_schema_except_initialize() {
    // Break caught: a token passed on each call ending up in a client's logs.
    let caps = capabilities();
    for op in caps["operations"].as_array().expect("operations") {
        let has = op["params"]["properties"].get("token").is_some();
        assert_eq!(has, op["name"] == "initialize", "{}", op["name"]);
    }
}

#[test]
fn version_negotiation_allows_one_step_back_and_nothing_else() {
    assert_eq!(negotiate(PROTOCOL_VERSION), Ok(PROTOCOL_VERSION));
    assert_eq!(negotiate_with(2, 2), Ok(2));
    assert_eq!(negotiate_with(2, 1), Ok(1));
    for (server, client) in [(2, 3), (3, 1), (1, 0), (2, 0), (1, 2)] {
        let err = negotiate_with(server, client).expect_err("hard mismatch");
        assert_eq!(err.code, ErrorCode::ProtocolMismatch, "{server} {client}");
    }
}

#[test]
fn comparison_covers_length_and_content() {
    assert!(ct_eq(b"abc", b"abc"));
    assert!(!ct_eq(b"abc", b"abd"));
    assert!(!ct_eq(b"abc", b"ab"));
    assert!(!ct_eq(b"ab", b"abc"));
    assert!(!ct_eq(b"", b"a"));
    assert!(ct_eq(b"", b""));
}

#[test]
fn the_token_never_prints() {
    // Break caught: `{:?}` of some state struct writing the token to a log.
    let token = Token::from_random_bytes(&[0xAB; 32]);
    assert_eq!(token.as_str().len(), 64);
    assert!(token.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert!(token.matches(token.as_str()));
    assert!(!token.matches("abab"));
    assert!(!token.matches(""));
    assert!(!format!("{token:?}").contains("abab"));
    let other = Token::from_random_bytes(&[0xAC; 32]);
    assert!(!token.matches(other.as_str()));
}

#[test]
fn writes_need_the_config_key_or_a_good_token() {
    assert!(!can_write(false, false));
    assert!(can_write(true, false));
    assert!(can_write(false, true));
}

#[test]
fn the_send_bucket_allows_a_burst_then_the_sustained_rate() {
    let mut bucket = TokenBucket::new(0);
    assert_eq!(bucket.take(8 * 1024, 0), Ok(()));
    // Empty: one byte has to wait, and it says for how long.
    let wait = bucket.take(1, 0).expect_err("empty");
    assert_eq!(wait, 1);
    // 64 KiB/s is 64 bytes in a millisecond and a bit.
    assert_eq!(bucket.take(64, 1), Ok(()));
    let wait = bucket.take(1024, 1).expect_err("not refilled yet");
    assert!((15..=17).contains(&wait), "{wait}");
    assert_eq!(bucket.take(1024, 1 + wait), Ok(()));
}

#[test]
fn the_send_bucket_refills_to_the_burst_and_no_further() {
    let mut bucket = TokenBucket::new(0);
    assert_eq!(bucket.take(8 * 1024, 0), Ok(()));
    // An hour idle buys one burst, not an hour of bytes.
    assert_eq!(bucket.take(8 * 1024, 3_600_000), Ok(()));
    assert!(bucket.take(1, 3_600_000).is_err());
    // A clock that goes backwards refills nothing and doesn't panic.
    assert!(bucket.take(1, 10).is_err());
}

#[test]
fn a_rejected_take_spends_nothing() {
    let mut bucket = TokenBucket::new(0);
    assert!(bucket.take(9 * 1024, 0).is_err());
    assert_eq!(bucket.take(8 * 1024, 0), Ok(()));
}

#[test]
fn sustained_sending_holds_the_published_rate() {
    let mut bucket = TokenBucket::new(0);
    let mut sent = 0u64;
    for ms in 0..=10_000u64 {
        while bucket.take(512, ms).is_ok() {
            sent += 512;
        }
    }
    // 10 s at 64 KiB/s plus the first burst.
    let want = 10 * SEND_RATE_BYTES_PER_SEC + SEND_BURST_BYTES;
    assert!(sent <= want, "{sent} over {want}");
    assert!(sent > want - 1024, "{sent} under {want}");
}

#[test]
fn escaped_length_matches_what_json_really_takes() {
    for text in [
        "plain",
        "quote\" back\\slash",
        "line\nfeed\ttab",
        "\x01\x1f control",
        "héllo wörld 日本語 🦀",
        "",
    ] {
        let real = serde_json::to_string(text).expect("json").len() - 2;
        assert_eq!(json_escaped_len(text), real, "{text:?}");
    }
}

#[test]
fn a_page_stops_before_the_budget_counting_escapes() {
    // Break caught: 1 MiB of `"` becoming a 2 MiB frame, which the frame cap
    // then refuses, so the read fails forever at that line.
    let line = "\"".repeat(100);
    let page = read_lines(10, 1000, 1000, |_| Some(line.clone()));
    assert_eq!(page.from, 10);
    assert!(json_escaped_len(&page.text) <= 1000, "{}", page.text.len());
    assert!(page.more);
    assert_eq!(page.next, 14, "4 lines of 202 escaped bytes each");
    assert_eq!(page.text.lines().count(), 4);
}

#[test]
fn a_page_reaches_the_end_and_says_so() {
    let page = read_lines(0, 2, 10_000, |abs| Some(format!("line {abs}")));
    assert_eq!(page.text, "line 0\nline 1\nline 2\n");
    assert_eq!(page.next, 3);
    assert!(!page.more);
    // A range that is already over is an empty page, not an error.
    let page = read_lines(5, 2, 10_000, |_| Some("x".to_string()));
    assert_eq!(page.text, "");
    assert_eq!(page.next, 5);
    assert!(!page.more);
}

#[test]
fn a_page_can_stop_early_when_time_is_up_and_still_moves() {
    // Break caught: one 1 MiB read holding the UI thread past a whole tick.
    let mut calls = 0;
    let page = read_lines_while(
        10,
        1000,
        1_000_000,
        &mut || {
            calls += 1;
            calls < 3
        },
        |abs| Some(format!("line {abs}")),
    );
    assert_eq!(page.text, "line 10\nline 11\nline 12\n");
    assert_eq!(page.next, 13);
    assert!(page.more, "the caller is told to come back");
    // Out of time from the start still reads one line, so a read can't stall.
    let page = read_lines_while(10, 1000, 1_000_000, &mut || false, |_| {
        Some("x".to_string())
    });
    assert_eq!(page.text, "x\n");
    assert_eq!(page.next, 11);
    // Finishing exactly as time runs out isn't "more".
    let page = read_lines_while(10, 10, 1_000_000, &mut || false, |_| Some("x".to_string()));
    assert!(!page.more);
}

#[test]
fn a_line_longer_than_the_page_is_cut_so_the_read_still_moves() {
    let page = read_lines(0, 1, 16, |_| Some("é".repeat(100)));
    assert!(json_escaped_len(&page.text) <= 16);
    assert_eq!(page.next, 1, "the read advanced");
    assert!(page.more);
    // Evicted lines read as blank rather than ending the page.
    let page = read_lines(0, 1, 100, |_| None);
    assert_eq!(page.text, "\n\n");
}

#[test]
fn cursors_belong_to_their_pane() {
    let cursor = encode_cursor(7, 123);
    assert_eq!(decode_cursor(7, &cursor), Ok(123));
    // Break caught: a cursor from one pane reading another pane's lines.
    assert_eq!(
        decode_cursor(8, &cursor).expect_err("other pane").code,
        ErrorCode::BadParams
    );
    for bad in ["", "7", "7:", ":1", "x:y", "7:-1", "7:1:2"] {
        assert!(decode_cursor(7, bad).is_err(), "{bad}");
    }
}

#[test]
fn the_outbox_refuses_past_its_cap_and_never_grows() {
    let mut out = Outbox::new(10);
    assert_eq!(out.push(vec![0; 6]), Push::Queued);
    assert_eq!(out.push(vec![0; 4]), Push::Queued);
    assert_eq!(out.bytes(), 10);
    assert_eq!(out.push(vec![0; 1]), Push::Overflow);
    assert!(out.is_closed());
    assert_eq!(out.bytes(), 0, "an overflowing client's backlog is dropped");
    assert_eq!(out.push(vec![0; 1]), Push::Closed);
    assert_eq!(out.pop(), None);
}

#[test]
fn the_outbox_keeps_order_and_frees_what_was_written() {
    let mut out = Outbox::new(10);
    out.push(vec![1; 6]);
    out.push(vec![2; 4]);
    assert_eq!(out.pop(), Some(vec![1; 6]));
    assert_eq!(out.bytes(), 4);
    assert_eq!(out.push(vec![3; 6]), Push::Queued);
    assert_eq!(out.pop(), Some(vec![2; 4]));
    assert_eq!(out.pop(), Some(vec![3; 6]));
    assert_eq!(out.pop(), None);
    assert!(!out.is_closed());
}

#[test]
fn the_slow_client_frame_goes_out_after_the_overflow() {
    let mut out = Outbox::new(4);
    out.push(vec![1; 4]);
    assert_eq!(out.push(vec![2; 4]), Push::Overflow);
    out.set_final(vec![9; 3]);
    assert_eq!(out.pop(), Some(vec![9; 3]));
    assert_eq!(out.pop(), None);
    assert!(out.is_closed());
}

fn entry(id: u64, pid: u32, quick: bool) -> DiscoveryEntry {
    DiscoveryEntry {
        id,
        pipe: format!(r"\\.\pipe\rustty.me.{pid}.1"),
        pid,
        start_time: 1,
        quick,
        version: "0".to_string(),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustty-auto-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn honest_entry() -> DiscoveryEntry {
    let pid = 4242;
    DiscoveryEntry {
        id: (u64::from(pid) << ID_PID_SHIFT) + 7,
        pipe: r"\\.\pipe\rustty.me.4242.1".to_string(),
        pid,
        start_time: 99,
        quick: false,
        version: "0".to_string(),
    }
}

#[test]
fn an_honest_entry_is_trusted() {
    assert_eq!(entry_problem(&honest_entry(), Some(99)), None);
}

#[test]
fn an_id_must_belong_to_the_process_claiming_it() {
    // The attack: the real window's id, with the attacker's own pid and pipe.
    let mut fake = honest_entry();
    fake.pid = 4243;
    assert!(entry_problem(&fake, Some(99)).is_some());
}

#[test]
fn a_start_time_must_be_real_and_match_the_live_process() {
    let mut fake = honest_entry();
    assert!(entry_problem(&fake, Some(100)).is_some(), "a reused pid");
    assert!(entry_problem(&fake, None).is_some(), "it can't be checked");
    fake.start_time = 0;
    assert!(
        entry_problem(&fake, Some(0)).is_some(),
        "zero proves nothing"
    );
}

#[test]
fn only_a_local_rustty_pipe_name_is_trusted() {
    for bad in [
        r"\\server\pipe\rustty.me.4242.1",
        r"\\?\pipe\rustty.me.4242.1",
        r"\\.\UNC\server\pipe\rustty.me.4242.1",
        "//./pipe/rustty.me.4242.1",
        r"\\.\pipe\other.me.4242.1",
        r"\\.\PIPE\rustty.me.4242.1",
        r"\\.\pipe\rustty.",
        // Windows folds `..` before it looks at the host part.
        r"\\.\pipe\rustty.me\..\..\..\UNC\server\pipe\x",
        r"\\.\pipe\rustty.me/../x",
        r"\\.\pipe\rustty.me 4242",
        "",
    ] {
        let mut fake = honest_entry();
        fake.pipe = bad.to_string();
        assert!(entry_problem(&fake, Some(99)).is_some(), "{bad}");
    }
    // What `pipe_name` makes is what is trusted, whatever the user is called.
    for user in ["Dhruv Parpia", "a.b\\c", "", "\u{e9}"] {
        let mut fine = honest_entry();
        fine.pipe = pipe_name(user, fine.pid, 3);
        assert_eq!(entry_problem(&fine, Some(99)), None, "{user:?}");
    }
}

#[test]
fn an_untrusted_entry_is_dropped_before_a_window_is_chosen() {
    let good = honest_entry();
    let mut bad = honest_entry();
    bad.id += 1;
    bad.pipe = "//./pipe/rustty.x".to_string();
    let kept = trusted_entries(vec![good.clone(), bad], &|_| Some(99));
    assert_eq!(kept, vec![good]);
}

#[test]
fn two_entries_for_one_window_fail_closed_rather_than_picking_one() {
    // Break caught: whichever file sorts first deciding who gets the token.
    let good = honest_entry();
    let mut fake = honest_entry();
    fake.pid = 4243;
    fake.pipe = r"\\.\pipe\rustty.evil.4243.1".to_string();
    // The fake isn't dropped for being untrusted: that would hide the duplicate.
    let kept = trusted_entries(vec![good.clone(), fake], &|_| Some(99));
    assert_eq!(kept.len(), 2);
    let error = select_window(&kept, Some(good.id), None).expect_err("refused");
    assert_eq!(error.code, ErrorCode::NoInstance, "{error}");
    let by_env = good.id.to_string();
    let error = select_window(&kept, None, Some(&by_env)).expect_err("refused");
    assert_eq!(error.code, ErrorCode::NoInstance, "{error}");
    // Nothing named: nobody is chosen either.
    assert!(select_window(&kept, None, None).is_err());
}

#[test]
fn a_discovery_file_holds_only_the_public_facts() {
    // Break caught: the write token landing in a world of files other tools read.
    let dir = temp_dir("fields");
    let path = write_discovery(&dir, &entry(9, 100, false)).expect("write");
    let text = std::fs::read_to_string(&path).expect("read");
    let value: Value = serde_json::from_str(&text).expect("json");
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "pid", "pipe", "quick", "startTime", "version"]);
    assert!(!text.to_ascii_lowercase().contains("token"));
    std::fs::remove_dir_all(&dir).expect("clean");
}

#[test]
fn discovery_prunes_the_dead_and_the_unreadable() {
    let dir = temp_dir("prune");
    write_discovery(&dir, &entry(1, 100, false)).expect("write");
    let dead = write_discovery(&dir, &entry(2, 200, false)).expect("write");
    let junk = dir.join("junk.json");
    std::fs::write(&junk, "{ not json").expect("write");
    let found = load_discovery(&dir, &|e| e.pid == 100);
    assert_eq!(found, vec![entry(1, 100, false)]);
    assert!(!dead.exists(), "a dead window's file is removed");
    assert!(!junk.exists(), "an unreadable file is removed");
    std::fs::remove_dir_all(&dir).expect("clean");
}

#[test]
fn a_missing_discovery_directory_is_just_no_windows() {
    let dir = std::env::temp_dir().join("rustty-auto-never-created-x9");
    assert!(load_discovery(&dir, &|_| true).is_empty());
}

#[test]
fn the_discovery_directory_lives_under_appdata() {
    assert_eq!(
        discovery_dir(Path::new(r"C:\Users\me\AppData\Roaming")),
        Path::new(r"C:\Users\me\AppData\Roaming")
            .join("rustty")
            .join("ipc")
    );
}

#[test]
fn pipe_names_follow_the_adr_and_stay_legal() {
    assert_eq!(
        pipe_name("Dhruv P\\x", 12, 3),
        r"\\.\pipe\rustty.dhruv_p_x.12.3"
    );
    assert_eq!(pipe_name("", 12, 3), r"\\.\pipe\rustty.user.12.3");
}

#[test]
fn no_window_at_all_is_no_instance() {
    for (explicit, env) in [(None, None), (Some(4), None), (None, Some("4"))] {
        let err = select_window(&[], explicit, env).expect_err("nothing open");
        assert_eq!(err.code, ErrorCode::NoInstance, "{explicit:?} {env:?}");
    }
}

#[test]
fn one_window_is_picked_without_asking() {
    let found = select_window(&[entry(1, 100, false)], None, None).expect("one");
    assert_eq!(found.id, 1);
}

#[test]
fn several_windows_fail_and_list_themselves() {
    let entries = [entry(11, 100, false), entry(22, 200, false)];
    let err = select_window(&entries, None, None).expect_err("ambiguous");
    assert_eq!(err.code, ErrorCode::BadParams);
    assert!(err.message.contains("11") && err.message.contains("22"));
    assert!(err.message.contains("--window"), "{}", err.message);
}

#[test]
fn the_environment_selects_a_sessions_own_window() {
    let entries = [entry(11, 100, false), entry(22, 200, false)];
    let found = select_window(&entries, None, Some("22")).expect("from env");
    assert_eq!(found.id, 22);
    // An explicit target beats the environment.
    let found = select_window(&entries, Some(11), Some("22")).expect("explicit");
    assert_eq!(found.id, 11);
}

#[test]
fn a_named_window_that_is_gone_is_stale_not_some_other_window() {
    // Break caught: a session typing into the one window left open after its
    // own has gone, because it was the only choice.
    let entries = [entry(11, 100, false)];
    for (explicit, env) in [(Some(99), None), (None, Some("99"))] {
        let err = select_window(&entries, explicit, env).expect_err("stale");
        assert_eq!(err.code, ErrorCode::TargetStale, "{explicit:?} {env:?}");
        assert_eq!(err.target, Some(99));
    }
    let err = select_window(&entries, None, Some("not-a-number")).expect_err("bad");
    assert_eq!(err.code, ErrorCode::BadParams);
}

#[test]
fn the_quick_terminal_does_not_make_the_choice_ambiguous() {
    let entries = [entry(11, 100, false), entry(12, 100, true)];
    assert_eq!(select_window(&entries, None, None).expect("main").id, 11);
    // It can still be named, and is the answer when it is all there is.
    assert_eq!(
        select_window(&entries, Some(12), None).expect("named").id,
        12
    );
    let only = [entry(12, 100, true)];
    assert_eq!(select_window(&only, None, None).expect("only").id, 12);
}
