//! How an encoder reaches a window: which one it means, and a [`Transport`] to
//! it. The encoders only see the trait, so they are tested against a fake.

use std::time::Duration;

use serde_json::{Value, json};

use super::{ENV_TOKEN, ENV_WINDOW, PROTOCOL_VERSION, RpcError};

/// How long a client waits for a window to answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// An open, initialized connection to one window.
pub trait Transport {
    /// Send a request and wait for its reply. Events that arrive meanwhile are
    /// kept for [`Transport::next_event`].
    ///
    /// # Errors
    ///
    /// The server's error, `timeout`, or `target-closed` when the connection ended.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError>;

    /// The next event's params, waiting up to `wait`. `Ok(None)` when none came.
    ///
    /// # Errors
    ///
    /// `slow-client` when the server cut this client off, `target-closed` when
    /// the connection ended any other way.
    fn next_event(&mut self, wait: Duration) -> Result<Option<Value>, RpcError>;
}

/// Makes connections, so an encoder can reconnect after its window goes away.
pub trait Connector {
    /// Pick a window and connect to it.
    ///
    /// # Errors
    ///
    /// Whatever [`super::select_window`] or the connection reports.
    fn connect(&mut self) -> Result<Box<dyn Transport>, RpcError>;
}

/// What a client was told about which window to use. It holds the token, so its
/// `Debug` leaves that out.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Target {
    /// `--window <id>`.
    pub explicit: Option<u64>,
    /// `RUSTTY_WINDOW`.
    pub env_window: Option<String>,
    /// `RUSTTY_AUTOMATION_TOKEN`, for the one encoder that reads it.
    pub env_token: Option<String>,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("explicit", &self.explicit)
            .field("env_window", &self.env_window)
            .field("has_token", &self.env_token.is_some())
            .finish()
    }
}

impl Target {
    /// From this process's environment, without the token. `rustty ctl` uses this:
    /// any command run inside a boss session inherits the variable, and running a
    /// command must not be enough to write (AUTO-007).
    #[must_use]
    pub fn from_env(explicit: Option<u64>) -> Self {
        Self {
            explicit,
            env_window: std::env::var(ENV_WINDOW).ok().filter(|v| !v.is_empty()),
            env_token: None,
        }
    }

    /// Also take the token from the environment. Only `rustty mcp` does: it is the
    /// boss's own tool server, behind Claude Code's permission prompts.
    #[must_use]
    pub fn with_env_token(mut self) -> Self {
        self.env_token = std::env::var(ENV_TOKEN).ok().filter(|v| !v.is_empty());
        self
    }

    /// The token to show window `id`. A token belongs to the window that made
    /// it, the one `RUSTTY_WINDOW` names, so it is never sent to any other.
    #[must_use]
    pub fn token_for(&self, id: u64) -> Option<&str> {
        let own = self.env_window.as_deref()?.trim().parse::<u64>().ok()?;
        if own == id {
            self.env_token.as_deref()
        } else {
            None
        }
    }
}

/// The `initialize` params for a client.
#[must_use]
pub fn initialize_params(client_name: &str, token: Option<&str>) -> Value {
    let mut params = json!({"protocolVersion": PROTOCOL_VERSION, "clientName": client_name});
    if let Some(token) = token {
        params["token"] = json!(token);
    }
    params
}

/// Connects to a real window, found through `%APPDATA%\rustty\ipc`.
#[derive(Debug, Clone)]
pub struct WindowConnector {
    target: Target,
    client_name: &'static str,
}

impl WindowConnector {
    /// For a target and a client name to show the window.
    #[must_use]
    pub const fn new(target: Target, client_name: &'static str) -> Self {
        Self {
            target,
            client_name,
        }
    }
}

#[cfg(windows)]
impl Connector for WindowConnector {
    fn connect(&mut self) -> Result<Box<dyn Transport>, RpcError> {
        let entries = super::discovery_dir_from_env()
            .map(|dir| super::load_discovery(&dir, &super::pipe::alive))
            .unwrap_or_default();
        let entry = super::select_window(
            &entries,
            self.target.explicit,
            self.target.env_window.as_deref(),
        )?;
        let mut transport = super::pipe::open(&entry)?;
        transport.call(
            "initialize",
            initialize_params(self.client_name, self.target.token_for(entry.id)),
        )?;
        Ok(Box::new(transport))
    }
}

/// No window can run on this platform, so there is never one to reach.
#[cfg(not(windows))]
impl Connector for WindowConnector {
    fn connect(&mut self) -> Result<Box<dyn Transport>, RpcError> {
        let _ = (&self.target, self.client_name);
        Err(RpcError::new(
            super::ErrorCode::NoInstance,
            "rustty windows only run on Windows",
        ))
    }
}

/// A [`Transport`] straight onto a dispatcher and a fake window, for encoder
/// tests. Requests are answered on the spot.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{Connector, Transport};
    use crate::automation::RpcError;
    use crate::automation::ops::testkit::FakeWindow;
    use crate::automation::ops::{ClientId, Dispatcher};

    /// One fake window that connections share.
    pub(crate) struct World {
        pub window: FakeWindow,
        pub dispatcher: Dispatcher,
        pub next_client: ClientId,
        pub calls: Vec<(String, Value)>,
        /// Replies to return instead of asking the dispatcher, oldest first.
        pub scripted: VecDeque<Result<Value, RpcError>>,
    }

    pub(crate) type Shared = Rc<RefCell<World>>;

    pub(crate) fn world() -> Shared {
        Rc::new(RefCell::new(World {
            window: FakeWindow::new(),
            dispatcher: Dispatcher::new(),
            next_client: 1,
            calls: Vec::new(),
            scripted: VecDeque::new(),
        }))
    }

    pub(crate) struct FakeTransport {
        pub world: Shared,
        pub client: ClientId,
        pub events: VecDeque<Value>,
    }

    impl Transport for FakeTransport {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
            let mut guard = self.world.borrow_mut();
            let world = &mut *guard;
            world.calls.push((method.to_string(), params.clone()));
            if let Some(scripted) = world.scripted.pop_front() {
                return scripted;
            }
            let request = json!({"id": 1, "method": method, "params": params});
            let frame = world
                .dispatcher
                .handle(&mut world.window, self.client, &request, 0)
                .ok_or_else(|| RpcError::bad_params("no reply"))?;
            match frame.get("error") {
                Some(error) => Err(RpcError::from_value(error)),
                None => Ok(frame["result"].clone()),
            }
        }

        fn next_event(&mut self, _wait: Duration) -> Result<Option<Value>, RpcError> {
            if self.events.is_empty() {
                let mut guard = self.world.borrow_mut();
                let world = &mut *guard;
                let client = self.client;
                let events = &mut self.events;
                world.dispatcher.scan(&world.window.app, &mut |to, frame| {
                    if to == client {
                        events.push_back(frame["params"].clone());
                    }
                });
                // The window's tick clears these right after its scan.
                world.window.app.take_bells();
            }
            Ok(self.events.pop_front())
        }
    }

    /// Connects to the shared fake window, or fails as told.
    pub(crate) struct FakeConnector {
        pub world: Shared,
        pub refuse: Option<RpcError>,
        pub connects: usize,
    }

    impl FakeConnector {
        pub(crate) fn new(world: &Shared) -> Self {
            Self {
                world: Rc::clone(world),
                refuse: None,
                connects: 0,
            }
        }
    }

    impl Connector for FakeConnector {
        fn connect(&mut self) -> Result<Box<dyn Transport>, RpcError> {
            if let Some(error) = &self.refuse {
                return Err(error.clone());
            }
            self.connects += 1;
            let client = {
                let mut world = self.world.borrow_mut();
                world.next_client += 1;
                world.next_client
            };
            // Straight to the dispatcher: scripted replies are for the calls a
            // test makes, not for this handshake.
            {
                let mut guard = self.world.borrow_mut();
                let world = &mut *guard;
                world.dispatcher.handle(
                    &mut world.window,
                    client,
                    &json!({"id": 0, "method": "initialize", "params": {"protocolVersion": 1}}),
                    0,
                );
            }
            Ok(Box::new(FakeTransport {
                world: Rc::clone(&self.world),
                client,
                events: VecDeque::new(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_only_shown_to_the_window_that_made_it() {
        // Break caught: `--window` sending this session's token to another window.
        let target = Target {
            explicit: Some(9),
            env_window: Some("7".to_string()),
            env_token: Some("secret".to_string()),
        };
        assert_eq!(target.token_for(7), Some("secret"));
        assert_eq!(target.token_for(9), None);
        let nameless = Target {
            env_token: Some("secret".to_string()),
            ..Target::default()
        };
        assert_eq!(nameless.token_for(7), None);
        let malformed = Target {
            env_window: Some("x".to_string()),
            env_token: Some("secret".to_string()),
            ..Target::default()
        };
        assert_eq!(malformed.token_for(7), None);
    }

    #[test]
    fn initialize_carries_the_token_only_when_there_is_one() {
        let bare = initialize_params("ctl", None);
        assert_eq!(bare["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(bare["clientName"], "ctl");
        assert!(bare.get("token").is_none());
        assert_eq!(initialize_params("ctl", Some("t"))["token"], "t");
    }

    #[test]
    fn printing_a_target_never_prints_its_token() {
        let target = Target {
            env_token: Some("secret-value".to_string()),
            ..Target::default()
        };
        let printed = format!("{target:?} {:?}", WindowConnector::new(target.clone(), "x"));
        assert!(!printed.contains("secret-value"), "{printed}");
    }
}
