//! The `rustty secrets` verbs through the real binary (SECRET-008..010).
//!
//! Every case here stops before `op` could run: `setup` is never invoked, and
//! the others fail validation or find no manifest. Each project folder holds
//! its own `.git`, which pins the project root there, so no manifest in a
//! parent folder of the temp directory can ever be picked up. There is no way
//! to point the binary at a fake `op` by design (SECRET-010), so the paths that
//! do reach `op` are tested in-process with a fake runner.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustty-secrets-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn dir_arg(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rustty(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustty"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_prints_the_usage_and_succeeds() {
    let output = rustty(&["secrets", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("secrets run --only"));
}

#[test]
fn a_malformed_verb_is_usage_and_never_a_window() {
    for args in [
        &["secrets"][..],
        &["secrets", "nope"],
        &["secrets", "run", "--", "cmd"],
        &["secrets", "run", "--only", "A"],
    ] {
        let output = rustty(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(text(&output.stderr).contains("usage"), "{args:?}");
    }
}

#[test]
fn list_of_a_fresh_project_is_an_honest_empty_answer() {
    let project = Project::new();
    let output = rustty(&["secrets", "list", "--dir", project.dir_arg(), "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema"], "secrets-list/v1");
    assert_eq!(json["setUp"], false);
    assert_eq!(json["secrets"].as_array().map(Vec::len), Some(0));
    assert!(
        !json["project"].as_str().unwrap().starts_with(r"\\?\"),
        "no verbatim prefix in what people read: {}",
        json["project"]
    );
    assert!(json["vault"].as_str().unwrap().starts_with("rustty-"));
}

#[test]
fn list_shows_names_and_references_from_the_manifest() {
    let project = Project::new();
    std::fs::create_dir_all(project.path().join(".rustty")).unwrap();
    std::fs::write(
        project.path().join(".rustty/secrets.toml"),
        "[secrets]\nAPI_KEY = \"op://some-vault/API_KEY/password\"\n",
    )
    .unwrap();
    let output = rustty(&["secrets", "list", "--dir", project.dir_arg()]);
    assert_eq!(output.status.code(), Some(0));
    let printed = text(&output.stdout);
    assert!(printed.contains("API_KEY"), "{printed}");
    assert!(
        printed.contains("op://some-vault/API_KEY/password"),
        "{printed}"
    );
}

#[test]
fn a_literal_value_in_the_manifest_is_refused_and_not_repeated() {
    let project = Project::new();
    std::fs::create_dir_all(project.path().join(".rustty")).unwrap();
    std::fs::write(
        project.path().join(".rustty/secrets.toml"),
        "[secrets]\nAPI_KEY = \"sk_live_abc123\"\n",
    )
    .unwrap();
    let output = rustty(&["secrets", "list", "--dir", project.dir_arg()]);
    assert_eq!(output.status.code(), Some(8));
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(!all.contains("sk_live"), "{all}");
    assert!(all.contains("API_KEY"), "{all}");
}

#[test]
fn a_missing_directory_is_not_found() {
    let project = Project::new();
    let missing = project.path().join("nope");
    let output = rustty(&["secrets", "list", "--dir", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn add_refuses_a_literal_value_on_the_command_line_without_repeating_it() {
    let project = Project::new();
    let output = rustty(&[
        "secrets",
        "add",
        "API_KEY",
        "--from",
        "sk_live_abc123",
        "--dir",
        project.dir_arg(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(!all.contains("sk_live"), "{all}");
}

#[test]
fn run_on_a_project_never_set_up_does_not_start_the_command() {
    let project = Project::new();
    let marker = project.path().join("started.txt");
    let script = format!("type nul>{}", marker.display());
    let output = rustty(&[
        "secrets",
        "run",
        "--only",
        "API_KEY",
        "--dir",
        project.dir_arg(),
        "--",
        "cmd",
        "/c",
        &script,
    ]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert!(!marker.exists(), "the command must not have run");
    assert!(text(&output.stderr).contains("rustty secrets setup"));
}

#[test]
fn run_needs_only_names_and_they_must_be_valid() {
    let project = Project::new();
    let output = rustty(&[
        "secrets",
        "run",
        "--only",
        "BAD-NAME",
        "--dir",
        project.dir_arg(),
        "--",
        "cmd",
    ]);
    assert_eq!(output.status.code(), Some(2));
}
