//! The dispatcher: the one place that resolves targets, checks grants and bounds
//! reads. Every encoder (raw IPC, `rustty ctl`, `rustty mcp`) ends up here, so
//! none of them can disagree about what a request means (ADR intro).
//!
//! It runs on the window's thread, one request at a time, against a
//! [`WindowOps`]. The window implements that for real; tests use a fake with a
//! real [`App`] and no HWND, which is why almost all of this is tested headless.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use serde_json::{Value, json};

use super::events::{Subscriptions, Tracker};
use super::{
    ErrorCode, OPERATIONS, OpSpec, PAGE_TEXT_BYTES, PROTOCOL_VERSION, READ_TIME_BUDGET, RpcError,
    SEND_BURST_BYTES, Token, TokenBucket, can_write, capabilities, decode_cursor, encode_cursor,
    error_frame, negotiate, read_lines, read_lines_while, time_box,
};
use crate::app::App;
use crate::pane::Pane;
use crate::rail;
use crate::shell::ShellState;
use crate::split::Dir;
use crate::tab::Tab;

/// A connection, as the transport numbers them.
pub type ClientId = u64;

/// How to start a session: the shared part of `tab.open` and `pane.split`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnRequest {
    /// Working directory.
    pub dir: Option<String>,
    /// Shell profile by name.
    pub profile: Option<String>,
    /// Tab title. Only `tab.open` takes one.
    pub name: Option<String>,
    /// Argv to run instead of the profile's shell.
    pub command: Option<Vec<String>>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

/// What the dispatcher needs from a window. The window's versions run on its
/// own thread and may destroy it, which is why reading the app can fail.
pub trait WindowOps {
    /// The window's tabs. `None` once the window has been destroyed.
    fn app(&self) -> Option<&App>;
    /// The window's tabs, mutably. `None` once the window has been destroyed.
    fn app_mut(&mut self) -> Option<&mut App>;
    /// This window's automation id.
    fn window_id(&self) -> u64;
    /// The `automation-writes` config key, as it was when the window started. It
    /// never changes while the window runs (AUTO-007).
    fn writes_allowed(&self) -> bool;
    /// This window's write token, when one could be generated.
    fn token(&self) -> Option<&Token>;
    /// The project heading for a pane whose folder is unknown.
    fn home_project(&self) -> String;
    /// Open a session in a new background tab: returns its tab and pane ids.
    ///
    /// # Errors
    ///
    /// `bad-params` for an unknown profile or a program that won't start.
    fn open_tab(&mut self, request: &SpawnRequest) -> Result<(u64, u64), RpcError>;
    /// Split a pane, keeping focus, and start a session: returns tab and pane ids.
    ///
    /// # Errors
    ///
    /// `bad-params` for an unknown profile or a program that won't start.
    fn split_pane(
        &mut self,
        tab: usize,
        pane: usize,
        dir: Dir,
        request: &SpawnRequest,
    ) -> Result<(u64, u64), RpcError>;
    /// Close a tab without asking. Closing the last one destroys the window.
    fn close_tab(&mut self, tab: usize);
    /// Select a tab.
    fn focus_tab(&mut self, tab: usize);
    /// Focus a pane and select its tab.
    fn focus_pane(&mut self, tab: usize, pane: usize);
    /// Type bytes into a pane.
    ///
    /// # Errors
    ///
    /// `target-closed` for a dead shell, `denied-grant` for a read-only view.
    fn send_text(&mut self, tab: usize, pane: usize, bytes: &[u8]) -> Result<(), RpcError> {
        let id = self.window_id();
        let gone = || RpcError::closed(id, "the window closed");
        let app = self.app_mut().ok_or_else(gone)?;
        let target = app.tab_at_mut(tab).ok_or_else(gone)?;
        let mut target = target.pane_at_mut(pane).ok_or_else(gone)?;
        deliver_text(&mut target, bytes)
    }
}

/// Type bytes into a pane through the same path as the keyboard, and record it
/// for the sidebar (AUTO-011).
///
/// # Errors
///
/// `target-closed` for a dead shell, `denied-grant` for a read-only view.
pub fn deliver_text(pane: &mut Pane, bytes: &[u8]) -> Result<(), RpcError> {
    let id = pane.id();
    if matches!(
        pane.shell().state(),
        ShellState::Exited(_) | ShellState::Inactive
    ) {
        return Err(RpcError::closed(id, "its shell has exited"));
    }
    if pane.view_alias().is_some() && !pane.view_controller() {
        return Err(RpcError {
            target: Some(id),
            ..RpcError::new(
                ErrorCode::DeniedGrant,
                "this pane is a read-only view of a host session",
            )
        });
    }
    pane.send_input(bytes)
        .map_err(|e| RpcError::closed(id, format!("its shell did not take the text: {e}")))?;
    pane.note_automation_input();
    Ok(())
}

#[derive(Debug, Default)]
struct ClientState {
    protocol: Option<u32>,
    token_ok: bool,
}

/// One window's automation state: its connections, send buckets and events.
#[derive(Debug, Default)]
pub struct Dispatcher {
    clients: BTreeMap<ClientId, ClientState>,
    subscriptions: BTreeMap<ClientId, Subscriptions>,
    buckets: HashMap<u64, TokenBucket>,
    tracker: Tracker,
    /// Tabs and panes a token-holding connection opened: all a token alone can
    /// write to (AUTO-007). The window's, not the connection's, so a boss's tool
    /// server can restart without losing its workers.
    token_opened: HashSet<u64>,
}

impl Dispatcher {
    /// With no connections yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer one request. `None` for a notification, which gets no reply.
    /// `now_ms` is any monotonic clock, for the send rate limit.
    pub fn handle(
        &mut self,
        win: &mut dyn WindowOps,
        client: ClientId,
        request: &Value,
        now_ms: u64,
    ) -> Option<Value> {
        let id = request.get("id").cloned();
        let outcome = self.run(win, client, request, now_ms);
        let id = id?;
        Some(match outcome {
            Ok(result) => crate::host::response(id, result),
            Err(error) => error_frame(id, &error),
        })
    }

    /// Forget a connection: its grant, its subscriptions.
    pub fn client_gone(&mut self, client: ClientId) {
        self.clients.remove(&client);
        self.subscriptions.remove(&client);
    }

    /// Whether anyone is subscribed to anything.
    #[must_use]
    pub fn has_subscribers(&self) -> bool {
        self.subscriptions.values().any(|s| !s.is_empty())
    }

    /// Give the output scan a different time budget.
    #[cfg(test)]
    pub(crate) fn set_output_budget(&mut self, budget: std::time::Duration) {
        self.tracker.set_output_budget(budget);
    }

    /// Send what changed since the last scan. Runs once per tick, before the
    /// window takes the bell flags and closes dead panes.
    pub fn scan(&mut self, app: &App, send: &mut dyn FnMut(ClientId, &Value)) {
        self.tracker.scan(app, &mut self.subscriptions, send);
    }

    fn run(
        &mut self,
        win: &mut dyn WindowOps,
        client: ClientId,
        request: &Value,
        now_ms: u64,
    ) -> Result<Value, RpcError> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::bad_params("the request has no method"))?;
        let op = OPERATIONS
            .iter()
            .find(|op| op.name == method)
            .ok_or_else(|| RpcError::bad_params(format!("unknown method {method:?}")))?;
        let empty = json!({});
        let params = request.get("params").unwrap_or(&empty);
        if !params.is_object() {
            return Err(RpcError::bad_params("params must be an object"));
        }
        if op.name == "initialize" {
            check_params(op, params)?;
            return self.initialize(win, client, params);
        }
        let state = self.clients.entry(client).or_default();
        if state.protocol.is_none() {
            return Err(RpcError::bad_params("send initialize first"));
        }
        // The grant comes first, so a refused client learns nothing about targets.
        if op.write && !can_write(win.writes_allowed(), state.token_ok) {
            return Err(RpcError::new(
                ErrorCode::DeniedGrant,
                format!(
                    "{method} needs a write grant: set automation-writes = true, \
                     or connect with this window's token"
                ),
            ));
        }
        check_params(op, params)?;
        let token_ok = state.token_ok;
        // Only the config key reaches everything; a token reaches what it opened.
        let token_only = op.write && !win.writes_allowed();
        if token_only {
            self.check_token_scope(op, params)?;
        }
        match op.name {
            "capabilities" => Ok(capabilities()),
            "list" => list(&*win),
            "pane.read" => pane_read(&*win, params),
            "events.subscribe" => self.subscribe(&*win, client, params),
            "events.unsubscribe" => self.unsubscribe(client, params),
            "tab.open" => {
                let request = spawn_request(params)?;
                let (tab, pane) = win.open_tab(&request)?;
                if token_ok {
                    self.note_token_opened(&*win, &[tab, pane]);
                }
                Ok(json!({"tab": tab, "pane": pane}))
            }
            "tab.close" => {
                let id = id_param(params, "tab")?;
                let index = locate_tab(&*win, id)?;
                win.close_tab(index);
                Ok(json!({"tab": id, "closed": true}))
            }
            "tab.focus" => {
                let id = id_param(params, "tab")?;
                let index = locate_tab(&*win, id)?;
                win.focus_tab(index);
                Ok(json!({"tab": id}))
            }
            "pane.split" => {
                let id = id_param(params, "pane")?;
                let dir = match params.get("direction").map(Value::as_str) {
                    None | Some(Some("right")) => Dir::Vertical,
                    Some(Some("down")) => Dir::Horizontal,
                    Some(_) => return Err(RpcError::bad_params("direction: right or down")),
                };
                let request = spawn_request(params)?;
                let (tab, pane) = locate_pane(&*win, id)?;
                let (tab, pane) = win.split_pane(tab, pane, dir, &request)?;
                if token_ok {
                    self.note_token_opened(&*win, &[pane]);
                }
                Ok(json!({"tab": tab, "pane": pane}))
            }
            "pane.focus" => {
                let id = id_param(params, "pane")?;
                let (tab, pane) = locate_pane(&*win, id)?;
                let tab_id = window_app(&*win)?.tabs().get(tab).map(Tab::id);
                win.focus_pane(tab, pane);
                Ok(json!({"tab": tab_id, "pane": id}))
            }
            "pane.send-text" => self.send_text(win, params, now_ms, token_only),
            // The table and this match are in the same crate and a test walks
            // the whole table; this only answers if they ever drift.
            other => Err(RpcError::new(
                ErrorCode::Internal,
                format!("{other} has no handler"),
            )),
        }
    }

    /// A token-only grant reaches only what a token opened. Checked before the
    /// target is looked up, so a refused client can't tell a closed id from one that
    /// never was.
    fn check_token_scope(&self, op: &OpSpec, params: &Value) -> Result<(), RpcError> {
        let id = match op.name {
            "tab.close" | "tab.focus" => id_param(params, "tab")?,
            "pane.split" | "pane.focus" | "pane.send-text" => id_param(params, "pane")?,
            _ => return Ok(()),
        };
        if self.token_opened.contains(&id) {
            return Ok(());
        }
        Err(RpcError {
            target: Some(id),
            ..RpcError::new(
                ErrorCode::DeniedGrant,
                format!(
                    "{} with this window's token reaches only sessions opened with it",
                    op.name
                ),
            )
        })
    }

    fn note_token_opened(&mut self, win: &dyn WindowOps, ids: &[u64]) {
        self.token_opened.extend(ids);
        if self.token_opened.len() > 1024
            && let Some(app) = win.app()
        {
            self.token_opened
                .retain(|id| app.locate_tab(*id).is_some() || app.locate_pane(*id).is_some());
        }
    }

    fn initialize(
        &mut self,
        win: &dyn WindowOps,
        client: ClientId,
        params: &Value,
    ) -> Result<Value, RpcError> {
        let state = self.clients.entry(client).or_default();
        if state.protocol.is_some() {
            return Err(RpcError::bad_params("this connection already initialized"));
        }
        let version = params
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError::bad_params("protocolVersion must be an integer"))?;
        let version = negotiate(u32::try_from(version).unwrap_or(u32::MAX))?;
        let presented = match params.get("token") {
            None => None,
            Some(Value::String(token)) => Some(token.as_str()),
            // Never echo it: whatever this is, it may be a secret.
            Some(_) => return Err(RpcError::bad_params("token must be a string")),
        };
        state.token_ok = match (win.token(), presented) {
            (Some(token), Some(presented)) => token.matches(presented),
            _ => false,
        };
        state.protocol = Some(version);
        Ok(json!({
            "protocolVersion": version,
            "serverProtocolVersion": PROTOCOL_VERSION,
            "capabilities": capabilities(),
            "window": win.window_id(),
            "grants": {"writes": can_write(win.writes_allowed(), state.token_ok)},
            "serverVersion": env!("CARGO_PKG_VERSION"),
        }))
    }

    fn subscribe(
        &mut self,
        win: &dyn WindowOps,
        client: ClientId,
        params: &Value,
    ) -> Result<Value, RpcError> {
        let app = window_app(win)?;
        let pane = optional_id(params, "pane")?;
        if let Some(id) = pane
            && app.locate_pane(id).is_none()
        {
            return Err(RpcError::stale(id));
        }
        let events = match params.get("events") {
            None => None,
            Some(list) => Some(event_names(list)?),
        };
        self.tracker.ensure_baseline(app);
        let subscriptions = self.subscriptions.entry(client).or_default();
        subscriptions.subscribe(app, pane, events);
        Ok(json!({"pane": pane, "subscribed": true}))
    }

    fn unsubscribe(&mut self, client: ClientId, params: &Value) -> Result<Value, RpcError> {
        let pane = optional_id(params, "pane")?;
        let removed = self
            .subscriptions
            .get_mut(&client)
            .is_some_and(|s| s.unsubscribe(pane));
        Ok(json!({"pane": pane, "removed": removed}))
    }

    fn send_text(
        &mut self,
        win: &mut dyn WindowOps,
        params: &Value,
        now_ms: u64,
        token_only: bool,
    ) -> Result<Value, RpcError> {
        let id = id_param(params, "pane")?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::bad_params("text must be a string"))?;
        if text.len() as u64 > SEND_BURST_BYTES {
            return Err(RpcError::bad_params(format!(
                "text is {} bytes; split it: at most {SEND_BURST_BYTES} bytes a call",
                text.len()
            )));
        }
        let (tab, pane) = locate_pane(&*win, id)?;
        // The person decides these (AUTO-007). The message never repeats the
        // program's words: a boss reads this, and they are the program's to choose.
        if token_only
            && window_app(&*win)?
                .tabs()
                .get(tab)
                .and_then(|tab| tab.pane_at(pane))
                .is_some_and(|pane| pane.awaits_approval())
        {
            return Err(RpcError {
                target: Some(id),
                ..RpcError::new(
                    ErrorCode::DeniedGrant,
                    "this session is waiting on a person's permission, trust or sign-in, which \
                     a token can't answer: tell the person",
                )
            });
        }
        // Per pane, not per connection: a client opening a connection per call
        // must not get a fresh burst each time (ADR amendment 2).
        let bucket = self
            .buckets
            .entry(id)
            .or_insert_with(|| TokenBucket::new(now_ms));
        bucket
            .take(text.len() as u64, now_ms)
            .map_err(|wait| RpcError {
                target: Some(id),
                ..RpcError::rate_limited(wait)
            })?;
        win.send_text(tab, pane, text.as_bytes())?;
        if self.buckets.len() > 256
            && let Some(app) = win.app()
        {
            self.buckets.retain(|id, _| app.locate_pane(*id).is_some());
        }
        Ok(json!({"pane": id, "bytes": text.len()}))
    }
}

/// Refuse parameters the operation doesn't have and require the ones it needs,
/// from its schema. Ignoring `{"cwd": ...}` would open a session in the wrong
/// place without saying so.
fn check_params(op: &OpSpec, params: &Value) -> Result<(), RpcError> {
    let schema = (op.params)();
    let empty = serde_json::Map::new();
    let known = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    if let Some(given) = params.as_object()
        && let Some(unknown) = given.keys().find(|key| !known.contains_key(key.as_str()))
    {
        return Err(RpcError::bad_params(format!(
            "{} has no parameter {unknown:?}",
            op.name
        )));
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if params.get(name).is_none() {
                return Err(RpcError::bad_params(format!("{} needs {name:?}", op.name)));
            }
        }
    }
    Ok(())
}

fn id_param(params: &Value, name: &str) -> Result<u64, RpcError> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::bad_params(format!("{name} must be an id")))
}

fn optional_id(params: &Value, name: &str) -> Result<Option<u64>, RpcError> {
    match params.get(name) {
        None => Ok(None),
        Some(_) => id_param(params, name).map(Some),
    }
}

fn event_names(list: &Value) -> Result<Vec<String>, RpcError> {
    let list = list
        .as_array()
        .ok_or_else(|| RpcError::bad_params("events must be a list of names"))?;
    let mut names = Vec::with_capacity(list.len());
    for name in list {
        match name.as_str() {
            Some(name) if super::EVENTS.contains(&name) => names.push(name.to_string()),
            _ => return Err(RpcError::bad_params(format!("no such event: {name}"))),
        }
    }
    Ok(names)
}

fn optional_text(params: &Value, name: &str) -> Result<Option<String>, RpcError> {
    match params.get(name) {
        None => Ok(None),
        Some(Value::String(text)) if !text.is_empty() && !text.contains('\0') => {
            Ok(Some(text.clone()))
        }
        Some(_) => Err(RpcError::bad_params(format!(
            "{name} must be non-empty text"
        ))),
    }
}

fn spawn_request(params: &Value) -> Result<SpawnRequest, RpcError> {
    let mut request = SpawnRequest {
        dir: optional_text(params, "dir")?,
        profile: optional_text(params, "profile")?,
        name: optional_text(params, "name")?,
        ..SpawnRequest::default()
    };
    if let Some(command) = params.get("command") {
        let bad = || RpcError::bad_params("command must be a list of arguments, program first");
        let list = command.as_array().ok_or_else(bad)?;
        let mut argv = Vec::with_capacity(list.len());
        for arg in list {
            let arg = arg.as_str().ok_or_else(bad)?;
            if arg.contains('\0') {
                return Err(bad());
            }
            argv.push(arg.to_string());
        }
        if argv.first().is_none_or(String::is_empty) {
            return Err(bad());
        }
        request.command = Some(argv);
    }
    if let Some(env) = params.get("env") {
        let bad = || RpcError::bad_params("env must map variable names to text");
        for (key, value) in env.as_object().ok_or_else(bad)? {
            let value = value.as_str().ok_or_else(bad)?;
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err(bad());
            }
            // A session is never handed the token by a client (AUTO-007). The name
            // is safe to say; the value might not be.
            if key.eq_ignore_ascii_case(super::ENV_TOKEN) {
                return Err(RpcError::bad_params(format!(
                    "env can't set {}: the write token doesn't pass to other sessions",
                    super::ENV_TOKEN
                )));
            }
            // The window always says which window a session is in (AUTO-006).
            if !key.eq_ignore_ascii_case(super::ENV_WINDOW) {
                request.env.insert(key.clone(), value.to_string());
            }
        }
    }
    Ok(request)
}

fn window_app(win: &dyn WindowOps) -> Result<&App, RpcError> {
    win.app()
        .ok_or_else(|| RpcError::closed(win.window_id(), "the window closed"))
}

fn locate_tab(win: &dyn WindowOps, id: u64) -> Result<usize, RpcError> {
    window_app(win)?
        .locate_tab(id)
        .ok_or_else(|| RpcError::stale(id))
}

fn locate_pane(win: &dyn WindowOps, id: u64) -> Result<(usize, usize), RpcError> {
    window_app(win)?
        .locate_pane(id)
        .ok_or_else(|| RpcError::stale(id))
}

fn project_of_pane(pane: &Pane, home: &str) -> String {
    pane.saved_cwd()
        .map_or_else(|| home.to_string(), |cwd| rail::project_of(Some(cwd)))
}

fn describe_pane(
    pane: &Pane,
    tab: &Tab,
    path: Option<&String>,
    focused: bool,
    home: &str,
) -> Value {
    let status = pane.rail_status();
    let state = pane.shell().state();
    let exit_code = match state {
        ShellState::Exited(code) => code,
        _ => None,
    };
    json!({
        "id": pane.id(),
        "path": path,
        "title": rail::row_title(pane.title().unwrap_or(""), tab.profile()),
        "status": status.as_str(),
        "detail": pane.rail_detail(),
        "project": project_of_pane(pane, home),
        "cwd": pane.saved_cwd(),
        "agent": pane.saved_agent(),
        "active": focused,
        "exited": matches!(state, ShellState::Exited(_) | ShellState::Inactive),
        "exitCode": exit_code,
        "commandRunning": pane.command_running(),
    })
}

/// `list`: statuses as the sidebar last computed them. Reading must not step
/// them: a step clears "finished" and moves the Working timer.
fn list(win: &dyn WindowOps) -> Result<Value, RpcError> {
    let app = window_app(win)?;
    let home = win.home_project();
    let selected = app.active_index();
    let mut tabs = Vec::with_capacity(app.tab_count());
    for (index, tab) in app.tabs().iter().enumerate() {
        let paths = tab.pane_paths();
        let focused = tab.focused_index();
        let mut panes = Vec::with_capacity(tab.pane_count());
        let mut statuses = Vec::with_capacity(tab.pane_count());
        let mut messages = Vec::with_capacity(tab.pane_count());
        let mut stopped = None;
        let mut slot = 0;
        tab.for_each_pane_ref(|pane, _| {
            let status = pane.rail_status();
            statuses.push(status);
            messages.push((status, pane.status_message(status)));
            if stopped.is_none() {
                stopped = pane.interrupted().map(str::to_string);
            }
            panes.push(describe_pane(
                pane,
                tab,
                paths.get(slot),
                focused == Some(slot),
                &home,
            ));
            slot += 1;
        });
        let status = rail::most_urgent(statuses);
        tabs.push(json!({
            "id": tab.id(),
            "number": index + 1,
            "title": rail::row_title(tab.title(), tab.profile()),
            "status": status.as_str(),
            "detail": rail::tab_detail(status, stopped.as_deref(), &messages),
            "project": project_of_pane(&tab.pane(), &home),
            "profile": tab.profile(),
            "active": index == selected,
            "panes": panes,
        }));
    }
    Ok(json!({"window": win.window_id(), "tabs": tabs}))
}

/// `pane.read`: the live screen, or a page of history.
fn pane_read(win: &dyn WindowOps, params: &Value) -> Result<Value, RpcError> {
    let id = id_param(params, "pane")?;
    let (tab, slot) = locate_pane(win, id)?;
    let app = window_app(win)?;
    let pane = app
        .tabs()
        .get(tab)
        .and_then(|tab| tab.pane_at(slot))
        .ok_or_else(|| RpcError::stale(id))?;
    let scrollback = match params.get("scrollback") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(RpcError::bad_params("scrollback must be true or false")),
    };
    let cursor = match params.get("cursor") {
        None => None,
        Some(Value::String(cursor)) => Some(cursor.as_str()),
        Some(_) => return Err(RpcError::bad_params("cursor must be text")),
    };
    if scrollback {
        read_history(&pane, cursor)
    } else if cursor.is_some() {
        Err(RpcError::bad_params("a cursor needs scrollback: true"))
    } else {
        Ok(read_screen(&pane))
    }
}

/// The live rows, wherever the person has scrolled the viewport to.
fn read_screen(pane: &Pane) -> Value {
    let grid = pane.grid();
    let first = grid.live_abs(0).unwrap_or_else(|| grid.first_abs());
    let last = first + grid.rows.saturating_sub(1);
    let page = read_lines(first, last, PAGE_TEXT_BYTES, |abs| grid.line_text_abs(abs));
    let trimmed = page.text.trim_end_matches('\n');
    let text = if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    };
    json!({
        "pane": pane.id(),
        "text": text,
        "cols": grid.cols,
        "rows": grid.rows,
        "cursorRow": pane.parser().cursor_row,
        "cursorCol": pane.parser().cursor_col,
        "altScreen": pane.in_alt(),
    })
}

/// A page of history, oldest first. The returned cursor never passes the line
/// the shell is still writing, so keeping it reads that line again once done.
fn read_history(pane: &Pane, cursor: Option<&str>) -> Result<Value, RpcError> {
    let grid = pane.history_grid();
    let live = (!pane.in_alt()).then(|| pane.cursor_abs());
    let end = live.unwrap_or_else(|| grid.last_abs());
    let start = match cursor {
        Some(cursor) => decode_cursor(pane.id(), cursor)?,
        None => grid.first_abs(),
    };
    let first = start.max(grid.first_abs());
    let mut keep_going = time_box(Instant::now(), READ_TIME_BUDGET);
    let page = read_lines_while(first, end, PAGE_TEXT_BYTES, &mut keep_going, |abs| {
        grid.line_text_abs(abs)
    });
    let next = live.map_or(page.next, |line| page.next.min(line));
    let mut result = json!({
        "pane": pane.id(),
        "text": page.text,
        "fromLine": page.from,
        "cursor": encode_cursor(pane.id(), next),
        "more": page.more,
    });
    if first > start {
        result["skipped"] = json!(first - start);
    }
    Ok(result)
}

/// A fake window for dispatcher and encoder tests.
#[cfg(test)]
pub(crate) mod testkit {
    use serde_json::{Value, json};

    use super::{ClientId, Dispatcher, SpawnRequest, WindowOps};
    use crate::app::App;
    use crate::automation::{RpcError, Token};
    use crate::split::Dir;

    /// A real [`App`] with no shells.
    pub(crate) struct FakeWindow {
        pub app: App,
        pub allow_writes: bool,
        pub token: Option<Token>,
        pub sent: Vec<(u64, Vec<u8>)>,
        pub opened: Vec<SpawnRequest>,
        pub splits: Vec<(SpawnRequest, Dir)>,
        pub fail_open: Option<RpcError>,
        pub fail_send: Option<RpcError>,
        pub gone: bool,
        id: u64,
    }

    impl FakeWindow {
        pub(crate) fn new() -> Self {
            Self::sized(80, 24, 1000)
        }

        pub(crate) fn sized(cols: usize, rows: usize, scrollback: usize) -> Self {
            Self {
                app: App::with_size(cols, rows, scrollback),
                allow_writes: false,
                token: None,
                sent: Vec::new(),
                opened: Vec::new(),
                splits: Vec::new(),
                fail_open: None,
                fail_send: None,
                gone: false,
                id: crate::automation::next_id(),
            }
        }
    }

    impl WindowOps for FakeWindow {
        fn app(&self) -> Option<&App> {
            (!self.gone).then_some(&self.app)
        }

        fn app_mut(&mut self) -> Option<&mut App> {
            (!self.gone).then_some(&mut self.app)
        }

        fn window_id(&self) -> u64 {
            self.id
        }

        fn writes_allowed(&self) -> bool {
            self.allow_writes
        }

        fn token(&self) -> Option<&Token> {
            self.token.as_ref()
        }

        fn home_project(&self) -> String {
            "home-project".to_string()
        }

        fn open_tab(&mut self, request: &SpawnRequest) -> Result<(u64, u64), RpcError> {
            if let Some(error) = &self.fail_open {
                return Err(error.clone());
            }
            self.opened.push(request.clone());
            let selected = self.app.active_index();
            let index = self.app.add_tab_sized(80, 24, 1000);
            self.app.select_tab(selected);
            let tab = &self.app.tabs()[index];
            Ok((tab.id(), tab.pane().id()))
        }

        fn split_pane(
            &mut self,
            tab: usize,
            pane: usize,
            dir: Dir,
            request: &SpawnRequest,
        ) -> Result<(u64, u64), RpcError> {
            self.splits.push((request.clone(), dir));
            let fresh = self
                .app
                .split_tab_pane(tab, pane, dir)
                .ok_or_else(|| RpcError::bad_params("no such pane"))?;
            Ok((self.app.tabs()[tab].id(), fresh))
        }

        fn close_tab(&mut self, tab: usize) {
            self.app.close_tab(tab);
        }

        fn focus_tab(&mut self, tab: usize) {
            self.app.select_tab(tab);
        }

        fn focus_pane(&mut self, tab: usize, pane: usize) {
            self.app.select_tab(tab);
            self.app.focus_pane(pane);
        }

        fn send_text(&mut self, tab: usize, pane: usize, bytes: &[u8]) -> Result<(), RpcError> {
            if let Some(error) = &self.fail_send {
                return Err(error.clone());
            }
            let id = self.app.tabs()[tab].pane_at(pane).map_or(0, |p| p.id());
            self.sent.push((id, bytes.to_vec()));
            Ok(())
        }
    }

    /// Send a request and return the reply frame.
    pub(crate) fn call(
        d: &mut Dispatcher,
        w: &mut FakeWindow,
        client: ClientId,
        method: &str,
        params: Value,
    ) -> Value {
        d.handle(
            w,
            client,
            &json!({"id": 1, "method": method, "params": params}),
            0,
        )
        .expect("a request gets a reply")
    }

    /// Initialize a connection.
    pub(crate) fn connect(d: &mut Dispatcher, w: &mut FakeWindow, client: ClientId) {
        let frame = call(
            d,
            w,
            client,
            "initialize",
            json!({"protocolVersion": 1, "clientName": "test"}),
        );
        ok(&frame);
    }

    /// The result of a reply that must have succeeded.
    pub(crate) fn ok(frame: &Value) -> Value {
        assert!(frame.get("error").is_none(), "{frame}");
        frame["result"].clone()
    }

    /// The error of a reply that must have failed.
    pub(crate) fn err(frame: &Value) -> RpcError {
        assert!(frame.get("result").is_none(), "{frame}");
        RpcError::from_value(&frame["error"])
    }
}

#[cfg(test)]
mod tests;
