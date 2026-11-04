//! TaskRunner seam (#89): task-store lookup, dependency readiness, canonical
//! session reconnect/create, stop and restart behind one interface. The window
//! layer is an adapter that renders palette rows and calls this seam.

use std::collections::BTreeSet;
use std::sync::Arc;

#[cfg(windows)]
use serde_json::{Value, json};

use crate::tasks::{Store, Task};

/// Host adapter: the real host client or a synthetic test adapter.
#[derive(Clone)]
#[allow(clippy::complexity, reason = "the adapter has three delegate fields")]
pub struct HostAdapter {
    /// Current host session records.
    list: Arc<dyn Fn() -> Result<Vec<crate::host::SessionRecord>, String> + Send + Sync + 'static>,
    /// Create a persistent session for one task; return its alias.
    create: Arc<dyn Fn(&Task) -> Result<u32, String> + Send + Sync + 'static>,
    /// Terminate one session by alias.
    terminate: Arc<dyn Fn(u32) -> Result<(), String> + Send + Sync + 'static>,
}

/// Explicit task-run outcomes; no generic exceptions across the seam.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A new run was started.
    Started(u32),
    /// An existing run was reconnected.
    AlreadyRunning(u32),
    /// A live run was stopped.
    Stopped(u32),
    /// The task id is absent or the store is unreadable.
    MissingTask(u32),
    /// The task is not live.
    NotLive(u32),
    /// A dependency is missing or failed to start.
    DependencyFailed(u32, String),
    /// The host adapter failed.
    HostFailed(String),
}

/// Deep task lifecycle module.
#[derive(Clone)]
pub struct TaskRunner {
    store: Store,
    host: HostAdapter,
}

impl TaskRunner {
    /// Build a runner from a store and a host adapter.
    #[must_use]
    pub fn with(store: &Store, host: &HostAdapter) -> Self {
        Self {
            store: store.clone(),
            host: host.clone(),
        }
    }

    /// Find the live session for `task` by canonical label.
    fn live_alias(&self, sessions: &[crate::host::SessionRecord], task: &Task) -> Option<u32> {
        for rec in sessions.iter() {
            if rec.state == "live" && rec.task_id == Some(task.id) && rec.project == task.project {
                return Some(rec.alias);
            }
        }
        None
    }

    /// Transitive dependency plan: deps first, target last, one store snapshot.
    pub fn plan(&self, id: u32) -> Result<Vec<Task>, u32> {
        let Some(task) = self.store.find(id).map(|i| self.store.task[i].clone()) else {
            return Err(id);
        };
        let mut visited = std::collections::BTreeSet::new();
        visited.insert(id);
        let mut out = Vec::new();
        if let Some(missing) = plan_deps(self, &task, &mut visited, &mut out) {
            return Err(missing);
        }
        Ok(out)
    }

    /// Run task `id`: start dependencies, reconnect or create the target.
    pub fn run(&self, id: u32) -> Outcome {
        if self.store.find(id).is_none() {
            return Outcome::MissingTask(id);
        }
        let plan_result = self.plan(id);
        if let Err(dep_id) = plan_result {
            return Outcome::DependencyFailed(dep_id, "missing dependency task".to_string());
        }
        let Ok(plan) = plan_result else {
            return Outcome::HostFailed("dependency plan failed".to_string());
        };
        let list_result = (self.host.list)();
        if let Err(e) = list_result {
            return Outcome::HostFailed(e);
        }
        let Ok(sessions) = list_result else {
            return Outcome::HostFailed("session list failed".to_string());
        };
        let Some(target) = plan.last() else {
            return Outcome::MissingTask(id);
        };
        if let Some(alias) = self.live_alias(sessions.as_slice(), target) {
            return Outcome::AlreadyRunning(alias);
        }
        for (i, dep) in plan.iter().enumerate() {
            if i == plan.len() - 1 {
                continue;
            }
            if self.live_alias(sessions.as_slice(), dep).is_none() {
                let dep_result = (self.host.create)(dep);
                if let Err(e) = dep_result {
                    return Outcome::DependencyFailed(dep.id, e);
                }
            }
        }
        let target_result = (self.host.create)(target);
        if let Err(e) = target_result {
            return Outcome::HostFailed(e);
        }
        let Ok(alias) = target_result else {
            return Outcome::HostFailed("session create failed".to_string());
        };
        Outcome::Started(alias)
    }

    /// Stop live task `id`.
    pub fn stop(&self, id: u32) -> Outcome {
        let Some(task) = self.store.find(id).map(|i| self.store.task[i].clone()) else {
            return Outcome::MissingTask(id);
        };
        let list_result = (self.host.list)();
        if let Err(e) = list_result {
            return Outcome::HostFailed(e);
        }
        let Ok(sessions) = list_result else {
            return Outcome::HostFailed("session list failed".to_string());
        };
        let Some(alias) = self.live_alias(sessions.as_slice(), &task) else {
            return Outcome::NotLive(id);
        };
        let stop_result = (self.host.terminate)(alias);
        if let Err(e) = stop_result {
            return Outcome::HostFailed(e);
        }
        Outcome::Stopped(alias)
    }

    /// Stop a live run, then run the task again.
    pub fn restart(&self, id: u32) -> Outcome {
        let Some(task) = self.store.find(id).map(|i| self.store.task[i].clone()) else {
            return Outcome::MissingTask(id);
        };
        let list_result = (self.host.list)();
        if let Err(e) = list_result {
            return Outcome::HostFailed(e);
        }
        let Ok(sessions) = list_result else {
            return Outcome::HostFailed("session list failed".to_string());
        };
        if let Some(alias) = self.live_alias(sessions.as_slice(), &task) {
            let stop_result = (self.host.terminate)(alias);
            if let Err(e) = stop_result {
                return Outcome::HostFailed(e);
            }
        }
        self.run(id)
    }
}

fn plan_deps(
    runner: &TaskRunner,
    task: &Task,
    visited: &mut BTreeSet<u32>,
    out: &mut Vec<Task>,
) -> Option<u32> {
    for dep in task.deps.iter() {
        let dep_id = *dep;
        if visited.contains(&dep_id) {
            continue;
        }
        visited.insert(dep_id);
        let Some(dep_task) = runner
            .store
            .find(dep_id)
            .map(|i| runner.store.task[i].clone())
        else {
            return Some(dep_id);
        };
        if let Some(missing) = plan_deps(runner, &dep_task, visited, out) {
            return Some(missing);
        }
    }
    out.push(task.clone());
    None
}

/// Parse one host `SessionRecord` from the session.list envelope.
#[cfg(windows)]
fn parse_record(v: &Value) -> crate::host::SessionRecord {
    crate::host::SessionRecord {
        id: v.get("id").and_then(Value::as_u64).unwrap_or(0),
        alias: v
            .get("alias")
            .and_then(Value::as_u64)
            .and_then(|a| u32::try_from(a).ok())
            .unwrap_or(0),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        profile: v
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        cwd: v
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        program: v
            .get("program")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        cols: v
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        rows: v
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        pid: v
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        persistent: v.get("persistent").and_then(Value::as_bool).unwrap_or(true),
        state: v
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        created_unix_secs: v
            .get("createdUnixSecs")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        exit_code: v
            .get("exitCode")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        task_id: v
            .get("taskId")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        project: v.get("project").and_then(Value::as_str).map(str::to_string),
    }
}

/// Real host adapter over the session host client.
#[must_use]
#[cfg(windows)]
pub fn real_host_adapter() -> HostAdapter {
    HostAdapter {
        list: Arc::new(move || {
            let Ok(mut client) = crate::host::client::Client::connect_existing() else {
                return Ok(Vec::new());
            };
            let Ok(resp) = client.call("session.list", json!({})) else {
                return Err("session host unavailable".to_string());
            };
            let Some(list) = resp
                .get("result")
                .and_then(|r| r.get("sessions"))
                .and_then(Value::as_array)
            else {
                return Ok(Vec::new());
            };
            let mut out = Vec::new();
            for v in list.iter() {
                out.push(parse_record(v));
            }
            Ok(out)
        }),
        create: Arc::new(move |task| {
            let Ok(mut client) = crate::host::client::Client::connect() else {
                return Err("session host unavailable".to_string());
            };
            let argv = task.argv.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let program = argv[0].clone();
            let mut args = Vec::new();
            for item in argv.iter().skip(1) {
                args.push(item.clone());
            }
            let Ok(resp) = client.call(
                "session.create",
                json!({
                    "profile": task.profile,
                    "name": task.session_label(),
                    "cwd": task.cwd,
                    "program": program,
                    "args": args,
                    "cols": 120,
                    "rows": 30,
                    "persistent": true,
                    "taskId": task.id,
                    "project": task.project,
                }),
            ) else {
                return Err("session host unavailable".to_string());
            };
            if let Some(err) = resp.get("error") {
                let code = err
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("internal");
                let message = err.get("message").and_then(Value::as_str).unwrap_or("");
                return Err(format!("{code}: {message}").to_string());
            }
            let alias = resp
                .get("result")
                .and_then(|r| r.get("session"))
                .and_then(|s| s.get("alias"))
                .and_then(Value::as_u64)
                .and_then(|a| u32::try_from(a).ok());
            if let Some(alias) = alias {
                Ok(alias)
            } else {
                Err("session create returned no alias".to_string())
            }
        }),
        terminate: Arc::new(move |alias| {
            let Ok(mut client) = crate::host::client::Client::connect_existing() else {
                return Err("session host unavailable".to_string());
            };
            let Ok(resp) = client.call("session.terminate", json!({"alias": alias})) else {
                return Err("session host unavailable".to_string());
            };
            if let Some(err) = resp.get("error") {
                let message = err.get("message").and_then(Value::as_str).unwrap_or("");
                return Err(message.to_string());
            }
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::host::SessionRecord;
    use crate::tasks::{Store, Task};

    fn fixture_store() -> Store {
        Store {
            version: 1,
            task: vec![
                Task {
                    id: 1,
                    name: "dev".into(),
                    argv: vec!["npm".into(), "run".into(), "dev".into()],
                    cwd: Some("C:/src/web".into()),
                    profile: None,
                    layout: None,
                    project: None,
                    deps: Vec::new(),
                },
                Task {
                    id: 2,
                    name: "test".into(),
                    argv: vec!["npm".into(), "test".into()],
                    cwd: Some("C:/src/web".into()),
                    profile: None,
                    layout: None,
                    project: None,
                    deps: vec![1],
                },
                Task {
                    id: 3,
                    name: "all".into(),
                    argv: vec!["all".into()],
                    cwd: None,
                    profile: None,
                    layout: None,
                    project: None,
                    deps: vec![2],
                },
                Task {
                    id: 4,
                    name: "bad".into(),
                    argv: vec!["bad".into()],
                    cwd: None,
                    profile: None,
                    layout: None,
                    project: None,
                    deps: vec![99],
                },
            ],
        }
    }

    fn fake_host() -> HostAdapter {
        let state = Arc::new(Mutex::new(Vec::new()));
        let list_state = Arc::clone(&state);
        let create_state = Arc::clone(&state);
        let terminate_state = Arc::clone(&state);
        HostAdapter {
            list: Arc::new(move || {
                let Ok(guard) = list_state.lock() else {
                    return Err("fake host lock failed".to_string());
                };
                let records = guard.clone();
                Ok(records)
            }),
            create: Arc::new(move |task| {
                let Ok(mut guard) = create_state.lock() else {
                    return Err("fake host lock failed".to_string());
                };
                let alias = u32::try_from(guard.len() + 1).unwrap_or(0);
                guard.push(SessionRecord {
                    id: u64::from(alias),
                    alias,
                    name: task.session_label(),
                    profile: "default".to_string(),
                    cwd: task.cwd.clone().unwrap_or_default().to_string(),
                    program: task.argv[0].clone(),
                    cols: 120,
                    rows: 30,
                    pid: alias,
                    persistent: true,
                    state: "live".to_string(),
                    created_unix_secs: 0u64,
                    exit_code: None,
                    task_id: Some(task.id),
                    project: task.project.clone(),
                });
                Ok(alias)
            }),
            terminate: Arc::new(move |alias| {
                let Ok(mut guard) = terminate_state.lock() else {
                    return Err("fake host lock failed".to_string());
                };
                guard.retain(|r| r.alias != alias);
                Ok(())
            }),
        }
    }

    #[test]
    fn missing_task_is_explicit() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let outcome = runner.run(99);
        assert!(matches!(outcome, Outcome::MissingTask(_)));
    }

    #[test]
    fn dependency_failure_is_explicit() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let outcome = runner.run(4);
        assert!(matches!(outcome, Outcome::DependencyFailed(_, _)));
    }

    #[test]
    fn transitive_dependency_plan_is_topological() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let Ok(plan) = runner.plan(3) else {
            panic!("plan must be valid");
        };
        let ids = plan.iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 2, 3], "deps before target");
    }

    #[test]
    fn run_starts_dependencies_then_target() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let outcome = runner.run(2);
        assert!(matches!(outcome, Outcome::Started(_)));
        assert!(matches!(runner.run(2), Outcome::AlreadyRunning(_)));
    }

    #[test]
    fn stop_and_restart_are_identity_based() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let started = runner.run(1);
        assert!(matches!(started, Outcome::Started(_)));
        let stopped = runner.stop(1);
        assert!(matches!(stopped, Outcome::Stopped(_)));
        let restarted = runner.restart(1);
        assert!(matches!(restarted, Outcome::Started(_)));
    }

    #[test]
    fn missing_task_does_not_reuse_another_alias() {
        let store = fixture_store();
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        let started = runner.run(1);
        assert!(matches!(started, Outcome::Started(_)));
        let missing = runner.stop(99);
        assert!(matches!(missing, Outcome::MissingTask(_)));
    }
    #[test]
    fn project_root_isolates_same_name_tasks() {
        let store = Store {
            version: 1,
            task: vec![
                Task {
                    id: 1,
                    name: "dev".into(),
                    argv: vec!["npm".into(), "run".into(), "dev".into()],
                    cwd: Some("C:/repo/wt1".into()),
                    profile: None,
                    layout: None,
                    project: Some("C:/repo/wt1".into()),
                    deps: Vec::new(),
                },
                Task {
                    id: 2,
                    name: "dev".into(),
                    argv: vec!["npm".into(), "run".into(), "dev".into()],
                    cwd: Some("C:/repo/wt2".into()),
                    profile: None,
                    layout: None,
                    project: Some("C:/repo/wt2".into()),
                    deps: Vec::new(),
                },
            ],
        };
        let host = fake_host();
        let runner = TaskRunner::with(&store, &host);
        assert!(matches!(runner.run(1), Outcome::Started(_)));
        assert!(matches!(runner.run(2), Outcome::Started(_)));
        assert!(matches!(runner.stop(1), Outcome::Stopped(_)));
        assert!(
            matches!(runner.run(2), Outcome::AlreadyRunning(_)),
            "wt2 stays live"
        );
    }
}
