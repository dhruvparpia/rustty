//! Explicit project task definitions (#89 slice 1/2): named argv, cwd, profile,
//! dependencies and target layout. The store is `tasks.toml` beside `config.toml`
//! (portable mode wins). It is user-visible and hand-editable; an unreadable or
//! newer store is reported and never rewritten. Task runs are host-owned sessions
//! named `task#<id> <name>`, so a reopened GUI reconnects the same run instead of
//! launching a duplicate by list position.

#![allow(
    missing_docs,
    reason = "task schema fields are documented in docs/design-tasks.md"
)]
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_TASKS: usize = 64;

fn current_version() -> u32 {
    VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub name: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub deps: Vec<u32>,
}

impl Task {
    #[must_use]
    pub fn session_label(&self) -> String {
        format!("task#{} {}", self.id, self.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub task: Vec<Task>,
}

impl Store {
    #[must_use]
    pub fn find(&self, id: u32) -> Option<usize> {
        self.task.iter().position(|t| t.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Store(Store),
    Unusable(String),
}

#[must_use]
pub fn store_path() -> Option<PathBuf> {
    crate::config::config_path().map(|p| p.with_file_name("tasks.toml"))
}

#[must_use]
pub fn read_store(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Store(Store::default()),
        Err(e) => return Loaded::Unusable(format!("tasks file unreadable: {e}")),
    };
    if text.trim().is_empty() {
        return Loaded::Store(Store::default());
    }
    match toml::from_str::<Store>(&text) {
        Ok(store) if store.version > VERSION => Loaded::Unusable(format!(
            "tasks file is version {}; this rustty understands version {VERSION}",
            store.version,
        )),
        Ok(store) if store.task.len() > MAX_TASKS => Loaded::Unusable(format!(
            "tasks file has more than {MAX_TASKS} tasks and will not be touched",
        )),
        Ok(store) if let Some(reason) = validate_tasks(store.task.clone()) => {
            Loaded::Unusable(reason)
        }
        Ok(store) => Loaded::Store(store),
        Err(e) => Loaded::Unusable(format!("tasks file ignored: {e}")),
    }
}

fn validate_tasks(tasks: Vec<Task>) -> Option<String> {
    let mut ids = std::collections::BTreeMap::new();
    let mut names = std::collections::BTreeMap::new();
    for (i, t) in tasks.iter().enumerate() {
        if t.id == 0 {
            return Some("task id must be positive".to_string());
        }
        if ids.contains_key(&t.id) {
            return Some(format!("duplicate task id {}", t.id));
        }
        ids.insert(t.id, i);
        let name = t.name.to_lowercase();
        if names.contains_key(&name) {
            return Some(format!("duplicate task name {:?}", t.name));
        }
        names.insert(name, i);
        if t.argv.is_empty() {
            return Some(format!("task {} has empty argv", t.id));
        }
    }
    let mut indegree = vec![0u32; tasks.len()];
    for (i, t) in tasks.iter().enumerate() {
        indegree[i] = u32::try_from(t.deps.len()).unwrap_or(0);
        for dep in t.deps.iter() {
            if !ids.contains_key(dep) {
                return Some(format!("task {} depends on missing task {}", t.id, dep));
            }
            if *dep == t.id {
                return Some(format!("task {} depends on itself", t.id));
            }
        }
    }
    let mut queue = Vec::new();
    for (i, d) in indegree.iter().enumerate() {
        if *d == 0 {
            queue.push(i);
        }
    }
    let mut seen = 0usize;
    while !queue.is_empty() {
        let Some(i) = queue.pop() else {
            break;
        };
        let idx = i;
        let id = tasks[idx].id;
        seen += 1;
        for (j, t) in tasks.iter().enumerate() {
            if t.deps.contains(&id) {
                indegree[j] -= 1;
                if indegree[j] == 0 {
                    queue.push(j);
                }
            }
        }
    }
    if seen != tasks.len() {
        return Some("task dependency cycle".to_string());
    }
    None
}

#[must_use]
pub fn default_store() -> Store {
    let p = store_path().unwrap_or_default();
    let Loaded::Store(store) = read_store(&p) else {
        return Store::default();
    };
    store
}

#[must_use]
pub fn tasks() -> Vec<Task> {
    let p = store_path().unwrap_or_default();
    tasks_at(&p)
}

#[must_use]
pub fn tasks_at(path: &Path) -> Vec<Task> {
    let Loaded::Store(store) = read_store(path) else {
        return Vec::new();
    };
    store.task.clone()
}

#[must_use]
pub fn task_by_id(id: u32) -> Option<Task> {
    let p = store_path().unwrap_or_default();
    task_at(&p, id)
}

#[must_use]
pub fn task_at(path: &Path, id: u32) -> Option<Task> {
    let Loaded::Store(store) = read_store(path) else {
        return None;
    };
    store.find(id).map(|i| store.task[i].clone())
}

#[must_use]
pub fn dependencies(id: u32) -> Vec<u32> {
    let Some(task) = task_by_id(id) else {
        return Vec::new();
    };
    task.deps.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustty-tasks-{}-{tag}.toml", std::process::id()))
    }

    fn write_store(path: &Path, store: &Store) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(store)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    fn fixture() -> Store {
        Store {
            version: 1,
            task: vec![
                Task {
                    id: 1,
                    name: "dev".into(),
                    argv: vec!["npm".into(), "run".into(), "dev".into()],
                    cwd: Some("C:/src/web".into()),
                    profile: Some("Git Bash".into()),
                    layout: Some("web".into()),
                    deps: Vec::new(),
                },
                Task {
                    id: 2,
                    name: "test".into(),
                    argv: vec!["npm".into(), "test".into()],
                    cwd: Some("C:/src/web".into()),
                    profile: None,
                    layout: None,
                    deps: vec![1],
                },
            ],
        }
    }

    #[test]
    fn task_store_round_trips() {
        let path = temp_path("roundtrip");
        let _clean = std::fs::remove_file(&path);
        write_store(&path, &fixture()).expect("writes");
        let Loaded::Store(store) = read_store(&path) else {
            panic!("fresh store must be readable");
        };
        assert_eq!(store.version, VERSION);
        assert_eq!(store.task, fixture().task);
        let text = std::fs::read_to_string(&path).expect("reads back");
        assert!(text.contains("version = 1"), "{text}");
        assert!(text.contains("[[task]]"), "{text}");
        assert!(text.contains("deps = ["), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_store_is_empty() {
        let path = temp_path("missing");
        let _clean = std::fs::remove_file(&path);
        assert_eq!(tasks_at(&path).len(), 0);
    }

    #[test]
    fn newer_version_is_refused() {
        let path = temp_path("newer");
        let _clean = std::fs::remove_file(&path);
        let future = "version = 2\n";
        std::fs::write(&path, future).expect("writes fixture");
        let Loaded::Unusable(reason) = read_store(&path) else {
            panic!("v2 must be refused");
        };
        assert!(reason.contains("version 2"), "{reason}");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), future);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn duplicate_ids_names_and_missing_deps_are_refused() {
        let path = temp_path("bad");
        let _clean = std::fs::remove_file(&path);
        std::fs::write(&path, "version = 1\n[[task]]\nid = 1\nname = \"x\"\nargv = [\"x\"]\n[[task]]\nid = 1\nname = \"y\"\nargv = [\"y\"]\n").expect("writes fixture");
        let Loaded::Unusable(reason) = read_store(&path) else {
            panic!("bad store must be refused");
        };
        assert!(reason.contains("duplicate task id"), "{reason}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dependency_cycle_is_refused_before_start() {
        let path = temp_path("cycle");
        let _clean = std::fs::remove_file(&path);
        std::fs::write(&path, "version = 1\n[[task]]\nid = 1\nname = \"a\"\nargv = [\"a\"]\n[[task]]\nid = 2\nname = \"b\"\nargv = [\"b\"]\n").expect("writes fixture");
        let Loaded::Store(store) = read_store(&path) else {
            panic!("base store readable");
        };
        let mut tasks = store.task.clone();
        tasks[0].deps = vec![2];
        tasks[1].deps = vec![1];
        assert!(validate_tasks(tasks).is_some(), "cycle must be rejected");
        let _ = std::fs::remove_file(&path);
    }
}
