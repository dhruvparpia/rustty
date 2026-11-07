//! Tests for `secrets` (SECRET-001..011). Nothing here touches a real
//! 1Password account: every `op` call goes to a scripted fake. Values are
//! throwaway strings.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A unique, empty temp directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustty-secrets-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- names -------------------------------------------------------------

#[test]
fn names_must_be_env_var_safe() {
    for good in ["API_KEY", "_x", "a1", "GitHubToken", "A"] {
        assert!(validate_name(good).is_ok(), "{good} should be accepted");
    }
    for bad in [
        "", "1A", "A-B", "A B", "A=B", "A;B", "A,B", "A\nB", "A\0B", "é", "A.B",
    ] {
        assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
    }
    assert!(validate_name(&"A".repeat(256)).is_err());
}

#[test]
fn names_that_would_break_the_child_or_op_are_reserved() {
    for bad in [
        "PATH",
        "path",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "OP_SERVICE_ACCOUNT_TOKEN",
        "op_session_x",
        "OP_CONNECT_TOKEN",
    ] {
        assert!(validate_name(bad).is_err(), "{bad} should be reserved");
    }
    assert!(validate_name("OPTIONS").is_ok(), "only the OP_ prefix");
}

// ---- references --------------------------------------------------------

#[test]
fn references_are_validated_against_the_documented_shape() {
    for good in [
        "op://vault/item/field",
        "op://vault/item/section/field",
        "op://My Vault/My Item/password",
        "op://v.1/i_2/f-3",
        "op://v/i/f?attribute=otp",
        "op://v/key/private key?ssh-format=openssh",
    ] {
        assert!(validate_reference(good).is_ok(), "{good} should be valid");
    }
    for bad in [
        "",
        "op://",
        "op://v",
        "op://v/i",
        "op://v//f",
        "op:///i/f",
        "op://v/i/",
        "op://a/b/c/d/e",
        "OP://v/i/f",
        "http://v/i/f",
        "op:/v/i/f",
        " op://v/i/f",
        "op://v/i/f\n",
        "op://v/i/f\"",
        "op://v/i/f;rm",
        "op://v/i/f&x",
        "op://v/i/f?",
        "op://v/i/f?a b=c",
    ] {
        assert!(
            validate_reference(bad).is_err(),
            "{bad:?} should be invalid"
        );
    }
}

#[test]
fn literal_secrets_are_rejected_without_being_echoed() {
    for literal in [
        "sk_live_abc123",
        "ghp_0123456789abcdef",
        "hunter2",
        "postgres://user:pw@host/db",
    ] {
        let message = validate_reference(literal).err().unwrap().to_string();
        assert!(!message.contains(literal), "echoed the value: {message}");
    }
}

#[test]
fn reference_vault_is_the_first_segment() {
    let parts = validate_reference("op://My Vault/item/field").unwrap();
    assert_eq!(parts.vault, "My Vault");
}

// ---- manifest ----------------------------------------------------------

#[test]
fn manifest_parses_and_round_trips_sorted() {
    let text = "[secrets]\nZED = \"op://v/z/password\"\nAPI_KEY = \"op://v/a/password\"\n";
    let manifest = Manifest::parse(text).unwrap();
    let names: Vec<&String> = manifest.secrets.keys().collect();
    assert_eq!(names, ["API_KEY", "ZED"]);
    let rendered = manifest.render().unwrap();
    assert!(rendered.starts_with('#'), "explains itself: {rendered}");
    assert!(rendered.find("API_KEY") < rendered.find("ZED"));
    assert_eq!(Manifest::parse(&rendered).unwrap(), manifest);
}

#[test]
fn empty_manifest_is_valid() {
    assert!(Manifest::parse("").unwrap().secrets.is_empty());
    assert!(Manifest::parse("[secrets]\n").unwrap().secrets.is_empty());
}

#[test]
fn manifest_rejects_a_literal_value_without_echoing_it() {
    let err = Manifest::parse("[secrets]\nAPI_KEY = \"sk_live_abc123\"\n")
        .err()
        .unwrap();
    assert_eq!(err.exit_code(), u32::from(EXIT_MANIFEST));
    let message = err.to_string();
    assert!(message.contains("API_KEY"), "names the entry: {message}");
    assert!(!message.contains("sk_live_abc123"), "{message}");
}

#[test]
fn manifest_syntax_errors_never_quote_the_file() {
    // toml's own error text quotes the offending line, which could hold a
    // pasted secret, so only the line number may come through.
    for text in [
        "[secrets]\nAPI_KEY = sk_live_abc123\n",
        "[secrets]\nAPI_KEY = 12345678\n",
        "API_KEY = \"sk_live_abc123\"\n",
        "[other]\nAPI_KEY = \"sk_live_abc123\"\n",
    ] {
        let err = Manifest::parse(text).err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_MANIFEST));
        let message = err.to_string();
        assert!(!message.contains("sk_live"), "{message}");
        assert!(!message.contains("12345678"), "{message}");
    }
}

#[test]
fn manifest_rejects_invalid_names_and_case_only_duplicates() {
    assert!(Manifest::parse("[secrets]\n\"A-B\" = \"op://v/i/f\"\n").is_err());
    assert!(Manifest::parse("[secrets]\nPATH = \"op://v/i/f\"\n").is_err());
    let dup = "[secrets]\nFOO = \"op://v/i/f\"\nfoo = \"op://v/i/f\"\n";
    assert!(Manifest::parse(dup).is_err(), "Windows env is case-blind");
}

#[test]
fn manifest_load_and_save_use_the_dot_rustty_folder() {
    let dir = TempDir::new();
    assert_eq!(Manifest::load(dir.path()).unwrap(), None);
    let mut manifest = Manifest::default();
    manifest
        .secrets
        .insert("A".to_owned(), "op://v/A/password".to_owned());
    manifest.save(dir.path()).unwrap();
    assert!(dir.path().join(".rustty").join("secrets.toml").is_file());
    assert_eq!(Manifest::load(dir.path()).unwrap(), Some(manifest));
}

// ---- vault naming and project identity ---------------------------------

#[test]
fn fnv_matches_the_published_vector() {
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
}

#[test]
fn vault_name_is_slug_plus_short_hash() {
    let name = vault_name(Path::new(r"C:\Users\me\Projects\Winty Ghost_2"));
    let rest = name.strip_prefix("rustty-winty-ghost-2-").unwrap();
    assert_eq!(rest.len(), 8);
    assert!(rest.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn vault_name_ignores_verbatim_prefix_and_case_but_not_location() {
    let plain = vault_name(Path::new(r"C:\Work\App"));
    assert_eq!(plain, vault_name(Path::new(r"\\?\C:\Work\App")));
    assert_eq!(plain, vault_name(Path::new(r"c:\work\app")));
    assert_eq!(plain, vault_name(Path::new(r"C:\Work\App\")));
    assert_ne!(plain, vault_name(Path::new(r"C:\Other\App")));
    assert_eq!(
        vault_name(Path::new(r"\\?\UNC\srv\share\app")),
        vault_name(Path::new(r"\\srv\share\app"))
    );
}

#[test]
fn vault_name_stays_op_safe_for_awkward_folder_names() {
    for path in [r"C:\Work\---", r"C:\Work\日本語", r"C:\"] {
        let name = vault_name(Path::new(path));
        assert!(name.starts_with("rustty-project-"), "{name}");
    }
    let long = format!(r"C:\Work\{}", "a".repeat(200));
    assert!(vault_name(Path::new(&long)).len() <= 7 + 32 + 1 + 8);
    let name = vault_name(Path::new(r"C:\Work\a:b,c"));
    assert!(!name.contains(':') && !name.contains(','), "{name}");
}

#[test]
fn project_root_finds_the_repository_from_a_subfolder() {
    let dir = TempDir::new();
    let sub = dir.path().join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert_eq!(
        project_root(&sub).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

#[test]
fn project_root_finds_a_manifest_outside_git() {
    let dir = TempDir::new();
    let sub = dir.path().join("a");
    std::fs::create_dir_all(&sub).unwrap();
    Manifest::default().save(dir.path()).unwrap();
    assert_eq!(
        project_root(&sub).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

/// What Git writes in a worktree's folder under `.git/worktrees`: the path of that
/// worktree's `.git` file.
fn write_back_pointer(git_entry: &Path, worktree: &Path) {
    let dot_git = worktree.join(".git").display().to_string();
    std::fs::write(
        git_entry.join("gitdir"),
        format!("{}\n", dot_git.replace('\\', "/")),
    )
    .unwrap();
}

#[test]
fn a_forged_git_file_cant_borrow_another_projects_vault() {
    // Break caught: an agent writing one line in its own folder to get the victim
    // project's vault and token.
    let dir = TempDir::new();
    let victim = dir.path().join("victim");
    let git_entry = victim.join(".git").join("worktrees").join("real");
    std::fs::create_dir_all(&git_entry).unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    write_back_pointer(&git_entry, &real);
    std::fs::write(
        real.join(".git"),
        format!("gitdir: {}\n", git_entry.display()),
    )
    .unwrap();
    // The genuine worktree still resolves.
    assert_eq!(
        project_root(&real).unwrap(),
        std::fs::canonicalize(&victim).unwrap()
    );

    // Forged: names a real worktree's folder, whose back-pointer names someone else.
    let attacker = dir.path().join("attacker");
    std::fs::create_dir_all(&attacker).unwrap();
    std::fs::write(
        attacker.join(".git"),
        format!("gitdir: {}\n", git_entry.display()),
    )
    .unwrap();
    assert_eq!(
        project_root(&attacker).unwrap(),
        std::fs::canonicalize(&attacker).unwrap()
    );
    assert_ne!(
        Project::resolve(Some(&attacker)).unwrap().vault,
        Project::resolve(Some(&victim)).unwrap().vault
    );

    // Forged: names a folder with no back-pointer at all.
    let empty_entry = victim.join(".git").join("worktrees").join("none");
    std::fs::create_dir_all(&empty_entry).unwrap();
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join(".git"),
        format!("gitdir: {}\n", empty_entry.display()),
    )
    .unwrap();
    assert_eq!(
        project_root(&other).unwrap(),
        std::fs::canonicalize(&other).unwrap()
    );
}

#[test]
fn a_worktree_made_by_real_git_still_resolves_to_its_main_checkout() {
    // Break caught: the back-pointer check refusing what Git itself writes.
    let dir = TempDir::new();
    let main = dir.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&main)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
    };
    // No Git on this machine: nothing to compare against.
    if git(&["init", "-q"]).is_err() {
        return;
    }
    let committed = git(&["commit", "-q", "--allow-empty", "--no-gpg-sign", "-m", "x"]).unwrap();
    assert!(committed.status.success(), "{committed:?}");
    let linked = dir.path().join("linked");
    let linked_text = linked.display().to_string();
    let added = git(&["worktree", "add", "-q", &linked_text]).unwrap();
    assert!(added.status.success(), "{added:?}");
    assert_eq!(
        project_root(&linked).unwrap(),
        std::fs::canonicalize(&main).unwrap()
    );
}

#[test]
fn a_linked_worktree_shares_the_main_checkouts_project() {
    let dir = TempDir::new();
    let main = dir.path().join("main");
    let git_entry = main.join(".git").join("worktrees").join("wt");
    std::fs::create_dir_all(&git_entry).unwrap();
    let worktree = dir.path().join("elsewhere").join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    write_back_pointer(&git_entry, &worktree);
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", git_entry.display()),
    )
    .unwrap();
    let main_canonical = std::fs::canonicalize(&main).unwrap();
    assert_eq!(project_root(&worktree).unwrap(), main_canonical);
    assert_eq!(
        Project::resolve(Some(&worktree)).unwrap().vault,
        Project::resolve(Some(&main)).unwrap().vault
    );
}

#[test]
fn a_worktree_git_file_written_with_forward_slashes_also_resolves() {
    // This is how Git itself writes the file on Windows.
    let dir = TempDir::new();
    let main = dir.path().join("main");
    let git_entry = main.join(".git").join("worktrees").join("wt");
    std::fs::create_dir_all(&git_entry).unwrap();
    let worktree = dir.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    write_back_pointer(&git_entry, &worktree);
    let target = git_entry.display().to_string().replace('\\', "/");
    std::fs::write(worktree.join(".git"), format!("gitdir: {target}\n")).unwrap();
    assert_eq!(
        project_root(&worktree).unwrap(),
        std::fs::canonicalize(&main).unwrap()
    );
}

#[test]
fn a_submodule_git_file_stays_its_own_project() {
    let dir = TempDir::new();
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();
    assert_eq!(
        project_root(&sub).unwrap(),
        std::fs::canonicalize(&sub).unwrap()
    );
}

#[test]
fn project_resolve_reports_a_missing_directory_as_not_found() {
    let dir = TempDir::new();
    let err = Project::resolve(Some(&dir.path().join("nope")))
        .err()
        .unwrap();
    assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
}

// ---- verb parsing ------------------------------------------------------

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn verb(args: &[&str]) -> Result<SecretsVerb, String> {
    parse_verb(os(args))
}

#[test]
fn parses_setup_list_and_add() {
    assert_eq!(
        verb(&["setup"]),
        Ok(SecretsVerb::Setup {
            dir: None,
            json: false
        })
    );
    assert_eq!(
        verb(&["setup", "--dir", "D", "--json"]),
        Ok(SecretsVerb::Setup {
            dir: Some(PathBuf::from("D")),
            json: true
        })
    );
    assert_eq!(
        verb(&["list", "--json"]),
        Ok(SecretsVerb::List {
            dir: None,
            json: true
        })
    );
    assert_eq!(
        verb(&["add", "NAME", "--from", "op://P/I/f", "--dir", "D"]),
        Ok(SecretsVerb::Add {
            name: "NAME".to_owned(),
            from: "op://P/I/f".to_owned(),
            dir: Some(PathBuf::from("D")),
            json: false
        })
    );
    assert_eq!(verb(&["--help"]), Ok(SecretsVerb::Help));
    assert_eq!(verb(&["help"]), Ok(SecretsVerb::Help));
}

#[test]
fn parses_run_and_keeps_everything_after_the_separator() {
    assert_eq!(
        verb(&[
            "run", "--only", "A,B", "--dir", "D", "--", "cmd", "--only", "--", "x y"
        ]),
        Ok(SecretsVerb::Run {
            only: vec!["A".to_owned(), "B".to_owned()],
            dir: Some(PathBuf::from("D")),
            argv: os(&["cmd", "--only", "--", "x y"]),
        })
    );
}

#[test]
fn malformed_verbs_are_errors() {
    for args in [
        &[][..],
        &["nope"],
        &["setup", "extra"],
        &["setup", "--dir"],
        &["list", "--bogus"],
        &["add"],
        &["add", "NAME"],
        &["add", "NAME", "--from"],
        &["add", "--from", "op://P/I/f"],
        &["add", "A", "B", "--from", "op://P/I/f"],
        // No default of "all": names are required.
        &["run", "--", "cmd"],
        &["run", "--only", "", "--", "cmd"],
        &["run", "--only", "A,,B", "--", "cmd"],
        &["run", "--only", "A"],
        &["run", "--only", "A", "--"],
        &["run", "--only", "A", "cmd"],
        &["run", "--json", "--only", "A", "--", "cmd"],
    ] {
        assert!(verb(args).is_err(), "{args:?} should be a usage error");
    }
}

#[test]
fn parse_errors_never_echo_what_was_typed() {
    // A stray argument could be a pasted secret.
    for args in [
        &["add", "NAME", "sk_live_abc123", "--from", "op://P/I/f"][..],
        &["setup", "sk_live_abc123"],
        &["sk_live_abc123"],
        &["run", "--only", "A", "sk_live_abc123", "--", "cmd"],
    ] {
        let message = verb(args).err().unwrap();
        assert!(!message.contains("sk_live"), "{message}");
    }
}

// ---- op command lines --------------------------------------------------

#[test]
fn op_command_lines_match_the_help_text_of_op_2_39() {
    assert_eq!(op_vault_get_args("V"), ["vault", "get", "V"]);
    assert_eq!(op_vault_create_args("V"), ["vault", "create", "V"]);
    assert_eq!(
        op_service_account_create_args("V"),
        [
            "service-account",
            "create",
            "V",
            "--vault",
            "V:read_items",
            "--raw"
        ]
    );
    assert_eq!(
        op_read_args("op://V/N/password"),
        ["read", "--no-newline", "op://V/N/password"]
    );
    assert_eq!(
        op_item_create_args("V"),
        ["item", "create", "--vault", "V", "--format", "json", "-"]
    );
}

#[test]
fn stored_reference_points_at_the_copied_field() {
    assert_eq!(stored_reference("V", "NAME"), "op://V/NAME/password");
    assert!(validate_reference(&stored_reference("rustty-a-12345678", "A_B")).is_ok());
}

// ---- Windows: real LockedSecret, fake op -------------------------------

#[cfg(windows)]
mod with_locked_memory {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::*;
    use crate::secure_mem::LockedSecret;

    const TOKEN: &str = "ops_THROWAWAY_TEST_TOKEN_0123456789";

    /// One call as the fake saw it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SeenCall {
        args: Vec<String>,
        token: Option<String>,
        stdin: Option<String>,
    }

    /// What the fake answers.
    #[derive(Clone)]
    struct Reply {
        exit: u32,
        stdout: Vec<u8>,
        stderr: String,
    }

    fn ok(stdout: &[u8]) -> Reply {
        Reply {
            exit: 0,
            stdout: stdout.to_vec(),
            stderr: String::new(),
        }
    }

    fn fail(stderr: &str) -> Reply {
        Reply {
            exit: 1,
            stdout: Vec::new(),
            stderr: stderr.to_owned(),
        }
    }

    /// An approval dialog that always answers the same.
    struct Approves(bool);

    impl CopyApproval for Approves {
        fn approve(&self, _from: &str, _vault: &str, _name: &str) -> bool {
            self.0
        }
    }

    type Script = Box<dyn Fn(&[String]) -> Reply>;

    /// Scripted `op`: records every call, answers from a closure.
    struct FakeOp {
        calls: RefCell<Vec<SeenCall>>,
        script: Script,
    }

    impl FakeOp {
        fn new(script: impl Fn(&[String]) -> Reply + 'static) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                script: Box::new(script),
            }
        }

        /// A healthy account: vaults absent, everything else succeeds.
        fn healthy() -> Self {
            Self::new(|args| match args[0].as_str() {
                "vault" if args[1] == "get" => fail("vault not found"),
                "service-account" => ok(format!("{TOKEN}\r\n").as_bytes()),
                "read" => ok(b"value-for-tests"),
                _ => ok(b"{}"),
            })
        }

        fn calls(&self) -> Vec<SeenCall> {
            self.calls.borrow().clone()
        }

        fn verbs(&self) -> Vec<String> {
            self.calls().iter().map(|c| c.args[..2].join(" ")).collect()
        }
    }

    impl OpRunner for FakeOp {
        fn run(&self, call: &OpCall<'_>) -> Result<OpOutput, SecretsError> {
            let token = call.token.map(|t| {
                t.with_plaintext(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap()
            });
            self.calls.borrow_mut().push(SeenCall {
                args: call.args.clone(),
                token,
                stdin: call.stdin.map(|s| String::from_utf8_lossy(s).into_owned()),
            });
            let reply = (self.script)(&call.args);
            Ok(OpOutput {
                exit_code: reply.exit,
                stdout: LockedSecret::from_slice(&reply.stdout).unwrap(),
                stderr: reply.stderr,
            })
        }
    }

    /// In-memory token store.
    #[derive(Default)]
    struct MemStore {
        tokens: RefCell<HashMap<String, Vec<u8>>>,
        fail_save: bool,
        fail_check: bool,
    }

    impl MemStore {
        fn with_token(vault: &str) -> Self {
            let store = Self::default();
            store
                .tokens
                .borrow_mut()
                .insert(vault.to_owned(), TOKEN.as_bytes().to_vec());
            store
        }
    }

    impl TokenStore for MemStore {
        fn exists(&self, vault: &str) -> bool {
            self.tokens.borrow().contains_key(vault)
        }

        fn check_writable(&self) -> Result<(), SecretsError> {
            if self.fail_check {
                return Err(SecretsError::Store("not writable".to_owned()));
            }
            Ok(())
        }

        fn save(&self, vault: &str, token: &LockedSecret) -> Result<(), SecretsError> {
            if self.fail_save {
                return Err(SecretsError::Store("disk full".to_owned()));
            }
            let bytes = token.with_plaintext(<[u8]>::to_vec).unwrap();
            self.tokens.borrow_mut().insert(vault.to_owned(), bytes);
            Ok(())
        }

        fn load(&self, vault: &str) -> Result<LockedSecret, SecretsError> {
            let tokens = self.tokens.borrow();
            let bytes = tokens
                .get(vault)
                .ok_or_else(|| SecretsError::NotFound("no token".to_owned()))?;
            Ok(LockedSecret::from_slice(bytes).unwrap())
        }
    }

    /// One started command: its argv, and its secrets as name and value.
    type Started = (Vec<OsString>, Vec<(String, String)>);

    /// Records what would be started, and with which values.
    #[derive(Default)]
    struct FakeLauncher {
        started: Rc<RefCell<Vec<Started>>>,
        exit: u32,
    }

    impl FakeLauncher {
        fn started(&self) -> Vec<Started> {
            self.started.borrow().clone()
        }
    }

    impl Launcher for FakeLauncher {
        fn launch(
            &self,
            argv: &[OsString],
            secrets: Vec<(String, LockedSecret)>,
        ) -> Result<u32, SecretsError> {
            let env = secrets
                .iter()
                .map(|(name, value)| {
                    let text = value
                        .with_plaintext(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap();
                    (name.clone(), text)
                })
                .collect();
            self.started.borrow_mut().push((argv.to_vec(), env));
            Ok(self.exit)
        }
    }

    /// A project folder with a manifest of `names`, each pointing at its
    /// own vault's standard reference.
    fn project_with(names: &[&str]) -> (TempDir, Project) {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let mut manifest = Manifest::default();
        for name in names {
            manifest
                .secrets
                .insert((*name).to_owned(), stored_reference(&project.vault, name));
        }
        manifest.save(&project.root).unwrap();
        (dir, project)
    }

    // ---- setup ---------------------------------------------------------

    #[test]
    fn setup_creates_the_vault_and_a_read_only_account_and_stores_the_token() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let store = MemStore::default();
        let outcome = setup(&project, &op, &store).unwrap();
        assert!(!outcome.already_set_up);
        let vault = &project.vault;
        assert_eq!(
            op.calls()
                .iter()
                .map(|c| c.args.clone())
                .collect::<Vec<_>>(),
            vec![
                op_vault_get_args(vault),
                op_vault_create_args(vault),
                op_service_account_create_args(vault),
            ]
        );
        // These run under the owner's session, never a service account.
        assert!(op.calls().iter().all(|c| c.token.is_none()));
        // Trailing CRLF from `--raw` is not part of the token.
        assert_eq!(store.tokens.borrow()[vault], TOKEN.as_bytes());
        assert!(Manifest::load(&project.root).unwrap().is_some());
    }

    #[test]
    fn setup_output_and_project_files_never_contain_the_token() {
        let dir = TempDir::new();
        let op = FakeOp::healthy();
        let store = MemStore::default();
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        for json in [false, true] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = execute(
                &SecretsVerb::Setup {
                    dir: Some(dir.path().to_path_buf()),
                    json,
                },
                &deps,
                &mut out,
                &mut err,
            );
            assert_eq!(code, 0);
            let all = format!(
                "{}{}",
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err)
            );
            assert!(!all.contains(TOKEN), "{all}");
        }
        let manifest = std::fs::read_to_string(dir.path().join(".rustty/secrets.toml")).unwrap();
        assert!(!manifest.contains(TOKEN));
    }

    #[test]
    fn setup_reuses_an_existing_vault() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::new(|args| match args[0].as_str() {
            "service-account" => ok(TOKEN.as_bytes()),
            _ => ok(b"{}"),
        });
        let store = MemStore::default();
        setup(&project, &op, &store).unwrap();
        assert_eq!(op.verbs(), ["vault get", "service-account create"]);
    }

    #[test]
    fn setup_again_is_a_no_op_that_touches_nothing_in_1password() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let store = MemStore::with_token(&project.vault);
        let outcome = setup(&project, &op, &store).unwrap();
        assert!(outcome.already_set_up);
        assert!(op.calls().is_empty());
    }

    #[test]
    fn setup_checks_the_token_store_before_creating_anything() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let store = MemStore {
            fail_check: true,
            ..MemStore::default()
        };
        let err = setup(&project, &op, &store).err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_STORE));
        assert!(op.calls().is_empty(), "the token is only shown once");
    }

    #[test]
    fn setup_refuses_a_broken_manifest_before_creating_anything() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join(".rustty")).unwrap();
        std::fs::write(dir.path().join(".rustty/secrets.toml"), "not toml [").unwrap();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let err = setup(&project, &op, &MemStore::default()).err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_MANIFEST));
        assert!(op.calls().is_empty());
    }

    #[test]
    fn setup_stops_when_the_vault_cannot_be_created() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::new(|_| fail("denied"));
        let store = MemStore::default();
        let err = setup(&project, &op, &store).err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_OP));
        assert_eq!(op.verbs(), ["vault get", "vault create"]);
        assert!(store.tokens.borrow().is_empty());
    }

    #[test]
    fn setup_refuses_an_empty_or_malformed_token() {
        for bad in [&b""[..], b"\r\n", b"two words", b"line\nline"] {
            let dir = TempDir::new();
            let project = Project::resolve(Some(dir.path())).unwrap();
            let reply = ok(bad);
            let op = FakeOp::new(move |args| {
                if args[0] == "service-account" {
                    reply.clone()
                } else {
                    ok(b"{}")
                }
            });
            let store = MemStore::default();
            let err = setup(&project, &op, &store).err().unwrap();
            assert_eq!(err.exit_code(), u32::from(EXIT_OP));
            assert!(store.tokens.borrow().is_empty());
            assert!(!err.to_string().contains("two words"), "{err}");
        }
    }

    #[test]
    fn a_failed_token_save_names_the_orphan_account_but_not_the_token() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let store = MemStore {
            fail_save: true,
            ..MemStore::default()
        };
        let err = setup(&project, &op, &store).err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_STORE));
        let message = err.to_string();
        assert!(message.contains(&project.vault), "{message}");
        assert!(!message.contains(TOKEN), "{message}");
    }

    // ---- add -----------------------------------------------------------

    #[test]
    fn add_copies_over_stdin_only_and_records_the_reference() {
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let secret_value = "ZZMARK\"ss\\wo\nrd-é";
        let op = FakeOp::new(move |args| {
            if args[0] == "read" {
                ok(secret_value.as_bytes())
            } else {
                ok(b"{\"id\":\"x\"}")
            }
        });
        let outcome = add(
            &project,
            "API_KEY",
            "op://Private/Item/field",
            &op,
            &store,
            &Approves(true),
        )
        .unwrap();
        let expected = stored_reference(&project.vault, "API_KEY");
        assert_eq!(outcome.reference, expected);

        let calls = op.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].args, op_read_args("op://Private/Item/field"));
        assert_eq!(calls[1].args, op_item_create_args(&project.vault));
        // Under the owner's session: the service account is read-only.
        assert!(calls.iter().all(|c| c.token.is_none()));
        // The value is on stdin, and on no command line anywhere.
        assert!(
            calls
                .iter()
                .all(|c| c.args.iter().all(|a| !a.contains("ZZMARK")))
        );
        let template: serde_json::Value =
            serde_json::from_str(calls[1].stdin.as_deref().unwrap()).unwrap();
        assert_eq!(template["title"], "API_KEY");
        assert_eq!(template["category"], "LOGIN");
        let field = &template["fields"][0];
        assert_eq!(field["id"], "password");
        assert_eq!(field["type"], "CONCEALED");
        assert_eq!(field["purpose"], "PASSWORD");
        assert_eq!(field["label"], "password");
        assert_eq!(field["value"], secret_value);

        let manifest = Manifest::load(&project.root).unwrap().unwrap();
        assert_eq!(manifest.secrets["API_KEY"], expected);
        let text = std::fs::read_to_string(project.root.join(".rustty/secrets.toml")).unwrap();
        assert!(
            !text.contains("ZZMARK"),
            "the manifest holds no values: {text}"
        );
    }

    #[test]
    fn add_output_never_shows_the_value() {
        let (dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|args| {
            if args[0] == "read" {
                ok(b"value-for-tests")
            } else {
                // `op item create` echoes the item: it must not be forwarded.
                ok(b"{\"value\":\"value-for-tests\"}")
            }
        });
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &SecretsVerb::Add {
                name: "A".to_owned(),
                from: "op://Private/I/f".to_owned(),
                dir: Some(dir.path().to_path_buf()),
                json: true,
            },
            &deps,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        let printed = String::from_utf8_lossy(&out);
        assert!(!printed.contains("value-for-tests"), "{printed}");
        let json: serde_json::Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(json["schema"], "secrets-add/v1");
        assert_eq!(json["name"], "A");
    }

    #[test]
    fn add_rejects_bad_input_before_calling_op() {
        let (_dir, project) = project_with(&["TAKEN"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        for (name, from) in [
            ("BAD-NAME", "op://Private/I/f"),
            ("PATH", "op://Private/I/f"),
            ("OK", "sk_live_abc123"),
            ("TAKEN", "op://Private/I/f"),
            ("taken", "op://Private/I/f"),
        ] {
            let err = add(&project, name, from, &op, &store, &Approves(true))
                .err()
                .unwrap();
            assert_eq!(
                err.exit_code(),
                u32::from(crate::host::EXIT_USAGE),
                "{name}"
            );
            assert!(!err.to_string().contains("sk_live"), "{err}");
        }
        assert!(op.calls().is_empty());
    }

    #[test]
    fn add_refuses_a_source_already_in_the_project_vault() {
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let from = format!("op://{}/Other/password", project.vault);
        let err = add(&project, "A", &from, &op, &store, &Approves(true))
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_USAGE));
        assert!(op.calls().is_empty());
    }

    #[test]
    fn declining_the_dialog_reads_nothing_and_has_a_stable_code() {
        // Break caught: an agent copying any secret the person's own session can
        // read, with nobody asked.
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let err = add(
            &project,
            "A",
            "op://Private/Item/field",
            &op,
            &store,
            &Approves(false),
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_DECLINED));
        assert_eq!(
            err.exit_code(),
            13,
            "a public contract: scripts branch on it"
        );
        assert!(op.calls().is_empty(), "{:?}", op.calls().len());
        assert!(
            Manifest::load(&project.root)
                .unwrap()
                .unwrap()
                .secrets
                .is_empty()
        );
    }

    #[test]
    fn the_dialog_names_the_source_and_the_destination_before_anything_is_read() {
        struct Asked<'a> {
            op: &'a FakeOp,
            seen: RefCell<Vec<(String, String, String, usize)>>,
        }
        impl CopyApproval for Asked<'_> {
            fn approve(&self, from: &str, vault: &str, name: &str) -> bool {
                self.seen.borrow_mut().push((
                    from.to_owned(),
                    vault.to_owned(),
                    name.to_owned(),
                    self.op.calls().len(),
                ));
                true
            }
        }
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|args| {
            if args[0] == "read" {
                ok(b"value")
            } else {
                ok(b"{\"id\":\"x\"}")
            }
        });
        let asked = Asked {
            op: &op,
            seen: RefCell::new(Vec::new()),
        };
        let from = "op://Private/Item/field";
        add(&project, "A", from, &op, &store, &asked).unwrap();
        let seen = asked.seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, from);
        assert_eq!(seen[0].1, project.vault);
        assert_eq!(seen[0].2, "A");
        assert_eq!(seen[0].3, 0, "op ran before the person was asked");
        let text = copy_prompt(from, &project.vault, "A");
        assert!(
            text.contains(from) && text.contains(&project.vault),
            "{text}"
        );
    }

    #[test]
    fn a_refused_add_never_gets_as_far_as_asking() {
        struct Never;
        impl CopyApproval for Never {
            fn approve(&self, _from: &str, _vault: &str, _name: &str) -> bool {
                panic!("nothing valid to ask about");
            }
        }
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let err = add(
            &project,
            "A",
            "op://P/I/f",
            &op,
            &MemStore::default(),
            &Never,
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
    }

    #[test]
    fn add_needs_setup_first() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let err = add(
            &project,
            "A",
            "op://P/I/f",
            &op,
            &MemStore::default(),
            &Approves(true),
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
        assert!(op.calls().is_empty());
    }

    #[test]
    fn add_stops_when_the_source_is_unreadable_or_empty() {
        for reply in [fail("no such item"), ok(b""), ok(b"\r\n"), ok(b"  ")] {
            let (_dir, project) = project_with(&[]);
            let store = MemStore::with_token(&project.vault);
            let op = FakeOp::new(move |_| reply.clone());
            let err = add(&project, "A", "op://P/I/f", &op, &store, &Approves(true))
                .err()
                .unwrap();
            assert_eq!(err.exit_code(), u32::from(EXIT_OP));
            assert_eq!(op.calls().len(), 1, "nothing is created");
            let manifest = Manifest::load(&project.root).unwrap().unwrap();
            assert!(manifest.secrets.is_empty());
        }
    }

    #[test]
    fn add_refuses_a_value_that_is_not_utf8() {
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|_| ok(&[0xff, 0xfe, 0x41]));
        let err = add(&project, "A", "op://P/I/f", &op, &store, &Approves(true))
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_OP));
        assert_eq!(op.calls().len(), 1);
    }

    #[test]
    fn a_failed_item_create_does_not_touch_the_manifest_or_leak_the_value() {
        let (_dir, project) = project_with(&[]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|args| {
            if args[0] == "read" {
                ok(b"value-for-tests")
            } else {
                fail("rejected value-for-tests")
            }
        });
        let err = add(&project, "A", "op://P/I/f", &op, &store, &Approves(true))
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_OP));
        assert!(!err.to_string().contains("value-for-tests"), "{err}");
        let manifest = Manifest::load(&project.root).unwrap().unwrap();
        assert!(manifest.secrets.is_empty());
    }

    // ---- list ----------------------------------------------------------

    #[test]
    fn list_shows_names_and_references_and_calls_nothing() {
        let (dir, project) = project_with(&["A", "B"]);
        let op = FakeOp::healthy();
        let store = MemStore::with_token(&project.vault);
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &SecretsVerb::List {
                dir: Some(dir.path().to_path_buf()),
                json: true,
            },
            &deps,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(op.calls().is_empty());
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["schema"], "secrets-list/v1");
        assert_eq!(json["vault"].as_str(), Some(project.vault.as_str()));
        assert_eq!(json["setUp"], true);
        assert_eq!(json["secrets"][0]["name"], "A");
        assert_eq!(
            json["secrets"][1]["reference"].as_str(),
            Some(stored_reference(&project.vault, "B").as_str())
        );

        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &SecretsVerb::List {
                dir: Some(dir.path().to_path_buf()),
                json: false,
            },
            &deps,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains('A') && text.contains("op://"), "{text}");
    }

    #[test]
    fn list_of_a_project_never_set_up_is_an_honest_empty_answer() {
        let dir = TempDir::new();
        let op = FakeOp::healthy();
        let store = MemStore::default();
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &SecretsVerb::List {
                dir: Some(dir.path().to_path_buf()),
                json: true,
            },
            &deps,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["setUp"], false);
        assert_eq!(json["secrets"].as_array().map(Vec::len), Some(0));
    }

    // ---- run -----------------------------------------------------------

    fn argv() -> Vec<OsString> {
        vec![OsString::from("cmd"), OsString::from("arg")]
    }

    fn only(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn run_fetches_only_the_named_secrets_with_the_token_and_returns_the_exit_code() {
        let (_dir, project) = project_with(&["A", "B", "C"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|args| {
            if args[2].ends_with("/A/password") {
                ok(b"value-a")
            } else {
                ok(b"value-c\r\n")
            }
        });
        let launcher = FakeLauncher {
            exit: 42,
            ..FakeLauncher::default()
        };
        let code = run(
            &project,
            &only(&["A", "C"]),
            &argv(),
            &op,
            &store,
            &launcher,
        )
        .unwrap();
        assert_eq!(code, 42);

        let calls = op.calls();
        assert_eq!(calls.len(), 2, "B is never fetched");
        for (call, name) in calls.iter().zip(["A", "C"]) {
            assert_eq!(
                call.args,
                op_read_args(&stored_reference(&project.vault, name))
            );
            // The token reaches only the op child, and never via arguments.
            assert_eq!(call.token.as_deref(), Some(TOKEN));
            assert!(call.args.iter().all(|a| !a.contains(TOKEN)));
            assert_eq!(call.stdin, None);
        }
        let started = launcher.started();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].0, argv());
        assert_eq!(
            started[0].1,
            vec![
                ("A".to_owned(), "value-a".to_owned()),
                ("C".to_owned(), "value-c".to_owned()),
            ],
            "only the requested names, and the line end is not part of the value"
        );
    }

    #[test]
    fn run_passes_a_32_bit_exit_code_through() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher {
            exit: 0xC000_0005,
            ..FakeLauncher::default()
        };
        let code = run(&project, &only(&["A"]), &argv(), &op, &store, &launcher).unwrap();
        assert_eq!(code, 0xC000_0005);
    }

    #[test]
    fn run_dedupes_repeated_names() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        run(
            &project,
            &only(&["A", "A"]),
            &argv(),
            &op,
            &store,
            &launcher,
        )
        .unwrap();
        assert_eq!(op.calls().len(), 1);
        assert_eq!(launcher.started()[0].1.len(), 1);
    }

    #[test]
    fn run_has_no_default_of_all() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        let err = run(&project, &[], &argv(), &op, &store, &launcher)
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_USAGE));
        assert!(op.calls().is_empty());
        assert!(launcher.started().is_empty());
    }

    #[test]
    fn run_needs_a_command() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        let err = run(&project, &only(&["A"]), &[], &op, &store, &launcher)
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_USAGE));
        assert!(op.calls().is_empty());
    }

    #[test]
    fn an_unknown_name_stops_run_before_any_secret_is_fetched() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        let err = run(
            &project,
            &only(&["A", "MISSING"]),
            &argv(),
            &op,
            &store,
            &launcher,
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
        assert!(err.to_string().contains("MISSING"));
        assert!(op.calls().is_empty());
        assert!(launcher.started().is_empty());
    }

    #[test]
    fn run_without_a_manifest_or_token_is_not_found() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        let err = run(
            &project,
            &only(&["A"]),
            &argv(),
            &op,
            &MemStore::default(),
            &launcher,
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));

        let (_dir, project) = project_with(&["A"]);
        let err = run(
            &project,
            &only(&["A"]),
            &argv(),
            &op,
            &MemStore::default(),
            &launcher,
        )
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
        assert!(op.calls().is_empty());
        assert!(launcher.started().is_empty());
    }

    #[test]
    fn run_refuses_a_reference_outside_the_project_vault() {
        let dir = TempDir::new();
        let project = Project::resolve(Some(dir.path())).unwrap();
        let mut manifest = Manifest::default();
        manifest
            .secrets
            .insert("A".to_owned(), "op://Private/Item/field".to_owned());
        manifest.save(&project.root).unwrap();
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::healthy();
        let launcher = FakeLauncher::default();
        let err = run(&project, &only(&["A"]), &argv(), &op, &store, &launcher)
            .err()
            .unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_MANIFEST));
        assert!(op.calls().is_empty());
        assert!(launcher.started().is_empty());
    }

    #[test]
    fn run_does_not_start_the_command_when_a_secret_is_empty_or_unreadable() {
        let bad_replies = [
            fail("item not found"),
            ok(b""),
            ok(b"\r\n"),
            ok(b"   "),
            ok(&[0xff, 0xfe]),
            ok(b"a\0b"),
            ok(&vec![b'x'; 32_768]),
        ];
        for reply in bad_replies {
            let (_dir, project) = project_with(&["A", "B"]);
            let store = MemStore::with_token(&project.vault);
            // The first secret is fine, the second is bad: nothing starts.
            let op = FakeOp::new(move |args| {
                if args[2].ends_with("/A/password") {
                    ok(b"fine")
                } else {
                    reply.clone()
                }
            });
            let launcher = FakeLauncher::default();
            let err = run(
                &project,
                &only(&["A", "B"]),
                &argv(),
                &op,
                &store,
                &launcher,
            )
            .err()
            .unwrap();
            assert_eq!(err.exit_code(), u32::from(EXIT_OP));
            assert!(err.to_string().contains('B'), "names the secret: {err}");
            assert!(launcher.started().is_empty());
        }
    }

    #[test]
    fn op_stderr_that_contains_the_token_is_withheld() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let op = FakeOp::new(|_| fail(&format!("bad token {TOKEN}")));
        let launcher = FakeLauncher::default();
        let err = run(&project, &only(&["A"]), &argv(), &op, &store, &launcher)
            .err()
            .unwrap();
        assert!(!err.to_string().contains(TOKEN), "{err}");
    }

    #[test]
    fn op_stderr_is_cleaned_of_control_characters_and_capped() {
        let (_dir, project) = project_with(&["A"]);
        let store = MemStore::with_token(&project.vault);
        let long = format!("\u{1b}[31mboom\nline2 {}", "x".repeat(5000));
        let op = FakeOp::new(move |_| fail(&long));
        let launcher = FakeLauncher::default();
        let err = run(&project, &only(&["A"]), &argv(), &op, &store, &launcher)
            .err()
            .unwrap();
        let message = err.to_string();
        assert!(
            !message.contains('\u{1b}') && !message.contains('\n'),
            "{message}"
        );
        assert!(message.contains("boom"));
        assert!(message.len() < 1000, "{}", message.len());
    }

    // ---- execute and the file token store ------------------------------

    #[test]
    fn execute_maps_errors_to_exit_codes_on_stderr() {
        let dir = TempDir::new();
        let op = FakeOp::healthy();
        let store = MemStore::default();
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &SecretsVerb::Run {
                only: only(&["A"]),
                dir: Some(dir.path().to_path_buf()),
                argv: argv(),
            },
            &deps,
            &mut out,
            &mut err,
        );
        assert_eq!(code, u32::from(crate::host::EXIT_NOT_FOUND));
        assert!(out.is_empty());
        assert!(String::from_utf8_lossy(&err).starts_with("rustty: "));
    }

    #[test]
    fn execute_prints_help() {
        let op = FakeOp::healthy();
        let store = MemStore::default();
        let launcher = FakeLauncher::default();
        let deps = Deps {
            op: &op,
            store: &store,
            launcher: &launcher,
            approval: &Approves(true),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(&SecretsVerb::Help, &deps, &mut out, &mut err);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).contains("secrets run --only"));
    }

    #[test]
    fn file_token_store_round_trips_under_dpapi_and_hides_the_value() {
        let dir = TempDir::new();
        let store = FileTokenStore::new(dir.path().join("secrets"));
        let vault = "rustty-app-12345678";
        assert!(!store.exists(vault));
        store.check_writable().unwrap();
        let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
        store.save(vault, &token).unwrap();
        assert!(store.exists(vault));

        let path = dir.path().join("secrets").join(format!("{vault}.token"));
        let on_disk = std::fs::read(&path).unwrap();
        assert!(
            !on_disk.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
            "plaintext on disk"
        );
        let back = store.load(vault).unwrap();
        assert_eq!(
            back.with_plaintext(<[u8]>::to_vec).unwrap(),
            TOKEN.as_bytes()
        );
    }

    #[test]
    fn file_token_store_binds_a_token_to_its_project() {
        let dir = TempDir::new();
        let store = FileTokenStore::new(dir.path().to_path_buf());
        let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
        store.save("rustty-a-11111111", &token).unwrap();
        std::fs::copy(
            dir.path().join("rustty-a-11111111.token"),
            dir.path().join("rustty-b-22222222.token"),
        )
        .unwrap();
        let err = store.load("rustty-b-22222222").err().unwrap();
        assert_eq!(err.exit_code(), u32::from(EXIT_STORE));
    }

    #[test]
    fn file_token_store_reports_a_missing_token_as_not_found() {
        let dir = TempDir::new();
        let store = FileTokenStore::new(dir.path().to_path_buf());
        let err = store.load("rustty-a-11111111").err().unwrap();
        assert_eq!(err.exit_code(), u32::from(crate::host::EXIT_NOT_FOUND));
    }

    #[test]
    fn file_token_store_refuses_a_name_that_could_escape_its_folder() {
        let dir = TempDir::new();
        let store = FileTokenStore::new(dir.path().join("secrets"));
        let token = LockedSecret::from_slice(TOKEN.as_bytes()).unwrap();
        for bad in [r"..\evil", "../evil", "a/b", "", "C:evil"] {
            assert!(store.save(bad, &token).is_err(), "{bad}");
            assert!(store.load(bad).is_err(), "{bad}");
            assert!(!store.exists(bad), "{bad}");
        }
    }
}
