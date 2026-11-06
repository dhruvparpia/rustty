//! `rustty ctl <verb>` (AUTO-012, issue #57): control a running window from a
//! script. It only ever connects to a window that already exists: nothing here
//! opens one or starts a process. Every verb is one dispatcher operation, so this
//! file only parses arguments and prints.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{Connector, Target, Transport, WindowConnector};
use super::ops::SpawnRequest;
use super::{ErrorCode, RpcError, SEND_BURST_BYTES};

/// The grammar, for `--help` and usage errors.
pub const USAGE: &str = "\
usage: rustty ctl [--window <id>] [--json] <verb>
  list                                 tabs and panes: ids, status, title, cwd
  capabilities                         operations, events, errors and limits
  open [--dir <d>] [--profile <p>] [--name <n>] [--env K=V]... [-- <argv>...]
  close <tab>
  focus <tab> | focus --pane <pane>
  split <pane> [--direction right|down] [--dir <d>] [--profile <p>] [--env K=V]... [-- <argv>...]
  read <pane> [--scrollback [--cursor <c>] [--all]]
  send <pane> [--enter] <text>... | send <pane> [--enter] --stdin
  watch [--pane <pane>] [--events <a,b>]
With no --window and several windows open it fails and lists them, unless
RUSTTY_WINDOW names one. Exit codes: 0 ok, 1 internal, 2 usage/bad-params,
3 target-stale, 4 denied-grant, 5 no-instance, 6 protocol-mismatch,
8 target-closed, 9 timeout, 10 slow-client, 11 rate-limited.";

/// What was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    /// Print the grammar.
    Help,
    /// `list`.
    List,
    /// `capabilities`.
    Capabilities,
    /// `open`.
    Open(SpawnRequest),
    /// `close <tab>`.
    Close {
        /// Tab id.
        tab: u64,
    },
    /// `focus <tab>`.
    FocusTab {
        /// Tab id.
        tab: u64,
    },
    /// `focus --pane <pane>`.
    FocusPane {
        /// Pane id.
        pane: u64,
    },
    /// `split <pane>`.
    Split {
        /// Pane id.
        pane: u64,
        /// `right` or `down`.
        direction: Option<String>,
        /// The session to start.
        request: SpawnRequest,
    },
    /// `read <pane>`.
    Read {
        /// Pane id.
        pane: u64,
        /// Read history instead of the screen.
        scrollback: bool,
        /// Where to continue.
        cursor: Option<String>,
        /// Follow every page to the end.
        all: bool,
    },
    /// `send <pane>`.
    Send {
        /// Pane id.
        pane: u64,
        /// The text, or `None` to read it from stdin.
        text: Option<String>,
        /// Press Enter afterwards.
        enter: bool,
    },
    /// `watch`.
    Watch {
        /// Only this pane.
        pane: Option<u64>,
        /// Only these events.
        events: Option<Vec<String>>,
    },
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `--window <id>`.
    pub window: Option<u64>,
    /// `--json`.
    pub json: bool,
    /// The verb.
    pub verb: Verb,
}

fn parse_id(text: &str, what: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{what} must be a number, not {text:?}"))
}

/// Pulls flags out of the arguments, leaving positionals. Everything after `--`
/// belongs to the command being run, flags included.
struct Args {
    before: Vec<String>,
    after: Option<Vec<String>>,
}

impl Args {
    fn new(args: &[String]) -> Self {
        match args.iter().position(|arg| arg == "--") {
            Some(at) => Self {
                before: args[..at].to_vec(),
                after: Some(args[at + 1..].to_vec()),
            },
            None => Self {
                before: args.to_vec(),
                after: None,
            },
        }
    }

    fn flag(&mut self, name: &str) -> bool {
        match self.before.iter().position(|arg| arg == name) {
            Some(at) => {
                self.before.remove(at);
                true
            }
            None => false,
        }
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(at) = self.before.iter().position(|arg| arg == name) else {
            return Ok(None);
        };
        if at + 1 >= self.before.len() {
            return Err(format!("{name} needs a value"));
        }
        let value = self.before.remove(at + 1);
        self.before.remove(at);
        Ok(Some(value))
    }

    /// Whatever is left, refusing anything that still looks like a flag.
    fn positionals(&mut self) -> Result<Vec<String>, String> {
        if let Some(flag) = self.before.iter().find(|arg| arg.starts_with('-')) {
            return Err(format!("unknown option {flag}"));
        }
        Ok(std::mem::take(&mut self.before))
    }

    fn spawn_request(&mut self) -> Result<SpawnRequest, String> {
        let mut env = BTreeMap::new();
        while let Some(pair) = self.value("--env")? {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("--env needs NAME=VALUE, not {pair:?}"))?;
            env.insert(key.to_string(), value.to_string());
        }
        let command = self.after.take();
        if command.as_ref().is_some_and(Vec::is_empty) {
            return Err("nothing to run after --".to_string());
        }
        Ok(SpawnRequest {
            dir: self.value("--dir")?,
            profile: self.value("--profile")?,
            name: None,
            command,
            env,
        })
    }
}

fn one_id(args: &mut Args, what: &str) -> Result<u64, String> {
    let positionals = args.positionals()?;
    match positionals.as_slice() {
        [id] => parse_id(id, what),
        [] => Err(format!("this needs a {what} id")),
        [_, extra, ..] => Err(format!("unexpected argument {extra:?}")),
    }
}

/// Parse the arguments after `rustty ctl`.
///
/// # Errors
///
/// A one-line description of what is wrong.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut args = Args::new(args);
    let window = args
        .value("--window")?
        .map(|id| parse_id(&id, "--window"))
        .transpose()?;
    let json = args.flag("--json");
    if args.flag("--help") || args.flag("-h") {
        return Ok(Invocation {
            window,
            json,
            verb: Verb::Help,
        });
    }
    if args.before.is_empty() {
        return Err("no verb given; see rustty ctl --help".to_string());
    }
    let name = args.before.remove(0);
    let verb = match name.as_str() {
        "help" => Verb::Help,
        "list" | "capabilities" => {
            let positionals = args.positionals()?;
            if let Some(extra) = positionals.first() {
                return Err(format!("unexpected argument {extra:?}"));
            }
            if name == "list" {
                Verb::List
            } else {
                Verb::Capabilities
            }
        }
        "open" => {
            let name = args.value("--name")?;
            let mut request = args.spawn_request()?;
            request.name = name;
            if let Some(extra) = args.positionals()?.first() {
                return Err(format!(
                    "unexpected argument {extra:?}; put a command after --"
                ));
            }
            Verb::Open(request)
        }
        "close" => Verb::Close {
            tab: one_id(&mut args, "tab")?,
        },
        "focus" => match args.value("--pane")? {
            Some(pane) => Verb::FocusPane {
                pane: parse_id(&pane, "pane")?,
            },
            None => Verb::FocusTab {
                tab: one_id(&mut args, "tab")?,
            },
        },
        "split" => {
            let direction = args.value("--direction")?;
            let request = args.spawn_request()?;
            Verb::Split {
                pane: one_id(&mut args, "pane")?,
                direction,
                request,
            }
        }
        "read" => {
            let scrollback = args.flag("--scrollback");
            let all = args.flag("--all");
            let cursor = args.value("--cursor")?;
            Verb::Read {
                pane: one_id(&mut args, "pane")?,
                scrollback,
                cursor,
                all,
            }
        }
        "send" => parse_send(&mut args)?,
        "watch" => {
            let pane = args
                .value("--pane")?
                .map(|id| parse_id(&id, "pane"))
                .transpose()?;
            let events = args
                .value("--events")?
                .map(|list| list.split(',').map(str::to_string).collect());
            if let Some(extra) = args.positionals()?.first() {
                return Err(format!("unexpected argument {extra:?}"));
            }
            Verb::Watch { pane, events }
        }
        other => return Err(format!("unknown verb {other:?}; see rustty ctl --help")),
    };
    Ok(Invocation { window, json, verb })
}

fn parse_send(args: &mut Args) -> Result<Verb, String> {
    let enter = args.flag("--enter");
    let stdin = args.flag("--stdin");
    let mut positionals = args.positionals()?;
    if positionals.is_empty() {
        return Err("send needs a pane id".to_string());
    }
    let pane = parse_id(&positionals.remove(0), "pane")?;
    if let Some(after) = args.after.take() {
        positionals.extend(after);
    }
    let text = match (stdin, positionals.is_empty()) {
        (true, true) => None,
        (false, false) => Some(positionals.join(" ")),
        (true, false) => return Err("send takes text or --stdin, not both".to_string()),
        (false, true) if enter => Some(String::new()),
        (false, true) => return Err("send needs text, --enter or --stdin".to_string()),
    };
    Ok(Verb::Send { pane, text, enter })
}

/// Cut text into pieces of at most `limit` bytes, never inside a character. A
/// character wider than `limit` goes out whole, so this always finishes.
#[must_use]
pub fn split_text(text: &str, limit: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = limit.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
}

/// Longest a `send` keeps retrying a single rate-limited chunk.
const SEND_RETRY_LIMIT: Duration = Duration::from_secs(30);

/// A failure, with what had already happened if that matters to a retry.
struct Failure {
    error: RpcError,
    note: Option<String>,
}

impl From<RpcError> for Failure {
    fn from(error: RpcError) -> Self {
        Self { error, note: None }
    }
}

fn send_all(transport: &mut dyn Transport, pane: u64, text: &str) -> Result<usize, Failure> {
    let mut sent = 0usize;
    let limit = usize::try_from(SEND_BURST_BYTES).unwrap_or(usize::MAX);
    for chunk in split_text(text, limit) {
        let started = Instant::now();
        loop {
            match transport.call("pane.send-text", json!({"pane": pane, "text": chunk})) {
                Ok(_) => {
                    sent += chunk.len();
                    break;
                }
                Err(error)
                    if error.code == ErrorCode::RateLimited
                        && started.elapsed() < SEND_RETRY_LIMIT =>
                {
                    let wait = error.retry_after_ms.unwrap_or(10).max(1);
                    std::thread::sleep(Duration::from_millis(wait));
                }
                Err(error) => {
                    // Saying so stops a script retrying and typing it all twice.
                    let note = (sent > 0).then(|| format!(" ({sent} bytes were already sent)"));
                    return Err(Failure { error, note });
                }
            }
        }
    }
    Ok(sent)
}

fn io_failure(e: &io::Error) -> Failure {
    Failure::from(RpcError::new(
        ErrorCode::Internal,
        format!("could not write output: {e}"),
    ))
}

fn print_json(out: &mut dyn Write, value: &Value) -> Result<(), Failure> {
    writeln!(out, "{value}").map_err(|e| io_failure(&e))
}

fn text_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn print_list(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    writeln!(
        out,
        "{:<18} {:<18} {:<10} {:<28} DETAIL",
        "TAB", "PANE", "STATUS", "TITLE"
    )?;
    for tab in result["tabs"].as_array().into_iter().flatten() {
        for pane in tab["panes"].as_array().into_iter().flatten() {
            let mark = if tab["active"] == true && pane["active"] == true {
                "*"
            } else {
                ""
            };
            writeln!(
                out,
                "{:<18} {:<18} {:<10} {:<28} {}",
                tab["id"].to_string(),
                format!("{}{mark}", pane["id"]),
                text_of(pane, "status"),
                text_of(pane, "title"),
                text_of(pane, "detail"),
            )?;
        }
    }
    Ok(())
}

fn print_capabilities(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    for op in result["operations"].as_array().into_iter().flatten() {
        writeln!(
            out,
            "{:<20} {:<6} {}",
            text_of(op, "name"),
            text_of(op, "grant"),
            text_of(op, "description")
        )?;
    }
    Ok(())
}

/// Print one event: its JSON in `--json` mode, otherwise a line a person can
/// read, with pane output as the plain text it is.
///
/// # Errors
///
/// The writer's errors.
pub fn print_event(event: &Value, json_mode: bool, out: &mut dyn Write) -> io::Result<()> {
    if json_mode {
        writeln!(out, "{event}")?;
    } else if event["event"] == "pane-output" {
        out.write_all(text_of(event, "text").as_bytes())?;
    } else {
        let mut line = text_of(event, "event").to_string();
        if let Some(fields) = event.as_object() {
            for (key, value) in fields {
                if key != "event" && key != "seq" && !value.is_null() {
                    let shown = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_string);
                    line.push_str(&format!(" {key}={shown}"));
                }
            }
        }
        writeln!(out, "{line}")?;
    }
    out.flush()
}

fn read_verb(
    transport: &mut dyn Transport,
    pane: u64,
    scrollback: bool,
    cursor: Option<String>,
    all: bool,
    json_mode: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut cursor = cursor;
    loop {
        let mut params = json!({"pane": pane});
        if scrollback {
            params["scrollback"] = json!(true);
        }
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let page = transport.call("pane.read", params)?;
        if json_mode {
            print_json(out, &page)?;
        } else {
            out.write_all(text_of(&page, "text").as_bytes())
                .map_err(|e| io_failure(&e))?;
        }
        if !(all && scrollback && page["more"] == true) {
            return Ok(());
        }
        cursor = page["cursor"].as_str().map(str::to_string);
    }
}

fn spawn_params(request: &SpawnRequest, base: Value) -> Value {
    let mut params = base;
    if let Some(dir) = &request.dir {
        params["dir"] = json!(dir);
    }
    if let Some(profile) = &request.profile {
        params["profile"] = json!(profile);
    }
    if let Some(name) = &request.name {
        params["name"] = json!(name);
    }
    if let Some(command) = &request.command {
        params["command"] = json!(command);
    }
    if !request.env.is_empty() {
        params["env"] = json!(request.env);
    }
    params
}

fn report_ids(result: &Value, json_mode: bool, out: &mut dyn Write) -> Result<(), Failure> {
    if json_mode {
        return print_json(out, result);
    }
    writeln!(out, "tab {} pane {}", result["tab"], result["pane"]).map_err(|e| io_failure(&e))
}

fn run_verb(
    invocation: &Invocation,
    transport: &mut dyn Transport,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let json_mode = invocation.json;
    match &invocation.verb {
        Verb::Help | Verb::Watch { .. } => Ok(()),
        Verb::List => {
            let result = transport.call("list", json!({}))?;
            if json_mode {
                return print_json(out, &result);
            }
            print_list(&result, out).map_err(|e| io_failure(&e))
        }
        Verb::Capabilities => {
            let result = transport.call("capabilities", json!({}))?;
            if json_mode {
                return print_json(out, &result);
            }
            print_capabilities(&result, out).map_err(|e| io_failure(&e))
        }
        Verb::Open(request) => {
            let result = transport.call("tab.open", spawn_params(request, json!({})))?;
            report_ids(&result, json_mode, out)
        }
        Verb::Close { tab } => {
            let result = transport.call("tab.close", json!({"tab": tab}))?;
            if json_mode {
                print_json(out, &result)?;
            }
            Ok(())
        }
        Verb::FocusTab { tab } => {
            let result = transport.call("tab.focus", json!({"tab": tab}))?;
            if json_mode {
                print_json(out, &result)?;
            }
            Ok(())
        }
        Verb::FocusPane { pane } => {
            let result = transport.call("pane.focus", json!({"pane": pane}))?;
            if json_mode {
                print_json(out, &result)?;
            }
            Ok(())
        }
        Verb::Split {
            pane,
            direction,
            request,
        } => {
            let mut params = spawn_params(request, json!({"pane": pane}));
            if let Some(direction) = direction {
                params["direction"] = json!(direction);
            }
            let result = transport.call("pane.split", params)?;
            report_ids(&result, json_mode, out)
        }
        Verb::Read {
            pane,
            scrollback,
            cursor,
            all,
        } => read_verb(
            transport,
            *pane,
            *scrollback,
            cursor.clone(),
            *all,
            json_mode,
            out,
        ),
        Verb::Send { pane, text, enter } => {
            let mut body = match text {
                Some(text) => text.clone(),
                None => {
                    let mut bytes = Vec::new();
                    stdin.read_to_end(&mut bytes).map_err(|e| {
                        Failure::from(RpcError::bad_params(format!("reading stdin: {e}")))
                    })?;
                    String::from_utf8(bytes)
                        .map_err(|_| Failure::from(RpcError::bad_params("stdin is not UTF-8")))?
                }
            };
            if *enter {
                body.push('\r');
            }
            let sent = send_all(transport, *pane, &body)?;
            if json_mode {
                print_json(out, &json!({"pane": pane, "bytes": sent}))?;
            }
            Ok(())
        }
    }
}

fn fail(
    error: &RpcError,
    note: Option<&str>,
    json_mode: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let _ = writeln!(err, "rustty: {error}{}", note.unwrap_or(""));
    if json_mode {
        let _ = writeln!(out, "{}", json!({"error": error.to_value()}));
    }
    error.code.exit_code()
}

/// Run `watch` on a connected transport until the stream ends. `limit` stops
/// after that many waits with nothing new, for tests.
pub fn run_watch(
    invocation: &Invocation,
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    err: &mut dyn Write,
    limit: Option<usize>,
) -> u8 {
    run_watch_with(invocation, transport, out, err, limit, &mut || {})
}

/// [`run_watch`], calling `on_idle` each time a wait comes back empty.
pub fn run_watch_with(
    invocation: &Invocation,
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    err: &mut dyn Write,
    limit: Option<usize>,
    on_idle: &mut dyn FnMut(),
) -> u8 {
    let Verb::Watch { pane, events } = &invocation.verb else {
        return 0;
    };
    let mut params = json!({});
    if let Some(pane) = pane {
        params["pane"] = json!(pane);
    }
    if let Some(events) = events {
        params["events"] = json!(events);
    }
    if let Err(error) = transport.call("events.subscribe", params) {
        return fail(&error, None, invocation.json, out, err);
    }
    let mut idle = 0usize;
    loop {
        match transport.next_event(Duration::from_millis(250)) {
            Ok(Some(event)) => {
                idle = 0;
                // Whoever was reading has gone, so there is no one left to tell.
                if print_event(&event, invocation.json, out).is_err() {
                    return 0;
                }
                if pane.is_some() && event["event"] == "pane-closed" {
                    return 0;
                }
            }
            Ok(None) => {
                idle += 1;
                if limit.is_some_and(|limit| idle >= limit) {
                    return 0;
                }
                on_idle();
            }
            Err(error) => return fail(&error, None, invocation.json, out, err),
        }
    }
}

/// Run a command line against a connector. Returns the exit code.
pub fn run(
    args: &[String],
    connector: &mut dyn Connector,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(message) => {
            let error = RpcError::bad_params(message);
            return fail(&error, None, false, out, err);
        }
    };
    if invocation.verb == Verb::Help {
        let _ = writeln!(out, "{USAGE}");
        return 0;
    }
    let mut transport = match connector.connect() {
        Ok(transport) => transport,
        Err(error) => return fail(&error, None, invocation.json, out, err),
    };
    if matches!(invocation.verb, Verb::Watch { .. }) {
        return run_watch(&invocation, transport.as_mut(), out, err, None);
    }
    match run_verb(&invocation, transport.as_mut(), stdin, out) {
        Ok(()) => 0,
        Err(failure) => fail(
            &failure.error,
            failure.note.as_deref(),
            invocation.json,
            out,
            err,
        ),
    }
}

/// The process entry: `rustty ctl` with its arguments. Returns the exit code.
#[must_use]
pub fn main(args: &[OsString]) -> u8 {
    let mut text = Vec::with_capacity(args.len());
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    for arg in args {
        let Some(arg) = arg.to_str() else {
            let _ = writeln!(
                stderr,
                "rustty: bad-params: an argument is not valid Unicode"
            );
            return ErrorCode::BadParams.exit_code();
        };
        text.push(arg.to_string());
    }
    // `--window` is parsed twice on purpose: the connector needs it up front, and
    // a malformed one is reported by `run`, which never connects.
    let explicit = parse(&text).ok().and_then(|invocation| invocation.window);
    let mut connector = WindowConnector::new(Target::from_env(explicit), "rustty ctl");
    let code = run(
        &text,
        &mut connector,
        &mut io::stdin(),
        &mut stdout,
        &mut stderr,
    );
    let _ = stdout.flush();
    code
}

#[cfg(test)]
mod tests;
