//! The MCP adapter over stdio, against a fake window. Both eras are covered:
//! `2026-07-28` (per-request `_meta`) and the `initialize` handshake.

use std::collections::BTreeSet;
use std::io::Cursor;

use serde_json::{Value, json};

use super::*;
use crate::automation::client::fake::{FakeConnector, Shared, world};
use crate::automation::{ErrorCode, RpcError, capabilities};

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";

fn meta() -> Value {
    json!({META_VERSION: MODERN_VERSION, META_CAPS: {}})
}

fn server(shared: &Shared) -> McpServer<FakeConnector> {
    McpServer::new(FakeConnector::new(shared))
}

/// One message in, everything it caused out.
fn send(mcp: &mut McpServer<FakeConnector>, message: Value) -> Vec<Value> {
    let mut out = Vec::new();
    mcp.handle(&message, &mut |frame| out.push(frame));
    out
}

/// A request and its single reply.
fn ask(mcp: &mut McpServer<FakeConnector>, method: &str, params: Value) -> Value {
    let out = send(
        mcp,
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["id"], 1);
    assert_eq!(out[0]["jsonrpc"], "2.0");
    out[0].clone()
}

fn legacy(mcp: &mut McpServer<FakeConnector>) -> Value {
    ask(
        mcp,
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    )
}

fn call_tool(mcp: &mut McpServer<FakeConnector>, name: &str, arguments: Value) -> Value {
    let reply = ask(
        mcp,
        "tools/call",
        json!({"name": name, "arguments": arguments, "_meta": meta()}),
    );
    assert!(reply.get("error").is_none(), "{reply}");
    reply["result"].clone()
}

fn pane_of(shared: &Shared) -> u64 {
    shared.borrow().window.app.tabs()[0].pane().id()
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

// ---- modern era ---------------------------------------------------------

#[test]
fn discover_reports_versions_capabilities_and_identity() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(&mut mcp, "server/discover", json!({"_meta": meta()}));
    let result = &reply["result"];
    assert_eq!(result["resultType"], "complete");
    let versions = result["supportedVersions"].as_array().expect("versions");
    assert_eq!(versions[0], MODERN_VERSION);
    assert!(versions.iter().any(|v| v == "2025-11-25"));
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "rustty"
    );
    assert!(result["ttlMs"].is_number());
    assert!(result["cacheScope"].is_string());
}

#[test]
fn a_request_with_neither_meta_nor_a_handshake_is_invalid_params() {
    let shared = world();
    let mut mcp = server(&shared);
    for method in ["server/discover", "tools/list", "tools/call"] {
        let reply = ask(&mut mcp, method, json!({}));
        assert_eq!(reply["error"]["code"], -32602, "{method}");
        let message = reply["error"]["message"].as_str().expect("message");
        assert!(message.contains(MODERN_VERSION), "{message}");
    }
}

#[test]
fn missing_required_meta_fields_are_invalid_params() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(
        &mut mcp,
        "tools/list",
        json!({"_meta": {META_VERSION: MODERN_VERSION}}),
    );
    assert_eq!(reply["error"]["code"], -32602);
}

#[test]
fn an_unsupported_version_lists_what_is_supported() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(
        &mut mcp,
        "tools/list",
        json!({"_meta": {META_VERSION: "1900-01-01", META_CAPS: {}}}),
    );
    assert_eq!(reply["error"]["code"], -32022);
    assert_eq!(reply["error"]["data"]["requested"], "1900-01-01");
    let supported = reply["error"]["data"]["supported"]
        .as_array()
        .expect("supported");
    assert!(supported.iter().any(|v| v == MODERN_VERSION));
}

#[test]
fn modern_results_say_they_are_complete_and_can_be_cached() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(&mut mcp, "tools/list", json!({"_meta": meta()}));
    let result = &reply["result"];
    assert_eq!(result["resultType"], "complete");
    assert!(result["ttlMs"].is_number());
    assert!(result["cacheScope"].is_string());
    let called = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(called["resultType"], "complete");
}

// ---- legacy era ---------------------------------------------------------

#[test]
fn a_legacy_client_can_initialize_and_gets_its_version_back() {
    let shared = world();
    let mut mcp = server(&shared);
    for version in ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"] {
        let reply = ask(
            &mut mcp,
            "initialize",
            json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
        );
        assert_eq!(reply["result"]["protocolVersion"], version);
        assert_eq!(reply["result"]["serverInfo"]["name"], "rustty");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
    }
}

#[test]
fn an_unknown_legacy_version_is_answered_with_the_newest_legacy_one() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(
        &mut mcp,
        "initialize",
        json!({"protocolVersion": "2001-01-01", "capabilities": {}}),
    );
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn after_the_handshake_a_legacy_client_needs_no_meta_and_gets_plain_results() {
    let shared = world();
    let mut mcp = server(&shared);
    legacy(&mut mcp);
    assert!(
        send(
            &mut mcp,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_empty()
    );
    let reply = ask(&mut mcp, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().expect("tools");
    assert!(!tools.is_empty());
    assert!(reply["result"].get("resultType").is_none(), "{reply}");
    let reply = ask(
        &mut mcp,
        "tools/call",
        json!({"name": "list", "arguments": {}}),
    );
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert!(reply["result"].get("resultType").is_none());
    assert_eq!(ask(&mut mcp, "ping", json!({}))["result"], json!({}));
    assert_eq!(
        ask(&mut mcp, "logging/setLevel", json!({"level": "debug"}))["result"],
        json!({})
    );
}

#[test]
fn both_eras_can_share_a_process() {
    let shared = world();
    let mut mcp = server(&shared);
    legacy(&mut mcp);
    let reply = ask(&mut mcp, "tools/list", json!({"_meta": meta()}));
    assert_eq!(reply["result"]["resultType"], "complete");
}

// ---- tools --------------------------------------------------------------

fn tools(mcp: &mut McpServer<FakeConnector>) -> Vec<Value> {
    ask(mcp, "tools/list", json!({"_meta": meta()}))["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone()
}

#[test]
fn every_operation_but_the_handshake_is_a_tool_with_its_own_schema() {
    // Acceptance (#59): tools map 1:1 onto the operations.
    let shared = world();
    let mut mcp = server(&shared);
    let tools = tools(&mut mcp);
    let caps = capabilities();
    let ops: Vec<&Value> = caps["operations"]
        .as_array()
        .expect("ops")
        .iter()
        .filter(|op| op["name"] != "initialize")
        .collect();
    assert_eq!(tools.len(), ops.len());
    for (tool, op) in tools.iter().zip(&ops) {
        assert_eq!(tool["name"], tool_name(op["name"].as_str().expect("name")));
        assert_eq!(tool["inputSchema"], op["params"]);
        assert_eq!(tool["description"], op["description"]);
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            op["grant"] != "write",
            "{}",
            tool["name"]
        );
    }
}

#[test]
fn tool_names_are_legal_where_clients_are_strictest() {
    // Break caught: `pane.send-text` is legal MCP, but clients that hand tools to
    // a model API reject dots, and the whole server's tool list fails with it.
    let shared = world();
    let mut mcp = server(&shared);
    let names: Vec<String> = tools(&mut mcp)
        .iter()
        .map(|tool| tool["name"].as_str().expect("name").to_string())
        .collect();
    for name in &names {
        assert!(!name.is_empty() && name.len() <= 64, "{name}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{name}"
        );
    }
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
    assert!(names.contains(&"pane_send-text".to_string()));
}

#[test]
fn the_tool_list_comes_from_the_window_not_a_second_list() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(&mut mcp, "tools/list", json!({"_meta": meta()}));
    assert!(
        !reply["result"]["tools"]
            .as_array()
            .expect("tools")
            .is_empty()
    );
    shared.borrow_mut().calls.clear();
    let mut mcp = server(&shared);
    shared.borrow_mut().scripted.push_back(Ok(json!({
        "operations": [
            {"name": "initialize", "grant": "none", "description": "", "params": {"type": "object"}},
            {"name": "future.op", "grant": "write", "description": "New.", "params": {"type": "object"}},
        ],
    })));
    let tools = tools(&mut mcp);
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0]["name"], "future_op");
    assert_eq!(tools[0]["description"], "New.");
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], false);
}

#[test]
fn with_no_window_the_tools_still_list_from_this_builds_table() {
    // A client lists tools at startup, often before any window exists.
    let shared = world();
    let mut connector = FakeConnector::new(&shared);
    connector.refuse = Some(RpcError::new(ErrorCode::NoInstance, "none"));
    let mut mcp = McpServer::new(connector);
    assert!(!tools_of(&mut mcp).is_empty());
}

fn tools_of(mcp: &mut McpServer<FakeConnector>) -> Vec<Value> {
    tools(mcp)
}

#[test]
fn a_tool_call_runs_the_operation_and_returns_text_and_structure() {
    let shared = world();
    let mut mcp = server(&shared);
    let result = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().expect("text");
    assert_eq!(result["content"][0]["type"], "text");
    let parsed: Value = serde_json::from_str(text).expect("text is the JSON result");
    assert_eq!(parsed, result["structuredContent"]);
    assert!(parsed["tabs"].is_array());
}

#[test]
fn a_dotted_operation_is_called_through_its_tool_name() {
    let shared = world();
    shared.borrow_mut().window.allow_writes = true;
    let pane = pane_of(&shared);
    let mut mcp = server(&shared);
    let result = call_tool(
        &mut mcp,
        "pane_send-text",
        json!({"pane": pane, "text": "hello\r"}),
    );
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        shared.borrow().window.sent,
        vec![(pane, b"hello\r".to_vec())]
    );
}

#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let shared = world();
    let mut mcp = server(&shared);
    for name in ["no_such_tool", "initialize"] {
        let reply = ask(
            &mut mcp,
            "tools/call",
            json!({"name": name, "arguments": {}, "_meta": meta()}),
        );
        assert_eq!(reply["error"]["code"], -32602, "{name}");
    }
    let reply = ask(&mut mcp, "tools/call", json!({"_meta": meta()}));
    assert_eq!(reply["error"]["code"], -32602);
    let reply = ask(
        &mut mcp,
        "tools/call",
        json!({"name": "list", "arguments": [1], "_meta": meta()}),
    );
    assert_eq!(reply["error"]["code"], -32602);
}

#[test]
fn operation_failures_are_tool_errors_a_model_can_read() {
    // A model can act on these, so they aren't protocol errors.
    let shared = world();
    let pane = pane_of(&shared);
    let mut mcp = server(&shared);
    let denied = call_tool(
        &mut mcp,
        "pane_send-text",
        json!({"pane": pane, "text": "x"}),
    );
    assert_eq!(denied["isError"], true);
    assert_eq!(denied["structuredContent"]["error"]["code"], "denied-grant");
    let text = denied["content"][0]["text"].as_str().expect("text");
    assert!(text.starts_with("denied-grant: "), "{text}");
    assert!(shared.borrow().window.sent.is_empty());

    let stale = call_tool(&mut mcp, "pane_read", json!({"pane": 987_654}));
    assert_eq!(stale["isError"], true);
    assert_eq!(stale["structuredContent"]["error"]["code"], "target-stale");
    assert_eq!(stale["structuredContent"]["error"]["target"], 987_654);
}

#[test]
fn rate_limited_tells_the_model_how_long_to_wait() {
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "list", json!({}));
    shared
        .borrow_mut()
        .scripted
        .push_back(Err(RpcError::rate_limited(120)));
    let limited = call_tool(&mut mcp, "pane_send-text", json!({"pane": 1, "text": "x"}));
    assert_eq!(limited["isError"], true);
    assert_eq!(limited["structuredContent"]["error"]["retryAfterMs"], 120);
}

#[test]
fn no_window_is_a_tool_error_and_a_later_window_is_found() {
    let shared = world();
    let mut connector = FakeConnector::new(&shared);
    connector.refuse = Some(RpcError::new(ErrorCode::NoInstance, "no window"));
    let mut mcp = McpServer::new(connector);
    let result = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["error"]["code"], "no-instance");
    mcp.connector_mut().refuse = None;
    let result = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(result["isError"], false, "{result}");
}

#[test]
fn a_connection_that_ended_is_replaced_on_the_next_call() {
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "list", json!({}));
    assert_eq!(mcp.connector_mut().connects, 1);
    shared.borrow_mut().scripted.push_back(Err(RpcError::new(
        ErrorCode::TargetClosed,
        "the window closed the connection",
    )));
    let result = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(result["isError"], true);
    let result = call_tool(&mut mcp, "list", json!({}));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(mcp.connector_mut().connects, 2);
}

#[test]
fn a_timeout_hangs_up_so_the_request_cant_run_later() {
    // Break caught: a timed-out send still queued, typing into a shell after
    // the model has already moved on.
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "list", json!({}));
    shared
        .borrow_mut()
        .scripted
        .push_back(Err(RpcError::new(ErrorCode::Timeout, "no answer")));
    call_tool(&mut mcp, "list", json!({}));
    call_tool(&mut mcp, "list", json!({}));
    assert_eq!(mcp.connector_mut().connects, 2);
}

#[test]
fn a_shell_that_exited_keeps_the_connection() {
    // target-closed about one pane says nothing about the connection.
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "list", json!({}));
    shared
        .borrow_mut()
        .scripted
        .push_back(Err(RpcError::closed(5, "its shell has exited")));
    call_tool(&mut mcp, "pane_send-text", json!({"pane": 5, "text": "x"}));
    call_tool(&mut mcp, "list", json!({}));
    assert_eq!(mcp.connector_mut().connects, 1);
}

// ---- events -------------------------------------------------------------

#[test]
fn a_legacy_client_gets_events_as_log_notifications_once_subscribed() {
    let shared = world();
    let mut mcp = server(&shared);
    legacy(&mut mcp);
    assert!(!mcp.wants_events());
    let reply = ask(
        &mut mcp,
        "tools/call",
        json!({"name": "events_subscribe", "arguments": {}}),
    );
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert!(mcp.wants_events());
    feed(&shared, b"\x07");
    let mut out = Vec::new();
    mcp.pump_events(&mut |frame| out.push(frame));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["method"], "notifications/message");
    assert_eq!(out[0]["params"]["logger"], "rustty");
    assert_eq!(out[0]["params"]["data"]["event"], "bell");
    assert!(out[0].get("id").is_none());
}

#[test]
fn a_legacy_clients_log_level_can_silence_events() {
    let shared = world();
    let mut mcp = server(&shared);
    legacy(&mut mcp);
    ask(&mut mcp, "logging/setLevel", json!({"level": "warning"}));
    ask(
        &mut mcp,
        "tools/call",
        json!({"name": "events_subscribe", "arguments": {}}),
    );
    feed(&shared, b"\x07");
    let mut out = Vec::new();
    mcp.pump_events(&mut |frame| out.push(frame));
    assert!(out.is_empty(), "{out:?}");
}

fn listen(mcp: &mut McpServer<FakeConnector>, id: u64) -> Vec<Value> {
    send(
        mcp,
        json!({"jsonrpc": "2.0", "id": id, "method": "subscriptions/listen", "params": {
            "_meta": meta(),
            "notifications": {"toolsListChanged": true, "com.rustty/events": true},
        }}),
    )
}

#[test]
fn a_modern_listen_is_acknowledged_with_what_was_agreed() {
    let shared = world();
    let mut mcp = server(&shared);
    let out = listen(&mut mcp, 7);
    assert_eq!(out.len(), 1, "the stream stays open: {out:?}");
    assert_eq!(out[0]["method"], "notifications/subscriptions/acknowledged");
    let params = &out[0]["params"];
    assert_eq!(params["_meta"]["io.modelcontextprotocol/subscriptionId"], 7);
    assert_eq!(params["notifications"]["com.rustty/events"], true);
    assert!(
        params["notifications"].get("toolsListChanged").is_none(),
        "this server never sends it, so it must not agree to"
    );
}

#[test]
fn a_modern_client_gets_events_only_on_a_stream_it_opened() {
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "events_subscribe", json!({}));
    feed(&shared, b"\x07");
    let mut out = Vec::new();
    mcp.pump_events(&mut |frame| out.push(frame));
    assert!(out.is_empty(), "nothing it asked for: {out:?}");

    listen(&mut mcp, 7);
    {
        let mut world = shared.borrow_mut();
        world.window.app.take_bells();
    }
    feed(&shared, b"\x07");
    let mut out = Vec::new();
    mcp.pump_events(&mut |frame| out.push(frame));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["method"], EVENT_NOTIFICATION);
    assert_eq!(
        out[0]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    assert_eq!(out[0]["params"]["event"], "bell");
}

#[test]
fn cancelling_a_stream_stops_it_without_a_reply() {
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "events_subscribe", json!({}));
    listen(&mut mcp, 7);
    let out = send(
        &mut mcp,
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}),
    );
    assert!(out.is_empty(), "{out:?}");
    feed(&shared, b"\x07");
    let mut out = Vec::new();
    mcp.pump_events(&mut |frame| out.push(frame));
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn a_second_listen_closes_the_first_with_a_completion() {
    let shared = world();
    let mut mcp = server(&shared);
    listen(&mut mcp, 7);
    let out = listen(&mut mcp, 8);
    let closed = out.iter().find(|f| f["id"] == 7).expect("first closed");
    assert_eq!(closed["result"]["resultType"], "complete");
    assert_eq!(
        closed["result"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    assert!(out.iter().any(
        |f| f["method"] == "notifications/subscriptions/acknowledged"
            && f["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"] == 8
    ));
}

#[test]
fn losing_the_connection_ends_the_subscription() {
    let shared = world();
    let mut mcp = server(&shared);
    call_tool(&mut mcp, "events_subscribe", json!({}));
    assert!(mcp.wants_events());
    shared
        .borrow_mut()
        .scripted
        .push_back(Err(RpcError::new(ErrorCode::TargetClosed, "ended")));
    call_tool(&mut mcp, "list", json!({}));
    assert!(!mcp.wants_events(), "a new connection has no subscription");
}

// ---- framing, over the stdio loop ---------------------------------------

fn lines(messages: &[Value]) -> Vec<u8> {
    let mut input = Vec::new();
    for message in messages {
        input.extend_from_slice(message.to_string().as_bytes());
        input.push(b'\n');
    }
    input
}

/// Run the real stdio loop over in-memory streams until the input ends.
fn stdio(shared: &Shared, input: Vec<u8>) -> Vec<Value> {
    let mut output = Vec::new();
    let mut mcp = server(shared);
    run_stdio(Cursor::new(input), &mut output, &mut mcp).expect("clean end of input");
    let text = String::from_utf8(output).expect("utf8");
    assert!(text.is_empty() || text.ends_with('\n'), "{text:?}");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON message"))
        .collect()
}

#[test]
fn a_whole_session_over_stdio_lists_and_calls_tools() {
    let shared = world();
    shared.borrow_mut().window.allow_writes = true;
    let pane = pane_of(&shared);
    feed(&shared, "héllo\r\n".as_bytes());
    let out = stdio(
        &shared,
        lines(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "pane_read", "arguments": {"pane": pane}}}),
            json!({"jsonrpc": "2.0", "id": "four", "method": "tools/call", "params": {"name": "tab_open", "arguments": {"name": "worker"}}}),
        ]),
    );
    let ids: Vec<Value> = out.iter().map(|m| m["id"].clone()).collect();
    assert_eq!(ids, [json!(1), json!(2), json!(3), json!("four")]);
    assert_eq!(out[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(out[1]["result"]["tools"].as_array().expect("tools").len() > 5);
    let text = out[2]["result"]["structuredContent"]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains("héllo"), "{text}");
    assert!(out[3]["result"]["structuredContent"]["pane"].is_u64());
    assert_eq!(
        shared.borrow().window.opened[0].name.as_deref(),
        Some("worker")
    );
}

#[test]
fn output_never_holds_an_embedded_newline() {
    // The framing is one message per line, and terminal text is full of newlines.
    let shared = world();
    let pane = pane_of(&shared);
    feed(&shared, b"one\r\ntwo\r\nthree\r\n");
    let out = stdio(
        &shared,
        lines(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "pane_read", "arguments": {"pane": pane}, "_meta": meta(),
            }}),
        ]),
    );
    assert_eq!(out.len(), 1, "{out:?}");
}

#[test]
fn a_line_that_is_not_json_is_a_parse_error_and_the_session_goes_on() {
    let shared = world();
    let mut input = b"{ not json\n".to_vec();
    input.extend(lines(&[
        json!({"jsonrpc": "2.0", "id": 2, "method": "server/discover", "params": {"_meta": meta()}}),
    ]));
    let out = stdio(&shared, input);
    assert_eq!(out[0]["error"]["code"], -32700);
    assert!(out[0].get("id").is_none());
    assert_eq!(out[1]["id"], 2);
}

#[test]
fn a_batch_or_a_null_id_is_an_invalid_request() {
    let shared = world();
    let mut mcp = server(&shared);
    let out = send(
        &mut mcp,
        json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]),
    );
    assert_eq!(out[0]["error"]["code"], -32600);
    let out = send(
        &mut mcp,
        json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
    );
    assert_eq!(out[0]["error"]["code"], -32600);
    let out = send(&mut mcp, json!({"jsonrpc": "2.0", "id": 1}));
    assert_eq!(out[0]["error"]["code"], -32600);
}

#[test]
fn an_oversized_line_is_refused_and_the_next_one_still_works() {
    // Break caught: reading an endless line into memory.
    let shared = world();
    let mut input = vec![b'x'; MAX_LINE_BYTES + 100];
    input.push(b'\n');
    input.extend(lines(&[
        json!({"jsonrpc": "2.0", "id": 2, "method": "server/discover", "params": {"_meta": meta()}}),
    ]));
    let out = stdio(&shared, input);
    assert_eq!(out[0]["error"]["code"], -32600, "{out:?}");
    assert_eq!(out[1]["id"], 2);
}

#[test]
fn an_unknown_method_is_method_not_found_and_an_unknown_notification_is_ignored() {
    let shared = world();
    let mut mcp = server(&shared);
    let reply = ask(&mut mcp, "resources/list", json!({"_meta": meta()}));
    assert_eq!(reply["error"]["code"], -32601);
    assert!(
        send(
            &mut mcp,
            json!({"jsonrpc": "2.0", "method": "notifications/whatever"})
        )
        .is_empty()
    );
}

#[test]
fn a_blank_line_is_skipped_and_end_of_input_ends_cleanly() {
    let shared = world();
    assert!(stdio(&shared, b"\n\r\n".to_vec()).is_empty());
    assert!(stdio(&shared, Vec::new()).is_empty());
}

#[test]
fn nothing_about_a_token_reaches_the_client() {
    // The token only ever goes to the window, in the connector's initialize.
    let shared = world();
    shared.borrow_mut().window.token = Some(crate::automation::Token::from_random_bytes(&[9; 32]));
    let secret = shared
        .borrow()
        .window
        .token
        .as_ref()
        .map(|t| t.as_str().to_string())
        .expect("token");
    let mut mcp = server(&shared);
    let listed = ask(&mut mcp, "tools/list", json!({"_meta": meta()})).to_string();
    assert!(!listed.contains(&secret));
    assert!(!listed.contains("\"token\""), "only initialize takes one");
}
