//! `rustty mcp` (AUTO-013, issue #59): MCP over stdio, one more encoder over the
//! same operations. Tools come from the window's capability list, so there is no
//! second list here, and everything a tool does goes through the dispatcher.
//!
//! The current MCP revision is [`MODERN_VERSION`], which has no handshake: each
//! request carries its version and capabilities in `_meta`. Clients on earlier
//! revisions open with `initialize` and expect a session. This server is what
//! the spec calls dual-era: how a client opens decides how it is served.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::client::{Connector, Target, Transport, WindowConnector};
use super::{ErrorCode, RpcError, capabilities};

/// The MCP revision this was built against, the current one on 2026-11-06.
pub const MODERN_VERSION: &str = "2026-07-28";
/// Handshake-based revisions still served, newest first.
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// Longest line read from a client. Requests to a window are far smaller.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// How a modern client asks for events on `subscriptions/listen`: the core
/// filter has no entry for them, so they are this server's own extension.
pub const EVENTS_EXTENSION: &str = "com.rustty/events";
/// The method events are delivered as on a modern stream.
pub const EVENT_NOTIFICATION: &str = "notifications/com.rustty/event";

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";
const META_SUBSCRIPTION: &str = "io.modelcontextprotocol/subscriptionId";
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const PARSE_ERROR: i64 = -32700;
const UNSUPPORTED_VERSION: i64 = -32022;
/// How long tool lists may be cached. They only change with a rustty upgrade.
const CACHE_TTL_MS: u64 = 3_600_000;
/// How often the loop looks for events while a client is subscribed.
const EVENT_POLL: Duration = Duration::from_millis(50);
/// The level events are logged at for handshake-era clients.
const EVENT_LOG_RANK: usize = 1;

const INSTRUCTIONS: &str = "Controls the rustty terminal this server runs under. `list` shows every \
session with a status: needs-you means it is waiting for a person. `tab_open` starts a session in \
the background; `pane_read` reads its screen or history; `pane_send-text` types into it, with \\r \
for Enter. Writes need the window's grant and answer denied-grant without it. A token reaches only \
the sessions opened with it, and can't answer a permission, trust or sign-in wait: when refused, tell \
the person.";

/// The tool name for an operation. MCP allows dots, but clients that hand tools
/// to a model API often don't, and one bad name can sink the whole list.
#[must_use]
pub fn tool_name(op: &str) -> String {
    op.replace('.', "_")
}

/// An error that goes back as a JSON-RPC error, not as a tool result.
struct ProtocolError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl ProtocolError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn frame(&self, id: Option<&Value>) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        let mut frame = json!({"jsonrpc": "2.0", "error": error});
        // A message we couldn't read has no id to echo, and null isn't a valid one.
        if let Some(id) = id {
            frame["id"] = id.clone();
        }
        frame
    }
}

fn supported_versions() -> Vec<&'static str> {
    let mut versions = vec![MODERN_VERSION];
    versions.extend_from_slice(LEGACY_VERSIONS);
    versions
}

fn server_info() -> Value {
    json!({"name": "rustty", "version": env!("CARGO_PKG_VERSION")})
}

fn log_rank(level: &str) -> usize {
    [
        "debug",
        "info",
        "notice",
        "warning",
        "error",
        "critical",
        "alert",
        "emergency",
    ]
    .iter()
    .position(|name| *name == level)
    .unwrap_or(0)
}

/// How a request is being served.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Era {
    /// Per-request `_meta`: results say they are complete.
    Modern,
    /// After an `initialize`.
    Legacy,
}

/// The adapter's state. A protocol-level session is the client's business; what
/// is kept here is the connection to the window, which comes and goes with it.
pub struct McpServer<C> {
    connector: C,
    session: Option<Box<dyn Transport>>,
    operations: Option<Vec<Value>>,
    legacy: Option<String>,
    subscribed: bool,
    stream: Option<Value>,
    stream_events: bool,
    log_rank: usize,
}

impl<C: Connector> McpServer<C> {
    /// With nothing connected yet.
    pub const fn new(connector: C) -> Self {
        Self {
            connector,
            session: None,
            operations: None,
            legacy: None,
            subscribed: false,
            stream: None,
            stream_events: false,
            log_rank: 0,
        }
    }

    /// The connector.
    pub const fn connector_mut(&mut self) -> &mut C {
        &mut self.connector
    }

    /// Whether the window may be sending events to pick up.
    #[must_use]
    pub const fn wants_events(&self) -> bool {
        self.subscribed
    }

    /// Handle one message from the client. `out` gets everything to send back.
    pub fn handle(&mut self, message: &Value, out: &mut dyn FnMut(Value)) {
        let Some(fields) = message.as_object() else {
            out(ProtocolError::new(INVALID_REQUEST, "batches are not supported").frame(None));
            return;
        };
        let method = fields.get("method").and_then(Value::as_str);
        let empty = json!({});
        let params = fields.get("params").unwrap_or(&empty);
        let Some(id) = fields.get("id") else {
            if let Some(method) = method {
                self.notification(method, params);
            }
            return;
        };
        if !id.is_string() && !id.is_number() {
            out(ProtocolError::new(INVALID_REQUEST, "the id must be text or a number").frame(None));
            return;
        }
        let Some(method) = method else {
            out(ProtocolError::new(INVALID_REQUEST, "the request has no method").frame(Some(id)));
            return;
        };
        match self.request(id, method, params, out) {
            Ok(Some(result)) => out(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Ok(None) => {}
            Err(error) => out(error.frame(Some(id))),
        }
    }

    /// Pick up events from the window and pass them on. A client that has opened
    /// no way to receive them doesn't get them: they are read and dropped, so
    /// they can't back up into a `slow-client` disconnect.
    pub fn pump_events(&mut self, out: &mut dyn FnMut(Value)) {
        if !self.subscribed {
            return;
        }
        loop {
            let Some(session) = &mut self.session else {
                return;
            };
            match session.next_event(Duration::ZERO) {
                Ok(Some(event)) => self.deliver(event, out),
                Ok(None) => return,
                Err(error) => {
                    let _ = writeln!(io::stderr(), "rustty mcp: {error}");
                    self.drop_session();
                    return;
                }
            }
        }
    }

    fn deliver(&self, event: Value, out: &mut dyn FnMut(Value)) {
        if let Some(stream) = self.stream.as_ref().filter(|_| self.stream_events) {
            let mut params = match event {
                Value::Object(fields) => fields,
                other => Map::from_iter([("data".to_string(), other)]),
            };
            params.insert("_meta".to_string(), json!({META_SUBSCRIPTION: stream}));
            out(json!({"jsonrpc": "2.0", "method": EVENT_NOTIFICATION, "params": params}));
        } else if self.legacy.is_some() && EVENT_LOG_RANK >= self.log_rank {
            out(json!({
                "jsonrpc": "2.0",
                "method": "notifications/message",
                "params": {"level": "info", "logger": "rustty", "data": event},
            }));
        }
    }

    fn notification(&mut self, method: &str, params: &Value) {
        if method == "notifications/cancelled"
            && self.stream.is_some()
            && params.get("requestId") == self.stream.as_ref()
        {
            self.stream = None;
            self.stream_events = false;
        }
    }

    fn era(&self, params: &Value) -> Result<Era, ProtocolError> {
        let meta = params.get("_meta");
        let Some(version) = meta.and_then(|meta| meta.get(META_VERSION)) else {
            if self.legacy.is_some() {
                return Ok(Era::Legacy);
            }
            return Err(ProtocolError::new(
                INVALID_PARAMS,
                format!(
                    "the request has no _meta[\"{META_VERSION}\"] and no initialize came first; \
                     this server supports {}",
                    supported_versions().join(", ")
                ),
            ));
        };
        let version = version.as_str().ok_or_else(|| {
            ProtocolError::new(INVALID_PARAMS, "the protocol version must be text")
        })?;
        if version == MODERN_VERSION {
            if meta.and_then(|meta| meta.get(META_CAPABILITIES)).is_none() {
                return Err(ProtocolError::new(
                    INVALID_PARAMS,
                    format!("the request has no _meta[\"{META_CAPABILITIES}\"]"),
                ));
            }
            return Ok(Era::Modern);
        }
        if self.legacy.as_deref() == Some(version) {
            return Ok(Era::Legacy);
        }
        Err(ProtocolError {
            code: UNSUPPORTED_VERSION,
            message: "Unsupported protocol version".to_string(),
            data: Some(json!({"supported": supported_versions(), "requested": version})),
        })
    }

    fn request(
        &mut self,
        id: &Value,
        method: &str,
        params: &Value,
        out: &mut dyn FnMut(Value),
    ) -> Result<Option<Value>, ProtocolError> {
        if method == "initialize" {
            return Ok(Some(self.initialize(params)));
        }
        // Handshake-era clients may ping at any time, even before initializing.
        if method == "ping" && params.get("_meta").is_none() {
            return Ok(Some(json!({})));
        }
        let era = self.era(params)?;
        let result = match method {
            "ping" => json!({}),
            "server/discover" => json!({
                "supportedVersions": supported_versions(),
                "capabilities": {"tools": {}, "extensions": {EVENTS_EXTENSION: {}}},
                "instructions": INSTRUCTIONS,
                "ttlMs": CACHE_TTL_MS,
                "cacheScope": "public",
            }),
            "tools/list" => {
                let mut result = json!({"tools": self.tools()});
                if era == Era::Modern {
                    result["ttlMs"] = json!(CACHE_TTL_MS);
                    result["cacheScope"] = json!("public");
                }
                result
            }
            "tools/call" => self.call_tool(params)?,
            "logging/setLevel" if era == Era::Legacy => {
                let level = params
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("debug");
                self.log_rank = log_rank(level);
                json!({})
            }
            "subscriptions/listen" if era == Era::Modern => {
                self.listen(id, params, out)?;
                return Ok(None);
            }
            other => {
                return Err(ProtocolError::new(
                    METHOD_NOT_FOUND,
                    format!("no method {other:?}"),
                ));
            }
        };
        Ok(Some(finish(era, result)))
    }

    /// An `initialize` selects handshake semantics. Their own version comes back
    /// when it is one served here, otherwise the newest that is.
    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .filter(|asked| LEGACY_VERSIONS.contains(asked))
            .unwrap_or(LEGACY_VERSIONS[0])
            .to_string();
        self.legacy = Some(version.clone());
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}, "logging": {}},
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn listen(
        &mut self,
        id: &Value,
        params: &Value,
        out: &mut dyn FnMut(Value),
    ) -> Result<(), ProtocolError> {
        let filter = params
            .get("notifications")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ProtocolError::new(INVALID_PARAMS, "listen needs a notifications filter")
            })?;
        if let Some(old) = self.stream.take() {
            out(json!({"jsonrpc": "2.0", "id": old, "result": finish_closed(&old)}));
        }
        // Only what this server will really send: tool lists never change.
        let events = filter.get(EVENTS_EXTENSION) == Some(&Value::Bool(true));
        let mut agreed = Map::new();
        if events {
            agreed.insert(EVENTS_EXTENSION.to_string(), json!(true));
        }
        self.stream = Some(id.clone());
        self.stream_events = events;
        out(json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": {"_meta": {META_SUBSCRIPTION: id}, "notifications": agreed},
        }));
        Ok(())
    }

    /// The window's operations, or this build's table when no window answers: a
    /// client lists tools at startup, often before any window exists.
    fn operation_list(&mut self) -> Vec<Value> {
        if let Some(operations) = &self.operations {
            return operations.clone();
        }
        let local = || {
            capabilities()
                .get("operations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        match self.window_call("capabilities", json!({})) {
            Ok(reply) => {
                let operations = reply
                    .get("operations")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(local);
                self.operations = Some(operations.clone());
                operations
            }
            Err(_) => local(),
        }
    }

    /// One tool per operation, except the handshake, which this adapter does itself.
    fn tools(&mut self) -> Vec<Value> {
        self.operation_list()
            .iter()
            .filter(|op| op.get("name").and_then(Value::as_str) != Some("initialize"))
            .map(|op| {
                let name = op.get("name").and_then(Value::as_str).unwrap_or_default();
                json!({
                    "name": tool_name(name),
                    "title": name,
                    "description": op.get("description").cloned().unwrap_or(Value::Null),
                    "inputSchema": op.get("params").cloned().unwrap_or_else(|| json!({"type": "object"})),
                    "annotations": {"readOnlyHint": op.get("grant").and_then(Value::as_str) == Some("none")},
                })
            })
            .collect()
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, ProtocolError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ProtocolError::new(INVALID_PARAMS, "tools/call needs a name"))?;
        let arguments = match params.get("arguments") {
            None => json!({}),
            Some(arguments) if arguments.is_object() => arguments.clone(),
            Some(_) => {
                return Err(ProtocolError::new(
                    INVALID_PARAMS,
                    "arguments must be an object",
                ));
            }
        };
        let operation = self
            .operation_list()
            .iter()
            .filter_map(|op| op.get("name").and_then(Value::as_str))
            .filter(|op| *op != "initialize")
            .find(|op| tool_name(op) == name)
            .map(str::to_string)
            .ok_or_else(|| ProtocolError::new(INVALID_PARAMS, format!("Unknown tool: {name}")))?;
        let outcome = self.window_call(&operation, arguments);
        if operation == "events.subscribe" && outcome.is_ok() {
            self.subscribed = true;
        }
        Ok(match outcome {
            Ok(result) => json!({
                "content": [{"type": "text", "text": result.to_string()}],
                "structuredContent": result,
                "isError": false,
            }),
            // A model can act on these, so they stay out of the protocol errors.
            Err(error) => json!({
                "content": [{"type": "text", "text": error.to_string()}],
                "structuredContent": {"error": error.to_value()},
                "isError": true,
            }),
        })
    }

    /// Call the window, connecting first if need be. A connection that can't be
    /// used any more is dropped, so the next call starts fresh. That also cancels
    /// a timed-out request still queued in the window.
    fn window_call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        if self.session.is_none() {
            self.session = Some(self.connector.connect()?);
        }
        let Some(session) = &mut self.session else {
            return Err(RpcError::new(ErrorCode::Internal, "no connection"));
        };
        let outcome = session.call(method, params);
        if let Err(error) = &outcome
            && connection_is_over(error)
        {
            self.drop_session();
        }
        outcome
    }

    fn drop_session(&mut self) {
        self.session = None;
        self.operations = None;
        self.subscribed = false;
    }
}

/// `target-closed` with a target is about that target; without one it is the
/// connection itself that has gone.
fn connection_is_over(error: &RpcError) -> bool {
    match error.code {
        ErrorCode::Timeout | ErrorCode::SlowClient => true,
        ErrorCode::TargetClosed => error.target.is_none(),
        _ => false,
    }
}

/// What every modern result carries.
fn finish(era: Era, mut result: Value) -> Value {
    if era == Era::Modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({META_SERVER: server_info()});
    }
    result
}

fn finish_closed(subscription: &Value) -> Value {
    json!({"resultType": "complete", "_meta": {META_SUBSCRIPTION: subscription}})
}

/// One line from the client.
enum Line {
    Text(Vec<u8>),
    TooLong,
}

/// Read one line, holding at most [`MAX_LINE_BYTES`] of it. `None` at end of input.
fn read_line(input: &mut impl BufRead) -> io::Result<Option<Line>> {
    let limit = MAX_LINE_BYTES as u64 + 1;
    let mut line = Vec::new();
    if input.by_ref().take(limit).read_until(b'\n', &mut line)? == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') || (line.len() as u64) < limit {
        return Ok(Some(Line::Text(line)));
    }
    // Over the limit: throw away the rest of it, in pieces.
    loop {
        line.clear();
        let read = input.by_ref().take(limit).read_until(b'\n', &mut line)?;
        if read == 0 || line.last() == Some(&b'\n') {
            return Ok(Some(Line::TooLong));
        }
    }
}

fn write_frames(output: &mut impl Write, frames: &[Value]) -> io::Result<()> {
    for frame in frames {
        // serde_json escapes newlines in strings, so a message is always one line.
        writeln!(output, "{frame}")?;
    }
    if frames.is_empty() {
        return Ok(());
    }
    output.flush()
}

/// Serve a client until its input ends: one JSON-RPC message per line. Reading
/// happens on its own thread, so events can go out while no request is arriving.
///
/// # Errors
///
/// The output failing, which means the client has gone.
pub fn run_stdio<C: Connector>(
    input: impl BufRead + Send + 'static,
    output: &mut impl Write,
    server: &mut McpServer<C>,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rustty-mcp-stdin".to_string())
        .spawn(move || {
            let mut input = input;
            while let Ok(Some(line)) = read_line(&mut input) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
    loop {
        let line = if server.wants_events() {
            match rx.recv_timeout(EVENT_POLL) {
                Ok(line) => Some(line),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match rx.recv() {
                Ok(line) => Some(line),
                Err(_) => return Ok(()),
            }
        };
        let mut frames = Vec::new();
        match line {
            Some(Line::TooLong) => {
                frames.push(
                    ProtocolError::new(INVALID_REQUEST, "the message is too long").frame(None),
                );
            }
            Some(Line::Text(text)) if !text.trim_ascii().is_empty() => {
                match serde_json::from_slice::<Value>(text.trim_ascii()) {
                    Ok(message) => server.handle(&message, &mut |frame| frames.push(frame)),
                    Err(_) => frames.push(
                        ProtocolError::new(PARSE_ERROR, "that line is not valid JSON").frame(None),
                    ),
                }
            }
            Some(Line::Text(_)) | None => {}
        }
        server.pump_events(&mut |frame| frames.push(frame));
        write_frames(output, &frames)?;
    }
}

/// The process entry: `rustty mcp`. Reads `RUSTTY_WINDOW` and
/// `RUSTTY_AUTOMATION_TOKEN` from the environment. Returns the exit code.
#[must_use]
pub fn main() -> u8 {
    let connector = WindowConnector::new(Target::from_env(None).with_env_token(), "rustty mcp");
    let mut server = McpServer::new(connector);
    let mut stdout = io::stdout();
    match run_stdio(BufReader::new(io::stdin()), &mut stdout, &mut server) {
        Ok(()) => 0,
        // The client hung up: that is how a session ends.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            let _ = writeln!(io::stderr(), "rustty mcp: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests;
