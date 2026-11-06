//! The server over a real named pipe: this test thread plays the window, pumping
//! requests into a dispatcher with a fake window behind it. Clients run on their
//! own threads. Each test has its own discovery folder, so nothing here can find
//! a real window.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::automation::client::Transport;
use crate::automation::ops::testkit::FakeWindow;
use crate::automation::ops::{Dispatcher, WindowOps};
use crate::automation::{ErrorCode, OUTBOUND_CAP, is_write_op, load_discovery};

const WAIT: Duration = Duration::from_secs(20);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustty-pipe-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

struct Rig {
    server: Server,
    dispatcher: Dispatcher,
    window: FakeWindow,
    dir: PathBuf,
    departed: Vec<ClientId>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Rig {
    fn new(tag: &str) -> Self {
        let dir = temp_dir(tag);
        let window = FakeWindow::new();
        let server = Server::start(window.window_id(), false, Some(dir.clone())).expect("server");
        Self {
            server,
            dispatcher: Dispatcher::new(),
            window,
            dir,
            departed: Vec::new(),
        }
    }

    fn entry(&self) -> DiscoveryEntry {
        let found = load_discovery(&self.dir, &alive);
        assert_eq!(found.len(), 1, "{found:?}");
        found[0].clone()
    }

    /// Housekeeping only, as the window's tick does before serving.
    fn housekeeping(&mut self) {
        self.server.reap();
        for gone in self.server.take_gone() {
            self.dispatcher.client_gone(gone);
            self.departed.push(gone);
        }
    }

    /// One tick's worth of serving.
    fn tick(&mut self) {
        self.housekeeping();
        while let Some((client, request)) = self.server.next_request(&|_| true) {
            if let Some(reply) = self
                .dispatcher
                .handle(&mut self.window, client, &request, 0)
            {
                self.server.send(client, &reply);
            }
        }
    }

    /// Serve until `done` says stop, or fail after a while.
    fn pump_until(&mut self, mut done: impl FnMut(&mut Self) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < WAIT, "timed out");
            self.tick();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Housekeep without serving until a client has departed.
    fn wait_departure(&mut self) -> ClientId {
        let start = Instant::now();
        while self.departed.is_empty() {
            assert!(start.elapsed() < WAIT, "nobody left");
            self.housekeeping();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.departed[0]
    }

    /// Wait for a request to show up without serving anything else.
    fn wait_request(&self, allow: &dyn Fn(&Value) -> bool) -> (ClientId, Value) {
        let start = Instant::now();
        loop {
            if let Some(found) = self.server.next_request(allow) {
                return found;
            }
            assert!(start.elapsed() < WAIT, "no request");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn hello(transport: &mut PipeTransport) -> Value {
    transport
        .call(
            "initialize",
            json!({"protocolVersion": 1, "clientName": "test"}),
        )
        .expect("initialize")
}

/// Run a client on its own thread while this thread serves it.
fn serve_client<T: Send + 'static>(
    rig: &mut Rig,
    client: impl FnOnce(PipeTransport) -> T + Send + 'static,
) -> T {
    let entry = rig.entry();
    let handle = std::thread::spawn(move || client(open(&entry).expect("connect")));
    rig.pump_until(|_| handle.is_finished());
    handle.join().expect("client thread")
}

/// A client with no reader thread: once it stops, nothing drains its pipe.
fn silent_client(entry: &DiscoveryEntry) -> crate::host::client::DupHandle {
    let mut pipe = open_named_pipe(&entry.pipe, 0).expect("open");
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "capabilities"});
    write_frame(&mut WriteEnd(&pipe), &request).expect("written");
    let _ = &mut pipe;
    pipe
}

/// Send frames totalling well over the cap, the way the window sends: returns
/// the longest any one send took.
fn overflow(rig: &Rig, client: ClientId) -> Duration {
    let page = "x".repeat(512 * 1024);
    let frame = json!({"jsonrpc": "2.0", "method": "event", "params": {"text": page}});
    let mut slowest = Duration::ZERO;
    for _ in 0..(OUTBOUND_CAP / page.len() + 64) {
        let one = Instant::now();
        rig.server.send(client, &frame);
        slowest = slowest.max(one.elapsed());
    }
    slowest
}

#[test]
fn a_client_finds_the_window_and_gets_answers() {
    let mut rig = Rig::new("basic");
    let window = rig.window.window_id();
    let (init, listed) = serve_client(&mut rig, |mut transport| {
        let init = hello(&mut transport);
        (init, transport.call("list", json!({})).expect("list"))
    });
    assert_eq!(init["window"], window);
    assert_eq!(listed["window"], window);
    assert_eq!(listed["tabs"].as_array().map(Vec::len), Some(1));
}

#[test]
fn errors_cross_the_pipe_with_their_code_and_target() {
    let mut rig = Rig::new("errors");
    let error = serve_client(&mut rig, |mut transport| {
        hello(&mut transport);
        transport
            .call("pane.read", json!({"pane": 424_242}))
            .expect_err("stale")
    });
    assert_eq!(error.code, ErrorCode::TargetStale);
    assert_eq!(error.target, Some(424_242));
}

#[test]
fn concurrent_clients_each_get_their_own_answers() {
    let mut rig = Rig::new("concurrent");
    let entry = rig.entry();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let entry = entry.clone();
            std::thread::spawn(move || {
                let mut transport = open(&entry).expect("connect");
                hello(&mut transport);
                for _ in 0..25 {
                    let listed = transport.call("list", json!({})).expect("list");
                    assert!(listed["tabs"].is_array());
                }
            })
        })
        .collect();
    rig.pump_until(|_| handles.iter().all(std::thread::JoinHandle::is_finished));
    for handle in handles {
        handle.join().expect("client");
    }
}

#[test]
fn a_client_vanishing_mid_request_costs_nobody_else_anything() {
    let mut rig = Rig::new("vanish");
    let entry = rig.entry();
    {
        let mut transport = open(&entry).expect("connect");
        transport
            .send_only("list", json!({}))
            .expect("request written");
    }
    rig.wait_departure();
    let listed = serve_client(&mut rig, |mut transport| {
        hello(&mut transport);
        transport.call("list", json!({})).expect("list")
    });
    assert!(listed["tabs"].is_array());
}

#[test]
fn requests_from_a_client_that_left_are_not_run() {
    // Break caught: a timed-out `send` typing into a shell minutes later, when a
    // dialog closes and the queue finally drains.
    let rig = Rig::new("cancelled");
    let entry = rig.entry();
    let mut transport = open(&entry).expect("connect");
    transport.send_only("list", json!({})).expect("written");
    let start = Instant::now();
    while !rig.server.has_queued() && start.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(rig.server.has_queued());
    drop(transport);
    while rig.server.has_queued() && start.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(rig.server.next_request(&|_| true).is_none());
}

#[test]
fn a_client_that_stops_reading_is_cut_off_and_stalls_only_itself() {
    // Acceptance (#56, #58): bounded memory with a slow reader, and the window
    // never waits on one.
    let mut rig = Rig::new("slow");
    let slow = silent_client(&rig.entry());
    let (client, _) = rig.wait_request(&|_| true);
    let started = Instant::now();
    let slowest = overflow(&rig, client);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the window waited on a client: {:?}",
        started.elapsed()
    );
    assert!(slowest < Duration::from_secs(1), "{slowest:?}");
    assert_eq!(rig.wait_departure(), client);
    // A well-behaved client carries on regardless.
    let listed = serve_client(&mut rig, |mut transport| {
        hello(&mut transport);
        transport.call("list", json!({})).expect("list")
    });
    assert!(listed["tabs"].is_array());
    drop(slow);
}

#[test]
fn a_client_that_reads_again_too_late_sees_the_stream_end() {
    let mut rig = Rig::new("slow-late");
    let slow = silent_client(&rig.entry());
    let (client, _) = rig.wait_request(&|_| true);
    overflow(&rig, client);
    rig.wait_departure();
    // What was already in the pipe may arrive, cut short; then it ends. Closing
    // the handle takes housekeeping, which the window's tick keeps doing.
    let reader =
        std::thread::spawn(
            move || {
                while read_frame(&mut ReadEnd(&slow), MAX_PAGE_BYTES).is_ok() {}
            },
        );
    let start = Instant::now();
    while !reader.is_finished() {
        assert!(start.elapsed() < WAIT, "the stream never ended");
        rig.housekeeping();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_reading_client_is_told_when_it_is_cut_off() {
    // Reaching the cap needs a client that isn't reading, which can't then be
    // told. One that is reading when the cut comes does get the error.
    let rig = Rig::new("slow-told");
    let mut transport = open(&rig.entry()).expect("connect");
    transport.send_only("list", json!({})).expect("written");
    let (client, _) = rig.wait_request(&|_| true);
    let error = RpcError::new(ErrorCode::SlowClient, "too slow");
    rig.server
        .client(client)
        .expect("client")
        .cut_off(encode_frame(&error_frame(Value::Null, &error)).ok());
    let told = transport.next_event(WAIT).expect_err("the stream ended");
    assert_eq!(told.code, ErrorCode::SlowClient);
}

#[test]
fn a_flooding_client_cant_starve_another() {
    let rig = Rig::new("flood");
    let entry = rig.entry();
    let mut flood = open(&entry).expect("connect");
    for _ in 0..200 {
        flood.send_only("list", json!({})).expect("written");
    }
    let (first, _) = rig.wait_request(&|_| true);
    let mut quiet = open(&entry).expect("connect");
    quiet.send_only("capabilities", json!({})).expect("written");
    let start = Instant::now();
    while rig.server.client_count() < 2 && start.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(100));
    let mut served = BTreeSet::from([first]);
    for _ in 0..3 {
        served.insert(rig.wait_request(&|_| true).0);
    }
    assert_eq!(served.len(), 2, "the quiet client is still waiting");
}

#[test]
fn a_client_over_its_inbox_cap_pushes_back_on_itself() {
    let rig = Rig::new("inbox");
    let entry = rig.entry();
    let mut flood = open(&entry).expect("connect");
    for _ in 0..200 {
        flood.send_only("list", json!({})).expect("written");
    }
    std::thread::sleep(Duration::from_millis(200));
    let queued = lock(&rig.server.shared.clients)
        .iter()
        .map(|client| lock(&client.inbox).len())
        .sum::<usize>();
    assert_eq!(queued, CLIENT_INBOX_CAP);
}

#[test]
fn the_allow_check_holds_a_clients_queue_in_order_and_lets_others_through() {
    // A dialog is up: writes wait, reads go on, and one client's requests never
    // run out of order.
    let rig = Rig::new("hold");
    let entry = rig.entry();
    let mut writer = open(&entry).expect("connect");
    writer.send_only("tab.open", json!({})).expect("written");
    writer.send_only("list", json!({})).expect("written");
    let mut reader = open(&entry).expect("connect");
    reader
        .send_only("capabilities", json!({}))
        .expect("written");
    let no_writes = |request: &Value| !is_write_op(request["method"].as_str().unwrap_or(""));
    let (_, got) = rig.wait_request(&no_writes);
    assert_eq!(got["method"], "capabilities", "the other client's read");
    std::thread::sleep(Duration::from_millis(50));
    assert!(rig.server.next_request(&no_writes).is_none(), "still held");
    let (_, first) = rig.wait_request(&|_| true);
    assert_eq!(first["method"], "tab.open");
    let (_, second) = rig.wait_request(&|_| true);
    assert_eq!(second["method"], "list");
}

#[test]
fn a_frame_over_the_cap_is_answered_and_the_connection_closed() {
    let rig = Rig::new("toolong");
    let mut transport = open(&rig.entry()).expect("connect");
    transport
        .write_raw(&(200_000u32).to_le_bytes())
        .expect("written");
    let error = transport.next_event(WAIT).expect_err("connection ended");
    assert_eq!(error.code, ErrorCode::BadParams, "{error}");
}

#[test]
fn malformed_json_is_answered_and_the_connection_closed() {
    let rig = Rig::new("badjson");
    let mut transport = open(&rig.entry()).expect("connect");
    let body = b"{ not json";
    transport
        .write_raw(&(body.len() as u32).to_le_bytes())
        .expect("written");
    transport.write_raw(body).expect("written");
    let error = transport.next_event(WAIT).expect_err("connection ended");
    assert_eq!(error.code, ErrorCode::BadParams, "{error}");
}

#[test]
fn the_discovery_file_lasts_as_long_as_the_server() {
    let rig = Rig::new("discovery");
    let entry = rig.entry();
    assert_eq!(entry.pid, std::process::id());
    assert!(
        entry.pipe.starts_with(r"\\.\pipe\rustty."),
        "{}",
        entry.pipe
    );
    let dir = rig.dir.clone();
    drop(rig);
    assert!(load_discovery(&dir, &|_| true).is_empty());
}

#[test]
fn a_reply_sent_just_before_shutdown_still_arrives() {
    // Break caught: closing the last tab destroying the window before the reply
    // to that very request is out, so the client reports a failure.
    let rig = Rig::new("flush");
    let mut transport = open(&rig.entry()).expect("connect");
    transport.send_only("list", json!({})).expect("written");
    let (client, _) = rig.wait_request(&|_| true);
    rig.server.send(
        client,
        &json!({"jsonrpc": "2.0", "id": 1, "result": {"closed": true}}),
    );
    drop(rig);
    let frame = transport
        .frames
        .recv_timeout(WAIT)
        .expect("the reply got out");
    assert_eq!(frame["result"]["closed"], true);
}

#[test]
fn a_taken_pipe_name_is_refused_rather_than_shared() {
    // Break caught: another process creating the pipe first and being connected
    // to by clients who then hand it their token.
    let mut security = PipeSecurity::for_current_user().expect("security");
    let name = format!(r"\\.\pipe\rustty.test-squat.{}", std::process::id());
    let first = create_instance(&name, &mut security, true).expect("first instance");
    assert!(create_instance(&name, &mut security, true).is_err());
    assert!(create_instance(&name, &mut security, false).is_ok());
    drop(first);
}

#[test]
fn a_client_wont_talk_to_a_pipe_run_by_the_wrong_process() {
    // Every field is true of the other process, except the pipe, which is ours.
    let rig = Rig::new("wrongpid");
    let mut other = std::process::Command::new("cmd.exe")
        .args(["/c", "pause"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("started");
    let mut entry = rig.entry();
    entry.pid = other.id();
    entry.id = u64::from(other.id()) << crate::automation::ID_PID_SHIFT;
    entry.start_time = process_start_time(other.id()).expect("its start time");
    assert_eq!(
        crate::automation::entry_problem(&entry, Some(entry.start_time)),
        None
    );
    let error = open(&entry).map(|_| ()).expect_err("refused");
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(error.code, ErrorCode::NoInstance, "{error}");
    assert!(error.message.contains("not the window"), "{error}");
}

#[test]
fn opening_refuses_an_entry_a_client_must_not_trust() {
    // `open` is the last check before a pipe is touched, whoever picked the entry.
    let rig = Rig::new("untrusted");
    let mut entry = rig.entry();
    entry.start_time = 0;
    assert_eq!(
        open(&entry).map(|_| ()).expect_err("refused").code,
        ErrorCode::NoInstance
    );
    let mut entry = rig.entry();
    entry.id = 1;
    assert_eq!(
        open(&entry).map(|_| ()).expect_err("refused").code,
        ErrorCode::NoInstance
    );
}

#[test]
fn a_pipe_that_is_not_there_is_no_instance() {
    let entry = DiscoveryEntry {
        id: crate::automation::next_id(),
        pipe: r"\\.\pipe\rustty.test-absent.0.0".to_string(),
        pid: std::process::id(),
        start_time: process_start_time(std::process::id()).expect("own start time"),
        quick: false,
        version: String::new(),
    };
    assert_eq!(
        open(&entry).map(|_| ()).expect_err("absent").code,
        ErrorCode::NoInstance
    );
}

#[test]
fn liveness_notices_a_dead_pid_and_a_reused_one() {
    let start_time = process_start_time(std::process::id()).expect("own start time");
    let mut entry = DiscoveryEntry {
        id: 1,
        pipe: String::new(),
        pid: std::process::id(),
        start_time,
        quick: false,
        version: String::new(),
    };
    assert!(alive(&entry));
    entry.start_time += 1;
    assert!(!alive(&entry), "same pid, different process");
    entry.pid = 0xFFFF_FFF0;
    entry.start_time = start_time;
    assert!(!alive(&entry), "no such process");
}

#[test]
fn window_tokens_are_random_and_never_repeat() {
    let a = new_token().expect("token");
    let b = new_token().expect("token");
    assert!(!a.matches(b.as_str()));
    assert_eq!(a.as_str().len(), 64);
}

#[test]
fn the_token_grants_writes_over_a_real_pipe_to_that_connection_only() {
    let token = new_token().expect("token");
    let secret = token.as_str().to_string();
    let mut rig = Rig::new("token");
    rig.window.token = Some(token);
    let entry = rig.entry();
    let handle = std::thread::spawn(move || {
        let mut with = open(&entry).expect("connect");
        let init = with
            .call("initialize", json!({"protocolVersion": 1, "token": secret}))
            .expect("initialize");
        assert_eq!(init["grants"]["writes"], true);
        with.call("tab.open", json!({})).expect("granted");

        let mut without = open(&entry).expect("connect");
        hello(&mut without);
        let error = without
            .call("tab.open", json!({}))
            .expect_err("not granted");
        assert_eq!(error.code, ErrorCode::DeniedGrant);
    });
    rig.pump_until(|_| handle.is_finished());
    handle.join().expect("client");
    assert_eq!(rig.window.opened.len(), 1);
}

/// What a window pays on its UI thread to start automation. Run with `--release`.
#[test]
#[ignore = "bench: run individually in release"]
fn starting_the_server_costs_the_window_little() {
    let dir = temp_dir("startcost");
    let mut slowest = Duration::ZERO;
    for _ in 0..20 {
        let start = Instant::now();
        let server =
            Server::start(crate::automation::next_id(), false, Some(dir.clone())).expect("server");
        slowest = slowest.max(start.elapsed());
        drop(server);
    }
    let _ = std::fs::remove_dir_all(&dir);
    println!("Server::start worst of 20: {slowest:?}");
    assert!(slowest < Duration::from_millis(50), "{slowest:?}");
}

#[test]
fn a_timed_out_call_says_timeout() {
    // Nothing serves this client: this thread isn't pumping.
    let rig = Rig::new("timeout");
    let mut transport = open(&rig.entry()).expect("connect");
    transport.set_timeout(Duration::from_millis(100));
    let error = transport.call("list", json!({})).expect_err("no answer");
    assert_eq!(error.code, ErrorCode::Timeout);
}

#[test]
fn a_call_on_a_closed_connection_says_target_closed() {
    let rig = Rig::new("closed");
    let mut transport = open(&rig.entry()).expect("connect");
    drop(rig);
    let start = Instant::now();
    let error = loop {
        match transport.next_event(Duration::from_millis(20)) {
            Ok(_) => assert!(start.elapsed() < WAIT),
            Err(error) => break error,
        }
    };
    assert_eq!(error.code, ErrorCode::TargetClosed);
}

#[test]
fn events_arrive_on_the_stream_with_their_fields() {
    let mut rig = Rig::new("events");
    let mut events = serve_client(&mut rig, |mut transport| {
        hello(&mut transport);
        transport
            .call("events.subscribe", json!({"events": ["tab-opened"]}))
            .expect("subscribe");
        transport
    });
    rig.window.app.add_tab_sized(80, 24, 100);
    let server = &rig.server;
    rig.dispatcher.scan(&rig.window.app, &mut |client, frame| {
        server.send(client, frame)
    });
    let event = events
        .next_event(WAIT)
        .expect("stream open")
        .expect("an event");
    assert_eq!(event["event"], "tab-opened");
    assert!(event["tab"].is_u64());
}
