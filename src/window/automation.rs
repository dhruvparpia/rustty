//! The window's side of the automation API (AUTO-003): the [`WindowOps`] the
//! dispatcher drives, and the tick that serves requests. Lives in the window
//! module for its private helpers, and everything here runs on the window's
//! thread. The pipe itself is on the server's threads; nothing here waits on one.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use super::{
    MIN_COLS, MIN_ROWS, ProfileSpec, WindowState, activate_tab, active_profile, close_tab,
    set_title, spec_opts, state_mut, state_ref, sync_tab_bar,
};
use crate::app::App;
use crate::automation::ops::{Dispatcher, SpawnRequest, WindowOps};
use crate::automation::pipe::{Server, new_token};
use crate::automation::{
    ErrorCode, RpcError, TICK_REQUEST_BUDGET, TICK_TIME_BUDGET_MS, Token, discovery_dir_from_env,
    is_write_op, next_id,
};
use crate::shell::SpawnOptions;
use crate::split::Dir;

/// How often statuses are stepped while the sidebar isn't there to do it. The
/// same cadence as the sidebar's own tick.
const HIDDEN_RAIL_STEP: Duration = Duration::from_millis(100);

/// What a window keeps for automation.
pub(super) struct Binding {
    server: Arc<Server>,
    dispatcher: Rc<RefCell<Dispatcher>>,
    token: Option<Arc<Token>>,
    id: u64,
    epoch: Instant,
    /// `automation-writes` as it was when the window started. A file edit can't
    /// grant anything to a running window (AUTO-007).
    writes: bool,
}

/// Start this window's server. A failure only costs this window its automation:
/// it is reported and the terminal carries on.
pub(super) fn start(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    let id = next_id();
    let writes = s.cfg.automation_writes;
    match Server::start(id, s.quick, discovery_dir_from_env()) {
        Ok(server) => {
            s.app.set_window_id(Some(id));
            s.automation = Some(Binding {
                server: Arc::new(server),
                dispatcher: Rc::new(RefCell::new(Dispatcher::new())),
                token: new_token().map(Arc::new),
                id,
                epoch: Instant::now(),
                writes,
            });
        }
        Err(e) => eprintln!("rustty: automation is off for this window: {e}"),
    }
}

/// What the sidebar's heading says about this window's automation grants.
pub(super) fn rail_heading(s: &WindowState) -> &'static str {
    crate::rail::heading(s.automation.as_ref().is_some_and(|b| b.writes))
}

/// The dispatcher's view of this window. It holds no borrow of the window state:
/// closing the last tab destroys the state in the middle of a request.
struct WindowHost {
    hwnd: HWND,
    id: u64,
    token: Option<Arc<Token>>,
    writes: bool,
}

fn window_closed(id: u64) -> RpcError {
    RpcError::closed(id, "the window closed")
}

impl WindowOps for WindowHost {
    fn app(&self) -> Option<&App> {
        state_ref(self.hwnd).map(|s| &s.app)
    }

    fn app_mut(&mut self) -> Option<&mut App> {
        state_mut(self.hwnd).map(|s| &mut s.app)
    }

    fn window_id(&self) -> u64 {
        self.id
    }

    fn writes_allowed(&self) -> bool {
        self.writes
    }

    fn token(&self) -> Option<&Token> {
        self.token.as_deref()
    }

    fn home_project(&self) -> String {
        state_ref(self.hwnd).map_or_else(String::new, |s| s.rail_home_project.clone())
    }

    fn open_tab(&mut self, request: &SpawnRequest) -> Result<(u64, u64), RpcError> {
        open_tab(self.hwnd, self.id, request, None)
    }

    fn split_pane(
        &mut self,
        tab: usize,
        pane: usize,
        dir: Dir,
        request: &SpawnRequest,
    ) -> Result<(u64, u64), RpcError> {
        split_pane(self.hwnd, self.id, tab, pane, dir, request)
    }

    fn close_tab(&mut self, tab: usize) {
        close_tab(self.hwnd, tab);
    }

    fn focus_tab(&mut self, tab: usize) {
        activate_tab(self.hwnd, tab);
    }

    fn focus_pane(&mut self, tab: usize, pane: usize) {
        if let Some(target) = state_mut(self.hwnd).and_then(|s| s.app.tab_at_mut(tab)) {
            target.focus_index(pane);
        }
        activate_tab(self.hwnd, tab);
    }
}

fn find_profile(s: &WindowState, name: &str) -> Result<ProfileSpec, RpcError> {
    let found = s.profiles.iter().find(|p| p.name == name).or_else(|| {
        s.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    });
    found.cloned().ok_or_else(|| {
        let names: Vec<&str> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        RpcError::bad_params(format!(
            "no profile {name:?}; there are: {}",
            names.join(", ")
        ))
    })
}

/// How to start what was asked for. A command runs as given, without the
/// profile's shell or its integration script.
fn spawn_options(
    s: &WindowState,
    spec: &ProfileSpec,
    request: &SpawnRequest,
    cols: usize,
    rows: usize,
) -> Result<SpawnOptions, RpcError> {
    let integrate = s.cfg.shell_integration && request.command.is_none();
    let mut opts = spec_opts(spec, cols, rows, integrate);
    if let Some(dir) = &request.dir {
        if !Path::new(dir).is_dir() {
            return Err(RpcError::bad_params(format!("{dir:?} is not a directory")));
        }
        opts.cwd = Some(dir.clone());
    }
    if let Some((program, args)) = request.command.as_ref().and_then(|c| c.split_first()) {
        opts.program.clone_from(program);
        opts.args = args.to_vec();
    }
    for (key, value) in &request.env {
        opts.env.retain(|have, _| !have.eq_ignore_ascii_case(key));
        opts.env.insert(key.clone(), value.clone());
    }
    s.app.apply_session_env(&mut opts);
    Ok(opts)
}

fn spawn_failed(program: &SpawnRequest, spec: &ProfileSpec, e: &dyn std::fmt::Display) -> RpcError {
    let name = program
        .command
        .as_ref()
        .and_then(|c| c.first())
        .unwrap_or(&spec.program);
    RpcError::bad_params(format!("could not start {name}: {e}"))
}

/// Open a tab in the background. The selected tab doesn't change, so the person's
/// typing keeps going where they are looking; `tab.focus` brings it forward.
fn open_tab(
    hwnd: HWND,
    window: u64,
    request: &SpawnRequest,
    boss_token: Option<&Token>,
) -> Result<(u64, u64), RpcError> {
    let s = state_mut(hwnd).ok_or_else(|| window_closed(window))?;
    let spec = match &request.profile {
        Some(name) => find_profile(s, name)?,
        None => active_profile(s),
    };
    // Check what can be checked before anything visible happens.
    spawn_options(s, &spec, request, MIN_COLS, MIN_ROWS)?;
    let selected = s.app.active_index();
    let (cols, rows) = s
        .app
        .active_tab()
        .map_or((s.spawn.cols, s.spawn.rows), |t| (t.cols(), t.rows()));
    let scrollback = spec.scrollback.unwrap_or(s.cfg.scrollback);
    let index = s.app.add_tab_profile(
        cols.max(MIN_COLS),
        rows.max(MIN_ROWS),
        scrollback,
        spec.name.clone(),
        spec.color,
    );
    s.app.select_tab(selected);
    if let Some(tab) = s.app.tab_at_mut(index) {
        tab.set_confirm(spec.confirm_close);
        if let Some(name) = &request.name {
            tab.set_title(name.clone());
        }
    }
    // A tab bar that appears with the second tab changes the grid the window
    // fits, so sync before the shell starts at that size (issue #8).
    sync_tab_bar(hwnd);
    let s = state_mut(hwnd).ok_or_else(|| window_closed(window))?;
    let (cols, rows) = s
        .renderer
        .as_ref()
        .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
    let (cols, rows) = (cols.max(MIN_COLS), rows.max(MIN_ROWS));
    let mut opts = spawn_options(s, &spec, request, cols, rows)?;
    if let Some(token) = boss_token {
        opts.add_boss_token(token.as_str());
    }
    let tab = s
        .app
        .tab_at_mut(index)
        .ok_or_else(|| RpcError::new(ErrorCode::Internal, "the new tab vanished"))?;
    let _ = tab.resize(cols, rows);
    let spawned = tab.spawn_panes(&opts);
    let ids = (tab.id(), tab.pane().id());
    if let Err(e) = spawned {
        s.app.close_tab(index);
        sync_tab_bar(hwnd);
        return Err(spawn_failed(request, &spec, &e));
    }
    set_title(hwnd, &s.app);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    Ok(ids)
}

/// Split a pane in any tab, keeping focus where it is.
fn split_pane(
    hwnd: HWND,
    window: u64,
    tab: usize,
    pane: usize,
    dir: Dir,
    request: &SpawnRequest,
) -> Result<(u64, u64), RpcError> {
    let s = state_mut(hwnd).ok_or_else(|| window_closed(window))?;
    let spec = match &request.profile {
        Some(name) => find_profile(s, name)?,
        // SPLIT-007: a split takes its tab's profile.
        None => {
            let label = s.app.tabs().get(tab).map(|t| t.profile().to_string());
            label
                .and_then(|label| s.profiles.iter().find(|p| p.name == label).cloned())
                .unwrap_or_else(|| active_profile(s))
        }
    };
    let opts = spawn_options(s, &spec, request, MIN_COLS, MIN_ROWS)?;
    let fresh = s
        .app
        .split_tab_pane(tab, pane, dir)
        .ok_or_else(|| RpcError::bad_params("that pane can't be split"))?;
    let target = s
        .app
        .tab_at_mut(tab)
        .ok_or_else(|| RpcError::new(ErrorCode::Internal, "the tab vanished"))?;
    let tab_id = target.id();
    let spawned = target.spawn_panes(&opts);
    if let Err(e) = spawned {
        if let Some((tab, slot)) = s.app.locate_pane(fresh) {
            s.app.close_tab_pane(tab, slot);
        }
        return Err(spawn_failed(request, &spec, &e));
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    Ok((tab_id, fresh))
}

/// Open a tab for a boss: like `tab.open`, and the session also gets this
/// window's write token as `RUSTTY_AUTOMATION_TOKEN` (AUTO-007). Nothing reachable
/// over the pipe calls this, so no client can ask for the token to be handed out.
///
/// # Errors
///
/// `denied-grant` if this window has no token, and whatever `tab.open` reports.
pub fn open_boss_tab(hwnd: HWND, request: &SpawnRequest) -> Result<(u64, u64), RpcError> {
    let binding = state_ref(hwnd).and_then(|s| s.automation.as_ref());
    let Some((id, token)) = binding.map(|b| (b.id, b.token.clone())) else {
        return Err(RpcError::new(
            ErrorCode::NoInstance,
            "automation is off for this window",
        ));
    };
    let Some(token) = token else {
        return Err(RpcError::new(
            ErrorCode::DeniedGrant,
            "this window could not generate a write token",
        ));
    };
    open_tab(hwnd, id, request, Some(&token))
}

/// Keep statuses current while the sidebar isn't running, so `list` and
/// `status-changed` work with it off, collapsed or in fullscreen. It steps them
/// itself whenever it is showing, and this must not step them a second time.
fn step_hidden_rail(s: &mut WindowState, hwnd: HWND) {
    if s.rail_w > 0.0 {
        return;
    }
    let now = Instant::now();
    if s.rail_next_tick.is_some_and(|next| now < next) {
        return;
    }
    s.rail_next_tick = Some(now + HIDDEN_RAIL_STEP);
    let focused = unsafe { GetForegroundWindow() } == hwnd;
    let selected = s.app.active_index();
    let elapsed = now.duration_since(s.rail_epoch);
    let now_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    for index in 0..s.app.tab_count() {
        if let Some(tab) = s.app.tab_at_mut(index) {
            let viewed = index == selected && focused;
            tab.for_each_pane(|pane, _| {
                pane.rail_step(viewed, now_ms);
            });
        }
    }
}

/// Runs each tick after the shells drained and before the window takes the bell
/// flags and closes dead panes: a pane's last output and its exit code are still
/// there to report. With nobody subscribed this costs one check.
pub(super) fn scan(s: &mut WindowState, hwnd: HWND) {
    let Some(binding) = &s.automation else { return };
    let server = Arc::clone(&binding.server);
    let dispatcher = Rc::clone(&binding.dispatcher);
    step_hidden_rail(s, hwnd);
    let Ok(mut dispatcher) = dispatcher.try_borrow_mut() else {
        return;
    };
    dispatcher.scan(&s.app, &mut |client, frame| server.send(client, frame));
}

/// Serve requests for this tick, within the budget. Runs last in the tick, since
/// a request can destroy the window. Replies go through the server's own handle,
/// which outlives the window state for exactly that case.
///
/// While a dialog is up or the window is being dragged, writes wait: code behind
/// a confirmation box picks up where it left off assuming the tabs didn't change.
/// Reads carry on. A client that gives up waiting takes its requests with it.
pub(super) fn tick(hwnd: HWND) {
    let Some(binding) = state_ref(hwnd).and_then(|s| s.automation.as_ref()) else {
        return;
    };
    let server = Arc::clone(&binding.server);
    let dispatcher = Rc::clone(&binding.dispatcher);
    let mut host = WindowHost {
        hwnd,
        id: binding.id,
        token: binding.token.clone(),
        writes: binding.writes,
    };
    let epoch = binding.epoch;
    server.reap();
    // A nested message loop can land here during a request; it waits its turn.
    let Ok(mut dispatcher) = dispatcher.try_borrow_mut() else {
        return;
    };
    for gone in server.take_gone() {
        dispatcher.client_gone(gone);
    }
    let writes_open = unsafe { IsWindowEnabled(hwnd) }.as_bool()
        && !state_ref(hwnd).is_some_and(|s| s.drag_resize);
    let allow = |request: &Value| {
        writes_open || !is_write_op(request.get("method").and_then(Value::as_str).unwrap_or(""))
    };
    let started = Instant::now();
    for _ in 0..TICK_REQUEST_BUDGET {
        let Some((client, request)) = server.next_request(&allow) else {
            break;
        };
        let now_ms = u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reply) = dispatcher.handle(&mut host, client, &request, now_ms) {
            server.send(client, &reply);
        }
        if state_ref(hwnd).is_none() || started.elapsed().as_millis() >= TICK_TIME_BUDGET_MS {
            break;
        }
    }
}
