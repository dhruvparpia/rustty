//! Minimal command-line parsing (no dependency): `--profile <name>` picks
//! the starting profile, `--theme <name>` overrides the configured theme,
//! and the Jump List tasks (CONFIG-013, THEME-009) launch with these flags.

use std::ffi::OsString;

/// Parsed command line. Unknown arguments are reported and ignored; the
/// app is a GUI, a stray flag must not stop a terminal from opening.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    /// `--profile <name>`: shell profile for the first tab.
    pub profile: Option<String>,
    /// `--theme <name>`: theme preset or `themes/<name>.toml`.
    pub theme: Option<String>,
    /// `--workspace <name>`: open this workspace template (WORKSPACE-004).
    pub workspace: Option<String>,
    /// `--workspaces`: print the template names and exit.
    pub workspaces: bool,
    /// First positional argument: directory to open the terminal in
    /// (issue #40, "Open terminal here" from Explorer).
    pub directory: Option<String>,
    /// `-h`/`--help` was given.
    pub help: bool,
    /// `--version` was given.
    pub version: bool,
    /// `--list-sessions`: ask the session host for its sessions (#46).
    pub list_sessions: bool,
    /// `--new-session`: ask the session host to create one (#46).
    pub new_session: bool,
    /// `--session-info <n>`: inspect one host session (#46).
    pub session_info: Option<u32>,
    /// `--terminate-session <n>`: terminate one host session (#46).
    pub terminate_session: Option<u32>,
    /// `--attach <n>`: console attach, the #47 entry point. Parsed here
    /// so it can never fall through to opening a GUI window.
    pub attach: Option<u32>,
    /// `--force`: with `--terminate-session`, skip the graceful attempt.
    pub force: bool,
    /// `--detach`: with `--new-session`, create without a view (#46).
    pub detach: bool,
    /// `--ephemeral`: with `--new-session`, end the session when its
    /// last client detaches (ADR §3 lifecycle).
    pub ephemeral: bool,
    /// `--name <name>`: display name for `--new-session` (label only).
    pub session_name: Option<String>,
    /// `--cols <n>` / `--rows <n>`: geometry for `--new-session`.
    pub session_cols: Option<u32>,
    /// `--rows <n>`: geometry for `--new-session`.
    pub session_rows: Option<u32>,
    /// `--json`: structured output for the session verbs (#46).
    pub json: bool,
    /// `--read-only`: with `--attach`, watch without sending (#47).
    pub read_only: bool,
    /// `--take-control`: with `--attach`, demote the current controller (#47).
    pub take_control: bool,
    /// Arguments this version does not know.
    pub unknown: Vec<String>,
}

/// Parse `args` (argv without the program name). A flag without its value
/// (last argument) reads as absent.
#[must_use]
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();
        match arg.as_str() {
            "--profile" => cli.profile = args.next().map(|v| v.to_string_lossy().into_owned()),
            "--theme" => cli.theme = args.next().map(|v| v.to_string_lossy().into_owned()),
            "--workspace" => {
                cli.workspace = args.next().map(|v| v.to_string_lossy().into_owned());
            }
            "--workspaces" => cli.workspaces = true,
            "--list-sessions" => cli.list_sessions = true,
            "--new-session" => cli.new_session = true,
            "--session-info" => cli.session_info = alias(&mut args),
            "--terminate-session" => cli.terminate_session = alias(&mut args),
            "--attach" => cli.attach = alias(&mut args),
            "--force" => cli.force = true,
            "--detach" => cli.detach = true,
            "--ephemeral" => cli.ephemeral = true,
            "--cwd" => cli.directory = args.next().map(|v| v.to_string_lossy().into_owned()),
            "--name" => cli.session_name = args.next().map(|v| v.to_string_lossy().into_owned()),
            "--cols" => cli.session_cols = numeric(&args.next()),
            "--rows" => cli.session_rows = numeric(&args.next()),
            "--json" => cli.json = true,
            "--read-only" => cli.read_only = true,
            "--take-control" => cli.take_control = true,
            "-h" | "--help" => cli.help = true,
            "--version" => cli.version = true,
            // First positional is the working directory (Open terminal
            // here); any later positional stays unknown.
            _ if cli.directory.is_none() && !arg.starts_with('-') => {
                cli.directory = Some(arg);
            }
            _ => cli.unknown.push(arg),
        }
    }
    cli
}

/// A positive integer argument; a missing or malformed value reads as
/// absent (the session verbs then answer "usage").
fn numeric(v: &Option<OsString>) -> Option<u32> {
    v.as_ref()
        .and_then(|v| v.to_string_lossy().parse::<u32>().ok())
}

/// Alias for a session verb: the flag is present, so this is always
/// `Some`. A missing, non-positive, or malformed value reads as 0, the
/// verbs' "usage" sentinel — the contract is that a malformed verb
/// fails visibly and never falls through to opening a GUI window
/// (docs/design-session-attach.md).
fn alias<I: Iterator<Item = OsString>>(args: &mut I) -> Option<u32> {
    let value = args.next();
    let parsed = value
        .as_ref()
        .and_then(|v| v.to_string_lossy().parse::<u32>().ok())
        .filter(|n| *n > 0);
    Some(parsed.unwrap_or(0))
}

/// One-line usage for `--help`.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: rusttyterm [directory] [--profile <name>] [--theme <name>] [--workspace <name> | --workspaces] [--version] [-h|--help]\n\
       session host: rusttyterm --list-sessions [--json] | --new-session [--profile <name>] [--name <label>] [--cwd <dir>] [--cols <n>] [--rows <n>] [--detach] [--ephemeral] | --session-info <n> [--json] | --terminate-session <n> [--force] | --attach <n> [--read-only | --take-control]"
}

/// Attach to the calling console so a GUI-subsystem exe can print to
/// where it was launched (a plain `AttachConsole` replaces redirected
/// standard handles with the console handle, which loses `> file` and
/// piped output; the redirected handles are restored after the attach).
#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
pub fn attach_parent_console() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, CONSOLE_MODE, GetConsoleMode, GetStdHandle,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // Safety: FFI boundary (NFR-401); all calls act on this process's
    // own standard handles.
    let out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap_or(HANDLE::default());
    let err = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.unwrap_or(HANDLE::default());
    // A redirected handle (file/pipe) is not a console: it fails
    // GetConsoleMode. Those must survive the attach. (The out-param
    // must be a real address: a null one access-violates in conhost.)
    let mut m = CONSOLE_MODE(0);
    let out_redir =
        !out.is_invalid() && unsafe { GetConsoleMode(out, core::ptr::from_mut(&mut m)) }.is_err();
    let err_redir =
        !err.is_invalid() && unsafe { GetConsoleMode(err, core::ptr::from_mut(&mut m)) }.is_err();
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok() {
        if out_redir {
            unsafe { SetStdHandle(STD_OUTPUT_HANDLE, out) }.ok();
        }
        if err_redir {
            unsafe { SetStdHandle(STD_ERROR_HANDLE, err) }.ok();
        }
    }
}

/// Detach this process from its console: a GUI launch under the console
/// subsystem gets a window the subsystem allocates at startup; closing
/// the console hides it (writes to the freed handles fail harmlessly).
#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI boundary (NFR-401)")]
pub fn free_console() {
    use windows::Win32::System::Console::FreeConsole;

    // Safety: FFI boundary (NFR-401); detaches this process from its console.
    unsafe { FreeConsole() }.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_profile_and_theme() {
        let cli = parse(os(&["--profile", "pwsh", "--theme", "gruvbox-dark"]));
        assert_eq!(cli.profile.as_deref(), Some("pwsh"));
        assert_eq!(cli.theme.as_deref(), Some("gruvbox-dark"));
        assert!(cli.unknown.is_empty());
    }

    #[test]
    fn unknown_arguments_are_collected_not_fatal() {
        let cli = parse(os(&["--nope", "x"]));
        // The first positional is the directory; the unknown flag stands alone.
        assert_eq!(cli.unknown, vec!["--nope"]);
        assert_eq!(cli.directory.as_deref(), Some("x"));
    }

    #[test]
    fn first_positional_is_the_open_directory() {
        let cli = parse(os(&[r"C:\My Docs", "--profile", "pwsh"]));
        assert_eq!(cli.directory.as_deref(), Some(r"C:\My Docs"));
        assert_eq!(cli.profile.as_deref(), Some("pwsh"));
        assert!(cli.unknown.is_empty());
        // A second positional is not a directory.
        let cli = parse(os(&["C:\\a", "C:\\b"]));
        assert_eq!(cli.directory.as_deref(), Some("C:\\a"));
        assert_eq!(cli.unknown, vec!["C:\\b"]);
    }

    #[test]
    fn flag_without_value_reads_as_absent() {
        let cli = parse(os(&["--profile"]));
        assert_eq!(cli.profile, None);
        assert!(cli.unknown.is_empty());
    }

    #[test]
    fn workspace_flags() {
        let cli = parse(os(&["--workspace", "Web App"]));
        assert_eq!(cli.workspace.as_deref(), Some("Web App"));
        assert!(!cli.workspaces);
        assert!(parse(os(&["--workspaces"])).workspaces);
        assert_eq!(parse(os(&["--workspace"])).workspace, None);
    }

    #[test]
    fn help_and_version() {
        assert!(parse(os(&["-h"])).help);
        assert!(parse(os(&["--help"])).help);
        assert!(parse(os(&["--version"])).version);
    }

    #[test]
    fn session_verbs() {
        let cli = parse(os(&[
            "--new-session",
            "--profile",
            "PowerShell",
            "--name",
            "build",
            "--cols",
            "120",
            "--detach",
            "--json",
        ]));
        assert!(cli.new_session);
        assert_eq!(cli.profile.as_deref(), Some("PowerShell"));
        assert_eq!(cli.session_name.as_deref(), Some("build"));
        assert_eq!(cli.session_cols, Some(120));
        assert!(cli.detach);
        assert!(cli.json);
        assert_eq!(parse(os(&["--session-info", "3"])).session_info, Some(3));
        let cli = parse(os(&["--terminate-session", "3", "--force"]));
        assert_eq!(cli.terminate_session, Some(3));
        assert!(cli.force);
        // A malformed alias is the 0 sentinel, so the verb answers "usage"
        // and never falls through to opening a window.
        assert_eq!(parse(os(&["--session-info", "x"])).session_info, Some(0));
        assert_eq!(parse(os(&["--session-info"])).session_info, Some(0));
        assert_eq!(parse(os(&["--attach", "4"])).attach, Some(4));
        assert_eq!(parse(os(&["--attach"])).attach, Some(0));
    }
}
