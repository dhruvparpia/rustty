//! Per-project 1Password secrets (SECRET-001..012): the manifest, vault
//! naming, the token store and the logic of the `rustty secrets` verbs.
//!
//! Portable. Everything that touches `op`, the token file or the started
//! command goes through a trait ([`OpRunner`], [`TokenStore`], [`Launcher`]),
//! so tests inject fakes as parameters. There is deliberately no environment
//! or config override for the path of `op` (SECRET-010): an agent could point
//! it at its own program and receive the token.
//!
//! This limits damage, it does not prevent it (SECRET-012): code running as
//! the same Windows user can still reach these secrets, and a command given a
//! secret can print it.
//!
//! No message in this module quotes a value, a token or a stray argument.
//! Messages name secrets and references only.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::host::{EXIT_NOT_FOUND, EXIT_USAGE};
use crate::secure_mem::{LockedSecret, dpapi_protect, dpapi_unprotect};

/// Exit code: the manifest is unreadable, invalid or points outside the
/// project's vault. (2 and 3 are the shared usage and not-found codes.)
pub const EXIT_MANIFEST: u8 = 8;
/// Exit code: `op` is missing, failed, or returned an unusable value.
pub const EXIT_OP: u8 = 9;
/// Exit code: the token store could not save or decrypt the token.
pub const EXIT_STORE: u8 = 10;
/// Exit code: secure memory could not be allocated, locked or encrypted.
pub const EXIT_MEMORY: u8 = 11;
/// Exit code: the command given to `secrets run` could not be started.
pub const EXIT_LAUNCH: u8 = 12;

/// Longest secret name accepted.
const MAX_NAME_LEN: usize = 255;
/// Longest environment variable value Windows allows, in UTF-16 units.
const MAX_ENV_VALUE_UNITS: usize = 32_767;
/// How much of `op`'s error output is repeated to the user.
const MAX_STDERR_CHARS: usize = 400;
/// Longest slug in a vault name.
const MAX_SLUG_LEN: usize = 32;
/// Variables a secret may not replace: the started program depends on them.
const RESERVED_NAMES: [&str; 5] = ["PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC"];

/// A failed secrets verb. Each variant has a stable exit code; the text never
/// contains a value.
#[derive(Debug)]
pub enum SecretsError {
    /// Bad arguments, an invalid name or reference (exit 2).
    Usage(String),
    /// No manifest, unknown name, or project not set up (exit 3).
    NotFound(String),
    /// Manifest unreadable or invalid (exit [`EXIT_MANIFEST`]).
    Manifest(String),
    /// `op` failed or returned something unusable (exit [`EXIT_OP`]).
    Op(String),
    /// Token store failure (exit [`EXIT_STORE`]).
    Store(String),
    /// Secure memory failure (exit [`EXIT_MEMORY`]).
    Memory(String),
    /// The command could not be started (exit [`EXIT_LAUNCH`]).
    Launch(String),
}

impl SecretsError {
    /// The stable process exit code for this failure.
    #[must_use]
    pub fn exit_code(&self) -> u32 {
        u32::from(match self {
            Self::Usage(_) => EXIT_USAGE,
            Self::NotFound(_) => EXIT_NOT_FOUND,
            Self::Manifest(_) => EXIT_MANIFEST,
            Self::Op(_) => EXIT_OP,
            Self::Store(_) => EXIT_STORE,
            Self::Memory(_) => EXIT_MEMORY,
            Self::Launch(_) => EXIT_LAUNCH,
        })
    }
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::Usage(message)
        | Self::NotFound(message)
        | Self::Manifest(message)
        | Self::Op(message)
        | Self::Store(message)
        | Self::Memory(message)
        | Self::Launch(message)) = self;
        f.write_str(message)
    }
}

impl std::error::Error for SecretsError {}

/// Wrap a secure-memory failure.
fn memory_error(error: &io::Error) -> SecretsError {
    SecretsError::Memory(format!("secure memory failed: {error}"))
}

/// SECRET-002: check a secret name: safe as an environment variable on
/// Windows, and not one the started program or `op` depends on.
///
/// # Errors
/// [`SecretsError::Usage`], without quoting the name.
pub fn validate_name(name: &str) -> Result<(), SecretsError> {
    let mut chars = name.chars();
    let shape_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= MAX_NAME_LEN;
    if !shape_ok {
        return Err(SecretsError::Usage(
            "a secret name uses letters, digits and _, and does not start with a digit".to_owned(),
        ));
    }
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("OP_") || RESERVED_NAMES.contains(&upper.as_str()) {
        return Err(SecretsError::Usage(
            "that secret name is reserved: OP_* configures 1Password, and PATH, PATHEXT, \
             SYSTEMROOT, WINDIR and COMSPEC are needed to start a program"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The parts of a valid `op://` reference that the verbs need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceParts {
    /// The first segment: the vault name or ID.
    pub vault: String,
}

/// True for the characters 1Password documents for a reference segment.
fn is_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')
}

/// SECRET-002: check `op://<vault>/<item>/[section/]<field>[?query]` against
/// the documented syntax. This is also what keeps a literal secret out of the
/// manifest: anything else is refused, and the message does not quote it.
///
/// # Errors
/// [`SecretsError::Usage`], without quoting the value.
pub fn validate_reference(reference: &str) -> Result<ReferenceParts, SecretsError> {
    let invalid = || {
        SecretsError::Usage(
            "expected a reference like op://vault/item/field. Only references are accepted here, \
             never secret values"
                .to_owned(),
        )
    };
    let body = reference.strip_prefix("op://").ok_or_else(invalid)?;
    let (path, query) = match body.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (body, None),
    };
    if let Some(query) = query {
        let query_ok = !query.is_empty()
            && query
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '=' | '&'));
        if !query_ok {
            return Err(invalid());
        }
    }
    let segments: Vec<&str> = path.split('/').collect();
    let shape_ok = matches!(segments.len(), 3 | 4)
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(is_segment_char));
    if !shape_ok {
        return Err(invalid());
    }
    Ok(ReferenceParts {
        vault: segments[0].to_owned(),
    })
}

/// The manifest as it sits in TOML.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    /// NAME to `op://` reference.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// SECRET-001: `.rustty/secrets.toml`: a map from NAME to `op://` reference.
/// It holds no values.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// NAME to `op://` reference, sorted by name.
    pub secrets: BTreeMap<String, String>,
}

/// The manifest's path under a project root.
fn manifest_path(root: &Path) -> PathBuf {
    root.join(".rustty").join("secrets.toml")
}

impl Manifest {
    /// Parse and validate manifest text.
    ///
    /// Errors never quote the file: a pasted secret could be on the offending
    /// line, and `toml`'s own message would repeat it, so only a line number
    /// comes through.
    ///
    /// # Errors
    /// [`SecretsError::Manifest`].
    pub fn parse(text: &str) -> Result<Self, SecretsError> {
        let raw: RawManifest = toml::from_str(text).map_err(|e| {
            let line = e.span().map_or(1, |span| {
                let end = span.start.min(text.len());
                1 + text.as_bytes()[..end]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count()
            });
            SecretsError::Manifest(format!(
                ".rustty/secrets.toml: line {line} is not valid: expected only a [secrets] table \
                 of NAME = \"op://...\" entries"
            ))
        })?;
        let mut seen = std::collections::BTreeSet::new();
        for (index, (name, reference)) in raw.secrets.iter().enumerate() {
            if validate_name(name).is_err() {
                return Err(SecretsError::Manifest(format!(
                    ".rustty/secrets.toml: entry {} has an invalid or reserved name",
                    index + 1
                )));
            }
            if !seen.insert(name.to_ascii_uppercase()) {
                return Err(SecretsError::Manifest(format!(
                    ".rustty/secrets.toml: {name} appears twice; Windows variable names ignore case"
                )));
            }
            if validate_reference(reference).is_err() {
                return Err(SecretsError::Manifest(format!(
                    ".rustty/secrets.toml: {name} is not an op:// reference. The manifest holds \
                     references, never values"
                )));
            }
        }
        Ok(Self {
            secrets: raw.secrets,
        })
    }

    /// The file's text: a short header, then the sorted entries.
    ///
    /// # Errors
    /// [`SecretsError::Manifest`] if serialising fails.
    pub fn render(&self) -> Result<String, SecretsError> {
        let raw = RawManifest {
            secrets: self.secrets.clone(),
        };
        let body = toml::to_string(&raw).map_err(|_| {
            SecretsError::Manifest("the manifest could not be serialised".to_owned())
        })?;
        Ok(format!(
            "# RusTTY project secrets (SECRET-001). Names and op:// references only: values \
             never\n# go in this file. Change it with `rustty secrets add`.\n{body}"
        ))
    }

    /// Read the manifest under `root`; `None` when there is none.
    ///
    /// # Errors
    /// [`SecretsError::Manifest`] when it cannot be read or is invalid.
    pub fn load(root: &Path) -> Result<Option<Self>, SecretsError> {
        match std::fs::read_to_string(manifest_path(root)) {
            Ok(text) => Self::parse(&text).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SecretsError::Manifest(format!(
                ".rustty/secrets.toml could not be read: {e}"
            ))),
        }
    }

    /// Write the manifest under `root`, atomically.
    ///
    /// # Errors
    /// [`SecretsError::Manifest`] when it cannot be written.
    pub fn save(&self, root: &Path) -> Result<(), SecretsError> {
        let path = manifest_path(root);
        let text = self.render()?;
        let write = || -> io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temp = path.with_extension("toml.tmp");
            std::fs::write(&temp, text.as_bytes())?;
            std::fs::rename(&temp, &path)
        };
        write().map_err(|e| {
            SecretsError::Manifest(format!(".rustty/secrets.toml could not be written: {e}"))
        })
    }
}

/// FNV-1a, 64 bit. A naming hash only: nothing relies on it being hard to
/// collide, because a same-user agent is past this boundary anyway.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A path as people read it, without the verbatim prefix `canonicalize` adds.
fn readable_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        text.into_owned()
    }
}

/// A path in the one form that gets hashed: no verbatim prefix, backslashes,
/// no trailing separator, lower case (Windows paths ignore case).
fn normalized_path(path: &Path) -> String {
    readable_path(path)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The folder name as `[a-z0-9-]`, safe in a vault name and in `--vault
/// <name>:read_items`. Falls back to `project` when nothing is left.
fn slug_of(normalized: &str) -> String {
    let last = normalized.rsplit('\\').next().unwrap_or_default();
    // A bare drive (`c:`) names no project.
    let last = if last.ends_with(':') { "" } else { last };
    let mut slug = String::new();
    for c in last.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_SLUG_LEN);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "project".to_owned()
    } else {
        slug.to_owned()
    }
}

/// SECRET-003: `rustty-<slug>-<short hash of the canonical project path>`.
#[must_use]
pub fn vault_name(canonical: &Path) -> String {
    let normalized = normalized_path(canonical);
    let hash = fnv1a64(normalized.as_bytes());
    // XOR-folding is FNV's recommended way to shorten the hash.
    let short = (hash ^ (hash >> 32)) & 0xffff_ffff;
    format!("rustty-{}-{short:08x}", slug_of(&normalized))
}

/// For a linked Git worktree, the main checkout it belongs to: its `.git` file
/// reads `gitdir: <main>/.git/worktrees/<name>`. Anything else (a submodule,
/// a bare repository) stays its own project.
fn main_checkout_of(worktree: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let target = text.strip_prefix("gitdir:")?.trim();
    let git_entry = worktree.join(target);
    let worktrees = git_entry.parent()?;
    let dot_git = worktrees.parent()?;
    if worktrees.file_name()? != "worktrees" || dot_git.file_name()? != ".git" {
        return None;
    }
    if !dot_git.is_dir() {
        return None;
    }
    std::fs::canonicalize(dot_git.parent()?).ok()
}

/// The project a directory belongs to: the nearest ancestor holding a manifest
/// or a `.git` entry, or the directory itself. Linked worktrees resolve to the
/// main checkout, so every agent worktree of a repository shares one project,
/// one vault and one token.
///
/// # Errors
/// The directory does not exist or cannot be canonicalised.
pub fn project_root(dir: &Path) -> io::Result<PathBuf> {
    let start = std::fs::canonicalize(dir)?;
    for ancestor in start.ancestors() {
        if manifest_path(ancestor).is_file() || ancestor.join(".git").exists() {
            return Ok(main_checkout_of(ancestor).unwrap_or_else(|| ancestor.to_path_buf()));
        }
    }
    Ok(start)
}

/// A resolved project: where it lives and its vault's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Canonical root folder.
    pub root: PathBuf,
    /// `rustty-<slug>-<hash>`.
    pub vault: String,
}

impl Project {
    /// Resolve `dir`, or the current directory when `None`.
    ///
    /// # Errors
    /// [`SecretsError::NotFound`] when the directory does not exist.
    pub fn resolve(dir: Option<&Path>) -> Result<Self, SecretsError> {
        let start = match dir {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_dir()
                .map_err(|e| SecretsError::NotFound(format!("no current directory: {e}")))?,
        };
        let root = project_root(&start).map_err(|e| {
            SecretsError::NotFound(format!("{} is not usable: {e}", start.display()))
        })?;
        let vault = vault_name(&root);
        Ok(Self { root, vault })
    }
}

/// `op vault get <name>`: does the vault exist?
#[must_use]
pub fn op_vault_get_args(vault: &str) -> Vec<String> {
    strings(&["vault", "get", vault])
}

/// `op vault create <name>`.
#[must_use]
pub fn op_vault_create_args(vault: &str) -> Vec<String> {
    strings(&["vault", "create", vault])
}

/// `op service-account create <name> --vault <vault>:read_items --raw`:
/// read-only on this one vault, and `--raw` returns only the token on stdout.
#[must_use]
pub fn op_service_account_create_args(vault: &str) -> Vec<String> {
    let grant = format!("{vault}:read_items");
    strings(&[
        "service-account",
        "create",
        vault,
        "--vault",
        &grant,
        "--raw",
    ])
}

/// `op read --no-newline <reference>`.
#[must_use]
pub fn op_read_args(reference: &str) -> Vec<String> {
    strings(&["read", "--no-newline", reference])
}

/// `op item create --vault <vault> --format json -`: the item arrives as a
/// JSON template on stdin, never on a command line.
#[must_use]
pub fn op_item_create_args(vault: &str) -> Vec<String> {
    strings(&["item", "create", "--vault", vault, "--format", "json", "-"])
}

/// Owned copies of `items`.
fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// The reference recorded for a value copied in by `secrets add`.
#[must_use]
pub fn stored_reference(vault: &str, name: &str) -> String {
    format!("op://{vault}/{name}/password")
}

/// One `op` invocation. Deliberately not `Debug`: `stdin` can hold a value in
/// the clear.
pub struct OpCall<'a> {
    /// Arguments after `op`. Never holds a value or a token.
    pub args: Vec<String>,
    /// When set, the run authenticates as this service account: the token is
    /// put only in the child's `OP_SERVICE_ACCOUNT_TOKEN`. When `None`, it
    /// runs under the owner's own 1Password session.
    pub token: Option<&'a LockedSecret>,
    /// Bytes for the child's stdin; its stdin is empty when `None`.
    pub stdin: Option<&'a [u8]>,
}

/// What an `op` run produced.
#[derive(Debug)]
pub struct OpOutput {
    /// The process exit code.
    pub exit_code: u32,
    /// Stdout, held as a secret because it may be one.
    pub stdout: LockedSecret,
    /// Stderr, which may be shown to the user after cleaning.
    pub stderr: String,
}

/// Runs `op`. Tests inject a fake; production resolves the real program with
/// no environment or config override (SECRET-010).
pub trait OpRunner {
    /// Run `op` once and wait for it.
    ///
    /// # Errors
    /// `op` could not be found or started, or its output could not be held.
    fn run(&self, call: &OpCall<'_>) -> Result<OpOutput, SecretsError>;
}

/// Where a project's service-account token is kept.
pub trait TokenStore {
    /// A token is stored for `vault`.
    fn exists(&self, vault: &str) -> bool;
    /// The store can take a token. `setup` checks this first because
    /// `op` shows a service-account token only once.
    ///
    /// # Errors
    /// [`SecretsError::Store`].
    fn check_writable(&self) -> Result<(), SecretsError>;
    /// Store the token for `vault`.
    ///
    /// # Errors
    /// [`SecretsError::Store`].
    fn save(&self, vault: &str, token: &LockedSecret) -> Result<(), SecretsError>;
    /// Read the token for `vault`.
    ///
    /// # Errors
    /// [`SecretsError::NotFound`] when absent; [`SecretsError::Store`] when
    /// it cannot be decrypted.
    fn load(&self, vault: &str) -> Result<LockedSecret, SecretsError>;
}

/// Starts the command for `secrets run`.
pub trait Launcher {
    /// Start `argv` with `secrets` as extra environment variables, wipe them,
    /// wait, and return the exit code. Consumes the secrets so they end at
    /// the start of the command, not at its exit.
    ///
    /// # Errors
    /// [`SecretsError::Launch`] when the command cannot be started.
    fn launch(
        &self,
        argv: &[OsString],
        secrets: Vec<(String, LockedSecret)>,
    ) -> Result<u32, SecretsError>;
}

/// SECRET-007: the per-project token store: one DPAPI-encrypted file per
/// project under a directory outside the project (production uses
/// `%LOCALAPPDATA%\rustty\secrets`).
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    /// The folder holding the `<vault>.token` files.
    dir: PathBuf,
}

impl FileTokenStore {
    /// A store rooted at `dir`, created on first write.
    #[must_use]
    pub const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The file for `vault`; `None` for a name that is not a plain file stem,
    /// so no name can reach outside the folder.
    fn path(&self, vault: &str) -> Option<PathBuf> {
        let safe = !vault.is_empty()
            && vault
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        safe.then(|| self.dir.join(format!("{vault}.token")))
    }

    /// The file for `vault`, or a store error for an unusable name.
    fn require_path(&self, vault: &str) -> Result<PathBuf, SecretsError> {
        self.path(vault)
            .ok_or_else(|| SecretsError::Store("that vault name is not usable".to_owned()))
    }
}

impl TokenStore for FileTokenStore {
    fn exists(&self, vault: &str) -> bool {
        self.path(vault).is_some_and(|path| path.is_file())
    }

    fn check_writable(&self) -> Result<(), SecretsError> {
        let probe = self
            .dir
            .join(format!(".write-check-{}", std::process::id()));
        let check = || -> io::Result<()> {
            std::fs::create_dir_all(&self.dir)?;
            std::fs::write(&probe, b"")?;
            std::fs::remove_file(&probe)
        };
        check().map_err(|e| {
            SecretsError::Store(format!(
                "the token store {} is not writable: {e}",
                self.dir.display()
            ))
        })
    }

    fn save(&self, vault: &str, token: &LockedSecret) -> Result<(), SecretsError> {
        let path = self.require_path(vault)?;
        let blob = dpapi_protect(token, vault.as_bytes())
            .map_err(|e| SecretsError::Store(format!("the token could not be encrypted: {e}")))?;
        let write = || -> io::Result<()> {
            std::fs::create_dir_all(&self.dir)?;
            let temp = path.with_extension("token.tmp");
            std::fs::write(&temp, &blob)?;
            std::fs::rename(&temp, &path)
        };
        write().map_err(|e| {
            SecretsError::Store(format!(
                "the token could not be written to {}: {e}",
                self.dir.display()
            ))
        })
    }

    fn load(&self, vault: &str) -> Result<LockedSecret, SecretsError> {
        let path = self.require_path(vault)?;
        let blob = match std::fs::read(&path) {
            Ok(blob) => blob,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(not_set_up(vault));
            }
            Err(e) => {
                return Err(SecretsError::Store(format!(
                    "the token could not be read: {e}"
                )));
            }
        };
        dpapi_unprotect(&blob, vault.as_bytes()).map_err(|e| {
            SecretsError::Store(format!(
                "the token for {vault} could not be decrypted by this Windows user: {e}"
            ))
        })
    }
}

/// The error for a project without a stored token or manifest.
fn not_set_up(vault: &str) -> SecretsError {
    SecretsError::NotFound(format!(
        "this project is not set up (vault {vault}): run `rustty secrets setup`"
    ))
}

/// Remove the line end `op` may add; it is not part of the value.
fn trim_line_end(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    &bytes[..end]
}

/// `secret` (without its line end) occurs in `text`.
fn contains_secret(text: &str, secret: &LockedSecret) -> bool {
    secret
        .with_plaintext(|plain| {
            let plain = trim_line_end(plain);
            !plain.is_empty() && text.as_bytes().windows(plain.len()).any(|w| w == plain)
        })
        // A secret that cannot be checked is treated as present.
        .unwrap_or(true)
}

/// An `op` failure with its stderr made safe to repeat: one line, no control
/// characters, capped, and withheld entirely if it contains a known secret.
fn op_failure(context: &str, output: &OpOutput, secrets: &[&LockedSecret]) -> SecretsError {
    if secrets.iter().any(|s| contains_secret(&output.stderr, s)) {
        return SecretsError::Op(format!(
            "{context} (op exit {}; its error output was withheld because it contained a secret)",
            output.exit_code
        ));
    }
    let cleaned: String = output
        .stderr
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_STDERR_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    SecretsError::Op(if cleaned.is_empty() {
        format!("{context} (op exit {})", output.exit_code)
    } else {
        format!("{context} (op exit {}): {cleaned}", output.exit_code)
    })
}

/// A call under the owner's own session, with no stdin.
const fn owner_call(args: Vec<String>) -> OpCall<'static> {
    OpCall {
        args,
        token: None,
        stdin: None,
    }
}

/// What `setup` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    /// The token already existed, so 1Password was not contacted.
    pub already_set_up: bool,
}

/// SECRET-008: `secrets setup`: create the vault and a read-only service
/// account, capture the token from stdout and store it. The token is never
/// printed, logged or put in the project folder.
///
/// # Errors
/// See [`SecretsError`]; the store and manifest are checked before anything
/// is created, because `op` shows the token only once.
pub fn setup(
    project: &Project,
    op: &dyn OpRunner,
    store: &dyn TokenStore,
) -> Result<SetupOutcome, SecretsError> {
    let vault = &project.vault;
    let manifest = Manifest::load(&project.root)?;
    if !store.exists(vault) {
        store.check_writable()?;
    }
    if manifest.is_none() {
        Manifest::default().save(&project.root)?;
    }
    if store.exists(vault) {
        return Ok(SetupOutcome {
            already_set_up: true,
        });
    }

    // 1Password allows two vaults with one name, and a reference through a
    // duplicated name is ambiguous, so an existing vault is reused.
    let existing = op.run(&owner_call(op_vault_get_args(vault)))?;
    if existing.exit_code != 0 {
        let created = op.run(&owner_call(op_vault_create_args(vault)))?;
        if created.exit_code != 0 {
            return Err(op_failure(
                &format!("vault {vault} could not be created"),
                &created,
                &[],
            ));
        }
    }

    let account = op.run(&owner_call(op_service_account_create_args(vault)))?;
    if account.exit_code != 0 {
        return Err(op_failure(
            &format!("service account {vault} could not be created"),
            &account,
            &[&account.stdout],
        ));
    }
    let token = account
        .stdout
        .with_plaintext(|raw| {
            let token = trim_line_end(raw);
            if token.is_empty() || token.iter().any(u8::is_ascii_whitespace) {
                return None;
            }
            Some(LockedSecret::from_slice(token))
        })
        .map_err(|e| memory_error(&e))?
        .ok_or_else(|| {
            SecretsError::Op(format!(
                "op did not return a usable token for service account {vault}. If the account \
                 was created, delete it in 1Password, then run setup again"
            ))
        })?
        .map_err(|e| memory_error(&e))?;
    store.save(vault, &token).map_err(|e| {
        SecretsError::Store(format!(
            "service account {vault} was created, but its token could not be stored and 1Password \
             will not show it again. Delete the account in 1Password, fix this, then run setup \
             again: {e}"
        ))
    })?;
    Ok(SetupOutcome {
        already_set_up: false,
    })
}

/// The start of the item template, up to where the value goes. `LOGIN` with a
/// `password` field is the concealed-field shape 1Password documents.
fn item_template_prefix(title: &str) -> String {
    format!(
        "{{\"title\":\"{title}\",\"category\":\"LOGIN\",\"fields\":[{{\"id\":\"password\",\
         \"type\":\"CONCEALED\",\"purpose\":\"PASSWORD\",\"label\":\"password\",\"value\":\""
    )
}

/// The end of the item template, after the value.
const ITEM_TEMPLATE_SUFFIX: &str = "\"}]}";

/// The JSON item template for `op item create -`, built directly in locked
/// memory: `serde_json` would leave copies of the value on the heap. `title`
/// is a validated name, which needs no escaping.
fn item_template(title: &str, value: &[u8]) -> io::Result<LockedSecret> {
    /// Hex digits for `\u00XX`.
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let prefix = item_template_prefix(title);
    // Worst case every byte becomes \u00XX.
    let capacity = prefix.len() + value.len() * 6 + ITEM_TEMPLATE_SUFFIX.len();
    LockedSecret::fill(capacity, |buffer| {
        let mut at = 0;
        let mut put = |bytes: &[u8]| {
            buffer[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(prefix.as_bytes());
        for byte in value {
            match byte {
                b'"' => put(b"\\\""),
                b'\\' => put(b"\\\\"),
                0..=0x1f => put(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xf)],
                ]),
                _ => put(&[*byte]),
            }
        }
        put(ITEM_TEMPLATE_SUFFIX.as_bytes());
        at
    })
}

/// What `add` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOutcome {
    /// The new reference in the project's vault.
    pub reference: String,
}

/// SECRET-009: `secrets add`: copy a value into the project's vault under the
/// owner's session and record the reference. The value only ever travels
/// through `op`'s stdout and stdin, and locked memory in between.
///
/// # Errors
/// See [`SecretsError`]; nothing is created unless every check passed and the
/// value was read.
pub fn add(
    project: &Project,
    name: &str,
    from: &str,
    op: &dyn OpRunner,
    store: &dyn TokenStore,
) -> Result<AddOutcome, SecretsError> {
    validate_name(name)?;
    let source = validate_reference(from)?;
    let vault = &project.vault;
    if source.vault.eq_ignore_ascii_case(vault) {
        return Err(SecretsError::Usage(
            "--from already points into this project's vault; there is nothing to copy".to_owned(),
        ));
    }
    let mut manifest = Manifest::load(&project.root)?.ok_or_else(|| not_set_up(vault))?;
    if !store.exists(vault) {
        return Err(not_set_up(vault));
    }
    if manifest
        .secrets
        .keys()
        .any(|existing| existing.eq_ignore_ascii_case(name))
    {
        return Err(SecretsError::Usage(format!(
            "{name} is already in this project's manifest"
        )));
    }

    let read = op.run(&owner_call(op_read_args(from)))?;
    if read.exit_code != 0 {
        return Err(op_failure(
            &format!("the value for {name} could not be read from its source"),
            &read,
            &[&read.stdout],
        ));
    }
    let template = read
        .stdout
        .with_plaintext(|raw| {
            let value = trim_line_end(raw);
            if value.iter().all(u8::is_ascii_whitespace) {
                return Err(SecretsError::Op(format!(
                    "the source for {name} is empty; nothing was copied"
                )));
            }
            if std::str::from_utf8(value).is_err() {
                return Err(SecretsError::Op(format!(
                    "the source for {name} is not text; nothing was copied"
                )));
            }
            item_template(name, value).map_err(|e| memory_error(&e))
        })
        .map_err(|e| memory_error(&e))??;

    let created = template
        .with_plaintext(|json| {
            op.run(&OpCall {
                args: op_item_create_args(vault),
                token: None,
                stdin: Some(json),
            })
        })
        .map_err(|e| memory_error(&e))??;
    if created.exit_code != 0 {
        return Err(op_failure(
            &format!("{name} could not be created in vault {vault}"),
            &created,
            &[&read.stdout, &created.stdout],
        ));
    }

    let reference = stored_reference(vault, name);
    manifest.secrets.insert(name.to_owned(), reference.clone());
    manifest.save(&project.root).map_err(|e| {
        SecretsError::Manifest(format!(
            "{name} was created in vault {vault}, but the manifest could not be updated. Add \
             {name} = \"{reference}\" by hand: {e}"
        ))
    })?;
    Ok(AddOutcome { reference })
}

/// Turn a raw `op read` result into the secret that goes in the environment,
/// or say why it cannot be used. An empty result is never a value.
fn env_value_from(raw: &[u8]) -> Result<io::Result<LockedSecret>, &'static str> {
    let value = trim_line_end(raw);
    if value.iter().all(u8::is_ascii_whitespace) {
        return Err("an empty value");
    }
    let Ok(text) = std::str::from_utf8(value) else {
        return Err("a value that is not text");
    };
    if text.contains('\0') {
        return Err("a value containing a NUL character");
    }
    if text.encode_utf16().count() > MAX_ENV_VALUE_UNITS {
        return Err("a value too long for an environment variable");
    }
    Ok(LockedSecret::from_slice(value))
}

/// SECRET-010: `secrets run`: fetch only the named secrets with the project's
/// service account, then hand them to the launcher. Nothing starts unless
/// every name resolves to a usable value. Returns the command's exit code.
///
/// # Errors
/// See [`SecretsError`]. On any error the command has not been started.
pub fn run(
    project: &Project,
    only: &[String],
    argv: &[OsString],
    op: &dyn OpRunner,
    store: &dyn TokenStore,
    launcher: &dyn Launcher,
) -> Result<u32, SecretsError> {
    if only.is_empty() {
        return Err(SecretsError::Usage(
            "--only is required: there is no default of all secrets".to_owned(),
        ));
    }
    if argv.is_empty() {
        return Err(SecretsError::Usage("no command given after --".to_owned()));
    }
    for name in only {
        validate_name(name)?;
    }
    let vault = &project.vault;
    let manifest = Manifest::load(&project.root)?.ok_or_else(|| not_set_up(vault))?;

    // Everything that can be checked is checked before the first fetch.
    let mut wanted: Vec<(&str, &str)> = Vec::with_capacity(only.len());
    for name in only {
        if wanted.iter().any(|(seen, _)| seen == name) {
            continue;
        }
        let Some(reference) = manifest.secrets.get(name) else {
            return Err(SecretsError::NotFound(format!(
                "{name} is not in this project's manifest; see `rustty secrets list`"
            )));
        };
        let parts = validate_reference(reference)
            .map_err(|_| SecretsError::Manifest(format!("{name} has an invalid reference")))?;
        if !parts.vault.eq_ignore_ascii_case(vault) {
            return Err(SecretsError::Manifest(format!(
                "{name} points outside this project's vault {vault}"
            )));
        }
        wanted.push((name, reference));
    }

    let token = store.load(vault)?;
    let mut secrets = Vec::with_capacity(wanted.len());
    for (name, reference) in wanted {
        let output = op.run(&OpCall {
            args: op_read_args(reference),
            token: Some(&token),
            stdin: None,
        })?;
        if output.exit_code != 0 {
            return Err(op_failure(
                &format!("{name} could not be read; the command was not started"),
                &output,
                &[&token, &output.stdout],
            ));
        }
        let value = output
            .stdout
            .with_plaintext(env_value_from)
            .map_err(|e| memory_error(&e))?
            .map_err(|problem| {
                SecretsError::Op(format!(
                    "op returned {problem} for {name}; the command was not started"
                ))
            })?
            .map_err(|e| memory_error(&e))?;
        secrets.push((name.to_owned(), value));
    }
    // The token is not needed from here on: wipe it before the command starts.
    drop(token);
    launcher.launch(argv, secrets)
}

/// A parsed `rustty secrets` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsVerb {
    /// `secrets --help`.
    Help,
    /// `secrets setup [--dir D] [--json]`.
    Setup {
        /// The project directory; the current directory when `None`.
        dir: Option<PathBuf>,
        /// Print JSON.
        json: bool,
    },
    /// `secrets add NAME --from op://... [--dir D] [--json]`.
    Add {
        /// The new secret's name.
        name: String,
        /// Where the value is copied from.
        from: String,
        /// The project directory; the current directory when `None`.
        dir: Option<PathBuf>,
        /// Print JSON.
        json: bool,
    },
    /// `secrets list [--dir D] [--json]`.
    List {
        /// The project directory; the current directory when `None`.
        dir: Option<PathBuf>,
        /// Print JSON.
        json: bool,
    },
    /// `secrets run --only A,B [--dir D] -- cmd args...`.
    Run {
        /// The names to fetch. Required: there is no default of all.
        only: Vec<String>,
        /// The project directory; the current directory when `None`.
        dir: Option<PathBuf>,
        /// The command and its arguments, verbatim.
        argv: Vec<OsString>,
    },
}

/// Usage text for the `secrets` verbs.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: rustty secrets setup [--dir <dir>] [--json]\n\
     \x20      rustty secrets add <NAME> --from op://vault/item/field [--dir <dir>] [--json]\n\
     \x20      rustty secrets list [--dir <dir>] [--json]\n\
     \x20      rustty secrets run --only <NAME,NAME> [--dir <dir>] -- <command> [args...]\n\
     exit codes: 2 usage, 3 not found, 8 manifest, 9 op, 10 token store, 11 secure memory, \
     12 command did not start. `run` otherwise returns the command's own exit code."
}

/// The flags shared by `setup`, `add` and `list`.
#[derive(Default)]
struct CommonArgs {
    /// `--dir`.
    dir: Option<PathBuf>,
    /// `--json`.
    json: bool,
    /// `--from`.
    from: Option<String>,
    /// Arguments that are not flags.
    positional: Vec<String>,
}

/// Parse `--dir`, `--json`, `--from` and positionals. Errors are the fixed
/// usage text: a stray argument could be a pasted secret, so none is echoed.
fn common_args(args: impl Iterator<Item = OsString>) -> Result<CommonArgs, String> {
    let bad = || usage().to_owned();
    let mut parsed = CommonArgs::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--dir") => parsed.dir = Some(PathBuf::from(args.next().ok_or_else(bad)?)),
            Some("--json") => parsed.json = true,
            Some("--from") => {
                let from = args.next().ok_or_else(bad)?;
                parsed.from = Some(from.to_str().ok_or_else(bad)?.to_owned());
            }
            Some(other) if !other.starts_with('-') => parsed.positional.push(other.to_owned()),
            _ => return Err(bad()),
        }
    }
    Ok(parsed)
}

/// Parse the arguments after `rustty secrets`.
///
/// # Errors
/// The usage text. It never echoes what was typed.
pub fn parse_verb<I: IntoIterator<Item = OsString>>(args: I) -> Result<SecretsVerb, String> {
    let bad = || usage().to_owned();
    let mut args = args.into_iter();
    let first = args.next().ok_or_else(bad)?;
    match first.to_str() {
        Some("-h" | "--help" | "help") => Ok(SecretsVerb::Help),
        Some("setup") => {
            let parsed = common_args(args)?;
            if !parsed.positional.is_empty() || parsed.from.is_some() {
                return Err(bad());
            }
            Ok(SecretsVerb::Setup {
                dir: parsed.dir,
                json: parsed.json,
            })
        }
        Some("list") => {
            let parsed = common_args(args)?;
            if !parsed.positional.is_empty() || parsed.from.is_some() {
                return Err(bad());
            }
            Ok(SecretsVerb::List {
                dir: parsed.dir,
                json: parsed.json,
            })
        }
        Some("add") => {
            let mut parsed = common_args(args)?;
            if parsed.positional.len() != 1 {
                return Err(bad());
            }
            let name = parsed.positional.remove(0);
            let from = parsed.from.ok_or_else(bad)?;
            Ok(SecretsVerb::Add {
                name,
                from,
                dir: parsed.dir,
                json: parsed.json,
            })
        }
        Some("run") => parse_run(args),
        _ => Err(bad()),
    }
}

/// Parse `secrets run`: flags up to `--`, then the command verbatim.
fn parse_run(args: impl Iterator<Item = OsString>) -> Result<SecretsVerb, String> {
    let bad = || usage().to_owned();
    let mut args = args;
    let mut only = Vec::new();
    let mut dir = None;
    let mut separated = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--") => {
                separated = true;
                break;
            }
            Some("--only") => {
                let value = args.next().ok_or_else(bad)?;
                for name in value.to_str().ok_or_else(bad)?.split(',') {
                    if name.is_empty() {
                        return Err(bad());
                    }
                    only.push(name.to_owned());
                }
            }
            Some("--dir") => dir = Some(PathBuf::from(args.next().ok_or_else(bad)?)),
            _ => return Err(bad()),
        }
    }
    let argv: Vec<OsString> = args.collect();
    if !separated || only.is_empty() || argv.is_empty() {
        return Err(bad());
    }
    Ok(SecretsVerb::Run { only, dir, argv })
}

/// The collaborators a verb needs, injected so tests can use fakes.
pub struct Deps<'a> {
    /// Runs `op`.
    pub op: &'a dyn OpRunner,
    /// Holds tokens.
    pub store: &'a dyn TokenStore,
    /// Starts the command for `run`.
    pub launcher: &'a dyn Launcher,
}

/// The `secrets list` output.
fn print_list(
    project: &Project,
    store: &dyn TokenStore,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), SecretsError> {
    let manifest = Manifest::load(&project.root)?.unwrap_or_default();
    let set_up = store.exists(&project.vault);
    if json {
        let secrets: Vec<serde_json::Value> = manifest
            .secrets
            .iter()
            .map(|(name, reference)| serde_json::json!({"name": name, "reference": reference}))
            .collect();
        let value = serde_json::json!({
            "schema": "secrets-list/v1",
            "project": readable_path(&project.root),
            "vault": project.vault,
            "setUp": set_up,
            "secrets": secrets,
        });
        let _ = writeln!(out, "{value}");
        return Ok(());
    }
    let state = if set_up {
        "set up"
    } else {
        "not set up: run `rustty secrets setup`"
    };
    let _ = writeln!(out, "vault {} ({state})", project.vault);
    if manifest.secrets.is_empty() {
        let _ = writeln!(out, "no secrets");
    }
    for (name, reference) in &manifest.secrets {
        let _ = writeln!(out, "{name:<24} {reference}");
    }
    Ok(())
}

/// Run one verb and return the exit code. Results go to `out`; failures go to
/// `err` as one `rustty: ...` line.
pub fn execute(
    verb: &SecretsVerb,
    deps: &Deps<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u32 {
    match execute_inner(verb, deps, out) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(err, "rustty: {e}");
            e.exit_code()
        }
    }
}

/// [`execute`] with errors still as values.
fn execute_inner(
    verb: &SecretsVerb,
    deps: &Deps<'_>,
    out: &mut dyn Write,
) -> Result<u32, SecretsError> {
    match verb {
        SecretsVerb::Help => {
            let _ = writeln!(out, "{}", usage());
            Ok(0)
        }
        SecretsVerb::Setup { dir, json } => {
            let project = Project::resolve(dir.as_deref())?;
            let outcome = setup(&project, deps.op, deps.store)?;
            if *json {
                let value = serde_json::json!({
                    "schema": "secrets-setup/v1",
                    "project": readable_path(&project.root),
                    "vault": project.vault,
                    "serviceAccount": project.vault,
                    "alreadySetUp": outcome.already_set_up,
                });
                let _ = writeln!(out, "{value}");
            } else if outcome.already_set_up {
                let _ = writeln!(out, "vault {} is already set up", project.vault);
            } else {
                let _ = writeln!(
                    out,
                    "vault {0} is ready; service account {0} can read it, and its token is \
                     stored for this Windows user",
                    project.vault
                );
            }
            Ok(0)
        }
        SecretsVerb::Add {
            name,
            from,
            dir,
            json,
        } => {
            let project = Project::resolve(dir.as_deref())?;
            let outcome = add(&project, name, from, deps.op, deps.store)?;
            if *json {
                let value = serde_json::json!({
                    "schema": "secrets-add/v1",
                    "name": name,
                    "reference": outcome.reference,
                });
                let _ = writeln!(out, "{value}");
            } else {
                let _ = writeln!(out, "added {name} as {}", outcome.reference);
            }
            Ok(0)
        }
        SecretsVerb::List { dir, json } => {
            let project = Project::resolve(dir.as_deref())?;
            print_list(&project, deps.store, *json, out)?;
            Ok(0)
        }
        SecretsVerb::Run { only, dir, argv } => {
            let project = Project::resolve(dir.as_deref())?;
            run(&project, only, argv, deps.op, deps.store, deps.launcher)
        }
    }
}

/// Run one verb with the production collaborators.
#[cfg(windows)]
fn run_production(verb: &SecretsVerb, out: &mut dyn Write, err: &mut dyn Write) -> u32 {
    use crate::secret_exec::{SystemLauncher, SystemOp, token_store_dir};

    let dir = match token_store_dir() {
        Ok(dir) => dir,
        Err(e) => {
            let _ = writeln!(err, "rustty: the token store folder was not found: {e}");
            return u32::from(EXIT_STORE);
        }
    };
    let store = FileTokenStore::new(dir);
    let deps = Deps {
        op: &SystemOp,
        store: &store,
        launcher: &SystemLauncher,
    };
    execute(verb, &deps, out, err)
}

/// Off Windows only the help works: there is no secure memory to use.
#[cfg(not(windows))]
fn run_production(verb: &SecretsVerb, out: &mut dyn Write, err: &mut dyn Write) -> u32 {
    if *verb == SecretsVerb::Help {
        let _ = writeln!(out, "{}", usage());
        return 0;
    }
    let _ = writeln!(err, "rustty: secrets are only implemented on Windows");
    u32::from(EXIT_MEMORY)
}

/// The `rustty secrets` entry point: a parsed command line in, the process
/// exit code out. Windows exit codes are 32 bits, so `run` can return any
/// code its command returned.
#[must_use]
pub fn run_cli(parsed: &Result<SecretsVerb, String>) -> i32 {
    let (mut out, mut err) = (io::stdout(), io::stderr());
    let code = match parsed {
        Ok(verb) => run_production(verb, &mut out, &mut err),
        Err(usage_text) => {
            let _ = writeln!(err, "{usage_text}");
            u32::from(EXIT_USAGE)
        }
    };
    i32::from_ne_bytes(code.to_ne_bytes())
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
